//! Comment-preserving TOML storage with validate-before-swap semantics.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use super::{Config, ConfigError};

pub type ReplaceFile = fn(&Path, &Path) -> std::io::Result<()>;

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
    source: StoreSource,
    replace_file: ReplaceFile,
}

#[derive(Debug, Clone)]
enum StoreSource {
    Raw(String),
    Parsed(DocumentMut),
}

impl StoreSource {
    fn text(&self) -> String {
        match self {
            Self::Raw(text) => text.clone(),
            Self::Parsed(document) => document.to_string(),
        }
    }
}

impl ConfigStore {
    pub fn open(
        path: impl Into<PathBuf>,
        fallback: &Config,
        replace_file: ReplaceFile,
    ) -> Result<Self, ConfigError> {
        let path = path.into();
        let text = if path.is_file() {
            std::fs::read_to_string(&path)
                .map_err(|e| ConfigError::Io(format!("cannot read {}: {e}", path.display())))?
        } else {
            fallback.to_toml()?
        };
        Config::parse(&text)?;
        Ok(Self::from_validated_text(path, text, replace_file))
    }

    pub(crate) fn from_validated_text(
        path: impl Into<PathBuf>,
        text: String,
        replace_file: ReplaceFile,
    ) -> Self {
        // The caller already parsed and validated this exact source. Keep the
        // compact text and defer the comment-preserving AST until the first edit.
        Self {
            path: path.into(),
            source: StoreSource::Raw(text),
            replace_file,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the exact source currently backing the active configuration.
    ///
    /// This intentionally serializes the comment-preserving document after a
    /// runtime edit instead of rereading the file, so callers see the same
    /// configuration that the engine is using.
    pub(crate) fn source_text(&self) -> String {
        self.source.text()
    }

    pub fn reload(&mut self) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| ConfigError::Io(format!("cannot read {}: {e}", self.path.display())))?;
        let config = Config::parse(&text)?;
        config.validate()?;
        self.source = StoreSource::Raw(text);
        Ok(config)
    }

    pub fn set(&mut self, path: &str, raw_value: &str) -> Result<Config, ConfigError> {
        let (candidate, config) = self.prepare_set(path, raw_value)?;
        candidate.persist()?;
        *self = candidate;
        Ok(config)
    }

    /// Build an edited, validated store without touching the active file.
    pub fn prepare_set(&self, path: &str, raw_value: &str) -> Result<(Self, Config), ConfigError> {
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

        let document = match &self.source {
            StoreSource::Raw(text) => text.parse::<DocumentMut>().map_err(|e| {
                ConfigError::Parse(format!("cannot edit {}: {e}", self.path.display()))
            })?,
            StoreSource::Parsed(document) => document.clone(),
        };
        let mut candidate = document;
        set_item(candidate.as_table_mut(), &segments, value)?;
        let text = candidate.to_string();
        let config = Config::parse(&text)?;
        config.validate()?;
        let mut store = self.clone();
        store.source = StoreSource::Parsed(candidate);
        Ok((store, config))
    }

    pub fn persist(&self) -> Result<(), ConfigError> {
        atomic_write(&self.path, self.source.text().as_bytes(), self.replace_file)
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

fn atomic_write(path: &Path, bytes: &[u8], replace_file: ReplaceFile) -> Result<(), ConfigError> {
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

    replace_file(&temp, path)
        .map_err(|e| ConfigError::Io(format!("cannot replace {}: {e}", path.display())))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
        if destination.exists() {
            std::fs::remove_file(destination)?;
        }
        std::fs::rename(source, destination)
    }

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
        let mut store = ConfigStore::open(&path, &Config::default(), replace_file).unwrap();
        let config = store
            .set("platform.macos.scroll.invert_vertical", "false")
            .unwrap();
        assert_eq!(config.platform.macos.scroll.invert_vertical, Some(false));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_update_keeps_the_last_valid_document() {
        let path = std::env::temp_dir().join(test_file_name("keysteer-store"));
        let mut store = ConfigStore::open(&path, &Config::default(), replace_file).unwrap();
        let before = store.source.text();
        assert!(store.set("pointer.initial_speed", "0").is_err());
        assert_eq!(store.source.text(), before);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn prepared_update_does_not_touch_disk_until_committed() {
        let path = std::env::temp_dir().join(test_file_name("keysteer-prepared-store"));
        let original = Config::default().to_toml().unwrap();
        std::fs::write(&path, &original).unwrap();
        let store = ConfigStore::open(&path, &Config::default(), replace_file).unwrap();

        let (candidate, config) = store.prepare_set("pointer.initial_speed", "17").unwrap();
        assert_eq!(config.pointer.initial_speed, 17.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        candidate.persist().unwrap();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn updates_platform_specific_key_aliases() {
        let path =
            std::env::temp_dir().join(format!("keysteer-alias-store-{}.toml", std::process::id()));
        let mut store = ConfigStore::open(&path, &Config::default(), replace_file).unwrap();
        let config = store
            .set("key_aliases.windows.Primary", "\"left_alt\"")
            .unwrap();
        assert_eq!(config.key_aliases.windows["Primary"], "left_alt");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn document_ast_is_lazy_and_reload_returns_to_raw_source() {
        let path = std::env::temp_dir().join(test_file_name("keysteer-lazy-store"));
        std::fs::write(&path, Config::default().to_toml().unwrap()).unwrap();
        let mut store = ConfigStore::open(&path, &Config::default(), replace_file).unwrap();
        assert!(matches!(store.source, StoreSource::Raw(_)));
        store.set("pointer.initial_speed", "11").unwrap();
        assert!(matches!(store.source, StoreSource::Parsed(_)));
        store.reload().unwrap();
        assert!(matches!(store.source, StoreSource::Raw(_)));
        let _ = std::fs::remove_file(path);
    }
}
