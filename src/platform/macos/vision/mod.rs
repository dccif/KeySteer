use std::ffi::c_char;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct PointSampler {
    native: *mut std::ffi::c_void,
}

impl PointSampler {
    pub(super) fn sample(
        &mut self,
        request: Option<crate::api::point_sample::Request>,
    ) -> Option<crate::api::Color> {
        let point = request.map(|r| r.point).unwrap_or_default();
        let mut rgba = [0u8; 4];
        // SAFETY: the opaque owner is created, used and released only on this
        // worker. The bridge writes four bytes synchronously and retains neither
        // stack pointer. Its late capture callbacks retain their own native owner.
        let ok = unsafe {
            NmkSamplePoint(
                &mut self.native,
                point.x,
                point.y,
                request.is_none(),
                rgba.as_mut_ptr(),
            )
        };
        ok.then(|| crate::api::Color::rgb(rgba[0], rgba[1], rgba[2]))
    }
}

impl Drop for PointSampler {
    fn drop(&mut self) {
        self.sample(None);
    }
}

use super::scan_workers::{Signal, Workers};
use crate::api::command::{UiScanStatus, VisionOptions};
use crate::api::geometry::{Rect, SemanticRole, UiTarget};
use crate::platform::common::image_tiles::{MAX_OCR_IN_FLIGHT, OcrLayout, Tile};
use crate::platform::common::spatial_index::{SpatialIndex, TargetSource};

pub(super) fn run_internal_ocr_helper() -> Option<std::process::ExitCode> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--keysteer-vision-ocr"))
        || arguments.next().is_some()
    {
        return None;
    }
    // Vision may dispatch work or load models on other threads. Run only after
    // dyld has completed initialization, before the application runtime starts.
    Some(if NmkRunVisionOcrProcess() == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativePoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeRect {
    origin: NativePoint,
    size: NativeSize,
}

impl From<Rect> for NativeRect {
    fn from(rect: Rect) -> Self {
        Self {
            origin: NativePoint {
                x: rect.x,
                y: rect.y,
            },
            size: NativeSize {
                width: rect.width,
                height: rect.height,
            },
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeConfig {
    detect_text: bool,
    detect_rectangles: bool,
    detect_contours: bool,
    timeout_ms: u64,
    minimum_confidence: f64,
    rectangle_max_candidates: u64,
    rectangle_min_size: f64,
    rectangle_min_aspect: f64,
    rectangle_max_aspect: f64,
    maximum_capture_pixels: u64,
    maximum_capture_dimension: u64,
}

#[repr(C)]
struct NativeRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    confidence: f64,
    is_text: bool,
    label: *mut c_char,
    label_len: u64,
}

#[repr(C)]
struct NativeResult {
    abi_version: u32,
    result_size: u32,
    region_stride: u32,
    status: i32,
    regions: *mut NativeRegion,
    count: u64,
    message: *mut c_char,
    message_len: u64,
    captured_bounds: NativeRect,
    image: usize,
    gray: *mut u8,
    gray_width: u64,
    gray_height: u64,
    pixel_owner: usize,
    pixel_length: u64,
    pixel_stride: u64,
}

const MAX_VISION_LABEL_BYTES: usize = 64 * 1024;
const MAX_VISION_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_CAPTURE_PIXELS: u64 = 8_388_608;
const MAX_CAPTURE_DIMENSION: u64 = 4_096;
const VISION_ABI_VERSION: u32 = 4;
const MIN_OCR_TILE_SIDE: u32 = 384;
const OCR_TILE_OVERLAP: usize = 64;

struct OwnedVisionResult(NonNull<NativeResult>);

// CGImage is immutable. Each provider group owns a separate native retain,
// independent of the capture result. Only this opaque
// image handle crosses threads; result buffers stay on their creating thread.
struct RetainedVisionImage(usize);

impl RetainedVisionImage {
    fn new(image: usize) -> Result<Self, String> {
        let image = NmkRetainVisionImage(image);
        if image == 0 {
            Err("Vision returned no captured image".into())
        } else {
            Ok(Self(image))
        }
    }

    fn dimensions(&self) -> Result<(u32, u32), String> {
        let width =
            u32::try_from(NmkVisionImageWidth(self.0)).map_err(|_| "invalid OCR image width")?;
        let height =
            u32::try_from(NmkVisionImageHeight(self.0)).map_err(|_| "invalid OCR image height")?;
        if width == 0 || height == 0 {
            return Err("empty OCR image".into());
        }
        Ok((width, height))
    }
}

impl Drop for RetainedVisionImage {
    fn drop(&mut self) {
        NmkReleaseVisionImage(self.0);
    }
}

impl OwnedVisionResult {
    fn new(result: *mut NativeResult) -> Result<Option<Self>, String> {
        let Some(result) = NonNull::new(result) else {
            return Ok(None);
        };
        if result.as_ptr().addr() % std::mem::align_of::<NativeResult>() != 0 {
            return Err("Vision returned a misaligned result pointer".into());
        }
        Ok(Some(Self(result)))
    }

    fn as_ref(&self) -> &NativeResult {
        // SAFETY: the bridge returned an owned non-null result and this wrapper
        // retains it until Drop. No mutable access is exposed while borrowed.
        unsafe { self.0.as_ref() }
    }

    fn contour_pixels(&self) -> Result<(&[u8], usize, usize, usize), String> {
        let raw = self.as_ref();
        let width = usize::try_from(raw.gray_width).map_err(|_| "invalid contour width")?;
        let height = usize::try_from(raw.gray_height).map_err(|_| "invalid contour height")?;
        let stride = usize::try_from(raw.pixel_stride).map_err(|_| "invalid contour stride")?;
        let area = width
            .checked_mul(height)
            .filter(|&n| n > 0)
            .ok_or("empty contour frame")?;
        let row_bytes = if stride == 0 {
            if area > crate::platform::common::contour::MAX_PIXELS {
                return Err("contour frame exceeds its luma budget".into());
            }
            width
        } else {
            if raw.pixel_owner == 0
                || width as u64 > MAX_CAPTURE_DIMENSION
                || height as u64 > MAX_CAPTURE_DIMENSION
                || area as u64 > MAX_CAPTURE_PIXELS + 2 * MAX_CAPTURE_DIMENSION + 1
                || width.checked_mul(4).is_none_or(|row| row > stride)
            {
                return Err("invalid packed contour frame".into());
            }
            stride
        };
        let length = row_bytes
            .checked_mul(height)
            .filter(|&n| n <= isize::MAX as usize && n as u64 <= raw.pixel_length)
            .ok_or("invalid contour byte length")?;
        let pixels = NonNull::new(raw.gray).ok_or("missing contour frame")?;
        // SAFETY: the version-checked result owns either a bounded luma allocation
        // or CFData with at least `length` immutable bytes. This borrow ends before Drop.
        let pixels = unsafe { std::slice::from_raw_parts(pixels.as_ptr(), length) };
        Ok((pixels, width, height, stride))
    }

    fn regions(&self) -> Result<&[NativeRegion], String> {
        let raw = self.as_ref();
        if raw.abi_version != VISION_ABI_VERSION
            || raw.result_size as usize != std::mem::size_of::<NativeResult>()
            || raw.region_stride as usize != std::mem::size_of::<NativeRegion>()
        {
            return Err(format!(
                "Vision ABI mismatch: version={}, result_size={}, region_stride={}",
                raw.abi_version, raw.result_size, raw.region_stride
            ));
        }
        let count = usize::try_from(raw.count)
            .map_err(|_| "Vision returned an unrepresentable region count".to_string())?;
        if count
            .checked_mul(std::mem::size_of::<NativeRegion>())
            .is_none_or(|bytes| bytes > isize::MAX as usize)
        {
            return Err("Vision region buffer does not fit the address space".into());
        }
        if count == 0 {
            return Ok(&[]);
        }
        let regions = NonNull::new(raw.regions)
            .ok_or_else(|| "Vision returned regions with a null pointer".to_string())?;
        if regions.as_ptr().addr() % std::mem::align_of::<NativeRegion>() != 0 {
            return Err("Vision returned a misaligned region pointer".into());
        }
        let _byte_len = count
            .checked_mul(std::mem::size_of::<NativeRegion>())
            .filter(|&length| length <= isize::MAX as usize)
            .ok_or_else(|| "Vision returned an unrepresentable region buffer".to_string())?;
        // SAFETY: the native bridge allocates `count` contiguous NativeRegion
        // values, the count is capped by the shared ABI maximum, and the owner
        // keeps the allocation alive for the returned borrow.
        Ok(unsafe { std::slice::from_raw_parts(regions.as_ptr(), count) })
    }
}

impl Drop for OwnedVisionResult {
    fn drop(&mut self) {
        // SAFETY: this wrapper is constructed only from an owned bridge result
        // and Drop is the sole release path.
        unsafe { NmkFreeVisionResult(self.0.as_ptr()) };
    }
}

unsafe extern "C" {
    fn NmkSamplePoint(
        owner: *mut *mut std::ffi::c_void,
        x: f64,
        y: f64,
        reset: bool,
        rgba: *mut u8,
    ) -> bool;
    safe fn NmkSetLatestVisionScan(scan_id: u64);
    safe fn NmkRunVisionOcrProcess() -> i32;
    safe fn NmkVisionOcrPerformanceCores() -> u32;
    safe fn NmkCreateVisionCaptureContext() -> usize;
    safe fn NmkReleaseVisionCaptureContext(context: usize);
    safe fn NmkCancelVisionScan(scan_id: u64);
    safe fn NmkRetainVisionImage(image: usize) -> usize;
    safe fn NmkReleaseVisionImage(image: usize);
    safe fn NmkVisionImageWidth(image: usize) -> u64;
    safe fn NmkVisionImageHeight(image: usize) -> u64;
    safe fn NmkWriteVisionOcrImage(image: usize, fd: i32, scan_id: u64) -> bool;
    safe fn NmkPrepareVisionContourImage(image: usize, scan_id: u64) -> *mut NativeResult;
    safe fn NmkDetectVisionElements(
        bounds: NativeRect,
        config: NativeConfig,
        scan_id: u64,
        capture_context: usize,
    ) -> *mut NativeResult;
    safe fn NmkRecognizeVisionImage(
        image: usize,
        bounds: NativeRect,
        config: NativeConfig,
        scan_id: u64,
    ) -> *mut NativeResult;
    fn NmkFreeVisionResult(result: *mut NativeResult);
}

#[derive(Debug, Clone)]
struct Candidate {
    target: UiTarget,
    confidence: f64,
    is_text: bool,
}

pub(super) fn mark_latest(scan_id: u64) {
    NmkSetLatestVisionScan(scan_id);
}

#[derive(Default)]
pub(super) struct CaptureCache(usize);

impl CaptureCache {
    fn handle(&mut self) -> Result<usize, String> {
        if self.0 == 0 {
            self.0 = NmkCreateVisionCaptureContext();
        }
        if self.0 == 0 {
            Err("cannot create screen capture filter cache".into())
        } else {
            Ok(self.0)
        }
    }
}

impl Drop for CaptureCache {
    fn drop(&mut self) {
        NmkReleaseVisionCaptureContext(self.0);
    }
}

struct RecognitionFrame {
    image: RetainedVisionImage,
    captured_bounds: NativeRect,
    coordinate_bounds: Rect,
}

// One queue per source for this scan's actual display intersections. A native
// call owns only its current image; queued images can be drained on cancellation
// even if an old AX/rectangle call has not returned.
struct ProviderInput {
    sender: Option<mpsc::Sender<RecognitionFrame>>,
    receiver: Arc<Mutex<mpsc::Receiver<RecognitionFrame>>>,
}

impl Drop for ProviderInput {
    fn drop(&mut self) {
        self.sender.take();
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for frame in receiver.try_iter() {
            drop(frame);
        }
    }
}

pub(super) fn detect(
    (scan_id, cache): (u64, &mut CaptureCache),
    regions: (
        impl IntoIterator<Item = Rect>,
        crate::platform::common::scan_priority::ScanPriority,
    ),
    options: &VisionOptions,
    strategy: crate::api::UiScanStrategy,
    workers: &Workers,
    cancelled: impl Fn() -> bool + Send + Sync + 'static,
    publisher: super::ui_scan::PartialPublisher,
) -> UiScanStatus {
    let run = || {
        detect_inner(
            (scan_id, cache.handle()?),
            regions,
            options,
            strategy,
            workers,
            cancelled,
            publisher,
        )
    };
    run().unwrap_or_else(UiScanStatus::Failed)
}

fn detect_inner(
    (scan_id, capture_context): (u64, usize),
    (regions, priority): (
        impl IntoIterator<Item = Rect>,
        crate::platform::common::scan_priority::ScanPriority,
    ),
    options: &VisionOptions,
    strategy: crate::api::UiScanStrategy,
    workers: &Workers,
    cancelled: impl Fn() -> bool + Send + Sync + 'static,
    publisher: super::ui_scan::PartialPublisher,
) -> Result<UiScanStatus, String> {
    let super::ui_scan::PartialPublisher {
        sink: output,
        preview,
    } = publisher;
    let deadline =
        Instant::now() + Duration::from_millis(options.request_timeout_ms.clamp(1, 30_000));
    let cancelled = Arc::new(cancelled);
    use crate::platform::common::scan_progress::Progress;
    let contour_only = strategy == crate::api::UiScanStrategy::Contour;
    let (detect_text, detect_contours) =
        crate::platform::common::contour::sources(strategy, options);
    let candidate_limit = crate::platform::common::contour::candidate_limit(strategy, options);
    let config = NativeConfig {
        detect_text,
        detect_rectangles: !contour_only && detect_contours,
        detect_contours,
        timeout_ms: options.request_timeout_ms,
        minimum_confidence: options.minimum_confidence,
        rectangle_max_candidates: if candidate_limit == usize::MAX {
            0
        } else {
            candidate_limit as u64
        },
        rectangle_min_size: options.rectangle_min_size,
        rectangle_min_aspect: options.rectangle_min_aspect,
        rectangle_max_aspect: options.rectangle_max_aspect,
        // Contours already use this luma budget. Apply it before the GPU
        // readback too when no OCR/rectangle source needs the original pixels.
        maximum_capture_pixels: if contour_only {
            2_073_600
        } else {
            MAX_CAPTURE_PIXELS
        },
        maximum_capture_dimension: if contour_only {
            2560
        } else {
            MAX_CAPTURE_DIMENSION
        },
    };
    let mut regions: smallvec::SmallVec<[Rect; 2]> = regions.into_iter().collect();
    priority.sort(&mut regions, |r| *r);
    static PERFORMANCE_CORES: OnceLock<usize> = OnceLock::new();
    let (frame_workers, request_budget) = ocr_concurrency(
        regions.len(),
        std::thread::available_parallelism().map_or(2, usize::from),
        *PERFORMANCE_CORES.get_or_init(|| NmkVisionOcrPerformanceCores() as usize),
    );
    let mut providers = smallvec::SmallVec::<[_; 3]>::new();
    for (source, enabled) in [
        (TargetSource::SystemOcr, config.detect_text),
        (TargetSource::NativeRectangles, config.detect_rectangles),
        (TargetSource::Contour, config.detect_contours),
    ] {
        if !enabled {
            continue;
        }
        output.progress(source, Progress::Plan(regions.to_vec()));
        let (sender, receiver) = mpsc::channel();
        let input = ProviderInput {
            sender: Some(sender),
            receiver: Arc::new(Mutex::new(receiver)),
        };
        let batch = workers.start_with_resources(&[source], {
            let receiver = Arc::clone(&input.receiver);
            let options = options.clone();
            let cancelled = Arc::clone(&cancelled);
            let output = output.clone();
            let preview = preview.clone();
            move |_, stopped, resources| {
                if source == TargetSource::SystemOcr {
                    stopped.reuse_processes(frame_workers);
                }
                let cancelled = || stopped.load(Ordering::Acquire) || cancelled();
                let publish = |targets| {
                    output.submit(source, targets);
                };
                let progress = |progress| output.progress(source, progress);
                let provider_config = NativeConfig {
                    detect_text: source == TargetSource::SystemOcr,
                    detect_rectangles: source == TargetSource::NativeRectangles,
                    ..config
                };
                // Only the serial contour branch borrows this workspace. OCR
                // display helpers receive None and never share mutable scratch.
                let consume = |tile_lanes,
                               mut contour: Option<
                    &mut crate::platform::common::contour::Workspace,
                >| {
                    let mut statuses = smallvec::SmallVec::<[UiScanStatus; 2]>::new();
                    loop {
                        if let Some(status) =
                            super::scan_workers::interrupt_status(deadline, cancelled())
                        {
                            return Some(status);
                        }
                        let frame = receiver
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
                        let frame = match frame {
                            Ok(frame) => frame,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                return Some(UiScanStatus::TimedOut);
                            }
                        };
                        // The receive lock has been released before any native work.
                        let region = frame.coordinate_bounds;
                        let status = if source == TargetSource::SystemOcr {
                            let (width, height) = match frame.image.dimensions() {
                                Ok(dimensions) => dimensions,
                                Err(error) => return Some(UiScanStatus::Failed(error)),
                            };
                            let layout = OcrLayout::new(
                                width,
                                height,
                                tile_lanes,
                                MIN_OCR_TILE_SIDE,
                                MAX_OCR_IN_FLIGHT,
                            );
                            let tiles =
                                priority.ocr_tiles(layout, width, height, OCR_TILE_OVERLAP, region);
                            progress(Progress::Refine {
                                region,
                                children: tiles
                                    .iter()
                                    .map(|tile| {
                                        pixel_rect_to_desktop(
                                            region,
                                            (width, height),
                                            Rect::new(
                                                tile.left_halo as f64,
                                                tile.top_halo as f64,
                                                (tile.right_halo - tile.left_halo) as f64,
                                                (tile.bottom_halo - tile.top_halo) as f64,
                                            ),
                                        )
                                    })
                                    .collect(),
                            });
                            recognize_ocr_image(
                                (
                                    frame.image,
                                    frame.coordinate_bounds,
                                    &tiles,
                                    layout.max_in_flight,
                                ),
                                scan_id,
                                &options,
                                deadline,
                                stopped,
                                &cancelled,
                                (&publish, &progress, config.detect_text.then_some(&preview)),
                            )
                        } else if source == TargetSource::Contour {
                            let Some(workspace) = contour.as_deref_mut() else {
                                return Some(UiScanStatus::Failed(
                                    "missing contour workspace".into(),
                                ));
                            };
                            recognize_contours(
                                frame,
                                scan_id,
                                candidate_limit,
                                deadline,
                                &cancelled,
                                &publish,
                                workspace,
                            )
                        } else {
                            recognize_image(
                                (frame.image.0, frame.captured_bounds),
                                provider_config,
                                scan_id,
                                &options,
                                deadline,
                                &cancelled,
                                &publish,
                            )
                        };
                        if source != TargetSource::SystemOcr {
                            progress(Progress::Complete(region));
                        }
                        if status == UiScanStatus::ContextChanged {
                            return Some(status);
                        }
                        statuses.push(status);
                    }
                    (!statuses.is_empty()).then(|| combined_status(&statuses))
                };
                if source != TargetSource::SystemOcr || frame_workers == 1 {
                    return consume(request_budget, Some(&mut resources.contour))
                        .unwrap_or(UiScanStatus::Success);
                }
                // Split one scan-wide request budget between display workers.
                // Every worker has at least one lane; the remainder is shared
                // without exceeding the hardware budget.
                std::thread::scope(|scope| {
                    let lanes = |index| display_request_lanes(request_budget, frame_workers, index);
                    let consume = &consume;
                    let mut displays = smallvec::SmallVec::<[_; 2]>::new();
                    for index in 1..frame_workers {
                        let lane_count = lanes(index);
                        if let Ok(worker) = std::thread::Builder::new()
                            .name("keysteer-ocr-display".into())
                            .spawn_scoped(scope, move || {
                                objc2::rc::autoreleasepool(|_| consume(lane_count, None))
                            })
                        {
                            displays.push(worker);
                        }
                    }
                    let mut statuses = smallvec::SmallVec::<[UiScanStatus; 2]>::new();
                    statuses.extend(consume(lanes(0), None));
                    for display in displays {
                        statuses.extend(display.join().unwrap_or_else(|_| {
                            Some(UiScanStatus::Failed("OCR display worker panicked".into()))
                        }));
                    }
                    combined_status(&statuses)
                })
            }
        });
        providers.push((input, batch));
    }
    let mut statuses = smallvec::SmallVec::<[UiScanStatus; 4]>::new();
    let mut captures = 0;
    for bounds in regions {
        if let Some(status) = super::scan_workers::interrupt_status(deadline, cancelled()) {
            statuses.push(status);
            break;
        }
        let mut capture_config = config;
        capture_config.timeout_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .clamp(1, 30_000) as u64;
        let captured_at = Instant::now();
        let capture = OwnedVisionResult::new(NmkDetectVisionElements(
            NativeRect::from(bounds),
            capture_config,
            scan_id,
            capture_context,
        ))?
        .ok_or("Vision could not allocate a capture result")?;
        capture.regions()?;
        let raw = capture.as_ref();
        if raw.status != 0 {
            statuses.push(decode_result(&capture, bounds, options).1);
            continue;
        }
        if raw.image == 0 && !providers.is_empty() {
            return Err("Vision returned no captured image".into());
        }
        captures += 1;
        let coordinate_bounds =
            capture_bounds_or_window(native_rect_to_rect(raw.captured_bounds), bounds);
        for source in [
            TargetSource::SystemOcr,
            TargetSource::NativeRectangles,
            TargetSource::Contour,
        ] {
            let enabled = match source {
                TargetSource::SystemOcr => config.detect_text,
                TargetSource::NativeRectangles => config.detect_rectangles,
                _ => config.detect_contours,
            };
            if enabled {
                output.progress(
                    source,
                    Progress::Refine {
                        region: bounds,
                        children: vec![coordinate_bounds],
                    },
                );
            }
        }
        // Queue all sources before starting the next display's capture.
        // The single capture permit still prevents overlapping full screenshots.
        for (input, _) in &providers {
            let frame = RecognitionFrame {
                image: RetainedVisionImage::new(raw.image)?,
                captured_bounds: raw.captured_bounds,
                coordinate_bounds,
            };
            if input
                .sender
                .as_ref()
                .is_some_and(|sender| sender.send(frame).is_err())
            {
                statuses.push(UiScanStatus::Failed(
                    "native source stopped before capture delivery".into(),
                ));
            }
        }
        crate::log_info!(
            "macos-ui-scan",
            "scan {scan_id}: capture delivered after {} ms",
            captured_at.elapsed().as_millis()
        );
        drop(capture);
    }
    // EOF wakes idle providers. Keep the drain owners until after cancellation
    // so pending screenshots are freed independently of a stuck native call.
    for (input, _) in &mut providers {
        input.sender.take();
    }
    for (input, batch) in providers {
        let completed = batch.wait(deadline, || cancelled());
        if captures != 0 {
            statuses.extend(completed);
        }
        drop(input);
    }
    if let Some(status) = super::scan_workers::interrupt_status(deadline, cancelled()) {
        NmkCancelVisionScan(scan_id);
        statuses.push(status);
    }
    if captures == 0 && statuses.is_empty() {
        return Err("screen scan requires display bounds".into());
    }
    Ok(combined_status(&statuses))
}

fn recognize_contours(
    frame: RecognitionFrame,
    scan_id: u64,
    candidate_limit: usize,
    deadline: Instant,
    cancelled: &(impl Fn() -> bool + Sync),
    publish: &impl Fn(Vec<UiTarget>),
    workspace: &mut crate::platform::common::contour::Workspace,
) -> UiScanStatus {
    let started = Instant::now();
    let interrupted = || super::scan_workers::interrupt_status(deadline, cancelled());
    let run = || -> Result<UiScanStatus, String> {
        if let Some(status) = interrupted() {
            return Ok(status);
        }
        let result = OwnedVisionResult::new(NmkPrepareVisionContourImage(frame.image.0, scan_id))?
            .ok_or("Vision could not allocate a contour frame")?;
        result.regions()?;
        let raw = result.as_ref();
        if raw.status != 0 {
            return Ok(native_status(raw.status, String::new()));
        }
        let (native, input) = if raw.pixel_stride != 0 {
            let (pixels, width, height, stride) = result.contour_pixels()?;
            let input = crate::platform::common::contour::input::from_bgra(
                pixels,
                (width, height, stride),
                frame.coordinate_bounds,
                || Ok(interrupted().is_some()),
            )?;
            if let Some(status) = interrupted() {
                return Ok(status);
            }
            // Release the full pixel backing before detecting bounded luma.
            drop(result);
            (
                None,
                Some(input.ok_or("contour conversion returned no pixels")?),
            )
        } else {
            (Some(result), None)
        };
        // Detection needs only luma. Do not retain the screenshot during it.
        drop(frame.image);
        if let Some(status) = interrupted() {
            return Ok(status);
        }
        let targets = if let Some(input) = &input {
            workspace.detect_prepared(input, candidate_limit, || interrupted().is_some())
        } else {
            let (pixels, width, height, _) = native
                .as_ref()
                .ok_or("missing contour frame")?
                .contour_pixels()?;
            workspace.detect(
                pixels,
                width,
                height,
                frame.coordinate_bounds,
                candidate_limit,
                || interrupted().is_some(),
            )
        };
        if let Some(status) = interrupted() {
            return Ok(status);
        }
        let count = targets.len();
        publish(targets);
        crate::log_info!(
            "macos-ui-scan",
            "scan {scan_id}: contour returned {count} targets after {} ms",
            started.elapsed().as_millis()
        );
        Ok(UiScanStatus::Success)
    };
    run().unwrap_or_else(UiScanStatus::Failed)
}

fn ocr_concurrency(displays: usize, cpus: usize, performance_cores: usize) -> (usize, usize) {
    // Each Accurate request also parallelizes internally. Use half the physical
    // performance cores as a conservative starting budget; keep input headroom
    // and the shared safety ceiling. Unknown hardware retains the tested two.
    let budget = if performance_cores == 0 {
        2
    } else {
        (performance_cores / 2).max(1)
    }
    .min(cpus.saturating_sub(1).max(1))
    .min(MAX_OCR_IN_FLIGHT);
    let frames = displays.max(1).min(budget);
    (frames, budget)
}

fn display_request_lanes(budget: usize, workers: usize, index: usize) -> usize {
    budget / workers + usize::from(index < budget % workers)
}

fn combined_status(statuses: &[UiScanStatus]) -> UiScanStatus {
    let status = crate::platform::common::scan_fusion::combined_status(statuses);
    if status == UiScanStatus::Success {
        for status in statuses {
            if let UiScanStatus::Failed(error) = status {
                crate::support::logging::report_error("macos-vision", error);
            }
        }
    }
    status
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum OcrMessage {
    Preview {
        index: usize,
        status: i32,
        message: String,
        regions: Vec<OcrRegion>,
    },
    Tile {
        index: usize,
        status: i32,
        message: String,
        regions: Vec<OcrRegion>,
    },
    Done,
}

#[derive(serde::Deserialize)]
struct OcrRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    confidence: f64,
    label: String,
}

fn accept_ocr_phase(
    index: usize,
    is_preview: bool,
    received: &mut [bool],
    previews: &mut [bool],
) -> Result<(), &'static str> {
    let complete = received
        .get_mut(index)
        .ok_or("OCR returned an unknown tile")?;
    if is_preview {
        let seen = previews.get_mut(index).ok_or("unexpected OCR preview")?;
        if *seen || *complete {
            return Err("OCR returned a late or duplicate preview");
        }
        *seen = true;
    } else {
        if *complete || (!previews.is_empty() && previews.get(index) != Some(&true)) {
            return Err("OCR returned a duplicate tile or omitted its preview");
        }
        *complete = true;
    }
    Ok(())
}

fn recognize_ocr_image(
    (image, bounds, tiles, parallelism): (RetainedVisionImage, Rect, &[Tile], usize),
    scan_id: u64,
    options: &VisionOptions,
    deadline: Instant,
    stopped: &Signal,
    cancelled: &impl Fn() -> bool,
    (publish, progress, preview): (
        &impl Fn(Vec<UiTarget>),
        &impl Fn(crate::platform::common::scan_progress::Progress),
        Option<&super::ui_scan::FastPreview>,
    ),
) -> UiScanStatus {
    let started = Instant::now();
    let interrupted = || super::scan_workers::interrupt_status(deadline, cancelled());
    let run = || -> Result<UiScanStatus, String> {
        if let Some(status) = interrupted() {
            return Ok(status);
        }
        let mut command = std::process::Command::new(
            std::env::current_exe()
                .map_err(|error| format!("cannot locate OCR executable: {error}"))?,
        );
        command.arg("--keysteer-vision-ocr");
        let mut process = stopped.process(&mut command)?;
        if let Some(status) = interrupted() {
            return Ok(status);
        }
        let input = process.input.as_mut().ok_or("OCR input pipe is missing")?;
        let crops: Vec<_> = tiles
            .iter()
            .map(|tile| {
                [
                    tile.left_halo,
                    tile.top_halo,
                    tile.right_halo - tile.left_halo,
                    tile.bottom_halo - tile.top_halo,
                ]
            })
            .collect();
        serde_json::to_writer(
            &mut *input,
            &serde_json::json!({
                "tiles": crops,
                "parallelism": parallelism,
                "minimum_confidence": options.minimum_confidence,
                "fast_preview": preview.is_some(),
            }),
        )
        .map_err(|error| format!("cannot send OCR layout: {error}"))?;
        input
            .write_all(b"\n")
            .map_err(|error| format!("cannot send OCR header: {error}"))?;
        let dimensions = image.dimensions()?;
        if !NmkWriteVisionOcrImage(image.0, input.as_raw_fd(), scan_id) {
            return Err("cannot transfer OCR image".into());
        }
        // The child owns its pixel backing now; retain no parent screenshot
        // while waiting for OCR or for the next display's work.
        drop(image);
        // Keep the writer open: the helper watches EOF to handle parent death,
        // including a crash while every Vision lane is permanently blocked.
        let output = process.output.take().ok_or("OCR output pipe is missing")?;
        let mut output = BufReader::new(output);
        let mut bytes = Vec::new();
        let mut received = vec![false; tiles.len()];
        let mut preview_received = if preview.is_some() {
            vec![false; tiles.len()]
        } else {
            Vec::new()
        };
        let mut published = false;
        loop {
            if let Some(status) = interrupted() {
                return Ok(status);
            }
            bytes.clear();
            // Bound IPC memory, independently of the number of scan targets.
            const MAX_MESSAGE_BYTES: u64 = 64 * 1024 * 1024;
            let length = (&mut output)
                .take(MAX_MESSAGE_BYTES + 1)
                .read_until(b'\n', &mut bytes)
                .map_err(|error| format!("cannot read OCR result: {error}"))?;
            if let Some(status) = interrupted() {
                return Ok(status);
            }
            if length == 0 || length as u64 > MAX_MESSAGE_BYTES {
                return Err("OCR process stopped before completing its results".into());
            }
            let record = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid OCR result: {error}"))?;
            let (is_preview, index, status, message, regions) = match record {
                OcrMessage::Done => {
                    return if received.iter().all(|received| *received)
                        && preview_received.iter().all(|received| *received)
                        && output.buffer().is_empty()
                    {
                        process.output = Some(output.into_inner());
                        process.completed();
                        Ok(UiScanStatus::Success)
                    } else {
                        Err("OCR process omitted a tile or sent data after completion".into())
                    };
                }
                OcrMessage::Tile {
                    index,
                    status,
                    message,
                    regions,
                } => (false, index, status, message, regions),
                OcrMessage::Preview {
                    index,
                    status,
                    message,
                    regions,
                } => (true, index, status, message, regions),
            };
            let tile = tiles.get(index).ok_or("OCR returned an unknown tile")?;
            accept_ocr_phase(index, is_preview, &mut received, &mut preview_received)?;
            let status = native_status(status, message);
            if status != UiScanStatus::Success {
                if is_preview {
                    crate::report_error!("macos-ocr-preview", "tile {index}: {status:?}");
                    continue;
                }
                return Ok(status);
            }
            let tile_bounds = pixel_rect_to_desktop(
                bounds,
                dimensions,
                Rect::new(
                    tile.left_halo as f64,
                    tile.top_halo as f64,
                    (tile.right_halo - tile.left_halo) as f64,
                    (tile.bottom_halo - tile.top_halo) as f64,
                ),
            );
            let core = pixel_rect_to_desktop(
                bounds,
                dimensions,
                Rect::new(
                    tile.x as f64,
                    tile.y as f64,
                    (tile.right - tile.x) as f64,
                    (tile.bottom - tile.y) as f64,
                ),
            );
            let mut candidates = Vec::new();
            for region in regions {
                if let Some(status) = interrupted() {
                    return Ok(status);
                }
                if region.label.len() > MAX_VISION_LABEL_BYTES {
                    return Err("OCR returned an oversized text label".into());
                }
                let rect = normalized_to_global(
                    tile_bounds,
                    Rect::new(region.x, region.y, region.width, region.height),
                );
                if rect.is_empty() || !rect.width.is_finite() || !rect.height.is_finite() {
                    continue;
                }
                candidates.push(Candidate {
                    target: if is_preview {
                        UiTarget {
                            rect,
                            name: String::new(),
                            role: SemanticRole::Control,
                            details: None,
                        }
                    } else {
                        UiTarget::recognized_text(rect, region.label)
                    },
                    confidence: region.confidence,
                    is_text: !is_preview,
                });
            }
            let mut targets = merge_candidates(candidates, options.merge_iou_threshold);
            targets.retain(|target| core.contains(&target.rect.center()));
            if let Some(status) = interrupted() {
                return Ok(status);
            }
            if is_preview {
                if let Some(preview) = preview {
                    preview.publish(targets);
                }
            } else {
                let first = !published && !targets.is_empty();
                published |= first;
                publish(targets);
                progress(crate::platform::common::scan_progress::Progress::Complete(
                    tile_bounds,
                ));
                if first {
                    crate::log_info!(
                        "macos-ui-scan",
                        "scan {scan_id}: OCR first targets after {} ms",
                        started.elapsed().as_millis()
                    );
                }
            }
        }
    };
    run().unwrap_or_else(|error| interrupted().unwrap_or(UiScanStatus::Failed(error)))
}

fn native_status(status: i32, message: String) -> UiScanStatus {
    let message = || {
        if message.is_empty() {
            "Vision scan failed".into()
        } else {
            message
        }
    };
    match status {
        0 => UiScanStatus::Success,
        1 => UiScanStatus::PermissionDenied(message()),
        2 => UiScanStatus::TimedOut,
        4 => UiScanStatus::ContextChanged,
        5 => UiScanStatus::Unsupported(message()),
        _ => UiScanStatus::Failed(message()),
    }
}

fn recognize_image(
    (image, bounds): (usize, NativeRect),
    config: NativeConfig,
    scan_id: u64,
    options: &VisionOptions,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
    publish: &impl Fn(Vec<UiTarget>),
) -> UiScanStatus {
    if let Some(status) = super::scan_workers::interrupt_status(deadline, cancelled()) {
        return status;
    }
    match OwnedVisionResult::new(NmkRecognizeVisionImage(image, bounds, config, scan_id)) {
        Ok(Some(result)) => {
            // Drop the owned native buffers immediately when a cancelled or
            // expired request returns, before allocating Rust strings/targets.
            if let Some(status) = super::scan_workers::interrupt_status(deadline, cancelled()) {
                return status;
            }
            let (targets, status) = decode_result(&result, native_rect_to_rect(bounds), options);
            if let Some(status) = super::scan_workers::interrupt_status(deadline, cancelled()) {
                return status;
            }
            publish(targets);
            status
        }
        Ok(None) => UiScanStatus::Failed("Vision could not allocate recognition result".into()),
        Err(error) => UiScanStatus::Failed(error),
    }
}

fn pixel_rect_to_desktop(bounds: Rect, (width, height): (u32, u32), rect: Rect) -> Rect {
    let scale_x = bounds.width / f64::from(width);
    let scale_y = bounds.height / f64::from(height);
    Rect::new(
        bounds.x + rect.x * scale_x,
        bounds.y + rect.y * scale_y,
        rect.width * scale_x,
        rect.height * scale_y,
    )
}

fn decode_result(
    result: &OwnedVisionResult,
    bounds: Rect,
    options: &VisionOptions,
) -> (Vec<UiTarget>, UiScanStatus) {
    let raw = result.as_ref();
    let coordinate_bounds =
        capture_bounds_or_window(native_rect_to_rect(raw.captured_bounds), bounds);
    if raw.abi_version != VISION_ABI_VERSION
        || raw.result_size as usize != std::mem::size_of::<NativeResult>()
    {
        return (
            Vec::new(),
            UiScanStatus::Failed("Vision returned incompatible result metadata".into()),
        );
    }
    let message = match native_string(raw.message, raw.message_len, MAX_VISION_MESSAGE_BYTES) {
        Ok(message) => message,
        Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
    };
    let status = native_status(raw.status, message);
    let regions = match result.regions() {
        Ok(regions) => regions,
        Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
    };
    let mut candidates = Vec::with_capacity(regions.len());
    for region in regions {
        match classify(region, coordinate_bounds, options) {
            Ok(Some(candidate)) => candidates.push(candidate),
            Ok(None) => {}
            Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
        }
    }
    let targets = merge_candidates(candidates, options.merge_iou_threshold);
    (targets, status)
}

fn native_rect_to_rect(rect: NativeRect) -> Rect {
    Rect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

fn capture_bounds_or_window(captured_bounds: Rect, window_bounds: Rect) -> Rect {
    if captured_bounds.is_empty()
        || !captured_bounds.x.is_finite()
        || !captured_bounds.y.is_finite()
        || !captured_bounds.width.is_finite()
        || !captured_bounds.height.is_finite()
    {
        window_bounds
    } else {
        captured_bounds
    }
}

fn native_string(value: *const c_char, length: u64, maximum: usize) -> Result<String, String> {
    let length = usize::try_from(length)
        .map_err(|_| "Vision returned an unrepresentable string length".to_string())?;
    if length > maximum {
        return Err(format!(
            "Vision returned a {length}-byte string; maximum is {maximum}"
        ));
    }
    if length == 0 {
        return Ok(String::new());
    }
    let value = NonNull::new(value.cast_mut())
        .ok_or_else(|| "Vision returned a non-empty string with a null pointer".to_string())?;
    // SAFETY: the ABI supplies an explicit bounded byte length, and the owning
    // result keeps every strdup allocation alive for this borrow.
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), length) };
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn classify(
    region: &NativeRegion,
    bounds: Rect,
    options: &VisionOptions,
) -> Result<Option<Candidate>, String> {
    let rect = normalized_to_global(
        bounds,
        Rect::new(region.x, region.y, region.width, region.height),
    );
    if rect.is_empty() || !rect.width.is_finite() || !rect.height.is_finite() {
        return Ok(None);
    }
    let aspect = rect.width / rect.height.max(f64::EPSILON);
    let role = if region.is_text {
        // Match the shared OCR contract used by Windows. Text provenance
        // drives metadata fusion; guessing a clickable role loses OCR details.
        SemanticRole::StaticText
    } else if rect.width <= options.checkbox_max_size
        && rect.height <= options.checkbox_max_size
        && (0.75..=1.35).contains(&aspect)
    {
        SemanticRole::Checkbox
    } else if (region.confidence >= options.button_min_confidence
        && (options.button_min_aspect..=options.button_max_aspect).contains(&aspect))
        || (region.confidence >= options.generic_clickable_min_confidence
            && rect.width <= options.button_icon_max_size
            && rect.height <= options.button_icon_max_size)
    {
        // Aspect-ratio fits and icon-sized candidates are both buttons. These
        // were separate arms only because they carried different diagnostic
        // native roles.
        SemanticRole::Button
    } else if rect.width >= options.image_min_size && rect.height >= options.image_min_size {
        SemanticRole::Image
    } else if region.confidence >= options.generic_clickable_min_confidence {
        SemanticRole::Control
    } else {
        return Ok(None);
    };
    let name = native_string(region.label, region.label_len, MAX_VISION_LABEL_BYTES)?;
    let target = if region.is_text {
        UiTarget::recognized_text(rect, name)
    } else {
        UiTarget {
            details: None,
            rect,
            name,
            role,
        }
    };
    Ok(Some(Candidate {
        target,
        confidence: region.confidence,
        is_text: region.is_text,
    }))
}

fn normalized_to_global(bounds: Rect, normalized: Rect) -> Rect {
    Rect::new(
        bounds.x + normalized.x * bounds.width,
        bounds.y + (1.0 - normalized.y - normalized.height) * bounds.height,
        normalized.width * bounds.width,
        normalized.height * bounds.height,
    )
}

fn merge_candidates(mut candidates: Vec<Candidate>, threshold: f64) -> Vec<UiTarget> {
    candidates.sort_by(|a, b| {
        b.is_text
            .cmp(&a.is_text)
            .then_with(|| b.confidence.total_cmp(&a.confidence))
    });
    let mut targets: Vec<UiTarget> = Vec::new();
    let mut index = SpatialIndex::new(64.0, 0.0, f64::EPSILON);
    for candidate in candidates {
        if index.insert_if_unique(candidate.target.rect, |existing, candidate| {
            intersection_over_union(existing, candidate) >= threshold
        }) {
            targets.push(candidate.target);
        }
    }
    targets
}

fn intersection_over_union(a: Rect, b: Rect) -> f64 {
    let Some(intersection) = a.intersect(&b) else {
        return 0.0;
    };
    let intersection_area = intersection.width * intersection.height;
    let union = a.width * a.height + b.width * b.height - intersection_area;
    if union <= 0.0 {
        0.0
    } else {
        intersection_area / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_phases_reject_missing_duplicate_and_unexpected_records() {
        let mut received = [false, false];
        let mut previews = [false, false];
        assert!(accept_ocr_phase(0, false, &mut received, &mut previews).is_err());
        assert_eq!(received, [false, false]);
        assert!(accept_ocr_phase(0, true, &mut received, &mut previews).is_ok());
        assert!(accept_ocr_phase(0, true, &mut received, &mut previews).is_err());
        assert!(accept_ocr_phase(1, true, &mut received, &mut previews).is_ok());
        assert!(accept_ocr_phase(0, false, &mut received, &mut previews).is_ok());
        assert!(accept_ocr_phase(0, true, &mut received, &mut previews).is_err());
        assert!(accept_ocr_phase(0, false, &mut received, &mut previews).is_err());
        assert!(accept_ocr_phase(1, false, &mut received, &mut previews).is_ok());
        assert!(accept_ocr_phase(2, true, &mut received, &mut previews).is_err());

        let mut received = [false];
        let mut previews = [];
        assert!(accept_ocr_phase(0, true, &mut received, &mut previews).is_err());
        assert!(accept_ocr_phase(0, false, &mut received, &mut previews).is_ok());
        assert!(accept_ocr_phase(0, false, &mut received, &mut previews).is_err());
    }

    #[test]
    fn displays_share_a_hardware_dependent_ocr_request_budget() {
        for (displays, cpus, performance, frames, budget) in [
            (0, 0, 0, 1, 1),
            (1, 8, 4, 1, 2),
            (2, 8, 4, 2, 2),
            (2, 12, 6, 2, 3),
            (4, 12, 6, 3, 3),
            (4, 12, 8, 4, 4),
            (6, 24, 16, 6, 8),
            (32, 128, 64, 16, 16),
            (4, 32, 0, 2, 2),
        ] {
            assert_eq!(
                ocr_concurrency(displays, cpus, performance),
                (frames, budget)
            );
            let lanes: Vec<_> = (0..frames)
                .map(|index| display_request_lanes(budget, frames, index))
                .collect();
            assert_eq!(lanes.iter().sum::<usize>(), budget);
            assert!(lanes.iter().all(|&lanes| lanes >= 1));
            assert!(lanes.iter().max().unwrap() - lanes.iter().min().unwrap() <= 1);
        }
    }

    #[test]
    fn converts_vision_bottom_left_coordinates_to_global_top_left() {
        let bounds = Rect::new(100.0, -200.0, 800.0, 600.0);
        let converted = normalized_to_global(bounds, Rect::new(0.25, 0.6, 0.5, 0.2));
        assert_eq!(converted, Rect::new(300.0, -80.0, 400.0, 120.0));
    }

    #[test]
    fn maps_vision_regions_to_the_actual_clipped_capture_bounds() {
        let window_bounds = Rect::new(-1600.0, 80.0, 1000.0, 700.0);
        let captured_bounds = Rect::new(-1600.0, 80.0, 600.0, 700.0);
        let bounds = capture_bounds_or_window(captured_bounds, window_bounds);
        assert_eq!(
            normalized_to_global(bounds, Rect::new(0.5, 0.25, 0.25, 0.5)),
            Rect::new(-1300.0, 255.0, 150.0, 350.0)
        );
    }

    #[test]
    fn falls_back_to_window_bounds_when_the_bridge_returns_no_capture_bounds() {
        let window_bounds = Rect::new(-1600.0, 80.0, 1000.0, 700.0);
        assert_eq!(
            capture_bounds_or_window(Rect::default(), window_bounds),
            window_bounds
        );
    }

    #[test]
    fn classifier_preserves_ocr_provenance_and_checkbox_thresholds() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let options = VisionOptions::default();
        let empty = std::ffi::CString::new("").unwrap();
        let link = NativeRegion {
            x: 0.1,
            y: 0.8,
            width: 0.3,
            height: 0.03,
            confidence: 0.9,
            is_text: true,
            label: empty.as_ptr().cast_mut(),
            label_len: 0,
        };
        assert_eq!(
            classify(&link, bounds, &options)
                .unwrap()
                .unwrap()
                .target
                .role,
            SemanticRole::StaticText
        );
        let checkbox = NativeRegion {
            x: 0.1,
            y: 0.7,
            width: 0.025,
            height: 0.03,
            confidence: 0.9,
            is_text: false,
            label: empty.as_ptr().cast_mut(),
            label_len: 0,
        };
        assert_eq!(
            classify(&checkbox, bounds, &options)
                .unwrap()
                .unwrap()
                .target
                .role,
            SemanticRole::Checkbox
        );
    }

    #[test]
    fn recognized_text_reaches_shared_ocr_metadata() {
        let text = std::ffi::CString::new("复制 Copy").unwrap();
        let region = NativeRegion {
            x: 0.1,
            y: 0.5,
            width: 0.3,
            height: 0.03,
            confidence: 0.9,
            is_text: true,
            label: text.as_ptr().cast_mut(),
            label_len: text.as_bytes().len() as u64,
        };
        let target = classify(
            &region,
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            &VisionOptions::default(),
        )
        .unwrap()
        .unwrap()
        .target;
        let mut scan = crate::platform::common::scan_accumulator::ScanAccumulator::new();
        let update = scan.push(TargetSource::SystemOcr, vec![target], 0.5);
        let targets = update.targets;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ocr_text(), "复制 Copy");
        assert_eq!(targets[0].accessibility_text(), "");
    }

    #[test]
    fn classifier_rejects_low_confidence_generic_rectangles() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let options = VisionOptions::default();
        let empty = std::ffi::CString::new("").unwrap();
        let region = NativeRegion {
            x: 0.1,
            y: 0.5,
            width: 0.04,
            height: 0.1,
            confidence: 0.1,
            is_text: false,
            label: empty.as_ptr().cast_mut(),
            label_len: 0,
        };
        assert!(classify(&region, bounds, &options).unwrap().is_none());
    }

    #[test]
    fn native_strings_reject_malformed_metadata_without_scanning_for_nul() {
        assert_eq!(native_string(std::ptr::null(), 0, 8).unwrap(), "");
        assert!(native_string(std::ptr::null(), 1, 8).is_err());
        assert!(native_string(std::ptr::null(), 9, 8).is_err());
        let bytes = b"hello";
        assert_eq!(
            native_string(bytes.as_ptr().cast(), bytes.len() as u64, 8).unwrap(),
            "hello"
        );
    }

    #[test]
    fn iou_merge_preserves_priority_and_stable_ties() {
        let candidate = |name: &str, x, confidence, is_text| Candidate {
            target: UiTarget {
                details: None,
                rect: Rect::new(x, 0.0, 100.0, 100.0),
                name: name.into(),
                role: if is_text {
                    SemanticRole::StaticText
                } else {
                    SemanticRole::Button
                },
            },
            confidence,
            is_text,
        };
        for (second_x, expected) in [(10.0, vec!["first"]), (80.0, vec!["first", "second"])] {
            let targets = merge_candidates(
                vec![
                    candidate("first", 0.0, 0.9, false),
                    candidate("second", second_x, 0.9, false),
                ],
                0.5,
            );
            assert_eq!(
                targets
                    .iter()
                    .map(|target| target.name.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        for is_text in [false, true] {
            let targets = merge_candidates(
                vec![
                    candidate("lower confidence", 0.0, 0.8, is_text),
                    candidate("higher confidence", 10.0, 0.9, is_text),
                ],
                0.5,
            );
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].name, "higher confidence");
        }
        let targets = merge_candidates(
            vec![
                candidate("rectangle", 0.0, 1.0, false),
                candidate("text", 10.0, 0.8, true),
            ],
            0.5,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "text");
    }

    #[test]
    fn native_status_preserves_error_context_and_fallback() {
        for (code, expected) in [
            (0, UiScanStatus::Success),
            (2, UiScanStatus::TimedOut),
            (4, UiScanStatus::ContextChanged),
        ] {
            assert_eq!(native_status(code, String::new()), expected);
        }
        for message in ["", "native request failed"] {
            let expected = if message.is_empty() {
                "Vision scan failed"
            } else {
                message
            };
            for (code, status) in [
                (1, UiScanStatus::PermissionDenied(expected.into())),
                (5, UiScanStatus::Unsupported(expected.into())),
                (-1, UiScanStatus::Failed(expected.into())),
            ] {
                assert_eq!(native_status(code, message.into()), status);
            }
        }
    }
}
