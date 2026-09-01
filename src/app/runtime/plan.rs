use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::api::{Appearance, Binding, Mode, ModeId, ModeIndicator, Palette, Plugin};

pub type Bindings = BTreeMap<String, Binding>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DebugSettings {
    pub enabled: bool,
    pub keys: bool,
    pub actions: bool,
    pub modes: bool,
    pub backend: bool,
    pub pointer: bool,
    pub motion: bool,
    pub overlay: bool,
    pub timers: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineSettings {
    pub debug: DebugSettings,
    pub excluded_apps: Vec<String>,
    pub resolved_key_aliases: BTreeMap<String, String>,
    pub long_press_toggle_ms: u64,
    pub auto_release_ms: u64,
    pub passthrough_unbound_keys: bool,
    pub invert_scroll: (bool, bool),
    pub default_scan_roles: Vec<String>,
    pub ui_hint_overlap_key: String,
    pub mode_indicator: ModeIndicator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteSet {
    pub light: Palette,
    pub dark: Palette,
}

impl PaletteSet {
    pub fn for_appearance(&self, appearance: Appearance) -> Palette {
        match appearance {
            Appearance::Light => self.light.clone(),
            Appearance::Dark => self.dark.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRouteOverride {
    pub pattern: String,
    pub bindings: Bindings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeRoute {
    pub bindings: Bindings,
    pub inherits: Vec<ModeId>,
    pub temporary_mode: Option<ModeId>,
    pub temporary_keys: Vec<String>,
    pub app_overrides: Vec<AppRouteOverride>,
}

/// Fully compiled, TOML-independent runtime input.
pub struct RuntimePlan {
    pub settings: EngineSettings,
    pub palettes: PaletteSet,
    pub routes: BTreeMap<ModeId, ModeRoute>,
    pub modes: Vec<Box<dyn Mode>>,
    pub plugins: Vec<Box<dyn Plugin>>,
}

/// A compiled candidate and the repository state that produced it. The engine
/// installs both only after accepting the plan.
pub struct ConfigurationCandidate {
    pub plan: RuntimePlan,
    pub repository: Box<dyn ConfigurationRepository>,
    pub source_path: Option<PathBuf>,
}

/// Runtime-owned port for configuration persistence. Implementations live at
/// the application boundary, so the engine knows neither TOML nor ConfigFile.
pub trait ConfigurationRepository: Send {
    fn source_text(&self) -> Result<String, String>;

    fn source_path(&self) -> Option<PathBuf>;

    fn reload_candidate(&self) -> Result<ConfigurationCandidate, String>;

    fn set_candidate(&self, path: &str, value: &str) -> Result<ConfigurationCandidate, String>;
}

impl RuntimePlan {
    pub fn mode_ids(&self) -> impl Iterator<Item = ModeId> + '_ {
        self.modes.iter().map(|mode| mode.id())
    }
}
