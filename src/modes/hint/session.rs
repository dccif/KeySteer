use std::collections::HashMap;

use smallvec::SmallVec;

use crate::api::{Rect, UiTarget};

use super::MAX_IDLE_RETAINED_TARGETS;
use super::labeling::CompactHint;

/// Request-scoped scan data. Dropping or resetting this value retires every
/// target, label, retry marker, and asynchronous scan identity together.
#[derive(Default)]
pub(super) struct ScanSession {
    pub(super) scanned: Vec<UiTarget>,
    pub(super) scanned_names_lower: Vec<String>,
    pub(super) search_names_initialized: bool,
    pub(super) seen_targets: HashMap<(i64, i64, i64, i64), SmallVec<[usize; 2]>>,
    pub(super) hints: Vec<CompactHint<usize>>,
    pub(super) scanning: bool,
    pub(super) status: Option<String>,
    pub(super) scan_id: u64,
    pub(super) retry_attempt: u32,
    pub(super) retry_pending: bool,
    pub(super) scan_bounds: Option<Rect>,
    pub(super) pending_relabel: bool,
    pub(super) selected: Option<usize>,
    pub(super) finished: bool,
    pub(super) active: bool,
}

impl ScanSession {
    pub(super) fn clear_results(&mut self, release_large_buffers: bool) {
        self.scanned.clear();
        self.scanned_names_lower.clear();
        self.search_names_initialized = false;
        self.seen_targets.clear();
        self.hints.clear();
        self.pending_relabel = false;
        if !release_large_buffers {
            return;
        }
        if self.scanned.capacity() > MAX_IDLE_RETAINED_TARGETS {
            self.scanned = Vec::new();
        }
        if self.scanned_names_lower.capacity() > MAX_IDLE_RETAINED_TARGETS {
            self.scanned_names_lower = Vec::new();
        }
        if self.hints.capacity() > MAX_IDLE_RETAINED_TARGETS {
            self.hints = Vec::new();
        }
        if self.seen_targets.capacity() > MAX_IDLE_RETAINED_TARGETS {
            self.seen_targets = HashMap::new();
        }
    }

    #[inline]
    pub(super) fn append_targets(&mut self, targets: Vec<UiTarget>) -> bool {
        let before = self.scanned.len();

        // Take the first small platform batch by ownership. A retained session
        // buffer is already cheaper to refill by move.
        if self.scanned.is_empty()
            && self.scanned.capacity() == 0
            && self.seen_targets.is_empty()
            && !self.search_names_initialized
            && targets.len() <= MAX_IDLE_RETAINED_TARGETS
        {
            self.scanned = targets;
            self.scanned.retain(|target| {
                self.scan_bounds
                    .is_none_or(|bounds| bounds.contains(&target.rect.center()))
            });
            self.seen_targets.reserve(self.scanned.len());

            let mut index = 0;
            while index < self.scanned.len() {
                let key = target_key(&self.scanned[index]);
                let duplicate = self.seen_targets.get(&key).is_some_and(|indices| {
                    indices.iter().any(|&existing_index| {
                        let existing = &self.scanned[existing_index];
                        let candidate = &self.scanned[index];
                        existing.name == candidate.name && existing.role == candidate.role
                    })
                });
                if duplicate {
                    self.scanned.remove(index);
                } else {
                    self.seen_targets.entry(key).or_default().push(index);
                    index += 1;
                }
            }
            return !self.scanned.is_empty();
        }

        let incoming = targets.len();
        self.scanned.reserve(incoming);
        self.seen_targets.reserve(incoming);
        if self.search_names_initialized {
            self.scanned_names_lower.reserve(incoming);
        }
        for target in targets {
            self.append_target(target);
        }
        self.scanned.len() != before
    }

    pub(super) fn ensure_search_names(&mut self) {
        if self.search_names_initialized {
            return;
        }
        self.scanned_names_lower.clear();
        self.scanned_names_lower
            .extend(self.scanned.iter().map(|target| target.name.to_lowercase()));
        self.search_names_initialized = true;
    }

    fn append_target(&mut self, target: UiTarget) -> bool {
        if !self
            .scan_bounds
            .is_none_or(|bounds| bounds.contains(&target.rect.center()))
        {
            return false;
        }
        let key = target_key(&target);
        let duplicate = self.seen_targets.get(&key).is_some_and(|indices| {
            indices.iter().any(|&index| {
                let existing = &self.scanned[index];
                existing.name == target.name && existing.role == target.role
            })
        });
        if duplicate {
            return false;
        }
        let index = self.scanned.len();
        if self.search_names_initialized {
            self.scanned_names_lower.push(target.name.to_lowercase());
        }
        self.scanned.push(target);
        self.seen_targets.entry(key).or_default().push(index);
        true
    }
}

fn target_key(target: &UiTarget) -> (i64, i64, i64, i64) {
    let rect = target.rect;
    (
        (rect.x * 4.0).round() as i64,
        (rect.y * 4.0).round() as i64,
        (rect.width * 4.0).round() as i64,
        (rect.height * 4.0).round() as i64,
    )
}
