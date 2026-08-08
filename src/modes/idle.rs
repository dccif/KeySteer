//! Idle mode: the silent resting state.
//!
//! Idle does nothing at all except wait for the key that enters `normal`. It
//! draws no overlay, arms no timers and captures no keystrokes, so the program
//! is invisible until the user asks for it. That activation key is resolved by
//! the engine from `[hotkeys]`, so idle itself has no logic.

use crate::api::command::{Command, HostContext, Mode, ModeEvent};
use crate::api::input::ModeId;
use crate::config::Config;

pub struct IdleMode;

impl IdleMode {
    pub fn new(_config: &Config) -> Self {
        Self
    }
}

impl Default for IdleMode {
    fn default() -> Self {
        Self
    }
}

impl Mode for IdleMode {
    fn id(&self) -> ModeId {
        ModeId::idle()
    }

    fn display_name(&self) -> String {
        "Idle".into()
    }

    /// Idle must never swallow keystrokes: the user is working in their app.
    fn captures_keyboard(&self) -> bool {
        false
    }

    fn handle(&mut self, event: &ModeEvent, _ctx: &HostContext<'_>) -> Vec<Command> {
        match event {
            // Clear anything the previous mode left on screen.
            ModeEvent::Activated { .. } => vec![Command::HideOverlay],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::geometry::{Point, Rect, Screen};
    use crate::api::input::{Key, KeyState};
    use crate::config::Palette;

    struct Env {
        screens: Vec<Screen>,
        palette: Palette,
        config: Config,
    }

    impl Env {
        fn new() -> Self {
            Self {
                screens: vec![Screen {
                    bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
                    work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
                    is_primary: true,
                    scale: 1.0,
                    name: None,
                }],
                palette: Palette::default(),
                config: Config::default(),
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

    #[test]
    fn idle_never_captures_the_keyboard() {
        assert!(!IdleMode::new(&Config::default()).captures_keyboard());
    }

    #[test]
    fn idle_clears_the_overlay_on_entry() {
        let env = Env::new();
        let mut mode = IdleMode::new(&env.config);
        let out = mode.handle(&ModeEvent::Activated { previous: None }, &env.ctx());
        assert_eq!(out, vec![Command::HideOverlay]);
    }

    #[test]
    fn idle_ignores_everything_else() {
        // The silence guarantee: no timers, no overlays, no pointer commands.
        let env = Env::new();
        let mut mode = IdleMode::new(&env.config);
        let events = [
            ModeEvent::Key {
                key: Key::new("h").unwrap(),
                state: KeyState::Down,
                repeat: false,
            },
            ModeEvent::PointerMoved(Point::new(1.0, 2.0)),
            ModeEvent::ScreensChanged(env.screens.clone()),
            ModeEvent::ConfigReloaded,
            ModeEvent::Deactivated,
        ];
        for event in events {
            assert!(
                mode.handle(&event, &env.ctx()).is_empty(),
                "idle should stay silent for {event:?}"
            );
        }
    }
}
