//! AppKit-owned main-loop integration for the engine.

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use objc2::MainThreadMarker;

use crate::Engine;
use crate::api::Backend;
use crate::app::errors::ErrorBundle;

use super::MacOsBackend;

const DRAIN_BUDGET: usize = 64;
const DORMANT_TIMER_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

thread_local! {
    static RUNTIME: RefCell<Option<ApplicationRuntime>> = const { RefCell::new(None) };
    static DRIVING: Cell<bool> = const { Cell::new(false) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimePhase {
    WaitingForLaunch,
    Running,
    Finished,
}

struct ApplicationRuntime {
    engine: Engine,
    backend: MacOsBackend,
    phase: RuntimePhase,
    result: Option<Result<(), String>>,
}

impl ApplicationRuntime {
    fn application_did_finish_launching(&mut self, mtm: MainThreadMarker) -> bool {
        if self.phase != RuntimePhase::WaitingForLaunch {
            return false;
        }
        self.backend.install_status_item(mtm);
        match self.engine.start_runtime(&mut self.backend) {
            Ok(()) => {
                self.phase = RuntimePhase::Running;
                false
            }
            Err(error) => {
                // `start_runtime` already performs backend shutdown on every
                // startup failure, so do not repeat cleanup or its logging.
                self.phase = RuntimePhase::Finished;
                self.result = Some(Err(error));
                true
            }
        }
    }

    fn drive(&mut self) -> bool {
        if self.phase != RuntimePhase::Running {
            return self.phase == RuntimePhase::Finished;
        }

        for turn_index in 0..DRAIN_BUDGET {
            match self
                .engine
                .run_runtime_turn(&mut self.backend, Duration::ZERO)
            {
                Ok(turn) if turn.should_quit => {
                    self.finish(Ok(()));
                    return true;
                }
                Ok(turn) => {
                    if !turn.event_processed {
                        let next_deadline = self
                            .engine
                            .runtime_next_deadline()
                            .into_iter()
                            .chain(self.backend.runtime_next_deadline())
                            .min();
                        match next_deadline {
                            Some(next_timeout) if next_timeout.is_zero() => {}
                            Some(next_timeout) => {
                                Self::schedule(next_timeout);
                                return false;
                            }
                            None => {
                                Self::schedule(DORMANT_TIMER_INTERVAL);
                                return false;
                            }
                        }
                    }
                    if turn_index + 1 == DRAIN_BUDGET {
                        Self::schedule(Duration::ZERO);
                        return false;
                    }
                }
                Err(error) => {
                    self.finish(Err(error));
                    return true;
                }
            }
        }
        false
    }

    fn finish(&mut self, result: Result<(), String>) {
        if self.phase == RuntimePhase::Finished {
            return;
        }
        self.phase = RuntimePhase::Finished;
        self.result = Some(self.engine.finish_runtime(&mut self.backend, result));
        Self::schedule(DORMANT_TIMER_INTERVAL);
    }

    fn finish_without_started_engine(&mut self, error: impl Into<String>) {
        if self.phase == RuntimePhase::Finished {
            return;
        }
        self.phase = RuntimePhase::Finished;
        let mut errors = ErrorBundle::default();
        errors.push("runtime", error);
        errors.record("backend shutdown", self.backend.shutdown());
        self.result = Some(errors.into_result());
        Self::schedule(DORMANT_TIMER_INTERVAL);
    }

    fn finish_after_unexpected_run_loop_exit(&mut self) {
        match self.phase {
            RuntimePhase::Running => {
                self.finish(Err(
                    "the AppKit application loop stopped unexpectedly".into()
                ));
            }
            RuntimePhase::WaitingForLaunch => self.finish_without_started_engine(
                "AppKit stopped before applicationDidFinishLaunching",
            ),
            RuntimePhase::Finished => {}
        }
    }

    fn schedule(timeout: Duration) {
        super::status_item::AppRuntimeDriver::schedule_current(timeout);
    }
}

pub(crate) fn run_application(engine: Engine, mut backend: MacOsBackend) -> Result<(), String> {
    let Some(mtm) = MainThreadMarker::new() else {
        return finish_unstarted_backend(
            &mut backend,
            "the AppKit application loop must run on the main thread",
        );
    };
    if let Err(error) = super::status_item::prepare_application(mtm) {
        return finish_unstarted_backend(&mut backend, error);
    }

    if RUNTIME.with(|slot| slot.borrow().is_some()) {
        return finish_unstarted_backend(
            &mut backend,
            "the macOS application runtime is already active",
        );
    }

    let driver = match super::status_item::AppRuntimeDriver::new(
        mtm,
        application_did_finish_launching_callback,
        drive_runtime_callback,
    ) {
        Ok(driver) => driver,
        Err(error) => return finish_unstarted_backend(&mut backend, error),
    };
    RUNTIME.with(|slot| {
        *slot.borrow_mut() = Some(ApplicationRuntime {
            engine,
            backend,
            phase: RuntimePhase::WaitingForLaunch,
            result: None,
        });
    });

    driver.run();
    drop(driver);

    RUNTIME.with(|slot| {
        let Some(mut runtime) = slot.borrow_mut().take() else {
            return Err("the AppKit application runtime disappeared".into());
        };
        runtime.finish_after_unexpected_run_loop_exit();
        runtime
            .result
            .take()
            .unwrap_or_else(|| Err("the AppKit application returned no result".into()))
    })
}

fn finish_unstarted_backend(
    backend: &mut MacOsBackend,
    error: impl Into<String>,
) -> Result<(), String> {
    let mut errors = ErrorBundle::default();
    errors.push("runtime", error);
    errors.record("backend shutdown", backend.shutdown());
    errors.into_result()
}

fn application_did_finish_launching() {
    let should_stop = RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(runtime) = slot.as_mut() else {
            return true;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            runtime.finish_without_started_engine(
                "applicationDidFinishLaunching ran off the main thread",
            );
            return true;
        };
        runtime.application_did_finish_launching(mtm)
    });
    if should_stop {
        stop_application_loop();
    } else {
        drive_runtime();
    }
}

fn drive_runtime() {
    let already_driving = DRIVING.with(|driving| driving.replace(true));
    if already_driving {
        return;
    }
    let should_stop = RUNTIME.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .is_some_and(ApplicationRuntime::drive)
    });
    DRIVING.with(|driving| driving.set(false));
    if should_stop {
        stop_application_loop();
    }
}

fn fail_runtime_after_callback_panic() {
    RUNTIME.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(runtime) = slot.as_mut() else {
            return;
        };
        if runtime.phase == RuntimePhase::Running {
            runtime.finish(Err("the AppKit engine callback panicked".into()));
        } else {
            runtime.finish_without_started_engine("the AppKit launch callback panicked");
        }
    });
    DRIVING.with(|driving| driving.set(false));
    stop_application_loop();
}

fn callback_boundary(callback: fn()) {
    if catch_unwind(AssertUnwindSafe(callback)).is_err() {
        // The first unwind released every RefCell borrow. Keep a second guard
        // so an unexpected cleanup bug can never unwind across Objective-C.
        let _ = catch_unwind(AssertUnwindSafe(fail_runtime_after_callback_panic));
    }
}

fn stop_application_loop() {
    super::status_item::AppRuntimeDriver::stop_current();
}

extern "C" fn application_did_finish_launching_callback() {
    callback_boundary(application_did_finish_launching);
}

extern "C" fn drive_runtime_callback() {
    callback_boundary(drive_runtime);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_budget_is_bounded() {
        assert!((1..=128).contains(&DRAIN_BUDGET));
    }

    #[test]
    fn dormant_timer_does_not_poll_frequently() {
        assert!(DORMANT_TIMER_INTERVAL >= Duration::from_secs(60));
    }
}
