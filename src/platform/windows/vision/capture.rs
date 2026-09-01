//! Capture geometry, image partitioning, and provider input construction.

use super::*;

pub(super) struct FallbackInput {
    pub(super) gray: Vec<u8>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) desktop_bounds: Rect,
}

pub(super) fn capture_geometry(bounds: Rect) -> Result<CaptureGeometry, String> {
    if bounds.width < 2.0 || bounds.height < 2.0 {
        return Err("visual capture bounds are empty".into());
    }
    let edge_scale = (MAX_CAPTURE_EDGE / bounds.width.max(bounds.height)).min(1.0);
    let pixel_scale = (MAX_CAPTURE_PIXELS / (bounds.width * bounds.height))
        .sqrt()
        .min(1.0);
    let scale = edge_scale.min(pixel_scale);
    Ok(CaptureGeometry {
        width: (bounds.width * scale).round().max(2.0) as u32,
        height: (bounds.height * scale).round().max(2.0) as u32,
        desktop_bounds: bounds,
        scale,
    })
}

pub(super) fn system_ocr_grid_for_parallelism(width: u32, height: u32, parallelism: usize) -> u32 {
    let cpu_grid = parallelism.max(1).isqrt().saturating_add(1);
    let size_grid = (width / MIN_SYSTEM_OCR_TILE_SIDE)
        .min(height / MIN_SYSTEM_OCR_TILE_SIDE)
        .max(1) as usize;
    cpu_grid.min(size_grid).max(1) as u32
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SystemOcrLayout {
    pub(super) grid: u32,
    pub(super) tile_count: usize,
    pub(super) max_in_flight: usize,
}

impl SystemOcrLayout {
    pub(super) fn new(geometry: CaptureGeometry) -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(MAX_SYSTEM_OCR_IN_FLIGHT);
        let grid = system_ocr_grid_for_parallelism(geometry.width, geometry.height, parallelism);
        let tile_count = (grid as usize).saturating_mul(grid as usize);
        let max_in_flight = {
            let default = system_ocr_concurrency_for(tile_count, parallelism);
            #[cfg(feature = "perf-probe")]
            {
                let mut selected = default;
                if let Some(value) = std::env::var_os("KEYSTEER_OCR_IN_FLIGHT") {
                    let value = value.to_string_lossy();
                    if value.eq_ignore_ascii_case("unbounded") {
                        selected = tile_count.max(1);
                    } else if let Ok(limit) = value.parse::<usize>() {
                        selected = tile_count.min(limit.max(1)).max(1);
                    }
                }
                selected
            }
            #[cfg(not(feature = "perf-probe"))]
            {
                default
            }
        };
        Self {
            grid,
            tile_count,
            max_in_flight,
        }
    }
}

pub(super) fn system_ocr_concurrency_for(tile_count: usize, parallelism: usize) -> usize {
    tile_count
        .min(parallelism.max(1))
        .clamp(1, MAX_SYSTEM_OCR_IN_FLIGHT)
}

pub(super) fn scaled_partition(value: u32, index: u32, divisions: u32) -> u32 {
    ((u64::from(value) * u64::from(index)) / u64::from(divisions)) as u32
}

pub(super) fn capture_pixel_rect(
    geometry: CaptureGeometry,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Rect {
    image_to_desktop(
        geometry,
        Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(width),
            f64::from(height),
        ),
    )
}

pub(super) fn wechat_full_frame_from_bgra(
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: &crate::platform::windows::native::SoftwareBitmapFactory,
) -> Result<WechatFullFrame, String> {
    let bitmap = factory.bgra(pixels, geometry.width, geometry.height)?;
    crate::support::perf_probe::mark("ocr_bitmap_ready");
    Ok(WechatFullFrame {
        geometry,
        bitmap: Some(bitmap),
        _ledger: crate::support::perf_probe::ResourceGuard::new(
            crate::support::perf_probe::ResourceKind::Bitmap,
        ),
    })
}

pub(super) fn submit_wechat_full_frame(
    input: &mut Option<mpsc::SyncSender<WechatInput>>,
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: Option<&crate::platform::windows::native::SoftwareBitmapFactory>,
    factory_error: Option<&str>,
) {
    let Some(input) = input.take() else {
        return;
    };
    let Some(factory) = factory else {
        let _ = input.send(WechatInput::Failed(
            factory_error
                .unwrap_or("cannot create the COM apartment for the WeChat OCR bitmap")
                .into(),
        ));
        return;
    };
    match wechat_full_frame_from_bgra(pixels, geometry, factory) {
        Ok(frame) => {
            let _ = input.send(WechatInput::Frame(frame));
        }
        Err(error) => {
            let _ = input.send(WechatInput::Failed(error));
        }
    }
}

pub(super) fn system_ocr_tile_from_bgra(
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: &crate::platform::windows::native::SoftwareBitmapFactory,
    core_x: u32,
    core_y: u32,
    core_right: u32,
    core_bottom: u32,
) -> Result<SystemOcrTile, String> {
    let bitmap_x = core_x.saturating_sub(SYSTEM_OCR_TILE_OVERLAP);
    let bitmap_y = core_y.saturating_sub(SYSTEM_OCR_TILE_OVERLAP);
    let bitmap_right = core_right
        .saturating_add(SYSTEM_OCR_TILE_OVERLAP)
        .min(geometry.width);
    let bitmap_bottom = core_bottom
        .saturating_add(SYSTEM_OCR_TILE_OVERLAP)
        .min(geometry.height);
    let tile_width = bitmap_right - bitmap_x;
    let tile_height = bitmap_bottom - bitmap_y;
    let bitmap = factory.bgra_region(
        pixels,
        geometry.width,
        geometry.height,
        bitmap_x,
        bitmap_y,
        tile_width,
        tile_height,
    )?;
    let desktop_bounds = capture_pixel_rect(geometry, bitmap_x, bitmap_y, tile_width, tile_height);
    Ok(SystemOcrTile {
        geometry: CaptureGeometry {
            width: tile_width,
            height: tile_height,
            desktop_bounds,
            scale: geometry.scale,
        },
        core_bounds: capture_pixel_rect(
            geometry,
            core_x,
            core_y,
            core_right - core_x,
            core_bottom - core_y,
        ),
        bitmap: SharedSoftwareBitmap {
            bitmap: Some(bitmap),
            _ledger: crate::support::perf_probe::ResourceGuard::new(
                crate::support::perf_probe::ResourceKind::Bitmap,
            ),
        },
    })
}

pub(super) fn stream_system_ocr_tiles(
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: &crate::platform::windows::native::SoftwareBitmapFactory,
    submission: &SystemOcrSubmission,
    mut progress: impl FnMut(Option<usize>) -> Result<(), String>,
) -> Result<(), String> {
    let expected = geometry.width as usize * geometry.height as usize * 4;
    if pixels.len() != expected {
        return Err("captured BGRA byte length does not match system OCR geometry".into());
    }
    if submission
        .sender
        .send(SystemOcrInput::Begin {
            tile_count: submission.layout.tile_count,
        })
        .is_err()
    {
        return Ok(());
    }
    let mut index = 0usize;
    let mut in_flight = 0usize;
    for row in 0..submission.layout.grid {
        for column in 0..submission.layout.grid {
            while in_flight == submission.layout.max_in_flight {
                progress(None)?;
                let remaining = submission
                    .deadline
                    .saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("system OCR tile submission timed out".into());
                }
                match submission.credits.recv_timeout(remaining) {
                    Ok(()) => {
                        in_flight = in_flight.saturating_sub(1);
                        progress(None)?;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        return Err("system OCR tile submission timed out".into());
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
            let core_x = scaled_partition(geometry.width, column, submission.layout.grid);
            let core_right = scaled_partition(geometry.width, column + 1, submission.layout.grid);
            let core_y = scaled_partition(geometry.height, row, submission.layout.grid);
            let core_bottom = scaled_partition(geometry.height, row + 1, submission.layout.grid);
            let tile = system_ocr_tile_from_bgra(
                pixels,
                geometry,
                factory,
                core_x,
                core_y,
                core_right,
                core_bottom,
            )?;
            if submission.sender.send(SystemOcrInput::Tile(tile)).is_err() {
                return Ok(());
            }
            in_flight += 1;
            progress(Some(index))?;
            index += 1;
        }
    }
    let _ = submission.sender.send(SystemOcrInput::Done);
    Ok(())
}

#[cfg(test)]
pub(super) fn fallback_input_from_bgra(
    pixels: &[u8],
    geometry: CaptureGeometry,
) -> Result<FallbackInput, String> {
    fallback_input_from_bgra_with_progress(pixels, geometry, || Ok(false))?
        .ok_or_else(|| "fallback grayscale conversion was cancelled".into())
}

pub(super) fn fallback_input_from_bgra_with_progress(
    pixels: &[u8],
    geometry: CaptureGeometry,
    mut cancelled: impl FnMut() -> Result<bool, String>,
) -> Result<Option<FallbackInput>, String> {
    let source_width = geometry.width as usize;
    let source_height = geometry.height as usize;
    let expected = source_width
        .checked_mul(source_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "captured image dimensions overflow".to_string())?;
    if pixels.len() != expected {
        return Err("captured BGRA length does not match its geometry".into());
    }
    let edge_scale = (MAX_FALLBACK_EDGE / source_width.max(source_height) as f64).min(1.0);
    let pixel_scale = (MAX_FALLBACK_PIXELS / (source_width * source_height) as f64)
        .sqrt()
        .min(1.0);
    let analysis_scale = edge_scale.min(pixel_scale);
    let width = (source_width as f64 * analysis_scale).round().max(2.0) as usize;
    let height = (source_height as f64 * analysis_scale).round().max(2.0) as usize;
    let mut gray = Vec::with_capacity(width * height);
    if width == source_width && height == source_height {
        // 1080p and smaller captures need no coordinate tables or zero-fill.
        for (y, row) in pixels.chunks_exact(source_width * 4).enumerate() {
            if y.is_multiple_of(32) && cancelled()? {
                return Ok(None);
            }
            gray.extend(row.as_chunks::<4>().0.iter().map(|pixel| bgra_luma(pixel)));
        }
    } else if source_width.is_multiple_of(width) && source_height.is_multiple_of(height) {
        // Native 4K commonly becomes an exact 2:1 1080p analysis image.
        // Stepping source rows and pixels avoids both offset allocations.
        let x_step = source_width / width;
        let y_step = source_height / height;
        for (y, source_y) in (0..source_height).step_by(y_step).enumerate() {
            if y.is_multiple_of(32) && cancelled()? {
                return Ok(None);
            }
            let row = &pixels[source_y * source_width * 4..][..source_width * 4];
            for source_x in (0..source_width).step_by(x_step) {
                gray.push(bgra_luma(&row[source_x * 4..source_x * 4 + 4]));
            }
        }
    } else {
        let source_x_offsets = nearest_offsets(source_width, width);
        let source_y_offsets = nearest_offsets(source_height, height);
        for (y, source_y) in source_y_offsets.into_iter().enumerate() {
            if y.is_multiple_of(32) && cancelled()? {
                return Ok(None);
            }
            for source_x in source_x_offsets.iter().copied() {
                let source = (source_y * source_width + source_x) * 4;
                gray.push(bgra_luma(&pixels[source..source + 4]));
            }
        }
    }
    Ok(Some(FallbackInput {
        gray,
        width,
        height,
        desktop_bounds: geometry.desktop_bounds,
    }))
}

#[inline]
pub(super) fn bgra_luma(pixel: &[u8]) -> u8 {
    ((u16::from(pixel[2]) * 77 + u16::from(pixel[1]) * 150 + u16::from(pixel[0]) * 29) >> 8) as u8
}

/// Return the exact `floor(output * source / destination)` mapping without a
/// division in the per-pixel conversion loop.
pub(super) fn nearest_offsets(source: usize, destination: usize) -> Vec<usize> {
    debug_assert!(source != 0 && destination != 0);
    let step = source / destination;
    let remainder_step = source % destination;
    let mut source_index = 0usize;
    let mut remainder = 0usize;
    let mut offsets = Vec::with_capacity(destination);
    for _ in 0..destination {
        offsets.push(source_index.min(source - 1));
        source_index += step;
        remainder += remainder_step;
        if remainder >= destination {
            source_index += 1;
            remainder -= destination;
        }
    }
    offsets
}
