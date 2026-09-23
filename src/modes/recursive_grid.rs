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
use crate::api::lifecycle::TargetingLifecycle;
use crate::api::overlay::Color;
use crate::api::presentation::{GridLayout, RecursiveGridView, View};
use crate::api::theme::Palette;

use super::targeting::{Layout, Selection, TargetingController};

pub use super::targeting::LayerSettings;

pub use crate::api::presentation::RecursiveGridStyle as VisualSettings;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub keys: String,
    pub min_size_width: u32,
    pub min_size_height: u32,
    pub max_depth: u32,
    pub cursor_follow_selection: bool,
    pub lifecycle: TargetingLifecycle,
    pub layers: Vec<LayerSettings>,
    pub ui: VisualSettings,
}

pub struct RecursiveGridMode {
    controller: TargetingController,
    ui: VisualSettings,
}

impl RecursiveGridMode {
    pub fn new(settings: Settings) -> Self {
        let controller = TargetingController::new(
            Layout {
                rows: settings.grid_rows.max(1) as usize,
                cols: settings.grid_cols.max(1) as usize,
                keys: settings.keys.chars().collect(),
            },
            &settings.layers,
            settings.max_depth,
            Some((
                settings.min_size_width as f64,
                settings.min_size_height as f64,
            )),
            settings.cursor_follow_selection,
            settings.lifecycle,
        );
        Self {
            controller,
            ui: settings.ui,
        }
    }

    fn depth(&self) -> u32 {
        self.controller.session.depth()
    }

    fn current(&self) -> Option<Rect> {
        self.controller.session.current()
    }

    /// Layout for `depth`, applying any `[recursive_grid.layers]` override.
    fn layout_at(&self, depth: u32) -> &Layout {
        self.controller.layout_at(depth)
    }

    /// Cells of the current area, paired with their keys.
    #[cfg(test)]
    fn cells(&self) -> Vec<(char, Rect)> {
        if self.controller.session.terminal {
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
        self.controller.can_descend()
    }

    fn view(&self) -> RecursiveGridView<'_> {
        let layout = self.layout_at(self.depth());
        let next = self.layout_at(self.depth() + 1);
        RecursiveGridView {
            layout: GridLayout {
                rows: layout.rows,
                cols: layout.cols,
                keys: &layout.keys,
            },
            next_layout: GridLayout {
                rows: next.rows,
                cols: next.cols,
                keys: &next.keys,
            },
            ui: &self.ui,
            current: self.current(),
            root: self.controller.session.root(),
            terminal: self.controller.session.terminal,
            can_descend: self.can_descend(),
        }
    }

    fn redraw(&self, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::one(ctx.present(View::RecursiveGrid(self.view())))
    }

    fn cancel(&self) -> CommandBatch {
        CommandBatch::two(
            Command::HideOverlay,
            Command::SwitchMode(self.controller.session.return_mode.clone()),
        )
    }

    fn toggle_cursor_follow(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        let mut commands = CommandBatch::new();
        if let Some(area) = self.controller.session.toggle_cursor_follow() {
            commands.push(Command::warp_to(area.center()));
        }
        commands.extend(self.redraw(ctx));
        commands
    }

    fn reset(&mut self, bounds: Rect) {
        self.controller.reset(bounds);
    }

    fn retarget(&mut self, bounds: Rect, preserve: bool, ctx: &HostContext<'_>) -> CommandBatch {
        let point = self.controller.retarget(bounds, preserve);
        let mut commands = CommandBatch::one(Command::warp_to(point));
        commands.extend(self.redraw(ctx));
        commands
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> CommandBatch {
        match self.controller.input(key, ctx.active_bounds()) {
            Selection::Ignored => CommandBatch::new(),
            Selection::Cancel => self.cancel(),
            Selection::Changed => self.redraw(ctx),
            Selection::Follow(point) => {
                let mut commands = CommandBatch::one(Command::warp_to(point));
                commands.extend(self.redraw(ctx));
                commands
            }
            Selection::Commit(point) => {
                let mut commands = CommandBatch::one(Command::warp_to(point));
                commands.extend(self.redraw(ctx));
                commands.push(Command::FinishMode {
                    cause: FinishCause::Selection,
                });
                commands
            }
        }
    }
}

impl Mode for RecursiveGridMode {
    fn id(&self) -> ModeId {
        ModeId::recursive_grid()
    }

    fn display_name(&self) -> String {
        "Recursive Grid".into()
    }

    fn claims_key(&self, key: &Key) -> bool {
        self.controller.claims_key(key)
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
        Some(palette.accent_alt)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Pushed { previous } => {
                self.controller.session.return_mode = previous.clone();
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Activated { previous } => {
                self.controller.session.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Restarted => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::FinishRequested { .. } if self.controller.session.finished => {
                CommandBatch::new()
            }
            ModeEvent::FinishRequested { .. } => {
                self.controller.session.finished = true;
                let mut commands = self.redraw(ctx);
                commands.extend(super::targeting::lifecycle_commands(
                    &self.controller.session.lifecycle.after_finish,
                    &self.controller.session.return_mode,
                ));
                commands
            }
            ModeEvent::Clicked { .. } => super::targeting::lifecycle_commands(
                &self.controller.session.lifecycle.after_click,
                &self.controller.session.return_mode,
            ),
            ModeEvent::Deactivated => {
                self.controller.session.stack.clear();
                self.controller.session.path.clear();
                self.controller.session.terminal = false;
                self.controller.session.finished = false;
                CommandBatch::new()
            }
            ModeEvent::ScreensChanged(_) => {
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::ScreenRetargeted { screen, preserve } => {
                self.retarget(screen.bounds, *preserve, ctx)
            }
            ModeEvent::PointerMoved(_)
                if self.controller.session.stack.first().copied() != Some(ctx.active_bounds()) =>
            {
                self.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Resumed => self.redraw(ctx),
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
    impl RecursiveGridMode {
        fn scene(&self, palette: &Palette) -> OverlayScene {
            self.view().scene(palette)
        }
    }

    use crate::api::geometry::Screen;
    use crate::config::{Config, GridLayer};

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
        config.recursive_grid.ui.label.font_size = 20;
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
                presenter: &crate::presentation::COMPOSER,
                screens: &self.screens,
                cursor: self.cursor,
                focused_app: None,
                palette: &self.palette,
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
    fn default_letters_scale_with_recursive_cells() {
        let env = Env::with(Config::default());
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        let initial = activate(&mut mode, &env);
        let initial_size = scene_of(&initial).labels[0].style.font_size;
        assert_eq!(initial_size, 300.0 * 0.40);
        assert!(!scene_of(&initial).labels[0].style.bold);
        let nested = press(&mut mode, &env, "s");
        let nested_size = scene_of(&nested).labels[0].style.font_size;
        assert_eq!(nested_size, 100.0 * 0.40);
    }

    #[test]
    fn configured_font_size_reaches_recursive_grid_letters() {
        for size in [17, 20, 36] {
            let config =
                Config::parse(&format!("[recursive_grid.ui]\nfont_size = {size}")).unwrap();
            let env = Env::with(config);
            let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
            let out = activate(&mut mode, &env);
            let labels = &scene_of(&out).labels;
            assert_eq!(labels.len(), 9);
            assert!(
                labels
                    .iter()
                    .all(|label| label.style.font_size == f64::from(size))
            );
        }
    }

    #[test]
    fn product_defaults_keep_the_three_by_three_recursive_grid_with_follow_enabled() {
        let env = Env::with(Config::default());
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        let out = activate(&mut mode, &env);
        assert_eq!(scene_of(&out).labels.len(), 9);
        assert!(mode.controller.session.cursor_follow_selection);

        toggle_follow(&mut mode, &env);
        assert!(!mode.controller.session.cursor_follow_selection);
        let out = press(&mut mode, &env, "q");
        assert!(
            !out.iter()
                .any(|command| matches!(command, Command::WarpPointer { .. }))
        );
    }

    #[test]
    fn enabling_follow_immediately_warps_to_the_current_recursive_cell_centre() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);
        toggle_follow(&mut mode, &env);
        press(&mut mode, &env, "g");
        let selected = mode.current().unwrap();
        let depth = mode.depth();

        let out = toggle_follow(&mut mode, &env);

        assert!(mode.controller.session.cursor_follow_selection);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);
        assert_eq!(mode.current(), Some(env.screens[0].bounds));
        assert_eq!(mode.depth(), 0);
    }

    #[test]
    fn default_grid_is_three_by_three_with_nine_keys() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);
        assert_eq!(mode.cells().len(), 9);
    }

    #[test]
    fn selecting_a_cell_narrows_the_area() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "t");
        press(&mut mode, &env, "g");
        assert_eq!(mode.controller.session.path.as_slice(), [1, 4]);
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
        assert_eq!(mode.controller.session.path.as_slice(), [1, 4]);
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
        assert!(mode.controller.session.path.is_empty());
        assert_eq!(mode.depth(), 0);
        assert_eq!(mode.current(), Some(target.bounds));
    }

    #[test]
    fn backspace_widens_and_then_dismisses_at_the_root() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);

        let out = press(&mut mode, &env, "g");
        assert!(out.iter().any(|c| matches!(c, Command::WarpPointer { .. })));
        assert!(mode.controller.session.terminal);
        assert!(!out.contains(&Command::SwitchMode(ModeId::idle())));
    }

    #[test]
    fn min_cell_size_stops_further_subdivision() {
        let mut config = legacy_config();
        config.recursive_grid.min_size_width = 200;
        config.recursive_grid.min_size_height = 200;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);

        // The selected 300×300 cell cannot produce another 3×3 layer whose
        // cells meet the 200px minimum, so it is terminal immediately.
        let out = press(&mut mode, &env, "g");
        assert!(out.iter().any(|c| matches!(c, Command::WarpPointer { .. })));
        assert_eq!(mode.depth(), 1);
        assert!(mode.controller.session.terminal);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "g");
        assert_eq!(mode.depth(), 1);

        env.cursor = Point::new(1500.0, 400.0);
        let out = mode.handle(&ModeEvent::PointerMoved(env.cursor), &env.ctx());

        assert_eq!(
            mode.controller.session.stack.as_slice(),
            [env.screens[1].bounds]
        );
        assert_eq!(mode.depth(), 0);
        assert_eq!(scene_of(&out).clip, Some(env.screens[1].bounds));
    }

    #[test]
    fn labels_shrink_before_they_reach_the_autohide_threshold() {
        let mut config = legacy_config();
        config.recursive_grid.ui.label.font_size = 20;
        config.recursive_grid.ui.label_min_font_size = 6;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
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
        assert!(!mode.controller.session.finished);
        assert_eq!(mode.current(), selected);

        let continued = press(&mut mode, &env, "g");
        assert!(!mode.controller.session.finished);
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
        let mut mode = crate::app::mode_catalog::recursive_grid(&env.config);
        mode.handle(
            &ModeEvent::Activated {
                previous: Some(ModeId::normal()),
            },
            &env.ctx(),
        );
        press(&mut mode, &env, "g");
        mode.handle(&ModeEvent::Restarted, &env.ctx());

        assert_eq!(mode.depth(), 0);
        assert!(!mode.controller.session.finished);
        assert_eq!(mode.controller.session.return_mode, ModeId::normal());
    }
}
