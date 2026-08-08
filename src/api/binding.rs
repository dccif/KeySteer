//! Bindings: the verb vocabulary shared by configuration, modes and plugins.
//!
//! A configuration value like `"move_left"` or `"grid"` parses into a
//! [`Binding`]. The engine resolves it and either acts on it directly (mode
//! switches, synthetic keystrokes, commands) or hands it to the active mode as
//! a [`ModeEvent::Binding`](crate::api::command::ModeEvent::Binding).
//!
//! This is deliberately the *same* vocabulary a plugin sees: there is no
//! `action` indirection and no separate internal path. A plugin can therefore
//! handle `move_left` exactly as the built-in `normal` mode does, and a user
//! can bind a key to a plugin mode exactly as to a built-in one.
//!
//! ```text
//! h        = "move_left"      # a verb the active mode handles
//! g        = "grid"           # enter a mode (built-in or plugin)
//! t        = "home"           # send a keystroke
//! "ctrl+c" = "send ctrl+c"    # explicit send, for chords
//! F5       = "exec make"      # run a command
//! q        = "none"           # remove an inherited binding
//! ```

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::input::{Key, KeyChord, ModeId};

/// Default interval used by `wait`/`wait 0` and repeated synthetic keystrokes.
pub const DEFAULT_WAIT_MS: u64 = 100;

/// A four-way direction, used by movement and scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

impl Direction {
    /// Unit vector in screen coordinates (y grows downwards).
    pub fn delta(self) -> (f64, f64) {
        match self {
            Direction::Left => (-1.0, 0.0),
            Direction::Down => (0.0, 1.0),
            Direction::Up => (0.0, -1.0),
            Direction::Right => (1.0, 0.0),
        }
    }
}

/// How far a single scroll binding travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScrollAmount {
    /// `scroll_step` pixels.
    Step,
    /// `scroll_step_half` pixels, i.e. half a page.
    Half,
    /// `scroll_step_full` pixels, i.e. to the end.
    Full,
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// A keyboard key or mouse button that can be latched by an input action.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputTarget {
    Key(Key),
    Mouse(Button),
}

impl InputTarget {
    fn parse(value: &str) -> Result<Self, String> {
        let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
        let button = match normalized.as_str() {
            "mouse_left" => Some(Button::Left),
            "mouse_right" => Some(Button::Right),
            "mouse_middle" => Some(Button::Middle),
            _ => None,
        };
        if let Some(button) = button {
            return Ok(Self::Mouse(button));
        }
        if !Key::is_known(value) {
            return Err(format!("unknown input target: {value:?}"));
        }
        Key::new(value).map(Self::Key)
    }

    pub fn canonical(&self) -> String {
        match self {
            Self::Key(key) => key.as_str().to_string(),
            Self::Mouse(Button::Left) => "mouse_left".into(),
            Self::Mouse(Button::Right) => "mouse_right".into(),
            Self::Mouse(Button::Middle) => "mouse_middle".into(),
        }
    }
}

/// Temporary speed modifier held alongside a movement key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Speed {
    /// Pixel-accurate movement for final positioning.
    Precision,
    Slow,
    Fast,
}

/// A resolved binding: what a key should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// Execute each action in order. Mode changes do not stop the sequence.
    Sequence(Vec<Binding>),
    /// Enter a mode. Built-in and plugin modes are indistinguishable here.
    Mode(ModeId),
    /// Invoke a verb exported by a plugin without coupling configuration to
    /// that plugin's concrete Rust type.
    Invoke { verb: String, args: Vec<String> },
    /// Move the pointer continuously while held.
    Move(Direction),
    /// Move the pointer to an absolute screen coordinate.
    Warp { x: i32, y: i32 },
    /// Scroll while held.
    Scroll(Direction, ScrollAmount),
    /// Click a button.
    Click(Button),
    /// Double-click a button.
    DoubleClick(Button),
    /// Hold one or more keyboard keys or mouse buttons until explicitly released.
    Press(Vec<InputTarget>),
    /// Release one or more inputs previously held by [`Binding::Press`] or [`Binding::Toggle`].
    Release(Vec<InputTarget>),
    /// Press unlatched inputs and release latched inputs. An empty list turns
    /// the activation key into a toggle modifier: companion actions select
    /// their targets, while a bare activation releases all latched inputs.
    Toggle(Vec<InputTarget>),
    /// Pause an action sequence without blocking the input event loop.
    Wait { min_ms: u64, max_ms: u64 },
    /// Scale pointer speed while held.
    Speed(Speed),
    /// Toggle live cursor tracking while selecting a grid cell.
    ToggleCursorFollowSelection,
    /// Send a synthetic keystroke to the focused application.
    Send(KeyChord),
    /// Run a command.
    Exec { program: String, args: Vec<String> },
    /// Request a fresh UI hint scan.
    RescanUi,
    /// Complete the current targeting session without re-entering its mode.
    FinishMode,
    /// Explicitly reset the active mode's current session.
    RestartMode,
    /// Re-read the configuration from disk.
    ReloadConfig,
    /// Persist a TOML value at a dotted configuration path.
    SetConfig { path: String, value: String },
    /// Leave the current mode and return to idle.
    Escape,
    /// Stop the program.
    Quit,
    /// Explicitly unbound: removes an inherited default.
    Disabled,
}

impl Binding {
    /// Sentinels that clear an inherited binding.
    pub const DISABLED: [&'static str; 2] = ["none", "__disabled__"];

    /// Parse a configuration value.
    ///
    /// Resolution order is: sentinel, explicit `send`/`exec`, known verb,
    /// known key name, then a mode id. Anything else is an error, so a typo
    /// surfaces at load time instead of silently doing nothing.
    pub fn parse(value: &str) -> Result<Self, String> {
        let text = value.trim();
        if text.is_empty() {
            return Err("binding must not be empty".into());
        }

        let mut words = text.split_whitespace();
        let head = words.next().unwrap_or_default();
        let rest: Vec<&str> = words.collect();

        if Self::DISABLED.contains(&head) && rest.is_empty() {
            return Ok(Binding::Disabled);
        }

        // Explicit forms, needed when the argument is not a bare word.
        match head {
            "call" => {
                let (verb, args) = rest
                    .split_first()
                    .ok_or_else(|| "`call` needs a plugin verb".to_string())?;
                return Ok(Binding::Invoke {
                    verb: (*verb).to_string(),
                    args: args.iter().map(|arg| (*arg).to_string()).collect(),
                });
            }
            "send" => {
                if rest.is_empty() {
                    return Err("`send` needs a key or chord, e.g. `send ctrl+c`".into());
                }
                return KeyChord::parse(&rest.join("")).map(Binding::Send);
            }
            "exec" => {
                let (program, args) = rest
                    .split_first()
                    .ok_or_else(|| "`exec` needs a command".to_string())?;
                return Ok(Binding::Exec {
                    program: (*program).to_string(),
                    args: args.iter().map(|s| (*s).to_string()).collect(),
                });
            }
            "move_mouse" => {
                if rest.len() != 2 {
                    return Err("`move_mouse` needs exactly two integer coordinates".into());
                }
                let x = rest[0]
                    .parse::<i32>()
                    .map_err(|_| format!("invalid x coordinate: {:?}", rest[0]))?;
                let y = rest[1]
                    .parse::<i32>()
                    .map_err(|_| format!("invalid y coordinate: {:?}", rest[1]))?;
                return Ok(Binding::Warp { x, y });
            }
            "set_config" => {
                let (path, value) = rest
                    .split_first()
                    .ok_or_else(|| "`set_config` needs a dotted path and TOML value".to_string())?;
                if value.is_empty() {
                    return Err("`set_config` needs a TOML value".into());
                }
                return Ok(Binding::SetConfig {
                    path: (*path).to_string(),
                    value: value.join(" "),
                });
            }
            "press" | "release" | "toggle" => {
                if head != "toggle" && rest.is_empty() {
                    return Err(format!("`{head}` needs at least one key or mouse button"));
                }
                let targets = rest
                    .iter()
                    .map(|target| InputTarget::parse(target))
                    .collect::<Result<Vec<_>, _>>()?;
                if targets.iter().collect::<BTreeSet<_>>().len() != targets.len() {
                    return Err(format!("`{head}` contains a duplicate input target"));
                }
                return Ok(match head {
                    "press" => Binding::Press(targets),
                    "release" => Binding::Release(targets),
                    _ => Binding::Toggle(targets),
                });
            }
            "wait" => {
                const MAX_WAIT_MS: u64 = 86_400_000;
                let parse_delay = |value: &str| {
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid wait duration: {value:?}"))
                        .and_then(|duration| {
                            (duration <= MAX_WAIT_MS)
                                .then_some(duration)
                                .ok_or_else(|| {
                                    "wait duration must not exceed 86400000ms".to_string()
                                })
                        })
                };
                let (min_ms, max_ms) = match rest.as_slice() {
                    [] | ["0"] => (DEFAULT_WAIT_MS, DEFAULT_WAIT_MS),
                    [max] => (0, parse_delay(max)?),
                    [min, max] => (parse_delay(min)?, parse_delay(max)?),
                    _ => return Err("`wait` accepts zero, one, or two millisecond values".into()),
                };
                if min_ms > max_ms {
                    return Err("`wait` minimum must not exceed its maximum".into());
                }
                return Ok(Binding::Wait { min_ms, max_ms });
            }
            _ => {}
        }

        // A known built-in verb never accepts stray arguments. An unknown
        // lower-case head with arguments is a plugin verb invocation, e.g.
        // `screen next`; use `call screen` for a zero-argument invocation.
        if !rest.is_empty() {
            if Self::verb(head).is_some() {
                return Err(format!("binding {head:?} does not take arguments"));
            }
            if head
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return Ok(Binding::Invoke {
                    verb: head.to_string(),
                    args: rest.iter().map(|arg| (*arg).to_string()).collect(),
                });
            }
            return Err(format!("invalid plugin verb invocation: {text:?}"));
        }

        if let Some(binding) = Self::verb(head) {
            return Ok(binding);
        }
        if head.contains('+') {
            return KeyChord::parse(head).map(Binding::Send);
        }
        // A bare key name sends that key: `t = "home"`.
        if Key::is_known(head) {
            return KeyChord::parse(head).map(Binding::Send);
        }
        // Otherwise it must name a mode. A bare word has to be a built-in;
        // plugin modes are namespaced, which keeps typos from resolving to a
        // mode that will never exist.
        if ModeId::is_plausible(head) {
            return ModeId::new(head).map(Binding::Mode);
        }
        Err(format!(
            "unknown binding: {text:?}. Expected a verb (e.g. `move_left`), a key \
             (e.g. `home`), a mode ({}), a namespaced plugin mode \
             (`my-plugin:zoom`), `send ...`, `exec ...`, or `none`",
            ModeId::BUILT_IN.join(", ")
        ))
    }

    fn verb(word: &str) -> Option<Self> {
        use Binding as B;
        use Button::{Left, Middle, Right};
        use Direction::{Down, Up};
        use ScrollAmount::{Full, Half, Step};

        Some(match word {
            "move_left" => B::Move(Direction::Left),
            "move_down" => B::Move(Down),
            "move_up" => B::Move(Up),
            "move_right" => B::Move(Direction::Right),

            "wheel_left" | "scroll_left" => B::Scroll(Direction::Left, Step),
            "wheel_down" | "scroll_down" => B::Scroll(Down, Step),
            "wheel_up" | "scroll_up" => B::Scroll(Up, Step),
            "wheel_right" | "scroll_right" => B::Scroll(Direction::Right, Step),
            "wheel_half_down" | "scroll_half_down" => B::Scroll(Down, Half),
            "wheel_half_up" | "scroll_half_up" => B::Scroll(Up, Half),
            "wheel_full_down" | "scroll_full_down" => B::Scroll(Down, Full),
            "wheel_full_up" | "scroll_full_up" => B::Scroll(Up, Full),

            "left_click" => B::Click(Left),
            "right_click" => B::Click(Right),
            "middle_click" => B::Click(Middle),
            "double_click" => B::DoubleClick(Left),
            "left_press" => B::Press(vec![InputTarget::Mouse(Left)]),
            "left_release" => B::Release(vec![InputTarget::Mouse(Left)]),
            "right_press" => B::Press(vec![InputTarget::Mouse(Right)]),
            "right_release" => B::Release(vec![InputTarget::Mouse(Right)]),
            "toggle_left" => B::Toggle(vec![InputTarget::Mouse(Left)]),
            "toggle_right" => B::Toggle(vec![InputTarget::Mouse(Right)]),
            "toggle" => B::Toggle(Vec::new()),
            "wait" => B::Wait {
                min_ms: DEFAULT_WAIT_MS,
                max_ms: DEFAULT_WAIT_MS,
            },

            "precision" => B::Speed(Speed::Precision),
            "slow" => B::Speed(Speed::Slow),
            "fast" => B::Speed(Speed::Fast),
            "follow" => B::ToggleCursorFollowSelection,
            "finish" | "finish_mode" => B::FinishMode,
            "restart_mode" => B::RestartMode,

            "escape" | "exit_mode" => B::Escape,
            "rescan" | "rescan_ui" => B::RescanUi,
            "reload_config" => B::ReloadConfig,
            "quit" => B::Quit,
            _ => return None,
        })
    }

    /// True for bindings whose effect lasts while the key is held, so the
    /// engine must deliver the release as well as the press.
    pub fn is_held(&self) -> bool {
        match self {
            Binding::Sequence(actions) => actions.iter().any(Binding::is_held),
            Binding::Move(_) | Binding::Scroll(..) | Binding::Speed(_) => true,
            _ => false,
        }
    }

    /// The mode this binding enters, if any.
    pub fn mode(&self) -> Option<&ModeId> {
        match self {
            Binding::Mode(id) => Some(id),
            _ => None,
        }
    }

    /// Canonical text form. `Binding::parse(&b.to_string()) == Ok(b)`.
    pub fn canonical(&self) -> String {
        use Binding as B;
        let button = |b: &Button| match b {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        };
        let direction = |d: &Direction| match d {
            Direction::Left => "left",
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::Right => "right",
        };
        match self {
            B::Sequence(actions) => actions
                .iter()
                .map(Binding::canonical)
                .collect::<Vec<_>>()
                .join(" ; "),
            B::Mode(id) => id.to_string(),
            B::Invoke { verb, args } if args.is_empty() => format!("call {verb}"),
            B::Invoke { verb, args } => format!("{verb} {}", args.join(" ")),
            B::Move(d) => format!("move_{}", direction(d)),
            B::Warp { x, y } => format!("move_mouse {x} {y}"),
            B::Scroll(d, ScrollAmount::Step) => format!("wheel_{}", direction(d)),
            B::Scroll(d, ScrollAmount::Half) => format!("wheel_half_{}", direction(d)),
            B::Scroll(d, ScrollAmount::Full) => format!("wheel_full_{}", direction(d)),
            B::Click(b) => format!("{}_click", button(b)),
            B::DoubleClick(Button::Left) => "double_click".into(),
            B::DoubleClick(b) => format!("{}_double_click", button(b)),
            B::Press(targets) => format_targets("press", targets),
            B::Release(targets) => format_targets("release", targets),
            B::Toggle(targets) => format_targets("toggle", targets),
            B::Wait { min_ms, max_ms } if min_ms == max_ms && *min_ms == DEFAULT_WAIT_MS => {
                "wait".into()
            }
            B::Wait { min_ms, max_ms } if min_ms == max_ms => format!("wait {min_ms} {max_ms}"),
            B::Wait { min_ms: 0, max_ms } => format!("wait {max_ms}"),
            B::Wait { min_ms, max_ms } => format!("wait {min_ms} {max_ms}"),
            B::Speed(Speed::Precision) => "precision".into(),
            B::Speed(Speed::Slow) => "slow".into(),
            B::Speed(Speed::Fast) => "fast".into(),
            B::ToggleCursorFollowSelection => "follow".into(),
            B::Send(chord) => format!("send {}", chord.canonical()),
            B::Exec { program, args } if args.is_empty() => format!("exec {program}"),
            B::Exec { program, args } => format!("exec {program} {}", args.join(" ")),
            B::RescanUi => "rescan".into(),
            B::FinishMode => "finish".into(),
            B::RestartMode => "restart_mode".into(),
            B::ReloadConfig => "reload_config".into(),
            B::SetConfig { path, value } => format!("set_config {path} {value}"),
            B::Escape => "escape".into(),
            B::Quit => "quit".into(),
            B::Disabled => "none".into(),
        }
    }
}

fn format_targets(verb: &str, targets: &[InputTarget]) -> String {
    if targets.is_empty() {
        verb.to_string()
    } else {
        format!(
            "{verb} {}",
            targets
                .iter()
                .map(InputTarget::canonical)
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

impl Serialize for Binding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Binding::Sequence(actions) => actions
                .iter()
                .map(Binding::canonical)
                .collect::<Vec<_>>()
                .serialize(serializer),
            _ => serializer.serialize_str(&self.canonical()),
        }
    }
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }

        match Raw::deserialize(deserializer)? {
            Raw::One(value) => Self::parse(&value).map_err(serde::de::Error::custom),
            Raw::Many(values) if values.is_empty() => Err(serde::de::Error::custom(
                "action sequence must not be empty",
            )),
            Raw::Many(values) => values
                .iter()
                .map(|value| Self::parse(value).map_err(serde::de::Error::custom))
                .collect::<Result<Vec<_>, _>>()
                .map(Binding::Sequence),
        }
    }
}

/// Public name for the single action vocabulary shared by config and plugins.
pub type Action = Binding;

/// An ordered list of actions dispatched as one binding.
pub type ActionSequence = Vec<Action>;

/// The edge of a stateful action invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionPhase {
    Start,
    End,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_movement_and_scroll_verbs() {
        assert_eq!(
            Binding::parse("move_left").unwrap(),
            Binding::Move(Direction::Left)
        );
        assert_eq!(
            Binding::parse("scroll_up").unwrap(),
            Binding::Scroll(Direction::Up, ScrollAmount::Step)
        );
        assert_eq!(
            Binding::parse("scroll_half_down").unwrap(),
            Binding::Scroll(Direction::Down, ScrollAmount::Half)
        );
    }

    #[test]
    fn follow_is_a_discrete_grid_selection_binding() {
        assert_eq!(
            Binding::parse("follow").unwrap(),
            Binding::ToggleCursorFollowSelection
        );
    }

    #[test]
    fn parses_generic_input_actions() {
        assert_eq!(
            Binding::parse("toggle").unwrap(),
            Binding::Toggle(Vec::new())
        );
        assert_eq!(
            Binding::parse("press shift mouse_left home").unwrap(),
            Binding::Press(vec![
                InputTarget::Key(Key::new("shift").unwrap()),
                InputTarget::Mouse(Button::Left),
                InputTarget::Key(Key::new("home").unwrap()),
            ])
        );
        assert_eq!(
            Binding::parse("release ctrl mouse_right").unwrap(),
            Binding::Release(vec![
                InputTarget::Key(Key::new("ctrl").unwrap()),
                InputTarget::Mouse(Button::Right),
            ])
        );
        assert!(Binding::parse("press").is_err());
        assert!(Binding::parse("press shift shift").is_err());
        assert!(Binding::parse("toggle unknown_key_name").is_err());
    }

    #[test]
    fn parses_default_fixed_and_random_waits() {
        assert_eq!(
            Binding::parse("wait").unwrap(),
            Binding::Wait {
                min_ms: 100,
                max_ms: 100,
            }
        );
        assert_eq!(
            Binding::parse("wait 0").unwrap(),
            Binding::parse("wait").unwrap()
        );
        assert_eq!(
            Binding::parse("wait 75").unwrap(),
            Binding::Wait {
                min_ms: 0,
                max_ms: 75,
            }
        );
        assert_eq!(
            Binding::parse("wait 50 100").unwrap(),
            Binding::Wait {
                min_ms: 50,
                max_ms: 100,
            }
        );
        assert!(Binding::parse("wait 100 50").is_err());
        assert!(Binding::parse("wait nope").is_err());
    }

    #[test]
    fn a_mode_name_is_a_binding_without_any_prefix() {
        // The point of dropping `action`: entering a mode reads naturally.
        assert_eq!(
            Binding::parse("grid").unwrap(),
            Binding::Mode(ModeId::grid())
        );
        assert_eq!(
            Binding::parse("plugin:screen-selector").unwrap(),
            Binding::Mode(ModeId::new("plugin:screen-selector").unwrap())
        );
    }

    #[test]
    fn a_bare_key_name_sends_that_key() {
        // `t = "home"` from the user's configuration.
        for name in ["home", "end", "page_up", "page_down", "f5"] {
            match Binding::parse(name).unwrap() {
                Binding::Send(chord) => assert_eq!(chord.canonical(), name),
                other => panic!("{name} should send a key, got {other:?}"),
            }
        }
    }

    #[test]
    fn send_accepts_a_chord() {
        match Binding::parse("send ctrl+c").unwrap() {
            Binding::Send(chord) => assert_eq!(chord.canonical(), "ctrl+c"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_chord_is_a_direct_send_action() {
        match Binding::parse("shift+home").unwrap() {
            Binding::Send(chord) => assert_eq!(chord.canonical(), "shift+home"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_absolute_pointer_and_config_actions() {
        assert_eq!(
            Binding::parse("move_mouse 23 43").unwrap(),
            Binding::Warp { x: 23, y: 43 }
        );
        assert_eq!(
            Binding::parse("reload_config").unwrap(),
            Binding::ReloadConfig
        );
        assert_eq!(
            Binding::parse("set_config pointer.max_speed 42.5").unwrap(),
            Binding::SetConfig {
                path: "pointer.max_speed".into(),
                value: "42.5".into(),
            }
        );
    }

    #[test]
    fn toml_arrays_deserialize_as_ordered_sequences() {
        #[derive(Deserialize)]
        struct Holder {
            action: Binding,
        }
        let holder: Holder =
            toml::from_str(r#"action = ["move_mouse 1 2", "left_click", "grid"]"#).unwrap();
        assert_eq!(
            holder.action,
            Binding::Sequence(vec![
                Binding::Warp { x: 1, y: 2 },
                Binding::Click(Button::Left),
                Binding::Mode(ModeId::grid()),
            ])
        );
        let double: Holder = toml::from_str(r#"action = ["left_click", "left_click"]"#).unwrap();
        assert_eq!(
            double.action,
            Binding::Sequence(vec![
                Binding::Click(Button::Left),
                Binding::Click(Button::Left),
            ])
        );
        assert!(toml::from_str::<Holder>("action = []").is_err());
    }

    #[test]
    fn exec_splits_program_and_arguments() {
        assert_eq!(
            Binding::parse("exec make -j8").unwrap(),
            Binding::Exec {
                program: "make".into(),
                args: vec!["-j8".into()],
            }
        );
    }

    #[test]
    fn typos_are_rejected_instead_of_silently_sending_keys() {
        // The important safety property: `gird` must not become four keys.
        for bad in ["gird", "mov_left", "scrol_up", "leftclick!"] {
            assert!(Binding::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn disabled_sentinels_are_recognised() {
        assert_eq!(Binding::parse("none").unwrap(), Binding::Disabled);
        assert_eq!(Binding::parse("__disabled__").unwrap(), Binding::Disabled);
    }

    #[test]
    fn only_held_bindings_need_a_release_event() {
        assert!(Binding::Move(Direction::Left).is_held());
        assert!(Binding::Speed(Speed::Slow).is_held());
        assert!(!Binding::Click(Button::Left).is_held());
        assert!(!Binding::Mode(ModeId::grid()).is_held());
        assert!(!Binding::ToggleCursorFollowSelection.is_held());
    }

    #[test]
    fn every_binding_round_trips_through_its_canonical_form() {
        let all = [
            Binding::Mode(ModeId::normal()),
            Binding::Invoke {
                verb: "screen".into(),
                args: vec!["previous".into()],
            },
            Binding::Invoke {
                verb: "screen".into(),
                args: Vec::new(),
            },
            Binding::Move(Direction::Right),
            Binding::Scroll(Direction::Down, ScrollAmount::Step),
            Binding::Scroll(Direction::Up, ScrollAmount::Half),
            Binding::Scroll(Direction::Down, ScrollAmount::Full),
            Binding::Click(Button::Middle),
            Binding::DoubleClick(Button::Left),
            Binding::Press(vec![InputTarget::Mouse(Button::Left)]),
            Binding::Release(vec![InputTarget::Mouse(Button::Left)]),
            Binding::Toggle(vec![InputTarget::Mouse(Button::Right)]),
            Binding::Wait {
                min_ms: 25,
                max_ms: 50,
            },
            Binding::Speed(Speed::Fast),
            Binding::ToggleCursorFollowSelection,
            Binding::Send(KeyChord::parse("ctrl+alt+delete").unwrap()),
            Binding::Exec {
                program: "ls".into(),
                args: vec!["-la".into()],
            },
            Binding::RescanUi,
            Binding::FinishMode,
            Binding::RestartMode,
            Binding::Escape,
            Binding::Quit,
            Binding::Disabled,
        ];
        for binding in all {
            let text = binding.canonical();
            assert_eq!(
                Binding::parse(&text).unwrap(),
                binding,
                "round trip failed for {text:?}"
            );
        }
    }

    #[test]
    fn empty_and_argument_misuse_are_errors() {
        assert!(Binding::parse("").is_err());
        assert!(Binding::parse("send").is_err());
        assert!(Binding::parse("exec").is_err());
        // A verb that does not take arguments must not silently ignore them.
        assert!(Binding::parse("move_left now").is_err());
    }
}
