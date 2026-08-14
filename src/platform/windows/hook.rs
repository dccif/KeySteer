//! Dedicated low-level keyboard hook with per-event disposition handshakes.

use std::cell::Cell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_UP, MSG,
    MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_MOUSEMOVE,
    WM_QUIT,
};

use crate::api::backend::{BackendEvent, KeyDisposition};
use crate::api::command::{ButtonAction, MouseButton};
use crate::api::geometry::Point;
use crate::api::input::{InputEvent, Key, KeyState};
use crate::app::worker::WorkerJoin;

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
static TIMEOUT_WARNING_PENDING: AtomicBool = AtomicBool::new(false);
static POINTER_PENDING: AtomicU64 = AtomicU64::new(0);
static POINTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CONSUMED_POINTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LATEST_POINTER: AtomicU64 = AtomicU64::new(0);
const WAKE_MESSAGE: u32 = WM_APP + 0x4D;
const RESET_PRESSED_MESSAGE: u32 = WM_APP + 0x4F;
const INJECTION_MESSAGE: u32 = WM_APP + 0x50;
const MENU_MASK_MESSAGE: u32 = WM_APP + 0x51;
const HOOK_STOP_TIMEOUT: Duration = Duration::from_secs(2);

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
    Chord(input::KeyChordBatch),
}

impl InjectionRequest {
    fn execute(self) -> Result<(), String> {
        match self {
            Self::MouseButton { button, action } => match input::mouse_button(button, action) {
                Ok(()) => Ok(()),
                Err(error) if matches!(action, ButtonAction::Click | ButtonAction::DoubleClick) => {
                    match input::mouse_button(button, ButtonAction::Release) {
                        Ok(()) => Err(format!(
                            "request=mouse button={button:?} action={action:?}; {error}; a defensive {button:?} release completed"
                        )),
                        Err(release_error) => Err(format!(
                            "request=mouse button={button:?} action={action:?}; {error}; defensive {button:?} release also failed: {release_error}"
                        )),
                    }
                }
                Err(error) if action == ButtonAction::Release => {
                    match input::mouse_button(button, ButtonAction::Release) {
                        Ok(()) => Err(format!(
                            "request=mouse button={button:?} action={action:?}; {error}; a defensive {button:?} release completed"
                        )),
                        Err(release_error) => Err(format!(
                            "request=mouse button={button:?} action={action:?}; {error}; defensive {button:?} release also failed: {release_error}"
                        )),
                    }
                }
                Err(error) => Err(format!(
                    "request=mouse button={button:?} action={action:?}; {error}"
                )),
            },
            Self::Scroll { dx, dy } => input::scroll(dx, dy)
                .map_err(|error| format!("request=scroll dx={dx:.3} dy={dy:.3}; {error}")),
            Self::Key { key, state } => match input::send_key(&key, state) {
                Ok(()) => Ok(()),
                Err(error) if state == KeyState::Up => match input::send_key(&key, KeyState::Up) {
                    Ok(()) => Err(format!(
                        "request=key key={key} state={state:?}; {error}; a defensive key release completed"
                    )),
                    Err(release_error) => Err(format!(
                        "request=key key={key} state={state:?}; {error}; defensive key release also failed: {release_error}"
                    )),
                },
                Err(error) => Err(format!("request=key key={key} state={state:?}; {error}")),
            },
            Self::Keys(events) => {
                let event_count = events.len();
                match input::send_keys(&events) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        // A partially accepted chord may have left one of its
                        // synthetic Down edges active. Releasing every key
                        // that had a Down edge is safe even when Windows
                        // accepted none of them, and keeps late asynchronous
                        // failures from leaving a modifier latched.
                        let mut release_failure = None;
                        for (key, state) in events.iter().rev() {
                            if *state != KeyState::Down {
                                continue;
                            }
                            if let Err(cleanup_error) = input::send_key(key, KeyState::Up)
                                && release_failure.is_none()
                            {
                                release_failure = Some(cleanup_error);
                            }
                        }
                        match release_failure {
                            Some(cleanup_error) => Err(format!(
                                "request=keyboard_chord events={event_count}; {error}; defensive key release also failed: {cleanup_error}"
                            )),
                            None => Err(format!(
                                "request=keyboard_chord events={event_count}; {error}; defensive key releases completed"
                            )),
                        }
                    }
                }
            }
            Self::Chord(chord) => input::send_chord(&chord)
                .map_err(|error| format!("request=keyboard_chord; {error}")),
        }
    }
}

struct PendingInjection {
    generation: u32,
    request: InjectionRequest,
}

#[derive(Debug)]
enum InjectionSubmissionFailure {
    Full,
    Post(String),
}

impl InjectionSubmissionFailure {
    fn stage(&self) -> &'static str {
        match self {
            Self::Full => "reserve",
            Self::Post(_) => "post",
        }
    }
}

const MAX_PENDING_INJECTIONS: usize = 32;

struct InjectionQueueState {
    pending: VecDeque<PendingInjection>,
    wake_posted: bool,
}

impl Default for InjectionQueueState {
    fn default() -> Self {
        Self {
            pending: VecDeque::with_capacity(MAX_PENDING_INJECTIONS),
            wake_posted: false,
        }
    }
}

/// A small bounded queue breaks the hook/engine lock cycle without adding a
/// worker thread. Submission never waits for the hook: the hook drains the
/// queue after any physical callbacks that were already in its message queue.
/// Successful requests are silent; only an execution failure returns to the
/// engine as a backend event.
#[derive(Default)]
struct InjectionQueue {
    next_generation: AtomicU32,
    state: Mutex<InjectionQueueState>,
}

impl InjectionQueue {
    fn submit(
        &self,
        request: InjectionRequest,
        post_wake: impl FnOnce(u32) -> Result<(), String>,
    ) -> Result<u32, (u32, InjectionSubmissionFailure)> {
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.pending.len() >= MAX_PENDING_INJECTIONS {
            return Err((generation, InjectionSubmissionFailure::Full));
        }
        let needs_wake = !state.wake_posted;
        state.pending.push_back(PendingInjection {
            generation,
            request,
        });
        if needs_wake {
            if let Err(error) = post_wake(generation) {
                let removed = state.pending.pop_back();
                debug_assert!(removed.is_some_and(|pending| pending.generation == generation));
                return Err((generation, InjectionSubmissionFailure::Post(error)));
            }
            state.wake_posted = true;
        }
        Ok(generation)
    }

    fn take_next(&self) -> Option<PendingInjection> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let next = state.pending.pop_front();
        if next.is_none() {
            state.wake_posted = false;
        }
        next
    }

    fn abandon_pending(&self) -> usize {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let count = state.pending.len();
        state.pending.clear();
        state.wake_posted = false;
        count
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
    injection: Arc<InjectionQueue>,
    pending: Option<u64>,
    thread_id: u32,
    worker: Option<WorkerJoin>,
}

struct OwnedHook {
    raw: HHOOK,
    _thread: PhantomData<Rc<()>>,
}

impl OwnedHook {
    fn new(raw: HHOOK) -> Self {
        Self {
            raw,
            _thread: PhantomData,
        }
    }
}

impl Drop for OwnedHook {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful SetWindowsHookExW call,
        // this non-Send guard remains on the installing thread, and Drop is
        // the only unhook path.
        if let Err(error) = unsafe { UnhookWindowsHookEx(self.raw) } {
            crate::log_warning!("windows-hook", "cannot remove hook: {error}");
        }
    }
}

impl HookThread {
    pub fn start() -> Result<Self, String> {
        // Build canonical shared Key values before the first physical key edge
        // reaches the latency-sensitive hook callback.
        input::prewarm_key_map();
        // Ensure the engine thread has a message queue before hook callbacks
        // try to wake it with PostThreadMessageW.
        let owner_thread = super::native::current_thread_id();
        let mut queue_probe = MSG::default();
        // SAFETY: `queue_probe` is a valid out-parameter and PM_NOREMOVE only
        // initializes this current thread's message queue.
        unsafe {
            let _ = PeekMessageW(&mut queue_probe, None, 0, 0, PM_NOREMOVE);
        }
        // The hook is synchronous, so at most a small bounded burst can be
        // outstanding. Preallocate the queue instead of growing an unbounded
        // linked channel on physical key edges.
        let (event_tx, event_rx) = mpsc::sync_channel(32);
        let mailbox = Arc::new(DispositionMailbox::default());
        let injection = Arc::new(InjectionQueue::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread_mailbox = Arc::clone(&mailbox);
        let thread_injection = Arc::clone(&injection);
        let mut worker = WorkerJoin::spawn(
            "Windows input hook",
            std::thread::Builder::new().name("keysteer-keyboard-hook".into()),
            move || {
                hook_thread(
                    event_tx,
                    thread_mailbox,
                    thread_injection,
                    ready_tx,
                    owner_thread,
                )
            },
        )?;
        let thread_id = match worker.wait_ready(&ready_rx, HOOK_STOP_TIMEOUT) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                let _ = worker.join_timeout(HOOK_STOP_TIMEOUT);
                return Err(error);
            }
            Err(error) => {
                let _ = worker.join_timeout(HOOK_STOP_TIMEOUT);
                return Err(error);
            }
        };
        Ok(Self {
            receiver: event_rx,
            mailbox,
            injection,
            pending: None,
            thread_id,
            worker: Some(worker),
        })
    }

    pub fn next_event(&mut self) -> Option<BackendEvent> {
        if WAKE_FAILED.swap(false, Ordering::AcqRel) {
            return Some(BackendEvent::Warning(
                "PostThreadMessageW could not wake the engine for hooked input".into(),
            ));
        }
        if let Ok(envelope) = self.receiver.try_recv() {
            if matches!(
                &envelope.event,
                BackendEvent::Warning(message) if message == TIMEOUT_WARNING
            ) {
                TIMEOUT_WARNING_PENDING.store(false, Ordering::Release);
            }
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

    /// Queue SendInput work without waiting on the hook thread. A synchronous
    /// wait here can deadlock when the next physical callback is already
    /// waiting for the engine's disposition. Execution failures return later
    /// as `BackendEvent::InputInjectionFailed`; successful requests stay
    /// allocation- and log-free after this bounded enqueue.
    pub fn inject(&self, request: InjectionRequest) -> Result<(), String> {
        if self.thread_id == 0 {
            return Err("keyboard hook stopped before synthetic input could be submitted".into());
        }
        self.injection
            .submit(request, |generation| {
                super::native::post_thread_message(
                    self.thread_id,
                    INJECTION_MESSAGE,
                    generation as usize,
                )
                .map_err(|error| error.to_string())
            })
            .map(|_| ())
            .map_err(|(generation, failure)| {
                let stage = failure.stage();
                let error = match failure {
                    InjectionSubmissionFailure::Full => {
                        format!("native input queue reached its {MAX_PENDING_INJECTIONS}-request bound")
                    }
                    InjectionSubmissionFailure::Post(error) => {
                        format!("cannot wake the input hook for native input: {error}")
                    }
                };
                format!(
                    "{error}; injection={{route=hook_queue, stage={stage}, generation={generation}, native_thread={}, version={}}}",
                    self.thread_id,
                    env!("CARGO_PKG_VERSION")
                )
            })
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if self.worker.is_none() {
            return Ok(());
        }
        let mut stop_posted = true;
        if self.thread_id != 0 {
            if let Err(error) =
                // SAFETY: `thread_id` belongs to the live hook message loop and
                // WM_QUIT carries no pointer payload.
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
        let result = self
            .worker
            .as_mut()
            .map_or(Ok(()), |worker| worker.join_timeout(HOOK_STOP_TIMEOUT));
        if result.is_ok() {
            self.worker.take();
        }
        if !stop_posted && result.is_ok() {
            return Err("Windows rejected the input-hook shutdown wake-up".into());
        }
        result
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-hook", &error);
        }
    }
}

fn hook_thread(
    sender: SyncSender<Envelope>,
    mailbox: Arc<DispositionMailbox>,
    injection: Arc<InjectionQueue>,
    ready: SyncSender<Result<u32, String>>,
    wake_thread: u32,
) {
    let slot = EVENT_SENDER.get_or_init(|| Mutex::new(None));
    // SAFETY: the callback has the required ABI, is process-lifetime code, and
    // the resulting handle is immediately placed in its thread-bound guard.
    let keyboard_hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) }
    {
        Ok(hook) => OwnedHook::new(hook),
        Err(error) => {
            let _ = ready.send(Err(format!("SetWindowsHookExW failed: {error}")));
            return;
        }
    };
    // SAFETY: the callback has the required ABI, is process-lifetime code, and
    // the resulting handle is immediately placed in its thread-bound guard.
    let mouse_hook = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) }
    {
        Ok(hook) => OwnedHook::new(hook),
        Err(error) => {
            let _ = ready.send(Err(format!("SetWindowsHookExW(mouse) failed: {error}")));
            return;
        }
    };
    let thread_id = super::native::current_thread_id();
    HOOK_THREAD.store(thread_id, Ordering::Release);
    let mut queue_probe = MSG::default();
    // SAFETY: `queue_probe` is a valid out-parameter and PM_NOREMOVE only
    // ensures this current hook thread owns an initialized message queue.
    unsafe {
        let _ = PeekMessageW(&mut queue_probe, None, 0, 0, PM_NOREMOVE);
    }
    if let Ok(mut current) = slot.lock() {
        *current = Some(EventSink { sender, mailbox });
    }
    WAKE_THREAD.store(wake_thread, Ordering::Release);
    POINTER_PENDING.store(0, Ordering::Release);
    if ready.send(Ok(thread_id)).is_err() {
        if let Ok(mut current) = slot.lock() {
            *current = None;
        }
        WAKE_THREAD.store(0, Ordering::Release);
        HOOK_THREAD.store(0, Ordering::Release);
        return;
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is writable for the duration of the synchronous
        // call and this function owns the current thread's message loop.
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
            while let Some(pending) = injection.take_next() {
                if let Err(error) = pending.request.execute() {
                    let message = format!(
                        "{error}; injection={{route=hook_queue, stage=execute, generation={}, native_thread={thread_id}, version={}}}",
                        pending.generation,
                        env!("CARGO_PKG_VERSION")
                    );
                    if !send_envelope(Envelope {
                        event: BackendEvent::InputInjectionFailed(message.clone()),
                        generation: None,
                    }) {
                        crate::app::logging::report_error("windows-input", message);
                    }
                }
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
        // SAFETY: `message` was initialized by GetMessageW and is dispatched
        // synchronously on the same owning thread.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(mouse_hook);
    drop(keyboard_hook);
    let abandoned = injection.abandon_pending();
    if abandoned != 0 {
        let _ = send_envelope(Envelope {
            event: BackendEvent::InputInjectionFailed(format!(
                "input hook stopped with {abandoned} native input request(s) still queued; injection={{route=hook_queue, stage=shutdown, native_thread={thread_id}, version={}}}",
                env!("CARGO_PKG_VERSION")
            )),
            generation: None,
        });
    }
    if let Ok(mut current) = slot.lock() {
        *current = None;
    }
    WAKE_THREAD.store(0, Ordering::Release);
    HOOK_THREAD.store(0, Ordering::Release);
    POINTER_PENDING.store(0, Ordering::Release);
}

fn send_envelope(envelope: Envelope) -> bool {
    let Some(slot) = EVENT_SENDER.get() else {
        return false;
    };
    let delivered = {
        let Ok(sender) = slot.try_lock() else {
            return false;
        };
        let Some(sender) = sender.as_ref() else {
            return false;
        };
        try_deliver(&sender.sender, envelope)
    };
    if !delivered {
        return false;
    }
    wake_owner();
    true
}

fn begin_disposition(event: BackendEvent) -> Option<(Arc<DispositionMailbox>, u64)> {
    let slot = EVENT_SENDER.get()?;
    let (mailbox, generation) = {
        let sink = slot.try_lock().ok()?;
        let sink = sink.as_ref()?;
        let generation = sink.mailbox.begin();
        try_deliver(
            &sink.sender,
            Envelope {
                event,
                generation: Some(generation),
            },
        )
        .then(|| (Arc::clone(&sink.mailbox), generation))?
    };
    wake_owner();
    Some((mailbox, generation))
}

#[inline(always)]
fn try_deliver(sender: &SyncSender<Envelope>, envelope: Envelope) -> bool {
    sender.try_send(envelope).is_ok()
}

fn queue_timeout_warning() {
    if TIMEOUT_WARNING_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    if !send_envelope(Envelope {
        event: BackendEvent::Warning(TIMEOUT_WARNING.into()),
        generation: None,
    }) {
        TIMEOUT_WARNING_PENDING.store(false, Ordering::Release);
    }
}

fn wake_owner() {
    let wake_thread = WAKE_THREAD.load(Ordering::Acquire);
    if wake_thread == 0 {
        return;
    }
    // SAFETY: `wake_thread` is the initialized Engine queue and the wake
    // message carries no borrowed pointer.
    unsafe {
        if PostThreadMessageW(wake_thread, WAKE_MESSAGE, WPARAM(0), LPARAM(0)).is_err() {
            WAKE_FAILED.store(true, Ordering::Release);
        }
    }
}

fn store_latest_pointer(point: Point) {
    let x = point.x as i32 as u32 as u64;
    let y = point.y as i32 as u32 as u64;
    // Release fences order the odd marker before the payload and the payload
    // before the final even marker. Readers only accept a value bracketed by
    // the same even sequence number.
    POINTER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    fence(Ordering::Release);
    LATEST_POINTER.store(x | (y << 32), Ordering::Relaxed);
    fence(Ordering::Release);
    POINTER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if mark_pointer_pending() {
        wake_owner();
    }
}

fn mark_pointer_pending() -> bool {
    // A counter, rather than a bool, closes the clear-vs-publish race: the
    // consumer's acquire swap either observes this release increment or a
    // concurrent producer observes zero and posts a fresh wake-up.
    POINTER_PENDING.fetch_add(1, Ordering::Release) == 0
}

fn take_latest_pointer() -> Option<Point> {
    if POINTER_PENDING.swap(0, Ordering::Acquire) == 0 {
        return None;
    }
    let (sequence, packed) = loop {
        let before = POINTER_SEQUENCE.load(Ordering::Acquire);
        if before & 1 != 0 {
            std::hint::spin_loop();
            continue;
        }
        let packed = LATEST_POINTER.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        let after = POINTER_SEQUENCE.load(Ordering::Relaxed);
        if before == after {
            break (after, packed);
        }
    };
    if CONSUMED_POINTER_SEQUENCE.swap(sequence, Ordering::Relaxed) == sequence {
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
        // SAFETY: forwarding the unchanged callback arguments is required by
        // the hook contract when `code` is negative.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // SAFETY: for a non-negative low-level keyboard callback Windows supplies
    // `lparam` as a valid KBDLLHOOKSTRUCT for this call's duration.
    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // Ignore only events tagged by this process. Keyboard remappers,
    // accessibility tools and software keyboards also set the Windows
    // injected flag; those inputs are user-owned and must remain bindable.
    if is_our_input(info.dwExtraInfo) {
        // SAFETY: the original callback arguments remain valid and unchanged.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let Some(key) = input::key_for_virtual_key(info.vkCode) else {
        // SAFETY: the original callback arguments remain valid and unchanged.
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
                queue_timeout_warning();
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
    // SAFETY: the original callback arguments remain valid and unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEMOVE {
        // SAFETY: for a non-negative low-level mouse callback Windows supplies
        // `lparam` as a valid MSLLHOOKSTRUCT for this call's duration.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        store_latest_pointer(Point::new(info.pt.x as f64, info.pt.y as f64));
    }
    // SAFETY: the original callback arguments remain valid and unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    fn percentiles(mut samples: Vec<u128>) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

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
            injection: Arc::new(InjectionQueue::default()),
            pending: Some(generation),
            thread_id: 0,
            worker: None,
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
            injection: Arc::new(InjectionQueue::default()),
            pending: Some(generation),
            thread_id: 0,
            worker: None,
        };

        hook.set_disposition(KeyDisposition::Forward).unwrap();
        assert!(hook.pending.is_none());
    }

    #[test]
    fn a_full_event_queue_fails_open_without_waiting() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        assert!(try_deliver(
            &sender,
            Envelope {
                event: BackendEvent::Warning("first".into()),
                generation: None,
            }
        ));

        let started = Instant::now();
        assert!(!try_deliver(
            &sender,
            Envelope {
                event: BackendEvent::Warning("second".into()),
                generation: None,
            }
        ));
        assert!(started.elapsed() < Duration::from_millis(10));
    }

    #[test]
    fn injection_queue_coalesces_wakes_and_preserves_order() {
        let queue = InjectionQueue::default();
        let posts = Cell::new(0);
        let request = || InjectionRequest::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Click,
        };

        let post = |_| {
            posts.set(posts.get() + 1);
            Ok(())
        };
        let first = queue.submit(request(), post).unwrap();
        let second = queue.submit(request(), post).unwrap();
        assert_eq!(posts.get(), 1, "one wake drains the whole pending burst");
        assert_eq!(queue.take_next().unwrap().generation, first);
        assert_eq!(queue.take_next().unwrap().generation, second);
        assert!(queue.take_next().is_none());

        let third = queue.submit(request(), post).unwrap();
        assert_eq!(posts.get(), 2);
        assert_eq!(queue.take_next().unwrap().generation, third);
        assert!(queue.take_next().is_none());
    }

    #[test]
    fn injection_queue_is_bounded_and_a_failed_wake_rolls_back() {
        let queue = InjectionQueue::default();
        let request = || InjectionRequest::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Click,
        };

        let failed = queue.submit(request(), |_| Err("post failed".into()));
        assert!(matches!(
            failed,
            Err((_, InjectionSubmissionFailure::Post(_)))
        ));
        assert_eq!(queue.abandon_pending(), 0);

        for _ in 0..MAX_PENDING_INJECTIONS {
            queue.submit(request(), |_| Ok(())).unwrap();
        }
        assert!(matches!(
            queue.submit(request(), |_| Ok(())),
            Err((_, InjectionSubmissionFailure::Full))
        ));
        assert_eq!(queue.abandon_pending(), MAX_PENDING_INJECTIONS);
    }

    #[test]
    #[ignore = "native performance probe"]
    fn native_performance_probe_hook_queue_and_disposition() {
        const WARMUP: usize = 5_000;
        const SAMPLES: usize = 20_000;
        let request = || InjectionRequest::MouseButton {
            button: MouseButton::Left,
            action: ButtonAction::Click,
        };

        let queue = InjectionQueue::default();
        let mut queue_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..WARMUP + SAMPLES {
            let started = Instant::now();
            black_box(queue.submit(request(), |_| Ok(())).unwrap());
            black_box(queue.take_next().unwrap());
            assert!(queue.take_next().is_none());
            if sample >= WARMUP {
                queue_samples.push(started.elapsed().as_nanos());
            }
        }
        let (queue_p50, queue_p95, queue_p99) = percentiles(queue_samples);

        let mailbox = Arc::new(DispositionMailbox::default());
        let responder_mailbox = Arc::clone(&mailbox);
        let (generation_tx, generation_rx) = mpsc::sync_channel(1);
        let responder = std::thread::spawn(move || {
            while let Ok(generation) = generation_rx.recv() {
                responder_mailbox.complete(generation, KeyDisposition::Consume);
            }
        });
        let mut disposition_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..WARMUP + SAMPLES {
            let generation = mailbox.begin();
            let started = Instant::now();
            generation_tx.send(generation).unwrap();
            assert_eq!(
                mailbox.wait(generation, Duration::from_secs(1)),
                Some(KeyDisposition::Consume)
            );
            if sample >= WARMUP {
                disposition_samples.push(started.elapsed().as_nanos());
            }
        }
        drop(generation_tx);
        responder.join().unwrap();
        let (disposition_p50, disposition_p95, disposition_p99) = percentiles(disposition_samples);
        println!(
            "native_hook samples={SAMPLES} queue_p50={queue_p50}ns queue_p95={queue_p95}ns queue_p99={queue_p99}ns disposition_p50={disposition_p50}ns disposition_p95={disposition_p95}ns disposition_p99={disposition_p99}ns"
        );
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
        POINTER_PENDING.store(0, Ordering::Release);
        assert!(mark_pointer_pending());
        assert!(
            !mark_pointer_pending(),
            "an existing pending move must not wake twice"
        );

        POINTER_PENDING.store(0, Ordering::Release);
        store_latest_pointer(Point::new(-300.0, 100.0));
        store_latest_pointer(Point::new(1920.0, 1080.0));
        assert_eq!(take_latest_pointer(), Some(Point::new(1920.0, 1080.0)));
        assert_eq!(take_latest_pointer(), None);

        // A repeated edge position is still a real native event.
        store_latest_pointer(Point::new(1920.0, 1080.0));
        assert_eq!(take_latest_pointer(), Some(Point::new(1920.0, 1080.0)));
    }
}

#[cfg(test)]
mod pointer_mailbox_loom_tests {
    use loom::sync::Arc;
    use loom::sync::atomic::{AtomicU64, Ordering, fence};
    use loom::thread;

    struct Mailbox {
        pending: AtomicU64,
        sequence: AtomicU64,
        value: AtomicU64,
    }

    impl Mailbox {
        fn new() -> Self {
            Self {
                pending: AtomicU64::new(0),
                sequence: AtomicU64::new(0),
                value: AtomicU64::new(0),
            }
        }

        fn store(&self, value: u64) {
            self.sequence.fetch_add(1, Ordering::Relaxed);
            fence(Ordering::Release);
            self.value.store(value, Ordering::Relaxed);
            fence(Ordering::Release);
            self.sequence.fetch_add(1, Ordering::Relaxed);
            self.pending.fetch_add(1, Ordering::Release);
        }

        fn take(&self) -> Option<(u64, u64)> {
            if self.pending.swap(0, Ordering::Acquire) == 0 {
                return None;
            }
            loop {
                let before = self.sequence.load(Ordering::Acquire);
                if before & 1 != 0 {
                    thread::yield_now();
                    continue;
                }
                let value = self.value.load(Ordering::Relaxed);
                fence(Ordering::Acquire);
                let after = self.sequence.load(Ordering::Relaxed);
                if before == after {
                    return Some((after, value));
                }
            }
        }
    }

    #[test]
    fn pointer_mailbox_orderings_never_pair_a_value_with_the_wrong_sequence() {
        loom::model(|| {
            let mailbox = Arc::new(Mailbox::new());
            mailbox.store(1);

            let writer_mailbox = Arc::clone(&mailbox);
            let writer = thread::spawn(move || writer_mailbox.store(2));
            let reader_mailbox = Arc::clone(&mailbox);
            let reader = thread::spawn(move || reader_mailbox.take());

            let first = reader.join().expect("reader should not panic");
            writer.join().expect("writer should not panic");
            let second = mailbox.take();

            let mut saw_latest = false;
            for (sequence, value) in [first, second].into_iter().flatten() {
                assert_eq!(sequence & 1, 0);
                assert_eq!(value, sequence / 2);
                saw_latest |= value == 2;
            }
            assert!(saw_latest, "the second store must not be lost");
        });
    }
}
