//! Multi-display selection and switching implemented only with the public mode,
//! command, geometry, overlay and plugin APIs.

use crate::api::binding::Binding;
use crate::api::command::{Command, CommandBatch, HostContext, Mode, ModeEvent};
use crate::api::geometry::Rect;
use crate::api::input::{KeyChord, KeyState, ModeId};
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape, Placement};
use crate::api::plugin::{Manifest, Plugin};
use crate::config::{Config, Palette};
use std::collections::BTreeMap;

const MODE_ID: &str = "plugin:screen-selector";
const VERB: &str = "screen";

pub struct ScreenSelector {
    manifest: Manifest,
    id: ModeId,
    /// Display number, zero-based screen index, and current bounds.
    cells: Vec<(String, usize, Rect)>,
    input: String,
    modal: bool,
    return_mode: ModeId,
}

impl ScreenSelector {
    pub fn new() -> Result<Self, String> {
        Self::with_key_aliases(&BTreeMap::new())
    }

    pub(crate) fn with_key_aliases(aliases: &BTreeMap<String, String>) -> Result<Self, String> {
        let manifest = Manifest::new("com.keysteer.screen-selector", "Screen Selector")
            .with_description("Switch displays directly or choose one from a numbered overlay")
            .with_verb(VERB);
        let manifest = match KeyChord::parse_with_aliases("primary+s", aliases) {
            Ok(chord) => manifest.with_default_binding(
                chord,
                Binding::Invoke {
                    verb: VERB.into(),
                    args: vec!["next".into()],
                },
            ),
            Err(_) => manifest,
        };
        Ok(Self {
            manifest,
            id: ModeId::new(MODE_ID)?,
            cells: Vec::new(),
            input: String::new(),
            modal: false,
            return_mode: ModeId::idle(),
        })
    }

    fn preserve(ctx: &HostContext<'_>) -> bool {
        ctx.config
            .downcast_ref::<Config>()
            .and_then(|config| config.plugin_setting_bool(MODE_ID, "preserve"))
            .unwrap_or(true)
    }

    fn build(&mut self, ctx: &HostContext<'_>) {
        self.cells = ctx
            .screens
            .iter()
            .enumerate()
            .map(|(index, screen)| ((index + 1).to_string(), index, screen.bounds))
            .collect();
        self.input.clear();
    }

    fn style(palette: &Palette) -> LabelStyle {
        LabelStyle {
            background: palette.surface_label(),
            text_color: palette.text,
            border_color: palette.accent,
            font_size: 72.0,
            padding_x: 30.0,
            padding_y: 20.0,
            border_radius: 14.0,
            bold: true,
            ..LabelStyle::default()
        }
    }

    fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let style = Self::style(palette);
        let mut scene = OverlayScene::new().with_backdrop(Color::rgba(0, 0, 0, 0x60));
        for (label, _, bounds) in &self.cells {
            scene.push_shape(OverlayShape::Rect {
                rect: bounds.inset(8.0, 8.0),
                fill: palette.highlight(),
                stroke: palette.accent,
                stroke_width: 4.0,
                corner_radius: 8.0,
                z_index: 0,
            });
            let size = style.font_size * 2.2;
            scene.push_label(
                OverlayLabel::new(
                    label.clone(),
                    Placement::Center.place(bounds, size, size),
                    style.clone(),
                )
                .with_matched_prefix(if label.starts_with(&self.input) {
                    self.input.chars().count()
                } else {
                    0
                })
                .with_z_index(1),
            );
        }
        scene
    }

    fn current_index(ctx: &HostContext<'_>) -> usize {
        ctx.screens
            .iter()
            .position(|screen| screen.bounds.contains(&ctx.cursor))
            .or_else(|| ctx.screens.iter().position(|screen| screen.is_primary))
            .unwrap_or(0)
    }

    fn resolve_target(args: &[String], ctx: &HostContext<'_>) -> Option<usize> {
        let count = ctx.screens.len();
        if count == 0 || args.len() != 1 {
            return None;
        }
        match args[0].to_ascii_lowercase().as_str() {
            "next" => Some((Self::current_index(ctx) + 1) % count),
            "previous" | "prev" => Some((Self::current_index(ctx) + count - 1) % count),
            number => number
                .parse::<usize>()
                .ok()
                .filter(|number| (1..=count).contains(number))
                .map(|number| number - 1),
        }
    }

    fn retarget(index: usize, ctx: &HostContext<'_>) -> Vec<Command> {
        vec![Command::RetargetScreen {
            index,
            preserve: Self::preserve(ctx),
        }]
    }

    fn close_and_retarget(&self, index: usize, ctx: &HostContext<'_>) -> Vec<Command> {
        let close = if self.modal {
            Command::PopMode
        } else {
            Command::SwitchMode(self.return_mode.clone())
        };
        vec![
            Command::HideOverlay,
            close,
            Command::RetargetScreen {
                index,
                preserve: Self::preserve(ctx),
            },
        ]
    }

    fn cancel(&self) -> Vec<Command> {
        let close = if self.modal {
            Command::PopMode
        } else {
            Command::SwitchMode(self.return_mode.clone())
        };
        vec![Command::HideOverlay, close]
    }

    fn choose_input(&mut self, ctx: &HostContext<'_>) -> Vec<Command> {
        let matches: Vec<_> = self
            .cells
            .iter()
            .filter(|(label, _, _)| label.starts_with(&self.input))
            .collect();
        if matches.is_empty() {
            self.input.clear();
            return vec![Command::show_overlay(self.scene(ctx))];
        }
        if matches.len() == 1 && matches[0].0 == self.input {
            return self.close_and_retarget(matches[0].1, ctx);
        }
        vec![Command::show_overlay(self.scene(ctx))]
    }
}

impl Mode for ScreenSelector {
    fn id(&self) -> ModeId {
        self.id.clone()
    }

    fn display_name(&self) -> String {
        "Screen".into()
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch {
        CommandBatch::from(match event {
            ModeEvent::Invoked { verb, args } if verb == VERB => {
                if args.is_empty() {
                    vec![Command::PushMode(self.id.clone())]
                } else {
                    Self::resolve_target(args, ctx)
                        .map(|index| Self::retarget(index, ctx))
                        .unwrap_or_default()
                }
            }
            ModeEvent::Pushed { previous } => {
                self.modal = true;
                self.return_mode = previous.clone();
                self.build(ctx);
                vec![Command::show_overlay(self.scene(ctx))]
            }
            ModeEvent::Activated { previous } => {
                self.modal = false;
                self.return_mode = previous.clone().unwrap_or_else(ModeId::idle);
                self.build(ctx);
                vec![Command::show_overlay(self.scene(ctx))]
            }
            ModeEvent::ScreensChanged(_) => {
                self.build(ctx);
                vec![Command::show_overlay(self.scene(ctx))]
            }
            ModeEvent::Deactivated => {
                self.cells.clear();
                self.input.clear();
                Vec::new()
            }
            ModeEvent::Key {
                key,
                state: KeyState::Down,
                repeat: false,
            } => match key.as_str() {
                "esc" => self.cancel(),
                "backspace" => {
                    self.input.pop();
                    vec![Command::show_overlay(self.scene(ctx))]
                }
                "enter" => self
                    .cells
                    .iter()
                    .find(|(label, _, _)| label == &self.input)
                    .map(|(_, index, _)| self.close_and_retarget(*index, ctx))
                    .unwrap_or_default(),
                _ => match key.as_char().filter(char::is_ascii_digit) {
                    Some(character) => {
                        self.input.push(character);
                        self.choose_input(ctx)
                    }
                    None => Vec::new(),
                },
            },
            _ => Vec::new(),
        })
    }
}

impl Plugin for ScreenSelector {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::{Point, Screen};

    fn screen(x: f64, primary: bool) -> Screen {
        Screen {
            bounds: Rect::new(x, 0.0, 1000.0, 800.0),
            work_area: Rect::new(x, 0.0, 1000.0, 800.0),
            is_primary: primary,
            scale: 1.0,
            name: None,
        }
    }

    struct Env {
        screens: Vec<Screen>,
        palette: Palette,
        config: Config,
        cursor: Point,
    }

    impl Env {
        fn new(screens: Vec<Screen>) -> Self {
            Self {
                screens,
                palette: Palette::default(),
                config: Config::default(),
                cursor: Point::new(10.0, 10.0),
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

    fn invoke(plugin: &mut ScreenSelector, env: &Env, args: &[&str]) -> Vec<Command> {
        plugin
            .handle(
                &ModeEvent::Invoked {
                    verb: VERB.into(),
                    args: args.iter().map(|arg| (*arg).into()).collect(),
                },
                &env.ctx(),
            )
            .into_iter()
            .collect()
    }

    #[test]
    fn manifest_exports_screen_and_primary_c_runs_next() {
        let plugin = ScreenSelector::new().unwrap();
        plugin.manifest().validate().unwrap();
        assert_eq!(plugin.manifest().verbs, [VERB]);
        assert_eq!(plugin.manifest().default_bindings.len(), 1);
        assert_eq!(
            plugin.manifest().default_bindings[0].1,
            Binding::Invoke {
                verb: VERB.into(),
                args: vec!["next".into()]
            }
        );
    }

    #[test]
    fn manifest_honours_the_configured_primary_alias() {
        let aliases = BTreeMap::from([("primary".into(), "left_alt".into())]);
        let plugin = ScreenSelector::with_key_aliases(&aliases).unwrap();
        assert_eq!(
            plugin.manifest().default_bindings[0].0.canonical(),
            "left_alt+s"
        );
    }

    #[test]
    fn next_previous_and_number_wrap_or_jump() {
        let env = Env::new(vec![screen(0.0, true), screen(1000.0, false)]);
        let mut plugin = ScreenSelector::new().unwrap();
        assert_eq!(
            invoke(&mut plugin, &env, &["next"]),
            vec![Command::RetargetScreen {
                index: 1,
                preserve: true
            }]
        );
        assert_eq!(
            invoke(&mut plugin, &env, &["previous"]),
            vec![Command::RetargetScreen {
                index: 1,
                preserve: true
            }]
        );
        assert_eq!(
            invoke(&mut plugin, &env, &["2"]),
            vec![Command::RetargetScreen {
                index: 1,
                preserve: true
            }]
        );
    }

    #[test]
    fn no_argument_pushes_numbered_selector() {
        let env = Env::new(vec![screen(0.0, true), screen(1000.0, false)]);
        let mut plugin = ScreenSelector::new().unwrap();
        assert_eq!(
            invoke(&mut plugin, &env, &[]),
            vec![Command::PushMode(plugin.id())]
        );
        let out = plugin.handle(
            &ModeEvent::Pushed {
                previous: ModeId::grid(),
            },
            &env.ctx(),
        );
        let Command::ShowOverlay(scene) = &out[0] else {
            panic!("expected numbered overlay")
        };
        assert_eq!(
            scene
                .labels
                .iter()
                .map(|label| label.text.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
    }

    #[test]
    fn selecting_a_number_restores_previous_mode_then_retargets() {
        let env = Env::new(vec![screen(0.0, true), screen(1000.0, false)]);
        let mut plugin = ScreenSelector::new().unwrap();
        plugin.handle(
            &ModeEvent::Pushed {
                previous: ModeId::grid(),
            },
            &env.ctx(),
        );
        let out = plugin.handle(
            &ModeEvent::Key {
                key: crate::api::input::Key::new("2").unwrap(),
                state: KeyState::Down,
                repeat: false,
            },
            &env.ctx(),
        );
        assert_eq!(out[1], Command::PopMode);
        assert_eq!(
            out[2],
            Command::RetargetScreen {
                index: 1,
                preserve: true
            }
        );
    }
}
