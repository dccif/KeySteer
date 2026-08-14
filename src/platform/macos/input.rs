//! macOS keycode mapping and CoreGraphics input injection.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use crate::api::command::{ButtonAction, MouseButton};
use crate::api::geometry::Point;
use crate::api::input::{Key, KeyState};
use crate::platform::multi_click::ClickTracker;

use super::native::OwnedCf;

/// Tags our synthetic events so the tap can ignore them. Any value works as
/// long as it is unlikely to collide with another tool's.
pub const INJECTED_TAG: i64 = 0x4E4D_4B31;

unsafe extern "C" {
    fn CGWarpMouseCursorPosition(new_position: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: u8) -> i32;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *const std::ffi::c_void) -> CGPoint;
    fn CGEventCreateMouseEvent(
        source: *const std::ffi::c_void,
        event_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> *mut std::ffi::c_void;
    fn CGEventCreateScrollWheelEvent(
        source: *const std::ffi::c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        ...
    ) -> *mut std::ffi::c_void;
    fn CGEventSetIntegerValueField(event: *mut std::ffi::c_void, field: u32, value: i64);
    fn CGEventSetLocation(event: *mut std::ffi::c_void, location: CGPoint);
    fn CGEventPost(tap: u32, event: *mut std::ffi::c_void);
}

/// `kCGScrollEventUnitPixel`.
const SCROLL_UNIT_PIXEL: u32 = 0;
/// `kCGSessionEventTap` — location-based mouse input is posted above the HID
/// accessibility-zoom transform, matching neru's reliable native path.
const SESSION_EVENT_TAP: u32 = 1;

pub(super) struct KeyboardInjector {
    source: Result<CGEventSource, String>,
}

impl KeyboardInjector {
    pub(super) fn new() -> Self {
        Self {
            source: CGEventSource::new(CGEventSourceStateID::HIDSystemState)
                .map_err(|_| "cannot create a CoreGraphics event source".to_string()),
        }
    }

    fn source(&self) -> Result<&CGEventSource, String> {
        self.source.as_ref().map_err(Clone::clone)
    }

    pub(super) fn send_key(&self, key: &Key, state: KeyState) -> Result<(), String> {
        post_key_event(self.source()?, key, state)
    }

    pub(super) fn send_keys(&self, events: Vec<(Key, KeyState)>) -> Result<(), String> {
        // Reject an invalid member before posting any prefix of the batch.
        for (key, _) in &events {
            if keycode_for(key).is_none() {
                return Err(format!("no macOS keycode for {key}"));
            }
        }
        let source = self.source()?;
        for (key, state) in events {
            post_key_event(source, &key, state)?;
        }
        Ok(())
    }

    pub(super) fn send_chord(&self, keys: &[Key]) -> Result<(), String> {
        // Validate the complete chord before posting its first edge so an
        // unsupported member cannot leave an earlier modifier pressed.
        for key in keys {
            if keycode_for(key).is_none() {
                return Err(format!("no macOS keycode for {key}"));
            }
        }
        let source = self.source()?;
        for (pressed, key) in keys.iter().enumerate() {
            if let Err(error) = post_key_event(source, key, KeyState::Down) {
                for pressed_key in keys[..pressed].iter().rev() {
                    let _ = post_key_event(source, pressed_key, KeyState::Up);
                }
                return Err(error);
            }
        }
        for key in keys.iter().rev() {
            if let Err(error) = post_key_event(source, key, KeyState::Up) {
                // Key-up is idempotent. Releasing the complete chord is safer
                // than trying to infer which native posts reached the session.
                for pressed_key in keys.iter().rev() {
                    let _ = post_key_event(source, pressed_key, KeyState::Up);
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

pub fn cursor_position() -> Result<Point, String> {
    // A NULL source is sufficient for reading the global cursor and avoids
    // allocating a separate CGEventSource for every mouse action.
    // SAFETY: CGEventCreate returns a +1 event or null; the wrapper takes the
    // create-rule reference exactly once.
    let Some(event) = (unsafe { OwnedCf::from_create_rule(CGEventCreate(std::ptr::null())) })
    else {
        return Err("cannot read the cursor position".into());
    };
    // SAFETY: `event` is a live CGEvent for this value-returning query.
    let position = unsafe { CGEventGetLocation(event.as_ptr()) };
    Ok(Point::new(position.x, position.y))
}

pub fn warp_cursor(to: Point) -> Result<(), String> {
    // Decoupling briefly stops the hardware delta from fighting the warp.
    // SAFETY: the global cursor association API has no pointer arguments.
    unsafe { CGAssociateMouseAndMouseCursorPosition(0) };
    // SAFETY: CGPoint is initialized and passed by value.
    let status = unsafe { CGWarpMouseCursorPosition(CGPoint::new(to.x, to.y)) };
    // SAFETY: this balances the preceding temporary disassociation.
    unsafe { CGAssociateMouseAndMouseCursorPosition(1) };

    if status == 0 {
        Ok(())
    } else {
        Err(format!("CGWarpMouseCursorPosition failed: {status}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MouseEventSpec {
    event_type: u32,
    button: u32,
}

fn button_number(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => CGMouseButton::Left as u32,
        MouseButton::Right => CGMouseButton::Right as u32,
        MouseButton::Middle => CGMouseButton::Center as u32,
        MouseButton::X1 => 3,
        MouseButton::X2 => 4,
    }
}

fn button_event(button: MouseButton, down: bool) -> MouseEventSpec {
    let event_type = match (button, down) {
        (MouseButton::Left, true) => CGEventType::LeftMouseDown,
        (MouseButton::Left, false) => CGEventType::LeftMouseUp,
        (MouseButton::Right, true) => CGEventType::RightMouseDown,
        (MouseButton::Right, false) => CGEventType::RightMouseUp,
        (_, true) => CGEventType::OtherMouseDown,
        (_, false) => CGEventType::OtherMouseUp,
    };
    MouseEventSpec {
        event_type: event_type as u32,
        button: button_number(button),
    }
}

fn drag_event(button: MouseButton) -> MouseEventSpec {
    let event_type = match button {
        MouseButton::Left => CGEventType::LeftMouseDragged,
        MouseButton::Right => CGEventType::RightMouseDragged,
        _ => CGEventType::OtherMouseDragged,
    };
    MouseEventSpec {
        event_type: event_type as u32,
        button: button_number(button),
    }
}

pub const fn button_mask(button: MouseButton) -> u8 {
    1 << (button as u8)
}

fn movement_event(held_buttons: u8) -> MouseEventSpec {
    for button in [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::X1,
        MouseButton::X2,
    ] {
        if held_buttons & button_mask(button) != 0 {
            return drag_event(button);
        }
    }
    MouseEventSpec {
        event_type: CGEventType::MouseMoved as u32,
        button: CGMouseButton::Left as u32,
    }
}

fn create_mouse_event(spec: MouseEventSpec, at: Point, click_count: i64) -> Option<OwnedCf> {
    // SAFETY: the event parameters are initialized enum/value types and the
    // returned +1 object is transferred into OwnedCf exactly once.
    let event = unsafe {
        OwnedCf::from_create_rule(CGEventCreateMouseEvent(
            std::ptr::null(),
            spec.event_type,
            CGPoint::new(at.x, at.y),
            spec.button,
        ))
    }?;
    // SAFETY: `event` is live and exclusively owned while both integer fields
    // are set synchronously.
    unsafe {
        CGEventSetIntegerValueField(
            event.as_mut_ptr(),
            EventField::EVENT_SOURCE_USER_DATA,
            INJECTED_TAG,
        );
        if click_count > 0 {
            CGEventSetIntegerValueField(
                event.as_mut_ptr(),
                EventField::MOUSE_EVENT_CLICK_STATE,
                click_count,
            );
        }
        if spec.button > CGMouseButton::Center as u32 {
            CGEventSetIntegerValueField(
                event.as_mut_ptr(),
                EventField::MOUSE_EVENT_BUTTON_NUMBER,
                spec.button as i64,
            );
        }
    }
    Some(event)
}

fn post_mouse_event(spec: MouseEventSpec, at: Point) -> Result<(), String> {
    let Some(event) = create_mouse_event(spec, at, 0) else {
        return Err("cannot create a macOS mouse movement event".into());
    };
    // SAFETY: `event` is a live CGEvent and CGEventPost does not retain it.
    unsafe {
        CGEventPost(SESSION_EVENT_TAP, event.as_mut_ptr());
    }
    Ok(())
}

pub fn move_cursor_relative(
    from: Point,
    dx: f64,
    dy: f64,
    held_buttons: u8,
) -> Result<Point, String> {
    let to = Point::new(from.x + dx, from.y + dy);
    post_mouse_event(movement_event(held_buttons), to)?;
    Ok(to)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ButtonStep {
    down: bool,
    click_count: i64,
}

fn button_steps(action: ButtonAction, click_counts: &[i64]) -> ([ButtonStep; 4], usize) {
    let mut steps = [ButtonStep {
        down: false,
        click_count: 0,
    }; 4];
    let down_edges: &[bool] = match action {
        ButtonAction::Press => &[true],
        ButtonAction::Release => &[false],
        ButtonAction::Click => &[true, false],
        ButtonAction::DoubleClick => &[true, false, true, false],
    };
    debug_assert_eq!(down_edges.len(), click_counts.len());
    for (index, (down, click_count)) in down_edges
        .iter()
        .copied()
        .zip(click_counts.iter().copied())
        .enumerate()
    {
        steps[index] = ButtonStep { down, click_count };
    }
    (steps, down_edges.len())
}

pub fn mouse_button(
    tracker: &Mutex<ClickTracker>,
    button: MouseButton,
    action: ButtonAction,
) -> Result<(), String> {
    let at = cursor_position()?;
    let mut tracker = tracker
        .lock()
        .map_err(|_| "macOS multi-click state is unavailable".to_string())?;
    let plan = tracker.prepare(button, action, at, Instant::now());
    let (steps, step_count) = button_steps(action, plan.counts());

    // Create the complete sequence before posting any part of it. If event
    // allocation fails, no partial click can leave a target application with a
    // button held down.
    let mut events: [Option<OwnedCf>; 4] = std::array::from_fn(|_| None);
    for index in 0..step_count {
        events[index] = create_mouse_event(
            button_event(button, steps[index].down),
            at,
            steps[index].click_count,
        );
        if events[index].is_none() {
            return Err("cannot create a macOS mouse button event".into());
        }
    }
    for event in events[..step_count].iter().flatten() {
        // SAFETY: every optional event is live for the synchronous post and is
        // retained by its OwnedCf until the loop iteration ends.
        unsafe {
            CGEventPost(SESSION_EVENT_TAP, event.as_mut_ptr());
        }
    }
    tracker.commit(plan);
    Ok(())
}

pub fn scroll(dx: f64, dy: f64) -> Result<(), String> {
    let at = cursor_position()?;
    // Match neru's proven macOS path exactly: raw configured pixel deltas,
    // NULL event source, both axes declared, and session-level posting.
    let vertical = (-dy).round() as i32;
    let horizontal = (-dx).round() as i32;
    if vertical == 0 && horizontal == 0 {
        return Ok(());
    }

    // SAFETY: the returned Create-rule event is transferred immediately into
    // OwnedCf. The variadic arguments exactly match wheel_count=2 (vertical,
    // horizontal).
    unsafe {
        let Some(event) = OwnedCf::from_create_rule(CGEventCreateScrollWheelEvent(
            std::ptr::null(),
            SCROLL_UNIT_PIXEL,
            2,
            vertical,
            horizontal,
        )) else {
            return Err("cannot create a macOS pixel scroll event".into());
        };
        CGEventSetLocation(event.as_mut_ptr(), CGPoint::new(at.x, at.y));
        CGEventPost(SESSION_EVENT_TAP, event.as_mut_ptr());
    }
    Ok(())
}

fn post_key_event(source: &CGEventSource, key: &Key, state: KeyState) -> Result<(), String> {
    let code = keycode_for(key).ok_or_else(|| format!("no macOS keycode for {key}"))?;
    let event = CGEvent::new_keyboard_event(source.clone(), code, state == KeyState::Down)
        .map_err(|_| format!("cannot create a key event for {key}"))?;
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_TAG);
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// Virtual keycode table, ordered to mirror `keycode_for`.
macro_rules! macos_keycodes {
    ($(($code:expr, $name:literal)),+ $(,)?) => {
        const KEYCODES: &[(u16, &str)] = &[$(($code, $name)),+];

        pub fn keycode_for(key: &Key) -> Option<u16> {
            match key.as_str() {
                $($name => Some($code),)+
                _ => None,
            }
        }
    };
}

macos_keycodes! {
    (0x00, "a"),
    (0x01, "s"),
    (0x02, "d"),
    (0x03, "f"),
    (0x04, "h"),
    (0x05, "g"),
    (0x06, "z"),
    (0x07, "x"),
    (0x08, "c"),
    (0x09, "v"),
    (0x0B, "b"),
    (0x0C, "q"),
    (0x0D, "w"),
    (0x0E, "e"),
    (0x0F, "r"),
    (0x10, "y"),
    (0x11, "t"),
    (0x12, "1"),
    (0x13, "2"),
    (0x14, "3"),
    (0x15, "4"),
    (0x16, "6"),
    (0x17, "5"),
    (0x18, "="),
    (0x19, "9"),
    (0x1A, "7"),
    (0x1B, "-"),
    (0x1C, "8"),
    (0x1D, "0"),
    (0x1E, "]"),
    (0x1F, "o"),
    (0x20, "u"),
    (0x21, "["),
    (0x22, "i"),
    (0x23, "p"),
    (0x24, "enter"),
    (0x25, "l"),
    (0x26, "j"),
    (0x27, "'"),
    (0x28, "k"),
    (0x29, ";"),
    (0x2A, "\\"),
    (0x2B, ","),
    (0x2C, "/"),
    (0x2D, "n"),
    (0x2E, "m"),
    (0x2F, "."),
    (0x30, "tab"),
    (0x31, "space"),
    (0x32, "`"),
    (0x33, "backspace"),
    (0x35, "esc"),
    (0x36, "right_win"),
    (0x37, "left_win"),
    (0x38, "left_shift"),
    (0x39, "caps_lock"),
    (0x3A, "left_alt"),
    (0x3B, "left_ctrl"),
    (0x3C, "right_shift"),
    (0x3D, "right_alt"),
    (0x3E, "right_ctrl"),
    (0x3F, "fn"),
    (0x40, "f17"),
    (0x4F, "f18"),
    (0x50, "f19"),
    (0x5A, "f20"),
    (0x60, "f5"),
    (0x61, "f6"),
    (0x62, "f7"),
    (0x63, "f3"),
    (0x64, "f8"),
    (0x65, "f9"),
    (0x67, "f11"),
    (0x69, "f13"),
    (0x6A, "f16"),
    (0x6B, "f14"),
    (0x6D, "f10"),
    (0x6F, "f12"),
    (0x71, "f15"),
    (0x72, "insert"),
    (0x73, "home"),
    (0x74, "page_up"),
    (0x75, "delete"),
    (0x76, "f4"),
    (0x77, "end"),
    (0x78, "f2"),
    (0x79, "page_down"),
    (0x7A, "f1"),
    (0x7B, "arrow_left"),
    (0x7C, "arrow_right"),
    (0x7D, "arrow_down"),
    (0x7E, "arrow_up"),
}

pub fn key_for_keycode(code: i64) -> Option<Key> {
    let index = usize::try_from(code).ok()?;
    key_map().get(index)?.clone()
}

static KEY_BY_CODE: OnceLock<[Option<Key>; 128]> = OnceLock::new();

fn key_map() -> &'static [Option<Key>; 128] {
    KEY_BY_CODE.get_or_init(|| {
        std::array::from_fn(|index| {
            KEYCODES
                .iter()
                .find(|(code, _)| usize::from(*code) == index)
                .and_then(|(_, name)| Key::new(name).ok())
        })
    })
}

pub(super) fn prewarm_key_map() {
    let _ = key_map();
}

/// Modifier state extracted from a `FlagsChanged` event, since macOS reports
/// modifiers as a bitmask rather than as key up/down.
pub fn modifier_is_down(flags: u64, key: &Key) -> bool {
    // Device-dependent bits distinguish left from right.
    const LSHIFT: u64 = 0x0000_0002;
    const RSHIFT: u64 = 0x0000_0004;
    const LCTRL: u64 = 0x0000_0001;
    const RCTRL: u64 = 0x0000_2000;
    const LALT: u64 = 0x0000_0020;
    const RALT: u64 = 0x0000_0040;
    const LCMD: u64 = 0x0000_0008;
    const RCMD: u64 = 0x0000_0010;
    // Device-independent CGEventFlagMask values.
    const CAPS_LOCK: u64 = 0x0001_0000;
    const FN: u64 = 0x0080_0000;

    let mask = match key.as_str() {
        "left_shift" => LSHIFT,
        "right_shift" => RSHIFT,
        "left_ctrl" => LCTRL,
        "right_ctrl" => RCTRL,
        "left_alt" => LALT,
        "right_alt" => RALT,
        "left_win" => LCMD,
        "right_win" => RCMD,
        "caps_lock" => CAPS_LOCK,
        "fn" => FN,
        _ => return false,
    };
    flags & mask != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_uses_dragged_event_for_the_held_button() {
        assert_eq!(movement_event(0).event_type, CGEventType::MouseMoved as u32);
        assert_eq!(
            movement_event(button_mask(MouseButton::Left)).event_type,
            CGEventType::LeftMouseDragged as u32
        );
        assert_eq!(
            movement_event(button_mask(MouseButton::Right)).event_type,
            CGEventType::RightMouseDragged as u32
        );
        assert_eq!(
            movement_event(button_mask(MouseButton::Middle)).event_type,
            CGEventType::OtherMouseDragged as u32
        );
    }

    #[test]
    fn movement_uses_a_stable_button_priority_when_several_are_held() {
        let held = button_mask(MouseButton::Right) | button_mask(MouseButton::Left);
        assert_eq!(
            movement_event(held),
            drag_event(MouseButton::Left),
            "one CGEvent can represent only one drag button"
        );
    }

    #[test]
    fn extra_buttons_keep_their_native_button_numbers() {
        assert_eq!(button_event(MouseButton::X1, true).button, 3);
        assert_eq!(button_event(MouseButton::X2, false).button, 4);
        assert_eq!(
            drag_event(MouseButton::X1).event_type,
            CGEventType::OtherMouseDragged as u32
        );
    }

    #[test]
    fn double_click_uses_native_click_counts_in_order() {
        let tracker = ClickTracker::new(std::time::Duration::from_millis(500));
        let plan = tracker.prepare(
            MouseButton::Left,
            ButtonAction::DoubleClick,
            Point::new(10.0, 20.0),
            Instant::now(),
        );
        let (steps, count) = button_steps(ButtonAction::DoubleClick, plan.counts());
        assert_eq!(count, 4);
        assert_eq!(
            &steps[..count],
            &[
                ButtonStep {
                    down: true,
                    click_count: 1,
                },
                ButtonStep {
                    down: false,
                    click_count: 1,
                },
                ButtonStep {
                    down: true,
                    click_count: 2,
                },
                ButtonStep {
                    down: false,
                    click_count: 2,
                },
            ]
        );
    }

    #[test]
    fn keycode_mapping_round_trips() {
        for (code, name) in KEYCODES {
            let key = Key::new(name).unwrap();
            assert_eq!(keycode_for(&key), Some(*code), "{name}");
            assert_eq!(key_for_keycode(*code as i64).as_ref(), Some(&key), "{name}");
        }
    }

    #[test]
    fn unknown_keycodes_are_rejected() {
        assert!(key_for_keycode(0xFFFF).is_none());
        assert!(keycode_for(&Key::new("nonexistent").unwrap()).is_none());
    }

    #[test]
    fn modifier_flags_distinguish_left_from_right() {
        let left = Key::new("left_shift").unwrap();
        let right = Key::new("right_shift").unwrap();
        assert!(modifier_is_down(0x02, &left));
        assert!(!modifier_is_down(0x02, &right));
        assert!(modifier_is_down(0x04, &right));
        assert!(modifier_is_down(
            0x0001_0000,
            &Key::new("caps_lock").unwrap()
        ));
        assert!(modifier_is_down(0x0080_0000, &Key::new("fn").unwrap()));
    }
}
