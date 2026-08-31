//! Shared state and lifecycle mapping for targeting modes.

use crate::api::{Command, CommandBatch, FinishCause, ModeId};
use crate::config::LifecycleAction;

#[derive(Debug, Clone)]
pub(crate) struct TargetingSession {
    pub(crate) finished: bool,
    pub(crate) return_mode: ModeId,
}

impl Default for TargetingSession {
    fn default() -> Self {
        Self {
            finished: false,
            return_mode: ModeId::idle(),
        }
    }
}

impl TargetingSession {
    pub(crate) fn activate(&mut self, previous: Option<&ModeId>) {
        self.return_mode = previous.cloned().unwrap_or_else(ModeId::idle);
        self.finished = false;
    }

    pub(crate) fn restart(&mut self) {
        self.finished = false;
    }

    /// Mark the session finished. Returns false for duplicate finish events.
    pub(crate) fn finish(&mut self) -> bool {
        if self.finished {
            return false;
        }
        self.finished = true;
        true
    }

    pub(crate) fn commands(&self, action: &LifecycleAction) -> CommandBatch {
        lifecycle_commands(action, &self.return_mode)
    }
}

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
    fn activation_restart_and_finish_are_consistent() {
        let mut session = TargetingSession::default();
        session.activate(Some(&ModeId::normal()));
        assert_eq!(session.return_mode, ModeId::normal());
        assert!(session.finish());
        assert!(!session.finish());
        session.restart();
        assert!(!session.finished);
    }

    #[test]
    fn lifecycle_actions_do_not_reactivate_the_mode() {
        let session = TargetingSession {
            finished: true,
            return_mode: ModeId::normal(),
        };
        assert!(session.commands(&LifecycleAction::Keep).is_empty());
        assert_eq!(
            session.commands(&LifecycleAction::Return),
            vec![Command::HideOverlay, Command::SwitchMode(ModeId::normal())]
        );
        assert_eq!(
            session.commands(&LifecycleAction::Click {
                button: MouseButton::Right,
                action: ButtonAction::Click,
            }),
            vec![Command::MouseButton {
                button: MouseButton::Right,
                action: ButtonAction::Click,
            }]
        );
    }
}
