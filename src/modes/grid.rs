//! Grid mode: Mousemaster-style layered keyboard grid.
//!
//! The active display is divided into a row-major keyboard layout. Selecting a
//! cell narrows the next layer to that cell; by default, two selections identify
//! the target. The cursor can follow every live selection and that behaviour is
//! toggled for the current mode session with a configurable key.

use crate::api::binding::Binding;
use crate::api::command::{Command, CommandBatch, FinishCause, HostContext, Mode, ModeEvent};
use crate::api::geometry::{Point, Rect};
use crate::api::input::{Key, KeyState, ModeId};
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape};
use crate::config::{Config, GridUi, Palette, TargetingLifecycle};
use smallvec::SmallVec;

#[derive(Debug, Clone, PartialEq)]
struct Layout {
    rows: usize,
    cols: usize,
    keys: Vec<char>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct Cell {
    rect: Rect,
}

pub struct GridMode {
    layout: Layout,
    max_depth: u32,
    default_cursor_follow_selection: bool,
    cursor_follow_selection: bool,
    ui: GridUi,
    /// Areas from the active display to the selected leaf.
    stack: SmallVec<[Rect; 12]>,
    /// Row-major cell indices used to replay the selection on another display.
    path: SmallVec<[usize; 12]>,
    /// A leaf has been selected and the session is ready to finish.
    terminal: bool,
    finished: bool,
    lifecycle: TargetingLifecycle,
    return_mode: ModeId,
}

impl GridMode {
    pub fn new(config: &Config) -> Self {
        let grid = &config.grid;
        Self {
            layout: Layout {
                rows: grid.grid_rows.max(1) as usize,
                cols: grid.grid_cols.max(1) as usize,
                keys: grid.keys.chars().collect(),
            },
            max_depth: grid.max_depth.max(1),
            default_cursor_follow_selection: grid.cursor_follow_selection,
            cursor_follow_selection: grid.cursor_follow_selection,
            ui: grid.ui.clone(),
            stack: SmallVec::new(),
            path: SmallVec::new(),
            terminal: false,
            finished: false,
            lifecycle: grid.lifecycle.clone(),
            return_mode: ModeId::idle(),
        }
    }

    fn depth(&self) -> u32 {
        self.stack.len().saturating_sub(1) as u32
    }

    fn current(&self) -> Option<Rect> {
        self.stack.last().copied()
    }

    fn root(&self) -> Option<Rect> {
        self.stack.first().copied()
    }

    #[cfg(test)]
    fn cells(&self) -> Vec<Cell> {
        let Some(area) = self.current() else {
            return Vec::new();
        };
        self.layout
            .keys
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                area.subdivision(self.layout.rows, self.layout.cols, index)
                    .map(|rect| Cell { rect })
            })
            .collect()
    }

    fn style(&self, palette: &Palette) -> LabelStyle {
        self.ui.label.resolve(
            palette,
            palette.surface_cell(),
            palette.text,
            palette.accent_border(),
        )
    }

    fn previews_second_layer(&self) -> bool {
        !self.terminal && self.depth() == 0 && self.max_depth > 1
    }

    fn push_rulings(
        scene: &mut OverlayScene,
        area: Rect,
        rows: usize,
        cols: usize,
        color: Color,
        width: f64,
        z_index: i32,
    ) {
        if width <= 0.0 || color.is_transparent() {
            return;
        }
        let cell_width = area.width / cols as f64;
        let cell_height = area.height / rows as f64;
        for column in 1..cols {
            let x = area.x + column as f64 * cell_width;
            scene.push_shape(OverlayShape::Line {
                from: Point::new(x, area.top()),
                to: Point::new(x, area.bottom()),
                color,
                width,
                z_index,
            });
        }
        for row in 1..rows {
            let y = area.y + row as f64 * cell_height;
            scene.push_shape(OverlayShape::Line {
                from: Point::new(area.left(), y),
                to: Point::new(area.right(), y),
                color,
                width,
                z_index,
            });
        }
    }

    fn push_second_layer_preview(
        &self,
        scene: &mut OverlayScene,
        area: Rect,
        style: &LabelStyle,
        palette: &Palette,
    ) {
        // Because every depth uses the same layout, the nested rulings align
        // into one fine grid. Draw only the fine lines that are not already an
        // outer boundary, avoiding per-cell duplicate edges and alpha blending.
        let preview_rows = self.layout.rows.saturating_mul(self.layout.rows);
        let preview_cols = self.layout.cols.saturating_mul(self.layout.cols);
        let line_color = crate::config::style::resolve(
            self.ui.matched_border_color.as_ref(),
            palette.appearance,
            style.border_color,
        )
        .with_opacity(0.35);
        let line_width = style.border_width * 0.7;
        if line_width > 0.0 && !line_color.is_transparent() {
            let cell_width = area.width / preview_cols as f64;
            let cell_height = area.height / preview_rows as f64;
            for column in 1..preview_cols {
                if column % self.layout.cols == 0 {
                    continue;
                }
                let x = area.x + column as f64 * cell_width;
                scene.push_shape(OverlayShape::Line {
                    from: Point::new(x, area.top()),
                    to: Point::new(x, area.bottom()),
                    color: line_color,
                    width: line_width,
                    z_index: 1,
                });
            }
            for row in 1..preview_rows {
                if row % self.layout.rows == 0 {
                    continue;
                }
                let y = area.y + row as f64 * cell_height;
                scene.push_shape(OverlayShape::Line {
                    from: Point::new(area.left(), y),
                    to: Point::new(area.right(), y),
                    color: line_color,
                    width: line_width,
                    z_index: 1,
                });
            }
        }

        let preview_color = style.text_color.with_opacity(0.62);
        let preview_style = LabelStyle {
            background: Color::TRANSPARENT,
            text_color: preview_color,
            matched_text_color: preview_color,
            border_color: Color::TRANSPARENT,
            border_width: 0.0,
            border_radius: 0.0,
            bold: false,
            ..style.clone()
        };
        for outer_index in 0..self.layout.keys.len() {
            let Some(outer) = area.subdivision(self.layout.rows, self.layout.cols, outer_index)
            else {
                break;
            };
            for (inner_index, suffix) in self.layout.keys.iter().copied().enumerate() {
                let Some(rect) = outer.subdivision(self.layout.rows, self.layout.cols, inner_index)
                else {
                    break;
                };
                let text = suffix.to_string();
                let label_style = preview_style.fit_to(&text, rect);
                scene.push_label(
                    OverlayLabel::new(text, rect, label_style)
                        .fitted()
                        .with_z_index(2),
                );
            }
        }

        // The large prefix sits above the faint suffix grid, matching the
        // visual hierarchy of Mousemaster's nested decorations. Its size is
        // relative to the outer cell rather than capped by the ordinary label
        // font, so the first key remains readable at a glance on any display.
        let primary_color = style.matched_text_color.with_opacity(0.92);
        for (outer_index, prefix) in self.layout.keys.iter().copied().enumerate() {
            let Some(outer) = area.subdivision(self.layout.rows, self.layout.cols, outer_index)
            else {
                break;
            };
            let text = prefix.to_string();
            let primary_style = LabelStyle {
                font_size: style.font_size.max(outer.width.min(outer.height) * 0.62),
                text_color: primary_color,
                matched_text_color: primary_color,
                background: Color::TRANSPARENT,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                border_radius: 0.0,
                padding_x: 0.0,
                padding_y: 0.0,
                bold: false,
                ..style.clone()
            }
            .fit_to(&text, outer);
            scene.push_label(
                OverlayLabel::new(text, outer, primary_style)
                    .fitted()
                    .with_z_index(3),
            );
        }
    }

    fn scene(&self, palette: &Palette) -> OverlayScene {
        let preview_second_layer = self.previews_second_layer();
        let outer_rulings = self.layout.rows.saturating_sub(1) + self.layout.cols.saturating_sub(1);
        let preview_rulings = if preview_second_layer {
            self.layout
                .rows
                .saturating_mul(self.layout.rows.saturating_sub(1))
                + self
                    .layout
                    .cols
                    .saturating_mul(self.layout.cols.saturating_sub(1))
        } else {
            0
        };
        let shape_capacity = if self.terminal {
            1
        } else {
            1 + outer_rulings + preview_rulings
        };
        let label_capacity = if preview_second_layer {
            self.layout
                .keys
                .len()
                .saturating_mul(self.layout.keys.len())
                .saturating_add(self.layout.keys.len())
        } else {
            usize::from(!self.terminal) * self.layout.keys.len()
        };
        let mut scene = OverlayScene::with_capacity(shape_capacity, label_capacity);
        let style = self.style(palette);

        if self.terminal {
            if let Some(rect) = self.current() {
                scene.push_shape(OverlayShape::Rect {
                    rect,
                    fill: palette.highlight().with_opacity(0.55),
                    stroke: palette.accent_border(),
                    stroke_width: style.border_width.max(1.0),
                    corner_radius: 0.0,
                    z_index: 1,
                });
            }
        } else {
            let Some(area) = self.current() else {
                return scene;
            };

            // Paint the common cell background once and each ruling once.
            // Drawing a filled/stroked rectangle per cell makes the software
            // Windows backend traverse the whole overlay multiple times and
            // alpha-blend shared edges twice. A grid is the same geometry as
            // one background rectangle plus O(rows + columns) straight lines.
            scene.push_shape(OverlayShape::Rect {
                rect: area,
                fill: style.background.with_opacity(0.55),
                stroke: style.border_color,
                stroke_width: style.border_width,
                corner_radius: 0.0,
                z_index: 0,
            });

            Self::push_rulings(
                &mut scene,
                area,
                self.layout.rows,
                self.layout.cols,
                style.border_color,
                style.border_width,
                1,
            );

            let text_style = LabelStyle {
                background: Color::TRANSPARENT,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                border_radius: 0.0,
                ..style.clone()
            };
            if preview_second_layer {
                self.push_second_layer_preview(&mut scene, area, &style, palette);
            } else {
                for (index, key) in self.layout.keys.iter().copied().enumerate() {
                    let Some(rect) = area.subdivision(self.layout.rows, self.layout.cols, index)
                    else {
                        break;
                    };
                    let text = key.to_string();
                    let label_style = text_style.fit_to(&text, rect);
                    scene.push_label(
                        OverlayLabel::new(text, rect, label_style)
                            .fitted()
                            .with_z_index(2),
                    );
                }
            }
        }

        scene.backdrop = Some(Color::rgba(0, 0, 0, 0x40));
        scene.clip = self.root();
        scene
    }

    fn redraw(&self, palette: &Palette) -> CommandBatch {
        CommandBatch::one(Command::show_overlay(self.scene(palette)))
    }

    fn toggle_cursor_follow(&mut self, palette: &Palette) -> CommandBatch {
        self.cursor_follow_selection = !self.cursor_follow_selection;
        let mut commands = CommandBatch::new();
        if self.cursor_follow_selection
            && let Some(area) = self.current()
        {
            commands.push(Command::warp_to(area.center()));
        }
        commands.extend(self.redraw(palette));
        commands
    }

    fn reset(&mut self, bounds: Rect) {
        self.stack.clear();
        self.stack.push(bounds);
        self.path.clear();
        self.terminal = false;
        self.finished = false;
        self.cursor_follow_selection = self.default_cursor_follow_selection;
    }

    fn retarget(&mut self, bounds: Rect, preserve: bool, palette: &Palette) -> CommandBatch {
        let path = if preserve {
            self.path.clone()
        } else {
            SmallVec::new()
        };
        let was_finished = preserve && self.finished;
        let follow = self.cursor_follow_selection;
        self.reset(bounds);
        if preserve {
            self.cursor_follow_selection = follow;
            for index in path {
                let Some(cell) = self
                    .current()
                    .and_then(|area| area.subdivision(self.layout.rows, self.layout.cols, index))
                else {
                    break;
                };
                self.stack.push(cell);
                self.path.push(index);
            }
            self.terminal = self.depth() >= self.max_depth;
            self.finished = was_finished;
        }
        let mut commands =
            CommandBatch::one(Command::warp_to(self.current().unwrap_or(bounds).center()));
        commands.extend(self.redraw(palette));
        commands
    }

    fn select(&mut self, index: usize, cell: Rect, palette: &Palette) -> CommandBatch {
        self.stack.push(cell);
        self.path.push(index);
        if self.depth() >= self.max_depth {
            self.terminal = true;
        }

        if self.terminal {
            let mut commands = CommandBatch::two(
                Command::warp_to(cell.center()),
                Command::show_overlay(self.scene(palette)),
            );
            commands.push(Command::FinishMode {
                cause: FinishCause::Selection,
            });
            return commands;
        }

        let mut commands = CommandBatch::new();
        if self.cursor_follow_selection {
            commands.push(Command::warp_to(cell.center()));
        }
        commands.extend(self.redraw(palette));
        commands
    }

    fn commit_current(&self, palette: &Palette) -> CommandBatch {
        let Some(area) = self.current() else {
            return self.cancel();
        };
        let mut commands = CommandBatch::two(
            Command::warp_to(area.center()),
            Command::show_overlay(self.scene(palette)),
        );
        commands.push(Command::FinishMode {
            cause: FinishCause::Selection,
        });
        commands
    }

    fn cancel(&self) -> CommandBatch {
        CommandBatch::two(
            Command::HideOverlay,
            Command::SwitchMode(self.return_mode.clone()),
        )
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> CommandBatch {
        match key.as_str() {
            "esc" => return self.cancel(),
            "enter" => return self.commit_current(ctx.palette),
            "backspace" | "tab" => {
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
                if let Some(root) = self.root() {
                    self.reset(root);
                    return self.redraw(ctx.palette);
                }
                return self.cancel();
            }
            _ => {}
        }

        if self.terminal || self.finished {
            return CommandBatch::new();
        }
        let Some(key) = key.as_char() else {
            return CommandBatch::new();
        };
        let Some(index) = self
            .layout
            .keys
            .iter()
            .position(|candidate| *candidate == key)
        else {
            return CommandBatch::new();
        };
        let Some(cell) = self
            .current()
            .and_then(|area| area.subdivision(self.layout.rows, self.layout.cols, index))
        else {
            return CommandBatch::new();
        };
        self.select(index, cell, ctx.palette)
    }
}

impl Mode for GridMode {
    fn id(&self) -> ModeId {
        ModeId::grid()
    }

    fn display_name(&self) -> String {
        "Grid".into()
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Activated { previous } => {
                self.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::Restarted => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::FinishRequested { .. } if self.finished => CommandBatch::new(),
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
            ModeEvent::ScreensChanged(_) => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::ScreenRetargeted { screen, preserve } => {
                self.retarget(screen.bounds, *preserve, ctx.palette)
            }
            ModeEvent::PointerMoved(_) if self.root() != Some(ctx.active_bounds()) => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx.palette)
            }
            ModeEvent::Resumed => self.redraw(ctx.palette),
            ModeEvent::Deactivated => {
                self.stack.clear();
                self.path.clear();
                self.terminal = false;
                self.finished = false;
                CommandBatch::new()
            }
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
            _ => CommandBatch::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::{Point, Screen};

    struct Env {
        screens: Vec<Screen>,
        cursor: Point,
        palette: Palette,
        config: Config,
    }

    impl Env {
        fn new() -> Self {
            Self::with(Config::default())
        }

        fn with(config: Config) -> Self {
            Self {
                screens: vec![Screen {
                    bounds: Rect::new(0.0, 0.0, 1000.0, 600.0),
                    work_area: Rect::new(0.0, 0.0, 1000.0, 600.0),
                    is_primary: true,
                    scale: 1.0,
                    name: None,
                }],
                cursor: Point::new(500.0, 300.0),
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

    fn activate(mode: &mut GridMode, env: &Env) -> Vec<Command> {
        mode.handle(&ModeEvent::Activated { previous: None }, &env.ctx())
            .into_iter()
            .collect()
    }

    fn press(mode: &mut GridMode, env: &Env, name: &str) -> Vec<Command> {
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

    fn toggle_follow(mode: &mut GridMode, env: &Env) -> Vec<Command> {
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
            .find_map(|command| match command {
                Command::ShowOverlay(scene) => Some(scene),
                _ => None,
            })
            .expect("expected an overlay")
    }

    #[test]
    fn activation_layers_large_prefixes_over_small_suffix_grids() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        let out = activate(&mut mode, &env);
        let scene = scene_of(&out);

        assert_eq!(scene.clip, Some(env.screens[0].bounds));
        let suffix_count = mode.layout.keys.len() * mode.layout.keys.len();
        assert_eq!(scene.labels.len(), suffix_count + mode.layout.keys.len());
        assert_eq!(
            scene
                .labels
                .iter()
                .take(22)
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "1", "2", "3", "4", "5", "q", "w", "e", "r", "t", "a", "s", "d", "f", "g", "z",
                "x", "c", "v", "b", "1", "2",
            ]
        );
        assert_eq!(
            scene.labels[suffix_count..]
                .iter()
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "1", "2", "3", "4", "5", "q", "w", "e", "r", "t", "a", "s", "d", "f", "g", "z",
                "x", "c", "v", "b",
            ]
        );
        let suffixes = &scene.labels[..suffix_count];
        let prefixes = &scene.labels[suffix_count..];
        assert!(suffixes.iter().all(|label| label.z_index == 2));
        assert!(prefixes.iter().all(|label| label.z_index == 3));
        assert!(prefixes.iter().all(|prefix| {
            suffixes.iter().all(|suffix| {
                prefix.style.font_size > suffix.style.font_size
                    && prefix.style.text_color.a > suffix.style.text_color.a
            })
        }));
    }

    #[test]
    fn scene_uses_one_fill_unique_rulings_and_text_only_labels() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        let out = activate(&mut mode, &env);
        let scene = scene_of(&out);

        let rectangles = scene
            .shapes
            .iter()
            .filter(|shape| matches!(shape, OverlayShape::Rect { .. }))
            .count();
        let lines = scene
            .shapes
            .iter()
            .filter(|shape| matches!(shape, OverlayShape::Line { .. }))
            .count();

        assert_eq!(rectangles, 1);
        assert_eq!(
            lines,
            mode.layout.cols - 1 + mode.layout.rows - 1
                + mode.layout.cols * (mode.layout.cols - 1)
                + mode.layout.rows * (mode.layout.rows - 1)
        );
        assert!(scene.labels.iter().all(|label| {
            label.fit_to_text
                && label.style.background == Color::TRANSPARENT
                && label.style.border_color == Color::TRANSPARENT
                && label.style.border_width == 0.0
        }));
    }

    #[test]
    fn selecting_the_first_key_restores_the_existing_single_layer_view() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);

        let out = press(&mut mode, &env, "1");
        let scene = scene_of(&out);
        assert_eq!(mode.depth(), 1);
        assert_eq!(scene.labels.len(), mode.layout.keys.len());
        assert_eq!(
            scene
                .labels
                .iter()
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            vec![
                "1", "2", "3", "4", "5", "q", "w", "e", "r", "t", "a", "s", "d", "f", "g", "z",
                "x", "c", "v", "b",
            ]
        );
        assert!(
            scene
                .labels
                .iter()
                .all(|label| label.style.text_color == mode.style(&env.palette).text_color)
        );
    }

    #[test]
    fn zero_width_border_does_not_emit_grid_lines() {
        let mut config = Config::default();
        config.grid.ui.label.border_width = 0;
        let env = Env::with(config);
        let mut mode = GridMode::new(&env.config);
        let out = activate(&mut mode, &env);

        assert!(
            scene_of(&out)
                .shapes
                .iter()
                .all(|shape| !matches!(shape, OverlayShape::Line { .. }))
        );
    }

    #[test]
    fn each_selection_narrows_the_grid_and_moves_to_its_centre_by_default() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);

        let first = mode.cells()[0].rect;
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.current(), Some(first));
        assert_eq!(mode.depth(), 1);
        assert!(out.contains(&Command::warp_to(first.center())));

        let second = mode.cells()[0].rect;
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.current(), Some(second));
        assert_eq!(mode.depth(), 2);
        assert!(!mode.terminal);
        assert!(out.contains(&Command::warp_to(second.center())));

        let third = mode.cells()[0].rect;
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.current(), Some(third));
        assert_eq!(mode.depth(), 3);
        assert!(mode.terminal);
        assert!(out.contains(&Command::warp_to(third.center())));
        assert!(
            out.iter()
                .any(|command| matches!(command, Command::ShowOverlay(_)))
        );
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::SwitchMode(_)))
        );
    }

    #[test]
    fn crossing_displays_restarts_grid_on_the_cursor_screen() {
        let mut env = Env::new();
        env.screens.push(Screen {
            bounds: Rect::new(1000.0, 0.0, 800.0, 700.0),
            work_area: Rect::new(1000.0, 0.0, 800.0, 700.0),
            is_primary: false,
            scale: 2.0,
            name: None,
        });
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "1");
        assert_eq!(mode.depth(), 1);

        env.cursor = Point::new(1400.0, 350.0);
        let out = mode.handle(&ModeEvent::PointerMoved(env.cursor), &env.ctx());

        assert_eq!(mode.root(), Some(env.screens[1].bounds));
        assert_eq!(mode.depth(), 0);
        assert_eq!(scene_of(&out).clip, Some(env.screens[1].bounds));
    }

    #[test]
    fn nested_labels_shrink_to_fit_and_remain_visible_on_the_third_layer() {
        let mut config = Config::default();
        config.grid.ui.label.font_size = 20;
        let env = Env::with(config);
        let mut mode = GridMode::new(&env.config);
        let initial = activate(&mut mode, &env);
        let initial_font = scene_of(&initial).labels[0].style.font_size;

        press(&mut mode, &env, "1");
        let third_layer = press(&mut mode, &env, "1");
        let labels = &scene_of(&third_layer).labels;
        assert_eq!(mode.depth(), 2);
        assert_eq!(labels.len(), mode.layout.rows * mode.layout.cols);
        assert!(
            labels
                .iter()
                .all(|label| label.style.font_size < initial_font)
        );
        assert!(labels.iter().all(|label| {
            let width = label.style.font_size * 0.75 + label.style.padding_x * 2.0;
            let height = label.style.font_size * 1.4 + label.style.padding_y * 2.0;
            width <= label.rect.width * 0.8 + f64::EPSILON
                && height <= label.rect.height * 0.8 + f64::EPSILON
        }));
    }

    #[test]
    fn follow_binding_disables_live_cursor_following_for_the_session() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);

        toggle_follow(&mut mode, &env);
        assert!(!mode.cursor_follow_selection);
        let out = press(&mut mode, &env, "1");
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::WarpPointer { .. }))
        );
    }

    #[test]
    fn enabling_follow_immediately_warps_to_the_current_grid_centre() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);
        toggle_follow(&mut mode, &env);
        press(&mut mode, &env, "1");
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
    fn enter_moves_to_the_selected_centre_without_clicking() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);
        toggle_follow(&mut mode, &env);
        press(&mut mode, &env, "1");
        let selected = mode.current().unwrap();

        let out = press(&mut mode, &env, "enter");
        assert!(out.contains(&Command::warp_to(selected.center())));
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
    fn screen_retarget_replays_or_resets_the_grid_path() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "q");
        press(&mut mode, &env, "w");
        assert_eq!(mode.path.as_slice(), [5, 6]);
        let target = Screen {
            bounds: Rect::new(1000.0, 0.0, 1600.0, 900.0),
            work_area: Rect::new(1000.0, 0.0, 1600.0, 900.0),
            is_primary: false,
            scale: 2.0,
            name: None,
        };

        let out = mode.handle(
            &ModeEvent::ScreenRetargeted {
                screen: target.clone(),
                preserve: true,
            },
            &env.ctx(),
        );
        assert_eq!(mode.path.as_slice(), [5, 6]);
        assert_eq!(mode.depth(), 2);
        assert_eq!(scene_of(&out).clip, Some(target.bounds));
        assert!(out.contains(&Command::warp_to(mode.current().unwrap().center())));

        let out = mode.handle(
            &ModeEvent::ScreenRetargeted {
                screen: target.clone(),
                preserve: false,
            },
            &env.ctx(),
        );
        assert!(mode.path.is_empty());
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(target.bounds));
        assert_eq!(scene_of(&out).clip, Some(target.bounds));
    }

    #[test]
    fn backspace_widens_and_space_resets_the_selection() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "q");
        press(&mut mode, &env, "q");
        assert_eq!(mode.depth(), 2);

        press(&mut mode, &env, "backspace");
        assert_eq!(mode.depth(), 1);
        assert!(!mode.terminal);
        press(&mut mode, &env, "space");
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(env.screens[0].bounds));
    }

    #[test]
    fn finish_is_idempotent_and_keep_preserves_the_selected_path() {
        let mut config = Config::default();
        config.grid.lifecycle.after_finish = crate::config::LifecycleAction::Keep;
        let env = Env::with(config);
        let mut mode = GridMode::new(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "1");
        let selected = mode.current();

        let finished = mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Explicit,
            },
            &env.ctx(),
        );
        assert!(mode.finished);
        assert_eq!(mode.current(), selected);
        let scene = scene_of(&finished);
        assert_eq!(scene.labels.len(), mode.layout.rows * mode.layout.cols);
        assert!(
            scene
                .shapes
                .iter()
                .any(|shape| matches!(shape, OverlayShape::Line { .. }))
        );
        assert!(
            !finished
                .iter()
                .any(|command| matches!(command, Command::SwitchMode(_)))
        );

        assert!(
            mode.handle(
                &ModeEvent::FinishRequested {
                    cause: FinishCause::Explicit,
                },
                &env.ctx(),
            )
            .is_empty()
        );

        press(&mut mode, &env, "backspace");
        assert!(!mode.finished);
        assert_eq!(mode.depth(), 0);
    }

    #[test]
    fn default_finish_returns_grid_to_normal() {
        let env = Env::new();
        let mut mode = GridMode::new(&env.config);
        mode.handle(
            &ModeEvent::Activated {
                previous: Some(ModeId::normal()),
            },
            &env.ctx(),
        );
        let out = mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Selection,
            },
            &env.ctx(),
        );
        assert!(out.contains(&Command::SwitchMode(ModeId::normal())));
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::MouseButton { .. }))
        );
    }
}
