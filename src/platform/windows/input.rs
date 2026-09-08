//! Windows keycode mapping and `SendInput` injection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MOUSE_EVENT_FLAGS, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, XBUTTON1, XBUTTON2};

use crate::api::command::{ButtonAction, MouseButton};
use crate::api::geometry::Point;
use crate::api::input::{Key, KeyState};
use crate::platform::common::character_candidates::{CharacterCandidates, CharacterDemand};

type KeyboardLayout = windows::Win32::UI::Input::KeyboardAndMouse::HKL;

/// Cold writers publish a layout-specific candidate bitmap. A stale or changing
/// bitmap always falls back to observation, never rejects a possible character.
pub struct CharacterCapture {
    candidates: CharacterCandidates,
    demand: CharacterDemand,
    characters: Mutex<Vec<char>>,
}

impl CharacterCapture {
    pub const fn new() -> Self {
        Self {
            candidates: CharacterCandidates::new(),
            demand: CharacterDemand::new(),
            characters: Mutex::new(Vec::new()),
        }
    }

    pub fn configure(&self, keys: &[Key]) {
        let mut previous = self.characters.lock().unwrap_or_else(|e| e.into_inner());
        let characters = CharacterDemand::characters(keys, |key| virtual_key_for(key).is_some());
        if *previous == characters {
            return;
        }
        *previous = characters;
        self.publish(&previous);
    }

    pub(super) fn refresh_if_needed(&self) {
        if self.candidates.take_refresh() {
            let characters = self.characters.lock().unwrap_or_else(|e| e.into_inner());
            self.publish(&characters);
        }
    }

    fn publish(&self, characters: &[char]) {
        // The demand mutex serializes publication and native enumeration.
        self.candidates.begin_update(!characters.is_empty());
        self.demand.publish(characters);
        let (layout, masks, scans) = if characters.is_empty() {
            (0, [u64::MAX; 256], [0; 256])
        } else {
            let layout = foreground_keyboard_layout();
            let (masks, scans) = compile_character_candidates(characters, layout);
            (layout.0 as usize, masks.map(u64::from), scans)
        };
        self.candidates.publish(layout, masks, scans);
    }

    #[inline]
    pub fn character_for_key(&self, vk: u32, scan: u32, pressed: [u64; 4]) -> Option<char> {
        if !self.is_enabled() {
            return None;
        }
        self.observe_candidate(vk, scan, pressed)
    }

    #[inline]
    pub(super) fn is_enabled(&self) -> bool {
        self.demand.is_enabled()
    }

    pub(super) fn observe_candidate(&self, vk: u32, scan: u32, pressed: [u64; 4]) -> Option<char> {
        // WH_KEYBOARD_LL carries no HKL. Validate the *current* foreground
        // layout, even before rejecting a candidate: notification-only caches
        // can miss the first key after a layout switch or a new foreground app.
        let layout = foreground_keyboard_layout();
        if !self.is_candidate(vk, scan, character_modifiers(pressed), layout.0 as usize) {
            return None;
        }
        translate_character(vk, scan, pressed, layout, Some(&self.demand))
    }

    fn is_candidate(&self, vk: u32, scan: u32, modifiers: u8, layout: usize) -> bool {
        self.candidates
            .is_candidate(vk as usize, scan, modifiers, layout)
    }
}

impl Default for CharacterCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Expose the same production converter without candidate filtering for the
/// standalone, uninstrumented release benchmark's conservative reference path.
#[cfg(feature = "benchmark-hooks")]
pub fn observe_character_unfiltered(vk: u32, scan: u32, pressed: [u64; 4]) -> Option<char> {
    translate_character(vk, scan, pressed, foreground_keyboard_layout(), None)
}

pub(super) static CHARACTER_CAPTURE: CharacterCapture = CharacterCapture::new();

fn foreground_keyboard_layout() -> KeyboardLayout {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: no pointers escape; all returned handles are borrowed from Windows.
    unsafe { GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), None)) }
}

fn character_modifiers(pressed: [u64; 4]) -> u8 {
    let down = |code: usize| pressed[code / 64] & (1u64 << (code % 64)) != 0;
    u8::from(down(0x10) || down(0xA0) || down(0xA1))
        | (u8::from(down(0x11) || down(0xA2) || down(0xA3)) << 1)
        | (u8::from(down(0x12) || down(0xA4) || down(0xA5)) << 2)
}

/// Enumerate forward translations, including numpad alternatives and lock
/// states. Reverse lookup alone only returns one way of producing a character.
fn compile_character_candidates(
    characters: &[char],
    layout: KeyboardLayout,
) -> ([u8; 256], [u32; 256]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MAPVK_VK_TO_VSC, MapVirtualKeyExW, ToUnicodeEx,
    };
    let mut candidates = [0u8; 256];
    let mut scan_codes = [0u32; 256];
    let mut found = std::collections::BTreeSet::new();
    // SAFETY: layout is a borrowed HKL; all input and output arrays have the API's
    // documented sizes. Flag 4 never mutates the system's dead-key state.
    unsafe {
        for (vk, _) in VIRTUAL_KEYS {
            if key_map()[*vk as usize]
                .as_ref()
                .is_none_or(Key::is_modifier)
            {
                continue;
            }
            let scan = MapVirtualKeyExW(u32::from(*vk), MAPVK_VK_TO_VSC, Some(layout));
            scan_codes[*vk as usize] = scan;
            for modifiers in 0..8u8 {
                for locks in 0..8u8 {
                    let mut state = [0u8; 256];
                    state[*vk as usize] = 0x80;
                    for (bit, generic, left) in [(1, 0x10, 0xA0), (2, 0x11, 0xA2), (4, 0x12, 0xA4)]
                    {
                        if modifiers & bit != 0 {
                            state[generic] |= 0x80;
                            state[left] |= 0x80;
                        }
                    }
                    for (bit, code) in [(1, 0x14), (2, 0x90), (4, 0x15)] {
                        state[code] |= u8::from(locks & bit != 0);
                    }
                    let mut units = [0u16; 8];
                    let count =
                        ToUnicodeEx(u32::from(*vk), scan, &state, &mut units, 4, Some(layout));
                    // Dead-key layouts depend on pending composition state, so
                    // negative filtering cannot be proved safe for that layout.
                    if count < 0 {
                        return ([u8::MAX; 256], scan_codes);
                    }
                    if count > 0
                        && count as usize <= units.len()
                        && let Some(character) =
                            crate::api::input::single_printable_character(&units[..count as usize])
                        && let Some(character) = character.to_lowercase().next()
                        && characters.binary_search(&character).is_ok()
                    {
                        candidates[*vk as usize] |= 1 << modifiers;
                        found.insert(character);
                    }
                }
            }
        }
    }
    // Unmappable characters / input methods retain the exact observation path.
    if found.len() != characters.len() {
        ([u8::MAX; 256], scan_codes)
    } else {
        (candidates, scan_codes)
    }
}

/// Translate a candidate without altering the foreground's dead-key state.
fn translate_character(
    vk: u32,
    scan: u32,
    pressed: [u64; 4],
    layout: KeyboardLayout,
    demand: Option<&CharacterDemand>,
) -> Option<char> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ToUnicodeEx};
    let mut state = [0u8; 256];
    for (word, mut bits) in pressed.into_iter().enumerate() {
        while bits != 0 {
            state[word * 64 + bits.trailing_zeros() as usize] = 0x80;
            bits &= bits - 1;
        }
    }
    for (generic, left, right) in [(0x10, 0xA0, 0xA1), (0x11, 0xA2, 0xA3), (0x12, 0xA4, 0xA5)] {
        state[generic] |= state[left] | state[right];
    }
    let mut units = [0u16; 8];
    // SAFETY: fixed-size buffers satisfy the APIs. The window/thread/layout
    // handles are borrowed synchronously. Flag 4 requests no keyboard-state
    // mutation (Windows 10 1607+), preserving the foreground dead-key state.
    let count = unsafe {
        for code in [0x14, 0x90, 0x15] {
            state[code] |= (GetKeyState(code as i32) & 1) as u8;
        }
        ToUnicodeEx(vk, scan, &state, &mut units, 4, Some(layout))
    };
    if count <= 0 || count as usize > units.len() {
        return None;
    }
    let units = &units[..count as usize];
    demand.map_or_else(
        || crate::api::input::single_printable_character(units),
        |demand| demand.decode(units),
    )
}

/// Written to `dwExtraInfo` so the low-level hook can ignore our own input.
pub const INJECTED_TAG: usize = 0x4E4D_4B31;
static MOUSE_EDGE_FALLBACK_REPORTED: AtomicBool = AtomicBool::new(false);

pub fn cursor_position() -> Result<Point, String> {
    let mut point = POINT::default();
    // SAFETY: `point` is a valid, correctly-sized out-parameter.
    unsafe { GetCursorPos(&mut point) }.map_err(|e| format!("GetCursorPos failed: {e}"))?;
    Ok(Point::new(point.x as f64, point.y as f64))
}

pub fn warp_cursor(to: Point) -> Result<(), String> {
    // SAFETY: plain scalar arguments.
    unsafe { SetCursorPos(to.x.round() as i32, to.y.round() as i32) }
        .map_err(|e| format!("SetCursorPos failed: {e}"))
}

pub fn move_cursor_relative(from: Point, dx: f64, dy: f64) -> Result<(), String> {
    if dx == 0.0 && dy == 0.0 {
        return Ok(());
    }
    // The engine already owns the authoritative sub-pixel position. An exact
    // absolute update avoids an extra GetCursorPos call and bypasses Windows'
    // user mouse-acceleration curve, so configured speed is deterministic.
    warp_cursor(Point::new(from.x + dx, from.y + dy))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SendFailure {
    sent: usize,
    expected: usize,
    last_error: u32,
}

impl SendFailure {
    fn message(self) -> String {
        send_input_failure(self.sent, self.expected, self.last_error)
    }
}

fn try_send(inputs: &[INPUT]) -> Result<(), SendFailure> {
    // SAFETY: the slice is valid for the call and INPUT's size is passed
    // explicitly, as SendInput requires. Last-error is captured in the same
    // block before any diagnostic API can overwrite it.
    let (sent, last_error) = unsafe {
        // SendInput is documented not to identify UIPI through last-error and
        // may leave the slot untouched. Clear it so diagnostics never report
        // an unrelated error left by an earlier API call.
        windows::Win32::Foundation::SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
        let sent = SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        let last_error = if sent as usize != inputs.len() {
            windows::Win32::Foundation::GetLastError().0
        } else {
            0
        };
        (sent, last_error)
    };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(SendFailure {
            sent: sent as usize,
            expected: inputs.len(),
            last_error,
        })
    }
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    try_send(inputs).map_err(SendFailure::message)
}

enum MouseBatchPath {
    Atomic,
    Individual { batch_failure: SendFailure },
}

enum MouseBatchFailure {
    Batch(SendFailure),
    Individual {
        batch_failure: SendFailure,
        event_index: usize,
        event_failure: SendFailure,
    },
}

fn send_mouse_batch_with(
    inputs: &[INPUT],
    mut attempt: impl FnMut(&[INPUT]) -> Result<(), SendFailure>,
) -> Result<MouseBatchPath, MouseBatchFailure> {
    match attempt(inputs) {
        Ok(()) => Ok(MouseBatchPath::Atomic),
        Err(batch_failure) if batch_failure.sent == 0 && inputs.len() > 1 => {
            for (event_index, input) in inputs.iter().enumerate() {
                if let Err(event_failure) = attempt(std::slice::from_ref(input)) {
                    return Err(MouseBatchFailure::Individual {
                        batch_failure,
                        event_index,
                        event_failure,
                    });
                }
            }
            Ok(MouseBatchPath::Individual { batch_failure })
        }
        Err(failure) => Err(MouseBatchFailure::Batch(failure)),
    }
}

fn send_mouse_batch(
    inputs: &[INPUT],
    button: MouseButton,
    action: ButtonAction,
) -> Result<(), String> {
    match send_mouse_batch_with(inputs, try_send) {
        Ok(MouseBatchPath::Atomic) => Ok(()),
        Ok(MouseBatchPath::Individual { batch_failure }) => {
            if !MOUSE_EDGE_FALLBACK_REPORTED.swap(true, Ordering::Relaxed) {
                crate::report_warning!(
                    "windows-input",
                    "SendInput rejected atomic mouse batch button={button:?} action={action:?} events={} with last_error=0x{:08X}; all individual-event compatibility attempts succeeded",
                    batch_failure.expected,
                    batch_failure.last_error
                );
            }
            Ok(())
        }
        Err(MouseBatchFailure::Batch(failure)) => Err(failure.message()),
        Err(MouseBatchFailure::Individual {
            batch_failure,
            event_index,
            event_failure,
        }) => Err(format_mouse_fallback_failure(
            button,
            action,
            batch_failure,
            event_index,
            event_failure.message(),
        )),
    }
}

fn format_mouse_fallback_failure(
    button: MouseButton,
    action: ButtonAction,
    batch_failure: SendFailure,
    event_index: usize,
    event_failure: String,
) -> String {
    let edge = if event_index.is_multiple_of(2) {
        "down"
    } else {
        "up"
    };
    format!(
        "SendInput rejected atomic mouse batch button={button:?} action={action:?} events={} with last_error=0x{:08X}; individual-event compatibility fallback failed at event={} edge={button:?}_{edge}: {event_failure}",
        batch_failure.expected,
        batch_failure.last_error,
        event_index + 1
    )
}

fn send_input_failure(sent: usize, expected: usize, last_error: u32) -> String {
    let context =
        super::native::send_input_failure_context(last_error, std::mem::size_of::<INPUT>());
    format_send_input_failure(sent, expected, &context)
}

fn format_send_input_failure(sent: usize, expected: usize, context: &str) -> String {
    format!("SendInput inserted {sent} of {expected} events; {context}")
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, mouse_data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_TAG,
            },
        },
    }
}

/// Down/up flags and `mouseData` for a button.
fn button_flags(button: MouseButton) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS, i32) {
    match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
        MouseButton::X1 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON1 as i32),
        MouseButton::X2 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON2 as i32),
    }
}

pub fn button_mask(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1 << 0,
        MouseButton::Right => 1 << 1,
        MouseButton::Middle => 1 << 2,
        MouseButton::X1 => 1 << 3,
        MouseButton::X2 => 1 << 4,
    }
}

fn mouse_button_inputs(button: MouseButton, action: ButtonAction) -> ([INPUT; 4], usize) {
    let (down, up, data) = button_flags(button);
    let mut inputs = [
        mouse_input(down, data),
        mouse_input(up, data),
        mouse_input(down, data),
        mouse_input(up, data),
    ];
    let len = match action {
        ButtonAction::Press => 1,
        ButtonAction::Release => {
            inputs.swap(0, 1);
            1
        }
        ButtonAction::Click => 2,
        ButtonAction::DoubleClick => 4,
    };
    (inputs, len)
}

pub fn mouse_button(button: MouseButton, action: ButtonAction) -> Result<(), String> {
    // Keep every logical action in one SendInput batch. Windows then applies
    // the user's GetDoubleClickTime/GetSystemMetrics settings to consecutive
    // Click calls, while an explicit DoubleClick cannot be split by another
    // producer between its two down/up pairs.
    let (inputs, len) = mouse_button_inputs(button, action);
    send_mouse_batch(&inputs[..len], button, action)
}

/// One wheel notch, in the units `SendInput` expects.
const WHEEL_DELTA: f64 = 120.0;
/// Pixels per notch, matching the Windows default of three lines.
const PIXELS_PER_NOTCH: f64 = 60.0;

pub fn scroll(dx: f64, dy: f64) -> Result<(), String> {
    let notches = |pixels: f64| (pixels / PIXELS_PER_NOTCH * WHEEL_DELTA).round() as i32;
    let vertical = (dy.abs() >= 0.5).then(|| {
        // Negated: positive `dy` scrolls the view down, i.e. the wheel back.
        mouse_input(MOUSEEVENTF_WHEEL, notches(-dy))
    });
    let horizontal = (dx.abs() >= 0.5).then(|| mouse_input(MOUSEEVENTF_HWHEEL, notches(dx)));
    match (vertical, horizontal) {
        (Some(vertical), Some(horizontal)) => send(&[vertical, horizontal]),
        (Some(input), None) | (None, Some(input)) => send(&[input]),
        (None, None) => Ok(()),
    }
}

fn virtual_key_input(vk: u16, state: KeyState) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if state == KeyState::Up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_TAG,
            },
        },
    }
}

fn key_input(key: &Key, state: KeyState) -> Result<INPUT, String> {
    let vk = virtual_key_for(key).ok_or_else(|| format!("no Windows virtual key for {key}"))?;
    Ok(virtual_key_input(vk, state))
}

pub fn send_key(key: &Key, state: KeyState) -> Result<(), String> {
    send(&[key_input(key, state)?])
}

/// Native key codes for one chord. Common chords remain entirely inline while
/// the heap fallback preserves the public API's unbounded input length.
const INLINE_CHORD_KEYS: usize = 8;

pub(super) enum KeyChordBatch {
    Inline {
        keys: [u16; INLINE_CHORD_KEYS],
        len: usize,
    },
    Heap(Box<[u16]>),
}

impl KeyChordBatch {
    pub(super) fn new(keys: &[Key]) -> Result<Self, String> {
        if keys.len() <= INLINE_CHORD_KEYS {
            let mut native = [0; INLINE_CHORD_KEYS];
            for (index, key) in keys.iter().enumerate() {
                native[index] = virtual_key_for(key)
                    .ok_or_else(|| format!("no Windows virtual key for {key}"))?;
            }
            return Ok(Self::Inline {
                keys: native,
                len: keys.len(),
            });
        }
        Ok(Self::Heap(
            keys.iter()
                .map(|key| {
                    virtual_key_for(key).ok_or_else(|| format!("no Windows virtual key for {key}"))
                })
                .collect::<Result<Box<_>, _>>()?,
        ))
    }

    fn as_slice(&self) -> &[u16] {
        match self {
            Self::Inline { keys, len } => &keys[..*len],
            Self::Heap(keys) => keys,
        }
    }
}

/// Submit one complete chord without materialising `(Key, KeyState)` pairs.
/// If Windows accepts only a prefix, release every member defensively.
pub(super) fn send_chord(chord: &KeyChordBatch) -> Result<(), String> {
    let keys = chord.as_slice();
    if keys.is_empty() {
        return Ok(());
    }
    let inputs = KeyInputBatch::from_chord(keys);
    let failure = match try_send(inputs.as_slice()) {
        Ok(()) => return Ok(()),
        Err(failure) => failure,
    };
    let releases = KeyInputBatch::from_releases(keys);
    match send(releases.as_slice()) {
        Ok(()) => Err(failure.message()),
        Err(release) => Err(format!(
            "{}; compensatory chord release failed: {release}",
            failure.message()
        )),
    }
}

/// Tell Windows that a forwarded Alt participated in a shortcut whose action
/// KeySteer consumed. `0xE8` is unassigned, so the tagged pair cannot type or
/// recursively enter our hook, but it prevents Alt-up from opening a menu.
pub(super) fn send_menu_mask() -> Result<(), String> {
    const VK_UNASSIGNED_E8: u16 = 0xE8;
    send(&[
        virtual_key_input(VK_UNASSIGNED_E8, KeyState::Down),
        virtual_key_input(VK_UNASSIGNED_E8, KeyState::Up),
    ])
}

/// Submit a complete chord in one native call. If Windows accepts only a
/// prefix, release every key left down by that prefix before reporting failure.
pub fn send_keys(events: &[(Key, KeyState)]) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let inputs = KeyInputBatch::new(events)?;
    let failure = match try_send(inputs.as_slice()) {
        Ok(()) => return Ok(()),
        Err(failure) => failure,
    };

    let mut held = Vec::<Key>::new();
    for (key, state) in events.iter().take(failure.sent) {
        match state {
            KeyState::Down => held.push(key.clone()),
            KeyState::Up => {
                if let Some(index) = held.iter().rposition(|held_key| held_key == key) {
                    held.remove(index);
                }
            }
        }
    }
    let releases = held
        .iter()
        .rev()
        .map(|key| key_input(key, KeyState::Up))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|release| {
            format!(
                "{}; cannot build compensatory key release: {release}",
                failure.message()
            )
        })?;
    if !releases.is_empty() {
        send(&releases).map_err(|release| {
            format!(
                "{}; compensatory key release failed: {release}",
                failure.message()
            )
        })?;
    }
    Err(failure.message())
}

/// Most configured chords contain at most eight keys (sixteen down/up edges).
/// Keep their native INPUT records on the hook thread's stack; arbitrary host
/// sequences remain supported by the heap fallback.
const INLINE_KEY_INPUTS: usize = 16;

// The size skew is intentional: boxing the inline variant would restore the
// allocation this batch exists to avoid on the input-injection hot path.
#[allow(clippy::large_enum_variant)]
enum KeyInputBatch {
    Inline {
        inputs: [INPUT; INLINE_KEY_INPUTS],
        len: usize,
    },
    Heap(Vec<INPUT>),
}

impl KeyInputBatch {
    fn new(events: &[(Key, KeyState)]) -> Result<Self, String> {
        if events.len() <= INLINE_KEY_INPUTS {
            let mut inputs = std::array::from_fn(|_| INPUT::default());
            for (index, (key, state)) in events.iter().enumerate() {
                inputs[index] = key_input(key, *state)?;
            }
            return Ok(Self::Inline {
                inputs,
                len: events.len(),
            });
        }
        Ok(Self::Heap(
            events
                .iter()
                .map(|(key, state)| key_input(key, *state))
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    fn from_chord(keys: &[u16]) -> Self {
        let len = keys.len().saturating_mul(2);
        if len <= INLINE_KEY_INPUTS {
            let mut inputs = std::array::from_fn(|_| INPUT::default());
            for (index, key) in keys.iter().enumerate() {
                inputs[index] = virtual_key_input(*key, KeyState::Down);
                inputs[len - 1 - index] = virtual_key_input(*key, KeyState::Up);
            }
            return Self::Inline { inputs, len };
        }
        Self::Heap(
            keys.iter()
                .map(|key| virtual_key_input(*key, KeyState::Down))
                .chain(
                    keys.iter()
                        .rev()
                        .map(|key| virtual_key_input(*key, KeyState::Up)),
                )
                .collect(),
        )
    }

    fn from_releases(keys: &[u16]) -> Self {
        if keys.len() <= INLINE_KEY_INPUTS {
            let mut inputs = std::array::from_fn(|_| INPUT::default());
            for (index, key) in keys.iter().rev().enumerate() {
                inputs[index] = virtual_key_input(*key, KeyState::Up);
            }
            return Self::Inline {
                inputs,
                len: keys.len(),
            };
        }
        Self::Heap(
            keys.iter()
                .rev()
                .map(|key| virtual_key_input(*key, KeyState::Up))
                .collect(),
        )
    }

    fn as_slice(&self) -> &[INPUT] {
        match self {
            Self::Inline { inputs, len } => &inputs[..*len],
            Self::Heap(inputs) => inputs,
        }
    }
}

/// Keys that require `KEYEVENTF_EXTENDEDKEY` to be delivered correctly.
fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 // page up/down, end, home, arrows
            | 0x2D | 0x2E // insert, delete
            | 0x5B | 0x5C // left/right win
            | 0x6F // divide
            | 0x90 // num lock
            | 0xA3 // right ctrl
            | 0xA5 // right alt
    )
}

/// Keep the reverse table and the compiler-optimised name lookup generated
/// from one source of truth. This avoids a linear scan on every injected edge
/// without adding a runtime map or another allocation.
macro_rules! define_virtual_keys {
    ($(($code:expr, $name:literal)),+ $(,)?) => {
        const VIRTUAL_KEYS: &[(u16, &str)] = &[
            $(($code, $name)),+
        ];

        pub fn virtual_key_for(key: &Key) -> Option<u16> {
            match key.as_str() {
                $($name => Some($code),)+
                _ => None,
            }
        }
    };
}

define_virtual_keys! {
    (0x08, "backspace"),
    (0x09, "tab"),
    (0x0D, "enter"),
    (0x13, "pause"),
    (0x14, "caps_lock"),
    (0x1B, "esc"),
    (0x20, "space"),
    (0x21, "page_up"),
    (0x22, "page_down"),
    (0x23, "end"),
    (0x24, "home"),
    (0x25, "arrow_left"),
    (0x26, "arrow_up"),
    (0x27, "arrow_right"),
    (0x28, "arrow_down"),
    (0x2C, "print_screen"),
    (0x2D, "insert"),
    (0x2E, "delete"),
    (0x30, "0"),
    (0x31, "1"),
    (0x32, "2"),
    (0x33, "3"),
    (0x34, "4"),
    (0x35, "5"),
    (0x36, "6"),
    (0x37, "7"),
    (0x38, "8"),
    (0x39, "9"),
    (0x41, "a"),
    (0x42, "b"),
    (0x43, "c"),
    (0x44, "d"),
    (0x45, "e"),
    (0x46, "f"),
    (0x47, "g"),
    (0x48, "h"),
    (0x49, "i"),
    (0x4A, "j"),
    (0x4B, "k"),
    (0x4C, "l"),
    (0x4D, "m"),
    (0x4E, "n"),
    (0x4F, "o"),
    (0x50, "p"),
    (0x51, "q"),
    (0x52, "r"),
    (0x53, "s"),
    (0x54, "t"),
    (0x55, "u"),
    (0x56, "v"),
    (0x57, "w"),
    (0x58, "x"),
    (0x59, "y"),
    (0x5A, "z"),
    (0x5B, "left_win"),
    (0x5C, "right_win"),
    (0x60, "numpad_0"),
    (0x61, "numpad_1"),
    (0x62, "numpad_2"),
    (0x63, "numpad_3"),
    (0x64, "numpad_4"),
    (0x65, "numpad_5"),
    (0x66, "numpad_6"),
    (0x67, "numpad_7"),
    (0x68, "numpad_8"),
    (0x69, "numpad_9"),
    (0x6A, "multiply"),
    (0x6B, "add"),
    (0x6D, "subtract"),
    (0x6E, "decimal"),
    (0x6F, "divide"),
    (0x70, "f1"),
    (0x71, "f2"),
    (0x72, "f3"),
    (0x73, "f4"),
    (0x74, "f5"),
    (0x75, "f6"),
    (0x76, "f7"),
    (0x77, "f8"),
    (0x78, "f9"),
    (0x79, "f10"),
    (0x7A, "f11"),
    (0x7B, "f12"),
    // F13-F24 exist on Windows but not on macOS, so a config using them is
    // not portable. They are supported here because the OS does.
    (0x7C, "f13"),
    (0x7D, "f14"),
    (0x7E, "f15"),
    (0x7F, "f16"),
    (0x80, "f17"),
    (0x81, "f18"),
    (0x82, "f19"),
    (0x83, "f20"),
    (0x84, "f21"),
    (0x85, "f22"),
    (0x86, "f23"),
    (0x87, "f24"),
    (0x90, "num_lock"),
    (0x91, "scroll_lock"),
    (0xA0, "left_shift"),
    (0xA1, "right_shift"),
    (0xA2, "left_ctrl"),
    (0xA3, "right_ctrl"),
    (0xA4, "left_alt"),
    (0xA5, "right_alt"),
    (0xBA, ";"),
    (0xBB, "="),
    (0xBC, ","),
    (0xBD, "-"),
    (0xBE, "."),
    (0xBF, "/"),
    (0xC0, "`"),
    (0xDB, "["),
    (0xDC, "\\"),
    (0xDD, "]"),
    (0xDE, "'"),
}

pub fn key_for_virtual_key(vk: u32) -> Option<Key> {
    let index = usize::try_from(vk).ok()?;
    key_map().get(index)?.clone()
}

static KEY_BY_VK: OnceLock<[Option<Key>; 256]> = OnceLock::new();

fn key_map() -> &'static [Option<Key>; 256] {
    KEY_BY_VK.get_or_init(|| {
        std::array::from_fn(|index| {
            VIRTUAL_KEYS
                .iter()
                .find(|(code, _)| usize::from(*code) == index)
                .and_then(|(_, name)| Key::new(name).ok())
        })
    })
}

pub(super) fn prewarm_key_map() {
    let _ = key_map();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    #[test]
    fn physical_only_bindings_leave_character_capture_off() {
        let capture = CharacterCapture::new();
        let keys = ["h", "j", "k", "l", ";", "'", "1", "/"].map(|name| Key::new(name).unwrap());
        capture.configure(&keys);
        assert!(!capture.candidates.is_enabled());
        assert_eq!(capture.character_for_key(0x48, 0x23, [0; 4]), None);
    }

    #[test]
    fn character_capture_configuration_can_be_removed_without_stale_candidates() {
        let capture = CharacterCapture::new();
        // Seed an old snapshot without consulting the user's live layout.
        *capture.characters.lock().unwrap() = vec!['?'];
        capture.candidates.enabled.store(true, Ordering::Release);
        capture.configure(&[]);
        assert!(!capture.candidates.is_enabled());
        assert!(capture.characters.lock().unwrap().is_empty());
        assert_eq!(capture.character_for_key(0xBF, 0x35, [0; 4]), None);
    }

    #[test]
    #[ignore = "read-only native layout probe; requires the US layout to be loaded"]
    fn native_character_layout_probe() {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;
        let mut layouts = [KeyboardLayout::default(); 32];
        // SAFETY: the fixed slice is writable for the entire synchronous call.
        // This only reads installed layouts; it never loads or activates one.
        let count = unsafe { GetKeyboardLayoutList(Some(&mut layouts)) };
        let layout = layouts[..(count.max(0) as usize).min(layouts.len())]
            .iter()
            .copied()
            .find(|layout| layout.0 as usize & 0xffff_ffff == 0x0409_0409)
            .expect("load the US keyboard layout before running this explicit native probe");
        let (masks, scans) = compile_character_candidates(&['!', '+', '?', '~'], layout);
        assert_eq!(masks[0x48], 0, "H is never a candidate for these symbols");
        assert_eq!(masks[0xBF] & 1, 0, "plain slash");
        assert_ne!(masks[0xBF] & (1 << 1), 0, "Shift+slash");
        assert_ne!(masks[0x31] & (1 << 1), 0, "Shift+1");
        assert_ne!(masks[0xC0] & (1 << 1), 0, "Shift+backtick");
        assert_ne!(masks[0xBB] & (1 << 1), 0, "Shift+equals");
        assert_ne!(masks[0x6B] & 1, 0, "numpad add also produces +");
        assert_eq!(scans[0x48], 0x23);
        for (expected, vk, shifted) in [
            ('?', 0xBFusize, true),
            ('!', 0x31, true),
            ('~', 0xC0, true),
            ('+', 0xBB, true),
            ('+', 0x6B, false),
        ] {
            let mut pressed = [0u64; 4];
            pressed[vk / 64] |= 1 << (vk % 64);
            if shifted {
                pressed[0xA0 / 64] |= 1 << (0xA0 % 64);
            }
            assert_eq!(
                translate_character(vk as u32, scans[vk], pressed, layout, None),
                Some(expected)
            );
        }
        assert_eq!(
            compile_character_candidates(&['🦀'], layout).0,
            [u8::MAX; 256],
            "unmappable characters preserve the observation fallback"
        );
    }

    fn mouse_data(input: &INPUT) -> MOUSEINPUT {
        // SAFETY: every caller passes an INPUT value built by
        // `mouse_button_inputs`, which activates the `mi` union variant.
        unsafe { input.Anonymous.mi }
    }

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
    fn virtual_key_mapping_round_trips() {
        for (code, name) in VIRTUAL_KEYS {
            let key = Key::new(name).unwrap();
            assert_eq!(virtual_key_for(&key), Some(*code), "{name}");
            assert_eq!(
                key_for_virtual_key(*code as u32).as_ref(),
                Some(&key),
                "{name}"
            );
        }
    }

    #[test]
    fn common_key_sequences_use_the_inline_native_batch() {
        let event = (Key::new("a").unwrap(), KeyState::Down);
        let events = vec![event; INLINE_KEY_INPUTS];
        let batch = KeyInputBatch::new(&events).unwrap();
        assert!(matches!(batch, KeyInputBatch::Inline { len, .. } if len == INLINE_KEY_INPUTS));
    }

    #[test]
    fn common_chords_keep_key_codes_and_native_inputs_inline() {
        let keys = ["left_ctrl", "left_shift", "left_alt", "a"].map(|name| Key::new(name).unwrap());
        let chord = KeyChordBatch::new(&keys).unwrap();
        assert!(matches!(chord, KeyChordBatch::Inline { len: 4, .. }));
        let inputs = KeyInputBatch::from_chord(chord.as_slice());
        assert!(matches!(inputs, KeyInputBatch::Inline { len: 8, .. }));
    }

    #[test]
    fn oversized_key_sequences_preserve_the_heap_fallback() {
        let event = (Key::new("a").unwrap(), KeyState::Down);
        let events = vec![event; INLINE_KEY_INPUTS + 1];
        let batch = KeyInputBatch::new(&events).unwrap();
        assert!(
            matches!(batch, KeyInputBatch::Heap(inputs) if inputs.len() == INLINE_KEY_INPUTS + 1)
        );
    }

    #[test]
    #[ignore = "native performance probe"]
    fn native_performance_probe_key_input_batch() {
        const WARMUP: usize = 10_000;
        const SAMPLES: usize = 50_000;
        let keys = ["left_ctrl", "left_shift", "left_alt", "a"].map(|name| Key::new(name).unwrap());
        let events = keys
            .iter()
            .cloned()
            .map(|key| (key, KeyState::Down))
            .chain(keys.iter().rev().cloned().map(|key| (key, KeyState::Up)))
            .collect::<Vec<_>>();

        for _ in 0..WARMUP {
            black_box(KeyInputBatch::new(black_box(&events)).unwrap());
        }
        let mut inline_samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            black_box(KeyInputBatch::new(black_box(&events)).unwrap());
            inline_samples.push(started.elapsed().as_nanos());
        }
        let (inline_p50, inline_p95, inline_p99) = percentiles(inline_samples);

        let mut legacy_samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            let inputs = events
                .iter()
                .map(|(key, state)| {
                    let vk = VIRTUAL_KEYS
                        .iter()
                        .find(|(_, name)| *name == key.as_str())
                        .map(|(code, _)| *code)
                        .ok_or_else(|| format!("no Windows virtual key for {key}"))?;
                    Ok::<_, String>(virtual_key_input(vk, *state))
                })
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            black_box(inputs);
            legacy_samples.push(started.elapsed().as_nanos());
        }
        let (legacy_p50, legacy_p95, legacy_p99) = percentiles(legacy_samples);
        println!(
            "native_key_batch samples={SAMPLES} inline_p50={inline_p50}ns inline_p95={inline_p95}ns inline_p99={inline_p99}ns legacy_p50={legacy_p50}ns legacy_p95={legacy_p95}ns legacy_p99={legacy_p99}ns"
        );
    }

    #[test]
    fn unknown_codes_are_rejected() {
        assert!(key_for_virtual_key(0xFFFF).is_none());
        assert!(virtual_key_for(&Key::new("nonexistent").unwrap()).is_none());
    }

    #[test]
    fn arrows_and_right_modifiers_are_extended() {
        assert!(is_extended(0x25), "arrow_left");
        assert!(is_extended(0xA3), "right_ctrl");
        assert!(!is_extended(0x41), "a");
    }

    #[test]
    fn send_input_failure_message_reports_evidence_without_guessing_uipi() {
        let message =
            format_send_input_failure(0, 2, "last_error=0x00000000, current={integrity=high}");
        assert!(message.contains("inserted 0 of 2"));
        assert!(message.contains("last_error=0x00000000"));
        assert!(!message.contains("may be blocking"));
    }

    #[test]
    fn zero_inserted_mouse_batch_retries_each_edge_once() {
        let inputs = std::array::from_fn::<_, 2, _>(|_| INPUT::default());
        let mut attempts = Vec::new();
        let result = send_mouse_batch_with(&inputs, |batch| {
            attempts.push(batch.len());
            if attempts.len() == 1 {
                Err(SendFailure {
                    sent: 0,
                    expected: batch.len(),
                    last_error: 0,
                })
            } else {
                Ok(())
            }
        });

        assert!(matches!(result, Ok(MouseBatchPath::Individual { .. })));
        assert_eq!(attempts, [2, 1, 1]);
    }

    #[test]
    fn partial_mouse_batch_is_never_replayed() {
        let inputs = std::array::from_fn::<_, 2, _>(|_| INPUT::default());
        let mut attempts = Vec::new();
        let result = send_mouse_batch_with(&inputs, |batch| {
            attempts.push(batch.len());
            Err(SendFailure {
                sent: 1,
                expected: batch.len(),
                last_error: 0,
            })
        });

        assert!(matches!(result, Err(MouseBatchFailure::Batch(_))));
        assert_eq!(attempts, [2]);
    }

    #[test]
    fn individual_mouse_fallback_reports_the_exact_failed_edge() {
        let inputs = std::array::from_fn::<_, 2, _>(|_| INPUT::default());
        let mut attempt = 0;
        let result = send_mouse_batch_with(&inputs, |batch| {
            attempt += 1;
            match attempt {
                1 => Err(SendFailure {
                    sent: 0,
                    expected: batch.len(),
                    last_error: 0,
                }),
                2 => Ok(()),
                _ => Err(SendFailure {
                    sent: 0,
                    expected: batch.len(),
                    last_error: 5,
                }),
            }
        });

        assert!(matches!(
            result,
            Err(MouseBatchFailure::Individual { event_index: 1, .. })
        ));

        let message = format_mouse_fallback_failure(
            MouseButton::Left,
            ButtonAction::Click,
            SendFailure {
                sent: 0,
                expected: 2,
                last_error: 0,
            },
            1,
            "last_error=0x00000005".into(),
        );
        assert!(message.contains("button=Left action=Click events=2"));
        assert!(message.contains("event=2 edge=Left_up"));
        assert!(message.contains("last_error=0x00000005"));
    }

    #[test]
    fn click_and_double_click_keep_complete_native_edge_sequences() {
        let flags = |action| {
            let (inputs, len) = mouse_button_inputs(MouseButton::Left, action);
            inputs
                .into_iter()
                .take(len)
                .map(|input| mouse_data(&input).dwFlags)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            flags(ButtonAction::Click),
            [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP]
        );
        assert_eq!(flags(ButtonAction::Press), [MOUSEEVENTF_LEFTDOWN]);
        assert_eq!(flags(ButtonAction::Release), [MOUSEEVENTF_LEFTUP]);
        assert_eq!(
            flags(ButtonAction::DoubleClick),
            [
                MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP,
                MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP,
            ]
        );
    }

    #[test]
    fn every_button_keeps_its_own_down_and_up_edges() {
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::X1,
            MouseButton::X2,
        ] {
            let (inputs, len) = mouse_button_inputs(button, ButtonAction::Click);
            assert_eq!(len, 2, "{button:?}");
            let down = mouse_data(&inputs[0]);
            let up = mouse_data(&inputs[1]);
            assert_ne!(down.dwFlags, up.dwFlags, "{button:?}");
            assert_eq!(down.mouseData, up.mouseData, "{button:?}");
        }
    }
}
