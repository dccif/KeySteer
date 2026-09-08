//! Discrete layout commands and ordinary held move/size gestures.
use super::*;
use crate::api::command::WindowScreenTarget;

impl WindowMode {
    pub(super) fn action(
        &mut self,
        action: W,
        state: KeyState,
        key: &Key,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        if action.is_held() {
            if state == KeyState::Up {
                self.held.remove(key);
                if self.held.is_empty() {
                    out.push(Command::SetFrameClock(false));
                }
            } else if self.edit.is_none()
                && !self.temporary
                && self.target.is_some()
                && !self.held.contains_key(key)
            {
                self.last_layout = None;
                if self.held.is_empty() {
                    self.group += 1;
                    out.push(Command::SetFrameClock(true));
                }
                self.held.insert(key.clone(), action);
                self.motion(None, out);
            }
            return;
        }
        if state != KeyState::Down {
            return;
        }
        self.stop_movement(out);
        if action != W::Layout {
            self.last_layout = None;
        }
        if action == W::Confirm && self.number.pending() {
            let completed = self.number.finish(&self.window_index, &self.slot_index);
            out.push(Command::CancelTimer {
                id: NUMBER_TIMER.into(),
            });
            if let Some((slot, number)) = completed {
                self.choose_number(slot, number, ctx, out);
            }
            return;
        }
        if action == W::Cancel {
            let number = self.cancel_number(out);
            let swap = self.swap_source.take().is_some();
            if number || swap {
                return;
            }
        }
        if !matches!(action, W::Navigate(_) | W::Split(_) | W::Ratio(_)) {
            self.cancel_number(out);
            self.swap_source = None;
        }
        match action {
            W::Navigate(direction) => self.edit_direction(direction, false, false, ctx, out),
            W::Split(direction) => self.edit_direction(direction, true, false, ctx, out),
            W::Ratio(direction) => self.edit_direction(direction, false, true, ctx, out),
            W::Layout => {
                if self.window_layout_double_tap() {
                    self.finish_edit(Finish::Tile, out);
                } else if self.edit.is_none() {
                    self.start_edit(false, ctx, out);
                    self.last_layout = Some(Instant::now());
                }
            }
            W::Edit => {
                if matches!(
                    self.edit.as_ref().map(|e| &e.model),
                    Some(EditModel::Quick(_))
                ) {
                    self.finish_edit(Finish::Tree, out);
                } else if self.edit.is_none() {
                    self.start_edit(true, ctx, out);
                }
            }
            W::Select => {
                if let Some(edit) = &self.edit
                    && let EditModel::Tree(tree) = &edit.model
                {
                    let windows: Vec<_> = self.visible_windows().collect();
                    let current = self.target.as_ref().map(|w| w.id).or_else(|| {
                        tree.slots()
                            .iter()
                            .find(|s| s.id == tree.selected)
                            .and_then(|s| s.window)
                    });
                    let next = current
                        .and_then(|id| windows.iter().position(|w| w.id == id))
                        .map_or(0, |i| (i + 1) % windows.len().max(1));
                    if let Some(window) = windows.get(next) {
                        let id = window.id;
                        if let Some(edit) = &mut self.edit
                            && let EditModel::Tree(tree) = &mut edit.model
                        {
                            tree.focus_window(id);
                        }
                        self.request(WindowOperation::Select(id), out);
                    }
                } else if self.edit.is_some() {
                    self.finish_edit(Finish::Cycle, out);
                } else {
                    self.request(WindowOperation::Cycle, out);
                }
            }
            W::Undo => {
                if let Some(edit) = &mut self.edit {
                    if let Some(previous) = edit.history.pop() {
                        if previous == EditModel::Quick(QuickPlacement::default()) {
                            self.finish_edit(Finish::QuickReset, out);
                            return;
                        }
                        edit.model = previous;
                        edit.dirty = true;
                        self.flush_edit(out);
                        self.rebuild_numbers();
                    } else {
                        self.status = Some("Nothing to undo in this edit".into());
                    }
                } else {
                    self.request(WindowOperation::Undo, out);
                    self.trees.clear();
                }
            }
            W::Confirm if self.edit.is_some() => self.finish_edit(Finish::Commit, out),
            W::Cancel if self.edit.is_some() => self.finish_edit(Finish::Commit, out),
            W::Exit if self.edit.is_some() => self.finish_edit(Finish::Exit, out),
            W::Exit | W::Confirm | W::Cancel => self.exit(out),
            W::Tile => {
                if self.edit.is_some() {
                    self.finish_edit(Finish::Tile, out);
                } else {
                    self.tile(out);
                }
            }
            _ if self.edit.is_some() => {}
            W::Size => self.size = !self.size,
            W::NextScreen | W::PreviousScreen | W::Maximize | W::Center => {
                self.group += 1;
                let change = match action {
                    W::NextScreen => WindowChange::Screen(WindowScreenTarget::Next),
                    W::PreviousScreen => WindowChange::Screen(WindowScreenTarget::Previous),
                    W::Maximize => WindowChange::Maximize,
                    _ => WindowChange::Center,
                };
                self.adjust(change, out);
            }
            _ => {}
        }
    }
}
