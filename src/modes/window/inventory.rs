//! Owned inventory delivery and change detection, outside the keyboard hot path.
use super::*;
use crate::api::window::WindowResult;

impl WindowSession {
    pub(super) fn window_result(
        &mut self,
        mut result: WindowResult,
        ctx: &HostContext<'_>,
    ) -> CommandBatch {
        let mut out = CommandBatch::new();
        if result.session != self.session || result.id <= self.result {
            return out;
        }
        self.result = result.id;
        if let Some(tabs) = result.tabs.take() {
            if self.tabs.state != tabs {
                self.numbers = tabs.numbers.iter().copied().collect();
                self.next_number = tabs.numbers.iter().map(|(_, n)| *n).max().unwrap_or(0) + 1;
                self.inventory_dirty = true;
            }
            self.tabs.state = tabs;
        }
        let had_target = self.target.is_some();
        let had_inventory = result.windows.is_some();
        let mut changed = false;
        let mut removed = false;
        for id in &result.closed {
            self.numbers.remove(id);
            removed |= self.inventory.remove(id).is_some();
        }
        self.inventory_dirty |= removed;
        changed |= removed;
        if self.refresh_pending == Some(result.id) {
            self.refresh_pending = None;
        }
        if let Some(windows) = result.windows.take() {
            let full = result.edit.is_none()
                || matches!(
                    result.edit.as_deref(),
                    Some(WindowEditResult::Started {
                        full_inventory: true,
                        ..
                    })
                );
            let same_members = self.inventory.len() == windows.len()
                && windows.iter().all(|w| self.inventory.contains_key(&w.id));
            if full && !same_members {
                removed |= self
                    .inventory
                    .keys()
                    .any(|id| !windows.iter().any(|w| w.id == *id));
                self.inventory = windows.into_iter().map(|w| (w.id, w)).collect();
                self.inventory_dirty = true;
                changed = true;
            } else {
                for window in windows {
                    match self.inventory.get_mut(&window.id) {
                        Some(old) if *old == window => {}
                        Some(old) => {
                            self.inventory_dirty |=
                                old.screen != window.screen || old.minimized != window.minimized;
                            *old = window;
                            changed = true;
                        }
                        None => {
                            self.inventory.insert(window.id, window);
                            self.inventory_dirty = true;
                            changed = true;
                        }
                    }
                }
            }
        }
        if self.target != result.target {
            self.target = result.target;
            changed = true;
        }
        if let Some(target) = &self.target {
            if self.inventory.get(&target.id) != Some(target) {
                self.inventory_dirty |= self
                    .inventory
                    .get(&target.id)
                    .is_none_or(|w| w.screen != target.screen || w.minimized != target.minimized);
                self.inventory.insert(target.id, target.clone());
                changed = true;
            }
            if self.edit.is_none() {
                self.screen = target.screen;
            }
        }
        // Polling must not erase operation errors or the pending selection hint.
        let ended = matches!(result.edit.as_deref(), Some(WindowEditResult::Ended { .. }));
        if (result.message.is_some()
            || !had_inventory
            || result.edit.is_some()
            || self.tabs.in_flight == Some(result.id))
            && !(ended && result.message.is_none())
        {
            if self.status != result.message {
                changed = true;
                self.status = result.message;
            }
        } else if result.id == 1 && self.status.take().is_some() {
            changed = true;
        }
        if let Some(pointer) = result.pointer
            && !self.temporary
        {
            out.push(Command::warp_to(pointer));
        }

        // A geometry-only acknowledgement never walks/clones the undo trees.
        if removed {
            let live: Vec<_> = self.inventory.keys().copied().collect();
            if let Some(edit) = &mut self.edit {
                for model in std::iter::once(&mut edit.model)
                    .chain(std::iter::once(&mut edit.accepted))
                    .chain(edit.in_flight.iter_mut().map(|(_, model)| model))
                    .chain(edit.history.iter_mut())
                    .chain(edit.redo.iter_mut())
                {
                    if let EditModel::Tree(tree) = model {
                        tree.retain_windows(&live);
                    }
                }
            }
            for tree in self.trees.values_mut() {
                tree.retain_windows(&live);
            }
            if self
                .swap_source
                .is_some_and(|id| !self.inventory.contains_key(&id))
            {
                self.swap_source = None;
            }
        }
        self.rebuild_numbers();
        if let Some(feedback) = &result.edit {
            changed |= !matches!(
                feedback.as_ref(),
                WindowEditResult::Applied { accepted: true, .. }
            );
            self.edit_result(feedback, ctx, &mut out);
        }
        if self.reopen_edit == Some(result.id) {
            self.reopen_edit = None;
            if self.edit.is_none() && matches!(self.kind, WindowKind::Quick | WindowKind::Editor) {
                self.start_edit(self.kind == WindowKind::Editor, ctx, &mut out);
                if let Some(edit) = &mut self.edit {
                    edit.entry_layout = false;
                }
                changed = true;
            }
        }
        if result.edit.is_none()
            && self.target.is_some()
            && self.edit.is_none()
            && self.pending_transition.is_none()
            && (self.resume_quick || (!had_target && self.kind == WindowKind::Quick))
        {
            self.resume_quick = false;
            self.start_edit(false, ctx, &mut out);
            changed = true;
        }
        if self.enter_pending && result.id >= 1 {
            self.enter_pending = false;
            self.enter_kind(ctx, &mut out);
            changed = true;
        }
        if self.target.is_none() && self.edit.is_none() {
            self.stop_movement(&mut out);
            if had_target {
                self.cancel_pending(&mut out);
            }
            if self.status.is_none() {
                self.status = Some("No target · choose a number or Tab".into());
                changed = true;
            }
        }
        self.rebuild_numbers();
        if self.kind == WindowKind::Tab {
            self.tab_result(result.id, ctx, &mut out);
            changed = true;
        }
        if result.id == 1 && !had_inventory {
            self.refresh(&mut out);
        }
        if changed
            && !out.iter().any(|c| {
                matches!(
                    c,
                    Command::PopMode | Command::SwitchMode(_) | Command::FinishMode { .. }
                )
            })
        {
            out.push(ctx.present(self.view()));
        }
        out
    }
}
