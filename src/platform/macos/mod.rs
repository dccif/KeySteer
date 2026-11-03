// Native AppKit/CoreGraphics services are being migrated file-by-file to
// documented typed wrappers. The crate-wide deny remains active outside this
// explicit platform boundary.

//! macOS backend composed from isolated native services.

mod accessibility;
mod autostart;
mod display;
mod event_loop;
mod input;
mod native;
mod permissions;
mod scan_workers;
mod status_item;
mod text_segmentation;
mod ui_scan;
mod update_installer;
mod vision;
mod window_audio;
mod window_move;
mod window_tabs;
mod workspace;

/// Own private process modes before normal application startup.
pub(crate) fn run(application: impl FnOnce() -> std::process::ExitCode) -> std::process::ExitCode {
    update_installer::run_internal_helper()
        .or_else(vision::run_internal_ocr_helper)
        .unwrap_or_else(application)
}

use crate::platform::common::event_queue::{self, Receiver, Sender};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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

use self::display::{display_link::DisplayFrameClock, overlay::Overlay};
use self::input::hook::{HookStartup, HookThread};
use self::input::multi_click::ClickTracker;

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
    sender: Sender,
}

impl EventSender {
    fn new(sender: Sender) -> Self {
        Self { sender }
    }

    fn send(&self, event: BackendEvent) -> Result<(), ()> {
        if self.sender.send(event)? {
            self.wake();
        }
        Ok(())
    }

    fn wake(&self) {
        event_loop::wake_main_run_loop();
    }
}

pub struct MacOsBackend {
    point_sampler: Option<crate::platform::common::point_sample::Worker>,
    hook: Option<HookThread>,
    async_rx: Receiver,
    background_budget: event_queue::BackgroundBudget,
    event_tx: EventSender,
    scan_mailbox: Arc<ScanMailbox>,
    scan_previews: Arc<ScanMailbox>,
    preview_view: ui_scan::PreviewView,
    scan_fusion: crate::platform::common::scan_fusion::Worker,
    scan_worker: ui_scan::UiScanWorker,
    pending: VecDeque<BackendEvent>,
    text_segmenter: Option<crate::platform::common::text_segmentation::Worker>,
    overlay: Overlay,
    screens: Vec<Screen>,
    window_move: RefCell<Option<window_move::WindowMove<accessibility::MovableWindow>>>,
    display_watcher: Option<display::DisplayWatcher>,
    frame_clock: DisplayFrameClock,
    scroll_worker: crate::platform::common::scroll_worker::ScrollWorker,
    workspace: workspace::Workspace,
    _permission_observer: permissions::ChangeObserver,
    status_item: Option<status_item::StatusItem>,
    update_worker: Option<crate::platform::common::update::UpdateWorker>,
    update_ready_signaled: bool,
    window_worker: Option<crate::platform::common::window_session::WindowWorker>,
    held_buttons: Cell<u8>,
    click_tracker: Arc<Mutex<ClickTracker>>,
    warned_about_permissions: bool,
    keyboard: input::KeyboardInjector,
    shutdown_complete: bool,
    shutdown_attempted: bool,
    native_loop_running: bool,
}

impl MacOsBackend {
    pub fn new() -> Result<Self, String> {
        let (async_tx, async_rx) = event_queue::channel();
        let event_tx = EventSender::new(async_tx);
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "macOS backend must be created on the main thread".to_string())?;
        status_item::prepare_application(mtm)?;
        // Install the top status item before permissions, Hook startup, screen
        // enumeration, or any other work that can be slow during login.
        let mut status_item = status_item::StatusItem::new(mtm, event_tx.clone());
        event_loop::pump_app_events();
        status_item.maintain_icon_attachment();

        // Give the independent scroll executor a head start while permission,
        // Hook and display setup runs. It stays asleep until a real command.
        let scroll_events = event_tx.clone();
        let scroll_worker = crate::platform::common::scroll_worker::ScrollWorker::start(
            input::scroll,
            move |event| {
                let _ = scroll_events.send(event);
            },
        )?;
        let frame_clock = DisplayFrameClock::new(mtm);
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
        let permission_observer = permissions::ChangeObserver::new();
        if !trusted {
            permissions::prompt_for_trust();
        }

        let hook_deadline = Instant::now() + Duration::from_secs(2);
        let hook_start = HookStartup::spawn(Arc::clone(&click_tracker));
        // Register before the initial snapshot so a display change during
        // enumeration remains pending for the first native refresh.
        let display_watcher = display::DisplayWatcher::new()?;
        let initial_screens = display::list_screens().unwrap_or_else(|error| {
            crate::support::logging::report_error(
                "macos-screen",
                format!("initial display enumeration failed: {error}"),
            );
            Vec::new()
        });
        let workspace = workspace::Workspace::new(mtm);
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
            point_sampler: None,
            hook,
            async_rx,
            background_budget: event_queue::BackgroundBudget::default(),
            event_tx,
            scan_mailbox,
            scan_previews: Arc::new(ScanMailbox::default()),
            preview_view: ui_scan::PreviewView::default(),
            scan_fusion: crate::platform::common::scan_fusion::Worker::default(),
            scan_worker: ui_scan::UiScanWorker::new(),
            pending: VecDeque::new(),
            text_segmenter: None,
            overlay: Overlay::new(),
            screens: initial_screens,
            window_move: RefCell::new(None),
            display_watcher: Some(display_watcher),
            frame_clock,
            scroll_worker,
            workspace,
            _permission_observer: permission_observer,
            status_item: Some(status_item),
            update_worker: None,
            update_ready_signaled: false,
            window_worker: None,
            held_buttons: Cell::new(0),
            click_tracker,
            warned_about_permissions: false,
            keyboard,
            shutdown_complete: false,
            shutdown_attempted: false,
            native_loop_running: false,
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
        let _ = self.refresh_screens();
    }

    fn native_timeout(&mut self, timeout: Duration) -> Duration {
        let timeout = timeout.min(self.status_item.as_ref().map_or(
            Duration::MAX,
            status_item::StatusItem::time_until_icon_retry,
        ));
        let timeout = timeout.min(
            self.display_watcher
                .as_ref()
                .map_or(Duration::MAX, display::DisplayWatcher::time_until_retry),
        );
        if self.window_move.get_mut().is_some() {
            timeout.min(window_move::POLL_INTERVAL)
        } else {
            timeout
        }
    }

    fn refresh_screens(&mut self) -> Result<(), String> {
        let changed = self
            .display_watcher
            .as_ref()
            .is_none_or(display::DisplayWatcher::take_changed);
        if changed || self.screens.is_empty() {
            let current = display::list_screens().inspect_err(|_| {
                if let Some(watcher) = &self.display_watcher {
                    watcher.retry_refresh();
                }
            })?;
            if current.is_empty() {
                if let Some(watcher) = &self.display_watcher {
                    watcher.retry_refresh();
                }
            } else {
                // Equal logical geometry does not guarantee equal backing scale
                // or AppKit placement. Refresh the live panels before another
                // position-only frame can bypass complete scene presentation.
                self.overlay.refresh_screens(&current).inspect_err(|_| {
                    if let Some(watcher) = &self.display_watcher {
                        watcher.retry_refresh();
                    }
                })?;
                if current != self.screens {
                    self.screens = current.clone();
                    self.pending
                        .push_back(BackendEvent::ScreensChanged(current));
                }
                if let Ok(source) = self.overlay.display_link_source() {
                    self.frame_clock.prepare(source);
                } else {
                    self.frame_clock.stop();
                }
            }
        }
        Ok(())
    }

    fn try_event(&mut self) -> Option<BackendEvent> {
        if let Some(event) = self.hook.as_mut().and_then(HookThread::take_capture_loss) {
            return Some(event);
        }
        // CGEventTap is synchronously waiting for disposition; never place a
        // scan result or status event ahead of physical input.
        if let Some(event) = self.hook.as_mut().and_then(HookThread::try_next_event) {
            return Some(event);
        }
        if let Some(elapsed) = self.frame_clock.try_next() {
            self.background_budget.reset();
            return Some(BackendEvent::Frame(elapsed));
        }
        if !self.native_loop_running && self.background_budget.yield_due() {
            // Service ready native sources even while completions keep arriving.
            // Recheck synchronous input after AppKit had an opportunity to run.
            event_loop::pump_ready_sources();
            if let Some(event) = self.hook.as_mut().and_then(HookThread::try_next_event) {
                return Some(event);
            }
            if let Some(elapsed) = self.frame_clock.try_next() {
                self.background_budget.reset();
                return Some(BackendEvent::Frame(elapsed));
            }
        }
        if let Some(event) = self.scan_mailbox.take_activation() {
            return Some(event);
        }
        let event = self
            .scan_mailbox
            .take()
            .map(|mut result| {
                self.preview_view.authoritative(&mut result);
                if result.status != crate::api::command::UiScanStatus::Partial {
                    // The engine retires its scan owner after this event.
                    // Cancel remaining native work before losing that route.
                    self.cancel_scan(result.id);
                }
                BackendEvent::UiScanned(result)
            })
            .or_else(|| {
                self.scan_previews
                    .take()
                    .and_then(|result| self.preview_view.preview(result))
                    .map(BackendEvent::UiScanned)
            })
            .or_else(|| self.pending.pop_front())
            .or_else(|| self.async_rx.try_recv().ok());
        if event.is_some() {
            self.background_budget.record();
        } else {
            self.background_budget.reset();
        }
        event
    }

    fn cancel_scan(&mut self, id: u64) {
        self.scan_mailbox.cancel(id);
        self.scan_previews.cancel(id);
        self.preview_view.cancel(id);
        self.scan_fusion.cancel(id);
        self.scan_worker.cancel_scan(id);
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
        self.scroll_worker.request_stop();
        self.stop_input_capture();
        if let Some(item) = &self.status_item {
            item.menu().cancelTrackingWithoutAnimation();
        }
        if let Some(movement) = self.window_move.get_mut().take()
            && let Err(error) = movement.cancel()
        {
            errors.push("restore moving fullscreen window", error);
        }
        self.frame_clock.stop();
        self.scan_worker.request_stop();
        self.scan_fusion.request_stop();
        if let Some(worker) = &self.text_segmenter {
            worker.request_stop();
        }
        if let Some(worker) = &self.point_sampler {
            worker.request_stop();
        }
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
        errors.record("UI scan fusion", self.scan_fusion.stop_until(deadline));
        if let Some(mut worker) = self.text_segmenter.take() {
            errors.record("text segmenter", worker.stop_until(deadline));
        }
        errors.record(
            "continuous scrolling",
            self.scroll_worker.stop_until(deadline),
        );
        if let Some(mut worker) = self.point_sampler.take() {
            errors.record("point sampler", worker.stop_until(deadline));
        }
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
        // Drop may acknowledge AppKit's pending system quit and terminate the
        // process. All native workers and input must already be cleaned up.
        self.status_item.take();
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
    fn request_text_segmentation(&mut self, request: crate::api::text_select::Request) -> bool {
        if self.text_segmenter.is_none() {
            let tx = self.event_tx.clone();
            self.text_segmenter = crate::platform::common::text_segmentation::Worker::new(
                text_segmentation::words,
                move |result| {
                    let _ = tx.send(BackendEvent::TextSegmented(result));
                },
            )
            .ok();
        }
        if let Some(worker) = &self.text_segmenter {
            worker.submit(Some(request));
            true
        } else {
            false
        }
    }
    fn cancel_text_segmentation(&mut self) {
        if let Some(worker) = &self.text_segmenter {
            worker.submit(None);
        }
    }
    fn request_point_sample(&mut self, request: crate::api::point_sample::Request) -> bool {
        if self.point_sampler.is_none() {
            let tx = self.event_tx.clone();
            self.point_sampler = crate::platform::common::point_sample::Worker::new(
                || {
                    let mut sampler = vision::PointSampler::default();
                    move |request| sampler.sample(request)
                },
                move |sample| {
                    let _ = tx.send(BackendEvent::PointSampled(sample));
                },
            )
            .ok();
        }
        if let Some(worker) = &self.point_sampler {
            worker.submit(Some(request));
            true
        } else {
            false
        }
    }
    fn cancel_point_sample(&mut self) {
        if let Some(worker) = &self.point_sampler {
            worker.submit(None);
        }
    }
    fn read_clipboard(&mut self) -> Result<String, String> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.get_text())
            .map_err(|error| error.to_string())
    }
    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text))
            .map_err(|e| e.to_string())
    }
    fn event_sink(&self) -> Option<Arc<dyn Fn(BackendEvent) + Send + Sync>> {
        let sender = self.event_tx.clone();
        Some(Arc::new(move |event| {
            let _ = sender.send(event);
        }))
    }

    fn request_focused_window_bounds(&mut self, id: u64, process: u32) -> Result<(), String> {
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
            .focused_bounds(id, process)
    }
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
    fn release_text_prompt(&mut self) {
        if let Some(item) = &self.status_item {
            item.release_text_prompt();
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
    fn clear_window_overlap_cache(&mut self) {
        if let Some(worker) = &self.window_worker {
            worker.clear_overlap_cache();
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
    fn run_event_loop(
        &mut self,
        turn: &mut crate::api::backend::EventLoopTurn<'_>,
    ) -> Option<Result<(), String>> {
        let menu = self.status_item.as_ref().map(status_item::StatusItem::menu);
        self.native_loop_running = true;
        let result = event_loop::run(menu.as_deref(), &mut || {
            let next = turn(self)?;
            Ok(next.map(|timeout| self.native_timeout(timeout)))
        });
        self.native_loop_running = false;
        Some(result)
    }

    fn poll(&mut self, timeout: Duration) -> Result<Option<BackendEvent>, String> {
        // Engine has loaded its environment and activated Idle before first poll.
        if !self.update_ready_signaled {
            update_installer::signal_ready()?;
            self.update_ready_signaled = true;
        }
        self.reap_update_worker();
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Some(event) = self.try_event() {
                return Ok(Some(event));
            }
            crate::support::worker::reap_quarantined();
            // Native dispatch must happen outside the engine/backend borrow.
            // The owning loop also runs turns inside AppKit menu tracking.
            if !self.native_loop_running {
                event_loop::pump_app_events();
            }
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
            let remaining = deadline.map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() || self.native_loop_running {
                return Ok(None);
            }
            let remaining = self.native_timeout(remaining);
            if self.frame_clock.is_running() {
                if let Some(elapsed) = self.frame_clock.next(remaining) {
                    return Ok(Some(BackendEvent::Frame(elapsed)));
                }
            } else {
                event_loop::wait_for_app_event(remaining);
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

    fn set_text_capture(&mut self, enabled: bool) {
        if let Some(hook) = self.hook.as_ref() {
            hook.set_text_capture(enabled);
        }
    }

    fn set_character_bindings(&mut self, keys: &[crate::api::Key]) {
        if let Some(hook) = self.hook.as_ref() {
            hook.set_character_bindings(keys);
        }
    }

    fn screens(&self) -> Result<Vec<Screen>, String> {
        if self.screens.is_empty() {
            display::list_screens()
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
        self.move_window_from_to_screen(crate::api::window::WindowTarget::Mouse, target)
    }

    fn move_window_from_to_screen(
        &self,
        source: crate::api::window::WindowTarget,
        target: crate::api::command::WindowScreenTarget,
    ) -> Result<Option<Point>, String> {
        let mut pending = self.window_move.borrow_mut();
        if pending.is_some() {
            return Ok(None);
        }
        let cursor = self.pointer()?;
        let selected = source.with_fallback(|source| match source {
            crate::api::window::WindowTarget::Mouse => accessibility::window_under_pointer(cursor),
            crate::api::window::WindowTarget::Active => accessibility::movable_focused_window(),
        })?;
        let Some(window) = selected else {
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

    fn scroll_frame(&self, frame: crate::api::scroll::ScrollFrame) -> Result<(), String> {
        self.scroll_worker.submit(frame)
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

    fn send_chord_suspending(&self, keys: &[Key], modifiers: &[Key]) -> Result<(), String> {
        self.keyboard.send_chord_suspending(keys, modifiers)
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
        // A ready frame can precede the ordinary native-event refresh. Consume
        // display changes here too, but never enumerate displays per frame.
        self.refresh_screens()?;
        self.overlay.present(scene, &self.screens)?;
        if let Ok(source) = self.overlay.display_link_source() {
            self.frame_clock.prepare(source);
        } else {
            self.frame_clock.stop();
        }
        crate::support::perf_probe::mark("native_presented");
        Ok(())
    }

    fn update_overlay_positions(
        &mut self,
        cursor: Option<Point>,
        indicator: Option<Point>,
    ) -> Result<bool, String> {
        self.refresh_screens()?;
        let updated = self.overlay.update_positions(cursor, indicator)?;
        if updated {
            if self.frame_clock.is_running() {
                self.frame_clock
                    .prepare(self.overlay.display_link_source()?);
            }
            crate::support::perf_probe::mark("native_presented");
        }
        Ok(updated)
    }

    fn dismiss(&mut self) -> Result<(), String> {
        self.frame_clock.stop();
        self.overlay.dismiss()
    }

    fn request_ui_scan(&mut self, request: crate::api::UiScanRequest) -> Result<(), String> {
        let generation = self.scan_mailbox.begin(request.id);
        let preview_generation = self.scan_previews.begin(request.id);
        self.preview_view.begin(&request);
        let fusion = match self.scan_fusion.prepare(generation) {
            Ok(fusion) => fusion,
            Err(error) => {
                self.scan_mailbox.cancel(request.id);
                self.scan_previews.cancel(request.id);
                self.preview_view.cancel(request.id);
                return Err(error);
            }
        };
        self.scan_worker.request_scan(ui_scan::ScanJob {
            request,
            generation,
            mailbox: Arc::clone(&self.scan_mailbox),
            preview_mailbox: Arc::clone(&self.scan_previews),
            preview_generation,
            wake: self.event_tx.clone(),
            fusion,
        });
        Ok(())
    }

    fn cancel_ui_scan(&mut self, id: u64) -> Result<(), String> {
        self.cancel_scan(id);
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

    fn stop_input_capture(&mut self) {
        if let Some(hook) = self.hook.as_mut() {
            hook.request_stop();
        }
    }

    fn shutdown(&mut self) -> Result<(), String> {
        self.shutdown_attempted = true;
        self.shutdown_resources()
    }
}
