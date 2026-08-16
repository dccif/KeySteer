//! Display enumeration via `EnumDisplayMonitors`, DPI-aware.

use std::cell::RefCell;

use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MONITORINFOEXW, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

use crate::api::geometry::{Rect, Screen};

/// Opt into per-monitor DPI awareness.
///
/// Without this, Windows lies about coordinates on scaled displays and every
/// overlay lands in the wrong place. Must run before any window is created.
pub fn enable_dpi_awareness() {
    // SAFETY: no arguments to validate; failure only means the process was
    // already marked aware, which is fine.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn rect_to_api(r: RECT) -> Rect {
    let width = i64::from(r.right) - i64::from(r.left);
    let height = i64::from(r.bottom) - i64::from(r.top);
    Rect::new(r.left as f64, r.top as f64, width as f64, height as f64)
}

fn monitor_dpi(monitor: HMONITOR, dpi_x: &mut u32, dpi_y: &mut u32) -> windows::core::Result<()> {
    // SAFETY: callers retain the live monitor handle and both output pointers
    // refer to initialized writable integers for this synchronous query.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, dpi_x, dpi_y) }
}

thread_local! {
    static SCREENS: RefCell<Vec<Screen>> = const { RefCell::new(Vec::new()) };
}

pub fn list_screens() -> Result<Vec<Screen>, String> {
    SCREENS.with_borrow_mut(Vec::clear);

    // SAFETY: the callback matches the expected signature and the pointer we
    // pass stays valid for the duration of the call.
    unsafe { EnumDisplayMonitors(None, None, Some(enum_callback), LPARAM(0)) }
        .ok()
        .map_err(|e| format!("EnumDisplayMonitors failed: {e}"))?;

    Ok(SCREENS.with_borrow_mut(std::mem::take))
}

extern "system" fn enum_callback(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    _data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    // SAFETY: `info` is correctly sized via cbSize.
    let ok = unsafe {
        GetMonitorInfoW(
            monitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    };
    if !ok.as_bool() {
        // Skip this monitor but keep enumerating the rest.
        crate::report_warning!(
            "windows-screen",
            "GetMonitorInfoW failed for monitor {monitor:?}"
        );
        return TRUE;
    }

    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    if let Err(error) = monitor_dpi(monitor, &mut dpi_x, &mut dpi_y) {
        crate::report_warning!(
            "windows-screen",
            "GetDpiForMonitor failed; using 96 DPI: {error}"
        );
    }

    SCREENS.with_borrow_mut(|screens| {
        screens.push(Screen {
            bounds: rect_to_api(info.monitorInfo.rcMonitor),
            work_area: rect_to_api(info.monitorInfo.rcWork),
            is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            scale: dpi_x as f64 / 96.0,
            name: Some(
                String::from_utf16_lossy(&info.szDevice)
                    .trim_end_matches('\0')
                    .to_string(),
            ),
        })
    });
    TRUE
}

/// DPI scale of the primary monitor.
///
/// Useful as a fallback before [`list_screens`] has succeeded, since overlay
/// sizing needs a scale factor even then.
#[allow(dead_code)]
pub fn primary_scale() -> f64 {
    // SAFETY: MonitorFromPoint with the primary fallback always yields a
    // monitor handle.
    let monitor = unsafe { MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY) };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    if let Err(error) = monitor_dpi(monitor, &mut dpi_x, &mut dpi_y) {
        crate::report_warning!(
            "windows-screen",
            "primary GetDpiForMonitor failed; using 96 DPI: {error}"
        );
    }
    dpi_x as f64 / 96.0
}
