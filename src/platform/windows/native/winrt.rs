//! Owned WinRT imaging and OCR factories; exact ABI calls stay local.
use super::NativeDimensions;
use std::path::Path;
use std::{marker::PhantomData, rc::Rc};

#[must_use = "the factory must stay in its creating COM apartment"]
pub(crate) struct SoftwareBitmapFactory {
    factory: windows::Graphics::Imaging::ISoftwareBitmapFactory,
    _thread: PhantomData<Rc<()>>,
}

impl SoftwareBitmapFactory {
    pub(crate) fn load() -> Result<Self, String> {
        let factory = windows::core::imp::load_factory::<
            windows::Graphics::Imaging::SoftwareBitmap,
            windows::Graphics::Imaging::ISoftwareBitmapFactory,
        >()
        .map_err(|error| format!("cannot load SoftwareBitmap factory: {error}"))?;
        Ok(Self {
            factory,
            _thread: PhantomData,
        })
    }

    pub(crate) fn bgra(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Result<windows::Graphics::Imaging::SoftwareBitmap, String> {
        self.bgra_region(pixels, width, height, 0, 0, width, height)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bgra_region(
        &self,
        pixels: &[u8],
        source_width: u32,
        source_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<windows::Graphics::Imaging::SoftwareBitmap, String> {
        let source = NativeDimensions::from_usize(source_width as usize, source_height as usize)?;
        if pixels.len() != source.byte_len() {
            return Err("BGRA byte length does not match source dimensions".into());
        }
        let right = x
            .checked_add(width)
            .ok_or_else(|| "SoftwareBitmap source x range overflowed".to_string())?;
        let bottom = y
            .checked_add(height)
            .ok_or_else(|| "SoftwareBitmap source y range overflowed".to_string())?;
        if right > source_width || bottom > source_height {
            return Err("SoftwareBitmap source region exceeds captured pixels".into());
        }
        let dimensions = NativeDimensions::from_usize(width as usize, height as usize)?;
        let source_stride = source_width as usize * 4;
        let source_offset = (y as usize)
            .checked_mul(source_stride)
            .and_then(|offset| offset.checked_add(x as usize * 4))
            .ok_or_else(|| "SoftwareBitmap source offset overflowed".to_string())?;
        software_bitmap_from_rows(
            &self.factory,
            pixels,
            source_stride,
            source_offset,
            dimensions,
        )
    }
}

fn software_bitmap_from_rows(
    factory: &windows::Graphics::Imaging::ISoftwareBitmapFactory,
    pixels: &[u8],
    source_stride: usize,
    source_offset: usize,
    dimensions: NativeDimensions,
) -> Result<windows::Graphics::Imaging::SoftwareBitmap, String> {
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap,
    };
    use windows::Win32::System::WinRT::IMemoryBufferByteAccess;
    use windows::core::Interface;

    // SAFETY: the factory was loaded for `SoftwareBitmap`, all value
    // parameters use the generated ABI types, and `result` is a valid
    // out-parameter converted into an owned projected object.
    let bitmap: SoftwareBitmap = unsafe {
        let mut result = core::ptr::null_mut();
        (windows::core::Interface::vtable(factory).CreateWithAlpha)(
            windows::core::Interface::as_raw(factory),
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
                crate::support::logging::report_error(
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
                    let source = source_offset
                        .checked_add(row * source_stride)
                        .and_then(|start| start.checked_add(row_bytes).map(|end| (start, end)))
                        .and_then(|(start, end)| pixels.get(start..end))
                        .ok_or_else(|| {
                            "SoftwareBitmap source row exceeds captured pixels".to_string()
                        })?;
                    std::ptr::copy_nonoverlapping(
                        source.as_ptr(),
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
                crate::support::logging::report_error("windows-native", close_error);
                Err(error)
            }
        }
    })();
    let closed = buffer
        .Close()
        .map_err(|error| format!("cannot close SoftwareBitmap buffer: {error}"));
    let fail = |error| {
        if let Err(close_error) = bitmap.Close() {
            crate::support::logging::report_error(
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
            crate::support::logging::report_error("windows-native", close_error);
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

#[must_use = "the factory must stay in its creating COM apartment"]
pub(crate) struct SystemOcrFactory {
    factory: windows::Media::Ocr::IOcrEngineStatics,
    _thread: PhantomData<Rc<()>>,
}

impl SystemOcrFactory {
    pub(crate) fn load() -> Result<Self, String> {
        use windows::Media::Ocr::{IOcrEngineStatics, OcrEngine};

        let factory = windows::core::imp::load_factory::<OcrEngine, IOcrEngineStatics>()
            .map_err(|error| format!("cannot load OcrEngine factory: {error}"))?;
        Ok(Self {
            factory,
            _thread: PhantomData,
        })
    }

    pub(crate) fn create_engine(&self) -> Result<windows::Media::Ocr::OcrEngine, String> {
        self.query(false).map(|(engine, _)| engine)
    }

    fn query(&self, include_metadata: bool) -> Result<LoadedSystemOcr, String> {
        // SAFETY: the local factory implements `IOcrEngineStatics`. Every result
        // slot has the exact generated ABI type and is converted immediately into
        // an owned projection before the factory can be released.
        unsafe {
            let mut result = core::ptr::null_mut();
            (windows::core::Interface::vtable(&self.factory).TryCreateFromUserProfileLanguages)(
                windows::core::Interface::as_raw(&self.factory),
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
            (windows::core::Interface::vtable(&self.factory).MaxImageDimension)(
                windows::core::Interface::as_raw(&self.factory),
                &mut maximum,
            )
            .ok()
            .map_err(|error| format!("cannot read OcrEngine maximum image dimension: {error}"))?;
            let mut languages = core::ptr::null_mut();
            (windows::core::Interface::vtable(&self.factory).AvailableRecognizerLanguages)(
                windows::core::Interface::as_raw(&self.factory),
                &mut languages,
            )
            .ok()
            .map_err(|error| format!("cannot enumerate OCR languages: {error}"))?;
            let languages = windows::core::Type::from_abi(languages)
                .map_err(|error| format!("cannot own OCR language collection: {error}"))?;
            Ok((engine, Some((maximum, languages))))
        }
    }
}

#[cfg(test)]
pub(crate) fn create_system_ocr_engine() -> Result<windows::Media::Ocr::OcrEngine, String> {
    SystemOcrFactory::load()?.create_engine()
}

pub(crate) fn probe_system_ocr_factory() -> Result<(u32, Vec<String>), String> {
    let (engine, metadata) = SystemOcrFactory::load()?.query(true)?;
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
