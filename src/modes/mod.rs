#![forbid(unsafe_code)]

//! The five built-in modes.
//!
//! Each is an ordinary [`Mode`] implementation with no
//! privileged access: they receive [`ModeEvent`](crate::api::ModeEvent)s and
//! return [`Command`]s, exactly as a plugin does. They are
//! therefore also worked examples of how to build a mode.
//!
//! The flow between them:
//!
//! ```text
//!   idle ──alt+e──► normal ──g───────► grid ─────────┐
//!    ▲               │  ▲   ──v───────► recursive_grid │
//!    │               │  │   ──Primary+f► ui_hint ───────┤
//!    └──────esc──────┘  └───────────────────esc/pick───┘
//! ```
//!
//! `idle` is silent, `normal` does the work, and the three targeting modes each
//! pick a point and hand control back.

pub mod grid;
pub mod hint;
pub mod idle;
pub mod normal;
pub mod recursive_grid;

pub use grid::GridMode;
pub use hint::HintMode;
pub use idle::IdleMode;
pub use normal::NormalMode;
pub use recursive_grid::RecursiveGridMode;

use crate::api::{Command, FinishCause, Mode};
use crate::config::{Config, LifecycleAction};

/// Instantiate the built-in modes enabled by `config`.
///
/// `idle` and `normal` are always present: they are the resting state and the
/// working state, and every other mode returns to one of them.
pub fn built_in(config: &Config) -> Vec<Box<dyn Mode>> {
    let mut modes: Vec<Box<dyn Mode>> = vec![
        Box::new(IdleMode::new(config)),
        Box::new(NormalMode::new(config)),
    ];
    if config.grid.enabled {
        modes.push(Box::new(GridMode::new(config)));
    }
    if config.recursive_grid.enabled {
        modes.push(Box::new(RecursiveGridMode::new(config)));
    }
    if config.ui_hint.enabled {
        modes.push(Box::new(HintMode::new(config)));
    }
    modes
}

fn lifecycle_commands(action: &LifecycleAction, return_mode: &crate::api::ModeId) -> Vec<Command> {
    match action {
        LifecycleAction::Keep => Vec::new(),
        LifecycleAction::Finish => vec![Command::FinishMode {
            cause: FinishCause::Click,
        }],
        LifecycleAction::Restart => vec![Command::RestartMode],
        LifecycleAction::Return => vec![
            Command::HideOverlay,
            Command::SwitchMode(return_mode.clone()),
        ],
        LifecycleAction::Mode(mode) => {
            vec![Command::HideOverlay, Command::SwitchMode(mode.clone())]
        }
        LifecycleAction::Click { button, action } => vec![Command::MouseButton {
            button: *button,
            action: *action,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ButtonAction, ModeId, MouseButton};

    #[test]
    fn defaults_register_all_five_modes() {
        let ids: Vec<ModeId> = built_in(&Config::default())
            .iter()
            .map(|m| m.id())
            .collect();
        for expected in [
            ModeId::idle(),
            ModeId::normal(),
            ModeId::grid(),
            ModeId::recursive_grid(),
            ModeId::ui_hint(),
        ] {
            assert!(
                ids.contains(&expected),
                "{expected} is missing from {ids:?}"
            );
        }
    }

    #[test]
    fn disabled_modes_are_not_registered_but_idle_and_normal_survive() {
        let mut config = Config::default();
        config.grid.enabled = false;
        config.recursive_grid.enabled = false;
        config.ui_hint.enabled = false;
        let ids: Vec<ModeId> = built_in(&config).iter().map(|m| m.id()).collect();
        assert_eq!(ids, vec![ModeId::idle(), ModeId::normal()]);
    }

    #[test]
    fn idle_and_default_normal_let_unbound_keystrokes_through() {
        for mode in built_in(&Config::default()) {
            let expected = !matches!(mode.id().as_str(), "idle" | "normal");
            assert_eq!(
                mode.captures_keyboard(),
                expected,
                "{} has the wrong capture policy",
                mode.id()
            );
        }
    }

    #[test]
    fn lifecycle_actions_map_without_implicitly_reactivating_the_mode() {
        let return_mode = ModeId::normal();
        assert!(lifecycle_commands(&LifecycleAction::Keep, &return_mode).is_empty());
        assert_eq!(
            lifecycle_commands(&LifecycleAction::Finish, &return_mode),
            vec![Command::FinishMode {
                cause: FinishCause::Click,
            }]
        );
        assert_eq!(
            lifecycle_commands(&LifecycleAction::Restart, &return_mode),
            vec![Command::RestartMode]
        );
        assert_eq!(
            lifecycle_commands(&LifecycleAction::Return, &return_mode),
            vec![
                Command::HideOverlay,
                Command::SwitchMode(return_mode.clone())
            ]
        );

        let plugin = ModeId::new("example:picker").unwrap();
        assert_eq!(
            lifecycle_commands(&LifecycleAction::Mode(plugin.clone()), &return_mode),
            vec![Command::HideOverlay, Command::SwitchMode(plugin)]
        );

        for (button, action) in [
            (MouseButton::Left, ButtonAction::Click),
            (MouseButton::Right, ButtonAction::Click),
            (MouseButton::Middle, ButtonAction::Click),
            (MouseButton::Left, ButtonAction::DoubleClick),
        ] {
            assert_eq!(
                lifecycle_commands(&LifecycleAction::Click { button, action }, &return_mode,),
                vec![Command::MouseButton { button, action }]
            );
        }
    }
}
