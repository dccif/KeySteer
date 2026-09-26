//! Thread-bound GDI font ownership and scoped selection.
use super::NativeDimensions;
use std::{marker::PhantomData, ptr::NonNull, rc::Rc};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject,
    FF_DONTCARE, FW_BOLD, FW_NORMAL, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
};
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

#[must_use = "the desktop DC must be released on its acquiring thread"]
pub(crate) struct ScreenDc(HDC, PhantomData<Rc<()>>);

impl ScreenDc {
    pub(crate) fn acquire() -> Result<Self, String> {
        use windows::Win32::Graphics::Gdi::GetDC;

        // SAFETY: a null HWND requests the desktop DC. This guard balances the
        // successful acquisition on the same visual worker thread.
        let dc = unsafe { GetDC(None) };
        if dc.is_invalid() {
            Err("GetDC failed for visual capture".into())
        } else {
            Ok(Self(dc, PhantomData))
        }
    }

    pub(crate) fn raw(&self) -> HDC {
        self.0
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::ReleaseDC;

        // SAFETY: this is the exact desktop DC acquired by `ScreenDc::acquire`.
        if unsafe { ReleaseDC(None, self.0) } == 0 {
            crate::support::logging::report_error(
                "windows-native",
                "ReleaseDC failed for visual capture",
            );
        }
    }
}

#[must_use = "the selected GDI bitmap and memory DC must be restored and released"]
pub(crate) struct GdiDibSurface {
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: NonNull<u8>,
    dimensions: NativeDimensions,
    _thread: PhantomData<Rc<()>>,
}

impl GdiDibSurface {
    pub(crate) fn new(
        reference: Option<HDC>,
        dimensions: NativeDimensions,
    ) -> Result<Self, String> {
        use windows::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, SelectObject,
        };

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: dimensions.width_i32(),
                biHeight: -dimensions.height_i32(),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut raw_bits = std::ptr::null_mut();
        // SAFETY: all GDI objects created here either transfer into the guard
        // or are destroyed before returning an error. The selected top-down
        // DIB remains selected until Drop restores `previous`.
        unsafe {
            let memory = CreateCompatibleDC(reference);
            if memory.is_invalid() {
                return Err("CreateCompatibleDC failed".into());
            }
            let bitmap = match CreateDIBSection(
                reference.or(Some(memory)),
                &info,
                DIB_RGB_COLORS,
                &mut raw_bits,
                None,
                0,
            ) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    if !DeleteDC(memory).as_bool() {
                        crate::support::logging::report_error(
                            "windows-native",
                            "cannot delete failed GDI capture DC",
                        );
                    }
                    return Err(format!("CreateDIBSection failed: {error}"));
                }
            };
            let Some(bits) = NonNull::new(raw_bits.cast::<u8>()) else {
                if !DeleteObject(HGDIOBJ(bitmap.0)).as_bool() {
                    crate::support::logging::report_error(
                        "windows-native",
                        "cannot delete null-buffer GDI capture bitmap",
                    );
                }
                if !DeleteDC(memory).as_bool() {
                    crate::support::logging::report_error(
                        "windows-native",
                        "cannot delete null-buffer GDI capture DC",
                    );
                }
                return Err("CreateDIBSection returned a null pixel buffer".into());
            };
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            if previous.0.is_null() || previous.0 as usize == usize::MAX {
                if !DeleteObject(HGDIOBJ(bitmap.0)).as_bool() {
                    crate::support::logging::report_error(
                        "windows-native",
                        "cannot delete unselected GDI capture bitmap",
                    );
                }
                if !DeleteDC(memory).as_bool() {
                    crate::support::logging::report_error(
                        "windows-native",
                        "cannot delete unselected GDI capture DC",
                    );
                }
                return Err("SelectObject failed for visual capture".into());
            }
            Ok(Self {
                memory,
                bitmap,
                previous,
                bits,
                dimensions,
                _thread: PhantomData,
            })
        }
    }

    pub(crate) fn width(&self) -> usize {
        self.dimensions.width_u32() as usize
    }

    pub(crate) fn height(&self) -> usize {
        self.dimensions.height_u32() as usize
    }

    pub(crate) fn dc(&self) -> HDC {
        self.memory
    }

    pub(crate) fn pixels(&self) -> &[u8] {
        // SAFETY: the surface owns a non-null DIB allocation of the validated
        // byte length, and the shared borrow prevents mutation.
        unsafe { std::slice::from_raw_parts(self.bits.as_ptr(), self.dimensions.byte_len()) }
    }

    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        // SAFETY: the surface uniquely owns the validated DIB allocation and
        // `&mut self` prevents aliases for the returned lifetime.
        unsafe { std::slice::from_raw_parts_mut(self.bits.as_ptr(), self.dimensions.byte_len()) }
    }

    pub(super) fn copy_from<R>(
        &mut self,
        screen: HDC,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        consume: impl FnOnce(&[u8], u32, u32) -> Result<R, String>,
    ) -> Result<R, String> {
        use windows::Win32::Graphics::Gdi::{
            BitBlt, CAPTUREBLT, HALFTONE, SRCCOPY, SetStretchBltMode, StretchBlt,
        };

        let copy_without_scaling = source_width == self.dimensions.width_i32()
            && source_height == self.dimensions.height_i32();
        // SAFETY: the cached DC owns a selected DIB of `dimensions`; BitBlt or
        // StretchBlt completes before a validated, temporary byte slice is
        // exposed to the caller. The callback cannot retain the slice beyond
        // this borrow.
        unsafe {
            let copied = if copy_without_scaling {
                BitBlt(
                    self.memory,
                    0,
                    0,
                    self.dimensions.width_i32(),
                    self.dimensions.height_i32(),
                    Some(screen),
                    source_x,
                    source_y,
                    SRCCOPY | CAPTUREBLT,
                )
            } else {
                SetStretchBltMode(self.memory, HALFTONE);
                StretchBlt(
                    self.memory,
                    0,
                    0,
                    self.dimensions.width_i32(),
                    self.dimensions.height_i32(),
                    Some(screen),
                    source_x,
                    source_y,
                    source_width,
                    source_height,
                    SRCCOPY | CAPTUREBLT,
                )
                .ok()
            };
            if let Err(error) = copied {
                let operation = if copy_without_scaling {
                    "BitBlt"
                } else {
                    "StretchBlt"
                };
                return Err(format!("{operation} failed for visual capture: {error}"));
            }
            let pixels = std::slice::from_raw_parts(self.bits.as_ptr(), self.dimensions.byte_len());
            consume(
                pixels,
                self.dimensions.width_u32(),
                self.dimensions.height_u32(),
            )
        }
    }
}

impl Drop for GdiDibSurface {
    fn drop(&mut self) {
        use windows::Win32::Graphics::Gdi::{DeleteDC, DeleteObject, SelectObject};

        // SAFETY: the guard owns these objects on this thread; restore the
        // previous selection before destroying the DIB and compatible DC.
        let (restored, bitmap_deleted, dc_deleted) = unsafe {
            (
                SelectObject(self.memory, self.previous),
                DeleteObject(HGDIOBJ(self.bitmap.0)).as_bool(),
                DeleteDC(self.memory).as_bool(),
            )
        };
        if restored.0.is_null() || restored.0 as usize == usize::MAX {
            crate::support::logging::report_error(
                "windows-native",
                "cannot restore selected GDI capture object",
            );
        }
        if !bitmap_deleted {
            crate::support::logging::report_error(
                "windows-native",
                "cannot delete GDI capture bitmap",
            );
        }
        if !dc_deleted {
            crate::support::logging::report_error("windows-native", "cannot delete GDI capture DC");
        }
    }
}
