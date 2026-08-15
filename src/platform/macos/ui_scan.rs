use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::api::command::{UiScanRequest, UiScanStatus, UiScanStrategy};
use crate::app::worker::WorkerJoin;
use crate::platform::partial_batcher::PartialBatcher;
use crate::platform::scan_mailbox::ScanMailbox;
use objc2::rc::autoreleasepool;

use super::{EventSender, accessibility, vision};

static LATEST_SCAN: AtomicU64 = AtomicU64::new(0);
const FIRST_PARTIAL_TARGETS: usize = 24;
const MAX_TARGETS: usize = 2_000;
const STOP_TIMEOUT: Duration = Duration::from_millis(500);

struct ScanJob {
    request: UiScanRequest,
    generation: u64,
    mailbox: Arc<ScanMailbox>,
    wake: EventSender,
}

impl ScanJob {
    fn publish(&self, targets: Vec<crate::api::UiTarget>, status: UiScanStatus) {
        if self
            .mailbox
            .publish(self.generation, self.request.id, targets, status)
        {
            self.wake.wake();
        }
    }
}

struct ScanQueue {
    state: Mutex<ScanQueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct ScanQueueState {
    pending: Option<ScanJob>,
    stopping: bool,
}

/// Backend-owned scan worker. It is created lazily on the first UIHint scan,
/// remains warm between scans, and is explicitly stopped when the backend
/// shuts down instead of relying on a process-static detached thread.
pub(super) struct UiScanWorker {
    queue: Option<Arc<ScanQueue>>,
    worker: Option<WorkerJoin>,
}

impl UiScanWorker {
    pub(super) fn new() -> Self {
        Self {
            queue: None,
            worker: None,
        }
    }

    pub(super) fn request_scan(
        &mut self,
        request: UiScanRequest,
        generation: u64,
        mailbox: Arc<ScanMailbox>,
        wake: EventSender,
    ) {
        LATEST_SCAN.store(generation, Ordering::Release);
        vision::mark_latest(generation);
        let queue = match self.ensure_started() {
            Ok(queue) => queue,
            Err(error) => {
                if mailbox.publish(
                    generation,
                    request.id,
                    Vec::new(),
                    UiScanStatus::Failed(error),
                ) {
                    wake.wake();
                }
                return;
            }
        };
        drop(queue.submit(ScanJob {
            request,
            generation,
            mailbox,
            wake,
        }));
    }

    pub(super) fn cancel_scan(&self, request_id: u64) {
        LATEST_SCAN.store(0, Ordering::Release);
        vision::mark_latest(0);
        if let Some(queue) = self.queue.as_ref() {
            queue.cancel(request_id);
        }
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        LATEST_SCAN.store(0, Ordering::Release);
        vision::mark_latest(0);
        if let Some(queue) = self.queue.as_ref() {
            queue.stop();
        }
        let Some(worker) = self.worker.as_mut() else {
            return Ok(());
        };
        worker.join_timeout(STOP_TIMEOUT)?;
        self.worker.take();
        self.queue.take();
        Ok(())
    }

    fn ensure_started(&mut self) -> Result<Arc<ScanQueue>, String> {
        if let Some(queue) = self.queue.as_ref() {
            return Ok(Arc::clone(queue));
        }
        let queue = Arc::new(ScanQueue {
            state: Mutex::new(ScanQueueState::default()),
            ready: Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        let worker = WorkerJoin::spawn(
            "macOS UI scan",
            std::thread::Builder::new().name("keysteer-ui-scan".into()),
            move || worker_queue.run(),
        )?;
        self.queue = Some(Arc::clone(&queue));
        self.worker = Some(worker);
        Ok(queue)
    }
}

impl Drop for UiScanWorker {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            crate::app::logging::report_error("macos-ui-scan", &error);
        }
    }
}

/// AX and Vision share this publisher in Hybrid mode, so their combined target
/// count follows one deterministic threshold sequence instead of each source
/// independently repainting the overlay.
struct PartialPublisher<'a> {
    job: &'a ScanJob,
    pid: libc::pid_t,
    batches: Mutex<PartialBatcher<crate::api::UiTarget>>,
}

impl<'a> PartialPublisher<'a> {
    fn new(job: &'a ScanJob, pid: libc::pid_t) -> Self {
        Self {
            job,
            pid,
            batches: Mutex::new(PartialBatcher::new(FIRST_PARTIAL_TARGETS, MAX_TARGETS)),
        }
    }

    fn push(&self, mut targets: Vec<crate::api::UiTarget>) {
        if !scan_is_current(self.job.generation, self.pid) {
            return;
        }
        if let Some(bounds) = self.job.request.bounds {
            targets.retain(|target| bounds.contains(&target.rect.center()));
        }
        if targets.is_empty() {
            return;
        }
        // Keep publication under the same short lock as threshold assignment.
        // In Hybrid mode this prevents the 48-target batch from overtaking the
        // 24-target batch after two sources cross thresholds concurrently.
        let mut batches = self
            .batches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let ready = batches.extend(targets);
        for batch in ready {
            self.send(batch);
        }
    }

    fn finish(&self) {
        let mut batches = self
            .batches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = batches.finish();
        if let Some(batch) = pending {
            self.send(batch);
        }
    }

    fn send(&self, targets: Vec<crate::api::UiTarget>) {
        if scan_is_current(self.job.generation, self.pid) {
            self.job.publish(targets, UiScanStatus::Partial);
        }
    }
}

impl ScanQueue {
    fn submit(&self, job: ScanJob) -> Option<ScanJob> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopping {
            return Some(job);
        }
        let replaced = state.pending.replace(job);
        self.ready.notify_one();
        replaced
    }

    fn cancel(&self, request_id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .pending
            .as_ref()
            .is_some_and(|job| job.request.id == request_id)
        {
            state.pending.take();
        }
    }

    fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.stopping = true;
        state.pending.take();
        self.ready.notify_all();
    }

    fn run(&self) {
        loop {
            let job = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                while state.pending.is_none() && !state.stopping {
                    state = self
                        .ready
                        .wait(state)
                        .unwrap_or_else(|error| error.into_inner());
                }
                if state.stopping {
                    return;
                }
                let Some(job) = state.pending.take() else {
                    continue;
                };
                job
            };
            autoreleasepool(|_| run_scan(job));
        }
    }
}

fn run_scan(job: ScanJob) {
    let original_pid = accessibility::frontmost_pid();
    let request_context_changed = job
        .request
        .app
        .as_ref()
        .is_some_and(|app| Some(app.process_id as libc::pid_t) != original_pid);
    if request_context_changed {
        job.publish(Vec::new(), UiScanStatus::ContextChanged);
        return;
    }

    let Some(pid) = original_pid else {
        job.publish(
            Vec::new(),
            UiScanStatus::Failed("No frontmost application is available".into()),
        );
        return;
    };

    let publisher = PartialPublisher::new(&job, pid);
    let status = match scan_sources(job.request.strategy) {
        (true, false) => stream_ax(&job, pid, &publisher),
        (false, true) => stream_vision(&job, pid, &publisher),
        (true, true) => std::thread::scope(|scope| {
            let ax = scope.spawn(|| autoreleasepool(|_| stream_ax(&job, pid, &publisher)));
            let vision_status = stream_vision(&job, pid, &publisher);
            let ax_status = ax
                .join()
                .unwrap_or_else(|_| UiScanStatus::Failed("AX scan worker panicked".into()));
            combined_status(ax_status, vision_status)
        }),
        (false, false) => UiScanStatus::Failed("UI scan strategy has no enabled source".into()),
    };

    let status = if scan_is_current(job.generation, pid) {
        publisher.finish();
        if scan_is_current(job.generation, pid) {
            status
        } else {
            UiScanStatus::ContextChanged
        }
    } else {
        UiScanStatus::ContextChanged
    };
    job.publish(Vec::new(), status);
}

fn scan_sources(strategy: UiScanStrategy) -> (bool, bool) {
    match strategy {
        UiScanStrategy::AxTree => (true, false),
        UiScanStrategy::Vision => (false, true),
        UiScanStrategy::Hybrid => (true, true),
    }
}

fn stream_ax(job: &ScanJob, pid: libc::pid_t, publisher: &PartialPublisher<'_>) -> UiScanStatus {
    accessibility::scan_process_stream(
        pid,
        &job.request,
        || scan_id_is_current(job.generation),
        |batch| publisher.push(batch),
    )
    .map(|_| UiScanStatus::Success)
    .unwrap_or_else(UiScanStatus::Failed)
}

fn stream_vision(
    job: &ScanJob,
    pid: libc::pid_t,
    publisher: &PartialPublisher<'_>,
) -> UiScanStatus {
    let (targets, status) = scan_vision(pid, job.generation, &job.request);
    publisher.push(targets);
    status
}

fn combined_status(ax: UiScanStatus, vision: UiScanStatus) -> UiScanStatus {
    match (&ax, &vision) {
        (UiScanStatus::Success, _) | (_, UiScanStatus::Success) => UiScanStatus::Success,
        _ => vision,
    }
}

fn scan_is_current(generation: u64, pid: libc::pid_t) -> bool {
    scan_id_is_current(generation) && accessibility::frontmost_pid() == Some(pid)
}

fn scan_id_is_current(generation: u64) -> bool {
    LATEST_SCAN.load(Ordering::Acquire) == generation
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
    generation: u64,
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
    vision::detect(generation, bounds, &request.vision)
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
            state: Mutex::new(ScanQueueState::default()),
            ready: Condvar::new(),
        };
        let (sender, _receiver) = std::sync::mpsc::channel();
        let wake = EventSender::new(sender);
        let mailbox = Arc::new(ScanMailbox::default());
        let first_generation = mailbox.begin(1);
        assert!(
            queue
                .submit(ScanJob {
                    request: request(1),
                    generation: first_generation,
                    mailbox: Arc::clone(&mailbox),
                    wake: wake.clone(),
                })
                .is_none()
        );
        let second_generation = mailbox.begin(2);
        let replaced = queue
            .submit(ScanJob {
                request: request(2),
                generation: second_generation,
                mailbox,
                wake,
            })
            .expect("the single slot should replace its old request");
        assert_eq!(replaced.request.id, 1);
        assert_eq!(
            queue
                .state
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap()
                .request
                .id,
            2
        );
    }

    #[test]
    fn stopping_the_scan_queue_drops_pending_work() {
        let queue = ScanQueue {
            state: Mutex::new(ScanQueueState::default()),
            ready: Condvar::new(),
        };
        let (sender, _receiver) = std::sync::mpsc::channel();
        let mailbox = Arc::new(ScanMailbox::default());
        queue.submit(ScanJob {
            request: request(7),
            generation: mailbox.begin(7),
            mailbox,
            wake: EventSender::new(sender),
        });

        queue.stop();

        let state = queue.state.lock().unwrap();
        assert!(state.stopping);
        assert!(state.pending.is_none());
    }
}
