use crate::api::{Command, CommandBatch, FinishCause, LifecycleAction, ModeId};

pub(crate) fn lifecycle_commands(action: &LifecycleAction, return_mode: &ModeId) -> CommandBatch {
    match action {
        LifecycleAction::Keep => CommandBatch::new(),
        LifecycleAction::Finish => CommandBatch::one(Command::FinishMode {
            cause: FinishCause::Click,
        }),
        LifecycleAction::Restart => CommandBatch::one(Command::RestartMode),
        LifecycleAction::Return => CommandBatch::two(
            Command::HideOverlay,
            Command::SwitchMode(return_mode.clone()),
        ),
        LifecycleAction::Mode(mode) => {
            CommandBatch::two(Command::HideOverlay, Command::SwitchMode(mode.clone()))
        }
        LifecycleAction::Click { button, action } => CommandBatch::one(Command::MouseButton {
            button: *button,
            action: *action,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ButtonAction, MouseButton};

    #[test]
    fn lifecycle_actions_do_not_implicitly_reactivate_the_mode() {
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
                lifecycle_commands(&LifecycleAction::Click { button, action }, &return_mode),
                vec![Command::MouseButton { button, action }]
            );
        }
    }
}
