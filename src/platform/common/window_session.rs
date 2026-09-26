//! Lazy, bounded window-operation worker. Native references are created and
//! released on its thread; only API values enter the engine's event queue.

mod access;
mod confirmation;
mod history;
mod history_confirmation;
mod layout_confirmation;
mod overlap;
mod transaction;
mod worker;
pub(crate) use access::WindowAccess;
use confirmation::PendingAdjustment;
use history::PlacementSnapshot;
use overlap::OverlapCache;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
pub(crate) use worker::WindowWorker;
#[cfg(test)]
use worker::execute_audio;

use super::{window_geometry as geometry, window_placement};
use crate::api::window::{
    WindowChange, WindowEditResult, WindowId, WindowInfo, WindowOperation, WindowRequest,
    WindowResult,
};
use crate::api::{BackendEvent, Point, Rect, Screen};
use crate::support::worker::WorkerJoin;

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub info: WindowInfo,
    pub restored: Rect,
}

impl Snapshot {
    /// Scalar-only native submission view; metadata remains owned by the result.
    pub(crate) fn placement(&self) -> Self {
        Self {
            info: WindowInfo {
                id: self.info.id,
                title: String::new(),
                app: String::new(),
                bounds: self.info.bounds,
                screen: self.info.screen,
                resizable: self.info.resizable,
                maximized: self.info.maximized,
                minimized: self.info.minimized,
                fullscreen: self.info.fullscreen,
            },
            restored: self.restored,
        }
    }
}

#[derive(Default)]
struct Session {
    scope: Option<crate::api::window::WindowScope>,
    id: u64,
    target: Option<WindowId>,
    history: VecDeque<(u64, Vec<PlacementSnapshot>)>,
    redo: VecDeque<(u64, Vec<PlacementSnapshot>)>,
    initial: std::collections::BTreeMap<WindowId, PlacementSnapshot>,
    changed_windows: std::collections::BTreeSet<WindowId>,
    cycle: Vec<WindowId>,
    overlap: Option<Box<OverlapCache>>,
    edit: Option<EditTransaction>,
    screens: Vec<Screen>,
    minimums: std::collections::BTreeMap<WindowId, Point>,
    error: Option<String>,
    pending_closed: Vec<WindowId>,
    resize_minimum: Option<(WindowId, u64, usize, f64, Point)>,
    move_remainder: Option<(WindowId, u64, f64, Rect, Point)>,
}

struct EditTransaction {
    id: u64,
    group: u64,
    revision: u64,
    before: Vec<PlacementSnapshot>,
    minimums: Vec<(WindowId, Point)>,
}

fn rect_matches(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= 1.5
        && (a.y - b.y).abs() <= 1.5
        && (a.width - b.width).abs() <= 1.5
        && (a.height - b.height).abs() <= 1.5
}

fn same_placement(a: impl Into<PlacementSnapshot>, b: impl Into<PlacementSnapshot>) -> bool {
    let (a, b) = (a.into(), b.into());
    rect_matches(a.info.bounds, b.info.bounds)
        && a.info.maximized == b.info.maximized
        && a.info.minimized == b.info.minimized
        && a.info.fullscreen == b.info.fullscreen
        && rect_matches(a.restored, b.restored)
}

/// Let explicit native probes exercise the actual transaction implementation
/// without exporting its mutable internals to production adapters.
#[cfg(test)]
pub(crate) struct WindowSessionProbe {
    session: Session,
    request: u64,
}

#[cfg(test)]
impl WindowSessionProbe {
    /// Functional native acceptance of the same state machines used by the worker.
    #[cfg(target_os = "windows")]
    pub(crate) fn execute_deferred(
        &mut self,
        access: &mut impl WindowAccess,
        operation: WindowOperation,
        screens: &[Screen],
    ) -> WindowResult {
        self.request += 1;
        let mut request = WindowRequest {
            scope: None,
            session: 1,
            id: self.request,
            operation,
        };
        let screens: Arc<[Screen]> = screens.into();
        let deadline = Instant::now() + Duration::from_secs(3);
        if let Some(mut pending) = transaction::PendingTransaction::take_begin(
            &mut self.session,
            access,
            &mut request,
            &screens,
        )
        .unwrap()
        {
            loop {
                if let Some(result) =
                    pending.advance_checked(&mut self.session, access, &|| false, Instant::now())
                {
                    return result;
                }
                assert!(
                    Instant::now() < deadline,
                    "native layout confirmation timed out"
                );
                if !access.wait_for_events(Some(Duration::from_millis(5))) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        if let Some(mut pending) =
            PendingAdjustment::begin(&mut self.session, access, &request, &screens)
                .unwrap()
                .0
        {
            loop {
                if let Some(result) = pending.poll(&mut self.session, access, false) {
                    return result;
                }
                assert!(
                    Instant::now() < deadline,
                    "native adjustment confirmation timed out"
                );
                if !access.wait_for_events(Some(Duration::from_millis(5))) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        self.session.execute(access, request, &screens, &|| false)
    }

    pub(crate) fn new(target: WindowId) -> Self {
        Self {
            session: Session {
                target: Some(target),
                ..Session::default()
            },
            request: 0,
        }
    }

    pub(crate) fn execute(
        &mut self,
        access: &mut impl WindowAccess,
        operation: WindowOperation,
        screens: &[Screen],
    ) -> WindowResult {
        self.request += 1;
        self.session.execute(
            access,
            WindowRequest {
                scope: None,
                session: 1,
                id: self.request,
                operation,
            },
            screens,
            &|| false,
        )
    }
}

impl Session {
    fn enumerate(
        &self,
        access: &mut impl WindowAccess,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowInfo>, String> {
        let mut windows = access.enumerate(screens, cancelled)?;
        windows.retain(|window| self.scope.is_none_or(|scope| scope.contains(window)));
        Ok(windows)
    }
    fn cleanup_edit(&mut self, _access: &mut impl WindowAccess) {
        // Applied changes are final. Teardown releases checkpoints without
        // sending old geometry back to applications or delaying the next session.
        self.edit = None;
    }

    fn result_for(request: &WindowRequest) -> WindowResult {
        WindowResult {
            tabs: None,
            session: request.session,
            id: request.id,
            target: None,
            windows: None,
            closed: Vec::new(),
            pointer: None,
            changed: 0,
            skipped: 0,
            message: None,
            edit: None,
        }
    }
    fn prepare_context(
        &mut self,
        access: &mut impl WindowAccess,
        request: &WindowRequest,
        screens: &[Screen],
    ) {
        if self.screens != screens {
            self.resize_minimum = None;
            self.screens.clear();
            self.screens.extend_from_slice(screens);
        }
        access.set_scope(
            request.scope,
            matches!(request.operation, WindowOperation::Acquire(_)),
        );
        self.scope = request.scope;
        self.error = None;
    }
    fn execute(
        &mut self,
        access: &mut impl WindowAccess,
        request: WindowRequest,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> WindowResult {
        self.execute_prepared(access, request, screens, cancelled, None)
    }
    fn execute_prepared(
        &mut self,
        access: &mut impl WindowAccess,
        request: WindowRequest,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
        prepared: Option<Snapshot>,
    ) -> WindowResult {
        self.prepare_context(access, &request, screens);
        let mut result = Self::result_for(&request);
        let starting = match &request.operation {
            WindowOperation::BeginEdit { transaction, .. } => Some(*transaction),
            _ => None,
        };
        let standalone = request.operation.is_standalone_cycle();
        let outcome = self.apply(
            access,
            request.operation,
            screens,
            cancelled,
            &mut result,
            prepared,
        );
        if standalone {
            if let Err(error) = outcome {
                result.message = Some(error);
            }
            // No UI inventory, tab-state clone, edit history or lifecycle notifications.
            // Leave closed identities for the actual window session to consume.
            return result;
        }
        if let Err(error) = outcome {
            if !cancelled() {
                self.error = Some(error.clone());
            }
            result.message = Some(error);
            if let Some(transaction) = starting {
                result.edit = Some(Box::new(WindowEditResult::Ended {
                    transaction,
                    committed: false,
                }));
            }
        }
        self.complete_result(access, result, screens)
    }

    fn complete_result(
        &mut self,
        access: &mut impl WindowAccess,
        mut result: WindowResult,
        screens: &[Screen],
    ) -> WindowResult {
        if result.target.as_ref().map(|w| w.id) != self.target {
            result.target = self.target.and_then(|id| {
                result
                    .windows
                    .as_ref()
                    .and_then(|windows| windows.iter().find(|w| w.id == id))
                    .cloned()
                    .or_else(|| access.snapshot(id, screens).ok().map(|s| s.info))
            });
        }
        result.closed = access.take_closed();
        result.closed.append(&mut self.pending_closed);
        if !result.closed.is_empty() {
            if let Some(windows) = &mut result.windows {
                windows.retain(|window| !result.closed.contains(&window.id));
            }
            if result
                .target
                .as_ref()
                .is_some_and(|w| result.closed.contains(&w.id))
            {
                result.target = None;
            }
            for id in &result.closed {
                self.minimums.remove(id);
                self.initial.remove(id);
                self.changed_windows.remove(id);
            }
            if self
                .resize_minimum
                .is_some_and(|(id, ..)| result.closed.contains(&id))
            {
                self.resize_minimum = None;
            }
            self.cycle.retain(|id| !result.closed.contains(id));
            for (_, before) in self.history.iter_mut().chain(self.redo.iter_mut()) {
                before.retain(|s| !result.closed.contains(&s.info.id));
            }
            self.history.retain(|(_, before)| !before.is_empty());
            self.redo.retain(|(_, before)| !before.is_empty());
            if let Some(edit) = &mut self.edit {
                edit.before.retain(|s| !result.closed.contains(&s.info.id));
                edit.minimums.retain(|(id, _)| !result.closed.contains(id));
            }
        }
        if result.target.is_none() {
            self.target = None;
        }
        result.tabs = access.tab_state();
        result
    }

    fn adjustment_rect(
        &mut self,
        access: &impl WindowAccess,
        target: WindowId,
        change: WindowChange,
        group: u64,
        before: &Snapshot,
        screens: &[Screen],
    ) -> Result<Option<Rect>, String> {
        let screen = screens
            .get(before.info.screen)
            .ok_or("display is unavailable")?;
        let scale = access.logical_scale(screen);
        let base = if before.info.maximized {
            before.restored
        } else {
            before.info.bounds
        };
        let next = match change {
            WindowChange::MoveTo(point) => {
                let destination = screens
                    .iter()
                    .find(|screen| screen.bounds.contains(&point))
                    .ok_or("target display is unavailable")?;
                self.move_remainder = None;
                geometry::constrain_move(
                    Rect::new(
                        point.x - base.width / 2.0,
                        point.y - base.height / 2.0,
                        base.width,
                        base.height,
                    ),
                    destination.work_area,
                )
            }
            WindowChange::Move { dx, dy } => {
                let remainder = self
                    .move_remainder
                    .filter(|(id, gesture, dpi, last, _)| {
                        *id == target && *gesture == group && *dpi == scale && *last == base
                    })
                    .map_or(Point::default(), |(_, _, _, _, remainder)| remainder);
                let (next, remainder) = geometry::move_with_remainder(
                    base,
                    screen.work_area,
                    Point::new(dx * scale, dy * scale),
                    remainder,
                );
                self.move_remainder = Some((target, group, scale, next, remainder));
                next
            }
            WindowChange::Resize { dw, dh } => {
                if !before.info.resizable {
                    return Err("Window does not support resizing".into());
                }
                let minimum = match self.resize_minimum {
                    Some((id, cached_group, cached_screen, cached_scale, minimum))
                        if id == target
                            && cached_group == group
                            && cached_screen == before.info.screen
                            && cached_scale == scale =>
                    {
                        minimum
                    }
                    _ => {
                        let minimum = access.minimum_size(target);
                        self.resize_minimum =
                            Some((target, group, before.info.screen, scale, minimum));
                        minimum
                    }
                };
                geometry::resize_center(base, dw * scale, dh * scale, screen.work_area, minimum)
            }
            WindowChange::Place { index, gap } => {
                if !before.info.resizable {
                    return Err("Window does not support resizing".into());
                }
                geometry::placement(screen.work_area, index, gap * scale)
                    .ok_or("invalid window layout")?
            }
            WindowChange::Center => Rect::new(
                screen.work_area.center().x - base.width / 2.0,
                screen.work_area.center().y - base.height / 2.0,
                base.width,
                base.height,
            ),
            WindowChange::Screen(destination) => {
                let Some((source, dest)) =
                    window_placement::destination(screens, before.info.bounds, destination)
                else {
                    return Ok(None);
                };
                window_placement::map_between_screens(base, &screens[source], &screens[dest])
            }
            WindowChange::CycleState
            | WindowChange::ToggleMaximize
            | WindowChange::ToggleMinimize => unreachable!(),
        };
        Ok(Some(next))
    }

    fn prepare_layout(
        &mut self,
        access: &mut impl WindowAccess,
        operation: WindowOperation,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
        result: &mut WindowResult,
    ) -> Result<Vec<(Snapshot, Rect)>, String> {
        let WindowOperation::ApplyLayout {
            additional_screens,
            transaction,
            revision,
            screen,
            placements,
            gap,
            strict,
        } = operation
        else {
            return Err("Not a layout request".into());
        };
        let edit = self
            .edit
            .as_mut()
            .filter(|edit| edit.id == transaction)
            .ok_or("Window edit expired")?;
        if revision <= edit.revision {
            return Err("Stale layout revision".into());
        }
        edit.revision = revision;
        let count = placements.len()
            + additional_screens
                .iter()
                .map(|s| s.placements.len())
                .sum::<usize>();
        let mut batch = Vec::with_capacity(count);
        let mut seen = std::collections::BTreeSet::new();
        let layouts =
            std::iter::once(crate::api::window::WindowScreenLayout { screen, placements })
                .chain(additional_screens);
        for layout in layouts {
            let display = screens.get(layout.screen).ok_or("display is unavailable")?;
            let gap = gap * access.logical_scale(display);
            for (id, rect) in layout.placements {
                if !seen.insert(id) {
                    return Err("Duplicate window in layout transaction".into());
                }
                if cancelled() {
                    return Ok(batch);
                }
                if !edit.before.iter().any(|s| s.info.id == id) {
                    return Err("Window is outside the edit transaction".into());
                }
                let Ok(before) = access.snapshot(id, screens) else {
                    result.skipped += 1;
                    continue;
                };
                if !before.info.resizable || before.info.fullscreen {
                    return Err("Window does not support this layout".into());
                }
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
                let mut requested =
                    crate::api::window_layout::placed_rect(display.work_area, rect, gap);
                if !strict {
                    let minimum = edit
                        .minimums
                        .iter()
                        .find(|(w, _)| *w == id)
                        .map_or(Point::new(100.0, 80.0), |(_, min)| *min);
                    let center = requested.center();
                    requested.width = requested.width.max(minimum.x);
                    requested.height = requested.height.max(minimum.y);
                    requested.x = center.x - requested.width / 2.0;
                    requested.y = center.y - requested.height / 2.0;
                    requested = geometry::constrain_move(requested, display.work_area);
                }
                batch.push((before, requested));
            }
        }
        Ok(batch)
    }

    fn apply(
        &mut self,
        access: &mut impl WindowAccess,
        operation: WindowOperation,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
        result: &mut WindowResult,
        prepared: Option<Snapshot>,
    ) -> Result<(), String> {
        match operation {
            WindowOperation::Tabs(operation) => {
                // Pooled group constraints change when members move or dissolve.
                // Re-query before the next edit instead of retaining old limits.
                self.minimums.clear();
                let outcome = access.tab_operation(operation, screens, cancelled);
                if let Some(state) = access.tab_state() {
                    self.target = state.active.or(self.target);
                }
                result.windows = Some(self.enumerate(access, screens, cancelled)?);
                outcome?;
            }
            WindowOperation::CancelPending => {}
            WindowOperation::Acquire(point) => {
                self.target = access.acquire(point, screens)?.map(|w| w.id);
                if let Some(id) = self.target {
                    self.capture_initial(access, id, screens);
                    if !cancelled() {
                        access.activate_window(id, screens, cancelled)?;
                    }
                }
            }
            WindowOperation::Close(id) => {
                if !cancelled() {
                    access.close(id)?;
                    result.message = Some("Close requested".into());
                }
            }
            WindowOperation::Enumerate => {
                let windows = self.enumerate(access, screens, cancelled)?;
                for window in &windows {
                    if cancelled() {
                        break;
                    }
                    self.capture_initial(access, window.id, screens);
                }
                result.windows = Some(windows);
            }
            WindowOperation::Select(id) => {
                self.capture_initial(access, id, screens);
                access.snapshot(id, screens)?;
                result.message = access.activate_window(id, screens, cancelled).err();
                self.error.clone_from(&result.message);
                let after = access.snapshot(id, screens)?;
                self.target = Some(id);
                result.pointer = Some(after.info.bounds.center());
                result.target = Some(after.info);
            }
            WindowOperation::Cycle
            | WindowOperation::CyclePrevious
            | WindowOperation::CycleActive { .. }
            | WindowOperation::CycleOverlapping { .. } => {
                let standalone = operation.is_standalone_cycle();
                let overlapping = matches!(operation, WindowOperation::CycleOverlapping { .. });
                let backwards = matches!(
                    operation,
                    WindowOperation::CyclePrevious
                        | WindowOperation::CycleActive { backwards: true }
                        | WindowOperation::CycleOverlapping { backwards: true }
                );
                let mut windows = self.enumerate(access, screens, cancelled)?;
                if standalone {
                    windows.retain(|window| !window.minimized);
                    self.target = access
                        .pointer_window(screens)?
                        .filter(|id| windows.iter().any(|w| w.id == *id))
                        .or_else(|| access.focused_window(&windows));
                } else {
                    result.windows = Some(windows.clone());
                }
                if cancelled() {
                    return Ok(());
                }
                if overlapping {
                    self.overlap.get_or_insert_with(Box::default).prepare(
                        &windows,
                        self.target,
                        cancelled,
                    );
                }
                // Activation changes native Z-order. Preserve the session's
                // existing ring so successive Tabs visit every window instead
                // of oscillating between the two most recently activated ones.
                self.cycle.retain(|id| windows.iter().any(|w| w.id == *id));
                for window in &windows {
                    if !self.cycle.contains(&window.id) {
                        self.cycle.push(window.id);
                    }
                }
                if self.cycle.is_empty() {
                    return if overlapping {
                        Ok(())
                    } else {
                        Err("No ordinary windows available".into())
                    };
                }
                let tabs = access.tab_state();
                let grouped = tabs
                    .as_ref()
                    .filter(|_| !standalone || overlapping)
                    .and_then(|state| {
                        let group = state.containing(self.target?)?;
                        Some((group.active, group.members.clone()))
                    });
                let application = if grouped.is_none() || overlapping {
                    super::window_tabs::application_cycle_order(
                        &self.cycle,
                        &windows,
                        tabs.as_ref(),
                    )
                } else {
                    Vec::new()
                };
                let cycle = grouped
                    .as_ref()
                    .map_or(application.as_slice(), |(_, members)| members.as_slice());
                let current = grouped.as_ref().map(|(active, _)| *active).or(self.target);
                // Prefer the current tab group, then scan every other candidate.
                // Both passes stay in the overlap component; no match is a no-op.
                for pass in 0..(1 + usize::from(overlapping && grouped.is_some())) {
                    let cycle = if pass == 1 {
                        application.as_slice()
                    } else {
                        cycle
                    };
                    let start = current
                        .and_then(|id| cycle.iter().position(|v| *v == id))
                        .map_or(0, |index| {
                            if backwards {
                                (index + cycle.len() - 1) % cycle.len()
                            } else {
                                (index + 1) % cycle.len()
                            }
                        });
                    for offset in 0..cycle.len() {
                        if cancelled() {
                            return Ok(());
                        }
                        let index = if backwards {
                            (start + cycle.len() - offset) % cycle.len()
                        } else {
                            (start + offset) % cycle.len()
                        };
                        let id = cycle[index];
                        if pass == 1
                            && grouped
                                .as_ref()
                                .is_some_and(|(_, members)| members.contains(&id))
                        {
                            continue;
                        }
                        if overlapping
                            && (Some(id) == self.target
                                || self
                                    .overlap
                                    .as_ref()
                                    .is_none_or(|cache| !cache.contains(id)))
                        {
                            continue;
                        }
                        match access.snapshot(id, screens) {
                            Ok(snapshot) if !overlapping || !snapshot.info.minimized => {}
                            Ok(_) => continue,
                            Err(_) => {
                                result.skipped += 1;
                                continue;
                            }
                        }
                        // Focus permission is independent of the selected target.
                        // Do not silently cycle all the way back to the old window
                        // when the OS denies foreground activation.
                        let activation = access.activate_window(id, screens, cancelled);
                        if let Ok(after) = access.snapshot(id, screens) {
                            self.target = Some(id);
                            result.pointer = Some(after.info.bounds.center());
                            result.message = activation.err();
                            self.error.clone_from(&result.message);
                            result.target = Some(after.info);
                            return Ok(());
                        }
                        result.skipped += 1;
                    }
                }
                return if overlapping {
                    Ok(())
                } else {
                    Err("No available window accepted activation".into())
                };
            }
            WindowOperation::BeginEdit {
                transaction,
                mut targets,
                screen,
                group,
            } => {
                if self.edit.is_some() {
                    return Err("Finish the current window edit first".into());
                }
                let inventory = if let Some(screen) = screen {
                    let windows = self.enumerate(access, screens, cancelled)?;
                    targets = windows
                        .iter()
                        .filter(|w| {
                            (self.scope.is_some_and(|scope| scope.screen.is_none())
                                || w.screen == screen)
                                && self.scope.is_none_or(|scope| scope.contains(w))
                                && w.resizable
                                && !w.fullscreen
                        })
                        .filter(|w| access.layout_representative(w.id) == w.id)
                        .map(|w| w.id)
                        .collect();
                    Some(windows)
                } else {
                    None
                };
                let mut before = Vec::with_capacity(targets.len());
                let mut minimums = Vec::with_capacity(targets.len());
                for id in targets {
                    if cancelled() {
                        break;
                    }
                    if before.iter().any(|s: &Snapshot| s.info.id == id) {
                        continue;
                    }
                    if let Ok(snapshot) = access.snapshot(id, screens) {
                        self.initial.entry(id).or_insert_with(|| (&snapshot).into());
                        let queried = access.minimum_size(id);
                        let cached = self.minimums.entry(id).or_insert(queried);
                        *cached = Point::new(cached.x.max(queried.x), cached.y.max(queried.y));
                        minimums.push((id, *cached));
                        before.push(snapshot);
                    }
                }
                let gap_scale = before
                    .first()
                    .and_then(|s| screens.get(s.info.screen))
                    .map_or(1.0, |s| access.logical_scale(s));
                result.windows = Some(
                    inventory.unwrap_or_else(|| before.iter().map(|s| s.info.clone()).collect()),
                );
                result.edit = Some(Box::new(WindowEditResult::Started {
                    transaction,
                    minimums: minimums.clone(),
                    gap_scale,
                    screen_scales: screens
                        .iter()
                        .map(|screen| access.logical_scale(screen))
                        .collect(),
                    full_inventory: screen.is_some(),
                }));
                self.edit = Some(EditTransaction {
                    id: transaction,
                    group,
                    revision: 0,
                    before: before.into_iter().map(PlacementSnapshot::from).collect(),
                    minimums,
                });
            }
            WindowOperation::ApplyLayout {
                additional_screens,
                transaction,
                revision,
                screen,
                placements,
                gap,
                strict,
            } => {
                result.edit = Some(Box::new(WindowEditResult::Applied {
                    transaction,
                    revision,
                    accepted: false,
                    minimums: Vec::new(),
                }));
                let batch = self.prepare_layout(
                    access,
                    WindowOperation::ApplyLayout {
                        additional_screens,
                        transaction,
                        revision,
                        screen,
                        placements,
                        gap,
                        strict,
                    },
                    screens,
                    cancelled,
                    result,
                )?;
                let edit = self.edit.as_mut().ok_or("Window edit expired")?;
                let mut failure = None;
                let mut restore_failed = false;
                let mut observed_minimum = None;
                let mut actual = Vec::with_capacity(batch.len());
                let mut attempted = 0;
                for (index, (before, requested)) in batch.iter().enumerate() {
                    if cancelled() {
                        failure = Some("Layout cancelled".into());
                        break;
                    }
                    if rect_matches(before.info.bounds, *requested)
                        && !before.info.maximized
                        && !before.info.minimized
                    {
                        actual.push(before.info.clone());
                        continue;
                    }
                    attempted = index + 1;
                    // Adapters return the acknowledged native geometry. A second
                    // full snapshot here only repeats title/AX/process queries.
                    match access.set_frame(before.info.id, *requested, screens, cancelled) {
                        Ok(after) if !strict || rect_matches(after.bounds, *requested) => {
                            result.changed += 1;
                            actual.push(after);
                        }
                        Ok(after) => {
                            // An unchanged or still-maximized frame is not evidence
                            // of an application's minimum size (restore may lag).
                            observed_minimum = (!after.maximized).then_some((
                                after.id,
                                Point::new(
                                    if after.bounds.width > requested.width + 1.5
                                        && (after.bounds.width - before.info.bounds.width).abs()
                                            > 1.5
                                    {
                                        after.bounds.width
                                    } else {
                                        0.0
                                    },
                                    if after.bounds.height > requested.height + 1.5
                                        && (after.bounds.height - before.info.bounds.height).abs()
                                            > 1.5
                                    {
                                        after.bounds.height
                                    } else {
                                        0.0
                                    },
                                ),
                            ));
                            failure = Some("Application rejected the requested size; restored the previous layout".into());
                            break;
                        }
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                if failure.is_some() {
                    // Restore only writes that were attempted, including the one
                    // whose acknowledgement failed after a possible partial move.
                    for (before, requested) in batch[..attempted].iter().rev() {
                        if rect_matches(before.info.bounds, *requested)
                            && !before.info.maximized
                            && !before.info.minimized
                        {
                            continue;
                        }
                        match access.restore(before, screens, &|| false) {
                            Ok(after) if rect_matches(after.bounds, before.info.bounds) => {}
                            Ok(_) => {
                                restore_failed = true;
                                result.skipped += 1;
                            }
                            Err(error) => {
                                result.skipped += 1;
                                if access.snapshot(before.info.id, screens).is_ok() {
                                    restore_failed = true;
                                    crate::report_error!(
                                        "window-worker",
                                        "restore window {:?}: {error}",
                                        before.info.id
                                    );
                                }
                            }
                        }
                    }
                    result.changed = 0;
                    if !cancelled() {
                        for (id, min) in &mut edit.minimums {
                            let queried = access.minimum_size(*id);
                            let observed = observed_minimum
                                .filter(|(window, _)| window == id)
                                .map_or(Point::new(0.0, 0.0), |(_, min)| min);
                            *min = Point::new(
                                queried.x.max(observed.x).max(min.x),
                                queried.y.max(observed.y).max(min.y),
                            );
                            self.minimums.insert(*id, *min);
                        }
                    }
                    actual = edit
                        .before
                        .iter()
                        .filter_map(|s| access.snapshot(s.info.id, screens).ok().map(|s| s.info))
                        .collect();
                }
                result.windows = Some(actual);
                result.edit = Some(Box::new(WindowEditResult::Applied {
                    transaction,
                    revision,
                    accepted: failure.is_none(),
                    minimums: if failure.is_some() {
                        edit.minimums.clone()
                    } else {
                        Vec::new()
                    },
                }));
                if restore_failed {
                    self.recover_edit(access);
                    result.edit = Some(Box::new(WindowEditResult::Ended {
                        transaction,
                        committed: false,
                    }));
                    return Err("Application rejected restoration; edit ended and entry-layout recovery was attempted".into());
                }
                if let Some(error) = failure {
                    return Err(error);
                }
                if result.changed > 0 {
                    self.redo.clear();
                }
            }
            WindowOperation::EndEdit {
                transaction,
                commit,
            } => {
                let Some(edit) = self.edit.take() else {
                    result.edit = Some(Box::new(WindowEditResult::Ended {
                        transaction,
                        committed: commit,
                    }));
                    return Ok(());
                };
                if edit.id != transaction {
                    self.edit = Some(edit);
                    return Err("Window edit expired".into());
                }
                let mut actual = Vec::with_capacity(edit.before.len());
                for before in edit.before {
                    if commit {
                        if let Ok(after) = access.snapshot(before.info.id, screens) {
                            if after.info.bounds != before.info.bounds
                                || after.info.maximized != before.info.maximized
                            {
                                self.remember(edit.group, before);
                                result.changed += 1;
                            }
                            actual.push(after.info);
                        }
                    } else {
                        match before.restore(access, screens, &|| false) {
                            Ok(info) => {
                                actual.push(info);
                                result.changed += 1;
                            }
                            Err(error) => {
                                result.skipped += 1;
                                crate::report_error!(
                                    "window-worker",
                                    "restore window {:?}: {error}",
                                    before.info.id
                                );
                            }
                        }
                    }
                }
                result.windows = Some(actual);
                result.edit = Some(Box::new(WindowEditResult::Ended {
                    transaction,
                    committed: commit,
                }));
                if result.skipped > 0 {
                    result.message = Some(format!(
                        "Restored {} · unavailable {}",
                        result.changed, result.skipped
                    ));
                }
            }
            WindowOperation::Adjust {
                target,
                change,
                group,
            } => {
                let before = match prepared {
                    Some(snapshot) => snapshot,
                    None => access.snapshot(target, screens)?,
                };
                if before.info.fullscreen {
                    if let WindowChange::Screen(destination) = change {
                        let (after, pointer) =
                            access.move_fullscreen(target, destination, screens, cancelled)?;
                        if before.info.bounds != after.bounds {
                            self.remember(group, before);
                            result.changed = 1;
                        }
                        result.pointer = pointer;
                        return Ok(());
                    }
                    return Err("Exit native fullscreen before adjusting this window".into());
                }
                let base = if before.info.maximized {
                    before.restored
                } else {
                    before.info.bounds
                };
                // Continuous geometry must not replay a stale cursor position.
                let pointer = (!matches!(
                    change,
                    WindowChange::Move { .. }
                        | WindowChange::MoveTo(_)
                        | WindowChange::Resize { .. }
                ))
                .then(|| access.pointer().ok())
                .flatten();
                let resize_center =
                    matches!(change, WindowChange::Resize { .. }).then_some(base.center());
                let change_result = match change {
                    WindowChange::CycleState => access.cycle_state(target, screens, cancelled),
                    WindowChange::ToggleMaximize => {
                        access.toggle_state(target, false, screens, cancelled)
                    }
                    WindowChange::ToggleMinimize => {
                        access.toggle_state(target, true, screens, cancelled)
                    }
                    change => {
                        let Some(next) =
                            self.adjustment_rect(access, target, change, group, &before, screens)?
                        else {
                            return Ok(());
                        };
                        if cancelled() {
                            return Ok(());
                        }
                        if next == base && !before.info.maximized && !before.info.minimized {
                            return Ok(());
                        }
                        access.set_frame(target, next, screens, cancelled)
                    }
                };
                if let Some(center) = resize_center
                    && let Ok(after) = &change_result
                    && after.bounds.center().distance_to(&center) > 1.0
                    && !cancelled()
                    && let Err(error) = access.set_frame(
                        target,
                        Rect::new(
                            center.x - after.bounds.width / 2.0,
                            center.y - after.bounds.height / 2.0,
                            after.bounds.width,
                            after.bounds.height,
                        ),
                        screens,
                        cancelled,
                    )
                    && !cancelled()
                {
                    crate::report_error!("window-worker", "resize center correction: {error}");
                }
                // Read even after a rejected acknowledgement: native apps may have
                // applied part of the operation, and that part must remain undoable.
                if let Ok(after) = access.snapshot(target, screens)
                    && (after.info.bounds != before.info.bounds
                        || after.info.maximized != before.info.maximized
                        || after.info.minimized != before.info.minimized)
                {
                    self.remember(group, PlacementSnapshot::from(&before));
                    result.changed = 1;
                    if !after.info.minimized
                        && let Some(pointer) = pointer
                        && let Some(screen) = screens.get(after.info.screen)
                    {
                        result.pointer = Some(window_placement::following_pointer(
                            pointer,
                            before.info.bounds,
                            after.info.bounds,
                            screen.bounds,
                        ));
                    }
                }
                if change_result.is_err() {
                    self.move_remainder = None;
                }
                change_result?;
                self.target = Some(target);
            }
            WindowOperation::Tile { target, gap, group } => {
                let target_screen = access.snapshot(target, screens)?.info.screen;
                let mut windows = self.enumerate(access, screens, cancelled)?;
                if self.scope.is_none_or(|scope| scope.screen.is_some()) {
                    windows.retain(|w| w.screen == target_screen);
                }
                result.skipped = windows
                    .iter()
                    .filter(|w| !w.resizable || w.fullscreen)
                    .count();
                windows.retain(|w| w.resizable && !w.fullscreen);
                windows.retain(|w| access.layout_representative(w.id) == w.id);
                windows.sort_by_key(|w| w.id != target);
                let mut placements = Vec::with_capacity(windows.len());
                let mut overlaps = false;
                for (index, screen) in screens.iter().enumerate() {
                    let members: Vec<_> = windows.iter().filter(|w| w.screen == index).collect();
                    let mut minimums = Vec::with_capacity(members.len());
                    for window in &members {
                        if cancelled() {
                            return Ok(());
                        }
                        minimums.push(access.minimum_size(window.id));
                    }
                    let areas = geometry::tile_with_minimums(
                        screen.work_area,
                        &minimums,
                        gap * access.logical_scale(screen),
                    );
                    overlaps |= areas.iter().enumerate().any(|(i, cell)| {
                        areas[i + 1..]
                            .iter()
                            .any(|other| cell.intersect(other).is_some())
                    });
                    placements.extend(
                        members
                            .into_iter()
                            .zip(areas)
                            .map(|(window, area)| (window, area, screen)),
                    );
                }
                let mut arranged = 0;
                for (window, area, screen) in placements {
                    if cancelled() {
                        break;
                    }
                    let Ok(before) = access.snapshot(window.id, screens) else {
                        result.skipped += 1;
                        continue;
                    };
                    // Submit every eligible window. Apps clamp their own minimum
                    // size; skipping a small cell leaves entire windows untouched.
                    let applied = access.set_frame(window.id, area, screens, cancelled);
                    if let Err(error) = &applied
                        && !cancelled()
                    {
                        crate::report_error!(
                            "window-worker",
                            "tile window {:?}: {error}",
                            window.id
                        );
                    }
                    if let Ok(after) = &applied {
                        let positioned = geometry::constrain_move(after.bounds, screen.work_area);
                        if positioned != after.bounds
                            && !cancelled()
                            && let Err(error) =
                                access.set_frame(window.id, positioned, screens, cancelled)
                            && !cancelled()
                        {
                            crate::report_error!(
                                "window-worker",
                                "tile position correction: {error}"
                            );
                        }
                    }
                    match access.snapshot(window.id, screens) {
                        Ok(after)
                            if after.info.bounds != before.info.bounds
                                || after.info.maximized != before.info.maximized =>
                        {
                            self.remember(group, before);
                            result.changed += 1;
                            arranged += 1;
                        }
                        Ok(after) if applied.is_ok() && after.info.bounds == area => arranged += 1,
                        _ => result.skipped += 1,
                    }
                }
                result.message = Some(format!(
                    "Arranged {arranged} · skipped {}{}",
                    result.skipped,
                    if overlaps {
                        " · minimum sizes require overlap"
                    } else {
                        ""
                    }
                ));
            }
            operation @ (WindowOperation::Undo | WindowOperation::Redo) => {
                if self.edit.is_some() {
                    return Err("Finish the current window edit first".into());
                }
                let redo = matches!(operation, WindowOperation::Redo);
                let source = if redo {
                    &mut self.redo
                } else {
                    &mut self.history
                };
                if let Some((group, snapshots)) = source.pop_back() {
                    let (inverse, remaining) =
                        Self::restore_snapshots(access, snapshots, screens, cancelled, result);
                    let (source, destination) = if redo {
                        (&mut self.redo, &mut self.history)
                    } else {
                        (&mut self.history, &mut self.redo)
                    };
                    Self::push_history(source, group, remaining);
                    Self::push_history(destination, group, inverse);
                    result.windows = Some(self.enumerate(access, screens, cancelled)?);
                    result.message = Some(format!(
                        "Restored {} · skipped {}",
                        result.changed, result.skipped
                    ));
                } else {
                    result.message = Some(
                        if redo {
                            "Nothing to redo"
                        } else {
                            "Nothing to undo"
                        }
                        .into(),
                    );
                }
            }
            WindowOperation::ResetInitial { group } => {
                if self.edit.is_some() {
                    return Err("Finish the current window edit first".into());
                }
                let snapshots = self
                    .changed_windows
                    .iter()
                    .filter_map(|id| self.initial.get(id).cloned())
                    .collect();
                let (inverse, _) =
                    Self::restore_snapshots(access, snapshots, screens, cancelled, result);
                if !inverse.is_empty() {
                    self.redo.clear();
                    Self::push_history(&mut self.history, group, inverse);
                }
                result.windows = Some(self.enumerate(access, screens, cancelled)?);
                result.message = Some(format!(
                    "Initial state · restored {} · skipped {}",
                    result.changed, result.skipped
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Fake {
        reset_notification: Option<std::sync::mpsc::Sender<()>>,
        windows: BTreeMap<WindowId, Snapshot>,
        writes: Vec<WindowId>,
        reject: Option<WindowId>,
        partial: bool,
        refuse_focus: bool,
        selected: std::cell::Cell<Option<WindowId>>,
        pointer_target: Option<WindowId>,
        close_requests: std::cell::RefCell<Vec<WindowId>>,
        volume_requests: std::cell::RefCell<Vec<(WindowId, crate::api::audio::AudioAction)>>,
        unchanged_ack: bool,
        snapshot_unavailable: bool,
        snapshot_reads: std::cell::Cell<usize>,
        decline_submission: bool,
        deferred: bool,
        async_states: bool,
        submitted_states: BTreeMap<WindowId, bool>,
        submitted: BTreeMap<WindowId, Rect>,
        minimum: Point,
        minimum_queries: std::cell::Cell<usize>,
        closed: Vec<WindowId>,
    }

    fn screens() -> Vec<Screen> {
        vec![
            Screen {
                bounds: Rect::new(-1200.0, 0.0, 1200.0, 900.0),
                work_area: Rect::new(-1200.0, 30.0, 1200.0, 870.0),
                scale: 2.0,
                is_primary: true,
                name: None,
            },
            Screen {
                bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                work_area: Rect::new(0.0, 0.0, 1000.0, 760.0),
                scale: 1.0,
                is_primary: false,
                name: None,
            },
        ]
    }

    impl Fake {
        fn new(count: u64) -> Self {
            Self {
                reset_notification: None,
                windows: (1..=count)
                    .map(|id| {
                        let bounds = Rect::new(-1100.0 + id as f64 * 20.0, 150.0, 320.0, 240.0);
                        (
                            WindowId(id),
                            Snapshot {
                                info: WindowInfo {
                                    id: WindowId(id),
                                    title: format!("Window {id}"),
                                    app: "test".into(),
                                    bounds,
                                    screen: 0,
                                    resizable: true,
                                    maximized: false,
                                    minimized: false,
                                    fullscreen: false,
                                },
                                restored: bounds,
                            },
                        )
                    })
                    .collect(),
                writes: Vec::new(),
                reject: None,
                partial: false,
                refuse_focus: false,
                selected: std::cell::Cell::new(None),
                pointer_target: None,
                close_requests: Default::default(),
                volume_requests: Default::default(),
                unchanged_ack: false,
                snapshot_unavailable: false,
                snapshot_reads: std::cell::Cell::new(0),
                decline_submission: false,
                deferred: false,
                async_states: false,
                submitted_states: BTreeMap::new(),
                submitted: BTreeMap::new(),
                minimum: Point::new(100.0, 80.0),
                minimum_queries: std::cell::Cell::new(0),
                closed: Vec::new(),
            }
        }
    }

    impl WindowAccess for Fake {
        fn focused_bounds(&self, process: u32) -> Result<Option<Rect>, String> {
            Ok(Some(Rect::new(f64::from(process), 0.0, 100.0, 100.0)))
        }
        fn pointer_window(&mut self, _: &[Screen]) -> Result<Option<WindowId>, String> {
            Ok(self.pointer_target)
        }
        fn focused_window(&self, _windows: &[WindowInfo]) -> Option<WindowId> {
            self.selected.get()
        }
        fn acquire(&mut self, _: Point, _: &[Screen]) -> Result<Option<WindowInfo>, String> {
            Ok(self.windows.values().next().map(|s| s.info.clone()))
        }
        fn enumerate(
            &mut self,
            _: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<Vec<WindowInfo>, String> {
            Ok(self
                .windows
                .values()
                .take_while(|_| !cancelled())
                .map(|s| s.info.clone())
                .collect())
        }
        fn snapshot(&self, id: WindowId, _: &[Screen]) -> Result<Snapshot, String> {
            self.snapshot_reads.set(self.snapshot_reads.get() + 1);
            if self.snapshot_unavailable {
                return Err("temporarily unavailable".into());
            }
            self.windows.get(&id).cloned().ok_or("closed".into())
        }
        fn can_submit_frame(&self, _: WindowId) -> bool {
            self.deferred
        }
        fn refresh_geometry(&self, snapshot: &mut Snapshot, _: &[Screen]) -> Result<(), String> {
            if self.snapshot_unavailable {
                return Err("temporarily unavailable".into());
            }
            let current = self.windows.get(&snapshot.info.id).ok_or("closed")?;
            snapshot.info.bounds = current.info.bounds;
            snapshot.info.maximized = current.info.maximized;
            snapshot.info.minimized = current.info.minimized;
            snapshot.info.fullscreen = current.info.fullscreen;
            snapshot.restored = current.restored;
            Ok(())
        }
        fn submit_maximize(
            &mut self,
            before: &Snapshot,
            maximize: bool,
            _: &[Screen],
        ) -> Result<bool, String> {
            if !self.async_states {
                return Ok(false);
            }
            self.submitted_states.insert(before.info.id, maximize);
            Ok(true)
        }
        fn submit_maximized_frame(
            &mut self,
            before: &Snapshot,
            rect: Rect,
            screens: &[Screen],
        ) -> Result<bool, String> {
            self.submit_frame(before, rect, screens)
        }
        fn submit_frame(
            &mut self,
            before: &Snapshot,
            rect: Rect,
            _: &[Screen],
        ) -> Result<bool, String> {
            let id = before.info.id;
            if !self.deferred || self.decline_submission {
                return Ok(false);
            }
            self.submitted.insert(id, rect);
            if self.reject == Some(id) {
                return Err("asynchronous write rejected".into());
            }
            Ok(true)
        }
        fn set_frame(
            &mut self,
            id: WindowId,
            rect: Rect,
            screens: &[Screen],
            cancelled: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            if cancelled() {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            if self.unchanged_ack {
                return self.snapshot(id, screens).map(|s| s.info);
            }
            if self.reject != Some(id) || self.partial {
                let s = self.windows.get_mut(&id).ok_or("closed")?;
                s.info.bounds = rect;
                s.info.maximized = false;
                s.info.minimized = false;
                s.info.screen = geometry::screen_index(screens, rect).unwrap();
                s.restored = rect;
                self.writes.push(id);
            }
            if self.reject == Some(id) {
                return Err("app refused acknowledgement".into());
            }
            self.snapshot(id, screens).map(|s| s.info)
        }
        fn restore(
            &mut self,
            before: &Snapshot,
            _: &[Screen],
            _: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            let value = self.windows.get_mut(&before.info.id).ok_or("closed")?;
            *value = before.clone();
            self.writes.push(before.info.id);
            Ok(value.info.clone())
        }
        fn cycle_state(
            &mut self,
            id: WindowId,
            screens: &[Screen],
            _: &dyn Fn() -> bool,
        ) -> Result<WindowInfo, String> {
            let s = self.windows.get_mut(&id).ok_or("closed")?;
            if s.info.minimized {
                s.info.minimized = false;
                s.info.bounds = s.restored;
                s.info.maximized = false;
            } else if s.info.maximized {
                s.info.minimized = true;
            } else {
                s.restored = s.info.bounds;
                s.info.bounds = screens[s.info.screen].work_area;
                s.info.maximized = true;
            }
            Ok(s.info.clone())
        }
        fn system_audio(&self, _change: crate::api::audio::AudioAction) -> Result<String, String> {
            Err("System audio control is unavailable on this backend".into())
        }
        fn volume(
            &self,
            id: WindowId,
            change: crate::api::audio::AudioAction,
        ) -> Result<String, String> {
            self.windows.get(&id).ok_or("closed")?;
            self.volume_requests.borrow_mut().push((id, change));
            Ok("App volume 45%".into())
        }
        fn close(&self, id: WindowId) -> Result<(), String> {
            self.close_requests.borrow_mut().push(id);
            Ok(())
        }
        fn select(&self, id: WindowId) -> Result<(), String> {
            self.selected.set(Some(id));
            if self.refuse_focus {
                Err("focus denied".into())
            } else {
                Ok(())
            }
        }
        fn minimum_size(&self, _: WindowId) -> Point {
            self.minimum_queries.set(self.minimum_queries.get() + 1);
            self.minimum
        }
        fn take_closed(&mut self) -> Vec<WindowId> {
            std::mem::take(&mut self.closed)
        }
        fn pointer(&self) -> Result<Point, String> {
            Ok(Point::new(-1000.0, 200.0))
        }
        fn logical_scale(&self, screen: &Screen) -> f64 {
            screen.scale
        }
        fn reset(&mut self) {
            if let Some(tx) = &self.reset_notification {
                self.windows.clear();
                let _ = tx.send(());
            }
        }
    }

    fn run(session: &mut Session, access: &mut Fake, operation: WindowOperation) -> WindowResult {
        session.execute(
            access,
            WindowRequest {
                scope: None,
                session: 1,
                id: 1,
                operation,
            },
            &screens(),
            &|| false,
        )
    }

    #[test]
    fn maximize_and_minimize_toggle_directly_back_to_restored_geometry() {
        for minimize in [false, true] {
            let mut access = Fake::new(2);
            let id = WindowId(1);
            let before = access.snapshot(id, &screens()).unwrap();
            for _ in 0..3 {
                let changed = access
                    .toggle_state(id, minimize, &screens(), &|| false)
                    .unwrap();
                assert_eq!(changed.minimized, minimize);
                assert_eq!(changed.maximized, !minimize);
                let restored = access
                    .toggle_state(id, minimize, &screens(), &|| false)
                    .unwrap();
                assert!(!restored.minimized && !restored.maximized);
                assert_eq!(restored.bounds, before.info.bounds);
            }
        }
    }

    #[test]
    fn acquire_activates_pointer_target_without_warp_or_geometry_change() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        assert_eq!(access.selected.get(), Some(WindowId(1)));
        assert_eq!(result.target.as_ref().map(|w| w.id), Some(WindowId(1)));
        assert!(result.pointer.is_none());
        assert!(result.message.is_none());
        assert!(access.writes.is_empty());
        assert!(session.history.is_empty());

        access.refuse_focus = true;
        let denied = run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        assert_eq!(denied.message.as_deref(), Some("focus denied"));
        assert!(denied.pointer.is_none());
    }

    #[test]
    fn close_requests_only_the_target_and_preserves_it_until_application_closes() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::Close(WindowId(1)),
        );
        assert_eq!(*access.close_requests.borrow(), vec![WindowId(1)]);
        assert_eq!(result.message.as_deref(), Some("Close requested"));
        assert!(result.closed.is_empty());
        assert_eq!(result.target.as_ref().map(|w| w.id), Some(WindowId(1)));
        assert!(session.history.is_empty());
        assert!(access.writes.is_empty());
        access.windows.remove(&WindowId(1));
        access.closed.push(WindowId(1));
        let result = run(&mut session, &mut access, WindowOperation::Enumerate);
        assert!(result.target.is_none());
        assert_eq!(result.closed, vec![WindowId(1)]);
        assert_eq!(result.windows.unwrap().len(), 1);
    }

    #[test]
    fn retired_tray_identity_cannot_restore_target_from_live_snapshot() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        access.closed.push(WindowId(1));
        let result = run(&mut session, &mut access, WindowOperation::Enumerate);
        assert!(access.windows.contains_key(&WindowId(1)));
        assert_eq!(result.closed, [WindowId(1)]);
        assert!(result.target.is_none());
        assert!(session.target.is_none());
        assert!(result.windows.unwrap().iter().all(|w| w.id != WindowId(1)));
    }

    #[test]
    fn continuous_move_preserves_cursor_and_skips_subpixel_native_writes() {
        let mut access = Fake::new(1);
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        let before = access.windows[&WindowId(1)].info.bounds;
        let writes = access.writes.len();
        for _ in 0..10 {
            let result = run(
                &mut session,
                &mut access,
                WindowOperation::Adjust {
                    target: WindowId(1),
                    change: WindowChange::Move { dx: 0.05, dy: 0.0 },
                    group: 99,
                },
            );
            assert!(result.pointer.is_none());
        }
        assert_eq!(access.windows[&WindowId(1)].info.bounds.x, before.x + 1.0);
        assert_eq!(access.writes.len(), writes + 1);
    }

    #[test]
    fn audio_dispatch_does_not_enter_or_end_a_layout_transaction() {
        use crate::api::audio::{AudioAction, AudioRequest, AudioTarget};
        let mut access = Fake::new(2);
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        run(
            &mut session,
            &mut access,
            WindowOperation::BeginEdit {
                transaction: 9,
                targets: vec![WindowId(1)],
                screen: None,
                group: 1,
            },
        );
        let request = AudioRequest {
            session: 55,
            id: 2,
            target: AudioTarget::Application(WindowId(1)),
            action: AudioAction::Down,
        };
        let result = execute_audio(&access, request.clone());
        assert_eq!(result.outcome.as_deref(), Ok("App volume 45%"));
        assert_eq!((result.session, result.id), (55, 2));
        assert_eq!(
            *access.volume_requests.borrow(),
            [(WindowId(1), AudioAction::Down)]
        );
        assert!(session.edit.is_some());
        assert!(session.history.is_empty());
        assert!(access.writes.is_empty());
        let result = execute_audio(
            &access,
            AudioRequest {
                target: AudioTarget::Application(WindowId(99)),
                ..request
            },
        );
        assert_eq!(result.outcome.as_deref(), Err(&"closed".to_string()));
        assert_eq!(access.volume_requests.borrow().len(), 1);
    }

    #[test]
    fn cancellation_releases_inventory_before_worker_shutdown() {
        let (reset_tx, reset_rx) = std::sync::mpsc::channel();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = WindowWorker::start(
            move || {
                let mut fake = Fake::new(4);
                fake.reset_notification = Some(reset_tx);
                fake
            },
            move |event| {
                tx.send(event).unwrap();
            },
        )
        .unwrap();
        worker
            .submit(
                WindowRequest {
                    session: 7,
                    id: 1,
                    scope: None,
                    operation: WindowOperation::Acquire(Point::new(-1000.0, 200.0)),
                },
                &screens(),
            )
            .unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(reset_rx.try_recv().is_err());
        worker.cancel(7);
        reset_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // The worker is still alive and idle; cleanup must not depend on Drop.
        worker
            .stop_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn focused_bounds_coalesces_without_blocking_submission() {
        use std::sync::mpsc;
        let (started, waiting) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let (sender, events) = mpsc::channel();
        let mut worker = WindowWorker::start(
            move || {
                started.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(5)).unwrap();
                Fake::new(0)
            },
            move |event| {
                sender.send(event).unwrap();
            },
        )
        .unwrap();
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.focused_bounds(1, 10).unwrap();
        worker.focused_bounds(2, 20).unwrap();
        release.send(()).unwrap();
        let BackendEvent::FocusedWindowBounds { id, bounds } =
            events.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("expected geometry reply");
        };
        assert_eq!(id, 2);
        assert_eq!(bounds.unwrap().unwrap().x, 20.0);
        worker
            .stop_until(Instant::now() + Duration::from_secs(5))
            .unwrap();
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn audio_worker_needs_no_window_session_and_cancels_only_its_owner() {
        use crate::api::audio::{AudioAction, AudioRequest, AudioTarget};
        use std::sync::mpsc;
        let (ready_tx, ready_rx) = mpsc::channel();
        let (start_tx, start_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let mut worker = WindowWorker::start(
            move || {
                ready_tx.send(()).unwrap();
                start_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Fake::new(0)
            },
            move |event| {
                tx.send(event).unwrap();
            },
        )
        .unwrap();
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for session in [17, 18] {
            worker
                .submit_audio(AudioRequest {
                    session,
                    id: 1,
                    target: AudioTarget::System,
                    action: AudioAction::Up,
                })
                .unwrap();
        }
        worker.cancel_audio(17);
        start_tx.send(()).unwrap();
        let BackendEvent::AudioResult(result) = rx.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("audio must have its own result channel");
        };
        assert_eq!(result.session, 18);
        assert!(result.outcome.unwrap_err().contains("unavailable"));
        worker
            .stop_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(rx.try_recv().is_err());
    }

    fn adjust(group: u64, change: WindowChange) -> WindowOperation {
        WindowOperation::Adjust {
            target: WindowId(1),
            change,
            group,
        }
    }

    #[test]
    fn targeting_move_crosses_screens_without_pointer_feedback_and_undoes_once() {
        let mut access = Fake::new(1);
        let mut session = Session::default();
        let original = access.windows[&WindowId(1)].clone();
        for point in [Point::new(-600.0, 400.0), Point::new(500.0, 400.0)] {
            let result = run(
                &mut session,
                &mut access,
                adjust(1, WindowChange::MoveTo(point)),
            );
            assert_eq!(result.changed, 1);
            assert!(result.pointer.is_none());
            assert_eq!(access.windows[&WindowId(1)].info.bounds.center(), point);
        }
        run(&mut session, &mut access, WindowOperation::Undo);
        assert!(same_placement(&access.windows[&WindowId(1)], &original));
    }

    #[test]
    fn all_screen_layout_keeps_each_display_and_rolls_back_the_whole_batch() {
        use crate::api::window::{WindowScope, WindowScreenLayout};
        for reject in [false, true] {
            let mut access = Fake::new(3);
            let other = access.windows.get_mut(&WindowId(2)).unwrap();
            other.info.screen = 1;
            other.info.bounds = Rect::new(100.0, 100.0, 320.0, 240.0);
            other.restored = other.info.bounds;
            access.windows.get_mut(&WindowId(3)).unwrap().info.minimized = true;
            let original = access.windows.clone();
            let mut session = Session::default();
            let mut execute = |access: &mut Fake, operation| {
                session.execute(
                    access,
                    WindowRequest {
                        scope: Some(WindowScope {
                            screen: None,
                            include_minimized: false,
                        }),
                        session: 1,
                        id: 1,
                        operation,
                    },
                    &screens(),
                    &|| false,
                )
            };
            let started = execute(
                &mut access,
                WindowOperation::BeginEdit {
                    transaction: 1,
                    targets: Vec::new(),
                    screen: Some(0),
                    group: 1,
                },
            );
            assert_eq!(started.windows.unwrap().len(), 2);
            access.reject = reject.then_some(WindowId(2));
            access.partial = reject;
            let result = execute(
                &mut access,
                WindowOperation::ApplyLayout {
                    transaction: 1,
                    revision: 1,
                    screen: 0,
                    placements: vec![(WindowId(1), Rect::new(0.0, 0.0, 1.0, 1.0))],
                    additional_screens: vec![WindowScreenLayout {
                        screen: 1,
                        placements: vec![(WindowId(2), Rect::new(0.0, 0.0, 1.0, 1.0))],
                    }],
                    gap: 0.0,
                    strict: true,
                },
            );
            assert_eq!(result.message.is_some(), reject);
            assert_eq!(
                access.windows[&WindowId(3)].info,
                original[&WindowId(3)].info
            );
            for (id, screen) in [(WindowId(1), 0), (WindowId(2), 1)] {
                assert_eq!(access.windows[&id].info.screen, screen);
                assert_eq!(
                    access.windows[&id].info.bounds,
                    if reject {
                        original[&id].info.bounds
                    } else {
                        screens()[screen].work_area
                    }
                );
            }
            execute(
                &mut access,
                WindowOperation::EndEdit {
                    transaction: 1,
                    commit: true,
                },
            );
            if !reject {
                execute(&mut access, WindowOperation::Undo);
                for id in [WindowId(1), WindowId(2)] {
                    assert_eq!(access.windows[&id].info, original[&id].info);
                }
            }
        }
    }

    #[test]
    fn resize_queries_constraints_once_per_gesture_and_refreshes_next_gesture() {
        let mut access = Fake::new(1);
        let mut session = Session::default();
        for _ in 0..20 {
            run(
                &mut session,
                &mut access,
                adjust(1, WindowChange::Resize { dw: 1.0, dh: 1.0 }),
            );
        }
        assert_eq!(access.minimum_queries.get(), 1);
        access.minimum = Point::new(500.0, 400.0);
        run(
            &mut session,
            &mut access,
            adjust(2, WindowChange::Resize { dw: -1.0, dh: -1.0 }),
        );
        assert_eq!(access.minimum_queries.get(), 2);
        assert!(access.windows[&WindowId(1)].info.bounds.width >= 500.0);
    }

    #[test]
    fn closed_windows_release_session_state_and_cancelled_delivery_is_replayed() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            adjust(1, WindowChange::Resize { dw: 1.0, dh: 1.0 }),
        );
        begin_edit(&mut session, &mut access);
        session.cycle = vec![WindowId(1), WindowId(2)];
        access.windows.remove(&WindowId(1));
        access.closed.push(WindowId(1));
        let result = run(&mut session, &mut access, WindowOperation::Enumerate);
        assert_eq!(result.closed, vec![WindowId(1)]);
        assert!(session.history.is_empty());
        assert!(session.resize_minimum.is_none());
        assert!(!session.minimums.contains_key(&WindowId(1)));
        assert_eq!(session.cycle, vec![WindowId(2)]);
        assert_eq!(session.edit.as_ref().unwrap().before.len(), 1);
        session.pending_closed.extend(result.closed);
        let delivered = run(&mut session, &mut access, WindowOperation::Enumerate);
        assert_eq!(delivered.closed, vec![WindowId(1)]);
        assert!(
            run(&mut session, &mut access, WindowOperation::Enumerate)
                .closed
                .is_empty()
        );
    }

    #[test]
    fn relative_gesture_scales_logical_pixels_and_undo_restores_first_snapshot() {
        let mut access = Fake::new(1);
        let original = access.windows[&WindowId(1)].info.bounds;
        let mut session = Session::default();
        for _ in 0..4 {
            run(
                &mut session,
                &mut access,
                adjust(1, WindowChange::Move { dx: 20.0, dy: 0.0 }),
            );
        }
        assert_eq!(
            access.windows[&WindowId(1)].info.bounds.x,
            original.x + 160.0
        );
        assert_eq!(session.history.len(), 1);
        run(&mut session, &mut access, WindowOperation::Undo);
        assert_eq!(access.windows[&WindowId(1)].info.bounds, original);
    }

    fn begin_edit(session: &mut Session, access: &mut Fake) {
        let targets = access.windows.keys().copied().collect();
        let started = run(
            session,
            access,
            WindowOperation::BeginEdit {
                transaction: 10,
                targets,
                screen: None,
                group: 10,
            },
        );
        assert!(matches!(
            started.edit.as_deref(),
            Some(WindowEditResult::Started { .. })
        ));
    }

    fn batch(revision: u64, width: f64) -> WindowOperation {
        WindowOperation::ApplyLayout {
            additional_screens: Vec::new(),
            transaction: 10,
            revision,
            screen: 0,
            gap: 8.0,
            strict: true,
            placements: vec![
                (WindowId(1), Rect::new(0.0, 0.0, width, 1.0)),
                (WindowId(2), Rect::new(width, 0.0, 1.0 - width, 1.0)),
            ],
        }
    }

    #[test]
    fn edit_commit_is_one_undo_and_unchanged_rectangles_do_not_write() {
        let mut access = Fake::new(2);
        access.windows.get_mut(&WindowId(1)).unwrap().info.maximized = true;
        let originals = access.windows.clone();
        let mut session = Session::default();
        begin_edit(&mut session, &mut access);
        assert!(
            run(&mut session, &mut access, batch(1, 0.5))
                .message
                .is_none()
        );
        let writes = access.writes.len();
        assert!(
            run(&mut session, &mut access, batch(2, 0.5))
                .message
                .is_none()
        );
        assert_eq!(access.writes.len(), writes);
        assert!(
            session.history.is_empty(),
            "unfinished edits do not leak into ordinary undo"
        );
        run(&mut session, &mut access, batch(3, 2.0 / 3.0));
        run(
            &mut session,
            &mut access,
            WindowOperation::EndEdit {
                transaction: 10,
                commit: true,
            },
        );
        assert_eq!(session.history.len(), 1);
        run(&mut session, &mut access, WindowOperation::Undo);
        for (id, before) in originals {
            assert_eq!(access.windows[&id].info, before.info);
            assert_eq!(access.windows[&id].restored, before.restored);
        }
    }

    #[test]
    fn unchanged_acknowledgements_never_poison_minimum_sizes() {
        for maximized in [false, true] {
            let mut access = Fake::new(2);
            access.windows.get_mut(&WindowId(1)).unwrap().info.maximized = maximized;
            let mut session = Session::default();
            begin_edit(&mut session, &mut access);
            access.unchanged_ack = true;
            let result = run(&mut session, &mut access, batch(1, 0.15));
            assert!(matches!(
                result.edit.as_deref(),
                Some(WindowEditResult::Applied {
                    accepted: false,
                    ..
                })
            ));
            assert_eq!(session.minimums[&WindowId(1)], Point::new(100.0, 80.0));
        }
    }

    #[test]
    fn rejected_partial_batch_restores_last_accepted_layout_then_escape_restores_entry() {
        let mut access = Fake::new(2);
        let originals = access.windows.clone();
        let mut session = Session::default();
        begin_edit(&mut session, &mut access);
        run(&mut session, &mut access, batch(1, 0.5));
        let accepted = access.windows.clone();
        access.reject = Some(WindowId(2));
        access.partial = true;
        let failed = run(&mut session, &mut access, batch(2, 2.0 / 3.0));
        assert!(matches!(
            failed.edit.as_deref(),
            Some(WindowEditResult::Applied {
                accepted: false,
                ..
            })
        ));
        for (id, before) in accepted {
            assert_eq!(access.windows[&id].info, before.info);
        }
        run(
            &mut session,
            &mut access,
            WindowOperation::EndEdit {
                transaction: 10,
                commit: false,
            },
        );
        for (id, before) in originals {
            assert_eq!(access.windows[&id].info, before.info);
        }
        assert!(session.history.is_empty());
    }

    #[test]
    fn stale_layout_and_foreign_windows_cannot_change_geometry() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        begin_edit(&mut session, &mut access);
        run(&mut session, &mut access, batch(7, 0.5));
        let writes = access.writes.len();
        assert!(
            run(&mut session, &mut access, batch(6, 0.25))
                .message
                .is_some()
        );
        assert_eq!(access.writes.len(), writes);
        let operation = WindowOperation::ApplyLayout {
            additional_screens: Vec::new(),
            transaction: 10,
            revision: 8,
            screen: 0,
            gap: 8.0,
            strict: true,
            placements: vec![(WindowId(3), Rect::new(0.0, 0.0, 1.0, 1.0))],
        };
        assert!(run(&mut session, &mut access, operation).message.is_some());
        assert_eq!(access.writes.len(), writes);
        access.windows.remove(&WindowId(2));
        let restored = run(
            &mut session,
            &mut access,
            WindowOperation::EndEdit {
                transaction: 10,
                commit: false,
            },
        );
        assert_eq!(restored.skipped, 1);
        assert_eq!(access.windows.len(), 1);
    }

    #[test]
    fn forced_session_cleanup_keeps_applied_geometry_without_native_writes() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        begin_edit(&mut session, &mut access);
        run(&mut session, &mut access, batch(1, 0.5));
        let originals = access.windows.clone();
        let writes = access.writes.len();
        session.cleanup_edit(&mut access);
        assert_eq!(access.writes.len(), writes);
        assert!(session.edit.is_none());
        for (id, before) in originals {
            assert_eq!(access.windows[&id].info, before.info);
        }
    }

    #[test]
    fn tile_is_target_first_one_undo_and_skips_other_screens_fixed_and_fullscreen() {
        let mut access = Fake::new(5);
        access.windows.get_mut(&WindowId(3)).unwrap().info.resizable = false;
        access
            .windows
            .get_mut(&WindowId(4))
            .unwrap()
            .info
            .fullscreen = true;
        let other = access.windows.get_mut(&WindowId(5)).unwrap();
        other.info.screen = 1;
        other.info.bounds.x = 100.0;
        let originals = access.windows.clone();
        let mut session = Session {
            target: Some(WindowId(2)),
            ..Session::default()
        };
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::Tile {
                target: WindowId(2),
                gap: 8.0,
                group: 1,
            },
        );
        assert_eq!((result.changed, result.skipped), (2, 2));
        assert_eq!(result.target.unwrap().id, WindowId(2));
        assert_eq!(access.writes, [WindowId(2), WindowId(1)]);
        assert!(result.pointer.is_none());
        assert_eq!(session.history.len(), 1);
        assert_eq!(
            run(&mut session, &mut access, WindowOperation::Undo).changed,
            2
        );
        for (id, snapshot) in originals {
            assert_eq!(access.windows[&id].info, snapshot.info);
        }
    }

    #[test]
    fn partial_failure_is_undoable_but_unchanged_failure_is_not() {
        for partial in [false, true] {
            let mut access = Fake::new(1);
            let original = access.windows[&WindowId(1)].info.clone();
            access.reject = Some(WindowId(1));
            access.partial = partial;
            let mut session = Session::default();
            let result = run(&mut session, &mut access, adjust(1, WindowChange::Center));
            assert!(result.message.unwrap().contains("refused"));
            assert_eq!(result.changed, usize::from(partial));
            assert_eq!(session.history.len(), usize::from(partial));
            run(&mut session, &mut access, WindowOperation::Undo);
            assert_eq!(access.windows[&WindowId(1)].info, original);
        }
    }

    #[test]
    fn undo_redo_replays_steps_and_a_new_change_discards_redo() {
        let mut access = Fake::new(1);
        let mut session = Session::default();
        let original = access.windows[&WindowId(1)].clone();
        run(
            &mut session,
            &mut access,
            adjust(1, WindowChange::Move { dx: 20.0, dy: 0.0 }),
        );
        let first = access.windows[&WindowId(1)].clone();
        run(
            &mut session,
            &mut access,
            adjust(2, WindowChange::Move { dx: 30.0, dy: 0.0 }),
        );
        let second = access.windows[&WindowId(1)].clone();
        run(&mut session, &mut access, WindowOperation::Undo);
        assert!(same_placement(&access.windows[&WindowId(1)], &first));
        run(&mut session, &mut access, WindowOperation::Undo);
        assert!(same_placement(&access.windows[&WindowId(1)], &original));
        run(&mut session, &mut access, WindowOperation::Undo);
        for expected in [&first, &second] {
            assert_eq!(
                run(&mut session, &mut access, WindowOperation::Redo).changed,
                1
            );
            assert!(same_placement(&access.windows[&WindowId(1)], expected));
        }
        run(&mut session, &mut access, WindowOperation::Undo);
        run(&mut session, &mut access, adjust(3, WindowChange::Center));
        assert!(session.redo.is_empty());
        assert_eq!(
            run(&mut session, &mut access, WindowOperation::Redo).changed,
            0
        );
    }

    #[test]
    fn initial_restore_outlives_history_limit_and_is_itself_undoable() {
        let mut access = Fake::new(3);
        access
            .cycle_state(WindowId(2), &screens(), &|| false)
            .unwrap();
        let original = access.windows.clone();
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            WindowOperation::Acquire(Point::default()),
        );
        run(&mut session, &mut access, WindowOperation::Enumerate);
        for group in 1..=40 {
            run(
                &mut session,
                &mut access,
                adjust(group, WindowChange::Move { dx: 1.0, dy: 0.0 }),
            );
        }
        run(
            &mut session,
            &mut access,
            adjust(41, WindowChange::CycleState),
        );
        run(
            &mut session,
            &mut access,
            adjust(42, WindowChange::CycleState),
        );
        run(
            &mut session,
            &mut access,
            WindowOperation::Adjust {
                target: WindowId(2),
                group: 43,
                change: WindowChange::Move { dx: 50.0, dy: 20.0 },
            },
        );
        // Another application's change is outside this session's write set.
        access
            .set_frame(
                WindowId(3),
                Rect::new(100.0, 100.0, 300.0, 200.0),
                &screens(),
                &|| false,
            )
            .unwrap();
        let before_reset = access.windows.clone();
        assert_eq!(session.history.len(), 32);
        let reset = run(
            &mut session,
            &mut access,
            WindowOperation::ResetInitial { group: 44 },
        );
        assert_eq!(reset.changed, 2);
        assert!(same_placement(
            &access.windows[&WindowId(1)],
            &original[&WindowId(1)]
        ));
        assert!(same_placement(
            &access.windows[&WindowId(2)],
            &original[&WindowId(2)]
        ));
        assert!(same_placement(
            &access.windows[&WindowId(3)],
            &before_reset[&WindowId(3)]
        ));
        run(&mut session, &mut access, WindowOperation::Undo);
        for id in [WindowId(1), WindowId(2), WindowId(3)] {
            assert!(same_placement(&access.windows[&id], &before_reset[&id]));
        }
        run(&mut session, &mut access, WindowOperation::Redo);
        assert!(access.windows[&WindowId(2)].info.maximized);
        assert!(!access.windows[&WindowId(1)].info.minimized);
        assert_eq!(
            run(
                &mut session,
                &mut access,
                WindowOperation::ResetInitial { group: 45 }
            )
            .changed,
            0
        );
    }

    #[test]
    fn temporarily_unavailable_windows_keep_their_undo_for_retry() {
        let mut access = Fake::new(1);
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            adjust(1, WindowChange::Move { dx: 20.0, dy: 0.0 }),
        );
        access.snapshot_unavailable = true;
        let result = run(&mut session, &mut access, WindowOperation::Undo);
        assert_eq!(result.changed, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(session.history.len(), 1);
        access.snapshot_unavailable = false;
        assert_eq!(
            run(&mut session, &mut access, WindowOperation::Undo).changed,
            1
        );
        assert_eq!(
            run(&mut session, &mut access, WindowOperation::Redo).changed,
            1
        );
    }

    #[test]
    fn cancelled_undo_preserves_unprocessed_windows_and_completed_redo() {
        let mut access = Fake::new(2);
        let mut session = Session::default();
        let originals = access.windows.clone();
        for id in [WindowId(1), WindowId(2)] {
            run(
                &mut session,
                &mut access,
                WindowOperation::Adjust {
                    target: id,
                    group: 1,
                    change: WindowChange::Move { dx: 15.0, dy: 0.0 },
                },
            );
        }
        let changed = access.windows.clone();
        let calls = std::cell::Cell::new(0);
        let result = session.execute(
            &mut access,
            WindowRequest {
                scope: None,
                session: 1,
                id: 3,
                operation: WindowOperation::Undo,
            },
            &screens(),
            &|| {
                calls.set(calls.get() + 1);
                calls.get() > 1
            },
        );
        assert_eq!(result.changed, 1);
        assert_eq!(session.history.back().unwrap().1.len(), 1);
        assert_eq!(session.redo.back().unwrap().1.len(), 1);
        run(&mut session, &mut access, WindowOperation::Undo);
        for id in [WindowId(1), WindowId(2)] {
            assert!(same_placement(&access.windows[&id], &originals[&id]));
        }
        run(&mut session, &mut access, WindowOperation::Redo);
        run(&mut session, &mut access, WindowOperation::Redo);
        for id in [WindowId(1), WindowId(2)] {
            assert!(same_placement(&access.windows[&id], &changed[&id]));
        }
    }

    #[test]
    fn state_cycle_retains_minimized_target_and_undo_restores_each_state() {
        let mut access = Fake::new(1);
        let original = access.windows[&WindowId(1)].info.bounds;
        let mut session = Session::default();
        for round in 0..2 {
            let first = run(
                &mut session,
                &mut access,
                adjust(round * 3 + 1, WindowChange::CycleState),
            );
            assert!(first.target.unwrap().maximized);
            let second = run(
                &mut session,
                &mut access,
                adjust(round * 3 + 2, WindowChange::CycleState),
            );
            assert!(second.target.unwrap().minimized);
            assert_eq!(second.changed, 1);
            assert!(second.pointer.is_none());
            assert_eq!(session.target, Some(WindowId(1)));
            let third = run(
                &mut session,
                &mut access,
                adjust(round * 3 + 3, WindowChange::CycleState),
            );
            let restored = third.target.unwrap();
            assert!(!restored.minimized && !restored.maximized);
            assert_eq!(restored.bounds, original);
        }
        let undone = run(&mut session, &mut access, WindowOperation::Undo);
        assert!(undone.target.unwrap().minimized);
        let undone = run(&mut session, &mut access, WindowOperation::Undo);
        assert!(!undone.target.as_ref().unwrap().minimized);
        assert!(undone.target.unwrap().maximized);
    }

    #[test]
    fn maximized_window_uses_restored_size_and_undo_restores_maximized_state() {
        let mut access = Fake::new(1);
        let normal = access.windows[&WindowId(1)].info.bounds;
        let mut session = Session::default();
        run(
            &mut session,
            &mut access,
            adjust(1, WindowChange::CycleState),
        );
        assert!(access.windows[&WindowId(1)].info.maximized);
        run(
            &mut session,
            &mut access,
            adjust(2, WindowChange::Move { dx: 10.0, dy: 0.0 }),
        );
        assert_eq!(access.windows[&WindowId(1)].info.bounds.width, normal.width);
        assert!(!access.windows[&WindowId(1)].info.maximized);
        run(&mut session, &mut access, WindowOperation::Undo);
        assert!(access.windows[&WindowId(1)].info.maximized);
        assert_eq!(access.windows[&WindowId(1)].restored, normal);
    }

    #[test]
    fn closed_window_is_skipped_by_undo_and_does_not_change_another_target() {
        let mut access = Fake::new(2);
        let mut session = Session {
            target: Some(WindowId(1)),
            ..Session::default()
        };
        run(
            &mut session,
            &mut access,
            WindowOperation::Tile {
                target: WindowId(1),
                gap: 8.0,
                group: 1,
            },
        );
        access.windows.remove(&WindowId(1));
        let result = run(&mut session, &mut access, WindowOperation::Undo);
        assert_eq!((result.changed, result.skipped), (1, 1));
        assert!(result.target.is_none());
    }

    #[test]
    fn undo_history_keeps_only_latest_32_changed_groups() {
        let mut access = Fake::new(1);
        let mut session = Session::default();
        for group in 1..=40 {
            run(
                &mut session,
                &mut access,
                adjust(
                    group,
                    WindowChange::Move {
                        dx: if group % 2 == 0 { -1.0 } else { 1.0 },
                        dy: 0.0,
                    },
                ),
            );
        }
        assert_eq!(session.history.len(), 32);
        assert_eq!(session.history.front().unwrap().0, 9);
    }

    #[test]
    fn tab_keeps_next_target_and_pointer_even_when_focus_is_denied() {
        let mut access = Fake::new(3);
        access.refuse_focus = true;
        let mut session = Session {
            target: Some(WindowId(1)),
            ..Session::default()
        };
        for expected in [WindowId(2), WindowId(3), WindowId(1)] {
            let result = run(&mut session, &mut access, WindowOperation::Cycle);
            assert_eq!(result.target.unwrap().id, expected);
            assert_eq!(
                result.pointer,
                Some(access.windows[&expected].info.bounds.center())
            );
            assert_eq!(result.message.as_deref(), Some("focus denied"));
        }
    }

    #[test]
    fn cycling_prefers_same_application_and_screen_then_falls_back() {
        for standalone in [false, true] {
            let mut access = Fake::new(5);
            access.windows.get_mut(&WindowId(2)).unwrap().info.app = "other".into();
            access
                .windows
                .get_mut(&WindowId(4))
                .unwrap()
                .info
                .app
                .clear();
            access.windows.get_mut(&WindowId(5)).unwrap().info.screen = 1;
            let mut session = Session {
                target: Some(WindowId(1)),
                ..Session::default()
            };
            access.selected.set(Some(WindowId(1)));
            for (backwards, expected) in [
                (false, 3),
                (false, 2),
                (false, 4),
                (false, 5),
                (false, 1),
                (true, 5),
                (true, 4),
                (true, 2),
                (true, 3),
                (true, 1),
            ] {
                let operation = if standalone {
                    WindowOperation::CycleActive { backwards }
                } else if backwards {
                    WindowOperation::CyclePrevious
                } else {
                    WindowOperation::Cycle
                };
                let result = run(&mut session, &mut access, operation);
                assert_eq!(result.target.unwrap().id, WindowId(expected));
                assert_eq!(
                    result.pointer,
                    Some(access.windows[&WindowId(expected)].info.bounds.center())
                );
            }
            // A singleton application can still switch to another application.
            session.target = Some(WindowId(2));
            access.selected.set(Some(WindowId(2)));
            let operation = if standalone {
                WindowOperation::CycleActive { backwards: false }
            } else {
                WindowOperation::Cycle
            };
            assert_eq!(
                run(&mut session, &mut access, operation).target.unwrap().id,
                WindowId(4)
            );
        }
    }

    #[test]
    fn overlap_cache_reuses_geometry_across_focus_and_title_changes_without_allocations() {
        let mut windows: Vec<_> = Fake::new(64)
            .windows
            .into_values()
            .map(|s| s.info)
            .collect();
        for window in &mut windows {
            window.bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        }
        let mut cache = OverlapCache::default();
        cache.prepare(&windows, Some(WindowId(1)), &|| false);
        assert_eq!(cache.comparisons, 63); // Fully stacked windows need n - 1 checks.
        windows.reverse(); // Native activation reorders inventory.
        windows[0].title = "changed title".into();
        cache.prepare(&windows, Some(WindowId(64)), &|| false);
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for id in 1..=64 {
            windows.rotate_left(1);
            cache.prepare(&windows, Some(WindowId(id)), &|| false);
            assert!(cache.contains(WindowId(id)));
        }
        let allocations = region.change();
        assert_eq!(allocations.allocations, 0);
        assert_eq!(allocations.reallocations, 0);
        assert_eq!(cache.comparisons, 63); // No further intersection calculations.
        windows
            .iter_mut()
            .find(|w| w.id == WindowId(64))
            .unwrap()
            .bounds
            .x = 500.0;
        cache.prepare(&windows, Some(WindowId(1)), &|| false);
        assert!(!cache.contains(WindowId(64)));
        cache.prepare(&windows, Some(WindowId(64)), &|| false);
        assert_eq!(cache.members, [WindowId(64)]);
    }

    #[test]
    fn overlap_cache_invalidates_each_geometry_field_and_external_growth() {
        let mut windows: Vec<_> = Fake::new(2).windows.into_values().map(|s| s.info).collect();
        windows[0].bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let connected = Rect::new(-75.0, -75.0, 100.0, 100.0);
        for disconnected in [
            Rect::new(-100.0, -75.0, 100.0, 100.0), // x only
            Rect::new(-75.0, -100.0, 100.0, 100.0), // y only
            Rect::new(-75.0, -75.0, 75.0, 100.0),   // width only
            Rect::new(-75.0, -75.0, 100.0, 75.0),   // height only
        ] {
            let mut cache = OverlapCache::default();
            for (bounds, expected) in [(connected, true), (disconnected, false), (connected, true)]
            {
                windows[1].bounds = bounds;
                cache.prepare(&windows, Some(WindowId(1)), &|| false);
                assert_eq!(cache.contains(WindowId(2)), expected, "{bounds:?}");
            }
        }
        // A previously disjoint window maximizes, then restores externally.
        let mut cache = OverlapCache::default();
        for (bounds, maximized, expected) in [
            (Rect::new(500.0, 500.0, 100.0, 100.0), false, false),
            (Rect::new(0.0, 0.0, 1920.0, 1080.0), true, true),
            (Rect::new(500.0, 500.0, 100.0, 100.0), false, false),
        ] {
            windows[1].bounds = bounds;
            windows[1].maximized = maximized;
            cache.prepare(&windows, Some(WindowId(1)), &|| false);
            assert_eq!(cache.contains(WindowId(2)), expected);
        }
    }

    #[test]
    fn ordinary_window_cycles_never_allocate_overlap_state() {
        let mut session = Session::default();
        let mut access = Fake::new(3);
        assert!(session.overlap.is_none());
        for operation in [
            WindowOperation::CycleActive { backwards: false },
            WindowOperation::CyclePrevious,
        ] {
            run(&mut session, &mut access, operation);
            assert!(session.overlap.is_none());
        }
        run(
            &mut session,
            &mut access,
            WindowOperation::CycleOverlapping { backwards: false },
        );
        assert!(session.overlap.is_some());
    }

    #[test]
    fn overlap_cache_releases_historic_peak_capacity() {
        let mut windows: Vec<_> = Fake::new(256)
            .windows
            .into_values()
            .map(|s| s.info)
            .collect();
        let mut cache = OverlapCache::default();
        cache.prepare(&windows, Some(WindowId(1)), &|| false);
        assert_eq!(cache.members.len(), 256);
        windows.truncate(1);
        cache.prepare(&windows, Some(WindowId(1)), &|| false);
        assert_eq!(cache.members, [WindowId(1)]);
        assert!(cache.inventory.capacity() <= 2);
        assert!(cache.queue.capacity() <= 2);
        assert!(cache.members.capacity() <= 2);
    }

    #[test]
    fn overlap_cache_cancellation_does_not_reuse_partial_component() {
        let windows: Vec<_> = Fake::new(4).windows.into_values().map(|s| s.info).collect();
        let mut cache = OverlapCache::default();
        cache.prepare(&windows, Some(WindowId(1)), &|| true);
        assert!(cache.members.is_empty());
        cache.prepare(&windows, Some(WindowId(1)), &|| false);
        assert_eq!(cache.members.len(), 4);
        cache.prepare(&windows, None, &|| false);
        assert!(cache.members.is_empty());
    }

    #[test]
    fn overlap_cache_matches_transitive_closure_across_inventory_changes() {
        let mut windows: Vec<_> = Fake::new(12)
            .windows
            .into_values()
            .map(|s| s.info)
            .collect();
        let mut cache = OverlapCache::default();
        let mut seed = 123_u32;
        for _ in 0..32 {
            for window in &mut windows {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                window.bounds = Rect::new(
                    (seed % 500) as f64,
                    ((seed >> 16) % 500) as f64,
                    100.0,
                    100.0,
                );
            }
            let mut reachable = [[false; 12]; 12];
            for i in 0..12 {
                for j in 0..12 {
                    reachable[i][j] =
                        i == j || windows[i].bounds.intersect(&windows[j].bounds).is_some();
                }
            }
            for k in 0..12 {
                for i in 0..12 {
                    for j in 0..12 {
                        reachable[i][j] |= reachable[i][k] && reachable[k][j];
                    }
                }
            }
            for i in 0..12 {
                cache.prepare(&windows, Some(windows[i].id), &|| false);
                for j in 0..12 {
                    assert_eq!(cache.contains(windows[j].id), reachable[i][j]);
                }
            }
            windows.pop();
            cache.prepare(&windows, Some(windows[0].id), &|| false);
            assert!(!cache.contains(WindowId(12)));
            windows.push(Fake::new(12).windows.remove(&WindowId(12)).unwrap().info);
        }
    }

    #[test]
    fn overlapping_cycle_follows_chains_and_rebuilds_after_bridge_changes() {
        for bridge_change in ["move", "close", "minimize"] {
            let mut access = Fake::new(5);
            // A--B--C form a chain; D only touches C; E is a separate island.
            for (id, x) in [(1, 0.0), (2, 75.0), (3, 150.0), (4, 250.0), (5, 500.0)] {
                access.windows.get_mut(&WindowId(id)).unwrap().info.bounds =
                    Rect::new(x, 0.0, 100.0, 100.0);
            }
            let mut session = Session::default();
            access.pointer_target = Some(WindowId(1));
            for (backwards, expected) in [
                (true, 3), // A reaches C through B, despite no direct overlap.
                (false, 1),
                (false, 2),
                (false, 3),
                (true, 2),
                (true, 1),
            ] {
                let result = run(
                    &mut session,
                    &mut access,
                    WindowOperation::CycleOverlapping { backwards },
                );
                assert_eq!(result.target.unwrap().id, WindowId(expected));
                assert_eq!(
                    result.pointer,
                    Some(access.windows[&WindowId(expected)].info.bounds.center())
                );
                assert!(result.windows.is_none() && result.tabs.is_none());
                access.pointer_target = Some(WindowId(expected));
            }
            match bridge_change {
                "move" => access.windows.get_mut(&WindowId(2)).unwrap().info.bounds.x = 600.0,
                "close" => {
                    access.windows.remove(&WindowId(2));
                }
                "minimize" => access.windows.get_mut(&WindowId(2)).unwrap().info.minimized = true,
                _ => unreachable!(),
            }
            for backwards in [false, true] {
                let result = run(
                    &mut session,
                    &mut access,
                    WindowOperation::CycleOverlapping { backwards },
                );
                assert!(
                    result.pointer.is_none() && result.message.is_none(),
                    "{bridge_change}"
                );
                assert_eq!(access.selected.get(), Some(WindowId(1)));
            }
        }
    }

    #[test]
    fn overlapping_cycle_skips_disjoint_and_touching_windows_in_both_directions() {
        let mut access = Fake::new(4);
        for (id, bounds) in [
            (1, Rect::new(0.0, 0.0, 100.0, 100.0)),
            (2, Rect::new(100.0, 0.0, 100.0, 100.0)),
            (3, Rect::new(10.0, 10.0, 80.0, 80.0)),
            (4, Rect::new(500.0, 500.0, 100.0, 100.0)),
        ] {
            access.windows.get_mut(&WindowId(id)).unwrap().info.bounds = bounds;
        }
        let mut session = Session::default();
        access.pointer_target = Some(WindowId(1));
        access.selected.set(Some(WindowId(4))); // Pointer, not foreground, anchors selection.
        for (backwards, expected) in [(false, 3), (false, 1), (true, 3), (true, 1)] {
            let result = run(
                &mut session,
                &mut access,
                WindowOperation::CycleOverlapping { backwards },
            );
            assert_eq!(result.target.unwrap().id, WindowId(expected));
            assert_eq!(
                result.pointer,
                Some(access.windows[&WindowId(expected)].info.bounds.center())
            );
            assert!(result.windows.is_none() && result.tabs.is_none());
            access.pointer_target = Some(WindowId(expected));
        }
        // Recheck live geometry; scan the entire ring, but never select a disjoint window.
        access.windows.get_mut(&WindowId(3)).unwrap().info.bounds.x = 300.0;
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::CycleOverlapping { backwards: false },
        );
        assert!(result.pointer.is_none() && result.message.is_none());
        assert_eq!(access.selected.get(), Some(WindowId(1)));
        access.pointer_target = Some(WindowId(1));
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::CycleOverlapping { backwards: true },
        );
        assert!(result.pointer.is_none() && result.message.is_none());
        assert_eq!(access.selected.get(), Some(WindowId(1)));
    }

    #[test]
    fn standalone_cycle_tracks_foreground_and_returns_no_ui_inventory() {
        let mut access = Fake::new(3);
        let mut session = Session::default();
        access.selected.set(Some(WindowId(2)));
        for (backwards, expected) in [(false, 3), (false, 1), (true, 3)] {
            let result = run(
                &mut session,
                &mut access,
                WindowOperation::CycleActive { backwards },
            );
            assert_eq!(access.selected.get(), Some(WindowId(expected)));
            assert_eq!(
                result.pointer,
                Some(access.windows[&WindowId(expected)].info.bounds.center())
            );
            assert!(result.windows.is_none() && result.tabs.is_none());
            assert!(session.history.is_empty() && session.initial.is_empty());
        }
        // External focus changes override our previous target; the pointer is elsewhere.
        access.selected.set(Some(WindowId(1)));
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::CycleActive { backwards: false },
        );
        assert_eq!(result.target.unwrap().id, WindowId(2));
        access.windows.remove(&WindowId(3));
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::CycleActive { backwards: false },
        );
        assert_eq!(result.target.unwrap().id, WindowId(1));
    }

    #[test]
    fn standalone_cycle_starts_at_pointer_instead_of_foreground_and_leaves_peers() {
        let mut access = Fake::new(4);
        access.windows.get_mut(&WindowId(2)).unwrap().info.app = "other".into();
        access.windows.get_mut(&WindowId(4)).unwrap().info.app = "other".into();
        access.selected.set(Some(WindowId(2)));
        access.pointer_target = Some(WindowId(1));
        let mut session = Session::default();
        for expected in [3, 2, 4, 1] {
            let result = run(
                &mut session,
                &mut access,
                WindowOperation::CycleActive { backwards: false },
            );
            assert_eq!(result.target.unwrap().id, WindowId(expected));
            // Simulate the successful pointer warp to the selected window.
            access.pointer_target = Some(WindowId(expected));
        }
        for expected in [4, 2, 3, 1] {
            let result = run(
                &mut session,
                &mut access,
                WindowOperation::CycleActive { backwards: true },
            );
            assert_eq!(result.target.unwrap().id, WindowId(expected));
            access.pointer_target = Some(WindowId(expected));
        }
    }

    #[test]
    fn overlapping_worker_no_match_emits_no_pointer_or_error_event() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = WindowWorker::start(
            || {
                let mut fake = Fake::new(2);
                fake.windows.get_mut(&WindowId(2)).unwrap().info.bounds.x = 2000.0;
                fake.selected.set(Some(WindowId(1)));
                fake.pointer_target = Some(WindowId(1));
                fake
            },
            move |event| {
                tx.send(event).unwrap();
            },
        )
        .unwrap();
        for operation in [
            WindowOperation::CycleOverlapping { backwards: false },
            WindowOperation::CycleActive { backwards: false },
        ] {
            worker
                .submit(
                    WindowRequest {
                        scope: None,
                        session: 0,
                        id: 0,
                        operation,
                    },
                    &screens(),
                )
                .unwrap();
        }
        let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            matches!(event, BackendEvent::WindowCycleCompleted(Ok(point)) if point.x == 2160.0)
        );
        worker
            .stop_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn standalone_cycle_worker_needs_no_session_and_only_emits_pointer_result() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = WindowWorker::start(
            || {
                let fake = Fake::new(3);
                fake.selected.set(Some(WindowId(1)));
                fake
            },
            move |event| {
                tx.send(event).unwrap();
            },
        )
        .unwrap();
        for (backwards, expected) in [(false, 2), (false, 3), (true, 2)] {
            worker
                .submit(
                    WindowRequest {
                        scope: None,
                        session: 0,
                        id: 0,
                        operation: WindowOperation::CycleActive { backwards },
                    },
                    &screens(),
                )
                .unwrap();
            let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(
                matches!(event, BackendEvent::WindowCycleCompleted(Ok(point))
                if point == Fake::new(3).windows[&WindowId(expected)].info.bounds.center())
            );
        }
        assert!(rx.try_recv().is_err());
        worker
            .stop_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn tile_attempts_windows_larger_than_the_equal_share_and_undo_restores_all() {
        let mut access = Fake::new(3);
        access.minimum = Point::new(1000.0, 700.0);
        let original = access.windows.clone();
        let mut session = Session::default();
        let result = run(
            &mut session,
            &mut access,
            WindowOperation::Tile {
                target: WindowId(1),
                gap: 8.0,
                group: 1,
            },
        );
        assert_eq!((result.changed, result.skipped), (3, 0));
        assert!(result.message.unwrap().contains("minimum sizes"));
        assert_eq!(access.writes, [WindowId(1), WindowId(2), WindowId(3)]);
        run(&mut session, &mut access, WindowOperation::Undo);
        for (id, before) in original {
            assert_eq!(access.windows[&id].info, before.info);
        }
    }

    #[test]
    fn tab_cycles_stable_ring_and_centers_cursor_without_touching_geometry() {
        let mut access = Fake::new(3);
        let mut session = Session {
            target: Some(WindowId(1)),
            cycle: vec![WindowId(3), WindowId(1), WindowId(2)],
            ..Session::default()
        };
        for expected in [WindowId(2), WindowId(3), WindowId(1)] {
            let result = run(&mut session, &mut access, WindowOperation::Cycle);
            assert_eq!(result.target.unwrap().id, expected);
            assert_eq!(
                result.pointer,
                Some(access.windows[&expected].info.bounds.center())
            );
        }
        access.windows.remove(&WindowId(2));
        assert_eq!(
            run(&mut session, &mut access, WindowOperation::Cycle)
                .target
                .unwrap()
                .id,
            WindowId(3)
        );
        assert!(access.writes.is_empty());
        assert!(session.history.is_empty());
    }

    #[test]
    fn worker_cancels_blocked_query_without_losing_target_or_history() {
        use std::sync::mpsc;
        struct Blocking {
            fake: Fake,
            entered: mpsc::Sender<()>,
            released: mpsc::Sender<()>,
        }
        impl WindowAccess for Blocking {
            fn acquire(&mut self, p: Point, s: &[Screen]) -> Result<Option<WindowInfo>, String> {
                self.fake.acquire(p, s)
            }
            fn enumerate(
                &mut self,
                _: &[Screen],
                cancelled: &dyn Fn() -> bool,
            ) -> Result<Vec<WindowInfo>, String> {
                self.entered.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while !cancelled() && Instant::now() < deadline {
                    std::thread::yield_now();
                }
                assert!(cancelled());
                self.released.send(()).unwrap();
                Ok(Vec::new())
            }
            fn snapshot(&self, id: WindowId, s: &[Screen]) -> Result<Snapshot, String> {
                self.fake.snapshot(id, s)
            }
            fn set_frame(
                &mut self,
                id: WindowId,
                r: Rect,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.fake.set_frame(id, r, s, c)
            }
            fn restore(
                &mut self,
                before: &Snapshot,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.fake.restore(before, s, c)
            }
            fn cycle_state(
                &mut self,
                id: WindowId,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.fake.cycle_state(id, s, c)
            }
            fn select(&self, id: WindowId) -> Result<(), String> {
                self.fake.select(id)
            }
            fn pointer(&self) -> Result<Point, String> {
                self.fake.pointer()
            }
            fn reset(&mut self) {}
        }
        let (entered_tx, entered) = mpsc::channel();
        let (released_tx, released) = mpsc::channel();
        let (result_tx, result) = mpsc::channel();
        let mut worker = WindowWorker::start(
            move || Blocking {
                fake: Fake::new(1),
                entered: entered_tx,
                released: released_tx,
            },
            move |e| {
                result_tx.send(e).unwrap();
            },
        )
        .unwrap();
        let submit = |id, operation| {
            worker
                .submit(
                    WindowRequest {
                        scope: None,
                        session: 7,
                        id,
                        operation,
                    },
                    &screens(),
                )
                .unwrap()
        };
        submit(1, WindowOperation::Acquire(Point::new(-1000.0, 200.0)));
        result.recv_timeout(Duration::from_secs(2)).unwrap();
        submit(2, WindowOperation::Enumerate);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        submit(3, adjust(1, WindowChange::Center));
        submit(4, WindowOperation::CancelPending);
        released.recv_timeout(Duration::from_secs(2)).unwrap();
        let BackendEvent::WindowResult(value) =
            result.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("wrong event");
        };
        assert_eq!(value.id, 4);
        assert_eq!(value.target.unwrap().bounds.x, -1080.0);
        assert!(result.try_recv().is_err());
        worker.cancel(7);
        worker
            .stop_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
    }

    #[test]
    fn deferred_geometry_requires_observation_and_preserves_undo_metadata() {
        let mut access = Fake::new(2);
        access.deferred = true;
        let mut session = Session::default();
        let screens: Arc<[Screen]> = screens().into();
        let request = WindowRequest {
            session: 1,
            id: 2,
            scope: None,
            operation: WindowOperation::Adjust {
                target: WindowId(1),
                change: WindowChange::Move { dx: 20.0, dy: 0.0 },
                group: 1,
            },
        };
        let before = access.windows[&WindowId(1)].info.bounds;
        let mut pending = PendingAdjustment::begin(&mut session, &mut access, &request, &screens)
            .unwrap()
            .0
            .unwrap();
        assert!(pending.poll(&mut session, &mut access, false).is_none());
        assert!(session.history.is_empty());
        let accepted = access.submitted[&WindowId(1)];
        access.windows.get_mut(&WindowId(1)).unwrap().info.bounds = accepted;
        let result = pending.poll(&mut session, &mut access, false).unwrap();
        assert_eq!(result.changed, 1);
        assert_eq!(result.target.unwrap().bounds, accepted);
        assert_eq!(session.history.len(), 1);
        access.windows.get_mut(&WindowId(1)).unwrap().info.title = "renamed after move".into();
        run(&mut session, &mut access, WindowOperation::Undo);
        assert_eq!(access.windows[&WindowId(1)].info.bounds, before);
        assert_eq!(
            access.windows[&WindowId(1)].info.title,
            "renamed after move"
        );
        assert!(std::mem::size_of::<PlacementSnapshot>() < std::mem::size_of::<Snapshot>());
    }

    #[test]
    fn independent_geometry_submits_while_another_window_has_not_acknowledged() {
        let mut access = Fake::new(2);
        access.deferred = true;
        let mut session = Session::default();
        let screens: Arc<[Screen]> = screens().into();
        let request = |target, id| WindowRequest {
            session: 1,
            id,
            scope: None,
            operation: WindowOperation::Adjust {
                target: WindowId(target),
                change: WindowChange::Move { dx: 20.0, dy: 0.0 },
                group: id,
            },
        };
        let mut first =
            PendingAdjustment::begin(&mut session, &mut access, &request(1, 1), &screens)
                .unwrap()
                .0
                .unwrap();
        let mut second =
            PendingAdjustment::begin(&mut session, &mut access, &request(2, 2), &screens)
                .unwrap()
                .0
                .unwrap();
        assert_eq!(access.submitted.len(), 2);
        assert!(first.poll(&mut session, &mut access, false).is_none());
        let accepted = access.submitted[&WindowId(2)];
        access.windows.get_mut(&WindowId(2)).unwrap().info.bounds = accepted;
        assert_eq!(
            second
                .poll(&mut session, &mut access, false)
                .unwrap()
                .changed,
            1
        );
        assert!(first.poll(&mut session, &mut access, false).is_none());
        let cancelled = first.poll(&mut session, &mut access, true).unwrap();
        assert_eq!(cancelled.changed, 0);
    }

    #[test]
    fn confirmation_waiting_has_zero_allocations_and_no_metadata_reads() {
        let mut access = Fake::new(1);
        access.deferred = true;
        let mut session = Session::default();
        let screens: Arc<[Screen]> = screens().into();
        let request = WindowRequest {
            session: 1,
            id: 2,
            scope: None,
            operation: WindowOperation::Adjust {
                target: WindowId(1),
                change: WindowChange::Move { dx: 20.0, dy: 0.0 },
                group: 1,
            },
        };
        let mut pending = PendingAdjustment::begin(&mut session, &mut access, &request, &screens)
            .unwrap()
            .0
            .unwrap();
        let reads = access.snapshot_reads.get();
        let now = Instant::now();
        let allocation = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..10_000 {
            assert!(
                pending
                    .poll_at(&mut session, &mut access, false, now)
                    .is_none()
            );
        }
        let stats = allocation.change();
        assert_eq!((stats.allocations, stats.reallocations), (0, 0));
        assert_eq!(access.snapshot_reads.get(), reads);
        access.windows.get_mut(&WindowId(1)).unwrap().info.bounds = access.submitted[&WindowId(1)];
        let result = pending.poll(&mut session, &mut access, false).unwrap();
        assert_eq!(result.target.unwrap().title, "Window 1");
    }

    #[test]
    fn fallback_reuses_prepared_snapshot_without_extra_native_reads() {
        for case in 0..6 {
            let mut direct = Fake::new(1);
            let mut prepared = Fake::new(1);
            prepared.deferred = case != 0;
            prepared.decline_submission = case == 4;
            for access in [&mut direct, &mut prepared] {
                let info = &mut access.windows.get_mut(&WindowId(1)).unwrap().info;
                info.maximized = case == 1;
                info.fullscreen = case == 2;
                info.minimized = case == 5;
            }
            let mut session = Session {
                target: Some(WindowId(1)),
                ..Session::default()
            };
            let mut reference = Session {
                target: Some(WindowId(1)),
                ..Session::default()
            };
            let screens: Arc<[Screen]> = screens().into();
            let request = WindowRequest {
                session: 1,
                id: 2,
                scope: None,
                operation: WindowOperation::Adjust {
                    target: WindowId(1),
                    change: WindowChange::Move {
                        dx: if case == 3 { 0.01 } else { 20.0 },
                        dy: 0.0,
                    },
                    group: 1,
                },
            };
            let expected = reference.execute(&mut direct, request.clone(), &screens, &|| false);
            let (pending, before) =
                PendingAdjustment::begin(&mut session, &mut prepared, &request, &screens).unwrap();
            assert!(pending.is_none());
            let actual =
                session.execute_prepared(&mut prepared, request, &screens, &|| false, before);
            assert_eq!(actual.target, expected.target);
            assert_eq!(actual.message, expected.message);
            assert_eq!(
                prepared.snapshot_reads.get(),
                direct.snapshot_reads.get(),
                "case {case}"
            );
            assert_eq!(session.move_remainder, reference.move_remainder);
        }
    }

    #[test]
    fn deferred_subpixel_fallback_does_not_accumulate_movement_twice() {
        let mut access = Fake::new(1);
        access.deferred = true;
        let mut session = Session::default();
        let screens: Arc<[Screen]> = screens().into();
        let request = WindowRequest {
            session: 1,
            id: 2,
            scope: None,
            operation: WindowOperation::Adjust {
                target: WindowId(1),
                change: WindowChange::Move { dx: 0.01, dy: 0.0 },
                group: 1,
            },
        };
        assert!(
            PendingAdjustment::begin(&mut session, &mut access, &request, &screens)
                .unwrap()
                .0
                .is_none()
        );
        assert!(session.move_remainder.is_none());
        assert!(access.submitted.is_empty());
    }

    #[test]
    fn deferred_timeout_and_cancel_never_claim_unobserved_success() {
        for partial in [false, true] {
            let mut access = Fake::new(1);
            access.deferred = true;
            let mut session = Session::default();
            let screens: Arc<[Screen]> = screens().into();
            let request = WindowRequest {
                session: 1,
                id: 2,
                scope: None,
                operation: WindowOperation::Adjust {
                    target: WindowId(1),
                    change: WindowChange::Move { dx: 20.0, dy: 0.0 },
                    group: 1,
                },
            };
            let mut pending =
                PendingAdjustment::begin(&mut session, &mut access, &request, &screens)
                    .unwrap()
                    .0
                    .unwrap();
            if partial {
                access.windows.get_mut(&WindowId(1)).unwrap().info.bounds.x += 5.0;
            }
            let result = pending
                .poll_at(
                    &mut session,
                    &mut access,
                    partial,
                    Instant::now() + Duration::from_secs(1),
                )
                .unwrap();
            assert_eq!(result.changed, usize::from(partial));
            assert_eq!(session.history.len(), usize::from(partial));
            if partial {
                assert!(result.message.is_none());
            } else {
                assert!(result.message.unwrap().contains("Timed out"));
            }
        }
    }

    #[test]
    fn worker_services_independent_geometry_and_audio_before_confirmation() {
        use crate::api::audio::{AudioAction, AudioRequest, AudioTarget};
        struct Deferred {
            fake: Fake,
            writes: BTreeMap<WindowId, Rect>,
            submitted: std::sync::mpsc::Sender<WindowId>,
            release: Arc<AtomicBool>,
        }
        impl WindowAccess for Deferred {
            fn acquire(&mut self, p: Point, s: &[Screen]) -> Result<Option<WindowInfo>, String> {
                self.fake.acquire(p, s)
            }
            fn enumerate(
                &mut self,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<Vec<WindowInfo>, String> {
                self.fake.enumerate(s, c)
            }
            fn snapshot(&self, id: WindowId, s: &[Screen]) -> Result<Snapshot, String> {
                let mut snapshot = self.fake.snapshot(id, s)?;
                if self.release.load(Ordering::Acquire)
                    && let Some(rect) = self.writes.get(&id)
                {
                    snapshot.info.bounds = *rect;
                }
                Ok(snapshot)
            }
            fn can_submit_frame(&self, _: WindowId) -> bool {
                true
            }
            fn submit_frame(
                &mut self,
                before: &Snapshot,
                rect: Rect,
                _: &[Screen],
            ) -> Result<bool, String> {
                let id = before.info.id;
                self.writes.insert(id, rect);
                self.submitted.send(id).unwrap();
                Ok(true)
            }
            fn set_frame(
                &mut self,
                _: WindowId,
                _: Rect,
                _: &[Screen],
                _: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                panic!("reactor must use submission")
            }
            fn restore(
                &mut self,
                before: &Snapshot,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.fake.restore(before, s, c)
            }
            fn cycle_state(
                &mut self,
                id: WindowId,
                s: &[Screen],
                c: &dyn Fn() -> bool,
            ) -> Result<WindowInfo, String> {
                self.fake.cycle_state(id, s, c)
            }
            fn select(&self, id: WindowId) -> Result<(), String> {
                self.fake.select(id)
            }
            fn pointer(&self) -> Result<Point, String> {
                self.fake.pointer()
            }
            fn volume(&self, id: WindowId, action: AudioAction) -> Result<String, String> {
                self.fake.volume(id, action)
            }
            fn reset(&mut self) {}
        }
        let (submitted, writes) = std::sync::mpsc::channel();
        let (tx, events) = std::sync::mpsc::channel();
        let release = Arc::new(AtomicBool::new(false));
        let worker_release = release.clone();
        let mut worker = WindowWorker::start(
            move || Deferred {
                fake: Fake::new(3),
                writes: BTreeMap::new(),
                submitted,
                release: worker_release,
            },
            move |event| {
                tx.send(event).unwrap();
            },
        )
        .unwrap();
        let request = |id, operation| WindowRequest {
            session: 1,
            id,
            scope: None,
            operation,
        };
        worker
            .submit(
                request(1, WindowOperation::Acquire(Point::default())),
                &screens(),
            )
            .unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            BackendEvent::WindowResult(_)
        ));
        for (id, target) in [(2, 1), (3, 2)] {
            worker
                .submit(
                    request(
                        id,
                        WindowOperation::Adjust {
                            target: WindowId(target),
                            change: WindowChange::Move { dx: 20.0, dy: 0.0 },
                            group: id,
                        },
                    ),
                    &screens(),
                )
                .unwrap();
            assert_eq!(
                writes.recv_timeout(Duration::from_secs(5)).unwrap(),
                WindowId(target)
            );
        }
        // Request 5 targets an independent window, but must not overtake the
        // blocked request 4: result consumers discard lower request ids.
        for (id, target) in [(4, 1), (5, 3)] {
            worker
                .submit(
                    request(
                        id,
                        WindowOperation::Adjust {
                            target: WindowId(target),
                            change: WindowChange::Move { dx: 20.0, dy: 0.0 },
                            group: id,
                        },
                    ),
                    &screens(),
                )
                .unwrap();
        }
        worker
            .submit_audio(AudioRequest {
                session: 9,
                id: 1,
                target: AudioTarget::Application(WindowId(1)),
                action: AudioAction::Down,
            })
            .unwrap();
        assert!(
            matches!(events.recv_timeout(Duration::from_secs(5)).unwrap(), BackendEvent::AudioResult(result) if result.outcome.is_ok())
        );
        assert!(
            writes.try_recv().is_err(),
            "later window request overtook blocked geometry"
        );
        release.store(true, Ordering::Release);
        for expected in [2, 3, 4, 5] {
            assert!(
                matches!(events.recv_timeout(Duration::from_secs(5)).unwrap(), BackendEvent::WindowResult(result) if result.id == expected && result.changed == 1)
            );
        }
        worker
            .stop_until(Instant::now() + Duration::from_secs(2))
            .unwrap();
    }
    include!("window_session/async_tests.rs");
}
