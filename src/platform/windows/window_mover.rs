//! On-demand, asynchronous Win32 window placement. No focus or input injection.
use windows::Win32::Foundation::{HWND, POINT, RECT};
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

pub(super) fn read_placement(hwnd: HWND) -> Result<WINDOWPLACEMENT, String> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: correctly initialized writable structure; HWND is only borrowed.
    unsafe { GetWindowPlacement(hwnd, &mut placement) }
        .map_err(|error| format!("cannot read window placement: {error}"))?;
    Ok(placement)
}

pub(super) fn read_bounds(hwnd: HWND) -> Result<Rect, String> {
    let mut bounds = RECT::default();
    // SAFETY: writable out-buffer, no pointers retained by the query.
    unsafe { GetWindowRect(hwnd, &mut bounds) }
        .map_err(|error| format!("cannot read window bounds: {error}"))?;
    Ok(rect(bounds))
}

pub(super) fn submit_frame(hwnd: HWND, bounds: Rect) -> Result<Rect, String> {
    if ![
        bounds.x,
        bounds.y,
        bounds.right(),
        bounds.bottom(),
        bounds.width,
        bounds.height,
    ]
    .iter()
    .all(|v| v.is_finite() && v.abs() < i32::MAX as f64 / 2.0)
        || bounds.width < 1.0
        || bounds.height < 1.0
    {
        return Err("invalid native window geometry".into());
    }
    let mapped = native_rect(bounds);
    // SAFETY: finite validated i32 coordinates; no Rust data is retained.
    // ASYNC avoids foreign UI-thread waits; preserve focus and Z-order.
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
    Ok(rect(mapped))
}

pub(super) fn submit_placement(hwnd: HWND, placement: &WINDOWPLACEMENT) -> Result<(), String> {
    let mut placement = *placement;
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    placement.flags |= WPF_ASYNCWINDOWPLACEMENT;
    // SAFETY: initialized, correctly sized structure copied by the asynchronous
    // placement request; the HWND is borrowed and no Rust pointer is retained.
    unsafe { SetWindowPlacement(hwnd, &placement) }
        .map_err(|error| format!("cannot update window placement: {error}"))
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
    let placement = read_placement(hwnd)?;
    let maximized = placement.showCmd == SW_SHOWMAXIMIZED.0 as u32;
    let bounds = read_bounds(hwnd)?;
    let mapped = if maximized {
        maximized_frame(bounds, source, target)
    } else {
        map_between_screens(bounds, source, target)
    };
    let mapped = submit_frame(hwnd, mapped)?;
    let pointer = if maximized {
        let workspace =
            super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0;
        let placement = maximized_relocation(placement, source, target, workspace);
        submit_placement(hwnd, &placement)?;
        following_pointer(cursor, source.work_area, target.work_area, target.bounds)
    } else {
        // Match the integer coordinates actually submitted to SetWindowPos.
        following_pointer(cursor, bounds, mapped, target.bounds)
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
