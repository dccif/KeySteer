//! Zero-allocation ownership primitives shared by macOS native services.

use std::ffi::c_void;

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

    #[inline(always)]
    pub(crate) fn as_mut_ptr(&self) -> *mut c_void {
        self.0.cast_mut()
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
