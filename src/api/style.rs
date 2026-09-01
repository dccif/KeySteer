//! Reusable UI style blocks, mirroring neru's `[<mode>.ui]` tables.
//!
//! Every color is optional: omitted values are derived from the theme palette
//! at resolve time, exactly as neru documents. `-1` means "auto" for the
//! numeric fields that neru documents that way.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::api::backend::Appearance;
use crate::api::overlay::{Color, LabelStyle, Placement};

use super::theme::{Palette, ThemedColor};

/// Runtime-neutral mode badge and cursor decoration settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModeIndicator {
    pub cursor: CursorIndicatorUi,
    pub ui: IndicatorUi,
    pub modes: BTreeMap<String, ModeIndicatorEntry>,
}

impl Default for ModeIndicator {
    fn default() -> Self {
        let ui = IndicatorUi {
            label: LabelUi {
                font_size: 11,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            cursor: CursorIndicatorUi::default(),
            ui,
            modes: BTreeMap::from([(
                "normal".into(),
                ModeIndicatorEntry {
                    enabled: Some(true),
                    text: Some("Normal".into()),
                    ..Default::default()
                },
            )]),
        }
    }
}

impl ModeIndicator {
    pub fn for_mode(&self, mode_id: &str, display_name: &str) -> Option<(String, IndicatorUi)> {
        self.for_mode_with(mode_id, || display_name.to_owned())
    }

    pub(crate) fn for_mode_with(
        &self,
        mode_id: &str,
        display_name: impl FnOnce() -> String,
    ) -> Option<(String, IndicatorUi)> {
        let entry = self.modes.get(mode_id);
        let enabled = entry
            .and_then(|entry| entry.enabled)
            .unwrap_or(mode_id != "idle");
        if !enabled {
            return None;
        }
        let text = entry
            .and_then(|entry| entry.text.clone())
            .unwrap_or_else(display_name);
        let ui = entry
            .map(|entry| entry.ui.apply(&self.ui))
            .unwrap_or_else(|| self.ui.clone());
        Some((text, ui))
    }

    pub fn cursor_for_mode(&self, mode_id: &str) -> Option<CursorIndicatorUi> {
        self.cursor_for_mode_ref(mode_id)
            .map(ResolvedCursorIndicatorUi::into_owned)
    }

    pub(crate) fn cursor_for_mode_ref(
        &self,
        mode_id: &str,
    ) -> Option<ResolvedCursorIndicatorUi<'_>> {
        if mode_id == "idle" {
            return None;
        }
        let override_cursor = self.modes.get(mode_id).map(|entry| &entry.cursor);
        let cursor = ResolvedCursorIndicatorUi {
            enabled: override_cursor
                .and_then(|cursor| cursor.enabled)
                .unwrap_or(self.cursor.enabled),
            radius: override_cursor
                .and_then(|cursor| cursor.radius)
                .unwrap_or(self.cursor.radius),
            fill_color: override_cursor
                .and_then(|cursor| cursor.fill_color.as_ref())
                .or(self.cursor.fill_color.as_ref()),
            stroke_color: override_cursor
                .and_then(|cursor| cursor.stroke_color.as_ref())
                .or(self.cursor.stroke_color.as_ref()),
            left_pressed_color: override_cursor
                .and_then(|cursor| cursor.left_pressed_color.as_ref())
                .or(self.cursor.left_pressed_color.as_ref()),
            middle_pressed_color: override_cursor
                .and_then(|cursor| cursor.middle_pressed_color.as_ref())
                .or(self.cursor.middle_pressed_color.as_ref()),
            right_pressed_color: override_cursor
                .and_then(|cursor| cursor.right_pressed_color.as_ref())
                .or(self.cursor.right_pressed_color.as_ref()),
            stroke_width: override_cursor
                .and_then(|cursor| cursor.stroke_width)
                .unwrap_or(self.cursor.stroke_width),
        };
        cursor.enabled.then_some(cursor)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ResolvedCursorIndicatorUi<'a> {
    pub enabled: bool,
    pub radius: i32,
    pub fill_color: Option<&'a ThemedColor>,
    pub stroke_color: Option<&'a ThemedColor>,
    pub left_pressed_color: Option<&'a ThemedColor>,
    pub middle_pressed_color: Option<&'a ThemedColor>,
    pub right_pressed_color: Option<&'a ThemedColor>,
    pub stroke_width: i32,
}

impl ResolvedCursorIndicatorUi<'_> {
    fn into_owned(self) -> CursorIndicatorUi {
        CursorIndicatorUi {
            enabled: self.enabled,
            radius: self.radius,
            fill_color: self.fill_color.cloned(),
            stroke_color: self.stroke_color.cloned(),
            left_pressed_color: self.left_pressed_color.cloned(),
            middle_pressed_color: self.middle_pressed_color.cloned(),
            right_pressed_color: self.right_pressed_color.cloned(),
            stroke_width: self.stroke_width,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModeIndicatorEntry {
    pub enabled: Option<bool>,
    pub text: Option<String>,
    pub cursor: CursorIndicatorOverride,
    pub ui: IndicatorUiOverride,
}

/// `-1` sentinel used by neru for "let the implementation decide".
pub const AUTO: i32 = -1;

fn auto() -> i32 {
    AUTO
}
fn one() -> i32 {
    1
}
fn font_size_default() -> i32 {
    10
}

/// Resolve an optional configured color against a derived default.
pub fn resolve(configured: Option<&ThemedColor>, appearance: Appearance, derived: Color) -> Color {
    configured
        .and_then(|c| c.resolve(appearance))
        .unwrap_or(derived)
}

/// Visual style shared by hint labels, grid cells and badges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelUi {
    pub font_size: i32,
    /// Empty means the platform default UI font.
    pub font_family: String,
    /// `-1` = auto.
    pub border_radius: i32,
    /// `-1` = auto.
    pub padding_x: i32,
    /// `-1` = auto.
    pub padding_y: i32,
    pub border_width: i32,
    pub background_color: Option<ThemedColor>,
    pub text_color: Option<ThemedColor>,
    pub matched_text_color: Option<ThemedColor>,
    pub border_color: Option<ThemedColor>,
}

impl Default for LabelUi {
    fn default() -> Self {
        Self {
            font_size: font_size_default(),
            font_family: String::new(),
            border_radius: auto(),
            padding_x: auto(),
            padding_y: auto(),
            border_width: one(),
            background_color: None,
            text_color: None,
            matched_text_color: None,
            border_color: None,
        }
    }
}

impl LabelUi {
    /// Turn configuration into a concrete [`LabelStyle`].
    ///
    /// `background`, `text` and `border` are the theme-derived defaults used
    /// when the config leaves the corresponding field unset.
    pub fn resolve(
        &self,
        palette: &Palette,
        background: Color,
        text: Color,
        border: Color,
    ) -> LabelStyle {
        let appearance = palette.appearance;
        let font_size = self.font_size.max(1) as f64;
        let background = resolve(self.background_color.as_ref(), appearance, background);
        LabelStyle {
            background,
            text_color: resolve(self.text_color.as_ref(), appearance, text),
            matched_text_color: resolve(
                self.matched_text_color.as_ref(),
                appearance,
                palette.accent_alt,
            ),
            border_color: resolve(self.border_color.as_ref(), appearance, border),
            border_width: self.border_width.max(0) as f64,
            // Auto radius/padding scale with the font so labels stay legible.
            border_radius: if self.border_radius == AUTO {
                (font_size * 0.35).round()
            } else {
                self.border_radius.max(0) as f64
            },
            padding_x: if self.padding_x == AUTO {
                (font_size * 0.4).round()
            } else {
                self.padding_x.max(0) as f64
            },
            padding_y: if self.padding_y == AUTO {
                (font_size * 0.2).round()
            } else {
                self.padding_y.max(0) as f64
            },
            font_size,
            font_family: self.font_family.clone(),
            bold: true,
        }
    }
}

/// `[ui_hint.boundary_highlight]`: optional outlines around hinted elements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoundaryHighlight {
    pub enabled: bool,
    pub border_width: i32,
    pub border_radius: i32,
    pub background_color: Option<ThemedColor>,
    pub border_color: Option<ThemedColor>,
}

impl Default for BoundaryHighlight {
    fn default() -> Self {
        Self {
            enabled: false,
            border_width: one(),
            border_radius: auto(),
            background_color: None,
            border_color: None,
        }
    }
}

impl BoundaryHighlight {
    pub fn fill(&self, palette: &Palette) -> Color {
        resolve(
            self.background_color.as_ref(),
            palette.appearance,
            Color::TRANSPARENT,
        )
    }

    pub fn stroke(&self, palette: &Palette) -> Color {
        resolve(
            self.border_color.as_ref(),
            palette.appearance,
            palette.accent_border(),
        )
    }

    pub fn radius(&self) -> f64 {
        if self.border_radius == AUTO {
            2.0
        } else {
            self.border_radius.max(0) as f64
        }
    }
}

/// Anchor for a floating panel such as the hint search box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    Center,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

impl Anchor {
    /// Place a `width` x `height` panel inside `area` with the given offsets.
    pub fn place(
        &self,
        area: crate::api::geometry::Rect,
        width: f64,
        height: f64,
        x_offset: f64,
        y_offset: f64,
    ) -> crate::api::geometry::Rect {
        use crate::api::geometry::Rect;
        let (x, y) = match self {
            Anchor::TopLeft => (area.left(), area.top() + y_offset),
            Anchor::TopCenter => (area.center().x - width / 2.0, area.top() + y_offset),
            Anchor::TopRight => (area.right() - width, area.top() + y_offset),
            Anchor::Center => (
                area.center().x - width / 2.0,
                area.center().y - height / 2.0,
            ),
            Anchor::BottomLeft => (area.left(), area.bottom() - height - y_offset),
            Anchor::BottomCenter => (
                area.center().x - width / 2.0,
                area.bottom() - height - y_offset,
            ),
            Anchor::BottomRight => (area.right() - width, area.bottom() - height - y_offset),
        };
        Rect::new(x + x_offset, y, width, height)
    }
}

/// `[ui_hint.search_input_ui]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchInputUi {
    pub position: Anchor,
    pub x_offset: i32,
    pub y_offset: i32,
    pub width: i32,
    #[serde(flatten)]
    pub label: LabelUi,
}

impl Default for SearchInputUi {
    fn default() -> Self {
        Self {
            position: Anchor::BottomCenter,
            x_offset: 0,
            y_offset: 24,
            width: 320,
            label: LabelUi::default(),
        }
    }
}

/// `[mode_indicator.ui]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorUi {
    #[serde(flatten)]
    pub label: LabelUi,
    pub indicator_x_offset: i32,
    pub indicator_y_offset: i32,
}

impl Default for IndicatorUi {
    fn default() -> Self {
        Self {
            label: LabelUi::default(),
            indicator_x_offset: -12,
            indicator_y_offset: 18,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorIndicatorUi {
    pub enabled: bool,
    pub radius: i32,
    pub fill_color: Option<ThemedColor>,
    pub stroke_color: Option<ThemedColor>,
    pub left_pressed_color: Option<ThemedColor>,
    pub middle_pressed_color: Option<ThemedColor>,
    pub right_pressed_color: Option<ThemedColor>,
    pub stroke_width: i32,
}

impl Default for CursorIndicatorUi {
    fn default() -> Self {
        Self {
            enabled: true,
            radius: 13,
            fill_color: None,
            stroke_color: None,
            left_pressed_color: Some(ThemedColor::Both("#00FF00FF".into())),
            middle_pressed_color: Some(ThemedColor::Both("#FF00FFFF".into())),
            right_pressed_color: Some(ThemedColor::Both("#00FFFFFF".into())),
            stroke_width: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorIndicatorOverride {
    pub enabled: Option<bool>,
    pub radius: Option<i32>,
    pub fill_color: Option<ThemedColor>,
    pub stroke_color: Option<ThemedColor>,
    pub left_pressed_color: Option<ThemedColor>,
    pub middle_pressed_color: Option<ThemedColor>,
    pub right_pressed_color: Option<ThemedColor>,
    pub stroke_width: Option<i32>,
}

impl CursorIndicatorOverride {
    pub fn apply(&self, base: &CursorIndicatorUi) -> CursorIndicatorUi {
        CursorIndicatorUi {
            enabled: self.enabled.unwrap_or(base.enabled),
            radius: self.radius.unwrap_or(base.radius),
            fill_color: self.fill_color.clone().or_else(|| base.fill_color.clone()),
            stroke_color: self
                .stroke_color
                .clone()
                .or_else(|| base.stroke_color.clone()),
            left_pressed_color: self
                .left_pressed_color
                .clone()
                .or_else(|| base.left_pressed_color.clone()),
            middle_pressed_color: self
                .middle_pressed_color
                .clone()
                .or_else(|| base.middle_pressed_color.clone()),
            right_pressed_color: self
                .right_pressed_color
                .clone()
                .or_else(|| base.right_pressed_color.clone()),
            stroke_width: self.stroke_width.unwrap_or(base.stroke_width),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndicatorUiOverride {
    pub font_size: Option<i32>,
    pub font_family: Option<String>,
    pub border_radius: Option<i32>,
    pub padding_x: Option<i32>,
    pub padding_y: Option<i32>,
    pub border_width: Option<i32>,
    pub background_color: Option<ThemedColor>,
    pub text_color: Option<ThemedColor>,
    pub matched_text_color: Option<ThemedColor>,
    pub border_color: Option<ThemedColor>,
    pub indicator_x_offset: Option<i32>,
    pub indicator_y_offset: Option<i32>,
}

impl IndicatorUiOverride {
    pub fn apply(&self, base: &IndicatorUi) -> IndicatorUi {
        let mut resolved = base.clone();
        if let Some(value) = self.font_size {
            resolved.label.font_size = value;
        }
        if let Some(value) = &self.font_family {
            resolved.label.font_family.clone_from(value);
        }
        if let Some(value) = self.border_radius {
            resolved.label.border_radius = value;
        }
        if let Some(value) = self.padding_x {
            resolved.label.padding_x = value;
        }
        if let Some(value) = self.padding_y {
            resolved.label.padding_y = value;
        }
        if let Some(value) = self.border_width {
            resolved.label.border_width = value;
        }
        for (target, value) in [
            (&mut resolved.label.background_color, &self.background_color),
            (&mut resolved.label.text_color, &self.text_color),
            (
                &mut resolved.label.matched_text_color,
                &self.matched_text_color,
            ),
            (&mut resolved.label.border_color, &self.border_color),
        ] {
            if let Some(value) = value {
                *target = Some(value.clone());
            }
        }
        if let Some(value) = self.indicator_x_offset {
            resolved.indicator_x_offset = value;
        }
        if let Some(value) = self.indicator_y_offset {
            resolved.indicator_y_offset = value;
        }
        resolved
    }
}

/// Placement of hint labels; re-exported for config ergonomics.
pub type HintPlacement = Placement;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::Rect;

    #[test]
    fn auto_padding_scales_with_font_size() {
        let palette = Palette::default();
        let ui = LabelUi {
            font_size: 20,
            ..Default::default()
        };
        let style = ui.resolve(&palette, palette.surface, palette.text, palette.accent);
        assert_eq!(style.padding_x, 8.0);
        assert_eq!(style.padding_y, 4.0);
        assert_eq!(style.border_radius, 7.0);
    }

    #[test]
    fn explicit_values_override_auto() {
        let palette = Palette::default();
        let ui = LabelUi {
            padding_x: 3,
            border_radius: 0,
            ..Default::default()
        };
        let style = ui.resolve(&palette, palette.surface, palette.text, palette.accent);
        assert_eq!(style.padding_x, 3.0);
        assert_eq!(style.border_radius, 0.0);
    }

    #[test]
    fn configured_color_wins_over_derived_default() {
        let palette = Palette::default();
        let ui = LabelUi {
            background_color: Some(ThemedColor::Both("#FF0000FF".into())),
            ..Default::default()
        };
        let style = ui.resolve(&palette, palette.surface, palette.text, palette.accent);
        assert_eq!(style.background, Color::rgb(255, 0, 0));
    }

    #[test]
    fn anchor_places_panel_within_area() {
        let area = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let r = Anchor::BottomCenter.place(area, 320.0, 30.0, 0.0, 24.0);
        assert_eq!(r, Rect::new(340.0, 746.0, 320.0, 30.0));
    }
}
