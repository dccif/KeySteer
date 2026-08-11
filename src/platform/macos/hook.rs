//! Dedicated CGEventTap thread with per-event disposition handshakes.

use std::sync::atomic::{AtomicBool, Ordering};
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
pub const TIMEOUT_WARNING: &str =
    "keyboard disposition timed out; the key was forwarded and the event tap remained active";

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
    pub(super) fn send(&self, event: BackendEvent) -> Result<(), ()> {
        self.sender
            .send(Envelope {
                event,
                generation: None,
            })
            .map_err(|_| ())
    }
}

#[derive(Default)]
struct TapState {
    last_flags: u64,
    caps_lock_down: bool,
    needs_reenable: bool,
}

type SharedState = Arc<Mutex<TapState>>;
type SharedPointer = Arc<Mutex<Option<Point>>>;
type SharedClickTracker = Arc<Mutex<ClickTracker>>;

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
    join: Option<std::thread::JoinHandle<()>>,
}

pub struct HookStartup {
    sender: SyncSender<Envelope>,
    receiver: Option<Receiver<Envelope>>,
    mailbox: Arc<crate::platform::disposition_mailbox::DispositionMailbox>,
    latest_pointer: SharedPointer,
    stop: Arc<AtomicBool>,
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
        let latest_pointer = Arc::new(Mutex::new(None));
        let thread_pointer = Arc::clone(&latest_pointer);
        let join = std::thread::Builder::new()
            .name("keysteer-event-tap".into())
            .spawn({
                let event_tx = event_tx.clone();
                move || {
                    event_tap_thread(
                        event_tx,
                        thread_mailbox,
                        handshake,
                        thread_stop,
                        thread_pointer,
                        click_tracker,
                    )
                }
            })
            .map_err(|error| format!("cannot start macOS event tap thread: {error}"))?;

        Ok(Self {
            sender: event_tx,
            receiver: Some(event_rx),
            mailbox,
            latest_pointer,
            stop,
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
                    join: self.join.take(),
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
        let _ = self.activate.try_send(());
        if let Some(join) = self.join.take()
            && join.join().is_err()
        {
            crate::app::logging::report_error(
                "macos-hook",
                "event tap thread panicked during startup",
            );
        }
    }
}

impl HookThread {
    pub(super) fn event_sender(&self) -> EventSender {
        EventSender {
            sender: self.sender.clone(),
        }
    }

    pub fn next_event(&mut self, timeout: Duration) -> Option<BackendEvent> {
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
        if let Some(join) = self.join.take()
            && join.join().is_err()
        {
            crate::app::logging::report_error("macos-hook", "event tap thread panicked");
        }
        self.pending = None;
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        self.stop();
    }
}

fn event_tap_thread(
    sender: SyncSender<Envelope>,
    mailbox: Arc<crate::platform::disposition_mailbox::DispositionMailbox>,
    handshake: HookHandshake,
    stop: Arc<AtomicBool>,
    latest_pointer: SharedPointer,
    click_tracker: SharedClickTracker,
) {
    let HookHandshake {
        ready,
        activate,
        activated,
    } = handshake;
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
    run_loop.add_source(&source, unsafe { kCFRunLoopDefaultMode });
    if ready.send(Ok(())).is_err() {
        return;
    }
    if activate.recv().is_err() || stop.load(Ordering::Acquire) {
        return;
    }
    tap.enable();
    if activated.send(()).is_err() {
        return;
    }

    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, RUN_LOOP_SLICE, true);
        let needs_reenable = state
            .lock()
            .map(|mut state| std::mem::take(&mut state.needs_reenable))
            .unwrap_or(false);
        if needs_reenable {
            tap.enable();
            let _ = sender.try_send(Envelope {
                event: BackendEvent::Warning(
                    "the macOS event tap was disabled by the system and re-enabled".into(),
                ),
                generation: None,
            });
        }
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
            state.needs_reenable = true;
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
            join: None,
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
            join: None,
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
            join: None,
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
            join: None,
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
