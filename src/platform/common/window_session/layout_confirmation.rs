//! Bounded layout writes, independent acknowledgements and transactional rollback.
use super::*;

#[derive(PartialEq)]
enum Phase {
    Applying,
    Rollback,
    Recovery,
    Ending,
}

pub(super) struct PendingLayout {
    pub request: WindowRequest,
    screens: Arc<[Screen]>,
    frames: Vec<PendingAdjustment>,
    before: Vec<PlacementSnapshot>,
    watched: Vec<WindowId>,
    phase: Phase,
    strict: bool,
    failure: Option<String>,
    minimums: Vec<(WindowId, Point)>,
    result: WindowResult,
}

impl PendingLayout {
    pub fn targets(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.watched.iter().copied()
    }

    pub fn begin(
        session: &mut Session,
        access: &mut impl WindowAccess,
        request: &WindowRequest,
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
        // Native minimized/fullscreen restoration retains the compatibility path.
        // Decline before revision changes or native writes.
        if edit.before.iter().any(|before| {
            before.info.minimized
                || before.info.fullscreen
                || !access.can_submit_frame(before.info.id)
        }) {
            return Ok(None);
        }
        let watched = edit.before.iter().map(|before| before.info.id).collect();
        let mut result = Session::result_for(request);
        let mut before = Vec::new();
        let mut frames = Vec::new();
        if ending {
            let edit = session.edit.take().ok_or("Window edit expired")?;
            for goal in edit.before {
                match access.snapshot(goal.info.id, screens) {
                    Ok(observed) => {
                        frames.push(Self::restore_frame(request, screens, observed, goal))
                    }
                    Err(_) => result.skipped += 1,
                }
            }
        } else {
            let batch = session.prepare_layout(
                access,
                request.operation.clone(),
                screens,
                &|| false,
                &mut result,
            )?;
            for (observed, desired) in batch {
                before.push(PlacementSnapshot::from(&observed));
                frames.push(PendingAdjustment::geometry(
                    request, screens, observed, desired, None,
                ));
            }
        }
        Ok(Some(Self {
            request: request.clone(),
            screens: screens.clone(),
            frames,
            before,
            watched,
            phase: if ending {
                Phase::Ending
            } else {
                Phase::Applying
            },
            strict,
            failure: None,
            minimums: Vec::new(),
            result,
        }))
    }

    fn restore_frame(
        request: &WindowRequest,
        screens: &Arc<[Screen]>,
        observed: Snapshot,
        goal: PlacementSnapshot,
    ) -> PendingAdjustment {
        let mut frame = PendingAdjustment::geometry(
            request,
            screens,
            observed,
            if goal.info.maximized {
                goal.restored
            } else {
                goal.info.bounds
            },
            goal.info.maximized.then_some(goal.info.bounds),
        );
        frame.force_submission();
        frame
    }

    pub fn advance(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: bool,
    ) -> Option<WindowResult> {
        self.advance_at(session, access, cancelled, Instant::now())
    }
    pub(super) fn advance_at(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: bool,
        now: Instant,
    ) -> Option<WindowResult> {
        let applying = self.phase == Phase::Applying;
        if applying && cancelled {
            self.failure
                .get_or_insert_with(|| "Layout cancelled".into());
        }
        let mut active = self
            .frames
            .iter()
            .filter(|frame| frame.started() && !frame.ready())
            .count();
        for frame in &mut self.frames {
            if frame.ready() {
                continue;
            }
            if !frame.started() {
                if applying && self.failure.is_some() {
                    continue;
                }
                if active >= 16 {
                    continue;
                }
                active += 1;
            }
            // A submitted write must settle before rollback, even after cancellation.
            frame.advance_at(access, false, now);
            if !frame.ready() {
                continue;
            }
            active = active.saturating_sub(1);
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
        }
        if self
            .frames
            .iter()
            .any(|frame| !frame.ready() && (frame.started() || !applying || self.failure.is_none()))
        {
            return None;
        }

        for frame in &mut self.frames {
            if frame.started()
                && frame.failure.is_none()
                && let Err(error) = access.confirmed_placement(&frame.observed, &self.screens)
            {
                frame.failure = Some(error.clone());
                self.failure.get_or_insert(error);
            }
        }
        if applying && self.failure.is_some() {
            let old = std::mem::take(&mut self.frames);
            self.frames = old
                .into_iter()
                .zip(self.before.iter().copied())
                .rev()
                .filter(|(frame, _)| frame.started())
                .map(|(frame, goal)| {
                    Self::restore_frame(&self.request, &self.screens, frame.observed, goal)
                })
                .collect();
            self.phase = Phase::Rollback;
            return None;
        }
        let rejected_restore = self.frames.iter().any(|frame| {
            frame.failure.is_some() || !rect_matches(frame.observed.info.bounds, frame.desired)
        });
        if self.phase == Phase::Rollback && rejected_restore {
            self.frames.clear();
            if let Some(edit) = session.edit.take() {
                for goal in edit.before {
                    if let Ok(observed) = access.snapshot(goal.info.id, &self.screens) {
                        self.frames.push(Self::restore_frame(
                            &self.request,
                            &self.screens,
                            observed,
                            goal,
                        ));
                    }
                }
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
            && !cancelled
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
            self.frames
                .iter()
                .filter(|frame| !same_placement(frame.before, &frame.observed))
                .count()
        } else {
            0
        };
        self.result.skipped += self
            .frames
            .iter()
            .filter(|frame| frame.failure.is_some())
            .count();
        self.result.windows = Some(if self.failure.is_some() && !ended {
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
            std::mem::take(&mut self.frames)
                .into_iter()
                .map(|frame| frame.observed.info)
                .collect()
        });
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
