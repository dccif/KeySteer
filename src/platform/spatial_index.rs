#![forbid(unsafe_code)]

//! Cache-friendly bounded spatial index shared by native UI scanners.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::api::geometry::Rect;

const MAX_CELLS_PER_RECT: i64 = 32;

/// Stores each rectangle once and keeps only compact indices in grid cells.
///
/// The matching policy remains provider-specific: Windows combines IoU,
/// containment and text-baseline proximity, while macOS Vision currently uses
/// IoU only. Queries allocate no temporary candidate list.
pub(crate) struct SpatialIndex {
    cell_size: f64,
    padding: f64,
    minimum_side: f64,
    cells: HashMap<(i32, i32), SmallVec<[u32; 4]>>,
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
            cells: HashMap::new(),
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
        if !self.usable(rect) {
            return false;
        }
        let range = self.covered_cells(rect);
        // Widen before subtracting: hostile or stale native coordinates can
        // saturate the i32 cell conversion at opposite ends. Such a rectangle
        // is simply treated as oversize instead of overflowing a debug build.
        let columns = i64::from(range.2)
            .saturating_sub(i64::from(range.0))
            .saturating_add(1);
        let rows = i64::from(range.3)
            .saturating_sub(i64::from(range.1))
            .saturating_add(1);
        let cell_count = columns.saturating_mul(rows);
        let oversize = cell_count > MAX_CELLS_PER_RECT;
        self.next_query_generation();
        let duplicate = {
            let generation = self.query_generation;
            let rects = &self.rects;
            let marks = &mut self.marks;
            let mut inspect = |candidate: u32| {
                let candidate = candidate as usize;
                if marks[candidate] == generation {
                    return false;
                }
                marks[candidate] = generation;
                matches(rects[candidate], rect)
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
        };
        if duplicate {
            return false;
        }

        let Ok(index) = u32::try_from(self.rects.len()) else {
            return false;
        };
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
