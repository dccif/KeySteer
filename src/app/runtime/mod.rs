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

mod commands;
mod config_handoff;
mod configuration_work;
mod input_router;
mod input_state;
mod key_help;
mod overlay_coordinator;
mod plan;
mod prefix_chords;
mod quick_switch;
mod registry;
mod scheduler;
mod window_presets;
pub(crate) use window_presets::PresetRepository;

pub use plan::{
    AppRouteOverride, ConfigurationCandidate, ConfigurationRepository, DebugSettings,
    EngineSettings, ModeRoute, ModeSpec, PaletteSet, PreparedRestart, QuickSwitchSettings,
    RuntimePlan,
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

pub struct Engine {
    settings: EngineSettings,
    palettes: PaletteSet,
    palette: Palette,
    appearance: Appearance,

    registry: ModeRegistry,

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
    configuration_work: configuration_work::ConfigurationWork,
    window_presets: window_presets::PresetController,
    quick_switch: quick_switch::QuickSwitcher,
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
        // Resolve canonical injection aliases before any physical input arrives.
        let _ = Key::injection_modifiers();
        let RuntimePlan {
            settings,
            palettes,
            modes,
        } = plan;
        let (routes, instances): (std::collections::BTreeMap<_, _>, Vec<_>) = modes
            .into_iter()
            .map(|spec| ((spec.id(), spec.route), spec.instance))
            .unzip();
        crate::support::logging::set_non_error_enabled(settings.debug.enabled);
        let palette = palettes.for_appearance(appearance);
        let mut engine = Self {
            settings,
            palettes,
            palette,
            appearance,
            registry: ModeRegistry::with_routes(routes),
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
            configuration_work: Default::default(),
            window_presets: window_presets::PresetController::default(),
            quick_switch: Default::default(),
            last_config_simulator_open: None,
            started_at: Instant::now(),
        };
        if register_catalog {
            for instance in instances {
                match instance {
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
        if self
            .scheduler
            .text_segmentation
            .as_ref()
            .is_some_and(|(candidate, _, _)| candidate == owner)
        {
            self.scheduler.text_segmentation = None;
            backend.cancel_text_segmentation();
        }
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
        self.scheduler.text_segmentation = None;
        backend.cancel_text_segmentation();
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

    fn recover_from_input_error(&mut self, backend: &mut dyn Backend) -> bool {
        let Some(RuntimeError::RecoverableInput(message)) = self.pending_runtime_error.take()
        else {
            return false;
        };
        self.reset_runtime_input_state(&message, false, backend);
        true
    }

    fn reset_runtime_input_state(
        &mut self,
        message: &str,
        capture_lost: bool,
        backend: &mut dyn Backend,
    ) {
        self.cancel_quick_switch(capture_lost);
        self.retire_workspace_requests(None);
        let mut recovery_errors = crate::support::errors::ErrorBundle::default();

        if capture_lost {
            // The native capture source has stopped, so the matching physical
            // key-up edges can never arrive. This is intentionally not done
            // for ordinary injection failure while the hook remains alive.
            self.input.forget_physical_capture();
        }

        let previous = self.registry.active.clone();
        if let Err(cancel_error) = self.cancel_transient_clicks(backend) {
            recovery_errors.push("cancel pending mouse presses", cancel_error);
        }
        self.input.reset_for_plan_swap();
        if let Err(release_error) = self.release_latched(backend) {
            recovery_errors.push("release held inputs", release_error);
        }

        for owner in self.registry.modal_stack.clone() {
            let context = HostContext {
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: self.focused_app.as_ref(),
                palette: &self.palette,
            };
            if let Some(mode) = self.registry.get_mut(&owner) {
                let _ = mode.handle(&ModeEvent::Deactivated, &context);
            }
        }
        self.registry.modal_stack.clear();
        for session in std::mem::take(&mut self.scheduler.audio_sessions).into_keys() {
            backend.cancel_audio_session(session);
        }
        for session in std::mem::take(&mut self.scheduler.window_sessions).into_keys() {
            backend.cancel_window_session(session);
        }
        backend.set_text_capture(false);
        backend.release_text_prompt();
        self.clear_point_input(backend);
        self.scheduler.reset();
        if let Err(cancel_error) = self.cancel_all_scans(backend) {
            recovery_errors.push("cancel scans", cancel_error);
        }
        if let Err(clock_error) = backend.set_frame_clock(false) {
            recovery_errors.push("stop frame clock", clock_error);
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
            recovery_errors.push("dismiss overlay", dismiss_error);
        }
        // Keep the logical state clean even if the native window was already
        // unavailable. A later scene must be rebuilt from scratch.
        self.overlay.content = None;
        self.overlay.last_scene = None;
        self.overlay.visible = false;
        // Input and UI recovery must complete before any diagnostic I/O.
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

        if let Err(error) = recovery_errors.into_result() {
            crate::report_error!("input-recovery", "operation=cleanup: {error}");
        }
    }

    fn recoverable_input_succeeded(&mut self) {
        self.input_failure_active = false;
    }

    fn report_action_error(&mut self, error: String, backend: &mut dyn Backend) {
        if !self.recover_from_input_error(backend) {
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

    #[inline(always)]
    fn run_runtime_turn(
        &mut self,
        backend: &mut dyn Backend,
        timeout: Duration,
    ) -> Result<bool, String> {
        let event = backend.poll(timeout)?;
        let handled = event.is_some();
        if let Some(event) = event {
            let event_result = self.handle_backend_event(event, backend);
            if let Err(error) = event_result
                && !self.recover_from_input_error(backend)
            {
                return Err(error);
            }
        }
        // Most input events schedule no delayed work. Check the authoritative
        // state after dispatch (an event may just have armed a timer), before
        // entering the five independent recovery/dispatch paths below.
        if self.input.pending_long_press_toggles.is_empty()
            && self.quick_switch.pending.is_none()
            && self.input.drag_auto_release.fires_at.is_none()
            && self.scheduler.timers.is_empty()
            && self.scheduler.sequences.is_empty()
        {
            return Ok(handled);
        }
        self.run_scheduled_work(backend)?;
        Ok(handled)
    }

    fn run_ready_turns(&mut self, backend: &mut dyn Backend) -> Result<Option<Duration>, String> {
        // Bound each native callback so AppKit can keep tracking menus and
        // deliver display frames even under sustained input/background work.
        for _ in 0..64 {
            if self.should_quit {
                return Ok(None);
            }
            let handled = self.run_runtime_turn(backend, Duration::ZERO)?;
            if self.should_quit {
                return Ok(None);
            }
            if !handled {
                return Ok(Some(self.next_timeout()));
            }
        }
        Ok((!self.should_quit).then_some(Duration::ZERO))
    }

    // Keep deferred-work recovery out of the common input turn's instruction path.
    #[inline(never)]
    fn run_scheduled_work(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        // The event just handled may have requested exit. Leave deferred work
        // for finish_runtime to discard instead of running it before cleanup.
        if self.should_quit {
            return Ok(());
        }
        let long_press_result = self.fire_due_long_press_toggles(backend);
        self.fire_quick_switch(backend)?;
        if let Err(error) = long_press_result
            && !self.recover_from_input_error(backend)
        {
            return Err(error);
        }
        let drag_release_result = self.fire_due_drag_auto_release(backend);
        if let Err(error) = drag_release_result
            && !self.recover_from_input_error(backend)
        {
            return Err(error);
        }
        let timer_result = self.fire_due_timers(backend);
        if let Err(error) = timer_result
            && !self.recover_from_input_error(backend)
        {
            return Err(error);
        }
        let sequence_result = self.fire_due_sequences(backend);
        if let Err(error) = sequence_result
            && !self.recover_from_input_error(backend)
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
        backend.stop_input_capture();
        let mut errors = crate::support::errors::ErrorBundle::default();
        errors.record("runtime", result);
        self.scheduler.text_prompt = None;
        self.clear_point_input(backend);
        backend.set_text_capture(false);
        backend.release_text_prompt();
        for session in std::mem::take(&mut self.scheduler.audio_sessions).into_keys() {
            backend.cancel_audio_session(session);
        }
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
        errors.record("finish configuration", self.configuration_work.shutdown());
        errors.record("save workspace", self.window_presets.store.flush_usage());
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

        let mut finished = false;
        let result = match backend.run_event_loop(&mut |backend| {
            match self.run_ready_turns(backend) {
                Ok(Some(timeout)) => Ok(Some(timeout)),
                result => {
                    // AppKit's TerminateLater runs a nested modal loop until
                    // shutdown acknowledges termination. Cleanup must happen
                    // in this final turn, not after that loop returns.
                    finished = true;
                    self.finish_runtime(backend, result.map(|_| ()))
                        .map(|()| None)
                }
            }
        }) {
            Some(result) => result,
            None => (|| {
                while !self.should_quit {
                    self.run_runtime_turn(backend, self.next_timeout())?;
                }
                Ok(())
            })(),
        };

        if finished {
            result
        } else {
            self.finish_runtime(backend, result)
        }
    }

    pub(crate) fn take_restart(&mut self) -> Option<Box<dyn PreparedRestart>> {
        self.configuration
            .as_mut()
            .and_then(|source| source.take_restart())
    }

    fn active_wants_pointer_events(&self) -> bool {
        self.registry
            .active_slot
            .filter(|&index| self.registry.id_at(index) == Some(&self.registry.active))
            .or_else(|| self.registry.index_of(&self.registry.active))
            .and_then(|index| self.registry.wants_pointer_events_at(index))
            .unwrap_or(true)
    }

    /// Input dominates the event loop. Keep its dispatch small enough to
    /// inline independently of the growing set of background notifications.
    #[inline(always)]
    fn handle_backend_event(
        &mut self,
        event: BackendEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match event {
            BackendEvent::Input(input) => {
                crate::support::perf_probe::mark("input_received");
                self.handle_key(input, backend)
            }
            event => self.handle_backend_notification(event, backend),
        }
    }

    #[inline(never)]
    fn handle_backend_notification(
        &mut self,
        event: BackendEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match event {
            BackendEvent::TextPromptChanged { id, text } => {
                if let Some((owner, prompt)) = &self.scheduler.text_prompt
                    && prompt.id == id
                {
                    self.dispatch_to(&owner.clone(), ModeEvent::TextChanged(text), backend)?;
                }
            }
            BackendEvent::PointSampled(sample) => {
                if let Some((owner, id, request)) = &self.scheduler.point_sample
                    && *id == sample.request.id
                {
                    let sample = crate::api::point_sample::Sample {
                        request: *request,
                        color: sample.color,
                    };
                    self.dispatch_to(&owner.clone(), ModeEvent::PointSampled(sample), backend)?;
                }
            }
            BackendEvent::TextSegmented(mut result) => {
                if let Some((owner, _, original)) = self
                    .scheduler
                    .text_segmentation
                    .take_if(|(_, id, _)| *id == result.id)
                {
                    result.id = original;
                    self.overlay.key_help_cache = None;
                    self.dispatch_to(&owner, ModeEvent::TextSegmented(result), backend)?;
                }
            }
            BackendEvent::TextPromptResult { id, value } => {
                if let Some((owner, prompt)) = self
                    .scheduler
                    .text_prompt
                    .take_if(|(_, prompt)| prompt.id == id)
                {
                    self.clear_point_input(backend);
                    // Retire capture before dropping ownership: a later
                    // CloseTextPrompt cannot find this already-taken prompt.
                    backend.set_text_capture(false);
                    self.scheduler.text_prompt_returning_focus = prompt.live_style.is_none();
                    let text = match value {
                        Ok(text) => text,
                        Err(error) => {
                            self.report_action_error(error, backend);
                            None
                        }
                    };
                    self.dispatch_to(&owner, ModeEvent::TextSubmitted(text), backend)?;
                } else {
                    self.finish_layout_note(id, value, backend)?
                }
            }
            BackendEvent::AudioResult(result) => {
                if let Some(owner) = self.scheduler.audio_sessions.get(&result.session).cloned() {
                    self.overlay.key_help_cache = None;
                    self.dispatch_to(&owner, ModeEvent::AudioResult(result), backend)?;
                }
            }
            BackendEvent::WindowResult(result) => {
                if let Some(owner) = self.scheduler.window_sessions.get(&result.session).cloned() {
                    self.overlay.key_help_cache = None;
                    self.dispatch_to(&owner, ModeEvent::WindowResult(result), backend)?;
                }
            }
            BackendEvent::Input(_) => unreachable!("input is routed by handle_backend_event"),
            BackendEvent::InputInjectionFailed(message) => {
                return Err(self.recoverable_input_error("asynchronous native input", message));
            }
            BackendEvent::InputCaptureLost(message) => {
                self.reset_runtime_input_state(&message, true, backend);
            }
            BackendEvent::WindowCycleCompleted(result) => match result {
                Ok(point) => {
                    let owner = self.registry.active.clone();
                    self.execute_for(&owner, [Command::warp_to(point)], backend)?;
                }
                Err(message) => crate::support::logging::report_error("window-cycle", message),
            },
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
                if self.active_wants_pointer_events() {
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
                // Native layout-note editing owns its lifetime until an explicit
                // result/cancel. IME switching may temporarily focus a helper
                // process; forwarding that focus to the mode tears down its
                // layout and cancels the still-active native editor.
                if self.window_presets.pending.is_some() {
                    return Ok(());
                }
                let expected_activation = self
                    .scheduler
                    .scan_activation
                    .is_some_and(|pid| app.as_ref().is_some_and(|app| app.process_id == pid));
                if app.is_some() {
                    self.scheduler.scan_activation = None;
                }
                // Preserve the editing snapshot through transient focus notifications.
                if self.scheduler.text_prompt.is_some() && !expected_activation {
                    if app.is_none()
                        || app
                            .as_ref()
                            .is_some_and(|app| app.process_id == std::process::id())
                        || same_binding_app_snapshot(self.focused_app.as_ref(), app.as_ref())
                    {
                        return Ok(());
                    }
                    if let Some((owner, prompt)) = self.scheduler.text_prompt.take() {
                        backend.set_text_capture(false);
                        backend.cancel_text_prompt(prompt.id);
                        self.dispatch_to(&owner, ModeEvent::TextSubmitted(None), backend)?;
                    }
                }
                if std::mem::take(&mut self.scheduler.text_prompt_returning_focus)
                    && same_binding_app_snapshot(self.focused_app.as_ref(), app.as_ref())
                {
                    return Ok(());
                }
                // Duplicate native notifications are common. If process,
                // bundle and title are unchanged, the cached profile is still
                // valid without even walking the override list. A changed
                // title may affect substring overrides, so compare the exact
                // resolved profile before deciding whether to recompile.
                let changed_profile =
                    if same_binding_app_snapshot(self.focused_app.as_ref(), app.as_ref()) {
                        None
                    } else {
                        let profile = self.binding_profile_key_for(app.as_ref());
                        (profile != self.registry.binding_profile_key).then_some(profile)
                    };
                match (&mut self.focused_app, &app) {
                    (Some(current), Some(next)) => {
                        current.bundle_id.clone_from(&next.bundle_id);
                        current.window_title.clone_from(&next.window_title);
                        current.process_id = next.process_id;
                    }
                    (current, next) => current.clone_from(next),
                }
                self.focused_app_excluded = self.excluded_app_matches(self.focused_app.as_ref());
                if self.focused_app_excluded {
                    let click_feedback_changed = self.cancel_transient_clicks(backend)?;
                    let released_drag = self.release_drag_auto_release(backend)?;
                    if click_feedback_changed || released_drag {
                        self.refresh_overlay(backend)?;
                    }
                }
                if let Some(profile) = changed_profile {
                    self.rebuild_tables_for_profile(profile);
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
            BackendEvent::UiScanActivationExpected { id, process_id } => {
                if let Some(owner) = self.scan_owners.get(&id).cloned()
                    && owner == self.registry.active
                {
                    self.scheduler.scan_activation = Some(process_id);
                    self.dispatch_to(
                        &owner,
                        ModeEvent::UiScanActivationExpected { id, process_id },
                        backend,
                    )?;
                }
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
                    self.overlay.key_help_cache = None;
                    self.dispatch_to(&owner, ModeEvent::UiScanned(result), backend)?;
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
                self.export_workspace(source, backend)?;
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
            BackendEvent::ConfigurationReady => self.finish_configuration(backend)?,
            BackendEvent::SaveWorkspace(reply) => self.checkpoint_workspace(reply, backend)?,
            BackendEvent::WorkspaceCompleted(completed) => {
                self.finish_workspace(*completed, backend)?
            }
            BackendEvent::Warning(message) => {
                crate::report_warning!("backend", "{message}")
            }
            BackendEvent::FocusedWindowBounds { id, bounds } => {
                if let Some((owner, request, process)) = self.scheduler.panel_geometry.remove(&id)
                    && self
                        .focused_app
                        .as_ref()
                        .is_some_and(|app| app.process_id == process)
                {
                    let bounds = match bounds {
                        Ok(bounds) => bounds,
                        Err(error) => {
                            crate::report_error!("panel", "geometry query: {error}");
                            None
                        }
                    };
                    self.dispatch_to(
                        &owner,
                        ModeEvent::PanelWindowBounds {
                            id: request,
                            bounds,
                        },
                        backend,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn reload_config(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        self.request_configuration(configuration_work::Operation::Reload, backend)
    }

    /// Swap in a precompiled plan without restarting runtime-owned tasks.
    /// This is intended for deterministic tests and initial assembly only;
    /// production Reload replaces the process, while set_config resets the runtime plan.
    pub fn apply_plan(&mut self, plan: RuntimePlan) -> Result<(), String> {
        let RuntimePlan {
            settings,
            palettes,
            mut modes,
        } = plan;
        // Retain unused instances through installation; move only their routes.
        let routes = modes
            .iter_mut()
            .map(|spec| (spec.id(), std::mem::take(&mut spec.route)))
            .collect();
        crate::support::logging::set_non_error_enabled(settings.debug.enabled);
        self.input.drag_auto_release.clear();
        self.settings = settings;
        self.quick_switch.pending = None;
        self.palettes = palettes;
        self.registry.replace_routes(routes);
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
        let (routes, instances): (std::collections::BTreeMap<_, _>, Vec<_>) = modes
            .into_iter()
            .map(|spec| ((spec.id(), spec.route), spec.instance))
            .unzip();

        self.retire_workspace_requests(None);
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
        for session in std::mem::take(&mut self.scheduler.audio_sessions).into_keys() {
            backend.cancel_audio_session(session);
        }
        for session in std::mem::take(&mut self.scheduler.window_sessions).into_keys() {
            backend.cancel_window_session(session);
        }
        backend.set_text_capture(false);
        backend.release_text_prompt();
        self.clear_point_input(backend);
        self.scheduler.reset();
        self.registry.modal_stack.clear();
        self.input.reset_for_plan_swap();
        self.release_toggle_session_for_safe_mode(backend)?;
        self.cancel_all_scans(backend)?;
        backend.set_frame_clock(false)?;
        backend.dismiss()?;
        self.overlay.reset();

        if self.settings.window_overlap_enabled && !settings.window_overlap_enabled {
            backend.clear_window_overlap_cache();
        }
        self.settings = settings;
        self.quick_switch.pending = None;
        self.palettes = palettes;
        self.palette = self.palettes.for_appearance(self.appearance);
        self.focused_app_excluded = self.excluded_app_matches(self.focused_app.as_ref());
        self.registry = ModeRegistry::with_routes(routes);
        crate::support::logging::set_non_error_enabled(self.settings.debug.enabled);
        for instance in instances {
            match instance {
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

        let grid = target.is_grid();
        if grid
            && self
                .registry
                .get(&self.registry.active)
                .is_some_and(|mode| mode.accepts_window_targeting())
        {
            return self.push_mode(target, backend);
        }
        if self.registry.modal_stack.contains(&ModeId::window()) && !grid {
            while self.registry.active != ModeId::window() {
                self.pop_mode(backend)?;
            }
            if target == ModeId::normal() || target == ModeId::idle() || target == ModeId::window()
            {
                return Ok(());
            }
        }

        if target != self.registry.active {
            if let Some((_, prompt)) = self.scheduler.text_prompt.take() {
                backend.set_text_capture(false);
                backend.cancel_text_prompt(prompt.id);
            }
            self.retire_workspace_requests(Some(&self.registry.active.clone()));
            // Native text entry belongs to the outgoing interaction, even when
            // its final layout acknowledgement defers the actual mode switch.
            if let Some(pending) = self
                .window_presets
                .pending
                .take_if(|pending| pending.owner == self.registry.active)
            {
                backend.cancel_text_prompt(pending.id);
            }
            let context = HostContext {
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: self.focused_app.as_ref(),
                palette: &self.palette,
            };
            let deferred = self
                .registry
                .get_mut(&self.registry.active.clone())
                .and_then(|mode| mode.prepare_transition(&target, &context));
            if let Some(commands) = deferred {
                return self.execute(commands, backend);
            }
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
            for (key, gesture) in std::mem::take(&mut self.input.active_gestures) {
                self.release_gesture(key, gesture, backend)?;
            }

            let shared_group = self
                .registry
                .get(&self.registry.active)
                .and_then(|mode| mode.session_group())
                .filter(|group| {
                    self.registry
                        .get(&target)
                        .and_then(|mode| mode.session_group())
                        == Some(*group)
                });
            if shared_group.is_some() {
                for owner in self
                    .scheduler
                    .window_sessions
                    .values_mut()
                    .chain(self.scheduler.audio_sessions.values_mut())
                {
                    if *owner == self.registry.active {
                        *owner = target.clone();
                    }
                }
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
                action => {
                    // Adjacent identical sends are a double tap, including
                    // sends separated only by a nested sequence boundary.
                    // An explicit wait already breaks the adjacency.
                    if matches!(
                        (flattened.last(), action),
                        (Some(Binding::Send(previous)), Binding::Send(current)) if previous == current
                    ) {
                        flattened.push(Binding::Wait {
                            min_ms: DEFAULT_WAIT_MS,
                            max_ms: DEFAULT_WAIT_MS,
                        });
                    }
                    flattened.push(action.clone());
                }
            }
        }

        let mut flattened = Vec::new();
        for action in actions {
            append(action, &mut flattened);
        }
        flattened
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
    fn stateful_binding_owner(&self, resolved: &ResolvedBinding) -> ModeId {
        // Window operations act on the active shared session, even when their
        // chord is inherited from another mode's binding table.
        if resolved.binding.window_action().is_some() && self.registry.active.is_window() {
            self.registry.active.clone()
        } else {
            resolved.owner.clone()
        }
    }

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
        // Direct key mappings follow native repeats; clicks and other discrete
        // actions still fire once. No synthetic repeat timer is needed.
        if input.repeat && !binding.repeats_on_key_down() {
            return Ok(true);
        }

        match binding {
            // These actions always belong to the resolved mode. Only window
            // actions below can transfer to the active shared window session.
            Binding::Move(_)
            | Binding::Scroll(..)
            | Binding::Speed(_)
            | Binding::SpeedToggle(_)
            | Binding::ToggleCursorFollowSelection
            | Binding::TargetingKey(_)
            | Binding::RescanUi
            | Binding::ScanRegion(_)
            | Binding::TextSelect(_)
            | Binding::SelectionReset
            | Binding::Bisect(_)
            | Binding::SelectionParent
            | Binding::SelectionReopen
            | Binding::SelectionAccept
            | Binding::SelectionCancel
            | Binding::SelectionSearch => self
                .dispatch_to(
                    &resolved.owner,
                    ModeEvent::Binding {
                        binding: resolved.binding,
                        state: input.state,
                        key: input.key.clone(),
                    },
                    backend,
                )
                .map(|_| true),
            Binding::Window(_) => {
                let recipient = self.stateful_binding_owner(&resolved);
                self.dispatch_to(
                    &recipient,
                    ModeEvent::Binding {
                        binding: resolved.binding,
                        state: input.state,
                        key: input.key.clone(),
                    },
                    backend,
                )
                .map(|_| true)
            }
            Binding::MoveWindowFromToScreen { source, target } => {
                let owner = self.registry.active.clone();
                self.execute_for(
                    &owner,
                    [Command::MoveWindowFromToScreen {
                        source: *source,
                        target: *target,
                    }],
                    backend,
                )?;
                Ok(true)
            }
            Binding::WindowTarget {
                binding: inner,
                source,
            } => {
                if inner.window_action().is_some() {
                    let recipient = self.stateful_binding_owner(&resolved);
                    self.dispatch_to(
                        &recipient,
                        ModeEvent::Binding {
                            binding: resolved.binding.clone(),
                            state: input.state,
                            key: input.key.clone(),
                        },
                        backend,
                    )?;
                } else {
                    match inner.as_ref() {
                        Binding::ActivateWindow { backwards }
                        | Binding::ActivateOverlappingWindow { backwards } => {
                            if matches!(inner.as_ref(), Binding::ActivateOverlappingWindow { .. })
                                && !self.settings.window_overlap_enabled
                            {
                                return Ok(true);
                            }
                            backend.request_window(crate::api::window::WindowRequest {
                                scope: None,
                                session: 0,
                                id: 0,
                                operation: crate::api::window::WindowOperation::CycleFrom {
                                    backwards: *backwards,
                                    overlapping: matches!(
                                        inner.as_ref(),
                                        Binding::ActivateOverlappingWindow { .. }
                                    ),
                                    source: *source,
                                },
                            })?;
                        }
                        Binding::Mode(id) => {
                            if *id != self.registry.active && self.registry.contains_key(id) {
                                self.dispatch_to(
                                    id,
                                    ModeEvent::PrepareWindowTarget(Some(*source)),
                                    backend,
                                )?;
                                if let Ok(pointer) = backend.pointer()
                                    && let Some(pointer) = self.constrain_absolute_pointer(pointer)
                                {
                                    self.cursor = pointer;
                                }
                                self.activate(
                                    id.clone(),
                                    Some(self.registry.active.clone()),
                                    backend,
                                )?;
                            } else if *id == self.registry.active {
                                self.activate(ModeId::idle(), Some(id.clone()), backend)?;
                            }
                        }
                        _ => return Err("unsupported targeted window binding".into()),
                    }
                }
                Ok(true)
            }
            Binding::ActivateWindow { backwards }
            | Binding::ActivateOverlappingWindow { backwards } => {
                let owner = self.registry.active.clone();
                let command = if matches!(binding, Binding::ActivateOverlappingWindow { .. }) {
                    Command::CycleOverlappingWindow {
                        backwards: *backwards,
                    }
                } else {
                    Command::CycleWindow {
                        backwards: *backwards,
                    }
                };
                self.execute_for(&owner, [command], backend)?;
                Ok(true)
            }
            Binding::KeyHelp => {
                if self.registry.active != ModeId::idle() {
                    if self.display_mode().is_window() {
                        self.overlay.window_help_override = Some(!self.window_help_visible());
                    } else {
                        self.overlay.key_help_visible = !self.overlay.key_help_visible;
                    }
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
                        action.mode().is_some()
                            || matches!(
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
                if id.is_window() && next == *id {
                    self.dispatch_to(id, ModeEvent::PrepareWindowTarget(None), backend)?;
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
                if let Some(action) = self
                    .registry
                    .keyboard_prompt_mode(&self.registry.active)
                    .and_then(|mode| {
                        mode.keyboard_prompt_edit(chord, &input.key, &self.input.pressed)
                    })
                {
                    if is_press {
                        self.dispatch_to(
                            &self.registry.active.clone(),
                            ModeEvent::TextEdit(action),
                            backend,
                        )?;
                    }
                    return Ok(true);
                }
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

            // Filtered out when the table was built.
            Binding::Disabled => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests;
