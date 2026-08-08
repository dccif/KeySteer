//! Geometry primitives shared by the engine, the modes and every backend.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance_to(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(&self, p: &Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let w = self.right().min(other.right()) - x;
        let h = self.bottom().min(other.bottom()) - y;
        (w > 0.0 && h > 0.0).then(|| Rect::new(x, y, w, h))
    }

    /// Smallest rect containing both inputs.
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Shrink (or grow, for negative values) on every edge.
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            (self.width - dx * 2.0).max(0.0),
            (self.height - dy * 2.0).max(0.0),
        )
    }

    /// Subdivide into `rows` x `cols` cells, row-major.
    pub fn subdivide(&self, rows: usize, cols: usize) -> Vec<Rect> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        (0..rows * cols)
            .filter_map(|index| self.subdivision(rows, cols, index))
            .collect()
    }

    /// Compute one row-major subdivision without allocating the complete grid.
    pub fn subdivision(&self, rows: usize, cols: usize, index: usize) -> Option<Rect> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if index >= rows.checked_mul(cols)? {
            return None;
        }
        let cell_width = self.width / cols as f64;
        let cell_height = self.height / rows as f64;
        let row = index / cols;
        let column = index % cols;
        Some(Rect::new(
            self.x + column as f64 * cell_width,
            self.y + row as f64 * cell_height,
            cell_width,
            cell_height,
        ))
    }

    pub fn left(&self) -> f64 {
        self.x
    }
    pub fn right(&self) -> f64 {
        self.x + self.width
    }
    pub fn top(&self) -> f64 {
        self.y
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// A connected display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub bounds: Rect,
    /// Bounds minus system chrome (menu bar / taskbar).
    pub work_area: Rect,
    pub is_primary: bool,
    /// Logical-to-physical pixel ratio (1.0 = non-HiDPI).
    pub scale: f64,
    pub name: Option<String>,
}

impl Screen {
    /// Pick the screen containing `p`, else the primary, else the first.
    pub fn containing<'a>(screens: &'a [Screen], p: &Point) -> Option<&'a Screen> {
        screens
            .iter()
            .find(|s| s.bounds.contains(p))
            .or_else(|| Self::primary(screens))
    }

    pub fn primary(screens: &[Screen]) -> Option<&Screen> {
        screens
            .iter()
            .find(|s| s.is_primary)
            .or_else(|| screens.first())
    }

    /// Union of every screen: the virtual desktop an overlay must cover.
    pub fn virtual_bounds(screens: &[Screen]) -> Rect {
        screens
            .iter()
            .map(|s| s.bounds)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default()
    }
}

/// An interactive element reported by the platform accessibility tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiTarget {
    pub rect: Rect,
    /// Accessible name / label, may be empty.
    pub name: String,
    /// Semantic role in this crate's vocabulary (`button`, `link`, ...).
    pub role: String,
    /// Native control type as reported by the OS, for diagnostics.
    pub native_role: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdivide_covers_the_whole_rect() {
        let r = Rect::new(0.0, 0.0, 900.0, 600.0);
        let cells = r.subdivide(3, 3);
        assert_eq!(cells.len(), 9);
        assert_eq!(cells[0], Rect::new(0.0, 0.0, 300.0, 200.0));
        assert_eq!(cells[8], Rect::new(600.0, 400.0, 300.0, 200.0));
        let covered = cells.into_iter().reduce(|a, b| a.union(&b)).unwrap();
        assert_eq!(covered, r);
    }

    #[test]
    fn subdivision_computes_one_cell_without_building_the_grid() {
        let rect = Rect::new(-100.0, 20.0, 900.0, 600.0);
        assert_eq!(
            rect.subdivision(3, 3, 5),
            Some(Rect::new(500.0, 220.0, 300.0, 200.0))
        );
        assert_eq!(rect.subdivision(3, 3, 9), None);
    }

    #[test]
    fn virtual_bounds_spans_all_screens() {
        let screen = |x: f64, primary: bool| Screen {
            bounds: Rect::new(x, 0.0, 100.0, 100.0),
            work_area: Rect::new(x, 0.0, 100.0, 100.0),
            is_primary: primary,
            scale: 1.0,
            name: None,
        };
        let screens = [screen(0.0, true), screen(100.0, false)];
        assert_eq!(
            Screen::virtual_bounds(&screens),
            Rect::new(0.0, 0.0, 200.0, 100.0)
        );
    }
}
