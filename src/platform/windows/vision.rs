//! Native Windows visual UI-hint scanning without OpenCV.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Media::Ocr::OcrEngine;
use windows::Win32::Foundation::HWND;
use windows_future::AsyncStatus;

use crate::api::command::{UiScanRequest, UiScanStatus};
use crate::api::geometry::{Rect, UiTarget};
use crate::app::worker::WorkerJoin;

use super::accessibility::{foreground_context, window_bounds};
use super::overlay_worker::CaptureLease;
use super::ui_scan::ScanSource;
use super::wechat_ocr::{WechatDescriptor, WechatOcr};

const MAX_CAPTURE_PIXELS: f64 = 8_000_000.0;
const MAX_CAPTURE_EDGE: f64 = 4_096.0;
const MAX_FALLBACK_PIXELS: f64 = 2_073_600.0;
const MAX_FALLBACK_EDGE: f64 = 2_560.0;
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

struct ScanJob {
    request: UiScanRequest,
    generation: u64,
    source: ScanSource,
    capture: Option<CaptureLease>,
}

#[derive(Default)]
struct QueueState {
    pending: Option<ScanJob>,
    active_request_id: Option<u64>,
    running: bool,
    stopping: bool,
}

#[derive(Default)]
struct SharedQueue {
    state: Mutex<QueueState>,
    latest_generation: AtomicU64,
    stopping: AtomicBool,
}

#[derive(Clone, Debug)]
struct SystemOcrDescriptor {
    languages: Vec<String>,
    maximum_dimension: u32,
}

#[derive(Clone, Debug, Default)]
struct OcrDiscoverySnapshot {
    system: Option<SystemOcrDescriptor>,
    wechat: Option<WechatDescriptor>,
}

#[derive(Clone, Debug)]
enum DiscoveryState {
    Pending,
    Ready(OcrDiscoverySnapshot),
    Unavailable,
}

struct DiscoveryShared {
    state: Mutex<DiscoveryState>,
    ready: Condvar,
    stopping: AtomicBool,
}

impl Default for DiscoveryShared {
    fn default() -> Self {
        Self {
            state: Mutex::new(DiscoveryState::Pending),
            ready: Condvar::new(),
            stopping: AtomicBool::new(false),
        }
    }
}

#[derive(Clone)]
struct DiscoveryHandle(Arc<DiscoveryShared>);

impl DiscoveryHandle {
    fn wait(
        &self,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
    ) -> Option<OcrDiscoverySnapshot> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            match &*state {
                DiscoveryState::Ready(snapshot) => return Some(snapshot.clone()),
                DiscoveryState::Unavailable => return Some(OcrDiscoverySnapshot::default()),
                DiscoveryState::Pending => {}
            }
            if cancelled() || Instant::now() >= deadline {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            state = self
                .0
                .ready
                .wait_timeout(state, remaining.min(Duration::from_millis(10)))
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
    }
}

struct OcrDiscovery {
    shared: Arc<DiscoveryShared>,
    worker: Option<WorkerJoin>,
}

impl OcrDiscovery {
    fn new() -> Self {
        Self {
            shared: Arc::new(DiscoveryShared::default()),
            worker: None,
        }
    }

    fn start(&mut self) {
        if self.worker.is_some() || self.shared.stopping.load(Ordering::Acquire) {
            return;
        }
        let should_start = matches!(
            *self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            DiscoveryState::Pending
        );
        if !should_start {
            return;
        }
        let worker_shared = Arc::clone(&self.shared);
        self.worker = WorkerJoin::spawn(
            "Windows OCR discovery",
            std::thread::Builder::new().name("keysteer-ocr-discovery".into()),
            move || discover_ocr(worker_shared),
        )
        .map_err(|error| {
            crate::app::logging::report_error("windows-vision", &error);
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *state = DiscoveryState::Unavailable;
            self.shared.ready.notify_all();
        })
        .ok();
    }

    fn handle(&self) -> DiscoveryHandle {
        DiscoveryHandle(Arc::clone(&self.shared))
    }

    fn reap_finished(&mut self) -> Result<(), String> {
        if let Some(worker) = self.worker.as_mut()
            && worker.reap_finished()?
        {
            self.worker.take();
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.ready.notify_all();
        if let Some(worker) = self.worker.as_mut() {
            worker.join_timeout(STOP_TIMEOUT)?;
        }
        self.worker.take();
        Ok(())
    }
}

#[derive(Clone)]
struct ScanCancellation {
    shared: Arc<SharedQueue>,
    generation: u64,
    local: Arc<AtomicBool>,
}

impl ScanCancellation {
    fn new(shared: &Arc<SharedQueue>, generation: u64) -> Self {
        Self {
            shared: Arc::clone(shared),
            generation,
            local: Arc::new(AtomicBool::new(false)),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.local.load(Ordering::Acquire)
            || self.shared.stopping.load(Ordering::Acquire)
            || self.shared.latest_generation.load(Ordering::Acquire) != self.generation
    }

    fn cancel(&self) {
        self.local.store(true, Ordering::Release);
    }
}

/// Own every provider thread started for one scan. Dropping a scan first
/// invalidates its cancellation token and then joins all providers, so leaving
/// UI Hint cannot strand OCR or pixel-analysis work in normal mode.
struct ProviderThreads {
    cancellation: ScanCancellation,
    threads: Vec<JoinHandle<()>>,
}

impl ProviderThreads {
    fn new(cancellation: ScanCancellation) -> Self {
        Self {
            cancellation,
            threads: Vec::with_capacity(3),
        }
    }

    fn spawn(&mut self, name: &'static str, work: impl FnOnce() + Send + 'static) -> bool {
        let background_work = move || {
            if let Err(error) = super::native::prefer_background_work() {
                crate::report_warning!(
                    "windows-vision",
                    "cannot lower {name} provider priority: {error}"
                );
            }
            work();
        };
        match std::thread::Builder::new()
            .name(name.into())
            .spawn(background_work)
        {
            Ok(thread) => {
                self.threads.push(thread);
                true
            }
            Err(error) => {
                crate::report_warning!("windows-vision", "cannot start {name} provider: {error}");
                false
            }
        }
    }

    fn join_all(&mut self) -> Result<(), String> {
        self.cancellation.cancel();
        let mut first_error = None;
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                let error = "visual scan provider thread panicked".to_string();
                if first_error.is_none() {
                    first_error = Some(error);
                } else {
                    crate::app::logging::report_error("windows-vision", error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for ProviderThreads {
    fn drop(&mut self) {
        if let Err(error) = self.join_all() {
            crate::app::logging::report_error("windows-vision", error);
        }
    }
}

pub(super) struct VisionWorker {
    shared: Arc<SharedQueue>,
    discovery: OcrDiscovery,
    workers: Vec<WorkerJoin>,
}

impl VisionWorker {
    pub(super) fn start() -> Self {
        Self {
            shared: Arc::new(SharedQueue::default()),
            discovery: OcrDiscovery::new(),
            workers: Vec::with_capacity(2),
        }
    }

    pub(super) fn begin_discovery(&mut self) {
        self.discovery.start();
    }

    pub(super) fn submit(
        &mut self,
        request: UiScanRequest,
        generation: u64,
        source: ScanSource,
        capture: CaptureLease,
    ) -> Result<(), String> {
        self.discovery.start();
        self.reap_finished();
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
        let superseded = state.pending.replace(ScanJob {
            request,
            generation,
            source,
            capture: Some(capture),
        });
        if !state.running {
            state.running = true;
            let shared = Arc::clone(&self.shared);
            let discovery = self.discovery.handle();
            drop(state);
            let worker = match WorkerJoin::spawn(
                "Windows visual UI scanner",
                std::thread::Builder::new().name("keysteer-vision".into()),
                move || worker_main(shared, discovery),
            ) {
                Ok(worker) => worker,
                Err(error) => {
                    let mut state = self
                        .shared
                        .state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    state.running = false;
                    let failed = state.pending.take();
                    self.shared.latest_generation.store(0, Ordering::Release);
                    drop(state);
                    if let Some(job) = superseded {
                        finish_cancelled_job(job);
                    }
                    if let Some(job) = failed {
                        finish_cancelled_job(job);
                    }
                    return Err(error);
                }
            };
            self.workers.push(worker);
        } else {
            drop(state);
        }
        if let Some(job) = superseded {
            finish_cancelled_job(job);
        }
        Ok(())
    }

    pub(super) fn cancel(&mut self, request_id: u64) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending_request_id = state.pending.as_ref().map(|job| job.request.id);
        let pending_cancelled = pending_request_id == Some(request_id);
        let clear_generation =
            cancellation_clears_generation(state.active_request_id, pending_request_id, request_id);
        let pending = pending_cancelled.then(|| state.pending.take()).flatten();
        if clear_generation {
            self.shared.latest_generation.store(0, Ordering::Release);
        }
        drop(state);
        if let Some(job) = pending {
            finish_cancelled_job(job);
        }
        self.reap_finished();
    }

    pub(super) fn reap_finished(&mut self) {
        if let Err(error) = self.discovery.reap_finished() {
            crate::app::logging::report_error("windows-vision", error);
        }
        let mut index = 0;
        while index < self.workers.len() {
            match self.workers[index].reap_finished() {
                Ok(true) => {
                    self.workers.swap_remove(index);
                }
                Ok(false) => index += 1,
                Err(error) => {
                    crate::app::logging::report_error("windows-vision", error);
                    self.workers.swap_remove(index);
                }
            }
        }
    }

    pub(super) fn stop(&mut self) -> Result<(), String> {
        let pending = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.stopping = true;
            let pending = state.pending.take();
            self.shared.stopping.store(true, Ordering::Release);
            pending
        };
        self.shared.latest_generation.store(0, Ordering::Release);
        if let Some(job) = pending {
            finish_cancelled_job(job);
        }
        let mut first_error = self.discovery.stop().err();
        let mut index = 0;
        while index < self.workers.len() {
            match self.workers[index].join_timeout(STOP_TIMEOUT) {
                Ok(()) => {
                    self.workers.swap_remove(index);
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    } else {
                        crate::app::logging::report_error("windows-vision", error);
                    }
                    index += 1;
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for VisionWorker {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-vision", error);
        }
    }
}

fn worker_main(shared: Arc<SharedQueue>, discovery: DiscoveryHandle) {
    if let Err(error) = super::native::prefer_background_work() {
        crate::report_warning!(
            "windows-vision",
            "cannot lower visual worker priority: {error}"
        );
    }
    loop {
        let job = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.stopping {
                state.running = false;
                return;
            }
            match state.pending.take() {
                Some(job) => {
                    state.active_request_id = Some(job.request.id);
                    job
                }
                None => {
                    state.running = false;
                    return;
                }
            }
        };
        let request_id = job.request.id;
        run_scan(job, &shared, &discovery);
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.active_request_id == Some(request_id) {
            state.active_request_id = None;
        }
    }
}

fn current(shared: &SharedQueue, generation: u64, context: Option<(HWND, u32)>) -> bool {
    !shared.stopping.load(Ordering::Acquire)
        && shared.latest_generation.load(Ordering::Acquire) == generation
        && foreground_context() == context
}

fn finish_cancelled_job(job: ScanJob) {
    let ScanJob {
        source, capture, ..
    } = job;
    drop(capture);
    source.finish(UiScanStatus::ContextChanged);
}

fn cancellation_clears_generation(
    active_request_id: Option<u64>,
    pending_request_id: Option<u64>,
    cancelled_request_id: u64,
) -> bool {
    pending_request_id == Some(cancelled_request_id)
        || (pending_request_id.is_none() && active_request_id == Some(cancelled_request_id))
}

struct ProviderImage {
    image: Arc<CapturedImage>,
    bitmap: Arc<SharedSoftwareBitmap>,
}

struct SharedSoftwareBitmap(windows::Graphics::Imaging::SoftwareBitmap);

impl Drop for SharedSoftwareBitmap {
    fn drop(&mut self) {
        if let Err(error) = self.0.Close() {
            crate::app::logging::report_error(
                "windows-vision",
                format!("cannot close OCR SoftwareBitmap: {error}"),
            );
        }
    }
}

fn run_scan(mut job: ScanJob, shared: &Arc<SharedQueue>, discovery: &DiscoveryHandle) {
    let status = run_scan_inner(&mut job, shared, discovery);
    job.source.finish(status);
}

fn run_scan_inner(
    job: &mut ScanJob,
    shared: &Arc<SharedQueue>,
    discovery: &DiscoveryHandle,
) -> UiScanStatus {
    let deadline = Instant::now()
        + Duration::from_millis(
            job.request
                .vision
                .request_timeout_ms
                .min(job.request.timeout_ms.max(250))
                .clamp(250, 30_000),
        );
    let original = foreground_context();
    if job
        .request
        .app
        .as_ref()
        .is_some_and(|app| original.is_none_or(|(_, pid)| app.process_id != pid))
    {
        return UiScanStatus::ContextChanged;
    }
    let Some((hwnd, _)) = original else {
        return UiScanStatus::Failed(
            "No foreground window is available for visual scanning".into(),
        );
    };
    let Some(window) = window_bounds(hwnd) else {
        return UiScanStatus::Failed("Cannot read foreground window bounds".into());
    };
    let bounds = job
        .request
        .bounds
        .and_then(|requested| requested.intersect(&window))
        .unwrap_or(window);
    let cancellation = ScanCancellation::new(shared, job.generation);
    let discovery_snapshot = if job.request.vision.detect_text {
        match discovery.wait(deadline, || cancellation.is_cancelled()) {
            Some(snapshot) => snapshot,
            None if cancellation.is_cancelled() => return UiScanStatus::ContextChanged,
            None => return UiScanStatus::TimedOut,
        }
    } else {
        OcrDiscoverySnapshot::default()
    };

    let (tx, rx) = mpsc::sync_channel(3);
    let mut providers = ProviderThreads::new(cancellation.clone());
    let mut provider_inputs = Vec::with_capacity(2);
    let mut pending_ocr = 0usize;
    if let Some(descriptor) = discovery_snapshot.system {
        let (image_tx, image_rx) = mpsc::sync_channel(1);
        let result_tx = tx.clone();
        let provider_cancellation = cancellation.clone();
        if providers.spawn("keysteer-system-ocr", move || {
            let started = Instant::now();
            let result =
                recognize_system_provider(descriptor, image_rx, deadline, &provider_cancellation);
            let _ = result_tx.send(ProviderEvent::Ocr {
                provider: "system",
                elapsed: started.elapsed(),
                result,
            });
        }) {
            provider_inputs.push(image_tx);
            pending_ocr += 1;
        }
    }
    if let Some(descriptor) = discovery_snapshot.wechat {
        let (image_tx, image_rx) = mpsc::sync_channel(1);
        let result_tx = tx.clone();
        let provider_cancellation = cancellation.clone();
        let minimum_confidence = job.request.vision.minimum_confidence;
        if providers.spawn("keysteer-wechat-ocr", move || {
            let started = Instant::now();
            let result = recognize_wechat_provider(
                descriptor,
                image_rx,
                deadline,
                minimum_confidence,
                &provider_cancellation,
            );
            let _ = result_tx.send(ProviderEvent::Ocr {
                provider: "wechat",
                elapsed: started.elapsed(),
                result,
            });
        }) {
            provider_inputs.push(image_tx);
            pending_ocr += 1;
        }
    }

    let Some(mut capture_lease) = job.capture.take() else {
        return UiScanStatus::Failed("visual capture lease was not created".into());
    };
    if let Err(error) =
        capture_lease.wait_hidden(deadline, || !current(shared, job.generation, original))
    {
        let current = current(shared, job.generation, original);
        if let Err(release_error) = capture_lease.release() {
            crate::app::logging::report_error("windows-overlay", release_error);
        }
        if !current {
            return UiScanStatus::ContextChanged;
        }
        if Instant::now() >= deadline {
            return UiScanStatus::TimedOut;
        }
        crate::app::logging::report_error("windows-vision", &error);
        return UiScanStatus::Failed(error);
    }
    if !current(shared, job.generation, original) {
        if let Err(error) = capture_lease.release() {
            crate::app::logging::report_error("windows-overlay", error);
        }
        return UiScanStatus::ContextChanged;
    }
    let captured = capture(bounds);
    super::native::release_capture_surface();
    if let Err(error) = capture_lease.release() {
        crate::app::logging::report_error("windows-overlay", error);
    }
    let image = match captured {
        Ok(image) => Arc::new(image),
        Err(error) => {
            crate::app::logging::report_error("windows-vision", &error);
            return UiScanStatus::Failed(error);
        }
    };
    if !current(shared, job.generation, original) {
        return UiScanStatus::ContextChanged;
    }

    let shared_bitmap = if provider_inputs.is_empty() {
        None
    } else {
        match super::native::software_bitmap_bgra(&image.pixels, image.width, image.height) {
            Ok(bitmap) => Some(Arc::new(SharedSoftwareBitmap(bitmap))),
            Err(error) => {
                crate::app::logging::report_error("windows-vision", &error);
                None
            }
        }
    };
    if let Some(bitmap) = shared_bitmap.as_ref() {
        for input in provider_inputs.drain(..) {
            let _ = input.send(Arc::new(ProviderImage {
                image: Arc::clone(&image),
                bitmap: Arc::clone(bitmap),
            }));
        }
    }
    drop(provider_inputs);

    let fallback_cancelled = Arc::new(AtomicBool::new(false));
    let fallback_pending = if job.request.vision.detect_rectangles {
        let result_tx = tx.clone();
        let fallback_image = Arc::clone(&image);
        let options = job.request.vision.clone();
        let provider_cancellation = cancellation.clone();
        let fallback_cancelled = Arc::clone(&fallback_cancelled);
        providers.spawn("keysteer-vision-fallback", move || {
            let mut scratch = FallbackScratch::default();
            let targets = detect_regions(&fallback_image, &options, &mut scratch, || {
                provider_cancellation.is_cancelled() || fallback_cancelled.load(Ordering::Acquire)
            });
            let _ = result_tx.send(ProviderEvent::Fallback(targets));
        })
    } else {
        false
    };
    drop(tx);

    let mut fallback = None;
    let mut accepted_ocr = 0usize;
    let mut timed_out = false;
    while pending_ocr != 0 || (fallback_pending && fallback.is_none()) {
        if !current(shared, job.generation, original) {
            cancellation.cancel();
            return UiScanStatus::ContextChanged;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            cancellation.cancel();
            break;
        }
        let first = match rx.recv_timeout(remaining.min(Duration::from_millis(10))) {
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
                    accepted_ocr += job.source.push(targets);
                    if accepted_ocr != 0 {
                        fallback_cancelled.store(true, Ordering::Release);
                    }
                    crate::log_info!(
                        "windows-vision",
                        "{provider} OCR completed in {elapsed:?} with {count} valid targets"
                    );
                }
                Err(error) if is_provider_cancellation(&error) => {}
                Err(error) => crate::report_warning!(
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
        job.source.push(targets);
    }
    if let Err(error) = providers.join_all() {
        crate::app::logging::report_error("windows-vision", error);
    }
    drop(shared_bitmap);
    if !current(shared, job.generation, original) {
        UiScanStatus::ContextChanged
    } else if timed_out {
        UiScanStatus::TimedOut
    } else {
        UiScanStatus::Success
    }
}

fn wait_provider_image(
    receiver: mpsc::Receiver<Arc<ProviderImage>>,
    deadline: Instant,
    cancellation: &ScanCancellation,
) -> Result<Arc<ProviderImage>, String> {
    loop {
        if cancellation.is_cancelled() {
            return Err("OCR provider cancelled".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("OCR provider timed out waiting for the screenshot".into());
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(image) => return Ok(image),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("OCR provider screenshot channel closed".into());
            }
        }
    }
}

fn recognize_system_provider(
    descriptor: SystemOcrDescriptor,
    receiver: mpsc::Receiver<Arc<ProviderImage>>,
    deadline: Instant,
    cancellation: &ScanCancellation,
) -> Result<Vec<UiTarget>, String> {
    let _apartment = super::native::ComApartment::initialise()?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|error| format!("cannot create per-scan OcrEngine: {error}"))?;
    let input = wait_provider_image(receiver, deadline, cancellation)?;
    if input.image.width > descriptor.maximum_dimension
        || input.image.height > descriptor.maximum_dimension
    {
        return Err(format!(
            "captured image exceeds system OCR limit {}",
            descriptor.maximum_dimension
        ));
    }
    recognize_system(&engine, &input.bitmap.0, &input.image, || {
        cancellation.is_cancelled()
    })
}

fn recognize_wechat_provider(
    descriptor: WechatDescriptor,
    receiver: mpsc::Receiver<Arc<ProviderImage>>,
    deadline: Instant,
    minimum_confidence: f64,
    cancellation: &ScanCancellation,
) -> Result<Vec<UiTarget>, String> {
    let _apartment = super::native::ComApartment::initialise()?;
    let mut provider = WechatOcr::start(&descriptor)?;
    let result = wait_provider_image(receiver, deadline, cancellation).and_then(|input| {
        provider.recognize(
            &input.image,
            &input.bitmap.0,
            deadline.saturating_duration_since(Instant::now()),
            minimum_confidence,
            || cancellation.is_cancelled(),
        )
    });
    let cleanup = provider.shutdown();
    match (result, cleanup) {
        (Ok(targets), Ok(())) => Ok(targets),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            crate::app::logging::report_error("windows-vision", cleanup);
            Err(error)
        }
    }
}

fn is_provider_cancellation(error: &str) -> bool {
    error.contains("cancelled")
        || error.contains("screenshot channel closed")
        || error.contains("timed out waiting for the screenshot")
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
    let system = match probe_system_ocr(|| false) {
        Ok(descriptor) => format!(
            "system OCR: available (languages [{}], maximum image dimension {})",
            descriptor.languages.join(", "),
            descriptor.maximum_dimension
        ),
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

fn discover_ocr(shared: Arc<DiscoveryShared>) {
    if shared.stopping.load(Ordering::Acquire) {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *state = DiscoveryState::Unavailable;
        shared.ready.notify_all();
        return;
    }
    if let Err(error) = super::native::prefer_background_work() {
        crate::report_warning!(
            "windows-vision",
            "cannot lower OCR discovery priority: {error}"
        );
    }
    let snapshot = probe_ocr(|| shared.stopping.load(Ordering::Acquire));
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *state = match snapshot {
        Some(snapshot) if snapshot.system.is_some() || snapshot.wechat.is_some() => {
            DiscoveryState::Ready(snapshot)
        }
        _ => DiscoveryState::Unavailable,
    };
    shared.ready.notify_all();
}

fn probe_ocr(cancelled: impl Fn() -> bool + Copy) -> Option<OcrDiscoverySnapshot> {
    if cancelled() {
        return None;
    }
    let system = match probe_system_ocr(cancelled) {
        Ok(descriptor) => {
            crate::log_info!(
                "windows-vision",
                "system OCR discovered (languages [{}], maximum image dimension {})",
                descriptor.languages.join(", "),
                descriptor.maximum_dimension
            );
            Some(descriptor)
        }
        Err(_error) if cancelled() => return None,
        Err(error) => {
            crate::report_warning!("windows-vision", "system OCR unavailable: {error}");
            None
        }
    };
    if cancelled() {
        return None;
    }
    let wechat = match super::wechat_ocr::discover_descriptor() {
        Ok(Some(descriptor)) => {
            crate::log_info!(
                "windows-vision",
                "WeChat OCR discovered ({})",
                descriptor.description()
            );
            Some(descriptor)
        }
        Ok(None) => {
            crate::report_warning!(
                "windows-vision",
                "WeChat OCR unavailable: optional components were not found"
            );
            None
        }
        Err(error) => {
            crate::report_warning!("windows-vision", "WeChat OCR unavailable: {error}");
            None
        }
    };
    Some(OcrDiscoverySnapshot { system, wechat })
}

fn probe_system_ocr(cancelled: impl Fn() -> bool) -> Result<SystemOcrDescriptor, String> {
    let apartment = super::native::ComApartment::initialise()?;
    let result = (|| {
        if cancelled() {
            return Err("system OCR discovery cancelled".into());
        }
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
        drop(engine);
        Ok(SystemOcrDescriptor {
            languages,
            maximum_dimension: maximum,
        })
    })();
    drop(apartment);
    result
}

fn recognize_system(
    engine: &OcrEngine,
    bitmap: &windows::Graphics::Imaging::SoftwareBitmap,
    image: &CapturedImage,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<UiTarget>, String> {
    let operation = engine
        .RecognizeAsync(bitmap)
        .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}"))?;
    let result = loop {
        if cancelled() {
            if let Err(error) = operation.Cancel() {
                crate::app::logging::report_error(
                    "windows-vision",
                    format!("cannot cancel system OCR operation: {error}"),
                );
            }
            return Err("system OCR cancelled".into());
        }
        match operation
            .Status()
            .map_err(|error| format!("cannot read system OCR status: {error}"))?
        {
            AsyncStatus::Started => std::thread::sleep(Duration::from_millis(5)),
            AsyncStatus::Completed => break operation.GetResults(),
            AsyncStatus::Canceled => return Err("system OCR cancelled".into()),
            AsyncStatus::Error => {
                let error = operation
                    .ErrorCode()
                    .map_err(|error| format!("cannot read system OCR error: {error}"))?;
                return Err(format!("OcrEngine::RecognizeAsync failed: {error}"));
            }
            _ => return Err("system OCR returned an unknown asynchronous status".into()),
        }
    }
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

fn detect_regions(
    image: &CapturedImage,
    options: &crate::api::VisionOptions,
    scratch: &mut FallbackScratch,
    cancelled: impl Fn() -> bool,
) -> Vec<UiTarget> {
    let source_width = image.width as usize;
    let source_height = image.height as usize;
    let edge_scale = (MAX_FALLBACK_EDGE / source_width.max(source_height) as f64).min(1.0);
    let pixel_scale = (MAX_FALLBACK_PIXELS / (source_width * source_height) as f64)
        .sqrt()
        .min(1.0);
    let analysis_scale = edge_scale.min(pixel_scale);
    let width = (source_width as f64 * analysis_scale).round().max(2.0) as usize;
    let height = (source_height as f64 * analysis_scale).round().max(2.0) as usize;
    if width < 3 || height < 3 || cancelled() {
        return Vec::new();
    }
    let FallbackScratch {
        gray,
        edge,
        dilated,
        closed,
        visited,
        queue,
    } = scratch;
    gray.resize(width * height, 0);
    for y in 0..height {
        if cancelled() {
            return Vec::new();
        }
        let source_y = (y * source_height / height).min(source_height - 1);
        for x in 0..width {
            let source_x = (x * source_width / width).min(source_width - 1);
            let source = (source_y * source_width + source_x) * 4;
            gray[y * width + x] = ((u16::from(image.pixels[source + 2]) * 77
                + u16::from(image.pixels[source + 1]) * 150
                + u16::from(image.pixels[source]) * 29)
                >> 8) as u8;
        }
    }
    edge.resize(width * height, false);
    edge.fill(false);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
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
    dilated.clone_from(edge);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let i = y * width + x;
            dilated[i] = (-1isize..=1).any(|dy| {
                (-1isize..=1).any(|dx| {
                    edge[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    closed.clone_from(dilated);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let i = y * width + x;
            closed[i] = (-1isize..=1).all(|dy| {
                (-1isize..=1).all(|dx| {
                    dilated[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    visited.resize(width * height, false);
    visited.fill(false);
    let mut candidates = Vec::new();
    let configured_minimum =
        (options.rectangle_min_size * width.min(height) as f64).ceil() as usize;
    let minimum_side = configured_minimum.max(6);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let start = y * width + x;
            if !closed[start] || visited[start] {
                continue;
            }
            queue.clear();
            queue.push_back(start as u32);
            visited[start] = true;
            let (mut min_x, mut max_x, mut min_y, mut max_y, mut pixels) = (x, x, y, y, 0usize);
            while let Some(index) = queue.pop_front().map(|index| index as usize) {
                if pixels & 0x03ff == 0 && cancelled() {
                    return Vec::new();
                }
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
                            queue.push_back(next as u32);
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
            let rect = Rect::new(
                image.desktop_bounds.x + min_x as f64 * image.desktop_bounds.width / width as f64,
                image.desktop_bounds.y + min_y as f64 * image.desktop_bounds.height / height as f64,
                box_width as f64 * image.desktop_bounds.width / width as f64,
                box_height as f64 * image.desktop_bounds.height / height as f64,
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
    if cancelled() {
        return Vec::new();
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    candidates
        .into_iter()
        .take(options.rectangle_max_candidates.min(2_000))
        .map(|(_, target)| target)
        .collect()
}

#[derive(Default)]
struct FallbackScratch {
    gray: Vec<u8>,
    edge: Vec<bool>,
    dilated: Vec<bool>,
    closed: Vec<bool>,
    visited: Vec<bool>,
    queue: VecDeque<u32>,
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
    use std::sync::atomic::AtomicUsize;

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
        let targets = detect_regions(
            &image,
            &crate::api::VisionOptions::default(),
            &mut FallbackScratch::default(),
            || false,
        );
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
    fn worker_start_does_not_wait_for_ocr_discovery_or_start_a_coordinator() {
        let started = Instant::now();
        let mut worker = VisionWorker::start();
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(worker.workers.is_empty());
        assert!(worker.discovery.worker.is_none());
        worker.stop().unwrap();
        worker.stop().unwrap();
    }

    #[test]
    fn cancellation_does_not_start_an_idle_coordinator() {
        let mut worker = VisionWorker::start();
        worker.shared.latest_generation.store(17, Ordering::Release);
        worker
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active_request_id = Some(91);
        worker.cancel(91);
        assert_eq!(worker.shared.latest_generation.load(Ordering::Acquire), 0);
        assert!(worker.workers.is_empty());
        worker.stop().unwrap();
    }

    #[test]
    fn completed_discovery_snapshot_is_reused() {
        let shared = Arc::new(DiscoveryShared::default());
        *shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            DiscoveryState::Ready(OcrDiscoverySnapshot::default());
        let discovery = DiscoveryHandle(shared);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(discovery.wait(deadline, || false).is_some());
        assert!(discovery.wait(deadline, || false).is_some());
    }

    #[test]
    fn cancelling_an_old_active_scan_does_not_invalidate_a_new_pending_scan() {
        assert!(!cancellation_clears_generation(Some(7), Some(8), 7));
        assert!(cancellation_clears_generation(Some(7), Some(8), 8));
        assert!(cancellation_clears_generation(Some(7), None, 7));
        assert!(!cancellation_clears_generation(Some(7), None, 9));
    }

    #[test]
    fn provider_group_cancels_and_joins_every_thread() {
        let shared = Arc::new(SharedQueue::default());
        shared.latest_generation.store(23, Ordering::Release);
        let cancellation = ScanCancellation::new(&shared, 23);
        let provider_cancellation = cancellation.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let provider_stopped = Arc::clone(&stopped);
        let mut providers = ProviderThreads::new(cancellation);
        providers.spawn("keysteer-cancellation-test", move || {
            while !provider_cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            provider_stopped.store(true, Ordering::Release);
        });
        drop(providers);
        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn pure_rust_detector_stops_at_cancellation_checkpoints() {
        let image = CapturedImage {
            pixels: vec![255; 256 * 256 * 4],
            width: 256,
            height: 256,
            desktop_bounds: Rect::new(0.0, 0.0, 256.0, 256.0),
            scale: 1.0,
        };
        let checks = AtomicUsize::new(0);
        let targets = detect_regions(
            &image,
            &crate::api::VisionOptions::default(),
            &mut FallbackScratch::default(),
            || checks.fetch_add(1, Ordering::Relaxed) >= 2,
        );
        assert!(targets.is_empty());
        assert!(checks.load(Ordering::Relaxed) >= 3);
    }

    #[test]
    #[ignore = "requires an installed Windows OCR language pack"]
    fn live_system_ocr_runtime_probe_creates_and_drops_an_engine() {
        assert!(probe_system_ocr(|| false).is_ok());
    }
}
