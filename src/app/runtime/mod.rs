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

mod command_executor;
mod input_router;
mod overlay_coordinator;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::api::backend::{Appearance, Backend, BackendEvent, KeyDisposition};
use crate::api::binding::{Binding, Button, DEFAULT_WAIT_MS, InputTarget};
use crate::api::command::{
    ButtonAction, Command, FinishCause, FocusedApp, HostContext, Mode, ModeEvent, MouseButton,
    UiScanRequest, UiScanStatus,
};
use crate::api::geometry::{Point, Screen};
use crate::api::input::{Key, KeyChord, KeyState, ModeId};
use crate::api::overlay::{CursorMarker, Indicator, OverlayScene};
use crate::config::{Bindings, Config, ConfigStore, Palette};
use input_router::CompiledKeymap;

const RECOVERABLE_INPUT_PREFIX: &str = "[recoverable-input] ";

/// A pending timer request from a mode.
#[derive(Debug, Clone)]
struct Timer {
    fires_at: Instant,
    last_fired: Instant,
    interval: Option<Duration>,
    owner: ModeId,
}

#[derive(Debug, Clone)]
struct PendingSequence {
    fires_at: Instant,
    actions: VecDeque<Binding>,
    owner: ModeId,
    input: crate::api::input::InputEvent,
}

#[derive(Debug, Clone)]
struct PendingLongPressToggle {
    fires_at: Instant,
    key: Key,
    button: Button,
}

/// What the engine decided to do with a key, so the caller can tell the
/// backend whether to swallow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyOutcome {
    Consumed,
    Forwarded,
}

/// A binding together with the mode that must receive its stateful events.
#[derive(Debug, Clone)]
struct ResolvedBinding {
    binding: Arc<Binding>,
    owner: ModeId,
}

/// Remembers the receiving mode across the release half of a held gesture.
#[derive(Debug, Clone)]
struct ActiveGesture {
    binding: Arc<Binding>,
    owner: ModeId,
}

/// Ordinary clicks remain atomic on key-down, but their cursor decoration
/// follows the physical activation key until it is released. This is visual
/// state only: unlike `latched`, it never owns a synthetic mouse button.
#[derive(Debug, Default)]
struct ActiveClickIndicators(Vec<(Key, Button)>);

impl ActiveClickIndicators {
    fn activate(&mut self, key: Key, button: Button) {
        if let Some(index) = self.0.iter().position(|(candidate, _)| candidate == &key) {
            self.0.remove(index);
        }
        self.0.push((key, button));
    }

    fn release(&mut self, key: &Key) -> bool {
        let Some(index) = self.0.iter().position(|(candidate, _)| candidate == key) else {
            return false;
        };
        self.0.remove(index);
        true
    }

    fn latest_button(&self) -> Option<Button> {
        self.0.last().map(|(_, button)| *button)
    }

    fn clear(&mut self) {
        self.0.clear();
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Physical keys are a tiny set in practice. A preallocated linear set avoids
/// one tree-node allocation on every fresh key-down while keeping lookups
/// cache-local (chords rarely contain more than a handful of keys).
#[derive(Debug)]
struct PressedKeys(Vec<Key>);

impl Default for PressedKeys {
    fn default() -> Self {
        Self(Vec::with_capacity(16))
    }
}

impl PressedKeys {
    fn insert(&mut self, key: Key) -> bool {
        if self.0.contains(&key) {
            return false;
        }
        self.0.push(key);
        true
    }

    fn remove(&mut self, key: &Key) -> bool {
        let Some(index) = self.0.iter().position(|candidate| candidate == key) else {
            return false;
        };
        self.0.swap_remove(index);
        true
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.0.clear();
    }
}

impl std::ops::Deref for PressedKeys {
    type Target = [Key];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
struct KeyMap<T>(Vec<(Key, T)>);

impl<T> Default for KeyMap<T> {
    fn default() -> Self {
        Self(Vec::with_capacity(16))
    }
}

impl<T> KeyMap<T> {
    fn insert(&mut self, key: Key, value: T) -> Option<T> {
        if let Some((_, current)) = self.0.iter_mut().find(|(candidate, _)| candidate == &key) {
            return Some(std::mem::replace(current, value));
        }
        self.0.push((key, value));
        None
    }

    fn get(&self, key: &Key) -> Option<&T> {
        self.0
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    fn remove(&mut self, key: &Key) -> Option<T> {
        let index = self.0.iter().position(|(candidate, _)| candidate == key)?;
        Some(self.0.swap_remove(index).1)
    }

    fn clear(&mut self) {
        self.0.clear();
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T> IntoIterator for KeyMap<T> {
    type Item = (Key, T);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Last overlay intent produced by one (possibly nested) command batch.
/// Pointer injection still happens immediately; only expensive presentation is
/// coalesced until the outermost batch completes.
enum PendingOverlay {
    Refresh,
    Show(Box<OverlayScene>),
    Hide,
}

pub struct Engine {
    config: Config,
    palette: Palette,
    appearance: Appearance,

    modes: BTreeMap<ModeId, Box<dyn Mode>>,
    active: ModeId,

    /// Resolved binding table per mode, rebuilt only when its effective profile changes.
    tables: BTreeMap<ModeId, CompiledKeymap>,
    /// Exact per-mode app-override patches used to build `tables`.
    binding_profile_key: Vec<Bindings>,
    #[cfg(test)]
    table_rebuild_count: usize,
    /// Suggested plugin bindings, used only where user configuration leaves the
    /// chord free.
    plugin_bindings: Vec<(KeyChord, Binding)>,
    /// Public verb name to receiving plugin mode.
    plugin_verbs: BTreeMap<String, ModeId>,
    /// Modes suspended beneath a temporary modal plugin.
    modal_stack: Vec<ModeId>,

    screens: Vec<Screen>,
    cursor: Point,
    focused_app: Option<FocusedApp>,

    pressed: PressedKeys,
    /// The OS must see matching down/up halves even if a mode switch happens
    /// between them (for example Ctrl down in idle, then Ctrl+E enters normal).
    key_dispositions: KeyMap<KeyDisposition>,
    /// Held bindings currently in effect, keyed by the key that started them.
    ///
    /// A release cannot be resolved by looking the chord up again: the key (and
    /// possibly its modifiers) are no longer held, so the chord would not match.
    /// Remembering the binding is what guarantees every press is followed by
    /// its release, rather than movement sticking on forever.
    active_gestures: KeyMap<ActiveGesture>,
    /// Successful atomic clicks whose physical activation keys remain down.
    /// This drives cursor color only and does not represent mouse-button state.
    active_click_indicators: ActiveClickIndicators,
    /// Synthetic keyboard keys and mouse buttons held by `press` or `toggle`.
    /// Keeping one shared set makes these actions idempotent and lets the UI
    /// report exactly what the engine is responsible for releasing.
    latched: BTreeSet<InputTarget>,
    /// Physically held activation keys for parameterless `toggle`. The value is
    /// true once a companion target was toggled during this press. Releasing an
    /// unused activation key clears every currently latched input instead.
    active_default_toggles: BTreeMap<Key, bool>,
    pending_sequences: Vec<PendingSequence>,
    /// Click keys waiting to cross the configured hold threshold. Firing one
    /// delegates to the same latched-input state machine as explicit toggle.
    pending_long_press_toggles: Vec<PendingLongPressToggle>,
    timers: HashMap<String, Timer>,
    scan_owners: HashMap<u64, ModeId>,
    /// Mode that owns native display frames. This can differ from `active`
    /// when grid or hint mode inherits normal-mode movement bindings.
    frame_clock_owner: Option<ModeId>,

    /// Last scene presented, so redundant presents can be skipped.
    last_scene: Option<Arc<OverlayScene>>,
    /// Mode-owned scene before cursor decorations are added.
    overlay_content: Option<Arc<OverlayScene>>,
    overlay_visible: bool,
    command_batch_depth: usize,
    pending_overlay: Option<PendingOverlay>,

    enabled: bool,
    /// Suppress repeated reports while the focused window rejects a stream of
    /// synthetic input (most commonly Windows UIPI on an elevated window).
    input_failure_active: bool,
    should_quit: bool,
    config_store: Option<ConfigStore>,
    /// Automatic startup keeps discovering this directory on every reload.
    /// An explicit `--config` leaves this unset and remains pinned to its path.
    config_discovery_directory: Option<PathBuf>,
    started_at: Instant,
}

impl Engine {
    pub fn new(config: Config, appearance: Appearance) -> Self {
        crate::app::logging::set_non_error_enabled(config.debug.enabled);
        let palette = config.palette(appearance);
        let mut engine = Self {
            config,
            palette,
            appearance,
            modes: BTreeMap::new(),
            active: ModeId::idle(),
            tables: BTreeMap::new(),
            binding_profile_key: Vec::new(),
            #[cfg(test)]
            table_rebuild_count: 0,
            plugin_bindings: Vec::new(),
            plugin_verbs: BTreeMap::new(),
            modal_stack: Vec::new(),
            screens: Vec::new(),
            cursor: Point::default(),
            focused_app: None,
            pressed: PressedKeys::default(),
            key_dispositions: KeyMap::default(),
            active_gestures: KeyMap::default(),
            active_click_indicators: ActiveClickIndicators::default(),
            latched: BTreeSet::new(),
            active_default_toggles: BTreeMap::new(),
            pending_sequences: Vec::new(),
            pending_long_press_toggles: Vec::new(),
            timers: HashMap::new(),
            scan_owners: HashMap::new(),
            frame_clock_owner: None,
            last_scene: None,
            overlay_content: None,
            overlay_visible: false,
            command_batch_depth: 0,
            pending_overlay: None,
            enabled: true,
            input_failure_active: false,
            should_quit: false,
            config_store: None,
            config_discovery_directory: None,
            started_at: Instant::now(),
        };
        engine.rebuild_tables();
        engine
    }

    /// Register a mode. Later registrations replace earlier ones with the same
    /// id, which is how a plugin can override a built-in mode.
    pub fn register(&mut self, mode: Box<dyn Mode>) {
        self.modes.insert(mode.id(), mode);
        self.rebuild_tables();
    }

    /// Register a plugin and merge its default chords, without letting them
    /// override bindings the user configured explicitly.
    pub fn register_plugin<P>(&mut self, plugin: Box<P>) -> Result<(), String>
    where
        P: crate::api::plugin::Plugin + 'static,
    {
        self.register_plugin_dyn(plugin)
    }

    /// Same as [`Self::register_plugin`] for an already-boxed trait object,
    /// which is what a plugin loader produces.
    pub fn register_plugin_dyn(
        &mut self,
        plugin: Box<dyn crate::api::plugin::Plugin>,
    ) -> Result<(), String> {
        plugin.manifest().validate()?;
        let id = plugin.id();

        for chord in plugin.manifest().default_chords.clone() {
            self.plugin_bindings
                .push((chord, Binding::Mode(id.clone())));
        }
        self.plugin_bindings
            .extend(plugin.manifest().default_bindings.clone());
        for verb in plugin.manifest().verbs.clone() {
            if let Some(existing) = self.plugin_verbs.insert(verb.clone(), id.clone()) {
                return Err(format!(
                    "plugin verb {verb:?} is already owned by {existing}"
                ));
            }
        }
        self.modes.insert(id, plugin as Box<dyn Mode>);
        self.rebuild_tables();
        Ok(())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn attach_config_store(&mut self, store: ConfigStore) {
        self.config_store = Some(store);
        self.config_discovery_directory = None;
    }

    pub(crate) fn attach_discovered_config_store(
        &mut self,
        store: ConfigStore,
        directory: PathBuf,
    ) {
        self.config_store = Some(store);
        self.config_discovery_directory = Some(directory);
    }

    fn recoverable_input_error(action: &str, error: String) -> String {
        format!("{RECOVERABLE_INPUT_PREFIX}{action}: {error}")
    }

    fn recover_from_input_error(&mut self, error: &str, backend: &mut dyn Backend) -> bool {
        let Some(message) = error.strip_prefix(RECOVERABLE_INPUT_PREFIX) else {
            return false;
        };
        if !self.input_failure_active {
            crate::app::logging::report_error(
                "input",
                format!("{message}; this action was rejected and runtime input state was reset"),
            );
            self.input_failure_active = true;
        }

        let previous = self.active.clone();
        self.pending_sequences.clear();
        self.pending_long_press_toggles.clear();
        self.active_default_toggles.clear();
        self.active_gestures.clear();
        self.active_click_indicators.clear();
        self.modal_stack.clear();
        self.timers.clear();
        self.scan_owners.clear();
        self.frame_clock_owner = None;
        if let Err(clock_error) = backend.set_frame_clock(false) {
            self.trace_lazy(self.config.debug.backend, "backend", || {
                format!("cannot stop frame clock during input recovery: {clock_error}")
            });
        }

        if let Err(release_error) = self.release_latched(backend) {
            crate::app::logging::report_error(
                "input",
                format!("cannot release every held input during recovery: {release_error}"),
            );
        }

        if previous != ModeId::idle() {
            if let Some(mut mode) = self.modes.remove(&previous) {
                // Let the mode clear its private session state, but discard
                // commands: the recovery path must not inject more input.
                let _ = mode.handle(&ModeEvent::Deactivated, &self.context());
                self.modes.insert(previous.clone(), mode);
            }
            self.active = ModeId::idle();
            if let Some(mut idle) = self.modes.remove(&ModeId::idle()) {
                let _ = idle.handle(
                    &ModeEvent::Activated {
                        previous: Some(previous),
                    },
                    &self.context(),
                );
                self.modes.insert(ModeId::idle(), idle);
            }
        }

        if let Err(dismiss_error) = backend.dismiss() {
            crate::app::logging::report_error(
                "overlay",
                format!("cannot dismiss overlay during input recovery: {dismiss_error}"),
            );
        }
        // Keep the logical state clean even if the native window was already
        // unavailable. A later scene must be rebuilt from scratch.
        self.overlay_content = None;
        self.last_scene = None;
        self.overlay_visible = false;
        true
    }

    fn recoverable_input_succeeded(&mut self) {
        self.input_failure_active = false;
    }

    fn report_action_error(&mut self, error: String, backend: &mut dyn Backend) {
        if !self.recover_from_input_error(&error, backend) {
            crate::app::logging::report_error("action", format!("action failed: {error}"));
        }
    }

    pub fn active_mode(&self) -> &ModeId {
        &self.active
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn registered_modes(&self) -> impl Iterator<Item = &ModeId> {
        self.modes.keys()
    }

    /// Bindings active in `mode`, for diagnostics and tests.
    pub fn bindings_in(&self, mode: &ModeId) -> Vec<(String, Binding)> {
        self.tables
            .get(mode)
            .map(CompiledKeymap::entries)
            .unwrap_or_default()
    }

    fn binding_mode_ids(&self) -> Vec<ModeId> {
        let mut ids: Vec<ModeId> = self.modes.keys().cloned().collect();
        // Idle must resolve even before any mode registers, so it is always
        // considered.
        if !ids.contains(&ModeId::idle()) {
            ids.push(ModeId::idle());
        }
        ids
    }

    fn binding_profile_key_for(&self, app: Option<&FocusedApp>) -> Vec<Bindings> {
        let ids = self.binding_mode_ids();
        self.config
            .binding_profile_key(ids.iter().map(|id| id.as_str()), app)
    }

    /// Resolve every mode's binding table from the configuration.
    ///
    /// Per-app overrides for the focused application are folded in, so a
    /// binding can differ per application, and disabled entries are dropped.
    fn rebuild_tables(&mut self) {
        let ids = self.binding_mode_ids();
        let binding_profile_key = self
            .config
            .binding_profile_key(ids.iter().map(|id| id.as_str()), self.focused_app.as_ref());

        let mut tables: BTreeMap<ModeId, CompiledKeymap> = BTreeMap::new();
        for id in ids {
            let Some(configured) = self.config.bindings_for(id.as_str()) else {
                continue;
            };
            let merged = self.merge_overrides(id.as_str(), configured);

            tables.insert(
                id,
                CompiledKeymap::compile(merged, self.config.resolved_key_aliases()),
            );
        }

        // A plugin's suggested chord applies in `normal`, which is where the
        // user works, and only if that chord is still free.
        let normal = ModeId::normal();
        if self.modes.contains_key(&normal) {
            let table = tables.entry(normal).or_default();
            for (chord, binding) in &self.plugin_bindings {
                let taken = table.contains_chord(chord);
                if !taken {
                    table.insert(chord.clone(), binding.clone());
                }
            }
        }

        self.tables = tables;
        self.binding_profile_key = binding_profile_key;
        #[cfg(test)]
        {
            self.table_rebuild_count += 1;
        }
    }

    /// Apply the focused application's overrides to one mode's table.
    fn merge_overrides(&self, mode_id: &str, base: &Bindings) -> Vec<(String, Binding)> {
        let mut merged: BTreeMap<String, Binding> = base.clone();
        if let Some(app) = &self.focused_app {
            for (pattern, bindings) in self.config.app_binding_overrides_for(mode_id) {
                if crate::config::app_override_matches(pattern, app) {
                    for (chord, binding) in bindings {
                        merged.insert(chord.clone(), binding.clone());
                    }
                }
            }
        }
        merged.into_iter().collect()
    }

    fn is_excluded_app(&self) -> bool {
        let Some(app) = &self.focused_app else {
            return false;
        };
        self.config
            .general
            .excluded_apps
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&app.bundle_id))
    }

    fn context(&self) -> HostContext<'_> {
        HostContext {
            screens: &self.screens,
            cursor: self.cursor,
            focused_app: self.focused_app.as_ref(),
            palette: &self.palette,
            config: &self.config,
        }
    }

    fn trace(&self, category_enabled: bool, category: &str, message: impl AsRef<str>) {
        if self.config.debug.enabled && category_enabled {
            let message = format!(
                "+{:>7.2}ms {}",
                self.started_at.elapsed().as_secs_f64() * 1000.0,
                message.as_ref()
            );
            crate::app::logging::debug_args(category, format_args!("{message}"));
        }
    }

    fn trace_lazy(&self, category_enabled: bool, category: &str, message: impl FnOnce() -> String) {
        if self.config.debug.enabled && category_enabled {
            self.trace(true, category, message());
        }
    }

    fn trace_binding_tables(&self) {
        if !self.config.debug.enabled || !self.config.debug.keys {
            return;
        }
        for (mode, table) in &self.tables {
            let entries = table.entries();
            self.trace(
                true,
                "bindings",
                format!("mode={mode} listening={} bindings", entries.len()),
            );
            for (chord, action) in entries {
                self.trace(
                    true,
                    "bindings",
                    format!("  mode={mode} key={chord:?} -> {action:?}"),
                );
            }
            if let Some((inherits, temporary, keys)) = self.config.inheritance_for(mode.as_str()) {
                self.trace(
                    true,
                    "bindings",
                    format!(
                        "  inherits={inherits:?} temporary_mode={temporary:?} temporary_keys={keys:?}"
                    ),
                );
            }
        }
    }

    /// Run until a backend event or a mode asks to quit.
    pub fn run(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if let Err(error) = backend.start() {
            if let Err(shutdown_error) = backend.shutdown() {
                crate::app::logging::report_error(
                    "backend",
                    format!("shutdown after startup failure also failed: {shutdown_error}"),
                );
            }
            return Err(error);
        }
        crate::app::logging::info_args(
            "backend",
            format_args!("{} backend started", backend.name()),
        );
        self.trace_lazy(self.config.debug.backend, "backend", || {
            format!("started {} backend", backend.name())
        });

        // Say up front when the keyboard cannot be read. Every mode depends on
        // it, so without this the program looks like it is doing nothing for no
        // reason.
        if !backend.keyboard_available() {
            let reason = backend
                .keyboard_unavailable_reason()
                .unwrap_or_else(|| "the keyboard cannot be observed".to_string());
            crate::app::logging::report_error("keyboard", reason);
        }

        self.appearance = backend.appearance();
        self.palette = self.config.palette(self.appearance);
        self.screens = backend.screens().unwrap_or_else(|error| {
            crate::app::logging::report_error("backend", format!("cannot read screens: {error}"));
            Vec::new()
        });
        self.cursor = backend.pointer().unwrap_or_else(|error| {
            crate::app::logging::report_error("backend", format!("cannot read pointer: {error}"));
            Point::default()
        });
        self.focused_app = backend.focused_app().unwrap_or_else(|error| {
            crate::app::logging::report_error(
                "backend",
                format!("cannot read focused application: {error}"),
            );
            None
        });
        crate::app::logging::info_args(
            "backend",
            format_args!(
                "initial state screens={} appearance={:?} keyboard_available={}",
                self.screens.len(),
                self.appearance,
                backend.keyboard_available()
            ),
        );
        // Registration happens before the backend can report its initial
        // foreground process, so fold app-specific bindings in once more now.
        self.rebuild_tables();
        self.trace_lazy(self.config.debug.backend, "backend", || {
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
                crate::app::logging::report_error(
                    "backend",
                    format!("shutdown after activation failure also failed: {shutdown_error}"),
                );
            }
            return Err(error);
        }

        let mut result = (|| {
            while !self.should_quit {
                let timeout = self.next_timeout();
                // A timeout returns `None`, which is our chance to run timers.
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
            }
            Ok(())
        })();

        self.pending_sequences.clear();
        self.pending_long_press_toggles.clear();
        self.active_click_indicators.clear();
        if let Err(error) = self.release_latched(backend) {
            crate::app::logging::report_error(
                "action",
                format!("cannot release held inputs: {error}"),
            );
            if result.is_ok() {
                result = Err(error);
            }
        }
        if let Err(error) = backend.dismiss() {
            crate::app::logging::report_error(
                "backend",
                format!("cannot dismiss overlay: {error}"),
            );
            if result.is_ok() {
                result = Err(error);
            }
        }
        if let Err(error) = backend.shutdown() {
            if result.is_ok() {
                result = Err(error);
            } else {
                crate::app::logging::report_error("backend", format!("shutdown failed: {error}"));
            }
        } else {
            crate::app::logging::info_args(
                "backend",
                format_args!("{} backend stopped", backend.name()),
            );
        }
        result
    }

    /// How long we may block before a timer needs servicing.
    fn next_timeout(&self) -> Duration {
        const MAX: Duration = Duration::from_millis(50);
        let now = Instant::now();
        self.timers
            .values()
            .map(|t| t.fires_at)
            .chain(
                self.pending_sequences
                    .iter()
                    .map(|sequence| sequence.fires_at),
            )
            .chain(
                self.pending_long_press_toggles
                    .iter()
                    .map(|pending| pending.fires_at),
            )
            .map(|fires_at| fires_at.saturating_duration_since(now))
            .min()
            .unwrap_or(MAX)
            .min(MAX)
    }

    fn fire_due_long_press_toggles(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.should_quit || !self.enabled || self.is_excluded_app() {
            self.pending_long_press_toggles.clear();
            return Ok(());
        }
        let now = Instant::now();
        let mut due = Vec::new();
        let mut pending = Vec::with_capacity(self.pending_long_press_toggles.len());
        for toggle in self.pending_long_press_toggles.drain(..) {
            if toggle.fires_at <= now {
                due.push(toggle);
            } else {
                pending.push(toggle);
            }
        }
        self.pending_long_press_toggles = pending;
        due.sort_by_key(|toggle| toggle.fires_at);
        for toggle in due {
            if !self.pressed.contains(&toggle.key) {
                continue;
            }
            let target = InputTarget::Mouse(toggle.button);
            self.toggle_targets(std::slice::from_ref(&target), backend)?;
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    fn fire_due_sequences(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let now = Instant::now();
        let mut due = Vec::new();
        let mut pending = Vec::with_capacity(self.pending_sequences.len());
        for sequence in self.pending_sequences.drain(..) {
            if sequence.fires_at <= now {
                due.push(sequence);
            } else {
                pending.push(sequence);
            }
        }
        self.pending_sequences = pending;
        due.sort_by_key(|sequence| sequence.fires_at);
        for sequence in due {
            if let Err(error) =
                self.continue_sequence(sequence.actions, sequence.owner, sequence.input, backend)
            {
                crate::app::logging::report_error(
                    "action",
                    format!("delayed action sequence stopped: {error}"),
                );
            }
        }
        Ok(())
    }

    fn fire_due_timers(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let now = Instant::now();
        let mut due: Vec<String> = self
            .timers
            .iter()
            .filter(|(_, t)| t.fires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        due.sort();

        for id in due {
            let Some(owner) = self.timers.get(&id).map(|timer| timer.owner.clone()) else {
                continue;
            };
            let elapsed = self
                .timers
                .get(&id)
                .map(|timer| now.saturating_duration_since(timer.last_fired))
                .unwrap_or_default();
            match self.timers.get_mut(&id) {
                Some(timer) => match timer.interval {
                    // Re-arm from now to avoid drift storms after a stall.
                    Some(interval) => {
                        timer.last_fired = now;
                        timer.fires_at = now + interval;
                    }
                    None => {
                        self.timers.remove(&id);
                    }
                },
                None => continue,
            }
            self.trace_lazy(self.config.debug.timers, "timer", || {
                format!("fire id={id:?} owner={} elapsed={elapsed:?}", owner)
            });
            self.dispatch_to(&owner, ModeEvent::Timer { id, elapsed }, backend)?;
        }
        Ok(())
    }

    fn handle_backend_event(
        &mut self,
        event: BackendEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match event {
            BackendEvent::Input(input) => {
                self.handle_key(input, backend)?;
            }
            BackendEvent::InputInjectionFailed(message) => {
                return Err(Self::recoverable_input_error(
                    "asynchronous native input",
                    message,
                ));
            }
            BackendEvent::PointerMoved(reported) => {
                let p = self
                    .constrain_absolute_pointer(reported)
                    .unwrap_or(self.cursor);
                let changed = self.cursor != p;
                self.cursor = p;
                if changed {
                    self.trace_lazy(self.config.debug.pointer, "pointer", || {
                        format!("position=({:.1},{:.1})", p.x, p.y)
                    });
                }
                self.dispatch(ModeEvent::PointerMoved(p), backend)?;
                if changed {
                    self.refresh_overlay(backend)?;
                }
            }
            BackendEvent::Frame(elapsed) => {
                if let Some(owner) = self.frame_clock_owner.clone() {
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
                        self.binding_profile_key_for(app.as_ref()) != self.binding_profile_key
                    };
                self.focused_app = app.clone();
                if profile_changed {
                    self.rebuild_tables();
                    self.trace_binding_tables();
                }
                self.dispatch(ModeEvent::FocusChanged(app), backend)?;
            }
            BackendEvent::ScreensChanged(screens) => {
                self.screens = screens.clone();
                self.dispatch(ModeEvent::ScreensChanged(screens), backend)?;
            }
            BackendEvent::AppearanceChanged(appearance) => {
                self.appearance = appearance;
                self.palette = self.config.palette(appearance);
                self.refresh_overlay(backend)?;
            }
            BackendEvent::UiScanned(result) => {
                match &result.status {
                    UiScanStatus::Failed(error) => crate::app::logging::report_error(
                        "ui-scan",
                        format!("scan {} failed: {error}", result.id),
                    ),
                    UiScanStatus::PermissionDenied(error) | UiScanStatus::Unsupported(error) => {
                        crate::app::logging::report_warning_args(
                            "ui-scan",
                            format_args!("scan {} unavailable: {error}", result.id),
                        );
                    }
                    UiScanStatus::TimedOut => crate::app::logging::report_warning_args(
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
                if owner.as_ref() == Some(&self.active) {
                    self.dispatch(ModeEvent::UiScanned(result), backend)?;
                }
            }
            BackendEvent::ReloadConfig => {
                if let Err(error) = self.reload_config(backend) {
                    crate::app::logging::report_error("config", error);
                }
            }
            BackendEvent::ToggleEnabled => {
                self.enabled = !self.enabled;
                backend.set_enabled(self.enabled)?;
                if !self.enabled {
                    self.pending_sequences.clear();
                    self.pending_long_press_toggles.clear();
                    self.active_click_indicators.clear();
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
                Err(error) => crate::app::logging::report_error("autostart", error),
            },
            BackendEvent::CheckForUpdates => {
                if let Err(error) = backend.check_for_updates() {
                    crate::app::logging::report_error("update-check", error);
                }
            }
            BackendEvent::UpdateProgress(progress) => {
                if let Err(error) = backend.present_update_progress(&progress) {
                    crate::app::logging::report_error("update-check", error);
                }
            }
            BackendEvent::UpdateChecked(result) => {
                if let Err(error) = backend.present_update_result(&result) {
                    crate::app::logging::report_error("update-check", error);
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
        let discovered_path = if let Some(directory) = self.config_discovery_directory.clone() {
            let (config, store, path) = match Config::discover_in(&directory) {
                Some(path) => {
                    let config = Config::load(&path).map_err(|error| {
                        format!(
                            "configuration reload rejected; keeping the last valid configuration: {error}"
                        )
                    })?;
                    let store = ConfigStore::open(&path, &config).map_err(|error| {
                        format!(
                            "configuration reload rejected; keeping the last valid configuration: {error}"
                        )
                    })?;
                    (config, store, Some(path))
                }
                None => {
                    let config = Config::default();
                    let path = directory.join("keysteer.user.toml");
                    let store = ConfigStore::open(path, &config).map_err(|error| {
                        format!(
                            "configuration reload rejected; keeping the last valid configuration: {error}"
                        )
                    })?;
                    (config, store, None)
                }
            };
            self.apply_config(config)?;
            self.config_store = Some(store);
            path
        } else if let Some(store) = self.config_store.as_mut() {
            match store.reload() {
                Ok(config) => self.apply_config(config)?,
                Err(error) => {
                    return Err(format!(
                        "configuration reload rejected; keeping the last valid configuration: {error}"
                    ));
                }
            }
            self.config_store
                .as_ref()
                .map(|store| store.path().to_path_buf())
        } else {
            self.palette = self.config.palette(self.appearance);
            self.rebuild_tables();
            None
        };
        self.notify_config_reloaded(backend)?;
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

    /// Swap in a new configuration at runtime.
    pub fn apply_config(&mut self, config: Config) -> Result<(), String> {
        config.validate().map_err(|e| e.to_string())?;
        crate::app::logging::set_non_error_enabled(config.debug.enabled);
        self.config = config;
        self.palette = self.config.palette(self.appearance);
        self.pending_long_press_toggles.clear();
        self.rebuild_tables();
        self.trace_binding_tables();
        Ok(())
    }

    fn handle_key(
        &mut self,
        input: crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        // Holding a key can generate dozens of repeats per second. The normal
        // debug stream records the physical down/up edges; opt into `motion`
        // only when every OS repeat is needed for a performance trace.
        let trace_key = self.config.debug.keys && (!input.repeat || self.config.debug.motion);
        // Never re-process our own injected events, or modes would loop.
        if input.injected {
            return self.dispose_input(&input, KeyOutcome::Forwarded, trace_key, backend);
        }

        let display_before = self.display_mode();
        let completed_default_toggle = match input.state {
            KeyState::Down => {
                self.pressed.insert(input.key.clone());
                None
            }
            KeyState::Up => {
                self.pressed.remove(&input.key);
                self.active_default_toggles.remove(&input.key)
            }
        };
        let click_indicator_released =
            input.state == KeyState::Up && self.active_click_indicators.release(&input.key);
        if input.state == KeyState::Up {
            self.pending_long_press_toggles
                .retain(|pending| pending.key != input.key);
        }
        let captures_default_toggle_partner = input.state == KeyState::Down
            && !input.repeat
            && self
                .active_default_toggles
                .keys()
                .any(|key| key != &input.key && self.pressed.contains(key));
        let display_changed = display_before != self.display_mode();

        if !self.enabled || self.is_excluded_app() {
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Forwarded);
            return self.dispose_input(&input, outcome, trace_key, backend);
        }

        // Resolve the key. On press we consult the active mode's table; on
        // release we use the gesture that press started, because the chord no
        // longer matches once the keys are up.
        let bound = match input.state {
            KeyState::Down if input.repeat => {
                self.active_gestures
                    .get(&input.key)
                    .cloned()
                    .map(|gesture| ResolvedBinding {
                        binding: gesture.binding,
                        owner: gesture.owner,
                    })
            }
            KeyState::Down => self.lookup(&input.key),
            KeyState::Up => {
                self.active_gestures
                    .remove(&input.key)
                    .map(|gesture| ResolvedBinding {
                        binding: gesture.binding,
                        owner: gesture.owner,
                    })
            }
        };

        if let Some(resolved) = bound {
            // A key pressed while a parameterless toggle activation key is held
            // becomes that toggle's target. Suppress its ordinary click/send/
            // movement action so the combination has exactly one effect.
            if captures_default_toggle_partner {
                let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
                self.dispose_input(&input, outcome, trace_key, backend)?;
                self.trace_key_resolution(&input, Some(&resolved), trace_key);
                if let Err(error) = self.capture_default_toggle_partner(
                    &input.key,
                    Some(&resolved.binding),
                    backend,
                ) {
                    self.report_action_error(error, backend);
                }
                return Ok(());
            }

            // Remember both the binding and its recipient so a release stops a
            // normal gesture even while grid, recursive_grid or ui_hint remains
            // the active mode.
            if input.state == KeyState::Down && resolved.binding.is_held() {
                self.active_gestures.insert(
                    input.key.clone(),
                    ActiveGesture {
                        binding: resolved.binding.clone(),
                        owner: resolved.owner.clone(),
                    },
                );
            }
            // The hook only waits for this disposition. Send it before mouse
            // injection, process spawning or overlay painting can block.
            let outcome = self.complete_key_disposition(&input, KeyOutcome::Consumed);
            self.dispose_input(&input, outcome, trace_key, backend)?;
            self.trace_key_resolution(&input, Some(&resolved), trace_key);
            if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
                self.report_action_error(error, backend);
            }
            let pending_long_press = self.pending_long_press_toggle(&resolved, &input);
            let suppress_click = pending_long_press.as_ref().is_some_and(|pending| {
                self.matching_latched_target(&InputTarget::Mouse(pending.button))
                    .is_some()
            });
            let applied = if suppress_click {
                true
            } else {
                match self.apply_binding(&resolved, &input, backend) {
                    Ok(_) => true,
                    Err(error) => {
                        self.report_action_error(error, backend);
                        false
                    }
                }
            };
            if applied && let Some(pending) = pending_long_press {
                self.pending_long_press_toggles
                    .retain(|current| current.key != pending.key);
                self.pending_long_press_toggles.push(pending);
            }
            if display_changed || click_indicator_released {
                self.refresh_overlay(backend)?;
            }
            return Ok(());
        }

        let captures = self
            .modes
            .get(&self.active)
            .map(|m| m.captures_keyboard())
            .unwrap_or(false);

        let outcome = if captures {
            KeyOutcome::Consumed
        } else {
            KeyOutcome::Forwarded
        };
        let outcome = self.complete_key_disposition(&input, outcome);
        self.dispose_input(&input, outcome, trace_key, backend)?;
        self.trace_key_resolution(&input, None, trace_key);
        if let Err(error) = self.finish_default_toggle(completed_default_toggle, backend) {
            self.report_action_error(error, backend);
        }
        if captures_default_toggle_partner {
            if let Err(error) = self.capture_default_toggle_partner(&input.key, None, backend) {
                self.report_action_error(error, backend);
            }
            return Ok(());
        }

        // Raw-mode handling may redraw a large target scene, so it must also
        // happen after the hook has received its disposition.
        self.dispatch(
            ModeEvent::Key {
                key: input.key.clone(),
                state: input.state,
                repeat: input.repeat,
            },
            backend,
        )?;
        if display_changed || click_indicator_released {
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

    fn dispose_input(
        &self,
        input: &crate::api::input::InputEvent,
        outcome: KeyOutcome,
        trace_key: bool,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let disposition = match outcome {
            KeyOutcome::Consumed => KeyDisposition::Consume,
            KeyOutcome::Forwarded => KeyDisposition::Forward,
        };
        backend.dispose_key(disposition)?;
        self.trace_lazy(trace_key, "key", || {
            format!(
                "received key={} state={:?} repeat={} injected={} mode={} disposition={disposition:?}",
                input.key, input.state, input.repeat, input.injected, self.active
            )
        });
        Ok(())
    }

    fn trace_key_resolution(
        &self,
        input: &crate::api::input::InputEvent,
        bound: Option<&ResolvedBinding>,
        trace_key: bool,
    ) {
        self.trace_lazy(trace_key, "resolve", || match bound {
            Some(resolved) => format!(
                "key={} pressed={:?} mode={} owner={} action={:?}",
                input.key, self.pressed, self.active, resolved.owner, resolved.binding
            ),
            None => format!(
                "key={} pressed={:?} mode={} action=<unbound>",
                input.key, self.pressed, self.active
            ),
        });
        if bound.is_none()
            && input.state == KeyState::Down
            && !input.key.is_modifier()
            && self.active == ModeId::idle()
        {
            self.trace_lazy(trace_key, "resolve", || {
                let available = self
                    .tables
                    .get(&ModeId::idle())
                    .map(CompiledKeymap::entries)
                    .unwrap_or_default();
                format!(
                    "idle chord did not match; pressed={:?}; configured launchers={available:?}",
                    self.pressed
                )
            });
        }
    }

    fn complete_key_disposition(
        &mut self,
        input: &crate::api::input::InputEvent,
        current: KeyOutcome,
    ) -> KeyOutcome {
        match input.state {
            KeyState::Down => {
                if let Some(disposition) = self.key_dispositions.get(&input.key) {
                    return match disposition {
                        KeyDisposition::Consume => KeyOutcome::Consumed,
                        KeyDisposition::Defer | KeyDisposition::Forward => KeyOutcome::Forwarded,
                    };
                }
                let disposition = match current {
                    KeyOutcome::Consumed => KeyDisposition::Consume,
                    KeyOutcome::Forwarded => KeyDisposition::Forward,
                };
                self.key_dispositions.insert(input.key.clone(), disposition);
                current
            }
            KeyState::Up => match self.key_dispositions.remove(&input.key) {
                Some(KeyDisposition::Consume) => KeyOutcome::Consumed,
                Some(KeyDisposition::Defer | KeyDisposition::Forward) => KeyOutcome::Forwarded,
                None => current,
            },
        }
    }

    fn temporary_mode_is_active(&self, temporary_keys: &[String]) -> bool {
        temporary_keys.iter().any(|name| {
            let reserved_for_overlap = self.active == ModeId::ui_hint()
                && self.config.ui_hint.overlap_cycle_conflicts_with(name);
            !reserved_for_overlap
                && KeyChord::parse(name).is_ok_and(|chord| chord.matches_pressed(&self.pressed))
        })
    }

    fn display_mode(&self) -> ModeId {
        if self.active == ModeId::idle() {
            return self.active.clone();
        }
        let Some((_, temporary_mode, temporary_keys)) =
            self.config.inheritance_for(self.active.as_str())
        else {
            return self.active.clone();
        };
        let temporary_active = self.temporary_mode_is_active(temporary_keys);
        if temporary_active
            && let Some(source) = temporary_mode
            && let Ok(target) = Self::source_mode_id(source)
            && self.modes.contains_key(&target)
        {
            return target;
        }
        self.active.clone()
    }

    /// Find the binding for `key` using the compiled mode precedence rules.
    fn lookup(&self, key: &Key) -> Option<ResolvedBinding> {
        let active_match = self.lookup_with_specificity_in(&self.active, key);
        if self.active == ModeId::idle() {
            return active_match.map(|(binding, _)| ResolvedBinding {
                binding,
                owner: self.active.clone(),
            });
        }

        if self.active == ModeId::ui_hint() && self.config.ui_hint.overlap_cycle_matches(key) {
            return None;
        }

        let Some((inherits, temporary_mode, temporary_keys)) =
            self.config.inheritance_for(self.active.as_str())
        else {
            return active_match.map(|(binding, _)| ResolvedBinding {
                binding,
                owner: self.active.clone(),
            });
        };
        let temporary_active = self.temporary_mode_is_active(temporary_keys);
        let temporary_match = temporary_active
            .then_some(temporary_mode)
            .flatten()
            .and_then(|source| Self::source_mode_id(source).ok())
            .and_then(|owner| {
                self.lookup_with_specificity_in(&owner, key)
                    .map(|(binding, specificity)| (binding, owner, specificity))
            });

        if let Some((binding, owner, temporary_specificity)) = temporary_match
            && active_match
                .as_ref()
                .is_none_or(|(_, active_specificity)| *active_specificity <= temporary_specificity)
        {
            return Some(ResolvedBinding { binding, owner });
        }

        match active_match {
            Some((binding, _)) if matches!(binding.as_ref(), Binding::Disabled) => None,
            Some((binding, _)) => Some(ResolvedBinding {
                binding,
                owner: self.active.clone(),
            }),
            None if self.active_claims_raw_key(key) => None,
            None => inherits.iter().find_map(|source| {
                let owner = Self::source_mode_id(source).ok()?;
                if owner == self.active {
                    return None;
                }
                self.lookup_inherited(&owner, key, &mut BTreeSet::new())
            }),
        }
    }

    fn lookup_inherited(
        &self,
        owner: &ModeId,
        key: &Key,
        visited: &mut BTreeSet<ModeId>,
    ) -> Option<ResolvedBinding> {
        if !visited.insert(owner.clone()) {
            return None;
        }
        if let Some(binding) = self.lookup_in(owner, key) {
            return (binding.as_ref() != &Binding::Disabled).then(|| ResolvedBinding {
                binding,
                owner: owner.clone(),
            });
        }
        let (sources, _, _) = self.config.inheritance_for(owner.as_str())?;
        sources.iter().find_map(|source| {
            let source = Self::source_mode_id(source).ok()?;
            self.lookup_inherited(&source, key, visited)
        })
    }

    fn active_claims_raw_key(&self, key: &Key) -> bool {
        if self.active == ModeId::ui_hint() && self.config.ui_hint.overlap_cycle_matches(key) {
            return true;
        }
        let Some(character) = key.as_char() else {
            return false;
        };
        match self.active.as_str() {
            "grid" => self.config.grid.keys.contains(character),
            "recursive_grid" => {
                self.config.recursive_grid.keys.contains(character)
                    || self
                        .config
                        .recursive_grid
                        .layers
                        .iter()
                        .filter_map(|layer| layer.keys.as_ref())
                        .any(|keys| keys.contains(character))
            }
            "ui_hint" => {
                character.is_ascii_alphanumeric()
                    || self.config.ui_hint.hint_characters.contains(character)
            }
            _ => false,
        }
    }

    fn source_mode_id(source: &str) -> Result<ModeId, String> {
        if source == "hotkeys" {
            Ok(ModeId::idle())
        } else {
            ModeId::new(source)
        }
    }

    fn lookup_in(&self, mode: &ModeId, key: &Key) -> Option<Arc<Binding>> {
        self.lookup_with_specificity_in(mode, key)
            .map(|(binding, _)| binding)
    }

    fn lookup_with_specificity_in(
        &self,
        mode: &ModeId,
        key: &Key,
    ) -> Option<(Arc<Binding>, usize)> {
        let table = self.tables.get(mode)?;
        if self.strict_modifier_matching_enabled() {
            table.lookup_with_specificity_strict(key, &self.pressed, |modifier| {
                matches!(
                    self.key_dispositions.get(modifier),
                    Some(KeyDisposition::Consume)
                )
            })
        } else {
            table.lookup_with_specificity(key, &self.pressed)
        }
    }

    fn strict_modifier_matching_enabled(&self) -> bool {
        self.active == ModeId::idle()
            || (self.active == ModeId::normal() && self.config.normal.passthrough_unbound_keys)
    }

    fn injected_key(key: &Key) -> Key {
        let concrete = match key.as_str() {
            "shift" => "left_shift",
            "ctrl" => "left_ctrl",
            "alt" => "left_alt",
            "win" => "left_win",
            _ => return key.clone(),
        };
        Key::new(concrete).unwrap_or_else(|_| key.clone())
    }

    fn modifier_family(key: &Key) -> Option<&'static str> {
        match key.as_str() {
            "shift" | "left_shift" | "right_shift" => Some("shift"),
            "ctrl" | "left_ctrl" | "right_ctrl" => Some("ctrl"),
            "alt" | "left_alt" | "right_alt" => Some("alt"),
            "win" | "left_win" | "right_win" => Some("win"),
            _ => None,
        }
    }

    fn targets_match(left: &InputTarget, right: &InputTarget) -> bool {
        match (left, right) {
            (InputTarget::Key(left), InputTarget::Key(right)) => {
                if left == right {
                    return true;
                }
                let left_generic = matches!(left.as_str(), "shift" | "ctrl" | "alt" | "win");
                let right_generic = matches!(right.as_str(), "shift" | "ctrl" | "alt" | "win");
                (left_generic || right_generic)
                    && Self::modifier_family(left)
                        .zip(Self::modifier_family(right))
                        .is_some_and(|(left, right)| left == right)
            }
            (InputTarget::Mouse(left), InputTarget::Mouse(right)) => left == right,
            _ => false,
        }
    }

    fn matching_latched_target(&self, target: &InputTarget) -> Option<InputTarget> {
        self.latched
            .iter()
            .find(|latched| Self::targets_match(latched, target))
            .cloned()
    }

    fn latched_key_matches(&self, key: &Key) -> bool {
        self.matching_latched_target(&InputTarget::Key(key.clone()))
            .is_some()
    }

    fn pending_long_press_toggle(
        &self,
        resolved: &ResolvedBinding,
        input: &crate::api::input::InputEvent,
    ) -> Option<PendingLongPressToggle> {
        if self.config.normal.long_press_toggle_ms == 0
            || resolved.owner != ModeId::normal()
            || input.state != KeyState::Down
            || input.repeat
            || input.injected
        {
            return None;
        }
        let button = match resolved.binding.as_ref() {
            Binding::Click(button) | Binding::DoubleClick(button) => *button,
            _ => return None,
        };
        Some(PendingLongPressToggle {
            fires_at: Instant::now()
                + Duration::from_millis(self.config.normal.long_press_toggle_ms),
            key: input.key.clone(),
            button,
        })
    }

    fn cancel_pending_long_press_targets(&mut self, targets: &[InputTarget]) {
        self.pending_long_press_toggles.retain(|pending| {
            !targets.iter().any(
                |target| matches!(target, InputTarget::Mouse(button) if button == &pending.button),
            )
        });
    }

    fn toggle_partner_targets(key: &Key, binding: Option<&Binding>) -> Vec<InputTarget> {
        if let Some(Binding::Click(button) | Binding::DoubleClick(button)) = binding {
            return vec![InputTarget::Mouse(*button)];
        }
        if key.is_modifier() {
            return vec![InputTarget::Key(key.clone())];
        }

        fn append(binding: &Binding, targets: &mut BTreeSet<InputTarget>) {
            match binding {
                Binding::Click(button) | Binding::DoubleClick(button) => {
                    targets.insert(InputTarget::Mouse(*button));
                }
                Binding::Send(chord) => {
                    targets.extend(chord.keys().iter().cloned().map(InputTarget::Key));
                }
                Binding::Press(items) | Binding::Release(items) | Binding::Toggle(items)
                    if !items.is_empty() =>
                {
                    targets.extend(items.iter().cloned());
                }
                Binding::Sequence(actions) => {
                    for action in actions {
                        append(action, targets);
                    }
                }
                _ => {}
            }
        }

        let mut targets = BTreeSet::new();
        if let Some(binding) = binding {
            append(binding, &mut targets);
        }
        if targets.is_empty() {
            targets.insert(InputTarget::Key(key.clone()));
        }
        targets.into_iter().collect()
    }

    fn pressed_toggle_targets(&self, activation: &Key) -> Vec<InputTarget> {
        let partners: Vec<Key> = self
            .pressed
            .iter()
            .filter(|key| *key != activation)
            .cloned()
            .collect();
        let mut targets = BTreeSet::new();
        for key in partners {
            let resolved = self.lookup(&key);
            targets.extend(Self::toggle_partner_targets(
                &key,
                resolved.as_ref().map(|item| item.binding.as_ref()),
            ));
        }
        targets.into_iter().collect()
    }

    fn capture_default_toggle_partner(
        &mut self,
        key: &Key,
        binding: Option<&Binding>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let targets = Self::toggle_partner_targets(key, binding);
        for used in self.active_default_toggles.values_mut() {
            *used = true;
        }
        self.cancel_pending_long_press_targets(&targets);
        self.toggle_targets(&targets, backend)?;
        self.refresh_overlay(backend)
    }

    fn finish_default_toggle(
        &mut self,
        used: Option<bool>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if used == Some(false) && !self.latched.is_empty() {
            self.release_latched(backend)?;
            self.refresh_overlay(backend)?;
        }
        Ok(())
    }

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
                if self.pending_sequences.len() >= MAX_PENDING_SEQUENCES {
                    return Err("too many action sequences are waiting".into());
                }
                let delay = random_wait_ms(min_ms, max_ms);
                self.pending_sequences.push(PendingSequence {
                    fires_at: Instant::now() + Duration::from_millis(delay),
                    actions,
                    owner,
                    input,
                });
                return Ok(());
            }
            let nested = ResolvedBinding {
                binding: Arc::new(action),
                owner: owner.clone(),
            };
            self.apply_binding(&nested, &input, backend)?;
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
        resolved: &ResolvedBinding,
        input: &crate::api::input::InputEvent,
        backend: &mut dyn Backend,
    ) -> Result<bool, String> {
        let binding = resolved.binding.as_ref();
        let is_press = input.state == KeyState::Down;
        self.trace_lazy(
            self.config.debug.actions && (!input.repeat || self.config.debug.motion),
            "action",
            || {
                format!(
                    "phase={:?} owner={} active={} action={binding:?}",
                    input.state, resolved.owner, self.active
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

        match binding {
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
                        self.apply_binding(&nested, input, backend)?;
                    }
                }
                Ok(true)
            }

            Binding::Mode(id) => {
                if !is_press {
                    return Ok(true);
                }
                if !self.modes.contains_key(id) {
                    crate::report_warning!(
                        "binding",
                        "binding targets unknown mode {:?}; is the plugin registered?",
                        id.as_str()
                    );
                    return Ok(true);
                }
                // Pressing a mode's own key while it is active leaves it.
                let next = if *id == self.active {
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
                self.activate(next, Some(self.active.clone()), backend)?;
                Ok(true)
            }

            Binding::Invoke { verb, args } => {
                if !is_press {
                    return Ok(true);
                }
                let Some(owner) = self.plugin_verbs.get(verb).cloned() else {
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
                    self.pending_sequences.clear();
                    // `press`/`toggle` are explicit engine-wide latches, not
                    // mode-owned gestures. Escape changes mode but must not
                    // synthesize an Up edge for them.
                    self.activate(ModeId::idle(), Some(self.active.clone()), backend)?;
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
                    vec![Command::WarpPointer {
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
                    vec![Command::FinishMode {
                        cause: FinishCause::Explicit,
                    }],
                    backend,
                )?;
                Ok(true)
            }
            Binding::RestartMode => {
                self.execute(vec![Command::RestartMode], backend)?;
                Ok(true)
            }
            Binding::SetConfig { path, value } => {
                self.execute(
                    vec![Command::SetConfigValue {
                        path: path.clone(),
                        value: value.clone(),
                    }],
                    backend,
                )?;
                Ok(true)
            }

            Binding::Click(button) => {
                self.execute(vec![Command::click(map_button(*button))], backend)?;
                self.activate_click_indicator(input, *button, backend)?;
                Ok(true)
            }
            Binding::DoubleClick(button) => {
                self.execute(
                    vec![Command::MouseButton {
                        button: map_button(*button),
                        action: ButtonAction::DoubleClick,
                    }],
                    backend,
                )?;
                self.activate_click_indicator(input, *button, backend)?;
                Ok(true)
            }
            Binding::Press(targets) => {
                self.cancel_pending_long_press_targets(targets);
                self.press_targets(targets, backend)?;
                self.refresh_overlay(backend)?;
                Ok(true)
            }
            Binding::Release(targets) => {
                self.cancel_pending_long_press_targets(targets);
                self.release_targets(targets, true, backend)?;
                self.refresh_overlay(backend)?;
                Ok(true)
            }
            Binding::Toggle(targets) => {
                if targets.is_empty() {
                    let inferred = self.pressed_toggle_targets(&input.key);
                    let used = !inferred.is_empty();
                    if used {
                        self.cancel_pending_long_press_targets(&inferred);
                        self.toggle_targets(&inferred, backend)?;
                        self.refresh_overlay(backend)?;
                    }
                    if self.pressed.contains(&input.key) {
                        self.active_default_toggles.insert(input.key.clone(), used);
                    }
                } else {
                    self.cancel_pending_long_press_targets(targets);
                    self.toggle_targets(targets, backend)?;
                    self.refresh_overlay(backend)?;
                }
                Ok(true)
            }
            Binding::Wait { .. } => Ok(true),

            // Stateful gestures and mode-specific discrete actions are owned
            // by the active mode, which alone knows their session state.
            Binding::Move(_)
            | Binding::Scroll(..)
            | Binding::Speed(_)
            | Binding::ToggleCursorFollowSelection
            | Binding::RescanUi => {
                self.dispatch_to(
                    &resolved.owner,
                    ModeEvent::Binding {
                        binding: binding.clone(),
                        state: input.state,
                        key: input.key.clone(),
                    },
                    backend,
                )?;
                Ok(true)
            }

            // Filtered out when the table was built.
            Binding::Disabled => Ok(false),
        }
    }

    fn activate_click_indicator(
        &mut self,
        input: &crate::api::input::InputEvent,
        button: Button,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        // Plugin/lifecycle actions have no physical release edge. A delayed
        // sequence retains its original key-down event, so also require that
        // the activation key is still physically held when the click runs.
        if input.injected
            || input.state != KeyState::Down
            || input.repeat
            || !self.pressed.contains(&input.key)
        {
            return Ok(());
        }
        self.active_click_indicators
            .activate(input.key.clone(), button);
        self.refresh_overlay(backend)
    }

    fn inject_target(
        target: &InputTarget,
        state: KeyState,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match target {
            InputTarget::Key(key) => backend.send_key(&Self::injected_key(key), state),
            InputTarget::Mouse(button) => backend.mouse_button(
                map_button(*button),
                match state {
                    KeyState::Down => ButtonAction::Press,
                    KeyState::Up => ButtonAction::Release,
                },
            ),
        }
    }

    fn press_targets(
        &mut self,
        targets: &[InputTarget],
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let mut pressed = Vec::new();
        for target in targets {
            if self.matching_latched_target(target).is_some() {
                continue;
            }
            let actual = match target {
                InputTarget::Key(key) => InputTarget::Key(Self::injected_key(key)),
                InputTarget::Mouse(button) => InputTarget::Mouse(*button),
            };
            if let Err(error) = Self::inject_target(&actual, KeyState::Down, backend) {
                for rollback in pressed.iter().rev() {
                    match Self::inject_target(rollback, KeyState::Up, backend) {
                        Ok(()) => {
                            self.latched.remove(rollback);
                        }
                        Err(rollback_error) => crate::app::logging::report_error(
                            "action",
                            format!("cannot roll back held input: {rollback_error}"),
                        ),
                    }
                }
                return Err(Self::recoverable_input_error("input press", error));
            }
            self.latched.insert(actual.clone());
            pressed.push(actual);
        }
        Ok(())
    }

    fn release_targets(
        &mut self,
        targets: &[InputTarget],
        force: bool,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let mut first_error = None;
        for target in targets.iter().rev() {
            let matched = self.matching_latched_target(target);
            if !force && matched.is_none() {
                continue;
            }
            let actual = matched.unwrap_or_else(|| match target {
                InputTarget::Key(key) => InputTarget::Key(Self::injected_key(key)),
                InputTarget::Mouse(button) => InputTarget::Mouse(*button),
            });
            match Self::inject_target(&actual, KeyState::Up, backend) {
                Ok(()) => {
                    self.latched.remove(&actual);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), |error| {
            Err(Self::recoverable_input_error("input release", error))
        })
    }

    fn toggle_targets(
        &mut self,
        targets: &[InputTarget],
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let original = self.latched.clone();
        for target in targets {
            let result = if self.matching_latched_target(target).is_some() {
                self.release_targets(std::slice::from_ref(target), false, backend)
            } else {
                self.press_targets(std::slice::from_ref(target), backend)
            };
            if let Err(error) = result {
                if let Err(rollback_error) = self.restore_latched(&original, backend) {
                    crate::app::logging::report_error(
                        "action",
                        format!("cannot roll back toggle action: {rollback_error}"),
                    );
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn restore_latched(
        &mut self,
        original: &BTreeSet<InputTarget>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let release: Vec<_> = self.latched.difference(original).cloned().collect();
        self.release_targets(&release, false, backend)?;
        let press: Vec<_> = original.difference(&self.latched).cloned().collect();
        self.press_targets(&press, backend)
    }

    fn release_latched(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let held: Vec<_> = self.latched.iter().cloned().collect();
        self.release_targets(&held, false, backend)
    }

    fn send_chord(&mut self, chord: &KeyChord, backend: &mut dyn Backend) -> Result<(), String> {
        // Do not emit an Up edge for a modifier held by `press`/`toggle`.
        let keys: Vec<Key> = chord
            .keys()
            .iter()
            .filter(|key| !self.latched_key_matches(key))
            .map(Self::injected_key)
            .collect();
        let events = keys
            .iter()
            .cloned()
            .map(|key| (key, KeyState::Down))
            .chain(keys.iter().rev().cloned().map(|key| (key, KeyState::Up)))
            .collect::<Vec<_>>();
        if let Err(error) = backend.send_keys(&events) {
            // A batch failure may mean that Windows accepted only a prefix.
            // Record every member conservatively; redundant key-up events are
            // harmless and safer than leaving a modifier held.
            self.latched.extend(keys.into_iter().map(InputTarget::Key));
            return Err(Self::recoverable_input_error("keyboard chord", error));
        }
        Ok(())
    }

    /// Send an event to the active mode and execute what it returns.
    fn dispatch(&mut self, event: ModeEvent, backend: &mut dyn Backend) -> Result<(), String> {
        let owner = self.active.clone();
        self.dispatch_to(&owner, event, backend)
    }

    /// Refresh configuration cached by every registered mode. Commands emitted
    /// by inactive modes are deliberately discarded: they may request overlays
    /// or scans that only make sense while active. The active mode runs last and
    /// its redraw/reconfiguration commands are executed normally.
    fn notify_config_reloaded(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let active = self.active.clone();
        let inactive: Vec<ModeId> = self
            .modes
            .keys()
            .filter(|id| **id != active)
            .cloned()
            .collect();

        for owner in inactive {
            let Some(mut mode) = self.modes.remove(&owner) else {
                continue;
            };
            let _ = mode.handle(&ModeEvent::ConfigReloaded, &self.context());
            self.modes.insert(owner, mode);
        }
        self.dispatch_to(&active, ModeEvent::ConfigReloaded, backend)
    }

    /// Deliver an event to a specific registered mode. This is used for normal
    /// pointer controls borrowed by an active label mode.
    fn dispatch_to(
        &mut self,
        owner: &ModeId,
        event: ModeEvent,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let Some(mut mode) = self.modes.remove(owner) else {
            return Ok(());
        };
        // The mode is detached during dispatch so it can hold `&mut self`
        // state while the engine hands it an immutable context.
        let commands = mode.handle(&event, &self.context());
        self.modes.insert(owner.clone(), mode);
        self.execute_for(owner, commands, backend)
    }

    fn push_mode(&mut self, target: ModeId, backend: &mut dyn Backend) -> Result<(), String> {
        if target == self.active || !self.modes.contains_key(&target) {
            return Ok(());
        }
        let previous = self.active.clone();
        self.dispatch(ModeEvent::Suspended, backend)?;
        self.modal_stack.push(previous.clone());
        self.active = target;
        self.dispatch(ModeEvent::Pushed { previous }, backend)
    }

    fn pop_mode(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let Some(previous) = self.modal_stack.pop() else {
            return Ok(());
        };
        let current = self.active.clone();
        self.dispatch(ModeEvent::Deactivated, backend)?;
        self.timers.retain(|_, timer| timer.owner != current);
        self.active = previous;
        self.dispatch(ModeEvent::Resumed, backend)
    }

    fn activate(
        &mut self,
        target: ModeId,
        previous: Option<ModeId>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if !self.modes.contains_key(&target) {
            self.trace_lazy(self.config.debug.modes, "mode", || {
                format!(
                    "ignored switch {} -> {target}: target is not registered",
                    self.active
                )
            });
            return Ok(());
        }

        self.trace_lazy(self.config.debug.modes, "mode", || {
            format!("switch {} -> {target}, previous={previous:?}", self.active)
        });

        if target != self.active {
            self.pending_sequences.clear();
            self.active_default_toggles.clear();
            // `latched` belongs to the engine and intentionally survives mode
            // and screen changes. Only physical held gestures are owned by the
            // outgoing mode and need a synthetic release here.
            // Deliver the release of anything still held, so the outgoing mode
            // can stop its timers rather than moving the pointer forever.
            let pending: Vec<(Key, ActiveGesture)> = std::mem::take(&mut self.active_gestures)
                .into_iter()
                .collect();
            for (key, gesture) in pending {
                self.dispatch_to(
                    &gesture.owner,
                    ModeEvent::Binding {
                        binding: gesture.binding.as_ref().clone(),
                        state: KeyState::Up,
                        key,
                    },
                    backend,
                )?;
            }

            // Tear down the outgoing mode and drop the timers it owned.
            if let Some(mut old) = self.modes.remove(&self.active) {
                let commands = old.handle(&ModeEvent::Deactivated, &self.context());
                let old_id = old.id();
                self.modes.insert(old_id.clone(), old);
                self.execute(commands, backend)?;
                self.timers.retain(|_, t| t.owner != old_id);
            }
            self.active = target;
        }

        self.dispatch(ModeEvent::Activated { previous }, backend)
    }

    fn restart_active(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let active = self.active.clone();
        self.pending_sequences
            .retain(|sequence| sequence.owner != active);
        self.timers.retain(|_, timer| timer.owner != active);
        self.dispatch(ModeEvent::Restarted, backend)
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::binding::Direction;
    use crate::api::geometry::Rect;
    use crate::api::input::InputEvent;
    use std::sync::{Arc, Mutex};

    /// Records what the engine asked of the platform.
    #[derive(Default)]
    struct Recorder {
        presents: usize,
        dismissals: usize,
        warps: Vec<Point>,
        moves: Vec<(f64, f64)>,
        scrolls: Vec<(f64, f64)>,
        scenes: Vec<OverlayScene>,
        timeline: Vec<&'static str>,
        dispositions: Vec<KeyDisposition>,
        scans: usize,
        shutdowns: usize,
        /// Button press/release/click calls, in order.
        buttons: Vec<(MouseButton, ButtonAction)>,
        /// Number of `Click` actions, for convenience.
        clicks: usize,
        /// Synthetic keystrokes, in order.
        sent: Vec<(String, KeyState)>,
        fail_next_key_up: bool,
    }

    struct FakeBackend {
        events: Vec<BackendEvent>,
        log: Arc<Mutex<Recorder>>,
        fail_start: bool,
        fail_warp: bool,
        fail_mouse: bool,
    }

    impl FakeBackend {
        fn new(events: Vec<BackendEvent>) -> (Self, Arc<Mutex<Recorder>>) {
            let log = Arc::new(Mutex::new(Recorder::default()));
            (
                Self {
                    events,
                    log: log.clone(),
                    fail_start: false,
                    fail_warp: false,
                    fail_mouse: false,
                },
                log,
            )
        }
    }

    impl Backend for FakeBackend {
        fn start(&mut self) -> Result<(), String> {
            if self.fail_start {
                Err("injected startup failure".into())
            } else {
                Ok(())
            }
        }

        fn poll(&mut self, _t: Duration) -> Result<Option<BackendEvent>, String> {
            // Quit once the script is exhausted so `run` terminates.
            Ok(Some(if self.events.is_empty() {
                BackendEvent::Quit
            } else {
                self.events.remove(0)
            }))
        }
        fn dispose_key(&mut self, d: KeyDisposition) -> Result<(), String> {
            let mut log = self.log.lock().unwrap();
            log.dispositions.push(d);
            log.timeline.push("dispose");
            Ok(())
        }
        fn screens(&self) -> Result<Vec<Screen>, String> {
            Ok(vec![Screen {
                bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
                is_primary: true,
                scale: 1.0,
                name: None,
            }])
        }
        fn pointer(&self) -> Result<Point, String> {
            Ok(Point::new(10.0, 10.0))
        }
        fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
            Ok(None)
        }
        fn warp_pointer(&self, to: Point) -> Result<(), String> {
            if self.fail_warp {
                return Err("injected warp failure".into());
            }
            self.log.lock().unwrap().warps.push(to);
            Ok(())
        }
        fn move_pointer(&self, _from: Point, dx: f64, dy: f64) -> Result<(), String> {
            let mut log = self.log.lock().unwrap();
            log.moves.push((dx, dy));
            log.timeline.push("move");
            Ok(())
        }
        fn mouse_button(&self, b: MouseButton, a: ButtonAction) -> Result<(), String> {
            if self.fail_mouse {
                return Err("SendInput blocked by UIPI".into());
            }
            let mut log = self.log.lock().unwrap();
            log.buttons.push((b, a));
            log.timeline.push("mouse");
            if a == ButtonAction::Click {
                log.clicks += 1;
            }
            Ok(())
        }
        fn scroll(&self, dx: f64, dy: f64) -> Result<(), String> {
            self.log.lock().unwrap().scrolls.push((dx, dy));
            Ok(())
        }
        fn send_key(&self, k: &Key, s: KeyState) -> Result<(), String> {
            let mut log = self.log.lock().unwrap();
            if s == KeyState::Up && log.fail_next_key_up {
                log.fail_next_key_up = false;
                return Err("injected key-up failure".into());
            }
            log.sent.push((k.as_str().to_string(), s));
            Ok(())
        }
        fn set_frame_clock(&mut self, _active: bool) -> Result<(), String> {
            Ok(())
        }
        fn present(&mut self, scene: Arc<OverlayScene>) -> Result<(), String> {
            let mut log = self.log.lock().unwrap();
            log.presents += 1;
            log.scenes.push(scene.as_ref().clone());
            log.timeline.push("present");
            Ok(())
        }
        fn dismiss(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().dismissals += 1;
            Ok(())
        }
        fn request_ui_scan(&mut self, _request: crate::api::UiScanRequest) -> Result<(), String> {
            self.log.lock().unwrap().scans += 1;
            Ok(())
        }
        fn shutdown(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().shutdowns += 1;
            Ok(())
        }
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    #[test]
    fn rejected_mouse_injection_does_not_stop_the_engine() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        let mut normal = ProbeMode::new("normal", seen);
        normal.on_key = vec![Command::click(MouseButton::Right)];
        engine.register(Box::new(idle));
        engine.register(Box::new(normal));

        let events = enter_normal().into_iter().chain([key_down("x")]).collect();
        let (mut backend, log) = FakeBackend::new(events);
        backend.fail_mouse = true;

        assert!(engine.run(&mut backend).is_ok());
        assert!(engine.input_failure_active);
        assert_eq!(engine.active_mode(), &ModeId::idle());
        assert_eq!(log.lock().unwrap().clicks, 0);
        assert_eq!(log.lock().unwrap().shutdowns, 1);
    }

    /// Minimal mode that records the events it saw and emits scripted commands.
    struct ProbeMode {
        id: ModeId,
        captures: bool,
        seen: Arc<Mutex<Vec<String>>>,
        on_key: Vec<Command>,
        on_reload: Vec<Command>,
    }

    impl ProbeMode {
        fn new(id: &str, seen: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                id: ModeId::new(id).unwrap(),
                captures: true,
                seen,
                on_key: Vec::new(),
                on_reload: Vec::new(),
            }
        }
    }

    impl Mode for ProbeMode {
        fn id(&self) -> ModeId {
            self.id.clone()
        }
        fn captures_keyboard(&self) -> bool {
            self.captures
        }
        fn handle(&mut self, event: &ModeEvent, _ctx: &HostContext<'_>) -> Vec<Command> {
            let label = match event {
                ModeEvent::Activated { .. } => "activated",
                ModeEvent::Pushed { .. } => "pushed",
                ModeEvent::Deactivated => "deactivated",
                ModeEvent::Suspended => "suspended",
                ModeEvent::Resumed => "resumed",
                ModeEvent::Restarted => "restarted",
                ModeEvent::FinishRequested { .. } => "finish_requested",
                ModeEvent::Clicked { .. } => "clicked",
                ModeEvent::Key { .. } => "key",
                ModeEvent::Binding { binding, .. } => &format!("binding({binding})"),
                ModeEvent::Invoked { .. } => "invoked",
                ModeEvent::Timer { .. } => "timer",
                ModeEvent::ScreensChanged(_) => "screens",
                ModeEvent::ScreenRetargeted { .. } => "retargeted",
                ModeEvent::UiScanned(_) => "scanned",
                ModeEvent::PointerMoved(_) => "pointer",
                ModeEvent::Frame { .. } => "frame",
                ModeEvent::FocusChanged(_) => "focus",
                ModeEvent::ConfigReloaded => "reloaded",
            };
            self.seen
                .lock()
                .unwrap()
                .push(format!("{}:{label}", self.id));
            match event {
                ModeEvent::Key { .. } => self.on_key.clone(),
                ModeEvent::ConfigReloaded => self.on_reload.clone(),
                _ => Vec::new(),
            }
        }
    }

    fn key_event(name: &str, state: KeyState) -> BackendEvent {
        BackendEvent::Input(InputEvent {
            key: Key::new(name).unwrap(),
            state,
            repeat: false,
            injected: false,
            timestamp_millis: 0,
        })
    }

    fn key_down(name: &str) -> BackendEvent {
        key_event(name, KeyState::Down)
    }

    fn key_up(name: &str) -> BackendEvent {
        key_event(name, KeyState::Up)
    }

    /// An engine whose `normal` table contains exactly `chord -> binding`,
    /// with probes registered for idle and normal.
    fn engine_with_normal_binding(chord: &str, binding: &str) -> Engine {
        engine_with_normal_action(chord, Binding::parse(binding).unwrap())
    }

    fn engine_with_normal_action(chord: &str, binding: Binding) -> Engine {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config.normal.bindings.insert(chord.into(), binding);
        // Keep a way back out so the mode is escapable.
        config.normal.bindings.insert("esc".into(), Binding::Escape);

        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));
        engine
    }

    /// Like [`engine_with_normal_binding`], but the caller keeps the probe log
    /// so it can assert on what the mode saw.
    fn engine_with_normal_probes(
        seen: &Arc<Mutex<Vec<String>>>,
        chord: &str,
        binding: &str,
    ) -> Engine {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert(chord.into(), Binding::parse(binding).unwrap());

        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
        engine
    }

    /// Press then release every key of `chord`, in a realistic order.
    ///
    /// Modifiers go down first and come up last, which is what a keyboard
    /// actually produces.
    fn tap_chord(chord: &str) -> Vec<BackendEvent> {
        let parsed = KeyChord::parse(chord).unwrap();
        let (modifiers, keys): (Vec<&Key>, Vec<&Key>) =
            parsed.keys().iter().partition(|k| k.is_modifier());

        let mut events = Vec::new();
        for key in &modifiers {
            events.push(key_event(key.as_str(), KeyState::Down));
        }
        for key in &keys {
            events.push(key_event(key.as_str(), KeyState::Down));
        }
        for key in keys.iter().rev() {
            events.push(key_event(key.as_str(), KeyState::Up));
        }
        for key in modifiers.iter().rev() {
            events.push(key_event(key.as_str(), KeyState::Up));
        }
        events
    }

    /// The chord that enters `normal` in the default configuration.
    ///
    /// Read from the config rather than hard-coded, so changing the default
    /// cannot silently invalidate every test below.
    fn normal_launcher() -> String {
        Config::default()
            .hotkeys
            .iter()
            .find(|(_, b)| b.mode() == Some(&ModeId::normal()))
            .map(|(chord, _)| chord.clone())
            .expect("a default binding must enter normal")
    }

    /// The key sequence that enters `normal` from `idle`.
    fn enter_normal() -> Vec<BackendEvent> {
        tap_chord(&normal_launcher())
    }

    /// Run `events` after entering `normal`.
    fn run_in_normal(engine: &mut Engine, events: Vec<BackendEvent>) -> Arc<Mutex<Recorder>> {
        let mut script = enter_normal();
        script.extend(events);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        log
    }

    /// Register probes for idle, normal and one extra mode.
    fn engine_with_probes(seen: &Arc<Mutex<Vec<String>>>, extra: &[&str]) -> Engine {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
        for id in extra {
            engine.register(Box::new(ProbeMode::new(id, seen.clone())));
        }
        engine
    }

    #[test]
    fn trace_lazy_does_not_build_messages_when_debug_is_disabled() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let builds = std::cell::Cell::new(0);

        engine.trace_lazy(true, "test", || {
            builds.set(builds.get() + 1);
            "disabled globally".to_string()
        });
        assert_eq!(builds.get(), 0);

        engine.config.debug.enabled = true;
        engine.trace_lazy(false, "test", || {
            builds.set(builds.get() + 1);
            "disabled category".to_string()
        });
        assert_eq!(builds.get(), 0);
    }

    #[test]
    fn normal_run_calls_backend_shutdown_once() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(vec![BackendEvent::Quit]);
        engine.run(&mut backend).unwrap();
        assert_eq!(log.lock().unwrap().shutdowns, 1);
    }

    #[test]
    fn startup_failure_still_calls_backend_shutdown() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        backend.fail_start = true;

        assert!(engine.run(&mut backend).is_err());
        assert_eq!(log.lock().unwrap().shutdowns, 1);
    }

    #[test]
    fn config_reload_refreshes_inactive_modes_without_running_their_commands() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        let mut normal = ProbeMode::new("normal", seen.clone());
        normal.on_reload = vec![Command::MovePointer { dx: 99.0, dy: 0.0 }];
        engine.register(Box::new(normal));
        engine.register(Box::new(ProbeMode::new("grid", seen.clone())));
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine.notify_config_reloaded(&mut backend).unwrap();

        let seen = seen.lock().unwrap();
        for mode in ["idle", "normal", "grid"] {
            assert!(seen.contains(&format!("{mode}:reloaded")), "{seen:?}");
        }
        assert!(
            log.lock().unwrap().moves.is_empty(),
            "inactive mode reload commands must not reach the backend"
        );
    }

    #[test]
    fn reload_while_idle_updates_grid_max_depth_before_activation() {
        let mut initial = Config::default();
        initial.grid.max_depth = 2;
        let mut engine = Engine::new(initial.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&initial) {
            engine.register(mode);
        }
        engine.screens = vec![Screen {
            bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
            work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        }];
        let (mut backend, _) = FakeBackend::new(Vec::new());

        let mut reloaded = initial;
        reloaded.grid.max_depth = 3;
        engine.apply_config(reloaded).unwrap();
        engine.notify_config_reloaded(&mut backend).unwrap();
        engine
            .activate(ModeId::grid(), Some(ModeId::normal()), &mut backend)
            .unwrap();

        for _ in 0..2 {
            engine
                .handle_backend_event(key_down("1"), &mut backend)
                .unwrap();
            engine
                .handle_backend_event(key_up("1"), &mut backend)
                .unwrap();
        }
        assert_eq!(engine.active_mode(), &ModeId::grid());

        engine
            .handle_backend_event(key_down("1"), &mut backend)
            .unwrap();
        assert_eq!(engine.active_mode(), &ModeId::normal());
    }

    #[test]
    fn idle_enters_normal_on_the_configured_chord() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &[]);

        let (mut backend, _) = FakeBackend::new(enter_normal());
        engine.run(&mut backend).unwrap();

        let log = seen.lock().unwrap().clone();
        assert!(log.contains(&"normal:activated".to_string()), "{log:?}");
        assert_eq!(engine.active_mode().as_str(), "normal");
    }

    #[test]
    fn launcher_modifier_release_keeps_the_forwarding_decision_from_its_press() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &[]);
        let (mut backend, log) = FakeBackend::new(enter_normal());
        engine.run(&mut backend).unwrap();

        assert_eq!(
            log.lock().unwrap().dispositions,
            [
                KeyDisposition::Forward,
                KeyDisposition::Consume,
                KeyDisposition::Consume,
                KeyDisposition::Forward,
            ]
        );
        assert!(engine.key_dispositions.is_empty());
    }

    #[test]
    fn alt_launcher_forwards_both_modifier_edges_without_replay() {
        let mut config = Config::default();
        config.hotkeys.clear();
        config
            .hotkeys
            .insert("alt+e".into(), Binding::Mode(ModeId::normal()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));
        let (mut backend, log) = FakeBackend::new(tap_chord("left_alt+e"));
        engine.run(&mut backend).unwrap();

        assert_eq!(
            log.lock().unwrap().dispositions,
            [
                KeyDisposition::Forward,
                KeyDisposition::Consume,
                KeyDisposition::Consume,
                KeyDisposition::Forward,
            ]
        );
        assert_eq!(engine.active_mode(), &ModeId::normal());
        assert!(engine.key_dispositions.is_empty());
    }

    #[test]
    fn idle_bare_binding_does_not_claim_an_external_alt_chord() {
        let mut config = Config::default();
        config.hotkeys.clear();
        config
            .hotkeys
            .insert("h".into(), Binding::Mode(ModeId::normal()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));
        let (mut backend, log) = FakeBackend::new(tap_chord("left_alt+h"));
        engine.run(&mut backend).unwrap();

        assert_eq!(
            log.lock().unwrap().dispositions,
            [
                KeyDisposition::Forward,
                KeyDisposition::Forward,
                KeyDisposition::Forward,
                KeyDisposition::Forward,
            ]
        );
        assert_eq!(engine.active_mode(), &ModeId::idle());
    }

    #[test]
    fn normal_passthrough_uses_complete_modifier_combinations() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("h".into(), Binding::Move(Direction::Left));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        let mut normal = ProbeMode::new("normal", seen.clone());
        normal.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(normal));

        let log = run_in_normal(&mut engine, tap_chord("left_alt+h"));
        let recorded = log.lock().unwrap();
        let dispositions = &recorded.dispositions;
        assert_eq!(
            &dispositions[dispositions.len() - 4..],
            [
                KeyDisposition::Forward,
                KeyDisposition::Forward,
                KeyDisposition::Forward,
                KeyDisposition::Forward,
            ]
        );
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|event| event != "normal:binding(move_left)"),
            "Alt+H must not fall back to bare h"
        );
    }

    #[test]
    fn consumed_normal_modifier_can_still_modify_a_bare_binding() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("left_shift".into(), Binding::Speed(crate::api::Speed::Slow));
        config
            .normal
            .bindings
            .insert("h".into(), Binding::Move(Direction::Left));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        let mut normal = ProbeMode::new("normal", seen.clone());
        normal.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(normal));

        let log = run_in_normal(&mut engine, tap_chord("left_shift+h"));
        let recorded = log.lock().unwrap();
        let dispositions = &recorded.dispositions;
        assert_eq!(
            &dispositions[dispositions.len() - 4..],
            [
                KeyDisposition::Consume,
                KeyDisposition::Consume,
                KeyDisposition::Consume,
                KeyDisposition::Consume,
            ]
        );
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|event| event == "normal:binding(slow)"));
        assert!(
            seen.iter()
                .any(|event| event == "normal:binding(move_left)")
        );
    }

    #[test]
    fn disabling_normal_passthrough_restores_exclusive_matching() {
        let mut config = Config::default();
        config.normal.passthrough_unbound_keys = false;
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("h".into(), Binding::Move(Direction::Left));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let log = run_in_normal(&mut engine, tap_chord("left_alt+h"));
        let recorded = log.lock().unwrap();
        let dispositions = &recorded.dispositions;
        assert_eq!(
            &dispositions[dispositions.len() - 4..],
            [
                KeyDisposition::Consume,
                KeyDisposition::Consume,
                KeyDisposition::Consume,
                KeyDisposition::Consume,
            ]
        );
    }

    #[test]
    fn repeat_and_release_keep_the_first_down_disposition() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let key = Key::new("f").unwrap();
        let input = |state, repeat| InputEvent {
            key: key.clone(),
            state,
            repeat,
            injected: false,
            timestamp_millis: 0,
        };

        assert_eq!(
            engine.complete_key_disposition(&input(KeyState::Down, false), KeyOutcome::Forwarded,),
            KeyOutcome::Forwarded
        );
        assert_eq!(
            engine.complete_key_disposition(&input(KeyState::Down, true), KeyOutcome::Consumed),
            KeyOutcome::Forwarded,
            "a repeat after enabling or changing mode must not consume a forwarded lifecycle"
        );
        assert_eq!(
            engine.complete_key_disposition(&input(KeyState::Up, false), KeyOutcome::Consumed),
            KeyOutcome::Forwarded
        );

        assert_eq!(
            engine.complete_key_disposition(&input(KeyState::Down, false), KeyOutcome::Consumed),
            KeyOutcome::Consumed
        );
        assert_eq!(
            engine.complete_key_disposition(&input(KeyState::Down, true), KeyOutcome::Forwarded),
            KeyOutcome::Consumed,
            "a repeat after pausing must not expose a previously consumed lifecycle"
        );
        assert_eq!(
            engine.complete_key_disposition(&input(KeyState::Up, false), KeyOutcome::Forwarded),
            KeyOutcome::Consumed
        );
        assert!(engine.key_dispositions.is_empty());
    }

    #[test]
    fn repeat_does_not_start_a_binding_missing_from_the_first_down() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "l", "move_right");
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("l"), &mut backend)
            .unwrap();
        engine
            .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
            .unwrap();
        engine
            .handle_backend_event(
                BackendEvent::Input(InputEvent {
                    key: Key::new("l").unwrap(),
                    state: KeyState::Down,
                    repeat: true,
                    injected: false,
                    timestamp_millis: 1,
                }),
                &mut backend,
            )
            .unwrap();
        engine
            .handle_backend_event(key_up("l"), &mut backend)
            .unwrap();

        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|event| !event.contains("binding(move_right)")),
            "a forwarded lifecycle must not acquire a held binding after context changes"
        );
        assert_eq!(
            log.lock().unwrap().dispositions,
            [
                KeyDisposition::Forward,
                KeyDisposition::Forward,
                KeyDisposition::Forward,
            ]
        );
        assert!(engine.active_gestures.is_empty());
        assert!(engine.key_dispositions.is_empty());
    }

    #[test]
    fn outward_motion_at_every_screen_edge_keeps_normal_active_and_can_reverse() {
        let cases = [
            (Point::new(0.0, 400.0), (-20.0, 0.0), (20.0, 0.0)),
            (Point::new(999.0, 400.0), (20.0, 0.0), (-20.0, 0.0)),
            (Point::new(500.0, 0.0), (0.0, -20.0), (0.0, 20.0)),
            (Point::new(500.0, 799.0), (0.0, 20.0), (0.0, -20.0)),
        ];

        for (edge, outward, inward) in cases {
            let mut engine = Engine::new(Config::default(), Appearance::Dark);
            engine.active = ModeId::normal();
            engine.cursor = edge;
            engine.screens = vec![Screen {
                bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
                is_primary: true,
                scale: 1.0,
                name: None,
            }];
            let (mut backend, log) = FakeBackend::new(Vec::new());

            for _ in 0..20 {
                engine
                    .execute(
                        vec![Command::MovePointer {
                            dx: outward.0,
                            dy: outward.1,
                        }],
                        &mut backend,
                    )
                    .unwrap();
            }
            assert_eq!(engine.cursor, edge);
            assert_eq!(engine.active_mode(), &ModeId::normal());
            assert!(log.lock().unwrap().moves.is_empty());

            engine
                .execute(
                    vec![Command::MovePointer {
                        dx: inward.0,
                        dy: inward.1,
                    }],
                    &mut backend,
                )
                .unwrap();
            assert_ne!(engine.cursor, edge);
            assert_eq!(engine.active_mode(), &ModeId::normal());
            assert_eq!(log.lock().unwrap().moves.len(), 1);
        }
    }

    #[test]
    fn relative_motion_crosses_adjacent_displays_but_not_virtual_desktop_gaps() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.active = ModeId::normal();
        engine.screens = vec![
            Screen {
                bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
                is_primary: true,
                scale: 1.0,
                name: None,
            },
            Screen {
                bounds: Rect::new(1000.0, 200.0, 1000.0, 800.0),
                work_area: Rect::new(1000.0, 200.0, 1000.0, 800.0),
                is_primary: false,
                scale: 1.0,
                name: None,
            },
        ];
        let (mut backend, log) = FakeBackend::new(Vec::new());
        assert_eq!(
            engine.constrain_absolute_pointer(Point::new(1000.0, 100.0)),
            Some(Point::new(999.0, 100.0)),
            "a reported coordinate in the layout gap must stay on a real display"
        );

        engine.cursor = Point::new(999.0, 100.0);
        engine
            .execute(
                vec![Command::MovePointer { dx: 10.0, dy: 0.0 }],
                &mut backend,
            )
            .unwrap();
        assert_eq!(engine.cursor, Point::new(999.0, 100.0));

        engine.cursor = Point::new(999.0, 300.0);
        engine
            .execute(
                vec![Command::MovePointer { dx: 10.0, dy: 0.0 }],
                &mut backend,
            )
            .unwrap();
        assert_eq!(engine.cursor, Point::new(1009.0, 300.0));
        assert_eq!(log.lock().unwrap().moves, vec![(10.0, 0.0)]);
    }

    #[test]
    fn real_normal_mode_moves_immediately_and_decorations_follow_the_pointer() {
        let config = Config::default();
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        let mut script = enter_normal();
        script.extend([
            // Move away from the fake pointer's nearby left screen edge. The
            // default acceleration profile intentionally covers more than ten
            // pixels in one display frame.
            key_down("l"),
            BackendEvent::Frame(Duration::from_millis(20)),
            BackendEvent::Frame(Duration::from_millis(20)),
            key_up("l"),
        ]);
        script.extend([key_down("m"), key_up("m")]);
        script.extend([key_down(";"), key_up(";")]);
        script.extend([key_down("u"), key_up("u")]);
        script.push(BackendEvent::PointerMoved(Point::new(200.0, 300.0)));
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        let log = log.lock().unwrap();
        assert!(log.moves.iter().any(|(dx, dy)| *dx > 0.0 && *dy == 0.0));
        assert_eq!(
            log.moves.len(),
            3,
            "initial tap plus two native display updates should each move once"
        );
        assert!(log.scrolls.iter().any(|(dx, dy)| {
            *dx == 0.0
                && if cfg!(target_os = "macos") {
                    *dy < 0.0
                } else {
                    *dy > 0.0
                }
        }));
        assert_eq!(log.clicks, 1);
        assert!(log.sent.contains(&("page_down".into(), KeyState::Down)));
        assert!(log.sent.contains(&("page_down".into(), KeyState::Up)));
        let first_move = log
            .timeline
            .iter()
            .position(|event| *event == "move")
            .expect("movement command");
        assert_eq!(
            log.timeline[..first_move]
                .iter()
                .copied()
                .filter(|event| *event == "dispose")
                .count(),
            5,
            "the l-down disposition must be sent before pointer movement"
        );
        let scene = log.scenes.last().expect("normal indicator scene");
        assert_eq!(
            scene.clip,
            Some(Rect::new(0.0, 0.0, 1000.0, 800.0)),
            "normal decorations must move inside a fixed screen overlay"
        );
        let indicator = scene.indicator.as_ref().expect("mode text indicator");
        assert_eq!(indicator.text, "Normal");
        assert!(indicator.position.x < 200.0);
        assert!(indicator.position.y > 300.0);
        let circle = scene.cursor_marker.as_ref().expect("cursor circle");
        assert_eq!(circle.center, Point::new(200.0, 300.0));
        assert!(
            engine.timers.is_empty(),
            "normal movement and scrolling must never create a timer"
        );
        assert!(
            engine.active_gestures.is_empty(),
            "every tapped held action must receive its release"
        );
    }

    #[test]
    fn normal_enters_a_targeting_mode_from_its_own_table() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &["grid"]);

        // The launcher enters normal, then a bare `g` enters grid.
        let mut script = enter_normal();
        script.push(key_down("g"));
        let (mut backend, _) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode().as_str(), "grid");
    }

    #[test]
    fn inherited_normal_movement_receives_frames_while_grid_is_active() {
        let config = Config::default();
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        let mut script = enter_normal();
        script.extend([
            key_down("g"),
            key_up("g"),
            key_down("h"),
            BackendEvent::Frame(Duration::from_millis(8)),
            key_up("h"),
        ]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode(), &ModeId::grid());
        assert_eq!(
            log.lock().unwrap().moves.len(),
            2,
            "initial key-down and the display frame must both reach normal"
        );
        assert!(engine.frame_clock_owner.is_none());
    }

    #[test]
    fn action_sequence_continues_after_a_mode_switch() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::default();
        config.normal.bindings.insert(
            "x".into(),
            Binding::Sequence(vec![
                Binding::Mode(ModeId::grid()),
                Binding::Warp { x: 23, y: 43 },
            ]),
        );
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
        engine.register(Box::new(ProbeMode::new("grid", seen)));

        let log = run_in_normal(&mut engine, vec![key_down("x")]);
        assert_eq!(engine.active_mode().as_str(), "grid");
        assert_eq!(log.lock().unwrap().warps, vec![Point::new(23.0, 43.0)]);
    }

    #[test]
    fn action_sequence_returns_to_idle_after_a_recoverable_input_failure() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::default();
        config.normal.bindings.insert(
            "x".into(),
            Binding::Sequence(vec![
                Binding::Warp { x: 23, y: 43 },
                Binding::Click(crate::api::binding::Button::Left),
            ]),
        );
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let (mut backend, log) =
            FakeBackend::new(enter_normal().into_iter().chain([key_down("x")]).collect());
        backend.fail_warp = true;
        engine.run(&mut backend).unwrap();

        assert_eq!(log.lock().unwrap().clicks, 0);
        assert_eq!(engine.active_mode(), &ModeId::idle());
    }

    #[test]
    fn a_semantic_click_notifies_the_active_mode() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &["grid"]);
        let mut script = enter_normal();
        script.extend([key_down("g"), key_down(";")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(log.lock().unwrap().clicks, 1);
        assert_eq!(engine.active_mode().as_str(), "grid");
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|event| event == "grid:clicked")
        );
    }

    #[test]
    fn every_semantic_click_notifies_once_but_press_release_and_toggle_do_not() {
        for binding in ["left_click", "right_click", "middle_click", "double_click"] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut engine = engine_with_normal_probes(&seen, "x", binding);
            run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
            let clicked = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.as_str() == "normal:clicked")
                .count();
            assert_eq!(clicked, 1, "binding={binding}");
        }

        for binding in [
            "press mouse_left",
            "release mouse_left",
            "toggle mouse_left",
        ] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut engine = engine_with_normal_probes(&seen, "x", binding);
            run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
            assert!(
                seen.lock()
                    .unwrap()
                    .iter()
                    .all(|event| event.as_str() != "normal:clicked"),
                "binding={binding}"
            );
        }
    }

    #[test]
    fn ordinary_click_colors_follow_the_physical_activation_key() {
        for (binding, color) in [
            ("left_click", (0, 255, 0)),
            ("middle_click", (255, 0, 255)),
            ("right_click", (0, 255, 255)),
            ("double_click", (0, 255, 0)),
        ] {
            let mut engine = engine_with_normal_binding("x", binding);
            let log = run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
            let log = log.lock().unwrap();
            let expected = crate::api::overlay::Color::rgb(color.0, color.1, color.2);
            assert!(
                log.scenes.iter().any(|scene| {
                    scene
                        .cursor_marker
                        .as_ref()
                        .is_some_and(|marker| marker.stroke == expected)
                }),
                "{binding} never presented its click-key color"
            );
            let final_marker = log
                .scenes
                .last()
                .and_then(|scene| scene.cursor_marker.as_ref())
                .expect("cursor marker after activation-key release");
            assert_ne!(final_marker.stroke, expected, "binding={binding}");

            let mouse = log
                .timeline
                .iter()
                .position(|event| *event == "mouse")
                .expect("mouse injection");
            let presentation = log
                .timeline
                .iter()
                .enumerate()
                .skip(mouse + 1)
                .find(|(_, event)| **event == "present")
                .map(|(index, _)| index)
                .expect("click-color presentation");
            assert!(mouse < presentation, "binding={binding}");
        }
    }

    #[test]
    fn repeated_click_key_does_not_click_or_reorder_feedback_again() {
        let mut engine = engine_with_normal_binding("x", "left_click");
        let mut repeat = key_down("x");
        let BackendEvent::Input(input) = &mut repeat else {
            unreachable!();
        };
        input.repeat = true;

        let log = run_in_normal(&mut engine, vec![key_down("x"), repeat, key_up("x")]);
        let log = log.lock().unwrap();
        assert_eq!(
            log.buttons
                .iter()
                .filter(|(_, action)| *action == ButtonAction::Click)
                .count(),
            1
        );
    }

    #[test]
    fn every_mouse_click_binding_long_press_toggles_its_button() {
        for (binding, button, initial_action) in [
            ("left_click", Button::Left, ButtonAction::Click),
            ("middle_click", Button::Middle, ButtonAction::Click),
            ("right_click", Button::Right, ButtonAction::Click),
            ("double_click", Button::Left, ButtonAction::DoubleClick),
        ] {
            let mut engine = engine_with_normal_binding("x", binding);
            engine.active = ModeId::normal();
            let (mut backend, log) = FakeBackend::new(Vec::new());

            engine
                .handle_backend_event(key_down("x"), &mut backend)
                .unwrap();
            assert_eq!(engine.pending_long_press_toggles.len(), 1);
            assert!(
                !engine.latched.contains(&InputTarget::Mouse(button)),
                "binding={binding}"
            );
            engine.pending_long_press_toggles[0].fires_at = Instant::now();
            engine.fire_due_long_press_toggles(&mut backend).unwrap();

            assert!(
                engine.latched.contains(&InputTarget::Mouse(button)),
                "binding={binding}"
            );
            engine
                .handle_backend_event(key_up("x"), &mut backend)
                .unwrap();
            assert!(
                engine.latched.contains(&InputTarget::Mouse(button)),
                "key release must not undo toggle for {binding}"
            );
            assert_eq!(
                log.lock().unwrap().buttons,
                vec![
                    (map_button(button), initial_action),
                    (map_button(button), ButtonAction::Press),
                ],
                "binding={binding}"
            );
        }
    }

    #[test]
    fn second_long_press_releases_latched_button_without_an_extra_click() {
        let mut engine = engine_with_normal_binding("x", "left_click");
        engine.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        engine.pending_long_press_toggles[0].fires_at = Instant::now();
        engine.fire_due_long_press_toggles(&mut backend).unwrap();
        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();

        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert_eq!(
            log.lock().unwrap().buttons,
            vec![
                (MouseButton::Left, ButtonAction::Click),
                (MouseButton::Left, ButtonAction::Press),
            ],
            "a latched button must not receive another atomic click"
        );
        engine.pending_long_press_toggles[0].fires_at = Instant::now();
        engine.fire_due_long_press_toggles(&mut backend).unwrap();
        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();

        assert!(!engine.latched.contains(&InputTarget::Mouse(Button::Left)));
        assert_eq!(
            log.lock().unwrap().buttons,
            vec![
                (MouseButton::Left, ButtonAction::Click),
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
        );
    }

    #[test]
    fn short_click_cancels_long_press_and_zero_disables_it() {
        let mut engine = engine_with_normal_binding("x", "left_click");
        engine.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();
        assert!(engine.pending_long_press_toggles.is_empty());
        assert!(engine.latched.is_empty());
        assert_eq!(log.lock().unwrap().clicks, 1);

        engine.config.normal.long_press_toggle_ms = 0;
        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert!(engine.pending_long_press_toggles.is_empty());
        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();
        assert_eq!(log.lock().unwrap().clicks, 2);
    }

    #[test]
    fn long_press_toggle_ignores_non_click_bindings() {
        let mut engine = engine_with_normal_binding("x", "move_left");
        engine.active = ModeId::normal();
        let (mut backend, _) = FakeBackend::new(Vec::new());
        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert!(engine.pending_long_press_toggles.is_empty());
    }

    #[test]
    fn explicit_parameterless_toggle_cancels_matching_long_press() {
        for (key, button) in [
            (";", MouseButton::Left),
            ("'", MouseButton::Right),
            ("right_shift", MouseButton::Middle),
        ] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut engine = engine_with_probes(&seen, &[]);
            engine.active = ModeId::normal();
            let (mut backend, log) = FakeBackend::new(Vec::new());

            engine
                .handle_backend_event(key_down(key), &mut backend)
                .unwrap();
            assert_eq!(engine.pending_long_press_toggles.len(), 1, "key={key}");
            engine
                .handle_backend_event(key_down("n"), &mut backend)
                .unwrap();
            assert!(engine.pending_long_press_toggles.is_empty(), "key={key}");
            engine.fire_due_long_press_toggles(&mut backend).unwrap();
            assert_eq!(
                log.lock().unwrap().buttons,
                vec![(button, ButtonAction::Click), (button, ButtonAction::Press)],
                "key={key}"
            );
        }
    }

    #[test]
    fn newest_held_click_key_controls_color_then_release_falls_back() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config.normal.bindings.insert(
            "x".into(),
            Binding::Click(crate::api::binding::Button::Left),
        );
        config.normal.bindings.insert(
            "y".into(),
            Binding::Click(crate::api::binding::Button::Right),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let log = run_in_normal(
            &mut engine,
            vec![key_down("x"), key_down("y"), key_up("y"), key_up("x")],
        );
        let log = log.lock().unwrap();
        let left = crate::api::overlay::Color::rgb(0, 255, 0);
        let right = crate::api::overlay::Color::rgb(0, 255, 255);
        let mut colors = log
            .scenes
            .iter()
            .filter_map(|scene| scene.cursor_marker.as_ref().map(|marker| marker.stroke))
            .filter(|color| *color == left || *color == right)
            .collect::<Vec<_>>();
        colors.dedup();
        assert_eq!(colors, vec![left, right, left]);
        assert_ne!(
            log.scenes
                .last()
                .and_then(|scene| scene.cursor_marker.as_ref())
                .expect("released cursor marker")
                .stroke,
            left
        );
    }

    #[test]
    fn latched_mouse_button_color_beats_ordinary_click_feedback() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config.normal.bindings.insert(
            "x".into(),
            Binding::Toggle(vec![InputTarget::Mouse(crate::api::binding::Button::Left)]),
        );
        config.normal.bindings.insert(
            "y".into(),
            Binding::Click(crate::api::binding::Button::Right),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let log = run_in_normal(
            &mut engine,
            vec![key_down("x"), key_up("x"), key_down("y"), key_up("y")],
        );
        let log = log.lock().unwrap();
        let right = crate::api::overlay::Color::rgb(0, 255, 255);
        assert!(log.scenes.iter().all(|scene| {
            scene
                .cursor_marker
                .as_ref()
                .is_none_or(|marker| marker.stroke != right)
        }));
    }

    #[test]
    fn click_indicator_survives_mode_switch_until_its_key_is_released() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_binding("x", "left_click");
        engine.register(Box::new(ProbeMode::new("grid", seen)));
        engine.active = ModeId::normal();
        let (mut backend, _) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert!(!engine.active_click_indicators.is_empty());
        assert_eq!(engine.pending_long_press_toggles.len(), 1);
        engine
            .activate(ModeId::grid(), Some(ModeId::normal()), &mut backend)
            .unwrap();
        assert!(!engine.active_click_indicators.is_empty());
        assert_eq!(engine.pending_long_press_toggles.len(), 1);
        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();
        assert!(engine.active_click_indicators.is_empty());
        assert!(engine.pending_long_press_toggles.is_empty());
    }

    #[test]
    fn failed_or_source_less_clicks_do_not_leave_click_indicators() {
        let mut engine = engine_with_normal_binding("x", "left_click");
        engine.active = ModeId::normal();
        let (mut backend, _) = FakeBackend::new(Vec::new());
        backend.fail_mouse = true;
        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert!(engine.active_click_indicators.is_empty());

        let mut engine = engine_with_normal_binding("x", "left_click");
        engine.active = ModeId::normal();
        let (mut backend, _) = FakeBackend::new(Vec::new());
        engine
            .execute(
                vec![Command::DispatchActions(vec![Binding::Click(
                    crate::api::binding::Button::Left,
                )])],
                &mut backend,
            )
            .unwrap();
        assert!(engine.active_click_indicators.is_empty());

        let delayed_input = InputEvent {
            key: Key::new("x").unwrap(),
            state: KeyState::Down,
            repeat: false,
            injected: false,
            timestamp_millis: 0,
        };
        engine
            .continue_sequence(
                VecDeque::from([Binding::Click(crate::api::binding::Button::Left)]),
                ModeId::normal(),
                delayed_input,
                &mut backend,
            )
            .unwrap();
        assert!(engine.active_click_indicators.is_empty());
    }

    #[test]
    fn disabling_clears_click_indicators() {
        let mut engine = engine_with_normal_binding("x", "left_click");
        engine.active = ModeId::normal();
        let (mut backend, _) = FakeBackend::new(Vec::new());
        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert!(!engine.active_click_indicators.is_empty());
        assert_eq!(engine.pending_long_press_toggles.len(), 1);
        engine
            .handle_backend_event(BackendEvent::ToggleEnabled, &mut backend)
            .unwrap();
        assert!(engine.active_click_indicators.is_empty());
        assert!(engine.pending_long_press_toggles.is_empty());
    }

    #[test]
    fn shutdown_clears_click_indicators_without_delaying_the_click() {
        let mut engine = engine_with_normal_binding("x", "left_click");
        let log = run_in_normal(&mut engine, vec![key_down("x")]);
        assert!(engine.active_click_indicators.is_empty());
        assert!(engine.pending_long_press_toggles.is_empty());
        let log = log.lock().unwrap();
        assert_eq!(log.clicks, 1);
        assert!(log.scenes.iter().any(|scene| {
            scene
                .cursor_marker
                .as_ref()
                .is_some_and(|marker| marker.stroke == crate::api::overlay::Color::rgb(0, 255, 0))
        }));
    }

    #[test]
    fn latched_mouse_buttons_recolor_the_transparent_cursor_marker() {
        for (target, color) in [
            ("mouse_left", (0, 255, 0)),
            ("mouse_middle", (255, 0, 255)),
            ("mouse_right", (0, 255, 255)),
        ] {
            let mut engine = engine_with_normal_binding("x", &format!("toggle {target}"));
            let log = run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
            let expected_fill = crate::api::overlay::Color::rgba(color.0, color.1, color.2, 51);
            let expected_stroke = crate::api::overlay::Color::rgba(color.0, color.1, color.2, 255);
            assert!(
                log.lock().unwrap().scenes.iter().any(|scene| {
                    scene.cursor_marker.as_ref().is_some_and(|marker| {
                        marker.fill == expected_fill && marker.stroke == expected_stroke
                    })
                }),
                "{target} never presented its pressed cursor color"
            );
        }
    }

    #[test]
    fn left_mouse_color_wins_when_multiple_buttons_are_latched() {
        let mut engine =
            engine_with_normal_binding("x", "toggle mouse_right mouse_middle mouse_left");
        let log = run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
        let expected = crate::api::overlay::Color::rgb(0, 255, 0);
        assert!(log.lock().unwrap().scenes.iter().any(|scene| {
            scene
                .cursor_marker
                .as_ref()
                .is_some_and(|marker| marker.stroke == expected)
        }));
    }

    #[test]
    fn releasing_the_mouse_button_restores_the_mode_cursor_color() {
        let mut engine = engine_with_normal_binding("x", "toggle mouse_left");
        let log = run_in_normal(
            &mut engine,
            vec![key_down("x"), key_up("x"), key_down("x"), key_up("x")],
        );
        let log = log.lock().unwrap();
        let marker = log
            .scenes
            .last()
            .and_then(|scene| scene.cursor_marker.as_ref())
            .expect("released cursor marker");
        assert_ne!(marker.stroke, crate::api::overlay::Color::rgb(0, 255, 0));
        assert_eq!(marker.fill.a, 34);
        assert_eq!(marker.stroke.a, 210);
    }

    #[test]
    fn finish_click_chain_is_idempotent_and_does_not_click_recursively() {
        let mut config = Config::default();
        config.grid.max_depth = 1;
        config.grid.lifecycle.after_finish = crate::config::LifecycleAction::Click {
            button: MouseButton::Left,
            action: ButtonAction::Click,
        };
        // The resulting Clicked event asks to finish again. Since the grid is
        // already finished this must be a no-op, not another click.
        config.grid.lifecycle.after_click = crate::config::LifecycleAction::Finish;
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        let mut script = enter_normal();
        script.extend([key_down("g"), key_up("g"), key_down("1"), key_up("1")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(log.lock().unwrap().clicks, 1);
        assert_eq!(engine.active_mode(), &ModeId::grid());
    }

    #[test]
    fn clicking_before_grid_max_depth_finishes_and_returns_to_normal_by_default() {
        let config = Config::default();
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        let mut script = enter_normal();
        script.extend([key_down("g"), key_up("g"), key_down(";"), key_up(";")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(log.lock().unwrap().clicks, 1);
        assert_eq!(engine.active_mode(), &ModeId::normal());
        assert!(log.lock().unwrap().dismissals > 0);
    }

    #[test]
    fn bindings_are_scoped_to_the_mode_that_declares_them() {
        // `g` belongs to normal, so it must do nothing while idle. This is what
        // keeps the program silent until it is asked for.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &["grid"]);

        let (mut backend, log) = FakeBackend::new(vec![key_down("g")]);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode().as_str(), "idle");
        assert!(
            log.lock()
                .unwrap()
                .dispositions
                .iter()
                .all(|d| *d == KeyDisposition::Forward),
            "idle must not swallow keys"
        );
    }

    #[test]
    fn pressing_an_active_modes_own_key_returns_to_idle() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &[]);

        // Enter normal, then use its bare q exit binding.
        let mut script = enter_normal();
        script.push(key_down("q"));
        let (mut backend, _) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode().as_str(), "idle");
    }

    #[test]
    fn escape_binding_returns_to_idle() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &[]);

        let mut script = enter_normal();
        script.push(key_down("q"));
        let (mut backend, _) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode().as_str(), "idle");
    }

    #[test]
    fn temporary_modifier_changes_only_the_display_mode() {
        let config = Config::default();
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        engine.active = ModeId::grid();
        assert_eq!(engine.display_mode(), ModeId::grid());

        let primary = Key::new(&config.grid.temporary_mode_keys[0]).unwrap();
        engine.pressed.insert(primary.clone());
        assert_eq!(engine.display_mode(), ModeId::normal());
        assert_eq!(engine.active, ModeId::grid(), "grid state remains active");

        engine.pressed.remove(&primary);
        assert_eq!(engine.display_mode(), ModeId::grid());
        assert_eq!(engine.active, ModeId::grid());
    }

    #[test]
    fn temporary_modifier_repaints_badge_without_leaving_grid() {
        let config = Config::default();
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        let primary = Key::new(&config.grid.temporary_mode_keys[0]).unwrap();
        let input = |state| {
            BackendEvent::Input(InputEvent {
                key: primary.clone(),
                state,
                repeat: false,
                injected: false,
                timestamp_millis: 0,
            })
        };
        let mut script = enter_normal();
        script.extend([key_down("g"), key_up("g")]);
        script.extend([input(KeyState::Down), input(KeyState::Up)]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode(), &ModeId::grid());
        let indicators: Vec<String> = log
            .lock()
            .unwrap()
            .scenes
            .iter()
            .filter_map(|scene| scene.indicator.as_ref().map(|item| item.text.clone()))
            .collect();
        assert!(
            indicators.windows(3).any(|values| {
                values[0] == "Grid" && values[1] == "Normal" && values[2] == "Grid"
            }),
            "temporary indicator sequence missing: {indicators:?}"
        );
    }

    #[test]
    fn primary_q_returns_every_configurable_targeting_mode_to_normal() {
        for target in [ModeId::grid(), ModeId::recursive_grid(), ModeId::ui_hint()] {
            let config = Config::default();
            let primary = Key::new(&config.grid.temporary_mode_keys[0]).unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let mut engine = Engine::new(config, Appearance::Dark);
            let mut idle = ProbeMode::new("idle", seen.clone());
            idle.captures = false;
            engine.register(Box::new(idle));
            engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
            engine.register(Box::new(ProbeMode::new(target.as_str(), seen)));
            let (mut backend, _) = FakeBackend::new(Vec::new());
            engine
                .activate(target.clone(), Some(ModeId::normal()), &mut backend)
                .unwrap();

            engine
                .handle_backend_event(
                    BackendEvent::Input(InputEvent {
                        key: primary.clone(),
                        state: KeyState::Down,
                        repeat: false,
                        injected: false,
                        timestamp_millis: 0,
                    }),
                    &mut backend,
                )
                .unwrap();
            engine
                .handle_backend_event(key_down("q"), &mut backend)
                .unwrap();

            assert_eq!(
                engine.active_mode(),
                &ModeId::normal(),
                "Primary+Q should leave {target} for normal"
            );
        }
    }

    #[test]
    fn label_modes_use_normal_pointer_controls_only_when_allowed_or_temporary() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let log = Arc::new(Mutex::new(Vec::new()));
        engine.register(Box::new(ProbeMode::new("normal", log.clone())));
        engine.register(Box::new(ProbeMode::new("grid", log.clone())));
        engine.register(Box::new(ProbeMode::new("recursive_grid", log)));

        engine.active = ModeId::grid();
        engine.pressed.insert(Key::new("g").unwrap());
        assert!(
            engine.lookup(&Key::new("g").unwrap()).is_none(),
            "the grid's raw label must beat inherited normal bindings"
        );
        engine.pressed.clear();
        engine.pressed.insert(Key::new("h").unwrap());
        let inherited = engine.lookup(&Key::new("h").unwrap()).unwrap();
        assert_eq!(inherited.owner, ModeId::normal());

        engine.pressed.insert(Key::new("left_ctrl").unwrap());
        let resolved = engine.lookup(&Key::new("h").unwrap()).unwrap();
        assert_eq!(resolved.owner, ModeId::normal());
        assert_eq!(resolved.binding.as_ref(), &Binding::Move(Direction::Left));

        engine.pressed.clear();
        engine.pressed.insert(Key::new("h").unwrap());
        engine.active = ModeId::recursive_grid();
        let resolved = engine.lookup(&Key::new("h").unwrap()).unwrap();
        assert_eq!(resolved.owner, ModeId::normal());
        assert_eq!(resolved.binding.as_ref(), &Binding::Move(Direction::Left));
    }

    #[test]
    fn ui_hint_overlap_key_wins_over_inheritance_and_temporary_mode() {
        let mut config = Config::default();
        config.ui_hint.overlap_cycle_key = "alt".into();
        config.ui_hint.temporary_mode_keys = vec!["alt".into()];
        config.normal.bindings.insert(
            "left_alt".into(),
            Binding::Click(crate::api::binding::Button::Middle),
        );
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::modes::built_in(&config) {
            engine.register(mode);
        }
        let key = Key::new("left_alt").unwrap();
        engine.pressed.insert(key.clone());

        engine.active = ModeId::ui_hint();
        assert!(
            engine.lookup(&key).is_none(),
            "the configured key must reach UI Hint as a raw hold event"
        );
        assert_eq!(
            engine.display_mode(),
            ModeId::ui_hint(),
            "the same temporary key must not hide overlap cycling"
        );

        engine.active = ModeId::normal();
        assert!(
            engine.lookup(&key).is_some(),
            "the key must retain its Normal binding outside UI Hint"
        );
    }

    #[test]
    fn a_longer_chord_wins_over_a_bare_key() {
        // A configured longer chord must win over its bare-key sibling.
        use crate::api::binding::{Direction, ScrollAmount};
        let mut config = Config::default();
        config.normal.bindings.insert(
            "e".into(),
            Binding::Scroll(Direction::Down, ScrollAmount::Step),
        );
        config.normal.bindings.insert(
            "shift+e".into(),
            Binding::Scroll(Direction::Down, ScrollAmount::Half),
        );
        let mut engine = Engine::new(config, Appearance::Dark);
        engine.register(Box::new(ProbeMode::new(
            "normal",
            Arc::new(Mutex::new(vec![])),
        )));
        engine.active = ModeId::normal();

        engine.pressed.insert(Key::new("left_shift").unwrap());
        engine.pressed.insert(Key::new("e").unwrap());
        assert_eq!(
            engine
                .lookup(&Key::new("e").unwrap())
                .map(|resolved| resolved.binding.as_ref().clone()),
            Some(Binding::Scroll(Direction::Down, ScrollAmount::Half))
        );

        engine.pressed.remove(&Key::new("left_shift").unwrap());
        assert_eq!(
            engine
                .lookup(&Key::new("e").unwrap())
                .map(|resolved| resolved.binding.as_ref().clone()),
            Some(Binding::Scroll(Direction::Down, ScrollAmount::Step))
        );
    }

    #[test]
    fn a_click_binding_is_executed_by_the_engine_not_the_mode() {
        let mut engine = engine_with_normal_binding("f", "left_click");
        let log = run_in_normal(&mut engine, vec![key_down("f")]);
        assert_eq!(log.lock().unwrap().clicks, 1);
    }

    #[test]
    fn two_physical_taps_are_never_coalesced_or_suppressed_by_the_engine() {
        let mut engine = engine_with_normal_binding(";", "left_click");
        let log = run_in_normal(
            &mut engine,
            vec![key_down(";"), key_up(";"), key_down(";"), key_up(";")],
        );
        assert_eq!(
            log.lock().unwrap().buttons,
            [
                (MouseButton::Left, ButtonAction::Click),
                (MouseButton::Left, ButtonAction::Click),
            ]
        );
    }

    #[test]
    fn two_clicks_in_one_binding_sequence_reach_the_backend_in_order() {
        let mut engine = engine_with_normal_action(
            ";",
            Binding::Sequence(vec![
                Binding::Click(crate::api::binding::Button::Left),
                Binding::Click(crate::api::binding::Button::Left),
            ]),
        );
        let log = run_in_normal(&mut engine, vec![key_down(";")]);
        assert_eq!(
            log.lock().unwrap().buttons,
            [
                (MouseButton::Left, ButtonAction::Click),
                (MouseButton::Left, ButtonAction::Click),
            ]
        );
    }

    #[test]
    fn a_double_click_reaches_the_backend_as_one_native_action() {
        let mut engine = engine_with_normal_binding("f", "double_click");
        let log = run_in_normal(&mut engine, vec![key_down("f")]);
        assert_eq!(
            log.lock().unwrap().buttons,
            [(MouseButton::Left, ButtonAction::DoubleClick)]
        );
    }

    #[test]
    fn a_send_binding_injects_the_keystroke() {
        let mut engine = engine_with_normal_binding("t", "home");
        let log = run_in_normal(&mut engine, vec![key_down("t")]);

        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("home".to_string(), KeyState::Down),
                ("home".into(), KeyState::Up)
            ]
        );
    }

    #[test]
    fn held_bindings_reach_the_mode_on_both_edges() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "l", "move_right");
        run_in_normal(&mut engine, vec![key_down("l"), key_up("l")]);

        let log = seen.lock().unwrap().clone();
        let deliveries = log
            .iter()
            .filter(|e| e.contains("binding(move_right)"))
            .count();
        assert_eq!(deliveries, 2, "press and release both matter: {log:?}");
    }

    #[test]
    fn discrete_bindings_ignore_auto_repeat() {
        let mut engine = engine_with_normal_binding("f", "left_click");
        let log = run_in_normal(
            &mut engine,
            vec![
                key_down("f"),
                BackendEvent::Input(InputEvent {
                    key: Key::new("f").unwrap(),
                    state: KeyState::Down,
                    repeat: true,
                    injected: false,
                    timestamp_millis: 0,
                }),
            ],
        );
        assert_eq!(log.lock().unwrap().clicks, 1, "repeat must not re-click");
    }

    #[test]
    fn held_sequence_repeat_does_not_repeat_discrete_actions() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config.normal.bindings.insert(
            "f".into(),
            Binding::Sequence(vec![
                Binding::parse("move_right").unwrap(),
                Binding::parse("left_click").unwrap(),
            ]),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));
        let log = run_in_normal(
            &mut engine,
            vec![
                key_down("f"),
                BackendEvent::Input(InputEvent {
                    key: Key::new("f").unwrap(),
                    state: KeyState::Down,
                    repeat: true,
                    injected: false,
                    timestamp_millis: 0,
                }),
                key_up("f"),
            ],
        );
        assert_eq!(log.lock().unwrap().clicks, 1);
    }

    #[test]
    fn held_sequence_rejects_mode_changes_before_starting() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, _) = FakeBackend::new(Vec::new());
        let resolved = ResolvedBinding {
            binding: Arc::new(Binding::Sequence(vec![
                Binding::parse("move_right").unwrap(),
                Binding::Mode(ModeId::grid()),
            ])),
            owner: ModeId::normal(),
        };
        let input = InputEvent {
            key: Key::new("f").unwrap(),
            state: KeyState::Down,
            repeat: false,
            injected: false,
            timestamp_millis: 0,
        };
        assert!(
            engine
                .apply_binding(&resolved, &input, &mut backend)
                .unwrap_err()
                .contains("mode-changing")
        );
    }

    #[test]
    fn follow_binding_is_dispatched_once_to_the_active_mode() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "`", "follow");
        run_in_normal(
            &mut engine,
            vec![
                key_down("`"),
                BackendEvent::Input(InputEvent {
                    key: Key::new("`").unwrap(),
                    state: KeyState::Down,
                    repeat: true,
                    injected: false,
                    timestamp_millis: 0,
                }),
            ],
        );

        let log = seen.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|entry| entry.as_str() == "normal:binding(follow)")
                .count(),
            1,
            "follow must be a single mode binding, got {log:?}"
        );
    }

    #[test]
    fn toggle_holds_then_releases_the_button() {
        let mut engine = engine_with_normal_binding("space", "toggle_left");
        let log = run_in_normal(
            &mut engine,
            vec![key_down("space"), key_up("space"), key_down("space")],
        );

        assert_eq!(
            log.lock().unwrap().buttons,
            vec![
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
        );
    }

    #[test]
    fn modifier_plus_bare_toggle_latches_that_modifier() {
        let mut engine = engine_with_normal_binding("n", "toggle");
        let log = run_in_normal(
            &mut engine,
            vec![
                key_down("left_shift"),
                key_down("n"),
                key_up("n"),
                key_up("left_shift"),
            ],
        );
        let log = log.lock().unwrap();
        assert_eq!(
            log.sent,
            vec![
                ("left_shift".into(), KeyState::Down),
                ("left_shift".into(), KeyState::Up),
            ]
        );
        assert!(log.buttons.is_empty());
        assert!(log.scenes.iter().any(|scene| {
            scene
                .indicator
                .as_ref()
                .is_some_and(|indicator| indicator.held_text.as_deref() == Some("● LEFT SHIFT"))
        }));
    }

    #[test]
    fn activation_key_then_modifier_upgrades_bare_toggle_without_mouse_fallback() {
        let mut engine = engine_with_normal_binding("n", "toggle");
        let log = run_in_normal(
            &mut engine,
            vec![
                key_down("n"),
                key_down("left_shift"),
                key_up("n"),
                key_up("left_shift"),
            ],
        );
        let log = log.lock().unwrap();
        assert_eq!(
            log.sent,
            vec![
                ("left_shift".into(), KeyState::Down),
                ("left_shift".into(), KeyState::Up),
            ]
        );
        assert!(log.buttons.is_empty());
    }

    #[test]
    fn separate_toggle_chords_accumulate_instead_of_clearing_existing_targets() {
        let mut engine = engine_with_normal_binding("n", "toggle");
        let log = run_in_normal(
            &mut engine,
            vec![
                key_down("left_shift"),
                key_down("n"),
                key_up("n"),
                key_up("left_shift"),
                key_down("n"),
                key_down("left_ctrl"),
                key_up("n"),
                key_up("left_ctrl"),
            ],
        );
        let log = log.lock().unwrap();
        assert_eq!(
            &log.sent[..2],
            &[
                ("left_shift".into(), KeyState::Down),
                ("left_ctrl".into(), KeyState::Down),
            ]
        );
        assert!(log.buttons.is_empty());
    }

    #[test]
    fn bare_parameterless_toggle_does_nothing_when_nothing_is_latched() {
        let mut engine = engine_with_normal_binding("n", "toggle");
        let log = run_in_normal(
            &mut engine,
            vec![key_down("n"), key_up("n"), key_down("n"), key_up("n")],
        );
        let log = log.lock().unwrap();
        assert!(log.sent.is_empty());
        assert!(log.buttons.is_empty());
    }

    #[test]
    fn bare_parameterless_toggle_releases_all_accumulated_targets() {
        let mut config = Config::default();
        config
            .normal
            .bindings
            .insert("n".into(), Binding::Toggle(Vec::new()));
        config.normal.bindings.insert(
            ";".into(),
            Binding::Click(crate::api::binding::Button::Left),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine
            .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
            .unwrap();
        for event in [
            key_down("left_shift"),
            key_down("n"),
            key_up("n"),
            key_up("left_shift"),
            key_down("n"),
            key_down(";"),
            key_up(";"),
            key_up("n"),
            key_down("n"),
            key_up("n"),
        ] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        let log = log.lock().unwrap();
        assert_eq!(
            log.sent,
            vec![
                ("left_shift".into(), KeyState::Down),
                ("left_shift".into(), KeyState::Up),
            ]
        );
        assert_eq!(
            log.buttons,
            vec![
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
        );
        assert!(engine.latched.is_empty());
    }

    #[test]
    fn toggle_plus_click_key_latches_the_mapped_mouse_button_without_clicking() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("n".into(), Binding::Toggle(Vec::new()));
        config.normal.bindings.insert(
            ";".into(),
            Binding::Click(crate::api::binding::Button::Left),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let log = run_in_normal(
            &mut engine,
            vec![key_down("n"), key_down(";"), key_up(";"), key_up("n")],
        );
        let log = log.lock().unwrap();
        assert_eq!(log.clicks, 0);
        assert_eq!(
            log.buttons,
            vec![
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
        );
    }

    #[test]
    fn generic_press_send_and_release_preserve_a_latched_modifier() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config.normal.bindings.insert(
            "n".into(),
            Binding::Sequence(vec![
                Binding::parse("press shift mouse_left").unwrap(),
                Binding::parse("send shift+x").unwrap(),
                Binding::parse("release shift mouse_left").unwrap(),
            ]),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let log = run_in_normal(&mut engine, vec![key_down("n")]);
        let log = log.lock().unwrap();
        assert_eq!(
            log.sent,
            vec![
                ("left_shift".into(), KeyState::Down),
                ("x".into(), KeyState::Down),
                ("x".into(), KeyState::Up),
                ("left_shift".into(), KeyState::Up),
            ]
        );
        assert_eq!(
            log.buttons,
            vec![
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
        );
    }

    #[test]
    fn inferred_side_modifier_satisfies_a_generic_modifier_send() {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config.normal.bindings.insert(
            "n".into(),
            Binding::Sequence(vec![
                Binding::parse("toggle").unwrap(),
                Binding::parse("send shift+x").unwrap(),
            ]),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen)));

        let log = run_in_normal(&mut engine, vec![key_down("left_shift"), key_down("n")]);
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("left_shift".into(), KeyState::Down),
                ("x".into(), KeyState::Down),
                ("x".into(), KeyState::Up),
                ("left_shift".into(), KeyState::Up),
            ]
        );
    }

    #[test]
    fn failed_chord_release_is_retried_during_cleanup() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        log.lock().unwrap().fail_next_key_up = true;
        let error = engine
            .send_chord(&KeyChord::parse("home").unwrap(), &mut backend)
            .unwrap_err();
        assert!(error.contains("key-up"));
        assert_eq!(
            engine.latched,
            BTreeSet::from([InputTarget::Key(Key::new("home").unwrap())])
        );
        engine.release_latched(&mut backend).unwrap();
        assert!(engine.latched.is_empty());
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("home".into(), KeyState::Down),
                ("home".into(), KeyState::Up),
            ]
        );
    }

    #[test]
    fn generic_modifier_release_matches_a_latched_side() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine
            .press_targets(
                &[InputTarget::Key(Key::new("right_shift").unwrap())],
                &mut backend,
            )
            .unwrap();
        engine
            .release_targets(
                &[InputTarget::Key(Key::new("shift").unwrap())],
                true,
                &mut backend,
            )
            .unwrap();
        assert!(engine.latched.is_empty());
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("right_shift".into(), KeyState::Down),
                ("right_shift".into(), KeyState::Up),
            ]
        );
    }

    #[test]
    fn explicit_modifier_sides_can_be_latched_independently() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine
            .press_targets(
                &[
                    InputTarget::Key(Key::new("left_shift").unwrap()),
                    InputTarget::Key(Key::new("right_shift").unwrap()),
                ],
                &mut backend,
            )
            .unwrap();
        assert_eq!(engine.latched.len(), 2);
        engine
            .release_targets(
                &[InputTarget::Key(Key::new("right_shift").unwrap())],
                true,
                &mut backend,
            )
            .unwrap();
        assert!(
            engine
                .latched
                .contains(&InputTarget::Key(Key::new("left_shift").unwrap()))
        );
        assert!(
            !engine
                .latched
                .contains(&InputTarget::Key(Key::new("right_shift").unwrap()))
        );
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("left_shift".into(), KeyState::Down),
                ("right_shift".into(), KeyState::Down),
                ("right_shift".into(), KeyState::Up),
            ]
        );
    }

    #[test]
    fn generic_modifier_toggle_releases_a_latched_side() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine
            .press_targets(
                &[InputTarget::Key(Key::new("left_ctrl").unwrap())],
                &mut backend,
            )
            .unwrap();
        engine
            .toggle_targets(&[InputTarget::Key(Key::new("ctrl").unwrap())], &mut backend)
            .unwrap();
        assert!(engine.latched.is_empty());
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("left_ctrl".into(), KeyState::Down),
                ("left_ctrl".into(), KeyState::Up),
            ]
        );
    }

    #[test]
    fn explicit_release_emits_an_up_edge_even_when_not_tracked() {
        let mut engine = engine_with_normal_binding("n", "release shift");
        let log = run_in_normal(&mut engine, vec![key_down("n")]);
        assert_eq!(
            log.lock().unwrap().sent,
            vec![("left_shift".into(), KeyState::Up)]
        );
    }

    #[test]
    fn repeated_key_send_uses_the_same_interval_as_default_wait() {
        let engine = Engine::new(Config::default(), Appearance::Dark);
        let home = Binding::parse("home").unwrap();
        assert_eq!(
            engine.flatten_sequence(&[home.clone(), home.clone()]),
            engine.flatten_sequence(&[home.clone(), Binding::parse("wait").unwrap(), home])
        );
    }

    #[test]
    fn wait_pauses_and_resumes_a_sequence_without_blocking() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        let input = InputEvent {
            key: Key::new("n").unwrap(),
            state: KeyState::Down,
            repeat: false,
            injected: false,
            timestamp_millis: 0,
        };
        let actions = VecDeque::from(vec![
            Binding::parse("home").unwrap(),
            Binding::parse("wait 1 1").unwrap(),
            Binding::parse("home").unwrap(),
        ]);
        engine
            .continue_sequence(actions, ModeId::normal(), input, &mut backend)
            .unwrap();
        assert_eq!(log.lock().unwrap().sent.len(), 2);
        assert_eq!(engine.pending_sequences.len(), 1);
        engine.pending_sequences[0].fires_at = Instant::now();
        engine.fire_due_sequences(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("home".into(), KeyState::Down),
                ("home".into(), KeyState::Up),
                ("home".into(), KeyState::Down),
                ("home".into(), KeyState::Up),
            ]
        );
        assert!(engine.pending_sequences.is_empty());
    }

    #[test]
    fn delayed_sequence_failure_does_not_drop_other_due_sequences() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        let now = Instant::now();
        let input = InputEvent {
            key: Key::new("n").unwrap(),
            state: KeyState::Down,
            repeat: false,
            injected: false,
            timestamp_millis: 0,
        };
        engine.pending_sequences = vec![
            PendingSequence {
                fires_at: now,
                actions: VecDeque::from([Binding::Warp { x: 1, y: 2 }]),
                owner: ModeId::normal(),
                input: input.clone(),
            },
            PendingSequence {
                fires_at: now,
                actions: VecDeque::from([Binding::parse("home").unwrap()]),
                owner: ModeId::normal(),
                input,
            },
        ];
        backend.fail_warp = true;
        engine.fire_due_sequences(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("home".into(), KeyState::Down),
                ("home".into(), KeyState::Up),
            ]
        );
    }

    #[test]
    fn random_wait_stays_inside_the_inclusive_range() {
        for _ in 0..128 {
            assert!((50..=100).contains(&random_wait_ms(50, 100)));
        }
        assert_eq!(random_wait_ms(25, 25), 25);
    }

    #[test]
    fn toggled_inputs_survive_mode_and_screen_changes_until_explicitly_released() {
        let mut engine = engine_with_normal_binding("n", "toggle left_shift mouse_left");
        engine.register(Box::new(ProbeMode::new(
            "grid",
            Arc::new(Mutex::new(Vec::new())),
        )));
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine
            .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
            .unwrap();

        engine
            .handle_backend_event(key_down("n"), &mut backend)
            .unwrap();
        engine
            .handle_backend_event(key_up("n"), &mut backend)
            .unwrap();
        let grid = ModeId::new("grid").unwrap();
        engine
            .activate(grid.clone(), Some(ModeId::normal()), &mut backend)
            .unwrap();
        let screens = backend.screens().unwrap();
        engine
            .handle_backend_event(BackendEvent::ScreensChanged(screens), &mut backend)
            .unwrap();
        engine
            .activate(ModeId::idle(), Some(grid), &mut backend)
            .unwrap();

        assert_eq!(
            log.lock().unwrap().sent,
            [("left_shift".into(), KeyState::Down)],
            "mode and screen changes must not synthesize a key-up"
        );
        assert_eq!(
            log.lock().unwrap().buttons,
            [(MouseButton::Left, ButtonAction::Press)],
            "mode and screen changes must not release a toggled mouse button"
        );
        assert_eq!(engine.latched.len(), 2);

        engine
            .toggle_targets(
                &[
                    InputTarget::Key(Key::new("left_shift").unwrap()),
                    InputTarget::Mouse(crate::api::binding::Button::Left),
                ],
                &mut backend,
            )
            .unwrap();
        assert!(engine.latched.is_empty());
        assert_eq!(
            log.lock().unwrap().sent,
            [
                ("left_shift".into(), KeyState::Down),
                ("left_shift".into(), KeyState::Up),
            ]
        );
        assert_eq!(
            log.lock().unwrap().buttons,
            [
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
        );
    }

    #[test]
    fn a_binding_to_an_unregistered_mode_is_ignored() {
        let mut engine = engine_with_normal_binding("z", "plugin:missing");
        run_in_normal(&mut engine, vec![key_down("z")]);
        assert_eq!(engine.active_mode().as_str(), "normal");
    }

    #[test]
    fn unbound_keys_still_reach_the_mode_for_label_input() {
        // Grid and hint modes read raw characters this way.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "l", "move_right");
        run_in_normal(&mut engine, vec![key_down("x")]);

        let log = seen.lock().unwrap().clone();
        assert!(log.contains(&"normal:key".to_string()), "{log:?}");
    }

    #[test]
    fn capturing_mode_consumes_keys_and_idle_forwards_them() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));

        let (mut backend, log) = FakeBackend::new(vec![key_down("x")]);
        engine.run(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().dispositions,
            vec![KeyDisposition::Forward]
        );

        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.register(Box::new(ProbeMode::new("idle", seen.clone())));
        let (mut backend, log) = FakeBackend::new(vec![key_down("x")]);
        engine.run(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().dispositions,
            vec![KeyDisposition::Consume]
        );
    }

    #[test]
    fn injected_keys_are_never_dispatched_to_modes() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.register(Box::new(ProbeMode::new("idle", seen.clone())));

        let (mut backend, _) = FakeBackend::new(vec![BackendEvent::Input(InputEvent {
            key: Key::new("g").unwrap(),
            state: KeyState::Down,
            repeat: false,
            injected: true,
            timestamp_millis: 0,
        })]);
        engine.run(&mut backend).unwrap();

        let log = seen.lock().unwrap().clone();
        assert!(!log.iter().any(|e| e.ends_with(":key")), "{log:?}");
    }

    #[test]
    fn identical_scenes_are_presented_only_once() {
        struct Redrawer(ModeId);
        impl Mode for Redrawer {
            fn id(&self) -> ModeId {
                self.0.clone()
            }
            fn handle(&mut self, event: &ModeEvent, _c: &HostContext<'_>) -> Vec<Command> {
                match event {
                    ModeEvent::Activated { .. } | ModeEvent::Key { .. } => {
                        vec![Command::ShowOverlay(OverlayScene::new())]
                    }
                    _ => Vec::new(),
                }
            }
        }

        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.register(Box::new(Redrawer(ModeId::idle())));
        let (mut backend, log) = FakeBackend::new(vec![key_down("a"), key_down("b")]);
        engine.run(&mut backend).unwrap();

        // Activation draws once; the two identical redraws are suppressed.
        assert_eq!(log.lock().unwrap().presents, 1);
    }

    #[test]
    fn one_command_batch_submits_only_its_final_overlay_state() {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine.screens = backend.screens().unwrap();
        engine.cursor = backend.pointer().unwrap();

        let mut first = OverlayScene::new();
        first.backdrop = Some(crate::api::Color::rgb(1, 2, 3));
        let mut final_scene = OverlayScene::new();
        final_scene.backdrop = Some(crate::api::Color::rgb(4, 5, 6));
        engine
            .execute(
                vec![
                    Command::ShowOverlay(first),
                    Command::warp_to(Point::new(30.0, 40.0)),
                    Command::ShowOverlay(final_scene),
                ],
                &mut backend,
            )
            .unwrap();

        let log = log.lock().unwrap();
        assert_eq!(log.warps, vec![Point::new(30.0, 40.0)]);
        assert_eq!(log.presents, 1);
        assert_eq!(
            log.scenes[0].backdrop,
            Some(crate::api::Color::rgb(4, 5, 6))
        );
    }

    #[test]
    fn switching_modes_drops_the_previous_modes_timers() {
        struct Ticker(ModeId);
        impl Mode for Ticker {
            fn id(&self) -> ModeId {
                self.0.clone()
            }
            fn handle(&mut self, event: &ModeEvent, _c: &HostContext<'_>) -> Vec<Command> {
                match event {
                    ModeEvent::Activated { .. } => vec![Command::SetTimer {
                        id: "tick".into(),
                        delay: Duration::from_millis(1),
                        repeating: true,
                    }],
                    _ => Vec::new(),
                }
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &[]);
        // Replace idle with a mode that arms a timer on entry.
        engine.register(Box::new(Ticker(ModeId::idle())));

        let (mut backend, _) = FakeBackend::new(enter_normal());
        engine.run(&mut backend).unwrap();

        assert!(engine.timers.is_empty(), "idle's timer outlived idle");
    }

    #[test]
    fn scan_requests_fall_back_to_configured_roles() {
        struct Scanner(ModeId);
        impl Mode for Scanner {
            fn id(&self) -> ModeId {
                self.0.clone()
            }
            fn handle(&mut self, event: &ModeEvent, _c: &HostContext<'_>) -> Vec<Command> {
                match event {
                    ModeEvent::Activated { .. } => vec![Command::ScanUi(UiScanRequest {
                        id: 1,
                        timeout_ms: 2_500,
                        bounds: None,
                        roles: Vec::new(),
                        max_depth: 0,
                        visible_only: false,
                        clickable_only: false,
                        strategy: crate::api::command::UiScanStrategy::AxTree,
                        vision: crate::api::command::VisionOptions::default(),
                        app: None,
                    })],
                    _ => Vec::new(),
                }
            }
        }

        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.register(Box::new(Scanner(ModeId::idle())));
        let (mut backend, log) = FakeBackend::new(vec![]);
        engine.run(&mut backend).unwrap();
        assert_eq!(log.lock().unwrap().scans, 1);
    }

    #[test]
    fn focus_changes_rebuild_only_when_the_effective_binding_profile_changes() {
        let mut config = Config::default();
        config.normal.app_configs = vec![
            crate::config::AppOverride {
                bundle_id: "com.example.editor".into(),
                bindings: Bindings::from([("j".into(), Binding::parse("move_left").unwrap())]),
            },
            crate::config::AppOverride {
                bundle_id: "Writer".into(),
                bindings: Bindings::from([("j".into(), Binding::parse("move_left").unwrap())]),
            },
            crate::config::AppOverride {
                bundle_id: "Admin".into(),
                bindings: Bindings::from([("j".into(), Binding::parse("move_right").unwrap())]),
            },
        ];
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        engine.register(Box::new(ProbeMode::new("idle", seen.clone())));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
        let (mut backend, _) = FakeBackend::new(Vec::new());
        let initial_rebuilds = engine.table_rebuild_count;
        let app = |bundle_id: &str, title: &str, process_id| FocusedApp {
            bundle_id: bundle_id.into(),
            window_title: title.into(),
            process_id,
        };
        fn focus(engine: &mut Engine, backend: &mut FakeBackend, app: FocusedApp) {
            engine
                .handle_backend_event(BackendEvent::FocusChanged(Some(app)), backend)
                .unwrap();
        }

        focus(
            &mut engine,
            &mut backend,
            app("com.example.browser", "Home", 1),
        );
        focus(
            &mut engine,
            &mut backend,
            app("com.example.browser", "Article", 1),
        );
        focus(
            &mut engine,
            &mut backend,
            app("com.example.mail", "Inbox", 2),
        );
        assert_eq!(
            engine.table_rebuild_count, initial_rebuilds,
            "identity and title changes with the same empty profile must reuse tables"
        );

        focus(
            &mut engine,
            &mut backend,
            app("com.example.editor", "Document", 3),
        );
        assert_eq!(engine.table_rebuild_count, initial_rebuilds + 1);
        focus(
            &mut engine,
            &mut backend,
            app("COM.EXAMPLE.EDITOR", "Another document", 4),
        );
        assert_eq!(
            engine.table_rebuild_count,
            initial_rebuilds + 1,
            "different processes matching the same override must reuse tables"
        );
        let writer = app("com.example.browser", "Writer document", 5);
        assert_eq!(
            engine.binding_profile_key_for(Some(&writer)),
            engine.binding_profile_key,
            "different matching entries with the same patch must produce the same profile"
        );
        focus(&mut engine, &mut backend, writer);
        assert_eq!(
            engine.table_rebuild_count,
            initial_rebuilds + 1,
            "different overrides resolving to the same bindings must reuse tables"
        );

        focus(
            &mut engine,
            &mut backend,
            app("com.example.browser", "Admin console", 5),
        );
        assert_eq!(engine.table_rebuild_count, initial_rebuilds + 2);
        focus(
            &mut engine,
            &mut backend,
            app("com.example.browser", "Public console", 5),
        );
        assert_eq!(
            engine.table_rebuild_count,
            initial_rebuilds + 3,
            "a title change that changes the resolved override must rebuild"
        );
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|event| event.ends_with(":focus"))
                .count(),
            8,
            "all focus events must still reach the active mode"
        );
    }

    #[test]
    fn excluded_apps_forward_every_key() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::default();
        config.general.excluded_apps = vec!["com.example.game".into()];
        let mut engine = Engine::new(config, Appearance::Dark);
        engine.register(Box::new(ProbeMode::new("idle", seen.clone())));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));

        let mut script = vec![BackendEvent::FocusChanged(Some(FocusedApp {
            bundle_id: "com.example.game".into(),
            window_title: String::new(),
            process_id: 1,
        }))];
        script.extend(enter_normal());
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();

        assert_eq!(engine.active_mode().as_str(), "idle");
        assert!(
            log.lock()
                .unwrap()
                .dispositions
                .iter()
                .all(|d| *d == KeyDisposition::Forward)
        );
    }

    #[test]
    fn a_release_is_delivered_even_though_the_chord_no_longer_matches() {
        // Regression: resolving the release by looking the chord up again fails
        // once the keys are up, which left movement running forever.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "alt+l", "move_right");

        run_in_normal(
            &mut engine,
            vec![
                key_down("left_alt"),
                key_down("l"),
                // Alt goes up first, so `alt+l` cannot match any more.
                key_up("left_alt"),
                key_up("l"),
            ],
        );

        let log = seen.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|e| e.contains("binding(move_right)"))
                .count(),
            2,
            "the release must still arrive: {log:?}"
        );
        assert!(engine.active_gestures.is_empty());
    }

    #[test]
    fn leaving_a_mode_releases_gestures_still_held() {
        // Otherwise the pointer would keep moving after the mode changed.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("l".into(), Binding::parse("move_right").unwrap());
        config.normal.bindings.insert("esc".into(), Binding::Escape);

        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));

        // Hold `l`, then escape out of the mode without releasing it.
        run_in_normal(&mut engine, vec![key_down("l"), key_down("esc")]);

        let log = seen.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|e| e.contains("binding(move_right)"))
                .count(),
            2,
            "switching modes should release the gesture: {log:?}"
        );
        assert!(engine.active_gestures.is_empty());
        assert_eq!(engine.active_mode().as_str(), "idle");
    }

    #[test]
    fn an_unavailable_keyboard_is_reported_once_at_startup() {
        // Every mode depends on the keyboard, so a silent failure here looks
        // like the whole program is broken for no reason.
        struct Deaf {
            inner: FakeBackend,
        }
        impl Backend for Deaf {
            fn poll(&mut self, t: Duration) -> Result<Option<BackendEvent>, String> {
                self.inner.poll(t)
            }
            fn dispose_key(&mut self, d: KeyDisposition) -> Result<(), String> {
                self.inner.dispose_key(d)
            }
            fn screens(&self) -> Result<Vec<Screen>, String> {
                self.inner.screens()
            }
            fn pointer(&self) -> Result<Point, String> {
                self.inner.pointer()
            }
            fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
                self.inner.focused_app()
            }
            fn warp_pointer(&self, p: Point) -> Result<(), String> {
                self.inner.warp_pointer(p)
            }
            fn move_pointer(&self, from: Point, x: f64, y: f64) -> Result<(), String> {
                self.inner.move_pointer(from, x, y)
            }
            fn mouse_button(&self, b: MouseButton, a: ButtonAction) -> Result<(), String> {
                self.inner.mouse_button(b, a)
            }
            fn scroll(&self, x: f64, y: f64) -> Result<(), String> {
                self.inner.scroll(x, y)
            }
            fn send_key(&self, k: &Key, s: KeyState) -> Result<(), String> {
                self.inner.send_key(k, s)
            }
            fn present(&mut self, s: Arc<OverlayScene>) -> Result<(), String> {
                self.inner.present(s)
            }
            fn dismiss(&mut self) -> Result<(), String> {
                self.inner.dismiss()
            }
            fn request_ui_scan(
                &mut self,
                request: crate::api::UiScanRequest,
            ) -> Result<(), String> {
                self.inner.request_ui_scan(request)
            }
            fn name(&self) -> &'static str {
                "deaf"
            }
            fn keyboard_available(&self) -> bool {
                false
            }
            fn keyboard_unavailable_reason(&self) -> Option<String> {
                Some("no permission".into())
            }
        }

        let (inner, _) = FakeBackend::new(vec![]);
        let mut backend = Deaf { inner };
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.register(Box::new(ProbeMode::new(
            "idle",
            Arc::new(Mutex::new(vec![])),
        )));

        // Must not fail: the program still controls the pointer.
        engine.run(&mut backend).unwrap();
        assert!(!backend.keyboard_available());
    }

    #[test]
    fn a_working_backend_reports_the_keyboard_as_available() {
        let (backend, _) = FakeBackend::new(vec![]);
        assert!(backend.keyboard_available());
        assert_eq!(backend.keyboard_unavailable_reason(), None);
    }

    #[test]
    fn automatic_reload_discovers_files_created_after_startup() {
        let directory = std::env::temp_dir().join(format!(
            "keysteer-runtime-reload-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let defaults = Config::default();
        let default_write_path = directory.join("keysteer.user.toml");
        let store = ConfigStore::open(&default_write_path, &defaults).unwrap();
        let mut engine = Engine::new(defaults, Appearance::Dark);
        engine.attach_discovered_config_store(store, directory.clone());

        let discovered_path = directory.join("keysteer.created-later.toml");
        std::fs::write(&discovered_path, "[pointer]\ninitial_speed = 321\n").unwrap();
        let (mut backend, _) = FakeBackend::new(vec![]);
        engine.reload_config(&mut backend).unwrap();

        assert_eq!(engine.config.pointer.initial_speed, 321.0);
        assert_eq!(
            engine.config_store.as_ref().unwrap().path(),
            discovered_path
        );

        std::fs::remove_file(&discovered_path).unwrap();
        engine.reload_config(&mut backend).unwrap();
        assert_eq!(
            engine.config.pointer.initial_speed,
            Config::default().pointer.initial_speed
        );
        assert_eq!(
            engine.config_store.as_ref().unwrap().path(),
            default_write_path
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_config_reload_remains_pinned_to_its_path() {
        let directory = std::env::temp_dir().join(format!(
            "keysteer-runtime-explicit-reload-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();

        let explicit_path = directory.join("keysteer.explicit.toml");
        std::fs::write(&explicit_path, "[pointer]\ninitial_speed = 200\n").unwrap();
        let config = Config::load(&explicit_path).unwrap();
        let store = ConfigStore::open(&explicit_path, &config).unwrap();
        let mut engine = Engine::new(config, Appearance::Dark);
        engine.attach_config_store(store);

        std::fs::write(
            directory.join("keysteer.aaa.toml"),
            "[pointer]\ninitial_speed = 999\n",
        )
        .unwrap();
        std::fs::write(&explicit_path, "[pointer]\ninitial_speed = 456\n").unwrap();
        let (mut backend, _) = FakeBackend::new(vec![]);
        engine.reload_config(&mut backend).unwrap();

        assert_eq!(engine.config.pointer.initial_speed, 456.0);
        assert_eq!(engine.config_store.as_ref().unwrap().path(), explicit_path);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_idle_binding_to_an_unregistered_plugin_mode_is_ignored() {
        // A namespaced id parses fine but may not be registered; the engine
        // must not switch to a mode that does not exist.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = Config::default();
        config
            .hotkeys
            .insert("alt+z".into(), Binding::parse("plugin:missing").unwrap());
        let mut engine = Engine::new(config, Appearance::Dark);
        engine.register(Box::new(ProbeMode::new("idle", seen.clone())));

        let (mut backend, _) = FakeBackend::new(vec![key_down("left_alt"), key_down("z")]);
        engine.run(&mut backend).unwrap();
        assert_eq!(engine.active_mode().as_str(), "idle");
    }
}
