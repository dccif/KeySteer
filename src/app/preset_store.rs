//! Bounded binary workspace presets using the same directory policy as logs.
mod codec;
use crate::api::window_presets::{MAX_PRESETS, SavedPreset, WindowTemplate};
use crate::config::ReplaceFile;
use std::io::{Read, Write};
use std::path::PathBuf;

pub(crate) const FILE_NAME: &str = "workspace.ksw";

const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Default)]
pub(crate) struct PresetStore {
    file: Option<(PathBuf, ReplaceFile)>,
    memory: Vec<SavedPreset>,
}
impl super::runtime::PresetRepository for PresetStore {
    fn delete(&mut self, expected: &SavedPreset) -> Result<Vec<SavedPreset>, String> {
        Self::delete(self, expected)
    }

    fn list(&self) -> Result<Vec<SavedPreset>, String> {
        Self::list(self)
    }
    fn save(
        &mut self,
        template: WindowTemplate,
        window_count: usize,
        note: String,
    ) -> Result<(u32, Vec<SavedPreset>), String> {
        Self::save(self, template, window_count, note)
    }
    fn export_file(&self) -> Result<Option<Vec<u8>>, String> {
        Self::export_file(self)
    }
}

impl PresetStore {
    pub(crate) fn export_file(&self) -> Result<Option<Vec<u8>>, String> {
        if let Some((path, _)) = &self.file {
            if !path
                .try_exists()
                .map_err(|e| format!("Cannot read saved presets: {e}"))?
            {
                return Ok(None);
            }
        } else if self.memory.is_empty() {
            return Ok(None);
        }
        codec::encode(&self.list()?).map(Some)
    }
    pub(crate) fn persistent(path: PathBuf, replace: ReplaceFile) -> Self {
        Self {
            file: Some((path, replace)),
            memory: Vec::new(),
        }
    }
    pub(crate) fn list(&self) -> Result<Vec<SavedPreset>, String> {
        let Some((path, _)) = &self.file else {
            return Ok(self.memory.clone());
        };
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Cannot read saved presets: {e}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read saved presets: {e}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("Saved presets file is too large".into());
        }
        codec::decode(&bytes)
    }
    pub(crate) fn save(
        &mut self,
        template: impl Into<WindowTemplate>,
        window_count: usize,
        note: String,
    ) -> Result<(u32, Vec<SavedPreset>), String> {
        let mut layouts = self.list()?;
        let id = (1..=MAX_PRESETS as u32)
            .find(|id| !layouts.iter().any(|l| l.id == *id))
            .ok_or("Preset library is full")?;
        let layout = SavedPreset {
            id,
            note: note.trim().to_string(),
            window_count,
            template: template.into(),
        };
        layout.validate()?;
        layouts.push(layout);
        layouts.sort_by_key(|l| l.id);
        self.write_layouts(&layouts)?;
        Ok((id, layouts))
    }
    pub(crate) fn delete(&mut self, expected: &SavedPreset) -> Result<Vec<SavedPreset>, String> {
        let mut layouts = self.list()?;
        let current = layouts
            .iter()
            .find(|layout| layout.id == expected.id)
            .ok_or("Preset no longer exists; select it again")?;
        if current != expected {
            return Err("Preset changed outside KeySteer; select it again before deleting".into());
        }
        layouts.retain(|layout| layout.id != expected.id);
        self.write_layouts(&layouts)?;
        Ok(layouts)
    }
    fn write_layouts(&mut self, layouts: &[SavedPreset]) -> Result<(), String> {
        if let Some((path, replace)) = &self.file {
            let bytes = codec::encode(layouts)?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err("Saved presets file is too large".into());
            }
            let parent = path
                .parent()
                .ok_or("Saved presets directory is unavailable")?;
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create workspace directory: {e}"))?;
            static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let temporary = parent.join(format!(
                ".workspace-{}-{timestamp}-{serial}.tmp",
                std::process::id()
            ));
            let mut created = false;
            let result = (|| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|e| format!("Cannot create saved presets file: {e}"))?;
                created = true;
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|e| format!("Cannot write saved presets: {e}"))?;
                drop(file);
                replace(&temporary, path).map_err(|e| format!("Cannot replace saved presets: {e}"))
            })();
            if created {
                let _ = std::fs::remove_file(&temporary);
            }
            result?;
        } else {
            self.memory = layouts.to_vec();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::window_presets::{RegionTemplate, TabTemplate};
    #[test]
    fn layout_and_tabs_share_ids_names_and_atomic_storage() {
        let path = path();
        let mut store = PresetStore::persistent(path.clone(), replace);
        let (id, _) = store
            .save(RegionTemplate::Slot { id: 1 }, 1, String::new())
            .unwrap();
        assert_eq!(id, 1);
        let (id, presets) = store
            .save(
                TabTemplate {
                    region: crate::api::Rect::new(0.1, 0.2, 0.6, 0.5),
                    active: 1,
                },
                2,
                String::new(),
            )
            .unwrap();
        assert_eq!(id, 2);
        assert_eq!(
            presets.iter().map(SavedPreset::name).collect::<Vec<_>>(),
            ["Layout 1", "Tabs 2"]
        );
        assert_eq!(store.list().unwrap(), presets);
        assert_eq!(store.delete(&presets[0]).unwrap(), vec![presets[1].clone()]);
        let (id, presets) = store
            .save(RegionTemplate::Slot { id: 1 }, 1, "  Custom  ".into())
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(presets[0].name(), "Custom");
        std::fs::remove_file(path).unwrap();
    }
    fn replace(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(source, target)
    }
    fn fail(_: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::other("injected replacement failure"))
    }
    fn path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "keysteer-workspace-test-{}-{}.ksw",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn unicode_persists_across_store_instances_and_failed_replacement_preserves_previous_bytes() {
        let path = path();
        let mut store = PresetStore::persistent(path.clone(), replace);
        assert!(store.list().unwrap().is_empty());
        store
            .save(RegionTemplate::Slot { id: 1 }, 1, "  中文备注 🦀  ".into())
            .unwrap();
        assert_eq!(
            PresetStore::persistent(path.clone(), replace)
                .list()
                .unwrap()[0]
                .name(),
            "中文备注 🦀"
        );
        let before = std::fs::read(&path).unwrap();
        let mut failing = PresetStore::persistent(path.clone(), fail);
        assert!(
            failing
                .save(RegionTemplate::Slot { id: 1 }, 1, String::new())
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn corrupt_file_is_not_silently_overwritten() {
        let path = path();
        std::fs::write(&path, b"broken").unwrap();
        let mut store = PresetStore::persistent(path.clone(), replace);
        assert!(
            store
                .save(RegionTemplate::Slot { id: 1 }, 1, String::new())
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn delete_rechecks_records_preserves_ids_and_persists_a_valid_empty_library() {
        let path = path();
        let mut store = PresetStore::persistent(path.clone(), replace);
        store
            .save(RegionTemplate::Slot { id: 1 }, 1, "First".into())
            .unwrap();
        store
            .save(RegionTemplate::Slot { id: 1 }, 1, "Second".into())
            .unwrap();
        let old = store.list().unwrap();
        let remaining = store.delete(&old[0]).unwrap();
        assert_eq!(remaining, vec![old[1].clone()]);
        assert!(store.delete(&old[0]).is_err());
        let bytes = std::fs::read(&path).unwrap();
        let mut failed = PresetStore::persistent(path.clone(), fail);
        assert!(failed.delete(&old[1]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let mut changed = old[1].clone();
        changed.note = "Replaced externally".into();
        std::fs::write(&path, codec::encode(&[changed.clone()]).unwrap()).unwrap();
        assert!(store.delete(&old[1]).is_err());
        assert_eq!(store.list().unwrap(), vec![changed.clone()]);
        assert!(store.delete(&changed).unwrap().is_empty());
        assert!(
            codec::decode(&std::fs::read(&path).unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(
            PresetStore::persistent(path.clone(), replace)
                .list()
                .unwrap()
                .is_empty()
        );
        std::fs::remove_file(path).unwrap();
    }
}
