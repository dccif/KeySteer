//! Proportional ticks with staggered labels; no native or configuration access.
use crate::api::overlay::{Color, LabelStyle, OverlayLabel};
use crate::api::window_layout::QuickRuler;
use crate::api::{OverlayScene, Rect};

pub(super) struct Plan {
    pub height: f64,
    frame: Rect,
    selection: Rect,
    labels: Vec<(String, Rect)>,
    ticks: Vec<(Rect, bool)>,
}

pub(super) fn plan(
    ruler: &QuickRuler,
    width: f64,
    available_height: f64,
    aspect: f64,
    font: f64,
    scale: f64,
) -> Plan {
    let font = font * 0.9;
    let label = |value: f64| {
        ruler
            .ticks
            .iter()
            .find(|t| (t.value - value).abs() < 1e-9)
            .map_or("1", |t| t.label.as_str())
            .to_owned()
    };
    let horizontal = label(ruler.selected.width);
    let vertical = label(ruler.selected.height);
    let text_width = |text: &str| (super::super::text_units(text) * font + 6.0) * scale;
    let side = text_width(&vertical) + 10.0 * scale;
    let bottom = (font * 1.7 + 10.0) * scale;
    let max_width = (width - side - 8.0 * scale).max(1.0).min(250.0 * scale);
    let max_height = (available_height - bottom - 8.0 * scale).max(36.0 * scale);
    let frame_width = max_width.min(max_height * aspect);
    let frame = Rect::new(4.0 * scale, 4.0 * scale, frame_width, frame_width / aspect);
    let selection = Rect::new(
        frame.x + frame.width * ruler.selected.x,
        frame.y + frame.height * ruler.selected.y,
        frame.width * ruler.selected.width,
        frame.height * ruler.selected.height,
    );
    let labels = vec![
        (
            horizontal.clone(),
            Rect::new(
                (frame.center().x - text_width(&horizontal) / 2.0).max(0.0),
                frame.bottom() + 8.0 * scale,
                text_width(&horizontal),
                font * 1.5 * scale,
            ),
        ),
        (
            vertical.clone(),
            Rect::new(
                frame.right() + 8.0 * scale,
                frame.center().y - font * 0.75 * scale,
                text_width(&vertical),
                font * 1.5 * scale,
            ),
        ),
    ];
    let mut ticks = Vec::new();
    for tick in ruler.ticks.iter() {
        let x = if ruler.selected.x > 0.0 {
            1.0 - tick.value
        } else {
            tick.value
        };
        let y = if ruler.selected.y > 0.0 {
            1.0 - tick.value
        } else {
            tick.value
        };
        let active_x = (tick.value - ruler.selected.width).abs() < 1e-9;
        let active_y = (tick.value - ruler.selected.height).abs() < 1e-9;
        ticks.push((
            Rect::new(
                frame.x + frame.width * x,
                frame.bottom(),
                scale,
                if active_x { 6.0 } else { 3.0 } * scale,
            ),
            active_x,
        ));
        ticks.push((
            Rect::new(
                frame.right(),
                frame.y + frame.height * y,
                if active_y { 6.0 } else { 3.0 } * scale,
                scale,
            ),
            active_y,
        ));
    }
    Plan {
        height: frame.bottom() + bottom,
        frame,
        selection,
        labels,
        ticks,
    }
}

pub(super) fn draw(
    scene: &mut OverlayScene,
    plan: Plan,
    x: f64,
    y: f64,
    base: &LabelStyle,
    foreground: Color,
    scale: f64,
) {
    for (mut rect, selected) in [(plan.frame, false), (plan.selection, true)] {
        rect.x += x;
        rect.y += y;
        scene.labels.push(
            OverlayLabel::new(
                "",
                rect,
                LabelStyle {
                    font_size: 1.0,
                    background: foreground.with_opacity(if selected { 0.18 } else { 0.03 }),
                    border_width: if selected { 1.5 } else { 1.0 },
                    border_color: foreground.with_opacity(if selected { 0.9 } else { 0.4 }),
                    ..base.clone()
                },
            )
            .with_z_index(i32::MAX),
        );
    }
    for (mut rect, active) in plan.ticks {
        rect.x += x;
        rect.y += y;
        scene.labels.push(
            OverlayLabel::new(
                "",
                rect,
                LabelStyle {
                    font_size: 1.0,
                    background: foreground.with_opacity(if active { 0.9 } else { 0.35 }),
                    ..base.clone()
                },
            )
            .with_z_index(i32::MAX),
        );
    }
    for (text, mut rect) in plan.labels {
        rect.x += x;
        rect.y += y;
        super::push_sized_help_text(
            scene,
            text,
            rect,
            &LabelStyle {
                font_size: base.font_size * 0.9,
                bold: true,
                ..base.clone()
            },
            scale,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screen_preview_preserves_aspect_selection_and_configured_labels() {
        let config: crate::config::WindowQuick =
            toml::from_str("split_ratios = ['1/2', '1/3', 0.3, '0.45']").unwrap();
        let ruler = QuickRuler {
            ticks: config.split_ratio_ticks().into(),
            selected: Rect::new(0.55, 0.0, 0.45, 0.3),
        };
        for scale in [1.0, 1.5, 2.0] {
            for aspect in [16.0 / 9.0, 9.0 / 16.0, 1.0] {
                let plan = plan(&ruler, 280.0 * scale, 140.0 * scale, aspect, 12.0, scale);
                assert!((plan.frame.width / plan.frame.height - aspect).abs() < 1e-9);
                assert!((plan.selection.width / plan.frame.width - 0.45).abs() < 1e-9);
                assert!((plan.selection.right() - plan.frame.right()).abs() < 1e-9);
                assert_eq!(
                    plan.labels
                        .iter()
                        .map(|(text, _)| text.as_str())
                        .collect::<Vec<_>>(),
                    ["0.45", "0.3"]
                );
                for (_, rect) in &plan.labels {
                    assert!(
                        rect.x >= 0.0
                            && rect.right() <= 280.0 * scale
                            && rect.bottom() <= plan.height
                    );
                }
            }
        }
    }
}
