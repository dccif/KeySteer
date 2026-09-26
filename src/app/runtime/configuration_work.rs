//! Serialized, bounded configuration I/O. Only validated candidates return to Engine.
use super::*;
use crate::support::worker::WorkerJoin;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

pub(super) enum Operation {
    Reload,
    Set { path: String, value: String },
}
impl Operation {
    fn run(
        self,
        repository: &dyn ConfigurationRepository,
    ) -> Result<ConfigurationCandidate, String> {
        match self {
            Self::Reload => repository.reload_candidate(),
            Self::Set { path, value } => repository.set_candidate(&path, &value),
        }
    }
}
type Job = (
    Operation,
    Box<dyn ConfigurationRepository>,
    Arc<dyn Fn(BackendEvent) + Send + Sync>,
);
struct Worker {
    sender: Option<SyncSender<Job>>,
    results: Receiver<(bool, Result<ConfigurationCandidate, String>)>,
    join: WorkerJoin,
}
#[derive(Default)]
pub(super) struct ConfigurationWork {
    worker: Option<Worker>,
    pending: VecDeque<Operation>,
    active: bool,
}

/// Allocated only after a successful Reload; keep restart state off the Engine layout.
struct RestartRepository {
    source: Box<dyn ConfigurationRepository>,
    restart: Option<Box<dyn PreparedRestart>>,
}
impl ConfigurationRepository for RestartRepository {
    fn take_restart(&mut self) -> Option<Box<dyn PreparedRestart>> {
        self.restart.take()
    }
    fn source_text(&self) -> Result<String, String> {
        self.source.source_text()
    }
    fn source_path(&self) -> Option<std::path::PathBuf> {
        self.source.source_path()
    }
    fn reload_candidate(&self) -> Result<ConfigurationCandidate, String> {
        self.source.reload_candidate()
    }
    fn set_candidate(&self, path: &str, value: &str) -> Result<ConfigurationCandidate, String> {
        self.source.set_candidate(path, value)
    }
}
impl ConfigurationWork {
    #[cfg(test)]
    pub(super) fn assert_released(&self) {
        assert!(self.worker.is_none());
        assert!(!self.active);
        assert_eq!(self.pending.capacity(), 0);
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.pending = VecDeque::new();
        self.active = false;
        if let Some(worker) = &mut self.worker {
            worker.sender.take();
            worker.join.join_timeout(Duration::from_secs(2))?;
        }
        self.worker = None;
        Ok(())
    }
}
impl Engine {
    #[cold]
    #[inline(never)]
    pub(super) fn request_configuration(
        &mut self,
        operation: Operation,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        if self.configuration_work.pending.len() >= 8 {
            return Err("Configuration request queue is full".into());
        }
        self.configuration_work.pending.push_back(operation);
        self.start_configuration(backend)
    }
    #[cold]
    #[inline(never)]
    fn start_configuration(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.configuration_work.active || self.should_quit {
            return Ok(());
        }
        while let Some(operation) = self.configuration_work.pending.pop_front() {
            let repository = self
                .configuration
                .as_ref()
                .ok_or("no configuration source is attached")?;
            if let Some(emit) = backend.event_sink()
                && let Some(source) = repository.fork_for_worker()
            {
                if self.configuration_work.worker.is_none() {
                    let (sender, jobs) = sync_channel::<Job>(1);
                    let (done, results) = sync_channel(1);
                    let join = WorkerJoin::spawn(
                        "configuration-io",
                        std::thread::Builder::new().name("configuration-io".into()),
                        move || {
                            while let Ok((operation, source, emit)) = jobs.recv() {
                                let reload = matches!(operation, Operation::Reload);
                                let result = operation.run(source.as_ref());
                                // Release the parsed source/store before publishing completion.
                                drop(source);
                                if done.send((reload, result)).is_err() {
                                    break;
                                }
                                emit(BackendEvent::ConfigurationReady);
                            }
                        },
                    )?;
                    self.configuration_work.worker = Some(Worker {
                        sender: Some(sender),
                        results,
                        join,
                    });
                }
                self.configuration_work
                    .worker
                    .as_ref()
                    .ok_or("Configuration worker unavailable")?
                    .sender
                    .as_ref()
                    .ok_or("Configuration worker stopped")?
                    .try_send((operation, source, emit))
                    .map_err(|_| "Configuration worker unavailable")?;
                self.configuration_work.active = true;
                return Ok(());
            }
            let reload = matches!(operation, Operation::Reload);
            let result = operation
                .run(repository.as_ref())
                .and_then(|candidate| self.accept_configuration(candidate, backend));
            if let Err(error) = result {
                if reload {
                    self.configuration_work.shutdown()?;
                }
                return Err(format!(
                    "configuration change rejected; keeping the last valid configuration: {error}"
                ));
            }
        }
        self.configuration_work.shutdown()
    }
    #[cold]
    #[inline(never)]
    fn accept_configuration(
        &mut self,
        candidate: ConfigurationCandidate,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let ConfigurationCandidate {
            plan,
            repository,
            source_path,
            restart,
        } = candidate;
        if let Some(restart) = restart {
            let source = self
                .configuration
                .take()
                .ok_or("no configuration source is attached")?;
            self.configuration = Some(Box::new(RestartRepository {
                source,
                restart: Some(restart),
            }));
            // Retire deferred work here, on the cold reload path. Keep native
            // sessions/scans/latched inputs for finish_runtime to cancel/release.
            self.scheduler.timers.clear();
            self.scheduler.sequences.clear();
            self.input.pending_long_press_toggles.clear();
            self.input.drag_auto_release.clear();
            self.quick_switch.pending = None;
            self.should_quit = true;
            self.configuration_work.pending.clear();
            return Ok(());
        }
        self.apply_runtime_plan(plan, backend)?;
        self.configuration = Some(repository);
        if let Some(path) = source_path {
            crate::log_info!("config", "configuration loaded from {}", path.display());
        } else {
            crate::log_info!("config", "using built-in defaults");
        }
        Ok(())
    }
    #[cold]
    #[inline(never)]
    pub(super) fn finish_configuration(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let Some(worker) = &self.configuration_work.worker else {
            return Ok(());
        };
        let Ok((reload, result)) = worker.results.try_recv() else {
            return Ok(());
        };
        self.configuration_work.active = false;
        if let Err(error) =
            result.and_then(|candidate| self.accept_configuration(candidate, backend))
        {
            if reload {
                // Failed Reload is a transaction boundary: discard follow-up
                // requests and release the worker, channels and queue storage.
                self.configuration_work.pending = VecDeque::new();
            }
            crate::report_error!(
                "config",
                "configuration change rejected; keeping the last valid configuration: {error}"
            );
        }
        if self.configuration_work.pending.is_empty() || self.should_quit {
            self.configuration_work.shutdown()
        } else {
            self.start_configuration(backend)
        }
    }
}
