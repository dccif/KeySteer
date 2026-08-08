//! The render contract between modes and platform backends.
//!
//! A mode never talks to a native drawing API. It builds an [`OverlayScene`]
//! out of primitives whose styling is **already fully resolved** to concrete
//! colors and sizes. The backend is therefore a dumb renderer: it needs no
//! knowledge of themes, configuration, grids or hints.
//!
//! This is what lets a plugin draw its own grid or full-screen mode with
//! exactly the fidelity of the built-in modes — it has the same primitives.

use serde::{Deserialize, Serialize};

use super::geometry::{Point, Rect};

/// Straight RGBA, 8 bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parse the configuration's canonical `#RRGGBBAA` representation.
    pub fn parse(hex: &str) -> Option<Self> {
        let hex = hex.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            8 => Some(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// Parse strictly as `#RRGGBBAA`.
    pub fn parse_rgba(hex: &str) -> Option<Self> {
        Self::parse(hex)
    }

    /// Same color at a fraction of its current opacity.
    pub fn with_opacity(self, factor: f64) -> Self {
        Self {
            a: (self.a as f64 * factor.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    pub fn is_transparent(&self) -> bool {
        self.a == 0
    }

    /// Premultiplied channels, as layered-window and Direct2D targets expect.
    pub fn premultiplied(&self) -> [u8; 4] {
        let a = self.a as u32;
        let p = |c: u8| ((c as u32 * a) / 255) as u8;
        [p(self.r), p(self.g), p(self.b), self.a]
    }

    /// Relative luminance, for picking readable foregrounds.
    pub fn luminance(&self) -> f64 {
        let c = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * c(self.r) + 0.7152 * c(self.g) + 0.0722 * c(self.b)
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
    }
}

/// Where a label sits relative to the element it annotates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Top,
    Center,
    #[default]
    Bottom,
}

impl Placement {
    /// Position a label of the given size against `element`.
    pub fn place(&self, element: &Rect, width: f64, height: f64) -> Rect {
        let x = element.center().x - width / 2.0;
        let y = match self {
            Placement::Top => element.top() - height,
            Placement::Center => element.center().y - height / 2.0,
            Placement::Bottom => element.bottom(),
        };
        Rect::new(x, y, width, height)
    }
}

/// Fully-resolved visual style for a text label. No theme lookups remain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelStyle {
    pub background: Color,
    pub text_color: Color,
    /// Color of the already-typed prefix, so users see their progress.
    pub matched_text_color: Color,
    pub border_color: Color,
    pub border_width: f64,
    pub border_radius: f64,
    pub padding_x: f64,
    pub padding_y: f64,
    pub font_size: f64,
    /// Empty means the platform default UI font.
    pub font_family: String,
    pub bold: bool,
}

impl Default for LabelStyle {
    fn default() -> Self {
        Self {
            background: Color::rgba(0x0A, 0x13, 0x38, 0xF2),
            text_color: Color::rgb(0xE8, 0xEE, 0xFF),
            matched_text_color: Color::rgb(0x8F, 0xA2, 0xF0),
            border_color: Color::rgb(0x6E, 0x82, 0xD6),
            border_width: 1.0,
            border_radius: 4.0,
            padding_x: 4.0,
            padding_y: 2.0,
            font_size: 10.0,
            font_family: String::new(),
            bold: true,
        }
    }
}

impl LabelStyle {
    /// Scale text and its surrounding metrics to occupy at most 80% of `rect`.
    /// The estimate deliberately matches the cross-platform label layout used
    /// by the overlay backends, so nested grid labels remain inside their cell.
    pub fn fit_scale(&self, text: &str, rect: Rect) -> f64 {
        let characters = text.chars().count().max(1) as f64;
        let width = self.font_size * 0.75 * characters + self.padding_x * 2.0;
        let height = self.font_size * 1.4 + self.padding_y * 2.0;
        ((rect.width * 0.8 / width.max(1.0)).min(rect.height * 0.8 / height.max(1.0)))
            .clamp(0.0, 1.0)
    }

    pub fn scaled(&self, scale: f64) -> Self {
        let scale = scale.clamp(0.0, 1.0);
        Self {
            font_size: (self.font_size * scale).max(1.0),
            border_width: self.border_width * scale,
            border_radius: self.border_radius * scale,
            padding_x: self.padding_x * scale,
            padding_y: self.padding_y * scale,
            ..self.clone()
        }
    }

    pub fn fit_to(&self, text: &str, rect: Rect) -> Self {
        self.scaled(self.fit_scale(text, rect))
    }
}

/// A text label to draw — a hint code, a grid cell key, a status badge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayLabel {
    pub text: String,
    /// Where to draw. Use [`Placement::place`] to derive it from an element.
    pub rect: Rect,
    pub style: LabelStyle,
    /// Length of the leading substring of `text` already typed by the user;
    /// the backend paints it with `style.matched_text_color`.
    pub matched_prefix_len: usize,
    /// Higher draws later (on top).
    pub z_index: i32,
    /// When false the backend centers text in `rect` without growing it.
    pub fit_to_text: bool,
}

impl OverlayLabel {
    pub fn new(text: impl Into<String>, rect: Rect, style: LabelStyle) -> Self {
        Self {
            text: text.into(),
            rect,
            style,
            matched_prefix_len: 0,
            z_index: 0,
            fit_to_text: false,
        }
    }

    pub fn with_matched_prefix(mut self, len: usize) -> Self {
        self.matched_prefix_len = len.min(self.text.chars().count());
        self
    }

    pub fn with_z_index(mut self, z: i32) -> Self {
        self.z_index = z;
        self
    }

    pub fn fitted(mut self) -> Self {
        self.fit_to_text = true;
        self
    }
}

/// A non-text primitive: grid lines, cell fills, element outlines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverlayShape {
    /// Filled and/or stroked rectangle.
    Rect {
        rect: Rect,
        fill: Color,
        stroke: Color,
        stroke_width: f64,
        corner_radius: f64,
        z_index: i32,
    },
    /// Straight line, used for recursive-grid rulings.
    Line {
        from: Point,
        to: Point,
        color: Color,
        width: f64,
        z_index: i32,
    },
}

impl OverlayShape {
    pub fn outline(rect: Rect, stroke: Color, stroke_width: f64) -> Self {
        Self::Rect {
            rect,
            fill: Color::TRANSPARENT,
            stroke,
            stroke_width,
            corner_radius: 0.0,
            z_index: -1,
        }
    }

    pub fn fill(rect: Rect, fill: Color) -> Self {
        Self::Rect {
            rect,
            fill,
            stroke: Color::TRANSPARENT,
            stroke_width: 0.0,
            corner_radius: 0.0,
            z_index: -1,
        }
    }

    pub fn line(from: Point, to: Point, color: Color, width: f64) -> Self {
        Self::Line {
            from,
            to,
            color,
            width,
            z_index: 0,
        }
    }

    pub fn z_index(&self) -> i32 {
        match self {
            Self::Rect { z_index, .. } | Self::Line { z_index, .. } => *z_index,
        }
    }
}

/// A lightweight marker pinned to the current cursor position.
///
/// Backends may put this on a separate native layer so pointer movement never
/// invalidates a full-screen grid or hint bitmap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorMarker {
    pub center: Point,
    pub radius: f64,
    pub fill: Color,
    pub stroke: Color,
    pub stroke_width: f64,
}

/// A badge pinned near the cursor showing the active mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Indicator {
    pub text: String,
    /// Optional second badge listing synthetic keys/buttons currently held by
    /// `press` or `toggle` actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_text: Option<String>,
    /// Anchor point with `x` as the shared right edge of both badges and `y`
    /// as the first badge's top edge; cursor offsets are already applied.
    pub position: Point,
    pub style: LabelStyle,
}

/// One complete frame for the backend to present.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OverlayScene {
    /// Shapes are drawn beneath labels of equal z-index.
    pub shapes: Vec<OverlayShape>,
    pub labels: Vec<OverlayLabel>,
    pub cursor_marker: Option<CursorMarker>,
    pub indicator: Option<Indicator>,
    /// Dim the whole desktop before drawing, e.g. for grid modes.
    pub backdrop: Option<Color>,
    /// Region the scene occupies; `None` means the whole virtual desktop.
    pub clip: Option<Rect>,
}

impl OverlayScene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a scene with exact primitive capacity for allocation-sensitive
    /// mode rendering.
    pub fn with_capacity(shapes: usize, labels: usize) -> Self {
        Self {
            shapes: Vec::with_capacity(shapes),
            labels: Vec::with_capacity(labels),
            ..Self::default()
        }
    }

    pub fn with_backdrop(mut self, color: Color) -> Self {
        self.backdrop = Some(color);
        self
    }

    pub fn push_label(&mut self, label: OverlayLabel) -> &mut Self {
        self.labels.push(label);
        self
    }

    pub fn push_shape(&mut self, shape: OverlayShape) -> &mut Self {
        self.shapes.push(shape);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
            && self.shapes.is_empty()
            && self.cursor_marker.is_none()
            && self.indicator.is_none()
    }

    /// Sort primitives by z-index so backends may draw them in order.
    pub fn sorted(mut self) -> Self {
        self.shapes.sort_by_key(OverlayShape::z_index);
        self.labels.sort_by_key(|l| l.z_index);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_canonical_rgba() {
        assert!(Color::parse("#F00").is_none());
        assert!(Color::parse("#FF0000").is_none());
        assert_eq!(
            Color::parse_rgba("#FF000080").unwrap(),
            Color::rgba(255, 0, 0, 0x80)
        );
        assert_eq!(Color::rgba(255, 0, 0, 0x80).to_hex(), "#FF000080");
    }

    #[test]
    fn opacity_matches_neru_alpha_reference() {
        let c = Color::rgb(0, 0, 0);
        assert_eq!(c.with_opacity(0.95).a, 0xF2);
        assert_eq!(c.with_opacity(0.70).a, 0xB3);
        assert_eq!(c.with_opacity(0.60).a, 0x99);
        assert_eq!(c.with_opacity(0.30).a, 0x4D);
    }

    #[test]
    fn placement_positions_label_against_element() {
        let el = Rect::new(100.0, 100.0, 50.0, 20.0);
        assert_eq!(
            Placement::Bottom.place(&el, 10.0, 6.0),
            Rect::new(120.0, 120.0, 10.0, 6.0)
        );
        assert_eq!(
            Placement::Top.place(&el, 10.0, 6.0),
            Rect::new(120.0, 94.0, 10.0, 6.0)
        );
    }

    #[test]
    fn label_style_scales_font_and_padding_to_fit_a_small_cell() {
        let style = LabelStyle {
            font_size: 20.0,
            padding_x: 8.0,
            padding_y: 4.0,
            ..Default::default()
        };
        let rect = Rect::new(0.0, 0.0, 8.0, 10.0);
        let fitted = style.fit_to("a", rect);
        let width = fitted.font_size * 0.75 + fitted.padding_x * 2.0;
        let height = fitted.font_size * 1.4 + fitted.padding_y * 2.0;

        assert!(fitted.font_size < style.font_size);
        assert!(width <= rect.width * 0.8 + f64::EPSILON);
        assert!(height <= rect.height * 0.8 + f64::EPSILON);
    }

    #[test]
    fn premultiply_scales_channels_by_alpha() {
        assert_eq!(
            Color::rgba(255, 128, 0, 128).premultiplied(),
            [128, 64, 0, 128]
        );
    }
}
