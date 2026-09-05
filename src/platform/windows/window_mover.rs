//! On-demand, asynchronous Win32 window placement. No focus or input injection.
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowPlacement, GetWindowRect, SW_SHOWMAXIMIZED, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT,
    WPF_ASYNCWINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

use crate::api::command::WindowScreenTarget;
use crate::api::geometry::{Point, Rect};
use crate::platform::common::window_placement::{
    destination, following_pointer, map_between_screens,
};

fn rect(value: RECT) -> Rect {
    Rect::new(
        value.left as f64,
        value.top as f64,
        f64::from(value.right) - f64::from(value.left),
        f64::from(value.bottom) - f64::from(value.top),
    )
}

fn native_rect(value: Rect) -> RECT {
    RECT {
        left: value.x.round() as i32,
        top: value.y.round() as i32,
        right: value.right().round() as i32,
        bottom: value.bottom().round() as i32,
    }
}

pub(super) fn move_to_screen(target: WindowScreenTarget) -> Result<Option<Point>, String> {
    let cursor = super::input::cursor_position()?;
    let Some((hwnd, _, visible)) = super::accessibility::movable_window_under_pointer(cursor)?
    else {
        return Ok(None);
    };
    let screens = super::screens::list_screens()?;
    let Some((source, target)) = destination(&screens, visible, target) else {
        return Ok(None);
    };
    let source = &screens[source];
    let target = &screens[target];
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: hwnd is a borrowed OS handle; placement is initialized with the
    // required length and lives throughout the synchronous output query.
    unsafe { GetWindowPlacement(hwnd, &mut placement) }
        .map_err(|error| format!("cannot read window placement: {error}"))?;
    let pointer = if placement.showCmd == SW_SHOWMAXIMIZED.0 as u32 {
        let workspace =
            super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0;
        let mut restored = rect(placement.rcNormalPosition);
        // WINDOWPLACEMENT uses workspace coordinates for ordinary top-level
        // windows, unlike GetWindowRect/SetWindowPos (screen coordinates).
        if workspace {
            restored.x += source.work_area.x - source.bounds.x;
            restored.y += source.work_area.y - source.bounds.y;
        }
        let mut mapped = map_between_screens(restored, source, target);
        if workspace {
            mapped.x -= target.work_area.x - target.bounds.x;
            mapped.y -= target.work_area.y - target.bounds.y;
        }
        placement.rcNormalPosition = native_rect(mapped);
        placement.ptMaxPosition = POINT { x: -1, y: -1 };
        placement.flags |= WPF_ASYNCWINDOWPLACEMENT;
        // SAFETY: the initialized placement retains the original show state;
        // ASYNC prevents an unresponsive foreign UI thread blocking the engine.
        unsafe { SetWindowPlacement(hwnd, &placement) }
            .map_err(|error| format!("cannot move maximized window: {error}"))?;
        following_pointer(cursor, visible, target.work_area, target.bounds)
    } else {
        let mut bounds = RECT::default();
        // SAFETY: bounds is writable and hwnd is only borrowed for this query.
        unsafe { GetWindowRect(hwnd, &mut bounds) }
            .map_err(|error| format!("cannot read window bounds: {error}"))?;
        let mapped = map_between_screens(rect(bounds), source, target);
        // SAFETY: no pointers are retained. NOACTIVATE/NOZORDER preserve focus;
        // ASYNC queues the operation to the owning UI thread without waiting.
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                mapped.x.round() as i32,
                mapped.y.round() as i32,
                mapped.width.round() as i32,
                mapped.height.round() as i32,
                SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
        .map_err(|error| format!("cannot move window: {error}"))?;
        // Match the integer coordinates actually submitted to SetWindowPos.
        following_pointer(
            cursor,
            rect(bounds),
            rect(native_rect(mapped)),
            target.bounds,
        )
    };
    Ok(Some(pointer))
}
