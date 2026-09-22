//! Undo/redo owns its source batch until acknowledged changes and retries are committed.
use super::*;

pub(super) struct PendingHistory {
    pub request: WindowRequest,
    screens: Arc<[Screen]>,
    group: u64,
    pending: VecDeque<PlacementSnapshot>,
    active: Vec<(PlacementSnapshot, PendingAdjustment)>,
    watched: Vec<WindowId>,
    inverse: Vec<PlacementSnapshot>,
    remaining: Vec<PlacementSnapshot>,
    result: WindowResult,
}
impl PendingHistory {
    pub fn begin(
        session: &mut Session,
        access: &mut impl WindowAccess,
        request: &WindowRequest,
        screens: &Arc<[Screen]>,
    ) -> Result<Option<Self>, String> {
        if !matches!(
            request.operation,
            WindowOperation::Undo | WindowOperation::Redo | WindowOperation::ResetInitial { .. }
        ) {
            return Ok(None);
        }
        if session.edit.is_some() {
            return Err("Finish the current window edit first".into());
        }
        session.prepare_context(access, request, screens);
        let (group, mut goals) = match request.operation {
            WindowOperation::Undo => match session.history.pop_back() {
                Some(batch) => batch,
                None => return Ok(None),
            },
            WindowOperation::Redo => match session.redo.pop_back() {
                Some(batch) => batch,
                None => return Ok(None),
            },
            WindowOperation::ResetInitial { group } => (
                group,
                session
                    .changed_windows
                    .iter()
                    .filter_map(|id| session.initial.get(id).copied())
                    .collect(),
            ),
            _ => unreachable!(),
        };
        goals.sort_by_key(|goal| access.layout_representative(goal.info.id) != goal.info.id);
        let mut represented = std::collections::BTreeSet::new();
        goals.retain(|goal| represented.insert(access.layout_representative(goal.info.id)));
        Ok(Some(Self {
            request: request.clone(),
            screens: screens.clone(),
            group,
            watched: goals.iter().map(|goal| goal.info.id).collect(),
            pending: goals.into_iter().rev().collect(),
            active: Vec::with_capacity(16),
            inverse: Vec::new(),
            remaining: Vec::new(),
            result: Session::result_for(request),
        }))
    }
    pub fn targets(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.watched.iter().copied()
    }
    pub fn preparing(&self) -> bool {
        !self.pending.is_empty() && self.active.len() < 16
    }
    fn record(
        &mut self,
        goal: PlacementSnapshot,
        before: PlacementSnapshot,
        observed: Option<&Snapshot>,
    ) {
        if let Some(after) = observed {
            if !same_placement(before, after) {
                self.inverse.push(before);
                self.result.changed += 1;
            }
            if same_placement(goal, after) {
                return;
            }
        } else {
            self.inverse.push(before);
        }
        self.remaining.push(goal);
        self.result.skipped += 1;
    }
    pub fn advance(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: &dyn Fn() -> bool,
        now: Instant,
    ) -> Option<WindowResult> {
        let mut index = 0;
        while index < self.active.len() {
            self.active[index].1.advance_at(access, false, now);
            if !self.active[index].1.ready() {
                index += 1;
                continue;
            }
            let (goal, frame) = self.active.swap_remove(index);
            if let Some(error) = &frame.failure {
                crate::support::logging::report_error_context(
                    "window-history",
                    error,
                    format_args!(
                        "session={} request={}",
                        self.request.session, self.request.id
                    ),
                );
            }
            if !frame.observation_failed
                && let Err(error) = access.confirmed_placement(&frame.observed, &self.screens)
            {
                crate::report_error!("window-history", "{error}");
            }
            self.record(
                goal,
                frame.before,
                (!frame.observation_failed).then_some(&frame.observed),
            );
        }
        let budget = Instant::now();
        for _ in 0..8 {
            if cancelled() {
                self.remaining.extend(self.pending.drain(..));
                break;
            }
            if self.active.len() >= 16 {
                break;
            }
            let Some(goal) = self.pending.pop_front() else {
                break;
            };
            let Ok(mut observed) = access.snapshot(goal.info.id, &self.screens) else {
                self.remaining.push(goal);
                self.result.skipped += 1;
                continue;
            };
            if same_placement(goal, &observed) {
                continue;
            }
            if cancelled() {
                self.remaining.push(goal);
                self.remaining.extend(self.pending.drain(..));
                break;
            }
            if access.can_submit_frame(goal.info.id)
                && !goal.info.minimized
                && !goal.info.fullscreen
                && !observed.info.fullscreen
            {
                let mut frame = PendingAdjustment::geometry(
                    &self.request,
                    &self.screens,
                    observed,
                    if goal.info.maximized {
                        goal.restored
                    } else {
                        goal.info.bounds
                    },
                    goal.info.maximized.then_some(goal.info.bounds),
                );
                frame.force_submission();
                frame.advance_at(access, false, now + budget.elapsed());
                self.active.push((goal, frame));
            } else {
                // Unsupported state transitions still yield between windows and
                // reuse the snapshot already obtained by this operation.
                let before = PlacementSnapshot::from(&observed);
                goal.apply_to(&mut observed);
                let applied = access.restore(&observed, &self.screens, cancelled);
                let readable = access
                    .refresh_geometry(&mut observed, &self.screens)
                    .is_ok();
                self.record(goal, before, readable.then_some(&observed));
                if let Err(error) = applied {
                    crate::report_error!("window-history", "{error}");
                }
                break;
            }
            if budget.elapsed() >= Duration::from_millis(4) {
                break;
            }
        }
        if !self.active.is_empty() || !self.pending.is_empty() {
            return None;
        }
        let inverse = std::mem::take(&mut self.inverse);
        let remaining = std::mem::take(&mut self.remaining);
        match self.request.operation {
            WindowOperation::Redo => {
                Session::push_history(&mut session.redo, self.group, remaining);
                Session::push_history(&mut session.history, self.group, inverse);
            }
            WindowOperation::Undo => {
                Session::push_history(&mut session.history, self.group, remaining);
                Session::push_history(&mut session.redo, self.group, inverse);
            }
            WindowOperation::ResetInitial { .. } => {
                if !inverse.is_empty() {
                    session.redo.clear();
                    Session::push_history(&mut session.history, self.group, inverse);
                }
            }
            _ => unreachable!(),
        }
        if !cancelled() {
            match session.enumerate(access, &self.screens, cancelled) {
                Ok(windows) => self.result.windows = Some(windows),
                Err(error) => {
                    crate::report_error!("window-history", "{error}");
                    self.result.message = Some(error);
                }
            }
        }
        if self.result.message.is_none() {
            self.result.message = Some(format!(
                "{}Restored {} · skipped {}",
                if matches!(self.request.operation, WindowOperation::ResetInitial { .. }) {
                    "Initial state · "
                } else {
                    ""
                },
                self.result.changed,
                self.result.skipped
            ));
        }
        let result = std::mem::replace(&mut self.result, Session::result_for(&self.request));
        Some(session.complete_result(access, result, &self.screens))
    }
}
