//! Conservative coverage of results that have not passed the fusion queue yet.
#![forbid(unsafe_code)]

use super::spatial_index::{TargetIndex, TargetSource};
use crate::api::Rect;

pub(crate) enum Progress {
    Plan(Vec<Rect>),
    Refine { region: Rect, children: Vec<Rect> },
    Complete(Rect),
}

struct Source {
    source: TargetSource,
    regions: Option<Vec<Rect>>,
}

#[derive(Default)]
pub(super) struct ScanProgress {
    sources: Vec<Source>,
    blocked: Vec<Rect>,
    invalid: bool,
}

impl ScanProgress {
    pub(super) fn new(sources: &[TargetSource]) -> Self {
        Self {
            sources: sources
                .iter()
                .map(|&source| Source {
                    source,
                    regions: None,
                })
                .collect(),
            ..Self::default()
        }
    }

    pub(super) fn update(&mut self, source: TargetSource, progress: Progress) {
        let Some(entry) = self.sources.iter_mut().find(|entry| entry.source == source) else {
            // An unexpected producer cannot make an incomplete plan safe.
            self.invalid = true;
            return;
        };
        let valid = match progress {
            Progress::Plan(regions) if entry.regions.is_none() => {
                let valid = regions.iter().copied().all(usable);
                entry.regions = Some(regions);
                valid
            }
            Progress::Refine { region, children } => {
                if let Some(regions) = &mut entry.regions
                    && children
                        .iter()
                        .copied()
                        .all(|child| usable(child) && contains(region, child))
                    && let Some(index) = regions.iter().position(|&old| old == region)
                {
                    regions.swap_remove(index);
                    regions.extend(children);
                    true
                } else {
                    false
                }
            }
            Progress::Complete(region) => {
                if let Some(regions) = &mut entry.regions
                    && let Some(index) = regions.iter().position(|&old| old == region)
                {
                    regions.swap_remove(index);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        self.invalid |= !valid;
    }

    pub(super) fn prepare(&mut self, index: &TargetIndex) -> bool {
        self.blocked.clear();
        if self.invalid || self.sources.is_empty() {
            return false;
        }
        for source in &self.sources {
            let Some(regions) = &source.regions else {
                return false;
            };
            let start = self.blocked.len();
            self.blocked.extend(regions.iter().copied());
            // Text can grow across tile seams. Close over both active text and
            // touching unfinished tiles within this source's appended range.
            // Earlier sources must remain independent.
            if source.source != TargetSource::Accessibility
                && source.source != TargetSource::Contour
            {
                let len = index.expand_pending_text(source.source, &mut self.blocked[start..]);
                self.blocked.truncate(start + len);
            }
        }
        self.blocked.iter().copied().all(usable)
    }

    pub(super) fn ready(&self, rect: Rect) -> bool {
        usable(rect) && self.blocked.iter().all(|&pending| !touches(rect, pending))
    }
}

fn usable(r: Rect) -> bool {
    [r.x, r.y, r.right(), r.bottom(), r.width, r.height]
        .iter()
        .all(|v| v.is_finite())
        && r.width > 0.0
        && r.height > 0.0
}

fn contains(parent: Rect, child: Rect) -> bool {
    child.x >= parent.x
        && child.y >= parent.y
        && child.right() <= parent.right()
        && child.bottom() <= parent.bottom()
}

pub(super) fn touches(a: Rect, b: Rect) -> bool {
    a.x <= b.right() + 8.0
        && a.right() + 8.0 >= b.x
        && a.y <= b.bottom() + 8.0
        && a.bottom() + 8.0 >= b.y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_sources_and_invalid_refinements_never_release_early() {
        let region = Rect::new(0., 0., 100., 100.);
        let mut progress = ScanProgress::new(&[TargetSource::Contour, TargetSource::SystemOcr]);
        let index = TargetIndex::new();
        progress.update(TargetSource::Contour, Progress::Plan(Vec::new()));
        assert!(!progress.prepare(&index));
        progress.update(TargetSource::SystemOcr, Progress::Plan(vec![region]));
        assert!(progress.prepare(&index));
        assert!(!progress.ready(region));
        progress.update(
            TargetSource::SystemOcr,
            Progress::Refine {
                region,
                children: vec![Rect::new(50., 0., 100., 100.)],
            },
        );
        progress.update(TargetSource::SystemOcr, Progress::Complete(region));
        assert!(!progress.prepare(&index));
    }

    #[test]
    fn overlapping_work_and_tile_halos_remain_pending_independently() {
        let region = Rect::new(-200., 0., 200., 200.);
        let mut progress = ScanProgress::new(&[TargetSource::Accessibility]);
        let index = TargetIndex::new();
        progress.update(
            TargetSource::Accessibility,
            Progress::Plan(vec![region, region]),
        );
        progress.update(TargetSource::Accessibility, Progress::Complete(region));
        assert!(progress.prepare(&index));
        assert!(!progress.ready(Rect::new(-10., 40., 15., 15.)));
        assert!(progress.ready(Rect::new(100., 40., 15., 15.)));
        progress.update(TargetSource::Accessibility, Progress::Complete(region));
        assert!(progress.prepare(&index));
        assert!(progress.ready(region));
    }

    #[test]
    fn future_text_can_bridge_the_union_of_touching_unfinished_tiles() {
        let mut progress = ScanProgress::new(&[TargetSource::SystemOcr]);
        progress.update(
            TargetSource::SystemOcr,
            Progress::Plan(vec![
                Rect::new(0., 0., 100., 100.),
                Rect::new(80., 80., 100., 100.),
            ]),
        );
        assert!(progress.prepare(&TargetIndex::new()));
        assert!(!progress.ready(Rect::new(20., 140., 10., 10.)));
        assert!(progress.ready(Rect::new(300., 140., 10., 10.)));
    }

    #[test]
    fn text_closure_does_not_merge_regions_from_other_sources() {
        let native = Rect::new(0., 0., 40., 40.);
        let left = Rect::new(45., 0., 40., 40.);
        let right = Rect::new(90., 0., 40., 40.);
        let third = Rect::new(175., 0., 40., 40.);
        let disjoint = Rect::new(500., 0., 40., 40.);
        let other_text_source = Rect::new(210., 0., 20., 40.);
        let contour = Rect::new(300., 0., 30., 40.);
        let mut progress = ScanProgress::new(&[
            TargetSource::Accessibility,
            TargetSource::SystemOcr,
            TargetSource::NativeRectangles,
            TargetSource::Contour,
        ]);
        for (source, regions) in [
            (TargetSource::Accessibility, vec![native]),
            (TargetSource::SystemOcr, vec![left, right, third, disjoint]),
            (TargetSource::NativeRectangles, vec![other_text_source]),
            (TargetSource::Contour, vec![contour]),
        ] {
            progress.update(source, Progress::Plan(regions));
        }
        let mut index = TargetIndex::new();
        let text =
            crate::api::UiTarget::recognized_text(Rect::new(130., 0., 40., 40.), "text".into());
        assert!(index.insert(&text, TargetSource::SystemOcr, 0.5));
        assert!(progress.prepare(&index));
        assert_eq!(
            progress.blocked,
            [
                native,
                Rect::new(45., 0., 170., 40.),
                disjoint,
                other_text_source,
                contour
            ]
        );

        progress.update(TargetSource::SystemOcr, Progress::Complete(right));
        assert!(progress.prepare(&index));
        assert_eq!(
            progress.blocked,
            [
                native,
                left,
                disjoint,
                Rect::new(130., 0., 85., 40.),
                other_text_source,
                contour
            ]
        );
        assert!(progress.ready(Rect::new(100., 0., 10., 10.)));
    }
}
