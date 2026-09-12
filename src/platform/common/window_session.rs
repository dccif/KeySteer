//! Lazy, bounded window-operation worker. Native references are created and
//! released on its thread; only API values enter the engine's event queue.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

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

pub(crate) trait WindowAccess {
    fn set_scope(&mut self, _scope: Option<crate::api::window::WindowScope>, _reset: bool) {}
    /// Optional native message-loop wakeup, created on the worker thread.
    fn event_waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        None
    }
    /// Wait for native events or the mailbox wakeup. False uses the portable condvar.
    fn wait_for_events(&self, _timeout: Option<Duration>) -> bool {
        false
    }
    /// Hide a single application window without changing its parent.
    fn tab_set_hidden(&mut self, _id: WindowId, _hidden: bool) -> Result<(), String> {
        Ok(())
    }
    /// Validate queued focus notifications against the current foreground window.
    fn tab_selected(&self, _id: WindowId) -> bool {
        true
    }
    fn activate_window(
        &mut self,
        id: WindowId,
        _screens: &[Screen],
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.select(id)
    }
    fn tab_operation(
        &mut self,
        _operation: crate::api::window_tabs::TabOperation,
        _screens: &[Screen],
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        Err("Window tab groups are unavailable on this backend".into())
    }
    fn tab_state(&self) -> Option<crate::api::window_tabs::TabState> {
        None
    }
    fn tab_eligible(&self, id: WindowId, screens: &[Screen]) -> Result<(), String> {
        let window = self.snapshot(id, screens)?.info;
        if !window.resizable || window.fullscreen {
            return Err("Only ordinary resizable windows can join a tab group".into());
        }
        Ok(())
    }
    fn tab_application(&self, id: WindowId, screens: &[Screen]) -> Result<String, String> {
        Ok(self.snapshot(id, screens)?.info.app)
    }
    fn tab_events(&mut self) -> Vec<crate::api::window_tabs::TabNativeEvent> {
        Vec::new()
    }
    fn tab_watch(&mut self, _ids: &[WindowId]) -> Result<(), String> {
        Ok(())
    }
    fn tab_bars(&mut self, _bars: &[crate::api::window_tabs::TabBar]) -> Result<(), String> {
        Ok(())
    }
    fn tab_visible(&self, _id: WindowId) -> bool {
        true
    }
    /// Occupied header height in this backend's screen coordinate system.
    /// Headless adapters have no native strip.
    fn tab_bar_height(&self, _screen: &Screen) -> f64 {
        0.0
    }
    /// Minimize a member without restoring or moving an inactive window first.
    fn tab_minimize(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let mut snapshot = self.snapshot(id, screens)?;
        if !snapshot.info.minimized {
            snapshot.info.minimized = true;
            self.restore(&snapshot, screens, cancelled)?;
        }
        Ok(())
    }
    /// Reserve strip space after an external move/maximize, preserving state
    /// where the platform supports resizing a maximized window in place.
    fn tab_fit_frame(
        &mut self,
        id: WindowId,
        bounds: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String> {
        self.set_frame(id, bounds, screens, cancelled)
    }
    fn layout_representative(&self, id: WindowId) -> WindowId {
        id
    }
    fn acquire(&mut self, point: Point, screens: &[Screen]) -> Result<Option<WindowInfo>, String>;
    fn enumerate(
        &mut self,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<WindowInfo>, String>;
    fn snapshot(&self, id: WindowId, screens: &[Screen]) -> Result<Snapshot, String>;
    fn set_frame(
        &mut self,
        id: WindowId,
        rect: Rect,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String>;
    fn restore(
        &mut self,
        snapshot: &Snapshot,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String>;
    fn cycle_state(
        &mut self,
        id: WindowId,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<WindowInfo, String>;
    fn select(&self, id: WindowId) -> Result<(), String>;
    /// Reap native audio routes; true requests a bounded maintenance wakeup.
    fn maintain_audio(&self) -> bool {
        false
    }
    fn system_audio(&self, _change: crate::api::audio::AudioAction) -> Result<String, String> {
        Err("System audio control is unavailable on this backend".into())
    }
    fn volume(
        &self,
        _id: WindowId,
        _change: crate::api::audio::AudioAction,
    ) -> Result<String, String> {
        Err("Application volume control is unavailable on this backend".into())
    }
    fn close(&self, _id: WindowId) -> Result<(), String> {
        Err("Closing windows is unavailable on this backend".into())
    }
    fn pointer(&self) -> Result<Point, String>;
    fn minimum_size(&self, _id: WindowId) -> Point {
        Point::new(100.0, 80.0)
    }
    fn logical_scale(&self, _screen: &Screen) -> f64 {
        1.0
    }
    fn move_fullscreen(
        &mut self,
        _id: WindowId,
        _destination: crate::api::command::WindowScreenTarget,
        _screens: &[Screen],
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(WindowInfo, Option<Point>), String> {
        Err("moving native fullscreen windows is not supported".into())
    }
    fn reset(&mut self);
    fn take_closed(&mut self) -> Vec<WindowId> {
        Vec::new()
    }
}

enum Pending {
    Window {
        request: WindowRequest,
        screens: Vec<Screen>,
    },
    Audio(crate::api::audio::AudioRequest),
}
impl Pending {
    fn operation(&self) -> Option<&WindowOperation> {
        match self {
            Self::Window { request, .. } => Some(&request.operation),
            Self::Audio(_) => None,
        }
    }
}

fn execute_audio(
    access: &impl WindowAccess,
    request: crate::api::audio::AudioRequest,
) -> crate::api::audio::AudioResult {
    use crate::api::audio::AudioTarget;
    let outcome = match request.target {
        AudioTarget::Application(id) => access.volume(id, request.action),
        AudioTarget::System => access.system_audio(request.action),
    };
    crate::api::audio::AudioResult {
        session: request.session,
        id: request.id,
        outcome,
    }
}

#[derive(Default)]
struct Mailbox {
    queue: Mutex<VecDeque<Pending>>,
    ready: Condvar,
    session: AtomicU64,
    cancel_before: AtomicU64,
    query_before: AtomicU64,
    stop: AtomicBool,
    native_waker: OnceLock<Arc<dyn Fn() + Send + Sync>>,
    native_waiting: AtomicBool,
}

impl Mailbox {
    fn notify(&self) {
        self.ready.notify_one();
        if self.native_waiting.load(Ordering::Acquire)
            && let Some(wake) = self.native_waker.get()
        {
            wake();
        }
    }
}

pub(crate) struct WindowWorker {
    mailbox: Arc<Mailbox>,
    worker: WorkerJoin,
}

impl WindowWorker {
    pub(crate) fn start<A: WindowAccess + 'static>(
        create: impl FnOnce() -> A + Send + 'static,
        emit: impl Fn(BackendEvent) + Send + 'static,
    ) -> Result<Self, String> {
        let mailbox = Arc::new(Mailbox::default());
        let input = mailbox.clone();
        let worker = WorkerJoin::spawn(
            "window-manager",
            std::thread::Builder::new().name("keysteer-window".into()),
            move || {
                let native = create();
                if let Some(wake) = native.event_waker() {
                    let _ = input.native_waker.set(wake);
                }
                let mut access = super::window_tabs::Grouped::new(native);
                let mut session = Session::default();
                let mut displays = Vec::new();
                loop {
                    let audio_active = access.maintain_audio();
                    if access.persistent()
                        && let Err(error) =
                            access.pump(&displays, &|| input.stop.load(Ordering::Acquire))
                    {
                        emit(BackendEvent::Warning(format!("window tabs: {error}")));
                    }
                    let pending = {
                        let mut queue = input.queue.lock().unwrap_or_else(|e| e.into_inner());
                        while queue.is_empty() && !input.stop.load(Ordering::Acquire) {
                            if access.persistent() || audio_active {
                                // Release the mailbox before native waiting. A
                                // posted wake remains queued even if submission
                                // races with entering the wait.
                                if input.native_waker.get().is_some() {
                                    input.native_waiting.store(true, Ordering::Release);
                                    drop(queue);
                                    if !input.stop.load(Ordering::Acquire) {
                                        access.wait_for_events(
                                            audio_active.then_some(Duration::from_millis(250)),
                                        );
                                    }
                                    input.native_waiting.store(false, Ordering::Release);
                                    queue = input.queue.lock().unwrap_or_else(|e| e.into_inner());
                                    break;
                                }
                                queue = input
                                    .ready
                                    .wait_timeout(
                                        queue,
                                        if audio_active && !access.persistent() {
                                            Duration::from_millis(250)
                                        } else {
                                            Duration::from_millis(20)
                                        },
                                    )
                                    .unwrap_or_else(|e| e.into_inner())
                                    .0;
                                break;
                            }
                            queue = input.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                        }
                        if input.stop.load(Ordering::Acquire) {
                            break;
                        }
                        queue.pop_front()
                    };
                    let Some(pending) = pending else {
                        continue;
                    };
                    let (request, screens) = match pending {
                        Pending::Audio(request) => {
                            emit(BackendEvent::AudioResult(Box::new(execute_audio(
                                &access, request,
                            ))));
                            continue;
                        }
                        Pending::Window { request, screens } => (request, screens),
                    };
                    let id = request.session;
                    let acquisition = matches!(
                        request.operation,
                        WindowOperation::Acquire(_) | WindowOperation::EndEdit { .. }
                    );
                    let query = matches!(request.operation, WindowOperation::Enumerate);
                    let request_id = request.id;
                    let cancelled = || {
                        input.stop.load(Ordering::Acquire)
                            || input.session.load(Ordering::Acquire) != id
                            || (!acquisition
                                && request_id < input.cancel_before.load(Ordering::Acquire))
                            || (query && request_id < input.query_before.load(Ordering::Acquire))
                    };
                    if cancelled() {
                        if input.session.load(Ordering::Acquire) == 0 {
                            session.cleanup_edit(&mut access);
                            access.reset();
                            session = Session::default();
                        }
                        continue;
                    }
                    // Cancellation wakeups carry no displays. Only accepted
                    // requests may replace the persistent groups' geometry context.
                    displays.clone_from(&screens);
                    if session.id != id {
                        session.cleanup_edit(&mut access);
                        access.reset();
                        session = Session {
                            id,
                            ..Session::default()
                        };
                    }
                    let result = session.execute(&mut access, request, &screens, &cancelled);
                    if !cancelled() {
                        emit(BackendEvent::WindowResult(Box::new(result)));
                    } else {
                        session.pending_closed.extend(result.closed);
                    }
                    // Deliver feedback before file I/O; errors never delay the
                    // acknowledgement or run on the engine's keyboard thread.
                    if let Some(error) = session.error.take() {
                        crate::report_error!(
                            "window-worker",
                            "session={id} request={request_id}: {error}"
                        );
                    }
                }
                session.cleanup_edit(&mut access);
                access.shutdown();
            },
        )?;
        Ok(Self { mailbox, worker })
    }

    pub(crate) fn submit(&self, request: WindowRequest, screens: &[Screen]) -> Result<(), String> {
        let mut queue = self
            .mailbox
            .queue
            .lock()
            .map_err(|_| "window queue poisoned")?;
        if matches!(request.operation, WindowOperation::Acquire(_)) {
            self.mailbox
                .session
                .store(request.session, Ordering::Release);
            self.mailbox.cancel_before.store(0, Ordering::Release);
            self.mailbox.query_before.store(0, Ordering::Release);
            queue.retain(|p| matches!(p, Pending::Audio(_)));
        }
        if self.mailbox.session.load(Ordering::Acquire) != request.session {
            return Err("window session expired".into());
        }
        if request.operation.precedes_inventory() {
            self.mailbox
                .query_before
                .store(request.id, Ordering::Release);
            queue.retain(|p| !matches!(p.operation(), Some(WindowOperation::Enumerate)));
        }
        if matches!(request.operation, WindowOperation::CancelPending) {
            self.mailbox
                .cancel_before
                .store(request.id, Ordering::Release);
            queue.retain(|p| {
                matches!(p, Pending::Audio(_))
                    || matches!(
                        p.operation(),
                        Some(
                            WindowOperation::Acquire(_)
                                | WindowOperation::BeginEdit { .. }
                                | WindowOperation::EndEdit { .. }
                        )
                    )
            });
        }
        if matches!(
            request.operation,
            WindowOperation::EndEdit { commit: false, .. }
        ) {
            self.mailbox
                .cancel_before
                .store(request.id, Ordering::Release);
            queue.retain(|p| {
                matches!(p, Pending::Audio(_))
                    || matches!(
                        p.operation(),
                        Some(WindowOperation::Acquire(_) | WindowOperation::BeginEdit { .. })
                    )
            });
        }
        // Absolute layouts replace pending revisions, rather than accumulating
        // intermediate native work. Transaction boundaries are never coalesced.
        if let Some(Pending::Window {
            request: last,
            screens: last_screens,
        }) = queue.back_mut()
            && let (
                WindowOperation::ApplyLayout { transaction: a, .. },
                WindowOperation::ApplyLayout { transaction: b, .. },
            ) = (&last.operation, &request.operation)
            && a == b
        {
            *last = request;
            *last_screens = screens.to_vec();
            return Ok(());
        }
        // Coalesce relative changes only within the same uninterrupted gesture.
        if let Some(Pending::Window { request: last, .. }) = queue.back_mut()
            && last.session == request.session
            && let (
                WindowOperation::Adjust {
                    target: a,
                    change: ca,
                    group: ga,
                },
                WindowOperation::Adjust {
                    target: b,
                    change: cb,
                    group: gb,
                },
            ) = (&mut last.operation, &request.operation)
            && a == b
            && ga == gb
        {
            let merged = match (ca, cb) {
                (WindowChange::Move { dx: ax, dy: ay }, WindowChange::Move { dx: bx, dy: by }) => {
                    *ax += bx;
                    *ay += by;
                    true
                }
                (
                    WindowChange::Resize { dw: ax, dh: ay },
                    WindowChange::Resize { dw: bx, dh: by },
                ) => {
                    *ax += bx;
                    *ay += by;
                    true
                }
                _ => false,
            };
            if merged {
                last.id = request.id;
                return Ok(());
            }
        }
        if queue.len() >= 64 {
            return Err("window operation queue is full".into());
        }
        queue.push_back(Pending::Window {
            request,
            screens: screens.to_vec(),
        });
        drop(queue);
        self.mailbox.notify();
        Ok(())
    }

    /// Audio shares native identity ownership and FIFO order with window work,
    /// but never enters the layout session or its history/cancellation barriers.
    pub(crate) fn submit_audio(
        &self,
        request: crate::api::audio::AudioRequest,
    ) -> Result<(), String> {
        let mut queue = self
            .mailbox
            .queue
            .lock()
            .map_err(|_| "audio queue poisoned")?;
        if self.mailbox.stop.load(Ordering::Acquire) {
            return Err("audio worker stopped".into());
        }
        if queue.len() >= 64 {
            return Err("audio operation queue is full".into());
        }
        queue.push_back(Pending::Audio(request));
        drop(queue);
        self.mailbox.notify();
        Ok(())
    }

    pub(crate) fn cancel_audio(&self, session: u64) {
        let mut queue = self.mailbox.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.retain(|p| !matches!(p, Pending::Audio(r) if r.session == session));
    }

    pub(crate) fn cancel(&self, session: u64) {
        let mut queue = self.mailbox.queue.lock().unwrap_or_else(|error| {
            crate::report_error!("window-worker", "recovering a poisoned cancellation queue");
            error.into_inner()
        });
        if self
            .mailbox
            .session
            .compare_exchange(session, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            queue.retain(|p| matches!(p, Pending::Audio(_)));
            // Wake the owner to release retained native references and undo
            // snapshots immediately, including when no operation is active.
            queue.push_back(Pending::Window {
                request: WindowRequest {
                    scope: None,
                    session,
                    id: 0,
                    operation: WindowOperation::Enumerate,
                },
                screens: Vec::new(),
            });
            self.mailbox.notify();
        }
    }

    pub(crate) fn stop_until(&mut self, deadline: Instant) -> Result<(), String> {
        let guard = self.mailbox.queue.lock().unwrap_or_else(|e| e.into_inner());
        self.mailbox.stop.store(true, Ordering::Release);
        drop(guard);
        self.mailbox.notify();
        self.worker.join_until(deadline)
    }
}

impl Drop for WindowWorker {
    fn drop(&mut self) {
        let guard = self.mailbox.queue.lock().unwrap_or_else(|e| e.into_inner());
        self.mailbox.stop.store(true, Ordering::Release);
        drop(guard);
        self.mailbox.notify();
        if !self.worker.shutdown_failure_was_returned()
            && let Err(error) = self.worker.join_timeout(Duration::from_secs(2))
        {
            crate::report_error!("window-worker", "{error}");
        }
    }
}

#[derive(Default)]
struct Session {
    scope: Option<crate::api::window::WindowScope>,
    id: u64,
    target: Option<WindowId>,
    history: VecDeque<(u64, Vec<Snapshot>)>,
    redo: VecDeque<(u64, Vec<Snapshot>)>,
    initial: std::collections::BTreeMap<WindowId, Snapshot>,
    changed_windows: std::collections::BTreeSet<WindowId>,
    cycle: Vec<WindowId>,
    edit: Option<EditTransaction>,
    screens: Vec<Screen>,
    minimums: std::collections::BTreeMap<WindowId, Point>,
    error: Option<String>,
    pending_closed: Vec<WindowId>,
    resize_minimum: Option<(WindowId, u64, usize, f64, Point)>,
}

struct EditTransaction {
    id: u64,
    group: u64,
    revision: u64,
    before: Vec<Snapshot>,
    minimums: Vec<(WindowId, Point)>,
}

fn rect_matches(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= 1.5
        && (a.y - b.y).abs() <= 1.5
        && (a.width - b.width).abs() <= 1.5
        && (a.height - b.height).abs() <= 1.5
}

fn same_placement(a: &Snapshot, b: &Snapshot) -> bool {
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

    fn recover_edit(&mut self, access: &mut impl WindowAccess) {
        if let Some(edit) = self.edit.take() {
            // Only native partial failure uses bounded entry-layout recovery.
            let deadline = Instant::now() + Duration::from_millis(1500);
            for before in edit.before.iter().rev() {
                if Instant::now() >= deadline {
                    crate::report_error!("window-worker", "entry-layout recovery timed out");
                    break;
                }
                if let Err(error) =
                    access.restore(before, &self.screens, &|| Instant::now() >= deadline)
                {
                    crate::report_error!(
                        "window-worker",
                        "recover window {:?}: {error}",
                        before.info.id
                    );
                }
            }
        }
    }

    fn remember(&mut self, group: u64, before: Snapshot) {
        self.initial
            .entry(before.info.id)
            .or_insert_with(|| before.clone());
        self.changed_windows.insert(before.info.id);
        self.redo.clear();
        if self.history.back().is_none_or(|(g, _)| *g != group) {
            if self.history.len() == 32 {
                self.history.pop_front();
            }
            self.history.push_back((group, Vec::new()));
        }
        if let Some((_, snapshots)) = self.history.back_mut()
            && !snapshots.iter().any(|s| s.info.id == before.info.id)
        {
            snapshots.push(before);
        }
    }

    fn capture_initial(&mut self, access: &impl WindowAccess, id: WindowId, screens: &[Screen]) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.initial.entry(id)
            && let Ok(snapshot) = access.snapshot(id, screens)
        {
            entry.insert(snapshot);
        }
    }

    fn push_history(
        stack: &mut VecDeque<(u64, Vec<Snapshot>)>,
        group: u64,
        snapshots: Vec<Snapshot>,
    ) {
        if snapshots.is_empty() {
            return;
        }
        if stack.len() == 32 {
            stack.pop_front();
        }
        stack.push_back((group, snapshots));
    }

    // Return inverse snapshots for every actual native change, and retain
    // uncompleted work so cancellation or a refused write can be retried.
    fn restore_snapshots(
        access: &mut impl WindowAccess,
        mut snapshots: Vec<Snapshot>,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
        result: &mut WindowResult,
    ) -> (Vec<Snapshot>, Vec<Snapshot>) {
        let mut inverse = Vec::new();
        let mut remaining = Vec::new();
        // One geometry operation per live group, preferring its stable anchor.
        snapshots.sort_by_key(|s| access.layout_representative(s.info.id) != s.info.id);
        let mut represented = std::collections::BTreeSet::new();
        snapshots.retain(|s| represented.insert(access.layout_representative(s.info.id)));
        while let Some(desired) = snapshots.pop() {
            if cancelled() {
                snapshots.push(desired);
                remaining.extend(snapshots);
                break;
            }
            let Ok(before) = access.snapshot(desired.info.id, screens) else {
                remaining.push(desired);
                result.skipped += 1;
                continue;
            };
            if same_placement(&before, &desired) {
                continue;
            }
            let applied = access.restore(&desired, screens, cancelled);
            let after = access.snapshot(desired.info.id, screens);
            if let Ok(after) = &after {
                if !same_placement(&before, after) {
                    inverse.push(before);
                    result.changed += 1;
                }
                if same_placement(after, &desired) {
                    continue;
                }
            } else {
                // The write may have succeeded before the read timed out.
                // Keep its inverse without claiming an observed state change.
                inverse.push(before);
            }
            remaining.push(desired);
            result.skipped += 1;
            if let Err(error) = applied
                && !cancelled()
            {
                crate::report_error!("window-worker", "restore history: {error}");
            }
        }
        (inverse, remaining)
    }

    fn execute(
        &mut self,
        access: &mut impl WindowAccess,
        request: WindowRequest,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
    ) -> WindowResult {
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
        let mut result = WindowResult {
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
        };
        let starting = match &request.operation {
            WindowOperation::BeginEdit { transaction, .. } => Some(*transaction),
            _ => None,
        };
        let outcome = self.apply(access, request.operation, screens, cancelled, &mut result);
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

    fn apply(
        &mut self,
        access: &mut impl WindowAccess,
        operation: WindowOperation,
        screens: &[Screen],
        cancelled: &dyn Fn() -> bool,
        result: &mut WindowResult,
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
            WindowOperation::Cycle | WindowOperation::CyclePrevious => {
                let backwards = matches!(operation, WindowOperation::CyclePrevious);
                let windows = self.enumerate(access, screens, cancelled)?;
                result.windows = Some(windows.clone());
                if cancelled() {
                    return Ok(());
                }
                // Activation changes native Z-order. Preserve the session's
                // existing ring so successive Tabs visit every window instead
                // of oscillating between the two most recently activated ones.
                self.cycle.retain(|id| windows.iter().any(|w| w.id == *id));
                for window in windows {
                    if !self.cycle.contains(&window.id) {
                        self.cycle.push(window.id);
                    }
                }
                if self.cycle.is_empty() {
                    return Err("No ordinary windows available".into());
                }
                let grouped = access.tab_state().and_then(|state| {
                    let group = state.containing(self.target?)?;
                    Some((group.active, group.members.clone()))
                });
                let cycle = grouped
                    .as_ref()
                    .map_or(self.cycle.as_slice(), |(_, members)| members.as_slice());
                let current = grouped.as_ref().map(|(active, _)| *active).or(self.target);
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
                    if access.snapshot(id, screens).is_err() {
                        result.skipped += 1;
                        continue;
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
                return Err("No available window accepted activation".into());
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
                        self.initial.entry(id).or_insert_with(|| snapshot.clone());
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
                    full_inventory: screen.is_some(),
                }));
                self.edit = Some(EditTransaction {
                    id: transaction,
                    group,
                    revision: 0,
                    before,
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
                            return Ok(());
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
                        match access.restore(&before, screens, &|| false) {
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
                let before = access.snapshot(target, screens)?;
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
                let screen = screens
                    .get(before.info.screen)
                    .ok_or("display is unavailable")?;
                let scale = access.logical_scale(screen);
                let base = if before.info.maximized {
                    before.restored
                } else {
                    before.info.bounds
                };
                let pointer = access.pointer().ok();
                let resize_center =
                    matches!(change, WindowChange::Resize { .. }).then_some(base.center());
                let change_result = match change {
                    WindowChange::CycleState => access.cycle_state(target, screens, cancelled),
                    change => {
                        let next = match change {
                            WindowChange::Move { dx, dy } => geometry::constrain_move(
                                Rect::new(
                                    base.x + dx * scale,
                                    base.y + dy * scale,
                                    base.width,
                                    base.height,
                                ),
                                screen.work_area,
                            ),
                            WindowChange::Resize { dw, dh } => {
                                if !before.info.resizable {
                                    return Err("Window does not support resizing".into());
                                }
                                let minimum = match self.resize_minimum {
                                    Some((
                                        id,
                                        cached_group,
                                        cached_screen,
                                        cached_scale,
                                        minimum,
                                    )) if id == target
                                        && cached_group == group
                                        && cached_screen == before.info.screen
                                        && cached_scale == scale =>
                                    {
                                        minimum
                                    }
                                    _ => {
                                        let minimum = access.minimum_size(target);
                                        self.resize_minimum = Some((
                                            target,
                                            group,
                                            before.info.screen,
                                            scale,
                                            minimum,
                                        ));
                                        minimum
                                    }
                                };
                                geometry::resize_center(
                                    base,
                                    dw * scale,
                                    dh * scale,
                                    screen.work_area,
                                    minimum,
                                )
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
                                let Some((source, dest)) = window_placement::destination(
                                    screens,
                                    before.info.bounds,
                                    destination,
                                ) else {
                                    return Ok(());
                                };
                                window_placement::map_between_screens(
                                    base,
                                    &screens[source],
                                    &screens[dest],
                                )
                            }
                            WindowChange::CycleState => unreachable!(),
                        };
                        if cancelled() {
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
                    self.remember(group, before.clone());
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
        windows: BTreeMap<WindowId, Snapshot>,
        writes: Vec<WindowId>,
        reject: Option<WindowId>,
        partial: bool,
        refuse_focus: bool,
        selected: std::cell::Cell<Option<WindowId>>,
        close_requests: std::cell::RefCell<Vec<WindowId>>,
        volume_requests: std::cell::RefCell<Vec<(WindowId, crate::api::audio::AudioAction)>>,
        unchanged_ack: bool,
        snapshot_unavailable: bool,
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
                close_requests: Default::default(),
                volume_requests: Default::default(),
                unchanged_ack: false,
                snapshot_unavailable: false,
                minimum: Point::new(100.0, 80.0),
                minimum_queries: std::cell::Cell::new(0),
                closed: Vec::new(),
            }
        }
    }

    impl WindowAccess for Fake {
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
            if self.snapshot_unavailable {
                return Err("temporarily unavailable".into());
            }
            self.windows.get(&id).cloned().ok_or("closed".into())
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
        fn reset(&mut self) {}
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
}
