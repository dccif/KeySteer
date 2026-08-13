//! Allocation-free rendezvous for synchronous native input callbacks.
//!
//! Native keyboard callbacks must wait for the engine's consume/forward
//! decision. A generation-tagged reusable slot avoids allocating a one-shot
//! channel for every physical key edge and prevents a late response from a
//! timed-out callback being observed by the next event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::api::backend::KeyDisposition;

#[derive(Default)]
struct Slot {
    generation: u64,
    disposition: Option<KeyDisposition>,
}

#[derive(Default)]
pub(super) struct DispositionMailbox {
    next_generation: AtomicU64,
    slot: Mutex<Slot>,
    ready: Condvar,
}

impl DispositionMailbox {
    /// Reserve the reusable slot for one native callback.
    pub(super) fn begin(&self) -> u64 {
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        slot.generation = generation;
        slot.disposition = None;
        generation
    }

    /// Complete `generation`; returns false when that callback already timed
    /// out and a newer event owns the slot.
    pub(super) fn complete(&self, generation: u64, disposition: KeyDisposition) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if slot.generation != generation || slot.disposition.is_some() {
            return false;
        }
        slot.disposition = Some(disposition);
        self.ready.notify_one();
        true
    }

    pub(super) fn wait(&self, generation: u64, timeout: Duration) -> Option<KeyDisposition> {
        let slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if slot.generation != generation {
            return None;
        }
        let (slot, _) = self
            .ready
            .wait_timeout_while(slot, timeout, |slot| {
                slot.generation == generation && slot.disposition.is_none()
            })
            .unwrap_or_else(|error| error.into_inner());
        (slot.generation == generation)
            .then_some(slot.disposition)
            .flatten()
    }

    /// Fail-open any callback currently waiting for the engine.
    ///
    /// Native hook shutdown and permission revocation must not wait for the
    /// normal disposition timeout. Advancing the generation makes the old
    /// waiter stale and the notification wakes it immediately.
    #[cfg(any(target_os = "macos", test))]
    pub(super) fn cancel_pending(&self) {
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        slot.generation = generation;
        slot.disposition = None;
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_completion_cannot_pollute_the_next_generation() {
        let mailbox = DispositionMailbox::default();
        let first = mailbox.begin();
        let second = mailbox.begin();

        assert!(!mailbox.complete(first, KeyDisposition::Consume));
        assert!(mailbox.complete(second, KeyDisposition::Forward));
        assert_eq!(
            mailbox.wait(second, Duration::ZERO),
            Some(KeyDisposition::Forward)
        );
    }

    #[test]
    fn timeout_fails_open_without_invalidating_future_events() {
        let mailbox = DispositionMailbox::default();
        let first = mailbox.begin();
        assert_eq!(mailbox.wait(first, Duration::ZERO), None);

        let second = mailbox.begin();
        assert!(mailbox.complete(second, KeyDisposition::Consume));
        assert_eq!(
            mailbox.wait(second, Duration::ZERO),
            Some(KeyDisposition::Consume)
        );
    }

    #[test]
    fn cancellation_invalidates_a_pending_callback() {
        let mailbox = DispositionMailbox::default();
        let generation = mailbox.begin();
        mailbox.cancel_pending();

        assert_eq!(mailbox.wait(generation, Duration::ZERO), None);
        assert!(!mailbox.complete(generation, KeyDisposition::Consume));
    }
}
