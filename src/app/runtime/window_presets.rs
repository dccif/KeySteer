//! Host-owned layout storage and native note-entry lifecycle.
use super::*;
use crate::api::window_presets::{
    LayoutLibraryOperation, LayoutLibraryRequest, LayoutLibraryResult, MAX_NOTE_CHARS,
    RegionTemplate, TextPrompt,
};

pub(crate) trait LayoutRepository {
    fn list(&self) -> Result<Vec<crate::api::window_presets::SavedLayout>, String> {
        Ok(Vec::new())
    }
    fn save(
        &mut self,
        _regions: RegionTemplate,
        _window_count: usize,
        _note: String,
    ) -> Result<(u32, Vec<crate::api::window_presets::SavedLayout>), String> {
        Err("Layout storage is unavailable".into())
    }
    fn export_file(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
struct UnavailableRepository;
impl LayoutRepository for UnavailableRepository {}

pub(super) struct LayoutController {
    pub(super) store: Box<dyn LayoutRepository>,
    pub(super) pending: Option<PendingSave>,
    serial: u64,
}
impl Default for LayoutController {
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
    owner: ModeId,
    regions: RegionTemplate,
    window_count: usize,
}
impl Engine {
    pub(crate) fn attach_layout_store(&mut self, store: Box<dyn LayoutRepository>) {
        self.window_layouts.store = store;
    }
    pub(super) fn request_window_layouts(
        &mut self,
        owner: &ModeId,
        request: LayoutLibraryRequest,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        match request.operation {
            LayoutLibraryOperation::List => {
                let (layouts, message) = match self.window_layouts.store.list() {
                    Ok(layouts) => (layouts, None),
                    Err(error) => {
                        crate::report_error!("window-layouts", "{error}");
                        (Vec::new(), Some(error))
                    }
                };
                self.dispatch_owned_to(
                    owner,
                    ModeEvent::WindowLayouts(Box::new(LayoutLibraryResult {
                        session: request.session,
                        layouts,
                        saved: None,
                        message,
                    })),
                    backend,
                )
            }
            LayoutLibraryOperation::Save {
                regions,
                window_count,
            } => {
                if self.window_layouts.pending.is_some() {
                    return Ok(());
                }
                self.window_layouts.serial += 1;
                let id = self.window_layouts.serial;
                self.window_layouts.pending = Some(PendingSave {
                    id,
                    session: request.session,
                    owner: owner.clone(),
                    regions,
                    window_count,
                });
                let screen = self.help_screen().ok_or("No display for layout input")?;
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
                    title: "Save layout".into(),
                    message: format!(
                        "Optional note · {window_count} windows. Leave blank for an automatic name."
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
            .window_layouts
            .pending
            .as_ref()
            .is_some_and(|p| p.id == id)
        {
            return Ok(());
        }
        let Some(pending) = self.window_layouts.pending.take() else {
            return Ok(());
        };
        let (layouts, saved, message) =
            match value {
                Ok(Some(note)) => match self.window_layouts.store.save(
                    pending.regions,
                    pending.window_count,
                    note,
                ) {
                    Ok((id, layouts)) => (layouts, Some(id), None),
                    Err(error) => {
                        crate::report_error!("window-layouts", "{error}");
                        (Vec::new(), None, Some(error))
                    }
                },
                Ok(None) => (Vec::new(), None, Some("Save cancelled".into())),
                Err(error) => {
                    crate::report_error!("window-layouts", "{error}");
                    (Vec::new(), None, Some(error))
                }
            };
        self.dispatch_owned_to(
            &pending.owner,
            ModeEvent::WindowLayouts(Box::new(LayoutLibraryResult {
                session: pending.session,
                layouts,
                saved,
                message,
            })),
            backend,
        )
    }
}
