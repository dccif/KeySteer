//! Compile a validated TOML document into the runtime's native input model.

use std::path::PathBuf;

#[cfg(test)]
use super::runtime::Engine;
use super::runtime::{
    ConfigurationCandidate, ConfigurationRepository, DebugSettings, EngineSettings, PaletteSet,
    RuntimePlan,
};
use crate::api::Appearance;
use crate::config::{ConfigFile, ConfigStore};

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
        let plan = compile(&config)
            .unwrap_or_else(|error| panic!("Engine::new requires valid configuration: {error}"));
        // `new` is a low-level test assembly convenience whose callers
        // register probes explicitly. Production always installs complete
        // ModeSpec values through `from_plan`.
        let mut engine = match Self::from_plan_without_modes(plan, appearance) {
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

    let mut specs = super::mode_catalog::built_in_specs(config)?;
    specs.extend(super::mode_catalog::bundled_specs(config)?);
    let mut ids: Vec<_> = specs.iter().map(|spec| spec.id()).collect();
    ids.sort();
    let duplicate = ids.windows(2).find(|ids| ids[0] == ids[1]);
    if let Some(ids) = duplicate {
        return Err(format!("duplicate mode id in catalog: {}", ids[0]));
    }

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
        modes: specs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModeId;
    use crate::config::{AppOverride, Bindings};

    #[test]
    fn compiles_unique_catalog_and_enable_flags() {
        let mut config = ConfigFile::default();
        config.grid.enabled = false;
        let plan = compile(&config).unwrap();
        let ids: Vec<_> = plan.mode_ids().collect();
        assert!(!ids.contains(&ModeId::grid()));
        assert_eq!(ids.iter().filter(|id| **id == ModeId::idle()).count(), 1);
        assert!(plan.route(&ModeId::normal()).is_some());
    }

    #[test]
    fn compiles_inheritance_and_app_overrides_without_toml_types() {
        let mut config = ConfigFile::default();
        config.normal.app_configs.push(AppOverride {
            bundle_id: "com.example.Editor".into(),
            bindings: Bindings::new(),
        });
        let plan = compile(&config).unwrap();
        let grid = plan.route(&ModeId::grid()).unwrap();
        assert_eq!(grid.inherits, [ModeId::idle(), ModeId::normal()]);
        assert_eq!(
            plan.route(&ModeId::normal()).unwrap().app_overrides[0].pattern,
            "com.example.Editor"
        );
    }
}
