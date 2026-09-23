use smallvec::SmallVec;

use crate::api::{
    Command, CommandBatch, FinishCause, Key, LifecycleAction, ModeId, Point, Rect,
    TargetingLifecycle,
};

/// Shared selection/lifecycle state. Geometry belongs to TargetingController;
/// rendering remains owned by each visible mode.
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

/// Precompiled geometry, shared by visible grids and blind Normal positioning.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    pub rows: usize,
    pub cols: usize,
    pub keys: Vec<char>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerSettings {
    pub depth: u32,
    pub grid_cols: Option<u32>,
    pub grid_rows: Option<u32>,
    pub keys: Option<String>,
}

pub(crate) enum Selection {
    Ignored,
    Changed,
    Cancel,
    Follow(Point),
    Commit(Point),
}

pub(crate) struct TargetingController {
    layouts: SmallVec<[Layout; 1]>,
    pub session: TargetingSession,
    pub max_depth: u32,
    min_size: Option<(f64, f64)>,
}

impl TargetingController {
    pub fn new(
        base: Layout,
        layers: &[LayerSettings],
        max_depth: u32,
        min_size: Option<(f64, f64)>,
        follow: bool,
        lifecycle: TargetingLifecycle,
    ) -> Self {
        let max_depth = max_depth.max(1);
        // Uniform grids store exactly one layout. Recursive overrides are
        // resolved only at construction, never searched on a key/frame event.
        let layouts = if layers.is_empty() {
            smallvec::smallvec![base]
        } else {
            (0..=max_depth)
                .map(|depth| {
                    let mut layout = base.clone();
                    if let Some(layer) = layers.iter().find(|layer| layer.depth == depth) {
                        if let Some(cols) = layer.grid_cols {
                            layout.cols = cols.max(1) as usize;
                        }
                        if let Some(rows) = layer.grid_rows {
                            layout.rows = rows.max(1) as usize;
                        }
                        if let Some(keys) = &layer.keys {
                            layout.keys = keys.chars().collect();
                        }
                    }
                    layout
                })
                .collect()
        };
        let mut session = TargetingSession::new(follow, lifecycle);
        // The first 12 levels stay inline. Larger configured paths reserve once
        // so even deep selection/backtracking is allocation-free afterwards.
        session.stack.reserve(max_depth as usize + 1);
        session.path.reserve(max_depth as usize);
        Self {
            layouts,
            session,
            max_depth,
            min_size,
        }
    }

    pub fn layout_at(&self, depth: u32) -> &Layout {
        &self.layouts[(depth as usize).min(self.layouts.len() - 1)]
    }

    pub fn claims_key(&self, key: &Key) -> bool {
        key.as_char()
            .is_some_and(|ch| self.layouts.iter().any(|layout| layout.keys.contains(&ch)))
    }

    /// Stateless absolute positioning for a one-level silent grid.
    pub fn root_point(&self, key: &Key, bounds: Rect) -> Option<Point> {
        let layout = self.layout_at(0);
        let ch = key.as_char()?;
        let index = layout.keys.iter().position(|candidate| *candidate == ch)?;
        bounds
            .subdivision(layout.rows, layout.cols, index)
            .map(|cell| cell.center())
    }

    pub fn can_descend(&self) -> bool {
        let Some((min_width, min_height)) = self.min_size else {
            return self.session.depth() < self.max_depth;
        };
        if self.session.depth() + 1 >= self.max_depth {
            return false;
        }
        let Some(area) = self.session.current() else {
            return false;
        };
        let layout = self.layout_at(self.session.depth());
        area.width / layout.cols as f64 >= min_width
            && area.height / layout.rows as f64 >= min_height
    }

    pub fn reset(&mut self, bounds: Rect) {
        self.session.reset(bounds);
    }

    pub fn retarget(&mut self, bounds: Rect, preserve: bool) -> Point {
        if !preserve {
            self.reset(bounds);
        } else {
            // Replay in place: retain the path and stack capacities, with no
            // temporary path clone (including paths beyond inline capacity).
            self.session.stack.clear();
            self.session.stack.push(bounds);
            for offset in 0..self.session.path.len() {
                let index = self.session.path[offset];
                let layout = self.layout_at(self.session.depth());
                let Some(cell) = self
                    .session
                    .current()
                    .and_then(|area| area.subdivision(layout.rows, layout.cols, index))
                else {
                    self.session.path.truncate(offset);
                    break;
                };
                self.session.stack.push(cell);
            }
            self.session.terminal = self.session.depth() >= self.max_depth || !self.can_descend();
        }
        self.session.current().unwrap_or(bounds).center()
    }

    pub fn input(&mut self, key: &Key, bounds: Rect) -> Selection {
        match key.as_str() {
            "esc" => return Selection::Cancel,
            "enter" => {
                return self
                    .session
                    .current()
                    .map(|area| Selection::Commit(area.center()))
                    .unwrap_or(Selection::Cancel);
            }
            "backspace" | "tab" => {
                if self.session.stack.len() <= 1 {
                    return Selection::Cancel;
                }
                self.session.stack.pop();
                self.session.path.pop();
                self.session.terminal = false;
                self.session.finished = false;
                return Selection::Changed;
            }
            "space" => {
                let root = if self.min_size.is_some() {
                    Some(bounds)
                } else {
                    self.session.root()
                };
                let Some(root) = root else {
                    return Selection::Cancel;
                };
                self.reset(root);
                return Selection::Changed;
            }
            _ => {}
        }
        if self.session.terminal || self.session.finished {
            return Selection::Ignored;
        }
        let Some(ch) = key.as_char() else {
            return Selection::Ignored;
        };
        let layout = self.layout_at(self.session.depth());
        let Some(index) = layout.keys.iter().position(|candidate| *candidate == ch) else {
            return Selection::Ignored;
        };
        let Some(cell) = self
            .session
            .current()
            .and_then(|area| area.subdivision(layout.rows, layout.cols, index))
        else {
            return Selection::Ignored;
        };
        self.session.stack.push(cell);
        self.session.path.push(index);
        self.session.terminal = self.session.depth() >= self.max_depth || !self.can_descend();
        if self.session.terminal {
            Selection::Commit(cell.center())
        } else if self.session.cursor_follow_selection {
            Selection::Follow(cell.center())
        } else {
            Selection::Changed
        }
    }
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
