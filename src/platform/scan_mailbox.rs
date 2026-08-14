//! Latest-generation mailbox for streamed UI scan results.
//!
//! Native providers may publish faster than the engine can redraw. Keeping a
//! single mergeable slot prevents stale partials from delaying the next scan
//! while preserving the first available batch wake-up.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::api::command::{UiScanResult, UiScanStatus};
use crate::api::geometry::UiTarget;

#[derive(Default)]
struct State {
    generation: u64,
    request_id: Option<u64>,
    targets: Vec<UiTarget>,
    terminal: Option<UiScanStatus>,
}

/// A bounded-by-request mailbox. At most the current scan's unconsumed targets
/// and one terminal status are retained.
#[derive(Default)]
pub(crate) struct ScanMailbox {
    next_generation: AtomicU64,
    state: Mutex<State>,
}

impl ScanMailbox {
    /// Start a new native generation and discard every unconsumed stale result.
    pub(crate) fn begin(&self, request_id: u64) -> u64 {
        let previous = self
            .next_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.wrapping_add(1).max(1))
            })
            .unwrap_or_else(|current| current);
        let generation = previous.wrapping_add(1).max(1);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.generation = generation;
        state.request_id = Some(request_id);
        state.targets = Vec::new();
        state.terminal = None;
        generation
    }

    /// Cancel only the currently matching public request.
    pub(crate) fn cancel(&self, request_id: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.request_id != Some(request_id) {
            return false;
        }
        state.request_id = None;
        state.targets = Vec::new();
        state.terminal = None;
        true
    }

    /// Merge one provider publication into the current slot.
    ///
    /// Returns `true` only when the mailbox transitioned from empty to ready,
    /// so callers need to wake the engine at most once before it drains the
    /// slot.
    pub(crate) fn publish(
        &self,
        generation: u64,
        request_id: u64,
        targets: Vec<UiTarget>,
        status: UiScanStatus,
    ) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.generation != generation || state.request_id != Some(request_id) {
            return false;
        }
        let was_ready = !state.targets.is_empty() || state.terminal.is_some();
        if state.targets.is_empty() {
            // Preserve the producer's allocation for the common first batch;
            // extending an empty Vec would allocate and move every target.
            state.targets = targets;
        } else {
            state.targets.extend(targets);
        }
        if status != UiScanStatus::Partial {
            state.terminal = Some(status);
        }
        !was_ready && (!state.targets.is_empty() || state.terminal.is_some())
    }

    /// Drain the accumulated partials. A terminal publication consumes the
    /// generation; a partial keeps it open for later batches.
    pub(crate) fn take(&self) -> Option<UiScanResult> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.targets.is_empty() && state.terminal.is_none() {
            return None;
        }
        let id = state.request_id?;
        let targets = std::mem::take(&mut state.targets);
        let status = state.terminal.take().unwrap_or(UiScanStatus::Partial);
        if status != UiScanStatus::Partial {
            state.request_id = None;
        }
        Some(UiScanResult {
            id,
            targets,
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::Rect;

    fn target(name: &str) -> UiTarget {
        UiTarget {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            name: name.into(),
            role: "button".into(),
            native_role: None,
        }
    }

    #[test]
    fn partials_merge_without_queueing_events() {
        let mailbox = ScanMailbox::default();
        let generation = mailbox.begin(7);
        assert!(mailbox.publish(generation, 7, vec![target("one")], UiScanStatus::Partial));
        assert!(!mailbox.publish(generation, 7, vec![target("two")], UiScanStatus::Partial));
        let result = mailbox.take().unwrap();
        assert_eq!(result.id, 7);
        assert_eq!(result.targets.len(), 2);
        assert_eq!(result.status, UiScanStatus::Partial);
        assert!(mailbox.take().is_none());
    }

    #[test]
    fn a_new_generation_drops_stale_work_even_when_ids_repeat() {
        let mailbox = ScanMailbox::default();
        let old = mailbox.begin(9);
        assert!(mailbox.publish(old, 9, vec![target("old")], UiScanStatus::Partial));
        let current = mailbox.begin(9);
        assert!(!mailbox.publish(old, 9, vec![target("stale")], UiScanStatus::Success));
        assert!(mailbox.publish(current, 9, vec![target("new")], UiScanStatus::Success));
        let result = mailbox.take().unwrap();
        assert_eq!(result.targets[0].name, "new");
        assert_eq!(result.status, UiScanStatus::Success);
    }

    #[test]
    fn cancellation_drops_ready_data_and_rejects_late_publications() {
        let mailbox = ScanMailbox::default();
        let generation = mailbox.begin(3);
        assert!(mailbox.publish(generation, 3, vec![target("ready")], UiScanStatus::Partial));
        assert!(mailbox.cancel(3));
        assert!(mailbox.take().is_none());
        assert_eq!(mailbox.state.lock().unwrap().targets.capacity(), 0);
        assert!(!mailbox.publish(generation, 3, vec![target("late")], UiScanStatus::Success));
    }

    #[test]
    fn one_hundred_rapid_sessions_never_expose_stale_targets() {
        let mailbox = ScanMailbox::default();
        for id in 1..=100 {
            let generation = mailbox.begin(id);
            assert!(mailbox.publish(
                generation,
                id,
                vec![target(&format!("session-{id}"))],
                UiScanStatus::Partial,
            ));
            assert!(mailbox.cancel(id));
            assert!(mailbox.take().is_none());
            assert!(!mailbox.publish(generation, id, vec![target("late")], UiScanStatus::Success,));
        }
    }
}
