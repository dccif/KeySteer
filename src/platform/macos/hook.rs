//! Dedicated CGEventTap thread with per-event disposition handshakes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CallbackResult, EventField,
};

use crate::api::backend::{BackendEvent, KeyDisposition};
use crate::api::command::MouseButton;
use crate::api::geometry::Point;
use crate::api::input::{InputEvent, Key, KeyState};
use crate::platform::multi_click::ClickTracker;

use super::input;

const DISPOSITION_TIMEOUT: Duration = Duration::from_millis(100);
const RUN_LOOP_SLICE: Duration = Duration::from_millis(20);
const STOP_TIMEOUT: Duration = Duration::from_millis(250);
pub const TIMEOUT_WARNING: &str =
    "keyboard disposition timed out; the key was forwarded and the event tap remained active";
const CAPTURE_LOSS_NONE: u8 = 0;
const CAPTURE_LOSS_USER_INPUT: u8 = 1;
const CAPTURE_LOSS_REPEATED_TIMEOUT: u8 = 2;

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
    UserInput,
}

#[derive(Default)]
struct TapState {
    last_flags: u64,
    caps_lock_down: bool,
    disabled: Option<TapDisabled>,
}

type SharedState = Arc<Mutex<TapState>>;
type SharedPointer = Arc<Mutex<Option<Point>>>;
type SharedClickTracker = Arc<Mutex<ClickTracker>>;
type SharedRunLoop = Arc<Mutex<Option<CFRunLoop>>>;

fn default_run_loop_mode() -> core_foundation::runloop::CFRunLoopMode {
    // SAFETY: Core Foundation exports this process-lifetime static mode; all
    // callers only borrow the pointer for a CFRunLoop API call.
    unsafe { kCFRunLoopDefaultMode }
}

struct CallbackContext {
    sender: SyncSender<Envelope>,
    mailbox: Arc<crate::platform::disposition_mailbox::DispositionMailbox>,
    state: SharedState,
    latest_pointer: SharedPointer,
    click_tracker: SharedClickTracker,
}

pub struct HookThread {
    sender: SyncSender<Envelope>,
    receiver: Receiver<Envelope>,
    mailbox: Arc<crate::platform::disposition_mailbox::DispositionMailbox>,
    pending: Option<u64>,
    latest_pointer: SharedPointer,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    capture_loss: Arc<AtomicU8>,
    run_loop: SharedRunLoop,
    finished: Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
    deferred: VecDeque<BackendEvent>,
}

pub struct HookStartup {
    sender: SyncSender<Envelope>,
    receiver: Option<Receiver<Envelope>>,
    mailbox: Arc<crate::platform::disposition_mailbox::DispositionMailbox>,
    latest_pointer: SharedPointer,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    capture_loss: Arc<AtomicU8>,
    run_loop: SharedRunLoop,
    finished: Option<Receiver<()>>,
    join: Option<std::thread::JoinHandle<()>>,
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
    mailbox: Arc<crate::platform::disposition_mailbox::DispositionMailbox>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    capture_loss: Arc<AtomicU8>,
    run_loop: SharedRunLoop,
    latest_pointer: SharedPointer,
    click_tracker: SharedClickTracker,
}

impl HookStartup {
    pub fn spawn(click_tracker: SharedClickTracker) -> Result<Self, String> {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mailbox = Arc::new(crate::platform::disposition_mailbox::DispositionMailbox::default());
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
        let capture_loss = Arc::new(AtomicU8::new(CAPTURE_LOSS_NONE));
        let thread_capture_loss = Arc::clone(&capture_loss);
        let run_loop = Arc::new(Mutex::new(None));
        let thread_run_loop = Arc::clone(&run_loop);
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let latest_pointer = Arc::new(Mutex::new(None));
        let thread_pointer = Arc::clone(&latest_pointer);
        let join = std::thread::Builder::new()
            .name("keysteer-event-tap".into())
            .spawn({
                let event_tx = event_tx.clone();
                move || {
                    event_tap_thread(
                        handshake,
                        HookThreadContext {
                            sender: event_tx,
                            mailbox: thread_mailbox,
                            stop: thread_stop,
                            active: thread_active,
                            capture_loss: thread_capture_loss,
                            run_loop: thread_run_loop,
                            latest_pointer: thread_pointer,
                            click_tracker,
                        },
                    );
                    let _ = finished_tx.send(());
                }
            })
            .map_err(|error| format!("cannot start macOS event tap thread: {error}"))?;

        Ok(Self {
            sender: event_tx,
            receiver: Some(event_rx),
            mailbox,
            latest_pointer,
            stop,
            active,
            capture_loss,
            run_loop,
            finished: Some(finished_rx),
            join: Some(join),
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
                    capture_loss: Arc::clone(&self.capture_loss),
                    run_loop: Arc::clone(&self.run_loop),
                    finished: self.finished.take().ok_or_else(|| {
                        "macOS event tap completion signal is unavailable".to_string()
                    })?,
                    join: self.join.take(),
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
        if self.join.is_none() {
            return;
        }
        self.stop.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        self.mailbox.cancel_pending();
        let _ = self.activate.try_send(());
        stop_run_loop(&self.run_loop);
        if let Some(finished) = self.finished.as_ref() {
            join_with_timeout(
                &mut self.join,
                finished,
                "event tap thread did not stop during startup cleanup",
            );
        }
    }
}

impl HookThread {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(super) fn event_sender(&self) -> EventSender {
        EventSender {
            sender: self.sender.clone(),
        }
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
            let point = self
                .latest_pointer
                .lock()
                .map(|mut latest| latest.take())
                .unwrap_or(None)
                .unwrap_or(marker);
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
        let reason = self.capture_loss.swap(CAPTURE_LOSS_NONE, Ordering::AcqRel);
        if reason == CAPTURE_LOSS_NONE {
            self.reap_finished();
            return None;
        }

        self.pending = None;
        self.mailbox.cancel_pending();
        if let Ok(mut pointer) = self.latest_pointer.lock() {
            pointer.take();
        }
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
            _ => "macOS physical input capture stopped; KeySteer must be restarted",
        };
        Some(BackendEvent::InputCaptureLost(message.into()))
    }

    fn reap_finished(&mut self) {
        if !self
            .join
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            return;
        }
        if let Some(handle) = self.join.take()
            && handle.join().is_err()
        {
            crate::app::logging::report_error("macos-hook", "event tap thread panicked");
        }
    }

    pub fn set_disposition(&mut self, disposition: KeyDisposition) -> Result<(), String> {
        let generation = self
            .pending
            .take()
            .ok_or_else(|| "no macOS keyboard event is awaiting a disposition".to_string())?;
        // A callback may already have timed out and failed open. Generation
        // matching makes its late response harmless.
        let _ = self.mailbox.complete(generation, disposition);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        self.mailbox.cancel_pending();
        stop_run_loop(&self.run_loop);
        join_with_timeout(
            &mut self.join,
            &self.finished,
            "event tap thread did not stop within 250 ms; detaching it safely",
        );
        self.pending = None;
    }
}

fn stop_run_loop(run_loop: &SharedRunLoop) {
    if let Ok(run_loop) = run_loop.lock()
        && let Some(run_loop) = run_loop.as_ref()
    {
        run_loop.stop();
    }
}

fn join_with_timeout(
    join: &mut Option<std::thread::JoinHandle<()>>,
    finished: &Receiver<()>,
    timeout_message: &str,
) {
    let Some(handle) = join.take() else {
        return;
    };
    match finished.recv_timeout(STOP_TIMEOUT) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => {
            if handle.join().is_err() {
                crate::app::logging::report_error("macos-hook", "event tap thread panicked");
            }
        }
        Err(RecvTimeoutError::Timeout) => {
            // Dropping JoinHandle detaches; the worker owns all native values
            // it may still touch and has already been told to fail open.
            drop(handle);
            crate::app::logging::report_error("macos-hook", timeout_message);
        }
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        self.stop();
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
        capture_loss,
        run_loop: shared_run_loop,
        latest_pointer,
        click_tracker,
    } = context;
    // Build the reverse key table on this worker while AppKit initializes on
    // the main thread. No physical event is captured until activation below.
    input::prewarm_key_map();
    let state = Arc::new(Mutex::new(TapState::default()));
    let callback = CallbackContext {
        sender: sender.clone(),
        mailbox,
        state: Arc::clone(&state),
        latest_pointer,
        click_tracker,
    };
    let callback_mailbox = Arc::clone(&callback.mailbox);
    let tap = match create_tap(move |proxy, event_type, event| {
        handle_event(proxy, event_type, event, &callback)
    }) {
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
    run_loop.add_source(&source, default_run_loop_mode());
    if ready.send(Ok(())).is_err() {
        return;
    }
    if activate.recv().is_err() || stop.load(Ordering::Acquire) {
        return;
    }
    tap.enable();
    active.store(true, Ordering::Release);
    if activated.send(()).is_err() {
        active.store(false, Ordering::Release);
        return;
    }

    let mut timeout_retried = false;
    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(default_run_loop_mode(), RUN_LOOP_SLICE, true);
        let disabled = state
            .lock()
            .map(|mut state| state.disabled.take())
            .unwrap_or(None);
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
            Some(disabled) => {
                active.store(false, Ordering::Release);
                stop.store(true, Ordering::Release);
                callback_mailbox.cancel_pending();
                let reason = if disabled == TapDisabled::UserInput {
                    CAPTURE_LOSS_USER_INPUT
                } else {
                    CAPTURE_LOSS_REPEATED_TIMEOUT
                };
                let _ = capture_loss.compare_exchange(
                    CAPTURE_LOSS_NONE,
                    reason,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                super::workspace::wake_main_run_loop();
            }
            None => {}
        }
    }
    active.store(false, Ordering::Release);
    callback_mailbox.cancel_pending();
    run_loop.remove_source(&source, default_run_loop_mode());
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
    } = context;
    if matches!(
        event_type,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        if let Ok(mut state) = state.lock() {
            state.disabled = Some(if matches!(event_type, CGEventType::TapDisabledByTimeout) {
                TapDisabled::Timeout
            } else {
                TapDisabled::UserInput
            });
        }
        return CallbackResult::Keep;
    }

    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == input::INJECTED_TAG {
        return CallbackResult::Keep;
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
            let input = InputEvent {
                key,
                state: key_state,
                repeat: event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0,
                injected: false,
                timestamp_millis: 0,
            };
            disposition_for(sender, mailbox, BackendEvent::Input(input))
        }
        CGEventType::FlagsChanged => {
            let flags = event.get_flags().bits();
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let Some((key, key_state)) = modifier_transition(state, code, flags) else {
                return CallbackResult::Keep;
            };
            disposition_for(
                sender,
                mailbox,
                BackendEvent::Input(InputEvent {
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

fn store_latest_pointer(latest_pointer: &SharedPointer, point: Point) -> bool {
    latest_pointer
        .lock()
        .map(|mut latest| {
            let should_signal = latest.is_none();
            *latest = Some(point);
            should_signal
        })
        .unwrap_or(false)
}

fn cancel_latest_pointer_signal(latest_pointer: &SharedPointer, point: Point) {
    if let Ok(mut latest) = latest_pointer.lock()
        && *latest == Some(point)
    {
        *latest = None;
    }
}

fn modifier_transition(state: &SharedState, code: i64, flags: u64) -> Option<(Key, KeyState)> {
    if !matches!(code, 54..=63) {
        return None;
    }
    let key = input::key_for_keycode(code)?;
    let mut state = state.lock().ok()?;
    let key_state = if code == 57 {
        state.caps_lock_down = !state.caps_lock_down;
        if state.caps_lock_down {
            KeyState::Down
        } else {
            KeyState::Up
        }
    } else {
        let was_down = input::modifier_is_down(state.last_flags, &key);
        let is_down = input::modifier_is_down(flags, &key);
        state.last_flags = flags;
        if was_down == is_down {
            return None;
        }
        if is_down {
            KeyState::Down
        } else {
            KeyState::Up
        }
    };
    state.last_flags = flags;
    Some((key, key_state))
}

fn disposition_for(
    sender: &SyncSender<Envelope>,
    mailbox: &crate::platform::disposition_mailbox::DispositionMailbox,
    event: BackendEvent,
) -> CallbackResult {
    let generation = mailbox.begin();
    if sender
        .try_send(Envelope {
            event,
            generation: Some(generation),
        })
        .is_err()
    {
        return CallbackResult::Keep;
    }
    super::workspace::wake_main_run_loop();

    match mailbox.wait(generation, DISPOSITION_TIMEOUT) {
        Some(KeyDisposition::Consume) => CallbackResult::Drop,
        Some(KeyDisposition::Defer | KeyDisposition::Forward) => CallbackResult::Keep,
        None => {
            let _ = sender.try_send(Envelope {
                event: BackendEvent::Warning(TIMEOUT_WARNING.into()),
                generation: None,
            });
            CallbackResult::Keep
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_timeout_is_automatically_reenabled() {
        let mut retried = false;
        assert!(matches!(TapDisabled::Timeout, TapDisabled::Timeout) && !retried);
        retried = true;
        assert!(!(matches!(TapDisabled::Timeout, TapDisabled::Timeout) && !retried));
        assert!(!matches!(TapDisabled::UserInput, TapDisabled::Timeout));
    }

    #[test]
    fn disposition_is_delivered_to_the_exact_waiting_event() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mailbox = Arc::new(crate::platform::disposition_mailbox::DispositionMailbox::default());
        let generation = mailbox.begin();
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox: Arc::clone(&mailbox),
            pending: Some(generation),
            latest_pointer: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            capture_loss: Arc::new(AtomicU8::new(CAPTURE_LOSS_NONE)),
            run_loop: Arc::new(Mutex::new(None)),
            finished: mpsc::sync_channel(1).1,
            join: None,
            deferred: VecDeque::new(),
        };
        hook.set_disposition(KeyDisposition::Consume).unwrap();
        assert_eq!(
            mailbox.wait(generation, Duration::ZERO),
            Some(KeyDisposition::Consume)
        );
    }

    #[test]
    fn timed_out_response_channel_is_nonfatal() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mailbox = Arc::new(crate::platform::disposition_mailbox::DispositionMailbox::default());
        let generation = mailbox.begin();
        let _newer = mailbox.begin();
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox,
            pending: Some(generation),
            latest_pointer: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            capture_loss: Arc::new(AtomicU8::new(CAPTURE_LOSS_NONE)),
            run_loop: Arc::new(Mutex::new(None)),
            finished: mpsc::sync_channel(1).1,
            join: None,
            deferred: VecDeque::new(),
        };
        hook.set_disposition(KeyDisposition::Forward).unwrap();
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
        let mailbox = crate::platform::disposition_mailbox::DispositionMailbox::default();

        assert!(matches!(
            disposition_for(
                &event_tx,
                &mailbox,
                BackendEvent::Input(InputEvent {
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
                    key: Key::new("a").unwrap(),
                    state: KeyState::Down,
                    repeat: false,
                    injected: false,
                    timestamp_millis: 0,
                }),
                generation: Some(1),
            })
            .unwrap();
        let capture_loss = Arc::new(AtomicU8::new(CAPTURE_LOSS_USER_INPUT));
        let mut hook = HookThread {
            sender: event_tx,
            receiver: event_rx,
            mailbox: Arc::new(crate::platform::disposition_mailbox::DispositionMailbox::default()),
            pending: Some(1),
            latest_pointer: Arc::new(Mutex::new(Some(Point::new(1.0, 2.0)))),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(false)),
            capture_loss,
            run_loop: Arc::new(Mutex::new(None)),
            finished: mpsc::sync_channel(1).1,
            join: None,
            deferred: VecDeque::new(),
        };

        assert!(matches!(
            hook.take_capture_loss(),
            Some(BackendEvent::InputCaptureLost(_))
        ));
        assert!(hook.try_next_event().is_none());
        assert!(hook.pending.is_none());
        assert_eq!(*hook.latest_pointer.lock().unwrap(), None);
    }

    #[test]
    fn pointer_burst_uses_one_latest_value_slot() {
        let latest = Arc::new(Mutex::new(None));
        let mut signals = 0;
        for index in 0..10_000 {
            signals += usize::from(store_latest_pointer(
                &latest,
                Point::new(index as f64, index as f64),
            ));
        }
        assert_eq!(signals, 1);
        assert_eq!(*latest.lock().unwrap(), Some(Point::new(9_999.0, 9_999.0)));
    }

    #[test]
    fn a_failed_pointer_signal_can_be_retried() {
        let latest = Arc::new(Mutex::new(None));
        let first = Point::new(10.0, 20.0);
        assert!(store_latest_pointer(&latest, first));
        cancel_latest_pointer_signal(&latest, first);
        assert!(store_latest_pointer(&latest, Point::new(11.0, 21.0)));
    }

    #[test]
    fn repeated_native_edge_position_is_still_delivered() {
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let latest_pointer = Arc::new(Mutex::new(None));
        let mut hook = HookThread {
            sender: event_tx.clone(),
            receiver: event_rx,
            mailbox: Arc::new(crate::platform::disposition_mailbox::DispositionMailbox::default()),
            pending: None,
            latest_pointer: Arc::clone(&latest_pointer),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            capture_loss: Arc::new(AtomicU8::new(CAPTURE_LOSS_NONE)),
            run_loop: Arc::new(Mutex::new(None)),
            finished: mpsc::sync_channel(1).1,
            join: None,
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
            mailbox: Arc::new(crate::platform::disposition_mailbox::DispositionMailbox::default()),
            pending: None,
            latest_pointer: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(true)),
            capture_loss: Arc::new(AtomicU8::new(CAPTURE_LOSS_NONE)),
            run_loop: Arc::new(Mutex::new(None)),
            finished: mpsc::sync_channel(1).1,
            join: None,
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
        let state = Arc::new(Mutex::new(TapState::default()));
        let (key, down) = modifier_transition(&state, 57, 0x0001_0000).unwrap();
        assert_eq!(key.as_str(), "caps_lock");
        assert_eq!(down, KeyState::Down);
        let (_, up) = modifier_transition(&state, 57, 0x0001_0000).unwrap();
        assert_eq!(up, KeyState::Up);
    }

    #[test]
    fn left_and_right_modifiers_are_distinct() {
        let state = Arc::new(Mutex::new(TapState::default()));
        let (left, state_value) = modifier_transition(&state, 56, 0x0000_0002).unwrap();
        assert_eq!(left.as_str(), "left_shift");
        assert_eq!(state_value, KeyState::Down);
        let (right, state_value) = modifier_transition(&state, 60, 0x0000_0006).unwrap();
        assert_eq!(right.as_str(), "right_shift");
        assert_eq!(state_value, KeyState::Down);
    }
}
