//! Thread-bound GDI font ownership and scoped selection.
use super::*;
pub(crate) struct OwnedFont {
    raw: HFONT,
    _thread: PhantomData<Rc<()>>,
}

impl OwnedFont {
    pub(crate) fn new(family: &str, pixel_height: i32, bold: bool) -> Result<Self, String> {
        let family: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `family` is NUL-terminated and remains live for the complete
        // CreateFontW call; all numeric parameters are initialized values.
        let font = unsafe {
            CreateFontW(
                -pixel_height,
                0,
                0,
                0,
                if bold {
                    FW_BOLD.0 as i32
                } else {
                    FW_NORMAL.0 as i32
                },
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                FF_DONTCARE.0 as u32,
                windows::core::PCWSTR(family.as_ptr()),
            )
        };
        if font.is_invalid() {
            Err("CreateFontW failed".into())
        } else {
            Ok(Self {
                raw: font,
                _thread: PhantomData,
            })
        }
    }
}

impl Drop for OwnedFont {
    fn drop(&mut self) {
        // SAFETY: this HFONT came from CreateFontW and this guard is its sole
        // owner, so DeleteObject is called exactly once.
        if !unsafe { DeleteObject(self.raw.into()) }.as_bool() {
            crate::support::logging::report_error(
                "windows-native",
                "DeleteObject(font) failed during drop",
            );
        }
    }
}

/// Restores the previously selected GDI object when the guard leaves scope.
pub(crate) struct SelectedObject<'dc> {
    dc: HDC,
    previous: HGDIOBJ,
    _dc: PhantomData<&'dc mut GdiDibSurface>,
    _font: PhantomData<&'dc OwnedFont>,
    _thread: PhantomData<Rc<()>>,
}

impl GdiDibSurface {
    #[inline(always)]
    pub(crate) fn select_font<'a>(
        &'a mut self,
        font: &'a OwnedFont,
    ) -> Result<SelectedObject<'a>, String> {
        use windows::Win32::Graphics::Gdi::SelectObject;

        // SAFETY: both handles are live for the guard lifetime. Drop restores
        // the exact object returned by this call.
        let previous = unsafe { SelectObject(self.memory, font.raw.into()) };
        if previous.0.is_null() || previous.0 as usize == usize::MAX {
            Err("SelectObject failed".into())
        } else {
            Ok(SelectedObject {
                dc: self.memory,
                previous,
                _dc: PhantomData,
                _font: PhantomData,
                _thread: PhantomData,
            })
        }
    }
}

impl SelectedObject<'_> {
    pub(crate) fn dc(&self) -> HDC {
        self.dc
    }
}

impl Drop for SelectedObject<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::SelectObject;

        // SAFETY: `previous` came from selecting into this same live DC.
        let restored = unsafe { SelectObject(self.dc, self.previous) };
        if restored.0.is_null() || restored.0 as usize == usize::MAX {
            crate::report_error!("windows-native", "cannot restore selected GDI object");
        }
    }
}
