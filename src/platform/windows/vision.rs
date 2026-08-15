//! Native Windows visual UI-hint scanning without OpenCV.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use windows::Media::Ocr::OcrEngine;
use windows::Win32::Foundation::HWND;
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

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
const PROVIDER_STOP_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug)]
enum VisionError {
    Cancelled,
    TimedOut,
    Unavailable(String),
    Operational(String),
    Cleanup(String),
}

impl VisionError {
    fn is_control_flow(&self) -> bool {
        matches!(self, Self::Cancelled | Self::TimedOut)
    }
}

impl std::fmt::Display for VisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("provider cancelled"),
            Self::TimedOut => formatter.write_str("provider timed out"),
            Self::Unavailable(error) | Self::Operational(error) | Self::Cleanup(error) => {
                formatter.write_str(error)
            }
        }
    }
}

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
    vision_disabled: AtomicBool,
    provider_quarantine: Mutex<Vec<WorkerJoin>>,
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
    started: AtomicBool,
    completed: AtomicBool,
}

impl Default for DiscoveryShared {
    fn default() -> Self {
        Self {
            state: Mutex::new(DiscoveryState::Pending),
            ready: Condvar::new(),
            stopping: AtomicBool::new(false),
            started: AtomicBool::new(false),
            completed: AtomicBool::new(false),
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
        if self.shared.stopping.load(Ordering::Acquire)
            || self
                .shared
                .started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
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
            self.shared.completed.store(true, Ordering::Release);
            self.shared.ready.notify_all();
        })
        .ok();
    }

    fn handle(&self) -> DiscoveryHandle {
        DiscoveryHandle(Arc::clone(&self.shared))
    }

    fn reap_finished(&mut self) -> Result<(), String> {
        if !self.shared.completed.load(Ordering::Acquire) {
            return Ok(());
        }
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
    shared: Arc<SharedQueue>,
    threads: Vec<WorkerJoin>,
}

impl ProviderThreads {
    fn new(cancellation: ScanCancellation, shared: &Arc<SharedQueue>) -> Self {
        Self {
            cancellation,
            shared: Arc::clone(shared),
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
        match WorkerJoin::spawn(
            name,
            std::thread::Builder::new().name(name.into()),
            background_work,
        ) {
            Ok(worker) => {
                self.threads.push(worker);
                true
            }
            Err(error) => {
                crate::report_warning!("windows-vision", "cannot start {name} provider: {error}");
                false
            }
        }
    }

    fn join_all(&mut self, deadline: Instant) -> Result<(), String> {
        self.cancellation.cancel();
        let mut failures = Vec::new();
        let mut quarantine = Vec::new();
        for mut worker in self.threads.drain(..) {
            if let Err(error) = worker.join_until(deadline) {
                failures.push(error);
                quarantine.push(worker);
            }
        }
        if !quarantine.is_empty() {
            self.shared.vision_disabled.store(true, Ordering::Release);
            self.shared
                .provider_quarantine
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(quarantine);
        }
        failures
            .is_empty()
            .then_some(())
            .ok_or_else(|| failures.join("; "))
    }
}

impl Drop for ProviderThreads {
    fn drop(&mut self) {
        if let Err(error) = self.join_all(Instant::now() + PROVIDER_STOP_TIMEOUT) {
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
        if self.shared.vision_disabled.load(Ordering::Acquire) {
            return Err("visual OCR was disabled after a provider failed to stop".into());
        }
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
                    drop(self.workers.swap_remove(index));
                }
                Ok(false) => index += 1,
                Err(error) => {
                    crate::app::logging::report_error("windows-vision", error);
                    drop(self.workers.swap_remove(index));
                }
            }
        }
        let mut quarantine = self
            .shared
            .provider_quarantine
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut index = 0;
        while index < quarantine.len() {
            match quarantine[index].reap_finished() {
                Ok(true) => {
                    drop(quarantine.swap_remove(index));
                }
                Ok(false) => index += 1,
                Err(error) => {
                    crate::app::logging::report_error("windows-vision", error);
                    drop(quarantine.swap_remove(index));
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
                    drop(self.workers.swap_remove(index));
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
        let deadline = Instant::now() + STOP_TIMEOUT;
        let mut quarantine = self
            .shared
            .provider_quarantine
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut index = 0;
        while index < quarantine.len() {
            match quarantine[index].join_until(deadline) {
                Ok(()) => {
                    drop(quarantine.swap_remove(index));
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

struct OcrFrame {
    geometry: CaptureGeometry,
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
    crate::app::perf_probe::mark("vision_terminal_cleanup");
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
    let mut providers = ProviderThreads::new(cancellation.clone(), shared);
    let mut provider_inputs = Vec::with_capacity(2);
    let mut pending_ocr = 0usize;
    if let Some(descriptor) = discovery_snapshot.system {
        let (image_tx, image_rx) = mpsc::sync_channel(1);
        let result_tx = tx.clone();
        let provider_cancellation = cancellation.clone();
        if providers.spawn("keysteer-system-ocr", move || {
            crate::app::perf_probe::mark("system_ocr_started");
            let started = Instant::now();
            let result =
                recognize_system_provider(descriptor, image_rx, deadline, &provider_cancellation);
            let _ = result_tx.send(ProviderEvent::Ocr {
                provider: "system",
                elapsed: started.elapsed(),
                result,
            });
            crate::app::perf_probe::mark("system_ocr_finished");
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
            crate::app::perf_probe::mark("wechat_ocr_started");
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
            crate::app::perf_probe::mark("wechat_ocr_finished");
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
    crate::app::perf_probe::mark("capture_hidden_ack");
    let geometry = match capture_geometry(bounds) {
        Ok(geometry) => geometry,
        Err(error) => {
            if let Err(release_error) = capture_lease.release() {
                crate::app::logging::report_error("windows-overlay", release_error);
            }
            return UiScanStatus::Failed(error);
        }
    };
    let bitmap_apartment = if provider_inputs.is_empty() {
        None
    } else {
        match super::native::ComApartment::initialise() {
            Ok(apartment) => Some(apartment),
            Err(error) => {
                crate::app::logging::report_error("windows-vision", &error);
                None
            }
        }
    };
    let mut capture_lease = Some(capture_lease);
    let mut context_changed_during_capture = false;
    let captured = super::native::capture_bgra_with(
        bounds.x.floor() as i32,
        bounds.y.floor() as i32,
        bounds.width.ceil() as i32,
        bounds.height.ceil() as i32,
        geometry.width as i32,
        geometry.height as i32,
        |pixels, width, height| {
            crate::app::perf_probe::mark("capture_gdi_ready");
            if width != geometry.width || height != geometry.height {
                return Err("native capture dimensions changed unexpectedly".into());
            }
            if !current(shared, job.generation, original) {
                context_changed_during_capture = true;
                return Err("visual capture context changed".into());
            }
            // The DIB already contains a stable desktop frame. Release the
            // generation gate before constructing OCR/fallback artifacts so a
            // deferred UIA frame can be shown without waiting for pixel work.
            if let Some(lease) = capture_lease.take() {
                lease.release()?;
            }
            let bitmap = bitmap_apartment.as_ref().and_then(|_| {
                match super::native::software_bitmap_bgra(pixels, width, height) {
                    Ok(bitmap) => Some(Arc::new(SharedSoftwareBitmap(bitmap))),
                    Err(error) => {
                        crate::app::logging::report_error("windows-vision", error);
                        None
                    }
                }
            });
            if bitmap.is_some() {
                crate::app::perf_probe::mark("ocr_bitmap_ready");
            }
            if let Some(bitmap) = bitmap.as_ref() {
                let frame = Arc::new(OcrFrame {
                    geometry,
                    bitmap: Arc::clone(bitmap),
                });
                for input in provider_inputs.drain(..) {
                    let _ = input.send(Arc::clone(&frame));
                }
            }
            let fallback = job
                .request
                .vision
                .detect_rectangles
                .then(|| fallback_input_from_bgra(pixels, geometry))
                .transpose()?;
            Ok((bitmap, fallback))
        },
    );
    super::native::release_capture_surface();
    if let Some(lease) = capture_lease.take()
        && let Err(error) = lease.release()
    {
        crate::app::logging::report_error("windows-overlay", error);
    }
    let (shared_bitmap, fallback_input) = match captured {
        Ok(artifacts) => artifacts,
        Err(_) if context_changed_during_capture => return UiScanStatus::ContextChanged,
        Err(error) => return UiScanStatus::Failed(error),
    };
    if !current(shared, job.generation, original) {
        return UiScanStatus::ContextChanged;
    }

    drop(provider_inputs);

    let fallback_cancelled = Arc::new(AtomicBool::new(false));
    let fallback_pending = if let Some(fallback_input) = fallback_input {
        let result_tx = tx.clone();
        let options = job.request.vision.clone();
        let provider_cancellation = cancellation.clone();
        let fallback_cancelled = Arc::clone(&fallback_cancelled);
        providers.spawn("keysteer-vision-fallback", move || {
            crate::app::perf_probe::mark("vision_fallback_started");
            let mut scratch = FallbackScratch::default();
            let targets = detect_regions(&fallback_input, &options, &mut scratch, || {
                provider_cancellation.is_cancelled() || fallback_cancelled.load(Ordering::Acquire)
            });
            let _ = result_tx.send(ProviderEvent::Fallback(targets));
            crate::app::perf_probe::mark("vision_fallback_finished");
        })
    } else {
        false
    };
    drop(tx);

    let mut fallback = None;
    let mut ocr_had_valid_targets = false;
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
                    if count != 0 {
                        ocr_had_valid_targets = true;
                        fallback_cancelled.store(true, Ordering::Release);
                    }
                    let accepted = job.source.push(targets);
                    if accepted != 0 {
                        crate::app::perf_probe::mark("vision_targets_accepted");
                    }
                    crate::log_info!(
                        "windows-vision",
                        "{provider} OCR completed in {elapsed:?} with {count} valid targets ({accepted} new)"
                    );
                }
                Err(error) if error.is_control_flow() => {}
                Err(error @ VisionError::Unavailable(_)) => crate::report_warning!(
                    "windows-vision",
                    "{provider} OCR failed after {elapsed:?}: {error}"
                ),
                Err(error) => crate::report_error!(
                    "windows-vision",
                    "{provider} OCR failed after {elapsed:?}: {error}"
                ),
            }
        }
    }

    if should_publish_fallback(ocr_had_valid_targets, job.request.vision.detect_rectangles)
        && let Some(targets) = fallback
    {
        job.source.push(targets);
    }
    if let Err(error) = providers.join_all(Instant::now() + PROVIDER_STOP_TIMEOUT) {
        crate::app::logging::report_error("windows-vision", error);
    }
    drop(shared_bitmap);
    drop(bitmap_apartment);
    if !current(shared, job.generation, original) {
        UiScanStatus::ContextChanged
    } else if timed_out {
        UiScanStatus::TimedOut
    } else {
        UiScanStatus::Success
    }
}

fn should_publish_fallback(ocr_had_valid_targets: bool, rectangles_enabled: bool) -> bool {
    rectangles_enabled && !ocr_had_valid_targets
}

fn wait_provider_image(
    receiver: mpsc::Receiver<Arc<OcrFrame>>,
    deadline: Instant,
    cancellation: &ScanCancellation,
) -> Result<Arc<OcrFrame>, VisionError> {
    loop {
        if cancellation.is_cancelled() {
            return Err(VisionError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(VisionError::TimedOut);
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(image) => return Ok(image),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(VisionError::Cancelled);
            }
        }
    }
}

fn recognize_system_provider(
    descriptor: SystemOcrDescriptor,
    receiver: mpsc::Receiver<Arc<OcrFrame>>,
    deadline: Instant,
    cancellation: &ScanCancellation,
) -> Result<Vec<UiTarget>, VisionError> {
    let _apartment = super::native::ComApartment::initialise().map_err(VisionError::Operational)?;
    let engine = super::native::create_system_ocr_engine().map_err(VisionError::Unavailable)?;
    let input = wait_provider_image(receiver, deadline, cancellation)?;
    if input.geometry.width > descriptor.maximum_dimension
        || input.geometry.height > descriptor.maximum_dimension
    {
        return Err(VisionError::Operational(format!(
            "captured image exceeds system OCR limit {}",
            descriptor.maximum_dimension
        )));
    }
    recognize_system(&engine, &input.bitmap.0, input.geometry, || {
        cancellation.is_cancelled()
    })
    .map_err(|error| {
        if cancellation.is_cancelled() {
            VisionError::Cancelled
        } else if Instant::now() >= deadline {
            VisionError::TimedOut
        } else {
            VisionError::Operational(error)
        }
    })
}

fn recognize_wechat_provider(
    descriptor: WechatDescriptor,
    receiver: mpsc::Receiver<Arc<OcrFrame>>,
    deadline: Instant,
    minimum_confidence: f64,
    cancellation: &ScanCancellation,
) -> Result<Vec<UiTarget>, VisionError> {
    let _apartment = super::native::ComApartment::initialise().map_err(VisionError::Operational)?;
    let mut provider = WechatOcr::start(&descriptor, deadline, &|| cancellation.is_cancelled())
        .map_err(VisionError::Unavailable)?;
    let result = wait_provider_image(receiver, deadline, cancellation).and_then(|input| {
        provider
            .recognize(
                input.geometry,
                &input.bitmap.0,
                deadline.saturating_duration_since(Instant::now()),
                minimum_confidence,
                || cancellation.is_cancelled(),
            )
            .map_err(|error| {
                if cancellation.is_cancelled() {
                    VisionError::Cancelled
                } else if Instant::now() >= deadline {
                    VisionError::TimedOut
                } else {
                    VisionError::Operational(error)
                }
            })
    });
    let cleanup = provider.shutdown();
    match (result, cleanup) {
        (Ok(targets), Ok(())) => Ok(targets),
        (Err(_), Ok(())) if cancellation.is_cancelled() => Err(VisionError::Cancelled),
        (Err(_), Ok(())) if Instant::now() >= deadline => Err(VisionError::TimedOut),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(VisionError::Cleanup(error)),
        (Err(error), Err(cleanup)) => {
            Err(VisionError::Cleanup(format!("{error}; cleanup: {cleanup}")))
        }
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
        result: Result<Vec<UiTarget>, VisionError>,
    },
    Fallback(Vec<UiTarget>),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CaptureGeometry {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) desktop_bounds: Rect,
    pub(super) scale: f64,
}

struct FallbackInput {
    gray: Vec<u8>,
    width: usize,
    height: usize,
    desktop_bounds: Rect,
}

fn capture_geometry(bounds: Rect) -> Result<CaptureGeometry, String> {
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

fn fallback_input_from_bgra(
    pixels: &[u8],
    geometry: CaptureGeometry,
) -> Result<FallbackInput, String> {
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
    let mut gray = vec![0; width * height];
    for y in 0..height {
        let source_y = (y * source_height / height).min(source_height - 1);
        for x in 0..width {
            let source_x = (x * source_width / width).min(source_width - 1);
            let source = (source_y * source_width + source_x) * 4;
            gray[y * width + x] = ((u16::from(pixels[source + 2]) * 77
                + u16::from(pixels[source + 1]) * 150
                + u16::from(pixels[source]) * 29)
                >> 8) as u8;
        }
    }
    Ok(FallbackInput {
        gray,
        width,
        height,
        desktop_bounds: geometry.desktop_bounds,
    })
}

fn discover_ocr(shared: Arc<DiscoveryShared>) {
    if shared.stopping.load(Ordering::Acquire) {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *state = DiscoveryState::Unavailable;
        shared.completed.store(true, Ordering::Release);
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
    shared.completed.store(true, Ordering::Release);
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
        let (maximum, languages) = super::native::probe_system_ocr_factory()?;
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
    geometry: CaptureGeometry,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<UiTarget>, String> {
    let operation = engine
        .RecognizeAsync(bitmap)
        .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}"))?;
    let (completed_tx, completed_rx) = mpsc::sync_channel(1);
    operation
        .SetCompleted(&AsyncOperationCompletedHandler::new(move |_, status| {
            let _ = completed_tx.try_send(status);
            Ok(())
        }))
        .map_err(|error| format!("cannot register system OCR completion: {error}"))?;
    let async_result = loop {
        if cancelled() {
            let cancel = operation
                .Cancel()
                .map_err(|error| format!("cannot cancel system OCR operation: {error}"));
            let _ = completed_rx.recv_timeout(Duration::from_millis(100));
            let close = operation
                .Close()
                .map_err(|error| format!("cannot close cancelled system OCR operation: {error}"));
            return match (cancel, close) {
                (Ok(()), Ok(())) => Err("system OCR cancelled".into()),
                (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                (Err(error), Err(close)) => Err(format!("{error}; cleanup: {close}")),
            };
        }
        match completed_rx.recv_timeout(Duration::from_millis(2)) {
            Ok(AsyncStatus::Completed) => break operation.GetResults(),
            Ok(AsyncStatus::Canceled) => {
                break Err(windows::core::Error::new(
                    windows::core::HRESULT(0x8007_04C7u32 as i32),
                    "system OCR cancelled",
                ));
            }
            Ok(AsyncStatus::Error) => {
                let error = operation
                    .ErrorCode()
                    .map_err(|error| format!("cannot read system OCR error: {error}"))?;
                break Err(error.into());
            }
            Ok(AsyncStatus::Started) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(_) => return Err("system OCR returned an unknown asynchronous status".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("system OCR completion handler disconnected".into());
            }
        }
    };
    let close = operation
        .Close()
        .map_err(|error| format!("cannot close system OCR operation: {error}"));
    let result = match (async_result, close) {
        (Ok(result), Ok(())) => result,
        (Err(error), Ok(())) => {
            return Err(format!("OcrEngine::RecognizeAsync failed: {error}"));
        }
        (Ok(_), Err(error)) => return Err(error),
        (Err(error), Err(close)) => {
            return Err(format!(
                "OcrEngine::RecognizeAsync failed: {error}; cleanup: {close}"
            ));
        }
    };
    let lines = result
        .Lines()
        .map_err(|error| format!("cannot read OCR lines: {error}"))?;
    let count = lines
        .Size()
        .map_err(|error| format!("cannot read OCR line count: {error}"))?;
    let mut targets = Vec::with_capacity((count as usize).min(2_000));
    for index in 0..count {
        if targets.len() == 2_000 {
            break;
        }
        let line = lines
            .GetAt(index)
            .map_err(|error| format!("cannot read OCR line {index}: {error}"))?;
        let mut text = line
            .Text()
            .map_err(|error| format!("cannot read OCR line text: {error}"))?
            .to_string();
        trim_string_in_place(&mut text);
        if text.is_empty() {
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
                geometry,
                Rect::new(
                    f64::from(native.X),
                    f64::from(native.Y),
                    f64::from(native.Width),
                    f64::from(native.Height),
                ),
            );
            union = Some(union.map_or(rect, |current| current.union(&rect)));
        }
        if let Some(rect) = union.filter(|rect| valid_target_rect(*rect, geometry.desktop_bounds)) {
            targets.push(UiTarget {
                rect,
                name: text,
                role: "static_text".into(),
                native_role: Some("vision:windows-ocr".into()),
            });
        }
    }
    Ok(targets)
}

pub(super) fn trim_string_in_place(value: &mut String) {
    let start = value.len() - value.trim_start().len();
    let end = value.trim_end().len();
    value.truncate(end);
    if start != 0 {
        value.drain(..start);
    }
}

pub(super) fn image_to_desktop(image: CaptureGeometry, rect: Rect) -> Rect {
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
    image: &FallbackInput,
    options: &crate::api::VisionOptions,
    scratch: &mut FallbackScratch,
    cancelled: impl Fn() -> bool,
) -> Vec<UiTarget> {
    let width = image.width;
    let height = image.height;
    if width < 3 || height < 3 || cancelled() {
        return Vec::new();
    }
    let FallbackScratch {
        edge,
        dilated,
        closed,
        queue,
    } = scratch;
    let gray = &image.gray;
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
    // `edge` is dead after the close pass; reuse its allocation as the visited
    // bitmap instead of retaining a fifth full analysis-plane buffer.
    edge.fill(false);
    let visited = edge;
    let candidate_limit = options.rectangle_max_candidates.min(2_000);
    let mut candidates = BinaryHeap::with_capacity(candidate_limit);
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
                    let candidate = Reverse(RegionCandidate {
                        confidence,
                        rect,
                        role,
                        native_role,
                    });
                    if candidates.len() < candidate_limit {
                        candidates.push(candidate);
                    } else if candidates
                        .peek()
                        .is_some_and(|current| candidate.0 > current.0)
                    {
                        candidates.pop();
                        candidates.push(candidate);
                    }
                }
            }
        }
    }
    if cancelled() {
        return Vec::new();
    }
    candidates
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse(candidate)| UiTarget {
            rect: candidate.rect,
            name: String::new(),
            role: candidate.role.into(),
            native_role: Some(candidate.native_role.into()),
        })
        .collect()
}

#[derive(Default)]
struct FallbackScratch {
    edge: Vec<bool>,
    dilated: Vec<bool>,
    closed: Vec<bool>,
    queue: VecDeque<u32>,
}

#[derive(Clone, Copy)]
struct RegionCandidate {
    confidence: f64,
    rect: Rect,
    role: &'static str,
    native_role: &'static str,
}

impl PartialEq for RegionCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.confidence.to_bits() == other.confidence.to_bits()
    }
}

impl Eq for RegionCandidate {}

impl PartialOrd for RegionCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RegionCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.confidence.total_cmp(&other.confidence)
    }
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
        let image = CaptureGeometry {
            width: 500,
            height: 300,
            desktop_bounds: Rect::new(-1_000.0, -200.0, 2_000.0, 1_200.0),
            scale: 0.5,
        };
        assert_eq!(
            image_to_desktop(image, Rect::new(25.0, 10.0, 50.0, 20.0)),
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
        let geometry = CaptureGeometry {
            width: 120,
            height: 80,
            desktop_bounds: Rect::new(0.0, 0.0, 120.0, 80.0),
            scale: 1.0,
        };
        let image = fallback_input_from_bgra(&pixels, geometry).unwrap();
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
    fn valid_ocr_suppresses_fallback_even_when_spatial_dedup_accepts_nothing() {
        assert!(!should_publish_fallback(true, true));
        assert!(should_publish_fallback(false, true));
        assert!(!should_publish_fallback(false, false));
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
        let mut providers = ProviderThreads::new(cancellation, &shared);
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
        let geometry = CaptureGeometry {
            width: 256,
            height: 256,
            desktop_bounds: Rect::new(0.0, 0.0, 256.0, 256.0),
            scale: 1.0,
        };
        let image = fallback_input_from_bgra(&vec![255; 256 * 256 * 4], geometry).unwrap();
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
