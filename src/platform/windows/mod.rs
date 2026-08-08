// Native Win32 bindings are being migrated file-by-file to documented RAII
// wrappers. Keep the crate-wide deny active everywhere outside this explicit
// transitional boundary.
#![allow(clippy::undocumented_unsafe_blocks)]

//! The Windows backend.
//!
//! Owns the engine-thread message loop, native overlay/tray windows and the
//! bounded channels used by the dedicated hook, UIA and DWM workers. All
//! thread-affine window work stays on the thread that calls [`Backend::poll`].

mod accessibility;
mod autostart;
mod console_control;
mod frame_clock;
mod gpu_overlay;
mod hook;
mod input;
mod native;
mod overlay;
mod overlay_worker;
mod screens;
mod status_item;
mod system_events;

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

use crate::api::Autostart;
use crate::api::backend::{Appearance, Backend, BackendEvent, KeyDisposition};
use crate::api::command::{ButtonAction, FocusedApp, MouseButton};
use crate::api::geometry::{Point, Rect, Screen};
use crate::api::input::{Key, KeyState};
use crate::api::overlay::OverlayScene;

use self::overlay_worker::OverlayWorker;

const WAKE_MESSAGE: u32 = WM_APP + 0x4C;

pub(crate) use native::{atomic_replace, prepare_console_for_cli};

/// Routes worker and tray events into the engine queue and wakes its native
/// message wait immediately. The queue remains empty while idle.
#[derive(Clone)]
pub(super) struct EventSender {
    sender: Sender<BackendEvent>,
    wake_thread: u32,
}

impl EventSender {
    fn new(sender: Sender<BackendEvent>, wake_thread: u32) -> Self {
        Self {
            sender,
            wake_thread,
        }
    }

    #[cfg(test)]
    fn without_wake(sender: Sender<BackendEvent>) -> Self {
        Self::new(sender, 0)
    }

    fn send(&self, event: BackendEvent) -> Result<(), ()> {
        self.sender.send(event).map_err(|_| ())?;
        if self.wake_thread != 0
            && let Err(error) = native::post_thread_wake(self.wake_thread, WAKE_MESSAGE)
        {
            crate::log_warning!(
                "windows-events",
                "cannot wake engine for asynchronous event: {error}"
            );
        }
        Ok(())
    }
}

pub struct WindowsBackend {
    hook: Option<hook::HookThread>,
    overlay: OverlayWorker,
    /// Events produced off-thread (scan results).
    async_rx: Receiver<BackendEvent>,
    event_tx: EventSender,
    pending: VecDeque<BackendEvent>,
    screens: Vec<Screen>,
    /// Foreground window at the last check, to detect focus changes.
    last_foreground: HWND,
    last_appearance: Appearance,
    frame_clock: frame_clock::DisplayFrameClock,
    foreground_watcher: Option<system_events::ForegroundWatcher>,
    status_item: Option<status_item::StatusItem>,
    console_control: Option<console_control::ConsoleControl>,
    ui_automation: Option<accessibility::UiAutomationWorker>,
    held_buttons: Cell<u8>,
}

impl WindowsBackend {
    pub fn new() -> Result<Self, String> {
        // Must happen before any window exists, or coordinates will be wrong
        // on scaled displays.
        screens::enable_dpi_awareness();
        // Must precede creation of the DXGI overlay and the first VBlank wait.
        // On Windows 11 this exposes real dynamic-refresh cadence; older
        // systems keep DXGI's compatible virtualized cadence.
        frame_clock::prefer_dynamic_vblank();
        if let Err(error) = native::prefer_input_latency() {
            crate::log_warning!(
                "windows-input",
                "cannot raise engine thread priority: {error}"
            );
        }

        let owner_thread = native::prepare_thread_message_queue();
        let (async_tx, async_rx) = mpsc::channel();
        let event_tx = EventSender::new(async_tx, owner_thread);
        let mut pending = VecDeque::new();
        let status_item = match status_item::StatusItem::new(event_tx.clone()) {
            Ok(item) => Some(item),
            Err(error) => {
                pending.push_back(BackendEvent::Warning(format!(
                    "Windows tray controls are unavailable: {error}"
                )));
                None
            }
        };
        let foreground_watcher = match system_events::ForegroundWatcher::new(owner_thread) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                pending.push_back(BackendEvent::Warning(error));
                None
            }
        };
        let console_control = match console_control::ConsoleControl::new(owner_thread) {
            Ok(control) => Some(control),
            Err(error) => {
                pending.push_back(BackendEvent::Warning(error));
                None
            }
        };
        let appearance = system_appearance();
        let initial_screens = screens::list_screens().unwrap_or_else(|error| {
            crate::app::logging::report_error(
                "windows-screen",
                format!("initial display enumeration failed: {error}"),
            );
            Vec::new()
        });
        let overlay = OverlayWorker::start(event_tx.clone())?;
        Ok(Self {
            hook: None,
            overlay,
            async_rx,
            event_tx,
            pending,
            screens: initial_screens,
            last_foreground: native::foreground_window(),
            last_appearance: appearance,
            frame_clock: frame_clock::DisplayFrameClock::new(owner_thread),
            foreground_watcher,
            status_item,
            console_control,
            ui_automation: None,
            held_buttons: Cell::new(0),
        })
    }

    /// Pump the message queue so hook callbacks and window messages run.
    #[inline(always)]
    fn pump_messages(&mut self) -> bool {
        native::pump_window_messages()
    }

    fn detect_focus_change(&mut self) -> Option<BackendEvent> {
        let current = native::foreground_window();
        if current == self.last_foreground {
            return None;
        }
        if !current.is_invalid() {
            let mut process_id = 0;
            native::window_thread_process_id(current, Some(&mut process_id));
            if process_id == native::current_process_id() {
                // Tray and overlay windows are implementation details, not an
                // application-context change. Preserve the previous foreground
                // handle so restoring it produces no spurious transition.
                return None;
            }
        }
        self.last_foreground = current;
        if current.is_invalid() {
            return Some(BackendEvent::FocusChanged(None));
        }

        // The low-level hook is desktop-global, so a foreground transition
        // does not end any physical key lifecycle. Preserve its pressed set:
        // clearing it here would turn the next auto-repeat into a fresh press.
        Some(BackendEvent::FocusChanged(Some(focused_app_for(
            current,
            native::window_title(current),
        ))))
    }

    fn detect_screen_change(&mut self) -> Option<BackendEvent> {
        let current = match screens::list_screens() {
            Ok(screens) => screens,
            Err(error) => {
                crate::app::logging::report_error(
                    "windows-screen",
                    format!("display re-enumeration failed: {error}"),
                );
                return None;
            }
        };
        if current == self.screens || current.is_empty() {
            return None;
        }
        self.screens = current.clone();
        Some(BackendEvent::ScreensChanged(current))
    }

    fn next_hook_event(&mut self) -> Result<Option<BackendEvent>, String> {
        Ok(self.hook.as_mut().and_then(hook::HookThread::next_event))
    }

    fn try_event(&mut self) -> Result<Option<BackendEvent>, String> {
        // Synchronous hook callbacks are blocked waiting for disposition, so
        // physical input must outrank scans, tray events and frame clocks.
        if let Some(event) = self.next_hook_event()? {
            return Ok(Some(event));
        }
        if let Some(event) = self.pending.pop_front() {
            return Ok(Some(event));
        }
        if let Ok(event) = self.async_rx.try_recv() {
            return Ok(Some(event));
        }
        if let Some(elapsed) = self.frame_clock.try_next() {
            return Ok(Some(BackendEvent::Frame(elapsed)));
        }
        Ok(self.take_native_change())
    }

    /// Region an overlay must cover: the union of every display.
    fn virtual_bounds(&self) -> Rect {
        Screen::virtual_bounds(&self.screens)
    }

    fn release_held_buttons(&self) -> Result<(), String> {
        let mut first_error = None;
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::X1,
            MouseButton::X2,
        ] {
            let bit = input::button_mask(button);
            if self.held_buttons.get() & bit == 0 {
                continue;
            }
            match input::mouse_button(button, ButtonAction::Release) {
                Ok(()) => self.held_buttons.set(self.held_buttons.get() & !bit),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Stop every native resource owned by this backend.
    ///
    /// Every step is independently idempotent: normal engine shutdown, a
    /// partially-started backend, and `Drop` can all use this path safely.
    /// In particular, stopping the UI Automation worker lets it cancel its
    /// active provider call, drop its COM interfaces, and call
    /// `CoUninitialize` on the worker thread before it is joined.
    fn shutdown_resources(&mut self) -> Result<(), String> {
        if let Some(mut worker) = self.ui_automation.take() {
            worker.stop();
        }
        let release_result = self.release_held_buttons();
        self.frame_clock.stop();
        if let Some(mut hook) = self.hook.take() {
            hook.stop();
        }
        if let Some(mut status_item) = self.status_item.take() {
            status_item.stop();
        }
        self.foreground_watcher.take();
        let dismiss_result = self.overlay.dismiss();
        if let Some(control) = self.console_control.as_ref() {
            control.mark_shutdown_complete();
        }
        self.console_control.take();
        release_result.and(dismiss_result)
    }

    fn take_native_change(&mut self) -> Option<BackendEvent> {
        let focus_changed = self
            .foreground_watcher
            .as_ref()
            .is_some_and(system_events::ForegroundWatcher::take_changed);
        if focus_changed && let Some(event) = self.detect_focus_change() {
            return Some(event);
        }
        if self
            .status_item
            .as_ref()
            .is_some_and(status_item::StatusItem::take_display_changed)
            && let Some(event) = self.detect_screen_change()
        {
            return Some(event);
        }
        if self
            .status_item
            .as_ref()
            .is_some_and(status_item::StatusItem::take_appearance_changed)
        {
            let appearance = system_appearance();
            if appearance != self.last_appearance {
                self.last_appearance = appearance;
                return Some(BackendEvent::AppearanceChanged(appearance));
            }
        }
        None
    }
}

impl Drop for WindowsBackend {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown_resources() {
            crate::app::logging::report_error(
                "windows-backend",
                format!("cannot clean up native resources during drop: {error}"),
            );
        }
    }
}

impl Backend for WindowsBackend {
    fn start(&mut self) -> Result<(), String> {
        self.hook = Some(hook::HookThread::start()?);
        Ok(())
    }

    fn poll(&mut self, timeout: Duration) -> Result<Option<BackendEvent>, String> {
        if let Some(event) = self.try_event()? {
            return Ok(Some(event));
        }

        if !self.pump_messages() {
            return Ok(Some(BackendEvent::Quit));
        }
        if let Some(event) = self.try_event()? {
            return Ok(Some(event));
        }

        // Sleep until input arrives or the timeout expires, so the engine's
        // timers stay punctual without spinning.
        let millis = timeout.as_millis().min(u32::MAX as u128) as u32;
        native::wait_for_input(millis);

        if !self.pump_messages() {
            return Ok(Some(BackendEvent::Quit));
        }
        if let Some(event) = self.try_event()? {
            return Ok(Some(event));
        }
        // The native event hooks are preferred. These fallbacks cover remote
        // sessions and stripped-down shells that decline a notification hook.
        if self.foreground_watcher.is_none()
            && let Some(event) = self.detect_focus_change()
        {
            return Ok(Some(event));
        }
        if self.status_item.is_none() {
            if let Some(event) = self.detect_screen_change() {
                return Ok(Some(event));
            }
            let appearance = system_appearance();
            if appearance != self.last_appearance {
                self.last_appearance = appearance;
                return Ok(Some(BackendEvent::AppearanceChanged(appearance)));
            }
        }
        Ok(None)
    }

    fn dispose_key(&mut self, disposition: KeyDisposition) -> Result<(), String> {
        self.hook
            .as_mut()
            .ok_or_else(|| "keyboard hook is not running".to_string())?
            .set_disposition(disposition)
    }

    fn screens(&self) -> Result<Vec<Screen>, String> {
        screens::list_screens()
    }

    fn pointer(&self) -> Result<Point, String> {
        input::cursor_position()
    }

    fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
        let hwnd = native::foreground_window();
        if hwnd.is_invalid() {
            return Ok(None);
        }
        Ok(Some(focused_app_for(hwnd, native::window_title(hwnd))))
    }

    fn warp_pointer(&self, to: Point) -> Result<(), String> {
        input::warp_cursor(to)?;
        self.frame_clock.retarget(to.x, to.y);
        Ok(())
    }

    fn move_pointer(&self, from: Point, dx: f64, dy: f64) -> Result<(), String> {
        input::move_cursor_relative(from, dx, dy)?;
        self.frame_clock.retarget(from.x + dx, from.y + dy);
        Ok(())
    }

    fn mouse_button(&self, button: MouseButton, action: ButtonAction) -> Result<(), String> {
        input::mouse_button(button, action)?;
        let bit = input::button_mask(button);
        match action {
            ButtonAction::Press => self.held_buttons.set(self.held_buttons.get() | bit),
            ButtonAction::Release => self.held_buttons.set(self.held_buttons.get() & !bit),
            ButtonAction::Click | ButtonAction::DoubleClick => {}
        }
        Ok(())
    }

    fn scroll(&self, dx: f64, dy: f64) -> Result<(), String> {
        input::scroll(dx, dy)
    }

    fn send_key(&self, key: &Key, state: KeyState) -> Result<(), String> {
        input::send_key(key, state)
    }

    fn send_keys(&self, events: &[(Key, KeyState)]) -> Result<(), String> {
        input::send_keys(events)
    }

    fn set_frame_clock(&mut self, active: bool) -> Result<(), String> {
        if active {
            if let Ok(pointer) = input::cursor_position() {
                self.frame_clock.retarget(pointer.x, pointer.y);
            }
            self.frame_clock.start()
        } else {
            self.frame_clock.stop();
            Ok(())
        }
    }

    fn present(&mut self, scene: Arc<OverlayScene>) -> Result<(), String> {
        let area = scene.clip.unwrap_or_else(|| self.virtual_bounds());
        let center = area.center();
        let scale = self
            .screens
            .iter()
            .find(|screen| screen.bounds.contains(&center))
            .map(|screen| screen.scale)
            .unwrap_or(1.0);
        self.overlay.present(scene, area, scale)
    }

    fn dismiss(&mut self) -> Result<(), String> {
        self.overlay.dismiss()
    }

    fn request_ui_scan(&mut self, request: crate::api::UiScanRequest) -> Result<(), String> {
        if self.ui_automation.is_none() {
            self.ui_automation = Some(accessibility::UiAutomationWorker::start()?);
        }
        let Some(worker) = self.ui_automation.as_ref() else {
            return Err("UI Automation worker was not retained after startup".into());
        };
        worker.submit(request, self.event_tx.clone())
    }

    fn appearance(&self) -> Appearance {
        self.last_appearance
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if let Some(item) = self.status_item.as_mut() {
            item.set_enabled(enabled);
        }
        Ok(())
    }

    fn toggle_autostart(&mut self) -> Result<bool, String> {
        autostart::WindowsAutostart::new().toggle()
    }

    fn check_for_updates(&mut self) -> Result<(), String> {
        let sender = self.event_tx.clone();
        crate::update::check_async(move |result| {
            let _ = sender.send(BackendEvent::UpdateChecked(result));
        })
    }

    fn present_update_result(
        &mut self,
        result: &crate::api::backend::UpdateCheckResult,
    ) -> Result<(), String> {
        status_item::present_update_result(result)
    }

    fn name(&self) -> &'static str {
        "windows"
    }

    fn shutdown(&mut self) -> Result<(), String> {
        self.shutdown_resources()
    }
}

fn focused_app_for(hwnd: HWND, window_title: String) -> FocusedApp {
    let mut process_id = 0;
    native::window_thread_process_id(hwnd, Some(&mut process_id));
    let bundle_id = if process_id == 0 {
        String::new()
    } else {
        match native::process_executable_name(process_id) {
            Some(name) => name,
            None => {
                crate::log_warning!(
                    "windows-focus",
                    "cannot resolve executable name for foreground pid {process_id}"
                );
                String::new()
            }
        }
    };
    FocusedApp {
        bundle_id,
        window_title,
        process_id,
    }
}

fn system_appearance() -> Appearance {
    if native::apps_use_light_theme() {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_name_resolves_the_current_process() {
        let actual =
            native::process_executable_name(std::process::id()).expect("current executable");
        let expected = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(actual.to_lowercase(), expected.to_lowercase());
    }
}
