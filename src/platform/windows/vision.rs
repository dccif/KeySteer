#![forbid(unsafe_code)]

//! Native Windows visual UI-hint scanning without OpenCV.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use windows::Media::Ocr::{OcrEngine, OcrResult};
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncOperation};

use crate::api::command::UiScanStatus;
use crate::api::geometry::{Rect, UiTarget};
use crate::app::worker::WorkerJoin;

use super::accessibility::WindowsScanPlan;
use super::overlay_worker::CaptureLease;
use super::ui_scan::ScanSource;
use super::wechat_ocr::{WechatDescriptor, WechatOcr};

// Covers a native 3840x2160 desktop so common 4K scans use the BitBlt fast
// path. Larger 5K/8K images are still scaled as a single capture.
const MAX_CAPTURE_PIXELS: f64 = 8_388_608.0;
const MAX_CAPTURE_EDGE: f64 = 4_096.0;
const MAX_FALLBACK_PIXELS: f64 = 2_073_600.0;
const MAX_FALLBACK_EDGE: f64 = 2_560.0;
const STOP_TIMEOUT: Duration = Duration::from_secs(2);
const PROVIDER_STOP_TIMEOUT: Duration = Duration::from_millis(500);
const PROVIDER_BATCH_SIZE: usize = 24;
const MAX_OCR_TARGETS: usize = 2_000;
const MIN_SYSTEM_OCR_TILE_SIDE: u32 = 64;
const SYSTEM_OCR_TILE_OVERLAP: u32 = 64;

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
    request: Arc<WindowsScanPlan>,
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

#[derive(Debug)]
enum OcrExecutionPlan {
    None,
    SystemOnly(SystemOcrDescriptor),
    WechatOnly(WechatDescriptor),
    Dual {
        system: SystemOcrDescriptor,
        wechat: WechatDescriptor,
    },
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OcrExecutionKind {
    None,
    SystemOnly,
    WechatOnly,
    Dual,
}

#[cfg(test)]
fn ocr_execution_kind(
    system_available: bool,
    wechat_available: bool,
    detect_text: bool,
) -> OcrExecutionKind {
    if !detect_text {
        return OcrExecutionKind::None;
    }
    match (system_available, wechat_available) {
        (false, false) => OcrExecutionKind::None,
        (true, false) => OcrExecutionKind::SystemOnly,
        (false, true) => OcrExecutionKind::WechatOnly,
        (true, true) => OcrExecutionKind::Dual,
    }
}

impl OcrExecutionPlan {
    fn from_snapshot(snapshot: OcrDiscoverySnapshot, detect_text: bool) -> Self {
        if !detect_text {
            return Self::None;
        }
        match (snapshot.system, snapshot.wechat) {
            (Some(system), Some(wechat)) => Self::Dual { system, wechat },
            (Some(system), None) => Self::SystemOnly(system),
            (None, Some(wechat)) => Self::WechatOnly(wechat),
            (None, None) => Self::None,
        }
    }

    fn into_descriptors(self) -> (Option<SystemOcrDescriptor>, Option<WechatDescriptor>) {
        match self {
            Self::None => (None, None),
            Self::SystemOnly(system) => (Some(system), None),
            Self::WechatOnly(wechat) => (None, Some(wechat)),
            Self::Dual { system, wechat } => (Some(system), Some(wechat)),
        }
    }
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
        request: Arc<WindowsScanPlan>,
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
        let mut errors = crate::app::errors::ErrorBundle::default();
        errors.record("OCR discovery shutdown", self.discovery.stop());
        let mut index = 0;
        while index < self.workers.len() {
            match self.workers[index].join_timeout(STOP_TIMEOUT) {
                Ok(()) => {
                    drop(self.workers.swap_remove(index));
                }
                Err(error) => {
                    errors.push("vision coordinator shutdown", error);
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
                    errors.push("quarantined provider shutdown", error);
                    index += 1;
                }
            }
        }
        errors.into_result()
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

fn generation_is_current(shared: &SharedQueue, generation: u64) -> bool {
    !shared.stopping.load(Ordering::Acquire)
        && shared.latest_generation.load(Ordering::Acquire) == generation
}

fn context_is_current(shared: &SharedQueue, generation: u64, plan: &WindowsScanPlan) -> bool {
    generation_is_current(shared, generation) && plan.target_is_current()
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

struct WechatFullFrame {
    geometry: CaptureGeometry,
    bitmap: Option<windows::Graphics::Imaging::SoftwareBitmap>,
    _ledger: crate::app::perf_probe::ResourceGuard,
}

enum WechatInput {
    Frame(WechatFullFrame),
    Failed(String),
}

impl WechatFullFrame {
    fn take_bitmap(&mut self) -> Result<windows::Graphics::Imaging::SoftwareBitmap, VisionError> {
        self.bitmap
            .take()
            .ok_or_else(|| VisionError::Cleanup("WeChat OCR bitmap was already consumed".into()))
    }
}

impl Drop for WechatFullFrame {
    fn drop(&mut self) {
        if let Some(bitmap) = self.bitmap.take()
            && let Err(error) = bitmap.Close()
        {
            crate::app::logging::report_error(
                "windows-vision",
                format!("cannot close unused WeChat OCR bitmap: {error}"),
            );
        }
    }
}

struct SystemOcrTile {
    geometry: CaptureGeometry,
    core_bounds: Rect,
    bitmap: SharedSoftwareBitmap,
}

enum SystemOcrInput {
    Begin { tile_count: usize },
    Tile(SystemOcrTile),
    Completed { index: usize, status: AsyncStatus },
    Failed(String),
    Done,
}

struct SharedSoftwareBitmap {
    bitmap: Option<windows::Graphics::Imaging::SoftwareBitmap>,
    _ledger: crate::app::perf_probe::ResourceGuard,
}

impl SharedSoftwareBitmap {
    fn bitmap(&self) -> Result<&windows::Graphics::Imaging::SoftwareBitmap, VisionError> {
        self.bitmap
            .as_ref()
            .ok_or_else(|| VisionError::Cleanup("OCR SoftwareBitmap was already closed".into()))
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(bitmap) = self.bitmap.take() else {
            return Ok(());
        };
        bitmap
            .Close()
            .map_err(|error| format!("cannot close OCR SoftwareBitmap: {error}"))
    }
}

impl Drop for SharedSoftwareBitmap {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            crate::app::logging::report_error("windows-vision", error);
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
    let _coordinator_ledger = crate::app::perf_probe::ResourceGuard::new(
        crate::app::perf_probe::ResourceKind::Coordinator,
    );
    let deadline = Instant::now()
        + Duration::from_millis(
            job.request
                .vision
                .request_timeout_ms
                .min(job.request.timeout_ms.max(250))
                .clamp(250, 30_000),
        );
    let bounds = job.request.scan_bounds();
    let geometry = match capture_geometry(bounds) {
        Ok(geometry) => geometry,
        Err(error) => {
            let mut errors = crate::app::errors::ErrorBundle::default();
            errors.push("capture geometry", error);
            if let Some(capture) = job.capture.take() {
                errors.record("capture gate release", capture.release());
            }
            return UiScanStatus::Failed(
                errors
                    .into_result()
                    .err()
                    .unwrap_or_else(|| "capture geometry failed without details".into()),
            );
        }
    };
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

    let (system_descriptor, wechat_descriptor) =
        OcrExecutionPlan::from_snapshot(discovery_snapshot, job.request.vision.detect_text)
            .into_descriptors();
    let (tx, rx) = mpsc::sync_channel(3);
    let mut providers = ProviderThreads::new(cancellation.clone(), shared);
    let mut system_input = None;
    let mut wechat_input = None;
    let mut pending_ocr = 0usize;
    if let Some(descriptor) = system_descriptor {
        let tile_count = system_ocr_tile_count(geometry);
        let (image_tx, image_rx) =
            mpsc::sync_channel(tile_count.saturating_mul(2).saturating_add(2));
        let completion_tx = image_tx.clone();
        let result_tx = tx.clone();
        let provider_cancellation = cancellation.clone();
        if providers.spawn("keysteer-system-ocr", move || {
            let _provider_ledger = crate::app::perf_probe::ResourceGuard::new(
                crate::app::perf_probe::ResourceKind::Provider,
            );
            crate::app::perf_probe::mark("system_ocr_started");
            let started = Instant::now();
            let result = recognize_system_provider(
                descriptor,
                image_rx,
                completion_tx,
                deadline,
                &provider_cancellation,
                &result_tx,
                started,
            );
            let _ = result_tx.send(ProviderEvent::OcrDone {
                provider: "system",
                elapsed: started.elapsed(),
                result,
            });
            crate::app::perf_probe::mark("system_ocr_finished");
        }) {
            system_input = Some(image_tx);
            pending_ocr += 1;
        }
    }
    if let Some(descriptor) = wechat_descriptor {
        let (image_tx, image_rx) = mpsc::sync_channel(1);
        let result_tx = tx.clone();
        let provider_cancellation = cancellation.clone();
        let minimum_confidence = job.request.vision.minimum_confidence;
        if providers.spawn("keysteer-wechat-ocr", move || {
            let _provider_ledger = crate::app::perf_probe::ResourceGuard::new(
                crate::app::perf_probe::ResourceKind::Provider,
            );
            crate::app::perf_probe::mark("wechat_ocr_started");
            let started = Instant::now();
            let result = recognize_wechat_provider(
                descriptor,
                image_rx,
                deadline,
                minimum_confidence,
                &provider_cancellation,
                &result_tx,
                started,
            );
            let _ = result_tx.send(ProviderEvent::OcrDone {
                provider: "wechat",
                elapsed: started.elapsed(),
                result,
            });
            crate::app::perf_probe::mark("wechat_ocr_finished");
        }) {
            wechat_input = Some(image_tx);
            pending_ocr += 1;
        }
    }
    let fallback_cancelled = Arc::new(AtomicBool::new(false));
    let mut ocr_had_valid_targets = false;
    let mut early_events = VecDeque::new();

    let Some(mut capture_lease) = job.capture.take() else {
        return UiScanStatus::Failed("visual capture lease was not created".into());
    };
    // Geometry, the thread-bound DIB and the WinRT apartment are prepared
    // while the renderer is independently hiding its tree. No desktop pixels
    // are read before the hidden ACK.
    let mut prepared_capture =
        match super::native::PreparedCapture::new(geometry.width, geometry.height) {
            Ok(capture) => capture,
            Err(error) => {
                let mut errors = crate::app::errors::ErrorBundle::default();
                errors.push("capture DIB preparation", error);
                errors.record("capture gate release", capture_lease.release());
                return UiScanStatus::Failed(
                    errors
                        .into_result()
                        .err()
                        .unwrap_or_else(|| "capture DIB preparation failed without details".into()),
                );
            }
        };
    let gdi_ledger = crate::app::perf_probe::ResourceGuard::new(
        crate::app::perf_probe::ResourceKind::GdiSurface,
    );
    crate::app::perf_probe::mark("capture_dib_prepared");
    let mut bitmap_factory_error = None;
    let bitmap_apartment = if system_input.is_none() && wechat_input.is_none() {
        None
    } else {
        match super::native::ComApartment::initialise() {
            Ok(apartment) => Some(apartment),
            Err(error) => {
                bitmap_factory_error = Some(error);
                None
            }
        }
    };
    let bitmap_factory = bitmap_apartment.as_ref().and_then(|_| {
        match super::native::SoftwareBitmapFactory::load() {
            Ok(factory) => Some(factory),
            Err(error) => {
                bitmap_factory_error = Some(error);
                None
            }
        }
    });
    if let Err(error) =
        capture_lease.wait_hidden(deadline, || !generation_is_current(shared, job.generation))
    {
        let current = context_is_current(shared, job.generation, &job.request);
        if let Err(release_error) = capture_lease.release() {
            crate::app::logging::report_error("windows-overlay", release_error);
        }
        if !current {
            return UiScanStatus::ContextChanged;
        }
        if Instant::now() >= deadline {
            return UiScanStatus::TimedOut;
        }
        return UiScanStatus::Failed(error);
    }
    if !context_is_current(shared, job.generation, &job.request) {
        if let Err(error) = capture_lease.release() {
            crate::app::logging::report_error("windows-overlay", error);
        }
        return UiScanStatus::ContextChanged;
    }
    crate::app::perf_probe::mark("capture_hidden_ack");
    let mut capture_lease = Some(capture_lease);
    let mut context_changed_during_capture = false;
    let captured = prepared_capture.capture_with(
        bounds.x.floor() as i32,
        bounds.y.floor() as i32,
        bounds.width.ceil() as i32,
        bounds.height.ceil() as i32,
        |pixels, width, height| {
            crate::app::perf_probe::mark("capture_gdi_ready");
            if width != geometry.width || height != geometry.height {
                return Err("native capture dimensions changed unexpectedly".into());
            }
            if !context_is_current(shared, job.generation, &job.request) {
                context_changed_during_capture = true;
                return Err("visual capture context changed".into());
            }
            // The DIB already contains a stable desktop frame. Release the
            // generation gate before constructing OCR/fallback artifacts so a
            // deferred UIA frame can be shown without waiting for pixel work.
            if let Some(lease) = capture_lease.take() {
                lease.release()?;
            }
            if let Some(input) = system_input.take() {
                if let Some(factory) = bitmap_factory.as_ref() {
                    if let Err(error) =
                        stream_system_ocr_tiles(pixels, geometry, factory, &input, |index| {
                            if index == 0 && wechat_input.is_some() {
                                submit_wechat_full_frame(
                                    &mut wechat_input,
                                    pixels,
                                    geometry,
                                    bitmap_factory.as_ref(),
                                    bitmap_factory_error.as_deref(),
                                );
                            }
                            if !drain_early_ocr_events(
                                &rx,
                                &job.source,
                                &fallback_cancelled,
                                &mut ocr_had_valid_targets,
                                &mut early_events,
                                || context_is_current(shared, job.generation, &job.request),
                            ) {
                                context_changed_during_capture = true;
                                return Err("visual capture context changed".into());
                            }
                            Ok(())
                        })
                    {
                        let _ = input.send(SystemOcrInput::Failed(error));
                        let _ = input.send(SystemOcrInput::Done);
                    }
                } else {
                    let _ = input.send(SystemOcrInput::Failed(
                        bitmap_factory_error.clone().unwrap_or_else(|| {
                            "cannot create the COM apartment for system OCR tiles".into()
                        }),
                    ));
                    let _ = input.send(SystemOcrInput::Done);
                }
            }
            if wechat_input.is_some() {
                submit_wechat_full_frame(
                    &mut wechat_input,
                    pixels,
                    geometry,
                    bitmap_factory.as_ref(),
                    bitmap_factory_error.as_deref(),
                );
            }
            if !drain_early_ocr_events(
                &rx,
                &job.source,
                &fallback_cancelled,
                &mut ocr_had_valid_targets,
                &mut early_events,
                || context_is_current(shared, job.generation, &job.request),
            ) {
                context_changed_during_capture = true;
                return Err("visual capture context changed".into());
            }
            let fallback = (job.request.vision.detect_rectangles && !ocr_had_valid_targets)
                .then(|| fallback_input_from_bgra(pixels, geometry))
                .transpose()?;
            Ok(fallback)
        },
    );
    drop(prepared_capture);
    drop(gdi_ledger);
    if let Some(lease) = capture_lease.take()
        && let Err(error) = lease.release()
    {
        crate::app::logging::report_error("windows-overlay", error);
    }
    let fallback_input = match captured {
        Ok(artifact) => artifact,
        Err(_) if context_changed_during_capture => return UiScanStatus::ContextChanged,
        Err(error) => return UiScanStatus::Failed(error),
    };
    if !context_is_current(shared, job.generation, &job.request) {
        return UiScanStatus::ContextChanged;
    }

    drop(system_input);
    drop(wechat_input);

    let fallback_pending = if let Some(fallback_input) = fallback_input {
        let result_tx = tx.clone();
        let options = job.request.vision.clone();
        let provider_cancellation = cancellation.clone();
        let fallback_cancelled = Arc::clone(&fallback_cancelled);
        providers.spawn("keysteer-vision-fallback", move || {
            let _provider_ledger = crate::app::perf_probe::ResourceGuard::new(
                crate::app::perf_probe::ResourceKind::Provider,
            );
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
    let mut timed_out = false;
    let mut context_changed = false;
    let mut cleanup_errors = crate::app::errors::ErrorBundle::default();
    while pending_ocr != 0 || (fallback_pending && fallback.is_none()) {
        if !generation_is_current(shared, job.generation) {
            cancellation.cancel();
            context_changed = true;
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            cancellation.cancel();
            break;
        }
        let first = if let Some(event) = early_events.pop_front() {
            event
        } else {
            match rx.recv_timeout(remaining.min(Duration::from_millis(10))) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        let mut events = VecDeque::from([first]);
        events.append(&mut early_events);
        events.extend(rx.try_iter());
        let mut ocr_ready = Vec::new();
        let mut ocr_done = Vec::new();
        while let Some(event) = events.pop_front() {
            match event {
                ProviderEvent::Fallback(targets) => fallback = Some(targets),
                ProviderEvent::OcrBatch {
                    provider,
                    elapsed,
                    targets,
                } => ocr_ready.push((provider, elapsed, targets)),
                ProviderEvent::OcrDone {
                    provider,
                    elapsed,
                    result,
                } => ocr_done.push((provider, elapsed, result)),
            }
        }
        ocr_ready.sort_by(|a, b| compare_ready(a.2.len(), a.1, b.2.len(), b.1));
        for (provider, _elapsed, targets) in ocr_ready {
            if !context_is_current(shared, job.generation, &job.request) {
                cancellation.cancel();
                context_changed = true;
                break;
            }
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
                "{provider} OCR streamed {count} valid targets ({accepted} new)"
            );
        }
        for (provider, elapsed, result) in ocr_done {
            pending_ocr = pending_ocr.saturating_sub(1);
            match result {
                Ok(count) => {
                    crate::log_info!(
                        "windows-vision",
                        "{provider} OCR completed in {elapsed:?} with {count} valid targets"
                    );
                }
                Err(error) if error.is_control_flow() => {}
                Err(error @ VisionError::Unavailable(_)) => crate::report_warning!(
                    "windows-vision",
                    "{provider} OCR failed after {elapsed:?}: {error}"
                ),
                Err(error) => {
                    if matches!(error, VisionError::Cleanup(_)) {
                        cleanup_errors.push(
                            format!("{provider} OCR cleanup after {elapsed:?}"),
                            error.to_string(),
                        );
                    } else {
                        // Operational provider failures can be masked by the
                        // other OCR or the Rust fallback, so this coordinator
                        // is their final reporting boundary.
                        crate::report_error!(
                            "windows-vision",
                            "{provider} OCR failed after {elapsed:?}: {error}"
                        );
                    }
                }
            }
        }
    }

    if !context_changed
        && should_publish_fallback(ocr_had_valid_targets, job.request.vision.detect_rectangles)
        && let Some(targets) = fallback
    {
        if context_is_current(shared, job.generation, &job.request) {
            job.source.push(targets);
        } else {
            context_changed = true;
        }
    }
    cleanup_errors.record(
        "provider join",
        providers.join_all(Instant::now() + PROVIDER_STOP_TIMEOUT),
    );
    drop(bitmap_apartment);
    let cleanup_result = cleanup_errors.into_result();
    if context_changed || !context_is_current(shared, job.generation, &job.request) {
        if let Err(error) = cleanup_result {
            crate::app::logging::report_error("windows-vision", error);
        }
        UiScanStatus::ContextChanged
    } else if timed_out {
        if let Err(error) = cleanup_result {
            crate::app::logging::report_error("windows-vision", error);
        }
        UiScanStatus::TimedOut
    } else if let Err(error) = cleanup_result {
        UiScanStatus::Failed(error)
    } else {
        UiScanStatus::Success
    }
}

fn should_publish_fallback(ocr_had_valid_targets: bool, rectangles_enabled: bool) -> bool {
    rectangles_enabled && !ocr_had_valid_targets
}

fn wait_provider_image(
    receiver: mpsc::Receiver<WechatInput>,
    deadline: Instant,
    cancellation: &ScanCancellation,
) -> Result<WechatFullFrame, VisionError> {
    loop {
        if cancellation.is_cancelled() {
            return Err(VisionError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(VisionError::TimedOut);
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
            Ok(WechatInput::Frame(image)) => return Ok(image),
            Ok(WechatInput::Failed(error)) => return Err(VisionError::Operational(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(VisionError::Cancelled);
            }
        }
    }
}

fn recognize_system_provider(
    descriptor: SystemOcrDescriptor,
    receiver: mpsc::Receiver<SystemOcrInput>,
    completion_sender: mpsc::SyncSender<SystemOcrInput>,
    deadline: Instant,
    cancellation: &ScanCancellation,
    result_tx: &mpsc::SyncSender<ProviderEvent>,
    started: Instant,
) -> Result<usize, VisionError> {
    let _apartment = super::native::ComApartment::initialise().map_err(VisionError::Operational)?;
    let factory = super::native::SystemOcrFactory::load().map_err(VisionError::Unavailable)?;
    // Preserve the existing overlap between OCR cold start and the overlay
    // hide/capture barrier. The remaining engines are created as tiles arrive.
    let mut first_engine = Some(factory.create_engine().map_err(VisionError::Unavailable)?);
    let mut expected_tiles = None;
    let mut active: Vec<Option<ActiveSystemOcrTile>> = Vec::new();
    let mut pending = 0usize;
    let mut accepted = 0usize;
    let mut operational_failures = Vec::new();
    let mut cleanup_failures = Vec::new();
    loop {
        if cancellation.is_cancelled() {
            cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
            return if cleanup_failures.is_empty() {
                Err(VisionError::Cancelled)
            } else {
                Err(VisionError::Cleanup(cleanup_failures.join("; ")))
            };
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
            return if cleanup_failures.is_empty() {
                Err(VisionError::TimedOut)
            } else {
                Err(VisionError::Cleanup(cleanup_failures.join("; ")))
            };
        }
        let input = match receiver.recv_timeout(remaining) {
            Ok(input) => input,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
                return if cleanup_failures.is_empty() {
                    Err(VisionError::TimedOut)
                } else {
                    Err(VisionError::Cleanup(cleanup_failures.join("; ")))
                };
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                operational_failures.push("system OCR tile stream disconnected".into());
                break;
            }
        };
        match input {
            SystemOcrInput::Begin { tile_count } => {
                if expected_tiles.replace(tile_count).is_some() {
                    operational_failures.push("system OCR received a duplicate tile header".into());
                    continue;
                }
                active.reserve(tile_count);
            }
            SystemOcrInput::Tile(mut tile) => {
                if expected_tiles.is_none() {
                    operational_failures
                        .push("system OCR received a tile before its header".into());
                    if let Err(error) = tile.bitmap.close() {
                        cleanup_failures.push(error);
                    }
                    continue;
                }
                let index = active.len();
                if accepted >= MAX_OCR_TARGETS {
                    if let Err(error) = tile.bitmap.close() {
                        cleanup_failures.push(error);
                    }
                    active.push(None);
                    continue;
                }
                if tile.geometry.width > descriptor.maximum_dimension
                    || tile.geometry.height > descriptor.maximum_dimension
                {
                    operational_failures.push(format!(
                        "system OCR tile {}x{} exceeds the OCR limit {}",
                        tile.geometry.width, tile.geometry.height, descriptor.maximum_dimension
                    ));
                    if let Err(error) = tile.bitmap.close() {
                        cleanup_failures.push(error);
                    }
                    active.push(None);
                    continue;
                }
                let engine = match first_engine
                    .take()
                    .map_or_else(|| factory.create_engine(), Ok)
                {
                    Ok(engine) => engine,
                    Err(error) => {
                        operational_failures.push(error);
                        if let Err(error) = tile.bitmap.close() {
                            cleanup_failures.push(error);
                        }
                        active.push(None);
                        continue;
                    }
                };
                let operation = match tile.bitmap.bitmap().and_then(|bitmap| {
                    OcrOperationGuard::start_notified(
                        &engine,
                        bitmap,
                        index,
                        completion_sender.clone(),
                    )
                    .map_err(VisionError::Operational)
                }) {
                    Ok(operation) => operation,
                    Err(error) => {
                        operational_failures.push(error.to_string());
                        if let Err(error) = tile.bitmap.close() {
                            cleanup_failures.push(error);
                        }
                        active.push(None);
                        continue;
                    }
                };
                active.push(Some(ActiveSystemOcrTile {
                    _engine: engine,
                    operation,
                    tile,
                }));
                pending += 1;
            }
            SystemOcrInput::Completed { index, status } => {
                if let Err(error) = complete_system_tile(
                    (index, status),
                    &mut active,
                    &mut pending,
                    &mut accepted,
                    result_tx,
                    started,
                    &mut operational_failures,
                    &mut cleanup_failures,
                ) {
                    cancel_active_system_tiles(
                        &mut active,
                        &receiver,
                        deadline,
                        &mut cleanup_failures,
                    );
                    return if cleanup_failures.is_empty() {
                        Err(error)
                    } else {
                        Err(VisionError::Cleanup(cleanup_failures.join("; ")))
                    };
                }
            }
            SystemOcrInput::Failed(error) => operational_failures.push(error),
            SystemOcrInput::Done => break,
        }
    }
    drop(first_engine);
    if let Some(expected) = expected_tiles
        && active.len() != expected
    {
        operational_failures.push(format!(
            "system OCR expected {expected} tiles but received {}",
            active.len()
        ));
    }

    while pending != 0 && accepted < MAX_OCR_TARGETS {
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let completion = receiver.recv_timeout(remaining);
        let (index, status) = match completion {
            Ok(SystemOcrInput::Completed { index, status }) => (index, status),
            Ok(SystemOcrInput::Failed(error)) => {
                operational_failures.push(error);
                continue;
            }
            Ok(SystemOcrInput::Begin { .. } | SystemOcrInput::Tile(_) | SystemOcrInput::Done) => {
                operational_failures.push("system OCR received input after tile stream end".into());
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                operational_failures.push("system OCR completion channel disconnected".into());
                break;
            }
        };
        if let Err(error) = complete_system_tile(
            (index, status),
            &mut active,
            &mut pending,
            &mut accepted,
            result_tx,
            started,
            &mut operational_failures,
            &mut cleanup_failures,
        ) {
            cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
            return if cleanup_failures.is_empty() {
                Err(error)
            } else {
                Err(VisionError::Cleanup(cleanup_failures.join("; ")))
            };
        }
    }
    cancel_active_system_tiles(&mut active, &receiver, deadline, &mut cleanup_failures);
    if !cleanup_failures.is_empty() {
        Err(VisionError::Cleanup(cleanup_failures.join("; ")))
    } else if cancellation.is_cancelled() {
        Err(VisionError::Cancelled)
    } else if Instant::now() >= deadline {
        Err(VisionError::TimedOut)
    } else if !operational_failures.is_empty() {
        Err(VisionError::Operational(operational_failures.join("; ")))
    } else {
        Ok(accepted)
    }
}

struct ActiveSystemOcrTile {
    _engine: OcrEngine,
    operation: OcrOperationGuard,
    tile: SystemOcrTile,
}

#[allow(clippy::too_many_arguments)]
fn complete_system_tile(
    (index, status): (usize, AsyncStatus),
    active: &mut [Option<ActiveSystemOcrTile>],
    pending: &mut usize,
    accepted: &mut usize,
    result_tx: &mpsc::SyncSender<ProviderEvent>,
    started: Instant,
    operational_failures: &mut Vec<String>,
    cleanup_failures: &mut Vec<String>,
) -> Result<(), VisionError> {
    let Some(slot) = active.get_mut(index) else {
        operational_failures.push(format!(
            "system OCR returned an invalid tile completion index {index}"
        ));
        return Ok(());
    };
    let Some(mut completed) = slot.take() else {
        operational_failures.push(format!(
            "system OCR returned a duplicate tile completion index {index}"
        ));
        return Ok(());
    };
    *pending = pending.saturating_sub(1);
    match completed.operation.complete(status) {
        Ok(result) => match stream_system_targets_from_result(
            &result,
            completed.tile.geometry,
            completed.tile.core_bounds,
            MAX_OCR_TARGETS.saturating_sub(*accepted),
            result_tx,
            started,
        ) {
            Ok(count) => *accepted += count,
            Err(VisionError::Operational(error)) => operational_failures.push(error),
            Err(error) => return Err(error),
        },
        Err(error) => operational_failures.push(format!("system OCR tile {index}: {error}")),
    }
    if let Err(error) = completed.tile.bitmap.close() {
        cleanup_failures.push(error);
    }
    Ok(())
}

fn cancel_active_system_tiles(
    active: &mut [Option<ActiveSystemOcrTile>],
    completions: &mpsc::Receiver<SystemOcrInput>,
    deadline: Instant,
    cleanup_failures: &mut Vec<String>,
) {
    let mut remaining = 0usize;
    for tile in active.iter().flatten() {
        remaining += 1;
        if let Err(error) = tile.operation.request_cancel() {
            cleanup_failures.push(error);
        }
    }
    while remaining != 0 {
        let remaining_time = deadline.saturating_duration_since(Instant::now());
        if remaining_time.is_zero() {
            cleanup_failures.push(format!(
                "{remaining} system OCR operation(s) did not complete cancellation before the generation deadline"
            ));
            break;
        }
        let completion = completions
            .recv_timeout(remaining_time)
            .map_err(|error| error.to_string());
        let (index, status) = match completion {
            Ok(SystemOcrInput::Completed { index, status }) => (index, status),
            Ok(_) => continue,
            Err(error) => {
                cleanup_failures.push(format!(
                    "system OCR cancellation completion channel failed: {error}"
                ));
                break;
            }
        };
        if status == AsyncStatus::Started {
            cleanup_failures.push(format!(
                "system OCR cancellation callback {index} reported Started"
            ));
            continue;
        }
        let Some(slot) = active.get_mut(index) else {
            cleanup_failures.push(format!(
                "system OCR cancellation returned invalid tile index {index}"
            ));
            continue;
        };
        let Some(mut tile) = slot.take() else {
            continue;
        };
        remaining = remaining.saturating_sub(1);
        if let Err(error) = tile.operation.close_terminal() {
            cleanup_failures.push(error);
        }
        if let Err(error) = tile.tile.bitmap.close() {
            cleanup_failures.push(error);
        }
    }
}

fn recognize_wechat_provider(
    descriptor: WechatDescriptor,
    receiver: mpsc::Receiver<WechatInput>,
    deadline: Instant,
    minimum_confidence: f64,
    cancellation: &ScanCancellation,
    result_tx: &mpsc::SyncSender<ProviderEvent>,
    started: Instant,
) -> Result<usize, VisionError> {
    let _apartment = super::native::ComApartment::initialise().map_err(VisionError::Operational)?;
    let mut provider = WechatOcr::start(&descriptor, deadline, &|| cancellation.is_cancelled())
        .map_err(VisionError::Unavailable)?;
    let result = wait_provider_image(receiver, deadline, cancellation).and_then(|mut input| {
        let geometry = input.geometry;
        let bitmap = input.take_bitmap()?;
        provider
            .recognize(
                geometry,
                bitmap,
                deadline.saturating_duration_since(Instant::now()),
                minimum_confidence,
                || cancellation.is_cancelled(),
                |targets| {
                    send_ocr_batches(result_tx, "wechat", started, targets)
                        .map_err(|error| error.to_string())
                },
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
    OcrBatch {
        provider: &'static str,
        elapsed: Duration,
        targets: Vec<UiTarget>,
    },
    OcrDone {
        provider: &'static str,
        elapsed: Duration,
        result: Result<usize, VisionError>,
    },
    Fallback(Vec<UiTarget>),
}

fn drain_early_ocr_events(
    receiver: &mpsc::Receiver<ProviderEvent>,
    source: &ScanSource,
    fallback_cancelled: &AtomicBool,
    ocr_had_valid_targets: &mut bool,
    deferred: &mut VecDeque<ProviderEvent>,
    mut context_is_current: impl FnMut() -> bool,
) -> bool {
    let mut ready = Vec::new();
    for event in receiver.try_iter() {
        match event {
            ProviderEvent::OcrBatch {
                provider,
                elapsed,
                targets,
            } => ready.push((provider, elapsed, targets)),
            event => deferred.push_back(event),
        }
    }
    ready.sort_by(|a, b| compare_ready(a.2.len(), a.1, b.2.len(), b.1));
    for (provider, _elapsed, targets) in ready {
        if !context_is_current() {
            return false;
        }
        let count = targets.len();
        if count != 0 {
            *ocr_had_valid_targets = true;
            fallback_cancelled.store(true, Ordering::Release);
        }
        let accepted = source.push(targets);
        if accepted != 0 {
            crate::app::perf_probe::mark("vision_targets_accepted");
        }
        crate::log_info!(
            "windows-vision",
            "{provider} OCR streamed {count} valid targets ({accepted} new)"
        );
    }
    true
}

fn send_ocr_batches(
    sender: &mpsc::SyncSender<ProviderEvent>,
    provider: &'static str,
    started: Instant,
    targets: Vec<UiTarget>,
) -> Result<usize, VisionError> {
    let count = targets.len();
    if count <= PROVIDER_BATCH_SIZE {
        if count != 0 {
            sender
                .send(ProviderEvent::OcrBatch {
                    provider,
                    elapsed: started.elapsed(),
                    targets,
                })
                .map_err(|_| VisionError::Cancelled)?;
        }
        return Ok(count);
    }
    let mut batch = Vec::with_capacity(PROVIDER_BATCH_SIZE);
    for target in targets {
        batch.push(target);
        if batch.len() == PROVIDER_BATCH_SIZE {
            sender
                .send(ProviderEvent::OcrBatch {
                    provider,
                    elapsed: started.elapsed(),
                    targets: std::mem::replace(&mut batch, Vec::with_capacity(PROVIDER_BATCH_SIZE)),
                })
                .map_err(|_| VisionError::Cancelled)?;
        }
    }
    if !batch.is_empty() {
        sender
            .send(ProviderEvent::OcrBatch {
                provider,
                elapsed: started.elapsed(),
                targets: batch,
            })
            .map_err(|_| VisionError::Cancelled)?;
    }
    Ok(count)
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

fn system_ocr_grid_for_parallelism(width: u32, height: u32, parallelism: usize) -> u32 {
    let cpu_grid = parallelism.max(1).isqrt().saturating_add(1);
    let size_grid = (width / MIN_SYSTEM_OCR_TILE_SIDE)
        .min(height / MIN_SYSTEM_OCR_TILE_SIDE)
        .max(1) as usize;
    cpu_grid.min(size_grid).max(1) as u32
}

fn system_ocr_grid(width: u32, height: u32) -> u32 {
    let parallelism = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(8);
    system_ocr_grid_for_parallelism(width, height, parallelism)
}

fn system_ocr_tile_count(geometry: CaptureGeometry) -> usize {
    let grid = system_ocr_grid(geometry.width, geometry.height) as usize;
    grid.saturating_mul(grid)
}

fn scaled_partition(value: u32, index: u32, divisions: u32) -> u32 {
    ((u64::from(value) * u64::from(index)) / u64::from(divisions)) as u32
}

fn capture_pixel_rect(geometry: CaptureGeometry, x: u32, y: u32, width: u32, height: u32) -> Rect {
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

fn wechat_full_frame_from_bgra(
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: &super::native::SoftwareBitmapFactory,
) -> Result<WechatFullFrame, String> {
    let bitmap = factory.bgra(pixels, geometry.width, geometry.height)?;
    crate::app::perf_probe::mark("ocr_bitmap_ready");
    Ok(WechatFullFrame {
        geometry,
        bitmap: Some(bitmap),
        _ledger: crate::app::perf_probe::ResourceGuard::new(
            crate::app::perf_probe::ResourceKind::Bitmap,
        ),
    })
}

fn submit_wechat_full_frame(
    input: &mut Option<mpsc::SyncSender<WechatInput>>,
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: Option<&super::native::SoftwareBitmapFactory>,
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

fn system_ocr_tile_from_bgra(
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: &super::native::SoftwareBitmapFactory,
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
            _ledger: crate::app::perf_probe::ResourceGuard::new(
                crate::app::perf_probe::ResourceKind::Bitmap,
            ),
        },
    })
}

fn stream_system_ocr_tiles(
    pixels: &[u8],
    geometry: CaptureGeometry,
    factory: &super::native::SoftwareBitmapFactory,
    sender: &mpsc::SyncSender<SystemOcrInput>,
    mut after_tile: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), String> {
    let expected = geometry.width as usize * geometry.height as usize * 4;
    if pixels.len() != expected {
        return Err("captured BGRA byte length does not match system OCR geometry".into());
    }
    let grid = system_ocr_grid(geometry.width, geometry.height);
    let tile_count = (grid as usize)
        .checked_mul(grid as usize)
        .ok_or_else(|| "system OCR tile count overflowed".to_string())?;
    if sender.send(SystemOcrInput::Begin { tile_count }).is_err() {
        return Ok(());
    }
    let mut index = 0usize;
    for row in 0..grid {
        for column in 0..grid {
            let core_x = scaled_partition(geometry.width, column, grid);
            let core_right = scaled_partition(geometry.width, column + 1, grid);
            let core_y = scaled_partition(geometry.height, row, grid);
            let core_bottom = scaled_partition(geometry.height, row + 1, grid);
            let tile = system_ocr_tile_from_bgra(
                pixels,
                geometry,
                factory,
                core_x,
                core_y,
                core_right,
                core_bottom,
            )?;
            if sender.send(SystemOcrInput::Tile(tile)).is_err() {
                return Ok(());
            }
            after_tile(index)?;
            index += 1;
        }
    }
    let _ = sender.send(SystemOcrInput::Done);
    Ok(())
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
    let source_x_offsets = nearest_offsets(source_width, width);
    let source_y_offsets = nearest_offsets(source_height, height);
    let mut gray = vec![0; width * height];
    for (y, source_y) in source_y_offsets.into_iter().enumerate() {
        for (x, source_x) in source_x_offsets.iter().copied().enumerate() {
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

/// Return the exact `floor(output * source / destination)` mapping without a
/// division in the per-pixel conversion loop.
fn nearest_offsets(source: usize, destination: usize) -> Vec<usize> {
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

fn stream_system_targets_from_result(
    result: &OcrResult,
    geometry: CaptureGeometry,
    core_bounds: Rect,
    maximum: usize,
    sender: &mpsc::SyncSender<ProviderEvent>,
    started: Instant,
) -> Result<usize, VisionError> {
    let lines = result
        .Lines()
        .map_err(|error| VisionError::Operational(format!("cannot read OCR lines: {error}")))?;
    let count = lines.Size().map_err(|error| {
        VisionError::Operational(format!("cannot read OCR line count: {error}"))
    })?;
    let mut accepted = 0usize;
    let mut batch = Vec::with_capacity(PROVIDER_BATCH_SIZE.min(maximum));
    for index in 0..count {
        if accepted == maximum {
            break;
        }
        let line = lines.GetAt(index).map_err(|error| {
            VisionError::Operational(format!("cannot read OCR line {index}: {error}"))
        })?;
        let words = line
            .Words()
            .map_err(|error| VisionError::Operational(format!("cannot read OCR words: {error}")))?;
        let word_count = words.Size().map_err(|error| {
            VisionError::Operational(format!("cannot read OCR word count: {error}"))
        })?;
        let mut union: Option<Rect> = None;
        for word_index in 0..word_count {
            let native = words
                .GetAt(word_index)
                .and_then(|word| word.BoundingRect())
                .map_err(|error| {
                    VisionError::Operational(format!("cannot read OCR word bounds: {error}"))
                })?;
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
        let Some(rect) = union.filter(|rect| {
            valid_target_rect(*rect, geometry.desktop_bounds)
                && core_bounds.contains(&rect.center())
        }) else {
            continue;
        };
        // The overlap/core ownership test is intentionally before Text():
        // seam duplicates never allocate a Rust String or UiTarget.
        let mut text = line
            .Text()
            .map_err(|error| {
                VisionError::Operational(format!("cannot read OCR line text: {error}"))
            })?
            .to_string();
        trim_string_in_place(&mut text);
        if text.is_empty() {
            continue;
        }
        batch.push(UiTarget {
            rect,
            name: text,
            role: "static_text".into(),
            native_role: Some("vision:windows-ocr".into()),
        });
        accepted += 1;
        if batch.len() == PROVIDER_BATCH_SIZE {
            sender
                .send(ProviderEvent::OcrBatch {
                    provider: "system",
                    elapsed: started.elapsed(),
                    targets: std::mem::replace(
                        &mut batch,
                        Vec::with_capacity(PROVIDER_BATCH_SIZE.min(maximum - accepted)),
                    ),
                })
                .map_err(|_| VisionError::Cancelled)?;
        }
    }
    if !batch.is_empty() {
        sender
            .send(ProviderEvent::OcrBatch {
                provider: "system",
                elapsed: started.elapsed(),
                targets: batch,
            })
            .map_err(|_| VisionError::Cancelled)?;
    }
    Ok(accepted)
}

#[must_use = "system OCR operations must be cancelled or closed explicitly"]
struct OcrOperationGuard {
    operation: IAsyncOperation<OcrResult>,
    closed: bool,
}

impl OcrOperationGuard {
    fn start_notified(
        engine: &OcrEngine,
        bitmap: &windows::Graphics::Imaging::SoftwareBitmap,
        index: usize,
        notifier: mpsc::SyncSender<SystemOcrInput>,
    ) -> Result<Self, String> {
        let operation = engine
            .RecognizeAsync(bitmap)
            .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}"))?;
        let mut guard = Self {
            operation,
            closed: false,
        };
        if let Err(error) = guard
            .operation
            .SetCompleted(&AsyncOperationCompletedHandler::new(move |_, status| {
                let _ = notifier.try_send(SystemOcrInput::Completed { index, status });
                Ok(())
            }))
        {
            return Err(guard.registration_error(format!(
                "cannot register tiled system OCR completion: {error}"
            )));
        }
        Ok(guard)
    }

    fn complete(mut self, status: AsyncStatus) -> Result<OcrResult, String> {
        let result = match status {
            AsyncStatus::Completed => self
                .operation
                .GetResults()
                .map_err(|error| format!("OcrEngine::RecognizeAsync failed: {error}")),
            AsyncStatus::Canceled => Err("system OCR cancelled".into()),
            AsyncStatus::Error => self.operation.ErrorCode().map_or_else(
                |error| Err(format!("cannot read system OCR error: {error}")),
                |error| Err(format!("OcrEngine::RecognizeAsync failed: {error}")),
            ),
            AsyncStatus::Started => Err("system OCR completed callback reported Started".into()),
            _ => Err("system OCR returned an unknown asynchronous status".into()),
        };
        self.finish(result)
    }

    fn request_cancel(&self) -> Result<(), String> {
        self.operation
            .Cancel()
            .map_err(|error| format!("cannot cancel system OCR operation: {error}"))
    }

    fn close_terminal(mut self) -> Result<(), String> {
        self.finish(Ok(()))
    }

    fn finish<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        let mut cleanup = Vec::new();
        if let Err(error) = self.operation.Close() {
            cleanup.push(format!("cannot close system OCR operation: {error}"));
        }
        self.closed = true;
        combine_result_and_cleanup(result, cleanup)
    }

    fn registration_error(&mut self, error: String) -> String {
        let mut cleanup = self.request_cancel().err().into_iter().collect::<Vec<_>>();
        match self.operation.Status() {
            Ok(AsyncStatus::Completed | AsyncStatus::Canceled | AsyncStatus::Error) => {
                if let Err(error) = self.operation.Close() {
                    cleanup.push(format!("cannot close system OCR operation: {error}"));
                }
                self.closed = true;
            }
            Ok(_) => cleanup.push(
                "system OCR operation did not reach a terminal state after handler failure".into(),
            ),
            Err(status_error) => cleanup.push(format!(
                "cannot query system OCR operation after handler failure: {status_error}"
            )),
        }
        match combine_result_and_cleanup::<()>(Err(error), cleanup) {
            Ok(()) => "system OCR cleanup lost its primary error".into(),
            Err(error) => error,
        }
    }
}

impl Drop for OcrOperationGuard {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let mut failures = self.request_cancel().err().into_iter().collect::<Vec<_>>();
        match self.operation.Status() {
            Ok(AsyncStatus::Completed | AsyncStatus::Canceled | AsyncStatus::Error) => {
                if let Err(error) = self.operation.Close() {
                    failures.push(format!("cannot close system OCR operation: {error}"));
                }
            }
            Ok(_) => failures.push("system OCR operation left its explicit owner".into()),
            Err(error) => failures.push(format!(
                "cannot query system OCR operation during drop: {error}"
            )),
        }
        for error in failures {
            crate::app::logging::report_error("windows-vision", error);
        }
    }
}

fn combine_result_and_cleanup<T>(
    result: Result<T, String>,
    cleanup: Vec<String>,
) -> Result<T, String> {
    match (result, cleanup.is_empty()) {
        (Ok(value), true) => Ok(value),
        (Ok(_), false) => Err(cleanup.join("; ")),
        (Err(error), true) => Err(error),
        (Err(error), false) => Err(format!("{error}; cleanup: {}", cleanup.join("; "))),
    }
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
        previous_runs,
        current_runs,
        components,
        next_components,
        root_remap,
        active_roots,
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
    // `edge` is dead after dilation; reuse it for the closed image.
    edge.fill(false);
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        for x in 1..width - 1 {
            let i = y * width + x;
            edge[i] = (-1isize..=1).all(|dy| {
                (-1isize..=1).all(|dx| {
                    dilated[((y as isize + dy) as usize) * width + (x as isize + dx) as usize]
                })
            });
        }
    }
    let candidate_limit = options.rectangle_max_candidates.min(2_000);
    let mut candidates = BinaryHeap::with_capacity(candidate_limit);
    let configured_minimum =
        (options.rectangle_min_size * width.min(height) as f64).ceil() as usize;
    let minimum_side = configured_minimum.max(6);
    previous_runs.clear();
    components.clear();
    for y in 1..height - 1 {
        if cancelled() {
            return Vec::new();
        }
        current_runs.clear();
        let mut x = 1usize;
        while x < width - 1 {
            if !edge[y * width + x] {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < width - 1 && edge[y * width + x + 1] {
                x += 1;
            }
            current_runs.push(ComponentRun {
                start,
                end: x,
                label: usize::MAX,
            });
            x += 1;
        }

        let mut previous_start = 0usize;
        for run in current_runs.iter_mut() {
            while previous_start < previous_runs.len()
                && previous_runs[previous_start].end.saturating_add(1) < run.start
            {
                previous_start += 1;
            }
            let mut previous = previous_start;
            let mut label = None;
            while previous < previous_runs.len()
                && previous_runs[previous].start <= run.end.saturating_add(1)
            {
                let previous_label = previous_runs[previous].label;
                label = Some(match label {
                    Some(current) => union_components(components, current, previous_label),
                    None => component_root(components, previous_label),
                });
                previous += 1;
            }
            let label = label.unwrap_or_else(|| {
                let label = components.len();
                components.push(ActiveComponent::new(label, run.start, run.end, y));
                label
            });
            let root = component_root(components, label);
            components[root].stats.add_run(run.start, run.end, y);
            run.label = root;
        }

        active_roots.clear();
        active_roots.resize(components.len(), false);
        for run in current_runs.iter_mut() {
            let root = component_root(components, run.label);
            run.label = root;
            active_roots[root] = true;
        }
        for index in 0..components.len() {
            let root = component_root(components, index);
            if root == index && !active_roots[root] {
                consider_region(
                    components[root].stats,
                    image,
                    options,
                    minimum_side,
                    candidate_limit,
                    &mut candidates,
                );
            }
        }

        root_remap.clear();
        root_remap.resize(components.len(), usize::MAX);
        next_components.clear();
        next_components.reserve(current_runs.len());
        for run in current_runs.iter_mut() {
            let root = run.label;
            let mapped = if root_remap[root] == usize::MAX {
                let mapped = next_components.len();
                root_remap[root] = mapped;
                next_components.push(ActiveComponent {
                    parent: mapped,
                    stats: components[root].stats,
                });
                mapped
            } else {
                root_remap[root]
            };
            run.label = mapped;
        }
        std::mem::swap(components, next_components);
        std::mem::swap(previous_runs, current_runs);
    }
    for component in components.iter() {
        consider_region(
            component.stats,
            image,
            options,
            minimum_side,
            candidate_limit,
            &mut candidates,
        );
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
    previous_runs: Vec<ComponentRun>,
    current_runs: Vec<ComponentRun>,
    components: Vec<ActiveComponent>,
    next_components: Vec<ActiveComponent>,
    root_remap: Vec<usize>,
    active_roots: Vec<bool>,
}

#[derive(Clone, Copy)]
struct ComponentRun {
    start: usize,
    end: usize,
    label: usize,
}

#[derive(Clone, Copy)]
struct ActiveComponent {
    parent: usize,
    stats: ComponentStats,
}

impl ActiveComponent {
    fn new(parent: usize, start: usize, end: usize, y: usize) -> Self {
        Self {
            parent,
            // The run is added after any unions so merged and new components
            // share the same update path.
            stats: ComponentStats {
                min_x: start,
                max_x: end,
                min_y: y,
                max_y: y,
                pixels: 0,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct ComponentStats {
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
    pixels: usize,
}

impl ComponentStats {
    fn add_run(&mut self, start: usize, end: usize, y: usize) {
        self.min_x = self.min_x.min(start);
        self.max_x = self.max_x.max(end);
        self.min_y = self.min_y.min(y);
        self.max_y = self.max_y.max(y);
        self.pixels += end - start + 1;
    }

    fn merge(&mut self, other: Self) {
        self.min_x = self.min_x.min(other.min_x);
        self.max_x = self.max_x.max(other.max_x);
        self.min_y = self.min_y.min(other.min_y);
        self.max_y = self.max_y.max(other.max_y);
        self.pixels += other.pixels;
    }
}

fn component_root(components: &mut [ActiveComponent], mut index: usize) -> usize {
    let mut root = index;
    while components[root].parent != root {
        root = components[root].parent;
    }
    while components[index].parent != index {
        let parent = components[index].parent;
        components[index].parent = root;
        index = parent;
    }
    root
}

fn union_components(components: &mut [ActiveComponent], first: usize, second: usize) -> usize {
    let first = component_root(components, first);
    let second = component_root(components, second);
    if first == second {
        return first;
    }
    let (root, merged) = if first < second {
        (first, second)
    } else {
        (second, first)
    };
    let merged_stats = components[merged].stats;
    components[merged].parent = root;
    components[root].stats.merge(merged_stats);
    root
}

fn consider_region(
    stats: ComponentStats,
    image: &FallbackInput,
    options: &crate::api::VisionOptions,
    minimum_side: usize,
    candidate_limit: usize,
    candidates: &mut BinaryHeap<Reverse<RegionCandidate>>,
) {
    let box_width = stats.max_x - stats.min_x + 1;
    let box_height = stats.max_y - stats.min_y + 1;
    let aspect = box_width as f64 / box_height as f64;
    let perimeter = (2 * (box_width + box_height)).max(1);
    if box_width < minimum_side
        || box_height < minimum_side
        || stats.pixels < 16
        || !(options.rectangle_min_aspect..=options.rectangle_max_aspect).contains(&aspect)
    {
        return;
    }
    let confidence = (stats.pixels as f64 / perimeter as f64).min(1.0);
    if confidence < options.minimum_confidence {
        return;
    }
    let rect = Rect::new(
        image.desktop_bounds.x
            + stats.min_x as f64 * image.desktop_bounds.width / image.width as f64,
        image.desktop_bounds.y
            + stats.min_y as f64 * image.desktop_bounds.height / image.height as f64,
        box_width as f64 * image.desktop_bounds.width / image.width as f64,
        box_height as f64 * image.desktop_bounds.height / image.height as f64,
    );
    if !valid_target_rect(rect, image.desktop_bounds) {
        return;
    }
    let Some((role, native_role)) = classify_region(rect, confidence, options) else {
        return;
    };
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
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn ocr_execution_kind_uses_only_discovered_providers() {
        assert_eq!(
            ocr_execution_kind(false, false, true),
            OcrExecutionKind::None
        );
        assert_eq!(
            ocr_execution_kind(true, false, true),
            OcrExecutionKind::SystemOnly
        );
        assert_eq!(
            ocr_execution_kind(false, true, true),
            OcrExecutionKind::WechatOnly
        );
        assert_eq!(ocr_execution_kind(true, true, true), OcrExecutionKind::Dual);
        assert_eq!(
            ocr_execution_kind(true, true, false),
            OcrExecutionKind::None
        );
    }

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
    fn standard_uhd_capture_keeps_native_bitblt_dimensions() {
        let geometry = capture_geometry(Rect::new(0.0, 0.0, 3_840.0, 2_160.0)).unwrap();
        assert_eq!(geometry.width, 3_840);
        assert_eq!(geometry.height, 2_160);
        assert_eq!(geometry.scale, 1.0);
    }

    #[test]
    fn system_ocr_grid_uses_the_next_cpu_square_and_a_64px_size_floor() {
        assert_eq!(system_ocr_grid_for_parallelism(3_840, 2_160, 16), 5);
        assert_eq!(system_ocr_grid_for_parallelism(7_680, 4_320, 64), 9);
        assert_eq!(system_ocr_grid_for_parallelism(320, 200, 16), 3);
        assert_eq!(system_ocr_grid_for_parallelism(200, 100, 16), 1);
    }

    #[test]
    fn system_ocr_core_tiles_cover_pixels_once_and_never_drop_below_64px() {
        let width = 3_841;
        let height = 2_161;
        let grid = system_ocr_grid_for_parallelism(width, height, 16);
        let mut previous_bottom = 0;
        for row in 0..grid {
            let mut previous_right = 0;
            for column in 0..grid {
                let left = scaled_partition(width, column, grid);
                let right = scaled_partition(width, column + 1, grid);
                let top = scaled_partition(height, row, grid);
                let bottom = scaled_partition(height, row + 1, grid);
                assert_eq!(left, previous_right);
                assert_eq!(top, previous_bottom);
                assert!(right - left >= MIN_SYSTEM_OCR_TILE_SIDE);
                assert!(bottom - top >= MIN_SYSTEM_OCR_TILE_SIDE);
                previous_right = right;
            }
            assert_eq!(previous_right, width);
            previous_bottom = scaled_partition(height, row + 1, grid);
        }
        assert_eq!(previous_bottom, height);
    }

    #[test]
    fn overlapping_system_ocr_tiles_map_a_seam_target_to_one_desktop_owner() {
        let parent = CaptureGeometry {
            width: 1_000,
            height: 500,
            desktop_bounds: Rect::new(-1_000.0, -200.0, 2_000.0, 1_000.0),
            scale: 0.5,
        };
        let seam = 500;
        let left_bitmap_right = seam + SYSTEM_OCR_TILE_OVERLAP;
        let right_bitmap_x = seam - SYSTEM_OCR_TILE_OVERLAP;
        let left_geometry = CaptureGeometry {
            width: left_bitmap_right,
            height: parent.height,
            desktop_bounds: capture_pixel_rect(parent, 0, 0, left_bitmap_right, parent.height),
            scale: parent.scale,
        };
        let right_geometry = CaptureGeometry {
            width: parent.width - right_bitmap_x,
            height: parent.height,
            desktop_bounds: capture_pixel_rect(
                parent,
                right_bitmap_x,
                0,
                parent.width - right_bitmap_x,
                parent.height,
            ),
            scale: parent.scale,
        };
        let global_pixel_rect = Rect::new(490.0, 120.0, 20.0, 12.0);
        let from_left = image_to_desktop(left_geometry, global_pixel_rect);
        let from_right = image_to_desktop(
            right_geometry,
            Rect::new(
                global_pixel_rect.x - f64::from(right_bitmap_x),
                global_pixel_rect.y,
                global_pixel_rect.width,
                global_pixel_rect.height,
            ),
        );
        assert_eq!(from_left, from_right);

        let left_core = capture_pixel_rect(parent, 0, 0, seam, parent.height);
        let right_core = capture_pixel_rect(parent, seam, 0, parent.width - seam, parent.height);
        let center = from_left.center();
        assert!(!left_core.contains(&center));
        assert!(right_core.contains(&center));
    }

    #[test]
    fn division_free_nearest_offsets_match_reference_mapping() {
        for (source, destination) in [
            (3usize, 2usize),
            (17, 7),
            (100, 100),
            (3_840, 2_560),
            (2_160, 1_440),
        ] {
            assert_eq!(
                nearest_offsets(source, destination),
                (0..destination)
                    .map(|output| output * source / destination)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn provider_targets_move_in_bounded_batches() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let targets = (0..49)
            .map(|index| UiTarget {
                rect: Rect::new(index as f64, 0.0, 4.0, 4.0),
                name: index.to_string(),
                role: "static_text".into(),
                native_role: None,
            })
            .collect();
        assert_eq!(
            send_ocr_batches(&sender, "test", Instant::now(), targets)
                .expect("test receiver is live"),
            49
        );
        drop(sender);
        let sizes = receiver
            .into_iter()
            .map(|event| match event {
                ProviderEvent::OcrBatch { targets, .. } => targets.len(),
                _ => 0,
            })
            .collect::<Vec<_>>();
        assert_eq!(sizes, [24, 24, 1]);
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

    #[derive(Clone, Copy)]
    struct OcrTileProbe {
        bitmap_x: u32,
        bitmap_y: u32,
        core_x: u32,
        core_y: u32,
        core_right: u32,
        core_bottom: u32,
    }

    struct OcrTileProbeResult {
        elapsed: Duration,
        raw_words: usize,
        owned_words: usize,
    }

    fn prepare_tile_probe(
        pixels: &[u8],
        width: u32,
        height: u32,
        grid: u32,
        overlap: u32,
    ) -> Result<Vec<(windows::Graphics::Imaging::SoftwareBitmap, OcrTileProbe)>, String> {
        let mut tiles = Vec::with_capacity((grid * grid) as usize);
        let factory = super::super::native::SoftwareBitmapFactory::load()?;
        for row in 0..grid {
            for column in 0..grid {
                let core_x = column * width / grid;
                let core_right = (column + 1) * width / grid;
                let core_y = row * height / grid;
                let core_bottom = (row + 1) * height / grid;
                let bitmap_x = core_x.saturating_sub(overlap);
                let bitmap_y = core_y.saturating_sub(overlap);
                let bitmap_right = core_right.saturating_add(overlap).min(width);
                let bitmap_bottom = core_bottom.saturating_add(overlap).min(height);
                let tile_width = bitmap_right - bitmap_x;
                let tile_height = bitmap_bottom - bitmap_y;
                let bitmap = factory.bgra_region(
                    pixels,
                    width,
                    height,
                    bitmap_x,
                    bitmap_y,
                    tile_width,
                    tile_height,
                )?;
                tiles.push((
                    bitmap,
                    OcrTileProbe {
                        bitmap_x,
                        bitmap_y,
                        core_x,
                        core_y,
                        core_right,
                        core_bottom,
                    },
                ));
            }
        }
        Ok(tiles)
    }

    fn run_tile_probe(
        tiles: &[(windows::Graphics::Imaging::SoftwareBitmap, OcrTileProbe)],
    ) -> Result<Vec<OcrTileProbeResult>, String> {
        let started = Instant::now();
        std::thread::scope(|scope| {
            let workers = tiles
                .iter()
                .map(|(bitmap, tile)| {
                    let bitmap = bitmap.clone();
                    scope.spawn(move || {
                        let _apartment = super::super::native::ComApartment::initialise()?;
                        let engine = super::super::native::create_system_ocr_engine()?;
                        let result = engine
                            .RecognizeAsync(&bitmap)
                            .and_then(|operation| operation.join())
                            .map_err(|error| format!("tiled OcrEngine failed: {error}"))?;
                        let mut raw_words = 0usize;
                        let mut owned_words = 0usize;
                        let lines = result
                            .Lines()
                            .map_err(|error| format!("cannot enumerate OCR lines: {error}"))?;
                        for line in &lines {
                            let words = line
                                .Words()
                                .map_err(|error| format!("cannot enumerate OCR words: {error}"))?;
                            for word in &words {
                                raw_words += 1;
                                let rect = word.BoundingRect().map_err(|error| {
                                    format!("cannot read OCR word bounds: {error}")
                                })?;
                                let center_x = tile.bitmap_x as f32 + rect.X + rect.Width * 0.5;
                                let center_y = tile.bitmap_y as f32 + rect.Y + rect.Height * 0.5;
                                if center_x >= tile.core_x as f32
                                    && center_x < tile.core_right as f32
                                    && center_y >= tile.core_y as f32
                                    && center_y < tile.core_bottom as f32
                                {
                                    owned_words += 1;
                                }
                            }
                        }
                        Ok(OcrTileProbeResult {
                            elapsed: started.elapsed(),
                            raw_words,
                            owned_words,
                        })
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .map_err(|_| "tiled OCR worker panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })
    }

    fn tile_probe_percentile(values: &mut [Duration], percentile: usize) -> Duration {
        values.sort_unstable();
        values[(values.len() - 1) * percentile / 100]
    }

    #[test]
    #[ignore = "manual Windows OCR tiling benchmark; set KEYSTEER_OCR_TILE_IMAGE if needed"]
    fn live_system_ocr_tiling_probe() -> Result<(), String> {
        use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat};
        use windows::Storage::Streams::{
            Buffer, DataReader, DataWriter, InMemoryRandomAccessStream,
        };

        const OVERLAP: u32 = 64;
        const WARMUPS: usize = 2;
        const SAMPLES: usize = 10;

        let _apartment = super::super::native::ComApartment::initialise()?;
        let image_path = std::env::var_os("KEYSTEER_OCR_TILE_IMAGE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("browser.jpg"));
        let encoded = std::fs::read(&image_path)
            .map_err(|error| format!("cannot read {}: {error}", image_path.display()))?;
        let stream = InMemoryRandomAccessStream::new()
            .map_err(|error| format!("cannot create image stream: {error}"))?;
        let writer = DataWriter::CreateDataWriter(&stream)
            .map_err(|error| format!("cannot create image writer: {error}"))?;
        writer
            .WriteBytes(&encoded)
            .map_err(|error| format!("cannot stage image bytes: {error}"))?;
        writer
            .StoreAsync()
            .and_then(|operation| operation.join())
            .map_err(|error| format!("cannot store image bytes: {error}"))?;
        writer
            .DetachStream()
            .map_err(|error| format!("cannot detach image stream: {error}"))?;
        writer
            .Close()
            .map_err(|error| format!("cannot close image writer: {error}"))?;
        stream
            .Seek(0)
            .map_err(|error| format!("cannot rewind image stream: {error}"))?;
        let decoder = BitmapDecoder::CreateAsync(&stream)
            .and_then(|operation| operation.join())
            .map_err(|error| format!("cannot decode {}: {error}", image_path.display()))?;
        let width = decoder
            .PixelWidth()
            .map_err(|error| format!("cannot read image width: {error}"))?;
        let height = decoder
            .PixelHeight()
            .map_err(|error| format!("cannot read image height: {error}"))?;
        let full_bitmap = decoder
            .GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Ignore)
            .and_then(|operation| operation.join())
            .map_err(|error| format!("cannot convert probe image to BGRA: {error}"))?;
        let byte_length = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "probe image byte length overflowed".to_string())?;
        let buffer = Buffer::Create(byte_length)
            .map_err(|error| format!("cannot create probe pixel buffer: {error}"))?;
        full_bitmap
            .CopyToBuffer(&buffer)
            .map_err(|error| format!("cannot copy probe bitmap pixels: {error}"))?;
        let reader = DataReader::FromBuffer(&buffer)
            .map_err(|error| format!("cannot create probe pixel reader: {error}"))?;
        let mut pixels = vec![0; byte_length as usize];
        reader
            .ReadBytes(&mut pixels)
            .map_err(|error| format!("cannot read probe bitmap pixels: {error}"))?;
        reader
            .Close()
            .map_err(|error| format!("cannot close probe pixel reader: {error}"))?;
        full_bitmap
            .Close()
            .map_err(|error| format!("cannot close decoded probe bitmap: {error}"))?;
        println!(
            "ocr_tile_probe image={} dimensions={}x{} overlap={}px samples={}",
            image_path.display(),
            width,
            height,
            OVERLAP,
            SAMPLES
        );

        for grid in 1..=7 {
            let preparation_started = Instant::now();
            let tiles = prepare_tile_probe(&pixels, width, height, grid, OVERLAP)?;
            let preparation = preparation_started.elapsed();
            for _ in 0..WARMUPS {
                run_tile_probe(&tiles)?;
            }
            let mut first_samples = Vec::with_capacity(SAMPLES);
            let mut total_samples = Vec::with_capacity(SAMPLES);
            let mut raw_words = 0usize;
            let mut owned_words = 0usize;
            for _ in 0..SAMPLES {
                let results = run_tile_probe(&tiles)?;
                first_samples.push(
                    results
                        .iter()
                        .map(|result| result.elapsed)
                        .min()
                        .ok_or_else(|| "tile probe produced no result".to_string())?,
                );
                total_samples.push(
                    results
                        .iter()
                        .map(|result| result.elapsed)
                        .max()
                        .ok_or_else(|| "tile probe produced no result".to_string())?,
                );
                raw_words += results.iter().map(|result| result.raw_words).sum::<usize>();
                owned_words += results
                    .iter()
                    .map(|result| result.owned_words)
                    .sum::<usize>();
            }
            let first_p50 = tile_probe_percentile(&mut first_samples, 50);
            let first_p95 = tile_probe_percentile(&mut first_samples, 95);
            let total_p50 = tile_probe_percentile(&mut total_samples, 50);
            let total_p95 = tile_probe_percentile(&mut total_samples, 95);
            println!(
                "ocr_tile_probe grid={}x{} tiles={} prepare_ms={:.3} first_p50_ms={:.3} first_p95_ms={:.3} total_p50_ms={:.3} total_p95_ms={:.3} raw_words_avg={} owned_words_avg={}",
                grid,
                grid,
                tiles.len(),
                preparation.as_secs_f64() * 1_000.0,
                first_p50.as_secs_f64() * 1_000.0,
                first_p95.as_secs_f64() * 1_000.0,
                total_p50.as_secs_f64() * 1_000.0,
                total_p95.as_secs_f64() * 1_000.0,
                raw_words / SAMPLES,
                owned_words / SAMPLES,
            );
            for (bitmap, _) in tiles {
                bitmap
                    .Close()
                    .map_err(|error| format!("cannot close OCR tile bitmap: {error}"))?;
            }
        }
        stream
            .Close()
            .map_err(|error| format!("cannot close image stream: {error}"))?;
        Ok(())
    }
}
