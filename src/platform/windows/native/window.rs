//! Owned application windows, scoped to their creating thread.
use super::{NativeDimensions, current_module};
use crate::api::Rect;
use std::{marker::PhantomData, rc::Rc};
use windows::Win32::Foundation::HWND;

/// A window created by KeySteer and destroyed on its owner thread.
#[repr(transparent)]
pub(crate) struct OwnedWindow {
    raw: HWND,
    _thread: PhantomData<Rc<()>>,
}

pub(crate) enum OwnedWindowSpec {
    CpuOverlay(Rect),
    GpuOverlay(Rect),
    Status,
}

/// Create and immediately own one of KeySteer's fixed native window kinds.
/// Keeping class names, styles and failure cleanup here prevents a safe caller
/// from accidentally adopting an arbitrary borrowed HWND.
pub(crate) fn create_owned_window(spec: OwnedWindowSpec) -> Result<OwnedWindow, String> {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, LWA_ALPHA, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes,
        ShowWindow, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_POPUP,
    };
    use windows::core::w;

    let instance = current_module().map_err(|error| format!("GetModuleHandleW failed: {error}"))?;
    let (extended_style, class_name, area, enable_gpu_alpha, show_immediately, sized) = match spec {
        OwnedWindowSpec::CpuOverlay(area) => (
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("KeySteerOverlay"),
            area,
            false,
            true,
            true,
        ),
        OwnedWindowSpec::GpuOverlay(area) => (
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("KeySteerGpuOverlay"),
            area,
            true,
            false,
            true,
        ),
        OwnedWindowSpec::Status => (
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("KeySteerStatusWindow"),
            Rect::new(0.0, 0.0, 0.0, 0.0),
            false,
            false,
            false,
        ),
    };
    let dimensions = sized
        .then(|| NativeDimensions::from_f64(area.width, area.height))
        .transpose()?;
    let (width, height) = dimensions
        .map(|dimensions| (dimensions.width_i32(), dimensions.height_i32()))
        .unwrap_or((0, 0));
    // SAFETY: every class and title is a process-lifetime static string. The
    // returned HWND transfers directly into `OwnedWindow`; GPU setup failure
    // destroys it before returning, and immediate show does not retain data.
    unsafe {
        let hwnd = CreateWindowExW(
            extended_style,
            class_name,
            w!("KeySteer"),
            WS_POPUP,
            area.x.round() as i32,
            area.y.round() as i32,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|error| format!("CreateWindowExW failed: {error}"))?;
        if enable_gpu_alpha
            && let Err(error) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)
        {
            return match DestroyWindow(hwnd) {
                Ok(()) => Err(format!("SetLayeredWindowAttributes failed: {error}")),
                Err(cleanup) => Err(format!(
                    "SetLayeredWindowAttributes failed: {error}; cannot destroy failed window: {cleanup}"
                )),
            };
        }
        if show_immediately {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(OwnedWindow::new(hwnd))
    }
}

pub(crate) fn reposition_owned_window(window: &OwnedWindow, area: Rect) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_TOPMOST, SWP_NOACTIVATE, SetWindowPos};

    let dimensions = NativeDimensions::from_f64(area.width, area.height)?;
    // SAFETY: `window` proves that KeySteer owns a live HWND for this
    // synchronous call; validated dimensions fit the Win32 coordinate types.
    unsafe {
        SetWindowPos(
            window.raw(),
            Some(HWND_TOPMOST),
            area.x.round() as i32,
            area.y.round() as i32,
            dimensions.width_i32(),
            dimensions.height_i32(),
            SWP_NOACTIVATE,
        )
    }
    .map_err(|error| format!("SetWindowPos failed: {error}"))
}

fn destroy_owned_window(hwnd: HWND) -> windows::core::Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

    // SAFETY: callers transfer a KeySteer-owned HWND on its creating thread;
    // the function consumes the final native ownership edge exactly once.
    unsafe { DestroyWindow(hwnd) }
}

impl OwnedWindow {
    #[inline(always)]
    fn new(hwnd: HWND) -> Self {
        Self {
            raw: hwnd,
            _thread: PhantomData,
        }
    }

    #[inline(always)]
    pub(crate) fn raw(&self) -> HWND {
        self.raw
    }

    #[inline(always)]
    pub(crate) fn destroy(mut self) -> windows::core::Result<()> {
        let hwnd = std::mem::take(&mut self.raw);
        if hwnd.is_invalid() {
            return Ok(());
        }
        destroy_owned_window(hwnd)
    }
}

impl Drop for OwnedWindow {
    #[inline(always)]
    fn drop(&mut self) {
        if !self.raw.is_invalid()
            && let Err(error) = destroy_owned_window(self.raw)
        {
            crate::report_error!("windows-native", "DestroyWindow failed: {error}");
        }
    }
}
