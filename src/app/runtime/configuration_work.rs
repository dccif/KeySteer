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
    results: Receiver<Result<ConfigurationCandidate, String>>,
    join: WorkerJoin,
}
#[derive(Default)]
pub(super) struct ConfigurationWork {
    worker: Option<Worker>,
    pending: VecDeque<Operation>,
    active: bool,
}
impl ConfigurationWork {
    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        self.pending.clear();
        if let Some(worker) = &mut self.worker {
            worker.sender.take();
            worker.join.join_timeout(Duration::from_secs(2))?;
        }
        self.worker = None;
        Ok(())
    }
}
impl Engine {
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
    fn start_configuration(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.configuration_work.active {
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
                                if done.send(operation.run(source.as_ref())).is_err() {
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
            let candidate = operation.run(repository.as_ref()).map_err(|error| {
                format!(
                    "configuration change rejected; keeping the last valid configuration: {error}"
                )
            })?;
            self.accept_configuration(candidate, backend)?;
        }
        Ok(())
    }
    fn accept_configuration(
        &mut self,
        candidate: ConfigurationCandidate,
        backend: &mut dyn Backend,
    ) -> Result<(), String> {
        let ConfigurationCandidate {
            plan,
            repository,
            source_path,
        } = candidate;
        self.apply_runtime_plan(plan, backend)?;
        self.configuration = Some(repository);
        if let Some(path) = source_path {
            crate::log_info!("config", "configuration loaded from {}", path.display());
        } else {
            crate::log_info!("config", "using built-in defaults");
        }
        Ok(())
    }
    pub(super) fn finish_configuration(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        let Some(worker) = &self.configuration_work.worker else {
            return Ok(());
        };
        let Ok(result) = worker.results.try_recv() else {
            return Ok(());
        };
        self.configuration_work.active = false;
        if let Err(error) =
            result.and_then(|candidate| self.accept_configuration(candidate, backend))
        {
            crate::report_error!(
                "config",
                "configuration change rejected; keeping the last valid configuration: {error}"
            );
        }
        self.start_configuration(backend)
    }
}
