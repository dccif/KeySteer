//! Central window identity cards, selection and BSP region composition.
use crate::api::overlay::{Color, OverlayLabel, SharedLabelStyle, TextAlignment};
use crate::api::presentation::WindowView;
use crate::api::window_layout::placed_rect;
use crate::api::{HostContext, OverlayScene, OverlayShape, Point, Rect};

use super::label_placement::{card_positions, logical};
use crate::api::overlay::LabelPlacementRole as Role;

fn app_name(window: &crate::api::window::WindowInfo) -> &str {
    window
        .app
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&window.app)
        .trim_end_matches(".exe")
}

impl WindowView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let mut next_group = 0;
        let mut scene = self.screen_scene(ctx, &mut next_group);
        scene.clip = ctx.screens.get(self.screen).map(|s| s.bounds);
        if self.tree.is_none() {
            for screen in 0..ctx.screens.len() {
                if screen != self.screen
                    && self
                        .visible
                        .iter()
                        .any(|id| self.inventory.get(id).is_some_and(|w| w.screen == screen))
                {
                    let other = Self {
                        screen,
                        target: None,
                        ..*self
                    }
                    .screen_scene(ctx, &mut next_group);
                    scene.clip = Some(scene.clip.map_or(ctx.screens[screen].bounds, |clip| {
                        clip.union(&ctx.screens[screen].bounds)
                    }));
                    scene.labels.extend(other.labels.iter().cloned());
                    scene.shapes.extend(other.shapes.iter().cloned());
                }
            }
        }
        scene
    }
    fn screen_scene(&self, ctx: &HostContext<'_>, next_group: &mut u32) -> OverlayScene {
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
            .filter(|w| w.screen == self.screen && self.numbers.contains_key(&w.id))
            .filter(|w| {
                if self.tree.is_some() {
                    self.tabs.representative(w.id) == w.id
                } else {
                    self.tabs
                        .containing(w.id)
                        .is_none_or(|group| group.active == w.id)
                }
            })
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
        let lines: Vec<Vec<String>> = windows
            .iter()
            .map(|window| {
                if let Some(group) = self.tabs.containing(window.id) {
                    std::iter::once(format!("~{} · {} windows", group.id.0, group.members.len()))
                        .chain(group.members.iter().map(|id| {
                            let number = self.numbers.get(id).copied().unwrap_or(0);
                            let selected = if *id == group.active { "●" } else { "○" };
                            self.inventory.get(id).map_or_else(
                                || format!("{selected} {number} · Window"),
                                |member| {
                                    format!(
                                        "{selected} {number} · {} — {}",
                                        app_name(member),
                                        member.title
                                    )
                                },
                            )
                        }))
                        .collect()
                } else {
                    vec![app_name(window).to_string(), window.title.clone()]
                }
            })
            .collect();
        let row_height = small.font_size * 1.4;
        let max_rows = ((screen.work_area.height / scale - 14.0) / row_height)
            .floor()
            .max(2.0) as usize;
        let columns = lines
            .iter()
            .map(|lines| lines.len().div_ceil(max_rows))
            .max()
            .unwrap_or(1);
        let group_height = lines
            .iter()
            .map(|lines| {
                (lines.len().div_ceil(lines.len().div_ceil(max_rows)) as f64 * row_height + 8.0)
                    .max(height)
            })
            .fold(height, f64::max);
        let (physical_width, positions) = card_positions(
            &centers,
            (number_width + 18.0 + 260.0 * columns as f64) * scale,
            group_height * scale,
            screen.work_area.inset(3.0 * scale, 3.0 * scale),
        );
        let width = physical_width / scale;
        for ((window, footprint), lines) in windows.iter().zip(&positions).zip(&lines) {
            *next_group += 1;
            let group = *next_group;
            let text = self.numbers[&window.id].to_string();
            let columns = lines.len().div_ceil(max_rows);
            let rows = lines.len().div_ceil(columns);
            let card = Rect::new(
                footprint.x,
                footprint.y,
                width * scale,
                (rows as f64 * row_height + 8.0).max(height) * scale,
            );

            if self.tree.is_none()
                && ((card.center().x - window.bounds.center().x).abs() > 5.0
                    || (card.center().y - window.bounds.center().y).abs() > height * scale)
            {
                scene.push_shape(OverlayShape::label_connector(
                    window.bounds.center(),
                    card.center(),
                    style.border_color,
                    3.0 * scale,
                    group,
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
                .with_z_index(19)
                .with_placement(group, Role::Background),
            );
            let number_rect = Rect::new(card.x, card.y, number_width * scale, card.height);
            scene.push_label(
                OverlayLabel::new(text, logical(number_rect, scale), style.clone())
                    .with_z_index(20)
                    .with_placement(group, Role::Fixed),
            );
            let content_width = ((width - number_width - 18.0) / columns as f64).max(1.0);
            for (line, text) in lines.iter().enumerate() {
                let label_style = if line == 0 { &small } else { &title_style };
                let rect = Rect::new(
                    card.x + (number_width + 9.0 + (line / rows) as f64 * content_width) * scale,
                    card.y + (4.0 + (line % rows) as f64 * row_height) * scale,
                    content_width * scale,
                    row_height * scale,
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
                    label.placement = Some(crate::api::overlay::LabelPlacement {
                        group,
                        role: Role::Flexible,
                    });
                }
            }
        }
        for slot in slots {
            *next_group += 1;
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
                OverlayLabel::new(text, logical(rect, scale), style.clone())
                    .with_z_index(22)
                    .with_placement(*next_group, Role::Standalone),
            );
        }
        if self.group_input {
            for group in &self.tabs.groups {
                let Some(window) = self
                    .inventory
                    .get(&group.active)
                    .filter(|w| w.screen == self.screen)
                else {
                    continue;
                };
                *next_group += 1;
                let width = (style.font_size * 0.75 * (group.id.0.to_string().len() + 1) as f64
                    + style.padding_x * 2.0)
                    .max(38.0)
                    * scale;
                let rect = Rect::new(
                    window.bounds.center().x - width / 2.0,
                    window.bounds.y + 5.0 * scale,
                    width,
                    (style.font_size * 1.4 + style.padding_y * 2.0).max(44.0) * scale,
                );
                scene.push_label(
                    OverlayLabel::new(
                        format!("~{}", group.id.0),
                        logical(rect, scale),
                        style.clone(),
                    )
                    .with_z_index(25)
                    .with_placement(*next_group, Role::Standalone),
                );
            }
        }
        super::label_placement::avoid_overlaps(&mut scene, screen, &[]);
        scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dense_cards_repack_above_help_without_detaching_numbers_or_titles() {
        let scale = 2.0;
        let screen = crate::api::Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 30.0, 1920.0, 1010.0),
            name: None,
            scale,
            is_primary: true,
        };
        let panel = Rect::new(380.0, 650.0, 1160.0, 390.0);
        let (_, cards) = card_positions(
            &[Point::new(900.0, 850.0); 24],
            600.0,
            120.0,
            screen.work_area,
        );
        let mut scene = OverlayScene::new();
        for (number, card) in cards.into_iter().enumerate() {
            let group = number as u32 + 1;
            scene.push_label(
                OverlayLabel::new("", card, crate::api::overlay::LabelStyle::default())
                    .with_z_index(19)
                    .with_placement(group, Role::Background),
            );
            scene.push_label(
                OverlayLabel::new(
                    number.to_string(),
                    logical(Rect::new(card.x, card.y, 120.0, card.height), scale),
                    crate::api::overlay::LabelStyle::default(),
                )
                .with_z_index(20)
                .with_placement(group, Role::Fixed),
            );
            scene.push_label(
                OverlayLabel::new(
                    "Application title",
                    logical(
                        Rect::new(card.x + 138.0, card.y + 30.0, card.width - 156.0, 40.0),
                        scale,
                    ),
                    crate::api::overlay::LabelStyle::default(),
                )
                .with_z_index(21)
                .with_placement(group, Role::Flexible),
            );
        }
        scene.sort_in_place();
        super::super::label_placement::avoid_overlaps(&mut scene, &screen, &[panel]);
        let cards: Vec<_> = scene
            .labels
            .iter()
            .filter(|l| l.z_index == 19)
            .map(|l| l.rect)
            .collect();
        assert_eq!(cards.len(), 24);
        assert!(cards.iter().any(|card| card.width < 600.0));
        for (index, card) in cards.iter().enumerate() {
            assert!(card.intersect(&panel).is_none());
            assert!(
                cards[..index]
                    .iter()
                    .all(|old| old.intersect(card).is_none())
            );
            let number = scene
                .labels
                .iter()
                .find(|l| l.text == index.to_string().as_str())
                .unwrap();
            assert!(card.contains(&number.rect.center()));
        }
        for title in scene.labels.iter().filter(|l| l.z_index == 21) {
            assert_eq!(
                cards
                    .iter()
                    .filter(|card| card.contains(&title.rect.center()))
                    .count(),
                1
            );
            assert!(!panel.contains(&title.rect.center()));
        }
    }
    #[test]
    fn overlapping_cards_avoid_final_help_panel_and_keep_text_together() {
        for scale in [1.0, 1.5, 2.0] {
            let screen = crate::api::Screen {
                bounds: Rect::new(-1920.0, 0.0, 1920.0, 1080.0),
                work_area: Rect::new(-1920.0, 30.0, 1920.0, 1010.0),
                name: None,
                scale,
                is_primary: true,
            };
            let panel = Rect::new(-1450.0, 650.0, 1000.0, 390.0);
            let mut scene = OverlayScene::new();
            let (_, positions) = card_positions(
                &[Point::new(-850.0, 700.0); 2],
                300.0 * scale,
                70.0 * scale,
                screen.work_area,
            );
            for (n, card) in positions.into_iter().enumerate() {
                let group = n as u32 + 1;
                scene.push_label(
                    OverlayLabel::new("", card, crate::api::overlay::LabelStyle::default())
                        .with_z_index(19)
                        .with_placement(group, Role::Background),
                );
                scene.push_label(
                    OverlayLabel::new(
                        n.to_string(),
                        logical(card, scale),
                        crate::api::overlay::LabelStyle::default(),
                    )
                    .with_z_index(20)
                    .with_placement(group, Role::Fixed),
                );
            }
            scene.sort_in_place();
            super::super::label_placement::avoid_overlaps(&mut scene, &screen, &[panel]);
            let cards: Vec<_> = scene
                .labels
                .iter()
                .filter(|l| l.z_index == 19)
                .map(|l| l.rect)
                .collect();
            assert!(cards[0].intersect(&cards[1]).is_none());
            for (index, card) in cards.iter().enumerate() {
                assert!(card.intersect(&panel).is_none());
                assert!(screen.work_area.contains(&card.center()));
                assert_eq!(
                    card.center(),
                    scene
                        .labels
                        .iter()
                        .find(|label| label.text == index.to_string().as_str())
                        .unwrap()
                        .rect
                        .center()
                );
            }
        }
    }
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
