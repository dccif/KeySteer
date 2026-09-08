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
use crate::api::lifecycle::TargetingLifecycle;
use crate::api::overlay::Color;
use crate::api::presentation::{GridLayout, GridView, View};
use crate::api::theme::Palette;
use smallvec::SmallVec;

use super::targeting::TargetingSession;

pub use crate::api::presentation::GridStyle as VisualSettings;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub keys: String,
    pub max_depth: u32,
    pub cursor_follow_selection: bool,
    pub lifecycle: TargetingLifecycle,
    pub ui: VisualSettings,
}

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
    ui: VisualSettings,
    session: TargetingSession,
}

impl GridMode {
    pub fn new(settings: Settings) -> Self {
        Self {
            layout: Layout {
                rows: settings.grid_rows.max(1) as usize,
                cols: settings.grid_cols.max(1) as usize,
                keys: settings.keys.chars().collect(),
            },
            max_depth: settings.max_depth.max(1),
            ui: settings.ui,
            session: TargetingSession::new(settings.cursor_follow_selection, settings.lifecycle),
        }
    }

    fn depth(&self) -> u32 {
        self.session.depth()
    }

    fn current(&self) -> Option<Rect> {
        self.session.current()
    }

    fn root(&self) -> Option<Rect> {
        self.session.root()
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

    fn view(&self) -> GridView<'_> {
        GridView {
            layout: GridLayout {
                rows: self.layout.rows,
                cols: self.layout.cols,
                keys: &self.layout.keys,
            },
            ui: &self.ui,
            current: self.current(),
            root: self.root(),
            terminal: self.session.terminal,
            depth: self.depth(),
            max_depth: self.max_depth,
        }
    }

    fn redraw(&self, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::one(ctx.present(View::Grid(self.view())))
    }

    fn toggle_cursor_follow(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        let mut commands = CommandBatch::new();
        if let Some(area) = self.session.toggle_cursor_follow() {
            commands.push(Command::warp_to(area.center()));
        }
        commands.extend(self.redraw(ctx));
        commands
    }

    fn reset(&mut self, bounds: Rect) {
        self.session.reset(bounds);
    }

    fn retarget(&mut self, bounds: Rect, preserve: bool, ctx: &HostContext<'_>) -> CommandBatch {
        let path = if preserve {
            self.session.path.clone()
        } else {
            SmallVec::new()
        };
        let was_finished = preserve && self.session.finished;
        let follow = self.session.cursor_follow_selection;
        self.reset(bounds);
        if preserve {
            self.session.cursor_follow_selection = follow;
            for index in path {
                let Some(cell) = self
                    .current()
                    .and_then(|area| area.subdivision(self.layout.rows, self.layout.cols, index))
                else {
                    break;
                };
                self.session.stack.push(cell);
                self.session.path.push(index);
            }
            self.session.terminal = self.depth() >= self.max_depth;
            self.session.finished = was_finished;
        }
        let mut commands =
            CommandBatch::one(Command::warp_to(self.current().unwrap_or(bounds).center()));
        commands.extend(self.redraw(ctx));
        commands
    }

    fn select(&mut self, index: usize, cell: Rect, ctx: &HostContext<'_>) -> CommandBatch {
        self.session.stack.push(cell);
        self.session.path.push(index);
        if self.depth() >= self.max_depth {
            self.session.terminal = true;
        }

        if self.session.terminal {
            let mut commands = CommandBatch::two(
                Command::warp_to(cell.center()),
                ctx.present(View::Grid(self.view())),
            );
            commands.push(Command::FinishMode {
                cause: FinishCause::Selection,
            });
            return commands;
        }

        let mut commands = CommandBatch::new();
        if self.session.cursor_follow_selection {
            commands.push(Command::warp_to(cell.center()));
        }
        commands.extend(self.redraw(ctx));
        commands
    }

    fn commit_current(&self, ctx: &HostContext<'_>) -> CommandBatch {
        let Some(area) = self.current() else {
            return self.cancel();
        };
        let mut commands = CommandBatch::two(
            Command::warp_to(area.center()),
            ctx.present(View::Grid(self.view())),
        );
        commands.push(Command::FinishMode {
            cause: FinishCause::Selection,
        });
        commands
    }

    fn cancel(&self) -> CommandBatch {
        CommandBatch::two(
            Command::HideOverlay,
            Command::SwitchMode(self.session.return_mode.clone()),
        )
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> CommandBatch {
        match key.as_str() {
            "esc" => return self.cancel(),
            "enter" => return self.commit_current(ctx),
            "backspace" | "tab" => {
                if self.session.stack.len() <= 1 {
                    return self.cancel();
                }
                self.session.stack.pop();
                self.session.path.pop();
                self.session.terminal = false;
                self.session.finished = false;
                return self.redraw(ctx);
            }
            "space" => {
                if let Some(root) = self.root() {
                    self.reset(root);
                    return self.redraw(ctx);
                }
                return self.cancel();
            }
            _ => {}
        }

        if self.session.terminal || self.session.finished {
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
        self.select(index, cell, ctx)
    }
}

impl Mode for GridMode {
    fn id(&self) -> ModeId {
        ModeId::grid()
    }

    fn display_name(&self) -> String {
        "Grid".into()
    }

    fn claims_key(&self, key: &Key) -> bool {
        key.as_char()
            .is_some_and(|character| self.layout.keys.contains(&character))
    }

    fn available_keys(&self) -> Vec<(String, String)> {
        // Selection characters are already drawn in the grid itself.
        [
            ("esc", "cancel"),
            ("enter", "select"),
            ("backspace", "back"),
            ("tab", "back"),
            ("space", "restart"),
        ]
        .into_iter()
        .map(|(key, action)| (key.into(), action.into()))
        .collect()
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Activated { previous } => {
                self.session.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Restarted => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::FinishRequested { .. } if self.session.finished => CommandBatch::new(),
            ModeEvent::FinishRequested { .. } => {
                self.session.finished = true;
                let mut commands = self.redraw(ctx);
                commands.extend(super::targeting::lifecycle_commands(
                    &self.session.lifecycle.after_finish,
                    &self.session.return_mode,
                ));
                commands
            }
            ModeEvent::Clicked { .. } => super::targeting::lifecycle_commands(
                &self.session.lifecycle.after_click,
                &self.session.return_mode,
            ),
            ModeEvent::ScreensChanged(_) => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::ScreenRetargeted { screen, preserve } => {
                self.retarget(screen.bounds, *preserve, ctx)
            }
            ModeEvent::PointerMoved(_) if self.root() != Some(ctx.active_bounds()) => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Resumed => self.redraw(ctx),
            ModeEvent::Deactivated => {
                self.session.stack.clear();
                self.session.path.clear();
                self.session.terminal = false;
                self.session.finished = false;
                CommandBatch::new()
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } if matches!(binding.as_ref(), Binding::ToggleCursorFollowSelection) => {
                self.toggle_cursor_follow(ctx)
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
    use crate::api::overlay::{LabelStyle, OverlayLabel, OverlayScene, OverlayShape};
    use crate::api::style::LabelUi;
    use crate::api::theme::ThemedColor;
    impl GridMode {
        fn scene(&self, palette: &Palette) -> OverlayScene {
            self.view().scene(palette)
        }
    }

    use crate::api::geometry::{Point, Screen};
    use crate::config::Config;

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
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: None,
                palette: &self.palette,
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
    fn activation_submits_borrowed_state_to_injected_presenter() {
        use crate::api::presentation::{HintContent, Presenter, View, VisualLayerPlan};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Recorder {
            calls: AtomicUsize,
            keys: usize,
        }
        impl Presenter for Recorder {
            fn compose(&self, view: View<'_>, _: &HostContext<'_>) -> OverlayScene {
                let View::Grid(view) = view else {
                    panic!("expected Grid view")
                };
                assert_eq!(view.layout.keys.as_ptr() as usize, self.keys);
                assert_eq!(view.depth, 0);
                assert!(view.root.is_some());
                self.calls.fetch_add(1, Ordering::Relaxed);
                // A substituted composer controls the entire rendered content.
                let mut scene = OverlayScene::new();
                scene.clip = Some(Rect::new(11.0, 22.0, 33.0, 44.0));
                scene
            }
            fn prepare_hints(
                &self,
                _: HintContent<'_>,
                _: &mut VisualLayerPlan,
                _: &mut Option<Vec<(usize, Rect)>>,
                _: &HostContext<'_>,
            ) {
                panic!("unexpected hint layout")
            }
        }
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        let recorder = Recorder {
            calls: AtomicUsize::new(0),
            keys: mode.view().layout.keys.as_ptr() as usize,
        };
        let mut context = env.ctx();
        context.presenter = &recorder;
        let commands = mode.handle(&ModeEvent::Activated { previous: None }, &context);
        assert_eq!(recorder.calls.load(Ordering::Relaxed), 1);
        let commands: Vec<_> = commands.into_iter().collect();
        let scene = scene_of(&commands);
        assert_eq!(scene.clip, Some(Rect::new(11.0, 22.0, 33.0, 44.0)));
        assert!(scene.labels.is_empty() && scene.shapes.is_empty());
    }

    #[test]
    fn activation_layers_large_prefixes_over_small_suffix_grids() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
                .all(|label| label.style.text_color == mode.view().style(&env.palette).text_color)
        );
    }

    #[test]
    fn zero_width_border_does_not_emit_grid_lines() {
        let mut config = Config::default();
        config.grid.ui.label.border_width = 0;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        assert!(!mode.session.terminal);
        assert!(out.contains(&Command::warp_to(second.center())));

        let third = mode.cells()[0].rect;
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.current(), Some(third));
        assert_eq!(mode.depth(), 3);
        assert!(mode.session.terminal);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);

        toggle_follow(&mut mode, &env);
        assert!(!mode.session.cursor_follow_selection);
        let out = press(&mut mode, &env, "1");
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::WarpPointer { .. }))
        );
    }

    #[test]
    fn enabling_follow_immediately_warps_to_the_current_grid_centre() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);
        toggle_follow(&mut mode, &env);
        press(&mut mode, &env, "1");
        let selected = mode.current().unwrap();
        let depth = mode.depth();

        let out = toggle_follow(&mut mode, &env);

        assert!(mode.session.cursor_follow_selection);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "q");
        press(&mut mode, &env, "w");
        assert_eq!(mode.session.path.as_slice(), [5, 6]);
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
        assert_eq!(mode.session.path.as_slice(), [5, 6]);
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
        assert!(mode.session.path.is_empty());
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(target.bounds));
        assert_eq!(scene_of(&out).clip, Some(target.bounds));
    }

    #[test]
    fn backspace_widens_and_space_resets_the_selection() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "q");
        press(&mut mode, &env, "q");
        assert_eq!(mode.depth(), 2);

        press(&mut mode, &env, "backspace");
        assert_eq!(mode.depth(), 1);
        assert!(!mode.session.terminal);
        press(&mut mode, &env, "space");
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(env.screens[0].bounds));
    }

    #[test]
    fn finish_is_idempotent_and_keep_preserves_the_selected_path() {
        let mut config = Config::default();
        config.grid.lifecycle.after_finish = crate::config::LifecycleAction::Keep;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "1");
        let selected = mode.current();

        let finished = mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Explicit,
            },
            &env.ctx(),
        );
        assert!(mode.session.finished);
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
        assert!(!mode.session.finished);
        assert_eq!(mode.depth(), 0);
    }

    #[test]
    fn default_finish_returns_grid_to_normal() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::grid(&env.config);
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
