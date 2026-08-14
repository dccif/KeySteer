//! Display-synchronised animation ticks backed by the Windows compositor.
//!
//! Windows 11's display-independent compositor clock handles mixed-refresh
//! screens and accepts a stop event, so shutdown and mode changes do not wait
//! for another VBlank. Windows 10 falls back to the cursor monitor's DXGI
//! VBlank, then `DwmFlush`. A one-slot channel coalesces frames when the engine
//! is busy, but elapsed wall-clock time is accumulated so coalescing never
//! loses movement distance.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use crate::app::worker::WorkerJoin;

use super::{WAKE_MESSAGE, native};

/// Let the Windows 10 DXGI fallback observe dynamic-refresh changes where the
/// optional Windows 11 export exists. The compositor-clock path observes its
/// native cadence directly.
pub fn prefer_dynamic_vblank() {
    native::prefer_dynamic_vblank();
}

pub struct DisplayFrameClock {
    owner_thread: u32,
    running: Arc<AtomicBool>,
    target_monitor: Arc<AtomicIsize>,
    uses_target_monitor: Arc<AtomicBool>,
    compositor_token: Arc<AtomicIsize>,
    receiver: Option<Receiver<Duration>>,
    worker: Option<WorkerJoin>,
}

const CLOCK_STOP_TIMEOUT: Duration = Duration::from_secs(2);

impl DisplayFrameClock {
    pub fn new(owner_thread: u32) -> Self {
        Self {
            owner_thread,
            running: Arc::new(AtomicBool::new(false)),
            target_monitor: Arc::new(AtomicIsize::new(0)),
            uses_target_monitor: Arc::new(AtomicBool::new(true)),
            compositor_token: Arc::new(AtomicIsize::new(0)),
            receiver: None,
            worker: None,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.is_running() {
            return Ok(());
        }
        let composition_enabled = native::dwm_composition_enabled()
            .map_err(|error| format!("cannot query DWM composition: {error}"))?;
        if !composition_enabled {
            return Err(
                "DWM composition is disabled; display-synchronised frames are unavailable".into(),
            );
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let running = Arc::clone(&self.running);
        let target_monitor = Arc::clone(&self.target_monitor);
        let uses_target_monitor = Arc::clone(&self.uses_target_monitor);
        let compositor_token = Arc::clone(&self.compositor_token);
        let owner_thread = self.owner_thread;
        self.uses_target_monitor.store(true, Ordering::Release);
        running.store(true, Ordering::Release);
        let worker = WorkerJoin::spawn(
            "Windows display clock",
            std::thread::Builder::new().name("keysteer-display-clock".into()),
            move || {
                // The worker owns the native stop event. If shutdown reaches
                // its deadline, the detached worker therefore cannot observe a
                // closed or recycled HANDLE; process exit remains the fallback.
                let compositor_signal = native::CompositorClockSignal::try_new();
                uses_target_monitor.store(compositor_signal.is_none(), Ordering::Release);
                let stop_event = compositor_signal
                    .as_ref()
                    .map(native::CompositorClockSignal::token);
                compositor_token.store(stop_event.unwrap_or_default(), Ordering::Release);
                run_clock(
                    running,
                    target_monitor,
                    uses_target_monitor,
                    stop_event,
                    sender,
                    owner_thread,
                );
                compositor_token.store(0, Ordering::Release);
            },
        )
        .inspect_err(|_| {
            self.running.store(false, Ordering::Release);
        })?;
        self.receiver = Some(receiver);
        self.worker = Some(worker);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.running.store(false, Ordering::Release);
        let compositor_token = self.compositor_token.load(Ordering::Acquire);
        if compositor_token != 0 && !native::interrupt_compositor_clock(compositor_token) {
            crate::app::logging::report_error(
                "windows-frame-clock",
                "cannot interrupt compositor frame wait",
            );
        }
        if let Some(worker) = self.worker.as_mut() {
            worker.join_timeout(CLOCK_STOP_TIMEOUT)?;
            self.worker.take();
        }
        self.compositor_token.store(0, Ordering::Release);
        self.receiver = None;
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Select the native output containing the pointer. MonitorFromPoint is a
    /// topology lookup, not a refresh-rate query; the clock still waits on the
    /// display hardware for each update.
    pub fn retarget(&self, x: f64, y: f64) {
        if self.uses_target_monitor.load(Ordering::Acquire) {
            self.target_monitor
                .store(native::monitor_for_point(x, y), Ordering::Release);
        }
    }

    pub fn try_next(&self) -> Option<Duration> {
        self.receiver.as_ref()?.try_recv().ok()
    }
}

impl Drop for DisplayFrameClock {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            crate::app::logging::report_error("windows-frame-clock", &error);
        }
    }
}

/// Balance the process-wide dynamic-refresh request on every worker exit,
/// including early error returns and debug-build unwinding.
struct CompositorBoost(bool);

impl CompositorBoost {
    fn request(enabled: bool) -> Self {
        Self(enabled && native::boost_compositor_clock(true))
    }

    fn disable(&mut self) {
        if self.0 {
            let _ = native::boost_compositor_clock(false);
            self.0 = false;
        }
    }
}

impl Drop for CompositorBoost {
    fn drop(&mut self) {
        self.disable();
    }
}

fn run_clock(
    running: Arc<AtomicBool>,
    target_monitor: Arc<AtomicIsize>,
    uses_target_monitor: Arc<AtomicBool>,
    mut compositor_stop: Option<isize>,
    sender: SyncSender<Duration>,
    owner_thread: u32,
) {
    let mut selected_monitor = isize::MIN;
    let mut selected_output = None;
    let mut last_delivered = Instant::now();
    let mut compositor_boost =
        CompositorBoost::request(compositor_stop.is_some() && running.load(Ordering::Acquire));
    while running.load(Ordering::Acquire) {
        let waited = if let Some(stop_event) = compositor_stop {
            match native::wait_for_compositor_frame(stop_event) {
                native::CompositorWait::Frame => Ok(()),
                native::CompositorWait::Interrupted => break,
                native::CompositorWait::Failed => {
                    crate::app::logging::report_error(
                        "windows-frame-clock",
                        "compositor clock failed; falling back to output VBlank",
                    );
                    compositor_boost.disable();
                    compositor_stop = None;
                    uses_target_monitor.store(true, Ordering::Release);
                    continue;
                }
            }
        } else {
            let requested_monitor = target_monitor.load(Ordering::Acquire);
            if requested_monitor != selected_monitor {
                selected_monitor = requested_monitor;
                selected_output = if requested_monitor == 0 {
                    None
                } else {
                    match native::display_output_for_monitor(requested_monitor) {
                        Ok(output) => output,
                        Err(error) => {
                            crate::app::logging::report_error("windows-frame-clock", error);
                            None
                        }
                    }
                };
            }

            if let Some(output) = selected_output.as_ref() {
                output.wait_for_vblank().map_err(|error| error.to_string())
            } else {
                native::wait_for_dwm_frame().map_err(|error| error.to_string())
            }
        };
        if let Err(error) = waited {
            if selected_output.take().is_some() {
                crate::app::logging::report_error(
                    "windows-frame-clock",
                    format!("output VBlank wait failed; falling back to DWM: {error}"),
                );
                continue;
            }
            crate::app::logging::report_error(
                "windows-frame-clock",
                format!("DwmFlush failed; frame delivery stopped: {error}"),
            );
            break;
        }
        let now = Instant::now();
        if !running.load(Ordering::Acquire) {
            break;
        }
        match deliver_elapsed(&sender, &mut last_delivered, now) {
            Ok(true) => {
                if let Err(error) = native::post_thread_wake(owner_thread, WAKE_MESSAGE) {
                    crate::app::logging::report_error(
                        "windows-frame-clock",
                        format!("cannot wake engine for display frame: {error}"),
                    );
                    break;
                }
            }
            Ok(false) => {}
            Err(()) => break,
        }
    }
    running.store(false, Ordering::Release);
}

/// Queue at most one frame while keeping the interval anchored to the last
/// successful delivery. If the slot is full, the next successful delivery
/// therefore includes every coalesced display interval.
fn deliver_elapsed(
    sender: &SyncSender<Duration>,
    last_delivered: &mut Instant,
    now: Instant,
) -> Result<bool, ()> {
    let elapsed = now.saturating_duration_since(*last_delivered);
    match sender.try_send(elapsed) {
        Ok(()) => {
            *last_delivered = now;
            Ok(true)
        }
        Err(TrySendError::Full(_)) => Ok(false),
        Err(TrySendError::Disconnected(_)) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_queue_is_bounded_and_coalesced_time_is_not_lost() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let start = Instant::now();
        let mut last_delivered = start;

        let first = start + Duration::from_millis(16);
        assert_eq!(
            deliver_elapsed(&sender, &mut last_delivered, first),
            Ok(true)
        );
        assert_eq!(last_delivered, first);

        let second = start + Duration::from_millis(33);
        assert_eq!(
            deliver_elapsed(&sender, &mut last_delivered, second),
            Ok(false)
        );
        assert_eq!(last_delivered, first);
        assert_eq!(receiver.try_recv().unwrap(), Duration::from_millis(16));

        let after_two_coalesced_frames = start + Duration::from_millis(64);
        assert_eq!(
            deliver_elapsed(&sender, &mut last_delivered, after_two_coalesced_frames),
            Ok(true)
        );
        assert_eq!(receiver.try_recv().unwrap(), Duration::from_millis(48));
    }

    #[test]
    fn compositor_path_does_not_query_a_target_monitor() {
        let clock = DisplayFrameClock::new(0);
        clock.target_monitor.store(123, Ordering::Release);
        clock.uses_target_monitor.store(false, Ordering::Release);
        clock.retarget(10.0, 20.0);
        assert_eq!(clock.target_monitor.load(Ordering::Acquire), 123);
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn compositor_retarget_performance_probe() {
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

        let clock = DisplayFrameClock::new(0);
        let skipped = || {
            clock.uses_target_monitor.store(false, Ordering::Release);
            clock.retarget(100.0, 100.0);
        };
        let queried = || {
            clock.uses_target_monitor.store(true, Ordering::Release);
            clock.retarget(100.0, 100.0);
        };
        for _ in 0..WARMUP {
            skipped();
            queried();
        }
        let mut skipped_samples = Vec::with_capacity(SAMPLES);
        let mut queried_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: &dyn Fn(), samples: &mut Vec<u128>| {
                let started = Instant::now();
                for _ in 0..CALLS_PER_SAMPLE {
                    operation();
                }
                samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
            };
            if sample % 2 == 0 {
                measure(&skipped, &mut skipped_samples);
                measure(&queried, &mut queried_samples);
            } else {
                measure(&queried, &mut queried_samples);
                measure(&skipped, &mut skipped_samples);
            }
        }
        println!(
            "windows_retarget_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} skipped={:?} queried={:?}",
            percentiles(&mut skipped_samples),
            percentiles(&mut queried_samples),
        );
    }
}
