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

        // Take the first platform batch by ownership. Large buffers are still
        // released on exit; retained small buffers are cheaper to refill.
        if self.scanned.is_empty()
            && self.scanned.capacity() == 0
            && self.seen_targets.is_empty()
            && !self.search_names_initialized
        {
            self.scanned = targets;
            self.scanned.retain(|target| {
                self.scan_bounds
                    .is_none_or(|bounds| bounds.contains(&target.rect.center()))
            });
            self.seen_targets.reserve(self.scanned.len());

            let mut retained = 0;
            for index in 0..self.scanned.len() {
                let key = target_key(&self.scanned[index]);
                let indices = self.seen_targets.entry(key).or_default();
                let duplicate = {
                    indices.iter().any(|&existing_index| {
                        let existing = &self.scanned[existing_index];
                        let candidate = &self.scanned[index];
                        existing.name == candidate.name && existing.role == candidate.role
                    })
                };
                if !duplicate {
                    // Compact in source order without shifting the remaining
                    // batch for each duplicate. Indices always refer to the
                    // retained prefix, so later duplicates see canonical data.
                    if retained != index {
                        self.scanned.swap(retained, index);
                    }
                    indices.push(retained);
                    retained += 1;
                }
            }
            self.scanned.truncate(retained);
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
        let indices = self.seen_targets.entry(key).or_default();
        let duplicate = {
            indices.iter().any(|&index| {
                let existing = &self.scanned[index];
                existing.name == target.name && existing.role == target.role
            })
        };
        if duplicate {
            return false;
        }
        let index = self.scanned.len();
        if self.search_names_initialized {
            self.scanned_names_lower.push(target.name.to_lowercase());
        }
        self.scanned.push(target);
        indices.push(index);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_batches_preserve_first_occurrence_order_and_collision_indices() {
        for count in [24, 128, 129, 500, 2_000] {
            let mut targets = Vec::new();
            let mut expected = Vec::new();
            for index in 0..count {
                let target = UiTarget {
                    rect: Rect::new((index % 10) as f64, 0.0, 1.0, 1.0),
                    name: format!("Target {index}"),
                    role: "button".into(),
                    native_role: None,
                };
                expected.push(target.clone());
                targets.push(target.clone());
                let mut duplicate = target;
                duplicate.native_role = Some("same semantic target".into());
                targets.push(duplicate);
            }
            let original_storage = targets.as_ptr();
            let mut session = ScanSession::default();
            assert!(session.append_targets(targets));
            assert_eq!(session.scanned.as_ptr(), original_storage);
            assert_eq!(session.scanned, expected);
            for indices in session.seen_targets.values() {
                for &index in indices {
                    assert_eq!(session.scanned[index], expected[index]);
                }
            }
            session.ensure_search_names();
            // Later partials use the append path and the same canonical index.
            assert!(!session.append_targets(expected.clone()));
            assert_eq!(session.scanned, expected);
            assert_eq!(session.scanned_names_lower.len(), count);
            session.clear_results(true);
            assert!(session.scanned.is_empty());
            assert!(session.scanned.capacity() <= MAX_IDLE_RETAINED_TARGETS);
        }
    }
}
