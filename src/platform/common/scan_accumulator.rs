//! Shared fusion; semantic targets stream while provisional geometry settles.

use smallvec::SmallVec;

use crate::api::{Rect, SemanticRole, UiTarget};

use super::spatial_index::{TargetIndex, TargetSource};

pub(crate) struct ScanAccumulator {
    index: TargetIndex,
    visual_text: Vec<UiTarget>,
    text_spatial: super::spatial_index::SpatialIndex,
    images: Vec<UiTarget>,
    search_targets: Vec<Option<UiTarget>>,
    ocr_scratch: OcrScratch,
    search_spatial: super::spatial_index::SpatialIndex,
    has_layout_containers: bool,
    defer_visual: bool,
    // Parallel to search_targets only when deferral is enabled. Reuse the
    // canonical cache instead of retaining another copy of visual targets.
    deferred: Vec<bool>,
    progress: super::scan_progress::ScanProgress,
}

pub(crate) struct ScanUpdate {
    pub(crate) accepted: usize,
    pub(crate) targets: Vec<UiTarget>,
    pub(crate) retired: Vec<Rect>,
}

impl ScanAccumulator {
    pub(crate) fn new() -> Self {
        Self {
            index: TargetIndex::new(),
            visual_text: Vec::new(),
            text_spatial: super::spatial_index::SpatialIndex::new(64.0, 0.0, 0.0),
            images: Vec::new(),
            search_targets: Vec::new(),
            ocr_scratch: OcrScratch::default(),
            search_spatial: super::spatial_index::SpatialIndex::new(64.0, 8.0, 2.0),
            has_layout_containers: false,
            defer_visual: false,
            deferred: Vec::new(),
            progress: super::scan_progress::ScanProgress::default(),
        }
    }

    pub(crate) fn defer_visual(mut self, enabled: bool) -> Self {
        self.defer_visual = enabled;
        self
    }

    pub(crate) fn progress_sources(mut self, sources: &[TargetSource]) -> Self {
        self.progress = super::scan_progress::ScanProgress::new(sources);
        self
    }

    pub(crate) fn progress(
        &mut self,
        source: TargetSource,
        progress: super::scan_progress::Progress,
    ) -> Vec<UiTarget> {
        self.progress.update(source, progress);
        self.take_ready()
    }

    fn take_ready(&mut self) -> Vec<UiTarget> {
        if !self.defer_visual
            || !self.deferred.iter().any(|&pending| pending)
            || !self.progress.prepare(&self.index)
        {
            return Vec::new();
        }
        self.search_targets
            .iter()
            .zip(&mut self.deferred)
            .filter_map(|(target, pending)| {
                if *pending
                    && let Some(target) = target
                    && self.progress.ready(target.rect)
                {
                    *pending = false;
                    Some(target.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Each input batch has one provenance. Semantic owners can retire an
    /// earlier text descendant in the same batch; prune before publication.
    /// The single-writer fusion worker publishes each returned delta atomically.
    // Keep this large batch pipeline in one function instead of expanding it
    // into each caller's event-processing loop.
    #[inline(never)]
    pub(crate) fn push(
        &mut self,
        source: TargetSource,
        mut targets: Vec<UiTarget>,
        threshold: f64,
    ) -> ScanUpdate {
        let text_start = self.visual_text.len();
        let has_text = source != TargetSource::Accessibility
            && source != TargetSource::Contour
            && targets.iter().any(|t| t.role == SemanticRole::StaticText);
        if has_text {
            for t in &targets {
                if t.role == SemanticRole::StaticText && !t.name.is_empty() {
                    // Evidence needs only geometry and normalized text, not a
                    // second copy of the provider's metadata/boxed strings.
                    let mut name = String::with_capacity(t.name.len());
                    let _ = crate::api::presentation::write_ocr_text(&mut name, t.name.chars());
                    self.text_spatial.store(t.rect);
                    self.visual_text.push(UiTarget {
                        rect: t.rect,
                        role: t.role,
                        name,
                        details: None,
                    });
                }
            }
        }
        self.has_layout_containers |= source == TargetSource::Accessibility
            && targets.iter().any(|target| {
                matches!(
                    target.role,
                    SemanticRole::ListItem
                        | SemanticRole::TreeItem
                        | SemanticRole::Row
                        | SemanticRole::Cell
                )
            });
        if self.has_layout_containers {
            self.insert_targets::<true>(&mut targets, source, threshold);
        } else {
            self.insert_targets::<false>(&mut targets, source, threshold);
        }
        // Dedup/stitching reads only geometry, role and the original caption.
        // Preserve all OCR evidence above, but enrich only accepted targets;
        // rejected contours need neither metadata nor image-description queries.
        if source != TargetSource::Accessibility {
            for target in &mut targets {
                if target.details.is_none() {
                    target.details = Some(Box::new(crate::api::geometry::UiTargetDetails {
                        ocr: if source != TargetSource::Contour
                            && target.role == SemanticRole::StaticText
                        {
                            target.name.clone()
                        } else {
                            String::new()
                        },
                        ..Default::default()
                    }));
                }
                if source == TargetSource::Contour && target.role == SemanticRole::Image {
                    image_name(
                        target,
                        &self.visual_text,
                        &mut self.ocr_scratch,
                        &mut self.text_spatial,
                    );
                }
            }
        }
        let mut retired = self.index.take_retired();
        let mut descriptions = Vec::new();
        if !retired.is_empty() {
            descriptions = targets
                .iter()
                .filter(|t| !t.name.is_empty() && retired.contains(&t.rect))
                .cloned()
                .collect();
            self.index.retain_active(&mut targets, source);
        }
        if !retired.is_empty() {
            self.index
                .retain_active(&mut self.images, TargetSource::Contour);
        }
        if has_text {
            // A late OCR description updates the existing image in place via
            // an atomic same-geometry replacement. It must not create a second
            // hint or disappear just because the image arrived first.
            for image in &mut self.images {
                if !self.visual_text[text_start..].iter().any(|t| {
                    t.rect
                        .intersect(&image.rect)
                        .is_some_and(|i| i.width * i.height >= 0.9 * t.rect.width * t.rect.height)
                }) {
                    continue;
                }
                if image_name(
                    image,
                    &self.visual_text,
                    &mut self.ocr_scratch,
                    &mut self.text_spatial,
                ) {
                    retired.push(image.rect);
                    targets.push(image.clone());
                }
            }
        }
        let accepted = targets.len();
        // Fusion order remains unchanged even for unpublished visual candidates.
        // Only retire identities that have actually reached the consumer.
        let mut previous = Vec::new();
        let preserve_names = !retired.is_empty() && targets.iter().any(needs_replaced_names);
        retired.retain(|&rect| {
            let mut visible = false;
            self.search_spatial.any_match(rect, |index, old, _| {
                if old == rect
                    && let Some(target) = self.search_targets[index].take()
                {
                    // Retired candidates must not keep coverage preparation
                    // active after their last target has disappeared.
                    visible |= !self.deferred.get_mut(index).is_some_and(std::mem::take);
                    if preserve_names && !target.name.is_empty() {
                        previous.push((index, target));
                    }
                }
                false
            });
            !self.defer_visual || visible
        });
        if self.defer_visual && retired.len().saturating_mul(4) < retired.capacity() {
            // A batch can replace many unpublished candidates but expose only
            // a few retirements; do not keep the large buffer in the mailbox.
            retired.shrink_to_fit();
        }
        // Cache order is publication order, independent of spatial bucket order.
        // Move retired evidence instead of making the mailbox and Hint recover
        // names from whichever partials they happened to have consumed.
        previous.sort_unstable_by_key(|(index, _)| *index);
        descriptions.extend(previous.into_iter().map(|(_, target)| target));
        // Metadata is a same-geometry delta, so OCR arriving after AX enriches
        // the original semantic target rather than creating an extra label.
        if has_text && !self.search_targets.is_empty() {
            let mut changed = SmallVec::<[usize; 16]>::new();
            // Grid cells include nearby rows that cannot contain any new text.
            // One conservative batch bound avoids rebuilding those neighbors,
            // without comparing every cached target against every new fragment.
            let mut text_bounds: Option<Rect> = None;
            for text in &self.visual_text[text_start..] {
                let r = text.rect;
                if ![r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite()) {
                    text_bounds = None;
                    break;
                }
                text_bounds = Some(text_bounds.map_or(r, |bounds| bounds.union(&r)));
            }
            // Overflowing native coordinates keep the original query behavior.
            let text_bounds = text_bounds.filter(|r| r.width.is_finite() && r.height.is_finite());
            // All new text is already indexed. Rebuild each affected target
            // once for this batch, including targets whose metadata is unchanged.
            self.search_spatial.for_each_candidate(
                self.visual_text[text_start..].iter().map(|text| text.rect),
                |index, _| {
                    if let Some(target) = &mut self.search_targets[index]
                        && text_bounds.is_none_or(|bounds| target.rect.intersect(&bounds).is_some())
                        && attach_ocr(
                            target,
                            &self.visual_text,
                            &mut self.ocr_scratch,
                            &mut self.text_spatial,
                        )
                    {
                        changed.push(index);
                    }
                },
            );
            changed.sort_unstable();
            for index in changed {
                if !self.deferred.get(index).copied().unwrap_or(false)
                    && let Some(target) = &self.search_targets[index]
                {
                    retired.push(target.rect);
                    targets.push(target.clone());
                }
            }
        }
        // Late-text replacements were cloned from the cache just above. Only
        // provider targets and image replacements still need indexing; querying
        // and comparing those appended clones cannot update the cache.
        let cache_initialized = source == TargetSource::Accessibility
            && self.search_targets.is_empty()
            && self.visual_text.is_empty()
            && retired.is_empty()
            && self.initialize_native_cache(&targets[..accepted], threshold);
        if !cache_initialized {
            for target in &mut targets[..accepted] {
                let mut existing = None;
                self.search_spatial
                    .any_match(target.rect, |index, rect, _| {
                        if rect == target.rect
                            && self.search_targets[index]
                                .as_ref()
                                .is_some_and(|old| old.role == target.role)
                        {
                            existing = Some(index);
                            true
                        } else {
                            false
                        }
                    });
                if existing.is_some_and(|index| self.search_targets[index].as_ref() == Some(target))
                {
                    continue;
                }
                // Accepted contour images were already enriched by image_name.
                // Their geometry and this batch's OCR evidence are unchanged,
                // so rebuilding that description here would repeat the query
                // and text assembly. Other targets still need enrichment.
                if source != TargetSource::Contour || target.role != SemanticRole::Image {
                    attach_ocr(
                        target,
                        &self.visual_text,
                        &mut self.ocr_scratch,
                        &mut self.text_spatial,
                    );
                }
                inherit_replaced_names(target, &descriptions);
                if let Some(index) = existing {
                    self.search_targets[index] = Some(target.clone());
                    if self.defer_visual {
                        self.deferred[index] &= provisional_visual(source, target);
                    }
                } else if self.search_spatial.store(target.rect) {
                    self.search_targets.push(Some(target.clone()));
                    if self.defer_visual {
                        self.deferred.push(provisional_visual(source, target));
                    }
                }
            }
        }
        if source == TargetSource::Contour {
            self.images.extend(
                targets[..accepted]
                    .iter()
                    .filter(|t| t.role == SemanticRole::Image)
                    .cloned(),
            );
        }
        if self.defer_visual {
            let mut index = 0;
            targets.retain(|target| {
                let publish = index >= accepted || !provisional_visual(source, target);
                index += 1;
                publish
            });
            if source == TargetSource::Contour {
                targets.extend(self.take_ready());
            }
            #[cfg(any(target_os = "macos", test))]
            if source == TargetSource::NativeRectangles {
                targets.extend(self.take_ready());
            }
        }
        ScanUpdate {
            accepted,
            targets,
            retired,
        }
    }

    // Keep the one-time grid copy out of the repeated enrichment pipeline.
    #[inline(never)]
    fn initialize_native_cache(&mut self, targets: &[UiTarget], threshold: f64) -> bool {
        let Some(spatial) = self.index.native_cache_snapshot(targets, threshold) else {
            return false;
        };
        self.search_spatial = spatial;
        self.search_targets
            .extend(targets.iter().cloned().map(Some));
        if self.defer_visual {
            self.deferred.resize(self.search_targets.len(), false);
        }
        true
    }

    /// Specialize once per ready batch. Scans without layout containers use
    /// the original geometric matcher without caption lookup in its hot loop.
    fn insert_targets<const CHECK_LAYOUT: bool>(
        &mut self,
        targets: &mut Vec<UiTarget>,
        source: TargetSource,
        threshold: f64,
    ) {
        let cached = &self.search_targets;
        let cache_spatial = &mut self.search_spatial;
        // Prior batches are already in the search cache. Only a native batch
        // containing text can need an unpublished owner's caption temporarily.
        let cache_native_captions = CHECK_LAYOUT
            && source == TargetSource::Accessibility
            && targets
                .iter()
                .any(|target| target.role == SemanticRole::StaticText);
        targets.retain_mut(|target| {
            self.index.stitch_text(target, source);
            self.index.insert_with_caption_lookup::<CHECK_LAYOUT>(
                target,
                source,
                threshold,
                cache_native_captions,
                |rect, role, query| {
                    let mut name = None;
                    cache_spatial.any_match(query, |index, old, _| {
                        if old == rect
                            && let Some(target) = &cached[index]
                            && target.role == role
                        {
                            name = Some(target.name.as_str());
                            true
                        } else {
                            false
                        }
                    });
                    name
                },
            )
        });
        self.index.clear_batch_captions();
    }

    /// Called once at a valid terminal, before releasing scan-only evidence.
    /// Move surviving candidates with their latest metadata; no replay or
    /// changed fusion order, extra timer, or retained screenshot is needed.
    pub(crate) fn take_deferred(&mut self) -> Vec<UiTarget> {
        self.search_targets
            .iter_mut()
            .zip(&mut self.deferred)
            .filter_map(|(target, deferred)| {
                std::mem::take(deferred).then(|| target.take()).flatten()
            })
            .collect()
    }
}

fn provisional_visual(source: TargetSource, target: &UiTarget) -> bool {
    match source {
        TargetSource::Contour => target.role != SemanticRole::Image,
        #[cfg(any(target_os = "macos", test))]
        TargetSource::NativeRectangles => true,
        _ => false,
    }
}

fn needs_replaced_names(target: &UiTarget) -> bool {
    matches!(
        target.role,
        SemanticRole::ListItem | SemanticRole::Image | SemanticRole::StaticText
    ) && !(target.role == SemanticRole::StaticText
        && target.details.as_ref().is_some_and(|details| {
            !details.ocr.is_empty()
                && details.accessibility.is_empty()
                && details.ocr == target.name
        }))
}

/// Complete searchable names before publishing a replacement. Transport and
/// display consumers apply the delta without reconstructing source evidence.
fn inherit_replaced_names(target: &mut UiTarget, descriptions: &[UiTarget]) {
    if descriptions.is_empty() || !needs_replaced_names(target) {
        return;
    }
    let inherit_descriptions = target.name.is_empty() || target.role == SemanticRole::StaticText;
    for old in descriptions {
        if (!inherit_descriptions && old.rect != target.rect)
            || !target.rect.contains(&old.rect.center())
            || target.name.contains(&old.name)
        {
            continue;
        }
        if !target.name.is_empty() {
            target.name.push(' ');
        }
        target.name.push_str(&old.name);
    }
}

#[derive(Default)]
struct OcrScratch {
    text: String,
    // Store indices rather than references so overflow capacity can be reused.
    runs: SmallVec<[usize; 16]>,
}

// Rebuild from bounded, original scan evidence, never from previously joined text.
// Scratch capacity is retained for this scan and released by finish/drop.
fn attach_ocr(
    target: &mut UiTarget,
    text: &[UiTarget],
    scratch: &mut OcrScratch,
    spatial: &mut super::spatial_index::SpatialIndex,
) -> bool {
    // Contours and accessibility can publish the first scene before OCR is
    // ready. There is no description to attach yet, so skip empty grid queries.
    if text.is_empty() {
        return false;
    }
    let OcrScratch {
        text: scratch,
        runs,
    } = scratch;
    runs.clear();
    // Evidence is append-only for this scan. Query overlapping cells rather
    // than repeatedly walking all previous OCR lines for each small quantum.
    spatial.any_match(target.rect, |index, _, _| {
        let item = &text[index];
        if !item.name.is_empty()
            && target
                .rect
                .intersect(&item.rect)
                .is_some_and(|r| r.width * r.height >= item.rect.width * item.rect.height * 0.9)
        {
            runs.push(index);
        }
        false
    });
    if runs.is_empty() {
        return false;
    }
    if runs.len() == 1 {
        let value = text[runs[0]].name.trim();
        if target.details.as_ref().is_some_and(|details| {
            details.ocr == value
                && (target.role != SemanticRole::StaticText
                    || !details.accessibility.is_empty()
                    || target.name == value)
        }) {
            return false;
        }
    }
    runs.sort_unstable_by(|&a, &b| {
        let (a, b) = (&text[a], &text[b]);
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then(a.rect.x.total_cmp(&b.rect.x))
            .then(a.name.cmp(&b.name))
    });
    // Anchor each row before ordering by x; a fuzzy sort comparator would not
    // be transitive when three rows have different heights.
    let mut start = 0;
    while start < runs.len() {
        let anchor = text[runs[start]].rect;
        let mut end = start + 1;
        while end < runs.len() && same_text_row(anchor, text[runs[end]].rect) {
            end += 1;
        }
        runs[start..end].sort_unstable_by(|&a, &b| {
            let (a, b) = (&text[a], &text[b]);
            a.rect
                .x
                .total_cmp(&b.rect.x)
                .then(a.rect.right().total_cmp(&b.rect.right()))
                .then(a.name.cmp(&b.name))
        });
        start = end;
    }
    scratch.clear();
    let mut previous: Option<&UiTarget> = None;
    for &index in runs.iter() {
        let run = &text[index];
        let value = run.name.trim();
        let overlap = previous.is_some_and(|old| {
            same_text_row(old.rect, run.rect)
                && old
                    .rect
                    .intersect(&run.rect)
                    .is_some_and(|r| r.width >= old.rect.height.min(run.rect.height) * 0.5)
        });
        if overlap
            && (scratch.ends_with(value)
                || previous.is_some_and(|old| {
                    old.rect.x <= run.rect.x
                        && old.rect.right() >= run.rect.right()
                        && old.name.contains(value)
                }))
        {
            continue;
        }
        let skip = if overlap {
            text_overlap(scratch, value)
        } else {
            0
        };
        if !scratch.is_empty() && skip == 0 {
            scratch.push(' ');
        }
        scratch.push_str(&value[skip..]);
        previous = Some(run);
    }
    let details = target.details.get_or_insert_with(|| {
        Box::new(crate::api::geometry::UiTargetDetails {
            accessibility: target.name.clone(),
            ..Default::default()
        })
    });
    let changed = details.ocr != *scratch;
    if details.ocr != *scratch {
        std::mem::swap(&mut details.ocr, scratch);
    }
    if target.role == SemanticRole::StaticText && details.accessibility.is_empty() {
        target.name.clone_from(&details.ocr);
    }
    changed
}

fn same_text_row(a: Rect, b: Rect) -> bool {
    let small = a.height.min(b.height);
    small > 0.0
        && a.height.max(b.height) <= small * 1.6
        && (a.center().y - b.center().y).abs() <= small * 0.3
}

// Only inspect a bounded seam, on UTF-8 boundaries. Never discard a single
// coincident character; uncertain recognition is preserved rather than guessed.
fn text_overlap(left: &str, right: &str) -> usize {
    const LIMIT: usize = 512;
    let n = left.len().min(right.len()).min(LIMIT);
    if n == 0 {
        return 0;
    }
    // KMP: linear time even for repeated characters. Fixed stack storage,
    // independent of the length of a page or a long OCR paragraph.
    let pattern = &right.as_bytes()[..n];
    let mut failure = [0u16; LIMIT];
    let mut matched = 0;
    for i in 1..n {
        while matched > 0 && pattern[i] != pattern[matched] {
            matched = usize::from(failure[matched - 1]);
        }
        if pattern[i] == pattern[matched] {
            matched += 1;
        }
        failure[i] = matched as u16;
    }
    matched = 0;
    for &byte in &left.as_bytes()[left.len() - n..] {
        while matched > 0 && (matched == n || pattern[matched] != byte) {
            matched = usize::from(failure[matched - 1]);
        }
        if pattern[matched] == byte {
            matched += 1;
        }
    }
    if right.is_char_boundary(matched) && right[..matched].chars().take(2).count() == 2 {
        matched
    } else {
        0
    }
}

fn image_name(
    image: &mut UiTarget,
    text: &[UiTarget],
    scratch: &mut OcrScratch,
    spatial: &mut super::spatial_index::SpatialIndex,
) -> bool {
    attach_ocr(image, text, scratch, spatial);
    if let Some(details) = &image.details
        && image.name != details.ocr
    {
        image.name.clone_from(&details.ocr);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{command::remove_retired_targets, geometry::SemanticRole};

    fn evidence_index(text: &[UiTarget]) -> super::super::spatial_index::SpatialIndex {
        let mut index = super::super::spatial_index::SpatialIndex::new(64.0, 0.0, 0.0);
        for item in text {
            assert!(index.store(item.rect));
        }
        index
    }

    #[test]
    fn published_refinements_inherit_only_retired_names_and_keep_native_names() {
        for separate in [false, true] {
            for width in [40.0, 60.0] {
                for name in ["", "native name"] {
                    let mut scan = ScanAccumulator::new();
                    let text = UiTarget {
                        rect: Rect::new(0.0, 0.0, width, 20.0),
                        role: SemanticRole::StaticText,
                        name: "member".into(),
                        details: None,
                    };
                    let row = UiTarget {
                        rect: Rect::new(0.0, 0.0, 100.0, 20.0),
                        role: SemanticRole::ListItem,
                        name: name.into(),
                        details: None,
                    };
                    let mut input = vec![text.clone()];
                    if separate {
                        scan.push(TargetSource::Accessibility, std::mem::take(&mut input), 0.5);
                    }
                    input.push(row.clone());
                    let update = scan.push(TargetSource::Accessibility, input, 0.5);
                    let published = update.targets.iter().find(|t| t.role == row.role).unwrap();
                    let retired = width >= 50.0;
                    assert_eq!(update.retired.contains(&text.rect), retired);
                    assert_eq!(
                        published.name,
                        if retired && name.is_empty() {
                            "member"
                        } else {
                            name
                        },
                        "separate={separate}, width={width}, name={name}"
                    );
                    assert!(
                        scan.push(TargetSource::Accessibility, vec![row], 0.5)
                            .targets
                            .is_empty()
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "allocation counter requires isolated execution"]
    fn suppressed_contours_do_not_allocate_metadata_or_assemble_image_names() {
        let mut scan = ScanAccumulator::new();
        let candidates: Vec<_> = (0..64)
            .map(|i| UiTarget {
                rect: Rect::new(f64::from(i) * 200.0, 0.0, 120.0, 80.0),
                role: if i % 2 == 0 {
                    SemanticRole::Image
                } else {
                    SemanticRole::Control
                },
                name: String::new(),
                details: None,
            })
            .collect();
        // Images reject duplicate images; native actions reject controls.
        // Keep OCR evidence present so rejected images would otherwise query
        // and assemble descriptions as well as allocate empty metadata.
        scan.push(
            TargetSource::Contour,
            candidates.iter().step_by(2).cloned().collect(),
            0.5,
        );
        let native = candidates
            .iter()
            .skip(1)
            .step_by(2)
            .map(|target| UiTarget {
                role: SemanticRole::Button,
                ..target.clone()
            })
            .collect();
        scan.push(TargetSource::Accessibility, native, 0.5);
        let text = candidates
            .iter()
            .map(|target| {
                UiTarget::recognized_text(
                    target.rect.inset(10.0, 30.0),
                    "caption inside a rejected contour image".into(),
                )
            })
            .collect();
        scan.push(TargetSource::SystemOcr, text, 0.5);
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        let update = scan.push(TargetSource::Contour, candidates, 0.5);
        let stats = region.change();
        assert!(update.targets.is_empty());
        assert!(update.retired.is_empty());
        assert_eq!(stats.allocations + stats.reallocations, 0, "{stats:?}");
    }

    #[test]
    #[ignore = "allocation counter requires isolated execution"]
    fn warmed_seam_assembly_reuses_strings_and_fragment_storage() {
        let text: Vec<_> = (0..32)
            .map(|i| {
                UiTarget::recognized_text(
                    Rect::new(f64::from(i) * 140.0, 0.0, 120.0, 20.0),
                    format!("fragment {i}"),
                )
            })
            .collect();
        let mut target =
            UiTarget::recognized_text(Rect::new(0.0, 0.0, 5000.0, 30.0), String::new());
        let mut scratch = OcrScratch::default();
        let mut spatial = evidence_index(&text);
        attach_ocr(&mut target, &text, &mut scratch, &mut spatial);
        attach_ocr(&mut target, &text, &mut scratch, &mut spatial);
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..1000 {
            assert!(!attach_ocr(&mut target, &text, &mut scratch, &mut spatial));
        }
        let stats = region.change();
        assert_eq!(stats.allocations + stats.reallocations, 0, "{stats:?}");
    }

    #[test]
    fn seam_text_converges_across_batches_and_arrival_orders() {
        for reverse in [false, true] {
            for separate in [false, true] {
                let mut scan = ScanAccumulator::new();
                let mut input = vec![
                    UiTarget::recognized_text(
                        Rect::new(0.0, 0.0, 140.0, 20.0),
                        "支持 搜 索 和 复 制".into(),
                    ),
                    UiTarget::recognized_text(
                        Rect::new(40.0, 0.0, 140.0, 20.0),
                        "搜索和复制结果".into(),
                    ),
                ];
                if reverse {
                    input.reverse();
                }
                let mut visible = Vec::new();
                let batches = if separate {
                    input.into_iter().map(|t| vec![t]).collect()
                } else {
                    vec![input]
                };
                for batch in batches {
                    let update = scan.push(TargetSource::SystemOcr, batch, 0.5);
                    remove_retired_targets(&mut visible, &update.retired);
                    visible.extend(update.targets);
                }
                assert_eq!(visible.len(), 1);
                assert_eq!(visible[0].name, "支持搜索和复制结果");
                assert_eq!(visible[0].ocr_text(), "支持搜索和复制结果");
            }
        }
    }

    #[test]
    fn seam_text_requires_geometry_and_preserves_uncertain_content() {
        let mut scratch = OcrScratch::default();
        for (x, y, right, expected) in [
            (80.0, 0.0, "copy results", "please copy results"),
            (140.0, 0.0, "copy results", "please copy copy results"),
            (0.0, 30.0, "copy results", "please copy copy results"),
            (80.0, 0.0, "different OCR", "please copy different OCR"),
        ] {
            let text = vec![
                UiTarget::recognized_text(Rect::new(0.0, 0.0, 120.0, 20.0), "please copy".into()),
                UiTarget::recognized_text(Rect::new(x, y, 120.0, 20.0), right.into()),
            ];
            let mut owner =
                UiTarget::recognized_text(Rect::new(0.0, 0.0, 300.0, 60.0), String::new());
            let mut spatial = evidence_index(&text);
            attach_ocr(&mut owner, &text, &mut scratch, &mut spatial);
            assert_eq!(owner.ocr_text(), expected);
        }
        assert_eq!(text_overlap("重复的字符字", "字结束"), 0);
        assert_eq!(text_overlap("🦀支持搜索", "支持搜索结果"), "支持搜索".len());
        assert_eq!(text_overlap(&"a".repeat(10_000), &"a".repeat(10_000)), 512);
    }

    #[test]
    fn indexed_evidence_matches_full_geometry_filter_for_small_and_oversized_owners() {
        let text: Vec<_> = (0..1024)
            .map(|i| {
                UiTarget::recognized_text(
                    Rect::new(
                        (i % 32 * 40) as f64 - 200.,
                        (i / 32 * 30) as f64 - 200.,
                        30.,
                        20.,
                    ),
                    i.to_string(),
                )
            })
            .collect();
        let mut spatial = evidence_index(&text);
        for rect in [
            Rect::new(-200., -200., 5000., 5000.),
            Rect::new(-200., -200., 30., 20.),
            Rect::new(10., 10., 251., 105.),
            Rect::new(4000., 4000., 30., 20.),
        ] {
            let mut target = UiTarget::recognized_text(rect, String::new());
            let mut scratch = OcrScratch::default();
            attach_ocr(&mut target, &text, &mut scratch, &mut spatial);
            let mut actual = scratch.runs.to_vec();
            actual.sort_unstable();
            let expected: Vec<_> = text
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    rect.intersect(&item.rect)
                        .is_some_and(|r| {
                            r.width * r.height >= item.rect.width * item.rect.height * 0.9
                        })
                        .then_some(i)
                })
                .collect();
            assert_eq!(actual, expected, "{rect:?}");
        }
    }

    #[test]
    fn search_details_keep_ocr_and_accessibility_in_both_arrival_orders() {
        for ocr_first in [false, true] {
            let mut scan = ScanAccumulator::new();
            let control = UiTarget {
                rect: Rect::new(0.0, 0.0, 100.0, 40.0),
                name: "Copy button".into(),
                role: SemanticRole::Button,
                details: None,
            };
            let ocr = UiTarget::recognized_text(Rect::new(20.0, 10.0, 40.0, 20.0), "复制".into());
            let mut sources = [
                (TargetSource::Accessibility, control),
                (TargetSource::SystemOcr, ocr.clone()),
            ];
            if ocr_first {
                sources.reverse();
            }
            let mut visible = Vec::new();
            for (source, target) in sources {
                let update = scan.push(source, vec![target], 0.5);
                remove_retired_targets(&mut visible, &update.retired);
                visible.extend(update.targets);
            }
            let repeated = scan.push(TargetSource::SystemOcr, vec![ocr], 0.5);
            assert!(repeated.targets.is_empty());
            assert!(repeated.retired.is_empty());
            let control = visible
                .iter()
                .find(|t| t.role == SemanticRole::Button)
                .unwrap();
            assert_eq!(control.ocr_text(), "复制");
            assert_eq!(control.accessibility_text(), "Copy button");
            assert_eq!(
                visible
                    .iter()
                    .filter(|t| t.role == SemanticRole::Button)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn independent_native_geometry_and_ocr_converge_in_both_arrival_orders() {
        for ocr_first in [false, true] {
            let mut scan = ScanAccumulator::new();
            let rect = Rect::new(0.0, 0.0, 100.0, 30.0);
            let geometry = UiTarget {
                rect,
                name: String::new(),
                role: SemanticRole::Button,
                details: None,
            };
            let text = UiTarget::recognized_text(rect, "复制 Copy".into());
            let mut sources = [
                (TargetSource::NativeRectangles, geometry),
                (TargetSource::SystemOcr, text),
            ];
            if ocr_first {
                sources.reverse();
            }
            let mut visible = Vec::new();
            for (index, (source, target)) in sources.into_iter().enumerate() {
                let update = scan.push(source, vec![target], 0.5);
                remove_retired_targets(&mut visible, &update.retired);
                visible.extend(update.targets);
                assert_eq!(
                    visible.len(),
                    1,
                    "first ready source must publish immediately"
                );
                if index == 0 && !ocr_first {
                    assert_eq!(visible[0].role, SemanticRole::Button);
                    assert_eq!(visible[0].ocr_text(), "");
                }
            }
            assert_eq!(visible[0].role, SemanticRole::StaticText);
            assert_eq!(visible[0].ocr_text(), "复制 Copy");
        }
    }

    #[test]
    fn image_ocr_and_native_action_converge_in_every_source_order() {
        let image = UiTarget {
            details: None,
            rect: Rect::new(0., 0., 240., 160.),
            name: String::new(),
            role: SemanticRole::Image,
        };
        let text = UiTarget {
            details: None,
            rect: Rect::new(30., 30., 80., 20.),
            name: "caption".into(),
            role: SemanticRole::StaticText,
        };
        let button = UiTarget {
            details: None,
            rect: Rect::new(150., 90., 30., 25.),
            name: "action".into(),
            role: SemanticRole::Button,
        };
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let groups = [
                (TargetSource::Contour, image.clone()),
                (TargetSource::SystemOcr, text.clone()),
                (TargetSource::Accessibility, button.clone()),
            ];
            let mut scan = ScanAccumulator::new();
            let mut visible = Vec::new();
            for i in order {
                let update = scan.push(groups[i].0, vec![groups[i].1.clone()], 0.5);
                remove_retired_targets(&mut visible, &update.retired);
                visible.extend(update.targets);
            }
            let repeated = scan.push(TargetSource::SystemOcr, vec![text.clone()], 0.5);
            assert!(repeated.targets.is_empty(), "{order:?}");
            assert!(repeated.retired.is_empty(), "{order:?}");
            assert_eq!(visible.len(), 2, "{order:?}");
            assert!(
                visible
                    .iter()
                    .any(|t| t.rect == image.rect && t.name == "caption"),
                "{order:?}"
            );
            assert!(visible.iter().any(|t| t.rect == button.rect));
        }
    }

    #[test]
    fn large_replacement_is_one_publication() {
        let mut scan = ScanAccumulator::new();
        let boxes: Vec<_> = (0..100)
            .map(|i| UiTarget {
                details: None,
                rect: Rect::new(f64::from(i) * 100., 0., 40., 20.),
                name: String::new(),
                role: SemanticRole::Control,
            })
            .collect();
        scan.push(TargetSource::Contour, boxes.clone(), 0.5);
        let stronger = boxes
            .into_iter()
            .map(|t| UiTarget {
                role: SemanticRole::Button,
                ..t
            })
            .collect();
        let update = scan.push(TargetSource::Accessibility, stronger, 0.5);
        assert_eq!(update.retired.len(), 100);
        assert_eq!(update.targets.len(), 100);
    }

    fn target(x: f64, width: f64, role: SemanticRole) -> UiTarget {
        UiTarget {
            details: None,
            rect: Rect::new(x, 0.0, width, 20.0),
            name: String::new(),
            role,
        }
    }

    #[test]
    fn overlapping_ocr_tile_runs_join_without_joining_adjacent_controls_or_rows() {
        for reverse in [false, true] {
            let mut scan = ScanAccumulator::new();
            let mut left = target(0.0, 100.0, SemanticRole::StaticText);
            left.name = "left phrase".into();
            let mut right = target(80.0, 100.0, SemanticRole::StaticText);
            right.name = "right phrase".into();
            let input = if reverse {
                vec![right, left]
            } else {
                vec![left, right]
            };
            let update = scan.push(TargetSource::SystemOcr, input, 0.5);
            assert_eq!(scan.index.len(), 1);
            let joined = &update.targets[0];
            assert_eq!(joined.rect, Rect::new(0.0, 0.0, 180.0, 20.0));
            assert!(joined.name.contains("left phrase"));
            assert!(joined.name.contains("right phrase"));
            let disjoint = target(190.0, 100.0, SemanticRole::StaticText);
            let mut next_row = target(0.0, 100.0, SemanticRole::StaticText);
            next_row.rect.y = 24.0;
            scan.push(TargetSource::SystemOcr, vec![disjoint, next_row], 0.5);
            assert_eq!(scan.index.len(), 3);
            let button = target(50.0, 20.0, SemanticRole::Button);
            scan.push(TargetSource::Accessibility, vec![button], 0.5);
            assert_eq!(scan.index.len(), 4);
        }
    }

    #[test]
    fn every_ready_batch_is_immediate_and_replacement_retires_fragments() {
        let mut scan = ScanAccumulator::new();
        let a = target(0.0, 20.0, SemanticRole::Control);
        let b = target(50.0, 20.0, SemanticRole::Control);
        let first = scan.push(TargetSource::Contour, vec![a.clone()], 0.5);
        assert_eq!(first.targets[0].rect, a.rect);
        assert_eq!(first.targets[0].accessibility_text(), "");
        let second = scan.push(TargetSource::Contour, vec![b.clone()], 0.5);
        assert_eq!(second.targets.len(), 1);
        assert_eq!(second.targets[0].rect, b.rect);
        let row = target(0.0, 100.0, SemanticRole::ListItem);
        let update = scan.push(TargetSource::Accessibility, vec![row.clone()], 0.5);
        assert_eq!(update.accepted, 1);
        assert_eq!(update.targets, [row]);
        assert_eq!(update.retired.len(), 2);
        assert!(update.retired.contains(&a.rect));
        assert!(update.retired.contains(&b.rect));
    }

    #[test]
    fn description_before_owner_in_one_batch_is_not_published_again() {
        let mut scan = ScanAccumulator::new();
        let text = target(10.0, 20.0, SemanticRole::StaticText);
        let row = target(0.0, 100.0, SemanticRole::ListItem);
        let update = scan.push(TargetSource::Accessibility, vec![text, row.clone()], 0.5);
        assert_eq!(update.accepted, 1);
        assert_eq!(update.targets, [row]);
    }

    #[test]
    fn identical_geometry_replacement_keeps_new_target() {
        let mut scan = ScanAccumulator::new();
        let visual = target(0.0, 20.0, SemanticRole::Control);
        scan.push(TargetSource::Contour, vec![visual.clone()], 0.5);
        let button = target(0.0, 20.0, SemanticRole::Button);
        let update = scan.push(TargetSource::Accessibility, vec![button.clone()], 0.5);
        assert_eq!(update.retired, vec![visual.rect]);
        assert_eq!(update.targets, [button]);
    }
    #[test]
    fn first_native_cache_keeps_shared_geometry_and_oversized_owners() {
        let mut scan = ScanAccumulator::new();
        let rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        let mut native: Vec<_> = [
            (rect, SemanticRole::Button, "button"),
            (rect, SemanticRole::Checkbox, "checkbox"),
            (
                Rect::new(-2000.0, -2000.0, 5000.0, 5000.0),
                SemanticRole::Button,
                "large action",
            ),
        ]
        .into_iter()
        .map(|(rect, role, name)| UiTarget {
            rect,
            role,
            name: name.into(),
            details: None,
        })
        .collect();
        let initial = scan.push(TargetSource::Accessibility, native.clone(), 0.5);
        assert_eq!(initial.targets, native);
        assert!(initial.retired.is_empty());
        let later = UiTarget {
            rect,
            role: SemanticRole::Link,
            name: "later link".into(),
            details: None,
        };
        let next = scan.push(TargetSource::Accessibility, vec![later.clone()], 0.5);
        assert_eq!(next.targets.as_slice(), std::slice::from_ref(&later));
        native.push(later);
        let update = scan.push(
            TargetSource::SystemOcr,
            vec![UiTarget::recognized_text(
                Rect::new(20.0, 10.0, 60.0, 20.0),
                "caption".into(),
            )],
            0.5,
        );
        assert_eq!(update.targets.len(), native.len());
        for (updated, original) in update.targets.iter().zip(&native) {
            assert_eq!(updated.rect, original.rect);
            assert_eq!(updated.role, original.role);
            assert_eq!(updated.name, original.name);
            assert_eq!(updated.ocr_text(), "caption");
        }
    }

    #[test]
    fn first_native_cache_handles_non_reflexive_geometry_matching() {
        for (rect, threshold) in [
            (Rect::new(1e30, 1e30, 10.0, 10.0), 0.5),
            (Rect::new(0.0, 0.0, 1e200, 1e200), 0.5),
            (Rect::new(0.0, 0.0, 100.0, 40.0), f64::NAN),
        ] {
            let mut scan = ScanAccumulator::new();
            let targets = ["first", "second"]
                .map(|name| UiTarget {
                    rect,
                    role: SemanticRole::Button,
                    name: name.into(),
                    details: None,
                })
                .to_vec();
            let update = scan.push(TargetSource::Accessibility, targets, threshold);
            assert_eq!(update.targets.len(), 2);
            assert!(update.retired.is_empty());
            assert_eq!(scan.search_targets.len(), 1);
            assert_eq!(scan.search_spatial.len(), 1);
            assert_eq!(scan.search_targets[0].as_ref().unwrap().name, "second");
            let next = scan.push(
                TargetSource::Accessibility,
                vec![UiTarget {
                    rect,
                    role: SemanticRole::Button,
                    name: "third".into(),
                    details: None,
                }],
                threshold,
            );
            assert_eq!(next.targets.len(), 1);
            assert_eq!(scan.search_targets.len(), 1);
            assert_eq!(scan.search_targets[0].as_ref().unwrap().name, "third");
        }
    }

    #[test]
    fn native_and_contour_images_reuse_the_same_search_identity() {
        let mut scan = ScanAccumulator::new();
        let image = UiTarget {
            rect: Rect::new(0.0, 0.0, 100.0, 80.0),
            role: SemanticRole::Image,
            name: "visual image".into(),
            details: None,
        };
        scan.push(TargetSource::Contour, vec![image.clone()], 0.5);
        let native = UiTarget {
            name: "native image".into(),
            ..image
        };
        let update = scan.push(TargetSource::Accessibility, vec![native.clone()], 0.5);
        assert_eq!(update.targets.as_slice(), std::slice::from_ref(&native));
        assert!(update.retired.is_empty());
        assert_eq!(scan.search_targets, [Some(native)]);
        assert_eq!(scan.search_spatial.len(), 1);
    }

    #[test]
    fn first_native_cache_includes_previously_unpublished_ocr() {
        let mut scan = ScanAccumulator::new();
        let text = scan.push(
            TargetSource::SystemOcr,
            vec![UiTarget::recognized_text(
                Rect::new(10.0, 10.0, 40.0, 1.0),
                "thin caption".into(),
            )],
            0.5,
        );
        assert!(text.targets.is_empty());
        let native = target(0.0, 100.0, SemanticRole::Button);
        let update = scan.push(TargetSource::Accessibility, vec![native], 0.5);
        assert_eq!(update.targets.len(), 1);
        assert_eq!(update.targets[0].ocr_text(), "thin caption");
    }
    #[test]
    fn settled_regions_preserve_metadata_and_wait_for_all_overlapping_sources() {
        use super::super::scan_progress::Progress;
        let left = Rect::new(0., 0., 500., 200.);
        let right = Rect::new(1000., 0., 500., 200.);
        let panel = UiTarget {
            rect: Rect::new(50., 50., 300., 100.),
            role: SemanticRole::Control,
            name: String::new(),
            details: None,
        };
        let sources = [
            TargetSource::Contour,
            TargetSource::SystemOcr,
            TargetSource::Accessibility,
            TargetSource::NativeRectangles,
        ];
        let mut scan = ScanAccumulator::new()
            .defer_visual(true)
            .progress_sources(&sources);
        for source in sources {
            assert!(
                scan.progress(source, Progress::Plan(vec![left, right]))
                    .is_empty()
            );
        }
        assert!(
            scan.push(TargetSource::Contour, vec![panel.clone()], 0.5)
                .targets
                .is_empty()
        );
        scan.push(
            TargetSource::SystemOcr,
            vec![UiTarget::recognized_text(
                Rect::new(80., 80., 80., 20.),
                "inside".into(),
            )],
            0.5,
        );
        for source in [
            TargetSource::SystemOcr,
            TargetSource::Contour,
            TargetSource::Accessibility,
        ] {
            assert!(scan.progress(source, Progress::Complete(left)).is_empty());
        }
        let ready = scan.progress(TargetSource::NativeRectangles, Progress::Complete(left));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].rect, panel.rect);
        assert_eq!(ready[0].ocr_text(), "inside");
        for source in sources {
            assert!(scan.progress(source, Progress::Complete(right)).is_empty());
        }
        assert!(
            scan.take_deferred().is_empty(),
            "early targets must not be duplicated at terminal"
        );
    }

    #[test]
    fn replacement_retires_only_previously_published_visual_targets() {
        use super::super::scan_progress::Progress;
        let visible = target(0.0, 20.0, SemanticRole::Control);
        let hidden = target(200.0, 20.0, SemanticRole::Control);
        let mut scan = ScanAccumulator::new()
            .defer_visual(true)
            .progress_sources(&[TargetSource::Contour]);
        scan.progress(
            TargetSource::Contour,
            Progress::Plan(vec![Rect::new(170.0, -20.0, 100.0, 100.0)]),
        );
        let initial = scan.push(
            TargetSource::Contour,
            vec![visible.clone(), hidden.clone()],
            0.5,
        );
        assert_eq!(initial.targets.len(), 1);
        assert_eq!(initial.targets[0].rect, visible.rect);

        let replacement = |old: UiTarget| UiTarget {
            role: SemanticRole::Button,
            ..old
        };
        let update = scan.push(
            TargetSource::Accessibility,
            vec![replacement(hidden.clone()), replacement(visible.clone())],
            0.5,
        );
        assert_eq!(update.retired, [visible.rect]);
        assert_eq!(
            update.targets.iter().map(|t| t.rect).collect::<Vec<_>>(),
            [hidden.rect, visible.rect]
        );
        assert!(scan.take_deferred().is_empty());
    }

    #[test]
    fn seam_stitch_can_reach_outside_the_last_unfinished_tile() {
        use super::super::scan_progress::Progress;
        let frame = Rect::new(0., 0., 600., 400.);
        let left = Rect::new(0., 0., 120., 100.);
        let right = Rect::new(80., 0., 220., 100.);
        let mut scan = ScanAccumulator::new()
            .defer_visual(true)
            .progress_sources(&[TargetSource::Contour, TargetSource::SystemOcr]);
        scan.progress(TargetSource::Contour, Progress::Plan(vec![frame]));
        scan.progress(TargetSource::SystemOcr, Progress::Plan(vec![left, right]));
        let fragment = UiTarget {
            rect: Rect::new(40., 17., 10., 3.),
            name: String::new(),
            role: SemanticRole::Control,
            details: None,
        };
        let stable = UiTarget {
            rect: Rect::new(400., 300., 20., 20.),
            ..fragment.clone()
        };
        assert!(
            scan.push(
                TargetSource::Contour,
                vec![fragment.clone(), stable.clone()],
                0.5
            )
            .targets
            .is_empty()
        );
        scan.push(
            TargetSource::SystemOcr,
            vec![UiTarget::recognized_text(
                Rect::new(0., 20., 100., 20.),
                "first".into(),
            )],
            0.5,
        );
        scan.progress(TargetSource::SystemOcr, Progress::Complete(left));
        let ready = scan.progress(TargetSource::Contour, Progress::Complete(frame));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].rect, stable.rect);
        let update = scan.push(
            TargetSource::SystemOcr,
            vec![UiTarget::recognized_text(
                Rect::new(90., 17., 150., 25.),
                "second".into(),
            )],
            0.5,
        );
        assert!(
            !update.retired.contains(&fragment.rect),
            "the fragment was never visible"
        );
        assert!(
            scan.progress(TargetSource::SystemOcr, Progress::Complete(right))
                .is_empty()
        );
        assert!(scan.take_deferred().is_empty());
    }

    #[test]
    fn deferred_geometry_preserves_final_targets_and_late_metadata() {
        let visual = |rect| UiTarget {
            rect,
            role: SemanticRole::Control,
            name: String::new(),
            details: None,
        };
        let panel = Rect::new(300., 0., 200., 100.);
        let sources = [
            (
                TargetSource::Contour,
                vec![
                    visual(Rect::new(0., 0., 20., 20.)),
                    visual(Rect::new(30., 0., 20., 20.)),
                    visual(panel),
                    visual(Rect::new(600., 0., 80., 30.)),
                ],
            ),
            (
                TargetSource::SystemOcr,
                vec![
                    UiTarget::recognized_text(Rect::new(0., 0., 80., 20.), "文章的一行".into()),
                    UiTarget::recognized_text(Rect::new(320., 20., 100., 20.), "框内文字".into()),
                    UiTarget::recognized_text(Rect::new(800., 0., 80., 30.), "矩形标题".into()),
                ],
            ),
            (
                TargetSource::Accessibility,
                vec![UiTarget {
                    rect: Rect::new(600., 0., 80., 30.),
                    role: SemanticRole::Button,
                    name: "native action".into(),
                    details: None,
                }],
            ),
            (
                TargetSource::NativeRectangles,
                vec![UiTarget {
                    rect: Rect::new(800., 0., 80., 30.),
                    role: SemanticRole::Button,
                    name: String::new(),
                    details: None,
                }],
            ),
        ];
        for order in [[0, 1, 2, 3], [2, 0, 3, 1], [3, 1, 2, 0]] {
            for chunk in [1, 8] {
                let mut reference = None;
                for defer in [false, true] {
                    let mut scan = ScanAccumulator::new().defer_visual(defer);
                    let mut visible = Vec::new();
                    for lane in order {
                        let (source, targets) = &sources[lane];
                        for batch in targets.chunks(chunk) {
                            let update = scan.push(*source, batch.to_vec(), 0.5);
                            assert!(
                                scan.deferred
                                    .iter()
                                    .zip(&scan.search_targets)
                                    .all(|(pending, target)| !pending || target.is_some()),
                                "retired candidates must not keep progress work pending"
                            );
                            if defer {
                                assert!(
                                    update
                                        .retired
                                        .iter()
                                        .all(|r| visible.iter().any(|t: &UiTarget| t.rect == *r))
                                );
                                assert!(update.targets.iter().all(|t| t.rect != panel));
                            }
                            remove_retired_targets(&mut visible, &update.retired);
                            visible.extend(update.targets);
                        }
                    }
                    visible.extend(scan.take_deferred());
                    assert!(scan.take_deferred().is_empty());
                    assert_eq!(
                        visible.iter().find(|t| t.rect == panel).unwrap().ocr_text(),
                        "框内文字"
                    );
                    visible.sort_by(|a, b| {
                        a.rect
                            .x
                            .total_cmp(&b.rect.x)
                            .then(a.rect.y.total_cmp(&b.rect.y))
                    });
                    if let Some(reference) = &reference {
                        assert_eq!(&visible, reference, "order={order:?}, chunk={chunk}");
                    } else {
                        reference = Some(visible);
                    }
                }
            }
        }
    }
}
