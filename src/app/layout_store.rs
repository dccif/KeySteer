//! Bounded binary layout favorites using the same directory policy as logs.
mod codec;
use crate::api::window_presets::{MAX_PRESETS, RegionTemplate, SavedLayout};
use crate::config::ReplaceFile;
use std::io::{Read, Write};
use std::path::PathBuf;

const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Default)]
pub(crate) struct LayoutStore {
    file: Option<(PathBuf, ReplaceFile)>,
    memory: Vec<SavedLayout>,
}
impl super::runtime::LayoutRepository for LayoutStore {
    fn list(&self) -> Result<Vec<SavedLayout>, String> {
        Self::list(self)
    }
    fn save(
        &mut self,
        regions: RegionTemplate,
        window_count: usize,
        note: String,
    ) -> Result<(u32, Vec<SavedLayout>), String> {
        Self::save(self, regions, window_count, note)
    }
    fn export_file(&self) -> Result<Option<Vec<u8>>, String> {
        Self::export_file(self)
    }
}

impl LayoutStore {
    pub(crate) fn export_file(&self) -> Result<Option<Vec<u8>>, String> {
        if let Some((path, _)) = &self.file {
            if !path
                .try_exists()
                .map_err(|e| format!("Cannot read saved layouts: {e}"))?
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
    pub(crate) fn list(&self) -> Result<Vec<SavedLayout>, String> {
        let Some((path, _)) = &self.file else {
            return Ok(self.memory.clone());
        };
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Cannot read saved layouts: {e}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read saved layouts: {e}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("Saved layouts file is too large".into());
        }
        codec::decode(&bytes)
    }
    pub(crate) fn save(
        &mut self,
        regions: RegionTemplate,
        window_count: usize,
        note: String,
    ) -> Result<(u32, Vec<SavedLayout>), String> {
        let mut layouts = self.list()?;
        let id = (1..=MAX_PRESETS as u32)
            .find(|id| !layouts.iter().any(|l| l.id == *id))
            .ok_or("Saved layout library is full")?;
        let layout = SavedLayout {
            id,
            note: note.trim().to_string(),
            window_count,
            regions,
        };
        layout.validate()?;
        layouts.push(layout);
        layouts.sort_by_key(|l| l.id);
        if let Some((path, replace)) = &self.file {
            let bytes = codec::encode(&layouts)?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err("Saved layouts file is too large".into());
            }
            let parent = path
                .parent()
                .ok_or("Saved layouts directory is unavailable")?;
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create layouts directory: {e}"))?;
            static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let serial = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let temporary = parent.join(format!(
                ".window-layouts-{}-{timestamp}-{serial}.tmp",
                std::process::id()
            ));
            let mut created = false;
            let result = (|| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)
                    .map_err(|e| format!("Cannot create saved layouts file: {e}"))?;
                created = true;
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|e| format!("Cannot write saved layouts: {e}"))?;
                drop(file);
                replace(&temporary, path).map_err(|e| format!("Cannot replace saved layouts: {e}"))
            })();
            if created {
                let _ = std::fs::remove_file(&temporary);
            }
            result?;
        } else {
            self.memory = layouts.clone();
        }
        Ok((id, layouts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn replace(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(source, target)
    }
    fn fail(_: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::other("injected replacement failure"))
    }
    fn path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "keysteer-layout-test-{}-{}.json",
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
        let mut store = LayoutStore::persistent(path.clone(), replace);
        assert!(store.list().unwrap().is_empty());
        store
            .save(RegionTemplate::Slot { id: 1 }, 1, "  中文备注 🦀  ".into())
            .unwrap();
        assert_eq!(
            LayoutStore::persistent(path.clone(), replace)
                .list()
                .unwrap()[0]
                .name(),
            "中文备注 🦀"
        );
        let before = std::fs::read(&path).unwrap();
        let mut failing = LayoutStore::persistent(path.clone(), fail);
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
        let mut store = LayoutStore::persistent(path.clone(), replace);
        assert!(
            store
                .save(RegionTemplate::Slot { id: 1 }, 1, String::new())
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::remove_file(path).unwrap();
    }
}
