//! Timer and delayed-sequence state, deadline calculation, and due-work dispatch.

use super::*;

/// A pending timer request from a mode.
#[derive(Debug, Clone)]
pub(super) struct Timer {
    pub(super) fires_at: Instant,
    pub(super) last_fired: Instant,
    pub(super) interval: Option<Duration>,
    pub(super) owner: ModeId,
}

#[derive(Debug, Clone)]
pub(super) struct PendingSequence {
    pub(super) fires_at: Instant,
    pub(super) actions: VecDeque<Binding>,
    pub(super) owner: ModeId,
    pub(super) input: crate::api::input::InputEvent,
}

#[derive(Debug, Default)]
pub(super) struct Scheduler {
    pub(super) window_sessions: BTreeMap<u64, ModeId>,
    pub(super) sequences: Vec<PendingSequence>,
    pub(super) timers: HashMap<String, Timer>,
    pub(super) frame_clock_owner: Option<ModeId>,
}

impl Scheduler {
    pub(super) fn reset(&mut self) {
        self.window_sessions.clear();
        self.sequences.clear();
        self.timers.clear();
        self.frame_clock_owner = None;
    }

    pub(super) fn cancel_owner(&mut self, owner: &ModeId) {
        self.sequences.retain(|sequence| &sequence.owner != owner);
        self.timers.retain(|_, timer| &timer.owner != owner);
    }

    pub(super) fn cancel_timers_for_owner(&mut self, owner: &ModeId) {
        self.timers.retain(|_, timer| &timer.owner != owner);
    }
}

impl Engine {
    pub(super) fn next_timeout(&self) -> Duration {
        const MAX: Duration = Duration::from_millis(50);
        if self.scheduler.timers.is_empty()
            && self.scheduler.sequences.is_empty()
            && self.input.pending_long_press_toggles.is_empty()
            && self.input.drag_auto_release.fires_at.is_none()
        {
            return MAX;
        }
        let now = Instant::now();
        self.scheduler
            .timers
            .values()
            .map(|t| t.fires_at)
            .chain(
                self.scheduler
                    .sequences
                    .last()
                    .map(|sequence| sequence.fires_at),
            )
            .chain(
                self.input
                    .pending_long_press_toggles
                    .last()
                    .map(|pending| pending.fires_at),
            )
            .chain(self.input.drag_auto_release.fires_at)
            .map(|fires_at| fires_at.saturating_duration_since(now))
            .min()
            .unwrap_or(MAX)
            .min(MAX)
    }

    pub(super) fn fire_due_sequences(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.scheduler.sequences.is_empty() {
            return Ok(());
        }
        let now = Instant::now();
        while self
            .scheduler
            .sequences
            .last()
            .is_some_and(|sequence| sequence.fires_at <= now)
        {
            let Some(sequence) = self.scheduler.sequences.pop() else {
                break;
            };
            if let Err(error) =
                self.continue_sequence(sequence.actions, sequence.owner, sequence.input, backend)
            {
                crate::support::logging::report_error(
                    "action",
                    format!("delayed action sequence stopped: {error}"),
                );
            }
        }
        Ok(())
    }

    pub(super) fn fire_due_timers(&mut self, backend: &mut dyn Backend) -> Result<(), String> {
        if self.scheduler.timers.is_empty() {
            return Ok(());
        }
        let now = Instant::now();
        let mut due: Vec<String> = self
            .scheduler
            .timers
            .iter()
            .filter(|(_, t)| t.fires_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        due.sort();

        for id in due {
            let Some(owner) = self
                .scheduler
                .timers
                .get(&id)
                .map(|timer| timer.owner.clone())
            else {
                continue;
            };
            let elapsed = self
                .scheduler
                .timers
                .get(&id)
                .map(|timer| now.saturating_duration_since(timer.last_fired))
                .unwrap_or_default();
            match self.scheduler.timers.get_mut(&id) {
                Some(timer) => match timer.interval {
                    // Re-arm from now to avoid drift storms after a stall.
                    Some(interval) => {
                        timer.last_fired = now;
                        timer.fires_at = now + interval;
                    }
                    None => {
                        self.scheduler.timers.remove(&id);
                    }
                },
                None => continue,
            }
            self.trace_lazy(self.settings.debug.timers, "timer", || {
                format!("fire id={id:?} owner={} elapsed={elapsed:?}", owner)
            });
            self.dispatch_to(&owner, ModeEvent::Timer { id, elapsed }, backend)?;
        }
        Ok(())
    }
}
