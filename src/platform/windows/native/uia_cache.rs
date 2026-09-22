//! Borrowed, value-returning UIA cache access.
/// Borrowed cached UIA properties. No provider refresh or Rust pointer escapes;
/// a missing cached property is a normal HRESULT, handled by the scanner.
pub(crate) struct CachedElement<'a>(&'a windows::Win32::UI::Accessibility::IUIAutomationElement);
impl<'a> CachedElement<'a> {
    pub(crate) fn new(
        element: &'a windows::Win32::UI::Accessibility::IUIAutomationElement,
    ) -> Self {
        Self(element)
    }
    pub(crate) fn control_type(
        &self,
    ) -> windows::core::Result<windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID> {
        // SAFETY: borrowed live COM interface; cached getter retains no Rust data.
        unsafe { self.0.CachedControlType() }
    }
    pub(crate) fn is_offscreen(&self) -> windows::core::Result<windows::core::BOOL> {
        // SAFETY: borrowed live COM interface; cached getter returns an owned scalar.
        unsafe { self.0.CachedIsOffscreen() }
    }
    pub(crate) fn is_enabled(&self) -> windows::core::Result<windows::core::BOOL> {
        // SAFETY: borrowed live COM interface; cached getter returns an owned scalar.
        unsafe { self.0.CachedIsEnabled() }
    }
    pub(crate) fn bounds(&self) -> windows::core::Result<windows::Win32::Foundation::RECT> {
        // SAFETY: borrowed live COM interface; rectangle is returned by value.
        unsafe { self.0.CachedBoundingRectangle() }
    }
    pub(crate) fn name(&self) -> windows::core::Result<windows::core::BSTR> {
        // SAFETY: the returned BSTR owns its buffer independently of this borrow.
        unsafe { self.0.CachedName() }
    }
    pub(crate) fn is_focusable(&self) -> windows::core::Result<windows::core::BOOL> {
        // SAFETY: borrowed live COM interface; cached getter returns an owned scalar.
        unsafe { self.0.CachedIsKeyboardFocusable() }
    }
}
