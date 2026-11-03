#![forbid(unsafe_code)]

//! Configuration.
//!
//! The section layout but bindings are our own:
//! a value is a [`Binding`] from the public API, so `h = "move_left"` and
//! `g = "grid"` read the same whether the target is a built-in verb, a built-in
//! mode or a plugin mode. There is no `action` prefix and no separate internal
//! vocabulary.
//!
//! Nothing is required: every section and field has a default, so an absent or
//! partial config file is valid.

mod aliases;
mod compatibility;
mod macros;
mod settings;
mod window;
pub use window::{
    Window, WindowEditor, WindowModeConfig, WindowQuick, WindowRestore, WindowScreens, WindowTab,
};
pub mod store;
pub mod theme;
mod validation;

use macros::{builtin_mode_fields, config_struct, mode_config};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::backend::Appearance;
use crate::api::binding::Binding;
use crate::api::command::{FocusedApp, UiScanStrategy};
use crate::api::input::{
    Key, KeyChord, ModeId, normalize_alias_name, normalize_builtin_key, with_key_aliases,
};

pub use crate::api::lifecycle::LifecycleAction;
#[cfg(test)]
pub use crate::api::style::HintPlacement;
pub use crate::api::style::{LabelUi, ModeIndicator};
#[cfg(test)]
pub use crate::api::{LabelDirection, TargetingLifecycle};
pub use aliases::KeyAliases;
use aliases::{
    compile_key_aliases, normalize_binding_keys, normalize_key_if_aliased, normalize_key_list,
    platform_warning,
};
pub(crate) use settings::GridLayer;
pub use settings::{
    Bisect, Grid, Normal, Pointer, RecursiveGrid, Scroll, TargetingMethod, TargetingReset,
    TextInput, TextSelect, UiHint,
};
pub use store::{ConfigStore, ReplaceFile};

pub use theme::{Palette, Theme, ThemedColor};

/// A binding table: chord text -> what it does.
pub type Bindings = BTreeMap<String, Binding>;

/// Borrowed routing fields shared by mode assembly and validation. The document
/// keeps its flat fields; this view neither clones them nor changes serialization.
#[derive(Default)]
pub(crate) struct Routing<'a> {
    pub inherits: &'a [String],
    pub temporary_mode: Option<&'a str>,
    pub temporary_mode_keys: &'a [String],
    pub temporary_mode_passthrough_keys: &'a [String],
}

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
    pub text_input: TextInput,
    #[serde(default)]
    pub text_select: TextSelect,
    #[serde(default)]
    pub window: Window,
    #[serde(default)]
    pub window_quick: WindowQuick,
    #[serde(default)]
    pub window_editor: WindowEditor,
    #[serde(default)]
    pub window_restore: WindowRestore,
    #[serde(default)]
    pub window_tab: WindowTab,
    #[serde(default)]
    pub ui_hint: UiHint,
    #[serde(default)]
    pub grid: Grid,
    #[serde(default)]
    pub recursive_grid: RecursiveGrid,
    #[serde(default)]
    pub bisect: Bisect,
    #[serde(default)]
    pub scroll: Scroll,
    #[serde(default)]
    pub pointer: Pointer,
    #[serde(default)]
    pub mode_indicator: ModeIndicator,
    #[serde(default)]
    pub key_help: crate::api::style::KeyHelp,
    #[serde(default)]
    pub mode_usage: ModeUsage,
    #[serde(default)]
    pub quick_switch: QuickSwitch,
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
    pub(crate) warnings: Vec<String>,
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
            text_input: TextInput::default(),
            text_select: TextSelect::default(),
            window: Window::default(),
            window_quick: WindowQuick::default(),
            window_editor: WindowEditor::default(),
            window_restore: WindowRestore::default(),
            window_tab: WindowTab::default(),
            ui_hint: UiHint::default(),
            grid: Grid::default(),
            recursive_grid: RecursiveGrid::default(),
            bisect: Bisect::default(),
            scroll: Scroll::default(),
            pointer: Pointer::default(),
            mode_indicator: ModeIndicator::default(),
            key_help: Default::default(),
            mode_usage: Default::default(),
            quick_switch: Default::default(),
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

config_struct! {
    /// Usage is checkpointed by events, never by a periodic timer.
    pub struct ModeUsage {
        save_after_entries: u32 = 100,
    }
}
mode_config! {
    pub struct QuickSwitch {
        enabled: bool = true,
        ..routing {
            omit_empty: all,
            inherits: Vec::new(),
            temporary_mode: None,
        }
        key: String = "q".into(),
        hint_characters: String = "wertyuiop".into(),
        bindings: Bindings = builtin_bindings(&[
            ("esc", "cancel_selection"),
            ("q", "cancel_selection"),
            ("backspace", "previous_level"),
        ]),
        hold_ms: u64 = 350,
        blacklist: Vec<String> = vec!["idle".into()],
        position: crate::api::style::QuickSwitchPosition,
        ui: LabelUi = LabelUi {
            font_size: 28,
            padding_x: 10,
            ..Default::default()
        },
    }
}

fn default_plugin_modes() -> BTreeMap<String, PluginModeConfig> {
    BTreeMap::from([(
        "plugin:screen-selector".into(),
        PluginModeConfig {
            settings: BTreeMap::from([("preserve".into(), toml::Value::Boolean(true))]),
            bindings: screen_selector_control_defaults(),
            ..Default::default()
        },
    )])
}

fn screen_selector_control_defaults() -> Bindings {
    builtin_bindings(&[
        ("esc", "cancel_selection"),
        ("enter", "confirm_selection"),
        ("backspace", "previous_level"),
    ])
}

/// Each provider owns the controls also used to construct its full defaults.
pub(crate) fn apply_control_defaults(
    mode: &str,
    configured: &mut Bindings,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let controls = match mode {
        "grid" | "recursive_grid" => settings::grid_control_defaults(),
        "bisect" => settings::bisect_control_defaults(),
        "ui_hint" => settings::ui_hint_control_defaults(),
        "window_editor" => window::editor_control_defaults(),
        "window_restore" => window::restore_control_defaults(),
        "window.multi_select" => window::multi_select_control_defaults(),
        "plugin:screen-selector" => screen_selector_control_defaults(),
        _ => return Ok(()),
    };
    merge_control_defaults(mode, configured, controls, aliases)
}

/// Fill unoccupied controls; explicit bindings, including `none`, win.
/// The mode name is only an error label; aliases are already fully validated.
fn merge_control_defaults(
    label: &str,
    configured: &mut Bindings,
    mut controls: Bindings,
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    normalize_binding_keys(&mut controls, label, aliases)?;
    normalize_binding_keys(configured, label, aliases)?;
    if controls.keys().all(|key| configured.contains_key(key)) {
        // Normalized defaults already have valid keys; exact matches occupy them.
        return Ok(());
    }
    let occupied: BTreeSet<_> = configured
        .keys()
        .filter_map(|key| {
            KeyChord::parse_with_aliases(key, aliases)
                .ok()
                .map(|key| key.canonical())
        })
        .collect();
    for (key, binding) in controls {
        if !occupied.contains(
            &KeyChord::parse_with_aliases(&key, aliases)
                .map_err(ConfigError::Parse)?
                .canonical(),
        ) {
            configured.insert(key, binding);
        }
    }
    Ok(())
}

/// Defaults for `idle`: only what is needed to wake the program up.
///
/// `Primary` is Cmd on macOS and Ctrl elsewhere, so one file works on both.
///
/// `Primary+Shift` rather than a bare modifier matters on macOS, where
/// `Option+<letter>` types a special character (`Option+E` is a dead-key acute
/// accent), and on Linux, where `Ctrl+Shift+C/V` are taken by terminals. The
/// letters here avoid both.
fn default_idle_bindings() -> Bindings {
    builtin_bindings(&[
        // Idle remains silent until an explicitly configured launcher is pressed.
        ("primary+e", "normal"),
        ("alt+w", "window"),
    ])
}

fn builtin_bindings(entries: &[(&str, &str)]) -> Bindings {
    entries
        .iter()
        .map(|&(chord, binding)| {
            let parsed = Binding::parse(binding).unwrap_or_else(|error| {
                panic!("invalid built-in binding `{binding}` for chord `{chord}`: {error}")
            });
            (chord.to_owned(), parsed)
        })
        .collect()
}

impl ConfigFile {
    pub fn plugin_setting_bool(&self, mode_id: &str, key: &str) -> Option<bool> {
        self.plugin_modes.get(mode_id)?.settings.get(key)?.as_bool()
    }
}

mode_config! {
    pub struct PluginModeConfig {
        ..routing {
            inherits: Vec::new(),
            temporary_mode: None,
        }
        /// Plugin-owned values exposed through the host settings snapshot.
        settings: BTreeMap<String, toml::Value>,
        bindings: Bindings,
        app_configs: Vec<AppOverride>,
    }
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

config_struct! {
    pub struct MacOsScrollConfig {
        // A missing table uses these defaults; a missing field inside a present
        // table stays None so the platform can apply its fallback.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invert_horizontal: Option<bool> = Some(false),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invert_vertical: Option<bool> = Some(true),
    }
}

config_struct! {
    /// Verbose runtime tracing written to stderr for diagnosing native input.
    #[derive(Eq)]
    pub struct DebugConfig {
        enabled: bool,
        keys: bool = true,
        actions: bool = true,
        modes: bool = true,
        backend: bool = true,
        /// Pointer coordinates are extremely high-volume; opt in separately.
        pointer: bool,
        /// High-volume OS key-repeat, movement and cursor-overlay details.
        motion: bool,
        overlay: bool = true,
        timers: bool = true,
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
            if !Self::is_portable_config_name(&entry.file_name()) {
                continue;
            }
            matches.push(entry.path());
        }
        matches.sort_by_cached_key(|path| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            (name == "keysteer.default.toml", name, path.clone())
        });
        // Follow links only in selection order. An unused lower-priority link
        // cannot prevent a valid profile from loading; a selected broken link
        // is diagnosed instead of silently disappearing from discovery.
        for path in matches {
            let metadata = std::fs::metadata(&path).map_err(|error| {
                ConfigError::Io(format!("cannot inspect config {}: {error}", path.display()))
            })?;
            if metadata.is_file() {
                return Ok(Some(path));
            }
        }
        Ok(None)
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
        let (config, warnings) = Self::parse_with_warnings(&text)?;
        config.validate()?;
        Ok(LoadedConfig {
            config,
            raw_text: text,
            path: path.to_path_buf(),
            warnings,
        })
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        Self::parse_with_warnings(text).map(|(config, _)| config)
    }

    pub(crate) fn parse_with_warnings(text: &str) -> Result<(Self, Vec<String>), ConfigError> {
        let mut value: toml::Value =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let warnings = compatibility::normalize(&mut value)?;
        let missing_text_select = value.get("text_select").is_none();
        let missing_text_input = value.get("text_input").is_none();
        let key_aliases = value
            .get("key_aliases")
            .cloned()
            .map(KeyAliases::deserialize)
            .transpose()
            .map_err(|e| ConfigError::Parse(e.to_string()))?
            .unwrap_or_default();
        let aliases = compile_key_aliases(&key_aliases.effective()?)?;
        // New region shortcuts must not replace existing labels or explicitly
        // inherited user actions. Explicit local bindings still win.
        let declares = |table: Option<&toml::Value>, shortcut: &str| {
            table.and_then(toml::Value::as_table).is_some_and(|table| {
                table
                    .keys()
                    .flat_map(|key| key.split_whitespace())
                    .any(|key| {
                        KeyChord::parse_with_aliases(key, &aliases)
                            .is_ok_and(|chord| chord.canonical() == shortcut)
                    })
            })
        };
        let override_declares = |overrides: Option<&toml::Value>, shortcut: &str| {
            overrides
                .and_then(toml::Value::as_array)
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| declares(entry.get("bindings"), shortcut))
                })
        };
        let shortcuts = [
            ("u", crate::api::binding::RegionTarget::UiHint),
            ("i", crate::api::binding::RegionTarget::TextSelect),
        ]
        .map(|(shortcut, target)| {
            let shortcut = aliases
                .get(shortcut)
                .cloned()
                .unwrap_or_else(|| shortcut.to_owned());
            let explicit = ["grid", "recursive_grid", "bisect"].map(|mode| {
                declares(
                    value.get(mode).and_then(|section| section.get("bindings")),
                    &shortcut,
                )
            });
            let normal = value.get("normal");
            let normal_uses = declares(
                normal.and_then(|section| section.get("bindings")),
                &shortcut,
            ) || override_declares(
                normal.and_then(|section| section.get("app_configs")),
                &shortcut,
            );
            let hotkeys_use = declares(value.get("hotkeys"), &shortcut)
                || override_declares(value.get("app_configs"), &shortcut);
            (shortcut, target, explicit, normal_uses, hotkeys_use)
        });
        let mut config: Self = with_key_aliases(&aliases, || Self::deserialize(value))
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.resolved_key_aliases = aliases;
        config.apply_configured_key_aliases()?;
        fn enters(binding: &Binding, mode: &ModeId) -> bool {
            binding.mode() == Some(mode)
                || matches!(binding, Binding::Sequence(items) if items.iter().any(|b| enters(b, mode)))
        }
        // A pre-feature Normal table replaces its defaults wholesale. Restore
        // only missing feature entrances, without overriding a custom/disabled
        // key or adding a second shortcut to an already remapped mode.
        for (missing, key, mode) in [
            (missing_text_select, "c", ModeId::text_select()),
            (missing_text_input, "\\", ModeId::text_input()),
        ] {
            let reserved = |table: &Bindings| {
                table.iter().any(|(configured, binding)| {
                    enters(binding, &mode)
                        || KeyChord::parse_with_aliases(configured, &config.resolved_key_aliases)
                            .ok()
                            .is_some_and(|chord| {
                                chord.keys().iter().any(|part| part.as_str() == key)
                            })
                })
            };
            if missing
                && !reserved(&config.normal.bindings)
                && !(config
                    .normal
                    .inherits
                    .iter()
                    .any(|name| name == "hotkeys" || name == "idle")
                    && reserved(&config.hotkeys))
                && !config.normal.targeting.as_ref().is_some_and(|targeting| {
                    let geometry = targeting.resolve(&config);
                    geometry.keys.contains(key)
                        || geometry.layers.iter().any(|layer| {
                            layer.keys.as_deref().is_some_and(|keys| keys.contains(key))
                        })
                })
            {
                config
                    .normal
                    .bindings
                    .insert(key.into(), Binding::Mode(mode));
            }
        }
        for (name, table) in [
            ("grid", &mut config.grid.bindings),
            ("bisect", &mut config.bisect.bindings),
            ("recursive_grid", &mut config.recursive_grid.bindings),
            ("ui_hint", &mut config.ui_hint.bindings),
            ("window_editor", &mut config.window_editor.common.bindings),
            ("window_restore", &mut config.window_restore.common.bindings),
            (
                "window.multi_select",
                &mut config.window.multi_select.bindings,
            ),
        ] {
            apply_control_defaults(name, table, &config.resolved_key_aliases)?;
        }
        for (name, plugin) in &mut config.plugin_modes {
            apply_control_defaults(name, &mut plugin.bindings, &config.resolved_key_aliases)?;
        }

        for (key, target, explicit_keys, normal_uses, hotkeys_use) in shortcuts {
            let inherited = |inherits: &[String]| {
                inherits.iter().any(|mode| match mode.as_str() {
                    "normal" => normal_uses,
                    "hotkeys" | "idle" => hotkeys_use,
                    _ => false,
                })
            };
            let shortcut_char = key.chars().next().filter(|_| key.len() == 1);
            let has_key = |keys: &str| {
                shortcut_char.is_some_and(|shortcut| {
                    keys.chars().any(|ch| ch.eq_ignore_ascii_case(&shortcut))
                })
            };
            let reserved = [
                has_key(&config.grid.keys) || inherited(&config.grid.inherits),
                has_key(&config.recursive_grid.keys)
                    || config
                        .recursive_grid
                        .layers
                        .iter()
                        .any(|layer| layer.keys.as_deref().is_some_and(has_key))
                    || inherited(&config.recursive_grid.inherits),
                inherited(&config.bisect.inherits),
            ];
            for ((explicit, reserved), bindings) in explicit_keys.into_iter().zip(reserved).zip([
                &mut config.grid.bindings,
                &mut config.recursive_grid.bindings,
                &mut config.bisect.bindings,
            ]) {
                let binding = Binding::ScanRegion(target);
                if !explicit {
                    if reserved && bindings.get(key.as_str()) == Some(&binding) {
                        bindings.remove(key.as_str());
                    } else if !reserved {
                        bindings.entry(key.clone()).or_insert(binding);
                    }
                }
            }
        }

        Ok((config, warnings))
    }

    /// Expand whitespace-separated key or chord alternatives in a binding key.
    ///
    /// For example, `"v b" = "fast"` is stored as the two ordinary bindings
    /// `v = "fast"` and `b = "fast"`. `"mod+c enter"` similarly binds
    /// both the chord and Enter; `+` combines keys within one alternative.
    fn apply_configured_key_aliases(&mut self) -> Result<(), ConfigError> {
        let aliases = self.resolved_key_aliases.clone();
        normalize_binding_keys(&mut self.hotkeys, "[hotkeys]", &aliases)?;
        normalize_binding_keys(
            &mut self.window.multi_select.bindings,
            "[window.multi_select.bindings]",
            &aliases,
        )?;
        // These tables historically report errors before window overrides.
        for (label, table) in [
            ("[normal.bindings]", &mut self.normal.bindings),
            ("[quick_switch.bindings]", &mut self.quick_switch.bindings),
            ("[text_select.bindings]", &mut self.text_select.bindings),
            ("[text_input.bindings]", &mut self.text_input.bindings),
        ] {
            normalize_binding_keys(table, label, &aliases)?;
        }
        for (name, mode) in self.window_modes_mut() {
            normalize_binding_keys(
                &mut mode.bindings,
                format_args!("[{name}.bindings]"),
                &aliases,
            )?;
            for over in &mut mode.app_configs {
                normalize_binding_keys(
                    &mut over.bindings,
                    format_args!("[[{name}.app_configs]]"),
                    &aliases,
                )?;
            }
            normalize_passthrough_keys(&mut mode.temporary_mode_passthrough_keys, &aliases)?;
        }
        let windows = self.window_modes().map(|(name, _)| name);
        for (name, table) in builtin_mode_fields!(self, [&mut], |mode| &mut mode.bindings)
            .into_iter()
            .filter(|(name, _)| {
                !matches!(
                    *name,
                    "normal" | "quick_switch" | "text_select" | "text_input"
                ) && !windows.contains(name)
            })
        {
            normalize_binding_keys(table, format_args!("[{name}.bindings]"), &aliases)?;
        }
        for (id, mode) in &mut self.plugin_modes {
            normalize_binding_keys(
                &mut mode.bindings,
                format_args!("[plugin_modes.{id:?}.bindings]"),
                &aliases,
            )?;
            for over in &mut mode.app_configs {
                normalize_binding_keys(
                    &mut over.bindings,
                    format_args!("[[plugin_modes.{id:?}.app_configs]] {:?}", over.bundle_id),
                    &aliases,
                )?;
            }
        }
        for (label, overrides) in [
            ("[[app_configs]]", &mut self.app_configs),
            ("[[normal.app_configs]]", &mut self.normal.app_configs),
            ("[[grid.app_configs]]", &mut self.grid.app_configs),
            ("[[bisect.app_configs]]", &mut self.bisect.app_configs),
            (
                "[[recursive_grid.app_configs]]",
                &mut self.recursive_grid.app_configs,
            ),
        ] {
            for over in overrides {
                normalize_binding_keys(
                    &mut over.bindings,
                    format_args!("{label} {:?}", over.bundle_id),
                    &aliases,
                )?;
            }
        }
        for over in &mut self.ui_hint.app_configs {
            normalize_binding_keys(
                &mut over.bindings,
                format_args!("[[ui_hint.app_configs]] {:?}", over.bundle_id),
                &aliases,
            )?;
        }
        normalize_key_if_aliased(&mut self.ui_hint.overlap_cycle_key, &aliases)?;
        normalize_key_list(&mut self.ui_hint.search_copy_keys, &aliases)?;
        normalize_binding_keys(
            &mut self.ui_hint.search_bindings,
            "[ui_hint.search_bindings]",
            &aliases,
        )?;
        for alternatives in self.ui_hint.search_edit_keys.values_mut() {
            if !alternatives.contains(char::is_whitespace) {
                normalize_key_if_aliased(alternatives, &aliases)?;
                continue;
            }
            let mut keys: Vec<String> =
                alternatives.split_whitespace().map(str::to_owned).collect();
            normalize_key_list(&mut keys, &aliases)?;
            *alternatives = keys.join(" ");
        }
        for (_, keys) in self.temporary_key_lists_mut() {
            normalize_key_list(keys, &aliases)?;
        }
        // Window passthrough keys were normalized with their application overrides.
        for (_, keys) in
            builtin_mode_fields!(self, [&mut], |mode| &mut mode.temporary_mode_passthrough_keys)
                .into_iter()
                .filter(|(name, _)| !windows.contains(name))
        {
            normalize_passthrough_keys(keys, &aliases)?;
        }
        for mode in self.plugin_modes.values_mut() {
            normalize_key_list(&mut mode.temporary_mode_keys, &aliases)?;
            normalize_passthrough_keys(&mut mode.temporary_mode_passthrough_keys, &aliases)?;
        }
        Ok(())
    }

    pub(crate) fn resolved_key_aliases(&self) -> &BTreeMap<String, String> {
        &self.resolved_key_aliases
    }

    fn temporary_key_lists_mut(
        &mut self,
    ) -> impl Iterator<Item = (&'static str, &mut Vec<String>)> {
        builtin_mode_fields!(self, [&mut], |mode| &mut mode.temporary_mode_keys).into_iter()
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|error| ConfigError::Serialize(error.to_string()))
    }

    /// Effective macOS values, with explicit per-axis settings taking priority.
    pub fn macos_scroll_invert(&self) -> (bool, bool) {
        (
            self.platform
                .macos
                .scroll
                .invert_horizontal
                .unwrap_or(false),
            self.platform.macos.scroll.invert_vertical.unwrap_or(true),
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

impl ConfigFile {
    fn mode_routing(&self) -> impl Iterator<Item = (&str, Routing<'_>)> {
        // Keep the historical target-error order: these two sections follow windows.
        builtin_mode_fields!(self, [&], |mode| mode.routing())
            .into_iter()
            .filter(|(name, _)| !matches!(*name, "recursive_grid" | "ui_hint"))
            .chain([
                ("recursive_grid", self.recursive_grid.routing()),
                ("ui_hint", self.ui_hint.routing()),
            ])
            .chain(
                self.plugin_modes
                    .iter()
                    .map(|(id, mode)| (id.as_str(), mode.routing())),
            )
    }

    pub(crate) fn window_modes(&self) -> [(&'static str, &WindowModeConfig); 5] {
        [
            ("window", &self.window.common),
            ("window_quick", &self.window_quick.common),
            ("window_editor", &self.window_editor.common),
            ("window_restore", &self.window_restore.common),
            ("window_tab", &self.window_tab.common),
        ]
    }
    fn window_modes_mut(&mut self) -> [(&'static str, &mut WindowModeConfig); 5] {
        [
            ("window", &mut self.window.common),
            ("window_quick", &mut self.window_quick.common),
            ("window_editor", &mut self.window_editor.common),
            ("window_restore", &mut self.window_restore.common),
            ("window_tab", &mut self.window_tab.common),
        ]
    }
}

fn normalize_passthrough_keys(
    keys: &mut [String],
    aliases: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    for key in keys {
        *key = crate::api::KeyChord::parse_with_aliases(key, aliases)
            .map_err(ConfigError::Parse)?
            .canonical();
    }
    Ok(())
}
