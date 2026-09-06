//! On-demand, asynchronous Win32 window placement. No focus or input injection.
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowPlacement, GetWindowRect, SW_SHOWMAXIMIZED, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT,
    WPF_ASYNCWINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

use crate::api::command::WindowScreenTarget;
use crate::api::geometry::{Point, Rect, Screen};
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

fn maximized_relocation(
    mut placement: WINDOWPLACEMENT,
    source: &Screen,
    target: &Screen,
    workspace: bool,
) -> WINDOWPLACEMENT {
    let mut restored = rect(placement.rcNormalPosition);
    // Placement uses workspace coordinates except for tool windows.
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
    placement.flags = WPF_ASYNCWINDOWPLACEMENT;
    placement
}

fn maximized_frame(bounds: Rect, source: &Screen, target: &Screen) -> Rect {
    // GetWindowRect includes invisible resize borders extending past the work
    // area. Preserve those insets at the destination DPI, rather than fitting
    // the entire native rectangle inside the work area and exposing borders.
    let scale = target.scale / source.scale;
    let left = target.work_area.x + (bounds.x - source.work_area.x) * scale;
    let top = target.work_area.y + (bounds.y - source.work_area.y) * scale;
    let right = target.work_area.right() + (bounds.right() - source.work_area.right()) * scale;
    let bottom = target.work_area.bottom() + (bounds.bottom() - source.work_area.bottom()) * scale;
    Rect::new(left, top, right - left, bottom - top)
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
    let maximized = placement.showCmd == SW_SHOWMAXIMIZED.0 as u32;
    let mut bounds = RECT::default();
    // SAFETY: bounds is writable and hwnd is only borrowed for this query.
    unsafe { GetWindowRect(hwnd, &mut bounds) }
        .map_err(|error| format!("cannot read window bounds: {error}"))?;
    let mapped = if maximized {
        maximized_frame(rect(bounds), source, target)
    } else {
        map_between_screens(rect(bounds), source, target)
    };
    let mapped = native_rect(mapped);
    // SAFETY: no pointers are retained. NOACTIVATE/NOZORDER preserve focus;
    // ASYNC queues the operation to the owning UI thread without waiting.
    // This moves the actual maximized window without a restore/maximize cycle.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            mapped.left,
            mapped.top,
            mapped.right - mapped.left,
            mapped.bottom - mapped.top,
            SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER,
        )
    }
    .map_err(|error| format!("cannot move window: {error}"))?;
    let pointer = if maximized {
        let workspace =
            super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0;
        let placement = maximized_relocation(placement, source, target, workspace);
        // SAFETY: placement has the required length and retains the maximized
        // show state. Queue its restore rectangle after the actual frame move.
        unsafe { SetWindowPlacement(hwnd, &placement) }
            .map_err(|error| format!("cannot update maximized window restore position: {error}"))?;
        following_pointer(cursor, source.work_area, target.work_area, target.bounds)
    } else {
        // Match the integer coordinates actually submitted to SetWindowPos.
        following_pointer(cursor, rect(bounds), rect(mapped), target.bounds)
    };
    Ok(Some(pointer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximized_restore_position_moves_without_changing_show_state() {
        let source = Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 40.0, 1920.0, 1040.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        };
        let target = Screen {
            bounds: Rect::new(-1920.0, 100.0, 1920.0, 1080.0),
            work_area: Rect::new(-1920.0, 100.0, 1920.0, 1040.0),
            is_primary: false,
            ..source.clone()
        };
        for (workspace, expected_y) in [(true, 220.0), (false, 180.0)] {
            let placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                showCmd: SW_SHOWMAXIMIZED.0 as u32,
                rcNormalPosition: native_rect(Rect::new(120.0, 80.0, 800.0, 600.0)),
                ..Default::default()
            };
            let step = maximized_relocation(placement, &source, &target, workspace);
            assert_eq!(step.showCmd, placement.showCmd);
            assert_eq!(
                rect(step.rcNormalPosition),
                Rect::new(-1800.0, expected_y, 800.0, 600.0)
            );
            assert_eq!(step.flags, WPF_ASYNCWINDOWPLACEMENT);
            assert_eq!(step.length, placement.length);
        }
    }

    #[test]
    fn maximized_frame_preserves_invisible_borders_on_a_different_dpi_monitor() {
        let source = Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 40.0, 1920.0, 1040.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        };
        let target = Screen {
            bounds: Rect::new(-2560.0, 0.0, 2560.0, 1440.0),
            work_area: Rect::new(-2560.0, 0.0, 2560.0, 1380.0),
            is_primary: false,
            scale: 1.5,
            name: None,
        };
        assert_eq!(
            maximized_frame(Rect::new(-8.0, 32.0, 1936.0, 1056.0), &source, &target),
            Rect::new(-2572.0, -12.0, 2584.0, 1404.0),
        );
    }
}
