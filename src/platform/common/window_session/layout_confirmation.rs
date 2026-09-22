//! Incremental preparation and bounded native transactions. No write precedes full validation.
use super::*;

#[derive(PartialEq)]
enum Phase {
    Applying,
    Rollback,
    Recovery,
    Ending,
}
struct Prepared {
    observed: Snapshot,
    desired: Rect,
    maximize_after: Option<Rect>,
    force: bool,
}
struct Completed {
    before: PlacementSnapshot,
    observed: Snapshot,
    desired: Rect,
    started: bool,
    order: usize,
    failure: Option<String>,
}
enum Source {
    Layout {
        id: WindowId,
        screen: usize,
        rect: Rect,
        gap: f64,
        strict: bool,
    },
    Restore(PlacementSnapshot),
}
pub(super) struct PendingLayout {
    pub request: WindowRequest,
    screens: Arc<[Screen]>,
    preparing: VecDeque<Source>,
    pending: VecDeque<Prepared>,
    active: Vec<PendingAdjustment>,
    completed: Vec<Completed>,
    watched: Vec<WindowId>,
    phase: Phase,
    strict: bool,
    next_order: usize,
    seen: std::collections::BTreeSet<WindowId>,
    failure: Option<String>,
    minimums: Vec<(WindowId, Point)>,
    result: WindowResult,
}
impl PendingLayout {
    pub fn targets(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.watched.iter().copied()
    }
    pub fn preparing(&self) -> bool {
        !self.preparing.is_empty()
    }
    #[cfg(test)]
    pub fn begin(
        session: &mut Session,
        access: &mut impl WindowAccess,
        request: &WindowRequest,
        screens: &Arc<[Screen]>,
    ) -> Result<Option<Self>, String> {
        Self::take_begin(session, access, &mut request.clone(), screens)
    }
    pub fn take_begin(
        session: &mut Session,
        access: &mut impl WindowAccess,
        request: &mut WindowRequest,
        screens: &Arc<[Screen]>,
    ) -> Result<Option<Self>, String> {
        let (transaction, strict, ending) = match request.operation {
            WindowOperation::ApplyLayout {
                transaction,
                strict,
                ..
            } => (transaction, strict, false),
            WindowOperation::EndEdit {
                transaction,
                commit: false,
            } => (transaction, false, true),
            _ => return Ok(None),
        };
        let Some(edit) = session.edit.as_ref().filter(|edit| edit.id == transaction) else {
            return Ok(None);
        };
        if edit.before.iter().any(|before| {
            before.info.minimized
                || before.info.fullscreen
                || !access.can_submit_frame(before.info.id)
        }) {
            return Ok(None);
        }
        if let WindowOperation::ApplyLayout { revision, .. } = request.operation
            && revision <= edit.revision
        {
            return Err("Stale layout revision".into());
        }
        let watched = edit.before.iter().map(|before| before.info.id).collect();
        let result = Session::result_for(request);
        // Move the payload from the mailbox; no clone of placements or metadata.
        let mut request = WindowRequest {
            session: request.session,
            id: request.id,
            scope: request.scope,
            operation: std::mem::replace(&mut request.operation, WindowOperation::CancelPending),
        };
        let mut preparing = VecDeque::new();
        if ending {
            let edit = session.edit.take().ok_or("Window edit expired")?;
            preparing.extend(edit.before.into_iter().map(Source::Restore));
        } else if let WindowOperation::ApplyLayout {
            revision,
            screen,
            placements,
            additional_screens,
            gap,
            strict,
            ..
        } = &mut request.operation
        {
            let edit = session.edit.as_mut().ok_or("Window edit expired")?;
            edit.revision = *revision;
            let layouts = std::iter::once(crate::api::window::WindowScreenLayout {
                screen: *screen,
                placements: std::mem::take(placements),
            })
            .chain(std::mem::take(additional_screens));
            for layout in layouts {
                preparing.extend(
                    layout
                        .placements
                        .into_iter()
                        .map(|(id, rect)| Source::Layout {
                            id,
                            screen: layout.screen,
                            rect,
                            gap: *gap,
                            strict: *strict,
                        }),
                );
            }
        }
        Ok(Some(Self {
            request,
            screens: screens.clone(),
            preparing,
            pending: VecDeque::new(),
            active: Vec::with_capacity(16),
            completed: Vec::new(),
            watched,
            phase: if ending {
                Phase::Ending
            } else {
                Phase::Applying
            },
            strict,
            next_order: 0,
            seen: Default::default(),
            failure: None,
            minimums: Vec::new(),
            result,
        }))
    }
    fn restoration(observed: Snapshot, goal: PlacementSnapshot) -> Prepared {
        Prepared {
            observed,
            desired: if goal.info.maximized {
                goal.restored
            } else {
                goal.info.bounds
            },
            maximize_after: goal.info.maximized.then_some(goal.info.bounds),
            force: true,
        }
    }
    fn prepare_one(
        &mut self,
        session: &Session,
        access: &mut impl WindowAccess,
        source: Source,
    ) -> Result<(), String> {
        match source {
            Source::Restore(goal) => match access.snapshot(goal.info.id, &self.screens) {
                Ok(observed) => self.pending.push_back(Self::restoration(observed, goal)),
                Err(_) => self.result.skipped += 1,
            },
            Source::Layout {
                id,
                screen,
                rect,
                gap,
                strict,
            } => {
                let edit = session.edit.as_ref().ok_or("Window edit expired")?;
                if !edit.before.iter().any(|before| before.info.id == id) {
                    return Err("Window is outside the edit transaction".into());
                }
                if !self.seen.insert(access.layout_representative(id)) {
                    return Err("Duplicate window in layout transaction".into());
                }
                let display = self.screens.get(screen).ok_or("display is unavailable")?;
                if ![rect.x, rect.y, rect.width, rect.height]
                    .iter()
                    .all(|v| v.is_finite())
                    || rect.x < 0.0
                    || rect.y < 0.0
                    || rect.width <= 0.0
                    || rect.height <= 0.0
                    || rect.right() > 1.0 + 1e-6
                    || rect.bottom() > 1.0 + 1e-6
                {
                    return Err("Invalid normalized window rectangle".into());
                }
                let Ok(observed) = access.snapshot(id, &self.screens) else {
                    self.result.skipped += 1;
                    return Ok(());
                };
                if !observed.info.resizable || observed.info.fullscreen {
                    return Err("Window does not support this layout".into());
                }
                let mut desired = crate::api::window_layout::placed_rect(
                    display.work_area,
                    rect,
                    gap * access.logical_scale(display),
                );
                if !strict {
                    let minimum = edit
                        .minimums
                        .iter()
                        .find(|(window, _)| *window == id)
                        .map_or(Point::new(100.0, 80.0), |(_, min)| *min);
                    let center = desired.center();
                    desired.width = desired.width.max(minimum.x);
                    desired.height = desired.height.max(minimum.y);
                    desired.x = center.x - desired.width / 2.0;
                    desired.y = center.y - desired.height / 2.0;
                    desired = geometry::constrain_move(desired, display.work_area);
                }
                self.pending.push_back(Prepared {
                    observed,
                    desired,
                    maximize_after: None,
                    force: false,
                });
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn advance(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: bool,
    ) -> Option<WindowResult> {
        self.advance_at(session, access, cancelled, Instant::now())
    }
    #[cfg(test)]
    pub(super) fn advance_at(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: bool,
        now: Instant,
    ) -> Option<WindowResult> {
        self.advance_checked(session, access, &|| cancelled, now)
    }
    pub fn advance_checked(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: &dyn Fn() -> bool,
        now: Instant,
    ) -> Option<WindowResult> {
        let applying = self.phase == Phase::Applying;
        if applying && cancelled() {
            self.failure
                .get_or_insert_with(|| "Layout cancelled".into());
        }
        let budget = Instant::now();
        for _ in 0..8 {
            if applying && (self.failure.is_some() || cancelled()) {
                self.failure
                    .get_or_insert_with(|| "Layout cancelled".into());
                self.preparing.clear();
                self.pending.clear();
                break;
            }
            let Some(source) = self.preparing.pop_front() else {
                break;
            };
            if let Err(error) = self.prepare_one(session, access, source) {
                self.failure.get_or_insert(error);
            }
            if budget.elapsed() >= Duration::from_millis(4) {
                break;
            }
        }
        if !self.preparing.is_empty() {
            return None;
        }
        if applying && (self.failure.is_some() || cancelled()) {
            self.failure
                .get_or_insert_with(|| "Layout cancelled".into());
            self.pending.clear();
        }
        let now = now + budget.elapsed();
        // Only the at-most-16 active records are polled. Completed results stay compact.
        let mut index = 0;
        while index < self.active.len() {
            self.active[index].advance_at(access, false, now);
            if !self.active[index].ready() {
                index += 1;
                continue;
            }
            let mut frame = self.active.swap_remove(index);
            let started = frame.started();
            if started
                && frame.failure.is_none()
                && let Err(error) = access.confirmed_placement(&frame.observed, &self.screens)
            {
                frame.failure = Some(error);
            }
            if let Some(error) = &frame.failure {
                self.failure.get_or_insert_with(|| error.clone());
            }
            if applying && self.strict && !rect_matches(frame.observed.info.bounds, frame.desired) {
                let after = &frame.observed.info;
                if !after.maximized {
                    self.minimums.push((
                        after.id,
                        Point::new(
                            if after.bounds.width > frame.desired.width + 1.5
                                && (after.bounds.width - frame.before.info.bounds.width).abs() > 1.5
                            {
                                after.bounds.width
                            } else {
                                0.0
                            },
                            if after.bounds.height > frame.desired.height + 1.5
                                && (after.bounds.height - frame.before.info.bounds.height).abs()
                                    > 1.5
                            {
                                after.bounds.height
                            } else {
                                0.0
                            },
                        ),
                    ));
                }
                self.failure.get_or_insert_with(|| {
                    "Application rejected the requested size; restored the previous layout".into()
                });
            }
            self.completed.push(Completed {
                order: frame.transaction_order,
                before: frame.before,
                observed: frame.observed,
                desired: frame.desired,
                started,
                failure: frame.failure,
            });
        }
        if applying && (self.failure.is_some() || cancelled()) {
            self.failure
                .get_or_insert_with(|| "Layout cancelled".into());
            self.pending.clear();
        }
        while self.active.len() < 16 {
            if applying && cancelled() {
                self.failure
                    .get_or_insert_with(|| "Layout cancelled".into());
                self.pending.clear();
                break;
            }
            let Some(job) = self.pending.pop_front() else {
                break;
            };
            let mut frame = PendingAdjustment::geometry(
                &self.request,
                &self.screens,
                job.observed,
                job.desired,
                job.maximize_after,
            );
            frame.transaction_order = self.next_order;
            self.next_order += 1;
            if job.force {
                frame.force_submission();
            }
            frame.advance_at(access, false, now);
            self.active.push(frame);
        }
        if !self.active.is_empty() || !self.pending.is_empty() {
            return None;
        }
        if applying && self.failure.is_some() && !self.completed.is_empty() {
            // Reverse native submission order, independent of completion order.
            self.completed.sort_by_key(|frame| frame.order);
            for frame in self.completed.drain(..).rev() {
                if frame.started {
                    self.pending
                        .push_back(Self::restoration(frame.observed, frame.before));
                }
            }
            self.phase = Phase::Rollback;
            return None;
        }
        let rejected_restore = self.completed.iter().any(|frame| {
            frame.failure.is_some() || !rect_matches(frame.observed.info.bounds, frame.desired)
        });
        if self.phase == Phase::Rollback && rejected_restore {
            self.completed.clear();
            if let Some(edit) = session.edit.take() {
                self.preparing
                    .extend(edit.before.into_iter().map(Source::Restore));
            }
            self.phase = Phase::Recovery;
            return None;
        }
        let transaction = match self.request.operation {
            WindowOperation::ApplyLayout { transaction, .. }
            | WindowOperation::EndEdit { transaction, .. } => transaction,
            _ => unreachable!(),
        };
        let ended = self.phase == Phase::Ending || self.phase == Phase::Recovery;
        if self.phase == Phase::Recovery {
            self.failure = Some("Application rejected restoration; edit ended and entry-layout recovery was attempted".into());
        }
        if let Some(edit) = session.edit.as_mut()
            && self.failure.is_some()
            && !cancelled()
        {
            for (id, min) in &mut edit.minimums {
                let queried = access.minimum_size(*id);
                let observed = self
                    .minimums
                    .iter()
                    .find(|(window, _)| window == id)
                    .map_or(Point::default(), |(_, min)| *min);
                *min = Point::new(
                    min.x.max(queried.x).max(observed.x),
                    min.y.max(queried.y).max(observed.y),
                );
                session.minimums.insert(*id, *min);
            }
        }
        self.result.changed = if self.failure.is_none() {
            self.completed
                .iter()
                .filter(|frame| !same_placement(frame.before, &frame.observed))
                .count()
        } else {
            0
        };
        self.result.skipped += self
            .completed
            .iter()
            .filter(|frame| frame.failure.is_some())
            .count();
        self.result.windows = Some(if self.failure.is_some() && !ended && !cancelled() {
            session
                .edit
                .iter()
                .flat_map(|edit| &edit.before)
                .filter_map(|before| {
                    access
                        .snapshot(before.info.id, &self.screens)
                        .ok()
                        .map(|snapshot| snapshot.info)
                })
                .collect()
        } else {
            std::mem::take(&mut self.completed)
                .into_iter()
                .map(|frame| frame.observed.info)
                .collect()
        });
        if cancelled() {
            self.result.windows = None;
        }
        self.result.edit = Some(Box::new(if ended {
            WindowEditResult::Ended {
                transaction,
                committed: false,
            }
        } else {
            let WindowOperation::ApplyLayout { revision, .. } = self.request.operation else {
                unreachable!()
            };
            WindowEditResult::Applied {
                transaction,
                revision,
                accepted: self.failure.is_none(),
                minimums: if self.failure.is_some() {
                    session
                        .edit
                        .as_ref()
                        .map_or(Vec::new(), |edit| edit.minimums.clone())
                } else {
                    Vec::new()
                },
            }
        }));
        if self.result.changed > 0 {
            session.redo.clear();
        }
        self.result.message = self.failure.take();
        let result = std::mem::replace(&mut self.result, Session::result_for(&self.request));
        Some(session.complete_result(access, result, &self.screens))
    }
}
