//! Host-owned preset storage and native note-entry lifecycle.
use super::*;
use crate::api::window_presets::{
    MAX_NOTE_CHARS, PresetLibraryOperation, PresetLibraryRequest, PresetLibraryResult, TextPrompt,
    WindowTemplate,
};

pub(crate) trait PresetRepository {
    fn delete(
        &mut self,
        _expected: &crate::api::window_presets::SavedPreset,
    ) -> Result<Vec<crate::api::window_presets::SavedPreset>, String> {
        Err("Preset storage is unavailable".into())
    }
    fn list(&self) -> Result<Vec<crate::api::window_presets::SavedPreset>, String> {
        Ok(Vec::new())
    }
    fn save(
        &mut self,
        _template: WindowTemplate,
        _window_count: usize,
        _note: String,
    ) -> Result<(u32, Vec<crate::api::window_presets::SavedPreset>), String> {
        Err("Preset storage is unavailable".into())
    }
    fn export_file(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
struct UnavailableRepository;
impl PresetRepository for UnavailableRepository {}

pub(super) struct PresetController {
    pub(super) store: Box<dyn PresetRepository>,
    pub(super) pending: Option<PendingSave>,
    serial: u64,
}
impl Default for PresetController {
    fn default() -> Self {
        Self {
            store: Box::new(UnavailableRepository),
            pending: None,
            serial: 0,
        }
    }
}
pub(super) struct PendingSave {
    pub(super) id: u64,
    pub(super) session: u64,
    pub(super) owner: ModeId,
    template: WindowTemplate,
    window_count: usize,
}
impl Engine {
    pub(crate) fn attach_preset_store(&mut self, store: Box<dyn PresetRepository>) {
        self.window_presets.store = store;
    }
    pub(super) fn request_window_presets(
        &mut self,
        owner: &ModeId,
        request: PresetLibraryRequest,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match request.operation {
            PresetLibraryOperation::Delete { expected } => {
                let (layouts, message) = match self.window_presets.store.delete(&expected) {
                    Ok(layouts) => (layouts, None),
                    Err(error) => {
                        crate::report_error!("window-presets", "{error}");
                        (
                            self.window_presets.store.list().unwrap_or_default(),
                            Some(error),
                        )
                    }
                };
                self.dispatch_owned_to(
                    owner,
                    ModeEvent::WindowPresets(Box::new(PresetLibraryResult {
                        session: request.session,
                        presets: layouts,
                        saved: None,
                        message,
                    })),
                    backend,
                )
            }
            PresetLibraryOperation::List => {
                let (layouts, message) = match self.window_presets.store.list() {
                    Ok(layouts) => (layouts, None),
                    Err(error) => {
                        crate::report_error!("window-presets", "{error}");
                        (Vec::new(), Some(error))
                    }
                };
                self.dispatch_owned_to(
                    owner,
                    ModeEvent::WindowPresets(Box::new(PresetLibraryResult {
                        session: request.session,
                        presets: layouts,
                        saved: None,
                        message,
                    })),
                    backend,
                )
            }
            PresetLibraryOperation::Save {
                template,
                window_count,
            } => {
                if self.window_presets.pending.is_some() {
                    return Ok(());
                }
                self.window_presets.serial += 1;
                let id = self.window_presets.serial;
                self.window_presets.pending = Some(PendingSave {
                    id,
                    session: request.session,
                    owner: owner.clone(),
                    template,
                    window_count,
                });
                let screen = self.help_screen().ok_or("No display for preset input")?;
                let scale = crate::presentation::label_scale(screen.scale);
                let area = screen.work_area;
                let width = (720.0 * scale).min(area.width - 24.0 * scale).max(1.0);
                let height = (100.0 * scale).min(area.height);
                let bounds = crate::api::Rect::new(
                    area.center().x - width / 2.0,
                    (area.bottom() - height - 12.0 * scale).max(area.y),
                    width,
                    height,
                );
                self.overlay.key_help_cache = None;
                if let Some(scene) = self.overlay.content.clone() {
                    self.show_overlay(scene, backend)?;
                }
                let prompt = TextPrompt {
                    bounds,
                    id,
                    title: "Save preset".into(),
                    message: format!(
                        "Optional name · {window_count} windows. Leave blank for an automatic name."
                    ),
                    placeholder: "e.g. Coding / Reading".into(),
                    max_chars: MAX_NOTE_CHARS,
                };
                if let Err(error) = backend.request_text_prompt(prompt) {
                    self.finish_layout_note(id, Err(error), backend)?;
                }
                Ok(())
            }
        }
    }
    pub(super) fn finish_layout_note(
        &mut self,
        id: u64,
        value: Result<Option<String>, String>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if !self
            .window_presets
            .pending
            .as_ref()
            .is_some_and(|p| p.id == id)
        {
            return Ok(());
        }
        let Some(pending) = self.window_presets.pending.take() else {
            return Ok(());
        };
        let (layouts, saved, message) =
            match value {
                Ok(Some(note)) => match self.window_presets.store.save(
                    pending.template,
                    pending.window_count,
                    note,
                ) {
                    Ok((id, layouts)) => (layouts, Some(id), None),
                    Err(error) => {
                        crate::report_error!("window-presets", "{error}");
                        (Vec::new(), None, Some(error))
                    }
                },
                Ok(None) => (Vec::new(), None, Some("Save cancelled".into())),
                Err(error) => {
                    crate::report_error!("window-presets", "{error}");
                    (Vec::new(), None, Some(error))
                }
            };
        self.dispatch_owned_to(
            &pending.owner,
            ModeEvent::WindowPresets(Box::new(PresetLibraryResult {
                session: pending.session,
                presets: layouts,
                saved,
                message,
            })),
            backend,
        )
    }
}
