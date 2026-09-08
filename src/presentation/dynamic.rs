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
    let style = ui.label.resolve(
        palette,
        background,
        palette.readable_on(background),
        palette.accent,
    );
    let text_width = |character_count: usize| {
        (character_count as f64 * style.font_size * 0.75 + style.padding_x * 2.0)
            .max(style.font_size * 2.0)
            .ceil()
    };
    let width = held_text
        .as_ref()
        .map(|held| text_width(held.character_count))
        .unwrap_or_default()
        .max(text_width(text.chars().count()));
    let line_height = (style.font_size * 1.4 + style.padding_y * 2.0).ceil();
    let height = line_height + held_text.as_ref().map_or(0.0, |_| line_height + 4.0);
    // `position.x` is the shared right edge of both badges. Keeping the
    // anchor independent of the longest line prevents a wide held-input
    // badge from pushing the shorter mode badge away from the cursor.
    let geometry = IndicatorGeometry {
        width,
        height,
        x_offset: ui.indicator_x_offset as f64,
        y_offset: ui.indicator_y_offset as f64,
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
