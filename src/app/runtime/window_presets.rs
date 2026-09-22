//! Host-owned preset storage and native note-entry lifecycle.
use super::*;
use crate::api::window_presets::{
    MAX_NOTE_CHARS, PresetLibraryOperation, PresetLibraryRequest, PresetLibraryResult, TextPrompt,
    WindowTemplate, WorkspaceCompletion, WorkspaceOperation, WorkspaceValue,
};

pub(crate) trait PresetRepository {
    /// Native-backed stores enqueue work; memory/headless stores complete inline.
    fn submit(
        &mut self,
        id: u64,
        operation: WorkspaceOperation,
        _emit: Option<Arc<dyn Fn(BackendEvent) + Send + Sync>>,
    ) -> Result<Option<WorkspaceCompletion>, String> {
        Ok(Some(WorkspaceCompletion {
            id,
            outcome: self.execute(operation),
        }))
    }
    fn execute(&mut self, operation: WorkspaceOperation) -> Result<WorkspaceValue, String> {
        match operation {
            WorkspaceOperation::List => self.list().map(|presets| WorkspaceValue::Library {
                presets,
                saved: None,
            }),
            WorkspaceOperation::Delete { expected } => {
                self.delete(&expected)
                    .map(|presets| WorkspaceValue::Library {
                        presets,
                        saved: None,
                    })
            }
            WorkspaceOperation::Save {
                template,
                window_count,
                note,
            } => self
                .save(template, window_count, note)
                .map(|(id, presets)| WorkspaceValue::Library {
                    presets,
                    saved: Some(id),
                }),
            WorkspaceOperation::Export => self.export_file().map(WorkspaceValue::Export),
            WorkspaceOperation::Checkpoint(reply) => {
                let result = self.flush_usage().map(|()| WorkspaceValue::Checkpoint);
                let _ = reply.send(());
                result
            }
        }
    }

    fn record_mode_entry(&mut self, _mode: &str, _save_after_entries: u32) {}
    fn flush_usage(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn mode_usage(&self) -> std::collections::BTreeMap<String, u64> {
        Default::default()
    }
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
    completions: BTreeMap<u64, CompletionOwner>,
}
impl Default for PresetController {
    fn default() -> Self {
        Self {
            store: Box::new(UnavailableRepository),
            pending: None,
            serial: 0,
            completions: BTreeMap::new(),
        }
    }
}
enum CompletionOwner {
    Mode { owner: ModeId, session: u64 },
    Simulator { source: String },
    Checkpoint,
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
            PresetLibraryOperation::Delete { expected } => self.submit_workspace(
                WorkspaceOperation::Delete { expected },
                CompletionOwner::Mode {
                    owner: owner.clone(),
                    session: request.session,
                },
                backend,
            ),
            PresetLibraryOperation::List => self.submit_workspace(
                WorkspaceOperation::List,
                CompletionOwner::Mode {
                    owner: owner.clone(),
                    session: request.session,
                },
                backend,
            ),
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
        let message = match value {
            Ok(Some(note)) => {
                return self.submit_workspace(
                    WorkspaceOperation::Save {
                        template: pending.template,
                        window_count: pending.window_count,
                        note,
                    },
                    CompletionOwner::Mode {
                        owner: pending.owner,
                        session: pending.session,
                    },
                    backend,
                );
            }
            Ok(None) => Some("Save cancelled".into()),
            Err(error) => {
                crate::report_error!("window-presets", "operation=note: {error}");
                Some(error)
            }
        };
        self.dispatch_owned_to(
            &pending.owner,
            ModeEvent::WindowPresets(Box::new(PresetLibraryResult {
                session: pending.session,
                presets: Vec::new(),
                saved: None,
                message,
            })),
            backend,
        )
    }

    fn submit_workspace(
        &mut self,
        operation: WorkspaceOperation,
        owner: CompletionOwner,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.window_presets.completions.len() >= 16 {
            return Err("Workspace request queue is full".into());
        }
        self.window_presets.serial += 1;
        let id = self.window_presets.serial;
        self.window_presets.completions.insert(id, owner);
        match self
            .window_presets
            .store
            .submit(id, operation, backend.event_sink())
        {
            Ok(Some(result)) => self.finish_workspace(result, backend),
            Ok(None) => Ok(()),
            Err(error) => self.finish_workspace(
                WorkspaceCompletion {
                    id,
                    outcome: Err(error),
                },
                backend,
            ),
        }
    }

    pub(super) fn retire_workspace_requests(&mut self, owner: Option<&ModeId>) {
        self.window_presets
            .completions
            .retain(|_, pending| match pending {
                CompletionOwner::Mode {
                    owner: candidate, ..
                } => owner.is_some_and(|owner| owner != candidate),
                _ => true,
            });
    }

    pub(super) fn export_workspace(
        &mut self,
        source: String,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self
            .window_presets
            .completions
            .values()
            .any(|owner| matches!(owner, CompletionOwner::Simulator { .. }))
        {
            return Ok(());
        }
        self.submit_workspace(
            WorkspaceOperation::Export,
            CompletionOwner::Simulator { source },
            backend,
        )
    }

    pub(super) fn checkpoint_workspace(
        &mut self,
        reply: std::sync::mpsc::Sender<()>,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        self.submit_workspace(
            WorkspaceOperation::Checkpoint(reply),
            CompletionOwner::Checkpoint,
            backend,
        )
    }

    pub(super) fn finish_workspace(
        &mut self,
        completed: WorkspaceCompletion,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if let Err(error) = &completed.outcome {
            crate::report_error!(
                "workspace",
                "request={} operation=persistence: {error}",
                completed.id
            );
        }
        let Some(owner) = self.window_presets.completions.remove(&completed.id) else {
            return Ok(());
        };
        match owner {
            CompletionOwner::Mode { owner, session } => {
                // The mode checks session identity as well; never reactivate an old owner.
                if owner != self.registry.active {
                    return Ok(());
                }
                let (presets, saved, message) = match completed.outcome {
                    Ok(WorkspaceValue::Library { presets, saved }) => (presets, saved, None),
                    Err(error) => (Vec::new(), None, Some(error)),
                    _ => return Err("Unexpected workspace completion".into()),
                };
                self.dispatch_owned_to(
                    &owner,
                    ModeEvent::WindowPresets(Box::new(PresetLibraryResult {
                        session,
                        presets,
                        saved,
                        message,
                    })),
                    backend,
                )
            }
            CompletionOwner::Simulator { source } => {
                let layouts = match completed.outcome {
                    Ok(WorkspaceValue::Export(bytes)) => Ok(bytes),
                    Err(error) => Err(error),
                    _ => return Err("Unexpected workspace export completion".into()),
                };
                let url = config_handoff::url_for_workspace(&source, layouts);
                if let Err(error) = backend.open_url(&url) {
                    crate::report_error!("config-simulator", "{error}");
                } else {
                    self.last_config_simulator_open = Some(Instant::now());
                }
                Ok(())
            }
            CompletionOwner::Checkpoint => Ok(()),
        }
    }
}
