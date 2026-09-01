//! Compile a validated TOML document into the runtime's native input model.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(test)]
use super::runtime::Engine;
use super::runtime::{
    AppRouteOverride, ConfigurationCandidate, ConfigurationRepository, DebugSettings,
    EngineSettings, ModeRoute, PaletteSet, RuntimePlan,
};
use crate::api::{Appearance, ModeId};
use crate::config::{AppOverride, Bindings, ConfigFile, ConfigStore, UiHintAppOverride};

#[derive(Clone)]
pub(crate) struct ConfigRepository {
    config: ConfigFile,
    source: String,
    store: Option<ConfigStore>,
    discovery_directory: Option<PathBuf>,
}

impl ConfigRepository {
    pub(crate) fn new(
        config: ConfigFile,
        source: String,
        store: Option<ConfigStore>,
        discovery_directory: Option<PathBuf>,
    ) -> Self {
        Self {
            config,
            source,
            store,
            discovery_directory,
        }
    }

    fn candidate(self, source_path: Option<PathBuf>) -> Result<ConfigurationCandidate, String> {
        let plan = compile(&self.config)?;
        Ok(ConfigurationCandidate {
            plan,
            repository: Box::new(self),
            source_path,
        })
    }
}

impl ConfigFile {
    pub fn discover() -> Result<Option<PathBuf>, crate::config::ConfigError> {
        let Some(directory) = super::paths::data_dir() else {
            return Ok(None);
        };
        Self::discover_in(&directory)
    }

    pub fn default_write_path() -> Option<PathBuf> {
        super::paths::data_file("keysteer.user.toml")
    }
}

impl ConfigurationRepository for ConfigRepository {
    fn source_text(&self) -> Result<String, String> {
        Ok(self.source.clone())
    }

    fn source_path(&self) -> Option<PathBuf> {
        self.store.as_ref().map(|store| store.path().to_path_buf())
    }

    fn reload_candidate(&self) -> Result<ConfigurationCandidate, String> {
        if let Some(directory) = self.discovery_directory.as_ref() {
            return match ConfigFile::discover_in(directory).map_err(|error| error.to_string())? {
                Some(path) => {
                    let loaded =
                        ConfigFile::load_with_source(&path).map_err(|error| error.to_string())?;
                    let store = ConfigStore::from_validated_text(
                        loaded.path.clone(),
                        loaded.raw_text.clone(),
                        crate::platform::atomic_replace,
                    );
                    Self::new(
                        loaded.config,
                        loaded.raw_text,
                        Some(store),
                        Some(directory.clone()),
                    )
                    .candidate(Some(loaded.path))
                }
                None => {
                    let config = ConfigFile::default();
                    let source = config.to_toml().map_err(|error| error.to_string())?;
                    let store = ConfigStore::from_validated_text(
                        directory.join("keysteer.user.toml"),
                        source.clone(),
                        crate::platform::atomic_replace,
                    );
                    Self::new(config, source, Some(store), Some(directory.clone())).candidate(None)
                }
            };
        }

        if let Some(current) = self.store.as_ref() {
            let mut store = current.clone();
            let config = store.reload().map_err(|error| error.to_string())?;
            let source = store.source_text();
            let path = store.path().to_path_buf();
            return Self::new(config, source, Some(store), None).candidate(Some(path));
        }

        self.clone().candidate(None)
    }

    fn set_candidate(&self, path: &str, value: &str) -> Result<ConfigurationCandidate, String> {
        let store = self
            .store
            .clone()
            .ok_or_else(|| "no writable configuration source is attached".to_string())?;
        let (store, config) = store
            .prepare_set(path, value)
            .map_err(|error| error.to_string())?;
        let plan = compile(&config)?;
        store.persist().map_err(|error| error.to_string())?;
        let source = store.source_text();
        let source_path = store.path().to_path_buf();
        let repository = Self::new(
            config,
            source,
            Some(store),
            self.discovery_directory.clone(),
        );
        Ok(ConfigurationCandidate {
            plan,
            repository: Box::new(repository),
            source_path: Some(source_path),
        })
    }
}

#[cfg(test)]
impl Engine {
    /// Convenience assembly entry point for callers and tests that own a
    /// configuration document. Runtime itself consumes only the compiled plan.
    pub fn new(config: ConfigFile, appearance: Appearance) -> Self {
        let source = match config.to_toml() {
            Ok(source) => source,
            Err(error) => panic!("Engine::new requires a serializable configuration: {error}"),
        };
        let mut plan = compile(&config)
            .unwrap_or_else(|error| panic!("Engine::new requires valid configuration: {error}"));
        // `new` is a low-level assembly convenience used by tests and custom
        // callers that register their own modes. Production bootstrap passes
        // the complete catalog through `from_plan` instead.
        plan.modes.clear();
        plan.plugins.clear();
        let mut engine = match Self::from_plan(plan, appearance) {
            Ok(engine) => engine,
            Err(error) => panic!("Engine::new requires a valid catalog: {error}"),
        };
        engine.attach_configuration(Box::new(ConfigRepository::new(
            config.clone(),
            source,
            None,
            None,
        )));
        engine
    }

    pub fn apply_config(&mut self, config: ConfigFile) -> Result<(), String> {
        let plan = compile(&config)?;
        self.apply_plan(plan)?;
        Ok(())
    }
}

pub fn compile(config: &ConfigFile) -> Result<RuntimePlan, String> {
    config.validate().map_err(|error| error.to_string())?;

    let modes = super::mode_catalog::built_in(config);
    let plugins = super::mode_catalog::bundled_plugins(config)?;
    let mut ids: Vec<ModeId> = modes.iter().map(|mode| mode.id()).collect();
    ids.extend(plugins.iter().map(|plugin| plugin.id()));
    ids.sort();
    ids.dedup();

    let routes = ids
        .into_iter()
        .filter_map(|id| {
            route(config, &id)
                .transpose()
                .map(|route| route.map(|route| (id, route)))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    Ok(RuntimePlan {
        settings: EngineSettings {
            debug: DebugSettings {
                enabled: config.debug.enabled,
                keys: config.debug.keys,
                actions: config.debug.actions,
                modes: config.debug.modes,
                backend: config.debug.backend,
                pointer: config.debug.pointer,
                motion: config.debug.motion,
                overlay: config.debug.overlay,
                timers: config.debug.timers,
            },
            excluded_apps: config.general.excluded_apps.clone(),
            resolved_key_aliases: config.resolved_key_aliases().clone(),
            long_press_toggle_ms: config.normal.long_press_toggle_ms,
            auto_release_ms: config.normal.auto_release_ms,
            passthrough_unbound_keys: config.normal.passthrough_unbound_keys,
            invert_scroll: config.effective_scroll_invert(),
            default_scan_roles: config.ui_hint.clickable_roles.clone(),
            ui_hint_overlap_key: config.ui_hint.overlap_cycle_key.clone(),
            mode_indicator: config.mode_indicator.clone(),
        },
        palettes: PaletteSet {
            light: config.palette(Appearance::Light),
            dark: config.palette(Appearance::Dark),
        },
        routes,
        modes,
        plugins,
    })
}

fn route(config: &ConfigFile, id: &ModeId) -> Result<Option<ModeRoute>, String> {
    let Some(bindings) = config.bindings_for(id.as_str()).cloned() else {
        return Ok(None);
    };
    let (inherits, temporary_mode, temporary_keys) = config
        .inheritance_for(id.as_str())
        .unwrap_or((&[], None, &[]));
    let inherits = inherits
        .iter()
        .map(|source| parse_source(source))
        .collect::<Result<Vec<_>, _>>()?;
    let temporary_mode = temporary_mode.map(ModeId::parse_borrowed).transpose()?;
    let app_overrides = app_overrides(config, id)
        .into_iter()
        .map(|(pattern, bindings)| AppRouteOverride { pattern, bindings })
        .collect();
    Ok(Some(ModeRoute {
        bindings,
        inherits,
        temporary_mode,
        temporary_keys: temporary_keys.to_vec(),
        app_overrides,
    }))
}

fn parse_source(source: &str) -> Result<ModeId, String> {
    if source == "hotkeys" {
        Ok(ModeId::idle())
    } else {
        ModeId::parse_borrowed(source)
    }
}

fn app_overrides(config: &ConfigFile, id: &ModeId) -> Vec<(String, Bindings)> {
    if id == &ModeId::ui_hint() {
        return config
            .ui_hint
            .app_configs
            .iter()
            .map(ui_hint_override)
            .collect();
    }
    let values: &[AppOverride] = match id.as_str() {
        "idle" => &config.app_configs,
        "normal" => &config.normal.app_configs,
        "grid" => &config.grid.app_configs,
        "recursive_grid" => &config.recursive_grid.app_configs,
        other => config
            .plugin_modes
            .get(other)
            .map(|mode| mode.app_configs.as_slice())
            .unwrap_or(&[]),
    };
    values
        .iter()
        .map(|value| (value.bundle_id.clone(), value.bindings.clone()))
        .collect()
}

fn ui_hint_override(value: &UiHintAppOverride) -> (String, Bindings) {
    (value.bundle_id.clone(), value.bindings.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_unique_catalog_and_enable_flags() {
        let mut config = ConfigFile::default();
        config.grid.enabled = false;
        let plan = compile(&config).unwrap();
        let ids: Vec<_> = plan.mode_ids().collect();
        assert!(!ids.contains(&ModeId::grid()));
        assert_eq!(ids.iter().filter(|id| **id == ModeId::idle()).count(), 1);
        assert!(plan.routes.contains_key(&ModeId::normal()));
    }

    #[test]
    fn compiles_inheritance_and_app_overrides_without_toml_types() {
        let mut config = ConfigFile::default();
        config.normal.app_configs.push(AppOverride {
            bundle_id: "com.example.Editor".into(),
            bindings: Bindings::new(),
        });
        let plan = compile(&config).unwrap();
        let grid = &plan.routes[&ModeId::grid()];
        assert_eq!(grid.inherits, [ModeId::idle(), ModeId::normal()]);
        assert_eq!(
            plan.routes[&ModeId::normal()].app_overrides[0].pattern,
            "com.example.Editor"
        );
    }
}
