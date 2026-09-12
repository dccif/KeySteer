// Native AppKit/CoreGraphics services are being migrated file-by-file to
// documented typed wrappers. The crate-wide deny remains active outside this
// explicit platform boundary.

//! macOS backend composed from isolated native services.

mod accessibility;
mod autostart;
mod display_link;
mod hook;
mod input;
pub(crate) mod latest_point_mailbox;
pub(crate) mod multi_click;
mod native;
mod overlay;
mod permissions;
mod screens;
mod status_item;
mod ui_scan;
mod vision;
mod window_audio;
mod window_move;
mod window_tabs;
mod workspace;

#[cfg(feature = "benchmark-hooks")]
pub use input::{CharacterCaptureProbe, observe_character_unfiltered};

use std::cell::{Cell, RefCell};
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
use crate::platform::common::scan_mailbox::ScanMailbox;

use self::hook::{HookStartup, HookThread};
use self::overlay::Overlay;
use crate::platform::multi_click::ClickTracker;

const BACKEND_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Independent asynchronous event lane. Synchronous CGEventTap input owns the
/// bounded hook queue exclusively so menu/update bursts cannot delay or drop
/// a physical key waiting for disposition.
#[derive(Clone)]
struct EventSender {
    sender: Sender<BackendEvent>,
}

impl EventSender {
    fn new(sender: Sender<BackendEvent>) -> Self {
        Self { sender }
    }

    fn send(&self, event: BackendEvent) -> Result<(), ()> {
        let result = self.sender.send(event).map_err(|_| ());
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
    window_move: RefCell<Option<window_move::WindowMove<accessibility::MovableWindow>>>,
    display_watcher: Option<screens::DisplayWatcher>,
    frame_clock: display_link::DisplayFrameClock,
    workspace: workspace::Workspace,
    status_item: Option<status_item::StatusItem>,
    update_worker: Option<crate::platform::common::update::UpdateWorker>,
    window_worker: Option<crate::platform::common::window_session::WindowWorker>,
    held_buttons: Cell<u8>,
    click_tracker: Arc<Mutex<ClickTracker>>,
    warned_about_permissions: bool,
    keyboard: input::KeyboardInjector,
    shutdown_complete: bool,
    shutdown_attempted: bool,
}

impl MacOsBackend {
    pub fn new() -> Result<Self, String> {
        let (async_tx, async_rx) = mpsc::channel();
        let event_tx = EventSender::new(async_tx);
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "macOS backend must be created on the main thread".to_string())?;
        status_item::prepare_application(mtm)?;
        // Install the top status item before permissions, Hook startup, screen
        // enumeration, or any other work that can be slow during login.
        let mut status_item = status_item::StatusItem::new(mtm, event_tx.clone());
        workspace::pump_app_events();
        status_item.maintain_icon_attachment();

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
        let frame_clock = display_link::DisplayFrameClock::new(mtm);
        let initial_screens = screens::list_screens().unwrap_or_else(|error| {
            crate::support::logging::report_error(
                "macos-screen",
                format!("initial display enumeration failed: {error}"),
            );
            Vec::new()
        });
        let display_watcher = screens::DisplayWatcher::new()?;
        let workspace = workspace::Workspace::new();
        let keyboard = input::KeyboardInjector::new();
        let hook = match hook_start.and_then(|startup| {
            startup.finish(hook_deadline.saturating_duration_since(Instant::now()))
        }) {
            Ok(hook) => Some(hook),
            Err(error) => {
                if trusted {
                    crate::support::logging::report_error("macos-hook", error);
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
            window_move: RefCell::new(None),
            display_watcher: Some(display_watcher),
            frame_clock,
            workspace,
            status_item: Some(status_item),
            update_worker: None,
            window_worker: None,
            held_buttons: Cell::new(0),
            click_tracker,
            warned_about_permissions: false,
            keyboard,
            shutdown_complete: false,
            shutdown_attempted: false,
        })
    }

    fn has_hook(&self) -> bool {
        self.hook.as_ref().is_some_and(HookThread::is_active)
    }

    fn refresh_native_events(&mut self) {
        self.pending.extend(self.workspace.refresh());
        if let Some(item) = self.status_item.as_mut() {
            item.maintain_icon_attachment();
        }
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
                self.scan_mailbox
                    .take()
                    .map(BackendEvent::UiScanned)
                    .or_else(|| self.pending.pop_front())
                    .or_else(|| self.async_rx.try_recv().ok())
            })
    }

    fn release_held_buttons(&self) -> Result<(), String> {
        let mut errors = crate::support::errors::ErrorBundle::default();
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
                Err(error) => errors.push(format!("release {button:?}"), error),
            }
        }
        errors.into_result()
    }

    fn reap_update_worker(&mut self) {
        if self
            .update_worker
            .as_mut()
            .is_some_and(crate::platform::common::update::UpdateWorker::reap_finished)
        {
            self.update_worker.take();
        }
    }

    fn shutdown_resources(&mut self) -> Result<(), String> {
        if self.shutdown_complete {
            return Ok(());
        }
        let now = Instant::now();
        let deadline = now.checked_add(BACKEND_SHUTDOWN_TIMEOUT).unwrap_or(now);

        // Phase one never waits. Make the synchronous event tap fail open
        // before joining any worker; otherwise physical input arriving while
        // Vision is stopping would wait for an Engine disposition that can no
        // longer be sent.
        let mut errors = crate::support::errors::ErrorBundle::default();
        if let Some(hook) = self.hook.as_mut() {
            hook.request_stop();
        }
        self.status_item.take();
        if let Some(movement) = self.window_move.get_mut().take()
            && let Err(error) = movement.cancel()
        {
            errors.push("restore moving fullscreen window", error);
        }
        self.frame_clock.stop();
        self.scan_worker.request_stop();
        if let Some(worker) = self.update_worker.as_ref() {
            worker.request_cancel();
        }
        errors.record("held mouse buttons", self.release_held_buttons());
        errors.record("overlay dismiss", self.overlay.dismiss());
        if let Some(watcher) = self.display_watcher.as_mut() {
            match watcher.stop() {
                Ok(()) => {
                    self.display_watcher.take();
                }
                Err(error) => errors.push("display watcher", error),
            }
        }

        if let Some(worker) = self.window_worker.as_mut() {
            match worker.stop_until(deadline) {
                Ok(()) => {
                    self.window_worker.take();
                }
                Err(error) => errors.push("window worker", error),
            }
        }
        if let Some(mtm) = MainThreadMarker::new() {
            window_tabs::clear(mtm);
        }
        // Phase two joins only producers that were cancelled above. Every
        // stage shares the same absolute deadline.
        if let Some(hook) = self.hook.as_mut() {
            match hook.stop_until(deadline) {
                Ok(()) => {
                    self.hook.take();
                }
                Err(error) => errors.push("input hook", error),
            }
        }
        errors.record("UI scan worker", self.scan_worker.shutdown_until(deadline));
        if let Some(worker) = self.update_worker.as_mut() {
            match worker.cancel_and_wait_until(deadline) {
                Ok(()) => {
                    self.update_worker.take();
                }
                Err(error) => errors.push("update worker", error),
            }
        }
        if errors.is_empty() {
            self.shutdown_complete = true;
        }
        errors.into_result()
    }
}

impl Drop for MacOsBackend {
    fn drop(&mut self) {
        if self.shutdown_complete || self.shutdown_attempted {
            return;
        }
        if let Err(error) = self.shutdown_resources() {
            crate::support::logging::report_error(
                "macos-shutdown",
                format!("cannot completely release macOS backend resources: {error}"),
            );
        }
    }
}

impl Backend for MacOsBackend {
    fn request_text_prompt(
        &mut self,
        prompt: crate::api::window_presets::TextPrompt,
    ) -> Result<(), String> {
        self.status_item
            .as_ref()
            .ok_or("macOS status item is unavailable")?
            .request_text_prompt(prompt)
    }
    fn cancel_text_prompt(&mut self, id: u64) {
        if let Some(item) = &self.status_item {
            item.cancel_text_prompt(id);
        }
    }
    fn request_audio(&mut self, request: crate::api::audio::AudioRequest) -> Result<(), String> {
        if self.window_worker.is_none() {
            let tx = self.event_tx.clone();
            self.window_worker = Some(
                crate::platform::common::window_session::WindowWorker::start(
                    accessibility::window_manager::MacWindows::default,
                    move |event| {
                        let _ = tx.send(event);
                    },
                )?,
            );
        }
        self.window_worker
            .as_ref()
            .ok_or("audio worker did not initialize")?
            .submit_audio(request)
    }
    fn cancel_audio_session(&mut self, session: u64) {
        if let Some(worker) = &self.window_worker {
            worker.cancel_audio(session);
        }
    }
    fn request_window(&mut self, request: crate::api::window::WindowRequest) -> Result<(), String> {
        if self.window_worker.is_none() {
            let tx = self.event_tx.clone();
            self.window_worker = Some(
                crate::platform::common::window_session::WindowWorker::start(
                    accessibility::window_manager::MacWindows::default,
                    move |event| {
                        let _ = tx.send(event);
                    },
                )?,
            );
        }
        self.window_worker
            .as_ref()
            .ok_or("window worker did not initialize")?
            .submit(request, &self.screens)
    }
    fn cancel_window_session(&mut self, session: u64) {
        if let Some(worker) = &self.window_worker {
            worker.cancel(session);
        }
    }
    fn poll(&mut self, timeout: Duration) -> Result<Option<BackendEvent>, String> {
        self.reap_update_worker();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.try_event() {
                return Ok(Some(event));
            }
            crate::support::worker::reap_quarantined();
            self.refresh_native_events();
            if let Some(mtm) = MainThreadMarker::new() {
                window_tabs::refresh(mtm, &self.screens);
            }
            if let Some(result) = self
                .window_move
                .get_mut()
                .as_mut()
                .and_then(|movement| movement.poll(&self.screens, Instant::now()))
            {
                self.window_move.get_mut().take();
                self.pending
                    .push_back(BackendEvent::WindowMoveCompleted(result));
            }
            if let Some(event) = self.try_event() {
                return Ok(Some(event));
            }
            let mut remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            if self.window_move.get_mut().is_some() {
                remaining = remaining.min(window_move::POLL_INTERVAL);
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

    fn set_character_bindings(&mut self, keys: &[crate::api::Key]) {
        if let Some(hook) = self.hook.as_ref() {
            hook.set_character_bindings(keys);
        }
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

    fn move_window_to_screen(
        &self,
        target: crate::api::command::WindowScreenTarget,
    ) -> Result<Option<Point>, String> {
        let mut pending = self.window_move.borrow_mut();
        if pending.is_some() {
            return Ok(None);
        }
        let cursor = self.pointer()?;
        let Some(window) = accessibility::window_under_pointer(cursor)? else {
            return Ok(None);
        };
        let (movement, pointer) = window_move::WindowMove::start(
            window,
            cursor,
            &self.screens()?,
            target,
            Instant::now(),
        )?;
        *pending = movement;
        Ok(pointer)
    }

    fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
        Ok(self.workspace.focused_app())
    }

    fn warp_pointer(&self, to: Point) -> Result<(), String> {
        input::warp_cursor(to)
    }

    fn move_pointer(&self, from: Point, dx: f64, dy: f64) -> Result<(), String> {
        let held_buttons = self.held_buttons.get();
        let drag_modifier_flags = if held_buttons == 0 {
            0
        } else {
            self.hook
                .as_ref()
                .map_or(0, HookThread::drag_modifier_flags)
        };
        input::move_cursor_relative(from, dx, dy, held_buttons, drag_modifier_flags).map(|_| ())
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
        crate::support::perf_probe::mark("native_presented");
        Ok(())
    }

    fn update_overlay_positions(
        &mut self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<bool, String> {
        let updated = self.overlay.update_positions(cursor, indicator)?;
        if updated {
            crate::support::perf_probe::mark("native_presented");
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
        self.update_worker = crate::platform::common::update::check_async(
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
        status_item::open_https_url(url)
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
        self.shutdown_attempted = true;
        self.shutdown_resources()
    }
}
