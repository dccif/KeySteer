use rustc_hash::FxHashMap as HashMap;
use std::collections::hash_map::Entry;
use std::hash::{Hash, Hasher};

use smallvec::SmallVec;

#[cfg(test)]
use crate::api::SemanticRole;
use crate::api::{Rect, UiTarget};

use super::MAX_INLINE_TARGETS;
use super::labeling::CompactHint;
use crate::api::hint::LabelDirection;

/// Request-scoped scan data. Dropping or resetting this value retires every
/// target, label, retry marker, and asynchronous scan identity together.
#[derive(Default)]
pub(super) struct ScanSession {
    pub(super) scanned: Vec<UiTarget>,
    /// Prepared prefix of scanned; append preserves it, retirement invalidates it.
    pub(super) search_text: Vec<super::search::SearchText>,
    pub(super) search_hints: Vec<CompactHint<usize>>,
    /// Positions in search_hints, rebuilt after each query or scan update.
    pub(super) search_matches: Vec<usize>,
    pub(super) search_cycle: super::search::SearchCycle,
    /// Explicit keyboard preview, separate from resolved multi-selection.
    pub(super) search_focus: Option<super::search::Focus>,
    /// Resolved input items, distinct from the broader search preview.
    pub(super) search_selected: Vec<CompactHint<usize>>,
    pub(super) search_seen: Vec<bool>,
    pub(super) search_preview: crate::api::presentation::HintInfoPreview,
    pub(super) search_query: String,
    pub(super) search_terms: super::search::SearchTerms,
    pub(super) search_selection: crate::api::text_edit::Selection,
    pub(super) search_names_initialized: bool,
    // One head per geometry; collision links are contiguous and need no bucket drops.
    next_same_rect: Vec<usize>,
    // Compact hash buckets; collision chains verify the full quantized geometry.
    pub(super) seen_targets: HashMap<u64, usize>,
    pub(super) hints: Vec<CompactHint<usize>>,
    pub(super) label_plan_count: usize,
    pub(super) next_label_index: usize,
    pub(super) scanning: bool,
    pub(super) status: Option<String>,
    pub(super) scan_id: u64,
    pub(super) expected_activation: Option<u32>,
    pub(super) retry_attempt: u32,
    pub(super) retry_pending: bool,
    pub(super) scan_bounds: Option<Rect>,
    pub(super) pending_relabel: bool,
    deferred_targets: Vec<UiTarget>,
    deferred_retired: Vec<Rect>,
    pub(super) selected: Option<usize>,
    pub(super) finished: bool,
    pub(super) active: bool,
}

impl ScanSession {
    pub(super) fn release_scan_index(&mut self) {
        self.seen_targets = HashMap::default();
        self.next_same_rect = Vec::new();
    }

    fn ensure_scan_index(&mut self) {
        if self.next_same_rect.len() != self.scanned.len() {
            self.rebuild_target_lookup();
        }
    }

    pub(super) fn clear_results(&mut self) {
        self.expected_activation = None;
        self.scanned = Vec::new();
        self.search_text = Vec::new();
        self.search_hints = Vec::new();
        self.search_matches = Vec::new();
        self.search_cycle = Default::default();
        self.search_focus = None;
        self.search_selected = Vec::new();
        self.search_seen = Vec::new();
        self.search_preview = Default::default();
        self.search_query = String::new();
        self.search_terms = Default::default();
        self.search_names_initialized = false;
        self.release_scan_index();
        self.hints = Vec::new();
        self.label_plan_count = 0;
        self.next_label_index = 0;
        self.pending_relabel = false;
        self.deferred_targets = Vec::new();
        self.deferred_retired = Vec::new();
    }

    pub(super) fn defer_update(&mut self, mut targets: Vec<UiTarget>, retired: Vec<Rect>) {
        crate::api::command::enrich_replacements(&mut targets, &self.deferred_targets, &retired);
        crate::api::command::remove_retired_targets(&mut self.deferred_targets, &retired);
        self.deferred_targets.extend(targets);
        self.deferred_retired.extend(retired);
        self.pending_relabel = true;
    }

    pub(super) fn apply_deferred(&mut self) {
        let targets = std::mem::take(&mut self.deferred_targets);
        let retired = std::mem::take(&mut self.deferred_retired);
        self.apply_update(targets, &retired);
    }

    pub(super) fn apply_update(&mut self, mut targets: Vec<UiTarget>, retired: &[Rect]) -> bool {
        crate::api::command::enrich_replacements(&mut targets, &self.scanned, retired);
        let before = self.scanned.len();
        crate::api::command::remove_retired_targets(&mut self.scanned, retired);
        let removed = self.scanned.len() != before;
        if removed {
            self.rebuild_target_lookup();
            self.invalidate_search_names();
        }
        self.append_targets(targets) || removed
    }

    /// Apply a delta without renumbering unaffected labels when the current
    /// prefix-free code space has enough room. No timer or provider barrier.
    /// Returns (targets changed, labels reconciled); false in the second slot
    /// asks the caller to rebuild the code space after genuine capacity growth.
    pub(super) fn apply_stable_update(
        &mut self,
        mut targets: Vec<UiTarget>,
        retired: &[Rect],
        alphabet: &[char],
        direction: LabelDirection,
        preserve_anchors: bool,
    ) -> (bool, bool) {
        if targets.is_empty() && retired.is_empty() {
            return (false, true);
        }
        crate::api::command::enrich_replacements(&mut targets, &self.scanned, retired);
        let mut removed_hints = Vec::new();
        let before = self.scanned.len();
        if !retired.is_empty() {
            self.ensure_scan_index();
            // Existing semantic-key buckets identify exact removals without
            // quadratic rectangle comparisons or a second hash table.
            let mut remap = SmallVec::<[usize; MAX_INLINE_TARGETS]>::new();
            remap.resize(before, 0);
            for rect in retired {
                if let Some(&head) = self.seen_targets.get(&rect_hash(*rect)) {
                    let mut index = head;
                    while index != usize::MAX {
                        if self.scanned[index].rect == *rect {
                            remap[index] = usize::MAX;
                        }
                        index = self.next_same_rect[index];
                    }
                }
            }
            let mut old = 0;
            let mut kept = 0;
            self.scanned.retain(|_| {
                let retain = remap[old] != usize::MAX;
                if retain {
                    remap[old] = kept;
                    kept += 1;
                }
                old += 1;
                retain
            });
            // A retirement may be outside this session's bounds or already
            // applied. Keep valid labels and search text when nothing changed.
            if self.scanned.len() != before {
                removed_hints.extend(self.hints.extract_if(.., |hint| {
                    let index = remap[hint.value];
                    if index == usize::MAX {
                        true
                    } else {
                        hint.value = index;
                        false
                    }
                }));
                self.rebuild_target_lookup();
                self.invalidate_search_names();
            }
        }
        let retained = self.scanned.len();
        let changed = self.append_targets(targets) || retained != before;
        let added = self.scanned.len() - retained;
        let Some(plan) = super::labeling::LabelPlan::for_stream(
            self.label_plan_count,
            alphabet.len(),
            direction,
        ) else {
            return (changed, false);
        };
        let capacity = plan.capacity(alphabet.len());
        if added > removed_hints.len() + capacity.saturating_sub(self.next_label_index) {
            return (changed, false);
        }
        self.hints.reserve(added);
        if removed_hints.is_empty() {
            for (offset, target) in self.scanned[retained..].iter().enumerate() {
                let label = plan.code(self.next_label_index, alphabet);
                self.next_label_index += 1;
                self.hints.push(CompactHint {
                    label,
                    bounds: target.rect,
                    value: retained + offset,
                });
            }
            return (changed, true);
        }
        // Associate a replacement with the closest old visible anchor inside
        // its verified control bounds. Row sorting bounds each normal query
        // to one control-height band, instead of testing every pair of boxes.
        removed_hints.sort_unstable_by(|a, b| {
            a.bounds
                .center()
                .y
                .total_cmp(&b.bounds.center().y)
                .then(a.value.cmp(&b.value))
        });
        let mut replacements = SmallVec::<[usize; MAX_INLINE_TARGETS]>::new();
        replacements.resize(added, usize::MAX);
        let mut used = SmallVec::<[bool; MAX_INLINE_TARGETS]>::new();
        used.resize(removed_hints.len(), false);
        // Give a nested checkbox/button its own anchor before its enclosing
        // row chooses one. Output order and code-space order stay unchanged.
        let mut association_order: SmallVec<[usize; MAX_INLINE_TARGETS]> = (0..added).collect();
        association_order.sort_unstable_by(|&a, &b| {
            let area = |i: usize| {
                let r = self.scanned[retained + i].rect;
                r.width * r.height
            };
            area(a).total_cmp(&area(b)).then(a.cmp(&b))
        });
        for offset in association_order {
            let target = &self.scanned[retained + offset];
            let rect = target.rect;
            let start = removed_hints.partition_point(|hint| hint.bounds.center().y < rect.y);
            let end = removed_hints.partition_point(|hint| hint.bounds.center().y <= rect.bottom());
            let center = rect.center();
            let best = (start..end)
                .filter(|&i| !used[i] && rect.contains(&removed_hints[i].bounds.center()))
                .min_by(|&a, &b| {
                    let distance = |i: usize| {
                        let p = removed_hints[i].bounds.center();
                        (p.x - center.x).powi(2) + (p.y - center.y).powi(2)
                    };
                    distance(a).total_cmp(&distance(b)).then(a.cmp(&b))
                });
            if let Some(index) = best {
                replacements[offset] = index;
                used[index] = true;
            }
        }
        let mut free = 0;
        for (offset, target) in self.scanned[retained..].iter().enumerate() {
            let inherited = replacements[offset];
            let (label, bounds) = if inherited != usize::MAX {
                let old = &mut removed_hints[inherited];
                // The visual anchor is retained only inside the new control.
                // Clicking still uses scanned[target].rect, never this anchor.
                (
                    std::mem::take(&mut old.label),
                    if preserve_anchors {
                        old.bounds
                    } else {
                        target.rect
                    },
                )
            } else {
                while free < used.len() && used[free] {
                    free += 1;
                }
                if free < used.len() {
                    let label = std::mem::take(&mut removed_hints[free].label);
                    used[free] = true;
                    free += 1;
                    (label, target.rect)
                } else {
                    let label = plan.code(self.next_label_index, alphabet);
                    self.next_label_index += 1;
                    (label, target.rect)
                }
            };
            self.hints.push(CompactHint {
                label,
                bounds,
                value: retained + offset,
            });
        }
        (changed, true)
    }

    fn rebuild_target_lookup(&mut self) {
        self.seen_targets.clear();
        self.next_same_rect.clear();
        for (index, target) in self.scanned.iter().enumerate() {
            let previous = self.seen_targets.insert(rect_hash(target.rect), index);
            self.next_same_rect.push(previous.unwrap_or(usize::MAX));
        }
    }

    fn invalidate_search_names(&mut self) {
        self.search_text.clear();
        self.search_names_initialized = false;
    }

    #[inline]
    pub(super) fn append_targets(&mut self, targets: Vec<UiTarget>) -> bool {
        if targets.is_empty() {
            return false;
        }
        self.ensure_scan_index();
        let before = self.scanned.len();

        // Take the first platform batch by ownership; every scan buffer is
        // released on exit or before the next scan.
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
            self.next_same_rect.reserve(self.scanned.len());

            let mut retained = 0;
            let mut geometry = RectKeyCache::default();
            for index in 0..self.scanned.len() {
                let key = hash_rect_key(geometry.key(self.scanned[index].rect));
                let entry = self.seen_targets.entry(key);
                let head = match &entry {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(_) => usize::MAX,
                };
                let duplicate = contains_target(
                    &self.scanned,
                    &self.next_same_rect,
                    head,
                    &self.scanned[index],
                );
                if !duplicate {
                    // Compact in source order without shifting the remaining
                    // batch for each duplicate. Indices always refer to the
                    // retained prefix, so later duplicates see canonical data.
                    if retained != index {
                        self.scanned.swap(retained, index);
                    }
                    self.next_same_rect.push(head);
                    entry.insert_entry(retained);
                    retained += 1;
                }
            }
            self.scanned.truncate(retained);
            return !self.scanned.is_empty();
        }

        let incoming = targets.len();
        self.scanned.reserve(incoming);
        self.seen_targets.reserve(incoming);
        self.next_same_rect.reserve(incoming);
        for target in targets {
            self.append_target(target);
        }
        let changed = self.scanned.len() != before;
        if changed {
            // Publish new labels before normalizing their search text. The
            // already prepared prefix remains valid even during prewarming.
            self.search_names_initialized = false;
        }
        changed
    }

    /// Prepare a bounded prefix without discarding previous work. Returns true
    /// once all current targets are indexed, including late scan additions.
    pub(super) fn prepare_search_names(&mut self, limit: usize) -> bool {
        if self.search_names_initialized {
            return true;
        }
        let prepared = self.search_text.len();
        debug_assert!(prepared <= self.scanned.len());
        self.search_text.reserve(self.scanned.len() - prepared);
        let end = prepared.saturating_add(limit).min(self.scanned.len());
        self.search_text.extend(
            self.scanned[prepared..end]
                .iter()
                .map(super::search::SearchText::target),
        );
        self.search_names_initialized = end == self.scanned.len();
        self.search_names_initialized
    }

    pub(super) fn ensure_search_names(&mut self) {
        self.prepare_search_names(usize::MAX);
    }

    fn append_target(&mut self, target: UiTarget) -> bool {
        if !self
            .scan_bounds
            .is_none_or(|bounds| bounds.contains(&target.rect.center()))
        {
            return false;
        }
        let key = rect_hash(target.rect);
        let entry = self.seen_targets.entry(key);
        let head = match &entry {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(_) => usize::MAX,
        };
        if contains_target(&self.scanned, &self.next_same_rect, head, &target) {
            return false;
        }
        let index = self.scanned.len();
        self.scanned.push(target);
        self.next_same_rect.push(head);
        entry.insert_entry(index);
        true
    }
}

fn contains_target(
    scanned: &[UiTarget],
    next: &[usize],
    mut index: usize,
    target: &UiTarget,
) -> bool {
    while index != usize::MAX {
        let existing = &scanned[index];
        if existing.name == target.name
            && existing.role == target.role
            && rect_key(existing.rect) == rect_key(target.rect)
        {
            return true;
        }
        index = next[index];
    }
    false
}

fn rect_hash(rect: Rect) -> u64 {
    hash_rect_key(rect_key(rect))
}

fn hash_rect_key(key: (i64, i64, i64, i64)) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Batch-local reuse for repeated row coordinates and control dimensions.
/// Changed values still use exactly the same rounding and saturating cast.
#[derive(Default)]
struct RectKeyCache {
    rect: Rect,
    key: (i64, i64, i64, i64),
}

impl RectKeyCache {
    fn key(&mut self, rect: Rect) -> (i64, i64, i64, i64) {
        let quantize = |value: f64, previous: f64, key: i64| {
            if value == previous {
                key
            } else {
                (value * 4.0).round() as i64
            }
        };
        self.key = (
            quantize(rect.x, self.rect.x, self.key.0),
            quantize(rect.y, self.rect.y, self.key.1),
            quantize(rect.width, self.rect.width, self.key.2),
            quantize(rect.height, self.rect.height, self.key.3),
        );
        self.rect = rect;
        self.key
    }
}

fn rect_key(rect: Rect) -> (i64, i64, i64, i64) {
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
    fn batch_geometry_reuse_matches_standard_quantization() {
        let mut cache = RectKeyCache::default();
        let mut rect = Rect::default();
        let values = [
            0.0,
            -0.0,
            0.124999999999,
            0.125,
            0.125000000001,
            -0.124999999999,
            -0.125,
            -0.125000000001,
            64.0,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            i64::MAX as f64,
            i64::MIN as f64,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ];
        let mut state = 1u64;
        for index in 0..10_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let value = if index % 2 == 0 {
                values[(state as usize) % values.len()]
            } else {
                f64::from_bits(state)
            };
            match index % 4 {
                0 => rect.x = value,
                1 => rect.y = value,
                2 => rect.width = value,
                _ => rect.height = value,
            }
            for _ in 0..2 {
                assert_eq!(cache.key(rect), rect_key(rect), "{rect:?}");
                assert_eq!(hash_rect_key(cache.key(rect)), rect_hash(rect));
            }
        }
    }

    #[test]
    fn compact_lookup_collision_chain_checks_full_geometry_and_semantics() {
        let target = |x, role| UiTarget {
            rect: Rect::new(x, 0.0, 20.0, 20.0),
            name: "same name".into(),
            role,
            details: None,
        };
        // Simulate different quantized rectangles landing in one hash bucket.
        let scanned = [
            target(0.0, SemanticRole::Button),
            target(100.0, SemanticRole::Button),
        ];
        let next = [usize::MAX, 0];
        assert!(!contains_target(
            &scanned,
            &next,
            1,
            &target(200.0, SemanticRole::Button)
        ));
        assert!(contains_target(
            &scanned,
            &next,
            1,
            &target(0.0, SemanticRole::Button)
        ));
        assert!(contains_target(
            &scanned,
            &next,
            1,
            &target(100.0, SemanticRole::Button)
        ));
        assert!(!contains_target(
            &scanned,
            &next,
            1,
            &target(100.0, SemanticRole::Checkbox)
        ));
        // Retain the original quarter-pixel identity rule, including subpixel updates.
        assert!(contains_target(
            &scanned,
            &next,
            1,
            &target(0.01, SemanticRole::Button)
        ));
    }

    fn labeled_session(count: usize, alphabet: &[char], direction: LabelDirection) -> ScanSession {
        let mut session = ScanSession::default();
        session.append_targets(
            (0..count)
                .map(|i| UiTarget {
                    details: None,
                    rect: Rect::new(i as f64 * 30.0, 10.0, 20.0, 20.0),
                    name: i.to_string(),
                    role: SemanticRole::Control,
                })
                .collect(),
        );
        super::super::labeling::assign_compact_into(
            &mut session.hints,
            session.scanned.iter().enumerate().map(|(i, t)| (t.rect, i)),
            alphabet,
            direction,
        )
        .unwrap();
        session.label_plan_count = count;
        session.next_label_index = count;
        session
    }

    #[test]
    fn repeated_scans_release_all_result_capacity() {
        let mut session = ScanSession::default();
        for count in [24, 512, 2000, 24, 10000, 1].into_iter().cycle().take(30) {
            session.append_targets(
                (0..count)
                    .map(|i| UiTarget {
                        details: None,
                        rect: Rect::new(i as f64 * 30., 0., 20., 20.),
                        name: format!("target {i}"),
                        role: SemanticRole::Button,
                    })
                    .collect(),
            );
            session.ensure_search_names();
            session.deferred_targets = session.scanned.clone();
            session.deferred_retired = session.scanned.iter().map(|t| t.rect).collect();
            session.clear_results();
            assert_eq!(session.scanned.capacity(), 0);
            assert_eq!(session.search_text.capacity(), 0);
            assert_eq!(session.seen_targets.capacity(), 0);
            assert_eq!(session.next_same_rect.capacity(), 0);
            assert_eq!(session.hints.capacity(), 0);
            assert_eq!(session.deferred_targets.capacity(), 0);
            assert_eq!(session.deferred_retired.capacity(), 0);
        }
    }

    #[test]
    fn stable_updates_preserve_codes_and_expand_only_when_code_space_is_full() {
        for direction in [LabelDirection::Normal, LabelDirection::Reverse] {
            let alphabet: Vec<_> = "asdfghjkl".chars().collect();
            let mut session = labeled_session(20, &alphabet, direction);
            let old_codes: Vec<_> = session.hints.iter().map(|h| h.label.clone()).collect();
            let new_target = |i: usize| UiTarget {
                details: None,
                rect: Rect::new(i as f64 * 30.0, 50.0, 20.0, 20.0),
                name: i.to_string(),
                role: SemanticRole::Control,
            };
            assert_eq!(
                session.apply_stable_update(
                    (20..23).map(new_target).collect(),
                    &[],
                    &alphabet,
                    direction,
                    true
                ),
                (true, true)
            );
            for (hint, code) in session.hints.iter().zip(old_codes) {
                assert_eq!(hint.label, code);
            }
            let capacity =
                super::super::labeling::LabelPlan::for_stream(20, alphabet.len(), direction)
                    .unwrap()
                    .capacity(alphabet.len());
            assert_eq!(
                session.apply_stable_update(
                    (23..capacity + 1).map(new_target).collect(),
                    &[],
                    &alphabet,
                    direction,
                    true
                ),
                (true, false)
            );
        }
    }

    #[test]
    fn replacement_inherits_one_label_and_anchor_without_renumbering_other_controls() {
        let alphabet: Vec<_> = "asdfghjkl".chars().collect();
        let mut session = labeled_session(4, &alphabet, LabelDirection::Normal);
        let unaffected = session.hints[3].label.clone();
        let inherited = session.hints[1].label.clone();
        let anchor = session.hints[1].bounds;
        let retired: Vec<_> = session.scanned[..3].iter().map(|t| t.rect).collect();
        let row = UiTarget {
            details: None,
            rect: Rect::new(0.0, 0.0, 85.0, 40.0),
            name: "row".into(),
            role: SemanticRole::ListItem,
        };
        assert_eq!(
            session.apply_stable_update(
                vec![row.clone()],
                &retired,
                &alphabet,
                LabelDirection::Normal,
                true
            ),
            (true, true)
        );
        assert_eq!(session.hints.len(), 2);
        assert_eq!(session.hints[0].label, unaffected);
        assert_eq!(session.hints[1].label, inherited);
        assert_eq!(session.hints[1].bounds, anchor);
        assert_eq!(session.scanned[session.hints[1].value], row);
        assert_ne!(
            anchor.center(),
            row.rect.center(),
            "visual anchor is not the authoritative click point"
        );
    }

    #[test]
    fn replacement_never_reuses_an_anchor_outside_its_new_control() {
        let alphabet: Vec<_> = "asdfghjkl".chars().collect();
        let mut session = labeled_session(4, &alphabet, LabelDirection::Normal);
        let rect = Rect::new(500.0, 300.0, 30.0, 20.0);
        let retired = [session.scanned[0].rect];
        let incoming = UiTarget {
            details: None,
            rect,
            name: "new".into(),
            role: SemanticRole::Button,
        };
        assert_eq!(
            session.apply_stable_update(
                vec![incoming],
                &retired,
                &alphabet,
                LabelDirection::Normal,
                true
            ),
            (true, true)
        );
        assert_eq!(session.hints.last().unwrap().bounds, rect);
    }

    #[test]
    fn nested_control_inherits_its_anchor_before_a_row_can_claim_it() {
        let alphabet: Vec<_> = "asdfghjkl".chars().collect();
        let mut session = labeled_session(2, &alphabet, LabelDirection::Normal);
        let first = session.hints[0].label.clone();
        let retired: Vec<_> = session.scanned.iter().map(|t| t.rect).collect();
        let checkbox = UiTarget {
            details: None,
            rect: session.scanned[0].rect,
            name: "check".into(),
            role: SemanticRole::Checkbox,
        };
        let row = UiTarget {
            details: None,
            rect: Rect::new(-50.0, 0.0, 105.0, 40.0),
            name: "row".into(),
            role: SemanticRole::Row,
        };
        assert_eq!(
            session.apply_stable_update(
                vec![row, checkbox.clone()],
                &retired,
                &alphabet,
                LabelDirection::Normal,
                true
            ),
            (true, true)
        );
        let hint = session
            .hints
            .iter()
            .find(|h| session.scanned[h.value] == checkbox)
            .unwrap();
        assert_eq!(hint.label, first);
    }

    #[test]
    fn deferred_refinements_preserve_existing_selection_until_applied() {
        let mut session = ScanSession::default();
        let old = UiTarget {
            details: None,
            rect: Rect::new(0.0, 0.0, 20.0, 20.0),
            name: "old".into(),
            role: SemanticRole::Control,
        };
        let middle = UiTarget {
            details: None,
            rect: Rect::new(2.0, 0.0, 24.0, 20.0),
            name: "text".into(),
            role: SemanticRole::Control,
        };
        let final_target = UiTarget {
            name: "button".into(),
            role: SemanticRole::Button,
            ..old.clone()
        };
        session.append_targets(vec![old.clone()]);
        session.ensure_search_names();
        session.defer_update(vec![middle.clone()], vec![old.rect]);
        session.defer_update(vec![final_target.clone()], vec![middle.rect]);
        assert_eq!(session.scanned, vec![old]);
        session.apply_deferred();
        assert_eq!(session.scanned, vec![final_target]);
        session.ensure_search_names();
        assert!(session.search_text[0].matches("button", ""));
        assert!(!session.append_targets(session.scanned.clone()));
    }

    #[test]
    fn partial_search_index_survives_append_and_rebuilds_after_retirement() {
        let alphabet = ['a', 's', 'd'];
        let mut session = labeled_session(512, &alphabet, LabelDirection::Normal);
        assert!(!session.prepare_search_names(128));
        assert_eq!(session.search_text.len(), 128);
        let first = session.search_text.as_ptr();
        assert!(!session.append_targets(session.scanned.clone()));
        assert_eq!(session.search_text.len(), 128);
        assert_eq!(session.search_text.as_ptr(), first);
        let mut added = session.scanned[0].clone();
        added.name = "Added 搜索框".into();
        added.rect.x += 20_000.0;
        assert!(session.append_targets(vec![added]));
        assert_eq!(session.search_text.len(), 128);
        assert_eq!(session.search_text.as_ptr(), first);
        assert!(!session.prepare_search_names(128));
        assert_eq!(session.search_text.len(), 256);
        let retired = session.scanned[7].rect;
        assert!(session.apply_update(Vec::new(), &[retired]));
        assert_eq!(session.search_text.len(), 0);
        assert!(!session.prepare_search_names(128));
        assert!(session.search_text[7].matches("8", ""));
        session.ensure_search_names();
        assert!(session.search_names_initialized);
        assert_eq!(session.search_text.len(), session.scanned.len());
        assert!(session.search_text.last().unwrap().matches("added", ""));
        assert!(session.search_text.last().unwrap().matches("ssk", ""));
    }

    #[test]
    fn unmatched_retirements_preserve_search_index_and_labels() {
        let alphabet = ['a', 's', 'd'];
        for add_target in [false, true] {
            let mut session = labeled_session(20, &alphabet, LabelDirection::Normal);
            session.ensure_search_names();
            let labels: Vec<_> = session
                .hints
                .iter()
                .map(|hint| hint.label.clone())
                .collect();
            // Same quantized bucket as target 0, but no exact rectangle match.
            let mut retired = session.scanned[0].rect;
            retired.x += 0.01;
            let targets = if add_target {
                vec![UiTarget {
                    rect: Rect::new(0.0, 50.0, 20.0, 20.0),
                    name: "Added".into(),
                    role: SemanticRole::Button,
                    details: None,
                }]
            } else {
                Vec::new()
            };
            assert_eq!(
                session.apply_stable_update(
                    targets,
                    &[retired, Rect::new(-100.0, -100.0, 1.0, 1.0)],
                    &alphabet,
                    LabelDirection::Normal,
                    true,
                ),
                (add_target, true)
            );
            assert_eq!(session.search_names_initialized, !add_target);
            assert_eq!(session.search_text.len(), 20);
            session.ensure_search_names();
            assert_eq!(session.search_text.len(), session.scanned.len());
            for (index, label) in labels.iter().enumerate() {
                assert_eq!(&session.hints[index].label, label);
                assert_eq!(session.hints[index].value, index);
                assert!(session.search_text[index].matches(&index.to_string(), ""));
            }
            if add_target {
                assert!(session.search_text[20].matches("added", ""));
            }
            assert!(!session.append_targets(session.scanned.clone()));
        }
    }

    #[test]
    fn released_index_rebuilds_for_late_updates_without_losing_search_or_dedup() {
        for _ in 0..30 {
            let mut session = labeled_session(512, &['a', 's', 'd'], LabelDirection::Normal);
            session.ensure_search_names();
            let target = session.scanned[7].clone();
            session.release_scan_index();
            assert_eq!(session.seen_targets.capacity(), 0);
            assert_eq!(session.next_same_rect.capacity(), 0);
            assert!(!session.append_targets(Vec::new()));
            assert_eq!(session.seen_targets.capacity(), 0);
            assert!(!session.append_targets(vec![target.clone()]));
            assert!(session.search_names_initialized);
            assert_eq!(session.search_text.len(), 512);
            session.release_scan_index();
            session.apply_stable_update(
                Vec::new(),
                &[target.rect],
                &['a', 's', 'd'],
                LabelDirection::Normal,
                true,
            );
            assert_eq!(session.scanned.len(), 511);
            assert!(!session.search_names_initialized);
            session.ensure_search_names();
            assert_eq!(session.search_text.len(), 511);
            session.release_scan_index();
            assert!(session.append_targets(vec![target]));
            assert_eq!(session.search_text.len(), 511);
            assert!(!session.search_names_initialized);
            session.ensure_search_names();
            assert_eq!(session.search_text.len(), 512);
            session.clear_results();
            assert_eq!(session.seen_targets.capacity(), 0);
            assert_eq!(session.next_same_rect.capacity(), 0);
        }
    }

    #[test]
    #[ignore = "allocation probe; run alone with --test-threads=1"]
    fn completed_scan_index_releases_memory_while_labels_remain() {
        let mut session = labeled_session(10_000, &['a', 's', 'd'], LabelDirection::Normal);
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        session.release_scan_index();
        let change = region.change();
        assert_eq!(change.allocations, 0);
        assert!(change.bytes_deallocated >= 10_000 * std::mem::size_of::<usize>());
        assert_eq!(session.scanned.len(), 10_000);
        assert_eq!(session.hints.len(), 10_000);
        println!(
            "completed scan index released {} bytes",
            change.bytes_deallocated
        );
    }

    #[test]
    fn collision_links_survive_exact_retirement_and_later_batches() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        let neighbors: Vec<_> = (0..5)
            .map(|i| UiTarget {
                details: None,
                // All five share a quantized geometry key, but only one is retired.
                rect: Rect::new(rect.x + i as f64 * 0.01, rect.y, rect.width, rect.height),
                name: i.to_string(),
                role: SemanticRole::Button,
            })
            .collect();
        let mut session = ScanSession::default();
        session.append_targets(neighbors.clone());
        let retired = neighbors[2].rect;
        session.apply_stable_update(
            Vec::new(),
            &[retired],
            &['a', 's', 'd'],
            LabelDirection::Normal,
            true,
        );
        let expected: Vec<_> = neighbors
            .iter()
            .filter(|target| target.rect != retired)
            .cloned()
            .collect();
        assert_eq!(session.scanned, expected);
        assert!(!session.append_targets(expected));
        assert!(session.append_targets(vec![neighbors[2].clone()]));
        assert_eq!(session.scanned.len(), 5);
        assert!(!session.append_targets(neighbors));
        session.clear_results();
        assert_eq!(session.next_same_rect.capacity(), 0);
    }

    #[test]
    fn owned_batches_preserve_first_occurrence_order_and_collision_indices() {
        for count in [24, 128, 129, 500, 2_000] {
            let mut targets = Vec::new();
            let mut expected = Vec::new();
            for index in 0..count {
                let target = UiTarget {
                    details: None,
                    rect: Rect::new((index % 10) as f64, 0.0, 1.0, 1.0),
                    name: format!("Target {index}"),
                    role: SemanticRole::Button,
                };
                expected.push(target.clone());
                targets.push(target.clone());
                // An identical second copy must collapse into the first one.
                targets.push(target);
            }
            let original_storage = targets.as_ptr();
            let mut session = ScanSession::default();
            assert!(session.append_targets(targets));
            assert_eq!(session.scanned.as_ptr(), original_storage);
            assert_eq!(session.scanned, expected);
            let mut visited = vec![false; count];
            for &head in session.seen_targets.values() {
                let mut index = head;
                while index != usize::MAX {
                    assert!(!visited[index]);
                    visited[index] = true;
                    assert_eq!(session.scanned[index], expected[index]);
                    index = session.next_same_rect[index];
                }
            }
            assert!(visited.into_iter().all(|seen| seen));
            session.ensure_search_names();
            // Later partials use the append path and the same canonical index.
            assert!(!session.append_targets(expected.clone()));
            assert_eq!(session.scanned, expected);
            assert_eq!(session.search_text.len(), count);
            session.clear_results();
            assert!(session.scanned.is_empty());
            assert_eq!(session.scanned.capacity(), 0);
        }
    }
}
