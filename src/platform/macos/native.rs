//! Zero-allocation ownership primitives shared by macOS native services.

use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPort;
use core_foundation::runloop::{CFRunLoopMode, kCFRunLoopDefaultMode};
use core_graphics::display::{
    CGDisplayRegisterReconfigurationCallback, CGDisplayRemoveReconfigurationCallback,
};
use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoopMode};

pub(crate) fn event_character(
    event: &core_graphics::event::CGEvent,
    demand: Option<&crate::platform::common::character_candidates::CharacterDemand>,
) -> Option<char> {
    let raw: &core_graphics::event::CGEventRef = event.as_ref();
    let raw =
        (raw as *const core_graphics::event::CGEventRef).cast::<objc2_core_graphics::CGEvent>();
    let mut units = [0u16; 8];
    let mut count = 0;
    // SAFETY: both crates wrap the same CGEventRef. The borrowed event remains
    // live during this read; buffers and output length have the required size.
    unsafe {
        objc2_core_graphics::CGEvent::keyboard_get_unicode_string(
            Some(&*raw),
            units.len() as u64,
            &mut count,
            units.as_mut_ptr(),
        );
    }
    if count > units.len() as u64 {
        return None;
    }
    let units = &units[..count as usize];
    demand.map_or_else(
        || crate::api::input::single_printable_character(units),
        |demand| demand.decode(units),
    )
}

/// Process-lifetime run-loop modes exported by Core Foundation/Foundation.
pub(crate) struct RunLoopModes {
    pub(crate) core_foundation: CFRunLoopMode,
    pub(crate) foundation: &'static NSRunLoopMode,
}

/// Keep the imported statics behind one reviewed native boundary.
pub(crate) fn default_run_loop_modes() -> RunLoopModes {
    // SAFETY: both frameworks export process-lifetime immutable mode objects.
    // Callers only borrow them for synchronous run-loop API calls.
    unsafe {
        RunLoopModes {
            core_foundation: kCFRunLoopDefaultMode,
            foundation: NSDefaultRunLoopMode,
        }
    }
}

pub(crate) fn register_display_reconfiguration_callback() -> i32 {
    // SAFETY: this callback has the exact CoreGraphics ABI, uses null userdata,
    // and touches only process-lifetime atomics plus the main run loop.
    unsafe {
        CGDisplayRegisterReconfigurationCallback(super::screens::display_changed, std::ptr::null())
    }
}

pub(crate) fn remove_display_reconfiguration_callback() -> i32 {
    // SAFETY: this is the exact callback/null-userdata pair installed above;
    // the callback owns no dynamically freed state.
    unsafe {
        CGDisplayRemoveReconfigurationCallback(super::screens::display_changed, std::ptr::null())
    }
}

pub(crate) fn event_tap_identity(tap: &CFMachPort) -> usize {
    tap.as_concrete_TypeRef() as usize
}

extern "C-unwind" fn event_tap_invalidated(
    port: *mut objc2_core_foundation::CFMachPort,
    _info: *mut c_void,
) {
    super::hook::event_tap_invalidated(port as usize);
}

fn update_event_tap_invalidation_callback(tap: &CFMachPort, install: bool) -> Result<(), String> {
    let raw = tap.as_concrete_TypeRef();
    // SAFETY: both crates wrap the same documented CFMachPortRef. `raw` comes
    // from the live CGEventTap owner and the typed reference cannot outlive
    // this synchronous function. The installed function has Core Foundation's
    // exact ABI, ignores the private event-tap `info` pointer and finds its
    // Arc-owned state by port identity. Teardown clears it before invalidation.
    unsafe {
        let Some(tap) = raw.cast::<objc2_core_foundation::CFMachPort>().as_ref() else {
            return Err("the macOS event-tap Mach port is unavailable".to_string());
        };
        if install && tap.invalidation_call_back().is_some() {
            return Err(
                "the macOS event-tap Mach port already has an invalidation callback".to_string(),
            );
        }
        let callback: objc2_core_foundation::CFMachPortInvalidationCallBack =
            install.then_some(event_tap_invalidated);
        tap.set_invalidation_call_back(callback);
    }
    Ok(())
}

pub(crate) fn install_event_tap_invalidation_callback(tap: &CFMachPort) -> Result<(), String> {
    update_event_tap_invalidation_callback(tap, true)
}

pub(crate) fn clear_event_tap_invalidation_callback(tap: &CFMachPort) {
    let _ = update_event_tap_invalidation_callback(tap, false);
}

/// A Core Foundation object returned at +1 by a Create/Copy function.
///
/// This is pointer-sized and performs the same single `CFRelease` that callers
/// would otherwise issue manually. It does not retain, clone or allocate.
#[repr(transparent)]
pub(crate) struct OwnedCf(*const c_void);

impl OwnedCf {
    /// Take ownership of a pointer returned under the Create/Copy rule.
    ///
    /// # Safety
    /// `value` must be either null or a live +1 Core Foundation object.
    #[inline(always)]
    pub(crate) unsafe fn from_create_rule(value: *const c_void) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }

    #[inline(always)]
    pub(crate) fn as_ptr(&self) -> *const c_void {
        self.0
    }

    /// Transfer the +1 reference into another typed create-rule wrapper.
    #[inline(always)]
    pub(crate) fn into_raw(self) -> *const c_void {
        let value = self.0;
        std::mem::forget(self);
        value
    }
}

impl Drop for OwnedCf {
    #[inline(always)]
    fn drop(&mut self) {
        // SAFETY: construction requires one owned Create/Copy reference and
        // this non-Clone wrapper has exactly one Drop path.
        unsafe { core_foundation::base::CFRelease(self.0) };
    }
}
