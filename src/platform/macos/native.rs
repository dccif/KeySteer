//! Zero-allocation ownership primitives shared by macOS native services.

use std::ffi::c_void;

use core_foundation::runloop::{CFRunLoopMode, kCFRunLoopDefaultMode};
use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoopMode};

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
