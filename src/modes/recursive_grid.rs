//! Recursive grid mode.
//!
//! The active area is divided into a keyboard-ordered grid (5x6 by default).
//! Pressing a cell key narrows the area and re-draws, so each keystroke
//! multiplies precision. `backspace` widens back out, `space` resets, and
//! `enter` clicks the centre of the current selected area.
//!
//! Supports neru's `[recursive_grid.layers]` per-depth overrides, `label_char`,
//! autohide thresholds and sub-key previews. `enter` finishes at the centre of
//! the current area without clicking.

use crate::api::binding::Binding;
use crate::api::command::{Command, CommandBatch, FinishCause, HostContext, Mode, ModeEvent};
use crate::api::geometry::{Point, Rect};
use crate::api::input::{Key, KeyState, ModeId};
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape};
use crate::config::{Config, GridLayer, Palette, RecursiveGridUi, TargetingLifecycle};

/// Grid shape at one depth.
#[derive(Debug, Clone, PartialEq)]
struct Layout {
    rows: usize,
    cols: usize,
    keys: Vec<char>,
}

pub struct RecursiveGridMode {
    /// Fully resolved per-depth layouts. Native input never clones a base
    /// layout or searches configuration overrides.
    layouts: Vec<Layout>,
    min_size: (f64, f64),
    max_depth: u32,
    default_cursor_follow_selection: bool,
    cursor_follow_selection: bool,
    ui: RecursiveGridUi,

    /// Areas from the root down to the current one; `stack[0]` is the root.
    stack: Vec<Rect>,
    /// Per-depth row-major indices replayed with each layer's layout.
    path: Vec<usize>,
    /// A leaf has been selected and the session is ready to finish.
    terminal: bool,
    finished: bool,
    lifecycle: TargetingLifecycle,
    return_mode: ModeId,
}

impl RecursiveGridMode {
    pub fn new(config: &Config) -> Self {
        let rg = &config.recursive_grid;
        let max_depth = rg.max_depth.max(1);
        let base = Layout {
            rows: rg.grid_rows.max(1) as usize,
            cols: rg.grid_cols.max(1) as usize,
            keys: rg.keys.chars().collect(),
        };
        Self {
            layouts: compile_layouts(&base, &rg.layers, max_depth),
            min_size: (rg.min_size_width as f64, rg.min_size_height as f64),
            max_depth,
            default_cursor_follow_selection: rg.cursor_follow_selection,
            cursor_follow_selection: rg.cursor_follow_selection,
            ui: rg.ui.clone(),
            stack: Vec::new(),
            path: Vec::new(),
            terminal: false,
            finished: false,
            lifecycle: rg.lifecycle.clone(),
            return_mode: ModeId::idle(),
        }
    }

    fn depth(&self) -> u32 {
        self.stack.len().saturating_sub(1) as u32
    }

    fn current(&self) -> Option<Rect> {
        self.stack.last().copied()
    }

    /// Layout for `depth`, applying any `[recursive_grid.layers]` override.
    fn layout_at(&self, depth: u32) -> &Layout {
        let index = (depth as usize).min(self.layouts.len().saturating_sub(1));
        &self.layouts[index]
    }

    /// Cells of the current area, paired with their keys.
    #[cfg(test)]
    fn cells(&self) -> Vec<(char, Rect)> {
        if self.terminal {
            return Vec::new();
        }
        let Some(area) = self.current() else {
            return Vec::new();
        };
        let layout = self.layout_at(self.depth());
        layout
            .keys
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(index, key)| {
                area.subdivision(layout.rows, layout.cols, index)
                    .map(|rect| (key, rect))
            })
            .collect()
    }

    /// Whether the current area may be subdivided again.
    fn can_descend(&self) -> bool {
        if self.depth() + 1 >= self.max_depth {
            return false;
        }
        let Some(area) = self.current() else {
            return false;
        };
        let layout = self.layout_at(self.depth());
        let cell_w = area.width / layout.cols as f64;
        let cell_h = area.height / layout.rows as f64;
        cell_w >= self.min_size.0 && cell_h >= self.min_size.1
    }

    /// Label text for a cell key, honouring `label_char`.
    fn label_text(&self, key: char) -> String {
        if self.ui.label_char.is_empty() {
            key.to_string()
        } else {
            self.ui.label_char.clone()
        }
    }

    /// A label is hidden once its cell is too small to read it in.
    fn label_fits(&self, cell: Rect, font_size: f64, multiplier: f64) -> bool {
        if multiplier <= 0.0 {
            return true;
        }
        let need = font_size * multiplier;
        cell.width >= need && cell.height >= need
    }

    fn scene(&self, palette: &Palette) -> OverlayScene {
        let Some(area) = self.current() else {
            return OverlayScene::new();
        };
        let appearance = palette.appearance;
        let resolve = crate::config::style::resolve;

        let line_color = resolve(self.ui.line_color.as_ref(), appearance, palette.accent);
        let highlight = resolve(
            self.ui.highlight_color.as_ref(),
            appearance,
            palette.highlight(),
        );
        let label_bg = if self.ui.label_background {
            resolve(
                self.ui.label_background_color.as_ref(),
                appearance,
                palette.surface_label(),
            )
        } else {
            Color::TRANSPARENT
        };

        let base_style = self.ui.label.resolve(
            palette,
            label_bg,
            palette.text,
            if self.ui.label_background {
                palette.accent_border()
            } else {
                Color::TRANSPARENT
            },
        );
        let line_width = self.ui.line_width.max(0) as f64;
        let layout = self.layout_at(self.depth());
        let shape_capacity = if self.terminal {
            1
        } else {
            1 + layout.rows.saturating_sub(1) + layout.cols.saturating_sub(1)
        };
        let label_capacity = if self.terminal {
            0
        } else if self.ui.sub_key_preview && self.can_descend() {
            layout.keys.len().saturating_mul(2)
        } else {
            layout.keys.len()
        };
        let mut scene = OverlayScene::with_capacity(shape_capacity, label_capacity);

        if self.terminal {
            scene.push_shape(OverlayShape::Rect {
                rect: area,
                fill: highlight.with_opacity(0.55),
                stroke: line_color,
                stroke_width: line_width.max(1.0),
                corner_radius: 0.0,
                z_index: 0,
            });
            scene.backdrop = Some(Color::rgba(0, 0, 0, 0x40));
            scene.clip = self.stack.first().copied();
            return scene;
        }

        // Highlight the narrowed area and outline it.
        scene.push_shape(OverlayShape::Rect {
            rect: area,
            fill: highlight,
            stroke: line_color,
            stroke_width: line_width.max(1.0),
            corner_radius: 0.0,
            z_index: 0,
        });

        // Interior rulings.
        let cell_w = area.width / layout.cols as f64;
        let cell_h = area.height / layout.rows as f64;
        for c in 1..layout.cols {
            let x = area.x + c as f64 * cell_w;
            scene.push_shape(OverlayShape::Line {
                from: Point::new(x, area.top()),
                to: Point::new(x, area.bottom()),
                color: line_color,
                width: line_width,
                z_index: 1,
            });
        }
        for r in 1..layout.rows {
            let y = area.y + r as f64 * cell_h;
            scene.push_shape(OverlayShape::Line {
                from: Point::new(area.left(), y),
                to: Point::new(area.right(), y),
                color: line_color,
                width: line_width,
                z_index: 1,
            });
        }

        // Cell keys, plus an optional preview of the next level's keys.
        let next_keys: String = if self.ui.sub_key_preview && self.can_descend() {
            self.layout_at(self.depth() + 1)
                .keys
                .iter()
                .map(|k| {
                    if self.ui.sub_key_preview_label_char().is_empty() {
                        *k
                    } else {
                        self.ui
                            .sub_key_preview_label_char()
                            .chars()
                            .next()
                            .unwrap_or(*k)
                    }
                })
                .collect()
        } else {
            String::new()
        };

        for (index, key) in layout.keys.iter().copied().enumerate() {
            let Some(cell) = area.subdivision(layout.rows, layout.cols, index) else {
                break;
            };
            let text = self.label_text(key);
            let scale = base_style.fit_scale(&text, cell);
            let fitted_font_size = base_style.font_size * scale;
            if fitted_font_size >= self.ui.label_min_font_size.max(1) as f64 {
                let label_style = base_style.scaled(scale);
                if self.label_fits(
                    cell,
                    label_style.font_size,
                    self.ui.label_autohide_multiplier,
                ) {
                    scene.push_label(
                        OverlayLabel::new(text, cell, label_style)
                            .fitted()
                            .with_z_index(3),
                    );
                }
            }

            if !next_keys.is_empty() {
                // Sit the preview in the lower part of the cell, then fit the
                // complete next-key string inside that smaller rectangle.
                let preview = Rect::new(
                    cell.x,
                    cell.center().y + cell.height * 0.15,
                    cell.width,
                    cell.height * 0.3,
                );
                let sub_style = LabelStyle {
                    font_size: self.ui.sub_key_preview_font_size.max(1) as f64,
                    text_color: resolve(
                        self.ui.sub_key_preview_text_color.as_ref(),
                        appearance,
                        palette.text.with_opacity(0.6),
                    ),
                    background: Color::TRANSPARENT,
                    border_color: Color::TRANSPARENT,
                    border_width: 0.0,
                    bold: false,
                    ..base_style.clone()
                };
                let scale = sub_style.fit_scale(&next_keys, preview);
                let fitted_font_size = sub_style.font_size * scale;
                let sub_style = sub_style.scaled(scale);
                if fitted_font_size >= 4.0
                    && self.label_fits(
                        preview,
                        sub_style.font_size,
                        self.ui.sub_key_preview_autohide_multiplier,
                    )
                {
                    scene.push_label(
                        OverlayLabel::new(next_keys.clone(), preview, sub_style)
                            .fitted()
                            .with_z_index(2),
                    );
                }
            }
        }

        // Keep the window scoped to the original active screen while the
        // visible cell narrows through successive recursive selections.
        scene.clip = self.stack.first().copied();
        scene
    }

    fn redraw(&self, palette: &Palette) -> Vec<Command> {
        vec![Command::show_overlay(self.scene(palette))]
    }

    /// Complete selection by moving only; lifecycle configuration decides what follows.
    fn commit(&self, point: Point, palette: &Palette) -> Vec<Command> {
        vec![
            Command::warp_to(point),
            Command::show_overlay(self.scene(palette)),
            Command::FinishMode {
                cause: FinishCause::Selection,
            },
        ]
    }

    fn cancel(&self) -> Vec<Command> {
        vec![
            Command::HideOverlay,
            Command::SwitchMode(self.return_mode.clone()),
        ]
    }

    fn toggle_cursor_follow(&mut self, palette: &Palette) -> Vec<Command> {
        self.cursor_follow_selection = !self.cursor_follow_selection;
        let mut commands = Vec::new();
        if self.cursor_follow_selection
            && let Some(area) = self.current()
        {
            commands.push(Command::warp_to(area.center()));
        }
        commands.extend(self.redraw(palette));
        commands
    }

    fn reset(&mut self, bounds: Rect) {
        self.stack = vec![bounds];
        self.path.clear();
        self.terminal = false;
        self.finished = false;
        self.cursor_follow_selection = self.default_cursor_follow_selection;
    }

    fn retarget(&mut self, bounds: Rect, preserve: bool, palette: &Palette) -> Vec<Command> {
        let path = if preserve {
            self.path.clone()
        } else {
            Vec::new()
        };
        let was_finished = preserve && self.finished;
        let follow = self.cursor_follow_selection;
        self.reset(bounds);
        if preserve {
            self.cursor_follow_selection = follow;
            for index in path {
                let layout = self.layout_at(self.depth());
                let Some(cell) = self
                    .current()
                    .and_then(|area| area.subdivision(layout.rows, layout.cols, index))
                else {
                    break;
                };
                self.stack.push(cell);
                self.path.push(index);
            }
            self.terminal = self.depth() >= self.max_depth || !self.can_descend();
            self.finished = was_finished;
        }
        let mut commands = vec![Command::warp_to(self.current().unwrap_or(bounds).center())];
        commands.extend(self.redraw(palette));
        commands
    }

    fn select(&mut self, index: usize, cell: Rect, palette: &Palette) -> Vec<Command> {
        self.stack.push(cell);
        self.path.push(index);
        // A selected cell is terminal after the configured depth, or when it
        // cannot be meaningfully subdivided further.
        self.terminal = self.depth() >= self.max_depth || !self.can_descend();

        if self.terminal {
            return self.commit(cell.center(), palette);
        }

        let mut commands = Vec::new();
        if self.cursor_follow_selection {
            commands.push(Command::warp_to(cell.center()));
        }
        commands.extend(self.redraw(palette));
        commands
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> Vec<Command> {
        match key.as_str() {
            "esc" => return self.cancel(),
            "enter" => {
                return match self.current() {
                    Some(area) => self.commit(area.center(), ctx.palette),
                    None => self.cancel(),
                };
            }
            "backspace" | "tab" => {
                // Widen back out; at the root this leaves the mode.
                if self.stack.len() <= 1 {
                    return self.cancel();
                }
                self.stack.pop();
                self.path.pop();
                self.terminal = false;
                self.finished = false;
                return self.redraw(ctx.palette);
            }
            "space" => {
                self.reset(ctx.active_bounds());
                return self.redraw(ctx.palette);
            }
            _ => {}
        }

        if self.terminal || self.finished {
            return Vec::new();
        }
        let Some(ch) = key.as_char() else {
            return Vec::new();
        };
        let layout = self.layout_at(self.depth());
        let Some(index) = layout.keys.iter().position(|candidate| *candidate == ch) else {
            return Vec::new();
        };
        let Some(cell) = self
            .current()
            .and_then(|area| area.subdivision(layout.rows, layout.cols, index))
        else {
            return Vec::new();
        };
        self.select(index, cell, ctx.palette)
    }
}

fn compile_layouts(base: &Layout, layers: &[GridLayer], max_depth: u32) -> Vec<Layout> {
    (0..=max_depth)
        .map(|depth| {
            let mut layout = base.clone();
            if let Some(layer) = layers.iter().find(|layer| layer.depth == depth) {
                if let Some(cols) = layer.grid_cols {
                    layout.cols = cols.max(1) as usize;
                }
                if let Some(rows) = layer.grid_rows {
                    layout.rows = rows.max(1) as usize;
                }
                if let Some(keys) = &layer.keys {
                    layout.keys = keys.chars().collect();
                }
            }
            layout
        })
        .collect()
}

impl RecursiveGridUi {
    fn sub_key_preview_label_char(&self) -> &str {
        &self.label_char
    }
}

impl Mode for RecursiveGridMode {
    fn id(&self) -> ModeId {
        ModeId::recursive_grid()
    }

    fn display_name(&self) -> String {
        "Recursive Grid".into()
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent_alt)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::from(match event {
            ModeEvent::Activated { previous } => {
                self.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::Restarted => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::FinishRequested { .. } if self.finished => Vec::new(),
            ModeEvent::FinishRequested { .. } => {
                self.finished = true;
                let mut commands = self.redraw(ctx.palette);
                commands.extend(super::lifecycle_commands(
                    &self.lifecycle.after_finish,
                    &self.return_mode,
                ));
                commands
            }
            ModeEvent::Clicked { .. } => {
                super::lifecycle_commands(&self.lifecycle.after_click, &self.return_mode)
            }
            ModeEvent::Deactivated => {
                self.stack.clear();
                self.path.clear();
                self.terminal = false;
                self.finished = false;
                Vec::new()
            }
            ModeEvent::ScreensChanged(_) => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::ScreenRetargeted { screen, preserve } => {
                self.retarget(screen.bounds, *preserve, ctx.palette)
            }
            ModeEvent::PointerMoved(_)
                if self.stack.first().copied() != Some(ctx.active_bounds()) =>
            {
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::Resumed => self.redraw(ctx.palette),
            ModeEvent::ConfigReloaded => {
                let return_mode = self.return_mode.clone();
                let Some(config) = ctx.config.downcast_ref::<Config>() else {
                    return CommandBatch::new();
                };
                *self = Self::new(config);
                self.return_mode = return_mode;
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } if matches!(binding.as_ref(), Binding::ToggleCursorFollowSelection) => {
                self.toggle_cursor_follow(ctx.palette)
            }
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                ..
            } => self.key_down(key, ctx),
            _ => Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::Screen;

    struct Env {
        screens: Vec<Screen>,
        cursor: Point,
        palette: Palette,
        config: Config,
    }

    fn legacy_config() -> Config {
        let mut config = Config::default();
        config.recursive_grid.grid_cols = 3;
        config.recursive_grid.grid_rows = 3;
        config.recursive_grid.keys = "rtyfghvbn".into();
        config.recursive_grid.max_depth = 10;
        config
    }

    impl Env {
        fn new() -> Self {
            Self::with(legacy_config())
        }
        fn with(config: Config) -> Self {
            Self {
                screens: vec![Screen {
                    bounds: Rect::new(0.0, 0.0, 900.0, 900.0),
                    work_area: Rect::new(0.0, 0.0, 900.0, 900.0),
                    is_primary: true,
                    scale: 1.0,
                    name: None,
                }],
                cursor: Point::new(450.0, 450.0),
                palette: Palette::default(),
                config,
            }
        }
        fn ctx(&self) -> HostContext<'_> {
            HostContext {
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: None,
                palette: &self.palette,
                config: &self.config,
            }
        }
    }

    fn activate(mode: &mut RecursiveGridMode, env: &Env) -> Vec<Command> {
        mode.handle(&ModeEvent::Activated { previous: None }, &env.ctx())
            .into_iter()
            .collect()
    }

    fn press(mode: &mut RecursiveGridMode, env: &Env, name: &str) -> Vec<Command> {
        mode.handle(
            &ModeEvent::Key {
                key: Key::new(name).unwrap(),
                state: KeyState::Down,
                repeat: false,
            },
            &env.ctx(),
        )
        .into_iter()
        .collect()
    }

    fn toggle_follow(mode: &mut RecursiveGridMode, env: &Env) -> Vec<Command> {
        mode.handle(
            &ModeEvent::Binding {
                binding: Binding::ToggleCursorFollowSelection.into(),
                state: KeyState::Down,
                key: Key::new("`").unwrap(),
            },
            &env.ctx(),
        )
        .into_iter()
        .collect()
    }

    fn scene_of<'a>(commands: impl IntoIterator<Item = &'a Command>) -> &'a OverlayScene {
        commands
            .into_iter()
            .find_map(|c| match c {
                Command::ShowOverlay(s) => Some(s),
                _ => None,
            })
            .expect("expected an overlay")
    }

    #[test]
    fn product_defaults_keep_the_three_by_three_recursive_grid_with_follow_enabled() {
        let env = Env::with(Config::default());
        let mut mode = RecursiveGridMode::new(&env.config);
        let out = activate(&mut mode, &env);
        assert_eq!(scene_of(&out).labels.len(), 9);
        assert!(mode.cursor_follow_selection);

        toggle_follow(&mut mode, &env);
        assert!(!mode.cursor_follow_selection);
        let out = press(&mut mode, &env, "q");
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::WarpPointer { .. }))
        );
    }

    #[test]
    fn enabling_follow_immediately_warps_to_the_current_recursive_cell_centre() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        toggle_follow(&mut mode, &env);
        press(&mut mode, &env, "g");
        let selected = mode.current().unwrap();
        let depth = mode.depth();

        let out = toggle_follow(&mut mode, &env);

        assert!(mode.cursor_follow_selection);
        assert_eq!(mode.depth(), depth, "toggle must not select another layer");
        assert!(out.contains(&Command::warp_to(selected.center())));
        assert!(
            out.iter()
                .any(|command| matches!(command, Command::ShowOverlay(_)))
        );
        assert!(!out.iter().any(|command| matches!(
            command,
            Command::MouseButton { .. } | Command::SwitchMode(_)
        )));
    }

    #[test]
    fn activation_starts_at_the_full_screen() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        assert_eq!(mode.current(), Some(env.screens[0].bounds));
        assert_eq!(mode.depth(), 0);
    }

    #[test]
    fn default_grid_is_three_by_three_with_nine_keys() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        assert_eq!(mode.cells().len(), 9);
    }

    #[test]
    fn selecting_a_cell_narrows_the_area() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);

        // "rtyfghvbn": 'g' is index 4, the centre cell of a 3x3 grid.
        let out = press(&mut mode, &env, "g");
        assert!(out.iter().any(|c| matches!(c, Command::ShowOverlay(_))));
        assert_eq!(scene_of(&out).clip, Some(env.screens[0].bounds));
        assert_eq!(mode.depth(), 1);
        assert_eq!(mode.current(), Some(Rect::new(300.0, 300.0, 300.0, 300.0)));
    }

    #[test]
    fn each_level_multiplies_precision() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "r"); // top-left
        assert_eq!(mode.current(), Some(Rect::new(0.0, 0.0, 300.0, 300.0)));
        press(&mut mode, &env, "r");
        assert_eq!(mode.current(), Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
        assert_eq!(mode.depth(), 2);
    }

    #[test]
    fn screen_retarget_replays_or_resets_each_recursive_layer() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "t");
        press(&mut mode, &env, "g");
        assert_eq!(mode.path, [1, 4]);
        let target = Screen {
            bounds: Rect::new(-1200.0, 100.0, 1200.0, 800.0),
            work_area: Rect::new(-1200.0, 100.0, 1200.0, 800.0),
            is_primary: false,
            scale: 1.0,
            name: None,
        };

        let out = mode.handle(
            &ModeEvent::ScreenRetargeted {
                screen: target.clone(),
                preserve: true,
            },
            &env.ctx(),
        );
        assert_eq!(mode.path, [1, 4]);
        assert_eq!(mode.depth(), 2);
        assert_eq!(scene_of(&out).clip, Some(target.bounds));
        assert!(out.contains(&Command::warp_to(mode.current().unwrap().center())));

        mode.handle(
            &ModeEvent::ScreenRetargeted {
                screen: target.clone(),
                preserve: false,
            },
            &env.ctx(),
        );
        assert!(mode.path.is_empty());
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(target.bounds));
    }

    #[test]
    fn backspace_widens_and_then_dismisses_at_the_root() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");
        assert_eq!(mode.depth(), 1);

        press(&mut mode, &env, "backspace");
        assert_eq!(mode.depth(), 0);

        let out = press(&mut mode, &env, "backspace");
        assert_eq!(out, Command::dismiss_to_idle());
    }

    #[test]
    fn space_resets_to_the_root() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");
        press(&mut mode, &env, "g");
        assert_eq!(mode.depth(), 2);

        press(&mut mode, &env, "space");
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(env.screens[0].bounds));
    }

    #[test]
    fn enter_moves_to_the_centre_without_clicking() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");

        let out = press(&mut mode, &env, "enter");
        assert!(
            out.iter()
                .any(|c| matches!(c, Command::WarpPointer { x, y } if *x == 450.0 && *y == 450.0)),
            "{out:?}"
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::MouseButton { .. }))
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::SwitchMode(_)))
        );
    }

    #[test]
    fn max_depth_marks_the_selected_cell_as_terminal() {
        let mut config = legacy_config();
        config.recursive_grid.max_depth = 1;
        let env = Env::with(config);
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);

        let out = press(&mut mode, &env, "g");
        assert!(out.iter().any(|c| matches!(c, Command::WarpPointer { .. })));
        assert!(mode.terminal);
        assert!(!out.contains(&Command::SwitchMode(ModeId::idle())));
    }

    #[test]
    fn min_cell_size_stops_further_subdivision() {
        let mut config = legacy_config();
        config.recursive_grid.min_size_width = 200;
        config.recursive_grid.min_size_height = 200;
        let env = Env::with(config);
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);

        // The selected 300×300 cell cannot produce another 3×3 layer whose
        // cells meet the 200px minimum, so it is terminal immediately.
        let out = press(&mut mode, &env, "g");
        assert!(out.iter().any(|c| matches!(c, Command::WarpPointer { .. })));
        assert_eq!(mode.depth(), 1);
        assert!(mode.terminal);
    }

    #[test]
    fn layers_override_the_shape_at_a_given_depth() {
        let mut config = legacy_config();
        config.recursive_grid.layers = vec![GridLayer {
            depth: 0,
            grid_cols: Some(2),
            grid_rows: Some(2),
            keys: Some("crtn".into()),
        }];
        let env = Env::with(config);
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);

        assert_eq!(mode.cells().len(), 4);
        press(&mut mode, &env, "c");
        assert_eq!(mode.current(), Some(Rect::new(0.0, 0.0, 450.0, 450.0)));
        // Depth 1 has no override, so it reverts to the base 3x3.
        assert_eq!(mode.cells().len(), 9);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        let out = press(&mut mode, &env, "z");
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(mode.depth(), 0);
    }

    #[test]
    fn label_char_replaces_every_cell_key() {
        let mut config = legacy_config();
        config.recursive_grid.ui.label_char = "\u{B7}".into();
        let env = Env::with(config);
        let mut mode = RecursiveGridMode::new(&env.config);
        let out = activate(&mut mode, &env);

        let scene = scene_of(&out);
        assert!(!scene.labels.is_empty());
        assert!(scene.labels.iter().all(|l| l.text == "\u{B7}"));
    }

    #[test]
    fn sub_key_preview_adds_a_second_label_per_cell() {
        let mut config = legacy_config();
        config.recursive_grid.ui.sub_key_preview = true;
        let env = Env::with(config);
        let mut mode = RecursiveGridMode::new(&env.config);
        let out = activate(&mut mode, &env);

        let scene = scene_of(&out);
        // Nine cell keys plus nine previews.
        assert_eq!(scene.labels.len(), 18, "{:?}", scene.labels.len());
        assert!(scene.labels.iter().any(|l| l.text == "rtyfghvbn"));
    }

    #[test]
    fn crossing_displays_restarts_recursive_grid_on_the_cursor_screen() {
        let mut env = Env::new();
        env.screens.push(Screen {
            bounds: Rect::new(900.0, 0.0, 1200.0, 800.0),
            work_area: Rect::new(900.0, 0.0, 1200.0, 800.0),
            is_primary: false,
            scale: 2.0,
            name: None,
        });
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");
        assert_eq!(mode.depth(), 1);

        env.cursor = Point::new(1500.0, 400.0);
        let out = mode.handle(&ModeEvent::PointerMoved(env.cursor), &env.ctx());

        assert_eq!(mode.stack, vec![env.screens[1].bounds]);
        assert_eq!(mode.depth(), 0);
        assert_eq!(scene_of(&out).clip, Some(env.screens[1].bounds));
    }

    #[test]
    fn labels_shrink_before_they_reach_the_autohide_threshold() {
        let mut config = legacy_config();
        config.recursive_grid.ui.label.font_size = 20;
        config.recursive_grid.ui.label_min_font_size = 6;
        let env = Env::with(config);
        let mut mode = RecursiveGridMode::new(&env.config);
        let initial = activate(&mut mode, &env);
        let initial_font = scene_of(&initial).labels[0].style.font_size;

        press(&mut mode, &env, "g");
        let out = press(&mut mode, &env, "g");
        let labels = &scene_of(&out).labels;
        assert!(!labels.is_empty());
        assert!(labels.iter().all(|label| {
            label.style.font_size < initial_font
                && label.style.font_size >= env.config.recursive_grid.ui.label_min_font_size as f64
        }));
    }

    #[test]
    fn labels_autohide_once_fitting_would_make_them_too_small() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");
        press(&mut mode, &env, "g");
        let out = press(&mut mode, &env, "g");
        assert!(
            scene_of(&out).labels.is_empty(),
            "tiny cells should hide their labels"
        );
    }

    #[test]
    fn scene_draws_interior_rulings() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        let out = activate(&mut mode, &env);
        let lines = scene_of(&out)
            .shapes
            .iter()
            .filter(|s| matches!(s, OverlayShape::Line { .. }))
            .count();
        // A 3x3 grid has two vertical and two horizontal rulings.
        assert_eq!(lines, 4);
    }

    #[test]
    fn click_keeps_the_recursive_grid_live_for_further_subdivision() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");
        let selected = mode.current();

        let clicked = mode.handle(
            &ModeEvent::Clicked {
                button: crate::api::MouseButton::Left,
                action: crate::api::ButtonAction::Click,
            },
            &env.ctx(),
        );
        assert!(clicked.is_empty());
        assert!(!mode.finished);
        assert_eq!(mode.current(), selected);

        let continued = press(&mut mode, &env, "g");
        assert!(!mode.finished);
        assert_eq!(mode.depth(), 2);
        assert!(
            continued
                .iter()
                .any(|command| matches!(command, Command::ShowOverlay(_)))
        );
    }

    #[test]
    fn restart_resets_the_session_but_preserves_its_return_mode() {
        let env = Env::new();
        let mut mode = RecursiveGridMode::new(&env.config);
        mode.handle(
            &ModeEvent::Activated {
                previous: Some(ModeId::normal()),
            },
            &env.ctx(),
        );
        press(&mut mode, &env, "g");
        mode.handle(&ModeEvent::Restarted, &env.ctx());

        assert_eq!(mode.depth(), 0);
        assert!(!mode.finished);
        assert_eq!(mode.return_mode, ModeId::normal());
    }
}
