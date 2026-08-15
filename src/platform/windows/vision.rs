//! Native Windows visual UI-hint scanning without OpenCV.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use windows::Media::Ocr::OcrEngine;
use windows::Win32::Foundation::HWND;

use crate::api::command::{UiScanRequest, UiScanStatus};
use crate::api::geometry::{Rect, UiTarget};
use crate::app::worker::WorkerJoin;

use super::accessibility::{foreground_context, window_bounds};
use super::ui_scan::ScanSource;
use super::wechat_ocr::WechatOcr;

const MAX_CAPTURE_PIXELS: f64 = 8_000_000.0;
const MAX_CAPTURE_EDGE: f64 = 4_096.0;
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const OCR_BATCH: usize = 24;

struct ScanJob {
    request: UiScanRequest,
    generation: u64,
    source: ScanSource,
}

#[derive(Default)]
struct QueueState {
    pending: Option<ScanJob>,
    stopping: bool,
}

#[derive(Default)]
struct SharedQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    latest_generation: AtomicU64,
    stopping: AtomicBool,
}

pub(super) struct VisionWorker {
    shared: Arc<SharedQueue>,
    worker: Option<WorkerJoin>,
}

impl VisionWorker {
    pub(super) fn start() -> Result<Self, String> {
        let shared = Arc::new(SharedQueue::default());
        let worker_shared = Arc::clone(&shared);
        // There is deliberately no readiness wait here: WinRT activation and
        // WeChat discovery/prewarming must not delay hooks, tray, or backend
        // construction.
        let worker = WorkerJoin::spawn(
            "Windows visual UI scanner",
            std::thread::Builder::new().name("keysteer-vision".into()),
            move || worker_main(worker_shared),
        )?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub(super) fn submit(
        &self,
        request: UiScanRequest,
        generation: u64,
        source: ScanSource,
    ) -> Result<(), String> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.stopping {
            return Err("visual UI scan worker is stopping".into());
        }
        self.shared
            .latest_generation
            .store(generation, Ordering::Release);
        state.pending = Some(ScanJob {
            request,
            generation,
            source,
        });
        self.shared.ready.notify_one();
        Ok(())
    }

    pub(super) fn cancel(&self, request_id: u64) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state
            .pending
            .as_ref()
            .is_some_and(|job| job.request.id == request_id)
        {
            state.pending.take();
        }
        self.shared.latest_generation.store(0, Ordering::Release);
    }

    fn stop(&mut self) -> Result<(), String> {
        if self.worker.is_none() {
            return Ok(());
        }
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.stopping = true;
            state.pending.take();
            self.shared.stopping.store(true, Ordering::Release);
            self.shared.ready.notify_all();
        }
        if let Some(worker) = self.worker.as_mut() {
            worker.join_timeout(STOP_TIMEOUT)?;
        }
        self.worker.take();
        Ok(())
    }
}

impl Drop for VisionWorker {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-vision", error);
        }
    }
}

fn worker_main(shared: Arc<SharedQueue>) {
    let apartment = match super::native::ComApartment::initialise() {
        Ok(apartment) => apartment,
        Err(error) => {
            fail_pending_until_stopped(&shared, &error);
            return;
        }
    };
    let system_ocr = prewarm_system_ocr();
    let wechat_ocr = WechatOcr::discover_and_start();
    crate::log_info!(
        "windows-vision",
        "OCR availability: system={}, wechat={}",
        system_ocr.is_some(),
        wechat_ocr.is_some()
    );

    while let Some(job) = next_job(&shared) {
        run_scan(job, &shared, system_ocr.clone(), wechat_ocr.clone());
    }
    drop(wechat_ocr);
    drop(system_ocr);
    drop(apartment);
}

fn fail_pending_until_stopped(shared: &SharedQueue, error: &str) {
    while let Some(job) = next_job(shared) {
        job.source.finish(UiScanStatus::Failed(error.to_string()));
    }
}

fn next_job(shared: &SharedQueue) -> Option<ScanJob> {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    while state.pending.is_none() && !state.stopping {
        state = shared
            .ready
            .wait(state)
            .unwrap_or_else(|error| error.into_inner());
    }
    (!state.stopping).then(|| state.pending.take()).flatten()
}

fn current(shared: &SharedQueue, generation: u64, context: Option<(HWND, u32)>) -> bool {
    !shared.stopping.load(Ordering::Acquire)
        && shared.latest_generation.load(Ordering::Acquire) == generation
        && foreground_context() == context
}

fn run_scan(
    job: ScanJob,
    shared: &SharedQueue,
    system_ocr: Option<OcrEngine>,
    wechat_ocr: Option<WechatOcr>,
) {
    let original = foreground_context();
    if job
        .request
        .app
        .as_ref()
        .is_some_and(|app| original.is_none_or(|(_, pid)| app.process_id != pid))
    {
        job.source.finish(UiScanStatus::ContextChanged);
        return;
    }
    let Some((hwnd, _)) = original else {
        job.source.finish(UiScanStatus::Failed(
            "No foreground window is available for visual scanning".into(),
        ));
        return;
    };
    let Some(window) = window_bounds(hwnd) else {
        job.source.finish(UiScanStatus::Failed(
            "Cannot read foreground window bounds".into(),
        ));
        return;
    };
    let bounds = job
        .request
        .bounds
        .and_then(|requested| requested.intersect(&window))
        .unwrap_or(window);
    let image = match capture(bounds) {
        Ok(image) => Arc::new(image),
        Err(error) => {
            job.source.finish(UiScanStatus::Failed(error));
            return;
        }
    };
    if !current(shared, job.generation, original) {
        job.source.finish(UiScanStatus::ContextChanged);
        return;
    }

    let deadline = Instant::now()
        + Duration::from_millis(
            job.request
                .vision
                .request_timeout_ms
                .min(job.request.timeout_ms.max(250))
                .clamp(250, 30_000),
        );
    let (tx, rx) = mpsc::channel();
    let mut pending_ocr = 0usize;
    if job.request.vision.detect_text {
        if let Some(engine) = system_ocr {
            pending_ocr += 1;
            let tx = tx.clone();
            let image = Arc::clone(&image);
            std::thread::spawn(move || {
                let started = Instant::now();
                let result = super::native::ComApartment::initialise()
                    .and_then(|_apartment| recognize_system(&engine, &image));
                let _ = tx.send(ProviderEvent::Ocr {
                    provider: "system",
                    elapsed: started.elapsed(),
                    result,
                });
            });
        }
        if let Some(wechat) = wechat_ocr {
            pending_ocr += 1;
            let tx = tx.clone();
            let image = Arc::clone(&image);
            let timeout = deadline.saturating_duration_since(Instant::now());
            let minimum_confidence = job.request.vision.minimum_confidence;
            std::thread::spawn(move || {
                let started = Instant::now();
                let result = super::native::ComApartment::initialise()
                    .and_then(|_apartment| wechat.recognize(&image, timeout, minimum_confidence));
                let _ = tx.send(ProviderEvent::Ocr {
                    provider: "wechat",
                    elapsed: started.elapsed(),
                    result,
                });
            });
        }
    }
    if job.request.vision.detect_rectangles {
        let tx = tx.clone();
        let image = Arc::clone(&image);
        let options = job.request.vision.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ProviderEvent::Fallback(detect_regions(&image, &options)));
        });
    }
    drop(tx);

    let mut fallback = None;
    let mut accepted_ocr = 0usize;
    let mut timed_out = false;
    while pending_ocr != 0 || (job.request.vision.detect_rectangles && fallback.is_none()) {
        if !current(shared, job.generation, original) {
            job.source.finish(UiScanStatus::ContextChanged);
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        let first = match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut events = VecDeque::from([first]);
        events.extend(rx.try_iter());
        let mut ocr_ready = Vec::new();
        while let Some(event) = events.pop_front() {
            match event {
                ProviderEvent::Fallback(targets) => fallback = Some(targets),
                ProviderEvent::Ocr {
                    provider,
                    elapsed,
                    result,
                } => ocr_ready.push((provider, elapsed, result)),
            }
        }
        // When completions were already queued together, prefer the provider
        // with more candidates, then the shorter elapsed time. A single first
        // completion is never delayed waiting for its peer.
        ocr_ready.sort_by(|a, b| {
            let ac = a.2.as_ref().map_or(0, Vec::len);
            let bc = b.2.as_ref().map_or(0, Vec::len);
            compare_ready(ac, a.1, bc, b.1)
        });
        for (provider, elapsed, result) in ocr_ready {
            pending_ocr = pending_ocr.saturating_sub(1);
            match result {
                Ok(targets) => {
                    let count = targets.len();
                    for batch in targets.chunks(OCR_BATCH) {
                        accepted_ocr += job.source.push(batch.to_vec());
                    }
                    crate::log_info!(
                        "windows-vision",
                        "{provider} OCR completed in {elapsed:?} with {count} valid targets"
                    );
                }
                Err(error) => crate::log_warning!(
                    "windows-vision",
                    "{provider} OCR failed after {elapsed:?}: {error}"
                ),
            }
        }
    }

    if accepted_ocr == 0
        && job.request.vision.detect_rectangles
        && let Some(targets) = fallback
    {
        for batch in targets.chunks(OCR_BATCH) {
            job.source.push(batch.to_vec());
        }
    }
    if !current(shared, job.generation, original) {
        job.source.finish(UiScanStatus::ContextChanged);
    } else if timed_out {
        job.source.finish(UiScanStatus::TimedOut);
    } else {
        job.source.finish(UiScanStatus::Success);
    }
}

fn compare_ready(
    first_count: usize,
    first_elapsed: Duration,
    second_count: usize,
    second_elapsed: Duration,
) -> std::cmp::Ordering {
    second_count
        .cmp(&first_count)
        .then_with(|| first_elapsed.cmp(&second_elapsed))
}

pub(super) fn diagnostic_lines() -> Vec<String> {
    let system = match super::native::ComApartment::initialise() {
        Ok(_apartment) => match OcrEngine::TryCreateFromUserProfileLanguages() {
            Ok(_) => format!(
                "system OCR: available (maximum image dimension {})",
                OcrEngine::MaxImageDimension().unwrap_or_default()
            ),
            Err(error) => format!("system OCR: unavailable ({error})"),
        },
        Err(error) => format!("system OCR: unavailable ({error})"),
    };
    vec![system, super::wechat_ocr::diagnostic_line()]
}

enum ProviderEvent {
    Ocr {
        provider: &'static str,
        elapsed: Duration,
        result: Result<Vec<UiTarget>, String>,
    },
    Fallback(Vec<UiTarget>),
}

#[derive(Debug)]
pub(super) struct CapturedImage {
    pub(super) pixels: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) desktop_bounds: Rect,
    pub(super) scale: f64,
}

fn capture(bounds: Rect) -> Result<CapturedImage, String> {
    if bounds.width < 2.0 || bounds.height < 2.0 {
        return Err("visual capture bounds are empty".into());
    }
    let edge_scale = (MAX_CAPTURE_EDGE / bounds.width.max(bounds.height)).min(1.0);
    let pixel_scale = (MAX_CAPTURE_PIXELS / (bounds.width * bounds.height))
        .sqrt()
        .min(1.0);
    let scale = edge_scale.min(pixel_scale);
    let width = (bounds.width * scale).round().max(2.0) as i32;
    let height = (bounds.height * scale).round().max(2.0) as i32;
    let captured = super::native::capture_bgra(
        bounds.x.floor() as i32,
        bounds.y.floor() as i32,
        bounds.width.ceil() as i32,
        bounds.height.ceil() as i32,
        width,
        height,
    )?;
    Ok(CapturedImage {
        pixels: captured.pixels,
        width: captured.width,
        height: captured.height,
        desktop_bounds: bounds,
        scale,
    })
}

fn prewarm_system_ocr() -> Option<OcrEngine> {
    let result = (|| -> Result<OcrEngine, String> {
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|error| format!("cannot create OcrEngine: {error}"))?;
        let maximum = OcrEngine::MaxImageDimension()
            .map_err(|error| format!("cannot query OCR image limit: {error}"))?;
        let languages = OcrEngine::AvailableRecognizerLanguages()
            .ok()
            .and_then(|languages| {
                let count = languages.Size().ok()?;
                Some(
                    (0..count)
                        .filter_map(|index| {
                            languages
                                .GetAt(index)
                                .and_then(|language| language.LanguageTag())
                                .ok()
                                .map(|tag| tag.to_string())
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        let blank = CapturedImage {
            pixels: vec![255; 64 * 64 * 4],
            width: 64,
            height: 64,
            desktop_bounds: Rect::new(0.0, 0.0, 64.0, 64.0),
            scale: 1.0,
        };
        recognize_system(&engine, &blank)?;
        crate::log_info!(
            "windows-vision",
            "system OCR prewarmed (languages [{}], maximum image dimension {maximum})",
            languages.join(", ")
        );
        Ok(engine)
    })();
    match result {
        Ok(engine) => Some(engine),
        Err(error) => {
            crate::log_info!("windows-vision", "system OCR unavailable: {error}");
            None
        }
    }
}

fn recognize_system(engine: &OcrEngine, image: &CapturedImage) -> Result<Vec<UiTarget>, String> {
    let bitmap = super::native::software_bitmap_bgra(&image.pixels, image.width, image.height)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|operation| operation.join())
        .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}"))?;
    let lines = result
        .Lines()
        .map_err(|error| format!("cannot read OCR lines: {error}"))?;
    let count = lines
        .Size()
        .map_err(|error| format!("cannot read OCR line count: {error}"))?;
    let mut targets = Vec::with_capacity(count as usize);
    for index in 0..count {
        let line = lines
            .GetAt(index)
            .map_err(|error| format!("cannot read OCR line {index}: {error}"))?;
        let text = line
            .Text()
            .map_err(|error| format!("cannot read OCR line text: {error}"))?
            .to_string();
        if text.trim().is_empty() {
            continue;
        }
        let words = line
            .Words()
            .map_err(|error| format!("cannot read OCR words: {error}"))?;
        let word_count = words
            .Size()
            .map_err(|error| format!("cannot read OCR word count: {error}"))?;
        let mut union: Option<Rect> = None;
        for word_index in 0..word_count {
            let native = words
                .GetAt(word_index)
                .and_then(|word| word.BoundingRect())
                .map_err(|error| format!("cannot read OCR word bounds: {error}"))?;
            let rect = image_to_desktop(
                image,
                Rect::new(
                    f64::from(native.X),
                    f64::from(native.Y),
                    f64::from(native.Width),
                    f64::from(native.Height),
                ),
            );
            union = Some(union.map_or(rect, |current| current.union(&rect)));
        }
        if let Some(rect) = union.filter(|rect| valid_target_rect(*rect, image.desktop_bounds)) {
            targets.push(UiTarget {
                rect,
                name: text.trim().to_string(),
                role: "static_text".into(),
                native_role: Some("vision:windows-ocr".into()),
            });
        }
    }
    Ok(targets)
}

pub(super) fn image_to_desktop(image: &CapturedImage, rect: Rect) -> Rect {
    Rect::new(
        image.desktop_bounds.x + rect.x / image.scale,
        image.desktop_bounds.y + rect.y / image.scale,
        rect.width / image.scale,
        rect.height / image.scale,
    )
}

pub(super) fn valid_target_rect(rect: Rect, bounds: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width >= 2.0
        && rect.height >= 2.0
        && bounds.intersect(&rect).is_some()
        && !(rect.width >= bounds.width && rect.height >= bounds.height)
}

fn detect_regions(image: &CapturedImage, options: &crate::api::VisionOptions) -> Vec<UiTarget> {
    let width = image.width as usize;
    let height = image.height as usize;
    if width < 3 || height < 3 {
        return Vec::new();
    }
    let mut gray = vec![0u8; width * height];
    for (index, pixel) in image.pixels.chunks_exact(4).enumerate() {
        gray[index] = ((u16::from(pixel[2]) * 77
            + u16::from(pixel[1]) * 150
            + u16::from(pixel[0]) * 29)
            >> 8) as u8;
    }
    let mut edge = vec![false; width * height];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            let gx = i16::from(gray[i + 1]) - i16::from(gray[i - 1]);
            let gy = i16::from(gray[i + width]) - i16::from(gray[i - width]);
            let local_min = gray[i - width - 1..=i - width + 1]
                .iter()
                .chain(&gray[i - 1..=i + 1])
                .chain(&gray[i + width - 1..=i + width + 1])
                .copied()
                .min()
                .unwrap_or(gray[i]);
            let local_max = gray[i - width - 1..=i - width + 1]
                .iter()
                .chain(&gray[i - 1..=i + 1])
                .chain(&gray[i + width - 1..=i + width + 1])
                .copied()
                .max()
                .unwrap_or(gray[i]);
            edge[i] = gx.unsigned_abs() + gy.unsigned_abs() >= 48
                || local_max.saturating_sub(local_min) >= 42;
        }
    }
    // A small close operation joins anti-aliased borders while preserving
    // neighbouring controls as distinct components.
    let mut dilated = edge.clone();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            dilated[i] = (-1isize..=1).any(|dy| {
                (-1isize..=1).any(|dx| {
                    edge[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    let mut closed = dilated.clone();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            closed[i] = (-1isize..=1).all(|dy| {
                (-1isize..=1).all(|dx| {
                    dilated[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    let mut visited = vec![false; width * height];
    let mut candidates = Vec::new();
    let configured_minimum =
        (options.rectangle_min_size * width.min(height) as f64).ceil() as usize;
    let minimum_side = configured_minimum.max(4);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let start = y * width + x;
            if !closed[start] || visited[start] {
                continue;
            }
            let mut queue = VecDeque::from([start]);
            visited[start] = true;
            let (mut min_x, mut max_x, mut min_y, mut max_y, mut pixels) = (x, x, y, y, 0usize);
            while let Some(index) = queue.pop_front() {
                pixels += 1;
                let px = index % width;
                let py = index / width;
                min_x = min_x.min(px);
                max_x = max_x.max(px);
                min_y = min_y.min(py);
                max_y = max_y.max(py);
                for ny in py.saturating_sub(1)..=(py + 1).min(height - 1) {
                    for nx in px.saturating_sub(1)..=(px + 1).min(width - 1) {
                        let next = ny * width + nx;
                        if closed[next] && !visited[next] {
                            visited[next] = true;
                            queue.push_back(next);
                        }
                    }
                }
            }
            let box_width = max_x - min_x + 1;
            let box_height = max_y - min_y + 1;
            let aspect = box_width as f64 / box_height as f64;
            let perimeter = (2 * (box_width + box_height)).max(1);
            if box_width < minimum_side
                || box_height < minimum_side
                || pixels < 16
                || !(options.rectangle_min_aspect..=options.rectangle_max_aspect).contains(&aspect)
            {
                continue;
            }
            let confidence = (pixels as f64 / perimeter as f64).min(1.0);
            if confidence < options.minimum_confidence {
                continue;
            }
            let rect = image_to_desktop(
                image,
                Rect::new(
                    min_x as f64,
                    min_y as f64,
                    box_width as f64,
                    box_height as f64,
                ),
            );
            if valid_target_rect(rect, image.desktop_bounds) {
                let role = classify_region(rect, confidence, options);
                if let Some((role, native_role)) = role {
                    candidates.push((
                        confidence,
                        UiTarget {
                            rect,
                            name: String::new(),
                            role: role.into(),
                            native_role: Some(native_role.into()),
                        },
                    ));
                }
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    candidates
        .into_iter()
        .take(options.rectangle_max_candidates.min(2_000))
        .map(|(_, target)| target)
        .collect()
}

fn classify_region(
    rect: Rect,
    confidence: f64,
    options: &crate::api::VisionOptions,
) -> Option<(&'static str, &'static str)> {
    let aspect = rect.width / rect.height.max(f64::EPSILON);
    if rect.width <= options.checkbox_max_size
        && rect.height <= options.checkbox_max_size
        && (0.75..=1.35).contains(&aspect)
    {
        Some(("checkbox", "vision:rust-checkbox"))
    } else if confidence >= options.button_min_confidence
        && (options.button_min_aspect..=options.button_max_aspect).contains(&aspect)
    {
        Some(("button", "vision:rust-button"))
    } else if rect.width >= options.image_min_size && rect.height >= options.image_min_size {
        Some(("image", "vision:rust-image"))
    } else if confidence >= options.generic_clickable_min_confidence {
        Some(("control", "vision:rust-rectangle"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_scaled_negative_desktop_coordinates() {
        let image = CapturedImage {
            pixels: Vec::new(),
            width: 500,
            height: 300,
            desktop_bounds: Rect::new(-1_000.0, -200.0, 2_000.0, 1_200.0),
            scale: 0.5,
        };
        assert_eq!(
            image_to_desktop(&image, Rect::new(25.0, 10.0, 50.0, 20.0)),
            Rect::new(-950.0, -180.0, 100.0, 40.0)
        );
    }

    #[test]
    fn pure_rust_detector_finds_a_closed_button_border() {
        let mut pixels = vec![255u8; 120 * 80 * 4];
        for y in 20..50 {
            for x in 25..95 {
                if y == 20 || y == 49 || x == 25 || x == 94 {
                    let index = (y * 120 + x) * 4;
                    pixels[index..index + 3].fill(0);
                }
            }
        }
        let image = CapturedImage {
            pixels,
            width: 120,
            height: 80,
            desktop_bounds: Rect::new(0.0, 0.0, 120.0, 80.0),
            scale: 1.0,
        };
        let targets = detect_regions(&image, &crate::api::VisionOptions::default());
        assert!(targets.iter().any(|target| target.role == "button"));
    }

    #[test]
    fn ready_ocr_batches_prefer_more_targets_then_lower_latency() {
        assert_eq!(
            compare_ready(12, Duration::from_millis(30), 20, Duration::from_millis(80)),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_ready(20, Duration::from_millis(30), 20, Duration::from_millis(80)),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn worker_start_does_not_wait_for_ocr_prewarm() {
        let started = Instant::now();
        let mut worker = VisionWorker::start().unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        worker.stop().unwrap();
    }

    #[test]
    #[ignore = "requires an installed Windows OCR language pack"]
    fn live_system_ocr_runtime_probe_recognizes_a_blank_bitmap() {
        let _apartment = super::super::native::ComApartment::initialise().unwrap();
        assert!(prewarm_system_ocr().is_some());
    }
}
