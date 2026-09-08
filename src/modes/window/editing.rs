//! Live-edit transactions and latest-desired-layout scheduling.
use super::*;
use crate::api::Direction;
use crate::api::window_layout::placed_rect;

impl WindowMode {
    pub(super) fn start_edit(&mut self, tree: bool, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        let Some(target) = &self.target else {
            self.status = Some("Select a window first".into());
            return;
        };
        self.screen = target.screen;
        let Some(screen) = ctx.screens.get(self.screen) else {
            return;
        };
        if !tree && (!target.resizable || target.fullscreen) {
            self.status = Some("This window cannot be resized".into());
            return;
        }
        // Build once, from the worker's fresh inventory and constraints.
        let targets = if tree { Vec::new() } else { vec![target.id] };
        let model = if tree {
            EditModel::Tree(LayoutTree::import(&[], None, screen.work_area))
        } else {
            EditModel::Quick(QuickPlacement::default())
        };
        self.group += 1;
        let transaction = self.group;
        self.edit = Some(LiveEdit {
            transaction,
            screen: self.screen,
            accepted: model.clone(),
            model,
            history: Vec::new(),
            minimums: BTreeMap::new(),
            gap_scale: 1.0,
            ready: false,
            revision: 0,
            in_flight: None,
            dirty: false,
            finishing: None,
            ending: false,
            deferred: Vec::new(),
        });
        self.status = Some(
            if tree {
                "Reading layout constraints…"
            } else {
                "Quick layout · choose a direction"
            }
            .into(),
        );
        self.swap_source = None;
        self.request(
            WindowOperation::BeginEdit {
                transaction,
                targets,
                screen: tree.then_some(self.screen),
                group: self.group,
            },
            out,
        );
        self.rebuild_numbers();
    }

    pub(super) fn flush_edit(&mut self, out: &mut CommandBatch) {
        let Some(edit) = &mut self.edit else { return };
        if !edit.ready || edit.in_flight.is_some() || edit.ending || self.temporary {
            return;
        }
        if edit.dirty {
            let placements = match &edit.model {
                EditModel::Quick(quick) => self
                    .target
                    .as_ref()
                    .map(|w| vec![(w.id, quick.rect())])
                    .unwrap_or_default(),
                EditModel::Tree(tree) => tree
                    .slots()
                    .into_iter()
                    .filter_map(|s| s.window.map(|id| (id, s.rect)))
                    .collect(),
            };
            edit.revision += 1;
            edit.in_flight = Some((edit.revision, edit.model.clone()));
            edit.dirty = false;
            let operation = WindowOperation::ApplyLayout {
                transaction: edit.transaction,
                revision: edit.revision,
                screen: edit.screen,
                placements,
                gap: self.settings.gap,
                strict: matches!(edit.model, EditModel::Tree(_)),
            };
            self.request(operation, out);
        } else if let Some(finish) = edit.finishing {
            edit.ending = true;
            let operation = WindowOperation::EndEdit {
                transaction: edit.transaction,
                commit: !matches!(finish, Finish::Cancel | Finish::QuickReset),
            };
            self.request(operation, out);
        }
    }

    pub(super) fn finish_edit(&mut self, finish: Finish, out: &mut CommandBatch) {
        self.last_layout = None;
        self.cancel_number(out);
        self.swap_source = None;
        if let Some(edit) = &mut self.edit {
            if edit.ending {
                return;
            }
            edit.finishing = Some(finish);
            if matches!(finish, Finish::Cancel | Finish::QuickReset) {
                edit.ending = true;
                edit.dirty = false;
                edit.in_flight = None;
                let operation = WindowOperation::EndEdit {
                    transaction: edit.transaction,
                    commit: false,
                };
                self.request(operation, out);
                self.result = self.request - 1;
                self.refresh_pending = None;
            } else {
                self.flush_edit(out);
            }
        }
    }

    pub(super) fn tile(&mut self, out: &mut CommandBatch) {
        self.trees.remove(&self.screen);
        self.group += 1;
        if let Some(target) = &self.target {
            self.request(
                WindowOperation::Tile {
                    target: target.id,
                    gap: self.settings.gap,
                    group: self.group,
                },
                out,
            );
        }
    }

    pub(super) fn edit_direction(
        &mut self,
        direction: Direction,
        split: bool,
        ratio: bool,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        self.last_layout = None;
        self.swap_source = None;
        self.cancel_number(out);
        let Some(edit) = &mut self.edit else { return };
        if edit.finishing.is_some() {
            return;
        }
        let before = edit.model.clone();
        match &mut edit.model {
            EditModel::Quick(quick) => {
                if split || ratio {
                    return;
                }
                quick.step(direction);
            }
            EditModel::Tree(tree) => {
                if !edit.ready {
                    if edit.deferred.len() < 64 {
                        edit.deferred
                            .push(DeferredEdit::Direction(direction, split, ratio));
                    }
                    return;
                }
                if split {
                    tree.split(direction);
                } else if ratio {
                    tree.resize(direction, &self.settings.split_ratios);
                } else {
                    tree.navigate(direction);
                    return;
                }
                if let Some(screen) = ctx.screens.get(edit.screen)
                    && let Err(error) = tree.fit(
                        &edit.minimums,
                        screen.work_area,
                        self.settings.gap * edit.gap_scale,
                    )
                {
                    edit.model = before;
                    self.status = Some(error);
                    return;
                }
            }
        }
        if edit.model != before {
            if edit.history.len() == 32 {
                edit.history.remove(0);
            }
            edit.history.push(before);
            edit.dirty = true;
            self.flush_edit(out);
        }
        self.rebuild_numbers();
    }

    pub(super) fn choose_number(
        &mut self,
        slot: bool,
        number: u32,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        self.last_layout = None;
        let id = self
            .visible_windows()
            .find(|w| self.numbers.get(&w.id) == Some(&number))
            .map(|w| w.id);
        if let Some(edit) = &mut self.edit
            && let EditModel::Tree(tree) = &mut edit.model
        {
            if !edit.ready {
                if edit.deferred.len() < 64 {
                    edit.deferred.push(DeferredEdit::Number(slot, number));
                }
                return;
            }
            if edit.finishing.is_some() {
                return;
            }
            let before = EditModel::Tree(tree.clone());
            let changed = if slot {
                if let Some(source) = self.swap_source.take() {
                    tree.move_window(source, number)
                } else {
                    tree.focus_slot(number);
                    false
                }
            } else if let Some(id) = id {
                if let Some(source) = self.swap_source.take() {
                    if source == id {
                        false
                    } else {
                        tree.swap_windows(source, id)
                    }
                } else if tree.focus_window(id) {
                    self.swap_source = Some(id);
                    self.request(WindowOperation::Select(id), out);
                    return;
                } else {
                    self.status =
                        Some("Window is outside this tree; layout membership is unchanged".into());
                    self.request(WindowOperation::Select(id), out);
                    return;
                }
            } else {
                return;
            };
            if changed {
                if let Some(screen) = ctx.screens.get(edit.screen)
                    && let Err(error) = tree.fit(
                        &edit.minimums,
                        screen.work_area,
                        self.settings.gap * edit.gap_scale,
                    )
                {
                    edit.model = before;
                    self.status = Some(error);
                    return;
                }
                if edit.history.len() == 32 {
                    edit.history.remove(0);
                }
                edit.history.push(before);
                edit.dirty = true;
                self.flush_edit(out);
            }
        } else if !slot && let Some(id) = id {
            if self.edit.is_some() {
                self.finish_edit(Finish::Select(id), out);
            } else {
                self.request(WindowOperation::Select(id), out);
            }
        }
    }

    pub(super) fn edit_result(
        &mut self,
        feedback: &WindowEditResult,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        match feedback {
            WindowEditResult::Started {
                transaction,
                minimums,
                gap_scale,
                ..
            } => {
                let fresh = if self
                    .edit
                    .as_ref()
                    .is_some_and(|e| matches!(e.model, EditModel::Tree(_)))
                {
                    ctx.screens.get(self.screen).map(|screen| {
                        let windows: Vec<_> = self
                            .visible_windows()
                            .filter(|w| minimums.iter().any(|(id, _)| *id == w.id))
                            .cloned()
                            .collect();
                        let mut cached = self.trees.get(&self.screen).cloned();
                        if let Some(tree) = &mut cached {
                            tree.retain_windows(&windows.iter().map(|w| w.id).collect::<Vec<_>>());
                            let slots = tree.slots();
                            if !windows.iter().all(|w| {
                                slots
                                    .iter()
                                    .find(|s| s.window == Some(w.id))
                                    .is_some_and(|s| {
                                        let expected = placed_rect(
                                            screen.work_area,
                                            s.rect,
                                            self.settings.gap * *gap_scale,
                                        );
                                        (expected.x - w.bounds.x).abs() < 2.0
                                            && (expected.y - w.bounds.y).abs() < 2.0
                                            && (expected.width - w.bounds.width).abs() < 2.0
                                            && (expected.height - w.bounds.height).abs() < 2.0
                                    })
                            }) {
                                cached = None;
                            }
                        }
                        cached.unwrap_or_else(|| {
                            LayoutTree::import(
                                &windows,
                                self.target.as_ref().map(|w| w.id),
                                screen.work_area,
                            )
                        })
                    })
                } else {
                    None
                };
                let Some(edit) = &mut self.edit else { return };
                if edit.transaction != *transaction || edit.ending {
                    return;
                }
                edit.minimums = minimums.iter().copied().collect();
                edit.gap_scale = *gap_scale;
                edit.ready = true;
                if let Some(tree) = fresh {
                    edit.model = EditModel::Tree(tree);
                }
                if let EditModel::Tree(tree) = &mut edit.model {
                    tree.retain_windows(&minimums.iter().map(|(id, _)| *id).collect::<Vec<_>>());
                    let fit = ctx
                        .screens
                        .get(edit.screen)
                        .ok_or_else(|| "Display is unavailable".to_string())
                        .and_then(|screen| {
                            tree.fit(
                                &edit.minimums,
                                screen.work_area,
                                self.settings.gap * edit.gap_scale,
                            )
                        });
                    if let Err(error) = fit {
                        self.status = Some(error);
                        self.finish_edit(Finish::Cancel, out);
                        return;
                    }
                    edit.accepted = edit.model.clone();
                    edit.dirty = true;
                    let excluded = self
                        .inventory
                        .values()
                        .filter(|w| w.screen == self.screen && (!w.resizable || w.fullscreen))
                        .count();
                    self.status = (excluded > 0).then(|| {
                        format!("{excluded} fixed-size/fullscreen windows remain outside the tree")
                    });
                }
                let deferred = std::mem::take(&mut edit.deferred);
                let finishing = edit.finishing.take();
                for action in deferred {
                    match action {
                        DeferredEdit::Direction(direction, split, ratio) => {
                            self.edit_direction(direction, split, ratio, ctx, out)
                        }
                        DeferredEdit::Number(slot, number) => {
                            self.choose_number(slot, number, ctx, out)
                        }
                    }
                }
                if let Some(edit) = &mut self.edit {
                    edit.finishing = finishing;
                }
                self.flush_edit(out);
            }
            WindowEditResult::Applied {
                transaction,
                revision,
                accepted,
                minimums,
            } => {
                let Some(edit) = &mut self.edit else { return };
                if edit.transaction != *transaction {
                    return;
                }
                let Some((pending, model)) = edit.in_flight.take() else {
                    return;
                };
                if pending != *revision {
                    edit.in_flight = Some((pending, model));
                    return;
                }
                if *accepted {
                    edit.accepted = model;
                } else {
                    edit.model = edit.accepted.clone();
                    edit.dirty = false;
                    edit.history.clear();
                    if !minimums.is_empty() {
                        edit.minimums = minimums.iter().copied().collect();
                    }
                    if *revision == 1 && matches!(edit.model, EditModel::Tree(_)) {
                        self.finish_edit(Finish::Cancel, out);
                        return;
                    }
                }
                self.flush_edit(out);
            }
            WindowEditResult::Ended {
                transaction,
                committed,
            } => {
                if self
                    .edit
                    .as_ref()
                    .is_none_or(|edit| edit.transaction != *transaction)
                {
                    return;
                }
                let Some(edit) = self.edit.take() else { return };
                if *committed && let EditModel::Tree(tree) = edit.model {
                    self.trees.insert(edit.screen, tree);
                }
                let finish = edit.finishing.unwrap_or(Finish::Cancel);
                if !committed
                    && matches!(
                        finish,
                        Finish::Tree
                            | Finish::Tile
                            | Finish::Select(_)
                            | Finish::Cycle
                            | Finish::Commit
                    )
                {
                    self.rebuild_numbers();
                    return;
                }
                match finish {
                    Finish::Tree => self.start_edit(true, ctx, out),
                    Finish::QuickReset => self.start_edit(false, ctx, out),
                    Finish::Select(id) => {
                        self.resume_quick = true;
                        self.request(WindowOperation::Select(id), out);
                    }
                    Finish::Cycle => {
                        self.resume_quick = true;
                        self.request(WindowOperation::Cycle, out);
                    }
                    Finish::Tile => self.tile(out),
                    Finish::Exit => self.exit(out),
                    Finish::Commit | Finish::Cancel => {}
                }
                self.rebuild_numbers();
            }
        }
    }
}
