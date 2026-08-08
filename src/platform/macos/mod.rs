// Native AppKit/CoreGraphics services are being migrated file-by-file to
// documented typed wrappers. The crate-wide deny remains active outside this
// explicit platform boundary.
#![allow(clippy::undocumented_unsafe_blocks)]

//! macOS backend composed from isolated native services.

mod accessibility;
mod autostart;
mod display_link;
mod hook;
mod input;
mod native;
mod overlay;
mod permissions;
mod screens;
mod status_item;
mod ui_scan;
mod vision;
mod workspace;

use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::NSEvent;

use crate::api::Autostart;
use crate::api::backend::{Appearance, Backend, BackendEvent, KeyDisposition};
use crate::api::command::{ButtonAction, FocusedApp, MouseButton};
use crate::api::geometry::{Point, Screen};
use crate::api::input::{Key, KeyState};
use crate::api::overlay::OverlayScene;

use self::hook::HookThread;
use self::overlay::Overlay;
use crate::platform::multi_click::ClickTracker;

/// Returns the launchable application bundle when `executable` is the main
/// binary at `Some.app/Contents/MacOS/*`.
fn app_bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// Routes worker and menu events into the hook queue when available. This
/// gives asynchronous work a real wake-up path instead of polling for it.
#[derive(Clone)]
enum EventSender {
    Hook(hook::EventSender),
    Channel(Sender<BackendEvent>),
}

impl EventSender {
    fn send(&self, event: BackendEvent) -> Result<(), ()> {
        let result = match self {
            Self::Hook(sender) => sender.send(event),
            Self::Channel(sender) => sender.send(event).map_err(|_| ()),
        };
        if result.is_ok() {
            workspace::wake_main_run_loop();
        }
        result
    }
}

pub struct MacOsBackend {
    hook: Option<HookThread>,
    async_rx: Receiver<BackendEvent>,
    event_tx: EventSender,
    pending: VecDeque<BackendEvent>,
    overlay: Overlay,
    screens: Vec<Screen>,
    display_watcher: screens::DisplayWatcher,
    frame_clock: display_link::DisplayFrameClock,
    workspace: workspace::Workspace,
    status_item: Option<status_item::StatusItem>,
    held_buttons: Cell<u8>,
    click_tracker: Arc<Mutex<ClickTracker>>,
    warned_about_permissions: bool,
}

impl MacOsBackend {
    pub fn new() -> Result<Self, String> {
        let (async_tx, async_rx) = mpsc::channel();
        let configured_interval = NSEvent::doubleClickInterval();
        let double_click_interval = if configured_interval.is_finite() && configured_interval > 0.0
        {
            Duration::from_secs_f64(configured_interval)
        } else {
            Duration::from_millis(500)
        };
        let click_tracker = Arc::new(Mutex::new(ClickTracker::new(double_click_interval)));
        let trusted = permissions::is_trusted();
        if !trusted {
            permissions::prompt_for_trust();
        }

        let hook = match HookThread::start(Arc::clone(&click_tracker)) {
            Ok(hook) => Some(hook),
            Err(error) => {
                if trusted {
                    crate::app::logging::report_error("macos-hook", error);
                }
                None
            }
        };

        let event_tx = hook.as_ref().map_or_else(
            || EventSender::Channel(async_tx),
            |hook| EventSender::Hook(hook.event_sender()),
        );
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "macOS backend must be created on the main thread".to_string())?;
        let status_item = Some(status_item::StatusItem::new(mtm, event_tx.clone()));
        let frame_clock = display_link::DisplayFrameClock::new(mtm);
        let initial_screens = screens::list_screens().unwrap_or_else(|error| {
            crate::app::logging::report_error(
                "macos-screen",
                format!("initial display enumeration failed: {error}"),
            );
            Vec::new()
        });
        Ok(Self {
            hook,
            async_rx,
            event_tx,
            pending: VecDeque::new(),
            overlay: Overlay::new(),
            screens: initial_screens,
            display_watcher: screens::DisplayWatcher::new(),
            frame_clock,
            workspace: workspace::Workspace::new(),
            status_item,
            held_buttons: Cell::new(0),
            click_tracker,
            warned_about_permissions: false,
        })
    }

    fn has_hook(&self) -> bool {
        self.hook.is_some()
    }

    fn refresh_native_events(&mut self) {
        self.pending.extend(self.workspace.refresh());
        if self.display_watcher.take_changed()
            && let Ok(current) = screens::list_screens()
            && !current.is_empty()
            && current != self.screens
        {
            self.screens = current.clone();
            self.pending
                .push_back(BackendEvent::ScreensChanged(current));
        }
    }

    fn try_event(&mut self) -> Option<BackendEvent> {
        // CGEventTap is synchronously waiting for disposition; never place a
        // scan result or status event ahead of physical input.
        self.hook
            .as_mut()
            .and_then(HookThread::try_next_event)
            .or_else(|| {
                self.pending
                    .pop_front()
                    .or_else(|| self.async_rx.try_recv().ok())
            })
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
            match input::mouse_button(&self.click_tracker, button, ButtonAction::Release) {
                Ok(()) => self.held_buttons.set(self.held_buttons.get() & !bit),
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for MacOsBackend {
    fn drop(&mut self) {
        if let Err(error) = self.release_held_buttons() {
            crate::app::logging::report_error(
                "macos-input",
                format!("cannot release held mouse buttons during drop: {error}"),
            );
        }
    }
}

impl Backend for MacOsBackend {
    fn poll(&mut self, timeout: Duration) -> Result<Option<BackendEvent>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.refresh_native_events();
            if let Some(event) = self.try_event() {
                return Ok(Some(event));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            if self.frame_clock.is_running() {
                if let Some(elapsed) = self.frame_clock.next(remaining) {
                    return Ok(Some(BackendEvent::Frame(elapsed)));
                }
            } else {
                workspace::wait_for_app_event(remaining);
            }
        }
    }

    fn dispose_key(&mut self, disposition: KeyDisposition) -> Result<(), String> {
        if let Some(hook) = self.hook.as_mut() {
            return hook.set_disposition(disposition);
        }

        if disposition == KeyDisposition::Consume && !self.warned_about_permissions {
            self.warned_about_permissions = true;
            crate::report_warning!(
                "macos-hook",
                "cannot capture the keyboard without Accessibility permission, so keys also reach the focused app"
            );
        }
        Ok(())
    }

    fn screens(&self) -> Result<Vec<Screen>, String> {
        screens::list_screens()
    }

    fn pointer(&self) -> Result<Point, String> {
        input::cursor_position()
    }

    fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
        Ok(self.workspace.focused_app())
    }

    fn warp_pointer(&self, to: Point) -> Result<(), String> {
        input::warp_cursor(to)
    }

    fn move_pointer(&self, from: Point, dx: f64, dy: f64) -> Result<(), String> {
        input::move_cursor_relative(from, dx, dy, self.held_buttons.get()).map(|_| ())
    }

    fn mouse_button(&self, button: MouseButton, action: ButtonAction) -> Result<(), String> {
        input::mouse_button(&self.click_tracker, button, action)?;
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

    fn set_frame_clock(&mut self, active: bool) -> Result<(), String> {
        if active {
            let source = self.overlay.display_link_source()?;
            self.frame_clock.start(source);
        } else {
            self.frame_clock.stop();
        }
        Ok(())
    }

    fn present(&mut self, scene: Arc<OverlayScene>) -> Result<(), String> {
        self.overlay.present(scene)
    }

    fn dismiss(&mut self) -> Result<(), String> {
        self.overlay.dismiss()
    }

    fn request_ui_scan(&mut self, request: crate::api::UiScanRequest) -> Result<(), String> {
        ui_scan::request_scan(request, self.event_tx.clone());
        Ok(())
    }

    fn appearance(&self) -> Appearance {
        self.workspace.appearance()
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if let Some(item) = self.status_item.as_mut() {
            item.set_enabled(enabled);
        }
        Ok(())
    }

    fn toggle_autostart(&mut self) -> Result<bool, String> {
        let enabled = autostart::MacosAutostart::new().toggle()?;
        if let Some(item) = self.status_item.as_mut() {
            item.set_autostart_enabled(enabled);
        }
        Ok(enabled)
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
        let Some(item) = self.status_item.as_mut() else {
            return Err("macOS status item is unavailable".into());
        };
        item.present_update_result(result)
    }

    fn name(&self) -> &'static str {
        "macos"
    }

    fn keyboard_available(&self) -> bool {
        self.has_hook()
    }

    fn keyboard_unavailable_reason(&self) -> Option<String> {
        if self.has_hook() {
            return None;
        }
        Some(if permissions::is_trusted() {
            "the event tap could not be installed even though Accessibility permission is granted"
                .to_string()
        } else {
            permissions::instructions()
        })
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let release_result = self.release_held_buttons();
        if let Some(mut hook) = self.hook.take() {
            hook.stop();
        }
        let dismiss_result = self.overlay.dismiss();
        release_result.and(dismiss_result)
    }
}
