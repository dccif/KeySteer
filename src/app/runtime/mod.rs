//! The engine: a mode-agnostic event router and binding resolver.
//!
//! The engine owns the mode registry and, for each mode, its binding table. It
//! knows nothing about what any mode *does* — it converts backend events into
//! [`ModeEvent`]s, hands them to the active mode, and executes the returned
//! [`Command`]s against the backend.
//!
//! Binding resolution lives here rather than in each mode, so every mode —
//! built-in or plugin — gets the same treatment: the engine looks the key up in
//! the active mode's table, handles the host-level verbs itself (mode switches,
//! synthetic keystrokes, `exec`, `quit`) and forwards the rest to the mode as a
//! [`ModeEvent::Binding`].

mod config_handoff;
mod input_router;
mod input_state;
mod key_help;
mod overlay_coordinator;
mod plan;
mod prefix_chords;
mod registry;
mod scheduler;
mod window_presets;
pub(crate) use window_presets::LayoutRepository;

pub use plan::{
    AppRouteOverride, ConfigurationCandidate, ConfigurationRepository, DebugSettings,
    EngineSettings, ModeRoute, ModeSpec, PaletteSet, RuntimePlan,
};

#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use smallvec::SmallVec;

use crate::api::Palette;
use crate::api::backend::{Appearance, Backend, BackendEvent, KeyDisposition};
use crate::api::binding::{Binding, Button, DEFAULT_WAIT_MS, InputTarget};
use crate::api::command::{
    ButtonAction, Command, FinishCause, FocusedApp, HostContext, ModeEvent, MouseButton,
    UiScanRequest, UiScanStatus,
};
use crate::api::geometry::{Point, Screen};
use crate::api::input::{Key, KeyChord, KeyState, ModeId};
use crate::api::overlay::{CursorMarker, Indicator, OverlayScene};
use input_state::*;
use overlay_coordinator::*;
use plan::{Bindings, ModeInstance};
use registry::ModeRegistry;
use scheduler::*;

/// Internal classification for failures crossing the Engine loop. Public APIs
/// keep their string errors, while recovery decisions never parse display text.
#[derive(Debug)]
enum RuntimeError {
    RecoverableInput(String),
    Platform(String),
    Fatal(String),
}

impl RuntimeError {
    fn message(&self) -> &str {
        match self {
            Self::RecoverableInput(message) | Self::Platform(message) | Self::Fatal(message) => {
                message
            }
        }
    }

    fn into_message(self) -> String {
        match self {
            Self::RecoverableInput(message) | Self::Platform(message) | Self::Fatal(message) => {
                message
            }
        }
    }
}
fn chords_conflict(left: &KeyChord, right: &KeyChord) -> bool {
    left.activation_matches(right.activation_key())
        || right.activation_matches(left.activation_key())
}

pub struct Engine {
    settings: EngineSettings,
    palettes: PaletteSet,
    palette: Palette,
    appearance: Appearance,

    registry: ModeRegistry,

    ui_hint_overlap_chord: Option<KeyChord>,

    screens: Vec<Screen>,
    cursor: Point,
    focused_app: Option<FocusedApp>,
    /// Cached because this guard is checked for every physical key edge.
    focused_app_excluded: bool,

    /// The OS must see matching down/up halves even if a mode switch happens
    /// between them (for example Ctrl down in idle, then Ctrl+E enters normal).
    /// Held bindings currently in effect, keyed by the key that started them.
    ///
    /// A release cannot be resolved by looking the chord up again: the key (and
    /// possibly its modifiers) are no longer held, so the chord would not match.
    /// Remembering the binding is what guarantees every press is followed by
    /// its release, rather than movement sticking on forever.
    /// Direct clicks awaiting short/long-press resolution, or completed atomic
    /// clicks whose physical activation keys remain down. This drives cursor
    /// color only and does not represent mouse-button state.
    /// Synthetic keyboard keys and mouse buttons held by `press` or `toggle`.
    /// Keeping one shared set makes these actions idempotent and lets the UI
    /// report exactly what the engine is responsible for releasing.
    /// Physically held activation keys for parameterless `toggle`. The value is
    /// true once a companion target was toggled during this press. Releasing an
    /// unused activation key clears every currently latched input instead.
    input: InputState,
    /// Click keys and parameterless toggle activations waiting to cross the
    /// configured hold threshold. A short click completes on key-up; firing
    /// delegates only the held target to the latched-input toggle state machine.
    /// Optional modifier-assisted drag release. This is separate from
    /// `latched` so explicit press/toggle actions keep their existing manual
    /// lifetime.
    scan_owners: HashMap<u64, ModeId>,
    /// Mode that owns native display frames. This can differ from `active`
    /// when grid or hint mode inherits normal-mode movement bindings.
    scheduler: Scheduler,

    /// Last scene presented, so redundant presents can be skipped.
    /// Mode-owned scene before cursor decorations are added.
    /// Avoid retrying a rejected native position-only update on every pointer
    /// event. A fresh visible overlay session gets one new attempt.
    overlay: OverlayCoordinator,

    enabled: bool,
    /// Suppress repeated reports while the focused window rejects a stream of
    /// synthetic input (most commonly Windows UIPI on an elevated window).
    input_failure_active: bool,
    pending_runtime_error: Option<RuntimeError>,
    should_quit: bool,
    configuration: Option<Box<dyn ConfigurationRepository>>,
    window_layouts: window_presets::LayoutController,
    /// Prevent rapid status-menu clicks from opening duplicate browser tabs.
    last_config_simulator_open: Option<Instant>,
    started_at: Instant,
}

impl Engine {
    fn empty(plan: RuntimePlan, appearance: Appearance) -> Result<Self, String> {
        Self::assemble(plan, appearance, true)
    }

    fn assemble(
        plan: RuntimePlan,
        appearance: Appearance,
        register_catalog: bool,
    ) -> Result<Self, String> {
        let RuntimePlan {
            settings,
            palettes,
            modes,
        } = plan;
        let routes = modes
            .iter()
            .map(|spec| (spec.id(), spec.route.clone()))
            .collect();
        crate::support::logging::set_non_error_enabled(settings.debug.enabled);
        let palette = palettes.for_appearance(appearance);
        let mut engine = Self {
            settings,
            palettes,
            palette,
            appearance,
            registry: ModeRegistry::with_routes(routes),
            ui_hint_overlap_chord: None,
            screens: Vec::new(),
            cursor: Point::default(),
            focused_app: None,
            focused_app_excluded: false,
            input: InputState::default(),
            scan_owners: HashMap::new(),
            scheduler: Scheduler::default(),
            overlay: OverlayCoordinator::default(),
            enabled: true,
            input_failure_active: false,
            pending_runtime_error: None,
            should_quit: false,
            configuration: None,
            window_layouts: window_presets::LayoutController::default(),
            last_config_simulator_open: None,
            started_at: Instant::now(),
        };
        if register_catalog {
            for spec in modes {
                match spec.instance {
                    ModeInstance::BuiltIn(mode) => engine.register_deferred(mode),
                    ModeInstance::Plugin(plugin) => {
                        engine.register_plugin_dyn_deferred(plugin)?;
                    }
                }
            }
        }
        Ok(engine)
    }

    pub fn from_plan(plan: RuntimePlan, appearance: Appearance) -> Result<Self, String> {
        Self::empty(plan, appearance)
    }

    #[cfg(test)]
    pub(crate) fn from_plan_without_modes(
        plan: RuntimePlan,
        appearance: Appearance,
    ) -> Result<Self, String> {
        Self::assemble(plan, appearance, false)
    }

    pub fn attach_configuration(&mut self, repository: Box<dyn ConfigurationRepository>) {
        self.configuration = Some(repository);
    }

    pub fn configuration_source_path(&self) -> Option<std::path::PathBuf> {
        self.configuration
            .as_ref()
            .and_then(|repository| repository.source_path())
    }

    fn recoverable_input_error(&mut self, action: &str, error: String) -> String {
        let error = RuntimeError::RecoverableInput(format!("{action}: {error}"));
        let message = error.message().to_owned();
        // Keep the first failure while rollback runs. A rollback can itself
        // inject or release input; replacing this value would lose the error
        // that is actually propagated to the Engine loop.
        if self.pending_runtime_error.is_none() {
            self.pending_runtime_error = Some(error);
        }
        message
    }

    fn cancel_scans_for_owner(
        &mut self,
        owner: &ModeId,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let ids = self
            .scan_owners
            .iter()
            .filter_map(|(&id, candidate)| (candidate == owner).then_some(id))
            .collect::<SmallVec<[u64; 1]>>();
        let mut errors = crate::support::errors::ErrorBundle::default();
        for id in ids {
            self.scan_owners.remove(&id);
            if let Err(error) = backend.cancel_ui_scan(id) {
                errors.push(format!("cancel UI scan {id}"), error);
            }
        }
        errors.into_result()
    }

    fn cancel_all_scans(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let ids = self
            .scan_owners
            .keys()
            .copied()
            .collect::<SmallVec<[u64; 2]>>();
        self.scan_owners.clear();
        let mut errors = crate::support::errors::ErrorBundle::default();
        for id in ids {
            if let Err(error) = backend.cancel_ui_scan(id) {
                errors.push(format!("cancel UI scan {id}"), error);
            }
        }
        errors.into_result()
    }

    fn recover_from_input_error(&mut self, error: &str, backend: &mut dyn Backend) -> bool {
        let Some(RuntimeError::RecoverableInput(message)) = self.pending_runtime_error.take()
        else {
            return false;
        };
        if message != error {
            return false;
        }
        self.reset_runtime_input_state(&message, false, backend);
        true
    }

    fn reset_runtime_input_state(
        &mut self,
        message: &str,
        capture_lost: bool,
        backend: &mut dyn Backend,
    ) {
        if !self.input_failure_active {
            crate::support::logging::report_error(
                "input",
                if capture_lost {
                    format!(
                        "{message}; stale physical input was discarded and runtime input state was reset"
                    )
                } else {
                    format!("{message}; this action was rejected and runtime input state was reset")
                },
            );
            self.input_failure_active = true;
        }

        if capture_lost {
            // The native capture source has stopped, so the matching physical
            // key-up edges can never arrive. This is intentionally not done
            // for ordinary injection failure while the hook remains alive.
            self.input.forget_physical_capture();
        }

        let previous = self.registry.active.clone();
        if let Err(cancel_error) = self.cancel_transient_clicks(backend) {
            crate::support::logging::report_error(
                "input",
                format!("cannot cancel pending mouse presses during recovery: {cancel_error}"),
            );
        }
        self.input.reset_for_plan_swap();
        self.registry.modal_stack.clear();
        for session in std::mem::take(&mut self.scheduler.window_sessions).into_keys() {
            backend.cancel_window_session(session);
        }
        self.scheduler.reset();
        if let Err(cancel_error) = self.cancel_all_scans(backend) {
            crate::support::logging::report_error(
                "ui-scan",
                format!("cannot cancel scans during input recovery: {cancel_error}"),
            );
        }
        if let Err(clock_error) = backend.set_frame_clock(false) {
            self.trace_lazy(self.settings.debug.backend, "backend", || {
                format!("cannot stop frame clock during input recovery: {clock_error}")
            });
        }

        if let Err(release_error) = self.release_latched(backend) {
            crate::support::logging::report_error(
                "input",
                format!("cannot release every held input during recovery: {release_error}"),
            );
        }

        if previous != ModeId::idle() {
            let context = HostContext {
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: self.focused_app.as_ref(),
                palette: &self.palette,
            };
            if let Some(mode) = self.registry.get_mut(&previous) {
                // Let the mode clear its private session state, but discard
                // commands: the recovery path must not inject more input.
                let _ = mode.handle(&ModeEvent::Deactivated, &context);
            }
            self.set_active(ModeId::idle());
            let context = HostContext {
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: self.focused_app.as_ref(),
                palette: &self.palette,
            };
            if let Some(idle) = self.registry.get_mut(&ModeId::idle()) {
                let _ = idle.handle(
                    &ModeEvent::Activated {
                        previous: Some(previous),
                    },
                    &context,
                );
            }
        }

        if let Err(dismiss_error) = backend.dismiss() {
            crate::support::logging::report_error(
                "overlay",
                format!("cannot dismiss overlay during input recovery: {dismiss_error}"),
            );
        }
        // Keep the logical state clean even if the native window was already
        // unavailable. A later scene must be rebuilt from scratch.
        self.overlay.content = None;
        self.overlay.last_scene = None;
        self.overlay.visible = false;
    }

    fn recoverable_input_succeeded(&mut self) {
        self.input_failure_active = false;
    }

    fn report_action_error(&mut self, error: String, backend: &mut dyn Backend) {
        if !self.recover_from_input_error(&error, backend) {
            crate::support::logging::report_error("action", format!("action failed: {error}"));
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn trace(&self, category_enabled: bool, category: &str, message: impl AsRef<str>) {
        if self.settings.debug.enabled && category_enabled {
            let message = format!(
                "+{:>7.2}ms {}",
                self.started_at.elapsed().as_secs_f64() * 1000.0,
                message.as_ref()
            );
            crate::support::logging::debug_args(category, format_args!("{message}"));
        }
    }

    fn trace_lazy(&self, category_enabled: bool, category: &str, message: impl FnOnce() -> String) {
        if self.settings.debug.enabled && category_enabled {
            self.trace(true, category, message());
        }
    }

    fn start_runtime(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if let Err(error) = backend.start() {
            if let Err(shutdown_error) = backend.shutdown() {
                crate::support::logging::report_error(
                    "backend",
                    format!("shutdown after startup failure also failed: {shutdown_error}"),
                );
            }
            return Err(RuntimeError::Platform(error).into_message());
        }
        crate::support::logging::info_args(
            "backend",
            format_args!("{} backend started", backend.name()),
        );
        crate::support::perf_probe::mark("backend_started");
        self.trace_lazy(self.settings.debug.backend, "backend", || {
            format!("started {} backend", backend.name())
        });

        // Say up front when the keyboard cannot be read. Every mode depends on
        // it, so without this the program looks like it is doing nothing for no
        // reason.
        if !backend.keyboard_available() {
            let reason = backend
                .keyboard_unavailable_reason()
                .unwrap_or_else(|| "the keyboard cannot be observed".to_string());
            crate::support::logging::report_error("keyboard", reason);
        }

        self.appearance = backend.appearance();
        self.palette = self.palettes.for_appearance(self.appearance);
        self.screens = backend.screens().unwrap_or_else(|error| {
            crate::support::logging::report_error(
                "backend",
                format!("cannot read screens: {error}"),
            );
            Vec::new()
        });
        self.cursor = backend.pointer().unwrap_or_else(|error| {
            crate::support::logging::report_error(
                "backend",
                format!("cannot read pointer: {error}"),
            );
            Point::default()
        });
        self.focused_app = backend.focused_app().unwrap_or_else(|error| {
            crate::support::logging::report_error(
                "backend",
                format!("cannot read focused application: {error}"),
            );
            None
        });
        self.focused_app_excluded = self.excluded_app_matches(self.focused_app.as_ref());
        crate::support::logging::info_args(
            "backend",
            format_args!(
                "initial state screens={} appearance={:?} keyboard_available={}",
                self.screens.len(),
                self.appearance,
                backend.keyboard_available()
            ),
        );
        // Bootstrap defers registration compilation until the backend can
        // provide its initial app profile. This is the only startup build.
        self.rebuild_tables();
        self.sync_character_bindings(backend);
        crate::support::perf_probe::mark("engine_ready");
        self.trace_lazy(self.settings.debug.backend, "backend", || {
            format!(
                "screens={} cursor=({:.1},{:.1}) focused_app={:?}",
                self.screens.len(),
                self.cursor.x,
                self.cursor.y,
                self.focused_app
            )
        });
        self.trace_binding_tables();

        // Enter the initial mode so it can arm timers and draw. Once start has
        // succeeded, even this early error must pass through native shutdown.
        if let Err(error) = self.activate(ModeId::idle(), None, backend) {
            if let Err(shutdown_error) = backend.shutdown() {
                crate::support::logging::report_error(
                    "backend",
                    format!("shutdown after activation failure also failed: {shutdown_error}"),
                );
            }
            return Err(error);
        }

        Ok(())
    }

    fn run_runtime_turn(
        &mut self,
        backend: &mut dyn Backend,
        timeout: Duration,
    ) -> Result<(), String> {
        if let Some(event) = backend.poll(timeout)? {
            let event_result = self.handle_backend_event(event, backend);
            if let Err(error) = event_result
                && !self.recover_from_input_error(&error, backend)
            {
                return Err(error);
            }
        }
        let long_press_result = self.fire_due_long_press_toggles(backend);
        if let Err(error) = long_press_result
            && !self.recover_from_input_error(&error, backend)
        {
            return Err(error);
        }
        let drag_release_result = self.fire_due_drag_auto_release(backend);
        if let Err(error) = drag_release_result
            && !self.recover_from_input_error(&error, backend)
        {
            return Err(error);
        }
        let timer_result = self.fire_due_timers(backend);
        if let Err(error) = timer_result
            && !self.recover_from_input_error(&error, backend)
        {
            return Err(error);
        }
        let sequence_result = self.fire_due_sequences(backend);
        if let Err(error) = sequence_result
            && !self.recover_from_input_error(&error, backend)
        {
            return Err(error);
        }

        Ok(())
    }

    fn finish_runtime(
        &mut self,
        backend: &mut dyn Backend,
        result: Result<(), String>,
    ) -> Result<(), String> {
        let mut errors = crate::support::errors::ErrorBundle::default();
        errors.record("runtime", result);
        for session in std::mem::take(&mut self.scheduler.window_sessions).into_keys() {
            backend.cancel_window_session(session);
        }
        self.scheduler.sequences.clear();
        errors.record(
            "cancel pending mouse presses",
            self.cancel_transient_clicks(backend).map(|_| ()),
        );
        self.input.drag_auto_release.clear();
        errors.record("cancel scans", self.cancel_all_scans(backend));
        errors.record("release held inputs", self.release_latched(backend));
        errors.record("dismiss overlay", backend.dismiss());
        self.overlay.reset();
        let shutdown = backend.shutdown();
        let shutdown_succeeded = shutdown.is_ok();
        errors.record("backend shutdown", shutdown);
        if shutdown_succeeded {
            crate::support::perf_probe::mark("shutdown_complete");
            crate::support::logging::info_args(
                "backend",
                format_args!("{} backend stopped", backend.name()),
            );
        }
        errors.into_result()
    }

    /// Run until a backend event or a mode asks to quit.
    pub fn run(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        self.start_runtime(backend)?;

        let result = (|| {
            while !self.should_quit {
                self.run_runtime_turn(backend, self.next_timeout())?;
            }
            Ok(())
        })();

        self.finish_runtime(backend, result)
    }

    /// How long we may block before a timer needs servicing.
    fn handle_backend_event(
        &mut self,
        event: BackendEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match event {
            BackendEvent::TextPromptResult { id, value } => {
                self.finish_layout_note(id, value, backend)?
            }
            BackendEvent::WindowResult(result) => {
                if let Some(owner) = self.scheduler.window_sessions.get(&result.session).cloned() {
                    self.dispatch_owned_to(&owner, ModeEvent::WindowResult(result), backend)?;
                }
            }
            BackendEvent::Input(input) => {
                crate::support::perf_probe::mark("input_received");
                self.handle_key(input, backend)?;
            }
            BackendEvent::InputInjectionFailed(message) => {
                return Err(self.recoverable_input_error("asynchronous native input", message));
            }
            BackendEvent::InputCaptureLost(message) => {
                self.reset_runtime_input_state(&message, true, backend);
            }
            BackendEvent::WindowMoveCompleted(result) => match result {
                Ok(point) => {
                    let owner = self.registry.active.clone();
                    self.execute_for(&owner, [Command::warp_to(point)], backend)?;
                }
                Err(message) => crate::support::logging::report_error("window-move", message),
            },
            BackendEvent::PointerMoved(reported) => {
                let p = self
                    .constrain_absolute_pointer(reported)
                    .unwrap_or(self.cursor);
                let changed = self.cursor != p;
                self.cursor = p;
                if changed {
                    self.note_drag_pointer_moved();
                    self.trace_lazy(self.settings.debug.pointer, "pointer", || {
                        format!("position=({:.1},{:.1})", p.x, p.y)
                    });
                }
                let active_slot = self
                    .registry
                    .active_slot
                    .filter(|&index| self.registry.id_at(index) == Some(&self.registry.active))
                    .or_else(|| self.registry.index_of(&self.registry.active));
                if active_slot
                    .and_then(|index| self.registry.wants_pointer_events_at(index))
                    .unwrap_or(true)
                {
                    self.dispatch(ModeEvent::PointerMoved(p), backend)?;
                }
                if changed {
                    self.refresh_overlay_positions(backend)?;
                }
            }
            BackendEvent::Frame(elapsed) => {
                if let Some(owner) = self.scheduler.frame_clock_owner.clone() {
                    self.dispatch_to(&owner, ModeEvent::Frame { elapsed }, backend)?;
                }
            }
            BackendEvent::FocusChanged(app) => {
                // Duplicate native notifications are common. If process,
                // bundle and title are unchanged, the cached profile is still
                // valid without even walking the override list. A changed
                // title may affect substring overrides, so compare the exact
                // resolved profile before deciding whether to recompile.
                let profile_changed =
                    if same_binding_app_snapshot(self.focused_app.as_ref(), app.as_ref()) {
                        false
                    } else {
                        self.binding_profile_key_for(app.as_ref())
                            != self.registry.binding_profile_key
                    };
                self.focused_app = app.clone();
                self.focused_app_excluded = self.excluded_app_matches(self.focused_app.as_ref());
                if self.focused_app_excluded {
                    let click_feedback_changed = self.cancel_transient_clicks(backend)?;
                    let released_drag = self.release_drag_auto_release(backend)?;
                    if click_feedback_changed || released_drag {
                        self.refresh_overlay(backend)?;
                    }
                }
                if profile_changed {
                    self.rebuild_tables();
                    self.sync_character_bindings(backend);
                    self.trace_binding_tables();
                }
                self.dispatch(ModeEvent::FocusChanged(app), backend)?;
                if self.overlay.key_help_visible {
                    self.refresh_overlay(backend)?;
                }
            }
            BackendEvent::ScreensChanged(screens) => {
                self.screens = screens.clone();
                self.dispatch(ModeEvent::ScreensChanged(screens), backend)?;
            }
            BackendEvent::AppearanceChanged(appearance) => {
                self.appearance = appearance;
                self.palette = self.palettes.for_appearance(appearance);
                self.refresh_overlay(backend)?;
            }
            BackendEvent::UiScanned(result) => {
                crate::support::perf_probe::mark_value(
                    if result.status == UiScanStatus::Partial {
                        "ui_scan_partial"
                    } else {
                        "ui_scan_terminal"
                    },
                    isize::try_from(result.id).unwrap_or(isize::MAX),
                );
                match &result.status {
                    UiScanStatus::Failed(error) => crate::support::logging::report_error(
                        "ui-scan",
                        format!("scan {} failed: {error}", result.id),
                    ),
                    UiScanStatus::PermissionDenied(error) | UiScanStatus::Unsupported(error) => {
                        crate::support::logging::report_warning_args(
                            "ui-scan",
                            format_args!("scan {} unavailable: {error}", result.id),
                        );
                    }
                    UiScanStatus::TimedOut => crate::support::logging::report_warning_args(
                        "ui-scan",
                        format_args!("scan {} timed out", result.id),
                    ),
                    UiScanStatus::Partial
                    | UiScanStatus::Success
                    | UiScanStatus::ContextChanged => {}
                }
                let owner = if result.status == UiScanStatus::Partial {
                    self.scan_owners.get(&result.id).cloned()
                } else {
                    self.scan_owners.remove(&result.id)
                };
                if let Some(owner) = owner.filter(|owner| owner == &self.registry.active) {
                    self.dispatch_owned_to(&owner, ModeEvent::UiScanned(result), backend)?;
                }
            }
            BackendEvent::ReloadConfig => {
                if let Err(error) = self.reload_config(backend) {
                    crate::support::logging::report_error("config", error);
                }
            }
            BackendEvent::OpenConfigSimulator => {
                let now = Instant::now();
                if self.last_config_simulator_open.is_some_and(|previous| {
                    now.saturating_duration_since(previous) < config_handoff::OPEN_DEBOUNCE
                }) {
                    return Ok(());
                }
                let source = self
                    .configuration
                    .as_ref()
                    .ok_or_else(|| "no configuration source is attached".to_string())?
                    .source_text()?;
                let layouts = self.window_layouts.store.export_file();
                if let Err(error) = &layouts {
                    crate::report_error!("window-layouts", "{error}");
                }
                let url = config_handoff::url_for_workspace(&source, layouts);
                if let Err(error) = backend.open_url(&url) {
                    crate::support::logging::report_error("config-simulator", error);
                } else {
                    self.last_config_simulator_open = Some(now);
                }
            }
            BackendEvent::ToggleEnabled => {
                self.enabled = !self.enabled;
                backend.set_enabled(self.enabled)?;
                if !self.enabled {
                    self.cancel_all_scans(backend)?;
                    self.scheduler.sequences.clear();
                    let _ = self.cancel_transient_clicks(backend)?;
                    self.input.drag_auto_release.clear();
                    self.release_latched(backend)?;
                    self.activate(ModeId::idle(), None, backend)?;
                    self.hide_overlay(backend)?;
                }
            }
            BackendEvent::ToggleAutostart => match backend.toggle_autostart() {
                Ok(enabled) => crate::log_info!(
                    "autostart",
                    "login-time startup {}",
                    if enabled { "enabled" } else { "disabled" }
                ),
                Err(error) => crate::support::logging::report_error("autostart", error),
            },
            BackendEvent::CheckForUpdates => {
                if let Err(error) = backend.check_for_updates() {
                    crate::support::logging::report_error("update-check", error);
                }
            }
            BackendEvent::UpdateProgress(progress) => {
                if let Err(error) = backend.present_update_progress(&progress) {
                    crate::support::logging::report_error("update-check", error);
                }
            }
            BackendEvent::UpdateChecked(result) => {
                if let Err(error) = backend.present_update_result(&result) {
                    crate::support::logging::report_error("update-check", error);
                }
            }
            BackendEvent::Quit => self.should_quit = true,
            BackendEvent::Warning(message) => {
                crate::report_warning!("backend", "{message}")
            }
        }
        Ok(())
    }

    fn reload_config(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        // The adapter parses, validates and compiles into a detached candidate.
        // Nothing owned by the active runtime is touched on failure.
        let candidate = self
            .configuration
            .as_ref()
            .ok_or_else(|| "no configuration source is attached".to_string())?
            .reload_candidate()
            .map_err(|error| {
                format!(
                    "configuration reload rejected; keeping the last valid configuration: {error}"
                )
            })?;
        let ConfigurationCandidate {
            plan,
            repository,
            source_path,
        } = candidate;
        self.apply_runtime_plan(plan, backend)?;
        self.configuration = Some(repository);
        let discovered_path = source_path;
        if let Some(path) = discovered_path {
            crate::log_info!(
                "config",
                "configuration reloaded successfully from {}",
                path.display()
            );
        } else {
            crate::log_info!(
                "config",
                "no configuration file found during reload; using built-in defaults"
            );
        }
        Ok(())
    }

    /// Swap in a precompiled plan without restarting runtime-owned tasks.
    /// This is intended for deterministic tests and initial assembly only;
    /// interactive reloads always use the controlled restart path.
    pub fn apply_plan(&mut self, plan: RuntimePlan) -> Result<(), String> {
        let RuntimePlan {
            settings,
            palettes,
            modes,
        } = plan;
        let routes = modes
            .iter()
            .map(|spec| (spec.id(), spec.route.clone()))
            .collect();
        crate::support::logging::set_non_error_enabled(settings.debug.enabled);
        self.input.drag_auto_release.clear();
        self.settings = settings;
        self.palettes = palettes;
        self.registry.routes = routes;
        self.focused_app_excluded = self.excluded_app_matches(self.focused_app.as_ref());
        self.palette = self.palettes.for_appearance(self.appearance);
        self.rebuild_tables();
        self.trace_binding_tables();
        Ok(())
    }

    fn apply_runtime_plan(
        &mut self,
        plan: RuntimePlan,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let RuntimePlan {
            settings,
            palettes,
            modes,
        } = plan;
        let routes = modes
            .iter()
            .map(|spec| (spec.id(), spec.route.clone()))
            .collect();

        // Keep physical pressed/disposition pairs until their real KeyUp, but
        // retire every task and every synthetic input owned by the old plan.
        let previous = self.registry.active.clone();
        let context = HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
        };
        if let Some(mode) = self.registry.get_mut(&previous) {
            let _ = mode.handle(&ModeEvent::Deactivated, &context);
        }
        for session in std::mem::take(&mut self.scheduler.window_sessions).into_keys() {
            backend.cancel_window_session(session);
        }
        self.scheduler.reset();
        self.registry.modal_stack.clear();
        self.input.reset_for_plan_swap();
        self.release_toggle_session_for_safe_mode(backend)?;
        self.cancel_all_scans(backend)?;
        backend.set_frame_clock(false)?;
        backend.dismiss()?;
        self.overlay.reset();

        self.settings = settings;
        self.palettes = palettes;
        self.palette = self.palettes.for_appearance(self.appearance);
        self.focused_app_excluded = self.excluded_app_matches(self.focused_app.as_ref());
        self.registry = ModeRegistry::with_routes(routes);
        crate::support::logging::set_non_error_enabled(self.settings.debug.enabled);
        for spec in modes {
            match spec.instance {
                ModeInstance::BuiltIn(mode) => self.register_deferred(mode),
                ModeInstance::Plugin(plugin) => {
                    self.register_plugin_dyn_deferred(plugin)?;
                }
            }
        }
        self.rebuild_tables();
        self.trace_binding_tables();
        self.set_active(ModeId::idle());
        self.sync_character_bindings(backend);
        self.dispatch(
            ModeEvent::Activated {
                previous: Some(previous),
            },
            backend,
        )
    }

    fn activate(
        &mut self,
        target: ModeId,
        previous: Option<ModeId>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if !self.registry.contains_key(&target) {
            self.trace_lazy(self.settings.debug.modes, "mode", || {
                format!(
                    "ignored switch {} -> {target}: target is not registered",
                    self.registry.active
                )
            });
            return Ok(());
        }

        if self.registry.active == ModeId::normal()
            && target != ModeId::normal()
            && !Self::releases_toggle_session_on_entry(&target)
        {
            let _ = self.release_drag_auto_release(backend)?;
        }
        if target != self.registry.active && Self::releases_toggle_session_on_entry(&target) {
            self.release_toggle_session_for_safe_mode(backend)?;
        }

        self.trace_lazy(self.settings.debug.modes, "mode", || {
            format!(
                "switch {} -> {target}, previous={previous:?}",
                self.registry.active
            )
        });

        if target != self.registry.active {
            self.scheduler.sequences.clear();
            self.input.active_default_toggles.clear();
            // Toggle state may cross temporary targeting modes, but entry into
            // Normal or Idle released it above. Physical held gestures remain
            // owned by the outgoing mode and need a synthetic release here.
            // Deliver the release of anything still held, so the outgoing mode
            // can stop its timers rather than moving the pointer forever.
            let pending: Vec<(Key, ActiveGesture)> =
                std::mem::take(&mut self.input.active_gestures)
                    .into_iter()
                    .collect();
            for (key, gesture) in pending {
                self.dispatch_to(
                    &gesture.owner,
                    ModeEvent::Binding {
                        binding: Arc::clone(&gesture.binding),
                        state: KeyState::Up,
                        key,
                    },
                    backend,
                )?;
            }

            // Tear down the outgoing mode and drop the timers it owned.
            let context = HostContext {
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: self.focused_app.as_ref(),
                palette: &self.palette,
            };
            let active = self.registry.active.clone();
            if let Some(old) = self.registry.get_mut(&active) {
                let commands = old.handle(&ModeEvent::Deactivated, &context);
                let old_id = old.id();
                self.execute(commands, backend)?;
                self.cancel_scans_for_owner(&old_id, backend)?;
                self.scheduler.cancel_timers_for_owner(&old_id);
            }
            self.set_active(target);
        }

        self.dispatch(ModeEvent::Activated { previous }, backend)
    }
}

fn same_binding_app_snapshot(current: Option<&FocusedApp>, next: Option<&FocusedApp>) -> bool {
    match (current, next) {
        (Some(current), Some(next)) => {
            current.process_id == next.process_id
                && current.bundle_id.eq_ignore_ascii_case(&next.bundle_id)
                && current.window_title == next.window_title
        }
        (None, None) => true,
        _ => false,
    }
}

fn random_wait_ms(min_ms: u64, max_ms: u64) -> u64 {
    if min_ms >= max_ms {
        return min_ms;
    }
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut current = STATE.load(Ordering::Relaxed);
    if current == 0 {
        current = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |duration| duration.as_nanos() as u64 | 1);
    }
    loop {
        let next = current
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        match STATE.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return min_ms + next % (max_ms - min_ms + 1),
            Err(actual) => current = actual,
        }
    }
}

/// Map the public [`Binding`] button to the platform command button.
///
/// Two types exist because `Binding` is the user-facing vocabulary while
/// `MouseButton` also covers the extra buttons a backend may support.
fn map_button(button: crate::api::binding::Button) -> MouseButton {
    match button {
        crate::api::binding::Button::Left => MouseButton::Left,
        crate::api::binding::Button::Right => MouseButton::Right,
        crate::api::binding::Button::Middle => MouseButton::Middle,
        crate::api::binding::Button::X1 => MouseButton::X1,
        crate::api::binding::Button::X2 => MouseButton::X2,
    }
}

const fn drag_button_bit(button: Button) -> u8 {
    match button {
        Button::Left => 1 << 0,
        Button::Right => 1 << 1,
        Button::Middle => 1 << 2,
        Button::X1 => 1 << 3,
        Button::X2 => 1 << 4,
    }
}

fn drag_modifier_bit(key: &Key) -> Option<u8> {
    let index = match key.as_str() {
        "left_shift" => 0,
        "right_shift" => 1,
        "left_ctrl" => 2,
        "right_ctrl" => 3,
        "left_alt" => 4,
        "right_alt" => 5,
        "left_win" => 6,
        "right_win" => 7,
        _ => return None,
    };
    Some(1 << index)
}

impl Engine {
    /// Keep an absolute pointer target on a real display, including layouts
    /// with negative origins or gaps between monitors.
    fn constrain_absolute_pointer(&self, requested: Point) -> Option<Point> {
        if !requested.x.is_finite() || !requested.y.is_finite() {
            return None;
        }
        if self
            .screens
            .iter()
            .any(|screen| screen.bounds.contains(&requested))
        {
            return Some(requested);
        }
        self.screens
            .iter()
            .map(|screen| clamp_to_screen(requested, screen))
            .min_by(|left, right| {
                requested
                    .distance_to(left)
                    .total_cmp(&requested.distance_to(right))
            })
    }

    /// Relative movement may cross directly into another display. If its target
    /// falls outside every display (an outer edge or a layout gap), clamp it to
    /// the current display without changing the held gesture or active mode.
    fn constrain_relative_pointer(&self, from: Point, requested: Point) -> Option<Point> {
        if !requested.x.is_finite() || !requested.y.is_finite() {
            return None;
        }
        if self
            .screens
            .iter()
            .any(|screen| screen.bounds.contains(&requested))
        {
            return Some(requested);
        }
        let current = self
            .screens
            .iter()
            .find(|screen| screen.bounds.contains(&from))
            .or_else(|| {
                self.screens.iter().min_by(|left, right| {
                    from.distance_to(&clamp_to_screen(from, left))
                        .total_cmp(&from.distance_to(&clamp_to_screen(from, right)))
                })
            })?;
        Some(clamp_to_screen(requested, current))
    }

    fn execute(
        &mut self,
        commands: impl IntoIterator<Item = Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let owner = self.registry.active.clone();
        self.execute_for(&owner, commands, backend)
    }

    fn execute_for(
        &mut self,
        owner: &ModeId,
        commands: impl IntoIterator<Item = Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.overlay.command_batch_depth += 1;
        let result = self.execute_commands(owner, commands, backend);
        self.overlay.command_batch_depth -= 1;

        if result.is_err() {
            self.overlay.pending = None;
            return result;
        }
        if self.overlay.command_batch_depth == 0 {
            self.flush_pending_overlay(backend)?;
        }
        Ok(())
    }

    fn execute_commands(
        &mut self,
        owner: &ModeId,
        commands: impl IntoIterator<Item = Command>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        for command in commands {
            let trace_command = if matches!(&command, Command::MovePointer { .. }) {
                self.settings.debug.motion
            } else {
                self.settings.debug.actions
            };
            self.trace_lazy(trace_command, "command", || {
                format!(
                    "owner={owner} active={} command={command:?}",
                    self.registry.active
                )
            });
            match command {
                Command::WindowLayouts(request) => {
                    self.request_window_layouts(owner, *request, backend)?
                }
                Command::WindowRequest(request) => {
                    self.scheduler
                        .window_sessions
                        .insert(request.session, owner.clone());
                    let (session, id) = (request.session, request.id);
                    let edit = match &request.operation {
                        crate::api::window::WindowOperation::BeginEdit { transaction, .. }
                        | crate::api::window::WindowOperation::ApplyLayout {
                            transaction, ..
                        }
                        | crate::api::window::WindowOperation::EndEdit { transaction, .. } => {
                            Some(Box::new(crate::api::window::WindowEditResult::Ended {
                                transaction: *transaction,
                                committed: false,
                            }))
                        }
                        _ => None,
                    };
                    if let Err(error) = backend.request_window(*request) {
                        if edit.is_some() {
                            backend.cancel_window_session(session);
                        }
                        crate::report_error!("window", "{error}");
                        self.dispatch_to(
                            owner,
                            ModeEvent::WindowResult(Box::new(crate::api::window::WindowResult {
                                closed: Vec::new(),
                                session,
                                id,
                                target: None,
                                windows: Some(Vec::new()),
                                pointer: None,
                                changed: 0,
                                skipped: 0,
                                message: Some(error),
                                edit,
                            })),
                            backend,
                        )?;
                    }
                }
                Command::CancelWindowSession(session) => {
                    if self
                        .window_layouts
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.session == session)
                        && let Some(prompt) = self.window_layouts.pending.take()
                    {
                        backend.cancel_text_prompt(prompt.id);
                    }
                    self.scheduler.window_sessions.remove(&session);
                    backend.cancel_window_session(session);
                }
                Command::DispatchActions(actions) => {
                    let input = crate::api::input::InputEvent {
                        character: None,
                        key: Key::new("plugin_action")?,
                        state: KeyState::Down,
                        repeat: false,
                        injected: true,
                        timestamp_millis: 0,
                    };
                    let resolved = ResolvedBinding {
                        binding: Arc::new(Binding::Sequence(actions)),
                        owner: owner.clone(),
                    };
                    self.apply_binding(resolved, &input, backend)?;
                }
                Command::MovePointer { dx, dy } => {
                    let requested = Point::new(self.cursor.x + dx, self.cursor.y + dy);
                    let Some(to) = self.constrain_relative_pointer(self.cursor, requested) else {
                        crate::report_warning!(
                            "pointer",
                            "ignoring non-finite or unavailable relative pointer target"
                        );
                        continue;
                    };
                    let actual_dx = to.x - self.cursor.x;
                    let actual_dy = to.y - self.cursor.y;
                    if actual_dx == 0.0 && actual_dy == 0.0 {
                        // Reaching an edge is not a gesture end. Keep the frame
                        // clock, pressed keys, acceleration and mode untouched;
                        // a later inward movement must work immediately.
                        continue;
                    }
                    if let Err(error) = backend.move_pointer(self.cursor, actual_dx, actual_dy) {
                        return Err(self.recoverable_input_error("pointer movement", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.settings.debug.motion, "backend", || {
                        format!(
                            "move_pointer requested=({dx:.3},{dy:.3}) actual=({actual_dx:.3},{actual_dy:.3}): ok"
                        )
                    });
                    // Synthetic movement is not guaranteed to re-enter the
                    // input hook. Store the constrained position actually sent.
                    self.cursor = to;
                    self.note_drag_pointer_moved();
                    self.refresh_overlay_positions(backend)?;
                }
                Command::WarpPointer { x, y } => {
                    let Some(to) = self.constrain_absolute_pointer(Point::new(x, y)) else {
                        crate::report_warning!(
                            "pointer",
                            "ignoring non-finite or unavailable absolute pointer target"
                        );
                        continue;
                    };
                    let changed = self.cursor != to;
                    if let Err(error) = backend.warp_pointer(to) {
                        return Err(self.recoverable_input_error("pointer warp", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.settings.debug.motion, "backend", || {
                        format!("warp_pointer x={:.3} y={:.3}: ok", to.x, to.y)
                    });
                    self.cursor = to;
                    if changed {
                        self.note_drag_pointer_moved();
                    }
                    self.refresh_overlay_positions(backend)?;
                }
                Command::MouseButton { button, action } => {
                    self.inject_mouse_button(button, action, backend)?;
                    if matches!(action, ButtonAction::Click | ButtonAction::DoubleClick) {
                        self.dispatch(ModeEvent::Clicked { button, action }, backend)?;
                    }
                }
                Command::FinishMode { cause } => {
                    self.cancel_scans_for_owner(owner, backend)?;
                    self.dispatch(ModeEvent::FinishRequested { cause }, backend)?;
                }
                Command::RestartMode => self.restart_active(backend)?,
                Command::Scroll { dx, dy } => {
                    let (invert_horizontal, invert_vertical) = self.settings.invert_scroll;
                    let dx = dx * if invert_horizontal { -1.0 } else { 1.0 };
                    let dy = dy * if invert_vertical { -1.0 } else { 1.0 };
                    if let Err(error) = backend.scroll(dx, dy) {
                        return Err(self.recoverable_input_error("scroll", error));
                    }
                    self.recoverable_input_succeeded();
                    self.trace_lazy(self.settings.debug.backend, "backend", || {
                        format!("scroll dx={dx:.3} dy={dy:.3}: ok")
                    });
                }
                Command::SetFrameClock(active) => {
                    self.scheduler.frame_clock_owner = active.then(|| owner.clone());
                    if let Err(error) = backend.set_frame_clock(active) {
                        self.scheduler.frame_clock_owner = None;
                        // A platform without a native display link retains
                        // keyboard-repeat movement as its compatibility path.
                        self.trace_lazy(self.settings.debug.backend, "backend", || {
                            format!("set_frame_clock active={active}: {error}")
                        });
                    }
                }

                Command::SetSpeedToggle { speed, active } => {
                    let selected = active.then_some(speed);
                    if self.overlay.speed_toggle != selected {
                        self.overlay.speed_toggle = selected;
                        self.refresh_overlay(backend)?;
                    }
                }

                Command::ShowOverlay(scene) => self.show_overlay(scene, backend)?,
                Command::HideOverlay => self.hide_overlay(backend)?,

                Command::SendKey { key, state } => {
                    if let Err(error) = backend.send_key(&key, state) {
                        return Err(self.recoverable_input_error("keyboard input", error));
                    }
                    crate::support::perf_probe::mark("injection_executed");
                    self.recoverable_input_succeeded();
                }
                Command::SendChord { keys } => {
                    if let Err(error) = backend.send_chord(&keys) {
                        self.input
                            .latched
                            .extend(keys.into_iter().map(InputTarget::Key));
                        return Err(self.recoverable_input_error("keyboard chord", error));
                    }
                    crate::support::perf_probe::mark("injection_executed");
                    self.recoverable_input_succeeded();
                }

                Command::ScanUi(request) => {
                    let request = *request;
                    let bounds = request
                        .bounds
                        .unwrap_or_else(|| self.context().active_bounds());
                    let roles = if request.roles.is_empty() {
                        self.settings.default_scan_roles.clone()
                    } else {
                        request.roles
                    };
                    let request = UiScanRequest {
                        bounds: Some(bounds),
                        roles,
                        ..request
                    };
                    let request_id = request.id;
                    crate::support::perf_probe::mark_value(
                        "scan_requested",
                        isize::try_from(request_id).unwrap_or(isize::MAX),
                    );
                    // A mode can only consume its latest scan generation.
                    // Cancel the superseded native job before publishing the
                    // new owner so providers cannot retain stale work.
                    self.cancel_scans_for_owner(owner, backend)?;
                    self.scan_owners.insert(request.id, owner.clone());
                    if let Err(error) = backend.request_ui_scan(request) {
                        self.scan_owners.remove(&request_id);
                        return Err(error);
                    }
                }

                Command::SwitchMode(id) => {
                    let previous = Some(self.registry.active.clone());
                    self.registry.modal_stack.clear();
                    self.activate(id, previous, backend)?;
                }
                Command::PushMode(id) => self.push_mode(id, backend)?,
                Command::PopMode => self.pop_mode(backend)?,
                Command::RetargetScreen { index, preserve } => {
                    let Some(screen) = self.screens.get(index).cloned() else {
                        crate::report_warning!(
                            "screen",
                            "screen {} does not exist ({} connected)",
                            index + 1,
                            self.screens.len()
                        );
                        continue;
                    };
                    self.dispatch(ModeEvent::ScreenRetargeted { screen, preserve }, backend)?;
                }

                Command::SetTimer {
                    id,
                    delay,
                    repeating,
                } => {
                    let now = Instant::now();
                    self.scheduler.timers.insert(
                        id.clone(),
                        Timer {
                            fires_at: now + delay,
                            last_fired: now,
                            interval: repeating.then_some(delay),
                            owner: owner.clone(),
                        },
                    );
                    self.trace_lazy(self.settings.debug.timers, "timer", || {
                        format!("set id={id:?} owner={owner} delay={delay:?} repeating={repeating}")
                    });
                }
                Command::CancelTimer { id } => {
                    self.scheduler.timers.remove(&id);
                    self.trace_lazy(self.settings.debug.timers, "timer", || {
                        format!("cancel id={id:?} owner={owner}")
                    });
                }

                Command::SetConfigValue { path, value } => {
                    let update = self
                        .configuration
                        .as_ref()
                        .ok_or_else(|| "no writable configuration source is attached".to_string())
                        .and_then(|repository| repository.set_candidate(&path, &value));
                    match update {
                        Ok(candidate) => {
                            let ConfigurationCandidate {
                                plan,
                                repository,
                                source_path: _,
                            } = candidate;
                            self.apply_runtime_plan(plan, backend)?;
                            self.configuration = Some(repository);
                        }
                        Err(error) => {
                            return Err(format!(
                                "set_config {path} rejected; keeping the last valid configuration: {error}"
                            ));
                        }
                    }
                }
                Command::ReloadConfig => self.reload_config(backend)?,

                Command::Exec { program, args } => {
                    std::process::Command::new(&program)
                        .args(&args)
                        .spawn()
                        .map_err(|error| format!("cannot run {program}: {error}"))?;
                }

                Command::Quit => self.should_quit = true,
                Command::MoveWindowToScreen(target) => {
                    if let Some(pointer) = backend.move_window_to_screen(target)? {
                        // Reuse the normal warp path so the authoritative
                        // cursor, overlay and drag state move together.
                        self.execute_for(owner, [Command::warp_to(pointer)], backend)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn inject_mouse_button(
        &mut self,
        button: MouseButton,
        action: ButtonAction,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if let Err(error) = backend.mouse_button(button, action) {
            return Err(
                self.recoverable_input_error(&format!("mouse button {button:?} {action:?}"), error)
            );
        }
        crate::support::perf_probe::mark("injection_executed");
        self.recoverable_input_succeeded();
        Ok(())
    }
}

fn clamp_to_screen(point: Point, screen: &Screen) -> Point {
    let bounds = screen.bounds;
    let unit = if screen.scale.is_finite() && screen.scale > 0.0 {
        1.0 / screen.scale
    } else {
        1.0
    };
    let max_x = (bounds.right() - unit).max(bounds.left());
    let max_y = (bounds.bottom() - unit).max(bounds.top());
    Point::new(
        point.x.clamp(bounds.left(), max_x),
        point.y.clamp(bounds.top(), max_y),
    )
}

impl Engine {
    fn flatten_sequence(&self, actions: &[Binding]) -> Vec<Binding> {
        fn append(binding: &Binding, flattened: &mut Vec<Binding>) {
            match binding {
                Binding::Sequence(nested) => {
                    for action in nested {
                        append(action, flattened);
                    }
                }
                action => flattened.push(action.clone()),
            }
        }

        let mut flattened = Vec::new();
        for action in actions {
            append(action, &mut flattened);
        }

        // Two identical key sends are a double tap. Give the focused app the
        // same default interval as an explicit `wait`/`wait 0`, while leaving
        // an explicitly configured wait untouched.
        let mut expanded = Vec::with_capacity(flattened.len());
        for action in flattened {
            if matches!(
                (expanded.last(), &action),
                (Some(Binding::Send(previous)), Binding::Send(current)) if previous == current
            ) {
                expanded.push(Binding::Wait {
                    min_ms: DEFAULT_WAIT_MS,
                    max_ms: DEFAULT_WAIT_MS,
                });
            }
            expanded.push(action);
        }
        expanded
    }

    fn continue_sequence(
        &mut self,
        mut actions: VecDeque<Binding>,
        owner: ModeId,
        mut input: crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        input.repeat = false;
        while let Some(action) = actions.pop_front() {
            if let Binding::Wait { min_ms, max_ms } = action {
                if actions.is_empty() {
                    return Ok(());
                }
                const MAX_PENDING_SEQUENCES: usize = 256;
                if self.scheduler.sequences.len() >= MAX_PENDING_SEQUENCES {
                    return Err("too many action sequences are waiting".into());
                }
                let delay = random_wait_ms(min_ms, max_ms);
                let pending = PendingSequence {
                    fires_at: Instant::now() + Duration::from_millis(delay),
                    actions,
                    owner,
                    input,
                };
                let index = self
                    .scheduler
                    .sequences
                    .partition_point(|current| current.fires_at > pending.fires_at);
                self.scheduler.sequences.insert(index, pending);
                return Ok(());
            }
            let nested = ResolvedBinding {
                binding: Arc::new(action),
                owner: owner.clone(),
            };
            self.apply_binding(nested, &input, backend)?;
        }
        Ok(())
    }

    /// Act on a resolved binding.
    ///
    /// Returns whether the key was consumed. Host-level verbs are executed
    /// here; everything else is forwarded to the mode as a
    /// [`ModeEvent::Binding`], which is what a plugin sees too.
    fn apply_binding(
        &mut self,
        resolved: ResolvedBinding,
        input: &crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        let binding = resolved.binding.as_ref();
        let is_press = input.state == KeyState::Down;
        self.trace_lazy(
            self.settings.debug.actions && (!input.repeat || self.settings.debug.motion),
            "action",
            || {
                format!(
                    "phase={:?} owner={} active={} action={binding:?}",
                    input.state, resolved.owner, self.registry.active
                )
            },
        );

        // Held bindings need both edges; the rest act on the press only.
        if !is_press && !binding.is_held() {
            // Still consume the release so the app never sees half a gesture.
            return Ok(true);
        }
        // Auto-repeat must not re-trigger a discrete action.
        if input.repeat && !binding.is_held() {
            return Ok(true);
        }

        // Stateful gestures and mode-specific discrete actions are owned by
        // the receiving mode. Transfer the resolved binding's Arc directly;
        // a held key already stored the one clone needed for its release edge.
        if matches!(
            binding,
            Binding::Move(_)
                | Binding::Scroll(..)
                | Binding::Speed(_)
                | Binding::SpeedToggle(_)
                | Binding::ToggleCursorFollowSelection
                | Binding::Window(_)
                | Binding::RescanUi
        ) {
            return self
                .dispatch_to(
                    &resolved.owner,
                    ModeEvent::Binding {
                        binding: resolved.binding,
                        state: input.state,
                        key: input.key.clone(),
                    },
                    backend,
                )
                .map(|_| true);
        }

        match binding {
            Binding::KeyHelp => {
                if self.registry.active != ModeId::idle() {
                    self.overlay.key_help_visible =
                        self.settings.key_help.enabled && !self.overlay.key_help_visible;
                    self.overlay.key_help_cache = None;
                    self.refresh_overlay(backend)?;
                }
                Ok(true)
            }
            Binding::Sequence(actions) => {
                let actions = self.flatten_sequence(actions);
                let has_held = actions.iter().any(Binding::is_held);
                if has_held
                    && actions
                        .iter()
                        .any(|action| matches!(action, Binding::Wait { .. }))
                {
                    return Err(
                        "`wait` cannot be combined with held movement, scroll, or speed actions"
                            .into(),
                    );
                }
                if has_held
                    && actions.iter().any(|action| {
                        matches!(
                            action,
                            Binding::Mode(_)
                                | Binding::Invoke { .. }
                                | Binding::FinishMode
                                | Binding::RestartMode
                                | Binding::Escape
                                | Binding::Quit
                        )
                    })
                {
                    return Err("held movement, scroll, or speed actions cannot be combined with mode-changing actions".into());
                }
                if is_press {
                    let actions = if input.repeat {
                        actions.into_iter().filter(Binding::is_held).collect()
                    } else {
                        actions
                    };
                    self.continue_sequence(
                        actions.into(),
                        resolved.owner.clone(),
                        input.clone(),
                        backend,
                    )?;
                } else {
                    // Stateful movement/scroll bindings still receive their
                    // release immediately; waits only order discrete actions.
                    for action in actions.into_iter().filter(Binding::is_held) {
                        let nested = ResolvedBinding {
                            binding: Arc::new(action),
                            owner: resolved.owner.clone(),
                        };
                        self.apply_binding(nested, input, backend)?;
                    }
                }
                Ok(true)
            }

            Binding::Mode(id) => {
                if !is_press {
                    return Ok(true);
                }
                if !self.registry.contains_key(id) {
                    crate::report_warning!(
                        "binding",
                        "binding targets unknown mode {:?}; is the plugin registered?",
                        id.as_str()
                    );
                    return Ok(true);
                }
                if *id == ModeId::window() {
                    if self.registry.active == *id {
                        self.dispatch(
                            ModeEvent::Binding {
                                binding: Arc::new(Binding::Window(
                                    crate::api::window::WindowAction::Exit,
                                )),
                                state: KeyState::Down,
                                key: input.key.clone(),
                            },
                            backend,
                        )?;
                    } else {
                        if let Ok(pointer) = backend.pointer() {
                            self.cursor = pointer;
                        }
                        self.push_mode(id.clone(), backend)?;
                    }
                    return Ok(true);
                }
                // Pressing a mode's own key while it is active leaves it.
                let next = if *id == self.registry.active {
                    ModeId::idle()
                } else {
                    id.clone()
                };
                // A coalesced pointer event can still be pending when the mode
                // hotkey arrives. Query the OS once so normal and every
                // targeting mode activate against the display actually under
                // the mouse rather than the last reported display.
                if let Ok(pointer) = backend.pointer()
                    && let Some(pointer) = self.constrain_absolute_pointer(pointer)
                {
                    self.cursor = pointer;
                }
                self.activate(next, Some(self.registry.active.clone()), backend)?;
                Ok(true)
            }

            Binding::Invoke { verb, args } => {
                if !is_press {
                    return Ok(true);
                }
                let Some(owner) = self.registry.plugin_verbs.get(verb).cloned() else {
                    crate::report_warning!("plugin", "no plugin exports verb {verb:?}");
                    return Ok(true);
                };
                self.dispatch_to(
                    &owner,
                    ModeEvent::Invoked {
                        verb: verb.clone(),
                        args: args.clone(),
                    },
                    backend,
                )?;
                Ok(true)
            }

            Binding::Escape => {
                if is_press {
                    self.scheduler.sequences.clear();
                    // `press`/`toggle` are explicit engine-wide latches, not
                    // mode-owned gestures. Escape changes mode but must not
                    // synthesize an Up edge for them.
                    self.activate(ModeId::idle(), Some(self.registry.active.clone()), backend)?;
                }
                Ok(true)
            }

            Binding::Quit => {
                self.should_quit = true;
                Ok(true)
            }

            Binding::Send(chord) => {
                self.send_chord(chord, backend)?;
                Ok(true)
            }

            Binding::Warp { x, y } => {
                self.execute(
                    [Command::WarpPointer {
                        x: *x as f64,
                        y: *y as f64,
                    }],
                    backend,
                )?;
                Ok(true)
            }

            Binding::Exec { program, args } => {
                std::process::Command::new(program)
                    .args(args)
                    .spawn()
                    .map_err(|error| format!("cannot run {program}: {error}"))?;
                Ok(true)
            }

            Binding::ReloadConfig => {
                self.reload_config(backend)?;
                Ok(true)
            }
            Binding::FinishMode => {
                self.execute(
                    [Command::FinishMode {
                        cause: FinishCause::Explicit,
                    }],
                    backend,
                )?;
                Ok(true)
            }
            Binding::RestartMode => {
                self.execute([Command::RestartMode], backend)?;
                Ok(true)
            }
            Binding::SetConfig { path, value } => {
                self.execute(
                    [Command::SetConfigValue {
                        path: path.clone(),
                        value: value.clone(),
                    }],
                    backend,
                )?;
                Ok(true)
            }

            Binding::Click(button) => {
                self.execute([Command::click(map_button(*button))], backend)?;
                self.activate_click_indicator(input, *button, backend)?;
                Ok(true)
            }
            Binding::DoubleClick(button) => {
                self.execute(
                    [Command::MouseButton {
                        button: map_button(*button),
                        action: ButtonAction::DoubleClick,
                    }],
                    backend,
                )?;
                self.activate_click_indicator(input, *button, backend)?;
                Ok(true)
            }
            Binding::Press(targets) => {
                self.transfer_pending_long_press_targets(targets);
                self.press_targets(targets, backend)?;
                self.refresh_overlay(backend)?;
                Ok(true)
            }
            Binding::Release(targets) => {
                self.transfer_pending_long_press_targets(targets);
                self.release_targets(targets, true, backend)?;
                self.refresh_overlay(backend)?;
                Ok(true)
            }
            Binding::Toggle(targets) => {
                if targets.is_empty() {
                    let inferred = self.pressed_toggle_targets(&input.key);
                    let used = !inferred.is_empty();
                    if used {
                        self.transfer_pending_long_press_targets(&inferred);
                        self.press_targets(&inferred, backend)?;
                        self.refresh_overlay(backend)?;
                    }
                    if self.input.pressed.contains(&input.key) {
                        self.input
                            .active_default_toggles
                            .insert(input.key.clone(), used);
                    }
                } else {
                    let toggle = self.unprimed_toggle_targets(targets);
                    self.toggle_targets(&toggle, backend)?;
                    self.refresh_overlay(backend)?;
                }
                Ok(true)
            }
            Binding::Wait { .. } => Ok(true),

            Binding::Move(_)
            | Binding::Scroll(..)
            | Binding::Speed(_)
            | Binding::SpeedToggle(_)
            | Binding::ToggleCursorFollowSelection
            | Binding::Window(_)
            | Binding::RescanUi => {
                Err("stateful binding reached the stateless runtime dispatch boundary".into())
            }

            // Filtered out when the table was built.
            Binding::Disabled => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests;
