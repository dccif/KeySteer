//! Dedicated low-level keyboard hook with per-event disposition handshakes.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_UP, MSG, MSLLHOOKSTRUCT,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_MOUSEMOVE, WM_QUIT,
};

use crate::api::backend::{BackendEvent, KeyDisposition};
use crate::api::geometry::Point;
use crate::api::input::{InputEvent, KeyState};

use super::input;
use crate::platform::disposition_mailbox::DispositionMailbox;

pub const TIMEOUT_WARNING: &str =
    "keyboard disposition timed out; the key was forwarded and the hook remained active";

struct Envelope {
    event: BackendEvent,
    generation: Option<u64>,
}

struct EventSink {
    sender: SyncSender<Envelope>,
    mailbox: Arc<DispositionMailbox>,
}

static EVENT_SENDER: OnceLock<Mutex<Option<EventSink>>> = OnceLock::new();
static WAKE_THREAD: AtomicU32 = AtomicU32::new(0);
static WAKE_FAILED: AtomicBool = AtomicBool::new(false);
static POINTER_PENDING: AtomicBool = AtomicBool::new(false);
static POINTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CONSUMED_POINTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LATEST_POINTER: AtomicU64 = AtomicU64::new(0);
const WAKE_MESSAGE: u32 = WM_APP + 0x4D;
const RESET_PRESSED_MESSAGE: u32 = WM_APP + 0x4F;

const MAX_DEFERRED_MODIFIERS: usize = 4;
const MAX_REPLAY_EVENTS: usize = MAX_DEFERRED_MODIFIERS + 1;

#[derive(Clone, Copy)]
struct ReplayBatch {
    events: [(u16, KeyState); MAX_REPLAY_EVENTS],
    len: usize,
}

impl ReplayBatch {
    fn empty() -> Self {
        Self {
            events: [(0, KeyState::Down); MAX_REPLAY_EVENTS],
            len: 0,
        }
    }

    fn push(&mut self, virtual_key: u16, state: KeyState) {
        if self.len < self.events.len() {
            self.events[self.len] = (virtual_key, state);
            self.len += 1;
        }
    }

    fn as_slice(&self) -> &[(u16, KeyState)] {
        &self.events[..self.len]
    }
}

enum DeferredDecision {
    Forward,
    Suppress,
    Replay(ReplayBatch),
}

#[derive(Default)]
struct DeferredModifiers {
    virtual_keys: [u16; MAX_DEFERRED_MODIFIERS],
    len: usize,
    chord_claimed: bool,
}

impl DeferredModifiers {
    fn clear(&mut self) {
        *self = Self::default();
    }

    fn contains(&self, virtual_key: u16) -> bool {
        self.virtual_keys[..self.len].contains(&virtual_key)
    }

    fn defer(&mut self, virtual_key: u16) {
        if !self.contains(virtual_key) && self.len < self.virtual_keys.len() {
            self.virtual_keys[self.len] = virtual_key;
            self.len += 1;
        }
    }

    fn remove(&mut self, virtual_key: u16) -> bool {
        let Some(index) = self.virtual_keys[..self.len]
            .iter()
            .position(|candidate| *candidate == virtual_key)
        else {
            return false;
        };
        self.len -= 1;
        self.virtual_keys[index] = self.virtual_keys[self.len];
        if self.len == 0 {
            self.chord_claimed = false;
        }
        true
    }

    fn decide(
        &mut self,
        virtual_key: u16,
        state: KeyState,
        is_modifier: bool,
        disposition: KeyDisposition,
    ) -> DeferredDecision {
        match disposition {
            KeyDisposition::Defer => match state {
                KeyState::Down => {
                    self.defer(virtual_key);
                    DeferredDecision::Suppress
                }
                KeyState::Up if self.contains(virtual_key) => {
                    let claimed = self.chord_claimed;
                    self.remove(virtual_key);
                    if claimed {
                        DeferredDecision::Suppress
                    } else {
                        let mut replay = ReplayBatch::empty();
                        replay.push(virtual_key, KeyState::Down);
                        replay.push(virtual_key, KeyState::Up);
                        DeferredDecision::Replay(replay)
                    }
                }
                KeyState::Up => DeferredDecision::Forward,
            },
            KeyDisposition::Consume => {
                if !is_modifier && self.len > 0 {
                    self.chord_claimed = true;
                }
                DeferredDecision::Suppress
            }
            KeyDisposition::Forward if self.len > 0 && !self.chord_claimed => {
                let mut replay = ReplayBatch::empty();
                for deferred in &self.virtual_keys[..self.len] {
                    replay.push(*deferred, KeyState::Down);
                }
                replay.push(virtual_key, state);
                self.clear();
                DeferredDecision::Replay(replay)
            }
            KeyDisposition::Forward => DeferredDecision::Forward,
        }
    }
}

thread_local! {
    /// Virtual keys are one byte. A fixed bitset avoids allocating a tree node
    /// for every held key inside the latency-sensitive hook callback.
    static PRESSED: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
    /// Alt prefixes hidden until the engine decides whether a chord matched.
    static DEFERRED_MODIFIERS: RefCell<DeferredModifiers> = RefCell::new(DeferredModifiers::default());
}

pub struct HookThread {
    receiver: Receiver<Envelope>,
    mailbox: Arc<DispositionMailbox>,
    pending: Option<u64>,
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl HookThread {
    pub fn start() -> Result<Self, String> {
        // Build canonical shared Key values before the first physical key edge
        // reaches the latency-sensitive hook callback.
        input::prewarm_key_map();
        // Ensure the engine thread has a message queue before hook callbacks
        // try to wake it with PostThreadMessageW.
        let owner_thread = unsafe { GetCurrentThreadId() };
        let mut queue_probe = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut queue_probe, None, 0, 0, PM_NOREMOVE);
        }
        // The hook is synchronous, so at most a small bounded burst can be
        // outstanding. Preallocate the queue instead of growing an unbounded
        // linked channel on physical key edges.
        let (event_tx, event_rx) = mpsc::sync_channel(32);
        let mailbox = Arc::new(DispositionMailbox::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_mailbox = Arc::clone(&mailbox);
        let join = std::thread::Builder::new()
            .name("keysteer-keyboard-hook".into())
            .spawn(move || hook_thread(event_tx, thread_mailbox, ready_tx, owner_thread))
            .map_err(|e| format!("cannot start keyboard hook thread: {e}"))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| "keyboard hook thread stopped before reporting readiness".to_string())??;
        Ok(Self {
            receiver: event_rx,
            mailbox,
            pending: None,
            thread_id,
            join: Some(join),
        })
    }

    pub fn next_event(&mut self) -> Option<BackendEvent> {
        if WAKE_FAILED.swap(false, Ordering::AcqRel) {
            return Some(BackendEvent::Warning(
                "PostThreadMessageW could not wake the engine for hooked input".into(),
            ));
        }
        if let Ok(envelope) = self.receiver.try_recv() {
            self.pending = envelope.generation;
            return Some(envelope.event);
        }
        take_latest_pointer().map(BackendEvent::PointerMoved)
    }

    pub fn set_disposition(&mut self, disposition: KeyDisposition) -> Result<(), String> {
        let generation = self
            .pending
            .take()
            .ok_or_else(|| "no keyboard event is awaiting a disposition".to_string())?;
        // A timed-out callback has already failed open. Generation matching
        // makes its late response harmless if a newer event owns the slot.
        let _ = self.mailbox.complete(generation, disposition);
        Ok(())
    }

    pub fn stop(&mut self) {
        let mut stop_posted = true;
        if self.thread_id != 0 {
            if let Err(error) =
                unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            {
                stop_posted = false;
                crate::app::logging::report_error(
                    "windows-hook",
                    format!("cannot request input hook shutdown: {error}"),
                );
            }
            self.thread_id = 0;
        }
        if let Some(join) = self.join.take() {
            if stop_posted {
                if join.join().is_err() {
                    crate::app::logging::report_error("windows-hook", "input hook thread panicked");
                }
            } else {
                // Waiting here could block forever because the quit message
                // was not delivered. Detaching is the only safe fallback.
                drop(join);
            }
        }
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        self.stop();
    }
}

fn hook_thread(
    sender: SyncSender<Envelope>,
    mailbox: Arc<DispositionMailbox>,
    ready: SyncSender<Result<u32, String>>,
    wake_thread: u32,
) {
    let slot = EVENT_SENDER.get_or_init(|| Mutex::new(None));
    let keyboard_hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) }
    {
        Ok(hook) => hook,
        Err(error) => {
            let _ = ready.send(Err(format!("SetWindowsHookExW failed: {error}")));
            return;
        }
    };
    let mouse_hook = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) }
    {
        Ok(hook) => hook,
        Err(error) => {
            if let Err(unhook_error) = unsafe { UnhookWindowsHookEx(keyboard_hook) } {
                crate::log_warning!(
                    "windows-hook",
                    "cannot remove keyboard hook after mouse-hook failure: {unhook_error}"
                );
            }
            let _ = ready.send(Err(format!("SetWindowsHookExW(mouse) failed: {error}")));
            return;
        }
    };
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut queue_probe = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut queue_probe, None, 0, 0, PM_NOREMOVE);
    }
    if let Ok(mut current) = slot.lock() {
        *current = Some(EventSink { sender, mailbox });
    }
    WAKE_THREAD.store(wake_thread, Ordering::Release);
    POINTER_PENDING.store(false, Ordering::Release);
    if ready.send(Ok(thread_id)).is_err() {
        if let Err(error) = unsafe { UnhookWindowsHookEx(mouse_hook) } {
            crate::log_warning!(
                "windows-hook",
                "cannot remove abandoned mouse hook: {error}"
            );
        }
        if let Err(error) = unsafe { UnhookWindowsHookEx(keyboard_hook) } {
            crate::log_warning!(
                "windows-hook",
                "cannot remove abandoned keyboard hook: {error}"
            );
        }
        if let Ok(mut current) = slot.lock() {
            *current = None;
        }
        WAKE_THREAD.store(0, Ordering::Release);
        return;
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if status == 0 {
            break;
        }
        if status == -1 {
            WAKE_FAILED.store(true, Ordering::Release);
            break;
        }
        if message.message == RESET_PRESSED_MESSAGE {
            PRESSED.with(|pressed| pressed.set([0; 4]));
            DEFERRED_MODIFIERS.with(|deferred| deferred.borrow_mut().clear());
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    if let Err(error) = unsafe { UnhookWindowsHookEx(mouse_hook) } {
        crate::log_warning!("windows-hook", "cannot remove mouse hook: {error}");
    }
    if let Err(error) = unsafe { UnhookWindowsHookEx(keyboard_hook) } {
        crate::log_warning!("windows-hook", "cannot remove keyboard hook: {error}");
    }
    if let Ok(mut current) = slot.lock() {
        *current = None;
    }
    WAKE_THREAD.store(0, Ordering::Release);
    POINTER_PENDING.store(false, Ordering::Release);
}

fn send_envelope(envelope: Envelope) -> bool {
    let Some(slot) = EVENT_SENDER.get() else {
        return false;
    };
    let delivered = {
        let Ok(sender) = slot.lock() else {
            return false;
        };
        let Some(sender) = sender.as_ref() else {
            return false;
        };
        sender.sender.send(envelope).is_ok()
    };
    if !delivered {
        return false;
    }
    wake_owner();
    true
}

fn begin_disposition(event: BackendEvent) -> Option<(Arc<DispositionMailbox>, u64)> {
    let slot = EVENT_SENDER.get()?;
    let (sender, mailbox, generation) = {
        let sink = slot.lock().ok()?;
        let sink = sink.as_ref()?;
        let generation = sink.mailbox.begin();
        (sink.sender.clone(), Arc::clone(&sink.mailbox), generation)
    };
    sender
        .send(Envelope {
            event,
            generation: Some(generation),
        })
        .ok()?;
    wake_owner();
    Some((mailbox, generation))
}

fn wake_owner() {
    let wake_thread = WAKE_THREAD.load(Ordering::Acquire);
    if wake_thread == 0 {
        return;
    }
    unsafe {
        if PostThreadMessageW(wake_thread, WAKE_MESSAGE, WPARAM(0), LPARAM(0)).is_err() {
            WAKE_FAILED.store(true, Ordering::Release);
        }
    }
}

fn store_latest_pointer(point: Point) {
    let x = point.x as i32 as u32 as u64;
    let y = point.y as i32 as u32 as u64;
    POINTER_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    LATEST_POINTER.store(x | (y << 32), Ordering::SeqCst);
    POINTER_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    if mark_pointer_pending() {
        wake_owner();
    }
}

fn mark_pointer_pending() -> bool {
    POINTER_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn take_latest_pointer() -> Option<Point> {
    if !POINTER_PENDING.swap(false, Ordering::AcqRel) {
        return None;
    }
    let (sequence, packed) = loop {
        let before = POINTER_SEQUENCE.load(Ordering::SeqCst);
        if before & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        let packed = LATEST_POINTER.load(Ordering::SeqCst);
        let after = POINTER_SEQUENCE.load(Ordering::SeqCst);
        if before == after {
            break (after, packed);
        }
    };
    if CONSUMED_POINTER_SEQUENCE.swap(sequence, Ordering::SeqCst) == sequence {
        return None;
    }
    Some(Point::new(
        (packed as u32 as i32) as f64,
        ((packed >> 32) as u32 as i32) as f64,
    ))
}

fn is_our_input(extra_info: usize) -> bool {
    extra_info == input::INJECTED_TAG
}

fn update_pressed_state(virtual_key: u32, state: KeyState) -> bool {
    PRESSED.with(|pressed| {
        let mut bits = pressed.get();
        let word = virtual_key as usize / 64;
        if word >= bits.len() {
            return false;
        }
        let mask = 1u64 << (virtual_key as usize % 64);
        match state {
            KeyState::Down => {
                let repeat = bits[word] & mask != 0;
                bits[word] |= mask;
                pressed.set(bits);
                repeat
            }
            KeyState::Up => {
                bits[word] &= !mask;
                pressed.set(bits);
                false
            }
        }
    })
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // Ignore only events tagged by this process. Keyboard remappers,
    // accessibility tools and software keyboards also set the Windows
    // injected flag; those inputs are user-owned and must remain bindable.
    if is_our_input(info.dwExtraInfo) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let Some(key) = input::key_for_virtual_key(info.vkCode) else {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    };
    let is_modifier = key.is_modifier();
    let state = if info.flags.0 & LLKHF_UP.0 != 0 {
        KeyState::Up
    } else {
        KeyState::Down
    };
    let repeat = update_pressed_state(info.vkCode, state);
    let event = BackendEvent::Input(InputEvent {
        key,
        state,
        repeat,
        injected: false,
        timestamp_millis: info.time as u64,
    });
    if let Some((mailbox, generation)) = begin_disposition(event) {
        let disposition = match mailbox.wait(generation, Duration::from_millis(100)) {
            Some(disposition) => disposition,
            None => {
                let _ = send_envelope(Envelope {
                    event: BackendEvent::Warning(TIMEOUT_WARNING.into()),
                    generation: None,
                });
                KeyDisposition::Forward
            }
        };
        let virtual_key = info.vkCode as u16;
        let decision = DEFERRED_MODIFIERS.with(|deferred| {
            deferred
                .borrow_mut()
                .decide(virtual_key, state, is_modifier, disposition)
        });
        match decision {
            DeferredDecision::Forward => {}
            DeferredDecision::Suppress => return LRESULT(1),
            DeferredDecision::Replay(batch) => {
                if input::send_virtual_keys(batch.as_slice()).is_ok() {
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEMOVE {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        store_latest_pointer(Point::new(info.pt.x as f64, info.pt.y as f64));
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay_events(decision: DeferredDecision) -> Vec<(u16, KeyState)> {
        match decision {
            DeferredDecision::Replay(batch) => batch.as_slice().to_vec(),
            DeferredDecision::Forward | DeferredDecision::Suppress => Vec::new(),
        }
    }

    #[test]
    fn matched_alt_chord_never_reaches_the_foreground_app() {
        let mut deferred = DeferredModifiers::default();
        assert!(matches!(
            deferred.decide(0xA4, KeyState::Down, true, KeyDisposition::Defer),
            DeferredDecision::Suppress
        ));
        assert!(matches!(
            deferred.decide(0x45, KeyState::Down, false, KeyDisposition::Consume),
            DeferredDecision::Suppress
        ));
        assert!(matches!(
            deferred.decide(0x45, KeyState::Up, false, KeyDisposition::Consume),
            DeferredDecision::Suppress
        ));
        assert!(matches!(
            deferred.decide(0xA4, KeyState::Up, true, KeyDisposition::Defer),
            DeferredDecision::Suppress
        ));
        assert_eq!(deferred.len, 0);
    }

    #[test]
    fn unmatched_alt_chord_is_replayed_in_original_order() {
        let mut deferred = DeferredModifiers::default();
        let _ = deferred.decide(0xA4, KeyState::Down, true, KeyDisposition::Defer);
        let replay =
            replay_events(deferred.decide(0x58, KeyState::Down, false, KeyDisposition::Forward));
        assert_eq!(replay, [(0xA4, KeyState::Down), (0x58, KeyState::Down)]);
        assert_eq!(deferred.len, 0);
        assert!(matches!(
            deferred.decide(0xA4, KeyState::Up, true, KeyDisposition::Defer),
            DeferredDecision::Forward
        ));
    }

    #[test]
    fn tapping_alt_without_a_chord_replays_a_complete_tap() {
        let mut deferred = DeferredModifiers::default();
        let _ = deferred.decide(0xA4, KeyState::Down, true, KeyDisposition::Defer);
        let replay =
            replay_events(deferred.decide(0xA4, KeyState::Up, true, KeyDisposition::Defer));
        assert_eq!(replay, [(0xA4, KeyState::Down), (0xA4, KeyState::Up)]);
    }

    #[test]
    fn disposition_is_delivered_to_the_exact_waiting_event() {
        let (_event_tx, event_rx) = mpsc::channel();
        let mailbox = Arc::new(DispositionMailbox::default());
        let generation = mailbox.begin();
        let mut hook = HookThread {
            receiver: event_rx,
            mailbox: Arc::clone(&mailbox),
            pending: Some(generation),
            thread_id: 0,
            join: None,
        };

        hook.set_disposition(KeyDisposition::Consume).unwrap();
        assert_eq!(
            mailbox.wait(generation, Duration::ZERO),
            Some(KeyDisposition::Consume)
        );
        assert!(hook.pending.is_none());
    }

    #[test]
    fn a_timed_out_disposition_is_nonfatal() {
        let (_event_tx, event_rx) = mpsc::channel();
        let mailbox = Arc::new(DispositionMailbox::default());
        let generation = mailbox.begin();
        let _newer = mailbox.begin();
        let mut hook = HookThread {
            receiver: event_rx,
            mailbox,
            pending: Some(generation),
            thread_id: 0,
            join: None,
        };

        hook.set_disposition(KeyDisposition::Forward).unwrap();
        assert!(hook.pending.is_none());
    }

    #[test]
    fn only_our_tag_is_ignored_not_other_injected_input() {
        assert!(is_our_input(input::INJECTED_TAG));
        assert!(!is_our_input(0));
        assert!(!is_our_input(0x1234_5678));
    }

    #[test]
    fn pressed_state_changes_only_on_physical_key_edges() {
        const VK_F: u32 = 0x46;
        PRESSED.with(|pressed| pressed.set([0; 4]));

        assert!(!update_pressed_state(VK_F, KeyState::Down));
        // Foreground changes and pause/resume do not touch this state, so the
        // next Down remains an auto-repeat until the matching Up arrives.
        assert!(update_pressed_state(VK_F, KeyState::Down));
        assert!(!update_pressed_state(VK_F, KeyState::Up));
        assert!(!update_pressed_state(VK_F, KeyState::Down));
        assert!(!update_pressed_state(VK_F, KeyState::Up));
    }

    #[test]
    fn pointer_burst_uses_one_latest_value_slot() {
        POINTER_PENDING.store(false, Ordering::Release);
        assert!(mark_pointer_pending());
        assert!(
            !mark_pointer_pending(),
            "an existing pending move must not wake twice"
        );

        POINTER_PENDING.store(false, Ordering::Release);
        store_latest_pointer(Point::new(-300.0, 100.0));
        store_latest_pointer(Point::new(1920.0, 1080.0));
        assert_eq!(take_latest_pointer(), Some(Point::new(1920.0, 1080.0)));
        assert_eq!(take_latest_pointer(), None);

        // A repeated edge position is still a real native event.
        store_latest_pointer(Point::new(1920.0, 1080.0));
        assert_eq!(take_latest_pointer(), Some(Point::new(1920.0, 1080.0)));
    }
}
