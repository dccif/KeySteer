#![forbid(unsafe_code)]

//! Configuration.
//!
//! The section layout and theming model follow neru, but bindings are our own:
//! a value is a [`Binding`] from the public API, so `h = "move_left"` and
//! `g = "grid"` read the same whether the target is a built-in verb, a built-in
//! mode or a plugin mode. There is no `action` prefix and no separate internal
//! vocabulary.
//!
//! Nothing is required: every section and field has a default, so an absent or
//! partial config file is valid.

pub mod store;
pub mod style;
pub mod theme;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::api::backend::Appearance;
use crate::api::binding::Binding;
use crate::api::command::{ButtonAction, FocusedApp, MouseButton, UiScanStrategy, VisionOptions};
use crate::api::input::{
    Key, KeyChord, ModeId, normalize_alias_name, normalize_builtin_key, with_key_aliases,
};

pub use crate::api::hint::LabelDirection;
pub use store::ConfigStore;
pub use style::{
    Anchor, BoundaryHighlight, CursorIndicatorOverride, CursorIndicatorUi, HintPlacement,
    IndicatorUi, IndicatorUiOverride, LabelUi, SearchInputUi,
};
pub use theme::{Palette, Theme, ThemeColors, ThemedColor};

/// A binding table: chord text -> what it does.
pub type Bindings = BTreeMap<String, Binding>;

/// What a targeting mode does after a semantic lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleAction {
    Keep,
    Finish,
    Restart,
    Return,
    Mode(ModeId),
    Click {
        button: MouseButton,
        action: ButtonAction,
    },
}

impl LifecycleAction {
    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(match text.trim() {
            "keep" => Self::Keep,
            "finish" => Self::Finish,
            "restart" => Self::Restart,
            "return" => Self::Return,
            "left_click" => Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::Click,
            },
            "right_click" => Self::Click {
                button: MouseButton::Right,
                action: ButtonAction::Click,
            },
            "middle_click" => Self::Click {
                button: MouseButton::Middle,
                action: ButtonAction::Click,
            },
            "double_click" => Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::DoubleClick,
            },
            mode => Self::Mode(ModeId::new(mode)?),
        })
    }

    pub fn canonical(&self) -> &str {
        match self {
            Self::Keep => "keep",
            Self::Finish => "finish",
            Self::Restart => "restart",
            Self::Return => "return",
            Self::Mode(mode) => mode.as_str(),
            Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::Click,
            } => "left_click",
            Self::Click {
                button: MouseButton::Right,
                action: ButtonAction::Click,
            } => "right_click",
            Self::Click {
                button: MouseButton::Middle,
                action: ButtonAction::Click,
            } => "middle_click",
            Self::Click {
                button: MouseButton::Left,
                action: ButtonAction::DoubleClick,
            } => "double_click",
            Self::Click { .. } => "unsupported_click",
        }
    }
}

impl Serialize for LifecycleAction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.canonical())
    }
}

impl<'de> Deserialize<'de> for LifecycleAction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TargetingLifecycle {
    pub after_finish: LifecycleAction,
    pub after_click: LifecycleAction,
}

impl Default for TargetingLifecycle {
    fn default() -> Self {
        Self {
            after_finish: LifecycleAction::Keep,
            after_click: LifecycleAction::Keep,
        }
    }
}

/// User-defined key names shared by every platform, with optional
/// platform-specific overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyAliases {
    #[serde(default)]
    #[serde(flatten)]
    pub all: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub windows: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub macos: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub linux: BTreeMap<String, String>,
}

impl Default for KeyAliases {
    fn default() -> Self {
        Self {
            all: BTreeMap::new(),
            windows: BTreeMap::from([("Primary".into(), "left_alt".into())]),
            macos: BTreeMap::new(),
            linux: BTreeMap::new(),
        }
    }
}

impl KeyAliases {
    fn effective(&self) -> Result<BTreeMap<String, String>, ConfigError> {
        let mut effective = normalized_alias_scope("[key_aliases]", &self.all)?;
        let (label, platform) = if cfg!(target_os = "windows") {
            ("[key_aliases.windows]", &self.windows)
        } else if cfg!(target_os = "macos") {
            ("[key_aliases.macos]", &self.macos)
        } else if cfg!(target_os = "linux") {
            ("[key_aliases.linux]", &self.linux)
        } else {
            ("[key_aliases]", &self.all)
        };
        if !std::ptr::eq(platform, &self.all) {
            effective.extend(normalized_alias_scope(label, platform)?);
        }
        Ok(effective)
    }
}

/// Flag chords that will not behave as the user expects on this platform.
///
/// These are warnings, not errors: the user may genuinely want the binding, or
/// may have a layout where the problem does not apply. They exist because the
/// failure mode is otherwise baffling — the key appears bound and simply does
/// nothing, or types a character instead.
fn platform_warning(chord: &KeyChord) -> Option<String> {
    let keys: Vec<&str> = chord.keys().iter().map(|k| k.as_str()).collect();
    let has = |name: &str| keys.contains(&name);
    let has_any = |names: &[&str]| names.iter().any(|n| has(n));

    let alt = has_any(&["alt", "left_alt", "right_alt"]);
    let ctrl = has_any(&["ctrl", "left_ctrl", "right_ctrl"]);
    let shift = has_any(&["shift", "left_shift", "right_shift"]);
    let cmd = has_any(&["win", "left_win", "right_win"]);
    let activation = chord.activation_key().as_str();
    let is_letter = activation.len() == 1
        && activation
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());

    // macOS: Option+<letter> is a dead key or special character, so the OS
    // consumes it to compose text. This is why `alt+e` never fires.
    if cfg!(target_os = "macos") && alt && !cmd && !ctrl && is_letter {
        return Some(format!(
            "on macOS, Option+{} types a special character (Option+E is a \
             dead-key accent) and may not reach this program. Prefer \
             `primary+shift+{}`, which is Cmd+Shift here and Ctrl+Shift elsewhere",
            activation.to_uppercase(),
            activation
        ));
    }

    // macOS reserves Cmd+Space and Cmd+Tab system-wide. Cmd+Q is intentionally
    // allowed for grid-like mode exit bindings, which the event tap consumes.
    if cfg!(target_os = "macos") && cmd && !shift && !alt && matches!(activation, "space" | "tab") {
        return Some(format!(
            "on macOS, Cmd+{activation} is reserved by the system \
             (Spotlight / app switcher / quit) and will not reach this program"
        ));
    }

    // Linux/Windows terminals own Ctrl+Shift+C and Ctrl+Shift+V for clipboard.
    if !cfg!(target_os = "macos") && ctrl && shift && matches!(activation, "c" | "v") {
        return Some(format!(
            "Ctrl+Shift+{} is the clipboard shortcut in most terminals and \
             will be swallowed there",
            activation.to_uppercase()
        ));
    }

    // F21-F24 do not exist on Apple keyboards.
    if cfg!(target_os = "macos")
        && let Some(digits) = activation.strip_prefix('f')
        && let Ok(n) = digits.parse::<u32>()
        && (21..=24).contains(&n)
    {
        return Some(format!(
            "F{n} does not exist on macOS; use F1-F20 for a portable binding"
        ));
    }

    None
}

/// Root configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    /// User-defined key names. Values resolve to one concrete key before the
    /// runtime binding tables are compiled.
    #[serde(default)]
    pub key_aliases: KeyAliases,
    #[serde(default)]
    pub debug: DebugConfig,
    /// Settings whose meaning is specific to an operating system.
    #[serde(default)]
    pub platform: PlatformConfig,
    #[serde(default)]
    pub theme: Theme,
    /// Bindings active in `idle`. Normally just the key that enters `normal`.
    ///
    /// Keeping this table small is the point of `idle`: the program stays
    /// silent until the user asks for it.
    #[serde(default = "default_idle_bindings")]
    pub hotkeys: Bindings,
    #[serde(default)]
    pub normal: Normal,
    #[serde(default)]
    pub ui_hint: UiHint,
    #[serde(default)]
    pub grid: Grid,
    #[serde(default)]
    pub recursive_grid: RecursiveGrid,
    #[serde(default)]
    pub scroll: Scroll,
    #[serde(default)]
    pub pointer: Pointer,
    #[serde(default)]
    pub mode_indicator: ModeIndicator,
    /// Binding tables for plugin modes, keyed by mode id.
    ///
    /// A plugin mode is configured exactly like a built-in one; it just lives
    /// here because its id is not known at compile time.
    #[serde(default = "default_plugin_modes")]
    pub plugin_modes: BTreeMap<String, PluginModeConfig>,
    /// Per-app overrides for the idle binding table.
    #[serde(default)]
    pub app_configs: Vec<AppOverride>,
    #[serde(skip)]
    resolved_key_aliases: BTreeMap<String, String>,
}

pub(crate) struct LoadedConfig {
    pub(crate) config: Config,
    pub(crate) raw_text: String,
    pub(crate) path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let mut config = Self {
            general: General::default(),
            key_aliases: KeyAliases::default(),
            debug: DebugConfig::default(),
            platform: PlatformConfig::default(),
            theme: Theme::default(),
            hotkeys: default_idle_bindings(),
            normal: Normal::default(),
            ui_hint: UiHint::default(),
            grid: Grid::default(),
            recursive_grid: RecursiveGrid::default(),
            scroll: Scroll::default(),
            pointer: Pointer::default(),
            mode_indicator: ModeIndicator::default(),
            plugin_modes: default_plugin_modes(),
            app_configs: Vec::new(),
            resolved_key_aliases: BTreeMap::new(),
        };
        let aliases = match config
            .key_aliases
            .effective()
            .and_then(|aliases| compile_key_aliases(&aliases))
        {
            Ok(aliases) => aliases,
            Err(error) => panic!("built-in key aliases must be valid: {error}"),
        };
        config.resolved_key_aliases = aliases;
        if let Err(error) = config.apply_configured_key_aliases() {
            panic!("built-in bindings must be valid: {error}");
        }
        config
    }
}

fn default_plugin_modes() -> BTreeMap<String, PluginModeConfig> {
    BTreeMap::from([(
        "plugin:screen-selector".into(),
        PluginModeConfig {
            settings: BTreeMap::from([("preserve".into(), toml::Value::Boolean(true))]),
            ..Default::default()
        },
    )])
}

/// Defaults for `idle`: only what is needed to wake the program up.
///
/// Modelled on neru's launcher bindings, which use `Primary+Shift+<letter>`:
/// `Primary` is Cmd on macOS and Ctrl elsewhere, so one file works on both.
///
/// `Primary+Shift` rather than a bare modifier matters on macOS, where
/// `Option+<letter>` types a special character (`Option+E` is a dead-key acute
/// accent), and on Linux, where `Ctrl+Shift+C/V` are taken by terminals. The
/// letters here avoid both.
fn default_idle_bindings() -> Bindings {
    let entries: &[(&str, &str)] = &[
        // Idle remains silent until this single portable launcher is pressed.
        ("primary+e", "normal"),
    ];
    entries
        .iter()
        .filter_map(|(chord, binding)| Some((chord.to_string(), Binding::parse(binding).ok()?)))
        .collect()
}

impl Config {
    /// Bindings for `mode_id`, or `None` if that mode has no table.
    ///
    /// Every mode resolves its keys the same way, including plugin modes:
    /// a plugin declares `[plugin_modes."my-plugin:zoom".bindings]` and gets the
    /// same treatment as `[grid.bindings]`.
    pub fn bindings_for(&self, mode_id: &str) -> Option<&Bindings> {
        match mode_id {
            "idle" => Some(&self.hotkeys),
            "normal" => Some(&self.normal.bindings),
            "grid" => Some(&self.grid.bindings),
            "recursive_grid" => Some(&self.recursive_grid.bindings),
            "ui_hint" => Some(&self.ui_hint.bindings),
            "hotkeys" => Some(&self.hotkeys),
            other => self.plugin_modes.get(other).map(|mode| &mode.bindings),
        }
    }

    /// Ordered inheritance and optional temporary-mode override for a mode.
    pub fn inheritance_for(&self, mode_id: &str) -> Option<(&[String], Option<&str>, &[String])> {
        match mode_id {
            "normal" => Some((&self.normal.inherits, None, &[])),
            "grid" => Some((
                &self.grid.inherits,
                self.grid.temporary_mode.as_deref(),
                &self.grid.temporary_mode_keys,
            )),
            "recursive_grid" => Some((
                &self.recursive_grid.inherits,
                self.recursive_grid.temporary_mode.as_deref(),
                &self.recursive_grid.temporary_mode_keys,
            )),
            "ui_hint" => Some((
                &self.ui_hint.inherits,
                self.ui_hint.temporary_mode.as_deref(),
                &self.ui_hint.temporary_mode_keys,
            )),
            other => self.plugin_modes.get(other).map(|mode| {
                (
                    mode.inherits.as_slice(),
                    mode.temporary_mode.as_deref(),
                    mode.temporary_mode_keys.as_slice(),
                )
            }),
        }
    }

    pub fn plugin_setting_bool(&self, mode_id: &str, key: &str) -> Option<bool> {
        self.plugin_modes.get(mode_id)?.settings.get(key)?.as_bool()
    }

    pub fn app_binding_overrides_for(&self, mode_id: &str) -> Vec<(&str, &Bindings)> {
        if mode_id == "ui_hint" {
            return self
                .ui_hint
                .app_configs
                .iter()
                .map(|over| (over.bundle_id.as_str(), &over.bindings))
                .collect();
        }
        let overrides: &[AppOverride] = match mode_id {
            "idle" => &self.app_configs,
            "normal" => &self.normal.app_configs,
            "grid" => &self.grid.app_configs,
            "recursive_grid" => &self.recursive_grid.app_configs,
            other => self
                .plugin_modes
                .get(other)
                .map(|mode| mode.app_configs.as_slice())
                .unwrap_or(&[]),
        };
        overrides
            .iter()
            .map(|over| (over.bundle_id.as_str(), &over.bindings))
            .collect()
    }

    /// Exact per-mode override patches that affect compiled binding tables.
    ///
    /// Storing the merged patch rather than the matching pattern indices means
    /// two applications resolving to identical bindings can share compiled
    /// tables even if they matched different configuration entries.
    pub(crate) fn binding_profile_key<'a>(
        &self,
        mode_ids: impl IntoIterator<Item = &'a str>,
        app: Option<&FocusedApp>,
    ) -> Vec<Bindings> {
        let Some(app) = app else {
            return Vec::new();
        };
        let profile: Vec<Bindings> = mode_ids
            .into_iter()
            .map(|mode_id| {
                let mut resolved = Bindings::new();
                for (pattern, bindings) in self.app_binding_overrides_for(mode_id) {
                    if app_override_matches(pattern, app) {
                        for (chord, binding) in bindings {
                            resolved.insert(chord.clone(), binding.clone());
                        }
                    }
                }
                resolved
            })
            .collect();
        if profile.iter().all(Bindings::is_empty) {
            Vec::new()
        } else {
            profile
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PluginModeConfig {
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    /// Plugin-owned values exposed through the host settings snapshot.
    pub settings: BTreeMap<String, toml::Value>,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

// ---------------------------------------------------------------------------
// [general]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Apps where the engine never activates.
    pub excluded_apps: Vec<String>,
}

// ---------------------------------------------------------------------------
// [platform]
// ---------------------------------------------------------------------------

/// Operating-system-specific behavior. These fields remain deserializable on
/// every target so one configuration file can be shared across platforms.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformConfig {
    pub macos: MacOsConfig,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MacOsConfig {
    pub scroll: MacOsScrollConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacOsScrollConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert_horizontal: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert_vertical: Option<bool>,
    /// Deprecated compatibility input for the former all-axis switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
}

impl Default for MacOsScrollConfig {
    fn default() -> Self {
        Self {
            invert_horizontal: Some(false),
            invert_vertical: Some(true),
            invert: None,
        }
    }
}

/// Verbose runtime tracing written to stderr for diagnosing native input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DebugConfig {
    pub enabled: bool,
    pub keys: bool,
    pub actions: bool,
    pub modes: bool,
    pub backend: bool,
    /// Pointer coordinates are extremely high-volume; opt in separately.
    pub pointer: bool,
    /// High-volume OS key-repeat, movement and cursor-overlay details.
    pub motion: bool,
    pub overlay: bool,
    pub timers: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            keys: true,
            actions: true,
            modes: true,
            backend: true,
            pointer: false,
            motion: false,
            overlay: true,
            timers: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-app overrides
// ---------------------------------------------------------------------------

/// Per-app override of a binding table.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppOverride {
    /// macOS bundle id, Linux `WM_CLASS`/`app_id`, Windows executable name,
    /// or a substring of the window title.
    pub bundle_id: String,
    pub bindings: Bindings,
}

pub fn app_override_matches(pattern: &str, app: &FocusedApp) -> bool {
    !pattern.is_empty()
        && (pattern.eq_ignore_ascii_case(&app.bundle_id)
            || app
                .window_title
                .to_lowercase()
                .contains(&pattern.to_lowercase()))
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiHintAppOverride {
    pub bundle_id: String,
    pub strategy: Option<UiScanStrategy>,
    pub bindings: Bindings,
}

// ---------------------------------------------------------------------------
// [normal]
// ---------------------------------------------------------------------------

/// The working mode: move the pointer, click, scroll, and enter the other
/// modes.
///
/// Everything here is a plain binding table, so a user can rebind `hjkl` to
/// anything, add `t = "home"`, or point a key at a plugin mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Normal {
    pub inherits: Vec<String>,
    /// Forward keyboard input that does not match a complete KeySteer binding.
    pub passthrough_unbound_keys: bool,
    /// Hold a physical key bound to click/double-click for this many
    /// milliseconds to toggle that mouse button. The same threshold lets a
    /// parameterless toggle activation key latch itself. Zero disables both.
    pub long_press_toggle_ms: u64,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Default for Normal {
    fn default() -> Self {
        Self {
            inherits: vec!["hotkeys".into()],
            passthrough_unbound_keys: true,
            long_press_toggle_ms: 500,
            bindings: default_normal_bindings(),
            app_configs: Vec::new(),
        }
    }
}

/// Defaults for `normal`: vim-style movement, clicks, scrolling, the three
/// targeting modes, and the navigation keys.
///
/// Bare letters are free for pointer control, while unbound keys pass through
/// by default. Complete modifier combinations take precedence over bare-key
/// bindings so external shortcuts remain usable.
fn default_normal_bindings() -> Bindings {
    let entries: &[(&str, &str)] = &[
        // Movement and speed modifiers, held alongside a direction.
        ("h", "move_left"),
        ("j", "move_down"),
        ("k", "move_up"),
        ("l", "move_right"),
        ("caps_lock", "precision"),
        ("left_shift", "slow"),
        ("v", "fast"),
        ("b", "fast"),
        // Scroll takes effect immediately on a tap and repeats while held.
        ("m", "wheel_down"),
        (",", "wheel_up"),
        // Pointer buttons.
        (";", "left_click"),
        ("'", "right_click"),
        ("right_shift", "middle_click"),
        ("n", "toggle"),
        // Targeting modes available directly from normal.
        ("g", "grid"),
        ("f", "recursive_grid"),
        ("primary+f", "ui_hint"),
        ("primary+s", "screen next"),
        // Navigation keys sent to the focused application.
        ("u", "page_down"),
        ("i", "page_up"),
        ("t", "home"),
        ("y", "end"),
        // Normal has no label-key conflict, so bare q exits immediately.
        ("q", "idle"),
        ("esc", "idle"),
    ];
    entries
        .iter()
        .filter_map(|(chord, binding)| Some((chord.to_string(), Binding::parse(binding).ok()?)))
        .collect()
}

// ---------------------------------------------------------------------------
// [ui_hint]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiHint {
    pub enabled: bool,
    pub strategy: UiScanStrategy,
    pub vision: VisionOptions,
    /// Characters used to build hint labels.
    pub hint_characters: String,
    pub label_direction: LabelDirection,
    /// Accessibility tree depth limit; 0 means unlimited.
    pub max_depth: u32,
    /// Soft budget for one accessibility scan. Incremental results remain
    /// visible while the scan continues.
    pub scan_timeout_ms: u64,
    /// Automatic retries when a scan times out or returns no targets.
    pub scan_retry_count: u32,
    /// Delay before an automatic retry, allowing a busy UI provider to settle.
    pub scan_retry_delay_ms: u64,
    /// Semantic roles that receive a hint.
    pub clickable_roles: Vec<String>,
    /// Skip the clickability heuristic and hint every matching role.
    pub ignore_clickable_check: bool,
    /// Hit-test each element; slower but removes occluded hints.
    pub visible_check_enabled: bool,
    pub placement: HintPlacement,
    /// Pixel offsets applied after placing each label relative to its element.
    pub label_x_offset: i32,
    pub label_y_offset: i32,
    pub ui: LabelUi,
    pub boundary_highlight: BoundaryHighlight,
    pub search_input_ui: SearchInputUi,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub lifecycle: TargetingLifecycle,
    /// Modifier held to expose the next label in each overlapping group.
    pub overlap_cycle_key: String,
    pub bindings: Bindings,
    pub app_configs: Vec<UiHintAppOverride>,
}

impl UiHint {
    pub fn strategy_for(&self, app: Option<&FocusedApp>) -> UiScanStrategy {
        let Some(app) = app else {
            return self.strategy;
        };
        self.app_configs
            .iter()
            .find(|over| app_override_matches(&over.bundle_id, app))
            .and_then(|over| over.strategy)
            .unwrap_or(self.strategy)
    }

    /// Side-agnostic modifiers such as `shift` match either physical key.
    pub fn overlap_cycle_matches(&self, key: &Key) -> bool {
        KeyChord::parse(&self.overlap_cycle_key).is_ok_and(|chord| chord.activation_matches(key))
    }

    /// Whether a temporary-mode key denotes the same physical modifier family
    /// as the overlap key. In UI Hint, overlap cycling deliberately wins.
    pub fn overlap_cycle_conflicts_with(&self, temporary_key: &str) -> bool {
        let (Ok(overlap), Ok(temporary)) = (
            KeyChord::parse(&self.overlap_cycle_key),
            KeyChord::parse(temporary_key),
        ) else {
            return false;
        };
        overlap.activation_matches(temporary.activation_key())
            || temporary.activation_matches(overlap.activation_key())
    }
}

impl Default for UiHint {
    fn default() -> Self {
        let ui = LabelUi {
            font_size: 15,
            ..Default::default()
        };
        let search_input_ui = SearchInputUi {
            label: LabelUi {
                font_size: 14,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            enabled: true,
            strategy: UiScanStrategy::Vision,
            vision: VisionOptions::default(),
            hint_characters: "asdfghjkl".into(),
            label_direction: LabelDirection::Normal,
            max_depth: 50,
            scan_timeout_ms: 2_500,
            scan_retry_count: 1,
            scan_retry_delay_ms: 200,
            clickable_roles: default_clickable_roles(),
            ignore_clickable_check: false,
            visible_check_enabled: false,
            placement: HintPlacement::Bottom,
            label_x_offset: 0,
            label_y_offset: -4,
            ui,
            boundary_highlight: BoundaryHighlight::default(),
            search_input_ui,
            inherits: vec!["hotkeys".into(), "normal".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            lifecycle: TargetingLifecycle {
                after_finish: LifecycleAction::Mode(ModeId::normal()),
                after_click: LifecycleAction::Mode(ModeId::normal()),
            },
            overlap_cycle_key: "shift".into(),
            bindings: Bindings::from([
                ("primary+r".into(), Binding::RescanUi),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]),
            app_configs: Vec::new(),
        }
    }
}

/// Neru's semantic role vocabulary. Backends map these to native roles.
fn default_clickable_roles() -> Vec<String> {
    [
        "button",
        "menu_button",
        "popup_button",
        "combo_box",
        "link",
        "checkbox",
        "radio",
        "switch",
        "text_field",
        "text_area",
        "search_field",
        "slider",
        "stepper",
        "tab",
        "menu_item",
        "cell",
        "list_item",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// [grid]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grid {
    pub enabled: bool,
    /// Columns in every selection layer.
    pub grid_cols: u32,
    /// Rows in every selection layer.
    pub grid_rows: u32,
    /// One selection key per cell, in row-major keyboard order.
    pub keys: String,
    /// Number of selection layers before the current cell becomes the target.
    pub max_depth: u32,
    /// Move the pointer to each selected cell's centre while drilling down.
    pub cursor_follow_selection: bool,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub lifecycle: TargetingLifecycle,
    pub ui: GridUi,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Default for Grid {
    fn default() -> Self {
        let ui = GridUi {
            label: LabelUi {
                font_size: 20,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            enabled: true,
            grid_cols: 5,
            grid_rows: 4,
            keys: "12345qwertasdfgzxcvb".into(),
            max_depth: 3,
            cursor_follow_selection: true,
            inherits: vec!["hotkeys".into(), "normal".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            lifecycle: TargetingLifecycle {
                after_finish: LifecycleAction::Mode(ModeId::normal()),
                after_click: LifecycleAction::Finish,
            },
            ui,
            bindings: Bindings::from([
                ("`".into(), Binding::ToggleCursorFollowSelection),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]),
            app_configs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GridUi {
    #[serde(flatten)]
    pub label: LabelUi,
    pub matched_background_color: Option<ThemedColor>,
    pub matched_border_color: Option<ThemedColor>,
}

// ---------------------------------------------------------------------------
// [recursive_grid]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecursiveGrid {
    pub enabled: bool,
    pub grid_cols: u32,
    pub grid_rows: u32,
    /// Cell keys; must hold `grid_cols * grid_rows` characters.
    pub keys: String,
    pub min_size_width: u32,
    pub min_size_height: u32,
    /// Recursion limit, 1..=20.
    pub max_depth: u32,
    /// Move the pointer to each selected cell's centre while drilling down.
    pub cursor_follow_selection: bool,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub lifecycle: TargetingLifecycle,
    /// Per-depth layout overrides.
    pub layers: Vec<GridLayer>,
    pub ui: RecursiveGridUi,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Default for RecursiveGrid {
    fn default() -> Self {
        let ui = RecursiveGridUi {
            label: LabelUi {
                font_size: 20,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            enabled: true,
            grid_cols: 3,
            grid_rows: 3,
            keys: "qweasdzxc".into(),
            min_size_width: 1,
            min_size_height: 1,
            max_depth: 10,
            cursor_follow_selection: true,
            inherits: vec!["hotkeys".into(), "normal".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            lifecycle: TargetingLifecycle::default(),
            layers: Vec::new(),
            ui,
            bindings: Bindings::from([
                ("`".into(), Binding::ToggleCursorFollowSelection),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]),
            app_configs: Vec::new(),
        }
    }
}

/// Overrides the grid shape at one recursion depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridLayer {
    /// Zero-based depth this entry applies to.
    pub depth: u32,
    pub grid_cols: Option<u32>,
    pub grid_rows: Option<u32>,
    pub keys: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecursiveGridUi {
    #[serde(flatten)]
    pub label: LabelUi,
    pub line_width: i32,
    pub line_color: Option<ThemedColor>,
    pub highlight_color: Option<ThemedColor>,
    /// Draw a filled pill behind each cell key.
    pub label_background: bool,
    pub label_background_color: Option<ThemedColor>,
    /// Replace every cell label with this character, e.g. `·`.
    pub label_char: String,
    /// Hide labels when fitting them would require a smaller font.
    pub label_min_font_size: i32,
    /// Hide labels once a cell is smaller than the fitted font size times this
    /// multiplier. `0` disables this additional threshold.
    pub label_autohide_multiplier: f64,
    pub sub_key_preview: bool,
    pub sub_key_preview_font_size: i32,
    pub sub_key_preview_text_color: Option<ThemedColor>,
    pub sub_key_preview_autohide_multiplier: f64,
}

impl Default for RecursiveGridUi {
    fn default() -> Self {
        Self {
            label: LabelUi::default(),
            line_width: 1,
            line_color: None,
            highlight_color: None,
            label_background: false,
            label_background_color: None,
            label_char: String::new(),
            label_min_font_size: 6,
            label_autohide_multiplier: 1.5,
            sub_key_preview: false,
            sub_key_preview_font_size: 8,
            sub_key_preview_text_color: None,
            sub_key_preview_autohide_multiplier: 1.5,
        }
    }
}

// ---------------------------------------------------------------------------
// [scroll] and [pointer]
//
// These are tuning parameters, not modes: scrolling is a set of bindings that
// any mode can use, so there is no `scroll` mode to enter.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scroll {
    /// Pixels for `scroll_up` and friends.
    pub scroll_step: i32,
    /// Pixels for `scroll_half_up` and friends.
    pub scroll_step_half: i32,
    /// Pixels for `scroll_full_up` and friends.
    pub scroll_step_full: i32,
    /// Deprecated compatibility input. Use `platform.macos.scroll.invert`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invert_scroll: Option<bool>,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            scroll_step: 50,
            scroll_step_half: 500,
            scroll_step_full: 1_000_000,
            invert_scroll: None,
        }
    }
}

impl Scroll {
    /// Pixels for one scroll binding.
    pub fn pixels(&self, amount: crate::api::binding::ScrollAmount) -> f64 {
        use crate::api::binding::ScrollAmount as A;
        match amount {
            A::Step => self.scroll_step as f64,
            A::Half => self.scroll_step_half as f64,
            A::Full => self.scroll_step_full as f64,
        }
    }
}

/// Keyboard-driven pointer acceleration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pointer {
    /// Initial pointer velocity in pixels per second.
    pub initial_speed: f64,
    /// Maximum pointer velocity in pixels per second.
    pub max_speed: f64,
    /// Velocity added per second while held, in pixels per second².
    pub acceleration: f64,
    /// Ease acceleration at both ends instead of changing velocity linearly.
    pub smooth_acceleration: bool,
    /// Immediate pixel distance for a tap shorter than the next display update.
    pub tap_distance: f64,
    pub slow_multiplier: f64,
    pub precision_multiplier: f64,
    pub fast_multiplier: f64,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            initial_speed: 1000.0,
            max_speed: 2200.0,
            acceleration: 3000.0,
            smooth_acceleration: true,
            tap_distance: 2.5,
            slow_multiplier: 0.35,
            precision_multiplier: 0.12,
            fast_multiplier: 2.0,
        }
    }
}

// ---------------------------------------------------------------------------
// [mode_indicator]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModeIndicator {
    pub cursor: CursorIndicatorUi,
    pub ui: IndicatorUi,
    /// Per-mode overrides, keyed by mode id.
    ///
    /// A nested table rather than flattened entries, because `flatten` and
    /// `deny_unknown_fields` cannot be combined: together they reject every
    /// sibling key, including `ui`.
    pub modes: BTreeMap<String, ModeIndicatorEntry>,
}

impl Default for ModeIndicator {
    fn default() -> Self {
        let ui = IndicatorUi {
            label: LabelUi {
                font_size: 11,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            cursor: CursorIndicatorUi::default(),
            ui,
            modes: BTreeMap::from([(
                "normal".into(),
                ModeIndicatorEntry {
                    enabled: Some(true),
                    text: Some("Normal".into()),
                    ..Default::default()
                },
            )]),
        }
    }
}

impl ModeIndicator {
    /// Whether to show a badge for `mode_id`, and its merged visual style.
    pub fn for_mode(&self, mode_id: &str, display_name: &str) -> Option<(String, IndicatorUi)> {
        let entry = self.modes.get(mode_id);
        let enabled = entry.and_then(|e| e.enabled).unwrap_or(mode_id != "idle");
        if !enabled {
            return None;
        }
        let text = entry
            .and_then(|e| e.text.clone())
            .unwrap_or_else(|| display_name.to_string());
        let ui = entry
            .map(|entry| entry.ui.apply(&self.ui))
            .unwrap_or_else(|| self.ui.clone());
        Some((text, ui))
    }

    pub fn cursor_for_mode(&self, mode_id: &str) -> Option<CursorIndicatorUi> {
        if mode_id == "idle" {
            return None;
        }
        let cursor = self
            .modes
            .get(mode_id)
            .map(|entry| entry.cursor.apply(&self.cursor))
            .unwrap_or_else(|| self.cursor.clone());
        cursor.enabled.then_some(cursor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModeIndicatorEntry {
    pub enabled: Option<bool>,
    pub text: Option<String>,
    pub cursor: CursorIndicatorOverride,
    pub ui: IndicatorUiOverride,
}

// ---------------------------------------------------------------------------
// Loading & validation
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    Invalid(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(s) => write!(f, "{s}"),
            ConfigError::Parse(s) => write!(f, "invalid TOML: {s}"),
            ConfigError::Invalid(s) => write!(f, "invalid configuration: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {}

fn validate_optional_color(path: &str, color: Option<&ThemedColor>) -> Result<(), ConfigError> {
    if color.is_some_and(|value| !value.is_valid()) {
        return Err(ConfigError::Invalid(format!(
            "{path} must use #RRGGBBAA for every appearance"
        )));
    }
    Ok(())
}

fn validate_label_colors(path: &str, label: &LabelUi) -> Result<(), ConfigError> {
    for (name, value) in [
        ("background_color", label.background_color.as_ref()),
        ("text_color", label.text_color.as_ref()),
        ("matched_text_color", label.matched_text_color.as_ref()),
        ("border_color", label.border_color.as_ref()),
    ] {
        validate_optional_color(&format!("{path}.{name}"), value)?;
    }
    Ok(())
}

fn normalized_alias_scope(
    label: &str,
    configured: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let mut normalized = BTreeMap::new();
    for (name, target) in configured {
        let name = normalize_alias_name(name);
        if name.is_empty() || name.contains('+') {
            return Err(ConfigError::Invalid(format!(
                "{label} alias {name:?} must name one key"
            )));
        }
        if normalized.insert(name.clone(), target.clone()).is_some() {
            return Err(ConfigError::Invalid(format!(
                "{label} alias {name:?} is defined more than once after normalization"
            )));
        }
    }
    Ok(normalized)
}

fn compile_key_aliases(
    configured: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let raw = normalized_alias_scope("[key_aliases]", configured)?;

    fn resolve(
        name: &str,
        raw: &BTreeMap<String, String>,
        resolved: &mut BTreeMap<String, String>,
        visiting: &mut BTreeSet<String>,
    ) -> Result<String, ConfigError> {
        if let Some(target) = resolved.get(name) {
            return Ok(target.clone());
        }
        if !visiting.insert(name.to_string()) {
            return Err(ConfigError::Invalid(format!(
                "key alias cycle contains {name:?}"
            )));
        }
        let Some(target) = raw.get(name) else {
            return Err(ConfigError::Invalid(format!(
                "key alias {name:?} disappeared while resolving aliases"
            )));
        };
        if target.contains('+') {
            return Err(ConfigError::Invalid(format!(
                "key alias {name:?} must resolve to one key, not chord {target:?}"
            )));
        }
        let target_name = normalize_alias_name(target);
        let canonical = if raw.contains_key(&target_name) {
            resolve(&target_name, raw, resolved, visiting)?
        } else {
            normalize_builtin_key(target)
        };
        visiting.remove(name);
        if !Key::is_known(&canonical) {
            return Err(ConfigError::Invalid(format!(
                "key alias {name:?} resolves to unknown key {target:?}"
            )));
        }
        resolved.insert(name.to_string(), canonical.clone());
        Ok(canonical)
    }

    let mut resolved = BTreeMap::new();
    for name in raw.keys() {
        resolve(name, &raw, &mut resolved, &mut BTreeSet::new())?;
    }
    Ok(resolved)
}

fn normalize_binding_keys(
    table: &mut Bindings,
    label: &str,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let source = std::mem::take(table);
    for (text, binding) in source {
        let names: Vec<&str> = text.split_ascii_whitespace().collect();
        let names = if names.len() > 1 {
            names
        } else {
            vec![text.as_str()]
        };
        for name in names {
            let uses_alias = name
                .split('+')
                .any(|part| aliases.contains_key(&normalize_alias_name(part)));
            let canonical = if uses_alias {
                KeyChord::parse_with_aliases(name, aliases)
                    .map_err(|error| {
                        ConfigError::Parse(format!(
                            "{label} binding {name:?} is not a valid key or chord: {error}"
                        ))
                    })?
                    .canonical()
            } else if text.split_ascii_whitespace().count() > 1 {
                let chord = KeyChord::parse(name).map_err(|error| {
                    ConfigError::Parse(format!(
                        "{label} whitespace alias {name:?} is not a single key: {error}"
                    ))
                })?;
                if chord.keys().len() != 1 {
                    return Err(ConfigError::Parse(format!(
                        "{label} whitespace aliases must be single keys; use `+` for a chord: {text:?}"
                    )));
                }
                chord.canonical()
            } else {
                name.to_string()
            };
            if table.insert(canonical.clone(), binding.clone()).is_some() {
                return Err(ConfigError::Parse(format!(
                    "{label} binding {text:?} resolves to duplicate chord {canonical:?}"
                )));
            }
        }
    }
    Ok(())
}

fn normalize_key_list(
    keys: &mut [String],
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    for key in keys {
        normalize_key_if_aliased(key, aliases)?;
    }
    Ok(())
}

fn normalize_key_if_aliased(
    key: &mut String,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    if aliases.contains_key(&normalize_alias_name(key)) {
        *key = Key::new_with_aliases(&*key, aliases)
            .map_err(ConfigError::Parse)?
            .to_string();
    }
    Ok(())
}

impl Config {
    /// Locate the first config in the active application data directory.
    ///
    /// Portable configs are named `keysteer.<name>.toml`. User profiles sort
    /// by name; the annotated `keysteer.default.toml` example is considered
    /// only when no user profile exists.
    pub fn discover() -> Option<PathBuf> {
        let directory = crate::app::paths::data_dir()?;
        Self::discover_in(&directory)
    }

    pub(crate) fn discover_in(directory: &Path) -> Option<PathBuf> {
        let mut matches = std::fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                if !entry.file_type().ok()?.is_file()
                    || !Self::is_portable_config_name(&entry.file_name())
                {
                    return None;
                }
                Some(entry.path())
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            let left_name = left
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let right_name = right
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let left_is_default = left_name == "keysteer.default.toml";
            let right_is_default = right_name == "keysteer.default.toml";
            left_is_default
                .cmp(&right_is_default)
                .then_with(|| left_name.cmp(&right_name))
                .then_with(|| left.cmp(right))
        });
        matches.into_iter().next()
    }

    pub(crate) fn is_portable_config_name(name: &std::ffi::OsStr) -> bool {
        let name = name.to_string_lossy().to_ascii_lowercase();
        name.strip_prefix("keysteer.")
            .and_then(|rest| rest.strip_suffix(".toml"))
            .is_some_and(|profile| !profile.is_empty())
    }

    pub fn default_write_path() -> Option<PathBuf> {
        crate::app::paths::data_file("keysteer.user.toml")
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::load_with_source(path).map(|loaded| loaded.config)
    }

    pub(crate) fn load_with_source(path: &Path) -> Result<LoadedConfig, ConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::Io(format!("cannot read {}: {e}", path.display())))?;
        let config = Self::parse(&text)?;
        config.validate()?;
        Ok(LoadedConfig {
            config,
            raw_text: text,
            path: path.to_path_buf(),
        })
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let value: toml::Value =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let key_aliases = value
            .get("key_aliases")
            .cloned()
            .map(KeyAliases::deserialize)
            .transpose()
            .map_err(|e| ConfigError::Parse(e.to_string()))?
            .unwrap_or_default();
        let aliases = compile_key_aliases(&key_aliases.effective()?)?;
        let mut config: Self = with_key_aliases(&aliases, || Self::deserialize(value))
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.resolved_key_aliases = aliases;
        config.apply_configured_key_aliases()?;
        Ok(config)
    }

    /// Expand whitespace-separated single-key aliases in a binding key.
    ///
    /// For example, `"v b" = "fast"` is stored as the two ordinary bindings
    /// `v = "fast"` and `b = "fast"`. Chords still use `+`, so
    /// `primary+f` remains one binding.
    fn apply_configured_key_aliases(&mut self) -> Result<(), ConfigError> {
        let aliases = self.resolved_key_aliases.clone();
        normalize_binding_keys(&mut self.hotkeys, "[hotkeys]", &aliases)?;
        normalize_binding_keys(&mut self.normal.bindings, "[normal.bindings]", &aliases)?;
        normalize_binding_keys(&mut self.grid.bindings, "[grid.bindings]", &aliases)?;
        normalize_binding_keys(
            &mut self.recursive_grid.bindings,
            "[recursive_grid.bindings]",
            &aliases,
        )?;
        normalize_binding_keys(&mut self.ui_hint.bindings, "[ui_hint.bindings]", &aliases)?;
        for (id, mode) in &mut self.plugin_modes {
            normalize_binding_keys(
                &mut mode.bindings,
                &format!("[plugin_modes.{id:?}.bindings]"),
                &aliases,
            )?;
            for over in &mut mode.app_configs {
                normalize_binding_keys(
                    &mut over.bindings,
                    &format!("[[plugin_modes.{id:?}.app_configs]] {:?}", over.bundle_id),
                    &aliases,
                )?;
            }
        }
        for (label, overrides) in [
            ("[[app_configs]]", &mut self.app_configs),
            ("[[normal.app_configs]]", &mut self.normal.app_configs),
            ("[[grid.app_configs]]", &mut self.grid.app_configs),
            (
                "[[recursive_grid.app_configs]]",
                &mut self.recursive_grid.app_configs,
            ),
        ] {
            for over in overrides {
                normalize_binding_keys(
                    &mut over.bindings,
                    &format!("{label} {:?}", over.bundle_id),
                    &aliases,
                )?;
            }
        }
        for over in &mut self.ui_hint.app_configs {
            normalize_binding_keys(
                &mut over.bindings,
                &format!("[[ui_hint.app_configs]] {:?}", over.bundle_id),
                &aliases,
            )?;
        }
        normalize_key_if_aliased(&mut self.ui_hint.overlap_cycle_key, &aliases)?;
        for keys in [
            &mut self.grid.temporary_mode_keys,
            &mut self.recursive_grid.temporary_mode_keys,
            &mut self.ui_hint.temporary_mode_keys,
        ] {
            normalize_key_list(keys, &aliases)?;
        }
        for mode in self.plugin_modes.values_mut() {
            normalize_key_list(&mut mode.temporary_mode_keys, &aliases)?;
        }
        Ok(())
    }

    pub(crate) fn resolved_key_aliases(&self) -> &BTreeMap<String, String> {
        &self.resolved_key_aliases
    }

    pub fn to_toml(&self) -> String {
        // Export only the new platform hierarchy. A legacy value is projected
        // into it so dumping and reloading a migrated configuration preserves
        // behavior without perpetuating the deprecated field.
        let mut exported = self.clone();
        let legacy = exported
            .platform
            .macos
            .scroll
            .invert
            .or(exported.scroll.invert_scroll);
        if let Some(legacy) = legacy {
            if exported.platform.macos.scroll.invert_horizontal.is_none() {
                exported.platform.macos.scroll.invert_horizontal = Some(legacy);
            }
            if exported.platform.macos.scroll.invert_vertical.is_none() {
                exported.platform.macos.scroll.invert_vertical = Some(legacy);
            }
        }
        exported.platform.macos.scroll.invert = None;
        exported.scroll.invert_scroll = None;
        toml::to_string_pretty(&exported).unwrap_or_else(|e| format!("# serialization failed: {e}"))
    }

    /// Effective macOS values, with explicit per-axis settings taking priority.
    pub fn macos_scroll_invert(&self) -> (bool, bool) {
        let legacy = self
            .platform
            .macos
            .scroll
            .invert
            .or(self.scroll.invert_scroll);
        (
            self.platform
                .macos
                .scroll
                .invert_horizontal
                .or(legacy)
                .unwrap_or(false),
            self.platform
                .macos
                .scroll
                .invert_vertical
                .or(legacy)
                .unwrap_or(true),
        )
    }

    /// Effective horizontal and vertical inversion on the current platform.
    pub fn effective_scroll_invert(&self) -> (bool, bool) {
        #[cfg(target_os = "macos")]
        {
            self.macos_scroll_invert()
        }
        #[cfg(not(target_os = "macos"))]
        {
            // The inversion controls model macOS's common natural-scrolling
            // preference. Windows and other platforms must preserve semantic
            // wheel directions: `wheel_down` stays down and `wheel_up` stays
            // up, regardless of macOS or deprecated compatibility settings in
            // a configuration shared across machines.
            (false, false)
        }
    }

    /// Deprecated settings that should be moved to their replacement path.
    pub fn deprecation_warnings(&self) -> Vec<String> {
        self.scroll
            .invert_scroll
            .map(|_| {
                "`scroll.invert_scroll` is deprecated; use \
                 `platform.macos.scroll.invert` instead"
                    .to_string()
            })
            .into_iter()
            .collect()
    }

    /// Resolve the palette for the current system appearance.
    pub fn palette(&self, appearance: Appearance) -> Palette {
        self.theme.palette(appearance)
    }

    /// Chords that will not behave as expected on this platform.
    ///
    /// Separate from [`Self::validate`] because these are advisory, and because
    /// a pure function can be tested and reported exactly once by the caller
    /// rather than every time the config happens to be validated.
    pub fn platform_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for (label, table) in self.binding_tables() {
            for chord in table.keys() {
                let Ok(parsed) = KeyChord::parse(chord) else {
                    continue; // `validate` reports unparseable chords.
                };
                if let Some(problem) = platform_warning(&parsed) {
                    warnings.push(format!("{label} binding {chord:?}: {problem}"));
                }
            }
        }

        warnings
    }

    /// Reject configurations that would misbehave at runtime.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: String| ConfigError::Invalid(m);

        if self.normal.long_press_toggle_ms > 60_000 {
            return Err(bad("normal.long_press_toggle_ms must be 0..=60000".into()));
        }

        for (appearance, colors) in [("light", &self.theme.light), ("dark", &self.theme.dark)] {
            for (name, value) in [
                ("surface", &colors.surface),
                ("accent", &colors.accent),
                ("accent_alt", &colors.accent_alt),
                ("on_accent_alt", &colors.on_accent_alt),
                ("text", &colors.text),
            ] {
                if crate::api::Color::parse(value).is_none() {
                    return Err(bad(format!("theme.{appearance}.{name} must use #RRGGBBAA")));
                }
            }
        }

        validate_label_colors("grid.ui", &self.grid.ui.label)?;
        validate_optional_color(
            "grid.ui.matched_background_color",
            self.grid.ui.matched_background_color.as_ref(),
        )?;
        validate_optional_color(
            "grid.ui.matched_border_color",
            self.grid.ui.matched_border_color.as_ref(),
        )?;
        validate_label_colors("recursive_grid.ui", &self.recursive_grid.ui.label)?;
        if self.recursive_grid.ui.label_min_font_size <= 0 {
            return Err(bad(
                "recursive_grid.ui.label_min_font_size must be positive".into(),
            ));
        }
        for (name, multiplier) in [
            (
                "label_autohide_multiplier",
                self.recursive_grid.ui.label_autohide_multiplier,
            ),
            (
                "sub_key_preview_autohide_multiplier",
                self.recursive_grid.ui.sub_key_preview_autohide_multiplier,
            ),
        ] {
            if !multiplier.is_finite() || multiplier < 0.0 {
                return Err(bad(format!(
                    "recursive_grid.ui.{name} must be finite and non-negative"
                )));
            }
        }
        for (name, value) in [
            ("line_color", self.recursive_grid.ui.line_color.as_ref()),
            (
                "highlight_color",
                self.recursive_grid.ui.highlight_color.as_ref(),
            ),
            (
                "label_background_color",
                self.recursive_grid.ui.label_background_color.as_ref(),
            ),
            (
                "sub_key_preview_text_color",
                self.recursive_grid.ui.sub_key_preview_text_color.as_ref(),
            ),
        ] {
            validate_optional_color(&format!("recursive_grid.ui.{name}"), value)?;
        }
        validate_label_colors("ui_hint.ui", &self.ui_hint.ui)?;
        validate_optional_color(
            "ui_hint.boundary_highlight.background_color",
            self.ui_hint.boundary_highlight.background_color.as_ref(),
        )?;
        validate_optional_color(
            "ui_hint.boundary_highlight.border_color",
            self.ui_hint.boundary_highlight.border_color.as_ref(),
        )?;
        validate_label_colors(
            "ui_hint.search_input_ui",
            &self.ui_hint.search_input_ui.label,
        )?;
        validate_label_colors("mode_indicator.ui", &self.mode_indicator.ui.label)?;
        for (name, value) in [
            ("fill_color", self.mode_indicator.cursor.fill_color.as_ref()),
            (
                "stroke_color",
                self.mode_indicator.cursor.stroke_color.as_ref(),
            ),
            (
                "left_pressed_color",
                self.mode_indicator.cursor.left_pressed_color.as_ref(),
            ),
            (
                "middle_pressed_color",
                self.mode_indicator.cursor.middle_pressed_color.as_ref(),
            ),
            (
                "right_pressed_color",
                self.mode_indicator.cursor.right_pressed_color.as_ref(),
            ),
        ] {
            validate_optional_color(&format!("mode_indicator.cursor.{name}"), value)?;
        }
        if self.mode_indicator.cursor.radius <= 0 || self.mode_indicator.cursor.stroke_width < 0 {
            return Err(bad(
                "mode_indicator.cursor radius must be positive and stroke_width non-negative"
                    .into(),
            ));
        }
        for (mode, entry) in &self.mode_indicator.modes {
            for (name, value) in [
                ("cursor.fill_color", entry.cursor.fill_color.as_ref()),
                ("cursor.stroke_color", entry.cursor.stroke_color.as_ref()),
                (
                    "cursor.left_pressed_color",
                    entry.cursor.left_pressed_color.as_ref(),
                ),
                (
                    "cursor.middle_pressed_color",
                    entry.cursor.middle_pressed_color.as_ref(),
                ),
                (
                    "cursor.right_pressed_color",
                    entry.cursor.right_pressed_color.as_ref(),
                ),
                ("ui.background_color", entry.ui.background_color.as_ref()),
                ("ui.text_color", entry.ui.text_color.as_ref()),
                (
                    "ui.matched_text_color",
                    entry.ui.matched_text_color.as_ref(),
                ),
                ("ui.border_color", entry.ui.border_color.as_ref()),
            ] {
                validate_optional_color(&format!("mode_indicator.modes.{mode}.{name}"), value)?;
            }
            if entry.cursor.radius.is_some_and(|value| value <= 0)
                || entry.cursor.stroke_width.is_some_and(|value| value < 0)
            {
                return Err(bad(format!(
                    "mode_indicator.modes.{mode}.cursor has invalid dimensions"
                )));
            }
        }

        if self.ui_hint.enabled && self.ui_hint.hint_characters.chars().count() < 2 {
            return Err(bad(
                "ui_hint.hint_characters needs at least 2 characters".into()
            ));
        }
        if !(250..=30_000).contains(&self.ui_hint.scan_timeout_ms) {
            return Err(bad("ui_hint.scan_timeout_ms must be 250..=30000".into()));
        }
        if self.ui_hint.scan_retry_count > 5 {
            return Err(bad("ui_hint.scan_retry_count must be 0..=5".into()));
        }
        if self.ui_hint.scan_retry_delay_ms > 5_000 {
            return Err(bad("ui_hint.scan_retry_delay_ms must be 0..=5000".into()));
        }
        let overlap_cycle_key = Key::new(&self.ui_hint.overlap_cycle_key).map_err(|error| {
            bad(format!(
                "ui_hint.overlap_cycle_key contains an invalid key: {error}"
            ))
        })?;
        if !overlap_cycle_key.is_modifier() {
            return Err(bad(
                "ui_hint.overlap_cycle_key must be a modifier key".into()
            ));
        }
        let vision = &self.ui_hint.vision;
        if !vision.detect_text && !vision.detect_rectangles {
            return Err(bad(
                "ui_hint.vision must enable detect_text or detect_rectangles".into(),
            ));
        }
        if !(1..=30_000).contains(&vision.request_timeout_ms) {
            return Err(bad(
                "ui_hint.vision.request_timeout_ms must be 1..=30000".into()
            ));
        }
        if !(1..=2_000).contains(&vision.rectangle_max_candidates) {
            return Err(bad(
                "ui_hint.vision.rectangle_max_candidates must be 1..=2000".into(),
            ));
        }
        for (name, value) in [
            ("minimum_confidence", vision.minimum_confidence),
            ("merge_iou_threshold", vision.merge_iou_threshold),
            ("rectangle_min_size", vision.rectangle_min_size),
            ("button_min_confidence", vision.button_min_confidence),
            (
                "generic_clickable_min_confidence",
                vision.generic_clickable_min_confidence,
            ),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(bad(format!(
                    "ui_hint.vision.{name} must be between 0 and 1"
                )));
            }
        }
        for (name, value) in [
            ("rectangle_min_aspect", vision.rectangle_min_aspect),
            ("rectangle_max_aspect", vision.rectangle_max_aspect),
            ("button_min_aspect", vision.button_min_aspect),
            ("button_max_aspect", vision.button_max_aspect),
            ("button_icon_max_size", vision.button_icon_max_size),
            ("link_min_aspect", vision.link_min_aspect),
            ("link_max_height", vision.link_max_height),
            ("link_min_width", vision.link_min_width),
            ("image_min_size", vision.image_min_size),
            ("checkbox_max_size", vision.checkbox_max_size),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(bad(format!("ui_hint.vision.{name} must be positive")));
            }
        }
        if vision.rectangle_min_aspect > vision.rectangle_max_aspect
            || vision.button_min_aspect > vision.button_max_aspect
        {
            return Err(bad(
                "ui_hint.vision minimum aspect ratios must not exceed maximums".into(),
            ));
        }
        for (mode, keys) in [
            ("grid", &self.grid.temporary_mode_keys),
            ("recursive_grid", &self.recursive_grid.temporary_mode_keys),
            ("ui_hint", &self.ui_hint.temporary_mode_keys),
        ] {
            for key in keys {
                let key = Key::new(key).map_err(|error| {
                    bad(format!(
                        "{mode}.temporary_mode_keys contains an invalid key: {error}"
                    ))
                })?;
                if !key.is_modifier() {
                    return Err(bad(format!(
                        "{mode}.temporary_mode_keys may contain only modifier keys"
                    )));
                }
            }
        }
        if self.grid.enabled {
            let grid = &self.grid;
            let cells = (grid.grid_cols as usize) * (grid.grid_rows as usize);
            if grid.grid_cols == 0 || grid.grid_rows == 0 || cells < 2 {
                return Err(bad(
                    "grid needs at least 2 cells (grid_cols * grid_rows)".into()
                ));
            }
            if grid.keys.chars().count() != cells {
                return Err(bad(format!(
                    "grid.keys has {} characters but the {}x{} grid needs {cells}",
                    grid.keys.chars().count(),
                    grid.grid_cols,
                    grid.grid_rows,
                )));
            }
            if grid
                .keys
                .chars()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != cells
            {
                return Err(bad("grid.keys must not contain duplicate characters".into()));
            }
            if !(1..=20).contains(&grid.max_depth) {
                return Err(bad("grid.max_depth must be 1..=20".into()));
            }
        }

        if self.recursive_grid.enabled {
            let rg = &self.recursive_grid;
            let cells = (rg.grid_cols as usize) * (rg.grid_rows as usize);
            if rg.grid_cols == 0 || rg.grid_rows == 0 || cells < 2 {
                return Err(bad(
                    "recursive_grid needs at least 2 cells (grid_cols * grid_rows)".into(),
                ));
            }
            if rg.keys.chars().count() != cells {
                return Err(bad(format!(
                    "recursive_grid.keys has {} characters but the {}x{} grid needs {cells}",
                    rg.keys.chars().count(),
                    rg.grid_cols,
                    rg.grid_rows,
                )));
            }
            if rg
                .keys
                .chars()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != cells
            {
                return Err(bad(
                    "recursive_grid.keys must not contain duplicate characters".into(),
                ));
            }
            if !(1..=20).contains(&rg.max_depth) {
                return Err(bad("recursive_grid.max_depth must be 1..=20".into()));
            }
            for layer in &rg.layers {
                let cols = layer.grid_cols.unwrap_or(rg.grid_cols) as usize;
                let rows = layer.grid_rows.unwrap_or(rg.grid_rows) as usize;
                if cols == 0 || rows == 0 || cols * rows < 2 {
                    return Err(bad(format!(
                        "recursive_grid.layers[depth={}] needs at least 2 cells",
                        layer.depth
                    )));
                }
                if let Some(keys) = &layer.keys
                    && keys.chars().count() != cols * rows
                {
                    return Err(bad(format!(
                        "recursive_grid.layers[depth={}].keys has {} characters but needs {}",
                        layer.depth,
                        keys.chars().count(),
                        cols * rows,
                    )));
                }
            }
        }

        for (mode, lifecycle) in [
            ("grid", &self.grid.lifecycle),
            ("recursive_grid", &self.recursive_grid.lifecycle),
            ("ui_hint", &self.ui_hint.lifecycle),
        ] {
            if lifecycle.after_finish == LifecycleAction::Finish {
                return Err(bad(format!(
                    "{mode}.lifecycle.after_finish cannot trigger finish recursively"
                )));
            }
            if matches!(lifecycle.after_click, LifecycleAction::Click { .. }) {
                return Err(bad(format!(
                    "{mode}.lifecycle.after_click cannot trigger another click"
                )));
            }
            for (field, action) in [
                ("after_finish", &lifecycle.after_finish),
                ("after_click", &lifecycle.after_click),
            ] {
                if let LifecycleAction::Mode(target) = action {
                    let known = ModeId::BUILT_IN.contains(&target.as_str())
                        || self.plugin_modes.contains_key(target.as_str());
                    if !known {
                        return Err(bad(format!(
                            "{mode}.lifecycle.{field} names unknown mode {:?}",
                            target.as_str()
                        )));
                    }
                }
                if action.canonical() == "unsupported_click" {
                    return Err(bad(format!(
                        "{mode}.lifecycle.{field} contains an unsupported click"
                    )));
                }
            }
        }

        // Every binding chord must parse, or it would silently never fire.
        for (label, table) in self.binding_tables() {
            for chord in table.keys() {
                KeyChord::parse(chord).map_err(|e| {
                    bad(format!(
                        "{label} binding {chord:?} is not a valid chord: {e}"
                    ))
                })?;
            }
        }

        validate_inheritance(self).map_err(bad)?;

        if self.pointer.max_speed <= 0.0 {
            return Err(bad("pointer.max_speed must be positive".into()));
        }
        if self.pointer.initial_speed <= 0.0 {
            return Err(bad("pointer.initial_speed must be positive".into()));
        }
        if self.pointer.acceleration < 0.0 {
            return Err(bad("pointer.acceleration must not be negative".into()));
        }
        if self.pointer.tap_distance < 0.0 {
            return Err(bad("pointer.tap_distance must not be negative".into()));
        }
        if self.pointer.precision_multiplier <= 0.0
            || self.pointer.slow_multiplier <= 0.0
            || self.pointer.fast_multiplier <= 0.0
        {
            return Err(bad(
                "pointer precision/slow/fast multipliers must be positive".into(),
            ));
        }

        Ok(())
    }

    /// Every binding table with a label for error messages.
    fn binding_tables(&self) -> Vec<(String, &Bindings)> {
        let mut tables: Vec<(String, &Bindings)> = vec![
            ("[hotkeys]".into(), &self.hotkeys),
            ("[normal.bindings]".into(), &self.normal.bindings),
            ("[grid.bindings]".into(), &self.grid.bindings),
            (
                "[recursive_grid.bindings]".into(),
                &self.recursive_grid.bindings,
            ),
            ("[ui_hint.bindings]".into(), &self.ui_hint.bindings),
        ];
        for (id, mode) in &self.plugin_modes {
            tables.push((format!("[plugin_modes.{id:?}.bindings]"), &mode.bindings));
        }
        for over in &self.app_configs {
            tables.push((
                format!("[[app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.normal.app_configs {
            tables.push((
                format!("[[normal.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.grid.app_configs {
            tables.push((
                format!("[[grid.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.recursive_grid.app_configs {
            tables.push((
                format!("[[recursive_grid.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.ui_hint.app_configs {
            tables.push((
                format!("[[ui_hint.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for (id, mode) in &self.plugin_modes {
            for over in &mode.app_configs {
                tables.push((
                    format!("[[plugin_modes.{id:?}.app_configs]] {:?}", over.bundle_id),
                    &over.bindings,
                ));
            }
        }
        tables
    }
}

fn validate_inheritance(config: &Config) -> Result<(), String> {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::from([
        ("normal".into(), config.normal.inherits.clone()),
        ("grid".into(), config.grid.inherits.clone()),
        (
            "recursive_grid".into(),
            config.recursive_grid.inherits.clone(),
        ),
        ("ui_hint".into(), config.ui_hint.inherits.clone()),
    ]);
    for (id, mode) in &config.plugin_modes {
        graph.insert(id.clone(), mode.inherits.clone());
    }
    let known = |name: &str| name == "hotkeys" || graph.contains_key(name);
    for (mode, sources) in &graph {
        for source in sources {
            if !known(source) {
                return Err(format!(
                    "{mode}.inherits contains unknown source {source:?}"
                ));
            }
        }
    }
    for (mode, source) in [
        ("grid", config.grid.temporary_mode.as_deref()),
        (
            "recursive_grid",
            config.recursive_grid.temporary_mode.as_deref(),
        ),
        ("ui_hint", config.ui_hint.temporary_mode.as_deref()),
    ] {
        if let Some(source) = source
            && !known(source)
        {
            return Err(format!(
                "{mode}.temporary_mode names unknown mode {source:?}"
            ));
        }
    }

    fn visit(
        mode: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut std::collections::BTreeSet<String>,
        done: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        if done.contains(mode) || mode == "hotkeys" {
            return Ok(());
        }
        if !visiting.insert(mode.to_string()) {
            return Err(format!("inheritance cycle contains {mode:?}"));
        }
        if let Some(sources) = graph.get(mode) {
            for source in sources {
                visit(source, graph, visiting, done)?;
            }
        }
        visiting.remove(mode);
        done.insert(mode.to_string());
        Ok(())
    }

    let mut done = std::collections::BTreeSet::new();
    for mode in graph.keys() {
        visit(
            mode,
            &graph,
            &mut std::collections::BTreeSet::new(),
            &mut done,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModeId;
    use crate::api::binding::{Button, Direction, ScrollAmount, Speed};

    #[test]
    fn portable_config_names_require_a_profile() {
        assert!(Config::is_portable_config_name(
            "keysteer.user.toml".as_ref()
        ));
        assert!(Config::is_portable_config_name(
            "KEYSTEER.Work.TOML".as_ref()
        ));
        assert!(!Config::is_portable_config_name("keysteer.toml".as_ref()));
        assert!(!Config::is_portable_config_name("keysteer..toml".as_ref()));
        assert!(!Config::is_portable_config_name("config.toml".as_ref()));
    }

    #[test]
    fn default_write_path_uses_the_application_data_directory() {
        let expected = crate::app::paths::data_file("keysteer.user.toml").unwrap();
        assert_eq!(Config::default_write_path(), Some(expected));
    }

    #[test]
    fn portable_discovery_is_filtered_and_deterministic() {
        let directory = std::env::temp_dir().join(format!(
            "keysteer-config-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        for name in [
            "keysteer.zebra.toml",
            "keysteer.Alpha.toml",
            "KEYSTEER.DEFAULT.TOML",
            "keysteer..toml",
            "config.toml",
        ] {
            std::fs::write(directory.join(name), "").unwrap();
        }

        assert_eq!(
            Config::discover_in(&directory),
            Some(directory.join("keysteer.Alpha.toml"))
        );

        std::fs::remove_file(directory.join("keysteer.Alpha.toml")).unwrap();
        std::fs::remove_file(directory.join("keysteer.zebra.toml")).unwrap();
        assert_eq!(
            Config::discover_in(&directory),
            Some(directory.join("KEYSTEER.DEFAULT.TOML")),
            "the annotated default should remain a fallback when no user profile exists"
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn user_key_alias_can_override_primary_with_one_physical_side() {
        let config = Config::parse(
            r#"
            [key_aliases]
            Primary = "left_alt"

            [hotkeys]
            "Primary+e" = "normal"
            "alt+f" = "grid"
            "right_alt+g" = "recursive_grid"
            "#,
        )
        .unwrap();

        assert!(config.hotkeys.contains_key("left_alt+e"));
        assert!(config.hotkeys.contains_key("alt+f"));
        assert!(config.hotkeys.contains_key("right_alt+g"));
        assert_eq!(config.resolved_key_aliases()["primary"], "left_alt");
    }

    #[test]
    fn current_platform_aliases_override_global_aliases_only_here() {
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let text = format!(
            r#"
            [key_aliases]
            Primary = "alt"
            Hyper = "right_ctrl"

            [key_aliases.{platform}]
            Primary = "left_shift"

            [hotkeys]
            "Primary+e" = "normal"
            "Hyper+g" = "grid"
            "#
        );
        let config = Config::parse(&text).unwrap();

        assert!(config.hotkeys.contains_key("left_shift+e"));
        assert!(config.hotkeys.contains_key("right_ctrl+g"));
        assert_eq!(config.resolved_key_aliases()["primary"], "left_shift");
    }

    #[test]
    fn inactive_platform_aliases_do_not_change_this_platform() {
        let inactive = if cfg!(target_os = "windows") {
            "macos"
        } else {
            "windows"
        };
        let text = format!(
            r#"
            [key_aliases]
            Primary = "alt"

            [key_aliases.{inactive}]
            Primary = "left_shift"

            [hotkeys]
            "Primary+e" = "normal"
            "#
        );
        let config = Config::parse(&text).unwrap();

        assert!(config.hotkeys.contains_key("alt+e"));
        assert!(!config.hotkeys.contains_key("left_shift+e"));
    }

    #[test]
    fn platform_key_aliases_round_trip_through_toml() {
        let config = Config::parse(
            r#"
            [key_aliases]
            Hyper = "right_ctrl"

            [key_aliases.windows]
            Primary = "left_alt"

            [key_aliases.macos]
            Primary = "left_cmd"
            "#,
        )
        .unwrap();
        let reparsed = Config::parse(&config.to_toml()).unwrap();

        assert_eq!(reparsed.key_aliases, config.key_aliases);
        assert_eq!(
            reparsed.resolved_key_aliases(),
            config.resolved_key_aliases()
        );
    }

    #[test]
    fn custom_key_aliases_chain_and_apply_to_send_actions() {
        let config = Config::parse(
            r#"
            [key_aliases]
            Primary = "Hyper"
            Hyper = "right_ctrl"

            [normal.bindings]
            h = "send Primary+x"
            "#,
        )
        .unwrap();

        match &config.normal.bindings["h"] {
            Binding::Send(chord) => assert_eq!(chord.canonical(), "right_ctrl+x"),
            other => panic!("expected send binding, got {other:?}"),
        }
    }

    #[test]
    fn invalid_key_aliases_are_rejected() {
        for text in [
            "[key_aliases]\nPrimary = 'missing_key'",
            "[key_aliases]\nPrimary = 'left_alt+right_alt'",
            "[key_aliases]\nPrimary = 'Hyper'\nHyper = 'Primary'",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn empty_config_is_valid_and_equals_the_defaults() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        config.validate().unwrap();
    }

    #[test]
    fn partial_config_keeps_defaults_for_omitted_fields() {
        let config = Config::parse(
            r#"
            [scroll]
            scroll_step = 25
            "#,
        )
        .unwrap();
        assert_eq!(config.scroll.scroll_step, 25);
        // Untouched fields keep their documented defaults.
        assert_eq!(config.scroll.scroll_step_half, 500);
        assert_eq!(config.grid.keys, Grid::default().keys);
    }

    #[test]
    fn pointer_smooth_acceleration_defaults_on_and_can_be_disabled() {
        let defaults = Config::default().pointer;
        assert_eq!(defaults.initial_speed, 1000.0);
        assert_eq!(defaults.max_speed, 2200.0);
        assert_eq!(defaults.acceleration, 3000.0);
        assert!(defaults.smooth_acceleration);

        let config = Config::parse("[pointer]\nsmooth_acceleration = false").unwrap();
        assert!(!config.pointer.smooth_acceleration);
        assert!(config.to_toml().contains("smooth_acceleration = false"));
    }

    #[test]
    fn macos_scroll_axes_have_independent_defaults() {
        assert_eq!(Config::default().macos_scroll_invert(), (false, true));
        let config = Config::parse(
            r#"
            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = false
            "#,
        )
        .unwrap();
        assert_eq!(config.macos_scroll_invert(), (true, false));
        assert!(config.deprecation_warnings().is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_ignores_macos_and_legacy_scroll_inversion() {
        let config = Config::parse(
            r#"
            [scroll]
            invert_scroll = true

            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = true
            "#,
        )
        .unwrap();

        assert_eq!(config.effective_scroll_invert(), (false, false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_effective_scroll_uses_macos_axis_settings() {
        let config = Config::parse(
            r#"
            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = false
            "#,
        )
        .unwrap();

        assert_eq!(config.effective_scroll_invert(), (true, false));
    }

    #[test]
    fn all_macos_scroll_axis_combinations_are_preserved() {
        for horizontal in [false, true] {
            for vertical in [false, true] {
                let mut config = Config::default();
                config.platform.macos.scroll.invert_horizontal = Some(horizontal);
                config.platform.macos.scroll.invert_vertical = Some(vertical);
                assert_eq!(config.macos_scroll_invert(), (horizontal, vertical));
            }
        }
    }

    #[test]
    fn explicit_axis_settings_override_legacy_values() {
        let config = Config::parse(
            r#"
            [scroll]
            invert_scroll = true

            [platform.macos.scroll]
            invert_horizontal = false
            invert_vertical = true
            "#,
        )
        .unwrap();
        assert_eq!(config.macos_scroll_invert(), (false, true));
        assert_eq!(config.deprecation_warnings().len(), 1);
    }

    #[test]
    fn exported_legacy_scroll_setting_is_migrated_to_both_axes() {
        let config = Config::parse(
            r#"
            [platform.macos.scroll]
            invert = true
            "#,
        )
        .unwrap();
        let exported = config.to_toml();
        assert!(!exported.contains("invert ="), "{exported}");
        let reparsed = Config::parse(&exported).unwrap();
        assert_eq!(reparsed.platform.macos.scroll.invert_horizontal, Some(true));
        assert_eq!(reparsed.platform.macos.scroll.invert_vertical, Some(true));
        assert_eq!(reparsed.macos_scroll_invert(), (true, true));
    }

    #[test]
    fn idle_binds_only_mode_launchers() {
        let config = Config::default();
        // The silence guarantee: idle does nothing except launch a mode.
        assert!(
            config.hotkeys.values().all(|b| b.mode().is_some()),
            "idle should only enter modes, got {:?}",
            config.hotkeys
        );
        // And `normal` must be among them, or the program is unreachable.
        assert_eq!(config.hotkeys.len(), 1);
        assert!(
            config
                .hotkeys
                .values()
                .any(|b| b.mode() == Some(&ModeId::normal()))
        );
    }

    #[test]
    fn idle_launchers_resolve_the_platform_neutral_primary_modifier() {
        // The source default uses `primary`; runtime tables contain its
        // concrete key so matching never needs to resolve aliases again.
        let config = Config::default();
        for chord in config.hotkeys.keys() {
            assert!(
                !chord.starts_with("primary+"),
                "unresolved launcher {chord:?}"
            );
            assert_eq!(KeyChord::parse(chord).unwrap().keys().len(), 2);
        }
    }

    #[test]
    fn idle_launchers_avoid_platform_reserved_chords() {
        // Regression: `alt+e` never fired on macOS because Option+E is a
        // dead key. No default may have that problem on any platform.
        for chord in Config::default().hotkeys.keys() {
            let parsed = KeyChord::parse(chord).unwrap();
            assert_eq!(
                platform_warning(&parsed),
                None,
                "default idle binding {chord:?} is problematic on this platform"
            );
        }
    }

    #[test]
    fn normal_defaults_avoid_platform_reserved_chords() {
        for chord in Config::default().normal.bindings.keys() {
            let parsed = KeyChord::parse(chord).unwrap();
            assert_eq!(
                platform_warning(&parsed),
                None,
                "default normal binding {chord:?} is problematic on this platform"
            );
        }
    }

    #[test]
    fn normal_defaults_cover_the_requested_controls_and_targeting_modes() {
        let normal = &Config::default().normal.bindings;
        assert_eq!(normal["h"], Binding::Move(Direction::Left));
        assert_eq!(normal["j"], Binding::Move(Direction::Down));
        assert_eq!(normal["k"], Binding::Move(Direction::Up));
        assert_eq!(normal["l"], Binding::Move(Direction::Right));
        assert_eq!(normal["left_shift"], Binding::Speed(Speed::Slow));
        assert_eq!(normal["caps_lock"], Binding::Speed(Speed::Precision));
        assert_eq!(normal["v"], Binding::Speed(Speed::Fast));
        assert_eq!(normal["b"], Binding::Speed(Speed::Fast));
        assert!(!normal.contains_key("e"));
        assert!(!normal.contains_key("r"));
        assert_eq!(
            normal["m"],
            Binding::Scroll(Direction::Down, ScrollAmount::Step)
        );
        assert_eq!(
            normal[","],
            Binding::Scroll(Direction::Up, ScrollAmount::Step)
        );
        assert_eq!(normal[";"], Binding::Click(Button::Left));
        assert_eq!(normal["'"], Binding::Click(Button::Right));
        assert_eq!(normal["n"], Binding::Toggle(Vec::new()));
        assert_eq!(normal["g"], Binding::Mode(ModeId::grid()));
        assert_eq!(normal["f"], Binding::Mode(ModeId::recursive_grid()));
        assert!(
            normal
                .values()
                .any(|binding| binding == &Binding::Mode(ModeId::ui_hint()))
        );
        assert_eq!(normal["q"], Binding::Mode(ModeId::idle()));
    }

    #[test]
    fn normal_long_press_toggle_threshold_is_configurable() {
        assert_eq!(Config::default().normal.long_press_toggle_ms, 500);

        let configured = Config::parse("[normal]\nlong_press_toggle_ms = 750").unwrap();
        assert_eq!(configured.normal.long_press_toggle_ms, 750);

        let disabled = Config::parse("[normal]\nlong_press_toggle_ms = 0").unwrap();
        assert_eq!(disabled.normal.long_press_toggle_ms, 0);

        let dumped = toml::to_string(&Config::default()).unwrap();
        assert!(dumped.contains("long_press_toggle_ms = 500"));

        let invalid = Config::parse("[normal]\nlong_press_toggle_ms = 60001").unwrap();
        let error = invalid.validate().unwrap_err();
        assert!(error.to_string().contains("normal.long_press_toggle_ms"));
    }

    #[test]
    fn normal_unbound_passthrough_defaults_on_and_round_trips() {
        assert!(Config::default().normal.passthrough_unbound_keys);

        let legacy = Config::parse("[normal]\nlong_press_toggle_ms = 750").unwrap();
        assert!(legacy.normal.passthrough_unbound_keys);

        let default_dumped = toml::to_string(&Config::default()).unwrap();
        assert!(default_dumped.contains("passthrough_unbound_keys = true"));

        let exclusive = Config::parse("[normal]\npassthrough_unbound_keys = false").unwrap();
        assert!(!exclusive.normal.passthrough_unbound_keys);

        let dumped = toml::to_string(&exclusive).unwrap();
        assert!(dumped.contains("passthrough_unbound_keys = false"));
        let reparsed = Config::parse(&dumped).unwrap();
        assert!(!reparsed.normal.passthrough_unbound_keys);
    }

    #[test]
    fn navigation_keys_are_bound_as_synthetic_keystrokes() {
        let normal = &Config::default().normal.bindings;
        for (chord, key) in [
            ("u", "page_down"),
            ("i", "page_up"),
            ("t", "home"),
            ("y", "end"),
        ] {
            match &normal[chord] {
                Binding::Send(sent) => assert_eq!(sent.canonical(), key),
                other => panic!("{chord} should send {key}, got {other:?}"),
            }
        }
    }

    #[test]
    fn every_mode_can_be_reached_from_the_defaults() {
        // Either directly from idle, or from normal.
        let config = Config::default();
        let reachable: Vec<&str> = config
            .hotkeys
            .values()
            .chain(config.normal.bindings.values())
            .filter_map(|b| b.mode())
            .map(|id| id.as_str())
            .collect();
        for mode in ["normal", "grid", "recursive_grid", "ui_hint"] {
            assert!(
                reachable.contains(&mode),
                "{mode} unreachable: {reachable:?}"
            );
        }
    }

    #[test]
    fn grid_modes_bind_follow_like_every_other_mode_action() {
        let config = Config::parse(
            r#"
            [grid.bindings]
            "`" = "follow"

            [recursive_grid.bindings]
            "`" = "follow"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.grid.bindings["`"],
            Binding::ToggleCursorFollowSelection
        );
        assert_eq!(
            config.recursive_grid.bindings["`"],
            Binding::ToggleCursorFollowSelection
        );
        config.validate().unwrap();
    }

    #[test]
    fn recursive_grid_defaults_are_the_qweasdzxc_nine_cell_layout() {
        let grid = &Config::default().recursive_grid;
        assert_eq!((grid.grid_cols, grid.grid_rows), (3, 3));
        assert_eq!(grid.keys, "qweasdzxc");
        assert_eq!(grid.max_depth, 10);
        assert_eq!(grid.bindings["`"], Binding::ToggleCursorFollowSelection);
    }

    #[test]
    fn bindings_need_no_action_prefix() {
        let config = Config::parse(
            r#"
            [normal.bindings]
            h = "move_left"
            g = "grid"
            t = "home"
            z = "plugin:screen-selector"
            "#,
        )
        .unwrap();
        let b = &config.normal.bindings;
        assert_eq!(b["h"], Binding::Move(Direction::Left));
        assert_eq!(b["g"], Binding::Mode(ModeId::grid()));
        assert!(matches!(b["t"], Binding::Send(_)));
        assert_eq!(
            b["z"],
            Binding::Mode(ModeId::new("plugin:screen-selector").unwrap())
        );
    }

    #[test]
    fn whitespace_separates_multiple_single_key_binding_aliases() {
        let config = Config::parse(
            r#"
            [normal.bindings]
            "v b" = "fast"
            "#,
        )
        .unwrap();
        assert_eq!(config.normal.bindings["v"], Binding::Speed(Speed::Fast));
        assert_eq!(config.normal.bindings["b"], Binding::Speed(Speed::Fast));
        assert!(!config.normal.bindings.contains_key("v b"));
        config.validate().unwrap();
    }

    #[test]
    fn grid_like_modes_do_not_steal_label_keys_with_exit_bindings() {
        let config = Config::default();
        for table in [
            &config.grid.bindings,
            &config.recursive_grid.bindings,
            &config.ui_hint.bindings,
        ] {
            assert!(!table.contains_key("q"));
            assert!(!table.contains_key("esc"));
            let exit = table
                .iter()
                .find(|(_, binding)| binding == &&Binding::Mode(ModeId::normal()))
                .map(|(chord, _)| chord)
                .expect("grid-like modes should expose a configurable exit");
            assert_eq!(
                KeyChord::parse(exit).unwrap().activation_key().as_str(),
                "q"
            );
        }
    }

    #[test]
    fn temporary_mode_keys_must_be_modifiers() {
        let config = Config::parse("[grid]\ntemporary_mode_keys = [\"h\"]").unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("temporary_mode_keys"), "{error}");
    }

    #[test]
    fn ui_hint_overlap_cycle_key_defaults_to_shift_and_is_configurable() {
        let default = Config::default();
        assert_eq!(default.ui_hint.overlap_cycle_key, "shift");
        assert!(
            default
                .ui_hint
                .overlap_cycle_matches(&Key::new("left_shift").unwrap())
        );
        assert!(
            default
                .ui_hint
                .overlap_cycle_matches(&Key::new("right_shift").unwrap())
        );

        let config = Config::parse("[ui_hint]\noverlap_cycle_key = \"option\"").unwrap();
        config.validate().unwrap();
        assert!(
            config
                .ui_hint
                .overlap_cycle_matches(&Key::new("left_alt").unwrap())
        );
        assert!(config.ui_hint.overlap_cycle_conflicts_with("alt"));
    }

    #[test]
    fn ui_hint_scan_timeout_and_retry_are_configurable() {
        let defaults = Config::default();
        assert_eq!(defaults.ui_hint.scan_timeout_ms, 2_500);
        assert_eq!(defaults.ui_hint.scan_retry_count, 1);
        assert_eq!(defaults.ui_hint.scan_retry_delay_ms, 200);

        let config = Config::parse(
            r#"
            [ui_hint]
            scan_timeout_ms = 8000
            scan_retry_count = 3
            scan_retry_delay_ms = 500
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.ui_hint.scan_timeout_ms, 8_000);
        assert_eq!(config.ui_hint.scan_retry_count, 3);
        assert_eq!(config.ui_hint.scan_retry_delay_ms, 500);
    }

    #[test]
    fn ui_hint_overlap_cycle_key_must_be_a_modifier() {
        let config = Config::parse("[ui_hint]\noverlap_cycle_key = \"b\"").unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("overlap_cycle_key"), "{error}");
    }

    #[test]
    fn inheritance_rejects_unknown_sources_and_cycles() {
        let unknown = Config::parse("[grid]\ninherits = [\"missing\"]").unwrap();
        assert!(
            unknown
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unknown source")
        );

        let cycle =
            Config::parse("[normal]\ninherits = [\"grid\"]\n[grid]\ninherits = [\"normal\"]")
                .unwrap();
        assert!(cycle.validate().unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn scroll_bindings_replace_the_old_scroll_mode() {
        let config = Config::parse(
            r#"
            [normal.bindings]
            e = "scroll_down"
            "alt+e" = "scroll_half_down"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.normal.bindings["e"],
            Binding::Scroll(Direction::Down, ScrollAmount::Step)
        );
        config.validate().unwrap();
    }

    #[test]
    fn scroll_is_rejected_as_a_mode_target() {
        // It used to be a mode; make the migration explicit rather than silent.
        let err = Config::parse(
            r#"
            [hotkeys]
            "alt+e" = "normal"
            "alt+s" = "scroll"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown binding"), "{err}");
        assert!(err.contains("scroll"), "{err}");
    }

    #[test]
    fn a_config_that_cannot_reach_normal_is_valid_but_diagnosable() {
        let config = Config::parse(
            r#"
            [hotkeys]
            "alt+g" = "grid"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn plugin_modes_get_a_binding_table_like_built_ins() {
        let config = Config::parse(
            r#"
            [plugin_modes."plugin:screen-selector".bindings]
            "1" = "left_click"
            esc = "escape"
            "#,
        )
        .unwrap();
        let table = config.bindings_for("plugin:screen-selector").unwrap();
        assert_eq!(table["1"], Binding::Click(Button::Left));
        config.validate().unwrap();
    }

    #[test]
    fn bindings_for_covers_every_built_in_mode() {
        let config = Config::default();
        for mode in ["idle", "normal", "grid", "recursive_grid", "ui_hint"] {
            assert!(
                config.bindings_for(mode).is_some(),
                "{mode} has no binding table"
            );
        }
    }

    #[test]
    fn parses_neru_style_theme_and_ui_sections() {
        let config = Config::parse(
            r##"
            [theme.dark]
            surface       = "#0A1338FF"
            accent        = "#6E82D6FF"
            accent_alt    = "#8FA2F0FF"
            on_accent_alt = "#081022FF"
            text          = "#E8EEFFFF"

            [ui_hint]
            placement = "top"
            label_x_offset = 3
            label_y_offset = -8

            [ui_hint.ui]
            font_size = 14
            background_color = { light = "#FFFFFFFF", dark = "#000000FF" }
            text_color = "#E8EEFFFF"
            matched_text_color = "#E4B400FF"

            [recursive_grid.ui]
            label_min_font_size = 5
            sub_key_preview = true
            label_char = "\u00B7"
            "##,
        )
        .unwrap();
        assert_eq!(config.ui_hint.placement, HintPlacement::Top);
        assert_eq!(config.ui_hint.label_x_offset, 3);
        assert_eq!(config.ui_hint.label_y_offset, -8);
        assert_eq!(config.ui_hint.ui.font_size, 14);
        assert!(config.ui_hint.ui.text_color.is_some());
        assert!(config.ui_hint.ui.matched_text_color.is_some());
        assert_eq!(config.recursive_grid.ui.label_min_font_size, 5);
        assert!(config.recursive_grid.ui.sub_key_preview);
        assert_eq!(config.recursive_grid.ui.label_char, "\u{B7}");
        config.validate().unwrap();
    }

    #[test]
    fn rejects_non_rgba_component_colors() {
        let config = Config::parse(
            r##"
            [ui_hint.ui]
            text_color = "#112233"
            "##,
        )
        .unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("ui_hint.ui.text_color"), "{error}");
        assert!(error.contains("#RRGGBBAA"), "{error}");
    }

    #[test]
    fn recursive_grid_key_count_must_match_the_grid() {
        let config = Config::parse(
            r#"
            [recursive_grid]
            grid_cols = 2
            grid_rows = 2
            keys = "abc"
            "#,
        )
        .unwrap();
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("needs 4"), "{err}");
    }

    #[test]
    fn layers_override_the_parent_shape() {
        let config = Config::parse(
            r#"
            [recursive_grid]
            layers = [
              { depth = 0, grid_cols = 2, grid_rows = 2, keys = "crtn" },
            ]
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.recursive_grid.layers[0].grid_cols, Some(2));
    }

    #[test]
    fn rejects_unparseable_chords() {
        let config = Config::parse(
            r#"
            [normal.bindings]
            "ctrl+shift" = "grid"
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn a_binding_typo_is_rejected_at_parse_time() {
        // `gird` must not be silently sent as four keystrokes.
        let err = Config::parse("[normal.bindings]\ng = \"gird\"").unwrap_err();
        assert!(err.to_string().contains("unknown binding"), "{err}");
    }

    #[test]
    fn none_disables_an_inherited_binding() {
        let config = Config::parse("[normal.bindings]\nh = \"none\"").unwrap();
        assert_eq!(config.normal.bindings["h"], Binding::Disabled);
    }

    #[test]
    fn targeting_lifecycle_accepts_actions_and_known_modes() {
        let config = Config::parse(
            r#"
            [grid.lifecycle]
            after_finish = "left_click"
            after_click = "recursive_grid"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.grid.lifecycle.after_finish,
            LifecycleAction::Click {
                button: MouseButton::Left,
                action: ButtonAction::Click,
            }
        );
        assert_eq!(
            config.grid.lifecycle.after_click,
            LifecycleAction::Mode(ModeId::recursive_grid())
        );
    }

    #[test]
    fn targeting_lifecycle_rejects_unknown_modes_and_recursive_clicks() {
        let config = Config::parse(
            r#"
            [ui_hint.lifecycle]
            after_click = "does_not_exist"
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());

        let config = Config::parse("[grid.lifecycle]\nafter_click = \"left_click\"").unwrap();
        assert!(config.validate().is_err());

        let config = Config::parse("[grid.lifecycle]\nafter_finish = \"finish\"").unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn obsolete_after_click_mode_is_rejected() {
        assert!(Config::parse("[grid]\nafter_click_mode = \"normal\"").is_err());
    }

    #[test]
    fn targeting_lifecycle_defaults_match_the_shipped_experience() {
        let config = Config::default();
        assert_eq!(
            config.ui_hint.lifecycle,
            TargetingLifecycle {
                after_finish: LifecycleAction::Mode(ModeId::normal()),
                after_click: LifecycleAction::Mode(ModeId::normal()),
            }
        );
        assert_eq!(
            config.grid.lifecycle,
            TargetingLifecycle {
                after_finish: LifecycleAction::Mode(ModeId::normal()),
                after_click: LifecycleAction::Finish,
            }
        );
        assert_eq!(
            config.recursive_grid.lifecycle,
            TargetingLifecycle {
                after_finish: LifecycleAction::Keep,
                after_click: LifecycleAction::Keep,
            }
        );
    }

    #[test]
    fn targeting_lifecycle_can_switch_to_a_configured_plugin_mode() {
        let config = Config::parse(
            r#"
            [plugin_modes."example:picker"]

            [grid.lifecycle]
            after_click = "example:picker"
            "#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn typos_in_field_names_are_rejected_rather_than_ignored() {
        // deny_unknown_fields turns a silent no-op into a visible error.
        assert!(Config::parse("[grid]\ncharacterz = \"abc\"").is_err());
        // The old mode-scoped `hotkeys` name is gone; catch stale configs.
        assert!(Config::parse("[grid]\nhotkeys = {}").is_err());
    }

    #[test]
    fn per_mode_indicator_entries_parse_alongside_the_shared_ui() {
        // Regression: `flatten` plus `deny_unknown_fields` rejected `ui`.
        let config = Config::parse(
            r#"
            [mode_indicator.ui]
            font_size = 13

            [mode_indicator.modes.normal]
            enabled = true
            text = "Normal"
            "#,
        )
        .unwrap();
        assert_eq!(config.mode_indicator.ui.label.font_size, 13);
        let (text, _) = config
            .mode_indicator
            .for_mode("normal", "Normal")
            .expect("normal should have a badge");
        assert_eq!(text, "Normal");
        // Unlisted active modes stay visible, while idle remains silent.
        assert!(config.mode_indicator.for_mode("grid", "Grid").is_some());
        assert!(config.mode_indicator.for_mode("idle", "Idle").is_none());
    }

    #[test]
    fn scan_strategy_defaults_to_vision_and_allows_per_app_hybrid() {
        let vision = VisionOptions::default();
        assert!(vision.detect_text && vision.detect_rectangles);
        assert_eq!(vision.request_timeout_ms, 5_000);
        assert_eq!(vision.minimum_confidence, 0.0);
        assert_eq!(vision.merge_iou_threshold, 0.5);
        assert_eq!(vision.rectangle_max_candidates, 100);
        assert_eq!(vision.rectangle_min_size, 0.01);
        assert_eq!(vision.button_icon_max_size, 48.0);
        assert_eq!(vision.checkbox_max_size, 32.0);
        assert_eq!(vision.generic_clickable_min_confidence, 0.5);

        assert_eq!(Config::default().ui_hint.strategy, UiScanStrategy::Vision);

        let config = Config::parse(
            r#"
            [ui_hint]
            strategy = "axtree"

            [[ui_hint.app_configs]]
            bundle_id = "com.example.editor"
            strategy = "hybrid"
            "#,
        )
        .unwrap();
        let app = FocusedApp {
            bundle_id: "com.example.editor".into(),
            window_title: String::new(),
            process_id: 7,
        };
        assert_eq!(
            config.ui_hint.strategy_for(Some(&app)),
            UiScanStrategy::Hybrid
        );
        assert_eq!(config.ui_hint.strategy_for(None), UiScanStrategy::AxTree);
    }

    #[test]
    fn mode_indicator_merges_per_mode_cursor_and_badge_styles() {
        let config = Config::parse(
            r##"
            [mode_indicator.cursor]
            radius = 12
            stroke_width = 1

            [mode_indicator.ui]
            font_size = 11
            background_color = "#112233FF"

            [mode_indicator.modes.normal]
            text = "Temp Normal"

            [mode_indicator.modes.normal.cursor]
            radius = 18
            fill_color = "#44556677"

            [mode_indicator.modes.normal.ui]
            font_size = 14
            text_color = "#FFFFFFFF"
            "##,
        )
        .unwrap();
        let cursor = config
            .mode_indicator
            .cursor_for_mode("normal")
            .expect("cursor");
        assert_eq!(cursor.radius, 18);
        assert_eq!(cursor.stroke_width, 1);
        assert!(cursor.fill_color.is_some());
        let (text, ui) = config
            .mode_indicator
            .for_mode("normal", "Normal")
            .expect("badge");
        assert_eq!(text, "Temp Normal");
        assert_eq!(ui.label.font_size, 14);
        assert!(ui.label.background_color.is_some());
        assert!(ui.label.text_color.is_some());
    }

    #[test]
    fn cursor_pressed_colors_are_configurable_and_inherit_into_modes() {
        let config = Config::parse(
            r##"
            [mode_indicator.cursor]
            left_pressed_color = "#11AA22FF"
            middle_pressed_color = "#BB33CCFF"
            right_pressed_color = "#44DDEEFF"

            [mode_indicator.modes.normal.cursor]
            left_pressed_color = "#123456FF"
            "##,
        )
        .unwrap();
        config.validate().unwrap();

        let cursor = config
            .mode_indicator
            .cursor_for_mode("normal")
            .expect("normal cursor");
        assert_eq!(
            cursor.left_pressed_color,
            Some(ThemedColor::Both("#123456FF".into()))
        );
        assert_eq!(
            cursor.middle_pressed_color,
            Some(ThemedColor::Both("#BB33CCFF".into()))
        );
        assert_eq!(
            cursor.right_pressed_color,
            Some(ThemedColor::Both("#44DDEEFF".into()))
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config::default();
        let reparsed = Config::parse(&config.to_toml()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn scroll_amounts_resolve_to_configured_pixels() {
        let scroll = Scroll::default();
        assert_eq!(scroll.pixels(ScrollAmount::Step), 50.0);
        assert_eq!(scroll.pixels(ScrollAmount::Half), 500.0);
        assert_eq!(scroll.pixels(ScrollAmount::Full), 1_000_000.0);
    }

    fn warn(chord: &str) -> Option<String> {
        platform_warning(&KeyChord::parse(chord).unwrap())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_option_letter_chords_are_flagged() {
        // The exact bug the user hit: alt+e silently never fires.
        let warning = warn("alt+e").expect("alt+e should be flagged on macOS");
        assert!(warning.contains("dead-key"), "{warning}");
        assert!(warning.contains("primary+shift"), "should suggest a fix");

        // Adding Cmd or Ctrl removes the text-composition behaviour.
        assert_eq!(warn("primary+shift+e"), None);
        assert_eq!(warn("ctrl+alt+e"), None);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_system_reserved_chords_are_flagged() {
        for chord in ["win+space", "win+tab"] {
            assert!(warn(chord).is_some(), "{chord} should be flagged");
        }
        // Cmd+Q is intentionally usable as grid/hint exit, while Shift also
        // disambiguates the other system shortcuts.
        assert_eq!(warn("win+q"), None);
        assert_eq!(warn("win+shift+space"), None);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_rejects_function_keys_it_does_not_have() {
        assert!(warn("f21").is_some(), "F21 does not exist on macOS");
        assert_eq!(warn("f20"), None);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn terminal_clipboard_chords_are_flagged() {
        for chord in ["ctrl+shift+c", "ctrl+shift+v"] {
            let warning = warn(chord).expect("{chord} should be flagged");
            assert!(warning.contains("clipboard"), "{warning}");
        }
        // Our own default is deliberately not one of them.
        assert_eq!(warn("ctrl+shift+e"), None);
        assert_eq!(warn("ctrl+shift+g"), None);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn option_letter_chords_are_fine_off_macos() {
        // Only macOS composes text from Option+letter.
        assert_eq!(warn("alt+e"), None);
    }

    #[test]
    fn a_flagged_chord_warns_but_still_validates() {
        // The user may know their layout better than we do, so this must not
        // be a hard error.
        let config = Config::parse(
            r#"
            [hotkeys]
            "primary+shift+e" = "normal"

            [normal.bindings]
            "alt+e" = "move_left"
            "#,
        )
        .unwrap();
        config
            .validate()
            .expect("a warning must not fail validation");

        // But it must be reported, and exactly once.
        let warnings = config.platform_warnings();
        if cfg!(target_os = "macos") {
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("alt+e"), "{warnings:?}");
        } else {
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }

    #[test]
    fn the_default_config_produces_no_platform_warnings() {
        // Nothing we ship may warn on the platform it runs on.
        let warnings = Config::default().platform_warnings();
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
