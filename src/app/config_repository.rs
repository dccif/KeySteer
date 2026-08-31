#![forbid(unsafe_code)]

//! Filesystem-backed configuration discovery and loading.
//!
//! The TOML model stays in `config`; application data-directory policy and
//! filesystem discovery belong to the composition layer.

use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigError, ConfigStore};

pub(crate) struct ConfigRepository {
    store: ConfigStore,
    discovery_directory: Option<PathBuf>,
}

impl ConfigRepository {
    pub(crate) fn fixed(store: ConfigStore) -> Self {
        Self {
            store,
            discovery_directory: None,
        }
    }

    pub(crate) fn discovered(store: ConfigStore, directory: PathBuf) -> Self {
        Self {
            store,
            discovery_directory: Some(directory),
        }
    }
}

impl crate::runtime::ConfigRepositoryPort for ConfigRepository {
    fn source_text(&self) -> String {
        self.store.source_text()
    }

    #[cfg(test)]
    fn source_label(&self) -> String {
        self.store.path().display().to_string()
    }

    fn set(&mut self, path: &str, value: &str) -> Result<Config, String> {
        self.store
            .set(path, value)
            .map_err(|error| error.to_string())
    }

    fn reload(&mut self) -> Result<(Config, Option<String>), String> {
        if let Some(directory) = self.discovery_directory.as_ref() {
            return match discover_in(directory).map_err(|error| error.to_string())? {
                Some(path) => {
                    let loaded = load_with_source(&path).map_err(|error| error.to_string())?;
                    self.store = ConfigStore::from_validated_text_with(
                        loaded.path.clone(),
                        loaded.raw_text,
                        crate::platform::atomic_replace,
                    );
                    Ok((loaded.config, Some(path.display().to_string())))
                }
                None => {
                    let config = Config::default();
                    self.store = ConfigStore::from_validated_text_with(
                        directory.join("keysteer.user.toml"),
                        config.to_toml().map_err(|error| error.to_string())?,
                        crate::platform::atomic_replace,
                    );
                    Ok((config, None))
                }
            };
        }

        // Reload into a detached candidate so invalid source never replaces
        // the last valid document used for later comment-preserving writes.
        let mut candidate = self.store.clone();
        let config = candidate.reload().map_err(|error| error.to_string())?;
        let source = candidate.path().display().to_string();
        self.store = candidate;
        Ok((config, Some(source)))
    }
}

pub(crate) struct LoadedConfig {
    pub(crate) config: Config,
    pub(crate) raw_text: String,
    pub(crate) path: PathBuf,
}

pub(crate) fn discover() -> Result<Option<PathBuf>, ConfigError> {
    let Some(directory) = super::paths::data_dir() else {
        return Ok(None);
    };
    discover_in(&directory)
}

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
        if file_type.is_file() && Config::is_portable_config_name(&entry.file_name()) {
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

pub(crate) fn default_write_path() -> Option<PathBuf> {
    super::paths::data_file("keysteer.user.toml")
}

pub(crate) fn load_with_source(path: &Path) -> Result<LoadedConfig, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| ConfigError::Io(format!("cannot read {}: {error}", path.display())))?;
    let config = Config::parse(&text)?;
    config.validate()?;
    Ok(LoadedConfig {
        config,
        raw_text: text,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_write_path_uses_the_application_data_directory() {
        let expected = super::super::paths::data_file("keysteer.user.toml").unwrap();
        assert_eq!(default_write_path(), Some(expected));
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
            discover_in(&directory).unwrap(),
            Some(directory.join("keysteer.Alpha.toml"))
        );

        std::fs::remove_file(directory.join("keysteer.Alpha.toml")).unwrap();
        std::fs::remove_file(directory.join("keysteer.zebra.toml")).unwrap();
        assert_eq!(
            discover_in(&directory).unwrap(),
            Some(directory.join("KEYSTEER.DEFAULT.TOML")),
            "the annotated default should remain a fallback when no user profile exists"
        );

        std::fs::remove_dir_all(directory).unwrap();
    }
}
