//! Display snapshots and event-driven reconfiguration notices.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use core_graphics::base::kCGErrorSuccess;
use core_graphics::display::{
    CGDirectDisplayID, CGDisplay, CGDisplayRegisterReconfigurationCallback,
    CGDisplayRemoveReconfigurationCallback,
};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

use crate::api::geometry::{Rect, Screen};

pub struct DisplayWatcher {
    registered: bool,
}

static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

impl DisplayWatcher {
    pub fn new() -> Result<Self, String> {
        // SAFETY: the callback ABI matches CoreGraphics. It uses no userdata
        // and only touches a process-lifetime atomic plus the main run loop.
        let result =
            unsafe { CGDisplayRegisterReconfigurationCallback(display_changed, std::ptr::null()) };
        if result != kCGErrorSuccess {
            return Err(format!(
                "cannot register display reconfiguration callback: {result:?}"
            ));
        }
        Ok(Self { registered: true })
    }

    pub fn take_changed(&self) -> bool {
        DISPLAY_CHANGED.swap(false, Ordering::AcqRel)
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if !self.registered {
            return Ok(());
        }
        // SAFETY: this is the exact callback/null-userdata pair registered by
        // `new`; the callback owns no dynamically freed state.
        let result =
            unsafe { CGDisplayRemoveReconfigurationCallback(display_changed, std::ptr::null()) };
        if result != kCGErrorSuccess {
            return Err(format!(
                "cannot remove display reconfiguration callback: {result:?}"
            ));
        }
        self.registered = false;
        Ok(())
    }
}

impl Drop for DisplayWatcher {
    fn drop(&mut self) {
        if self.registered {
            let _ = self.stop();
        }
    }
}

unsafe extern "C" fn display_changed(
    _display: CGDirectDisplayID,
    _flags: u32,
    user_info: *const c_void,
) {
    let _ = user_info;
    DISPLAY_CHANGED.store(true, Ordering::Release);
    super::workspace::wake_main_run_loop();
}

pub fn list_screens() -> Result<Vec<Screen>, String> {
    if let Some(mtm) = MainThreadMarker::new() {
        let screens = NSScreen::screens(mtm);
        if !screens.is_empty() {
            let main_height = screens.objectAtIndex(0).frame().size.height;
            return Ok(screens
                .iter()
                .enumerate()
                .map(|(index, screen)| {
                    let frame = screen.frame();
                    let visible = screen.visibleFrame();
                    Screen {
                        bounds: appkit_rect(frame, main_height),
                        work_area: appkit_rect(visible, main_height),
                        is_primary: index == 0,
                        scale: screen.backingScaleFactor(),
                        name: Some(screen.localizedName().to_string()),
                    }
                })
                .collect());
        }
    }

    list_core_graphics_screens()
}

fn appkit_rect(rect: objc2_foundation::NSRect, main_height: f64) -> Rect {
    Rect::new(
        rect.origin.x,
        main_height - rect.origin.y - rect.size.height,
        rect.size.width,
        rect.size.height,
    )
}

fn list_core_graphics_screens() -> Result<Vec<Screen>, String> {
    let displays = CGDisplay::active_displays()
        .map_err(|error| format!("cannot enumerate displays: CoreGraphics error {error:?}"))?;
    let main = CGDisplay::main().id;
    Ok(displays
        .into_iter()
        .map(|id| {
            let display = CGDisplay::new(id);
            let bounds = display.bounds();
            let scale = display
                .display_mode()
                .map(|mode| mode.pixel_width() as f64 / bounds.size.width.max(1.0))
                .unwrap_or(1.0);
            let bounds = Rect::new(
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            );
            Screen {
                bounds,
                work_area: bounds,
                is_primary: id == main,
                scale,
                name: Some(format!("Display {id}")),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appkit_coordinates_convert_to_core_graphics_space() {
        let rect = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(-1280.0, 100.0),
            objc2_foundation::NSSize::new(1280.0, 900.0),
        );
        assert_eq!(
            appkit_rect(rect, 1080.0),
            Rect::new(-1280.0, 80.0, 1280.0, 900.0)
        );
    }
}
