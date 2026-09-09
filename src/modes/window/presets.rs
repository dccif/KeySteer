//! Saved-region selection. Persistence and text entry belong to the host.
use super::*;
use crate::api::window_presets::{
    LayoutLibraryOperation, LayoutLibraryRequest, LayoutLibraryResult, RegionTemplate,
};
pub(super) const PAGE_SIZE: usize = 6;
impl WindowMode {
    pub(super) fn open_library(&mut self, out: &mut CommandBatch) {
        self.library_open = true;
        self.library_page = 0;
        self.cancel_number(out);
        self.status = Some("Reading saved layouts…".into());
        out.push(Command::WindowLayouts(Box::new(LayoutLibraryRequest {
            session: self.session,
            operation: LayoutLibraryOperation::List,
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
        let operation = LayoutLibraryOperation::Save {
            regions: RegionTemplate::from_tree(tree),
            window_count: tree.slots().iter().filter(|s| s.window.is_some()).count(),
        };
        self.status = Some("Save layout · enter an optional note".into());
        self.save_pending = true;
        out.push(Command::WindowLayouts(Box::new(LayoutLibraryRequest {
            session: self.session,
            operation,
        })));
    }
    pub(super) fn library_result(
        &mut self,
        result: LayoutLibraryResult,
        ctx: &HostContext<'_>,
    ) -> CommandBatch {
        if result.session != self.session {
            return CommandBatch::new();
        }
        if result.message.is_none() || !result.layouts.is_empty() {
            self.saved_layouts = result.layouts;
            self.library_index = NumberIndex::new(self.saved_layouts.iter().map(|l| l.id));
        }
        self.status = result.message.or_else(|| {
            result.saved.and_then(|id| {
                self.saved_layouts
                    .iter()
                    .find(|l| l.id == id)
                    .map(|l| format!("Saved {id} · {}", l.name()))
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
        let Some(saved) = self.saved_layouts.iter().find(|l| l.id == number).cloned() else {
            return;
        };
        self.library_open = false;
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
                        .min(self.saved_layouts.len().saturating_sub(1) / PAGE_SIZE);
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
            } => match binding.as_ref() {
                Binding::Window(W::SavedLayouts) => self.open_library(&mut out),
                Binding::Window(W::Cancel) => {
                    self.library_open = false;
                    self.cancel_number(&mut out);
                    self.status = None;
                }
                Binding::Window(W::Exit) => {
                    self.library_open = false;
                    self.exit(&mut out);
                    return Some(out);
                }
                Binding::Window(W::Confirm) => {
                    if let Some((_, number)) =
                        self.number.finish(&self.library_index, &self.slot_index)
                    {
                        self.restore_saved(number, ctx, &mut out);
                    }
                }
                _ => {}
            },
            _ => return None,
        }
        if !out
            .iter()
            .any(|command| matches!(command, Command::WindowLayouts(_)))
        {
            out.push(ctx.present(self.view()));
        }
        Some(out)
    }
}
