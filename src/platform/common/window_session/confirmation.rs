//! In-flight ordinary geometry. Submission never advertises unobserved success.
use super::*;

pub(super) struct PendingAdjustment {
    pub request: WindowRequest,
    pub screens: Arc<[Screen]>,
    pub(super) before: PlacementSnapshot,
    pub(super) observed: Snapshot,
    pub(super) desired: Rect,
    group: u64,
    maximize: bool,
    restoring: bool,
    fitting: bool,
    pointer: Option<Point>,
    started: bool,
    maximize_after: Option<Rect>,
    previous: Rect,
    stable_since: Instant,
    deadline: Instant,
    retried: bool,
    centered: bool,
    pub(super) failure: Option<String>,
    ready: bool,
    observation_failed: bool,
}

impl PendingAdjustment {
    pub fn target(&self) -> WindowId {
        self.before.info.id
    }

    pub fn begin(
        session: &mut Session,
        access: &mut impl WindowAccess,
        request: &WindowRequest,
        screens: &Arc<[Screen]>,
    ) -> Result<(Option<Self>, Option<Snapshot>), String> {
        let WindowOperation::Adjust {
            target,
            change,
            group,
        } = &request.operation
        else {
            return Ok((None, None));
        };
        if !matches!(
            change,
            WindowChange::Move { .. }
                | WindowChange::MoveTo(_)
                | WindowChange::Resize { .. }
                | WindowChange::ToggleMaximize
                | WindowChange::CycleState
        ) {
            return Ok((None, None));
        }
        if !access.can_submit_frame(*target) {
            return Ok((None, None));
        }
        session.prepare_context(access, request, screens);
        let before = access.snapshot(*target, screens)?;
        if before.info.fullscreen
            || (matches!(change, WindowChange::CycleState)
                && before.info.maximized
                && !before.info.minimized)
        {
            return Ok((None, Some(before)));
        }
        // Preserve the gesture cache if an adapter declines asynchronous geometry.
        let old_remainder = session.move_remainder;
        let old_minimum = session.resize_minimum;
        let state_change = matches!(
            change,
            WindowChange::ToggleMaximize | WindowChange::CycleState
        );
        let maximize = state_change && !before.info.maximized && !before.info.minimized;
        let restoring = !maximize && (before.info.maximized || before.info.minimized);
        let pointer = state_change.then(|| access.pointer().ok()).flatten();
        let desired = if state_change {
            if maximize {
                screens
                    .get(before.info.screen)
                    .ok_or("display unavailable")?
                    .work_area
            } else {
                before.restored
            }
        } else {
            let Some(desired) = session.adjustment_rect(
                access,
                *target,
                change.clone(),
                *group,
                &before,
                screens,
            )?
            else {
                return Ok((None, Some(before)));
            };
            desired
        };
        if desired == before.info.bounds && !maximize && !restoring {
            session.move_remainder = old_remainder;
            session.resize_minimum = old_minimum;
            return Ok((None, Some(before)));
        }
        let submitted = if maximize || restoring {
            access.submit_maximize(&before, maximize, screens)
        } else {
            access.submit_frame(&before, desired, screens)
        };
        let failure = match submitted {
            Ok(true) => None,
            Ok(false) => {
                session.move_remainder = old_remainder;
                session.resize_minimum = old_minimum;
                return Ok((None, Some(before)));
            }
            Err(error) => Some(error),
        };
        let now = Instant::now();
        Ok((
            Some(Self {
                request: request.clone(),
                screens: screens.clone(),
                previous: before.info.bounds,
                before: PlacementSnapshot::from(&before),
                observed: before,
                desired,
                group: *group,
                maximize,
                restoring,
                fitting: false,
                pointer,
                started: true,
                maximize_after: None,
                stable_since: now,
                deadline: now + Duration::from_millis(250),
                retried: false,
                centered: false,
                failure,
                ready: false,
                observation_failed: false,
            }),
            None,
        ))
    }

    pub(super) fn geometry(
        request: &WindowRequest,
        screens: &Arc<[Screen]>,
        observed: Snapshot,
        desired: Rect,
        maximize_after: Option<Rect>,
    ) -> Self {
        let now = Instant::now();
        let ready = maximize_after.is_none()
            && !observed.info.maximized
            && !observed.info.minimized
            && rect_matches(observed.info.bounds, desired);
        Self {
            request: WindowRequest {
                operation: WindowOperation::Adjust {
                    target: observed.info.id,
                    change: WindowChange::Move { dx: 0.0, dy: 0.0 },
                    group: 0,
                },
                session: request.session,
                id: request.id,
                scope: request.scope,
            },
            screens: screens.clone(),
            before: PlacementSnapshot::from(&observed),
            previous: observed.info.bounds,
            restoring: observed.info.maximized || observed.info.minimized,
            observed,
            desired,
            group: 0,
            maximize: false,
            fitting: false,
            pointer: None,
            started: false,
            maximize_after,
            stable_since: now,
            deadline: now,
            retried: false,
            centered: true,
            failure: None,
            ready,
            observation_failed: false,
        }
    }
    pub(super) fn force_submission(&mut self) {
        self.ready = false;
    }
    pub(super) fn started(&self) -> bool {
        self.started
    }

    #[cfg(test)]
    pub fn poll(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: bool,
    ) -> Option<WindowResult> {
        self.poll_at(session, access, cancelled, Instant::now())
    }

    #[cfg(test)]
    pub(super) fn poll_at(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
        cancelled: bool,
        now: Instant,
    ) -> Option<WindowResult> {
        self.advance_at(access, cancelled, now);
        self.ready.then(|| self.finish(session, access))
    }

    pub fn ready(&self) -> bool {
        self.ready
    }
    pub fn advance(&mut self, access: &mut impl WindowAccess, cancelled: bool) {
        self.advance_at(access, cancelled, Instant::now());
    }
    pub(super) fn advance_at(
        &mut self,
        access: &mut impl WindowAccess,
        cancelled: bool,
        now: Instant,
    ) {
        if self.ready {
            return;
        }
        if !self.started {
            if cancelled {
                self.ready = true;
                return;
            }
            self.started = true;
            self.deadline = now + Duration::from_millis(250);
            self.stable_since = now;
            let submitted = if self.restoring {
                access.submit_maximize(&self.observed, false, &self.screens)
            } else {
                access.submit_frame(&self.observed, self.desired, &self.screens)
            };
            match submitted {
                Ok(true) => return,
                Ok(false) => self.failure = Some("Window declined asynchronous placement".into()),
                Err(error) => self.failure = Some(error),
            }
        }
        let geometry_only = !self.restoring && !self.maximize;
        let observed = if geometry_only {
            access.refresh_frame(&mut self.observed, &self.screens)
        } else {
            access.refresh_geometry(&mut self.observed, &self.screens)
        };
        let mut failure = self.failure.take();
        if observed.is_ok() {
            if !rect_matches(self.observed.info.bounds, self.previous) {
                self.previous = self.observed.info.bounds;
                self.stable_since = now;
            }
            if self.maximize
                && !self.observed.info.maximized
                && !rect_matches(self.observed.info.bounds, self.before.info.bounds)
                && now.saturating_duration_since(self.stable_since) >= Duration::from_millis(30)
            {
                access.accept_maximized_frame(&mut self.observed, &self.screens);
            }

            if failure.is_none() && !cancelled && self.restoring {
                if self.observed.info.maximized || self.observed.info.minimized {
                    if now < self.deadline {
                        return;
                    }
                    failure = Some("Timed out restoring window state".into());
                } else {
                    self.restoring = false;
                    self.stable_since = now;
                    self.previous = self.observed.info.bounds;
                    match access.submit_frame(&self.observed, self.desired, &self.screens) {
                        Ok(true) => return,
                        Ok(false) => {
                            failure = Some("Window declined asynchronous restored geometry".into())
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            if failure.is_none() && !cancelled && self.maximize {
                if !self.observed.info.maximized {
                    if now < self.deadline {
                        return;
                    }
                    failure = Some("Timed out maximizing window".into());
                } else if !self.fitting && !rect_matches(self.observed.info.bounds, self.desired) {
                    self.fitting = true;
                    match access.submit_maximized_frame(&self.observed, self.desired, &self.screens)
                    {
                        Ok(true) => return,
                        Ok(false) => {
                            failure = Some("Window declined asynchronous maximized geometry".into())
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            let after = &self.observed;
            if !rect_matches(after.info.bounds, self.previous) {
                self.previous = after.info.bounds;
                self.stable_since = now;
            }
            let matched = rect_matches(after.info.bounds, self.desired);
            let settled = !rect_matches(after.info.bounds, self.before.info.bounds)
                && now.saturating_duration_since(self.stable_since) >= Duration::from_millis(30);
            if failure.is_some() && !cancelled && !matched && !settled && now < self.deadline {
                self.failure = failure;
                return;
            }
            if failure.is_none() && !cancelled && !matched && now < self.deadline {
                if !settled {
                    return;
                }
                if self.retried
                    && !self.centered
                    && matches!(
                        self.request.operation,
                        WindowOperation::Adjust {
                            change: WindowChange::Resize { .. },
                            ..
                        }
                    )
                {
                    self.centered = true;
                    let center = self.desired.center();
                    if after.info.bounds.center().distance_to(&center) > 1.0 {
                        self.desired = Rect::new(
                            center.x - after.info.bounds.width / 2.0,
                            center.y - after.info.bounds.height / 2.0,
                            after.info.bounds.width,
                            after.info.bounds.height,
                        );
                        self.stable_since = now;
                        match if self.maximize {
                            access.submit_maximized_frame(after, self.desired, &self.screens)
                        } else {
                            access.submit_frame(after, self.desired, &self.screens)
                        } {
                            Ok(true) => return,
                            Ok(false) => {
                                failure = Some("Window geometry changed during correction".into())
                            }
                            Err(error) => failure = Some(error),
                        }
                    }
                }
                if !self.retried {
                    // An asynchronous AX position write can initially constrain
                    // size at the previous origin. Retry once before accepting
                    // a stable application-imposed constraint.
                    self.retried = true;
                    self.stable_since = now;
                    match if self.maximize {
                        access.submit_maximized_frame(after, self.desired, &self.screens)
                    } else {
                        access.submit_frame(after, self.desired, &self.screens)
                    } {
                        Ok(true) => return,
                        Ok(false) => {
                            failure = Some("Window geometry changed during confirmation".into())
                        }
                        Err(error) => failure = Some(error),
                    }
                }
            }
            if !cancelled && !matched && !settled && now >= self.deadline {
                failure = Some("Timed out confirming window geometry".into());
            }
        } else if let Err(error) = &observed {
            failure = Some(error.clone());
            // A write without readable acknowledgement may already have applied.
            self.observation_failed = true;
        }
        if geometry_only && failure.is_none() {
            if let Err(error) = access.validate_frame(&mut self.observed, &self.screens) {
                self.observation_failed = true;
                failure = Some(error);
            } else if self.observed.info.maximized
                || self.observed.info.minimized
                || self.observed.info.fullscreen
            {
                failure = Some("Window state changed during geometry confirmation".into());
            }
        }
        if failure.is_none()
            && !cancelled
            && let Some(desired) = self.maximize_after.take()
        {
            self.desired = desired;
            self.maximize = true;
            self.fitting = false;
            self.retried = false;
            self.deadline = now + Duration::from_millis(250);
            self.previous = self.observed.info.bounds;
            self.stable_since = now;
            match access.submit_maximize(&self.observed, true, &self.screens) {
                Ok(true) => return,
                Ok(false) => failure = Some("Window declined asynchronous restoration".into()),
                Err(error) => failure = Some(error),
            }
        }
        self.failure = failure;
        self.ready = true;
    }
    pub fn finish(
        &mut self,
        session: &mut Session,
        access: &mut impl WindowAccess,
    ) -> WindowResult {
        let mut failure = self.failure.take();
        if self.observation_failed {
            session.remember(self.group, self.before);
        }
        let mut result = Session::result_for(&self.request);
        if !self.observation_failed {
            let after = &mut self.observed;
            if !same_placement(self.before, &*after) {
                session.remember(self.group, self.before);
                result.changed = 1;
            }
            if let Err(error) = access.confirmed_placement(after, &self.screens) {
                failure.get_or_insert(error);
            }
            if !after.info.minimized
                && let Some(pointer) = self.pointer
                && let Some(screen) = self.screens.get(after.info.screen)
            {
                result.pointer = Some(window_placement::following_pointer(
                    pointer,
                    self.before.info.bounds,
                    after.info.bounds,
                    screen.bounds,
                ));
            }
            session.target = Some(self.before.info.id);
            let target = WindowInfo {
                id: after.info.id,
                title: std::mem::take(&mut after.info.title),
                app: std::mem::take(&mut after.info.app),
                bounds: after.info.bounds,
                screen: after.info.screen,
                resizable: after.info.resizable,
                maximized: after.info.maximized,
                minimized: after.info.minimized,
                fullscreen: after.info.fullscreen,
            };
            result.target = Some(target);
        }
        if failure.is_some() {
            session.move_remainder = None;
        }
        result.message = failure;
        session.complete_result(access, result, &self.screens)
    }
}
