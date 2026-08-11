use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use objc2::rc::autoreleasepool;

use crate::api::backend::BackendEvent;
use crate::api::command::{UiScanRequest, UiScanResult, UiScanStatus, UiScanStrategy};

use super::{EventSender, accessibility, vision};

static LATEST_SCAN: AtomicU64 = AtomicU64::new(0);
static SCAN_QUEUE: OnceLock<Result<Arc<ScanQueue>, String>> = OnceLock::new();

struct ScanJob {
    request: UiScanRequest,
    sender: EventSender,
}

struct ScanQueue {
    pending: Mutex<Option<ScanJob>>,
    ready: Condvar,
}

impl ScanQueue {
    fn start() -> Result<Arc<Self>, String> {
        let queue = Arc::new(Self {
            pending: Mutex::new(None),
            ready: Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        std::thread::Builder::new()
            .name("keysteer-ui-scan".into())
            .spawn(move || worker_queue.run())
            .map_err(|error| format!("Cannot start UI scan worker: {error}"))?;
        Ok(queue)
    }

    fn submit(&self, job: ScanJob) -> Option<ScanJob> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let replaced = pending.replace(job);
        self.ready.notify_one();
        replaced
    }

    fn run(&self) {
        loop {
            let job = {
                let mut pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                while pending.is_none() {
                    pending = self
                        .ready
                        .wait(pending)
                        .unwrap_or_else(|error| error.into_inner());
                }
                let Some(job) = pending.take() else {
                    continue;
                };
                job
            };
            autoreleasepool(|_| run_scan(job));
        }
    }
}

pub(super) fn request_scan(request: UiScanRequest, sender: EventSender) {
    LATEST_SCAN.store(request.id, Ordering::Release);
    vision::mark_latest(request.id);
    let request_id = request.id;
    let queue = match SCAN_QUEUE.get_or_init(ScanQueue::start) {
        Ok(queue) => queue,
        Err(error) => {
            let _ = sender.send(scan_result(
                request_id,
                Vec::new(),
                UiScanStatus::Failed(error.clone()),
            ));
            return;
        }
    };
    if let Some(replaced) = queue.submit(ScanJob { request, sender }) {
        let _ = replaced.sender.send(scan_result(
            replaced.request.id,
            Vec::new(),
            UiScanStatus::ContextChanged,
        ));
    }
}

fn run_scan(job: ScanJob) {
    let request_id = job.request.id;
    let original_pid = accessibility::frontmost_pid();
    let request_context_changed = job
        .request
        .app
        .as_ref()
        .is_some_and(|app| Some(app.process_id as libc::pid_t) != original_pid);
    if request_context_changed {
        let _ = job.sender.send(scan_result(
            request_id,
            Vec::new(),
            UiScanStatus::ContextChanged,
        ));
        return;
    }

    let Some(pid) = original_pid else {
        let _ = job.sender.send(scan_result(
            request_id,
            Vec::new(),
            UiScanStatus::Failed("No frontmost application is available".into()),
        ));
        return;
    };

    let status = match scan_sources(job.request.strategy) {
        (true, false) => stream_ax(&job, pid),
        (false, true) => stream_vision(&job, pid),
        (true, true) => std::thread::scope(|scope| {
            let ax = scope.spawn(|| autoreleasepool(|_| stream_ax(&job, pid)));
            let vision_status = stream_vision(&job, pid);
            let ax_status = ax
                .join()
                .unwrap_or_else(|_| UiScanStatus::Failed("AX scan worker panicked".into()));
            combined_status(ax_status, vision_status)
        }),
        (false, false) => unreachable!("every UI scan strategy has a source"),
    };

    let status = if scan_is_current(request_id, pid) {
        status
    } else {
        UiScanStatus::ContextChanged
    };
    let _ = job.sender.send(scan_result(request_id, Vec::new(), status));
}

fn scan_sources(strategy: UiScanStrategy) -> (bool, bool) {
    match strategy {
        UiScanStrategy::AxTree => (true, false),
        UiScanStrategy::Vision => (false, true),
        UiScanStrategy::Hybrid => (true, true),
    }
}

fn stream_ax(job: &ScanJob, pid: libc::pid_t) -> UiScanStatus {
    accessibility::scan_process_stream(
        pid,
        &job.request,
        || scan_id_is_current(job.request.id),
        |batch| send_partial(job, pid, batch),
    )
    .map(|_| UiScanStatus::Success)
    .unwrap_or_else(UiScanStatus::Failed)
}

fn stream_vision(job: &ScanJob, pid: libc::pid_t) -> UiScanStatus {
    let (targets, status) = scan_vision(pid, &job.request);
    let mut batch = Vec::with_capacity(16);
    for target in targets {
        batch.push(target);
        if batch.len() == 16 {
            send_partial(
                job,
                pid,
                std::mem::replace(&mut batch, Vec::with_capacity(16)),
            );
        }
    }
    if !batch.is_empty() {
        send_partial(job, pid, batch);
    }
    status
}

fn combined_status(ax: UiScanStatus, vision: UiScanStatus) -> UiScanStatus {
    match (&ax, &vision) {
        (UiScanStatus::Success, _) | (_, UiScanStatus::Success) => UiScanStatus::Success,
        _ => vision,
    }
}

fn scan_is_current(request_id: u64, pid: libc::pid_t) -> bool {
    scan_id_is_current(request_id) && accessibility::frontmost_pid() == Some(pid)
}

fn scan_id_is_current(request_id: u64) -> bool {
    LATEST_SCAN.load(Ordering::Acquire) == request_id
}

fn send_partial(job: &ScanJob, pid: libc::pid_t, mut targets: Vec<crate::api::UiTarget>) {
    if !scan_is_current(job.request.id, pid) {
        return;
    }
    if let Some(bounds) = job.request.bounds {
        targets.retain(|target| bounds.contains(&target.rect.center()));
    }
    if !targets.is_empty() {
        let _ = job
            .sender
            .send(scan_result(job.request.id, targets, UiScanStatus::Partial));
    }
}

fn scan_result(id: u64, targets: Vec<crate::api::UiTarget>, status: UiScanStatus) -> BackendEvent {
    BackendEvent::UiScanned(UiScanResult {
        id,
        targets,
        status,
    })
}

fn requested_window_bounds(
    window: crate::api::geometry::Rect,
    request: &UiScanRequest,
) -> Option<crate::api::geometry::Rect> {
    match request.bounds {
        Some(screen) => window.intersect(&screen),
        None => Some(window),
    }
}

fn scan_vision(
    pid: libc::pid_t,
    request: &UiScanRequest,
) -> (Vec<crate::api::UiTarget>, UiScanStatus) {
    let window_bounds = match accessibility::focused_window_bounds(pid) {
        Ok(bounds) => bounds,
        Err(error) => return (Vec::new(), UiScanStatus::Failed(error)),
    };
    let Some(bounds) = requested_window_bounds(window_bounds, request) else {
        return (Vec::new(), UiScanStatus::Success);
    };
    // Vision is deliberately executed on this one persistent worker. A timed
    // out native capture may finish late, but another full-resolution capture
    // can never overlap it and multiply memory consumption.
    vision::detect(request.id, bounds, &request.vision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::command::VisionOptions;

    fn request(id: u64) -> UiScanRequest {
        UiScanRequest {
            id,
            timeout_ms: 2_500,
            bounds: None,
            roles: Vec::new(),
            max_depth: 1,
            visible_only: true,
            clickable_only: true,
            strategy: UiScanStrategy::AxTree,
            vision: VisionOptions::default(),
            app: None,
        }
    }

    #[test]
    fn scan_strategies_use_only_their_configured_sources() {
        assert_eq!(scan_sources(UiScanStrategy::AxTree), (true, false));
        assert_eq!(scan_sources(UiScanStrategy::Vision), (false, true));
        assert_eq!(scan_sources(UiScanStrategy::Hybrid), (true, true));
    }

    #[test]
    fn hybrid_succeeds_when_either_source_succeeds() {
        assert_eq!(
            combined_status(
                UiScanStatus::Failed("AX failed".into()),
                UiScanStatus::Success,
            ),
            UiScanStatus::Success
        );
        assert_eq!(
            combined_status(
                UiScanStatus::Success,
                UiScanStatus::Failed("Vision failed".into()),
            ),
            UiScanStatus::Success
        );
    }

    #[test]
    fn vision_capture_is_clipped_to_the_requested_cursor_screen() {
        let mut request = request(1);
        request.bounds = Some(crate::api::geometry::Rect::new(1000.0, 0.0, 800.0, 600.0));
        assert_eq!(
            requested_window_bounds(
                crate::api::geometry::Rect::new(700.0, 100.0, 900.0, 400.0),
                &request,
            ),
            Some(crate::api::geometry::Rect::new(1000.0, 100.0, 600.0, 400.0))
        );
        assert_eq!(
            requested_window_bounds(
                crate::api::geometry::Rect::new(0.0, 100.0, 700.0, 400.0),
                &request,
            ),
            None
        );
    }

    #[test]
    fn pending_scan_slot_keeps_only_the_latest_request() {
        let queue = ScanQueue {
            pending: Mutex::new(None),
            ready: Condvar::new(),
        };
        let (sender, _receiver) = std::sync::mpsc::channel();
        let sender = EventSender::new(sender);
        assert!(
            queue
                .submit(ScanJob {
                    request: request(1),
                    sender: sender.clone(),
                })
                .is_none()
        );
        let replaced = queue
            .submit(ScanJob {
                request: request(2),
                sender,
            })
            .expect("the single slot should replace its old request");
        assert_eq!(replaced.request.id, 1);
        assert_eq!(
            queue.pending.lock().unwrap().as_ref().unwrap().request.id,
            2
        );
    }
}
