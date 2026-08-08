//! Windows keycode mapping and `SendInput` injection.

use std::sync::OnceLock;

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

/// Written to `dwExtraInfo` so the low-level hook can ignore our own input.
pub const INJECTED_TAG: usize = 0x4E4D_4B31;

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

fn send(inputs: &[INPUT]) -> Result<(), String> {
    // SAFETY: the slice is valid for the call and INPUT's size is passed
    // explicitly, as SendInput requires.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!(
            "SendInput sent {sent} of {} events; Windows UIPI may be blocking input to a higher-integrity window",
            inputs.len()
        ))
    }
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
    send(&inputs[..len])
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

/// Replay a short keyboard prefix from the low-level hook without allocating.
pub(super) fn send_virtual_keys(events: &[(u16, KeyState)]) -> Result<(), String> {
    if events.len() > 5 {
        return Err("deferred keyboard replay exceeds its fixed capacity".into());
    }
    let inputs: [INPUT; 5] = std::array::from_fn(|index| {
        events
            .get(index)
            .map_or_else(INPUT::default, |(vk, state)| virtual_key_input(*vk, *state))
    });
    send(&inputs[..events.len()])
}

/// Submit a complete chord in one native call. If Windows accepts only a
/// prefix, release every key left down by that prefix before reporting failure.
pub fn send_keys(events: &[(Key, KeyState)]) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let inputs = events
        .iter()
        .map(|(key, state)| key_input(key, *state))
        .collect::<Result<Vec<_>, _>>()?;
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == inputs.len() {
        return Ok(());
    }

    let mut held = Vec::<Key>::new();
    for (key, state) in events.iter().take(sent) {
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
        .filter_map(|key| key_input(key, KeyState::Up).ok())
        .collect::<Vec<_>>();
    if !releases.is_empty() {
        let _ = send(&releases);
    }
    Err(format!(
        "SendInput sent {sent} of {} keyboard events; Windows UIPI may be blocking input to a higher-integrity window",
        inputs.len()
    ))
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

/// Virtual-key table, shared by both mapping directions.
const VIRTUAL_KEYS: &[(u16, &str)] = &[
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
];

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

pub fn virtual_key_for(key: &Key) -> Option<u16> {
    VIRTUAL_KEYS
        .iter()
        .find(|(_, name)| *name == key.as_str())
        .map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn click_and_double_click_keep_complete_native_edge_sequences() {
        let flags = |action| {
            let (inputs, len) = mouse_button_inputs(MouseButton::Left, action);
            inputs
                .into_iter()
                .take(len)
                // SAFETY: every item was constructed above with the `mi`
                // variant of INPUT_0 active.
                .map(|input| unsafe { input.Anonymous.mi.dwFlags })
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
            // SAFETY: mouse_button_inputs creates only INPUT_MOUSE values.
            let down = unsafe { inputs[0].Anonymous.mi };
            let up = unsafe { inputs[1].Anonymous.mi };
            assert_ne!(down.dwFlags, up.dwFlags, "{button:?}");
            assert_eq!(down.mouseData, up.mouseData, "{button:?}");
        }
    }
}
