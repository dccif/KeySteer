//! Saved-region selection. Persistence and text entry belong to the host.
use super::*;
use crate::api::window_presets::{
    PresetLibraryOperation, PresetLibraryRequest, PresetLibraryResult, RegionTemplate,
    WindowTemplate,
};
pub(super) const PAGE_SIZE: usize = 6;
impl WindowSession {
    pub(super) fn open_library(&mut self, out: &mut CommandBatch) {
        self.library_open = true;
        self.library_page = 0;
        self.cancel_number(out);
        self.status = Some("Reading saved presets…".into());
        out.push(Command::WindowPresets(Box::new(PresetLibraryRequest {
            session: self.session,
            operation: PresetLibraryOperation::List,
        })));
    }
    pub(super) fn save_layout(&mut self, out: &mut CommandBatch) {
        let Some(edit) = &self.edit else { return };
        let EditModel::Tree(tree) = &edit.model else {
            return;
        };
        if !edit.ready || edit.finishing.is_some() {
            return;
        }
        let operation = PresetLibraryOperation::Save {
            template: WindowTemplate::Layout(RegionTemplate::from_tree(tree)),
            window_count: tree.slots().iter().filter(|s| s.window.is_some()).count(),
        };
        self.status = Some("Save preset · enter an optional name".into());
        self.save_pending = true;
        out.push(Command::WindowPresets(Box::new(PresetLibraryRequest {
            session: self.session,
            operation,
        })));
    }
    pub(super) fn library_result(
        &mut self,
        result: PresetLibraryResult,
        ctx: &HostContext<'_>,
    ) -> CommandBatch {
        if result.session != self.session {
            return CommandBatch::new();
        }
        if result.message.is_none() || !result.presets.is_empty() {
            self.saved_presets = result.presets.into_iter().collect();
            self.library_index = NumberIndex::new(self.saved_presets.iter().map(|l| l.id));
            self.library_page = self
                .library_page
                .min(self.saved_presets.len().saturating_sub(1) / PAGE_SIZE);
        }
        self.status = result.message.or_else(|| {
            result.saved.and_then(|id| {
                self.saved_presets
                    .iter()
                    .find(|l| l.id == id)
                    .map(|l| format!("Saved · {}", l.name()))
            })
        });
        let mut out = CommandBatch::new();
        if std::mem::take(&mut self.save_pending)
            && let Some(target) = &self.target
        {
            self.request(WindowOperation::Select(target.id), &mut out);
        }
        out.push(ctx.present(self.view()));
        out
    }
    fn restore_saved(&mut self, number: u32, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        let Some(saved) = self.saved_presets.iter().find(|l| l.id == number).cloned() else {
            return;
        };
        if self.deleting_presets {
            self.delete_selection = Some(saved);
            self.cancel_number(out);
            self.status = None;
            return;
        }
        if self.restore_pending {
            return;
        }
        if let WindowTemplate::Tabs(template) = saved.template {
            self.tabs.restore = Some(tabs::Restore {
                template,
                count: saved.window_count,
                members: Vec::new(),
            });
            self.tabs.restoring = None;
            self.cancel_number(out);
            out.push(Command::SwitchMode(ModeId::window_tab()));
            return;
        }
        self.restore_pending = true;
        self.finished = false;
        self.pending_template = Some(saved);
        self.cancel_number(out);
        if self.edit.is_some() {
            self.finish_edit(Finish::Tree, out);
        } else {
            self.start_edit(true, ctx, out);
        }
    }
    pub(super) fn library_event(
        &mut self,
        event: &ModeEvent,
        ctx: &HostContext<'_>,
    ) -> Option<CommandBatch> {
        if !self.library_open {
            return None;
        }
        let mut out = CommandBatch::new();
        match event {
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                ..
            } => {
                if let Some(digit) = key.as_char().filter(char::is_ascii_digit) {
                    self.delete_selection = None;
                    for (_, number) in
                        self.number
                            .digit(digit, &self.library_index, &self.slot_index)
                    {
                        self.restore_saved(number, ctx, &mut out);
                    }
                    if self.library_open
                        && self
                            .number
                            .needs_timer(&self.library_index, &self.slot_index)
                    {
                        out.push(Command::SetTimer {
                            id: NUMBER_TIMER.into(),
                            delay: Duration::from_millis(self.settings.number_timeout_ms),
                            repeating: false,
                        });
                    }
                } else if key.as_str() == "page_down" {
                    self.library_page = (self.library_page + 1)
                        .min(self.saved_presets.len().saturating_sub(1) / PAGE_SIZE);
                } else if key.as_str() == "page_up" {
                    self.library_page = self.library_page.saturating_sub(1);
                }
            }
            ModeEvent::Timer { id, .. } if id == NUMBER_TIMER => {
                if let Some((_, number)) = self.number.finish(&self.library_index, &self.slot_index)
                {
                    self.restore_saved(number, ctx, &mut out);
                }
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } => {
                if let Binding::Window(W::DeletePreset) = binding.as_ref() {
                    if !self.restore_pending {
                        self.deleting_presets = !self.deleting_presets;
                        self.delete_selection = None;
                        self.cancel_number(&mut out);
                        self.status = None;
                    }
                } else if let Binding::Window(W::Confirm) = binding.as_ref() {
                    if let Some(expected) = self.delete_selection.take() {
                        self.status = Some("Deleting preset…".into());
                        out.push(Command::WindowPresets(Box::new(PresetLibraryRequest {
                            session: self.session,
                            operation: PresetLibraryOperation::Delete { expected },
                        })));
                        return Some(out);
                    }
                    if let Some((_, number)) =
                        self.number.finish(&self.library_index, &self.slot_index)
                    {
                        self.restore_saved(number, ctx, &mut out);
                    }
                }
            }
            _ => return None,
        }
        if !out
            .iter()
            .any(|command| matches!(command, Command::WindowPresets(_)))
        {
            out.push(ctx.present(self.view()));
        }
        Some(out)
    }
}
