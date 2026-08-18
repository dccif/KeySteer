//! Incremental Windows UI Automation scanning.
//!
//! A backend-owned worker keeps COM and UIA on one MTA thread. Traversal is
//! deliberately incremental: usable targets are published before the full tree
//! has been visited, while input and tray handling remain responsive.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use smallvec::SmallVec;
use windows::Win32::Foundation::{
    HWND, LPARAM, POINT, RECT, RPC_E_CALL_COMPLETE, RPC_E_NO_CONTEXT, VARIANT_FALSE, VARIANT_TRUE,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTONULL, MonitorFromRect,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCancelCall, CoCreateInstance,
    CoDisableCallCancellation, CoEnableCallCancellation, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_I4};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest,
    IUIAutomationCondition, IUIAutomationElement, TreeScope_Descendants,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_IsEnabledPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsKeyboardFocusablePropertyId, UIA_IsOffscreenPropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId, UIA_IsTogglePatternAvailablePropertyId,
    UIA_NamePropertyId, UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GWL_EXSTYLE, GWL_STYLE, GetWindowRect, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, Interface};

use super::ui_scan::ScanSource;
use crate::api::command::{UiScanRequest, UiScanStatus};
use crate::api::geometry::{Rect, UiTarget};
use crate::app::worker::WorkerJoin;
use crate::platform::partial_batcher::PartialBatcher;

const PARTIAL_BATCH_SIZE: usize = 24;
const MAX_TARGETS: usize = 2_000;
const MAX_VISITED_ELEMENTS: usize = 20_000;
const MAX_SCAN_WINDOWS: usize = 16;
type InlineScanWindows = SmallVec<[ScanWindow; MAX_SCAN_WINDOWS]>;
type InlineOccluders = SmallVec<[Rect; MAX_SCAN_WINDOWS]>;
const MINIMUM_SPACING: f64 = 8.0;
const MIN_SCAN_TIMEOUT_MS: u64 = 250;
const MAX_SCAN_TIMEOUT_MS: u64 = 30_000;
const UIA_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

fn request_call_cancellation(thread_id: u32) {
    if thread_id != 0 {
        // SAFETY: `thread_id` belongs to the live worker thread, which enabled
        // COM call cancellation before accepting jobs. A zero-second timeout
        // requests cancellation without blocking the engine thread.
        if let Err(error) = unsafe { CoCancelCall(thread_id, 0) }
            && !cancellation_context_is_gone(error.code())
        {
            crate::report_error!("windows-uia", "cannot cancel blocked UIA call: {error}");
        }
    }
}

fn cancellation_context_is_gone(code: windows::core::HRESULT) -> bool {
    // These are expected races when cancellation arrives just before a
    // provider call begins or just after it returns.
    code == RPC_E_NO_CONTEXT || code == RPC_E_CALL_COMPLETE
}

struct ScanJob {
    request: Arc<WindowsScanPlan>,
    generation: u64,
    source: ScanSource,
}

impl ScanJob {
    fn publish(&self, targets: Vec<UiTarget>, status: UiScanStatus) {
        if status == UiScanStatus::Partial {
            self.source.push(targets);
        }
    }

    fn finish(self, status: UiScanStatus) {
        self.source.finish(status);
    }
}

#[derive(Default)]
struct QueueState {
    pending: Option<ScanJob>,
    active: Option<(u64, u64)>,
    stopping: bool,
}

struct SharedQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    latest_generation: AtomicU64,
    stopping: AtomicBool,
}

impl Default for SharedQueue {
    fn default() -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            ready: Condvar::new(),
            latest_generation: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
        }
    }
}

/// Backend-owned UIA worker. `stop` is idempotent and joins the COM thread.
pub struct UiAutomationWorker {
    shared: Arc<SharedQueue>,
    thread_id: Arc<AtomicU32>,
    worker: Option<WorkerJoin>,
}

impl UiAutomationWorker {
    pub fn start() -> Result<Self, String> {
        let shared = Arc::new(SharedQueue::default());
        let worker_shared = Arc::clone(&shared);
        let thread_id = Arc::new(AtomicU32::new(0));
        let worker_thread_id = Arc::clone(&thread_id);
        let worker = WorkerJoin::spawn(
            "Windows UI Automation",
            std::thread::Builder::new().name("keysteer-uia".into()),
            move || worker_main(worker_shared, worker_thread_id),
        )?;
        Ok(Self {
            shared,
            thread_id,
            worker: Some(worker),
        })
    }

    pub fn submit(
        &self,
        request: Arc<WindowsScanPlan>,
        generation: u64,
        source: ScanSource,
    ) -> Result<(), String> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.stopping {
            return Err("UI Automation worker is stopping".into());
        }
        self.shared
            .latest_generation
            .store(generation, Ordering::Release);
        let replaced = state.pending.replace(ScanJob {
            request,
            generation,
            source,
        });
        let cancel_active = state
            .active
            .is_some_and(|(_, active_generation)| active_generation != generation);
        if cancel_active {
            // Keep the queue lock until cancellation has been requested. The
            // worker cannot finish the old job and pick up this new one in the
            // small interval before CoCancelCall, so the new call can never be
            // cancelled by mistake.
            let thread_id = self.thread_id.load(Ordering::Acquire);
            request_call_cancellation(thread_id);
        }
        self.shared.ready.notify_one();
        drop(state);
        drop(replaced);
        Ok(())
    }

    pub fn cancel(&self, request_id: u64) {
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
        let cancel_active = state
            .active
            .is_some_and(|(active_id, _)| active_id == request_id);
        if cancel_active {
            self.shared.latest_generation.store(0, Ordering::Release);
            // Keep the queue lock until the request is sent, so this worker
            // cannot finish the old job and begin a newer generation first.
            let thread_id = self.thread_id.load(Ordering::Acquire);
            request_call_cancellation(thread_id);
        }
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if self.worker.is_none() {
            return Ok(());
        }
        let (pending, cancel_active) = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.stopping = true;
            self.shared.stopping.store(true, Ordering::Release);
            let pending = state.pending.take();
            let cancel_active = state.active.is_some();
            self.shared.ready.notify_all();
            (pending, cancel_active)
        };
        drop(pending);
        if cancel_active {
            let thread_id = self.thread_id.load(Ordering::Acquire);
            request_call_cancellation(thread_id);
        }
        let Some(worker) = self.worker.as_mut() else {
            return Ok(());
        };
        worker.join_timeout(UIA_SHUTDOWN_WAIT)?;
        self.worker.take();
        Ok(())
    }
}

impl Drop for UiAutomationWorker {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-uia", &error);
        }
    }
}

struct ComApartment(std::marker::PhantomData<std::rc::Rc<()>>);

impl ComApartment {
    fn initialise() -> Result<Self, String> {
        // SAFETY: this constructor runs on the future UIA owner thread. A
        // successful COM initialization is rolled back immediately if call
        // cancellation cannot be enabled; otherwise Drop owns both releases.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| format!("CoInitializeEx failed: {error}"))?;
            if let Err(error) = CoEnableCallCancellation(None) {
                CoUninitialize();
                return Err(format!("CoEnableCallCancellation failed: {error}"));
            }
        }
        Ok(Self(std::marker::PhantomData))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: this guard is dropped on the same thread that successfully
        // enabled COM call cancellation.
        if let Err(error) = unsafe { CoDisableCallCancellation(None) } {
            crate::report_error!(
                "windows-uia",
                "cannot disable UIA call cancellation: {error}"
            );
        }
        // SAFETY: this balances the successful CoInitializeEx owned by this
        // thread-bound guard after all COM interfaces were dropped.
        unsafe { CoUninitialize() };
    }
}

/// Immutable UIA query objects retained by the worker's MTA.
///
/// They describe which properties to cache and whether to filter for
/// interactive controls; no window or element from an earlier scan is kept.
struct UiaQueryPlan {
    cache: IUIAutomationCacheRequest,
    all: IUIAutomationCondition,
    interactive: Option<IUIAutomationCondition>,
}

impl UiaQueryPlan {
    fn new(automation: &IUIAutomation) -> Result<Self, String> {
        let cache = create_cache_request(automation)?;
        // SAFETY: the live automation interface creates an owned immutable
        // condition on its owning MTA thread.
        let all = unsafe { automation.CreateTrueCondition() }
            .map_err(|error| format!("cannot create the UIA scan condition: {error}"))?;
        let interactive = match create_interactive_condition(automation) {
            Ok(condition) => Some(condition),
            Err(error) => {
                crate::report_warning!(
                    "windows-uia",
                    "cannot create interactive UIA condition; clickable scans will filter cached properties locally: {error}"
                );
                None
            }
        };
        Ok(Self {
            cache,
            all,
            interactive,
        })
    }

    fn condition(&self, clickable_only: bool) -> (&IUIAutomationCondition, bool) {
        if clickable_only && let Some(interactive) = self.interactive.as_ref() {
            (interactive, true)
        } else {
            (&self.all, false)
        }
    }
}

fn worker_main(shared: Arc<SharedQueue>, thread_id: Arc<AtomicU32>) {
    thread_id.store(super::native::current_thread_id(), Ordering::Release);
    let apartment = match ComApartment::initialise() {
        Ok(apartment) => apartment,
        Err(error) => {
            fail_jobs_until_stopped(&shared, error);
            thread_id.store(0, Ordering::Release);
            return;
        }
    };
    // SAFETY: the current worker owns an initialized MTA apartment and requests
    // the documented in-process UI Automation class/interface pair.
    let automation: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
            Ok(automation) => automation,
            Err(error) => {
                fail_jobs_until_stopped(&shared, format!("cannot create UI Automation: {error}"));
                drop(apartment);
                thread_id.store(0, Ordering::Release);
                return;
            }
        };
    // IUIAutomation2 is optional. Keep its interface when available so every
    // request can install its own adaptive timeout before querying providers.
    let automation2 = match automation.cast::<IUIAutomation2>() {
        Ok(automation2) => Some(automation2),
        Err(error) => {
            crate::report_warning!(
                "windows-uia",
                "IUIAutomation2 is unavailable; continuing with base UI Automation: {error}"
            );
            None
        }
    };
    let query_plan = match UiaQueryPlan::new(&automation) {
        Ok(plan) => plan,
        Err(error) => {
            fail_jobs_until_stopped(&shared, error);
            drop(automation2);
            drop(automation);
            drop(apartment);
            thread_id.store(0, Ordering::Release);
            return;
        }
    };
    crate::app::perf_probe::mark("uia_prewarm_ready");

    let mut configured_timeout = None;
    while let Some(job) = next_job(&shared) {
        let id = job.request.id;
        let generation = job.generation;
        run_scan(
            job,
            &automation,
            automation2.as_ref(),
            &query_plan,
            &shared,
            &mut configured_timeout,
        );
        finish_job(&shared, id, generation);
    }
    // COM interfaces must be released before the apartment is uninitialised.
    drop(query_plan);
    drop(automation2);
    drop(automation);
    drop(apartment);
    thread_id.store(0, Ordering::Release);
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
    if state.stopping {
        None
    } else {
        let job = state.pending.take();
        state.active = job.as_ref().map(|job| (job.request.id, job.generation));
        job
    }
}

fn finish_job(shared: &SharedQueue, id: u64, generation: u64) {
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.active == Some((id, generation)) {
        state.active = None;
    }
}

fn fail_jobs_until_stopped(shared: &SharedQueue, error: String) {
    while let Some(job) = next_job(shared) {
        job.finish(UiScanStatus::Failed(error.clone()));
    }
}

fn is_current(shared: &SharedQueue, generation: u64) -> bool {
    !shared.stopping.load(Ordering::Acquire)
        && shared.latest_generation.load(Ordering::Acquire) == generation
}

#[inline]
fn context_is_current(shared: &SharedQueue, generation: u64, plan: &WindowsScanPlan) -> bool {
    is_current(shared, generation) && plan.target_is_current()
}

fn run_scan(
    job: ScanJob,
    automation: &IUIAutomation,
    automation2: Option<&IUIAutomation2>,
    query_plan: &UiaQueryPlan,
    shared: &SharedQueue,
    configured_timeout: &mut Option<u32>,
) {
    let timeout_ms = scan_timeout_ms(job.request.timeout_ms);
    if let Some(automation2) = automation2
        && timeout_needs_configuration(*configured_timeout, timeout_ms)
    {
        // SAFETY: `automation2` is a live interface on its owning MTA thread and
        // timeout_ms is clamped to the supported positive range.
        let connection_set = unsafe { automation2.SetConnectionTimeout(timeout_ms) };
        if let Err(error) = &connection_set {
            crate::report_warning!(
                "windows-uia",
                "cannot set UIA connection timeout; continuing with provider defaults: {error}"
            );
        }
        // SAFETY: `automation2` is a live interface on its owning MTA thread and
        // timeout_ms is clamped to the supported positive range.
        let transaction_set = unsafe { automation2.SetTransactionTimeout(timeout_ms) };
        if let Err(error) = &transaction_set {
            crate::report_warning!(
                "windows-uia",
                "cannot set UIA transaction timeout; continuing with provider defaults: {error}"
            );
        }
        if connection_set.is_ok() && transaction_set.is_ok() {
            *configured_timeout = Some(timeout_ms);
        }
    }

    let status = match stream_scan(automation, query_plan, &job, shared) {
        Ok(status) => status,
        Err(error) => UiScanStatus::Failed(format!("UI Automation scan failed: {error}")),
    };
    job.finish(status);
}

fn scan_timeout_ms(requested: u64) -> u32 {
    requested.clamp(MIN_SCAN_TIMEOUT_MS, MAX_SCAN_TIMEOUT_MS) as u32
}

#[inline]
fn timeout_needs_configuration(configured: Option<u32>, requested: u32) -> bool {
    configured != Some(requested)
}

fn is_timeout_hresult(code: i32) -> bool {
    matches!(code as u32, 0x8013_1505 | 0x8007_05B4 | 0x8001_011F)
}

fn is_timeout_error(error: &windows::core::Error) -> bool {
    is_timeout_hresult(error.code().0)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ScanWindow {
    hwnd: isize,
    pub(super) bounds: Rect,
    /// Prefix length in the plan's shared front-to-back occluder array.
    occluder_end: u8,
}

/// Immutable, generation-scoped Windows scan input shared by UIA and Vision.
///
/// The request, native target identity and visibility snapshot live in one
/// allocation. Provider jobs only clone the `Arc`, so Hybrid does not clone
/// role/config vectors or a per-window occluder list.
pub(super) struct WindowsScanPlan {
    request: UiScanRequest,
    target_hwnd: isize,
    target_process_id: u32,
    target_bounds: Rect,
    scan_bounds: Rect,
    windows: InlineScanWindows,
    occluders: InlineOccluders,
}

impl Deref for WindowsScanPlan {
    type Target = UiScanRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

impl WindowsScanPlan {
    pub(super) fn target_hwnd(&self) -> HWND {
        HWND(self.target_hwnd as *mut core::ffi::c_void)
    }

    pub(super) fn scan_bounds(&self) -> Rect {
        self.scan_bounds
    }

    fn windows(&self) -> &[ScanWindow] {
        &self.windows
    }

    fn window_occluders(&self, window: &ScanWindow) -> &[Rect] {
        &self.occluders[..usize::from(window.occluder_end)]
    }

    pub(super) fn target_is_current(&self) -> bool {
        let target_hwnd = self.target_hwnd();
        if target_hwnd.is_invalid() || !super::native::is_window(target_hwnd) {
            return false;
        }
        let mut process_id = 0;
        super::native::window_thread_process_id(target_hwnd, Some(&mut process_id));
        process_id == self.target_process_id
            && window_bounds(target_hwnd) == Some(self.target_bounds)
            && self.windows.iter().all(|window| {
                let hwnd = window.hwnd();
                !hwnd.is_invalid()
                    && super::native::is_window(hwnd)
                    && window_bounds(hwnd).and_then(|bounds| bounds.intersect(&self.scan_bounds))
                        == Some(window.bounds)
            })
    }

    pub(super) fn target_center_is_visible(&self, target: &UiTarget) -> bool {
        let center = target.rect.center();
        self.windows.iter().any(|window| {
            window.bounds.contains(&center)
                && !self
                    .window_occluders(window)
                    .iter()
                    .any(|rect| rect.contains(&center))
        })
    }
}

#[cfg(test)]
pub(super) fn test_scan_plan(request: UiScanRequest) -> Arc<WindowsScanPlan> {
    let bounds = request
        .bounds
        .unwrap_or_else(|| Rect::new(0.0, 0.0, 1_920.0, 1_080.0));
    Arc::new(WindowsScanPlan {
        request,
        target_hwnd: 0,
        target_process_id: 0,
        target_bounds: bounds,
        scan_bounds: bounds,
        windows: SmallVec::from_slice(&[ScanWindow {
            hwnd: 0,
            bounds,
            occluder_end: 0,
        }]),
        occluders: SmallVec::new(),
    })
}

impl ScanWindow {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }
}

fn rect_from_native(rect: RECT) -> Option<Rect> {
    let width = i64::from(rect.right) - i64::from(rect.left);
    let height = i64::from(rect.bottom) - i64::from(rect.top);
    (width > 0 && height > 0).then(|| {
        Rect::new(
            rect.left as f64,
            rect.top as f64,
            width as f64,
            height as f64,
        )
    })
}

fn native_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.left().floor() as i32,
        top: rect.top().floor() as i32,
        right: rect.right().ceil() as i32,
        bottom: rect.bottom().ceil() as i32,
    }
}

/// DWM's extended frame excludes the invisible resize border and drop shadow.
/// Fall back to GetWindowRect for classic controls and windows without DWM.
pub(super) fn window_bounds(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a correctly sized writable out-buffer and `hwnd` is
    // borrowed only for this synchronous DWM query.
    let dwm_rect = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if dwm_rect.is_err()
        // SAFETY: `rect` is a valid writable out-parameter and `hwnd` is only
        // borrowed for this synchronous fallback query.
        && unsafe { GetWindowRect(hwnd, &mut rect) }.is_err()
    {
        return None;
    }
    rect_from_native(rect)
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is a correctly sized writable out-buffer and `hwnd` is
    // borrowed only for this synchronous DWM query.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok_and(|_| cloaked != 0)
}

fn class_name_is_shell_surface(hwnd: HWND) -> bool {
    fn ascii_lower(unit: u16) -> u16 {
        if unit >= u16::from(b'A') && unit <= u16::from(b'Z') {
            unit + u16::from(b'a' - b'A')
        } else {
            unit
        }
    }

    let mut class_name = [0u16; 64];
    let length = super::native::window_class_name(hwnd, &mut class_name);
    let class_name = &class_name[..length.min(class_name.len())];
    [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
    ]
    .iter()
    .any(|expected| {
        class_name
            .iter()
            .copied()
            .map(ascii_lower)
            .eq(expected.encode_utf16().map(ascii_lower))
    })
}

fn normalize_root_owner(hwnd: HWND) -> HWND {
    if hwnd.is_invalid() {
        return hwnd;
    }
    let root = super::native::root_owner(hwnd);
    if root.is_invalid() { hwnd } else { root }
}

fn scannable_target(hwnd: HWND) -> Option<(HWND, u32, Rect)> {
    let hwnd = normalize_root_owner(hwnd);
    let desktop = super::native::desktop_window();
    let valid = super::native::is_window(hwnd);
    if hwnd.is_invalid()
        || hwnd == desktop
        || !valid
        || !super::native::is_window_visible(hwnd)
        || super::native::is_window_iconic(hwnd)
        || is_cloaked(hwnd)
        || super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0
        || class_name_is_shell_surface(hwnd)
    {
        return None;
    }
    let mut process_id = 0;
    super::native::window_thread_process_id(hwnd, Some(&mut process_id));
    if process_id == 0 || process_id == super::native::current_process_id() {
        return None;
    }
    window_bounds(hwnd).map(|bounds| (hwnd, process_id, bounds))
}

fn native_point_in_rect(point: POINT, rect: Rect) -> bool {
    let x = f64::from(point.x);
    let y = f64::from(point.y);
    x >= rect.left() && x < rect.right() && y >= rect.top() && y < rect.bottom()
}

struct PointWindowCollector {
    point: POINT,
    target: Option<(HWND, u32, Rect)>,
}

extern "system" fn collect_window_at_point(hwnd: HWND, _data: LPARAM) -> BOOL {
    POINT_WINDOW_COLLECTOR.with_borrow_mut(|slot| {
        let Some(collector) = slot.as_mut() else {
            return BOOL(0);
        };
        if collector.target.is_none()
            && window_bounds(hwnd)
                .is_some_and(|bounds| native_point_in_rect(collector.point, bounds))
            && let Some(target) = scannable_target(hwnd)
        {
            collector.target = Some(target);
        }
        BOOL(1)
    })
}

fn window_under_pointer() -> Result<Option<(HWND, u32, Rect)>, String> {
    let cursor = super::input::cursor_position()?;
    let point = POINT {
        x: cursor.x.round() as i32,
        y: cursor.y.round() as i32,
    };
    if let Some(target) = scannable_target(super::native::window_from_point(point)) {
        return Ok(Some(target));
    }

    POINT_WINDOW_COLLECTOR.with_borrow_mut(|slot| {
        *slot = Some(PointWindowCollector {
            point,
            target: None,
        })
    });
    let enumeration = super::native::enum_windows(Some(collect_window_at_point), LPARAM(0))
        .map_err(|error| format!("cannot resolve the window under the pointer: {error}"));
    let target = POINT_WINDOW_COLLECTOR
        .with_borrow_mut(Option::take)
        .and_then(|collector| collector.target);
    enumeration?;
    Ok(target)
}

thread_local! {
    static MONITOR_COLLECTOR: RefCell<SmallVec<[HMONITOR; 4]>> = RefCell::new(SmallVec::new());
    static THREAD_WINDOW_COLLECTOR: RefCell<Option<ThreadWindowCollector>> = const { RefCell::new(None) };
    static Z_ORDER_COLLECTOR: RefCell<Option<ZOrderCollector>> = const { RefCell::new(None) };
    static POINT_WINDOW_COLLECTOR: RefCell<Option<PointWindowCollector>> = const { RefCell::new(None) };
}

extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    _data: LPARAM,
) -> BOOL {
    MONITOR_COLLECTOR.with_borrow_mut(|monitors| monitors.push(monitor));
    BOOL(1)
}

fn monitors_intersecting(hwnd: HWND) -> SmallVec<[HMONITOR; 4]> {
    let Some(bounds) = window_bounds(hwnd) else {
        return SmallVec::new();
    };
    let clip = native_rect(bounds);
    MONITOR_COLLECTOR.with_borrow_mut(|monitors| monitors.clear());
    // SAFETY: the callback ABI matches and the collector pointer remains valid
    // for the complete synchronous enumeration.
    unsafe {
        let _ = EnumDisplayMonitors(None, Some(&clip), Some(collect_monitor), LPARAM(0));
    };
    MONITOR_COLLECTOR.with_borrow_mut(std::mem::take)
}

fn is_owned_by(hwnd: HWND, foreground: HWND) -> bool {
    hwnd != foreground && normalize_root_owner(hwnd) == foreground
}

fn popup_relationship_is_scannable(
    has_popup_style: bool,
    owned_by_foreground: bool,
    shares_foreground_monitor: bool,
) -> bool {
    has_popup_style && (owned_by_foreground || shares_foreground_monitor)
}

struct ThreadWindowCollector {
    foreground: HWND,
    foreground_monitors: SmallVec<[HMONITOR; 4]>,
    windows: SmallVec<[HWND; MAX_SCAN_WINDOWS]>,
}

extern "system" fn collect_thread_window(hwnd: HWND, _data: LPARAM) -> BOOL {
    THREAD_WINDOW_COLLECTOR.with_borrow_mut(|slot| {
        let Some(collector) = slot.as_mut() else {
            return BOOL(0);
        };
        if hwnd == collector.foreground
            || !super::native::is_window_visible(hwnd)
            || super::native::is_window_iconic(hwnd)
            || is_cloaked(hwnd)
        {
            return BOOL(1);
        }

        let has_popup_style = super::native::window_long(hwnd, GWL_STYLE) as u32 & WS_POPUP.0 != 0;
        let owned = is_owned_by(hwnd, collector.foreground);
        let shares_monitor = window_bounds(hwnd).is_some_and(|bounds| {
            let rect = native_rect(bounds);
            // SAFETY: `rect` is initialized and lives for the synchronous monitor
            // lookup; the returned HMONITOR is borrowed.
            let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
            !monitor.is_invalid() && collector.foreground_monitors.contains(&monitor)
        });
        if collector.windows.len() < MAX_SCAN_WINDOWS
            && popup_relationship_is_scannable(has_popup_style, owned, shares_monitor)
        {
            collector.windows.push(hwnd);
        }
        BOOL(1)
    })
}

fn foreground_and_popup_windows(foreground: HWND) -> SmallVec<[HWND; MAX_SCAN_WINDOWS]> {
    let mut windows = SmallVec::new();
    windows.push(foreground);
    let collector = ThreadWindowCollector {
        foreground,
        foreground_monitors: monitors_intersecting(foreground),
        windows,
    };
    THREAD_WINDOW_COLLECTOR.with_borrow_mut(|slot| *slot = Some(collector));
    let thread_id = super::native::window_thread_process_id(foreground, None);
    if thread_id != 0 {
        // SAFETY: callback ABI and collector pointer are valid for the complete
        // synchronous enumeration of this live UI thread.
        unsafe {
            let _ = EnumThreadWindows(thread_id, Some(collect_thread_window), LPARAM(0));
        };
    }
    THREAD_WINDOW_COLLECTOR
        .with_borrow_mut(Option::take)
        .map_or_else(SmallVec::new, |collector| collector.windows)
}

struct ZOrderCollector {
    candidates: SmallVec<[HWND; MAX_SCAN_WINDOWS]>,
    scan_bounds: Rect,
    own_process_id: u32,
    scan_windows: InlineScanWindows,
    occluders: InlineOccluders,
}

extern "system" fn collect_z_order_window(hwnd: HWND, _data: LPARAM) -> BOOL {
    Z_ORDER_COLLECTOR.with_borrow_mut(|slot| {
        let Some(collector) = slot.as_mut() else {
            return BOOL(0);
        };
        if !super::native::is_window_visible(hwnd)
            || super::native::is_window_iconic(hwnd)
            || is_cloaked(hwnd)
        {
            return BOOL(1);
        }
        let click_through =
            super::native::window_long(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0;
        if click_through {
            return BOOL(1);
        }
        let Some(bounds) = window_bounds(hwnd) else {
            return BOOL(1);
        };
        let Some(bounds_in_scan) = bounds.intersect(&collector.scan_bounds) else {
            return BOOL(1);
        };

        let candidate = collector.candidates.contains(&hwnd);
        if candidate {
            collector.scan_windows.push(ScanWindow {
                hwnd: hwnd.0 as isize,
                bounds: bounds_in_scan,
                occluder_end: collector.occluders.len().min(u8::MAX as usize) as u8,
            });
        }

        let mut process_id = 0;
        super::native::window_thread_process_id(hwnd, Some(&mut process_id));
        // Our click-through overlay was already skipped above. Ignore any other
        // helper/tray HWND owned by this process so it cannot hide application UI.
        if (candidate || process_id != collector.own_process_id)
            && collector.occluders.len() < MAX_SCAN_WINDOWS
        {
            collector.occluders.push(bounds_in_scan);
        }
        BOOL(1)
    })
}

fn scan_windows_in_z_order(
    foreground: HWND,
    scan_bounds: Rect,
) -> Result<(InlineScanWindows, InlineOccluders), String> {
    let candidates = foreground_and_popup_windows(foreground);
    let collector = ZOrderCollector {
        candidates,
        scan_bounds,
        own_process_id: super::native::current_process_id(),
        scan_windows: SmallVec::new(),
        occluders: SmallVec::new(),
    };
    Z_ORDER_COLLECTOR.with_borrow_mut(|slot| *slot = Some(collector));
    let enumeration = super::native::enum_windows(Some(collect_z_order_window), LPARAM(0))
        .map_err(|error| format!("cannot enumerate top-level windows: {error}"));
    let collector = Z_ORDER_COLLECTOR
        .with_borrow_mut(Option::take)
        .ok_or_else(|| "top-level window collector disappeared".to_string())?;
    enumeration?;
    Ok((collector.scan_windows, collector.occluders))
}

pub(super) fn build_scan_plan(
    request: UiScanRequest,
) -> Result<Option<Arc<WindowsScanPlan>>, String> {
    let Some((target_hwnd, target_process_id, target_bounds)) = window_under_pointer()? else {
        return Ok(None);
    };
    // The mode bounds identify the pointer's display. Use them to discover
    // owned popups which can extend beyond the root-owner rectangle, but ignore
    // stale bounds after a cross-display pointer move between mode dispatch and
    // native submission.
    let enumeration_bounds = request
        .bounds
        .filter(|requested| requested.intersect(&target_bounds).is_some())
        .unwrap_or(target_bounds);
    let (mut windows, occluders) = scan_windows_in_z_order(target_hwnd, enumeration_bounds)?;
    // A live target can be omitted transiently while its z-order changes. The
    // resolved root remains a safe single-window fallback for this generation.
    if windows.is_empty()
        && let Some(bounds) = target_bounds.intersect(&enumeration_bounds)
    {
        windows.push(ScanWindow {
            hwnd: target_hwnd.0 as isize,
            bounds,
            occluder_end: 0,
        });
    }
    let scan_bounds = windows
        .iter()
        .map(|window| window.bounds)
        .reduce(|left, right| left.union(&right))
        .unwrap_or(target_bounds);
    Ok(Some(Arc::new(WindowsScanPlan {
        request,
        target_hwnd: target_hwnd.0 as isize,
        target_process_id,
        target_bounds,
        scan_bounds,
        windows,
        occluders,
    })))
}

fn stream_scan(
    automation: &IUIAutomation,
    query_plan: &UiaQueryPlan,
    job: &ScanJob,
    shared: &SharedQueue,
) -> Result<UiScanStatus, String> {
    let hwnd = job.request.target_hwnd();
    if hwnd.is_invalid() {
        return Ok(UiScanStatus::Success);
    }
    let cache = &query_plan.cache;
    let (condition, provider_filters_interactive) =
        query_plan.condition(job.request.clickable_only);
    let mut allowed = control_types_for(&job.request.roles);
    if allowed.is_empty() {
        allowed.extend(DEFAULT_CONTROL_TYPES);
    }
    let scan_bounds = job.request.scan_bounds();
    let deadline =
        Instant::now() + Duration::from_millis(u64::from(scan_timeout_ms(job.request.timeout_ms)));
    let mut deduper = SpatialDeduper::new(MINIMUM_SPACING);
    let mut batches = PartialBatcher::new(PARTIAL_BATCH_SIZE, MAX_TARGETS);
    let mut target_count = 0usize;
    let mut visited_count = 0usize;
    let mut queried_window = false;
    let mut provider_timed_out = false;
    let mut terminal_status = UiScanStatus::Success;

    // Windows are ordered front-to-back. Popup/menu targets are therefore
    // published before their owner, and every lower window carries the bounds
    // of higher click-receiving windows as occluders.
    for window in job.request.windows() {
        if !context_is_current(shared, job.generation, &job.request) {
            return Ok(UiScanStatus::ContextChanged);
        }
        if Instant::now() >= deadline {
            terminal_status = UiScanStatus::TimedOut;
            break;
        }
        if visited_count >= MAX_VISITED_ELEMENTS {
            terminal_status = UiScanStatus::TimedOut;
            break;
        }
        if target_count >= MAX_TARGETS {
            break;
        }

        // SAFETY: the HWND is live for this iteration and `cache` belongs to
        // the same automation instance and MTA thread.
        let root = match unsafe { automation.ElementFromHandleBuildCache(window.hwnd(), cache) } {
            Ok(root) => root,
            Err(error) => {
                provider_timed_out |= is_timeout_error(&error);
                crate::report_warning!(
                    "windows-uia",
                    "cannot inspect popup/top-level window {:?}; continuing: {error}",
                    window.hwnd()
                );
                continue;
            }
        };
        // A bulk descendant query is substantially more reliable for Chromium,
        // WebView2 and other virtualized trees than walking only Control View.
        // SAFETY: root, condition and cache are live COM interfaces from this
        // automation instance and remain borrowed through the bulk call.
        let elements =
            match unsafe { root.FindAllBuildCache(TreeScope_Descendants, condition, cache) } {
                Ok(elements) => elements,
                Err(error) => {
                    provider_timed_out |= is_timeout_error(&error);
                    crate::report_warning!(
                        "windows-uia",
                        "cannot query UIA descendants for {:?}; continuing: {error}",
                        window.hwnd()
                    );
                    continue;
                }
            };
        queried_window = true;
        // SAFETY: `elements` is a live array returned by the preceding query.
        let element_count = unsafe { elements.Length() }
            .map_err(|error| format!("cannot read UIA descendant count: {error}"))?
            .max(0) as usize;
        let remaining = MAX_VISITED_ELEMENTS - visited_count;

        for index in 0..element_count.min(remaining) {
            // SAFETY: `index` is bounded by Length and the returned COM element
            // is independently reference counted.
            let element = unsafe { elements.GetElement(index as i32) }
                .map_err(|error| format!("cannot read UIA descendant {index}: {error}"))?;
            visited_count += 1;
            // The atomic generation is cheap enough for every node. Querying
            // target HWND/PID/bounds cross into user32/DWM, so sample them every 32
            // nodes and always immediately before publishing a partial batch.
            if !is_current(shared, job.generation)
                || visited_count.is_multiple_of(32) && !job.request.target_is_current()
            {
                return Ok(UiScanStatus::ContextChanged);
            }
            if Instant::now() >= deadline {
                terminal_status = UiScanStatus::TimedOut;
                break;
            }
            if target_count >= MAX_TARGETS {
                break;
            }

            if let Some(target) = target_from_element(
                &element,
                &job.request,
                &allowed,
                scan_bounds,
                provider_filters_interactive,
            ) && job.request.target_center_is_visible(&target)
                && deduper.insert(&target)
            {
                target_count += 1;
                if let Some(batch) = batches.push_one(target) {
                    if !context_is_current(shared, job.generation, &job.request) {
                        return Ok(UiScanStatus::ContextChanged);
                    }
                    send_partial(job, batch);
                }
            }
        }
        if element_count > remaining {
            terminal_status = UiScanStatus::TimedOut;
        }
        if terminal_status == UiScanStatus::TimedOut || target_count >= MAX_TARGETS {
            break;
        }
    }
    if !queried_window {
        if provider_timed_out {
            return Ok(UiScanStatus::TimedOut);
        }
        return Err("cannot inspect any target or popup UIA window".into());
    }
    if provider_timed_out && terminal_status == UiScanStatus::Success {
        terminal_status = UiScanStatus::TimedOut;
    }
    if !context_is_current(shared, job.generation, &job.request) {
        return Ok(UiScanStatus::ContextChanged);
    }
    if let Some(batch) = batches.finish() {
        send_partial(job, batch);
    }
    Ok(terminal_status)
}

fn send_partial(job: &ScanJob, targets: Vec<UiTarget>) {
    job.publish(targets, UiScanStatus::Partial);
}

fn create_cache_request(automation: &IUIAutomation) -> Result<IUIAutomationCacheRequest, String> {
    // SAFETY: `automation` is live on its owning MTA and returns an owned cache
    // request interface.
    let cache = unsafe { automation.CreateCacheRequest() }
        .map_err(|error| format!("cannot create UIA cache request: {error}"))?;
    for property in [
        UIA_BoundingRectanglePropertyId,
        UIA_ControlTypePropertyId,
        UIA_IsEnabledPropertyId,
        UIA_IsKeyboardFocusablePropertyId,
        UIA_IsOffscreenPropertyId,
        UIA_NamePropertyId,
    ] {
        // SAFETY: every property id is a documented UIA property and `cache`
        // remains live on its owning MTA thread.
        unsafe { cache.AddProperty(property) }
            .map_err(|error| format!("cannot configure UIA property cache: {error}"))?;
    }
    Ok(cache)
}

fn variant_bool(value: bool) -> VARIANT {
    let mut variant = VARIANT::default();
    // SAFETY: the active VARIANT union field is selected by setting VT_BOOL,
    // then initialized with the corresponding bool member.
    unsafe {
        let inner = &mut variant.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if value { VARIANT_TRUE } else { VARIANT_FALSE };
    }
    variant
}

fn variant_i32(value: i32) -> VARIANT {
    let mut variant = VARIANT::default();
    // SAFETY: the active VARIANT union field is selected by setting VT_I4,
    // then initialized with the corresponding integer member.
    unsafe {
        let inner = &mut variant.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = value;
    }
    variant
}

/// Mirrors mousemaster's provider-side filter: visible and enabled elements
/// which expose at least one interaction affordance. Keeping this condition
/// in UIA avoids materializing every static text/container in large WebView
/// trees, while the Rust-side role filter still honours user configuration.
fn create_interactive_condition(
    automation: &IUIAutomation,
) -> Result<IUIAutomationCondition, String> {
    let true_value = variant_bool(true);
    let false_value = variant_bool(false);
    let focusable = property_condition(
        automation,
        UIA_IsKeyboardFocusablePropertyId,
        &true_value,
        "keyboard-focusable",
    )?;
    let invokable = property_condition(
        automation,
        UIA_IsInvokePatternAvailablePropertyId,
        &true_value,
        "invoke",
    )?;
    let button = property_condition(
        automation,
        UIA_ControlTypePropertyId,
        &variant_i32(50000),
        "button",
    )?;
    let expandable = property_condition(
        automation,
        UIA_IsExpandCollapsePatternAvailablePropertyId,
        &true_value,
        "expand/collapse",
    )?;
    let toggle = property_condition(
        automation,
        UIA_IsTogglePatternAvailablePropertyId,
        &true_value,
        "toggle",
    )?;
    let selection = property_condition(
        automation,
        UIA_IsSelectionItemPatternAvailablePropertyId,
        &true_value,
        "selection",
    )?;
    let enabled = property_condition(automation, UIA_IsEnabledPropertyId, &true_value, "enabled")?;
    let onscreen = property_condition(
        automation,
        UIA_IsOffscreenPropertyId,
        &false_value,
        "onscreen",
    )?;

    let mut interactive = or_condition(automation, &focusable, &invokable, "interactive")?;
    for extra in [&button, &expandable, &toggle, &selection] {
        interactive = or_condition(automation, &interactive, extra, "interactive")?;
    }
    let enabled_and_visible = and_condition(automation, &enabled, &onscreen, "visibility")?;
    and_condition(automation, &enabled_and_visible, &interactive, "final UIA")
}

fn property_condition(
    automation: &IUIAutomation,
    property: UIA_PROPERTY_ID,
    value: &VARIANT,
    context: &str,
) -> Result<IUIAutomationCondition, String> {
    // SAFETY: the property id and VARIANT are initialized UIA values; the
    // returned COM condition is independently reference counted.
    unsafe { automation.CreatePropertyCondition(property, value) }
        .map_err(|error| format!("{context} condition failed: {error}"))
}

fn or_condition(
    automation: &IUIAutomation,
    left: &IUIAutomationCondition,
    right: &IUIAutomationCondition,
    context: &str,
) -> Result<IUIAutomationCondition, String> {
    // SAFETY: both conditions are live COM interfaces from this automation
    // instance and remain borrowed for the complete call.
    unsafe { automation.CreateOrCondition(left, right) }
        .map_err(|error| format!("{context} condition failed: {error}"))
}

fn and_condition(
    automation: &IUIAutomation,
    left: &IUIAutomationCondition,
    right: &IUIAutomationCondition,
    context: &str,
) -> Result<IUIAutomationCondition, String> {
    // SAFETY: both conditions are live COM interfaces from this automation
    // instance and remain borrowed for the complete call.
    unsafe { automation.CreateAndCondition(left, right) }
        .map_err(|error| format!("{context} condition failed: {error}"))
}

const DEFAULT_CONTROL_TYPES: [i32; 17] = [
    50000, 50002, 50003, 50004, 50005, 50006, 50007, 50011, 50013, 50015, 50016, 50019, 50020,
    50024, 50029, 50031, 50035,
];

fn target_from_element(
    element: &IUIAutomationElement,
    request: &UiScanRequest,
    allowed: &[i32],
    within: Rect,
    provider_filters_interactive: bool,
) -> Option<UiTarget> {
    // FindAllBuildCache already populated this element. Reading cached
    // properties directly avoids a second provider call, which is important
    // for WebView2/Chromium providers where BuildUpdatedCache may time out or
    // return E_ELEMENTNOTAVAILABLE after the bulk query.
    // SAFETY: the element came from a cache-populated array and the cached
    // property call returns a value without retaining Rust data.
    let control_type = unsafe { element.CachedControlType() }.ok()?.0;
    // Windows applications often expose custom controls with a generic UIA
    // control type. Their interaction pattern is more reliable than the type,
    // and also lets shared macOS-oriented role lists work on Windows.
    let role_matches = allowed.contains(&control_type);
    // When FindAllBuildCache used mousemaster's provider-side interaction
    // condition, the returned set is already interactive even if the cached
    // keyboard-focus property is false (common for WebView buttons).
    let interactive = provider_filters_interactive || is_interactive(element);
    if !role_matches && !interactive {
        return None;
    }
    // SAFETY: the cached element remains live for this synchronous property
    // read on the owning MTA thread.
    let visible = unsafe { element.CachedIsOffscreen() }
        .ok()
        .is_none_or(|offscreen| !offscreen.as_bool());
    // SAFETY: the cached element remains live for this synchronous property
    // read on the owning MTA thread.
    let enabled = unsafe { element.CachedIsEnabled() }
        .ok()
        .is_none_or(|value| value.as_bool());
    if (request.visible_only && !visible)
        || (request.clickable_only && (!enabled || (!role_matches && !interactive)))
    {
        return None;
    }
    // SAFETY: the cached element remains live and returns its rectangle by
    // value on the owning MTA thread.
    let raw_rect = unsafe { element.CachedBoundingRectangle() }.ok()?;
    let rect = Rect::new(
        raw_rect.left as f64,
        raw_rect.top as f64,
        (raw_rect.right - raw_rect.left) as f64,
        (raw_rect.bottom - raw_rect.top) as f64,
    );
    if !rect_is_usable(&rect, within) {
        return None;
    }
    // SAFETY: the cached element remains live; the returned BSTR is converted
    // to an owned Rust String before the COM value is released.
    let name = unsafe { element.CachedName() }
        .map(|name| name.to_string())
        .unwrap_or_default();
    Some(to_target(rect, name, control_type))
}

fn is_interactive(element: &IUIAutomationElement) -> bool {
    // SAFETY: the cached element remains live for this synchronous property
    // read on the owning MTA thread.
    unsafe { element.CachedIsKeyboardFocusable() }.is_ok_and(|value| value.as_bool())
}

/// Map semantic roles to UIA control type ids.
pub fn control_types_for(semantic_roles: &[String]) -> Vec<i32> {
    let mut out = Vec::new();
    for role in semantic_roles {
        let normalized = role.trim().to_ascii_lowercase();
        if let Some(raw) = normalized.strip_prefix("uia:") {
            if let Ok(id) = raw.parse::<i32>() {
                out.push(id);
            }
            continue;
        }
        // AX/AT-SPI entries belong to other platforms. Ignoring them here is
        // intentional; an all-native foreign list falls back to Windows's
        // default control set in stream_scan.
        if normalized.starts_with("ax:") || normalized.starts_with("atspi:") {
            continue;
        }
        if normalized.contains(':') {
            continue;
        }
        out.extend(match normalized.as_str() {
            "button" | "toolbar_button" => &[50000][..],
            "menu_button" | "popup_button" => &[50031, 50000],
            "combo_box" => &[50003],
            "link" => &[50005],
            "checkbox" | "switch" => &[50002],
            "radio" => &[50013],
            "text_field" | "text_area" | "search_field" => &[50004],
            "slider" => &[50015],
            "stepper" => &[50016],
            "tab" => &[50019],
            "menu_item" => &[50011],
            "menubar_item" => &[50010],
            "cell" => &[50029, 50035],
            "list_item" => &[50007],
            "row" => &[50029],
            "tree_item" => &[50024],
            "image" => &[50006],
            "static_text" | "heading" => &[50020],
            "calendar" => &[50001],
            _ => &[],
        });
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn semantic_role_for(control_type: i32) -> &'static str {
    match control_type {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combo_box",
        50004 => "text_field",
        50005 => "link",
        50006 => "image",
        50007 => "list_item",
        50010 => "menubar_item",
        50011 => "menu_item",
        50013 => "radio",
        50015 => "slider",
        50016 => "stepper",
        50019 => "tab",
        50020 => "static_text",
        50024 => "tree_item",
        50029 => "row",
        50031 => "menu_button",
        50035 => "cell",
        _ => "unknown",
    }
}

pub fn to_target(rect: Rect, name: String, control_type: i32) -> UiTarget {
    UiTarget {
        rect,
        name,
        role: semantic_role_for(control_type).to_string(),
        native_role: Some(control_type.to_string()),
    }
}

#[cfg(test)]
pub fn is_usable(target: &UiTarget, within: Rect) -> bool {
    rect_is_usable(&target.rect, within)
}

fn rect_is_usable(rect: &Rect, within: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width >= 2.0
        && rect.height >= 2.0
        && !(rect.width >= within.width && rect.height >= within.height)
        && within.intersect(rect).is_some()
}

struct DedupEntry {
    x: f64,
    y: f64,
    semantic_index: usize,
}

struct SpatialDeduper {
    cell_size: f64,
    cells: HashMap<(i32, i32), SmallVec<[DedupEntry; 2]>>,
    semantics: Vec<(String, String)>,
    semantic_buckets: HashMap<u64, SmallVec<[usize; 2]>>,
}

impl SpatialDeduper {
    fn new(minimum_spacing: f64) -> Self {
        Self {
            cell_size: minimum_spacing.max(1.0),
            cells: HashMap::new(),
            semantics: Vec::new(),
            semantic_buckets: HashMap::new(),
        }
    }

    fn insert(&mut self, target: &UiTarget) -> bool {
        let center = target.rect.center();
        let semantic_index = self.semantic_index(target);
        let cell = (
            (center.x / self.cell_size).floor() as i32,
            (center.y / self.cell_size).floor() as i32,
        );
        for y in cell.1.saturating_sub(1)..=cell.1.saturating_add(1) {
            for x in cell.0.saturating_sub(1)..=cell.0.saturating_add(1) {
                if self.cells.get(&(x, y)).is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        (entry.x - center.x).abs() < self.cell_size
                            && (entry.y - center.y).abs() < self.cell_size
                            && entry.semantic_index == semantic_index
                    })
                }) {
                    return false;
                }
            }
        }
        self.cells.entry(cell).or_default().push(DedupEntry {
            x: center.x,
            y: center.y,
            semantic_index,
        });
        true
    }

    fn semantic_index(&mut self, target: &UiTarget) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        target.name.hash(&mut hasher);
        target.role.hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(index) = self.semantic_buckets.get(&hash).and_then(|indices| {
            indices.iter().copied().find(|&index| {
                let (name, role) = &self.semantics[index];
                name == &target.name && role == &target.role
            })
        }) {
            return index;
        }
        let index = self.semantics.len();
        self.semantics
            .push((target.name.clone(), target.role.clone()));
        self.semantic_buckets.entry(hash).or_default().push(index);
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::command::{UiScanStrategy, VisionOptions};
    use stats_alloc::{INSTRUMENTED_SYSTEM, Region};

    #[test]
    fn completed_com_cancellation_contexts_are_control_flow() {
        assert!(cancellation_context_is_gone(RPC_E_NO_CONTEXT));
        assert!(cancellation_context_is_gone(RPC_E_CALL_COMPLETE));
        assert!(!cancellation_context_is_gone(
            windows::Win32::Foundation::E_FAIL
        ));
    }

    fn target(x: f64, y: f64, name: &str, role: &str) -> UiTarget {
        UiTarget {
            rect: Rect::new(x, y, 40.0, 20.0),
            name: name.into(),
            role: role.into(),
            native_role: None,
        }
    }

    #[test]
    fn semantic_roles_map_to_control_types() {
        assert_eq!(control_types_for(&["button".into()]), [50000]);
        assert_eq!(control_types_for(&["link".into()]), [50005]);
    }

    #[test]
    fn raw_ids_pass_through_and_foreign_prefixes_are_dropped() {
        assert_eq!(
            control_types_for(&[
                "uia:50042".into(),
                "ax:AXButton".into(),
                "atspi:push button".into(),
            ]),
            [50042]
        );
    }

    #[test]
    fn foreign_only_roles_trigger_windows_default_fallback() {
        let roles = vec!["ax:AXButton".into(), "atspi:push button".into()];
        assert!(control_types_for(&roles).is_empty());
        assert!(DEFAULT_CONTROL_TYPES.contains(&50000));
        assert!(DEFAULT_CONTROL_TYPES.contains(&50005));
        assert!(DEFAULT_CONTROL_TYPES.contains(&50019));
    }

    #[test]
    fn semantic_roles_are_case_and_whitespace_tolerant() {
        assert_eq!(control_types_for(&["  BUTTON  ".into()]), [50000]);
        assert_eq!(control_types_for(&["UIA:50042".into()]), [50042]);
    }

    #[test]
    fn spatial_deduper_keeps_overlapping_distinct_controls() {
        let mut deduper = SpatialDeduper::new(8.0);
        assert!(deduper.insert(&target(10.0, 10.0, "Save", "button")));
        assert!(!deduper.insert(&target(11.0, 11.0, "Save", "button")));
        assert!(deduper.insert(&target(11.0, 11.0, "Cancel", "button")));
        assert!(deduper.insert(&target(11.0, 11.0, "Save", "link")));
    }

    #[test]
    fn spatial_deduper_handles_extreme_native_coordinates() {
        let mut deduper = SpatialDeduper::new(8.0);
        let coordinate = f64::from(i32::MAX) * 8.0;
        assert!(deduper.insert(&target(coordinate, coordinate, "Max", "button")));
        let coordinate = f64::from(i32::MIN) * 8.0;
        assert!(deduper.insert(&target(coordinate, coordinate, "Min", "button")));
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn deduper_inline_buckets_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        const BUCKETS: usize = 100;

        fn fill_inline() {
            let mut cells: HashMap<usize, SmallVec<[usize; 2]>> = HashMap::new();
            for index in 0..BUCKETS {
                cells.entry(index).or_default().push(index);
            }
            std::hint::black_box(cells);
        }

        fn fill_heap() {
            let mut cells: HashMap<usize, Vec<usize>> = HashMap::new();
            for index in 0..BUCKETS {
                cells.entry(index).or_default().push(index);
            }
            std::hint::black_box(cells);
        }

        fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        for _ in 0..WARMUP {
            fill_inline();
            fill_heap();
        }
        let mut inline_samples = Vec::with_capacity(SAMPLES);
        let mut heap_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: fn(), samples: &mut Vec<u128>| {
                let started = Instant::now();
                operation();
                samples.push(started.elapsed().as_nanos());
            };
            if sample % 2 == 0 {
                measure(fill_inline, &mut inline_samples);
                measure(fill_heap, &mut heap_samples);
            } else {
                measure(fill_heap, &mut heap_samples);
                measure(fill_inline, &mut inline_samples);
            }
        }

        let inline_region = Region::new(&INSTRUMENTED_SYSTEM);
        fill_inline();
        let inline_allocations = inline_region.change().allocations;
        let heap_region = Region::new(&INSTRUMENTED_SYSTEM);
        fill_heap();
        let heap_allocations = heap_region.change().allocations;
        println!(
            "uia_deduper_bucket_probe samples={SAMPLES} buckets={BUCKETS} inline={:?} heap={:?} inline_allocations={inline_allocations} heap_allocations={heap_allocations}",
            percentiles(&mut inline_samples),
            percentiles(&mut heap_samples),
        );
    }

    #[test]
    fn atomic_scan_generation_cancels_stale_and_stopping_work() {
        let shared = SharedQueue::default();
        shared.latest_generation.store(7, Ordering::Release);
        assert!(is_current(&shared, 7));
        assert!(!is_current(&shared, 6));
        shared.latest_generation.store(8, Ordering::Release);
        assert!(!is_current(&shared, 7));
        shared.stopping.store(true, Ordering::Release);
        assert!(!is_current(&shared, 8));
    }

    #[test]
    fn popup_requires_an_owner_or_a_foreground_monitor() {
        assert!(popup_relationship_is_scannable(true, true, false));
        assert!(popup_relationship_is_scannable(true, false, true));
        assert!(!popup_relationship_is_scannable(true, false, false));
        assert!(!popup_relationship_is_scannable(false, true, true));
    }

    #[test]
    fn scan_timeout_is_bounded_for_plugin_requests() {
        assert_eq!(scan_timeout_ms(0), 250);
        assert_eq!(scan_timeout_ms(2_500), 2_500);
        assert_eq!(scan_timeout_ms(u64::MAX), 30_000);
    }

    #[test]
    fn timeout_configuration_is_reused_only_for_the_same_successful_value() {
        assert!(timeout_needs_configuration(None, 2_500));
        assert!(!timeout_needs_configuration(Some(2_500), 2_500));
        assert!(timeout_needs_configuration(Some(2_500), 5_000));
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn timeout_configuration_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        const CALLS_PER_SAMPLE: usize = 100;

        #[inline(never)]
        fn fake_com_setter(timeout: u32) {
            std::hint::black_box(timeout);
        }

        fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        let cached = || {
            if timeout_needs_configuration(Some(2_500), 2_500) {
                fake_com_setter(2_500);
                fake_com_setter(2_500);
            }
        };
        let repeated = || {
            fake_com_setter(2_500);
            fake_com_setter(2_500);
        };
        for _ in 0..WARMUP {
            cached();
            repeated();
        }

        let mut cached_samples = Vec::with_capacity(SAMPLES);
        let mut repeated_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: &dyn Fn(), samples: &mut Vec<u128>| {
                let started = Instant::now();
                for _ in 0..CALLS_PER_SAMPLE {
                    operation();
                }
                samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
            };
            if sample % 2 == 0 {
                measure(&cached, &mut cached_samples);
                measure(&repeated, &mut repeated_samples);
            } else {
                measure(&repeated, &mut repeated_samples);
                measure(&cached, &mut cached_samples);
            }
        }

        println!(
            "uia_timeout_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} cached={:?} repeated={:?}",
            percentiles(&mut cached_samples),
            percentiles(&mut repeated_samples),
        );
    }

    #[test]
    #[ignore = "manual native pointer-target benchmark; run release with --test-threads=1"]
    fn pointer_target_resolution_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;

        fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        for _ in 0..WARMUP {
            std::hint::black_box(window_under_pointer()).expect("native target resolution");
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            std::hint::black_box(window_under_pointer()).expect("native target resolution");
            samples.push(started.elapsed().as_nanos());
        }

        let allocation_region = Region::new(&INSTRUMENTED_SYSTEM);
        for _ in 0..SAMPLES {
            std::hint::black_box(window_under_pointer()).expect("native target resolution");
        }
        let allocations = allocation_region.change().allocations;
        println!(
            "windows_pointer_target_probe samples={SAMPLES} p50_p95_p99_ns={:?} allocations={allocations}",
            percentiles(&mut samples),
        );
        assert_eq!(
            allocations, 0,
            "the normal WindowFromPoint path must not allocate"
        );
    }

    #[test]
    fn native_provider_timeout_codes_are_retryable() {
        assert!(is_timeout_hresult(0x8013_1505_u32 as i32));
        assert!(is_timeout_hresult(0x8007_05B4_u32 as i32));
        assert!(is_timeout_hresult(0x8001_011F_u32 as i32));
        assert!(!is_timeout_hresult(0x8000_4005_u32 as i32));
    }

    #[test]
    fn higher_z_order_windows_hide_only_covered_target_centres() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 100.0);
        let item = target(20.0, 20.0, "Save", "button");
        let mut plan = test_scan_plan(UiScanRequest {
            id: 1,
            timeout_ms: 1_000,
            bounds: Some(bounds),
            roles: Vec::new(),
            max_depth: 8,
            visible_only: true,
            clickable_only: true,
            strategy: UiScanStrategy::AxTree,
            vision: VisionOptions::default(),
            app: None,
        });
        assert!(plan.target_center_is_visible(&item));
        let plan = Arc::get_mut(&mut plan).expect("test owns the only plan reference");
        plan.occluders = SmallVec::from_slice(&[Rect::new(30.0, 25.0, 20.0, 20.0)]);
        plan.windows[0].occluder_end = 1;
        assert!(!plan.target_center_is_visible(&item));
        plan.occluders = SmallVec::from_slice(&[Rect::new(0.0, 0.0, 20.0, 20.0)]);
        assert!(plan.target_center_is_visible(&item));
    }

    #[test]
    fn common_window_and_occluder_snapshot_stays_inline() {
        let mut windows = SmallVec::<[ScanWindow; MAX_SCAN_WINDOWS]>::new();
        let mut occluders = SmallVec::<[Rect; MAX_SCAN_WINDOWS]>::new();
        for index in 0..MAX_SCAN_WINDOWS {
            let bounds = Rect::new(index as f64, 0.0, 100.0, 100.0);
            windows.push(ScanWindow {
                hwnd: index as isize,
                bounds,
                occluder_end: index as u8,
            });
            occluders.push(bounds);
        }
        assert!(!windows.spilled());
        assert!(!occluders.spilled());
    }

    #[test]
    fn shared_scan_plan_has_one_generation_allocation() {
        let request = UiScanRequest {
            id: 2,
            timeout_ms: 1_000,
            bounds: Some(Rect::new(0.0, 0.0, 1_920.0, 1_080.0)),
            roles: Vec::new(),
            max_depth: 8,
            visible_only: true,
            clickable_only: true,
            strategy: UiScanStrategy::Hybrid,
            vision: VisionOptions::default(),
            app: None,
        };
        let allocation_region = Region::new(&INSTRUMENTED_SYSTEM);
        let plan = test_scan_plan(request);
        let allocations = allocation_region.change().allocations;
        std::hint::black_box(&plan);
        assert_eq!(allocations, 1, "only Arc<WindowsScanPlan> should allocate");
    }

    #[test]
    fn window_bounds_conversion_rejects_empty_rectangles() {
        assert_eq!(
            rect_from_native(RECT {
                left: -100,
                top: 20,
                right: 300,
                bottom: 220,
            }),
            Some(Rect::new(-100.0, 20.0, 400.0, 200.0))
        );
        assert!(rect_from_native(RECT::default()).is_none());
        assert_eq!(
            rect_from_native(RECT {
                left: i32::MIN,
                top: i32::MIN,
                right: i32::MAX,
                bottom: i32::MAX,
            }),
            Some(Rect::new(
                f64::from(i32::MIN),
                f64::from(i32::MIN),
                4_294_967_295.0,
                4_294_967_295.0,
            ))
        );
    }

    #[test]
    fn rejects_degenerate_and_container_targets() {
        let window = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let mut item = target(10.0, 10.0, "Save", "button");
        assert!(is_usable(&item, window));
        item.rect.width = 1.0;
        assert!(!is_usable(&item, window));
        item.rect = window;
        assert!(!is_usable(&item, window));
        item.rect = Rect::new(f64::NAN, 10.0, 40.0, 20.0);
        assert!(!is_usable(&item, window));
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn rect_filter_before_strings_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        const CALLS_PER_SAMPLE: usize = 100;

        fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        let within = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let rejected = Rect::new(2000.0, 2000.0, 40.0, 20.0);
        let early = || std::hint::black_box(rect_is_usable(&rejected, within));
        let late = || {
            let target = to_target(rejected, "Save changes".to_owned(), 50_000);
            std::hint::black_box(is_usable(&target, within))
        };
        for _ in 0..WARMUP {
            early();
            late();
        }
        let mut early_samples = Vec::with_capacity(SAMPLES);
        let mut late_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: &dyn Fn() -> bool, samples: &mut Vec<u128>| {
                let started = Instant::now();
                for _ in 0..CALLS_PER_SAMPLE {
                    std::hint::black_box(operation());
                }
                samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
            };
            if sample % 2 == 0 {
                measure(&early, &mut early_samples);
                measure(&late, &mut late_samples);
            } else {
                measure(&late, &mut late_samples);
                measure(&early, &mut early_samples);
            }
        }
        println!(
            "uia_rect_filter_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} early={:?} late={:?}",
            percentiles(&mut early_samples),
            percentiles(&mut late_samples),
        );
    }
}
