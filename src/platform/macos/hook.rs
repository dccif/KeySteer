//! Dedicated CGEventTap thread with per-event disposition handshakes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CallbackResult, EventField,
};

use crate::api::backend::{BackendEvent, KeyDisposition};
use crate::api::command::MouseButton;
use crate::api::geometry::Point;
use crate::api::input::{InputEvent, Key, KeyState};
use crate::platform::multi_click::ClickTracker;
use crate::support::worker::WorkerJoin;

use super::input;

const DISPOSITION_TIMEOUT: Duration = Duration::from_millis(100);
const STOP_TIMEOUT: Duration = Duration::from_millis(250);
pub const TIMEOUT_WARNING: &str =
    "keyboard disposition timed out; the key was forwarded and the event tap remained active";
const CAPTURE_LOSS_NONE: u8 = 0;
const CAPTURE_LOSS_USER_INPUT: u8 = 1;
const CAPTURE_LOSS_REPEATED_TIMEOUT: u8 = 2;
const CAPTURE_LOSS_RUN_LOOP: u8 = 3;
const CAPTURE_LOSS_TAP_UNAVAILABLE: u8 = 4;
const TAP_CAPTURE_ARMED: u8 = 0;
const TAP_CAPTURE_TERMINAL: u8 = 1;
const TAP_CAPTURE_DISARMED: u8 = 2;

struct Envelope {
    event: BackendEvent,
    generation: Option<u64>,
}

/// Cloneable producer for non-hook backend events. Sending through the hook's
/// queue wakes `next_event` immediately, so worker results never wait for
/// pointer input or a polling interval.
#[derive(Clone)]
pub(super) struct EventSender {
    sender: SyncSender<Envelope>,
}

impl EventSender {
    #[cfg(test)]
    pub(super) fn send(&self, event: BackendEvent) -> Result<(), ()> {
        self.sender
            .send(Envelope {
                event,
                generation: None,
            })
            .map_err(|_| ())
    }

    pub(super) fn try_send(&self, event: BackendEvent) -> Result<(), BackendEvent> {
        self.sender
            .try_send(Envelope {
                event,
                generation: None,
            })
            .map_err(|error| match error {
                std::sync::mpsc::TrySendError::Full(envelope)
                | std::sync::mpsc::TrySendError::Disconnected(envelope) => envelope.event,
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TapDisabled {
    Timeout,
}

struct TapState {
    // The event-tap callback is the sole writer for modifier state. Atomics
    // avoid taking a mutex inside the synchronous system callback; only the
    // disabled signal crosses from the callback to the surrounding run loop.
    last_flags: AtomicU64,
    caps_lock_down: AtomicBool,
    side_button_decisions: AtomicU8,
    disabled: AtomicU8,
}

impl Default for TapState {
    fn default() -> Self {
        Self {
            last_flags: AtomicU64::new(0),
            caps_lock_down: AtomicBool::new(false),
            side_button_decisions: AtomicU8::new(0),
            disabled: AtomicU8::new(0),
        }
    }
}

impl TapDisabled {
    const fn code(self) -> u8 {
        match self {
            Self::Timeout => 1,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Timeout),
            _ => None,
        }
    }
}

type SharedState = Arc<TapState>;
type SharedPointer = Arc<crate::platform::latest_point_mailbox::LatestPointMailbox>;
type SharedClickTracker = Arc<Mutex<ClickTracker>>;
type SharedRunLoop = Arc<Mutex<Option<CFRunLoop>>>;
type SharedHookSignals = Arc<HookSignals>;

struct HookSignals {
    capture_loss: AtomicU8,
    drag_modifier_flags: AtomicU8,
    tap_capture_state: AtomicU8,
    character_demand: crate::platform::common::character_candidates::CharacterDemand,
}

impl HookSignals {
    const fn new() -> Self {
        Self {
            capture_loss: AtomicU8::new(CAPTURE_LOSS_NONE),
            drag_modifier_flags: AtomicU8::new(0),
            tap_capture_state: AtomicU8::new(TAP_CAPTURE_ARMED),
            character_demand: crate::platform::common::character_candidates::CharacterDemand::new(),
        }
    }

    fn disarm_tap_capture(&self) {
        let _ = self.tap_capture_state.compare_exchange(
            TAP_CAPTURE_ARMED,
            TAP_CAPTURE_DISARMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn claim_terminal_capture_loss(&self) -> bool {
        self.tap_capture_state
            .compare_exchange(
                TAP_CAPTURE_ARMED,
                TAP_CAPTURE_TERMINAL,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

struct TapCaptureLifecycle {
    signals: SharedHookSignals,
    mailbox: Arc<crate::platform::common::disposition_mailbox::DispositionMailbox>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    run_loop: SharedRunLoop,
}

struct TapInvalidationRegistration {
    port_key: usize,
    lifecycle: Arc<TapCaptureLifecycle>,
}

static TAP_INVALIDATION_REGISTRY: Mutex<Option<TapInvalidationRegistration>> = Mutex::new(None);

struct RegisteredEventTap {
    tap: CGEventTap<'static>,
    port_key: usize,
    lifecycle: Arc<TapCaptureLifecycle>,
}

struct CallbackContext {
    sender: SyncSender<Envelope>,
    mailbox: Arc<crate::platform::common::disposition_mailbox::DispositionMailbox>,
    state: SharedState,
    latest_pointer: SharedPointer,
    click_tracker: SharedClickTracker,
    signals: SharedHookSignals,
    lifecycle: Arc<TapCaptureLifecycle>,
}

pub struct HookThread {
    sender: SyncSender<Envelope>,
    receiver: Receiver<Envelope>,
    mailbox: Arc<crate::platform::common::disposition_mailbox::DispositionMailbox>,
    pending: Option<u64>,
    latest_pointer: SharedPointer,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    signals: SharedHookSignals,
    run_loop: SharedRunLoop,
    worker: WorkerJoin,
    stop_failure_returned: bool,
    deferred: VecDeque<BackendEvent>,
}

pub struct HookStartup {
    sender: SyncSender<Envelope>,
    receiver: Option<Receiver<Envelope>>,
    mailbox: Arc<crate::platform::common::disposition_mailbox::DispositionMailbox>,
    latest_pointer: SharedPointer,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    signals: SharedHookSignals,
    run_loop: SharedRunLoop,
    worker: Option<WorkerJoin>,
    ready: Receiver<Result<(), String>>,
    activate: SyncSender<()>,
    activated: Receiver<()>,
}

struct HookHandshake {
    ready: SyncSender<Result<(), String>>,
    activate: Receiver<()>,
    activated: SyncSender<()>,
}

struct HookThreadContext {
    sender: SyncSender<Envelope>,
    mailbox: Arc<crate::platform::common::disposition_mailbox::DispositionMailbox>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    signals: SharedHookSignals,
    run_loop: SharedRunLoop,
    latest_pointer: SharedPointer,
    click_tracker: SharedClickTracker,
}

impl HookStartup {
    pub fn spawn(click_tracker: SharedClickTracker) -> Result<Self, String> {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mailbox =
            Arc::new(crate::platform::common::disposition_mailbox::DispositionMailbox::default());
        let thread_mailbox = Arc::clone(&mailbox);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (activate_tx, activate_rx) = mpsc::sync_channel(1);
        let (activated_tx, activated_rx) = mpsc::sync_channel(1);
        let handshake = HookHandshake {
            ready: ready_tx,
            activate: activate_rx,
            activated: activated_tx,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let active = Arc::new(AtomicBool::new(false));
        let thread_active = Arc::clone(&active);
        let signals = Arc::new(HookSignals::new());
        let thread_signals = Arc::clone(&signals);
        let run_loop = Arc::new(Mutex::new(None));
        let thread_run_loop = Arc::clone(&run_loop);
        let latest_pointer =
            Arc::new(crate::platform::latest_point_mailbox::LatestPointMailbox::default());
        let thread_pointer = Arc::clone(&latest_pointer);
        let worker = WorkerJoin::spawn(
            "macOS event tap",
            std::thread::Builder::new().name("keysteer-event-tap".into()),
            {
                let event_tx = event_tx.clone();
                move || {
                    event_tap_thread(
                        handshake,
                        HookThreadContext {
                            sender: event_tx,
                            mailbox: thread_mailbox,
                            stop: thread_stop,
                            active: thread_active,
                            signals: thread_signals,
                            run_loop: thread_run_loop,
                            latest_pointer: thread_pointer,
                            click_tracker,
                        },
                    );
                }
            },
        )?;

        Ok(Self {
            sender: event_tx,
            receiver: Some(event_rx),
            mailbox,
            latest_pointer,
            stop,
            active,
            signals,
            run_loop,
            worker: Some(worker),
            ready: ready_rx,
            activate: activate_tx,
            activated: activated_rx,
        })
    }

    pub fn finish(mut self, timeout: Duration) -> Result<HookThread, String> {
        let deadline = Instant::now() + timeout;
        match self.ready.recv_timeout(timeout) {
            Ok(Ok(())) => {
                self.activate
                    .send(())
                    .map_err(|_| "macOS event tap stopped before activation".to_string())?;
                self.activated
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| "macOS event tap did not activate".to_string())?;
                Ok(HookThread {
                    sender: self.sender.clone(),
                    receiver: self
                        .receiver
                        .take()
                        .ok_or_else(|| "macOS event tap receiver is unavailable".to_string())?,
                    mailbox: Arc::clone(&self.mailbox),
                    pending: None,
                    latest_pointer: Arc::clone(&self.latest_pointer),
                    stop: Arc::clone(&self.stop),
                    active: Arc::clone(&self.active),
                    signals: Arc::clone(&self.signals),
                    run_loop: Arc::clone(&self.run_loop),
                    worker: self
                        .worker
                        .take()
                        .ok_or_else(|| "macOS event tap worker is unavailable".to_string())?,
                    stop_failure_returned: false,
                    deferred: VecDeque::new(),
                })
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err("macOS event tap did not start".into()),
        }
    }
}

impl Drop for HookStartup {
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        self.signals.disarm_tap_capture();
        self.stop.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        self.signals.drag_modifier_flags.store(0, Ordering::Relaxed);
        self.mailbox.close();
        let _ = self.activate.try_send(());
        stop_run_loop(&self.run_loop);
        if let Some(mut worker) = self.worker.take()
            && let Err(error) = worker.join_timeout(STOP_TIMEOUT)
        {
            crate::support::logging::report_error("macos-hook", &error);
        }
    }
}

impl HookThread {
    pub(super) fn set_character_bindings(&self, keys: &[Key]) {
        self.signals
            .character_demand
            .configure(keys, |key| input::keycode_for(key).is_some());
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(super) fn event_sender(&self) -> EventSender {
        EventSender {
            sender: self.sender.clone(),
        }
    }

    #[inline]
    pub(super) fn drag_modifier_flags(&self) -> u8 {
        // This is a latest-value snapshot with no ordering relationship to any
        // other state, so Relaxed avoids adding a fence to every drag frame.
        self.signals.drag_modifier_flags.load(Ordering::Relaxed)
    }

    pub fn next_event(&mut self, timeout: Duration) -> Option<BackendEvent> {
        if let Some(event) = self.deferred.pop_front() {
            return Some(event);
        }
        let envelope = match self.receiver.recv_timeout(timeout) {
            Ok(envelope) => envelope,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
        };

        if let BackendEvent::PointerMoved(marker) = envelope.event {
            let point = self.latest_pointer.take().unwrap_or(marker);
            self.pending = envelope.generation;
            return Some(BackendEvent::PointerMoved(point));
        }

        self.pending = envelope.generation;
        Some(envelope.event)
    }

    pub fn try_next_event(&mut self) -> Option<BackendEvent> {
        self.next_event(Duration::ZERO)
    }

    /// Report terminal capture loss out-of-band from the bounded callback
    /// queue. Permission removal can happen while that queue is full, so a
    /// normal `try_send` is not reliable enough for state recovery.
    pub fn take_capture_loss(&mut self) -> Option<BackendEvent> {
        if self.signals.capture_loss.load(Ordering::Acquire) == CAPTURE_LOSS_NONE {
            self.reap_finished();
            return None;
        }
        let reason = self
            .signals
            .capture_loss
            .swap(CAPTURE_LOSS_NONE, Ordering::AcqRel);
        if reason == CAPTURE_LOSS_NONE {
            self.reap_finished();
            return None;
        }

        self.pending = None;
        self.mailbox.close();
        self.signals.drag_modifier_flags.store(0, Ordering::Relaxed);
        self.latest_pointer.clear();
        while let Ok(envelope) = self.receiver.try_recv() {
            if envelope.generation.is_some()
                || matches!(
                    envelope.event,
                    BackendEvent::Input(_)
                        | BackendEvent::PointerMoved(_)
                        | BackendEvent::InputInjectionFailed(_)
                        | BackendEvent::Warning(_)
                )
            {
                continue;
            }
            self.deferred.push_back(envelope.event);
        }
        self.reap_finished();

        let message = match reason {
            CAPTURE_LOSS_USER_INPUT => {
                "macOS disabled physical input capture, usually because Accessibility permission was removed; KeySteer stopped capturing input and must be restarted after permission is restored"
            }
            CAPTURE_LOSS_REPEATED_TIMEOUT => {
                "the macOS event tap timed out again after its single recovery attempt; KeySteer stopped capturing input and must be restarted"
            }
            CAPTURE_LOSS_RUN_LOOP => {
                "the macOS event-tap run loop stopped unexpectedly; KeySteer stopped capturing input and must be restarted"
            }
            CAPTURE_LOSS_TAP_UNAVAILABLE => {
                "the macOS event tap became unavailable without a disable event, usually because Accessibility permission was removed; KeySteer stopped capturing input and must be restarted after permission is restored"
            }
            _ => "macOS physical input capture stopped; KeySteer must be restarted",
        };
        Some(BackendEvent::InputCaptureLost(message.into()))
    }

    fn reap_finished(&mut self) {
        if let Err(error) = self.worker.reap_finished() {
            crate::support::logging::report_error("macos-hook", &error);
        }
    }

    pub fn set_disposition(&mut self, disposition: KeyDisposition) -> Result<(), String> {
        let generation = self
            .pending
            .take()
            .ok_or_else(|| "no macOS keyboard event is awaiting a disposition".to_string())?;
        if self.mailbox.complete(generation, disposition) {
            Ok(())
        } else {
            Err("keyboard disposition deadline expired".into())
        }
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let now = Instant::now();
        let deadline = now.checked_add(STOP_TIMEOUT).unwrap_or(now);
        self.stop_until(deadline)
    }

    /// Make the synchronous event tap fail open immediately. Joining the
    /// worker stays separate so shutdown can release input before waiting on
    /// unrelated scan or network workers.
    pub(super) fn request_stop(&mut self) {
        self.mailbox.close();
        self.signals.disarm_tap_capture();
        self.stop.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        self.signals.drag_modifier_flags.store(0, Ordering::Relaxed);
        self.pending = None;
        stop_run_loop(&self.run_loop);
    }

    pub fn stop_until(&mut self, deadline: Instant) -> Result<(), String> {
        self.request_stop();
        let now = Instant::now();
        let local_deadline = deadline.min(now.checked_add(STOP_TIMEOUT).unwrap_or(deadline));
        let result = self.worker.join_until(local_deadline);
        self.stop_failure_returned = result.is_err();
        result
    }
}

fn stop_run_loop(run_loop: &SharedRunLoop) {
    if let Ok(run_loop) = run_loop.lock()
        && let Some(run_loop) = run_loop.as_ref()
    {
        run_loop.stop();
    }
}

fn invalidation_registry() -> std::sync::MutexGuard<'static, Option<TapInvalidationRegistration>> {
    TAP_INVALIDATION_REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl TapCaptureLifecycle {
    /// Fail open without depending on the bounded event queue. This is safe to
    /// call from either Core Graphics callback: it only updates atomics,
    /// releases a disposition waiter and stops/wakes existing run loops.
    fn signal_capture_loss(&self, reason: u8) {
        if !self.signals.claim_terminal_capture_loss() {
            return;
        }

        self.active.store(false, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        self.signals.drag_modifier_flags.store(0, Ordering::Relaxed);
        self.mailbox.close();
        if self
            .signals
            .capture_loss
            .compare_exchange(
                CAPTURE_LOSS_NONE,
                reason,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            super::workspace::wake_main_run_loop();
        }
        stop_run_loop(&self.run_loop);
    }
}

impl RegisteredEventTap {
    fn new(tap: CGEventTap<'static>, lifecycle: Arc<TapCaptureLifecycle>) -> Result<Self, String> {
        let port_key = super::native::event_tap_identity(tap.mach_port());
        {
            let mut registration = invalidation_registry();
            if registration.is_some() {
                return Err(
                    "another macOS event-tap invalidation callback is already registered"
                        .to_string(),
                );
            }
            *registration = Some(TapInvalidationRegistration {
                port_key,
                lifecycle: Arc::clone(&lifecycle),
            });
        }

        if let Err(error) = super::native::install_event_tap_invalidation_callback(tap.mach_port())
        {
            unregister_event_tap_invalidation(port_key);
            return Err(error);
        }

        Ok(Self {
            tap,
            port_key,
            lifecycle,
        })
    }

    fn mach_port(&self) -> &core_foundation::mach_port::CFMachPort {
        self.tap.mach_port()
    }

    fn enable(&self) {
        self.tap.enable();
    }
}

impl Drop for RegisteredEventTap {
    fn drop(&mut self) {
        self.lifecycle.signals.disarm_tap_capture();
        super::native::clear_event_tap_invalidation_callback(self.tap.mach_port());
        unregister_event_tap_invalidation(self.port_key);
        // `CGEventTap` invalidates its Mach port after this Drop body. The
        // callback is already detached and its Arc registry owner is gone.
    }
}

fn unregister_event_tap_invalidation(port_key: usize) {
    let mut registration = invalidation_registry();
    if registration
        .as_ref()
        .is_some_and(|registered| registered.port_key == port_key)
    {
        *registration = None;
    }
}

/// Core Foundation may invoke this on a non-hook thread when TCC destroys the
/// underlying Mach port. The registry owns the callback state and the clone is
/// taken before releasing the lock, so teardown cannot leave a dangling info
/// pointer. The opaque `info` belongs to the `core-graphics` closure wrapper
/// and must not be interpreted here.
pub(super) fn event_tap_invalidated(port_key: usize) {
    let lifecycle = {
        let registration = invalidation_registry();
        registration
            .as_ref()
            .filter(|registered| registered.port_key == port_key)
            .map(|registered| Arc::clone(&registered.lifecycle))
    };
    if let Some(lifecycle) = lifecycle {
        lifecycle.signal_capture_loss(CAPTURE_LOSS_TAP_UNAVAILABLE);
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        if self.stop_failure_returned || self.worker.shutdown_failure_was_returned() {
            return;
        }
        if let Err(error) = self.stop() {
            crate::support::logging::report_error("macos-hook", &error);
        }
    }
}

fn event_tap_thread(handshake: HookHandshake, context: HookThreadContext) {
    let HookHandshake {
        ready,
        activate,
        activated,
    } = handshake;
    let HookThreadContext {
        sender,
        mailbox,
        stop,
        active,
        signals,
        run_loop: shared_run_loop,
        latest_pointer,
        click_tracker,
    } = context;
    // Build the reverse key table on this worker while AppKit initializes on
    // the main thread. No physical event is captured until activation below.
    input::prewarm_key_map();
    let state = Arc::new(TapState::default());
    let lifecycle = Arc::new(TapCaptureLifecycle {
        signals: Arc::clone(&signals),
        mailbox: Arc::clone(&mailbox),
        stop: Arc::clone(&stop),
        active: Arc::clone(&active),
        run_loop: Arc::clone(&shared_run_loop),
    });
    let callback = CallbackContext {
        sender: sender.clone(),
        mailbox,
        state: Arc::clone(&state),
        latest_pointer,
        click_tracker,
        signals: Arc::clone(&signals),
        lifecycle: Arc::clone(&lifecycle),
    };
    let callback_mailbox = Arc::clone(&callback.mailbox);
    let tap = match create_tap(move |proxy, event_type, event| {
        handle_event(proxy, event_type, event, &callback)
    })
    .and_then(|tap| RegisteredEventTap::new(tap, Arc::clone(&lifecycle)))
    {
        Ok(tap) => tap,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(_) => {
            let _ = ready.send(Err(
                "cannot create a run-loop source for the macOS event tap".into(),
            ));
            return;
        }
    };
    let run_loop = CFRunLoop::get_current();
    if let Ok(mut shared) = shared_run_loop.lock() {
        *shared = Some(run_loop.clone());
    }
    run_loop.add_source(
        &source,
        super::native::default_run_loop_modes().core_foundation,
    );
    if stop.load(Ordering::Acquire) {
        let _ = ready.send(Err(
            "the macOS event tap became unavailable during startup".to_string()
        ));
        detach_event_tap_source(&run_loop, &source, &shared_run_loop);
        return;
    }
    if ready.send(Ok(())).is_err() {
        detach_event_tap_source(&run_loop, &source, &shared_run_loop);
        return;
    }
    if activate.recv().is_err() || stop.load(Ordering::Acquire) {
        detach_event_tap_source(&run_loop, &source, &shared_run_loop);
        return;
    }
    tap.enable();
    active.store(true, Ordering::Release);
    if activated.send(()).is_err() {
        active.store(false, Ordering::Release);
        detach_event_tap_source(&run_loop, &source, &shared_run_loop);
        return;
    }
    crate::support::perf_probe::mark("hook_ready");

    let mut timeout_retried = false;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        // The run loop now sleeps until an explicit stop or source
        // invalidation. Normal input is handled entirely inside its callback
        // and performs no lifecycle load or periodic permission query.
        CFRunLoop::run_current();
        if stop.load(Ordering::Acquire) {
            break;
        }
        let disabled = TapDisabled::from_code(state.disabled.swap(0, Ordering::AcqRel));
        match disabled {
            Some(TapDisabled::Timeout) if !timeout_retried => {
                timeout_retried = true;
                tap.enable();
                let _ = sender.try_send(Envelope {
                    event: BackendEvent::Warning(
                        "the macOS event tap timed out and was re-enabled".into(),
                    ),
                    generation: None,
                });
                super::workspace::wake_main_run_loop();
            }
            Some(TapDisabled::Timeout) => {
                lifecycle.signal_capture_loss(CAPTURE_LOSS_REPEATED_TIMEOUT);
                break;
            }
            None => {
                lifecycle.signal_capture_loss(CAPTURE_LOSS_RUN_LOOP);
                break;
            }
        }
    }
    signals.drag_modifier_flags.store(0, Ordering::Relaxed);
    active.store(false, Ordering::Release);
    callback_mailbox.close();
    detach_event_tap_source(&run_loop, &source, &shared_run_loop);
}

fn detach_event_tap_source(
    run_loop: &CFRunLoop,
    source: &core_foundation::runloop::CFRunLoopSource,
    shared_run_loop: &SharedRunLoop,
) {
    run_loop.remove_source(
        source,
        super::native::default_run_loop_modes().core_foundation,
    );
    if let Ok(mut shared) = shared_run_loop.lock() {
        *shared = None;
    }
}

fn create_tap(
    callback: impl Fn(CGEventTapProxy, CGEventType, &CGEvent) -> CallbackResult + Send + 'static,
) -> Result<CGEventTap<'static>, String> {
    CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::MouseMoved,
            CGEventType::LeftMouseDragged,
            CGEventType::RightMouseDragged,
            CGEventType::OtherMouseDragged,
            CGEventType::TapDisabledByTimeout,
            CGEventType::TapDisabledByUserInput,
        ],
        callback,
    )
    .map_err(|_| "cannot create a CGEventTap (the keyboard cannot be observed)".to_string())
}

fn handle_event(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: &CGEvent,
    context: &CallbackContext,
) -> CallbackResult {
    let CallbackContext {
        sender,
        mailbox,
        state,
        latest_pointer,
        click_tracker,
        signals,
        lifecycle,
    } = context;

    if matches!(
        event_type,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        if matches!(event_type, CGEventType::TapDisabledByTimeout) {
            // A timeout is the only recoverable disable reason. Defer it to the
            // surrounding run loop so that it can perform the one permitted
            // re-enable attempt outside the synchronous callback.
            state
                .disabled
                .store(TapDisabled::Timeout.code(), Ordering::Release);
            stop_run_loop(&lifecycle.run_loop);
        } else {
            // Permission revocation is terminal. Publish capture loss directly
            // from the callback instead of waiting for the surrounding run
            // loop to return. Do not also write the deferred slot: the Engine
            // may consume the out-of-band signal before this worker resumes,
            // and a second signal would otherwise reset input twice.
            lifecycle.signal_capture_loss(CAPTURE_LOSS_USER_INPUT);
        }
        return CallbackResult::Keep;
    }

    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == input::INJECTED_TAG {
        return CallbackResult::Keep;
    }

    if matches!(
        event_type,
        CGEventType::OtherMouseDown | CGEventType::OtherMouseUp
    ) && let Some((number, key_state, key)) = side_button_input(
        event_type,
        event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER),
    ) {
        let previous = state.side_button_decisions.load(Ordering::Relaxed);
        let held = 1 << number;
        let consumed = held << 2;
        let proposed = disposition_for(
            sender,
            mailbox,
            BackendEvent::Input(InputEvent {
                character: None,
                key,
                state: key_state,
                repeat: key_state == KeyState::Down && previous & held != 0,
                injected: false,
                timestamp_millis: 0,
            }),
        );
        let consume = if previous & held != 0 {
            previous & consumed != 0
        } else {
            matches!(proposed, CallbackResult::Drop)
        };
        let next = match key_state {
            KeyState::Down => (previous | held) & !consumed | if consume { consumed } else { 0 },
            KeyState::Up => previous & !(held | consumed),
        };
        state.side_button_decisions.store(next, Ordering::Relaxed);
        if key_state == KeyState::Up && !consume {
            let point = event.location();
            let count = event.get_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE);
            if let Ok(mut tracker) = click_tracker.lock() {
                tracker.observe_completed(
                    if number == 0 {
                        MouseButton::X1
                    } else {
                        MouseButton::X2
                    },
                    Point::new(point.x, point.y),
                    count,
                    Instant::now(),
                );
            }
        }
        return if consume {
            CallbackResult::Drop
        } else {
            CallbackResult::Keep
        };
    }

    match event_type {
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let Some(key) = input::key_for_keycode(code) else {
                return CallbackResult::Keep;
            };
            let key_state = if matches!(event_type, CGEventType::KeyDown) {
                KeyState::Down
            } else {
                KeyState::Up
            };
            let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            let input = InputEvent {
                character: if key_state == KeyState::Down
                    && !repeat
                    && signals.character_demand.is_enabled()
                    && !key.is_modifier()
                {
                    super::native::event_character(event, Some(&signals.character_demand))
                } else {
                    None
                },
                key,
                state: key_state,
                repeat,
                injected: false,
                timestamp_millis: 0,
            };
            disposition_for(sender, mailbox, BackendEvent::Input(input))
        }
        CGEventType::FlagsChanged => {
            let flags = event.get_flags().bits();
            // The callback is the sole writer. Store only the four semantic
            // modifier families needed by synthetic dragged events; clicks and
            // ordinary MouseMoved events never read this atomic.
            signals
                .drag_modifier_flags
                .store(input::compact_drag_modifier_flags(flags), Ordering::Relaxed);
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let Some((key, key_state)) = modifier_transition(state, code, flags) else {
                return CallbackResult::Keep;
            };
            disposition_for(
                sender,
                mailbox,
                BackendEvent::Input(InputEvent {
                    character: None,
                    key,
                    state: key_state,
                    repeat: false,
                    injected: false,
                    timestamp_millis: 0,
                }),
            )
        }
        CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp => {
            let button = match event_type {
                CGEventType::LeftMouseUp => Some(MouseButton::Left),
                CGEventType::RightMouseUp => Some(MouseButton::Right),
                CGEventType::OtherMouseUp => {
                    match event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER) {
                        2 => Some(MouseButton::Middle),
                        3 => Some(MouseButton::X1),
                        4 => Some(MouseButton::X2),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(button) = button {
                let point = event.location();
                let count = event.get_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE);
                if let Ok(mut tracker) = click_tracker.lock() {
                    tracker.observe_completed(
                        button,
                        Point::new(point.x, point.y),
                        count,
                        Instant::now(),
                    );
                }
            }
            CallbackResult::Keep
        }
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            CallbackResult::Keep
        }
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => {
            let point = event.location();
            let point = Point::new(point.x, point.y);
            let should_signal = store_latest_pointer(latest_pointer, point);
            if should_signal {
                let sent = sender
                    .try_send(Envelope {
                        event: BackendEvent::PointerMoved(point),
                        generation: None,
                    })
                    .is_ok();
                if sent {
                    super::workspace::wake_main_run_loop();
                } else {
                    // A full engine queue must never block a CGEventTap
                    // callback. Clear the coalescing marker so the next
                    // native movement can retry publishing a wake signal.
                    cancel_latest_pointer_signal(latest_pointer, point);
                }
            }
            CallbackResult::Keep
        }
        _ => CallbackResult::Keep,
    }
}

fn side_button_input(event_type: CGEventType, button: i64) -> Option<(u8, KeyState, Key)> {
    let number = match button {
        3 => 1,
        4 => 2,
        _ => return None,
    };
    let state = match event_type {
        CGEventType::OtherMouseDown => KeyState::Down,
        CGEventType::OtherMouseUp => KeyState::Up,
        _ => return None,
    };
    Some((number - 1, state, Key::mouse_side_button(number)?))
}

fn store_latest_pointer(latest_pointer: &SharedPointer, point: Point) -> bool {
    latest_pointer.publish(point)
}

fn cancel_latest_pointer_signal(latest_pointer: &SharedPointer, _point: Point) {
    latest_pointer.cancel_signal();
}

fn modifier_transition(state: &SharedState, code: i64, flags: u64) -> Option<(Key, KeyState)> {
    if !matches!(code, 54..=63) {
        return None;
    }
    let key = input::key_for_keycode(code)?;
    let previous_flags = state.last_flags.swap(flags, Ordering::Relaxed);
    let key_state = if code == 57 {
        let was_down = state.caps_lock_down.fetch_xor(true, Ordering::Relaxed);
        if !was_down {
            KeyState::Down
        } else {
            KeyState::Up
        }
    } else {
        let was_down = input::modifier_is_down(previous_flags, &key);
        let is_down = input::modifier_is_down(flags, &key);
        if was_down == is_down {
            return None;
        }
        if is_down {
            KeyState::Down
        } else {
            KeyState::Up
        }
    };
    Some((key, key_state))
}

fn disposition_for(
    sender: &SyncSender<Envelope>,
    mailbox: &crate::platform::common::disposition_mailbox::DispositionMailbox,
    event: BackendEvent,
) -> CallbackResult {
    let correlation_id = crate::support::perf_probe::next_correlation_id();
    crate::support::perf_probe::mark_correlated("hook_received", correlation_id);
    let Some(generation) = mailbox.try_begin() else {
        crate::support::perf_probe::mark_correlated("disposition_returned", correlation_id);
        return CallbackResult::Keep;
    };
    if sender
        .try_send(Envelope {
            event,
            generation: Some(generation),
        })
        .is_err()
    {
        crate::support::perf_probe::mark_correlated("disposition_returned", correlation_id);
        return CallbackResult::Keep;
    }
    super::workspace::wake_main_run_loop();

    let result = match mailbox.wait(generation, DISPOSITION_TIMEOUT) {
        Some(KeyDisposition::Consume) => CallbackResult::Drop,
        Some(KeyDisposition::Defer | KeyDisposition::Forward) => CallbackResult::Keep,
        None => {
            let _ = sender.try_send(Envelope {
                event: BackendEvent::Warning(TIMEOUT_WARNING.into()),
                generation: None,
            });
            CallbackResult::Keep
        }
    };
    crate::support::perf_probe::mark_correlated("disposition_returned", correlation_id);
    result
}

#[cfg(test)]
mod tests {

    #[test]
    fn mouse_side_button_edges_do_not_capture_middle_or_other_buttons() {
        for (button, name) in [(3, "mouse_x1"), (4, "mouse_x2")] {
            let (_, down, key) =
                super::side_button_input(core_graphics::event::CGEventType::OtherMouseDown, button)
                    .unwrap();
            let (_, up, _) =
                super::side_button_input(core_graphics::event::CGEventType::OtherMouseUp, button)
                    .unwrap();
            assert_eq!(key.as_str(), name);
            assert_eq!(down, crate::api::KeyState::Down);
            assert_eq!(up, crate::api::KeyState::Up);
        }
        for button in [0, 1, 2, 5, -1] {
            assert!(
                super::side_button_input(core_graphics::event::CGEventType::OtherMouseDown, button)
                    .is_none()
            );
        }
    }
    use super::*;

    #[test]
    fn only_one_timeout_is_automatically_reenabled() {
        let mut retried = false;
        assert!(matches!(TapDisabled::Timeout, TapDisabled::Timeout) && !retried);
        retried = true;
        assert!(!(matches!(TapDisabled::Timeout, TapDisabled::Timeout) && !retried));
        assert_eq!(
            TapDisabled::from_code(TapDisabled::Timeout.code()),
            Some(TapDisabled::Timeout)
        );
        assert_eq!(TapDisabled::from_code(0), None);
    }

    #[test]
    fn terminal_capture_loss_is_claimed_exactly_once() {
        let signals = HookSignals::new();
        assert!(signals.claim_terminal_capture_loss());
        assert!(!signals.claim_terminal_capture_loss());
        assert_eq!(
            signals.tap_capture_state.load(Ordering::Acquire),
            TAP_CAPTURE_TERMINAL
        );
    }

    #[test]
    fn explicit_shutdown_disarms_native_invalidation() {
        let signals = HookSignals::new();
        signals.disarm_tap_capture();
        assert!(!signals.claim_terminal_capture_loss());
        assert_eq!(
            signals.tap_capture_state.load(Ordering::Acquire),
            TAP_CAPTURE_DISARMED
        );
    }

    #[test]
    fn disposition_is_delivered_to_the_exact_waiting_event() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mailbox =
            Arc::new(crate::platform::common::disposition_mailbox::DispositionMailbox::default());
        let generation = mailbox.begin();
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox: Arc::clone(&mailbox),
            pending: Some(generation),
            latest_pointer: Arc::new(
                crate::platform::latest_point_mailbox::LatestPointMailbox::default(),
            ),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            signals: Arc::new(HookSignals::new()),
            run_loop: Arc::new(Mutex::new(None)),
            worker: WorkerJoin::spawn("test hook", std::thread::Builder::new(), || {}).unwrap(),
            stop_failure_returned: false,
            deferred: VecDeque::new(),
        };
        hook.set_disposition(KeyDisposition::Consume).unwrap();
        assert_eq!(
            mailbox.wait(generation, Duration::ZERO),
            Some(KeyDisposition::Consume)
        );
    }

    #[test]
    fn timed_out_response_rejects_late_acknowledgement() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mailbox =
            Arc::new(crate::platform::common::disposition_mailbox::DispositionMailbox::default());
        let generation = mailbox.begin();
        assert_eq!(mailbox.wait(generation, Duration::ZERO), None);
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox,
            pending: Some(generation),
            latest_pointer: Arc::new(
                crate::platform::latest_point_mailbox::LatestPointMailbox::default(),
            ),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            signals: Arc::new(HookSignals::new()),
            run_loop: Arc::new(Mutex::new(None)),
            worker: WorkerJoin::spawn("test hook", std::thread::Builder::new(), || {}).unwrap(),
            stop_failure_returned: false,
            deferred: VecDeque::new(),
        };
        assert!(hook.set_disposition(KeyDisposition::Forward).is_err());
        assert!(hook.pending.is_none());
    }

    #[test]
    fn a_full_event_queue_fails_open_without_blocking_the_event_tap() {
        let (event_tx, _event_rx) = mpsc::sync_channel(1);
        event_tx
            .try_send(Envelope {
                event: BackendEvent::ReloadConfig,
                generation: None,
            })
            .unwrap();
        let mailbox = crate::platform::common::disposition_mailbox::DispositionMailbox::default();

        assert!(matches!(
            disposition_for(
                &event_tx,
                &mailbox,
                BackendEvent::Input(InputEvent {
                    character: None,
                    key: Key::new("a").unwrap(),
                    state: KeyState::Down,
                    repeat: false,
                    injected: false,
                    timestamp_millis: 0,
                }),
            ),
            CallbackResult::Keep
        ));
    }

    #[test]
    fn capture_loss_is_delivered_even_when_the_event_queue_is_full() {
        let (event_tx, event_rx) = mpsc::sync_channel(1);
        event_tx
            .try_send(Envelope {
                event: BackendEvent::Input(InputEvent {
                    character: None,
                    key: Key::new("a").unwrap(),
                    state: KeyState::Down,
                    repeat: false,
                    injected: false,
                    timestamp_millis: 0,
                }),
                generation: Some(1),
            })
            .unwrap();
        let signals = Arc::new(HookSignals {
            capture_loss: AtomicU8::new(CAPTURE_LOSS_NONE),
            drag_modifier_flags: AtomicU8::new(
                input::DRAG_MODIFIER_SHIFT | input::DRAG_MODIFIER_COMMAND,
            ),
            tap_capture_state: AtomicU8::new(TAP_CAPTURE_ARMED),
            character_demand: crate::platform::common::character_candidates::CharacterDemand::new(),
        });
        let mailbox =
            Arc::new(crate::platform::common::disposition_mailbox::DispositionMailbox::default());
        let generation = mailbox.begin();
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(true));
        let run_loop = Arc::new(Mutex::new(None));
        let lifecycle = TapCaptureLifecycle {
            signals: Arc::clone(&signals),
            mailbox: Arc::clone(&mailbox),
            stop: Arc::clone(&stop),
            active: Arc::clone(&active),
            run_loop: Arc::clone(&run_loop),
        };
        lifecycle.signal_capture_loss(CAPTURE_LOSS_USER_INPUT);
        assert_eq!(mailbox.wait(generation, Duration::ZERO), None);
        let latest_pointer =
            Arc::new(crate::platform::latest_point_mailbox::LatestPointMailbox::default());
        latest_pointer.publish(Point::new(1.0, 2.0));
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox,
            pending: Some(generation),
            latest_pointer,
            stop: Arc::clone(&stop),
            active: Arc::clone(&active),
            signals: Arc::clone(&signals),
            run_loop,
            worker: WorkerJoin::spawn("test hook", std::thread::Builder::new(), || {}).unwrap(),
            stop_failure_returned: false,
            deferred: VecDeque::new(),
        };

        assert!(matches!(
            hook.take_capture_loss(),
            Some(BackendEvent::InputCaptureLost(_))
        ));
        // A port invalidation can race the disabled event after the main
        // thread has consumed its signal. The lifecycle terminal claim, not
        // the transient mailbox value, prevents a second recovery cycle.
        lifecycle.signal_capture_loss(CAPTURE_LOSS_TAP_UNAVAILABLE);
        assert!(hook.take_capture_loss().is_none());
        assert!(hook.try_next_event().is_none());
        assert!(hook.pending.is_none());
        assert_eq!(hook.latest_pointer.take(), None);
        assert_eq!(
            signals.capture_loss.load(Ordering::Acquire),
            CAPTURE_LOSS_NONE
        );
        assert_eq!(signals.drag_modifier_flags.load(Ordering::Relaxed), 0);
        assert!(stop.load(Ordering::Acquire));
        assert!(!active.load(Ordering::Acquire));
    }

    #[test]
    fn pointer_burst_uses_one_latest_value_slot() {
        let latest = Arc::new(crate::platform::latest_point_mailbox::LatestPointMailbox::default());
        let mut signals = 0;
        for index in 0..10_000 {
            signals += usize::from(store_latest_pointer(
                &latest,
                Point::new(index as f64, index as f64),
            ));
        }
        assert_eq!(signals, 1);
        assert_eq!(latest.take(), Some(Point::new(9_999.0, 9_999.0)));
    }

    #[test]
    fn a_failed_pointer_signal_can_be_retried() {
        let latest = Arc::new(crate::platform::latest_point_mailbox::LatestPointMailbox::default());
        let first = Point::new(10.0, 20.0);
        assert!(store_latest_pointer(&latest, first));
        cancel_latest_pointer_signal(&latest, first);
        assert!(store_latest_pointer(&latest, Point::new(11.0, 21.0)));
    }

    #[test]
    fn repeated_native_edge_position_is_still_delivered() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let latest_pointer =
            Arc::new(crate::platform::latest_point_mailbox::LatestPointMailbox::default());
        let mut hook = HookThread {
            sender: event_tx.clone(),
            receiver: event_rx,
            mailbox: Arc::new(
                crate::platform::common::disposition_mailbox::DispositionMailbox::default(),
            ),
            pending: None,
            latest_pointer: Arc::clone(&latest_pointer),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            signals: Arc::new(HookSignals::new()),
            run_loop: Arc::new(Mutex::new(None)),
            worker: WorkerJoin::spawn("test hook", std::thread::Builder::new(), || {}).unwrap(),
            stop_failure_returned: false,
            deferred: VecDeque::new(),
        };
        let edge = Point::new(999.0, 400.0);
        for _ in 0..2 {
            assert!(store_latest_pointer(&latest_pointer, edge));
            event_tx
                .send(Envelope {
                    event: BackendEvent::PointerMoved(edge),
                    generation: None,
                })
                .unwrap();
            assert!(matches!(
                hook.next_event(Duration::from_millis(10)),
                Some(BackendEvent::PointerMoved(point)) if point == edge
            ));
        }
    }

    #[test]
    fn external_events_wake_the_hook_queue_without_pointer_input() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox: Arc::new(
                crate::platform::common::disposition_mailbox::DispositionMailbox::default(),
            ),
            pending: None,
            latest_pointer: Arc::new(
                crate::platform::latest_point_mailbox::LatestPointMailbox::default(),
            ),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            signals: Arc::new(HookSignals::new()),
            run_loop: Arc::new(Mutex::new(None)),
            worker: WorkerJoin::spawn("test hook", std::thread::Builder::new(), || {}).unwrap(),
            stop_failure_returned: false,
            deferred: VecDeque::new(),
        };
        hook.event_sender()
            .send(BackendEvent::ReloadConfig)
            .unwrap();
        assert!(matches!(
            hook.next_event(Duration::from_millis(10)),
            Some(BackendEvent::ReloadConfig)
        ));
    }

    #[test]
    fn external_event_sender_never_blocks_when_the_hook_queue_is_full() {
        let (event_tx, _event_rx) = mpsc::sync_channel(1);
        event_tx
            .send(Envelope {
                event: BackendEvent::ReloadConfig,
                generation: None,
            })
            .unwrap();
        let sender = EventSender { sender: event_tx };

        assert!(matches!(
            sender.try_send(BackendEvent::OpenConfigSimulator),
            Err(BackendEvent::OpenConfigSimulator)
        ));
    }

    #[test]
    fn caps_lock_reports_a_physical_press_and_release() {
        let state = Arc::new(TapState::default());
        let (key, down) = modifier_transition(&state, 57, 0x0001_0000).unwrap();
        assert_eq!(key.as_str(), "caps_lock");
        assert_eq!(down, KeyState::Down);
        let (_, up) = modifier_transition(&state, 57, 0x0001_0000).unwrap();
        assert_eq!(up, KeyState::Up);
    }

    #[test]
    fn left_and_right_modifiers_are_distinct() {
        let state = Arc::new(TapState::default());
        let (left, state_value) = modifier_transition(&state, 56, 0x0000_0002).unwrap();
        assert_eq!(left.as_str(), "left_shift");
        assert_eq!(state_value, KeyState::Down);
        let (right, state_value) = modifier_transition(&state, 60, 0x0000_0006).unwrap();
        assert_eq!(right.as_str(), "right_shift");
        assert_eq!(state_value, KeyState::Down);
    }
}
