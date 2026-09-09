//! Platform-independent window layout language and constrained split trees.
use std::collections::BTreeMap;

use super::window::{WindowId, WindowInfo};
use super::{Direction, Point, Rect};

pub const DEFAULT_SPLIT_RATIOS: [f64; 5] = [0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75];
pub const RATIOS: [f64; 6] = [0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    pub fn of(direction: Direction) -> Self {
        match direction {
            Direction::Left | Direction::Right => Self::X,
            Direction::Up | Direction::Down => Self::Y,
        }
    }
}

fn negative(direction: Direction) -> bool {
    matches!(direction, Direction::Left | Direction::Up)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisPlacement {
    pub near: bool,
    pub ratio: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuickPlacement {
    pub horizontal: Option<AxisPlacement>,
    pub vertical: Option<AxisPlacement>,
}

impl QuickPlacement {
    pub fn step(&mut self, direction: Direction) {
        self.step_with(direction, &RATIOS);
    }

    pub fn step_with(&mut self, direction: Direction, ratios: &[f64]) {
        let axis = match Axis::of(direction) {
            Axis::X => &mut self.horizontal,
            Axis::Y => &mut self.vertical,
        };
        let near = negative(direction);
        match axis {
            None => {
                *axis = Some(AxisPlacement {
                    near,
                    ratio: ratios
                        .iter()
                        .enumerate()
                        .min_by(|(_, a), (_, b)| (**a - 0.5).abs().total_cmp(&(**b - 0.5).abs()))
                        .map_or(0, |(i, _)| i),
                })
            }
            Some(value) if value.near == near => value.ratio = value.ratio.saturating_sub(1),
            Some(value) => value.ratio = (value.ratio + 1).min(ratios.len().saturating_sub(1)),
        }
    }

    pub fn rect(self) -> Rect {
        self.rect_with(&RATIOS)
    }

    pub fn rect_with(self, ratios: &[f64]) -> Rect {
        let axis = |value: Option<AxisPlacement>| {
            value.map_or((0.0, 1.0), |value| {
                let ratio = ratios.get(value.ratio).copied().unwrap_or(1.0);
                (if value.near { 0.0 } else { 1.0 - ratio }, ratio)
            })
        };
        let (x, width) = axis(self.horizontal);
        let (y, height) = axis(self.vertical);
        Rect::new(x, y, width, height)
    }

    pub fn caption(self) -> String {
        self.caption_with(&RATIOS)
    }

    pub fn caption_with(self, ratios: &[f64]) -> String {
        const NAMES: [&str; 6] = ["1/4", "1/3", "1/2", "2/3", "3/4", "1"];
        let axis = |value: Option<AxisPlacement>, near: &str, far: &str| {
            value.map_or_else(
                || "1".into(),
                |v| {
                    let ratio = ratios.get(v.ratio).copied().unwrap_or(1.0);
                    let name = RATIOS
                        .iter()
                        .position(|r| (*r - ratio).abs() < 1e-9)
                        .map_or_else(|| format!("{ratio:.3}"), |i| NAMES[i].into());
                    format!("{} {}", if v.near { near } else { far }, name)
                },
            )
        };
        format!(
            "{} × {}",
            axis(self.horizontal, "Left", "Right"),
            axis(self.vertical, "Top", "Bottom")
        )
    }
}

/// Convert a normalized cell to the actual inset rectangle. Shared by both
/// front ends and the native worker, including their minimum-size calculations.
pub fn placed_rect(area: Rect, normalized: Rect, gap: f64) -> Rect {
    let gap = gap.max(0.0).min(area.width.min(area.height) / 8.0);
    Rect::new(
        area.x + area.width * normalized.x,
        area.y + area.height * normalized.y,
        area.width * normalized.width,
        area.height * normalized.height,
    )
    .inset(gap / 2.0, gap / 2.0)
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    Split {
        axis: Axis,
        ratio: f64,
        first: Box<Self>,
        second: Box<Self>,
    },
    Slot {
        id: u32,
        window: Option<WindowId>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutTree {
    pub root: LayoutNode,
    pub selected: u32,
    next_slot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutSlot {
    pub id: u32,
    pub window: Option<WindowId>,
    pub rect: Rect,
}

impl LayoutNode {
    fn contains(&self, slot: u32) -> bool {
        match self {
            Self::Slot { id, .. } => *id == slot,
            Self::Split { first, second, .. } => first.contains(slot) || second.contains(slot),
        }
    }

    fn slot_mut(&mut self, slot: u32) -> Option<&mut Option<WindowId>> {
        match self {
            Self::Slot { id, window } => (*id == slot).then_some(window),
            Self::Split { first, second, .. } => {
                first.slot_mut(slot).or_else(|| second.slot_mut(slot))
            }
        }
    }

    fn slots(&self, rect: Rect, out: &mut Vec<LayoutSlot>) {
        match self {
            Self::Slot { id, window } => out.push(LayoutSlot {
                id: *id,
                window: *window,
                rect,
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(rect, *axis, *ratio);
                first.slots(a, out);
                second.slots(b, out);
            }
        }
    }

    fn split(&mut self, slot: u32, direction: Direction, new_id: u32) -> bool {
        match self {
            Self::Slot { id, .. } if *id == slot => {
                let old = std::mem::replace(
                    self,
                    Self::Slot {
                        id: new_id,
                        window: None,
                    },
                );
                let empty = Self::Slot {
                    id: new_id,
                    window: None,
                };
                let (a, b) = if negative(direction) {
                    (empty, old)
                } else {
                    (old, empty)
                };
                *self = Self::Split {
                    axis: Axis::of(direction),
                    ratio: 0.5,
                    first: Box::new(a),
                    second: Box::new(b),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(slot, direction, new_id) || second.split(slot, direction, new_id)
            }
            _ => false,
        }
    }

    fn resize(&mut self, slot: u32, direction: Direction, ratios: &[f64]) -> bool {
        let Self::Split {
            axis,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        let child = if first.contains(slot) {
            first
        } else if second.contains(slot) {
            second
        } else {
            return false;
        };
        if child.resize(slot, direction, ratios) {
            return true;
        }
        if *axis != Axis::of(direction) {
            return false;
        }
        let next = if negative(direction) {
            ratios.iter().rev().find(|r| **r < *ratio - 1e-6)
        } else {
            ratios.iter().find(|r| **r > *ratio + 1e-6)
        };
        if let Some(next) = next {
            *ratio = *next;
        }
        true
    }
}

fn split_rect(rect: Rect, axis: Axis, ratio: f64) -> (Rect, Rect) {
    match axis {
        Axis::X => (
            Rect::new(rect.x, rect.y, rect.width * ratio, rect.height),
            Rect::new(
                rect.x + rect.width * ratio,
                rect.y,
                rect.width * (1.0 - ratio),
                rect.height,
            ),
        ),
        Axis::Y => (
            Rect::new(rect.x, rect.y, rect.width, rect.height * ratio),
            Rect::new(
                rect.x,
                rect.y + rect.height * ratio,
                rect.width,
                rect.height * (1.0 - ratio),
            ),
        ),
    }
}

impl LayoutTree {
    /// Try balanced row/column arrangements against actual native minimums.
    /// Window overlap on entry must not turn every split into a narrow column.
    pub fn automatic(
        windows: &[WindowInfo],
        target: Option<WindowId>,
        area: Rect,
        minimums: &BTreeMap<WindowId, Point>,
        gap: f64,
    ) -> Result<Self, String> {
        if windows.is_empty() {
            return Ok(Self::import(windows, target, area));
        }
        let count = windows.len();
        let mut columns: Vec<_> = (1..=count).collect();
        columns.sort_by(|a, b| {
            let score = |cols: usize| {
                ((area.width / cols as f64) / (area.height / count.div_ceil(cols) as f64))
                    .ln()
                    .abs()
            };
            score(*a).total_cmp(&score(*b))
        });
        for columns in columns {
            let rows = count.div_ceil(columns);
            fn row(windows: &[WindowInfo], start: usize, end: usize) -> LayoutNode {
                if end - start == 1 {
                    return LayoutNode::Slot {
                        id: start as u32 + 1,
                        window: Some(windows[start].id),
                    };
                }
                let mid = start + (end - start) / 2;
                LayoutNode::Split {
                    axis: Axis::X,
                    ratio: (mid - start) as f64 / (end - start) as f64,
                    first: Box::new(row(windows, start, mid)),
                    second: Box::new(row(windows, mid, end)),
                }
            }
            fn grid(
                windows: &[WindowInfo],
                columns: usize,
                start: usize,
                end: usize,
            ) -> LayoutNode {
                if end - start == 1 {
                    return row(windows, start * columns, (end * columns).min(windows.len()));
                }
                let mid = start + (end - start) / 2;
                LayoutNode::Split {
                    axis: Axis::Y,
                    ratio: (mid - start) as f64 / (end - start) as f64,
                    first: Box::new(grid(windows, columns, start, mid)),
                    second: Box::new(grid(windows, columns, mid, end)),
                }
            }
            let mut tree = Self::from_saved_root(grid(windows, columns, 0, rows));
            if let Some(target) = target {
                tree.focus_window(target);
            }
            if tree.fit(minimums, area, gap).is_ok() {
                return Ok(tree);
            }
        }
        Err("The available screen cannot fit these windows' minimum sizes; reduce the number of windows".into())
    }
    pub(super) fn from_saved_root(root: LayoutNode) -> Self {
        let mut tree = Self {
            root,
            selected: 1,
            next_slot: 1,
        };
        let slots = tree.slots();
        tree.next_slot = slots.iter().map(|s| s.id).max().unwrap_or(1) + 1;
        tree.selected = slots.first().map_or(1, |s| s.id);
        tree
    }
    pub fn import(windows: &[WindowInfo], target: Option<WindowId>, area: Rect) -> Self {
        // Input order is the stable display-number order, independent of Z-order.
        fn build(mut values: Vec<(u32, WindowId, Rect)>, area: Rect) -> LayoutNode {
            if values.len() == 1 {
                return LayoutNode::Slot {
                    id: values[0].0,
                    window: Some(values[0].1),
                };
            }
            type ImportCandidate = (f64, Axis, usize, f64, Vec<(u32, WindowId, Rect)>);
            let mut best: Option<ImportCandidate> = None;
            for axis in [Axis::X, Axis::Y] {
                values.sort_by(|a, b| {
                    let center = |r: Rect| match axis {
                        Axis::X => r.center().x,
                        Axis::Y => r.center().y,
                    };
                    center(a.2).total_cmp(&center(b.2)).then(a.0.cmp(&b.0))
                });
                for cut in 1..values.len() {
                    let end = values[..cut]
                        .iter()
                        .map(|v| match axis {
                            Axis::X => v.2.right(),
                            Axis::Y => v.2.bottom(),
                        })
                        .fold(f64::NEG_INFINITY, f64::max);
                    let start = values[cut..]
                        .iter()
                        .map(|v| match axis {
                            Axis::X => v.2.x,
                            Axis::Y => v.2.y,
                        })
                        .fold(f64::INFINITY, f64::min);
                    let (origin, length) = match axis {
                        Axis::X => (area.x, area.width),
                        Axis::Y => (area.y, area.height),
                    };
                    let score = (start - end) / length.max(1.0);
                    if score > 0.0 && best.as_ref().is_none_or(|b| score > b.0) {
                        best = Some((
                            score,
                            axis,
                            cut,
                            ((start + end) / 2.0 - origin) / length,
                            values.clone(),
                        ));
                    }
                }
            }
            let (axis, cut, ratio, mut ordered) = if let Some((_, axis, cut, ratio, values)) = best
            {
                (axis, cut, ratio.clamp(0.05, 0.95), values)
            } else {
                let spread = |axis| {
                    let coord = |v: &(u32, WindowId, Rect)| match axis {
                        Axis::X => v.2.center().x,
                        Axis::Y => v.2.center().y,
                    };
                    (values.iter().map(coord).fold(f64::NEG_INFINITY, f64::max)
                        - values.iter().map(coord).fold(f64::INFINITY, f64::min))
                        / match axis {
                            Axis::X => area.width.max(1.0),
                            Axis::Y => area.height.max(1.0),
                        }
                };
                let axis = if spread(Axis::X) >= spread(Axis::Y) {
                    Axis::X
                } else {
                    Axis::Y
                };
                values.sort_by(|a, b| {
                    let coord = |r: Rect| match axis {
                        Axis::X => r.center().x,
                        Axis::Y => r.center().y,
                    };
                    coord(a.2).total_cmp(&coord(b.2)).then(a.0.cmp(&b.0))
                });
                let cut = values.len() / 2;
                (axis, cut, cut as f64 / values.len() as f64, values)
            };
            let tail = ordered.split_off(cut);
            let (a, b) = split_rect(area, axis, ratio);
            LayoutNode::Split {
                axis,
                ratio,
                first: Box::new(build(ordered, a)),
                second: Box::new(build(tail, b)),
            }
        }
        let root = if windows.is_empty() {
            LayoutNode::Slot {
                id: 1,
                window: None,
            }
        } else {
            build(
                windows
                    .iter()
                    .enumerate()
                    .map(|(i, w)| (i as u32 + 1, w.id, w.bounds))
                    .collect(),
                area,
            )
        };
        let mut tree = Self {
            root,
            selected: 1,
            next_slot: windows.len().max(1) as u32 + 1,
        };
        tree.selected = tree
            .slots()
            .iter()
            .find(|s| s.window == target && target.is_some())
            .map_or(1, |s| s.id);
        tree
    }

    pub fn slots(&self) -> Vec<LayoutSlot> {
        let mut out = Vec::with_capacity(self.slot_count() as usize);
        self.root.slots(Rect::new(0.0, 0.0, 1.0, 1.0), &mut out);
        out
    }

    /// Splits only append stable IDs; undo restores both the tree and counter.
    pub fn slot_count(&self) -> u32 {
        fn count(node: &LayoutNode) -> u32 {
            match node {
                LayoutNode::Slot { .. } => 1,
                LayoutNode::Split { first, second, .. } => count(first) + count(second),
            }
        }
        count(&self.root)
    }

    pub fn focus_window(&mut self, window: WindowId) -> bool {
        if let Some(slot) = self.slots().iter().find(|s| s.window == Some(window)) {
            self.selected = slot.id;
            true
        } else {
            false
        }
    }

    /// Remove a leaf and promote its sibling. Window identities are not closed;
    /// the removed leaf's occupant simply leaves this tree.
    pub fn remove_selected(&mut self) -> bool {
        fn remove(node: &mut LayoutNode, selected: u32) -> bool {
            let LayoutNode::Split { first, second, .. } = node else {
                return false;
            };
            if matches!(first.as_ref(), LayoutNode::Slot { id, .. } if *id == selected) {
                *node = second.as_ref().clone();
                return true;
            }
            if matches!(second.as_ref(), LayoutNode::Slot { id, .. } if *id == selected) {
                *node = first.as_ref().clone();
                return true;
            }
            remove(first, selected) || remove(second, selected)
        }
        if !remove(&mut self.root, self.selected) {
            return false;
        }
        if let Some(slot) = self.slots().first() {
            self.selected = slot.id;
        }
        true
    }

    pub fn focus_slot(&mut self, slot: u32) -> bool {
        if self.root.contains(slot) {
            self.selected = slot;
            true
        } else {
            false
        }
    }

    pub fn navigate(&mut self, direction: Direction) {
        let slots = self.slots();
        let Some(current) = slots.iter().find(|s| s.id == self.selected) else {
            return;
        };
        let from = current.rect.center();
        let (dx, dy) = direction.delta();
        let score = |slot: &&LayoutSlot| {
            let point = slot.rect.center();
            let along = (point.x - from.x) * dx + (point.y - from.y) * dy;
            let across = ((point.x - from.x) * dy - (point.y - from.y) * dx).abs();
            along + across * 2.0
        };
        if let Some(next) = slots
            .iter()
            .filter(|s| {
                let point = s.rect.center();
                (point.x - from.x) * dx + (point.y - from.y) * dy > 1e-6
            })
            .min_by(|a, b| score(a).total_cmp(&score(b)).then(a.id.cmp(&b.id)))
        {
            self.selected = next.id;
        }
    }

    pub fn split(&mut self, direction: Direction) -> bool {
        if self.slot_count() >= 256 || self.next_slot == u32::MAX {
            return false;
        }
        if self.root.split(self.selected, direction, self.next_slot) {
            self.next_slot += 1;
            true
        } else {
            false
        }
    }

    /// Move the nearest matching ancestor divider by screen-space pixels.
    pub fn resize_by(&mut self, direction: Direction, pixels: f64, area: Rect) -> bool {
        fn move_divider(
            node: &mut LayoutNode,
            slot: u32,
            direction: Direction,
            pixels: f64,
            rect: Rect,
        ) -> bool {
            let LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } = node
            else {
                return false;
            };
            let (a, b) = split_rect(rect, *axis, *ratio);
            let found = if first.contains(slot) {
                move_divider(first, slot, direction, pixels, a)
            } else if second.contains(slot) {
                move_divider(second, slot, direction, pixels, b)
            } else {
                return false;
            };
            if found {
                return true;
            }
            if *axis != Axis::of(direction) {
                return false;
            }
            let length = match axis {
                Axis::X => rect.width,
                Axis::Y => rect.height,
            };
            *ratio = (*ratio
                + if negative(direction) { -pixels } else { pixels } / length.max(1.0))
            .clamp(0.0, 1.0);
            true
        }
        move_divider(&mut self.root, self.selected, direction, pixels, area)
    }

    pub fn resize(&mut self, direction: Direction, ratios: &[f64]) -> bool {
        self.root.resize(self.selected, direction, ratios)
    }

    pub fn move_window(&mut self, window: WindowId, destination: u32) -> bool {
        let source = self
            .slots()
            .iter()
            .find(|s| s.window == Some(window))
            .map(|s| s.id);
        let Some(other) = self.root.slot_mut(destination) else {
            return false;
        };
        if source == Some(destination) {
            return false;
        }
        let occupant = other.replace(window);
        if let Some(source) = source.and_then(|id| self.root.slot_mut(id)) {
            *source = occupant;
        }
        self.selected = destination;
        true
    }

    pub fn swap_windows(&mut self, source: WindowId, target: WindowId) -> bool {
        let destination = self
            .slots()
            .iter()
            .find(|s| s.window == Some(target))
            .map(|s| s.id);
        destination.is_some_and(|slot| self.move_window(source, slot))
    }

    pub fn retain_windows(&mut self, live: &[WindowId]) {
        fn retain(node: &mut LayoutNode, live: &[WindowId]) {
            match node {
                LayoutNode::Slot { window, .. } => {
                    if window.is_some_and(|w| !live.contains(&w)) {
                        *window = None;
                    }
                }
                LayoutNode::Split { first, second, .. } => {
                    retain(first, live);
                    retain(second, live);
                }
            }
        }
        retain(&mut self.root, live);
    }

    /// Measure once, then constrain top-down: O(number of nodes), no native calls.
    /// Call on a candidate clone so an infeasible edit cannot mutate the live tree.
    pub fn fit(
        &mut self,
        minimums: &BTreeMap<WindowId, Point>,
        area: Rect,
        gap: f64,
    ) -> Result<(), String> {
        struct Measure {
            min: Point,
            children: Option<(Box<Self>, Box<Self>)>,
        }
        fn measure(node: &LayoutNode, minimums: &BTreeMap<WindowId, Point>, gap: f64) -> Measure {
            match node {
                LayoutNode::Slot { window, .. } => {
                    let min = window
                        .and_then(|id| minimums.get(&id))
                        .copied()
                        .unwrap_or(Point::new(24.0, 24.0));
                    Measure {
                        min: Point::new(min.x + gap, min.y + gap),
                        children: None,
                    }
                }
                LayoutNode::Split {
                    axis,
                    first,
                    second,
                    ..
                } => {
                    let a = measure(first, minimums, gap);
                    let b = measure(second, minimums, gap);
                    let min = match axis {
                        Axis::X => Point::new(a.min.x + b.min.x, a.min.y.max(b.min.y)),
                        Axis::Y => Point::new(a.min.x.max(b.min.x), a.min.y + b.min.y),
                    };
                    Measure {
                        min,
                        children: Some((Box::new(a), Box::new(b))),
                    }
                }
            }
        }
        fn constrain(node: &mut LayoutNode, measured: &Measure, rect: Rect) {
            if let LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } = node
            {
                let Some((a, b)) = measured.children.as_ref() else {
                    return;
                };
                let (lo, hi) = match axis {
                    Axis::X => (a.min.x / rect.width, 1.0 - b.min.x / rect.width),
                    Axis::Y => (a.min.y / rect.height, 1.0 - b.min.y / rect.height),
                };
                *ratio = ratio.clamp(lo, hi.max(lo));
                let (ra, rb) = split_rect(rect, *axis, *ratio);
                constrain(first, a, ra);
                constrain(second, b, rb);
            }
        }
        let gap = gap.max(0.0).min(area.width.min(area.height) / 8.0);
        let measured = measure(&self.root, minimums, gap);
        if area.width + 1e-6 < measured.min.x || area.height + 1e-6 < measured.min.y {
            return Err("Windows' minimum sizes do not fit this split layout".into());
        }
        constrain(&mut self.root, &measured, area);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_grid_fits_overlapping_windows_instead_of_narrow_columns() {
        let area = Rect::new(-1920.0, 40.0, 1920.0, 1080.0);
        let windows: Vec<_> = (1..=9)
            .map(|id| WindowInfo {
                id: WindowId(id),
                title: String::new(),
                app: String::new(),
                bounds: Rect::new(-1800.0, 80.0, 1000.0, 700.0),
                screen: 0,
                resizable: true,
                maximized: false,
                minimized: false,
                fullscreen: false,
            })
            .collect();
        let minimums = windows
            .iter()
            .map(|w| (w.id, Point::new(600.0, 250.0)))
            .collect();
        assert!(
            LayoutTree::import(&windows, None, area)
                .fit(&minimums, area, 8.0)
                .is_err()
        );
        let tree =
            LayoutTree::automatic(&windows, Some(WindowId(5)), area, &minimums, 8.0).unwrap();
        assert_eq!(tree.selected, 5);
        let slots = tree.slots();
        assert_eq!(slots.len(), 9);
        for (index, slot) in slots.iter().enumerate() {
            let rect = placed_rect(area, slot.rect, 8.0);
            assert!(rect.width >= 600.0 && rect.height >= 250.0);
            assert!(
                slots[..index]
                    .iter()
                    .all(|other| slot.rect.intersect(&other.rect).is_none())
            );
        }
        let impossible = windows
            .iter()
            .map(|w| (w.id, Point::new(1000.0, 700.0)))
            .collect();
        assert!(LayoutTree::automatic(&windows, None, area, &impossible, 8.0).is_err());
    }

    #[test]
    fn custom_dividers_step_from_imported_ratio_and_stop_at_endpoints() {
        let mut tree = LayoutTree::import(&[], None, Rect::new(0.0, 0.0, 1000.0, 700.0));
        tree.split(Direction::Right);
        let ratios = [0.2, 0.4, 0.6, 0.8];
        for expected in [0.6, 0.8, 0.8] {
            tree.resize(Direction::Right, &ratios);
            assert_eq!(tree.slots()[0].rect.width, expected);
        }
        for expected in [0.6, 0.4, 0.2, 0.2] {
            tree.resize(Direction::Left, &ratios);
            assert_eq!(tree.slots()[0].rect.width, expected);
        }
    }

    #[test]
    fn pixel_dividers_clamp_at_minimums_and_deleted_ids_are_not_reused() {
        let area = Rect::new(0.0, 0.0, 1000.0, 700.0);
        let mut tree = LayoutTree::import(&[], None, area);
        assert!(!tree.resize_by(Direction::Right, 20.0, area));
        tree.split(Direction::Right);
        tree.resize_by(Direction::Right, 20.0, area);
        assert!((tree.slots()[0].rect.width - 0.52).abs() < 1e-9);
        tree.resize_by(Direction::Right, 10000.0, area);
        tree.fit(&BTreeMap::new(), area, 8.0).unwrap();
        assert!((tree.slots()[1].rect.width * area.width - 32.0).abs() < 1e-9);
        tree.focus_slot(2);
        assert!(tree.remove_selected());
        assert_eq!(tree.slot_count(), 1);
        assert_eq!(tree.selected, 1);
        assert!(!tree.remove_selected());
        tree.split(Direction::Down);
        assert_eq!(
            tree.slots().iter().map(|s| s.id).collect::<Vec<_>>(),
            [1, 3]
        );
        assert!(!tree.focus_slot(2));
    }

    #[test]
    fn quick_axes_and_endpoints_are_independent() {
        let mut quick = QuickPlacement::default();
        quick.step(Direction::Left);
        assert_eq!(quick.rect(), Rect::new(0.0, 0.0, 0.5, 1.0));
        quick.step(Direction::Up);
        quick.step(Direction::Down);
        assert_eq!(quick.rect(), Rect::new(0.0, 0.0, 0.5, 2.0 / 3.0));
        for _ in 0..10 {
            quick.step(Direction::Left);
        }
        assert_eq!(quick.rect().width, 0.25);
        for _ in 0..10 {
            quick.step(Direction::Right);
        }
        assert_eq!(quick.rect().width, 1.0);
    }

    #[test]
    fn tree_split_move_close_and_constraints_preserve_slots() {
        let mut tree = LayoutTree::import(&[], None, Rect::new(0.0, 0.0, 1000.0, 700.0));
        assert!(tree.move_window(WindowId(1), 1));
        assert!(tree.split(Direction::Right));
        assert_eq!(tree.slots()[0].window, Some(WindowId(1)));
        assert_eq!(tree.slots()[1].window, None);
        assert!(tree.move_window(WindowId(1), 2));
        let mins = [(WindowId(1), Point::new(720.0, 600.0))].into();
        tree.fit(&mins, Rect::new(0.0, 0.0, 1000.0, 700.0), 8.0)
            .unwrap();
        assert!(
            placed_rect(
                Rect::new(0.0, 0.0, 1000.0, 700.0),
                tree.slots()[1].rect,
                8.0
            )
            .width
                >= 720.0 - 1e-6
        );
        assert!(
            tree.fit(&mins, Rect::new(0.0, 0.0, 700.0, 700.0), 8.0)
                .is_err()
        );
        tree.retain_windows(&[]);
        assert_eq!(tree.slots().len(), 2);
        assert!(tree.slots().iter().all(|s| s.window.is_none()));
    }
    #[test]
    fn spatial_import_prefers_gaps_and_ancestor_resize_moves_both_descendants() {
        let window = |id, x, y| WindowInfo {
            id: WindowId(id),
            title: String::new(),
            app: String::new(),
            bounds: Rect::new(x, y, 200.0, 200.0),
            screen: 0,
            resizable: true,
            maximized: false,
            minimized: false,
            fullscreen: false,
        };
        let windows = vec![
            window(1, 700.0, 400.0),
            window(2, 0.0, 0.0),
            window(3, 700.0, 0.0),
        ];
        let mut tree = LayoutTree::import(
            &windows,
            Some(WindowId(1)),
            Rect::new(0.0, 0.0, 1000.0, 700.0),
        );
        let before = tree.slots();
        let left = before
            .iter()
            .find(|s| s.window == Some(WindowId(2)))
            .unwrap();
        let bottom = before
            .iter()
            .find(|s| s.window == Some(WindowId(1)))
            .unwrap();
        let top = before
            .iter()
            .find(|s| s.window == Some(WindowId(3)))
            .unwrap();
        assert!(left.rect.right() <= bottom.rect.x + 1e-6);
        assert!(top.rect.bottom() <= bottom.rect.y + 1e-6);
        let heights = (top.rect.height, bottom.rect.height);
        assert!(tree.resize(Direction::Right, &DEFAULT_SPLIT_RATIOS));
        let after = tree.slots();
        for id in [WindowId(1), WindowId(3)] {
            let old = before.iter().find(|s| s.window == Some(id)).unwrap();
            let new = after.iter().find(|s| s.window == Some(id)).unwrap();
            assert!(new.rect.x > old.rect.x);
            assert!(new.rect.width < old.rect.width);
        }
        assert_eq!(
            (
                after
                    .iter()
                    .find(|s| s.window == Some(WindowId(3)))
                    .unwrap()
                    .rect
                    .height,
                after
                    .iter()
                    .find(|s| s.window == Some(WindowId(1)))
                    .unwrap()
                    .rect
                    .height
            ),
            heights
        );
    }
}
