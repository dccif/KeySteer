//! Central window identity cards, selection and BSP region composition.
use crate::api::overlay::{Color, OverlayLabel, SharedLabelStyle, TextAlignment};
use crate::api::presentation::WindowView;
use crate::api::window_layout::placed_rect;
use crate::api::{HostContext, OverlayScene, OverlayShape, Point, Rect};

// Test physical footprints before accepting a candidate. Clamping afterwards
// would collapse displaced cards back onto the same bottom-edge position.
fn card_position(center: Point, width: f64, height: f64, area: Rect, occupied: &[Rect]) -> Rect {
    let candidate = |x: f64, y: f64| {
        Rect::new(
            x.clamp(area.x, (area.right() - width).max(area.x)),
            y.clamp(area.y, (area.bottom() - height).max(area.y)),
            width,
            height,
        )
    };
    let initial = candidate(center.x - width / 2.0, center.y - height / 2.0);
    let free = |rect: &Rect| !occupied.iter().any(|old| old.intersect(rect).is_some());
    if free(&initial) {
        return initial;
    }
    let columns = (area.width / (width + 6.0)).floor().max(1.0) as usize;
    let rows = (area.height / (height + 6.0)).floor().max(1.0) as usize;
    let mut best = None;
    let mut distance = f64::INFINITY;
    for row in 0..rows {
        for col in 0..columns {
            let rect = candidate(
                area.x + col as f64 * (width + 6.0),
                area.y + row as f64 * (height + 6.0),
            );
            let d = (rect.center().x - center.x).powi(2) + (rect.center().y - center.y).powi(2);
            if free(&rect) && d < distance {
                best = Some(rect);
                distance = d;
            }
        }
    }
    best.unwrap_or(initial)
}
fn card_positions(
    centers: &[Point],
    desired_width: f64,
    height: f64,
    area: Rect,
) -> (f64, Vec<Rect>) {
    let rows = (area.height / (height + 6.0)).floor().max(1.0) as usize;
    let columns = centers.len().div_ceil(rows).max(1);
    let width =
        desired_width.min(((area.width - (columns - 1) as f64 * 6.0) / columns as f64).max(1.0));
    let mut positions = Vec::with_capacity(centers.len());
    for center in centers {
        let rect = card_position(*center, width, height, area, &positions);
        if positions
            .iter()
            .any(|old: &Rect| old.intersect(&rect).is_some())
        {
            // Arbitrary center anchors can fragment otherwise sufficient space.
            // Repack the whole set deterministically instead of overlapping.
            positions.clear();
            for index in 0..centers.len() {
                positions.push(Rect::new(
                    area.x + (index / rows) as f64 * (width + 6.0),
                    area.y + (index % rows) as f64 * (height + 6.0),
                    width,
                    height,
                ));
            }
            return (width, positions);
        }
        positions.push(rect);
    }
    (width, positions)
}

fn logical(rect: Rect, scale: f64) -> Rect {
    Rect::new(
        rect.center().x - rect.width / scale / 2.0,
        rect.center().y - rect.height / scale / 2.0,
        rect.width / scale,
        rect.height / scale,
    )
}
impl WindowView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let mut scene = OverlayScene::new();
        let style: SharedLabelStyle = self
            .ui
            .resolve(
                ctx.palette,
                ctx.palette.surface_label(),
                ctx.palette.text,
                ctx.palette.accent,
            )
            .into();
        let Some(screen) = ctx.screens.get(self.screen) else {
            return scene;
        };
        let scale = crate::presentation::label_scale(screen.scale);
        let slots = self.tree.map(|tree| tree.slots()).unwrap_or_default();
        if self.tree.is_some() {
            scene.push_shape(OverlayShape::fill(
                screen.work_area,
                Color::rgba(10, 16, 30, 150),
            ));
        }
        for slot in &slots {
            let rect = placed_rect(screen.work_area, slot.rect, self.gap);
            let selected = self.tree.is_some_and(|tree| slot.id == tree.selected);
            scene.push_shape(OverlayShape::fill(
                rect,
                if selected {
                    ctx.palette.accent.with_opacity(0.3)
                } else {
                    Color::rgba(228, 235, 255, 20)
                },
            ));
            scene.push_shape(OverlayShape::outline(
                rect,
                Color::rgba(12, 16, 24, 240),
                if selected { 5.0 } else { 3.0 } * scale,
            ));
            scene.push_shape(OverlayShape::outline(
                rect,
                if selected {
                    ctx.palette.accent
                } else {
                    Color::rgb(220, 230, 255)
                },
                if selected { 3.0 } else { 1.5 } * scale,
            ));
        }
        if let Some(target) = &self.target {
            scene.push_shape(OverlayShape::outline(
                target.bounds,
                style.border_color,
                self.border_width,
            ));
        }
        let mut text_style = (*style).clone();
        text_style.font_size = (style.font_size * 0.6).max(14.0);
        text_style.text_alignment = TextAlignment::Left;
        text_style.background = Color::TRANSPARENT;
        text_style.border_color = Color::TRANSPARENT;
        text_style.border_width = 0.0;
        text_style.padding_x = 0.0;
        text_style.padding_y = 0.0;
        let small: SharedLabelStyle = text_style.clone().into();
        text_style.bold = false;
        text_style.font_size = (style.font_size * 0.45).max(12.0);
        let title_style: SharedLabelStyle = text_style.into();
        let windows: Vec<_> = self
            .visible
            .iter()
            .filter_map(|id| self.inventory.get(id))
            .filter(|w| self.numbers.contains_key(&w.id))
            .collect();
        let centers: Vec<_> = windows
            .iter()
            .map(|w| {
                slots
                    .iter()
                    .find(|s| s.window == Some(w.id))
                    .map_or(w.bounds.center(), |slot| {
                        let area = placed_rect(screen.work_area, slot.rect, self.gap);
                        Point::new(area.center().x, area.y + 40.0 * scale)
                    })
            })
            .collect();
        let digits = self
            .numbers
            .values()
            .map(|n| n.to_string().len())
            .chain(slots.iter().map(|s| s.id.to_string().len() + 1))
            .max()
            .unwrap_or(1);
        let number_width =
            (style.font_size * 0.75 * digits as f64 + style.padding_x * 2.0).max(38.0);
        let height = (style.font_size * 1.4 + style.padding_y * 2.0)
            .max(small.font_size * 1.4 * 2.0 + 8.0)
            .max(44.0);
        let group_height = height;
        let (physical_width, positions) = card_positions(
            &centers,
            (number_width + 260.0) * scale,
            group_height * scale,
            screen.work_area.inset(3.0 * scale, 3.0 * scale),
        );
        let width = physical_width / scale;
        for (window, footprint) in windows.iter().zip(&positions) {
            let text = self.numbers[&window.id].to_string();
            let card = Rect::new(footprint.x, footprint.y, width * scale, height * scale);

            if self.tree.is_none()
                && ((card.center().x - window.bounds.center().x).abs() > 5.0
                    || (card.center().y - window.bounds.center().y).abs() > height * scale)
            {
                scene.push_shape(OverlayShape::line(
                    window.bounds.center(),
                    card.center(),
                    style.border_color,
                    3.0 * scale,
                ));
            }
            scene.push_label(
                OverlayLabel::new(
                    "",
                    card,
                    crate::api::overlay::LabelStyle {
                        font_size: 1.0,
                        ..(*style).clone()
                    },
                )
                .with_z_index(19),
            );
            let number_rect = Rect::new(card.x, card.y, number_width * scale, card.height);
            scene.push_label(
                OverlayLabel::new(text, logical(number_rect, scale), style.clone())
                    .with_z_index(20),
            );
            let app = window
                .app
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&window.app)
                .trim_end_matches(".exe");
            let content_width = (width - number_width - 18.0).max(1.0);
            for (line, text, label_style) in
                [(0, app, &small), (1, window.title.as_str(), &title_style)]
            {
                let rect = Rect::new(
                    card.x + (number_width + 9.0) * scale,
                    card.y + 4.0 * scale + line as f64 * (card.height - 8.0 * scale) / 2.0,
                    content_width * scale,
                    (card.height - 8.0 * scale) / 2.0,
                );
                crate::presentation::key_help::push_sized_help_text(
                    &mut scene,
                    crate::presentation::elide_width(text, content_width / label_style.font_size),
                    rect,
                    label_style,
                    scale,
                );
                if let Some(label) = scene.labels.last_mut() {
                    label.z_index = 21;
                }
            }
        }
        for slot in slots {
            let text = format!("`{}", slot.id);
            let width = (style.font_size * 0.75 * text.len() as f64 + style.padding_x * 2.0)
                .max(38.0)
                * scale;
            let height = (style.font_size * 1.4 + style.padding_y * 2.0).max(44.0) * scale;
            let area = placed_rect(screen.work_area, slot.rect, self.gap);
            let rect = Rect::new(
                area.center().x - width.min(area.width) / 2.0,
                area.center().y - height.min(area.height) / 2.0,
                width.min(area.width),
                height.min(area.height),
            );
            scene.push_label(
                OverlayLabel::new(text, logical(rect, scale), style.clone()).with_z_index(22),
            );
        }
        scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crowded_cards_are_clamped_before_collision_tests_at_multiple_scales() {
        for scale in [1.0, 1.5, 2.0] {
            let area = Rect::new(-1920.0, 40.0, 1920.0, 1040.0);
            let centers = vec![Point::new(area.right() - 3.0, area.bottom() - 3.0); 30];
            let (_, positions) = card_positions(&centers, 220.0 * scale, 94.0 * scale, area);
            for (i, rect) in positions.iter().enumerate() {
                assert!(rect.x >= area.x && rect.y >= area.y);
                assert!(
                    rect.right() <= area.right() + 1e-6 && rect.bottom() <= area.bottom() + 1e-6
                );
                assert!(
                    positions[..i]
                        .iter()
                        .all(|other| other.intersect(rect).is_none())
                );
            }
        }
    }
}
