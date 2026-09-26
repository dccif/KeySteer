//! Mailbox, cancellation and the native worker event loop.
use super::transaction::PendingTransaction;
use super::*;
enum Pending {
    FocusedBounds {
        id: u64,
        process: u32,
    },
    ClearOverlap,
    Window {
        request: WindowRequest,
        screens: Arc<[Screen]>,
        requires_barrier: bool,
    },
    Audio(crate::api::audio::AudioRequest, Arc<AtomicBool>),
}
fn runnable(queue: &VecDeque<Pending>, frames: &VecDeque<PendingAdjustment>) -> Option<usize> {
    if frames.is_empty() {
        return (!queue.is_empty()).then_some(0);
    }
    // Audio has its own worker; focused bounds are a read-only query outside
    // the edit session. Neither needs to wait for transaction frame barriers.
    if let Some(index) = queue
        .iter()
        .position(|item| matches!(item, Pending::Audio(..) | Pending::FocusedBounds { .. }))
    {
        return Some(index);
    }
    if frames.len() >= 16 {
        return None;
    }
    // Never overtake an earlier window request: consumers reject stale request ids.
    let Pending::Window {
        request,
        screens,
        requires_barrier,
    } = queue.front()?
    else {
        return None;
    };
    if *requires_barrier {
        return None;
    }
    let WindowOperation::Adjust { target, change, .. } = &request.operation else {
        return None;
    };
    if !matches!(
        change,
        WindowChange::Move { .. } | WindowChange::MoveTo(_) | WindowChange::Resize { .. }
    ) {
        return None;
    }
    frames
        .iter()
        .all(|frame| {
            frame.request.session == request.session
                && frame.screens.as_ref() == screens.as_ref()
                && frame.target() != *target
        })
        .then_some(0)
}
impl Pending {
    fn operation(&self) -> Option<&WindowOperation> {
        match self {
            Self::Window { request, .. } => Some(&request.operation),
            Self::Audio(..) | Self::ClearOverlap | Self::FocusedBounds { .. } => None,
        }
    }
}

pub(super) fn execute_audio(
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
    screens: Mutex<Arc<[Screen]>>,
    ready: Condvar,
    audio_tokens: Mutex<std::collections::BTreeMap<u64, Arc<AtomicBool>>>,
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
    pub(crate) fn focused_bounds(&self, id: u64, process: u32) -> Result<(), String> {
        let mut queue = self
            .mailbox
            .queue
            .lock()
            .map_err(|_| "window queue poisoned")?;
        if self.mailbox.stop.load(Ordering::Acquire) {
            return Err("window worker stopped".into());
        }
        queue.retain(|pending| !matches!(pending, Pending::FocusedBounds { .. }));
        if queue.len() >= 64 {
            return Err("window operation queue is full".into());
        }
        queue.push_back(Pending::FocusedBounds { id, process });
        drop(queue);
        self.mailbox.notify();
        Ok(())
    }
    pub(crate) fn clear_overlap_cache(&self) {
        let mut queue = self.mailbox.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.retain(|pending| {
            !matches!(
                pending.operation(),
                Some(WindowOperation::CycleOverlapping { .. })
            ) && !matches!(pending, Pending::ClearOverlap)
        });
        queue.push_back(Pending::ClearOverlap);
        self.mailbox.notify();
    }
    pub(crate) fn start<A: WindowAccess + 'static>(
        create: impl FnOnce() -> A + Send + 'static,
        emit: impl Fn(BackendEvent) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let emit: crate::platform::common::audio_worker::EventSink = Arc::new(emit);
        let mailbox = Arc::new(Mailbox::default());
        let input = mailbox.clone();
        let worker = WorkerJoin::spawn(
            "window-manager",
            std::thread::Builder::new().name("keysteer-window".into()),
            move || {
                let native = A::native_batch(create);
                if let Some(wake) = native.event_waker() {
                    let _ = input.native_waker.set(wake);
                }
                let mut access = crate::platform::common::window_tabs::Grouped::new(native);
                let mut audio: Option<crate::platform::common::audio_worker::AudioWorker> = None;
                let mut session = Session::default();
                let mut focus_cycle = Session::default();
                let mut displays: Arc<[Screen]> = Arc::from([]);
                let mut frames: VecDeque<PendingAdjustment> = VecDeque::new();
                let mut layout: Option<PendingTransaction> = None;
                loop {
                    A::native_batch(|| {
                        if let Err(error) = access.poll_native() {
                            crate::support::logging::report_error_context(
                                "window-confirmation",
                                &error,
                                format_args!("operation=focus"),
                            );
                        }
                        let mut changed = false;
                        if let Some(pending) = &mut layout {
                            let request_session = pending.request().session;
                            let request_id = pending.request().id;
                            let cancelled = || {
                                input.stop.load(Ordering::Acquire)
                                    || input.session.load(Ordering::Acquire) != request_session
                                    || request_id < input.cancel_before.load(Ordering::Acquire)
                            };
                            if let Some(result) = pending.advance_checked(
                                &mut session,
                                &mut access,
                                &cancelled,
                                Instant::now(),
                            ) {
                                if !cancelled() {
                                    if pending.is_layout()
                                        && let Some(error) = &result.message
                                    {
                                        crate::report_error!("window-layout", "{error}");
                                    }
                                    emit(BackendEvent::WindowResult(Box::new(result)));
                                }
                                layout = None;
                                changed = true;
                            }
                        }

                        // Read back every independent operation, but commit history,
                        // target selection and feedback strictly in request order.
                        for frame in &mut frames {
                            let cancelled = input.stop.load(Ordering::Acquire)
                                || input.session.load(Ordering::Acquire) != frame.request.session
                                || frame.request.id < input.cancel_before.load(Ordering::Acquire);
                            frame.advance(&mut access, cancelled);
                        }
                        while let Some(frame) = frames.front_mut() {
                            let cancelled = input.stop.load(Ordering::Acquire)
                                || input.session.load(Ordering::Acquire) != frame.request.session
                                || frame.request.id < input.cancel_before.load(Ordering::Acquire);
                            if !frame.ready() {
                                break;
                            }
                            let result = frame.finish(&mut session, &mut access);
                            let error = result.message.clone();
                            if !cancelled {
                                emit(BackendEvent::WindowResult(Box::new(result)));
                            }
                            if let Some(error) = error.filter(|_| !cancelled) {
                                crate::support::logging::report_error_context(
                                    "window-confirmation",
                                    &error,
                                    format_args!(
                                        "session={} request={}",
                                        frame.request.session, frame.request.id
                                    ),
                                );
                            }
                            frames.pop_front();
                            changed = true;
                        }
                        if changed
                            && let Err(error) = access
                                .watch_confirmations(frames.iter().map(PendingAdjustment::target))
                        {
                            crate::report_error!(
                                "window-confirmation",
                                "operation=unwatch: {error}"
                            );
                        }
                    });
                    let audio_active = A::native_batch(|| {
                        let active = access.maintain_audio();
                        if (access.persistent() || !frames.is_empty() || layout.is_some())
                            && let Err(error) =
                                access.pump(&displays, &|| input.stop.load(Ordering::Acquire))
                        {
                            crate::support::logging::report_error_context(
                                "window-tabs",
                                &error,
                                format_args!("operation=maintain"),
                            );
                        }
                        active
                    });
                    let pending = {
                        let mut queue = input.queue.lock().unwrap_or_else(|e| e.into_inner());
                        if (layout.is_some() || runnable(&queue, &frames).is_none())
                            && (!input.stop.load(Ordering::Acquire) || layout.is_some())
                        {
                            let mut timeout =
                                if layout.as_ref().is_some_and(PendingTransaction::preparing) {
                                    Some(Duration::ZERO)
                                } else if !frames.is_empty() || layout.is_some() {
                                    Some(Duration::from_millis(16))
                                } else if audio_active {
                                    Some(Duration::from_millis(250))
                                } else {
                                    None
                                };
                            if let Some(deadline) = access.native_deadline() {
                                let remaining = deadline.saturating_duration_since(Instant::now());
                                timeout =
                                    Some(timeout.map_or(remaining, |value| value.min(remaining)));
                            }
                            if input.native_waker.get().is_some()
                                && (access.persistent()
                                    || !frames.is_empty()
                                    || layout.is_some()
                                    || audio_active
                                    || access.native_deadline().is_some())
                            {
                                input.native_waiting.store(true, Ordering::Release);
                                drop(queue);
                                if !input.stop.load(Ordering::Acquire) || layout.is_some() {
                                    A::native_batch(|| access.wait_for_events(timeout));
                                }
                                input.native_waiting.store(false, Ordering::Release);
                                queue = input.queue.lock().unwrap_or_else(|e| e.into_inner());
                            } else if let Some(timeout) = timeout.or_else(|| {
                                access.persistent().then_some(Duration::from_millis(20))
                            }) {
                                queue = input
                                    .ready
                                    .wait_timeout(queue, timeout)
                                    .unwrap_or_else(|e| e.into_inner())
                                    .0;
                            } else {
                                queue = input.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                            }
                        }
                        if input.stop.load(Ordering::Acquire) && layout.is_none() {
                            break;
                        }
                        if layout.is_some() {
                            queue
                                .iter()
                                .position(|item| {
                                    matches!(
                                        item,
                                        Pending::Audio(..) | Pending::FocusedBounds { .. }
                                    )
                                })
                                .and_then(|index| queue.remove(index))
                        } else {
                            runnable(&queue, &frames).and_then(|index| queue.remove(index))
                        }
                    };
                    let Some(pending) = pending else {
                        continue;
                    };
                    A::native_batch(|| {
                        let (mut request, screens) = match pending {
                            Pending::FocusedBounds { id, process } => {
                                emit(BackendEvent::FocusedWindowBounds {
                                    id,
                                    bounds: access.focused_bounds(process),
                                });
                                return;
                            }
                            Pending::Audio(request, cancelled) => {
                                if cancelled.load(Ordering::Acquire) {
                                    return;
                                }
                                if let Some(factory) = access.audio_factory() {
                                    let (session, id) = (request.session, request.id);
                                    let submitted = (|| {
                                        let process = access.audio_process(request.target)?;
                                        if audio.is_none() {
                                            audio = Some(crate::platform::common::audio_worker::AudioWorker::start(
                                                factory,
                                                emit.clone(),
                                            )?);
                                        }
                                        audio.as_ref().ok_or("audio worker unavailable")?.submit(
                                            request,
                                            process,
                                            cancelled.clone(),
                                        )
                                    })();
                                    if let Err(error) = submitted
                                        && !cancelled.load(Ordering::Acquire)
                                    {
                                        crate::platform::common::audio_worker::publish_result(
                                            &emit,
                                            crate::api::audio::AudioResult {
                                                session,
                                                id,
                                                outcome: Err(error),
                                            },
                                        );
                                    }
                                } else {
                                    // Portable test/adaptor fallback; native platforms
                                    // always supply the independent audio factory.
                                    let result = execute_audio(&access, request);
                                    if !cancelled.load(Ordering::Acquire) {
                                        crate::platform::common::audio_worker::publish_result(
                                            &emit, result,
                                        );
                                    }
                                }
                                return;
                            }
                            Pending::ClearOverlap => {
                                focus_cycle.overlap = None;
                                session.overlap = None;
                                return;
                            }
                            Pending::Window {
                                request, screens, ..
                            } => (request, screens),
                        };
                        if request.operation.is_standalone_cycle() {
                            let cancelled = || input.stop.load(Ordering::Acquire);
                            displays.clone_from(&screens);
                            let result =
                                focus_cycle.execute(&mut access, request, &screens, &cancelled);
                            if !cancelled() {
                                let outcome = match result.message {
                                    Some(error) => Err(error),
                                    None => match result.pointer {
                                        Some(point) => Ok(point),
                                        None => return, // No overlapping candidate: no focus or pointer change.
                                    },
                                };
                                emit(BackendEvent::WindowCycleCompleted(outcome));
                            }
                            return;
                        }
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
                                || (query
                                    && request_id < input.query_before.load(Ordering::Acquire))
                        };
                        if cancelled() {
                            if input.session.load(Ordering::Acquire) == 0 {
                                session.cleanup_edit(&mut access);
                                access.end_session();
                                session = Session::default();
                            }
                            return;
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
                        match PendingTransaction::take_begin(
                            &mut session,
                            &mut access,
                            &mut request,
                            &screens,
                        ) {
                            Ok(Some(pending)) => {
                                if let Err(error) = access.watch_confirmations(pending.targets()) {
                                    crate::report_warning!("window-layout", "watch: {error}");
                                }
                                layout = Some(pending);
                                return;
                            }
                            Err(error) => {
                                let mut result = Session::result_for(&request);
                                result.message = Some(error.clone());
                                if let WindowOperation::ApplyLayout {
                                    transaction,
                                    revision,
                                    ..
                                } = request.operation
                                {
                                    result.edit = Some(Box::new(WindowEditResult::Applied {
                                        transaction,
                                        revision,
                                        accepted: false,
                                        minimums: Vec::new(),
                                    }));
                                }
                                emit(BackendEvent::WindowResult(Box::new(
                                    session.complete_result(&mut access, result, &screens),
                                )));
                                crate::report_error!("window-layout", "{error}");
                                return;
                            }
                            Ok(None) => {}
                        }
                        if let WindowOperation::Adjust { target, .. } = &request.operation
                            && frames.iter().any(|frame| {
                                access.layout_representative(frame.target())
                                    == access.layout_representative(*target)
                            })
                        {
                            input
                                .queue
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push_front(Pending::Window {
                                    request,
                                    screens,
                                    requires_barrier: true,
                                });
                            return;
                        }
                        let prepared = match PendingAdjustment::begin(
                            &mut session,
                            &mut access,
                            &request,
                            &screens,
                        ) {
                            Ok((Some(frame), _)) => {
                                frames.push_back(frame);
                                if let Err(error) = access.watch_confirmations(
                                    frames.iter().map(PendingAdjustment::target),
                                ) {
                                    // Missing native notifications retain bounded readback fallback.
                                    crate::report_warning!(
                                        "window-confirmation",
                                        "operation=watch: {error}"
                                    );
                                }
                                return;
                            }
                            Err(_) if !frames.is_empty() => {
                                // Preparation failed before any native write. Keep error
                                // delivery behind earlier requests as well.
                                input
                                    .queue
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push_front(Pending::Window {
                                        request,
                                        screens,
                                        requires_barrier: true,
                                    });
                                return;
                            }
                            Err(error) => {
                                // A native write can partially apply before returning an error.
                                // Preserve the normal recovery/history path without replaying it.
                                let mut result = Session::result_for(&request);
                                result.message = Some(error.clone());
                                emit(BackendEvent::WindowResult(Box::new(
                                    session.complete_result(&mut access, result, &screens),
                                )));
                                crate::support::logging::report_error_context(
                                    "window-confirmation",
                                    &error,
                                    format_args!("session={id} request={request_id}"),
                                );
                                return;
                            }
                            Ok((None, _)) if !frames.is_empty() => {
                                input
                                    .queue
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push_front(Pending::Window {
                                        request,
                                        screens,
                                        requires_barrier: true,
                                    });
                                return;
                            }
                            Ok((None, prepared)) => prepared,
                        };
                        let result = session.execute_prepared(
                            &mut access,
                            request,
                            &screens,
                            &cancelled,
                            prepared,
                        );
                        if !cancelled() {
                            emit(BackendEvent::WindowResult(Box::new(result)));
                        } else {
                            session.pending_closed.extend(result.closed);
                        }
                        // Deliver feedback before file I/O; errors never delay the
                        // acknowledgement or run on the engine's keyboard thread.
                        if let Some(error) = session.error.take() {
                            crate::support::logging::report_error_context(
                                "window-worker",
                                &error,
                                format_args!("session={id} request={request_id}"),
                            );
                        }
                    });
                }
                A::native_batch(|| {
                    session.cleanup_edit(&mut access);
                    access.shutdown();
                    drop(access);
                });
            },
        )?;
        Ok(Self { mailbox, worker })
    }

    pub(crate) fn submit(&self, request: WindowRequest, screens: &[Screen]) -> Result<(), String> {
        let screens = {
            let mut cached = self
                .mailbox
                .screens
                .lock()
                .map_err(|_| "display snapshot poisoned")?;
            if cached.as_ref() != screens {
                *cached = Arc::from(screens);
            }
            Arc::clone(&cached)
        };
        let mut queue = self
            .mailbox
            .queue
            .lock()
            .map_err(|_| "window queue poisoned")?;
        if request.operation.is_standalone_cycle() {
            if queue.len() >= 64 {
                return Err("window operation queue is full".into());
            }
            queue.push_back(Pending::Window {
                request,
                screens: screens.clone(),
                requires_barrier: false,
            });
            drop(queue);
            self.mailbox.notify();
            return Ok(());
        }
        if matches!(request.operation, WindowOperation::Acquire(_)) {
            self.mailbox
                .session
                .store(request.session, Ordering::Release);
            self.mailbox.cancel_before.store(0, Ordering::Release);
            self.mailbox.query_before.store(0, Ordering::Release);
            queue.retain(|p| {
                matches!(
                    p,
                    Pending::Audio(..) | Pending::ClearOverlap | Pending::FocusedBounds { .. }
                )
            });
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
                matches!(
                    p,
                    Pending::Audio(..) | Pending::ClearOverlap | Pending::FocusedBounds { .. }
                ) || matches!(
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
                matches!(
                    p,
                    Pending::Audio(..) | Pending::ClearOverlap | Pending::FocusedBounds { .. }
                ) || matches!(
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
            ..
        }) = queue.back_mut()
            && let (
                WindowOperation::ApplyLayout { transaction: a, .. },
                WindowOperation::ApplyLayout { transaction: b, .. },
            ) = (&last.operation, &request.operation)
            && a == b
        {
            *last = request;
            *last_screens = screens.clone();
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
                (WindowChange::MoveTo(a), WindowChange::MoveTo(b)) => {
                    *a = *b;
                    true
                }
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
            screens: screens.clone(),
            requires_barrier: false,
        });
        drop(queue);
        self.mailbox.notify();
        Ok(())
    }

    /// Resolve audio identity in window FIFO order, then execute independently,
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
        let token = self
            .mailbox
            .audio_tokens
            .lock()
            .map_err(|_| "audio cancellation map poisoned")?
            .entry(request.session)
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();
        queue.push_back(Pending::Audio(request, token));
        drop(queue);
        self.mailbox.notify();
        Ok(())
    }

    pub(crate) fn cancel_audio(&self, session: u64) {
        let mut queue = self.mailbox.queue.lock().unwrap_or_else(|e| {
            crate::report_error!("audio", "recovering a poisoned cancellation queue");
            e.into_inner()
        });
        queue.retain(|p| !matches!(p, Pending::Audio(r, _) if r.session == session));
        if let Some(token) = self
            .mailbox
            .audio_tokens
            .lock()
            .unwrap_or_else(|e| {
                crate::report_error!("audio", "recovering a poisoned cancellation map");
                e.into_inner()
            })
            .remove(&session)
        {
            token.store(true, Ordering::Release);
        }
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
            queue.retain(|p| {
                matches!(
                    p,
                    Pending::Audio(..) | Pending::ClearOverlap | Pending::FocusedBounds { .. }
                )
            });
            // Wake the owner to release retained native references and undo
            // snapshots immediately, including when no operation is active.
            queue.push_back(Pending::Window {
                request: WindowRequest {
                    scope: None,
                    session,
                    id: 0,
                    operation: WindowOperation::Enumerate,
                },
                screens: Arc::from([]),
                requires_barrier: false,
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
