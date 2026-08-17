#![forbid(unsafe_code)]

//! Windows UI-scan result coordination.
//!
//! UI Automation and visual providers run on independent workers. This
//! session is the single publication point so a fast provider can stream
//! immediately without allowing competing terminal results or duplicate hint
//! positions to destabilise labels which are already visible.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smallvec::SmallVec;

use crate::api::command::{UiScanStatus, VisionOptions};
use crate::api::geometry::{Rect, UiTarget};
use crate::platform::partial_batcher::PartialBatcher;
use crate::platform::scan_mailbox::ScanMailbox;

use super::EventSender;

const FIRST_BATCH: usize = 24;
const MAX_TARGETS: usize = 2_000;
const MINIMUM_SPACING: f64 = 8.0;

pub(super) struct ScanSession {
    id: u64,
    generation: u64,
    mailbox: Arc<ScanMailbox>,
    wake: EventSender,
    state: Mutex<SessionState>,
}

struct SessionState {
    remaining: usize,
    batcher: PartialBatcher<UiTarget>,
    index: SpatialIndex,
    statuses: Vec<UiScanStatus>,
    finished: bool,
    published_any: bool,
}

impl ScanSession {
    pub(super) fn new(
        id: u64,
        generation: u64,
        sources: usize,
        vision: &VisionOptions,
        mailbox: Arc<ScanMailbox>,
        wake: EventSender,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            generation,
            mailbox,
            wake,
            state: Mutex::new(SessionState {
                remaining: sources,
                batcher: PartialBatcher::new(FIRST_BATCH, MAX_TARGETS),
                index: SpatialIndex::new(
                    MINIMUM_SPACING,
                    vision.merge_iou_threshold.clamp(0.0, 1.0),
                ),
                statuses: Vec::with_capacity(sources),
                finished: false,
                published_any: false,
            }),
        })
    }

    pub(super) fn source(self: &Arc<Self>, name: &'static str) -> ScanSource {
        ScanSource {
            name,
            session: Arc::clone(self),
            complete: false,
        }
    }

    fn publish(&self, targets: Vec<UiTarget>, status: UiScanStatus) {
        if self
            .mailbox
            .publish(self.generation, self.id, targets, status)
        {
            self.wake.wake();
        }
    }
}

/// A completion token for one independently scheduled scan provider.
pub(super) struct ScanSource {
    name: &'static str,
    session: Arc<ScanSession>,
    complete: bool,
}

impl ScanSource {
    pub(super) fn push(&self, targets: Vec<UiTarget>) -> usize {
        let mut ready = SmallVec::<[Vec<UiTarget>; 2]>::new();
        let accepted = {
            let mut state = self
                .session
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.finished {
                return 0;
            }
            let mut accepted = 0;
            for target in targets {
                if state.index.len() >= MAX_TARGETS {
                    break;
                }
                if state.index.insert(&target) {
                    accepted += 1;
                    if let Some(batch) = state.batcher.push_one(target) {
                        ready.push(batch);
                    }
                }
            }
            if accepted != 0
                && ready.is_empty()
                && !state.published_any
                && let Some(batch) = state.batcher.flush_pending()
            {
                ready.push(batch);
            }
            state.published_any |= !ready.is_empty();
            accepted
        };
        for batch in ready {
            self.session.publish(batch, UiScanStatus::Partial);
        }
        accepted
    }

    pub(super) fn finish(mut self, status: UiScanStatus) {
        self.complete = true;
        let (tail, terminal, masked_failures) = {
            let mut state = self
                .session
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.finished {
                return;
            }
            state.statuses.push(status);
            state.remaining = state.remaining.saturating_sub(1);
            if state.remaining != 0 {
                return;
            }
            state.finished = true;
            let tail = state.batcher.finish();
            let terminal = combined_status(&state.statuses);
            let masked_failures = if terminal == UiScanStatus::Success {
                state
                    .statuses
                    .iter()
                    .filter_map(|status| match status {
                        UiScanStatus::Failed(error) => Some(error.clone()),
                        _ => None,
                    })
                    .collect::<SmallVec<[String; 1]>>()
            } else {
                SmallVec::new()
            };
            (tail, terminal, masked_failures)
        };
        for error in masked_failures {
            crate::report_error!(
                "windows-ui-scan",
                "a provider failed while another provider completed the hybrid scan: {error}"
            );
        }
        if let Some(batch) = tail {
            self.session.publish(batch, UiScanStatus::Partial);
        }
        self.session.publish(Vec::new(), terminal);
    }
}

impl Drop for ScanSource {
    fn drop(&mut self) {
        if !self.complete {
            crate::report_error!(
                "windows-ui-scan",
                "{} provider exited without a terminal status",
                self.name
            );
        }
    }
}

fn combined_status(statuses: &[UiScanStatus]) -> UiScanStatus {
    if statuses
        .iter()
        .any(|status| status == &UiScanStatus::ContextChanged)
    {
        return UiScanStatus::ContextChanged;
    }
    if statuses
        .iter()
        .any(|status| status == &UiScanStatus::Success)
    {
        return UiScanStatus::Success;
    }
    if statuses
        .iter()
        .any(|status| status == &UiScanStatus::TimedOut)
    {
        return UiScanStatus::TimedOut;
    }
    statuses
        .iter()
        .find_map(|status| match status {
            UiScanStatus::PermissionDenied(message) => {
                Some(UiScanStatus::PermissionDenied(message.clone()))
            }
            UiScanStatus::Unsupported(message) => Some(UiScanStatus::Unsupported(message.clone())),
            UiScanStatus::Failed(message) => Some(UiScanStatus::Failed(message.clone())),
            _ => None,
        })
        .unwrap_or(UiScanStatus::Success)
}

struct SpatialIndex {
    cell_size: f64,
    minimum_spacing: f64,
    iou_threshold: f64,
    cells: HashMap<(i32, i32), SmallVec<[usize; 4]>>,
    oversize: SmallVec<[usize; 16]>,
    rects: Vec<Rect>,
    marks: Vec<u32>,
    query_generation: u32,
}

impl SpatialIndex {
    fn new(minimum_spacing: f64, iou_threshold: f64) -> Self {
        Self {
            cell_size: 64.0,
            minimum_spacing: minimum_spacing.max(1.0),
            iou_threshold,
            cells: HashMap::new(),
            oversize: SmallVec::new(),
            rects: Vec::new(),
            marks: Vec::new(),
            query_generation: 0,
        }
    }

    fn len(&self) -> usize {
        self.rects.len()
    }

    fn insert(&mut self, target: &UiTarget) -> bool {
        let rect = target.rect;
        if !usable_rect(rect) {
            return false;
        }
        let range = self.covered_cells(rect);
        let cell_count =
            i64::from(range.2 - range.0 + 1).saturating_mul(i64::from(range.3 - range.1 + 1));
        let oversize = cell_count > 32;
        self.next_query_generation();
        let duplicate = {
            let generation = self.query_generation;
            let rects = &self.rects;
            let marks = &mut self.marks;
            let mut inspect = |candidate: usize| {
                if marks[candidate] == generation {
                    return false;
                }
                marks[candidate] = generation;
                rectangles_match(
                    rects[candidate],
                    rect,
                    self.iou_threshold,
                    self.minimum_spacing,
                )
            };
            if oversize {
                (0..rects.len()).any(&mut inspect)
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
        };
        if duplicate {
            return false;
        }
        let index = self.rects.len();
        self.rects.push(rect);
        self.marks.push(0);
        if oversize {
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

    fn covered_cells(&self, rect: Rect) -> (i32, i32, i32, i32) {
        let spacing = self.minimum_spacing;
        let cell = |value: f64| (value / self.cell_size).floor() as i32;
        (
            cell(rect.x - spacing),
            cell(rect.y - spacing),
            cell(rect.right() + spacing),
            cell(rect.bottom() + spacing),
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

fn usable_rect(rect: Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width >= 2.0
        && rect.height >= 2.0
}

fn rectangles_match(a: Rect, b: Rect, iou_threshold: f64, minimum_spacing: f64) -> bool {
    let intersection = a.intersect(&b).map_or(0.0, |rect| rect.width * rect.height);
    let a_area = a.width * a.height;
    let b_area = b.width * b.height;
    let union = a_area + b_area - intersection;
    let iou = if union > 0.0 {
        intersection / union
    } else {
        0.0
    };
    let containment = intersection / a_area.min(b_area).max(1.0);
    let ac = a.center();
    let bc = b.center();
    let near = (ac.x - bc.x).hypot(ac.y - bc.y) < minimum_spacing
        && (ac.y - bc.y).abs() <= (a.height.min(b.height) * 0.35).max(2.0);
    iou >= iou_threshold || containment >= 0.8 || near
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect::new(x, y, width, height)
    }

    #[test]
    fn matches_iou_containment_and_same_baseline_centres() {
        assert!(rectangles_match(
            rect(0.0, 0.0, 20.0, 20.0),
            rect(2.0, 2.0, 20.0, 20.0),
            0.5,
            8.0
        ));
        assert!(rectangles_match(
            rect(0.0, 0.0, 30.0, 30.0),
            rect(4.0, 4.0, 5.0, 5.0),
            0.9,
            8.0
        ));
        assert!(rectangles_match(
            rect(0.0, 0.0, 8.0, 10.0),
            rect(6.0, 0.0, 8.0, 10.0),
            0.9,
            8.0
        ));
    }

    #[test]
    fn adjacent_buttons_and_cross_line_text_remain_distinct() {
        assert!(!rectangles_match(
            rect(0.0, 0.0, 20.0, 20.0),
            rect(24.0, 0.0, 20.0, 20.0),
            0.5,
            8.0
        ));
        assert!(!rectangles_match(
            rect(0.0, 0.0, 20.0, 8.0),
            rect(0.0, 10.0, 20.0, 8.0),
            0.5,
            8.0
        ));
    }

    #[test]
    fn spatial_index_finds_containment_outside_neighbouring_center_cells() {
        let mut index = SpatialIndex::new(8.0, 0.5);
        let target = |rect| UiTarget {
            rect,
            name: String::new(),
            role: "control".into(),
            native_role: None,
        };
        assert!(index.insert(&target(rect(0.0, 0.0, 200.0, 80.0))));
        assert!(!index.insert(&target(rect(170.0, 10.0, 20.0, 20.0))));
        assert!(index.insert(&target(rect(220.0, 10.0, 20.0, 20.0))));
    }

    #[test]
    fn providers_share_first_writer_dedup_and_one_terminal() {
        let mailbox = Arc::new(ScanMailbox::default());
        let generation = mailbox.begin(41);
        let (events, _ignored) = mpsc::channel();
        let session = ScanSession::new(
            41,
            generation,
            2,
            &VisionOptions::default(),
            Arc::clone(&mailbox),
            super::super::EventSender::without_wake(events),
        );
        let first = session.source("first");
        let second = session.source("second");
        let target = |rect, name: &str| UiTarget {
            rect,
            name: name.into(),
            role: "control".into(),
            native_role: None,
        };
        assert_eq!(
            first.push(vec![target(rect(0.0, 0.0, 40.0, 20.0), "first text")]),
            1
        );
        let first_result = mailbox.take().unwrap();
        assert_eq!(first_result.status, UiScanStatus::Partial);
        assert_eq!(first_result.targets[0].name, "first text");
        assert_eq!(
            second.push(vec![
                target(rect(1.0, 1.0, 40.0, 20.0), "later replacement"),
                target(rect(80.0, 0.0, 40.0, 20.0), "unique"),
            ]),
            1
        );
        first.finish(UiScanStatus::TimedOut);
        assert!(mailbox.take().is_none());
        second.finish(UiScanStatus::Success);
        let result = mailbox.take().unwrap();
        assert_eq!(result.status, UiScanStatus::Success);
        assert_eq!(result.targets.len(), 1);
        assert_eq!(result.targets[0].name, "unique");
        assert!(mailbox.take().is_none());
    }
}
