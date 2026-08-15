//! Shared ownership and bounded joining for background workers.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

#[must_use = "workers must be joined or transferred to an explicit quarantine owner"]
pub(crate) struct WorkerJoin {
    name: &'static str,
    finished: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl WorkerJoin {
    pub(crate) fn spawn(
        name: &'static str,
        builder: Builder,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let (finished_tx, finished) = mpsc::channel();
        let join = builder
            .spawn(move || {
                let _finished = WorkerFinished(finished_tx);
                work();
            })
            .map_err(|error| format!("cannot start {name}: {error}"))?;
        Ok(Self {
            name,
            finished,
            join: Some(join),
        })
    }

    pub(crate) fn reap_finished(&mut self) -> Result<bool, String> {
        if self.join.as_ref().is_some_and(JoinHandle::is_finished) {
            self.join_finished()?;
        }
        Ok(self.join.is_none())
    }

    #[cfg(any(target_os = "windows", test))]
    pub(crate) fn wait_ready<T>(
        &self,
        ready: &Receiver<T>,
        timeout: Duration,
    ) -> Result<T, String> {
        match ready.recv_timeout(timeout) {
            Ok(value) => Ok(value),
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "{} did not report readiness within {timeout:?}",
                self.name
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(format!("{} stopped before reporting readiness", self.name))
            }
        }
    }

    /// Join a cooperative worker within `timeout`.
    pub(crate) fn join_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        let now = Instant::now();
        let deadline = now.checked_add(timeout).unwrap_or(now);
        self.join_until(deadline)
    }

    /// Join a cooperative worker before a shared absolute `deadline`.
    ///
    /// A timeout deliberately retains the handle so shutdown code can retry
    /// the join or report a fatal shutdown failure without silently detaching
    /// a still-running KeySteer worker.
    pub(crate) fn join_until(&mut self, deadline: Instant) -> Result<(), String> {
        let Some(join) = self.join.as_ref() else {
            return Ok(());
        };
        if !join.is_finished() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if matches!(
                self.finished.recv_timeout(remaining),
                Err(RecvTimeoutError::Timeout)
            ) && !join.is_finished()
            {
                return Err(format!(
                    "{} did not stop before the shutdown deadline",
                    self.name
                ));
            }
        }
        self.join_finished()
    }

    fn join_finished(&mut self) -> Result<(), String> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join.join()
            .map_err(|_| format!("{} worker panicked", self.name))
    }
}

impl Drop for WorkerJoin {
    fn drop(&mut self) {
        if self.join.take().is_some() {
            crate::app::logging::report_error(
                "worker",
                format!("{} was dropped without a completed join", self.name),
            );
        }
    }
}

struct WorkerFinished(Sender<()>);

impl Drop for WorkerFinished {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_completed_worker() {
        let mut worker = WorkerJoin::spawn("test", Builder::new(), || {}).unwrap();
        worker.join_timeout(Duration::from_secs(1)).unwrap();
        assert!(worker.reap_finished().unwrap());
    }

    #[test]
    fn reports_panics() {
        let mut worker =
            WorkerJoin::spawn("panic-test", Builder::new(), || panic!("boom")).unwrap();
        assert!(worker.join_timeout(Duration::from_secs(1)).is_err());
    }

    #[test]
    fn timeout_is_an_explicit_shutdown_failure() {
        let (release_tx, release_rx) = mpsc::channel();
        let mut worker = WorkerJoin::spawn("slow-test", Builder::new(), move || {
            release_rx.recv().unwrap()
        })
        .unwrap();
        let error = worker.join_timeout(Duration::from_millis(1)).unwrap_err();
        assert!(error.contains("shutdown deadline"));
        assert!(
            worker.join.is_some(),
            "a timeout must retain the join handle"
        );
        release_tx.send(()).unwrap();
        worker.join_timeout(Duration::from_secs(1)).unwrap();
        assert!(worker.reap_finished().unwrap());
    }

    #[test]
    fn readiness_has_a_deadline() {
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let mut worker = WorkerJoin::spawn("not-ready", Builder::new(), move || {
            let _keep_connected = ready_tx;
            std::thread::sleep(Duration::from_millis(20));
        })
        .unwrap();
        let error = worker
            .wait_ready(&ready_rx, Duration::from_millis(1))
            .unwrap_err();
        assert!(error.contains("did not report readiness"));
        worker.join_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn startup_shutdown_performance_probe() {
        const SAMPLES: usize = 100;
        let mut elapsed = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = std::time::Instant::now();
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let mut worker = WorkerJoin::spawn("probe", Builder::new(), move || {
                let _ = ready_tx.send(());
            })
            .unwrap();
            worker
                .wait_ready(&ready_rx, Duration::from_secs(1))
                .unwrap();
            worker.join_timeout(Duration::from_secs(1)).unwrap();
            elapsed.push(started.elapsed().as_nanos());
        }
        elapsed.sort_unstable();
        println!(
            "worker_probe samples={SAMPLES} p50={}ns p95={}ns p99={}ns",
            elapsed[SAMPLES * 50 / 100],
            elapsed[SAMPLES * 95 / 100],
            elapsed[SAMPLES * 99 / 100],
        );
    }
}
