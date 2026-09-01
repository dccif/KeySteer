use smallvec::SmallVec;

use crate::api::{
    Command, CommandBatch, FinishCause, LifecycleAction, ModeId, Rect, TargetingLifecycle,
};

/// Shared session state for grid-style targeting modes. Layout calculation and
/// rendering remain owned by each concrete mode.
pub(crate) struct TargetingSession {
    pub(crate) default_cursor_follow_selection: bool,
    pub(crate) cursor_follow_selection: bool,
    pub(crate) stack: SmallVec<[Rect; 12]>,
    pub(crate) path: SmallVec<[usize; 12]>,
    pub(crate) terminal: bool,
    pub(crate) finished: bool,
    pub(crate) lifecycle: TargetingLifecycle,
    pub(crate) return_mode: ModeId,
}

impl TargetingSession {
    pub(crate) fn new(cursor_follow_selection: bool, lifecycle: TargetingLifecycle) -> Self {
        Self {
            default_cursor_follow_selection: cursor_follow_selection,
            cursor_follow_selection,
            stack: SmallVec::new(),
            path: SmallVec::new(),
            terminal: false,
            finished: false,
            lifecycle,
            return_mode: ModeId::idle(),
        }
    }

    pub(crate) fn depth(&self) -> u32 {
        self.stack.len().saturating_sub(1) as u32
    }

    pub(crate) fn current(&self) -> Option<Rect> {
        self.stack.last().copied()
    }

    pub(crate) fn root(&self) -> Option<Rect> {
        self.stack.first().copied()
    }

    pub(crate) fn reset(&mut self, bounds: Rect) {
        self.stack.clear();
        self.stack.push(bounds);
        self.path.clear();
        self.terminal = false;
        self.finished = false;
        self.cursor_follow_selection = self.default_cursor_follow_selection;
    }

    pub(crate) fn toggle_cursor_follow(&mut self) -> Option<Rect> {
        self.cursor_follow_selection = !self.cursor_follow_selection;
        self.cursor_follow_selection
            .then(|| self.current())
            .flatten()
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
