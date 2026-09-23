//! Stateless geometry and styling for host-owned cursor decorations.
use crate::api::binding::Button;
use crate::api::overlay::{Color, CursorMarker, Indicator};
use crate::api::style::{IndicatorUi, ResolvedCursorIndicatorUi};
use crate::api::{Palette, Point, Screen};

pub(crate) struct HeldTargetsText {
    pub(crate) value: String,
    pub(crate) character_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IndicatorGeometry {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) x_offset: f64,
    pub(crate) y_offset: f64,
}

impl IndicatorGeometry {
    pub(crate) fn position(self, cursor: Point, screens: &[Screen]) -> Point {
        let mut position = Point::new(cursor.x + self.x_offset, cursor.y + self.y_offset);
        if let Some(screen) = Screen::containing(screens, &cursor) {
            position.x = position.x.clamp(
                (screen.bounds.x + self.width).min(screen.bounds.right()),
                screen.bounds.right(),
            );
            position.y = position.y.clamp(
                screen.bounds.y,
                (screen.bounds.bottom() - self.height).max(screen.bounds.y),
            );
        }
        position
    }
}

pub(crate) fn cursor_marker(
    cursor: ResolvedCursorIndicatorUi<'_>,
    pressed_button: Option<Button>,
    palette: &Palette,
    position: Point,
) -> CursorMarker {
    let pressed_color = match pressed_button {
        Some(crate::api::binding::Button::Left) => cursor.left_pressed_color,
        Some(crate::api::binding::Button::Middle) => cursor.middle_pressed_color,
        Some(crate::api::binding::Button::Right) => cursor.right_pressed_color,
        Some(crate::api::binding::Button::X1 | crate::api::binding::Button::X2) | None => None,
    }
    .and_then(|color| color.resolve(palette.appearance));
    let fill = pressed_color.map_or_else(
        || {
            crate::api::style::resolve(
                cursor.fill_color,
                palette.appearance,
                palette.accent.with_alpha(34),
            )
        },
        |color| color.with_opacity(0.2),
    );
    let stroke = pressed_color.unwrap_or_else(|| {
        crate::api::style::resolve(
            cursor.stroke_color,
            palette.appearance,
            palette.accent_alt.with_alpha(210),
        )
    });
    CursorMarker {
        center: position,
        radius: cursor.radius.max(1) as f64,
        fill,
        stroke,
        stroke_width: cursor.stroke_width.max(0) as f64,
    }
}

pub(crate) fn indicator(
    text: String,
    ui: &IndicatorUi,
    background: Color,
    held_text: Option<HeldTargetsText>,
    palette: &Palette,
    cursor: Point,
    screens: &[Screen],
) -> (Indicator, IndicatorGeometry) {
    let mut style = ui.label.resolve(
        palette,
        background,
        palette.readable_on(background),
        palette.accent,
    );
    // Windows scenes use physical pixels; macOS scenes use logical points.
    // Resolve DPI before measuring so placement and native drawing agree.
    let scale =
        super::label_scale(Screen::containing(screens, &cursor).map_or(1.0, |screen| screen.scale));
    if scale != 1.0 {
        style.font_size = (style.font_size * scale).round().max(1.0);
        style.padding_x = (style.padding_x * scale).round();
        style.padding_y = (style.padding_y * scale).round();
        style.border_width = if style.border_width > 0.0 {
            (style.border_width * scale).round().max(1.0)
        } else {
            0.0
        };
        style.border_radius = (style.border_radius * scale).round();
    }
    let text_width = |character_count: usize| Indicator::label_size(character_count, &style).0;
    let width = held_text
        .as_ref()
        .map(|held| text_width(held.character_count))
        .unwrap_or_default()
        .max(text_width(text.chars().count()));
    let line_height = Indicator::label_size(0, &style).1;
    let height = line_height + held_text.as_ref().map_or(0.0, |_| line_height + 4.0);
    // `position.x` is the shared right edge of both badges. Keeping the
    // anchor independent of the longest line prevents a wide held-input
    // badge from pushing the shorter mode badge away from the cursor.
    let geometry = IndicatorGeometry {
        width,
        height,
        x_offset: f64::from(ui.indicator_offset[0]),
        y_offset: f64::from(ui.indicator_offset[1]),
    };
    (
        Indicator {
            text,
            held_text: held_text.map(|held| held.value),
            position: geometry.position(cursor, screens),
            style,
        },
        geometry,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Rect;

    #[test]
    fn indicator_preserves_original_compact_badge_dimensions() {
        let style = crate::api::overlay::LabelStyle {
            font_size: 17.0,
            padding_x: 6.0,
            padding_y: 3.0,
            ..Default::default()
        };
        let (width, height) = Indicator::label_size(6, &style);
        assert!((width - 83.4).abs() < 1.0e-9);
        assert_eq!(height, 23.0);
    }

    #[test]
    fn indicator_dpi_is_applied_before_measuring_and_clamping() {
        let palette = Palette::default();
        let ui = IndicatorUi::default();
        let (base, _) = indicator(
            "Normal".into(),
            &ui,
            palette.surface,
            None,
            &palette,
            Point::new(400.0, 300.0),
            &[],
        );
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let bounds = Rect::new(0.0, 0.0, 1000.0, 800.0);
            let screens = [Screen {
                bounds,
                work_area: bounds,
                is_primary: true,
                scale: dpi,
                name: None,
            }];
            let (badge, geometry) = indicator(
                "Normal".into(),
                &ui,
                palette.surface,
                None,
                &palette,
                Point::new(1.0, 799.0),
                &screens,
            );
            let scale = if cfg!(target_os = "windows") {
                dpi
            } else {
                1.0
            };
            assert_eq!(
                badge.style.font_size,
                (base.style.font_size * scale).round()
            );
            assert_eq!(
                Indicator::label_size(6, &badge.style),
                (geometry.width, geometry.height)
            );
            assert!(badge.position.x - geometry.width >= 0.0);
            assert!(badge.position.y + geometry.height <= 800.0);
            assert_eq!(geometry.x_offset, f64::from(ui.indicator_offset[0]));
        }
    }

    #[test]
    fn offset_pair_uses_right_top_anchor_and_cached_metrics() {
        let palette = Palette::default();
        for pair in [[-12, 18], [40, 30], [-32768, 32767]] {
            let ui = IndicatorUi {
                indicator_offset: pair,
                ..Default::default()
            };
            let (badge, geometry) = indicator(
                "Normal".into(),
                &ui,
                palette.surface,
                None,
                &palette,
                Point::new(400.0, 300.0),
                &[],
            );
            assert_eq!(
                badge.position,
                Point::new(400.0 + f64::from(pair[0]), 300.0 + f64::from(pair[1]))
            );
            assert_eq!(
                Indicator::label_size(6, &badge.style),
                (geometry.width, geometry.height)
            );
            for x in 0..1000 {
                assert_eq!(
                    geometry.position(Point::new(x as f64, 300.0), &[]).x,
                    x as f64 + f64::from(pair[0])
                );
            }
        }
    }

    #[test]
    fn indicator_placement_preserves_anchor_and_clamps_both_lines() {
        let palette = Palette::default();
        let cursor = Point::new(400.0, 300.0);
        for pair in [[-12, 18], [40, 30], [-80, -60], [0, 0]] {
            for held in [false, true] {
                let ui = IndicatorUi {
                    indicator_offset: pair,
                    ..Default::default()
                };
                let (badge, geometry) = indicator(
                    "Normal".into(),
                    &ui,
                    palette.surface,
                    held.then(|| HeldTargetsText {
                        value: "Ctrl + Left".into(),
                        character_count: 11,
                    }),
                    &palette,
                    cursor,
                    &[],
                );
                assert_eq!(badge.position.x, cursor.x + f64::from(pair[0]));
                assert_eq!(badge.position.y, cursor.y + f64::from(pair[1]));
                // Pointer movement uses the cached geometry, without rebuilding text.
                assert_eq!(
                    geometry.position(Point::new(410.0, 320.0), &[]),
                    Point::new(badge.position.x + 10.0, badge.position.y + 20.0)
                );
                let bounds = Rect::new(-1000.0, -800.0, 1000.0, 800.0);
                let screens = [Screen {
                    bounds,
                    work_area: bounds,
                    is_primary: true,
                    scale: 1.0,
                    name: None,
                }];
                for corner in [Point::new(-999.0, -799.0), Point::new(-1.0, -1.0)] {
                    let p = geometry.position(corner, &screens);
                    assert!(p.x - geometry.width >= bounds.x && p.x <= bounds.right());
                    assert!(p.y >= bounds.y && p.y + geometry.height <= bounds.bottom());
                }
            }
        }
    }
}
