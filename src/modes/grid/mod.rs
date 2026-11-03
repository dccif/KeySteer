//! Ordinary, recursive and bisect grids share selection and lifecycle handling.
//! Their geometry is precompiled by the catalog; each keeps its existing view.

use crate::api::binding::Binding;
use crate::api::command::{Command, CommandBatch, FinishCause, HostContext, Mode, ModeEvent};
use crate::api::geometry::Rect;
use crate::api::input::{Key, KeyState, ModeId};
use crate::api::overlay::Color;
use crate::api::presentation::View;
use crate::api::theme::Palette;

use super::targeting::{Selection, TargetingController};
mod variants;
pub(crate) use variants::GridPresentation;

pub(crate) struct GridMode<G: GridPresentation> {
    controller: TargetingController<G>,
    style: G::Style,
}

impl<G: GridPresentation> GridMode<G> {
    pub(crate) fn new(controller: TargetingController<G>, style: G::Style) -> Self {
        Self { controller, style }
    }

    fn view(&self) -> View<'_> {
        self.controller.geometry.view(
            &self.controller.session,
            self.controller.max_depth,
            &self.style,
        )
    }

    fn redraw(&self, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::one(ctx.present(self.view()))
    }

    fn toggle_cursor_follow(&mut self, ctx: &HostContext<'_>) -> CommandBatch {
        let mut commands = CommandBatch::new();
        if let Some(area) = self.controller.session.toggle_cursor_follow() {
            commands.push(Command::warp_to(area.center()));
        }
        commands.extend(self.redraw(ctx));
        commands
    }

    fn cancel(&self) -> CommandBatch {
        CommandBatch::two(
            Command::HideOverlay,
            Command::SwitchMode(self.controller.session.return_mode.clone()),
        )
    }

    fn retarget(&mut self, bounds: Rect, preserve: bool, ctx: &HostContext<'_>) -> CommandBatch {
        let point = self.controller.retarget(bounds, preserve);
        CommandBatch::two(Command::warp_to(point), ctx.present(self.view()))
    }

    fn key_down(&mut self, key: &Key, ctx: &HostContext<'_>) -> CommandBatch {
        let selection = self.controller.input(key, ctx.active_bounds());
        self.apply_selection(selection, ctx)
    }

    fn apply_selection(&mut self, selection: Selection, ctx: &HostContext<'_>) -> CommandBatch {
        match selection {
            Selection::Ignored => CommandBatch::new(),
            Selection::Cancel => self.cancel(),
            Selection::Changed => self.redraw(ctx),
            Selection::Follow(point) => {
                CommandBatch::two(Command::warp_to(point), ctx.present(self.view()))
            }
            Selection::Commit(point) => {
                let mut commands =
                    CommandBatch::two(Command::warp_to(point), ctx.present(self.view()));
                commands.push(Command::FinishMode {
                    cause: FinishCause::Selection,
                });
                commands
            }
        }
    }
}

impl<G: GridPresentation> Mode for GridMode<G> {
    fn id(&self) -> ModeId {
        G::id()
    }

    fn display_name(&self) -> String {
        G::NAME.into()
    }

    fn claims_key(&self, key: &Key) -> bool {
        self.controller.claims_key(key)
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(G::indicator_color(palette))
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        match event {
            ModeEvent::Pushed { previous } => {
                self.controller.session.return_mode = previous.clone();
                self.controller.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Activated { previous } => {
                self.controller.session.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.controller.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Restarted => {
                self.controller.reset(ctx.active_bounds());
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
            ModeEvent::ScreensChanged(_) => {
                self.controller.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::ScreenRetargeted { screen, preserve } => {
                self.retarget(screen.bounds, *preserve, ctx)
            }
            ModeEvent::PointerMoved(_)
                if self.controller.session.root() != Some(ctx.active_bounds()) =>
            {
                self.controller.reset(ctx.active_bounds());
                self.redraw(ctx)
            }
            ModeEvent::Resumed => self.redraw(ctx),
            ModeEvent::Deactivated => {
                self.controller.session.stack.clear();
                self.controller.session.path.clear();
                self.controller.session.terminal = false;
                self.controller.session.finished = false;
                CommandBatch::new()
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } if matches!(binding.as_ref(), Binding::ToggleCursorFollowSelection) => {
                self.toggle_cursor_follow(ctx)
            }
            ModeEvent::Binding {
                binding,
                state: KeyState::Down,
                ..
            } => {
                let selection = match binding.as_ref() {
                    Binding::ScanRegion(target) => {
                        return self
                            .controller
                            .session
                            .current()
                            .filter(|area| !area.is_empty())
                            .map_or_else(CommandBatch::new, |bounds| {
                                CommandBatch::one(Command::PushModeInRegion {
                                    mode: target.mode(),
                                    bounds,
                                })
                            });
                    }
                    Binding::Bisect(direction) => self.controller.cut(*direction),
                    Binding::SelectionReset => self.controller.reset_selection(ctx.active_bounds()),
                    Binding::SelectionParent => self.controller.parent(),
                    Binding::SelectionAccept => self
                        .controller
                        .session
                        .current()
                        .map_or(Selection::Cancel, |area| Selection::Commit(area.center())),
                    Binding::SelectionCancel => Selection::Cancel,
                    _ => return CommandBatch::new(),
                };
                self.apply_selection(selection, ctx)
            }
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                ..
            } if self.claims_key(key) => self.key_down(key, ctx),
            _ => CommandBatch::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::overlay::{OverlayScene, OverlayShape};

    use crate::api::geometry::{Point, Screen};
    use crate::config::Config;

    fn cells<G: GridPresentation + super::super::targeting::LetterGeometry>(
        mode: &GridMode<G>,
    ) -> Vec<Rect> {
        let Some(area) = mode.controller.session.current() else {
            return Vec::new();
        };
        mode.controller
            .layout_at(0)
            .keys
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                area.subdivision(
                    mode.controller.layout_at(0).rows,
                    mode.controller.layout_at(0).cols,
                    index,
                )
            })
            .collect()
    }

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

    fn activate<G: GridPresentation>(mode: &mut GridMode<G>, env: &Env) -> Vec<Command> {
        mode.handle(&ModeEvent::Activated { previous: None }, &env.ctx())
            .into_iter()
            .collect()
    }

    fn press<G: GridPresentation>(mode: &mut GridMode<G>, env: &Env, name: &str) -> Vec<Command> {
        let key = Key::new(name).unwrap();
        let binding = env.config.grid.bindings.get(name);
        if let Some(binding) = binding {
            return mode
                .handle(
                    &ModeEvent::Binding {
                        binding: binding.clone().into(),
                        state: KeyState::Down,
                        key,
                    },
                    &env.ctx(),
                )
                .into_iter()
                .collect();
        }
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

    fn toggle_follow<G: GridPresentation>(mode: &mut GridMode<G>, env: &Env) -> Vec<Command> {
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

    fn bisect_action<G: GridPresentation>(
        mode: &mut GridMode<G>,
        env: &Env,
        action: &str,
    ) -> Vec<Command> {
        mode.handle(
            &ModeEvent::Binding {
                binding: Binding::parse(action).unwrap().into(),
                state: KeyState::Down,
                key: Key::new("f8").unwrap(),
            },
            &env.ctx(),
        )
        .into_iter()
        .collect()
    }

    #[test]
    fn ui_hint_uses_the_current_region_for_all_grid_variants() {
        fn assert_region<G: GridPresentation>(mode: &mut GridMode<G>, env: &Env, area: Rect) {
            assert_eq!(mode.controller.session.current(), Some(area));
            for (key, target) in [("u", ModeId::ui_hint()), ("i", ModeId::text_select())] {
                assert_eq!(
                    press(mode, env, key),
                    vec![Command::PushModeInRegion {
                        mode: target,
                        bounds: area
                    }]
                );
            }
        }

        let env = Env::new();
        let screen = env.screens[0].bounds;
        let mut grid = crate::app::mode_catalog::grid(&env.config);
        let mut recursive = crate::app::mode_catalog::recursive_grid(&env.config);
        let mut bisect = crate::app::mode_catalog::bisect(&env.config);
        activate(&mut grid, &env);
        activate(&mut recursive, &env);
        activate(&mut bisect, &env);
        assert_region(&mut grid, &env, screen);
        assert_region(&mut recursive, &env, screen);
        assert_region(&mut bisect, &env, screen);

        press(&mut grid, &env, "1");
        assert_region(&mut grid, &env, Rect::new(0.0, 0.0, 200.0, 150.0));
        press(&mut recursive, &env, "q");
        assert_region(&mut recursive, &env, screen.subdivision(3, 3, 0).unwrap());
        bisect_action(&mut bisect, &env, "bisect_left");
        bisect_action(&mut bisect, &env, "bisect_down");
        assert_region(&mut bisect, &env, Rect::new(0.0, 300.0, 500.0, 300.0));
    }

    #[test]
    fn bisect_halves_one_axis_and_shares_unlabelled_region_styling() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::bisect(&env.config);
        let out = activate(&mut mode, &env);
        assert_eq!(mode.id(), ModeId::bisect());
        assert!(!mode.claims_key(&Key::new("h").unwrap())); // All cuts use bindings.
        assert!(scene_of(&out).labels.is_empty());
        assert_eq!(scene_of(&out).shapes.len(), 3);
        for (action, expected) in [
            ("bisect_left", Rect::new(0., 0., 500., 600.)),
            ("bisect_down", Rect::new(0., 300., 500., 300.)),
            ("bisect_right", Rect::new(250., 300., 250., 300.)),
            ("bisect_up", Rect::new(250., 300., 250., 150.)),
        ] {
            let out = bisect_action(&mut mode, &env, action);
            assert_eq!(mode.controller.session.current(), Some(expected));
            assert!(out.contains(&Command::warp_to(expected.center())));
            assert!(scene_of(&out).labels.is_empty());
            assert_eq!(scene_of(&out).clip, Some(env.screens[0].bounds));
        }
        let mut recursive = crate::app::mode_catalog::recursive_grid(&env.config);
        activate(&mut recursive, &env);
        let bisect = mode.redraw(&env.ctx());
        let recursive = recursive.redraw(&env.ctx());
        // Same fill/outline style, with only the region geometry differing.
        match (
            &scene_of(&bisect).shapes[0],
            &scene_of(&recursive).shapes[0],
        ) {
            (
                OverlayShape::Rect {
                    fill: a,
                    stroke: b,
                    stroke_width: c,
                    ..
                },
                OverlayShape::Rect {
                    fill: x,
                    stroke: y,
                    stroke_width: z,
                    ..
                },
            ) => assert_eq!((a, b, c), (x, y, z)),
            _ => panic!("region outline missing"),
        }
    }

    #[test]
    fn bisect_limits_each_axis_and_does_not_record_rejected_cuts() {
        let mut config = Config::default();
        config.bisect.min_size_width = 250;
        config.bisect.min_size_height = 150;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::bisect(&env.config);
        activate(&mut mode, &env);
        bisect_action(&mut mode, &env, "bisect_left");
        bisect_action(&mut mode, &env, "bisect_right");
        assert!(bisect_action(&mut mode, &env, "bisect_left").is_empty());
        assert_eq!(mode.controller.session.depth(), 2);
        assert_eq!(
            mode.controller
                .geometry
                .axes(&mode.controller.session, mode.controller.max_depth),
            (false, true)
        );
        bisect_action(&mut mode, &env, "bisect_up");
        let out = bisect_action(&mut mode, &env, "bisect_down");
        assert!(out.contains(&Command::FinishMode {
            cause: FinishCause::Selection
        }));
        assert!(mode.controller.session.terminal);
        assert_eq!(
            mode.controller.session.current(),
            Some(Rect::new(250., 150., 250., 150.))
        );
        assert!(scene_of(&out).labels.is_empty());
        assert_eq!(scene_of(&out).shapes.len(), 1);
        bisect_action(&mut mode, &env, "previous_level");
        assert!(!mode.controller.session.terminal);
        assert_eq!(mode.controller.session.depth(), 3);
    }

    #[test]
    fn bisect_follow_lifecycle_retarget_and_restart_preserve_grid_contracts() {
        let mut config = Config::default();
        config.bisect.cursor_follow_selection = false;
        config.bisect.max_depth = 2;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::bisect(&env.config);
        activate(&mut mode, &env);
        let out = bisect_action(&mut mode, &env, "bisect_right");
        assert_eq!(out.len(), 1); // Only redraw when follow is off.
        assert!(toggle_follow(&mut mode, &env).contains(&Command::warp_to(Point::new(750., 300.))));
        let out = bisect_action(&mut mode, &env, "bisect_down");
        assert!(out.contains(&Command::FinishMode {
            cause: FinishCause::Selection
        }));
        mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Selection,
            },
            &env.ctx(),
        );
        let current = mode.controller.session.current();
        assert!(bisect_action(&mut mode, &env, "bisect_left").is_empty());
        mode.handle(&ModeEvent::Resumed, &env.ctx());
        assert_eq!(mode.controller.session.current(), current);
        let bounds = Rect::new(-2000., 100., 2000., 1200.);
        mode.retarget(bounds, true, &env.ctx());
        assert_eq!(
            mode.controller.session.current(),
            Some(Rect::new(-1000., 700., 1000., 600.))
        );
        bisect_action(&mut mode, &env, "previous_level");
        assert!(!mode.controller.session.finished);
        assert_eq!(
            mode.controller.session.current(),
            Some(Rect::new(-1000., 100., 1000., 1200.))
        );
        mode.handle(&ModeEvent::Restarted, &env.ctx());
        assert_eq!(
            mode.controller.session.current(),
            Some(env.screens[0].bounds)
        );
        assert!(!mode.controller.session.cursor_follow_selection);
        assert_eq!(mode.controller.session.depth(), 0);
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
            fn input_panel(
                &self,
                _: &crate::api::style::CompiledSearchPanel,
                _: Option<Rect>,
                _: &HostContext<'_>,
            ) -> (Rect, crate::api::overlay::SharedLabelStyle) {
                panic!("Grid never requests text input")
            }
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
            keys: mode.controller.layout_at(0).keys.as_ptr() as usize,
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
        let suffix_count =
            mode.controller.layout_at(0).keys.len() * mode.controller.layout_at(0).keys.len();
        assert_eq!(
            scene.labels.len(),
            suffix_count + mode.controller.layout_at(0).keys.len()
        );
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
            mode.controller.layout_at(0).cols - 1 + mode.controller.layout_at(0).rows - 1
                + mode.controller.layout_at(0).cols * (mode.controller.layout_at(0).cols - 1)
                + mode.controller.layout_at(0).rows * (mode.controller.layout_at(0).rows - 1)
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
        assert_eq!(mode.controller.session.depth(), 1);
        assert_eq!(scene.labels.len(), mode.controller.layout_at(0).keys.len());
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
        let View::Grid(view) = mode.view() else {
            panic!("expected Grid view")
        };
        assert!(
            scene
                .labels
                .iter()
                .all(|label| label.style.text_color == view.style(&env.palette).text_color)
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

        let first = cells(&mode)[0];
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.controller.session.current(), Some(first));
        assert_eq!(mode.controller.session.depth(), 1);
        assert!(out.contains(&Command::warp_to(first.center())));

        let second = cells(&mode)[0];
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.controller.session.current(), Some(second));
        assert_eq!(mode.controller.session.depth(), 2);
        assert!(!mode.controller.session.terminal);
        assert!(out.contains(&Command::warp_to(second.center())));

        let third = cells(&mode)[0];
        let out = press(&mut mode, &env, "1");
        assert_eq!(mode.controller.session.current(), Some(third));
        assert_eq!(mode.controller.session.depth(), 3);
        assert!(mode.controller.session.terminal);
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
        assert_eq!(mode.controller.session.depth(), 1);

        env.cursor = Point::new(1400.0, 350.0);
        let out = mode.handle(&ModeEvent::PointerMoved(env.cursor), &env.ctx());

        assert_eq!(mode.controller.session.root(), Some(env.screens[1].bounds));
        assert_eq!(mode.controller.session.depth(), 0);
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
        assert_eq!(mode.controller.session.depth(), 2);
        assert_eq!(
            labels.len(),
            mode.controller.layout_at(0).rows * mode.controller.layout_at(0).cols
        );
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
        assert!(!mode.controller.session.cursor_follow_selection);
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
        let selected = mode.controller.session.current().unwrap();
        let depth = mode.controller.session.depth();

        let out = toggle_follow(&mut mode, &env);

        assert!(mode.controller.session.cursor_follow_selection);
        assert_eq!(
            mode.controller.session.depth(),
            depth,
            "toggle must not select another layer"
        );
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
        let selected = mode.controller.session.current().unwrap();

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
        assert_eq!(mode.controller.session.path.as_slice(), [5, 6]);
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
        assert_eq!(mode.controller.session.path.as_slice(), [5, 6]);
        assert_eq!(mode.controller.session.depth(), 2);
        assert_eq!(scene_of(&out).clip, Some(target.bounds));
        assert!(out.contains(&Command::warp_to(
            mode.controller.session.current().unwrap().center()
        )));

        let out = mode.handle(
            &ModeEvent::ScreenRetargeted {
                screen: target.clone(),
                preserve: false,
            },
            &env.ctx(),
        );
        assert!(mode.controller.session.path.is_empty());
        assert_eq!(mode.controller.session.depth(), 0);
        assert_eq!(mode.controller.session.current(), Some(target.bounds));
        assert_eq!(scene_of(&out).clip, Some(target.bounds));
    }

    #[test]
    fn backspace_widens_and_space_resets_the_selection() {
        let env = Env::new();
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "q");
        press(&mut mode, &env, "q");
        assert_eq!(mode.controller.session.depth(), 2);

        press(&mut mode, &env, "backspace");
        assert_eq!(mode.controller.session.depth(), 1);
        assert!(!mode.controller.session.terminal);
        press(&mut mode, &env, "space");
        assert_eq!(mode.controller.session.depth(), 0);
        assert_eq!(
            mode.controller.session.current(),
            Some(env.screens[0].bounds)
        );
    }

    #[test]
    fn finish_is_idempotent_and_keep_preserves_the_selected_path() {
        let mut config = Config::default();
        config.grid.lifecycle.after_finish = crate::config::LifecycleAction::Keep;
        let env = Env::with(config);
        let mut mode = crate::app::mode_catalog::grid(&env.config);
        activate(&mut mode, &env);
        press(&mut mode, &env, "1");
        let selected = mode.controller.session.current();

        let finished = mode.handle(
            &ModeEvent::FinishRequested {
                cause: FinishCause::Explicit,
            },
            &env.ctx(),
        );
        assert!(mode.controller.session.finished);
        assert_eq!(mode.controller.session.current(), selected);
        let scene = scene_of(&finished);
        assert_eq!(
            scene.labels.len(),
            mode.controller.layout_at(0).rows * mode.controller.layout_at(0).cols
        );
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
        assert!(!mode.controller.session.finished);
        assert_eq!(mode.controller.session.depth(), 0);
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

#[cfg(test)]
mod recursive_tests;
