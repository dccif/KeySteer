#![forbid(unsafe_code)]

//! Allocation-free rendezvous for synchronous native input callbacks.
//!
//! Native keyboard callbacks must wait for the engine's consume/forward
//! decision. A generation-tagged reusable slot avoids allocating a one-shot
//! channel for every physical key edge and prevents a late response from a
//! timed-out callback being observed by the next event.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::api::backend::KeyDisposition;

#[derive(Default)]
struct Slot {
    generation: u64,
    disposition: Option<KeyDisposition>,
    #[cfg(any(target_os = "macos", test))]
    closed: bool,
}

#[derive(Default)]
pub(crate) struct DispositionMailbox {
    slot: Mutex<Slot>,
    ready: Condvar,
}

impl DispositionMailbox {
    /// Reserve the reusable slot for one native callback.
    #[cfg(any(not(target_os = "macos"), test))]
    pub(crate) fn begin(&self) -> u64 {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        slot.generation = slot.generation.wrapping_add(1);
        slot.disposition = None;
        slot.generation
    }

    /// Reserve the macOS callback slot unless native input capture has entered
    /// its terminal state. The check shares the existing slot lock, so it adds
    /// no atomic, allocation, or second synchronization step to the callback.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn try_begin(&self) -> Option<u64> {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if slot.closed {
            return None;
        }
        slot.generation = slot.generation.wrapping_add(1);
        slot.disposition = None;
        Some(slot.generation)
    }

    /// Complete `generation`; returns false when that callback already timed
    /// out and a newer event owns the slot.
    pub(crate) fn complete(&self, generation: u64, disposition: KeyDisposition) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        if slot.generation != generation || slot.disposition.is_some() {
            return false;
        }
        slot.disposition = Some(disposition);
        self.ready.notify_one();
        true
    }

    pub(crate) fn wait(&self, generation: u64, timeout: Duration) -> Option<KeyDisposition> {
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

    /// Permanently fail open this mailbox. A native callback racing with
    /// shutdown either reserves its generation before this lock and is woken,
    /// or observes `closed` and returns without waiting.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn close(&self) {
        let mut slot = self.slot.lock().unwrap_or_else(|error| error.into_inner());
        slot.closed = true;
        slot.generation = slot.generation.wrapping_add(1);
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
    fn close_invalidates_current_and_rejects_future_callbacks() {
        let mailbox = DispositionMailbox::default();
        let generation = mailbox.try_begin().unwrap();
        mailbox.close();

        assert_eq!(mailbox.wait(generation, Duration::ZERO), None);
        assert!(!mailbox.complete(generation, KeyDisposition::Consume));
        assert_eq!(mailbox.try_begin(), None);
    }
}
