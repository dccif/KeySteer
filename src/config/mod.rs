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

mod aliases;
mod settings;
pub mod store;
pub mod theme;
mod validation;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::backend::Appearance;
use crate::api::binding::Binding;
use crate::api::command::{FocusedApp, UiScanStrategy};
use crate::api::input::{
    Key, KeyChord, ModeId, normalize_alias_name, normalize_builtin_key, with_key_aliases,
};

pub use crate::api::hint::LabelDirection;
pub use crate::api::lifecycle::{LifecycleAction, TargetingLifecycle};
pub use crate::api::style::{
    Anchor, BoundaryHighlight, CursorIndicatorOverride, CursorIndicatorUi, HintPlacement,
    IndicatorUi, IndicatorUiOverride, LabelUi, ModeIndicator, ModeIndicatorEntry, SearchInputUi,
};
pub use aliases::KeyAliases;
use aliases::{
    compile_key_aliases, normalize_binding_keys, normalize_key_if_aliased, normalize_key_list,
    platform_warning,
};
pub use settings::{
    Grid, GridLayer, GridUi, Normal, Pointer, RecursiveGrid, RecursiveGridUi, Scroll, UiHint,
};
pub use store::{ConfigStore, ReplaceFile};
pub use theme::{Palette, Theme, ThemeColors, ThemedColor};

/// A binding table: chord text -> what it does.
pub type Bindings = BTreeMap<String, Binding>;

/// Root configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
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
    #[serde(default)]
    pub key_help: crate::api::style::KeyHelp,
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

/// Compatibility name for internal and test callers while the application
/// migrates to the document-oriented terminology.
pub type Config = ConfigFile;

pub(crate) struct LoadedConfig {
    pub(crate) config: Config,
    pub(crate) raw_text: String,
    pub(crate) path: PathBuf,
}

impl Default for ConfigFile {
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
            key_help: Default::default(),
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
    builtin_bindings(entries)
}

fn builtin_bindings(entries: &[(&str, &str)]) -> Bindings {
    entries
        .iter()
        .map(|&(chord, binding)| {
            let parsed = match Binding::parse(binding) {
                Ok(parsed) => parsed,
                Err(error) => {
                    panic!("invalid built-in binding `{binding}` for chord `{chord}`: {error}")
                }
            };
            (chord.to_owned(), parsed)
        })
        .collect()
}

impl ConfigFile {
    pub fn plugin_setting_bool(&self, mode_id: &str, key: &str) -> Option<bool> {
        self.plugin_modes.get(mode_id)?.settings.get(key)?.as_bool()
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
    app.matches_pattern(pattern)
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiHintAppOverride {
    pub bundle_id: String,
    pub strategy: Option<UiScanStrategy>,
    pub bindings: Bindings,
}

// ---------------------------------------------------------------------------
// Loading & validation
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    Invalid(String),
    Serialize(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(s) => write!(f, "{s}"),
            ConfigError::Parse(s) => write!(f, "invalid TOML: {s}"),
            ConfigError::Invalid(s) => write!(f, "invalid configuration: {s}"),
            ConfigError::Serialize(s) => write!(f, "cannot serialize configuration: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl ConfigFile {
    pub(crate) fn discover_in(directory: &Path) -> Result<Option<PathBuf>, ConfigError> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ConfigError::Io(format!(
                    "cannot enumerate {}: {error}",
                    directory.display()
                )));
            }
        };
        let mut matches = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                ConfigError::Io(format!("cannot enumerate {}: {error}", directory.display()))
            })?;
            let file_type = entry.file_type().map_err(|error| {
                ConfigError::Io(format!(
                    "cannot inspect {}: {error}",
                    entry.path().display()
                ))
            })?;
            if file_type.is_file() && Self::is_portable_config_name(&entry.file_name()) {
                matches.push(entry.path());
            }
        }
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
        Ok(matches.into_iter().next())
    }

    pub(crate) fn is_portable_config_name(name: &std::ffi::OsStr) -> bool {
        let name = name.to_string_lossy().to_ascii_lowercase();
        name.strip_prefix("keysteer.")
            .and_then(|rest| rest.strip_suffix(".toml"))
            .is_some_and(|profile| !profile.is_empty())
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

    pub fn to_toml(&self) -> Result<String, ConfigError> {
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
        toml::to_string_pretty(&exported).map_err(|error| ConfigError::Serialize(error.to_string()))
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

    /// Resolve the palette for the current system appearance.
    pub fn palette(&self, appearance: Appearance) -> Palette {
        self.theme.palette(appearance)
    }
}

#[cfg(test)]
mod tests;
