//! Central window identity cards, selection and BSP region composition.
use crate::api::overlay::{Color, OverlayLabel};
use crate::api::presentation::WindowView;
use crate::api::window_layout::placed_rect;
use crate::api::{HostContext, OverlayScene, OverlayShape, Rect};

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

pub(crate) const RENDERERS: crate::api::style::WindowSceneRenderers =
    crate::api::style::WindowSceneRenderers {
        plain: |view, ctx| view.scene_using(ctx, NoGuides),
        with_guides: |view, ctx| {
            let Some(style) = view
                .styles
                .for_appearance(ctx.palette.appearance)
                .guide_line
            else {
                return view.scene_using(ctx, NoGuides);
            };
            view.scene_using(ctx, Guides(style))
        },
    };

#[derive(Clone, Copy)]
struct GuideGeometry {
    source: Rect,
    card: Rect,
    height: f64,
    scale: f64,
    tree: bool,
}
trait CardGuides: Copy {
    fn initialize(self, scene: &mut OverlayScene);
    fn attach(self, scene: &mut OverlayScene, group: u32, geometry: GuideGeometry);
}
#[derive(Clone, Copy)]
struct NoGuides;
impl CardGuides for NoGuides {
    #[inline(always)]
    fn initialize(self, _: &mut OverlayScene) {}
    #[inline(always)]
    fn attach(self, _: &mut OverlayScene, _: u32, _: GuideGeometry) {}
}
#[derive(Clone, Copy)]
struct Guides(crate::api::overlay::LabelConnectorStyle);
impl CardGuides for Guides {
    fn initialize(self, scene: &mut OverlayScene) {
        scene.set_connector_style(self.0);
    }
    fn attach(self, scene: &mut OverlayScene, group: u32, geometry: GuideGeometry) {
        let GuideGeometry {
            source,
            card,
            height,
            scale,
            tree,
        } = geometry;
        if !tree
            && ((card.center().x - source.center().x).abs() > 5.0
                || (card.center().y - source.center().y).abs() > height * scale)
        {
            scene.push_shape(OverlayShape::label_connector(
                source.center(),
                card.center(),
                self.0.color,
                self.0.width * scale,
                group,
            ));
        }
    }
}

// Compare formatted content directly against the cache, with no temporary String or hash collisions.
fn formatted_eq(mut expected: &str, args: std::fmt::Arguments<'_>) -> bool {
    struct Compare<'a, 'b>(&'a mut &'b str);
    impl std::fmt::Write for Compare<'_, '_> {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if let Some(rest) = self.0.strip_prefix(text) {
                *self.0 = rest;
                Ok(())
            } else {
                Err(std::fmt::Error)
            }
        }
    }
    std::fmt::write(&mut Compare(&mut expected), args).is_ok() && expected.is_empty()
}

impl WindowView<'_> {
    fn text_matches(&self, window: &crate::api::window::WindowInfo, lines: &[String]) -> bool {
        if let Some(group) = self.tabs.containing(window.id) {
            lines.len() == group.members.len() + 1
                && formatted_eq(
                    &lines[0],
                    format_args!("~{} · {} windows", group.id.0, group.members.len()),
                )
                && group.members.iter().zip(&lines[1..]).all(|(id, line)| {
                    let number = self.numbers.get(id).copied().unwrap_or(0);
                    let selected = if *id == group.active { "●" } else { "○" };
                    self.inventory.get(id).map_or_else(
                        || formatted_eq(line, format_args!("{selected} {number} · Window")),
                        |member| {
                            formatted_eq(
                                line,
                                format_args!(
                                    "{selected} {number} · {} — {}",
                                    app_name(member),
                                    member.title
                                ),
                            )
                        },
                    )
                })
        } else {
            lines.len() == 2 && lines[0] == app_name(window) && lines[1] == window.title
        }
    }
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        (self.styles.render_scene)(self, ctx)
    }

    fn scene_using<G: CardGuides>(&self, ctx: &HostContext<'_>, guides: G) -> OverlayScene {
        let mut next_group = 0;
        let mut scene = self.screen_scene(ctx, &mut next_group, guides);
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
                    .screen_scene(ctx, &mut next_group, guides);
                    scene.clip = Some(scene.clip.map_or(ctx.screens[screen].bounds, |clip| {
                        clip.union(&ctx.screens[screen].bounds)
                    }));
                    scene.extend_labels(&other);
                    scene.shapes.extend(other.shapes.iter().cloned());
                }
            }
        }
        scene
    }
    fn screen_scene<G: CardGuides>(
        &self,
        ctx: &HostContext<'_>,
        next_group: &mut u32,
        guides: G,
    ) -> OverlayScene {
        let mut scene = OverlayScene::new();
        let resolved = self.styles.for_appearance(ctx.palette.appearance);
        let card_config = &self.styles.card;
        let style = &resolved.base;
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
        let style = &resolved.number;
        let small = &resolved.app;
        let title_style = &resolved.title;
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
        let digits = self
            .numbers
            .values()
            .map(|n| n.checked_ilog10().unwrap_or(0) as usize + 1)
            .chain(
                slots
                    .iter()
                    .map(|s| s.id.checked_ilog10().unwrap_or(0) as usize + 2),
            )
            .max()
            .unwrap_or(1);
        let number_width = (style.font_size * 0.75 * digits as f64 + style.padding_x * 2.0)
            .max(card_config.number_min_width);
        let height = resolved.min_height;
        let mut cache = self.text_cache.map(|cache| cache.entries.borrow_mut());
        if let Some(cache) = &mut cache {
            cache.retain(|id, _| self.inventory.contains_key(id));
        }
        let lines: Vec<std::sync::Arc<[String]>> = windows
            .iter()
            .map(|window| {
                if let Some(lines) = cache.as_ref().and_then(|cache| cache.get(&window.id))
                    && self.text_matches(window, lines)
                {
                    return lines.clone();
                }
                let built: Vec<String> = {
                    if let Some(group) = self.tabs.containing(window.id) {
                        std::iter::once(format!(
                            "~{} · {} windows",
                            group.id.0,
                            group.members.len()
                        ))
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
                };
                let lines: std::sync::Arc<[String]> = built.into();
                if let Some(cache) = &mut cache {
                    cache.insert(window.id, lines.clone());
                }
                lines
            })
            .collect();
        drop(cache);
        let row_height = resolved.row_height;
        let max_rows = ((screen.work_area.height / scale - card_config.padding_y * 2.0 - 6.0)
            / row_height)
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
                (lines.len().div_ceil(lines.len().div_ceil(max_rows)) as f64 * row_height
                    + card_config.padding_y * 2.0)
                    .max(height)
            })
            .fold(height, f64::max);
        let desired_width =
            (number_width + card_config.padding_x * 2.0 + card_config.text_width * columns as f64)
                * scale;
        let area = screen.work_area.inset(3.0 * scale, 3.0 * scale);
        let position_mode = if self.configurable_position {
            card_config.position_mode
        } else {
            crate::api::style::WindowCardPositionMode::Window
        };
        let (physical_width, positions) = match position_mode {
            crate::api::style::WindowCardPositionMode::Window => {
                let centers: Vec<_> = windows
                    .iter()
                    .map(|w| {
                        if !self.configurable_position {
                            return slots.iter().find(|s| s.window == Some(w.id)).map_or(
                                w.bounds.center(),
                                |slot| {
                                    let area = placed_rect(screen.work_area, slot.rect, self.gap);
                                    crate::api::Point::new(area.center().x, area.y + 40.0 * scale)
                                },
                            );
                        }
                        let area = slots
                            .iter()
                            .find(|s| s.window == Some(w.id))
                            .map_or(w.bounds, |slot| {
                                placed_rect(screen.work_area, slot.rect, self.gap)
                            });
                        crate::api::Point::new(
                            area.x + area.width * self.styles.anchor.x,
                            area.y + area.height * self.styles.anchor.y,
                        )
                    })
                    .collect();
                card_positions(&centers, desired_width, group_height * scale, area)
            }
            crate::api::style::WindowCardPositionMode::Screen => {
                super::label_placement::screen_card_positions(
                    windows.len(),
                    desired_width,
                    group_height * scale,
                    area,
                    self.styles.position,
                    6.0 * scale,
                )
            }
        };
        let width = physical_width / scale;
        guides.initialize(&mut scene);
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
                (rows as f64 * row_height + card_config.padding_y * 2.0).max(height) * scale,
            );

            guides.attach(
                &mut scene,
                group,
                GuideGeometry {
                    source: window.bounds,
                    card,
                    height,
                    scale,
                    tree: self.tree.is_some(),
                },
            );
            scene.push_label(
                OverlayLabel::new("", card, resolved.background.clone())
                    .with_z_index(19)
                    .with_placement(group, Role::Background),
            );
            let number_rect = Rect::new(card.x, card.y, number_width * scale, card.height);
            scene.push_label(
                OverlayLabel::new(text, logical(number_rect, scale), style.clone())
                    .with_z_index(20)
                    .with_placement(group, Role::Fixed),
            );
            let content_width =
                ((width - number_width - card_config.padding_x * 2.0) / columns as f64).max(1.0);
            for (line, text) in lines.iter().enumerate() {
                let label_style = if line == 0 { small } else { title_style };
                let rect = Rect::new(
                    card.x
                        + (number_width
                            + card_config.padding_x
                            + (line / rows) as f64 * content_width)
                            * scale,
                    card.y + (card_config.padding_y + (line % rows) as f64 * row_height) * scale,
                    content_width * scale,
                    row_height * scale,
                );
                crate::presentation::key_help::push_sized_shared_help_text(
                    &mut scene,
                    crate::presentation::elide_width(text, content_width / label_style.font_size),
                    rect,
                    label_style,
                    scale,
                );
                if let Some(label) = scene.labels.last_mut() {
                    label.z_index = 21;
                }
                scene.set_label_placement(
                    scene.labels.len() - 1,
                    crate::api::overlay::LabelPlacement {
                        group,
                        role: Role::Flexible,
                    },
                );
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
    use crate::api::Point;

    #[test]
    fn card_cache_reuses_geometry_only_content_and_invalidates_exact_text_changes() {
        use crate::api::presentation::WindowTextCache;
        use crate::api::window::{WindowId, WindowInfo};
        use crate::api::window_tabs::{TabGroup, TabGroupId, TabState};
        let config = crate::config::Config::default();
        let palette = config.palette(crate::api::Appearance::Light);
        let styles = crate::api::style::WindowStyles::new(
            &config.window.ui,
            &config.window.card,
            &palette,
            &config.palette(crate::api::Appearance::Dark),
            RENDERERS,
        );
        let screens = [crate::api::Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            name: None,
            scale: 1.0,
            is_primary: true,
        }];
        let ctx = HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &screens,
            cursor: Point::default(),
            focused_app: None,
            palette: &palette,
        };
        let cache = WindowTextCache::default();
        let mut inventory: std::collections::BTreeMap<_, _> = (1..=2)
            .map(|id| {
                (
                    WindowId(id),
                    WindowInfo {
                        id: WindowId(id),
                        title: format!("Title {id}"),
                        app: "Example.exe".into(),
                        bounds: Rect::new(20.0, 30.0, 500.0, 400.0),
                        screen: 0,
                        resizable: true,
                        minimized: false,
                        maximized: false,
                        fullscreen: false,
                    },
                )
            })
            .collect();
        let mut numbers = [(WindowId(1), 1), (WindowId(2), 2)].into_iter().collect();
        let mut tabs = TabState::default();
        let render = |inventory: &std::collections::BTreeMap<_, _>,
                      numbers: &std::collections::BTreeMap<_, _>,
                      tabs: &TabState| {
            WindowView {
                text_cache: Some(&cache),
                configurable_position: true,
                tabs,
                group_input: false,
                styles: &styles,
                border_width: 3.0,
                target: None,
                screen: 0,
                inventory,
                visible: &[WindowId(1), WindowId(2)],
                numbers,
                tree: None,
                gap: 0.0,
            }
            .scene(&ctx)
        };
        render(&inventory, &numbers, &tabs);
        let first = cache.entries.borrow()[&WindowId(1)].clone();
        inventory.get_mut(&WindowId(1)).unwrap().bounds.x += 200.0;
        render(&inventory, &numbers, &tabs);
        assert!(std::sync::Arc::ptr_eq(
            &first,
            &cache.entries.borrow()[&WindowId(1)]
        ));
        inventory.get_mut(&WindowId(1)).unwrap().title = "Renamed".into();
        let scene = render(&inventory, &numbers, &tabs);
        assert!(scene.labels.iter().any(|label| label.text == "Renamed"));
        assert!(!std::sync::Arc::ptr_eq(
            &first,
            &cache.entries.borrow()[&WindowId(1)]
        ));
        tabs.groups.push(TabGroup {
            id: TabGroupId(1),
            active: WindowId(1),
            members: vec![WindowId(1), WindowId(2)],
        });
        render(&inventory, &numbers, &tabs);
        let grouped = cache.entries.borrow()[&WindowId(1)].clone();
        assert_eq!(grouped.len(), 3);
        numbers.insert(WindowId(2), 9);
        render(&inventory, &numbers, &tabs);
        assert!(!std::sync::Arc::ptr_eq(
            &grouped,
            &cache.entries.borrow()[&WindowId(1)]
        ));
        assert!(cache.entries.borrow()[&WindowId(1)][2].contains("9 ·"));
        tabs.groups.clear();
        inventory.remove(&WindowId(2));
        render(&inventory, &numbers, &tabs);
        assert!(!cache.entries.borrow().contains_key(&WindowId(2)));
        assert_eq!(cache.entries.borrow()[&WindowId(1)].len(), 2);
    }
    #[test]
    fn card_colors_and_large_title_keep_text_separate() {
        use crate::api::window::{WindowId, WindowInfo};
        let config = crate::config::Config::parse(
            r##"[window.card]
app_font_size = 18
title_font_size = 30
app_color = "#123456FF"
title_color = "#654321FF"
number_color = "#112233FF"
background_color = "#ABCDEFEE"
border_color = "#FEDCBAFF"
"##,
        )
        .unwrap();
        let palette = config.palette(crate::api::Appearance::Light);
        let screens = [crate::api::Screen {
            bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            work_area: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            name: None,
            scale: 1.0,
            is_primary: true,
        }];
        let ctx = HostContext {
            presenter: &crate::presentation::COMPOSER,
            screens: &screens,
            cursor: Point::default(),
            focused_app: None,
            palette: &palette,
        };
        let id = WindowId(1);
        let inventory = [(
            id,
            WindowInfo {
                id,
                app: "Example".into(),
                title: "Title".into(),
                bounds: screens[0].work_area,
                screen: 0,
                resizable: true,
                minimized: false,
                maximized: false,
                fullscreen: false,
            },
        )]
        .into_iter()
        .collect();
        let view = WindowView {
            text_cache: None,
            configurable_position: true,
            tabs: &Default::default(),
            group_input: false,
            styles: &crate::api::style::WindowStyles::new(
                &config.window.ui,
                &config.window.card,
                &palette,
                &config.palette(crate::api::Appearance::Dark),
                crate::presentation::window::RENDERERS,
            ),
            border_width: 3.0,
            target: None,
            screen: 0,
            inventory: &inventory,
            visible: &[id],
            numbers: &[(id, 1)].into_iter().collect(),
            tree: None,
            gap: 0.0,
        };
        let scene = view.scene(&ctx);
        let app = scene.labels.iter().find(|l| l.text == "Example").unwrap();
        let title = scene.labels.iter().find(|l| l.text == "Title").unwrap();
        let number = scene.labels.iter().find(|l| l.text == "1").unwrap();
        assert_eq!(app.style.font_size, 18.0);
        assert_eq!(title.style.font_size, 30.0);
        assert!(title.rect.y >= app.rect.y + app.rect.height);
        assert_eq!(app.style.text_color, Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(title.style.text_color, Color::rgb(0x65, 0x43, 0x21));
        assert_eq!(number.style.text_color, Color::rgb(0x11, 0x22, 0x33));
        assert_eq!(number.style.border_color, Color::rgb(0xFE, 0xDC, 0xBA));
        // The actual TOML flag selects a specialized renderer once, including
        // reload from enabled -> disabled -> enabled. Both themes agree.
        for enabled in [true, false, true] {
            let configured = crate::config::Config::parse(&format!(
                "[window.card]\nguide_line_enabled = {enabled}\n"
            ))
            .unwrap();
            let styles = crate::api::style::WindowStyles::new(
                &configured.window.ui,
                &configured.window.card,
                &configured.palette(crate::api::Appearance::Light),
                &configured.palette(crate::api::Appearance::Dark),
                RENDERERS,
            );
            for appearance in [crate::api::Appearance::Light, crate::api::Appearance::Dark] {
                assert_eq!(
                    styles.for_appearance(appearance).guide_line.is_some(),
                    enabled
                );
            }
            let mut rendered = WindowView {
                text_cache: None,
                styles: &styles,
                ..view
            }
            .scene(&ctx);
            assert_eq!(rendered.connector_style().is_some(), enabled);
            let card = rendered
                .labels
                .iter()
                .find(|l| l.z_index == 19)
                .unwrap()
                .rect;
            super::super::label_placement::avoid_overlaps(&mut rendered, &screens[0], &[card]);
            let count = rendered
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(
                        shape,
                        OverlayShape::Line {
                            placement_group: Some(_),
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(count, usize::from(enabled));
        }
        let repeated = view.scene(&ctx);
        for (first, second) in scene.labels.iter().zip(repeated.labels.iter()) {
            assert!(
                first.style.ptr_eq(&second.style),
                "static card styles must be reused"
            );
        }
    }
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

// Native compact tag geometry differs on macOS; this fixture is Windows-specific.
#[cfg(all(test, target_os = "windows"))]
mod baseline_tests;
