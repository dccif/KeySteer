//! Normal mode: the working state.
//!
//! Normal is where the user actually drives the pointer: vim-style movement,
//! clicks, scrolling, and the key that enters `grid`, `recursive_grid` or
//! `ui_hint`.
//!
//! It captures the keyboard, so `hjkl` moves the cursor instead of typing. Keys
//! it has no binding for are swallowed rather than passed through, which is what
//! makes the mode predictable; bind them explicitly (`t = "home"`) to send
//! keystrokes to the focused application.
//!
//! The mode only implements the *held* gestures — movement, scrolling and the
//! speed modifiers — because those need acceleration state. Discrete verbs
//! (clicks, mode switches, `send`, `exec`) are handled by the engine before they
//! ever reach here, and identically for plugins.

use crate::api::binding::{Binding, Direction, ScrollAmount, Speed};
use crate::api::command::{Command, HostContext, Mode, ModeEvent};
use crate::api::input::{Key, KeyState, ModeId};
use crate::api::overlay::Color;
use crate::config::{Config, Palette, Pointer, Scroll};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Allocation-free set of active movement directions.
#[derive(Debug, Clone, Copy, Default)]
struct DirectionMask(u8);

impl DirectionMask {
    const fn bit(direction: Direction) -> u8 {
        match direction {
            Direction::Left => 1 << 0,
            Direction::Down => 1 << 1,
            Direction::Up => 1 << 2,
            Direction::Right => 1 << 3,
        }
    }

    fn insert(&mut self, direction: Direction) {
        self.0 |= Self::bit(direction);
    }

    fn contains(self, direction: Direction) -> bool {
        self.0 & Self::bit(direction) != 0
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn delta(self) -> (f64, f64) {
        let active = |direction| u8::from(self.contains(direction)) as f64;
        (
            active(Direction::Right) - active(Direction::Left),
            active(Direction::Down) - active(Direction::Up),
        )
    }
}

impl FromIterator<Direction> for DirectionMask {
    fn from_iter<T: IntoIterator<Item = Direction>>(directions: T) -> Self {
        let mut mask = Self::default();
        for direction in directions {
            mask.insert(direction);
        }
        mask
    }
}

/// Acceleration state for one continuous gesture.
///
/// Tracks sub-pixel remainders so slow speeds still move the cursor and
/// diagonals do not drift.
#[derive(Debug, Clone, Default)]
struct Motion {
    elapsed_seconds: f64,
    remainder_x: f64,
    remainder_y: f64,
}

impl Motion {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advance the held gesture by the real elapsed wall-clock time.
    ///
    /// Speed is pixels per second and acceleration is pixels per second², so
    /// display cadence never changes travel over an equal amount of time.
    fn step(
        &mut self,
        directions: DirectionMask,
        profile: &Pointer,
        multiplier: f64,
        elapsed: Duration,
    ) -> (f64, f64) {
        if directions.is_empty() {
            self.reset();
            return (0.0, 0.0);
        }

        let seconds = elapsed.as_secs_f64();
        if seconds <= 0.0 {
            return (0.0, 0.0);
        }
        let start = self.elapsed_seconds;
        self.elapsed_seconds += seconds;
        // The multiplier scales velocity and acceleration equally, so the
        // ramp duration stays stable when a speed modifier is pressed.
        let distance = Self::distance(profile, start, self.elapsed_seconds) * multiplier;
        self.advance(directions, distance)
    }

    /// Exact distance travelled between two points in the gesture timeline.
    ///
    /// Integrating the velocity curve rather than sampling it once per frame
    /// keeps total travel independent of display cadence, including a frame
    /// that crosses from acceleration into cruising speed.
    fn distance(profile: &Pointer, start: f64, end: f64) -> f64 {
        Self::travel_at(profile, end) - Self::travel_at(profile, start)
    }

    /// Distance travelled from the beginning of the gesture through `time`.
    fn travel_at(profile: &Pointer, time: f64) -> f64 {
        let time = time.max(0.0);
        let max_speed = profile.max_speed;
        let initial_speed = profile.initial_speed.min(max_speed);
        let speed_range = max_speed - initial_speed;
        if speed_range <= 0.0 || profile.acceleration <= 0.0 {
            return initial_speed * time;
        }

        let ramp_duration = speed_range / profile.acceleration;
        let ramp_time = time.min(ramp_duration);
        let ramp_distance = if profile.smooth_acceleration {
            let progress = ramp_time / ramp_duration;
            initial_speed * ramp_time
                + speed_range * ramp_duration * Self::smootherstep_integral(progress)
        } else {
            initial_speed * ramp_time + 0.5 * profile.acceleration * ramp_time * ramp_time
        };
        ramp_distance + max_speed * (time - ramp_time)
    }

    /// Integral from zero to `u` of `6u⁵ - 15u⁴ + 10u³`.
    fn smootherstep_integral(u: f64) -> f64 {
        let u2 = u * u;
        let u4 = u2 * u2;
        u4 * (u2 - 3.0 * u + 2.5)
    }

    /// Move once immediately so a press-and-release faster than the next
    /// display update remains observable without synthesising a frame interval.
    fn tap(&mut self, directions: DirectionMask, profile: &Pointer, multiplier: f64) -> (f64, f64) {
        self.advance(directions, profile.tap_distance * multiplier)
    }

    fn advance(&mut self, directions: DirectionMask, distance: f64) -> (f64, f64) {
        let (dx, dy) = directions.delta();
        // Opposed keys cancel out; nothing to do.
        if dx == 0.0 && dy == 0.0 || distance <= 0.0 {
            return (0.0, 0.0);
        }
        // Normalise so diagonal travel matches axis-aligned travel.
        let length = (dx * dx + dy * dy).sqrt();
        self.remainder_x += dx / length * distance;
        self.remainder_y += dy / length * distance;

        let out_x = self.remainder_x.trunc();
        let out_y = self.remainder_y.trunc();
        self.remainder_x -= out_x;
        self.remainder_y -= out_y;
        (out_x, out_y)
    }
}

pub struct NormalMode {
    profile: Pointer,
    scroll: Scroll,
    passthrough_unbound_keys: bool,

    /// Directions currently held, keyed by the key holding them so releasing
    /// the right key stops the right direction.
    moving: BTreeMap<Key, Direction>,
    /// Scroll gestures currently held.
    scrolling: BTreeMap<Key, (Direction, ScrollAmount)>,
    /// Speed modifiers currently held.
    speeds: BTreeMap<Key, Speed>,

    motion: Motion,
    /// Once the first native display update arrives, OS key repeats are
    /// ignored. Before that they remain a fallback without a display link.
    frame_driven: bool,
    /// Real timestamp for the keyboard-repeat fallback.
    fallback_tick: Option<Instant>,
}

impl NormalMode {
    pub fn new(config: &Config) -> Self {
        Self {
            profile: config.pointer.clone(),
            scroll: config.scroll.clone(),
            passthrough_unbound_keys: config.normal.passthrough_unbound_keys,
            moving: BTreeMap::new(),
            scrolling: BTreeMap::new(),
            speeds: BTreeMap::new(),
            motion: Motion::default(),
            frame_driven: false,
            fallback_tick: None,
        }
    }

    /// Current speed multiplier. Fast wins if both are held.
    fn multiplier(&self) -> f64 {
        if self.speeds.values().any(|s| *s == Speed::Precision) {
            self.profile.precision_multiplier
        } else if self.speeds.values().any(|s| *s == Speed::Fast) {
            self.profile.fast_multiplier
        } else if self.speeds.values().any(|s| *s == Speed::Slow) {
            self.profile.slow_multiplier
        } else {
            1.0
        }
    }

    fn directions(&self) -> DirectionMask {
        self.moving.values().copied().collect()
    }

    fn binding(&mut self, binding: &Binding, state: KeyState, key: &Key) -> Vec<Command> {
        let mut out = Vec::new();
        let pressed = state == KeyState::Down;

        match binding {
            Binding::Move(direction) => {
                if pressed {
                    let was_still = self.moving.is_empty();
                    let is_first_press = self.moving.insert(key.clone(), *direction).is_none();
                    if !is_first_press && self.frame_driven {
                        // Native display updates own held movement. Keyboard
                        // autorepeat is only a fallback when no update arrives.
                        return out;
                    }

                    let now = Instant::now();
                    let directions = self.directions();
                    let (dx, dy) = if is_first_press {
                        self.motion.reset();
                        self.fallback_tick = Some(now);
                        self.motion
                            .tap(directions, &self.profile, self.multiplier())
                    } else {
                        let elapsed = now.saturating_duration_since(
                            self.fallback_tick.replace(now).unwrap_or(now),
                        );
                        self.motion
                            .step(directions, &self.profile, self.multiplier(), elapsed)
                    };
                    if dx != 0.0 || dy != 0.0 {
                        out.push(Command::MovePointer { dx, dy });
                    }
                    if was_still {
                        self.frame_driven = false;
                        out.push(Command::SetFrameClock(true));
                    }
                } else if self.moving.remove(key).is_some() && self.moving.is_empty() {
                    self.motion.reset();
                    self.frame_driven = false;
                    self.fallback_tick = None;
                    out.push(Command::SetFrameClock(false));
                }
            }

            Binding::Scroll(direction, amount) => {
                if pressed {
                    self.scrolling.insert(key.clone(), (*direction, *amount));
                    // Each key-down is either the initial press or a repeat
                    // generated by the operating system.
                    out.push(self.scroll_command(*direction, *amount));
                } else {
                    self.scrolling.remove(key);
                }
            }

            Binding::Speed(speed) => {
                if pressed {
                    self.speeds.insert(key.clone(), *speed);
                } else {
                    self.speeds.remove(key);
                }
            }

            // The engine handles every other verb before it reaches a mode.
            _ => {}
        }
        out
    }

    fn scroll_command(&self, direction: Direction, amount: ScrollAmount) -> Command {
        let (dx, dy) = direction.delta();
        let pixels = self.scroll.pixels(amount) * self.multiplier();
        Command::Scroll {
            dx: dx * pixels,
            dy: dy * pixels,
        }
    }

    fn release_all(&mut self) -> bool {
        let was_moving = !self.moving.is_empty();
        self.moving.clear();
        self.scrolling.clear();
        self.speeds.clear();
        self.motion.reset();
        self.frame_driven = false;
        self.fallback_tick = None;
        was_moving
    }

    fn frame(&mut self, elapsed: Duration) -> Vec<Command> {
        self.frame_driven = true;
        self.fallback_tick = None;
        if self.moving.is_empty() {
            return vec![Command::SetFrameClock(false)];
        }
        let directions = self.directions();
        let (dx, dy) = self
            .motion
            .step(directions, &self.profile, self.multiplier(), elapsed);
        if dx == 0.0 && dy == 0.0 {
            Vec::new()
        } else {
            vec![Command::MovePointer { dx, dy }]
        }
    }
}

impl Mode for NormalMode {
    fn id(&self) -> ModeId {
        ModeId::normal()
    }

    fn display_name(&self) -> String {
        "Normal".into()
    }

    /// Bound keys are consumed by the engine. This controls only what happens
    /// to keys that do not match a complete binding.
    fn captures_keyboard(&self) -> bool {
        !self.passthrough_unbound_keys
    }

    fn indicator_color(&self, palette: &Palette) -> Option<Color> {
        Some(palette.accent)
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> Vec<Command> {
        match event {
            ModeEvent::Activated { .. } => {
                self.release_all();
                // Draw an empty scene so the engine can attach the indicator.
                vec![Command::ShowOverlay(
                    crate::api::overlay::OverlayScene::new(),
                )]
            }
            ModeEvent::Deactivated => {
                self.release_all();
                Vec::new()
            }
            ModeEvent::Binding {
                binding,
                state,
                key,
            } => self.binding(binding, *state, key),
            ModeEvent::Frame { elapsed } => self.frame(*elapsed),
            ModeEvent::ScreenRetargeted { screen, .. } => {
                vec![Command::warp_to(screen.bounds.center())]
            }
            ModeEvent::Resumed => vec![Command::ShowOverlay(
                crate::api::overlay::OverlayScene::new(),
            )],
            ModeEvent::ConfigReloaded => {
                let Some(config) = ctx.config.downcast_ref::<Config>() else {
                    return Vec::new();
                };
                *self = Self::new(config);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::{Point, Rect, Screen};
    use std::time::Duration;

    struct Env {
        screens: Vec<Screen>,
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
                    bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                    work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
                    is_primary: true,
                    scale: 1.0,
                    name: None,
                }],
                palette: Palette::default(),
                config,
            }
        }
        fn ctx(&self) -> HostContext<'_> {
            HostContext {
                screens: &self.screens,
                cursor: Point::new(500.0, 400.0),
                focused_app: None,
                palette: &self.palette,
                config: &self.config,
            }
        }
    }

    fn send(
        mode: &mut NormalMode,
        env: &Env,
        binding: Binding,
        state: KeyState,
        key: &str,
    ) -> Vec<Command> {
        mode.handle(
            &ModeEvent::Binding {
                binding,
                state,
                key: Key::new(key).unwrap(),
            },
            &env.ctx(),
        )
    }

    fn down(mode: &mut NormalMode, env: &Env, binding: Binding, key: &str) -> Vec<Command> {
        send(mode, env, binding, KeyState::Down, key)
    }

    fn up(mode: &mut NormalMode, env: &Env, binding: Binding, key: &str) -> Vec<Command> {
        send(mode, env, binding, KeyState::Up, key)
    }

    fn horizontal(commands: &[Command]) -> Option<f64> {
        commands.iter().find_map(|command| match command {
            Command::MovePointer { dx, .. } => Some(*dx),
            _ => None,
        })
    }

    #[test]
    fn normal_capture_policy_follows_unbound_passthrough() {
        let mut config = Config::default();
        assert!(!NormalMode::new(&config).captures_keyboard());
        config.normal.passthrough_unbound_keys = false;
        assert!(NormalMode::new(&config).captures_keyboard());
    }

    #[test]
    fn config_reload_updates_normal_capture_policy() {
        let mut mode = NormalMode::new(&Config::default());
        let mut config = Config::default();
        config.normal.passthrough_unbound_keys = false;
        let env = Env::with(config);

        let _ = mode.handle(&ModeEvent::ConfigReloaded, &env.ctx());

        assert!(mode.captures_keyboard());
    }

    #[test]
    fn first_press_moves_once_without_arming_a_timer() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        let out = down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        assert!(horizontal(&out).is_some_and(|dx| dx > 0.0), "{out:?}");
        assert!(!out.iter().any(|command| matches!(
            command,
            Command::SetTimer { .. } | Command::CancelTimer { .. }
        )));
    }

    #[test]
    fn release_stops_the_gesture_and_display_clock() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        let out = up(&mut mode, &env, Binding::Move(Direction::Right), "l");
        assert_eq!(out, vec![Command::SetFrameClock(false)]);
        assert!(mode.moving.is_empty());
    }

    #[test]
    fn linear_acceleration_respects_max_speed() {
        let mut profile = Config::default().pointer;
        profile.initial_speed = 100.0;
        profile.max_speed = 200.0;
        profile.acceleration = 500.0;
        profile.smooth_acceleration = false;
        let directions = [Direction::Right].into_iter().collect();
        let mut motion = Motion::default();
        let elapsed = Duration::from_millis(100);

        let first = motion.step(directions, &profile, 1.0, elapsed).0;
        let second = motion.step(directions, &profile, 1.0, elapsed).0;
        let third = motion.step(directions, &profile, 1.0, elapsed).0;
        assert_eq!((first, second, third), (12.0, 18.0, 20.0));
        assert!((motion.elapsed_seconds - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn smooth_acceleration_eases_the_ramp_without_changing_total_ramp_travel() {
        let mut smooth = Config::default().pointer;
        smooth.initial_speed = 100.0;
        smooth.max_speed = 200.0;
        smooth.acceleration = 100.0;
        smooth.smooth_acceleration = true;
        let mut linear = smooth.clone();
        linear.smooth_acceleration = false;

        let smooth_first_quarter = Motion::distance(&smooth, 0.0, 0.25);
        let linear_first_quarter = Motion::distance(&linear, 0.0, 0.25);
        assert!((smooth_first_quarter - 25.708_007_812_5).abs() < 1e-9);
        assert!(smooth_first_quarter < linear_first_quarter);
        assert!((Motion::distance(&smooth, 0.0, 1.0) - 150.0).abs() < 1e-9);
        assert!((Motion::distance(&smooth, 1.0, 1.25) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn smooth_acceleration_integrates_across_the_cruise_boundary() {
        let mut profile = Config::default().pointer;
        profile.initial_speed = 100.0;
        profile.max_speed = 200.0;
        profile.acceleration = 100.0;

        let whole = Motion::distance(&profile, 0.0, 1.25);
        let split = Motion::distance(&profile, 0.0, 0.75) + Motion::distance(&profile, 0.75, 1.25);
        assert!((whole - split).abs() < 1e-9);
        assert!((whole - 200.0).abs() < 1e-9);
    }

    #[test]
    fn zero_acceleration_and_inverted_speed_range_use_constant_speed() {
        let mut profile = Config::default().pointer;
        profile.initial_speed = 100.0;
        profile.max_speed = 200.0;
        profile.acceleration = 0.0;
        assert!((Motion::distance(&profile, 0.0, 2.0) - 200.0).abs() < 1e-9);

        profile.initial_speed = 300.0;
        profile.acceleration = 100.0;
        assert!((Motion::distance(&profile, 0.0, 2.0) - 400.0).abs() < 1e-9);
    }

    #[test]
    fn equal_wall_clock_time_has_equal_travel_at_any_update_cadence() {
        let mut profile = Config::default().pointer;
        profile.initial_speed = 100.0;
        profile.max_speed = 600.0;
        profile.acceleration = 500.0;
        let directions = [Direction::Right].into_iter().collect();

        let distance = |updates: usize, elapsed: Duration| {
            let mut motion = Motion::default();
            (0..updates)
                .map(|_| motion.step(directions, &profile, 1.0, elapsed).0)
                .sum::<f64>()
        };
        assert_eq!(
            distance(20, Duration::from_millis(50)),
            distance(5, Duration::from_millis(200))
        );
    }

    #[test]
    fn native_display_updates_take_over_from_keyboard_repeat() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        let frame = mode.handle(
            &ModeEvent::Frame {
                elapsed: Duration::from_millis(20),
            },
            &env.ctx(),
        );
        assert!(horizontal(&frame).is_some(), "{frame:?}");

        let repeat = down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        assert!(
            repeat.is_empty(),
            "native display updates should own motion: {repeat:?}"
        );
    }

    #[test]
    fn diagonal_travel_is_normalised() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        down(&mut mode, &env, Binding::Move(Direction::Down), "j");

        let (mut x, mut y) = (0.0, 0.0);
        for _ in 0..10 {
            let out = mode.handle(
                &ModeEvent::Frame {
                    elapsed: Duration::from_millis(20),
                },
                &env.ctx(),
            );
            if let [Command::MovePointer { dx, dy }] = out.as_slice() {
                x += dx;
                y += dy;
            }
        }
        assert!(x > 0.0 && y > 0.0, "expected diagonal motion: {x},{y}");
        assert!(
            (x - y).abs() <= 2.0,
            "axes should advance together: {x},{y}"
        );
    }

    #[test]
    fn opposed_directions_cancel() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Left), "h");
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        for _ in 0..5 {
            let out = down(&mut mode, &env, Binding::Move(Direction::Right), "l");
            assert!(
                !out.iter().any(|c| matches!(c, Command::MovePointer { .. })),
                "{out:?}"
            );
        }
    }

    #[test]
    fn releasing_one_key_keeps_the_other_direction() {
        // Keys are tracked individually, so releasing `l` must not stop `j`.
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        down(&mut mode, &env, Binding::Move(Direction::Down), "j");
        up(&mut mode, &env, Binding::Move(Direction::Right), "l");

        let out = mode.handle(
            &ModeEvent::Frame {
                elapsed: Duration::from_millis(20),
            },
            &env.ctx(),
        );
        match out.as_slice() {
            [Command::MovePointer { dx, dy }] => {
                assert_eq!(*dx, 0.0, "horizontal motion should have stopped");
                assert!(*dy > 0.0, "vertical motion should continue");
            }
            other => panic!("expected downward motion, got {other:?}"),
        }
    }

    #[test]
    fn direction_change_restarts_acceleration() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        for _ in 0..10 {
            down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        }
        up(&mut mode, &env, Binding::Move(Direction::Right), "l");
        let out = down(&mut mode, &env, Binding::Move(Direction::Left), "h");

        let first = horizontal(&out)
            .map(f64::abs)
            .unwrap_or_else(|| panic!("expected movement, got {out:?}"));
        assert!(
            first <= env.config.pointer.tap_distance,
            "new direction should start at the configured tap distance, got {first}"
        );
    }

    #[test]
    fn scrolling_fires_on_initial_and_os_repeat_key_downs() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        let out = down(
            &mut mode,
            &env,
            Binding::Scroll(Direction::Down, ScrollAmount::Step),
            "e",
        );
        assert!(
            out.iter()
                .any(|c| matches!(c, Command::Scroll { dy, .. } if *dy > 0.0)),
            "{out:?}"
        );
        assert!(
            down(
                &mut mode,
                &env,
                Binding::Scroll(Direction::Down, ScrollAmount::Step),
                "e",
            )
            .iter()
            .any(|c| matches!(c, Command::Scroll { dy, .. } if *dy > 0.0))
        );
    }

    #[test]
    fn releasing_a_scroll_key_stops_scrolling() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        let binding = Binding::Scroll(Direction::Down, ScrollAmount::Step);
        down(&mut mode, &env, binding.clone(), "e");
        let out = up(&mut mode, &env, binding, "e");
        assert!(out.is_empty());
        assert!(mode.scrolling.is_empty());
    }

    #[test]
    fn scroll_amounts_use_their_configured_distance() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        let out = down(
            &mut mode,
            &env,
            Binding::Scroll(Direction::Down, ScrollAmount::Half),
            "alt+e",
        );
        let expected = env.config.scroll.scroll_step_half as f64;
        assert!(
            out.iter()
                .any(|c| matches!(c, Command::Scroll { dy, .. } if *dy == expected)),
            "{out:?}, expected dy {expected}"
        );
    }

    #[test]
    fn speed_modifiers_scale_vertical_and_horizontal_scrolling() {
        let env = Env::new();
        for (speed, multiplier) in [
            (Speed::Slow, env.config.pointer.slow_multiplier),
            (Speed::Fast, env.config.pointer.fast_multiplier),
        ] {
            for (direction, axis) in [
                (Direction::Up, (0.0, -1.0)),
                (Direction::Down, (0.0, 1.0)),
                (Direction::Left, (-1.0, 0.0)),
                (Direction::Right, (1.0, 0.0)),
            ] {
                let mut mode = NormalMode::new(&env.config);
                down(&mut mode, &env, Binding::Speed(speed), "shift");
                let out = down(
                    &mut mode,
                    &env,
                    Binding::Scroll(direction, ScrollAmount::Step),
                    "e",
                );
                let expected = env.config.scroll.scroll_step as f64 * multiplier;
                assert!(
                    out.iter().any(|command| matches!(
                        command,
                        Command::Scroll { dx, dy }
                            if *dx == axis.0 * expected && *dy == axis.1 * expected
                    )),
                    "{speed:?} {direction:?}: {out:?}"
                );
            }
        }
    }

    /// Total horizontal travel over equal wall-clock intervals, optionally
    /// holding a modifier.
    fn travel_right(env: &Env, speed: Option<Speed>, updates: usize) -> f64 {
        let mut mode = NormalMode::new(&env.config);
        if let Some(speed) = speed {
            down(&mut mode, env, Binding::Speed(speed), "shift");
        }
        down(&mut mode, env, Binding::Move(Direction::Right), "l");

        let mut total = 0.0;
        for _ in 0..updates {
            let out = mode.handle(
                &ModeEvent::Frame {
                    elapsed: Duration::from_millis(20),
                },
                &env.ctx(),
            );
            if let Some(dx) = horizontal(&out) {
                total += dx;
            }
        }
        total
    }

    #[test]
    fn speed_modifiers_scale_travel_in_both_directions() {
        let env = Env::new();
        let normal = travel_right(&env, None, 8);
        let slow = travel_right(&env, Some(Speed::Slow), 8);
        let fast = travel_right(&env, Some(Speed::Fast), 8);

        assert!(
            slow < normal,
            "slow ({slow}) should trail normal ({normal})"
        );
        assert!(fast > normal, "fast ({fast}) should lead normal ({normal})");
    }

    #[test]
    fn slow_modifier_still_moves_the_pointer() {
        // Sub-pixel speeds must accumulate rather than round away to nothing.
        let env = Env::new();
        assert!(travel_right(&env, Some(Speed::Slow), 8) > 0.0);
    }

    #[test]
    fn deactivation_clears_state_without_canceling_a_timer() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        let out = mode.handle(&ModeEvent::Deactivated, &env.ctx());
        assert!(out.is_empty());
        assert!(mode.moving.is_empty());
        assert!(mode.speeds.is_empty());
    }

    #[test]
    fn unrelated_bindings_are_left_to_the_engine() {
        // Clicks and mode switches never reach a mode, so handling them here
        // would be dead code.
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        for binding in [
            Binding::Click(crate::api::binding::Button::Left),
            Binding::Mode(ModeId::grid()),
            Binding::Escape,
        ] {
            assert!(
                down(&mut mode, &env, binding.clone(), "x").is_empty(),
                "{binding:?} should be the engine's job"
            );
        }
    }

    #[test]
    fn timer_events_never_drive_normal_mode() {
        let env = Env::new();
        let mut mode = NormalMode::new(&env.config);
        down(&mut mode, &env, Binding::Move(Direction::Right), "l");
        let out = mode.handle(
            &ModeEvent::Timer {
                id: "unrelated.plugin.timer".into(),
                elapsed: Duration::from_secs(1),
            },
            &env.ctx(),
        );
        assert!(
            out.is_empty(),
            "normal movement must only follow key events"
        );
    }
}
