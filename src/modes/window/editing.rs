//! Live-edit transactions and latest-desired-layout scheduling.
use super::*;
use crate::api::Direction;
use crate::api::window_layout::placed_rect;

impl WindowSession {
    pub(super) fn start_edit(&mut self, tree: bool, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        let target = self.target.as_ref();
        if target.is_none() && !tree {
            self.status = Some("Select a window first".into());
            return;
        }
        if let Some(target) = target {
            self.screen = target.screen;
        }
        let Some(screen) = ctx.screens.get(self.screen) else {
            self.restore_pending = false;
            self.pending_template = None;
            self.status = Some("Display is unavailable".into());
            return;
        };
        if !tree && target.is_some_and(|target| !target.resizable || target.fullscreen) {
            self.status = Some("This window cannot be resized".into());
            return;
        }
        // Build once, from the worker's fresh inventory and constraints.
        let targets = if tree {
            Vec::new()
        } else {
            target.map(|target| vec![target.id]).unwrap_or_default()
        };
        let model = if tree {
            EditModel::Tree(LayoutTree::import(&[], None, screen.work_area))
        } else {
            EditModel::Quick(QuickPlacement::default())
        };
        self.group += 1;
        let transaction = self.group;
        self.edit = Some(LiveEdit {
            additional_trees: BTreeMap::new(),
            transaction,
            screen: self.screen,
            accepted: model.clone(),
            model,
            history: Vec::new(),
            redo: Vec::new(),
            divider_gesture: false,
            entry_layout: tree,
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
                    .map(|w| vec![(w.id, quick.rect_with(&self.settings.split_ratios))])
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
                additional_screens: edit
                    .additional_trees
                    .iter()
                    .map(|(screen, tree)| crate::api::window::WindowScreenLayout {
                        screen: *screen,
                        placements: tree
                            .slots()
                            .into_iter()
                            .filter_map(|s| s.window.map(|id| (id, s.rect)))
                            .collect(),
                    })
                    .collect(),
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
                commit: !matches!(
                    finish,
                    Finish::Cancel | Finish::QuickReset | Finish::TreeReset
                ),
            };
            self.request(operation, out);
        }
    }

    pub(super) fn finish_edit(&mut self, finish: Finish, out: &mut CommandBatch) {
        self.cancel_number(out);
        self.swap_source = None;
        if let Some(edit) = &mut self.edit {
            if edit.ending {
                if matches!(finish, Finish::Transition) {
                    edit.finishing = Some(finish);
                }
                return;
            }
            edit.finishing = Some(finish);
            if matches!(
                finish,
                Finish::Cancel | Finish::QuickReset | Finish::TreeReset
            ) {
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

    pub(super) fn request_history(&mut self, redo: bool, reopen: bool, out: &mut CommandBatch) {
        self.group += 1;
        self.trees.clear();
        self.request(
            if redo {
                WindowOperation::Redo
            } else {
                WindowOperation::Undo
            },
            out,
        );
        if reopen {
            self.reopen_edit = Some(self.request);
        }
    }

    pub(super) fn request_initial(&mut self, reopen: bool, out: &mut CommandBatch) {
        self.group += 1;
        self.trees.clear();
        self.request(WindowOperation::ResetInitial { group: self.group }, out);
        if reopen {
            self.reopen_edit = Some(self.request);
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
                quick.step_with(direction, &self.settings.split_ratios);
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
                    if let Some(screen) = ctx.screens.get(edit.screen) {
                        tree.resize_region_by(
                            direction,
                            self.settings.resize_step * edit.gap_scale,
                            screen.work_area,
                            &edit.minimums,
                            self.settings.gap * edit.gap_scale,
                        );
                    }
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
        if edit.model != before && (split || ratio || matches!(edit.model, EditModel::Quick(_))) {
            edit.remember(before);
            edit.dirty = true;
            self.flush_edit(out);
        }
        self.rebuild_numbers();
    }

    pub(super) fn divider_motion(
        &mut self,
        seconds: Option<f64>,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        if self.temporary || self.held.is_empty() {
            return;
        }
        let Some(edit) = &mut self.edit else { return };
        if !edit.ready || edit.finishing.is_some() {
            return;
        }
        let Some(screen) = ctx.screens.get(edit.screen) else {
            return;
        };
        let EditModel::Tree(tree) = &mut edit.model else {
            return;
        };
        let before = tree.clone();
        let amount = seconds.map_or(self.settings.resize_step, |s| {
            s * self.settings.resize_speed
        }) * edit.gap_scale;
        let mut dx: f64 = 0.0;
        let mut dy: f64 = 0.0;
        for action in self.held.values() {
            if let W::Ratio(direction) = action {
                let (x, y) = direction.delta();
                dx += x;
                dy += y;
            }
        }
        let mut found = false;
        if dx != 0.0 {
            found |= tree.resize_region_by(
                if dx < 0.0 {
                    Direction::Left
                } else {
                    Direction::Right
                },
                amount,
                screen.work_area,
                &edit.minimums,
                self.settings.gap * edit.gap_scale,
            );
        }
        if dy != 0.0 {
            found |= tree.resize_region_by(
                if dy < 0.0 {
                    Direction::Up
                } else {
                    Direction::Down
                },
                amount,
                screen.work_area,
                &edit.minimums,
                self.settings.gap * edit.gap_scale,
            );
        }
        if let Err(error) = tree.fit(
            &edit.minimums,
            screen.work_area,
            self.settings.gap * edit.gap_scale,
        ) {
            *tree = before;
            self.status = Some(error);
            return;
        }
        if *tree == before {
            if dx != 0.0 || dy != 0.0 {
                self.status = Some(
                    if found {
                        "Region reached the neighboring-window size limit"
                    } else {
                        "Region spans the screen on this axis"
                    }
                    .into(),
                );
            }
            return;
        }
        if !edit.divider_gesture {
            edit.remember(EditModel::Tree(before));
            edit.divider_gesture = true;
        }
        self.status = None;
        edit.dirty = true;
        self.flush_edit(out);
    }

    pub(super) fn remove_region(&mut self, ctx: &HostContext<'_>, out: &mut CommandBatch) {
        let Some(edit) = &mut self.edit else { return };
        if !edit.ready || edit.finishing.is_some() {
            return;
        }
        let EditModel::Tree(tree) = &mut edit.model else {
            return;
        };
        let before = tree.clone();
        if !tree.remove_selected() {
            self.status = Some("Keep at least one region".into());
            return;
        }
        if let Some(screen) = ctx.screens.get(edit.screen)
            && let Err(error) = tree.fit(
                &edit.minimums,
                screen.work_area,
                self.settings.gap * edit.gap_scale,
            )
        {
            *tree = before;
            self.status = Some(error);
            return;
        }
        edit.remember(EditModel::Tree(before));
        edit.dirty = true;
        self.numbered_slots = 0;
        self.rebuild_numbers();
        self.flush_edit(out);
    }

    pub(super) fn choose_number(
        &mut self,
        slot: bool,
        number: u32,
        ctx: &HostContext<'_>,
        out: &mut CommandBatch,
    ) {
        let id = self
            .visible_windows()
            .find(|w| self.numbers.get(&w.id) == Some(&number))
            .map(|w| w.id);
        let layout_id = id.map(|id| self.tabs.state.representative(id));
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
                    if !tree.focus_slot(number) {
                        return;
                    }
                    let selected = tree.slots().into_iter().find(|region| region.id == number);
                    if let Some(region) = selected {
                        if let Some(id) = region.window {
                            self.request(WindowOperation::Select(id), out);
                        } else if let Some(screen) = ctx.screens.get(edit.screen) {
                            out.push(Command::warp_to(
                                placed_rect(
                                    screen.work_area,
                                    region.rect,
                                    self.settings.gap * edit.gap_scale,
                                )
                                .center(),
                            ));
                        }
                    }
                    return;
                }
            } else if let Some(id) = id {
                let representative = layout_id.unwrap_or(id);
                if let Some(source) = self.swap_source.take() {
                    if source == representative {
                        false
                    } else {
                        tree.swap_windows(source, representative)
                    }
                } else if tree.focus_window(representative) {
                    self.swap_source = Some(representative);
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
                edit.remember(before);
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
                screen_scales,
                ..
            } => {
                if self
                    .edit
                    .as_ref()
                    .is_none_or(|e| e.transaction != *transaction || e.ending)
                {
                    return;
                }
                let saved = if self
                    .edit
                    .as_ref()
                    .is_some_and(|e| matches!(e.model, EditModel::Tree(_)))
                {
                    self.pending_template.take()
                } else {
                    None
                };
                let fresh = if let Some(saved) = &saved {
                    // BeginEdit minimums preserve the backend's fresh front-to-back
                    // activity order; stable on-screen numbers do not change it.
                    let windows: Vec<_> = minimums
                        .iter()
                        .filter_map(|(id, _)| self.inventory.get(id))
                        .filter(|w| w.screen == self.screen)
                        .cloned()
                        .collect();
                    match saved.instantiate_layout(&windows) {
                        Ok(tree) => Some(tree),
                        Err(error) => {
                            self.status = Some(error);
                            self.restore_pending = false;
                            None
                        }
                    }
                } else if self
                    .edit
                    .as_ref()
                    .is_some_and(|e| matches!(e.model, EditModel::Tree(_)))
                {
                    ctx.screens.get(self.screen).map(|screen| {
                        let windows: Vec<_> = self
                            .visible_windows()
                            .filter(|w| w.screen == self.screen)
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
                        if self.edit.as_ref().is_some_and(|edit| !edit.entry_layout) {
                            return LayoutTree::import(
                                &windows,
                                self.target
                                    .as_ref()
                                    .map(|w| self.tabs.state.representative(w.id)),
                                screen.work_area,
                            );
                        }
                        cached.unwrap_or_else(|| {
                            LayoutTree::automatic(
                                &windows,
                                self.target
                                    .as_ref()
                                    .map(|w| self.tabs.state.representative(w.id)),
                                screen.work_area,
                                &minimums.iter().copied().collect(),
                                self.settings.gap * *gap_scale,
                            )
                            .unwrap_or_else(|_| {
                                LayoutTree::import(
                                    &windows,
                                    self.target
                                        .as_ref()
                                        .map(|w| self.tabs.state.representative(w.id)),
                                    screen.work_area,
                                )
                            })
                        })
                    })
                } else {
                    None
                };
                let mut additional_trees = BTreeMap::new();
                let mut additional_error = None;
                if self.settings.all_screens
                    && self
                        .edit
                        .as_ref()
                        .is_some_and(|e| e.entry_layout && matches!(e.model, EditModel::Tree(_)))
                {
                    for (index, screen) in ctx.screens.iter().enumerate() {
                        if index == self.screen {
                            continue;
                        }
                        let windows: Vec<_> = self
                            .visible_windows()
                            .filter(|w| {
                                w.screen == index && minimums.iter().any(|(id, _)| *id == w.id)
                            })
                            .cloned()
                            .collect();
                        if windows.is_empty() {
                            continue;
                        }
                        let layout = saved
                            .as_ref()
                            .map_or_else(
                                || {
                                    LayoutTree::automatic(
                                        &windows,
                                        None,
                                        screen.work_area,
                                        &minimums.iter().copied().collect(),
                                        self.settings.gap
                                            * screen_scales.get(index).copied().unwrap_or(1.0),
                                    )
                                },
                                |saved| saved.instantiate_layout(&windows),
                            )
                            .and_then(|mut tree| {
                                tree.fit(
                                    &minimums.iter().copied().collect(),
                                    screen.work_area,
                                    self.settings.gap
                                        * screen_scales.get(index).copied().unwrap_or(1.0),
                                )?;
                                Ok(tree)
                            });
                        match layout {
                            Ok(tree) => {
                                additional_trees.insert(index, tree);
                            }
                            Err(error) => {
                                additional_error = Some(error);
                                self.restore_pending = false;
                                if let Some(edit) = &mut self.edit {
                                    edit.entry_layout = false;
                                }
                                additional_trees.clear();
                                break;
                            }
                        }
                    }
                }
                let Some(edit) = &mut self.edit else { return };
                edit.additional_trees = additional_trees;
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
                    let fit_error = fit.err().or(additional_error);
                    if fit_error.is_some() {
                        self.restore_pending = false;
                        edit.additional_trees.clear();
                    }
                    edit.accepted = edit.model.clone();
                    edit.entry_layout &=
                        fit_error.is_none() && (saved.is_none() || self.restore_pending);
                    edit.dirty = edit.entry_layout;
                    let excluded = self
                        .inventory
                        .values()
                        .filter(|w| w.screen == self.screen && (!w.resizable || w.fullscreen))
                        .count();
                    self.status =
                        fit_error.or_else(|| {
                            (excluded > 0).then(|| {
                        format!("{excluded} fixed-size/fullscreen windows remain outside the tree")
                    })
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
                self.rebuild_numbers();
                if self.number.slot && !self.number.prefix.is_empty() {
                    if self
                        .number
                        .needs_timer(&self.window_index, &self.slot_index)
                    {
                        out.push(Command::SetTimer {
                            id: NUMBER_TIMER.into(),
                            delay: Duration::from_millis(self.settings.number_timeout_ms),
                            repeating: false,
                        });
                    } else if let Some((slot, number)) =
                        self.number.finish(&self.window_index, &self.slot_index)
                    {
                        self.choose_number(slot, number, ctx, out);
                    }
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
                    self.trees.append(&mut edit.additional_trees);
                } else {
                    edit.additional_trees.clear();
                    edit.model = edit.accepted.clone();
                    edit.dirty = false;
                    edit.history.clear();
                    edit.redo.clear();
                    self.status.get_or_insert_with(|| {
                        "Layout could not be applied; choose a layout to retry".into()
                    });
                    if !minimums.is_empty() {
                        edit.minimums = minimums.iter().copied().collect();
                    }
                    self.numbered_slots = 0;
                }
                self.rebuild_numbers();
                self.flush_edit(out);
                if self.restore_pending {
                    self.restore_pending = false;
                    if *accepted && self.pending_transition.is_none() {
                        out.push(Command::FinishMode {
                            cause: crate::api::FinishCause::Explicit,
                        });
                    }
                }
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
                if !committed && self.restore_pending {
                    self.restore_pending = false;
                    self.pending_template = None;
                    self.status.get_or_insert_with(|| {
                        "Layout could not be restored; choose a layout to retry".into()
                    });
                }
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
                            | Finish::Cycle { .. }
                            | Finish::Commit
                            | Finish::History { .. }
                            | Finish::ResetInitial
                    )
                {
                    self.restore_pending = false;
                    self.pending_template = None;
                    self.rebuild_numbers();
                    return;
                }
                match finish {
                    Finish::Tree => self.start_edit(true, ctx, out),
                    Finish::QuickReset => {
                        self.start_edit(false, ctx, out);
                        if let Some(next) = &mut self.edit {
                            next.redo = edit.redo;
                        }
                    }
                    Finish::TreeReset => {
                        let reset_redo = edit.redo;
                        self.start_edit(true, ctx, out);
                        if let Some(edit) = &mut self.edit {
                            edit.entry_layout = false;
                            edit.redo = reset_redo;
                        }
                    }
                    Finish::Select(id) => {
                        self.resume_quick = true;
                        self.request(WindowOperation::Select(id), out);
                    }
                    Finish::Cycle { backwards } => {
                        self.resume_quick = true;
                        self.request(
                            if backwards {
                                WindowOperation::CyclePrevious
                            } else {
                                WindowOperation::Cycle
                            },
                            out,
                        );
                    }
                    Finish::Tile => self.tile(out),
                    Finish::History { redo } => self.request_history(redo, true, out),
                    Finish::ResetInitial => self.request_initial(true, out),
                    Finish::Transition => {
                        if let Some(target) = self.pending_transition.take() {
                            out.push(Command::SwitchMode(target));
                        }
                    }
                    Finish::Commit | Finish::Cancel => {}
                }
                self.rebuild_numbers();
            }
        }
    }
}
