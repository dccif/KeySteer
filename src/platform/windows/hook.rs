//! Dedicated low-level keyboard hook with per-event disposition handshakes.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_UP, MSG, MSLLHOOKSTRUCT,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_MOUSEMOVE, WM_QUIT,
};

use crate::api::backend::{BackendEvent, KeyDisposition};
use crate::api::command::{ButtonAction, MouseButton};
use crate::api::geometry::Point;
use crate::api::input::{InputEvent, Key, KeyState};

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
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
static WAKE_FAILED: AtomicBool = AtomicBool::new(false);
static POINTER_PENDING: AtomicBool = AtomicBool::new(false);
static POINTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CONSUMED_POINTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LATEST_POINTER: AtomicU64 = AtomicU64::new(0);
const WAKE_MESSAGE: u32 = WM_APP + 0x4D;
const RESET_PRESSED_MESSAGE: u32 = WM_APP + 0x4F;
const INJECTION_MESSAGE: u32 = WM_APP + 0x50;
const MENU_MASK_MESSAGE: u32 = WM_APP + 0x51;

/// Native input work executed only from the hook thread's message loop. A
/// posted request cannot run until the physical-key callback ahead of it has
/// returned, closing the barrier-to-SendInput race on the engine thread.
pub(super) enum InjectionRequest {
    MouseButton {
        button: MouseButton,
        action: ButtonAction,
    },
    Scroll {
        dx: f64,
        dy: f64,
    },
    Key {
        key: Key,
        state: KeyState,
    },
    Keys(Vec<(Key, KeyState)>),
}

impl InjectionRequest {
    fn execute(self) -> Result<(), String> {
        match self {
            Self::MouseButton { button, action } => {
                input::mouse_button(button, action).map_err(|error| {
                    format!("request=mouse button={button:?} action={action:?}; {error}")
                })
            }
            Self::Scroll { dx, dy } => input::scroll(dx, dy)
                .map_err(|error| format!("request=scroll dx={dx:.3} dy={dy:.3}; {error}")),
            Self::Key { key, state } => input::send_key(&key, state)
                .map_err(|error| format!("request=key key={key} state={state:?}; {error}")),
            Self::Keys(events) => {
                let event_count = events.len();
                input::send_keys(&events).map_err(|error| {
                    format!("request=keyboard_chord events={event_count}; {error}")
                })
            }
        }
    }
}

struct PendingInjection {
    generation: u32,
    request: InjectionRequest,
}

#[derive(Debug)]
enum InjectionFailure {
    Wait(String),
    Execute(String),
}

impl InjectionFailure {
    fn stage(&self) -> &'static str {
        match self {
            Self::Wait(_) => "wait",
            Self::Execute(_) => "execute",
        }
    }

    fn into_message(self) -> String {
        match self {
            Self::Wait(message) | Self::Execute(message) => message,
        }
    }
}

#[derive(Default)]
struct InjectionState {
    pending: Option<PendingInjection>,
    running: Option<u32>,
    completed: Option<(u32, Result<(), InjectionFailure>)>,
}

/// One reusable request slot keeps injection ordered without an unbounded
/// queue. The engine submits at most one synchronous backend operation at a
/// time, so additional allocations are needed only for an existing key chord.
#[derive(Default)]
struct InjectionSlot {
    next_generation: AtomicU32,
    state: Mutex<InjectionState>,
    ready: Condvar,
}

impl InjectionSlot {
    fn begin(&self, request: InjectionRequest) -> Result<u32, String> {
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.pending.is_some() || state.running.is_some() || state.completed.is_some() {
            return Err("another native input request is still active".into());
        }
        state.pending = Some(PendingInjection {
            generation,
            request,
        });
        Ok(generation)
    }

    fn cancel_pending(&self, generation: u32) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            state.pending = None;
        }
    }

    fn take_pending(&self, generation: u32) -> Option<InjectionRequest> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let pending = state.pending.take()?;
        if pending.generation != generation {
            state.pending = Some(pending);
            return None;
        }
        state.running = Some(generation);
        Some(pending.request)
    }

    fn complete(&self, generation: u32, result: Result<(), String>) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.running == Some(generation) {
            state.running = None;
            state.completed = Some((generation, result.map_err(InjectionFailure::Execute)));
            self.ready.notify_all();
        }
    }

    fn fail_pending(&self, message: &str) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(pending) = state.pending.take() {
            state.completed = Some((
                pending.generation,
                Err(InjectionFailure::Wait(message.into())),
            ));
            self.ready.notify_all();
        }
    }

    fn take_completed(
        state: &mut InjectionState,
        generation: u32,
    ) -> Option<Result<(), InjectionFailure>> {
        if state
            .completed
            .as_ref()
            .is_some_and(|(completed, _)| *completed == generation)
        {
            return state.completed.take().map(|(_, result)| result);
        }
        None
    }

    fn wait(&self, generation: u32, pending_timeout: Duration) -> Result<(), InjectionFailure> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let (mut state, _) = self
            .ready
            .wait_timeout_while(state, pending_timeout, |state| {
                state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.generation == generation)
                    && state
                        .completed
                        .as_ref()
                        .is_none_or(|(completed, _)| *completed != generation)
            })
            .unwrap_or_else(|error| error.into_inner());
        if let Some(result) = Self::take_completed(&mut state, generation) {
            return result;
        }
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            state.pending = None;
            return Err(InjectionFailure::Wait(
                "input-hook message queue did not start the native input request".into(),
            ));
        }
        if state.running == Some(generation) {
            // Once SendInput is running it cannot be cancelled. Waiting here
            // has exactly the same lifetime as the previous direct call and
            // prevents a delayed click after the engine reports failure.
            state = self
                .ready
                .wait_while(state, |state| {
                    state
                        .completed
                        .as_ref()
                        .is_none_or(|(completed, _)| *completed != generation)
                })
                .unwrap_or_else(|error| error.into_inner());
            return Self::take_completed(&mut state, generation).unwrap_or_else(|| {
                Err(InjectionFailure::Wait(
                    "native input request lost its completion".into(),
                ))
            });
        }
        Err(InjectionFailure::Wait(
            "native input request disappeared before execution".into(),
        ))
    }
}

const VK_LMENU_VALUE: u16 = 0xA4;
const VK_RMENU_VALUE: u16 = 0xA5;

/// Which physical Alt keys have already been forwarded to Windows. Keeping
/// this as two bits avoids allocation and lets physical Alt remain visible to
/// the foreground application, mouse shortcuts, AHK and Quicker.
#[derive(Clone, Copy, Default)]
struct ForwardedAlt(u8);

impl ForwardedAlt {
    fn observe(
        &mut self,
        virtual_key: u16,
        state: KeyState,
        repeat: bool,
        is_modifier: bool,
        disposition: KeyDisposition,
    ) -> bool {
        let disposition = match disposition {
            KeyDisposition::Defer => KeyDisposition::Forward,
            other => other,
        };
        let alt_mask = match virtual_key {
            VK_LMENU_VALUE => 1,
            VK_RMENU_VALUE => 2,
            _ => 0,
        };
        if alt_mask != 0 {
            match (state, disposition) {
                (KeyState::Down, KeyDisposition::Forward) => self.0 |= alt_mask,
                (KeyState::Up, _) => self.0 &= !alt_mask,
                _ => {}
            }
            return false;
        }

        state == KeyState::Down
            && !repeat
            && !is_modifier
            && disposition == KeyDisposition::Consume
            && self.0 != 0
    }
}

thread_local! {
    /// Virtual keys are one byte. A fixed bitset avoids allocating a tree node
    /// for every held key inside the latency-sensitive hook callback.
    static PRESSED: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
    /// Physical Alt state that has already reached the rest of Windows.
    static FORWARDED_ALT: Cell<ForwardedAlt> = const { Cell::new(ForwardedAlt(0)) };
}

pub struct HookThread {
    receiver: Receiver<Envelope>,
    mailbox: Arc<DispositionMailbox>,
    injection: Arc<InjectionSlot>,
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
        let injection = Arc::new(InjectionSlot::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_mailbox = Arc::clone(&mailbox);
        let thread_injection = Arc::clone(&injection);
        let join = std::thread::Builder::new()
            .name("keysteer-keyboard-hook".into())
            .spawn(move || {
                hook_thread(
                    event_tx,
                    thread_mailbox,
                    thread_injection,
                    ready_tx,
                    owner_thread,
                )
            })
            .map_err(|e| format!("cannot start keyboard hook thread: {e}"))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| "keyboard hook thread stopped before reporting readiness".to_string())??;
        Ok(Self {
            receiver: event_rx,
            mailbox,
            injection,
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

    /// Queue SendInput work behind the callback currently deciding a physical
    /// key. Running the native call from that message closes the race in which
    /// another callback could start after an idle acknowledgement.
    pub fn inject(&self, request: InjectionRequest) -> Result<(), String> {
        if self.thread_id == 0 {
            return Err("keyboard hook stopped before synthetic input could be submitted".into());
        }
        let generation = self.injection.begin(request).map_err(|error| {
            format!(
                "{error}; injection={{route=hook_message, stage=reserve, native_thread={}, version={}}}",
                self.thread_id,
                env!("CARGO_PKG_VERSION")
            )
        })?;
        if let Err(error) = super::native::post_thread_message(
            self.thread_id,
            INJECTION_MESSAGE,
            generation as usize,
        ) {
            self.injection.cancel_pending(generation);
            return Err(format!(
                "cannot queue native input on the hook thread: {error}; injection={{route=hook_message, stage=post, generation={generation}, native_thread={}, version={}}}",
                self.thread_id,
                env!("CARGO_PKG_VERSION")
            ));
        }
        self.injection
            .wait(generation, Duration::from_millis(100))
            .map_err(|failure| {
                let stage = failure.stage();
                let error = failure.into_message();
                format!(
                    "{error}; injection={{route=hook_message, stage={stage}, generation={generation}, native_thread={}, version={}}}",
                    self.thread_id,
                    env!("CARGO_PKG_VERSION")
                )
            })
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
    injection: Arc<InjectionSlot>,
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
    HOOK_THREAD.store(thread_id, Ordering::Release);
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
        HOOK_THREAD.store(0, Ordering::Release);
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
            FORWARDED_ALT.with(|alt| alt.set(ForwardedAlt::default()));
            continue;
        }
        if message.message == INJECTION_MESSAGE {
            let generation = message.wParam.0 as u32;
            if let Some(request) = injection.take_pending(generation) {
                let result = request.execute();
                injection.complete(generation, result);
            }
            continue;
        }
        if message.message == MENU_MASK_MESSAGE {
            if let Err(error) = input::send_menu_mask() {
                let _ = send_envelope(Envelope {
                    event: BackendEvent::Warning(format!(
                        "could not mask the Windows Alt menu after a consumed shortcut: {error}"
                    )),
                    generation: None,
                });
            }
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
    injection.fail_pending("input hook stopped before the native input request ran");
    WAKE_THREAD.store(0, Ordering::Release);
    HOOK_THREAD.store(0, Ordering::Release);
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
    let disposition = if let Some((mailbox, generation)) = begin_disposition(event) {
        match mailbox.wait(generation, Duration::from_millis(100)) {
            Some(disposition) => disposition,
            None => {
                let _ = send_envelope(Envelope {
                    event: BackendEvent::Warning(TIMEOUT_WARNING.into()),
                    generation: None,
                });
                KeyDisposition::Forward
            }
        }
    } else {
        KeyDisposition::Forward
    };
    let virtual_key = info.vkCode as u16;
    let mask_menu = FORWARDED_ALT.with(|alt| {
        let mut alt_state = alt.get();
        let mask = alt_state.observe(virtual_key, state, repeat, is_modifier, disposition);
        alt.set(alt_state);
        mask
    });
    if mask_menu
        && let Err(error) =
            super::native::post_thread_wake(HOOK_THREAD.load(Ordering::Acquire), MENU_MASK_MESSAGE)
    {
        let _ = send_envelope(Envelope {
            event: BackendEvent::Warning(format!(
                "could not queue the Windows Alt menu mask after a consumed shortcut: {error}"
            )),
            generation: None,
        });
    }
    match disposition {
        KeyDisposition::Consume => return LRESULT(1),
        KeyDisposition::Defer | KeyDisposition::Forward => {}
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

    #[test]
    fn menu_mask_is_needed_only_for_consumed_non_modifier_down_with_forwarded_alt() {
        let mut alt = ForwardedAlt::default();
        assert!(!alt.observe(
            VK_LMENU_VALUE,
            KeyState::Down,
            false,
            true,
            KeyDisposition::Forward
        ));
        assert!(!alt.observe(0x48, KeyState::Down, false, false, KeyDisposition::Forward));
        assert!(!alt.observe(0x45, KeyState::Down, true, false, KeyDisposition::Consume));
        assert!(alt.observe(0x45, KeyState::Down, false, false, KeyDisposition::Consume));
        assert!(!alt.observe(0x45, KeyState::Up, false, false, KeyDisposition::Consume));
        assert!(!alt.observe(
            VK_LMENU_VALUE,
            KeyState::Up,
            false,
            true,
            KeyDisposition::Forward
        ));
        assert!(!alt.observe(0x45, KeyState::Down, false, false, KeyDisposition::Consume));
    }

    #[test]
    fn deferred_compatibility_disposition_is_treated_as_forwarded_alt() {
        let mut alt = ForwardedAlt::default();
        assert!(!alt.observe(
            VK_RMENU_VALUE,
            KeyState::Down,
            false,
            true,
            KeyDisposition::Defer
        ));
        assert!(alt.observe(0x45, KeyState::Down, false, false, KeyDisposition::Consume));
    }

    #[test]
    fn disposition_is_delivered_to_the_exact_waiting_event() {
        let (_event_tx, event_rx) = mpsc::channel();
        let mailbox = Arc::new(DispositionMailbox::default());
        let generation = mailbox.begin();
        let mut hook = HookThread {
            receiver: event_rx,
            mailbox: Arc::clone(&mailbox),
            injection: Arc::new(InjectionSlot::default()),
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
            injection: Arc::new(InjectionSlot::default()),
            pending: Some(generation),
            thread_id: 0,
            join: None,
        };

        hook.set_disposition(KeyDisposition::Forward).unwrap();
        assert!(hook.pending.is_none());
    }

    #[test]
    fn injection_slot_executes_and_completes_only_its_generation() {
        let slot = InjectionSlot::default();
        let request = || InjectionRequest::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Click,
        };

        let first = slot.begin(request()).unwrap();
        assert!(slot.take_pending(first.wrapping_add(1)).is_none());
        assert!(slot.take_pending(first).is_some());
        slot.complete(first, Ok(()));
        assert!(slot.wait(first, Duration::ZERO).is_ok());

        let second = slot.begin(request()).unwrap();
        assert!(slot.wait(second, Duration::ZERO).is_err());
        assert!(slot.take_pending(second).is_none());

        let third = slot.begin(request()).unwrap();
        slot.fail_pending("hook stopped");
        let Err(InjectionFailure::Wait(message)) = slot.wait(third, Duration::ZERO) else {
            panic!("stopped hook must report a wait-stage failure");
        };
        assert_eq!(message, "hook stopped");

        let fourth = slot.begin(request()).unwrap();
        assert!(slot.take_pending(fourth).is_some());
        slot.complete(fourth, Err("send failed".into()));
        let Err(InjectionFailure::Execute(message)) = slot.wait(fourth, Duration::ZERO) else {
            panic!("native failure must retain the execute stage");
        };
        assert_eq!(message, "send failed");
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
