// Native AppKit/CoreGraphics services are being migrated file-by-file to
// documented typed wrappers. The crate-wide deny remains active outside this
// explicit platform boundary.

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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

use crate::api::Autostart;
use crate::api::backend::{Appearance, Backend, BackendEvent, KeyDisposition};
use crate::api::command::{ButtonAction, FocusedApp, MouseButton};
use crate::api::geometry::{Point, Screen};
use crate::api::input::{Key, KeyState};
use crate::api::overlay::OverlayScene;
use crate::platform::scan_mailbox::ScanMailbox;

use self::hook::{HookStartup, HookThread};
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
struct EventSender {
    hook: Arc<OnceLock<hook::EventSender>>,
    fallback: Sender<BackendEvent>,
}

impl EventSender {
    fn new(fallback: Sender<BackendEvent>) -> Self {
        Self {
            hook: Arc::new(OnceLock::new()),
            fallback,
        }
    }

    fn promote(&self, sender: hook::EventSender) {
        let _ = self.hook.set(sender);
    }

    fn send(&self, event: BackendEvent) -> Result<(), ()> {
        let result = match self.hook.get() {
            Some(sender) => match sender.try_send(event) {
                Ok(()) => Ok(()),
                // Status/update workers must never block behind the bounded
                // keyboard queue. Preserve the event through the unbounded
                // fallback and wake the main run loop explicitly.
                Err(event) => self.fallback.send(event).map_err(|_| ()),
            },
            None => self.fallback.send(event).map_err(|_| ()),
        };
        if result.is_ok() {
            self.wake();
        }
        result
    }

    fn wake(&self) {
        workspace::wake_main_run_loop();
    }
}

pub struct MacOsBackend {
    hook: Option<HookThread>,
    async_rx: Receiver<BackendEvent>,
    event_tx: EventSender,
    scan_mailbox: Arc<ScanMailbox>,
    scan_worker: ui_scan::UiScanWorker,
    pending: VecDeque<BackendEvent>,
    overlay: Overlay,
    screens: Vec<Screen>,
    display_watcher: Option<screens::DisplayWatcher>,
    frame_clock: display_link::DisplayFrameClock,
    workspace: workspace::Workspace,
    status_item: Option<status_item::StatusItem>,
    update_worker: Option<crate::app::update::UpdateWorker>,
    held_buttons: Cell<u8>,
    click_tracker: Arc<Mutex<ClickTracker>>,
    warned_about_permissions: bool,
    keyboard: input::KeyboardInjector,
    shutdown_complete: bool,
}

impl MacOsBackend {
    pub fn new() -> Result<Self, String> {
        let (async_tx, async_rx) = mpsc::channel();
        let event_tx = EventSender::new(async_tx);
        let scan_mailbox = Arc::new(ScanMailbox::default());
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

        let hook_deadline = Instant::now() + Duration::from_secs(2);
        let hook_start = HookStartup::spawn(Arc::clone(&click_tracker));
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
        let display_watcher = screens::DisplayWatcher::new();
        let workspace = workspace::Workspace::new();
        let keyboard = input::KeyboardInjector::new();
        let hook = match hook_start.and_then(|startup| {
            startup.finish(hook_deadline.saturating_duration_since(Instant::now()))
        }) {
            Ok(hook) => {
                event_tx.promote(hook.event_sender());
                Some(hook)
            }
            Err(error) => {
                if trusted {
                    crate::app::logging::report_error("macos-hook", error);
                }
                None
            }
        };
        Ok(Self {
            hook,
            async_rx,
            event_tx,
            scan_mailbox,
            scan_worker: ui_scan::UiScanWorker::new(),
            pending: VecDeque::new(),
            overlay: Overlay::new(),
            screens: initial_screens,
            display_watcher: Some(display_watcher),
            frame_clock,
            workspace,
            status_item,
            update_worker: None,
            held_buttons: Cell::new(0),
            click_tracker,
            warned_about_permissions: false,
            keyboard,
            shutdown_complete: false,
        })
    }

    fn has_hook(&self) -> bool {
        self.hook.as_ref().is_some_and(HookThread::is_active)
    }

    fn refresh_native_events(&mut self) {
        self.pending.extend(self.workspace.refresh());
        if self
            .display_watcher
            .as_ref()
            .is_some_and(screens::DisplayWatcher::take_changed)
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
        if let Some(event) = self.hook.as_mut().and_then(HookThread::take_capture_loss) {
            return Some(event);
        }
        // CGEventTap is synchronously waiting for disposition; never place a
        // scan result or status event ahead of physical input.
        self.hook
            .as_mut()
            .and_then(HookThread::try_next_event)
            .or_else(|| {
                self.pending
                    .pop_front()
                    .or_else(|| self.scan_mailbox.take().map(BackendEvent::UiScanned))
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

    fn reap_update_worker(&mut self) {
        if self
            .update_worker
            .as_mut()
            .is_some_and(crate::app::update::UpdateWorker::reap_finished)
        {
            self.update_worker.take();
        }
    }

    fn shutdown_resources(&mut self) -> Result<(), String> {
        if self.shutdown_complete {
            return Ok(());
        }

        // Stop every producer before tearing down the AppKit objects they may
        // wake or update. All operations are idempotent so Drop can safely use
        // the same path after an earlier error.
        self.status_item.take();
        self.display_watcher.take();
        self.frame_clock.stop();
        let mut first_error = self.scan_worker.shutdown().err();
        if let Some(worker) = self.update_worker.as_mut() {
            match worker.cancel_and_wait() {
                Ok(()) => {
                    self.update_worker.take();
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }

        if let Err(error) = self.release_held_buttons()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Some(hook) = self.hook.as_mut() {
            match hook.stop() {
                Ok(()) => {
                    self.hook.take();
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if let Err(error) = self.overlay.dismiss()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        match first_error {
            Some(error) => Err(error),
            None => {
                self.shutdown_complete = true;
                Ok(())
            }
        }
    }
}

impl Drop for MacOsBackend {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown_resources() {
            crate::app::logging::report_error(
                "macos-shutdown",
                format!("cannot completely release macOS backend resources: {error}"),
            );
        }
    }
}

impl Backend for MacOsBackend {
    fn poll(&mut self, timeout: Duration) -> Result<Option<BackendEvent>, String> {
        self.reap_update_worker();
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
        if self.screens.is_empty() {
            screens::list_screens()
        } else {
            Ok(self.screens.clone())
        }
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
        let bit = input::button_mask(button);
        if super::redundant_button_action(self.held_buttons.get() & bit != 0, action) {
            return Ok(());
        }
        input::mouse_button(&self.click_tracker, button, action)?;
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
        self.keyboard.send_key(key, state)
    }

    fn send_keys(&self, events: Vec<(Key, KeyState)>) -> Result<(), String> {
        self.keyboard.send_keys(events)
    }

    fn send_chord(&self, keys: &[Key]) -> Result<(), String> {
        self.keyboard.send_chord(keys)
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
        self.overlay.present(scene)?;
        crate::app::perf_probe::mark("native_presented");
        Ok(())
    }

    fn update_overlay_positions(
        &mut self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<bool, String> {
        let updated = self.overlay.update_positions(cursor, indicator)?;
        if updated {
            crate::app::perf_probe::mark("native_presented");
        }
        Ok(updated)
    }

    fn dismiss(&mut self) -> Result<(), String> {
        self.overlay.dismiss()
    }

    fn request_ui_scan(&mut self, request: crate::api::UiScanRequest) -> Result<(), String> {
        let generation = self.scan_mailbox.begin(request.id);
        self.scan_worker.request_scan(
            request,
            generation,
            Arc::clone(&self.scan_mailbox),
            self.event_tx.clone(),
        );
        Ok(())
    }

    fn cancel_ui_scan(&mut self, id: u64) -> Result<(), String> {
        if self.scan_mailbox.cancel(id) {
            self.scan_worker.cancel_scan(id);
        }
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
        self.reap_update_worker();
        if self.update_worker.is_some() {
            return Ok(());
        }
        let progress_sender = self.event_tx.clone();
        let complete_sender = self.event_tx.clone();
        self.update_worker = crate::app::update::check_async(
            move |progress| {
                let _ = progress_sender.send(BackendEvent::UpdateProgress(progress));
            },
            move |result| {
                let _ = complete_sender.send(BackendEvent::UpdateChecked(result));
            },
        )?;
        Ok(())
    }

    fn present_update_progress(
        &mut self,
        progress: &crate::api::backend::UpdateProgress,
    ) -> Result<(), String> {
        let Some(item) = self.status_item.as_mut() else {
            return Err("macOS status item is unavailable".into());
        };
        item.present_update_progress(progress);
        Ok(())
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

    fn open_url(&mut self, url: &str) -> Result<(), String> {
        let text = NSString::from_str(url);
        let url = NSURL::URLWithString(&text)
            .ok_or_else(|| "macOS could not parse the simulator URL".to_string())?;
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err("macOS could not open the default browser".into())
        }
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
        self.shutdown_resources()
    }
}
