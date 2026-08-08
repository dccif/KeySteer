//! Comment-preserving TOML storage with validate-before-swap semantics.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use super::{Config, ConfigError};

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    document: DocumentMut,
}

impl ConfigStore {
    pub fn open(path: impl Into<PathBuf>, fallback: &Config) -> Result<Self, ConfigError> {
        let path = path.into();
        let text = if path.is_file() {
            std::fs::read_to_string(&path)
                .map_err(|e| ConfigError::Io(format!("cannot read {}: {e}", path.display())))?
        } else {
            fallback.to_toml()
        };
        let document = text
            .parse::<DocumentMut>()
            .map_err(|e| ConfigError::Parse(format!("cannot edit {}: {e}", path.display())))?;
        Ok(Self { path, document })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn reload(&mut self) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| ConfigError::Io(format!("cannot read {}: {e}", self.path.display())))?;
        let document = text
            .parse::<DocumentMut>()
            .map_err(|e| ConfigError::Parse(format!("cannot edit {}: {e}", self.path.display())))?;
        let config = Config::parse(&text)?;
        config.validate()?;
        self.document = document;
        Ok(config)
    }

    pub fn set(&mut self, path: &str, raw_value: &str) -> Result<Config, ConfigError> {
        let segments: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
        if segments.is_empty() {
            return Err(ConfigError::Invalid("config path must not be empty".into()));
        }

        let parsed = format!("value = {raw_value}")
            .parse::<DocumentMut>()
            .map_err(|e| ConfigError::Parse(format!("invalid TOML value for {path}: {e}")))?;
        let value = parsed
            .get("value")
            .cloned()
            .ok_or_else(|| ConfigError::Parse(format!("missing TOML value for {path}")))?;

        let mut candidate = self.document.clone();
        set_item(candidate.as_table_mut(), &segments, value)?;
        let text = candidate.to_string();
        let config = Config::parse(&text)?;
        config.validate()?;
        atomic_write(&self.path, text.as_bytes())?;
        self.document = candidate;
        Ok(config)
    }
}

fn set_item(table: &mut Table, path: &[&str], value: Item) -> Result<(), ConfigError> {
    let Some((head, tail)) = path.split_first() else {
        return Err(ConfigError::Invalid("config path must not be empty".into()));
    };
    if tail.is_empty() {
        table.insert(head, value);
        return Ok(());
    }
    if !table.contains_key(head) {
        table.insert(head, Item::Table(Table::new()));
    }
    let child = table
        .get_mut(head)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| ConfigError::Invalid(format!("{head:?} is not a TOML table")))?;
    set_item(child, tail, value)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| {
        ConfigError::Io(format!(
            "cannot create config directory {}: {e}",
            parent.display()
        ))
    })?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes)
        .map_err(|e| ConfigError::Io(format!("cannot write {}: {e}", temp.display())))?;

    crate::platform::atomic_replace(&temp, path)
        .map_err(|e| ConfigError::Io(format!("cannot replace {}: {e}", path.display())))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file_name(prefix: &str) -> String {
        let current_thread = std::thread::current();
        let thread_name = current_thread.name().unwrap_or("test");
        let sanitized: String = thread_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        format!("{prefix}-{}-{sanitized}.toml", std::process::id())
    }

    #[test]
    fn creates_nested_platform_setting_tables() {
        let path = std::env::temp_dir().join(test_file_name("keysteer-platform-store"));
        let mut store = ConfigStore::open(&path, &Config::default()).unwrap();
        let config = store
            .set("platform.macos.scroll.invert_vertical", "false")
            .unwrap();
        assert_eq!(config.platform.macos.scroll.invert_vertical, Some(false));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_update_keeps_the_last_valid_document() {
        let path = std::env::temp_dir().join(test_file_name("keysteer-store"));
        let mut store = ConfigStore::open(&path, &Config::default()).unwrap();
        let before = store.document.to_string();
        assert!(store.set("pointer.initial_speed", "0").is_err());
        assert_eq!(store.document.to_string(), before);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn updates_platform_specific_key_aliases() {
        let path =
            std::env::temp_dir().join(format!("keysteer-alias-store-{}.toml", std::process::id()));
        let mut store = ConfigStore::open(&path, &Config::default()).unwrap();
        let config = store
            .set("key_aliases.windows.Primary", "\"left_alt\"")
            .unwrap();
        assert_eq!(config.key_aliases.windows["Primary"], "left_alt");
        let _ = std::fs::remove_file(path);
    }
}
