#![forbid(unsafe_code)]

//! Cache-friendly bounded spatial index shared by native UI scanners.

use rustc_hash::FxHashMap;

use smallvec::SmallVec;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::api::geometry::{Rect, SemanticRole, UiTarget};

const NATIVE_DEDUP_CELL_SIZE: f64 = 8.0;

/// Native-only prefilter. Names and roles must agree, and geometry must also
/// satisfy the same IoU rule as accessibility evidence in shared fusion.
/// Grid cells only narrow candidates; proximity never proves duplication.
/// Larger offsets remain the fusion index's responsibility.
pub(crate) struct NativeTargetDeduper {
    threshold: f64,
    // The first rectangle lives with its name/role. Only additional rectangles
    // need grid storage, so unique captions never allocate a spatial bucket.
    rects: Vec<Rect>,
    cells: HashMap<(usize, i32, i32), SmallVec<[usize; 2]>>,
    semantics: Vec<(smol_str::SmolStr, SemanticRole, Rect)>,
    semantic_buckets: HashMap<u64, SmallVec<[usize; 2]>>,
}

impl NativeTargetDeduper {
    pub(crate) fn new(threshold: f64) -> Self {
        Self {
            threshold,
            rects: Vec::new(),
            cells: HashMap::new(),
            semantics: Vec::new(),
            semantic_buckets: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, target: &UiTarget) -> bool {
        let known_semantics = self.semantics.len();
        let semantic_index = self.semantic_index(target);
        if semantic_index == known_semantics {
            return true;
        }
        let cell = Self::cell(target.rect);
        let first = self.semantics[semantic_index].2;
        let first_cell = Self::cell(first);
        if (cell.0.saturating_sub(1)..=cell.0.saturating_add(1)).contains(&first_cell.0)
            && (cell.1.saturating_sub(1)..=cell.1.saturating_add(1)).contains(&first_cell.1)
            && native_rectangles_match(first, target.rect, self.threshold)
        {
            return false;
        }
        for y in cell.1.saturating_sub(1)..=cell.1.saturating_add(1) {
            for x in cell.0.saturating_sub(1)..=cell.0.saturating_add(1) {
                if self
                    .cells
                    .get(&(semantic_index, x, y))
                    .is_some_and(|entries| {
                        entries.iter().any(|&index| {
                            native_rectangles_match(self.rects[index], target.rect, self.threshold)
                        })
                    })
                {
                    return false;
                }
            }
        }
        let index = self.rects.len();
        self.rects.push(target.rect);
        self.cells
            .entry((semantic_index, cell.0, cell.1))
            .or_default()
            .push(index);
        true
    }

    fn cell(rect: Rect) -> (i32, i32) {
        let center = rect.center();
        (
            (center.x / NATIVE_DEDUP_CELL_SIZE).floor() as i32,
            (center.y / NATIVE_DEDUP_CELL_SIZE).floor() as i32,
        )
    }

    fn semantic_index(&mut self, target: &UiTarget) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        target.name.hash(&mut hasher);
        target.role.hash(&mut hasher);
        let hash = hasher.finish();
        let indices = self.semantic_buckets.entry(hash).or_default();
        if let Some(index) = indices.iter().copied().find(|&index| {
            let (name, role, _) = &self.semantics[index];
            name.as_str() == target.name.as_str() && role == &target.role
        }) {
            return index;
        }
        let index = self.semantics.len();
        self.semantics
            .push((target.name.as_str().into(), target.role, target.rect));
        indices.push(index);
        index
    }
}

const MAX_CELLS_PER_RECT: i64 = 32;

/// Stores each rectangle once and keeps only compact indices in grid cells.
///
/// Callers choose the matching policy: native candidate de-duplication can use
/// IoU, while shared fusion also uses containment and text-baseline proximity.
/// Queries allocate no temporary candidate list.
#[derive(Clone)]
pub(crate) struct SpatialIndex {
    cell_size: f64,
    padding: f64,
    minimum_side: f64,
    cells: FxHashMap<(i32, i32), SmallVec<[u32; 4]>>,
    oversize: SmallVec<[u32; 16]>,
    rects: Vec<Rect>,
    marks: Vec<u32>,
    query_generation: u32,
}

impl SpatialIndex {
    pub(crate) fn new(cell_size: f64, padding: f64, minimum_side: f64) -> Self {
        Self {
            cell_size: cell_size.max(1.0),
            padding: padding.max(0.0),
            minimum_side: minimum_side.max(0.0),
            cells: FxHashMap::default(),
            oversize: SmallVec::new(),
            rects: Vec::new(),
            marks: Vec::new(),
            query_generation: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.rects.len()
    }

    /// Insert `rect` unless `matches` considers an indexed rectangle equal.
    pub(crate) fn insert_if_unique(
        &mut self,
        rect: Rect,
        mut matches: impl FnMut(Rect, Rect) -> bool,
    ) -> bool {
        self.insert_with_index(rect, |_, a, b| matches(a, b))
    }

    fn insert_with_index(
        &mut self,
        rect: Rect,
        matches: impl FnMut(usize, Rect, Rect) -> bool,
    ) -> bool {
        if !self.usable(rect) || self.any_match(rect, matches) {
            return false;
        }
        self.store(rect)
    }

    pub(super) fn any_match(
        &mut self,
        rect: Rect,
        matches: impl FnMut(usize, Rect, Rect) -> bool,
    ) -> bool {
        self.next_query_generation();
        self.query(rect, matches)
    }

    /// Visit the union of grid candidates once, even when several query boxes
    /// touch the same target. The callback must check against the whole batch,
    /// since grid membership is conservative, not an intersection proof.
    pub(super) fn for_each_candidate(
        &mut self,
        rects: impl IntoIterator<Item = Rect>,
        mut visit: impl FnMut(usize, Rect),
    ) {
        self.next_query_generation();
        for rect in rects {
            self.query(rect, |index, candidate, _| {
                visit(index, candidate);
                false
            });
        }
    }

    fn query(&mut self, rect: Rect, mut matches: impl FnMut(usize, Rect, Rect) -> bool) -> bool {
        let range = self.covered_cells(rect);
        let oversize = Self::oversize_range(range);
        let generation = self.query_generation;
        let rects = &self.rects;
        let marks = &mut self.marks;
        let mut inspect = |candidate: u32| {
            let candidate = candidate as usize;
            if marks[candidate] == generation {
                return false;
            }
            marks[candidate] = generation;
            matches(candidate, rects[candidate], rect)
        };
        if oversize {
            (0..rects.len()).any(|index| inspect(index as u32))
        } else {
            self.oversize.iter().copied().any(&mut inspect)
                || (range.1..=range.3).any(|y| {
                    (range.0..=range.2).any(|x| {
                        self.cells
                            .get(&(x, y))
                            .is_some_and(|entries| entries.iter().copied().any(&mut inspect))
                    })
                })
        }
    }

    fn oversize_range(range: (i32, i32, i32, i32)) -> bool {
        // Widen first: finite native coordinates may saturate opposite i32 ends.
        let columns = i64::from(range.2)
            .saturating_sub(i64::from(range.0))
            .saturating_add(1);
        let rows = i64::from(range.3)
            .saturating_sub(i64::from(range.1))
            .saturating_add(1);
        columns.saturating_mul(rows) > MAX_CELLS_PER_RECT
    }

    pub(super) fn store(&mut self, rect: Rect) -> bool {
        let Ok(index) = u32::try_from(self.rects.len()) else {
            return false;
        };
        let range = self.covered_cells(rect);
        self.rects.push(rect);
        self.marks.push(0);
        if Self::oversize_range(range) {
            self.oversize.push(index);
        } else {
            for y in range.1..=range.3 {
                for x in range.0..=range.2 {
                    self.cells.entry((x, y)).or_default().push(index);
                }
            }
        }
        true
    }

    fn usable(&self, rect: Rect) -> bool {
        rect.x.is_finite()
            && rect.y.is_finite()
            && rect.width.is_finite()
            && rect.height.is_finite()
            && rect.width >= self.minimum_side
            && rect.height >= self.minimum_side
    }

    fn covered_cells(&self, rect: Rect) -> (i32, i32, i32, i32) {
        let cell = |value: f64| (value / self.cell_size).floor() as i32;
        (
            cell(rect.x - self.padding),
            cell(rect.y - self.padding),
            cell(rect.right() + self.padding),
            cell(rect.bottom() + self.padding),
        )
    }

    fn next_query_generation(&mut self) {
        self.query_generation = self.query_generation.wrapping_add(1);
        if self.query_generation == 0 {
            self.marks.fill(0);
            self.query_generation = 1;
        }
    }
}

/// Batch provenance stays in the scanner, without enlarging public UiTarget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetSource {
    Accessibility,
    #[cfg(any(target_os = "windows", target_os = "macos", test))]
    SystemOcr,
    #[cfg(any(target_os = "macos", test))]
    NativeRectangles,
    Contour,
}

#[derive(Clone, Copy)]
struct Evidence {
    source: TargetSource,
    role: SemanticRole,
    active: bool,
}

impl Evidence {
    fn is_layout_container(&self) -> bool {
        matches!(
            self.role,
            SemanticRole::ListItem
                | SemanticRole::TreeItem
                | SemanticRole::Row
                | SemanticRole::Cell
        )
    }

    fn priority(&self) -> u8 {
        match self.source {
            TargetSource::Accessibility
                if !matches!(
                    self.role,
                    SemanticRole::StaticText
                        | SemanticRole::Image
                        | SemanticRole::Control
                        | SemanticRole::Unknown
                ) =>
            {
                5
            }
            TargetSource::Contour if self.role == SemanticRole::Image => 3,
            TargetSource::Contour => 1,
            _ => 2,
        }
    }
}

/// Incremental source-priority suppression. A late semantic control can retire
/// multiple visual fragments; the public delta preserves asynchronous delivery.
/// This is a deterministic evidence tier, not a calibrated probability score.
pub(crate) struct TargetIndex {
    spatial: SpatialIndex,
    evidence: Vec<Evidence>,
    captions: FxHashMap<usize, smol_str::SmolStr>,
    active: usize,
    retired: Vec<Rect>,
}

impl TargetIndex {
    pub(crate) fn new() -> Self {
        Self {
            spatial: SpatialIndex::new(64.0, 8.0, 2.0),
            evidence: Vec::new(),
            captions: FxHashMap::default(),
            active: 0,
            retired: Vec::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.active
    }

    /// Reuse the first native batch's completed grid for its publication cache.
    /// Exact identity checks also preserve the fallback for extreme floating
    /// point rectangles whose geometric matcher can accept identical inputs.
    pub(super) fn native_cache_snapshot(
        &self,
        targets: &[UiTarget],
        threshold: f64,
    ) -> Option<SpatialIndex> {
        if self.evidence.len() != targets.len() || self.active != targets.len() {
            return None;
        }
        let unique = self
            .evidence
            .iter()
            .zip(&self.spatial.rects)
            .zip(targets)
            .all(|((&entry, &rect), target)| {
                entry.source == TargetSource::Accessibility
                    && entry.role == target.role
                    && rect == target.rect
                    && evidence_matches(rect, entry, rect, entry, threshold)
            });
        unique.then(|| self.spatial.clone())
    }

    pub(crate) fn take_retired(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.retired)
    }

    /// Expand and merge one source's slice, returning its remaining prefix length.
    pub(super) fn expand_pending_text(&self, source: TargetSource, regions: &mut [Rect]) -> usize {
        let mut len = regions.len();
        let mut changed = true;
        while changed {
            changed = false;
            for region in &mut regions[..len] {
                for (entry, &rect) in self.evidence.iter().zip(&self.spatial.rects) {
                    if entry.active
                        && entry.source == source
                        && entry.role == SemanticRole::StaticText
                        && super::scan_progress::touches(*region, rect)
                    {
                        let union = region.union(&rect);
                        changed |= union != *region;
                        *region = union;
                    }
                }
            }
            let mut i = 0;
            while i < len {
                let mut j = i + 1;
                while j < len {
                    if super::scan_progress::touches(regions[i], regions[j]) {
                        // Compact like Vec::swap_remove; the caller truncates the suffix.
                        let other = regions[j];
                        len -= 1;
                        regions[j] = regions[len];
                        regions[i] = regions[i].union(&other);
                        changed = true;
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
        }
        len
    }

    /// Join only genuinely overlapping text runs (typically OCR tile seams).
    /// No proximity grouping, widget rules, or native actions participate.
    pub(crate) fn stitch_text(&mut self, target: &mut UiTarget, source: TargetSource) {
        if source == TargetSource::Accessibility || target.role != SemanticRole::StaticText {
            return;
        }
        // Bounded fixed point: one long tile can bridge multiple earlier runs.
        for _ in 0..4 {
            let old_rect = target.rect;
            let mut rect = old_rect;
            let evidence = &self.evidence;
            self.spatial.any_match(old_rect, |i, a, _| {
                let e = evidence[i];
                if e.active
                    && e.source == source
                    && e.role == SemanticRole::StaticText
                    && overlapping_text_runs(a, old_rect)
                {
                    let x = rect.x.min(a.x);
                    let y = rect.y.min(a.y);
                    rect = Rect::new(
                        x,
                        y,
                        rect.right().max(a.right()) - x,
                        rect.bottom().max(a.bottom()) - y,
                    );
                }
                false
            });
            target.rect = rect;
            if rect == old_rect {
                break;
            }
        }
    }

    pub(crate) fn retain_active(&mut self, targets: &mut Vec<UiTarget>, source: TargetSource) {
        let evidence = &self.evidence;
        targets.retain(|target| {
            self.spatial.any_match(target.rect, |index, old, new| {
                let entry = evidence[index];
                entry.active && entry.source == source && entry.role == target.role && old == new
            })
        });
    }

    #[cfg(test)]
    pub(crate) fn insert(
        &mut self,
        target: &UiTarget,
        source: TargetSource,
        threshold: f64,
    ) -> bool {
        let accepted =
            self.insert_with_caption_lookup::<true>(target, source, threshold, true, |_, _, _| {
                None
            });
        // Direct index tests have no accumulator-owned visual text cache.
        if accepted
            && source != TargetSource::Accessibility
            && target.role == SemanticRole::StaticText
            && !target.name.is_empty()
        {
            self.captions
                .insert(self.evidence.len() - 1, target.name.as_str().into());
        }
        accepted
    }

    pub(crate) fn insert_with_caption_lookup<'a, const CHECK_LAYOUT: bool>(
        &mut self,
        target: &UiTarget,
        source: TargetSource,
        threshold: f64,
        cache_native_captions: bool,
        mut cached_caption: impl FnMut(Rect, SemanticRole, Rect) -> Option<&'a str>,
    ) -> bool {
        let rect = target.rect;
        if !self.spatial.usable(rect) {
            return false;
        }
        let candidate = Evidence {
            source,
            role: target.role,
            active: true,
        };
        let candidate_layout =
            source == TargetSource::Accessibility && candidate.is_layout_container();
        let candidate_text = candidate.role == SemanticRole::StaticText;
        let evidence = &self.evidence;
        let captions = &self.captions;
        let mut suppressed = SmallVec::<[usize; 8]>::new();
        let blocked = self.spatial.any_match(rect, |index, a, b| {
            let old = evidence[index];
            if !old.active || !evidence_matches(a, old, b, candidate, threshold) {
                return false;
            }
            if CHECK_LAYOUT
                && (candidate_layout && old.role == SemanticRole::StaticText
                    || candidate_text
                        && old.source == TargetSource::Accessibility
                        && old.is_layout_container())
                && distinct_container_field(
                    (a, old.role),
                    (b, target.name.as_str()),
                    captions.get(&index).map(|text| text.as_str()),
                    threshold,
                    &mut cached_caption,
                )
            {
                return false;
            }
            if old.priority() >= candidate.priority() {
                let expands_text = old.source != TargetSource::Accessibility
                    && source != TargetSource::Accessibility
                    && old.role == SemanticRole::StaticText
                    && candidate.role == SemanticRole::StaticText
                    && a != b
                    && b.x <= a.x
                    && b.y <= a.y
                    && b.right() >= a.right()
                    && b.bottom() >= a.bottom()
                    && overlapping_text_runs(a, b);
                // Different OCR engines may segment one line as words vs a
                // phrase. Pick a stable representative instead of first arrival.
                let preferred_text = old.priority() == candidate.priority()
                    && old.source != TargetSource::Accessibility
                    && source != TargetSource::Accessibility
                    && old.role == SemanticRole::StaticText
                    && candidate.role == SemanticRole::StaticText
                    && (b.width * b.height)
                        .total_cmp(&(a.width * a.height))
                        .then_with(|| a.y.total_cmp(&b.y))
                        .then_with(|| a.x.total_cmp(&b.x))
                        .is_gt();
                // macOS' newly independent rectangle provider must converge
                // with late OCR. Keep this out of the Windows production path.
                #[cfg(any(target_os = "macos", test))]
                let preferred_text = preferred_text
                    || (old.source == TargetSource::NativeRectangles
                        && source == TargetSource::SystemOcr
                        && candidate.role == SemanticRole::StaticText);
                if !expands_text && !preferred_text {
                    return true;
                }
            }
            suppressed.push(index);
            false
        });
        if blocked {
            return false;
        }
        // Public retirements identify geometry. Do not withdraw an AX text
        // rectangle if an independent AX action shares those exact bounds.
        // Keeping the description is preferable to deleting that action.
        if source == TargetSource::Accessibility {
            suppressed.retain(|index| {
                let index = *index;
                let old = self.evidence[index];
                if old.source != source || old.role != SemanticRole::StaticText {
                    return true;
                }
                let evidence = &self.evidence;
                !self
                    .spatial
                    .any_match(self.spatial.rects[index], |other, a, b| {
                        let entry = evidence[other];
                        other != index
                            && entry.active
                            && entry.source == source
                            && entry.role != SemanticRole::StaticText
                            && a == b
                    })
            });
        }
        if !self.spatial.store(rect) {
            return false;
        }
        for index in suppressed {
            self.evidence[index].active = false;
            self.retired.push(self.spatial.rects[index]);
            self.active -= 1;
        }
        if CHECK_LAYOUT
            && cache_native_captions
            && source == TargetSource::Accessibility
            && !target.name.is_empty()
            && (target.role == SemanticRole::StaticText || candidate.is_layout_container())
        {
            self.captions
                .insert(self.evidence.len(), target.name.as_str().into());
        }
        self.evidence.push(candidate);
        self.active += 1;
        // Bound stale grid entries during long, heavily refined scans. No
        // published ID depends on these internal indices.
        if self.evidence.len() > 512 && self.evidence.len() > self.active.saturating_mul(2) {
            let mut spatial = SpatialIndex::new(64.0, 8.0, 2.0);
            let mut captions = FxHashMap::default();
            let mut index = 0;
            self.evidence.retain(|entry| {
                let rect = self.spatial.rects[index];
                let old_index = index;
                index += 1;
                if entry.active {
                    if let Some(text) = self.captions.remove(&old_index) {
                        captions.insert(spatial.len(), text);
                    }
                    spatial.store(rect);
                }
                entry.active
            });
            self.spatial = spatial;
            self.captions = captions;
        }
        true
    }

    pub(crate) fn clear_batch_captions(&mut self) {
        self.captions.clear();
    }
}

/// Field text is a cold refinement of an already matching region pair. Keep
/// its lookup and normalization out of ordinary contour/OCR insertion frames.
#[inline(never)]
fn distinct_container_field<'a>(
    old: (Rect, SemanticRole),
    new: (Rect, &str),
    caption: Option<&str>,
    threshold: f64,
    cached_caption: &mut impl FnMut(Rect, SemanticRole, Rect) -> Option<&'a str>,
) -> bool {
    let (a, role) = old;
    let (b, name) = new;
    let old_container = matches!(
        role,
        SemanticRole::ListItem | SemanticRole::TreeItem | SemanticRole::Row | SemanticRole::Cell
    );
    let old_name = caption
        .or_else(|| cached_caption(a, role, if old_container { b } else { a }))
        .unwrap_or("");
    if old_container {
        distinct_field(a, b, old_name, name, threshold)
    } else {
        distinct_field(a, b, name, old_name, threshold)
    }
}

/// Geometry establishes an eligible pair before any caption work. Most
/// distinct fields differ at the first character, so normalization is lazy.
#[inline]
fn distinct_field(a: Rect, b: Rect, left: &str, text: &str, threshold: f64) -> bool {
    if text.is_empty() || left == text {
        return false;
    }
    let intersection = (a.right().min(b.right()) - a.x.max(b.x)).max(0.)
        * (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.);
    if intersection
        >= threshold.clamp(0., 1.) * (a.width * a.height + b.width * b.height - intersection)
    {
        return false;
    }
    if !left
        .chars()
        .filter(|c| !c.is_whitespace())
        .eq(text.chars().filter(|c| !c.is_whitespace()))
    {
        return text
            .chars()
            .any(|c| !c.is_whitespace() || c == '\n' || c == '\r');
    }
    let normalize = |text: &str| {
        let mut output = smol_str::SmolStrBuilder::new();
        let _ = crate::api::presentation::write_ocr_text(&mut output, text.chars());
        output
    };
    let text = normalize(text);
    !text.is_empty() && normalize(left).as_str() != text.as_str()
}

fn native_rectangles_match(a: Rect, b: Rect, threshold: f64) -> bool {
    let width = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let height = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    native_overlap_matches(
        width * height,
        a.width * a.height,
        b.width * b.height,
        threshold,
    )
}

fn native_overlap_matches(intersection: f64, a_area: f64, b_area: f64, threshold: f64) -> bool {
    intersection > 0.0
        && intersection >= threshold.clamp(0.0, 1.0) * (a_area + b_area - intersection)
}

fn evidence_matches(a: Rect, ae: Evidence, b: Rect, be: Evidence, threshold: f64) -> bool {
    let iw = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0);
    let ih = (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
    let intersection = iw * ih;
    let aa = a.width * a.height;
    let ba = b.width * b.height;
    let image = |e: Evidence| e.source == TargetSource::Contour && e.role == SemanticRole::Image;
    if image(ae) || image(be) {
        let (owner, other, other_e) = if image(ae) { (a, b, be) } else { (b, a, ae) };
        // Native actions inside an image remain independent; visual evidence
        // cannot revoke them. This predicate is symmetric in arrival order.
        if other_e.source == TargetSource::Accessibility {
            return false;
        }
        if image(ae) && image(be) {
            return intersection >= 0.85 * (aa + ba - intersection);
        }
        return owner.width * owner.height > other.width * other.height
            && intersection >= 0.9 * other.width * other.height;
    }
    if ae.source == be.source {
        if ae.source == TargetSource::Accessibility {
            // Static text describes its containing control; it
            // is not another action. Keep independently actionable children
            // (checkboxes, links, cells, buttons) even inside a selectable row.
            let description = |role| role == SemanticRole::StaticText;
            let owned = if ae.priority() == 5 && description(be.role) {
                intersection >= ba * 0.9
            } else if be.priority() == 5 && description(ae.role) {
                intersection >= aa * 0.9
            } else {
                false
            };
            if intersection > 0.0 && owned {
                return true;
            }
            // A row and its independent checkbox/button must both survive.
            return ae.role == be.role && native_overlap_matches(intersection, aa, ba, threshold);
        }
        return rectangles_match(a, b, threshold, 8.0);
    }
    if intersection <= 0.0 {
        return false;
    }
    if ae.source != TargetSource::Accessibility
        && be.source != TargetSource::Accessibility
        && ae.role == SemanticRole::StaticText
        && be.role == SemanticRole::StaticText
    {
        // Symmetric coverage: a short OCR word contained in a phrase must
        // match regardless of which provider supplied the phrase first.
        let small_height = a.height.min(b.height);
        return a.height.max(b.height) <= small_height * 1.6
            && (a.center().y - b.center().y).abs() <= small_height * 0.35
            && intersection >= 0.8 * aa.min(ba);
    }
    if (ae.source == TargetSource::Accessibility && ae.priority() < 3)
        || (be.source == TargetSource::Accessibility && be.priority() < 3)
    {
        // A generic accessible container is not evidence that everything
        // inside it shares one click action.
        return intersection >= threshold.clamp(0.0, 1.0) * (aa + ba - intersection);
    }
    let (high, he, low, low_area) = if ae.priority() >= be.priority() {
        (a, ae, b, ba)
    } else {
        (b, be, a, aa)
    };
    // OCR and contour see different stroke extents. A small glyph fragment
    // can protrude beyond a text line without being a separate button. Require
    // its center and almost all of its width inside the text band, with only
    // a font-relative margin (bounded to four screen-coordinate units).
    if he.source != TargetSource::Accessibility
        && he.role == SemanticRole::StaticText
        && (ae.source == TargetSource::Contour || be.source == TargetSource::Contour)
    {
        let margin = (high.height * 0.2).min(4.0);
        let center = low.center();
        let covered_width = (high.right() + margin).min(low.right()) - (high.x - margin).max(low.x);
        // A tight edge envelope can also surround the OCR box (especially a
        // short word). Symmetric coverage handles that direction without
        // promoting an enclosing panel to the same text target.
        if low.height <= high.height * 1.6
            && low.width <= high.width * 1.6
            && intersection >= high.width * high.height * 0.9
            && low_area <= high.width * high.height * 2.0
        {
            return true;
        }
        if low.height <= high.height * 1.6
            && center.x >= high.x - margin
            && center.x <= high.right() + margin
            && center.y >= high.y - margin
            && center.y <= high.bottom() + margin
            && covered_width >= low.width * 0.8
        {
            return true;
        }
    }
    if he.priority() >= 3 {
        // Native actions own their fragments. Generic containers/static text
        // cannot absorb an entire region based on appearance alone.
        return intersection >= 0.8 * low_area
            || intersection >= threshold.clamp(0.0, 1.0) * (aa + ba - intersection);
    }
    // OCR may own jittered glyph contours, but not a nearby disjoint icon or
    // an enclosing panel. Require substantial coverage of the visual target.
    let aligned = (high.center().y - low.center().y).abs() <= high.height.max(low.height) * 0.5;
    aligned && intersection >= 0.6 * low_area
}

fn overlapping_text_runs(a: Rect, b: Rect) -> bool {
    let small = a.height.min(b.height);
    let tall = a.height.max(b.height);
    a.width >= a.height * 2.0
        && b.width >= b.height * 2.0
        && small > 0.0
        && tall <= small * 1.6
        && (a.center().y - b.center().y).abs() <= tall * 0.3
        && a.intersect(&b)
            .is_some_and(|r| r.height >= small * 0.7 && r.width >= small * 0.5)
}

/// Shared visual duplicate predicate used after independent native providers
/// have already validated their rectangles.
pub(crate) fn rectangles_match(a: Rect, b: Rect, iou_threshold: f64, minimum_spacing: f64) -> bool {
    let ac = a.center();
    let bc = b.center();
    let dx = ac.x - bc.x;
    let dy = ac.y - bc.y;
    let spacing = minimum_spacing.max(1.0);
    let same_baseline = dy.abs() <= (a.height.min(b.height) * 0.35).max(2.0);
    let near = same_baseline && dx * dx + dy * dy < spacing * spacing;

    let intersection_width = a.right().min(b.right()) - a.x.max(b.x);
    let intersection_height = a.bottom().min(b.bottom()) - a.y.max(b.y);
    if intersection_width <= 0.0 || intersection_height <= 0.0 {
        return near;
    }
    let intersection = intersection_width * intersection_height;
    let a_area = a.width * a.height;
    let b_area = b.width * b.height;
    let union = a_area + b_area - intersection;
    let iou_match = union > 0.0 && intersection >= iou_threshold.clamp(0.0, 1.0) * union;
    let containment_match = intersection >= 0.8 * a_area.min(b_area).max(1.0);
    iou_match || containment_match || near
}

#[cfg(test)]
mod tests {
    use super::*;
    use stats_alloc::{INSTRUMENTED_SYSTEM, Region};
    use std::time::Instant;

    fn target(rect: Rect, role: SemanticRole) -> UiTarget {
        UiTarget {
            details: None,
            rect,
            role,
            name: String::new(),
        }
    }

    #[test]
    fn native_prefilter_checks_semantics_overlap_and_threshold() {
        let target = |rect, name: &str, role| UiTarget {
            rect,
            name: name.into(),
            role,
            details: None,
        };
        let mut deduper = NativeTargetDeduper::new(0.5);
        let bounds = Rect::new(10.0, 10.0, 30.0, 20.0);
        assert!(deduper.insert(&target(bounds, "Save", SemanticRole::Button)));
        assert!(!deduper.insert(&target(
            Rect::new(10.02, 10.02, 30.01, 20.01),
            "Save",
            SemanticRole::Button
        )));
        assert!(!deduper.insert(&target(
            Rect::new(11.0, 11.0, 30.0, 20.0),
            "Save",
            SemanticRole::Button
        )));
        assert!(deduper.insert(&target(bounds, "Cancel", SemanticRole::Button)));
        assert!(deduper.insert(&target(bounds, "Save", SemanticRole::Link)));
        for name in [
            "复制",
            "abcdefghijklmnopqrstuvw",
            "abcdefghijklmnopqrstuvwx",
        ] {
            let control = target(bounds, name, SemanticRole::Button);
            assert!(deduper.insert(&control));
            assert!(!deduper.insert(&control));
        }
        // Nearby small buttons and a differently sized enclosing control are
        // separate candidates, even with the same caption and role.
        let mut deduper = NativeTargetDeduper::new(0.5);
        assert!(deduper.insert(&target(
            Rect::new(0.0, 0.0, 2.0, 2.0),
            "A",
            SemanticRole::Button
        )));
        assert!(deduper.insert(&target(
            Rect::new(4.0, 0.0, 2.0, 2.0),
            "A",
            SemanticRole::Button
        )));
        assert!(deduper.insert(&target(
            Rect::new(-49.0, -49.0, 100.0, 100.0),
            "A",
            SemanticRole::Button
        )));
        // Both the first rectangle and later same-cell entries remain visible.
        for rect in [
            Rect::new(0.0, 0.0, 2.0, 2.0),
            Rect::new(4.0, 0.0, 2.0, 2.0),
            Rect::new(-49.0, -49.0, 100.0, 100.0),
        ] {
            assert!(!deduper.insert(&target(rect, "A", SemanticRole::Button)));
        }
        // Larger offsets are still left to fusion, even when the first
        // rectangle has sufficient IoU. Storing it with the name must not
        // broaden the native prefilter's neighborhood.
        let mut deduper = NativeTargetDeduper::new(0.5);
        for x in [0.0, 24.0] {
            assert!(deduper.insert(&target(
                Rect::new(x, 0.0, 100.0, 100.0),
                "Save",
                SemanticRole::Button
            )));
        }
        assert!(!deduper.insert(&target(
            Rect::new(24.0, 0.0, 100.0, 100.0),
            "Save",
            SemanticRole::Button
        )));
        // The source uses the configured fusion threshold, including its bounds.
        for threshold in [0.0, 0.5, 1.0] {
            let mut deduper = NativeTargetDeduper::new(threshold);
            let first = target(bounds, "Save", SemanticRole::Button);
            let second = target(
                Rect::new(11.0, 11.0, 30.0, 20.0),
                "Save",
                SemanticRole::Button,
            );
            assert!(deduper.insert(&first));
            let mut index = TargetIndex::new();
            assert!(index.insert(&first, TargetSource::Accessibility, threshold));
            assert_eq!(
                deduper.insert(&second),
                index.insert(&second, TargetSource::Accessibility, threshold)
            );
        }
    }

    #[test]
    fn native_prefilter_checks_identity_with_multiple_hash_collisions() {
        let rect = Rect::new(10.0, 10.0, 30.0, 20.0);
        let candidate = UiTarget {
            rect,
            name: "candidate".into(),
            role: SemanticRole::Button,
            details: None,
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        candidate.name.hash(&mut hasher);
        candidate.role.hash(&mut hasher);
        let hash = hasher.finish();
        let mut deduper = NativeTargetDeduper::new(0.5);
        // Model two distinct identities with the same hash before the real
        // candidate. Hash equality alone must never make them duplicates.
        deduper.semantics.extend([
            ("other".into(), SemanticRole::Button, rect),
            ("candidate".into(), SemanticRole::Link, rect),
        ]);
        deduper
            .semantic_buckets
            .insert(hash, smallvec::smallvec![0, 1]);

        assert!(deduper.insert(&candidate));
        assert!(!deduper.insert(&candidate));
        assert_eq!(deduper.semantic_buckets[&hash].as_slice(), &[0, 1, 2]);
    }

    #[test]
    fn native_prefilter_handles_extreme_native_coordinates() {
        let mut deduper = NativeTargetDeduper::new(0.5);
        for coordinate in [f64::from(i32::MAX) * 8.0, f64::from(i32::MIN) * 8.0] {
            let target = target(
                Rect::new(coordinate, coordinate, 30.0, 20.0),
                SemanticRole::Button,
            );
            assert!(deduper.insert(&target));
            assert!(!deduper.insert(&target));
        }
    }

    #[test]
    fn native_prefilter_keeps_dense_captions_and_screen_cells_separate() {
        let mut deduper = NativeTargetDeduper::new(0.5);
        let targets: Vec<_> = [-3840.0, 0.0, 3840.0]
            .into_iter()
            .flat_map(|x| {
                (0..64).map(move |index| UiTarget {
                    rect: Rect::new(x, -2160.0, 30.0, 20.0),
                    name: format!("Control {index}"),
                    role: SemanticRole::Button,
                    details: None,
                })
            })
            .collect();
        for target in &targets {
            assert!(deduper.insert(target));
        }
        for target in targets.iter().rev() {
            assert!(!deduper.insert(target));
        }
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn deduper_inline_buckets_performance_probe() {
        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        const BUCKETS: usize = 100;

        fn fill_inline() {
            let mut cells: HashMap<usize, SmallVec<[usize; 2]>> = HashMap::new();
            for index in 0..BUCKETS {
                cells.entry(index).or_default().push(index);
            }
            std::hint::black_box(cells);
        }

        fn fill_heap() {
            let mut cells: HashMap<usize, Vec<usize>> = HashMap::new();
            for index in 0..BUCKETS {
                cells.entry(index).or_default().push(index);
            }
            std::hint::black_box(cells);
        }

        fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        for _ in 0..WARMUP {
            fill_inline();
            fill_heap();
        }
        let mut inline_samples = Vec::with_capacity(SAMPLES);
        let mut heap_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: fn(), samples: &mut Vec<u128>| {
                let started = Instant::now();
                operation();
                samples.push(started.elapsed().as_nanos());
            };
            if sample % 2 == 0 {
                measure(fill_inline, &mut inline_samples);
                measure(fill_heap, &mut heap_samples);
            } else {
                measure(fill_heap, &mut heap_samples);
                measure(fill_inline, &mut inline_samples);
            }
        }

        let inline_region = Region::new(&INSTRUMENTED_SYSTEM);
        fill_inline();
        let inline_allocations = inline_region.change().allocations;
        let heap_region = Region::new(&INSTRUMENTED_SYSTEM);
        fill_heap();
        let heap_allocations = heap_region.change().allocations;
        println!(
            "uia_deduper_bucket_probe samples={SAMPLES} buckets={BUCKETS} inline={:?} heap={:?} inline_allocations={inline_allocations} heap_allocations={heap_allocations}",
            percentiles(&mut inline_samples),
            percentiles(&mut heap_samples),
        );
    }

    #[test]
    fn enclosing_accessible_controls_preserve_distinct_visual_regions() {
        for role in [
            SemanticRole::ListItem,
            SemanticRole::TreeItem,
            SemanticRole::Row,
            SemanticRole::Cell,
        ] {
            for owner_first in [false, true] {
                let mut index = TargetIndex::new();
                let mut owner = target(Rect::new(0., 0., 480., 88.), role);
                owner.name = "Group, sender: preview, 6 unread, 20:21".into();
                let icon = target(Rect::new(8., 12., 56., 56.), SemanticRole::Image);
                let text = [
                    Rect::new(80., 12., 240., 20.),
                    Rect::new(388., 12., 68., 20.),
                    Rect::new(80., 48., 64., 20.),
                    Rect::new(156., 48., 210., 20.),
                    Rect::new(404., 48., 40., 20.),
                ];
                if owner_first {
                    assert!(index.insert(&owner, TargetSource::Accessibility, 0.5));
                }
                assert!(index.insert(&icon, TargetSource::Contour, 0.5));
                for (rect, name) in text
                    .into_iter()
                    .zip(["Group", "20:21", "sender:", "preview", "6"])
                {
                    let mut text = target(rect, SemanticRole::StaticText);
                    text.name = name.into();
                    assert!(index.insert(&text, TargetSource::SystemOcr, 0.5));
                }
                if !owner_first {
                    assert!(index.insert(&owner, TargetSource::Accessibility, 0.5));
                }
                assert_eq!(index.len(), 7, "{role:?} owner_first={owner_first}");
                assert!(index.take_retired().is_empty());
            }
        }
    }

    #[test]
    fn matching_single_captions_and_button_fragments_keep_existing_deduplication() {
        for role in [
            SemanticRole::Button,
            SemanticRole::ListItem,
            SemanticRole::Cell,
        ] {
            for owner_first in [false, true] {
                let mut index = TargetIndex::new();
                let mut owner = target(Rect::new(0., 0., 200., 60.), role);
                owner.name = "图片".into();
                let mut text = target(Rect::new(70., 20., 60., 20.), SemanticRole::StaticText);
                text.name = "图 片".into();
                if owner_first {
                    assert!(index.insert(&owner, TargetSource::Accessibility, 0.5));
                }
                assert_eq!(
                    index.insert(&text, TargetSource::SystemOcr, 0.5),
                    !owner_first
                );
                if !owner_first {
                    assert!(index.insert(&owner, TargetSource::Accessibility, 0.5));
                }
                assert_eq!(index.len(), 1);
            }
        }
    }

    #[test]
    fn description_sharing_geometry_with_an_independent_action_is_not_retired() {
        let mut index = TargetIndex::new();
        let rect = Rect::new(10.0, 5.0, 30.0, 20.0);
        index.insert(
            &target(rect, SemanticRole::StaticText),
            TargetSource::Accessibility,
            0.5,
        );
        index.insert(
            &target(rect, SemanticRole::Image),
            TargetSource::Accessibility,
            0.5,
        );
        index.insert(
            &target(Rect::new(0.0, 0.0, 300.0, 32.0), SemanticRole::ListItem),
            TargetSource::Accessibility,
            0.5,
        );
        assert_eq!(index.len(), 3);
        assert!(index.take_retired().is_empty());
    }

    #[test]
    fn semantic_control_owns_description_but_not_independent_actions() {
        let row = target(Rect::new(0.0, 0.0, 400.0, 32.0), SemanticRole::ListItem);
        for owner_first in [false, true] {
            let mut index = TargetIndex::new();
            let text = target(Rect::new(80.0, 5.0, 60.0, 20.0), SemanticRole::StaticText);
            if owner_first {
                assert!(index.insert(&row, TargetSource::Accessibility, 0.5));
            }
            assert_eq!(
                index.insert(&text, TargetSource::Accessibility, 0.5),
                !owner_first
            );
            if !owner_first {
                assert!(index.insert(&row, TargetSource::Accessibility, 0.5));
            }
            assert_eq!(index.len(), 1);
            for role in [
                SemanticRole::Checkbox,
                SemanticRole::Button,
                SemanticRole::Link,
                SemanticRole::Cell,
                SemanticRole::Image,
            ] {
                assert!(index.insert(&target(text.rect, role), TargetSource::Accessibility, 0.5));
            }
        }
    }

    #[test]
    fn text_band_absorbs_protruding_strokes_but_keeps_adjacent_icons_and_rows() {
        let text = target(Rect::new(30.0, 10.0, 100.0, 20.0), SemanticRole::StaticText);
        let fragment = target(Rect::new(45.0, 21.0, 18.0, 20.0), SemanticRole::Control);
        for text_first in [false, true] {
            let mut index = TargetIndex::new();
            if text_first {
                index.insert(&text, TargetSource::SystemOcr, 0.5);
            }
            assert_eq!(
                index.insert(&fragment, TargetSource::Contour, 0.5),
                !text_first
            );
            if !text_first {
                index.insert(&text, TargetSource::SystemOcr, 0.5);
            }
            assert_eq!(index.len(), 1);
            for rect in [
                Rect::new(5.0, 10.0, 20.0, 20.0),
                Rect::new(40.0, 39.0, 20.0, 20.0),
            ] {
                assert!(index.insert(
                    &target(rect, SemanticRole::Control),
                    TargetSource::Contour,
                    0.5
                ));
            }
        }
    }

    #[test]
    fn semantic_row_replaces_jittered_visual_fragments_in_either_arrival_order() {
        let row = target(Rect::new(10.0, 100.0, 900.0, 32.0), SemanticRole::ListItem);
        let fragments = [
            target(Rect::new(20.0, 103.0, 70.0, 18.0), SemanticRole::Control),
            target(Rect::new(600.0, 108.0, 100.0, 20.0), SemanticRole::Control),
        ];
        for row_first in [false, true] {
            let mut index = TargetIndex::new();
            if row_first {
                assert!(index.insert(&row, TargetSource::Accessibility, 0.5));
            }
            for fragment in &fragments {
                assert_eq!(
                    index.insert(fragment, TargetSource::Contour, 0.5),
                    !row_first
                );
            }
            if !row_first {
                assert!(index.insert(&row, TargetSource::Accessibility, 0.5));
            }
            assert_eq!(index.len(), 1);
            assert_eq!(index.take_retired().len(), if row_first { 0 } else { 2 });
            // Same row does not imply same action: an accessible checkbox survives.
            assert!(index.insert(
                &target(Rect::new(14.0, 106.0, 16.0, 16.0), SemanticRole::Checkbox),
                TargetSource::Accessibility,
                0.5
            ));
            assert!(index.insert(
                &target(Rect::new(10.0, 135.0, 900.0, 32.0), SemanticRole::ListItem),
                TargetSource::Accessibility,
                0.5
            ));
        }
    }

    #[test]
    fn text_envelope_is_symmetric_but_does_not_absorb_a_panel_or_native_button() {
        let text = target(Rect::new(12.0, 13.0, 10.0, 14.0), SemanticRole::StaticText);
        let edge = target(Rect::new(10.0, 10.0, 14.0, 20.0), SemanticRole::Control);
        for text_first in [false, true] {
            let mut index = TargetIndex::new();
            if text_first {
                index.insert(&text, TargetSource::SystemOcr, 0.5);
            }
            assert_eq!(index.insert(&edge, TargetSource::Contour, 0.5), !text_first);
            if !text_first {
                assert!(index.insert(&text, TargetSource::SystemOcr, 0.5));
            }
            assert_eq!(index.len(), 1);
            let panel = target(Rect::new(0.0, 0.0, 300.0, 150.0), SemanticRole::Control);
            assert!(index.insert(&panel, TargetSource::Contour, 0.5));
            let mut button = edge.clone();
            button.role = SemanticRole::Button;
            assert!(index.insert(&button, TargetSource::Accessibility, 0.5));
        }
    }

    #[test]
    fn ocr_replaces_contour_fragments_but_does_not_merge_disjoint_neighbors() {
        let mut index = TargetIndex::new();
        let glyph = target(Rect::new(12.0, 12.0, 20.0, 12.0), SemanticRole::Control);
        let text = target(Rect::new(10.0, 8.0, 120.0, 22.0), SemanticRole::Control);
        assert!(index.insert(&glyph, TargetSource::Contour, 0.5));
        assert!(index.insert(&text, TargetSource::SystemOcr, 0.5));
        assert_eq!(index.len(), 1);
        assert_eq!(index.take_retired(), vec![glyph.rect]);
        assert!(index.insert(
            &target(Rect::new(134.0, 10.0, 20.0, 20.0), SemanticRole::Control),
            TargetSource::Contour,
            0.5
        ));
        assert!(index.insert(
            &target(Rect::new(10.0, 34.0, 120.0, 22.0), SemanticRole::Control),
            TargetSource::SystemOcr,
            0.5
        ));
    }

    #[test]
    fn large_scans_keep_replacements_and_continue_growing() {
        let mut index = TargetIndex::new();
        for i in 0..10_025 {
            assert!(index.insert(
                &target(
                    Rect::new((i % 100) as f64 * 80.0, (i / 100) as f64 * 40.0, 20.0, 20.0),
                    SemanticRole::Control
                ),
                TargetSource::Contour,
                0.5
            ));
        }
        assert!(index.insert(
            &target(Rect::new(0.0, 0.0, 24.0, 24.0), SemanticRole::Button),
            TargetSource::Accessibility,
            0.5
        ));
        assert_eq!(index.len(), 10_025);
        assert_eq!(index.take_retired().len(), 1);
        assert!(index.insert(
            &target(Rect::new(-100.0, -100.0, 20.0, 20.0), SemanticRole::Button),
            TargetSource::Accessibility,
            0.5
        ));
    }

    #[test]
    fn batch_queries_visit_the_complete_candidate_union_once() {
        let mut index = SpatialIndex::new(64., 8., 2.);
        for rect in [
            Rect::new(-80., 5., 100., 25.),
            Rect::new(0., 0., 700., 500.), // oversize storage
            Rect::new(65., 10., 40., 20.),
            Rect::new(900., 700., 40., 20.),
        ] {
            assert!(index.store(rect));
        }
        let queries = [
            Rect::new(-30., 0., 70., 30.),
            Rect::new(10., 5., 90., 20.),
            Rect::new(895., 695., 60., 40.),
        ];
        let mut expected = std::collections::BTreeSet::new();
        for query in queries {
            index.any_match(query, |id, _, _| {
                expected.insert(id);
                false
            });
        }
        // A later batch must revisit targets; an empty batch visits none.
        for query_batch in [&queries[..], &[][..], &queries[..]] {
            let mut visits = Vec::new();
            index.for_each_candidate(query_batch.iter().copied(), |id, _| visits.push(id));
            let unique: std::collections::BTreeSet<_> = visits.iter().copied().collect();
            assert_eq!(visits.len(), unique.len());
            if query_batch.is_empty() {
                assert!(visits.is_empty());
            } else {
                assert_eq!(unique, expected);
            }
        }
    }

    #[test]
    fn grid_deduplication_matches_exhaustive_scan_across_distributions() {
        for (spacing, width, height) in [4.0, 30.0, 70.0, 256.0]
            .into_iter()
            .flat_map(|spacing| [(spacing, 2.0, 2.0), (spacing, 64.0, 24.0)])
        {
            let mut index = SpatialIndex::new(64.0, 8.0, 2.0);
            let mut accepted = Vec::<Rect>::new();
            for i in 0..1_000 {
                let cell = i / 2;
                let oversized = cell % 97 == 0;
                let rect = Rect::new(
                    (cell % 29) as f64 * spacing - 800.0,
                    (cell / 29) as f64 * spacing - 300.0,
                    if oversized { 600.0 } else { width },
                    if oversized { 400.0 } else { height },
                );
                let unique = !accepted
                    .iter()
                    .any(|&old| rectangles_match(old, rect, 0.5, 8.0));
                assert_eq!(
                    index.insert_if_unique(rect, |a, b| rectangles_match(a, b, 0.5, 8.0)),
                    unique
                );
                if unique {
                    accepted.push(rect);
                }
                assert_eq!(index.len(), accepted.len());
            }
        }
    }

    #[test]
    fn query_padding_keeps_nearby_rectangles_across_cell_edges() {
        for edge in [-64.0, 0.0, 64.0] {
            let mut index = SpatialIndex::new(64.0, 8.0, 2.0);
            let first = Rect::new(edge - 5.0, edge - 5.0, 2.0, 2.0);
            let nearby = Rect::new(edge + 1.0, edge - 5.0, 2.0, 2.0);
            let matcher = |a, b| rectangles_match(a, b, 0.5, 8.0);
            assert!(first.intersect(&nearby).is_none());
            assert!(matcher(first, nearby));
            assert!(index.insert_if_unique(first, matcher));
            assert!(!index.insert_if_unique(nearby, matcher));
            let mut fusion = TargetIndex::new();
            assert!(fusion.insert(
                &target(first, SemanticRole::Control),
                TargetSource::Contour,
                0.5
            ));
            assert!(!fusion.insert(
                &target(nearby, SemanticRole::Control),
                TargetSource::Contour,
                0.5
            ));
        }
    }

    #[test]
    #[ignore = "run alone with --test-threads=1 to isolate allocation counts"]
    fn scan_index_allocation_profile() {
        for count in [100, 2_000] {
            let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
            let mut index = SpatialIndex::new(64.0, 8.0, 2.0);
            for i in 0..count {
                let rect = Rect::new(
                    (i % 50) as f64 * 70.0 - 800.0,
                    (i / 50) as f64 * 70.0 - 300.0,
                    64.0,
                    24.0,
                );
                assert!(index.insert_if_unique(rect, |a, b| rectangles_match(a, b, 0.5, 8.0)));
            }
            std::hint::black_box(&index);
            println!("scan_index targets={count} {:?}", region.change());
        }
    }

    #[test]
    fn indexes_negative_coordinates_and_oversize_rectangles() {
        let mut index = SpatialIndex::new(64.0, 8.0, 2.0);
        let same = |a: Rect, b: Rect| a.intersect(&b).is_some();
        assert!(index.insert_if_unique(Rect::new(-500.0, -200.0, 400.0, 300.0), same));
        assert!(!index.insert_if_unique(Rect::new(-200.0, -100.0, 8.0, 8.0), same));
        assert!(index.insert_if_unique(Rect::new(100.0, 100.0, 8.0, 8.0), same));
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn queries_without_building_a_candidate_container() {
        let mut index = SpatialIndex::new(64.0, 0.0, 1.0);
        let close = |a: Rect, b: Rect| {
            let ac = a.center();
            let bc = b.center();
            (ac.x - bc.x).abs() < 4.0 && (ac.y - bc.y).abs() < 4.0
        };
        assert!(index.insert_if_unique(Rect::new(10.0, 10.0, 10.0, 10.0), close));
        assert!(!index.insert_if_unique(Rect::new(12.0, 12.0, 10.0, 10.0), close));
        assert!(index.insert_if_unique(Rect::new(80.0, 12.0, 10.0, 10.0), close));
    }

    #[test]
    fn shared_matcher_covers_iou_containment_and_same_baseline_proximity() {
        assert!(rectangles_match(
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Rect::new(2.0, 2.0, 20.0, 20.0),
            0.5,
            8.0,
        ));
        assert!(rectangles_match(
            Rect::new(0.0, 0.0, 30.0, 30.0),
            Rect::new(4.0, 4.0, 5.0, 5.0),
            0.9,
            8.0,
        ));
        assert!(rectangles_match(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(5.0, 1.0, 10.0, 10.0),
            0.9,
            8.0,
        ));
        assert!(!rectangles_match(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Rect::new(5.0, 20.0, 10.0, 10.0),
            0.9,
            8.0,
        ));
    }

    #[test]
    fn extreme_finite_coordinates_cannot_overflow_cell_accounting() {
        let mut index = SpatialIndex::new(64.0, 8.0, 1.0);
        assert!(index.insert_if_unique(
            Rect::new(-f64::MAX / 4.0, 0.0, f64::MAX / 2.0, 10.0),
            |_, _| false,
        ));
        assert_eq!(index.len(), 1);
    }
}
