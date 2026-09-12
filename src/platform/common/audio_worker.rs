//! Native audio has its own bounded worker: permission prompts and endpoint
//! discovery must not occupy the window/tab event pump.

use crate::api::BackendEvent;
use crate::api::audio::{AudioAction, AudioRequest, AudioResult};
use crate::support::worker::WorkerJoin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AudioProcess {
    pub pid: u32,
    /// Native process creation identity, checked again on the audio thread.
    pub started: u64,
}

pub(crate) trait AudioBackend {
    fn execute(
        &mut self,
        process: Option<AudioProcess>,
        action: AudioAction,
    ) -> Result<String, String>;
    fn maintain(&mut self) -> bool {
        false
    }
}
pub(crate) type AudioFactory = fn() -> Box<dyn AudioBackend>;
pub(crate) type EventSink = Arc<dyn Fn(BackendEvent) + Send + Sync>;

/// One completion boundary for native execution and identity/queue failures.
/// Successful commands do not format or write diagnostics on this path.
pub(super) fn publish_result(emit: &EventSink, result: AudioResult) {
    if let Err(error) = &result.outcome {
        crate::report_error!(
            "audio",
            "session={} request={}: {error}",
            result.session,
            result.id
        );
    }
    emit(BackendEvent::AudioResult(Box::new(result)));
}

struct Job {
    request: AudioRequest,
    process: Option<AudioProcess>,
    cancelled: Arc<AtomicBool>,
}
pub(crate) struct AudioWorker {
    sender: Option<mpsc::SyncSender<Job>>,
    stop: Arc<AtomicBool>,
    worker: WorkerJoin,
}
impl AudioWorker {
    pub fn start(create: AudioFactory, emit: EventSink) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(64);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = WorkerJoin::spawn(
            "audio",
            std::thread::Builder::new().name("keysteer-audio".into()),
            move || {
                let mut backend = create();
                loop {
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let job = if backend.maintain() {
                        match receiver.recv_timeout(Duration::from_millis(250)) {
                            Ok(job) => job,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match receiver.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    };
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if job.cancelled.load(Ordering::Acquire) {
                        continue;
                    }
                    let outcome = backend.execute(job.process, job.request.action);
                    if !job.cancelled.load(Ordering::Acquire) && !stopped.load(Ordering::Acquire) {
                        publish_result(
                            &emit,
                            AudioResult {
                                session: job.request.session,
                                id: job.request.id,
                                outcome,
                            },
                        );
                    }
                }
            },
        )?;
        Ok(Self {
            sender: Some(sender),
            stop,
            worker,
        })
    }
    pub fn submit(
        &self,
        request: AudioRequest,
        process: Option<AudioProcess>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), String> {
        self.sender
            .as_ref()
            .ok_or("audio worker stopped")?
            .try_send(Job {
                request,
                process,
                cancelled,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "audio operation queue is full".into(),
                mpsc::TrySendError::Disconnected(_) => "audio worker stopped".into(),
            })
    }
}
impl Drop for AudioWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.sender.take();
        if let Err(error) = self.worker.join_timeout(Duration::from_secs(2)) {
            crate::report_error!("audio", "{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::audio::AudioTarget;
    use std::sync::Mutex;

    #[test]
    fn failed_audio_returns_error_and_worker_can_process_next_request() {
        struct FailingAudio;
        impl AudioBackend for FailingAudio {
            fn execute(
                &mut self,
                _: Option<AudioProcess>,
                _: AudioAction,
            ) -> Result<String, String> {
                Err("injected audio failure".into())
            }
        }
        fn create() -> Box<dyn AudioBackend> {
            Box::new(FailingAudio)
        }
        let (events, received) = mpsc::channel();
        let worker = AudioWorker::start(
            create,
            Arc::new(move |event| {
                events.send(event).unwrap();
            }),
        )
        .unwrap();
        for id in [1, 2] {
            worker
                .submit(request(id), None, Arc::new(AtomicBool::new(false)))
                .unwrap();
            let BackendEvent::AudioResult(result) =
                received.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("unexpected event")
            };
            assert_eq!(result.id, id);
            assert_eq!(result.outcome, Err("injected audio failure".into()));
        }
    }
    type Gate = (mpsc::Sender<()>, mpsc::Receiver<()>);
    static GATE: Mutex<Option<Gate>> = Mutex::new(None);
    struct BlockingAudio {
        gate: Option<Gate>,
    }
    impl AudioBackend for BlockingAudio {
        fn execute(&mut self, _: Option<AudioProcess>, _: AudioAction) -> Result<String, String> {
            if let Some((entered, release)) = self.gate.take() {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            Ok("done".into())
        }
    }
    fn create() -> Box<dyn AudioBackend> {
        Box::new(BlockingAudio {
            gate: GATE.lock().unwrap().take(),
        })
    }
    fn request(id: u64) -> AudioRequest {
        AudioRequest {
            session: 1,
            id,
            target: AudioTarget::System,
            action: AudioAction::Up,
        }
    }
    #[test]
    fn slow_audio_is_async_bounded_and_cancellation_suppresses_queued_and_inflight_results() {
        let (entered, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        *GATE.lock().unwrap() = Some((entered, gate));
        let (results, received) = mpsc::channel();
        let worker = AudioWorker::start(
            create,
            Arc::new(move |e| {
                results.send(e).unwrap();
            }),
        )
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        worker.submit(request(0), None, cancelled.clone()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        // Native execution is still blocked, but all submissions return immediately.
        for id in 1..=63 {
            worker.submit(request(id), None, cancelled.clone()).unwrap();
        }
        worker
            .submit(request(64), None, Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!(worker.submit(request(65), None, cancelled.clone()).is_err());
        cancelled.store(true, Ordering::Release);
        release.send(()).unwrap();
        let BackendEvent::AudioResult(result) =
            received.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("unexpected event")
        };
        assert_eq!(result.id, 64);
        drop(worker);
        assert!(received.try_recv().is_err());
    }
}
