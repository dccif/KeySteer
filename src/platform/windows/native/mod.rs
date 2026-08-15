//! Minimal Win32 safety boundary for process-wide utilities.

use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{HBITMAP, HDC, HGDIOBJ};

pub(crate) struct ComApartment(PhantomData<Rc<()>>);

#[must_use = "closing the job is the fail-safe that terminates its helper process"]
pub(crate) struct KillOnCloseJob(HANDLE);

impl KillOnCloseJob {
    pub(crate) fn create() -> Result<Self, String> {
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: no security attributes or name are supplied. The returned
        // handle transfers immediately into the owner before configuration.
        let job = Self(
            unsafe { CreateJobObjectW(None, None) }
                .map_err(|error| format!("cannot create WeChat OCR job object: {error}"))?,
        );
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the pointer and byte count describe the exact initialized
        // information struct and remain valid for this synchronous call.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        }
        .map_err(|error| format!("cannot configure WeChat OCR job object: {error}"))?;
        Ok(job)
    }

    pub(crate) fn assign(&self, process: HANDLE) -> Result<(), String> {
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;

        // SAFETY: both handles are live for the duration of this synchronous
        // call; ownership of the process handle remains with `Child`.
        unsafe { AssignProcessToJobObject(self.0, process) }
            .map_err(|error| format!("cannot contain WeChat OCR helper in job object: {error}"))
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        // SAFETY: this owner holds the unique job handle returned at creation.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            crate::app::logging::report_error(
                "windows-native",
                format!("cannot close WeChat OCR job object: {error}"),
            );
        }
    }
}

impl ComApartment {
    pub(crate) fn initialise() -> Result<Self, String> {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

        // SAFETY: the returned !Send guard binds the successful COM apartment
        // initialization to this thread and balances it in Drop.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| format!("cannot initialize COM apartment: {error}"))?;
        Ok(Self(PhantomData))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        use windows::Win32::System::Com::CoUninitialize;

        // SAFETY: this !Send guard is dropped on the same thread that
        // successfully initialized the apartment.
        unsafe { CoUninitialize() };
    }
}

#[must_use = "the desktop DC must be released on its acquiring thread"]
pub(crate) struct ScreenDc(HDC);

impl ScreenDc {
    pub(crate) fn acquire() -> Result<Self, String> {
        use windows::Win32::Graphics::Gdi::GetDC;

        // SAFETY: a null HWND requests the desktop DC. This guard balances the
        // successful acquisition on the same visual worker thread.
        let dc = unsafe { GetDC(None) };
        if dc.is_invalid() {
            Err("GetDC failed for visual capture".into())
        } else {
            Ok(Self(dc))
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
            crate::app::logging::report_error(
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
                        crate::app::logging::report_error(
                            "windows-native",
                            "cannot delete failed GDI capture DC",
                        );
                    }
                    return Err(format!("CreateDIBSection failed: {error}"));
                }
            };
            let Some(bits) = NonNull::new(raw_bits.cast::<u8>()) else {
                if !DeleteObject(HGDIOBJ(bitmap.0)).as_bool() {
                    crate::app::logging::report_error(
                        "windows-native",
                        "cannot delete null-buffer GDI capture bitmap",
                    );
                }
                if !DeleteDC(memory).as_bool() {
                    crate::app::logging::report_error(
                        "windows-native",
                        "cannot delete null-buffer GDI capture DC",
                    );
                }
                return Err("CreateDIBSection returned a null pixel buffer".into());
            };
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            if previous.0.is_null() || previous.0 as usize == usize::MAX {
                if !DeleteObject(HGDIOBJ(bitmap.0)).as_bool() {
                    crate::app::logging::report_error(
                        "windows-native",
                        "cannot delete unselected GDI capture bitmap",
                    );
                }
                if !DeleteDC(memory).as_bool() {
                    crate::app::logging::report_error(
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

    fn copy_from<R>(
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
            crate::app::logging::report_error(
                "windows-native",
                "cannot restore selected GDI capture object",
            );
        }
        if !bitmap_deleted {
            crate::app::logging::report_error("windows-native", "cannot delete GDI capture bitmap");
        }
        if !dc_deleted {
            crate::app::logging::report_error("windows-native", "cannot delete GDI capture DC");
        }
    }
}

#[must_use = "prepared capture resources must remain on their acquiring thread"]
pub(crate) struct PreparedCapture {
    screen: ScreenDc,
    surface: GdiDibSurface,
}

impl PreparedCapture {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, String> {
        let dimensions = NativeDimensions::from_usize(width as usize, height as usize)?;
        let screen = ScreenDc::acquire()?;
        let surface = GdiDibSurface::new(Some(screen.raw()), dimensions)?;
        Ok(Self { screen, surface })
    }

    pub(crate) fn capture_with<R>(
        &mut self,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        consume: impl FnOnce(&[u8], u32, u32) -> Result<R, String>,
    ) -> Result<R, String> {
        self.surface.copy_from(
            self.screen.raw(),
            source_x,
            source_y,
            source_width,
            source_height,
            consume,
        )
    }
}

pub(crate) fn software_bitmap_bgra(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<windows::Graphics::Imaging::SoftwareBitmap, String> {
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap,
    };
    use windows::Win32::System::WinRT::IMemoryBufferByteAccess;
    use windows::core::Interface;

    let dimensions = NativeDimensions::from_usize(width as usize, height as usize)?;
    if pixels.len() != dimensions.byte_len() {
        return Err("BGRA byte length does not match bitmap dimensions".into());
    }
    let factory = windows::core::imp::load_factory::<
        SoftwareBitmap,
        windows::Graphics::Imaging::ISoftwareBitmapFactory,
    >()
    .map_err(|error| format!("cannot load SoftwareBitmap factory: {error}"))?;
    // SAFETY: the factory was loaded for `SoftwareBitmap`, all value
    // parameters use the generated ABI types, and `result` is a valid
    // out-parameter converted into an owned projected object.
    let bitmap: SoftwareBitmap = unsafe {
        let mut result = core::ptr::null_mut();
        (windows::core::Interface::vtable(&factory).CreateWithAlpha)(
            windows::core::Interface::as_raw(&factory),
            BitmapPixelFormat::Bgra8,
            dimensions.width_i32(),
            dimensions.height_i32(),
            BitmapAlphaMode::Ignore,
            &mut result,
        )
        .and_then(|| windows::core::Type::from_abi(result))
    }
    .map_err(|error| format!("SoftwareBitmap creation failed: {error}"))?;
    let buffer = match bitmap.LockBuffer(BitmapBufferAccessMode::Write) {
        Ok(buffer) => buffer,
        Err(error) => {
            let error = format!("cannot lock SoftwareBitmap pixels: {error}");
            if let Err(close_error) = bitmap.Close() {
                crate::app::logging::report_error(
                    "windows-native",
                    format!("cannot close unlocked SoftwareBitmap: {close_error}"),
                );
            }
            return Err(error);
        }
    };
    let copy = (|| -> Result<(), String> {
        let plane = buffer
            .GetPlaneDescription(0)
            .map_err(|error| format!("cannot describe SoftwareBitmap plane: {error}"))?;
        let reference = buffer
            .CreateReference()
            .map_err(|error| format!("cannot reference SoftwareBitmap memory: {error}"))?;
        let copied = (|| -> Result<(), String> {
            let access: IMemoryBufferByteAccess = reference
                .cast()
                .map_err(|error| format!("cannot access SoftwareBitmap memory: {error}"))?;
            let start = usize::try_from(plane.StartIndex)
                .map_err(|_| "SoftwareBitmap returned a negative start index".to_string())?;
            let stride = usize::try_from(plane.Stride)
                .map_err(|_| "SoftwareBitmap returned a negative stride".to_string())?;
            let bitmap_width = dimensions.width_u32() as usize;
            let bitmap_height = dimensions.height_u32() as usize;
            let row_bytes = bitmap_width
                .checked_mul(4)
                .ok_or_else(|| "SoftwareBitmap row byte length overflowed".to_string())?;
            let required = start
                .checked_add(
                    stride
                        .checked_mul(bitmap_height.saturating_sub(1))
                        .and_then(|offset| offset.checked_add(row_bytes))
                        .ok_or_else(|| "SoftwareBitmap plane size overflowed".to_string())?,
                )
                .ok_or_else(|| "SoftwareBitmap plane range overflowed".to_string())?;
            let mut destination = std::ptr::null_mut();
            let mut capacity = 0u32;
            // SAFETY: `reference` keeps the memory buffer alive, `required` is
            // checked against its capacity, and destination rows are disjoint.
            unsafe {
                access
                    .GetBuffer(&mut destination, &mut capacity)
                    .map_err(|error| format!("cannot get SoftwareBitmap memory: {error}"))?;
                if destination.is_null() || required > capacity as usize || stride < row_bytes {
                    return Err("SoftwareBitmap returned an invalid writable plane".into());
                }
                for row in 0..bitmap_height {
                    std::ptr::copy_nonoverlapping(
                        pixels.as_ptr().add(row * row_bytes),
                        destination.add(start + row * stride),
                        row_bytes,
                    );
                }
            }
            Ok(())
        })();
        let closed = reference
            .Close()
            .map_err(|error| format!("cannot close SoftwareBitmap reference: {error}"));
        match (copied, closed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(close_error)) => {
                crate::app::logging::report_error("windows-native", close_error);
                Err(error)
            }
        }
    })();
    let closed = buffer
        .Close()
        .map_err(|error| format!("cannot close SoftwareBitmap buffer: {error}"));
    let fail = |error| {
        if let Err(close_error) = bitmap.Close() {
            crate::app::logging::report_error(
                "windows-native",
                format!("cannot close failed SoftwareBitmap: {close_error}"),
            );
        }
        Err(error)
    };
    match (copy, closed) {
        (Ok(()), Ok(())) => Ok(bitmap),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => fail(error),
        (Err(error), Err(close_error)) => {
            crate::app::logging::report_error("windows-native", close_error);
            fail(error)
        }
    }
}

/// Load and call the OCR activation factory without the projection's static
/// `FactoryCache`. Windows can unload an in-process WinRT server when a
/// temporary COM apartment ends, which would otherwise leave that process-wide
/// cache pointing into freed code before the next UI Hint scan.
type SystemOcrLanguages = windows_collections::IVectorView<windows::Globalization::Language>;
type LoadedSystemOcr = (
    windows::Media::Ocr::OcrEngine,
    Option<(u32, SystemOcrLanguages)>,
);

fn load_system_ocr(include_metadata: bool) -> Result<LoadedSystemOcr, String> {
    use windows::Media::Ocr::{IOcrEngineStatics, OcrEngine};

    let factory = windows::core::imp::load_factory::<OcrEngine, IOcrEngineStatics>()
        .map_err(|error| format!("cannot load OcrEngine factory: {error}"))?;
    // SAFETY: the local factory implements `IOcrEngineStatics`. Every result
    // slot has the exact generated ABI type and is converted immediately into
    // an owned projection before the factory can be released.
    unsafe {
        let mut result = core::ptr::null_mut();
        (windows::core::Interface::vtable(&factory).TryCreateFromUserProfileLanguages)(
            windows::core::Interface::as_raw(&factory),
            &mut result,
        )
        .ok()
        .map_err(|error| format!("cannot create per-scan OcrEngine: {error}"))?;
        let engine = windows::core::Type::from_abi(result)
            .map_err(|error| format!("cannot own per-scan OcrEngine: {error}"))?;
        if !include_metadata {
            return Ok((engine, None));
        }
        let mut maximum = 0;
        (windows::core::Interface::vtable(&factory).MaxImageDimension)(
            windows::core::Interface::as_raw(&factory),
            &mut maximum,
        )
        .ok()
        .map_err(|error| format!("cannot read OcrEngine maximum image dimension: {error}"))?;
        let mut languages = core::ptr::null_mut();
        (windows::core::Interface::vtable(&factory).AvailableRecognizerLanguages)(
            windows::core::Interface::as_raw(&factory),
            &mut languages,
        )
        .ok()
        .map_err(|error| format!("cannot enumerate OCR languages: {error}"))?;
        let languages = windows::core::Type::from_abi(languages)
            .map_err(|error| format!("cannot own OCR language collection: {error}"))?;
        Ok((engine, Some((maximum, languages))))
    }
}

pub(crate) fn create_system_ocr_engine() -> Result<windows::Media::Ocr::OcrEngine, String> {
    load_system_ocr(false).map(|(engine, _)| engine)
}

pub(crate) fn probe_system_ocr_factory() -> Result<(u32, Vec<String>), String> {
    let (engine, metadata) = load_system_ocr(true)?;
    let (maximum, languages) =
        metadata.ok_or_else(|| "OCR factory did not return discovery metadata".to_string())?;
    let mut tags = Vec::with_capacity(languages.Size().unwrap_or_default() as usize);
    for language in &languages {
        tags.push(
            language
                .LanguageTag()
                .map_err(|error| format!("cannot read OCR language tag: {error}"))?
                .to_string(),
        );
    }
    drop(engine);
    Ok((maximum, tags))
}

pub(crate) fn create_png_bitmap_encoder_operation(
    stream: &windows::Storage::Streams::IRandomAccessStream,
) -> Result<windows_future::IAsyncOperation<windows::Graphics::Imaging::BitmapEncoder>, String> {
    use windows::Graphics::Imaging::{BitmapEncoder, IBitmapEncoderStatics};

    let factory = windows::core::imp::load_factory::<BitmapEncoder, IBitmapEncoderStatics>()
        .map_err(|error| format!("cannot load BitmapEncoder factory: {error}"))?;
    // SAFETY: the local factory implements `IBitmapEncoderStatics`, the caller
    // keeps the stream alive through completion, and both output slots use the
    // exact generated ABI types converted into owned values.
    let operation: windows_future::IAsyncOperation<BitmapEncoder> = unsafe {
        let mut result = windows::core::GUID::zeroed();
        (windows::core::Interface::vtable(&factory).PngEncoderId)(
            windows::core::Interface::as_raw(&factory),
            &mut result,
        )
        .ok()
        .map_err(|error| format!("cannot read PNG encoder id: {error}"))?;
        let encoder_id = result;
        let mut operation = core::ptr::null_mut();
        (windows::core::Interface::vtable(&factory).CreateAsync)(
            windows::core::Interface::as_raw(&factory),
            encoder_id,
            windows::core::Interface::as_raw(stream),
            &mut operation,
        )
        .ok()
        .map_err(|error| format!("cannot start PNG encoder creation: {error}"))?;
        windows::core::Type::from_abi(operation)
            .map_err(|error| format!("cannot own PNG encoder operation: {error}"))?
    };
    Ok(operation)
}

pub(crate) fn create_file_random_access_stream(
    path: &Path,
) -> Result<windows::Storage::Streams::IRandomAccessStream, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Com::{STGM_CREATE, STGM_SHARE_EXCLUSIVE, STGM_WRITE};
    use windows::Win32::System::WinRT::CreateRandomAccessStreamOnFile;
    use windows::core::PCWSTR;

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let access = STGM_CREATE.0 | STGM_WRITE.0 | STGM_SHARE_EXCLUSIVE.0;
    // SAFETY: `wide` is a NUL-terminated path retained for this call and the
    // requested interface type matches the documented WinRT stream factory.
    unsafe { CreateRandomAccessStreamOnFile(PCWSTR(wide.as_ptr()), access) }
        .map_err(|error| format!("cannot create random-access file stream: {error}"))
}

type OcrCallback = unsafe extern "C" fn(windows::core::PCSTR);
type WechatOcrFn = unsafe extern "C" fn(
    windows::core::PCWSTR,
    windows::core::PCWSTR,
    windows::core::PCSTR,
    OcrCallback,
) -> bool;
type StopOcrFn = unsafe extern "C" fn();
static WECHAT_CALLBACK_VALUE: std::sync::OnceLock<std::sync::Mutex<Vec<u8>>> =
    std::sync::OnceLock::new();
const MAX_WECHAT_RESPONSE: usize = 8 * 1024 * 1024;

unsafe extern "C" fn capture_wechat_callback(value: windows::core::PCSTR) {
    if value.is_null() {
        return;
    }
    // SAFETY: wcocr documents a readable NUL-terminated callback string valid
    // for this callback. Scan no farther than the IPC ceiling and copy before
    // returning to the bridge.
    let bytes = unsafe {
        let mut length = 0usize;
        while length <= MAX_WECHAT_RESPONSE && *value.0.add(length) != 0 {
            length += 1;
        }
        if length > MAX_WECHAT_RESPONSE {
            return;
        }
        std::slice::from_raw_parts(value.0, length)
    };
    let mut output = WECHAT_CALLBACK_VALUE
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    output.clear();
    if bytes.len() <= MAX_WECHAT_RESPONSE {
        output.extend_from_slice(bytes);
    }
}

#[must_use = "the module must stay loaded while exported function pointers are used"]
struct OwnedModule(windows::Win32::Foundation::HMODULE);

impl Drop for OwnedModule {
    fn drop(&mut self) {
        use windows::Win32::Foundation::FreeLibrary;

        // SAFETY: this guard uniquely owns the successful LoadLibraryExW
        // result and is destroyed only after all exported pointers are dead.
        if let Err(error) = unsafe { FreeLibrary(self.0) } {
            crate::app::logging::report_error(
                "windows-native",
                format!("cannot unload WeChat OCR bridge: {error}"),
            );
        }
    }
}

pub(crate) struct WechatBridge {
    _module: OwnedModule,
    recognize: WechatOcrFn,
    stop: Option<StopOcrFn>,
}

fn wechat_recognize_export(
    address: windows::Win32::Foundation::FARPROC,
) -> Result<WechatOcrFn, String> {
    const _: () = assert!(
        std::mem::size_of::<windows::Win32::Foundation::FARPROC>()
            == std::mem::size_of::<WechatOcrFn>()
    );
    #[repr(C)]
    union Export {
        raw: windows::Win32::Foundation::FARPROC,
        typed: WechatOcrFn,
    }
    if address.is_none() {
        return Err("wcocr.dll returned a null wechat_ocr export".into());
    }
    // SAFETY: GetProcAddress returned this exact symbol from the architecture-
    // checked bridge. The compile-time size assertion and dedicated union keep
    // the only ABI reinterpretation local to this audited loader.
    Ok(unsafe { Export { raw: address }.typed })
}

fn wechat_stop_export(address: windows::Win32::Foundation::FARPROC) -> Option<StopOcrFn> {
    const _: () = assert!(
        std::mem::size_of::<windows::Win32::Foundation::FARPROC>()
            == std::mem::size_of::<StopOcrFn>()
    );
    #[repr(C)]
    union Export {
        raw: windows::Win32::Foundation::FARPROC,
        typed: StopOcrFn,
    }
    address.map(|raw| {
        // SAFETY: this optional address is the exact `stop_ocr` export from
        // the same architecture-checked module and has the asserted size.
        unsafe { Export { raw: Some(raw) }.typed }
    })
}

impl WechatBridge {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::LibraryLoader::{
            GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            LoadLibraryExW,
        };
        use windows::core::{PCSTR, PCWSTR};

        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: the absolute path and symbol names are NUL terminated.
        // Restricted search uses only the bridge directory and Windows safe
        // defaults; the typed pointers use the bridge's documented C ABI and
        // cannot outlive the returned module owner.
        let module = unsafe {
            OwnedModule(
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )
                .map_err(|error| format!("cannot load {}: {error}", path.display()))?,
            )
        };
        // SAFETY: symbol names are static NUL-terminated C strings and the
        // owned module remains loaded through all returned function pointers.
        let (recognize, stop) = unsafe {
            let address = GetProcAddress(module.0, PCSTR(c"wechat_ocr".as_ptr().cast()))
                .ok_or_else(|| "wcocr.dll lacks wechat_ocr".to_string())?;
            let recognize = wechat_recognize_export(Some(address))?;
            let stop =
                wechat_stop_export(GetProcAddress(module.0, PCSTR(c"stop_ocr".as_ptr().cast())));
            (recognize, stop)
        };
        Ok(Self {
            _module: module,
            recognize,
            stop,
        })
    }

    pub(crate) fn recognize(
        &self,
        component: &Path,
        runtime: &Path,
        image: &std::ffi::CStr,
    ) -> Result<Vec<u8>, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::{PCSTR, PCWSTR};

        let component = component
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let runtime = runtime
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        WECHAT_CALLBACK_VALUE
            .get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        // SAFETY: all path buffers remain NUL terminated for the synchronous
        // call and the callback copies its result before returning.
        let success = unsafe {
            (self.recognize)(
                PCWSTR(component.as_ptr()),
                PCWSTR(runtime.as_ptr()),
                PCSTR(image.as_ptr().cast()),
                capture_wechat_callback,
            )
        };
        let mut value = WECHAT_CALLBACK_VALUE
            .get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if success && !value.is_empty() {
            Ok(std::mem::take(&mut *value))
        } else {
            Err("WeChat OCR bridge returned no response".into())
        }
    }
}

impl Drop for WechatBridge {
    fn drop(&mut self) {
        // SAFETY: no recognition call remains active; all function pointers
        // still belong to the live `OwnedModule`, which is dropped afterwards.
        unsafe {
            if let Some(stop) = self.stop {
                stop();
            }
        }
    }
}

/// Dimensions that are representable by Win32 APIs and by a Rust byte slice.
///
/// Construction performs every narrowing conversion and length calculation so
/// native allocation sizes cannot diverge from the slices exposed to Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDimensions {
    width: i32,
    height: i32,
    byte_len: usize,
}

impl NativeDimensions {
    pub(crate) fn from_usize(width: usize, height: usize) -> Result<Self, String> {
        let width_i32 =
            i32::try_from(width).map_err(|_| format!("native width {width} exceeds i32::MAX"))?;
        let height_i32 = i32::try_from(height)
            .map_err(|_| format!("native height {height} exceeds i32::MAX"))?;
        if width_i32 == 0 || height_i32 == 0 {
            return Err("native dimensions must be positive".into());
        }
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or_else(|| format!("native BGRA surface {width}x{height} is too large"))?;
        Ok(Self {
            width: width_i32,
            height: height_i32,
            byte_len,
        })
    }

    pub(crate) fn from_f64(width: f64, height: f64) -> Result<Self, String> {
        fn rounded(value: f64, name: &str) -> Result<usize, String> {
            if !value.is_finite() || value <= 0.0 || value.round() > i32::MAX as f64 {
                return Err(format!("invalid native {name} {value}"));
            }
            Ok(value.round().max(1.0) as usize)
        }

        Self::from_usize(rounded(width, "width")?, rounded(height, "height")?)
    }

    pub(crate) const fn width_i32(self) -> i32 {
        self.width
    }

    pub(crate) const fn height_i32(self) -> i32 {
        self.height
    }

    pub(crate) const fn width_u32(self) -> u32 {
        self.width as u32
    }

    pub(crate) const fn height_u32(self) -> u32 {
        self.height as u32
    }

    pub(crate) const fn byte_len(self) -> usize {
        self.byte_len
    }
}

#[cfg(test)]
mod native_dimension_tests {
    use super::NativeDimensions;

    #[test]
    fn native_dimensions_reject_unrepresentable_surfaces() {
        assert!(NativeDimensions::from_usize(0, 1).is_err());
        assert!(NativeDimensions::from_usize(1, 0).is_err());
        assert!(NativeDimensions::from_usize(i32::MAX as usize + 1, 1).is_err());
        assert!(NativeDimensions::from_usize(i32::MAX as usize, i32::MAX as usize).is_err());
        assert!(NativeDimensions::from_f64(f64::NAN, 1.0).is_err());
        assert!(NativeDimensions::from_f64(1.0, f64::INFINITY).is_err());
    }

    #[test]
    fn native_dimensions_preserve_the_validated_byte_length() {
        let dimensions = NativeDimensions::from_usize(3840, 2160).unwrap();
        assert_eq!(dimensions.width_i32(), 3840);
        assert_eq!(dimensions.height_i32(), 2160);
        assert_eq!(dimensions.byte_len(), 3840 * 2160 * 4);
    }
}

unsafe extern "C" {
    fn keysteer_compositor_clock_create() -> isize;
    fn keysteer_compositor_clock_wait(stop_event: isize) -> isize;
    fn keysteer_compositor_clock_signal(stop_event: isize) -> isize;
    fn keysteer_compositor_clock_boost(enable: isize) -> isize;
}

enum CompositorCall {
    Create,
    Wait(isize),
    Signal(isize),
    Boost(bool),
}

/// Keep the C ABI and its dynamically resolved Windows 11 functions inside one
/// reviewed native boundary. The bridge normalizes every result to `isize`.
fn compositor_call(call: CompositorCall) -> isize {
    // SAFETY: the C bridge is compiled into this crate with matching signatures.
    // Handle tokens originate from `CreateEventW`, remain owned by
    // `CompositorClockSignal`, and outlive every synchronous call using them.
    unsafe {
        match call {
            CompositorCall::Create => keysteer_compositor_clock_create(),
            CompositorCall::Wait(stop_event) => keysteer_compositor_clock_wait(stop_event),
            CompositorCall::Signal(stop_event) => keysteer_compositor_clock_signal(stop_event),
            CompositorCall::Boost(enable) => keysteer_compositor_clock_boost(enable as isize),
        }
    }
}

/// A process or thread handle that is closed exactly once.
#[repr(transparent)]
struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        // SAFETY: this wrapper is created only from an owned successful handle
        // and Drop is its sole close path.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            crate::report_error!("windows-native", "CloseHandle failed: {error}");
        }
    }
}

/// A window created by KeySteer and destroyed on its owner thread.
#[repr(transparent)]
pub(crate) struct OwnedWindow {
    raw: HWND,
    _thread: PhantomData<Rc<()>>,
}

impl OwnedWindow {
    #[inline(always)]
    pub(crate) fn new(hwnd: HWND) -> Self {
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
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

        let hwnd = std::mem::take(&mut self.raw);
        if hwnd.is_invalid() {
            return Ok(());
        }
        // SAFETY: ownership was transferred into this wrapper and the caller
        // invokes destroy on the thread that created the window.
        unsafe { DestroyWindow(hwnd) }
    }
}

impl Drop for OwnedWindow {
    #[inline(always)]
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

        if !self.raw.is_invalid() {
            // SAFETY: the wrapper is the sole owner and remains on the window
            // thread for its complete lifetime.
            if let Err(error) = unsafe { DestroyWindow(self.raw) } {
                crate::report_error!("windows-native", "DestroyWindow failed: {error}");
            }
        }
    }
}

/// Restores the previously selected GDI object when the guard leaves scope.
pub(crate) struct SelectedObject<'dc> {
    dc: HDC,
    previous: HGDIOBJ,
    _dc: PhantomData<&'dc GdiDibSurface>,
    _thread: PhantomData<Rc<()>>,
}

impl GdiDibSurface {
    #[inline(always)]
    pub(crate) fn select_object(&self, object: HGDIOBJ) -> Result<SelectedObject<'_>, String> {
        use windows::Win32::Graphics::Gdi::SelectObject;

        // SAFETY: both handles are live for the guard lifetime. Drop restores
        // the exact object returned by this call.
        let previous = unsafe { SelectObject(self.memory, object) };
        if previous.0.is_null() || previous.0 as usize == usize::MAX {
            Err("SelectObject failed".into())
        } else {
            Ok(SelectedObject {
                dc: self.memory,
                previous,
                _dc: PhantomData,
                _thread: PhantomData,
            })
        }
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

/// Whether Desktop Window Manager composition is available.
pub(crate) fn dwm_composition_enabled() -> windows::core::Result<bool> {
    use windows::Win32::Graphics::Dwm::DwmIsCompositionEnabled;

    // SAFETY: the function has no pointer arguments and returns a BOOL value.
    unsafe { DwmIsCompositionEnabled() }.map(|enabled| enabled.as_bool())
}

/// Block the calling worker until DWM completes the next composition pass.
pub(crate) fn wait_for_dwm_frame() -> windows::core::Result<()> {
    use windows::Win32::Graphics::Dwm::DwmFlush;

    // SAFETY: DwmFlush has no arguments or caller-owned resources.
    unsafe { DwmFlush() }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompositorWait {
    Frame,
    Interrupted,
    Failed,
}

/// Owner of the event used to interrupt the Windows 11 compositor clock.
pub(crate) struct CompositorClockSignal(OwnedHandle);

impl CompositorClockSignal {
    /// Return `None` on Windows 10 or when the compositor-clock export cannot
    /// be loaded. Callers then retain the DXGI/DWM compatibility path.
    pub(crate) fn try_new() -> Option<Self> {
        let token = compositor_call(CompositorCall::Create);
        (token != 0).then(|| Self(OwnedHandle::new(HANDLE(token as *mut _))))
    }

    pub(crate) fn token(&self) -> isize {
        self.0.raw().0 as isize
    }
}

/// Wake a compositor-clock wait without borrowing the worker-owned handle.
/// The token is published only while that worker retains the corresponding
/// event, so callers never own or close it.
pub(crate) fn interrupt_compositor_clock(token: isize) -> bool {
    token != 0 && compositor_call(CompositorCall::Signal(token)) != 0
}

pub(crate) fn wait_for_compositor_frame(stop_event: isize) -> CompositorWait {
    match compositor_call(CompositorCall::Wait(stop_event)) {
        1 => CompositorWait::Frame,
        0 => CompositorWait::Interrupted,
        _ => CompositorWait::Failed,
    }
}

/// Ask Windows 11 to use its high dynamic-refresh cadence while movement is
/// active. Unsupported systems return false and continue normally.
pub(crate) fn boost_compositor_clock(enable: bool) -> bool {
    compositor_call(CompositorCall::Boost(enable)) != 0
}

/// Retained DXGI output selected for display-synchronised movement.
pub(crate) struct DisplayOutput(windows::Win32::Graphics::Dxgi::IDXGIOutput);

impl DisplayOutput {
    /// Block until this output reaches its next vertical blank.
    pub(crate) fn wait_for_vblank(&self) -> windows::core::Result<()> {
        // SAFETY: the retained COM interface stays alive for the synchronous
        // wait and is used only by the frame-clock worker that owns it.
        unsafe { self.0.WaitForVBlank() }
    }
}

/// Prefer actual Windows 11 dynamic-refresh VBlank cadence when supported.
pub(crate) fn prefer_dynamic_vblank() {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::{s, w};

    // SAFETY: dxgi.dll is a system component. The optional export has the same
    // no-argument system ABI as FARPROC, and its ignored HRESULT only reports
    // whether the preference was accepted. FreeLibrary balances our load.
    unsafe {
        let Ok(module) = LoadLibraryW(w!("dxgi.dll")) else {
            return;
        };
        if let Some(disable) = GetProcAddress(module, s!("DXGIDisableVBlankVirtualization")) {
            let _ = disable();
        }
        if let Err(error) = FreeLibrary(module) {
            crate::report_error!("windows-native", "cannot unload dxgi.dll: {error}");
        }
    }
}

/// Map a desktop point to the nearest monitor without querying refresh rate.
pub(crate) fn monitor_for_point(x: f64, y: f64) -> isize {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};

    let point = POINT {
        x: x.round() as i32,
        y: y.round() as i32,
    };
    // SAFETY: `point` is a value type and nearest-monitor fallback returns a
    // stable HMONITOR whenever a display is attached.
    unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) }.0 as isize
}

/// Find the DXGI output whose native monitor handle matches `monitor`.
pub(crate) fn display_output_for_monitor(monitor: isize) -> Result<Option<DisplayOutput>, String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::HMONITOR;

    // SAFETY: DXGI creates retained COM wrappers. Enumeration is read-only,
    // and every adapter/output/factory interface is released by RAII.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()
            .map_err(|error| format!("cannot create DXGI factory for frame clock: {error}"))?;
        let monitor = HMONITOR(monitor as *mut _);
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                if let Ok(description) = output.GetDesc()
                    && description.Monitor == monitor
                {
                    return Ok(Some(DisplayOutput(output)));
                }
                output_index += 1;
            }
            adapter_index += 1;
        }
        Ok(None)
    }
}

/// Wake a thread whose Win32 message queue has already been initialized.
#[inline(always)]
pub(crate) fn post_thread_wake(thread: u32, message: u32) -> windows::core::Result<()> {
    post_thread_message(thread, message, 0)
}

/// Post an integer payload to an initialized Win32 thread message queue.
#[inline(always)]
pub(crate) fn post_thread_message(
    thread: u32,
    message: u32,
    payload: usize,
) -> windows::core::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

    // SAFETY: the payload contains no pointers and the receiver treats this as
    // an integer generation attached to an application-owned message.
    unsafe { PostThreadMessageW(thread, message, WPARAM(payload), LPARAM(0)) }
}

/// Register a process-lifetime window class. Re-registering an existing class
/// is idempotent for independently constructed renderer/tray workers.
pub(crate) fn register_window_class(
    class: &windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW,
) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::RegisterClassExW;

    // SAFETY: callers provide a fully initialized class whose callback and
    // static strings remain alive for the process lifetime.
    if unsafe { RegisterClassExW(class) } != 0 {
        return Ok(());
    }
    let last = windows::core::Error::from_thread();
    if last.code() == windows::core::HRESULT::from_win32(1410) {
        Ok(())
    } else {
        Err(format!("RegisterClassExW failed: {last}"))
    }
}

#[inline(always)]
pub(crate) fn default_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;

    // SAFETY: this forwards the unchanged callback arguments to User32.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[inline(always)]
pub(crate) fn get_window_message(
    message: &mut windows::Win32::UI::WindowsAndMessaging::MSG,
) -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::GetMessageW;

    // SAFETY: `message` is a valid writable out-parameter owned by the caller.
    unsafe { GetMessageW(message, None, 0, 0) }.0
}

#[inline(always)]
pub(crate) fn window_long(
    hwnd: HWND,
    index: windows::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX,
) -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowLongW;

    // SAFETY: this reads one documented integer field from a borrowed HWND.
    unsafe { GetWindowLongW(hwnd, index) }
}

#[inline(always)]
pub(crate) fn call_next_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;

    // SAFETY: the low-level hook forwards the original callback arguments
    // unchanged and retains no pointer from `lparam`.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub(crate) fn set_console_control_handler(
    handler: windows::Win32::System::Console::PHANDLER_ROUTINE,
    enabled: bool,
) -> windows::core::Result<()> {
    use windows::Win32::System::Console::SetConsoleCtrlHandler;

    // SAFETY: the callback has process lifetime and the same value is used to
    // unregister it before backend teardown completes.
    unsafe { SetConsoleCtrlHandler(handler, enabled) }
}

pub(crate) fn install_foreground_event_hook(
    callback: windows::Win32::UI::Accessibility::WINEVENTPROC,
) -> windows::Win32::UI::Accessibility::HWINEVENTHOOK {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    };

    // SAFETY: the callback accesses only process-lifetime atomics and the
    // returned owned hook is uninstalled by the matching wrapper below.
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            callback,
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    }
}

pub(crate) fn uninstall_event_hook(hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK) -> bool {
    use windows::Win32::UI::Accessibility::UnhookWinEvent;

    // SAFETY: `hook` came from `install_foreground_event_hook` and is consumed
    // once during owner Drop.
    unsafe { UnhookWinEvent(hook) }.as_bool()
}

/// Return the current foreground window, which may be null while focus changes.
#[inline(always)]
pub(crate) fn foreground_window() -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    // SAFETY: this call has no arguments and returns a borrowed HWND value.
    unsafe { GetForegroundWindow() }
}

#[inline(always)]
pub(crate) fn current_process_id() -> u32 {
    use windows::Win32::System::Threading::GetCurrentProcessId;

    // SAFETY: this call has no arguments or failure mode.
    unsafe { GetCurrentProcessId() }
}

#[inline(always)]
pub(crate) fn current_thread_id() -> u32 {
    use windows::Win32::System::Threading::GetCurrentThreadId;

    // SAFETY: this call has no arguments or failure mode.
    unsafe { GetCurrentThreadId() }
}

#[inline(always)]
pub(crate) fn current_module() -> windows::core::Result<windows::Win32::Foundation::HMODULE> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    // SAFETY: a null module name requests the current executable module.
    unsafe { GetModuleHandleW(None) }
}

#[inline(always)]
pub(crate) fn is_window_visible(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

    // SAFETY: the borrowed HWND is used only for this synchronous query.
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

#[inline(always)]
pub(crate) fn is_window_iconic(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;

    // SAFETY: the borrowed HWND is used only for this synchronous query.
    unsafe { IsIconic(hwnd) }.as_bool()
}

/// Return a window's creating thread and optionally its owning process.
#[inline(always)]
pub(crate) fn window_thread_process_id(hwnd: HWND, process_id: Option<&mut u32>) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    // SAFETY: the optional process id is a valid out-parameter for this call.
    unsafe { GetWindowThreadProcessId(hwnd, process_id.map(std::ptr::from_mut)) }
}

pub(crate) fn window_title(hwnd: HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

    // SAFETY: the HWND is borrowed for the synchronous query.
    let length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut title = vec![0u16; length.saturating_add(1)];
    // SAFETY: the UTF-16 buffer is writable and includes room for the trailing
    // NUL requested by GetWindowTextW.
    let copied = unsafe { GetWindowTextW(hwnd, &mut title) }.max(0) as usize;
    String::from_utf16_lossy(&title[..copied])
}

pub(crate) fn process_executable_name(process_id: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    // SAFETY: access is query-only. Every UTF-16 buffer is writable for its
    // advertised length, and OwnedHandle closes the successful process handle.
    unsafe {
        let process = OwnedHandle::new(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?,
        );
        for capacity in [512usize, 32_768] {
            let mut path = vec![0u16; capacity];
            let mut length = capacity as u32;
            if QueryFullProcessImageNameW(
                process.raw(),
                Default::default(),
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
            .is_ok()
            {
                return std::path::Path::new(&String::from_utf16_lossy(&path[..length as usize]))
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned());
            }
        }
        None
    }
}

fn integrity_name(rid: u32) -> &'static str {
    match rid {
        0x0000..=0x0FFF => "untrusted",
        0x1000..=0x1FFF => "low",
        0x2000..=0x20FF => "medium",
        0x2100..=0x2FFF => "medium-plus",
        0x3000..=0x3FFF => "high",
        0x4000..=0x4FFF => "system",
        0x5000.. => "protected",
    }
}

/// Expensive context captured only after `SendInput` has already failed.
/// This deliberately lives in the native boundary so token handles and
/// read-only process handles cannot leak into the portable input code.
pub(crate) fn send_input_failure_context(last_error: u32, input_size: usize) -> String {
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenElevation, TokenIntegrityLevel, TokenUIAccess,
    };
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let error = if last_error == 0 {
        "not set (UIPI may leave it unset)".into()
    } else {
        format!("{}", std::io::Error::from_raw_os_error(last_error as i32))
    };
    let current_pid = current_process_id();
    let foreground = foreground_window();
    let mut foreground_pid = 0u32;
    let foreground_thread = window_thread_process_id(foreground, Some(&mut foreground_pid));

    // SAFETY: every native call below is read-only. Out-parameters point to
    // correctly sized live storage; query handles are immediately wrapped and
    // closed once this failure-only diagnostic snapshot is formatted. The SID
    // pointers originate from a successful TOKEN_MANDATORY_LABEL query and do
    // not outlive its aligned backing allocation.
    let (current_session, current_security, foreground_session, foreground_security) = unsafe {
        let process_session = |process_id: u32| -> Option<u32> {
            let mut session_id = 0;
            ProcessIdToSessionId(process_id, &mut session_id)
                .ok()
                .map(|()| session_id)
        };
        let token_u32 = |token: HANDLE,
                         class: windows::Win32::Security::TOKEN_INFORMATION_CLASS|
         -> Option<u32> {
            let mut value = 0u32;
            let mut returned = 0u32;
            GetTokenInformation(
                token,
                class,
                Some((&mut value as *mut u32).cast()),
                std::mem::size_of::<u32>() as u32,
                &mut returned,
            )
            .ok()
            .map(|()| value)
        };
        let token_integrity = |token: HANDLE| -> Option<u32> {
            let mut required = 0u32;
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut required);
            if required < std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 {
                return None;
            }
            let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
            let mut storage = vec![0usize; words];
            let mut returned = 0u32;
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(storage.as_mut_ptr().cast()),
                required,
                &mut returned,
            )
            .ok()?;
            if returned < std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 {
                return None;
            }
            let sid = (*(storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()))
                .Label
                .Sid;
            if sid.is_invalid() {
                return None;
            }
            let count = GetSidSubAuthorityCount(sid).as_ref().copied()?;
            if count == 0 {
                return None;
            }
            GetSidSubAuthority(sid, u32::from(count - 1))
                .as_ref()
                .copied()
        };
        let process_security = |process_id: u32| -> String {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
            else {
                return "security=unavailable".into();
            };
            let process = OwnedHandle::new(process);
            let mut token = HANDLE::default();
            if OpenProcessToken(process.raw(), TOKEN_QUERY, &mut token).is_err() {
                return "security=unavailable".into();
            }
            let token = OwnedHandle::new(token);
            let integrity = token_integrity(token.raw())
                .map(|rid| format!("{}(0x{rid:04X})", integrity_name(rid)))
                .unwrap_or_else(|| "unknown".into());
            let elevated = token_u32(token.raw(), TokenElevation)
                .map(|value| value != 0)
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            let ui_access = token_u32(token.raw(), TokenUIAccess)
                .map(|value| value != 0)
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            format!("integrity={integrity}, elevated={elevated}, ui_access={ui_access}")
        };

        (
            process_session(current_pid),
            process_security(current_pid),
            (foreground_pid != 0)
                .then(|| process_session(foreground_pid))
                .flatten(),
            (foreground_pid != 0).then(|| process_security(foreground_pid)),
        )
    };
    let current_session =
        current_session.map_or_else(|| "unknown".into(), |value| value.to_string());
    let foreground_context = if foreground.is_invalid() || foreground_pid == 0 {
        "foreground=none".into()
    } else {
        let executable =
            process_executable_name(foreground_pid).unwrap_or_else(|| "unknown".into());
        let session =
            foreground_session.map_or_else(|| "unknown".into(), |value| value.to_string());
        let security = foreground_security.unwrap_or_else(|| "security=unavailable".into());
        format!(
            "foreground={{hwnd=0x{:X}, thread={}, pid={}, exe={:?}, session={}, {}}}",
            foreground.0 as usize, foreground_thread, foreground_pid, executable, session, security
        )
    };

    format!(
        "last_error=0x{last_error:08X} ({error}), input_size={input_size}, pointer_width={}, current={{pid={current_pid}, session={current_session}, {current_security}}}, {foreground_context}",
        usize::BITS
    )
}

#[inline(always)]
pub(crate) fn apps_use_light_theme() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;

    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both out-parameters are correctly sized and live for the
    // synchronous registry query.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && value != 0
}

#[inline(always)]
pub(crate) fn wait_for_input(timeout_ms: u32) {
    use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT};

    // SAFETY: no handles are supplied, so Windows waits only on queued input.
    unsafe {
        MsgWaitForMultipleObjects(None, false, timeout_ms, QS_ALLINPUT);
    }
}

/// Create the current thread's Win32 message queue and return its thread ID.
///
/// `PostThreadMessageW` fails until a thread has called a User32 message API,
/// so workers publish their ID only after this function returns.
pub(crate) fn prepare_thread_message_queue() -> u32 {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};

    let mut probe = MSG::default();
    // SAFETY: `probe` is a valid out-parameter. PM_NOREMOVE initializes the
    // queue without consuming a pending message.
    unsafe {
        let _ = PeekMessageW(&mut probe, None, 0, 0, PM_NOREMOVE);
        GetCurrentThreadId()
    }
}

/// Block for and dispatch one message on the current window-owning thread.
/// Returns `false` for `WM_QUIT`.
pub(crate) fn wait_and_dispatch_window_message() -> windows::core::Result<bool> {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, TranslateMessage,
    };

    let mut message = MSG::default();
    // SAFETY: `message` is a valid out-parameter. A positive GetMessageW
    // result fully initializes it for TranslateMessage/DispatchMessageW.
    unsafe {
        match GetMessageW(&mut message, None, 0, 0).0 {
            0 => Ok(false),
            -1 => Err(windows::core::Error::from_thread()),
            _ => {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
                Ok(true)
            }
        }
    }
}

/// Make an overlay HWND transparent to mouse hit testing.
pub(crate) fn click_through_hit_test(message: u32) -> Option<windows::Win32::Foundation::LRESULT> {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{HTTRANSPARENT, WM_NCHITTEST};

    (message == WM_NCHITTEST).then_some(LRESULT(HTTRANSPARENT as isize))
}

#[cfg(test)]
pub(crate) struct OverlayProbe {
    pub(crate) hit_test: windows::Win32::Foundation::LRESULT,
    pub(crate) ex_style: u32,
}

#[cfg(test)]
pub(crate) fn probe_overlay_hit_test(
    class_names: &[windows::core::PCWSTR],
    timeout_ms: u32,
) -> windows::core::Result<Option<OverlayProbe>> {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GWL_EXSTYLE, GetWindowLongW, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
        WM_NCHITTEST,
    };

    // SAFETY: class names are static NUL-terminated strings. A successfully
    // found HWND is used synchronously, and `result` is a valid out-parameter.
    unsafe {
        for class_name in class_names {
            let Ok(hwnd) = FindWindowW(*class_name, None) else {
                continue;
            };
            let mut result = 0usize;
            if SendMessageTimeoutW(
                hwnd,
                WM_NCHITTEST,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                timeout_ms,
                Some(&mut result),
            )
            .0 == 0
            {
                return Err(windows::core::Error::from_thread());
            }
            return Ok(Some(OverlayProbe {
                hit_test: LRESULT(result as isize),
                ex_style: GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            }));
        }
        Ok(None)
    }
}

/// Drain the current render thread's window messages.
#[inline(always)]
pub(crate) fn pump_window_messages() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
    };

    let mut message = MSG::default();
    // SAFETY: `message` is a valid out-parameter. Each successful PeekMessageW
    // initializes it before translation and dispatch.
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            if message.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}

/// Prefer the engine's synchronous input work without entering real-time
/// priority classes that could starve the compositor or system services.
pub(crate) fn prefer_input_latency() -> std::io::Result<()> {
    use windows::Win32::System::Threading::THREAD_PRIORITY_HIGHEST;

    set_current_thread_priority(THREAD_PRIORITY_HIGHEST)
}

/// Keep OCR and pixel analysis below interactive input/compositor work.
pub(crate) fn prefer_background_work() -> std::io::Result<()> {
    use windows::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL;

    set_current_thread_priority(THREAD_PRIORITY_BELOW_NORMAL)
}

fn set_current_thread_priority(
    priority: windows::Win32::System::Threading::THREAD_PRIORITY,
) -> std::io::Result<()> {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};

    // SAFETY: The pseudo-handle always identifies the calling thread and does
    // not need closing. Callers select non-realtime documented priorities.
    unsafe { SetThreadPriority(GetCurrentThread(), priority) }.map_err(std::io::Error::other)
}

/// Attach the parent console when possible, otherwise allocate one.
pub(crate) fn prepare_console_for_cli() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole};

    // SAFETY: Both functions take no borrowed pointers. Failure to attach is
    // expected for Explorer launches and is handled by allocating a console.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

/// Atomically replace `destination` with an already-written temporary file.
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both UTF-16 buffers are NUL-terminated and remain alive for the
    // complete synchronous call. Flags request an atomic durable replacement.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{HTTRANSPARENT, WM_NCHITTEST, WM_PAINT};

    #[test]
    fn overlays_never_claim_mouse_hit_tests() {
        assert_eq!(
            click_through_hit_test(WM_NCHITTEST),
            Some(windows::Win32::Foundation::LRESULT(HTTRANSPARENT as isize))
        );
        assert_eq!(click_through_hit_test(WM_PAINT), None);
    }

    #[test]
    fn integrity_rids_are_labeled_for_input_diagnostics() {
        assert_eq!(integrity_name(0x1000), "low");
        assert_eq!(integrity_name(0x2000), "medium");
        assert_eq!(integrity_name(0x2100), "medium-plus");
        assert_eq!(integrity_name(0x3000), "high");
        assert_eq!(integrity_name(0x4000), "system");
        assert_eq!(integrity_name(0x5000), "protected");
    }

    #[test]
    fn compositor_clock_stop_event_is_immediately_interruptible_when_available() {
        let Some(signal) = CompositorClockSignal::try_new() else {
            // Windows 10 intentionally uses the DXGI compatibility path.
            return;
        };

        assert!(interrupt_compositor_clock(signal.token()));
        assert_eq!(
            wait_for_compositor_frame(signal.token()),
            CompositorWait::Interrupted
        );
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn visual_capture_prepares_and_releases_thread_bound_surface() -> Result<(), String> {
        let mut capture = PreparedCapture::new(64, 64)?;
        let first = capture.capture_with(0, 0, 64, 64, |pixels, width, height| {
            Ok((pixels.len(), width, height))
        })?;
        let second = capture.capture_with(0, 0, 64, 64, |pixels, width, height| {
            Ok((pixels.len(), width, height))
        })?;
        drop(capture);
        assert_eq!(first, (64 * 64 * 4, 64, 64));
        assert_eq!(second, first);
        Ok(())
    }

    #[test]
    #[ignore = "requires the optional Windows OCR capability"]
    fn system_ocr_factory_survives_transient_apartments() -> Result<(), String> {
        for _ in 0..3 {
            std::thread::spawn(|| -> Result<(), String> {
                let apartment = ComApartment::initialise()?;
                let (maximum, _) = probe_system_ocr_factory()?;
                let engine = create_system_ocr_engine()?;
                assert!(maximum > 0);
                drop(engine);
                drop(apartment);
                Ok(())
            })
            .join()
            .map_err(|_| "transient OCR apartment thread panicked".to_string())??;
        }
        Ok(())
    }
}
