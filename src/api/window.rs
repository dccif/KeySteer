//! Window management vocabulary. Native handles never cross this boundary.
use super::command::WindowScreenTarget;
use super::{Direction, Point, Rect};

/// Normalized placement rectangles, shared by the preview and native placement.
pub const LAYOUTS: [(&str, Rect); 12] = [
    ("Full", Rect::new(0.0, 0.0, 1.0, 1.0)),
    ("Left half", Rect::new(0.0, 0.0, 0.5, 1.0)),
    ("Right half", Rect::new(0.5, 0.0, 0.5, 1.0)),
    ("Top half", Rect::new(0.0, 0.0, 1.0, 0.5)),
    ("Bottom half", Rect::new(0.0, 0.5, 1.0, 0.5)),
    ("Top left", Rect::new(0.0, 0.0, 0.5, 0.5)),
    ("Top right", Rect::new(0.5, 0.0, 0.5, 0.5)),
    ("Bottom left", Rect::new(0.0, 0.5, 0.5, 0.5)),
    ("Bottom right", Rect::new(0.5, 0.5, 0.5, 0.5)),
    ("Left third", Rect::new(0.0, 0.0, 1.0 / 3.0, 1.0)),
    ("Center third", Rect::new(1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)),
    ("Right third", Rect::new(2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub app: String,
    pub bounds: Rect,
    pub screen: usize,
    pub resizable: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub fullscreen: bool,
}

/// Allocate new numbers in application batches while preserving every existing
/// number. The acquired application's existing number keeps its batch first.
pub(crate) fn application_number_order<'a>(
    windows: impl IntoIterator<Item = &'a WindowInfo>,
    numbers: impl IntoIterator<Item = (WindowId, u32)>,
) -> Vec<&'a WindowInfo> {
    let mut windows: Vec<_> = windows.into_iter().collect();
    let numbers: std::collections::BTreeMap<_, _> = numbers.into_iter().collect();
    let mut first = std::collections::BTreeMap::<String, u32>::new();
    for window in &windows {
        if let Some(number) = numbers.get(&window.id) {
            first
                .entry(window.app.to_lowercase())
                .and_modify(|value| *value = (*value).min(*number))
                .or_insert(*number);
        }
    }
    windows.sort_by_cached_key(|window| {
        let app = window.app.to_lowercase();
        (first.get(&app).copied().unwrap_or(u32::MAX), app)
    });
    windows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    Left,
    Down,
    Up,
    Right,
    Size,
    Navigate(Direction),
    Split(Direction),
    Ratio(Direction),
    Tile,
    NextScreen,
    PreviousScreen,
    CycleState,
    Center,
    Close,
    Select,
    SelectPrevious,
    Undo,
    Redo,
    ResetInitial,
    RemoveRegion,
    SaveLayout,
    DeletePreset,
    Confirm,
    TabEnd,
    TabPrefix,
    TabSeparator,
    TabRemove,
    TabDissolve,
    TabNext,
    TabPrevious,
    TabMoveLeft,
    TabMoveRight,
}

impl WindowAction {
    pub const fn is_held(self) -> bool {
        matches!(
            self,
            Self::Left | Self::Down | Self::Up | Self::Right | Self::Ratio(_)
        )
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Left => "window_left",
            Self::Down => "window_down",
            Self::Up => "window_up",
            Self::Right => "window_right",
            Self::Size => "window_size",
            Self::Navigate(Direction::Left) => "window_layout_left",
            Self::Navigate(Direction::Down) => "window_layout_down",
            Self::Navigate(Direction::Up) => "window_layout_up",
            Self::Navigate(Direction::Right) => "window_layout_right",
            Self::Split(Direction::Left) => "window_split_left",
            Self::Split(Direction::Down) => "window_split_down",
            Self::Split(Direction::Up) => "window_split_up",
            Self::Split(Direction::Right) => "window_split_right",
            Self::Ratio(Direction::Left) => "window_ratio_left",
            Self::Ratio(Direction::Down) => "window_ratio_down",
            Self::Ratio(Direction::Up) => "window_ratio_up",
            Self::Ratio(Direction::Right) => "window_ratio_right",
            Self::Tile => "window_tile",
            Self::NextScreen => "window_screen_next",
            Self::PreviousScreen => "window_screen_previous",
            Self::CycleState => "size_cycle",
            Self::Center => "window_center",
            Self::Close => "window_close",
            Self::Select => "window_select",
            Self::SelectPrevious => "window_select_previous",
            Self::Undo => "window_undo",
            Self::Redo => "window_redo",
            Self::ResetInitial => "window_reset_initial",
            Self::RemoveRegion => "window_remove_region",
            Self::SaveLayout => "window_save_layout",
            Self::DeletePreset => "window_delete",
            Self::Confirm => "window_confirm",
            Self::TabEnd => "window_tab_end",
            Self::TabPrefix => "window_tab_group",
            Self::TabSeparator => "window_number_end",
            Self::TabRemove => "window_tab_remove",
            Self::TabDissolve => "window_tab_dissolve",
            Self::TabNext => "window_tab_next",
            Self::TabPrevious => "window_tab_previous",
            Self::TabMoveLeft => "window_tab_move_left",
            Self::TabMoveRight => "window_tab_move_right",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Left,
            Self::Down,
            Self::Up,
            Self::Right,
            Self::Size,
            Self::Navigate(Direction::Left),
            Self::Navigate(Direction::Down),
            Self::Navigate(Direction::Up),
            Self::Navigate(Direction::Right),
            Self::Split(Direction::Left),
            Self::Split(Direction::Down),
            Self::Split(Direction::Up),
            Self::Split(Direction::Right),
            Self::Ratio(Direction::Left),
            Self::Ratio(Direction::Down),
            Self::Ratio(Direction::Up),
            Self::Ratio(Direction::Right),
            Self::Tile,
            Self::NextScreen,
            Self::PreviousScreen,
            Self::CycleState,
            Self::Center,
            Self::Close,
            Self::Select,
            Self::SelectPrevious,
            Self::Undo,
            Self::Redo,
            Self::ResetInitial,
            Self::RemoveRegion,
            Self::SaveLayout,
            Self::DeletePreset,
            Self::Confirm,
            Self::TabEnd,
            Self::TabPrefix,
            Self::TabSeparator,
            Self::TabRemove,
            Self::TabDissolve,
            Self::TabNext,
            Self::TabPrevious,
            Self::TabMoveLeft,
            Self::TabMoveRight,
        ]
        .into_iter()
        .find(|action| action.name() == value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowChange {
    Move { dx: f64, dy: f64 },
    Resize { dw: f64, dh: f64 },
    Place { index: usize, gap: f64 },
    Center,
    CycleState,
    Screen(WindowScreenTarget),
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowOperation {
    Tabs(super::window_tabs::TabOperation),
    /// Lock and activate the ordinary window under the physical pointer.
    Acquire(Point),
    /// Cancel older queued/in-flight operations while preserving target/history.
    CancelPending,
    Enumerate,
    Select(WindowId),
    /// Ask the application to close this window, preserving save/cancel dialogs.
    Close(WindowId),
    /// Query eligible windows and immediately lock/activate the next one.
    Cycle,
    CyclePrevious,
    /// Capture native restore state and constraints before a live edit.
    BeginEdit {
        transaction: u64,
        targets: Vec<WindowId>,
        /// When present, refresh the inventory and capture every eligible
        /// resizable window on this screen instead of relying on cached targets.
        screen: Option<usize>,
        group: u64,
    },
    /// Latest absolute geometry, in normalized screen work-area coordinates.
    ApplyLayout {
        additional_screens: Vec<WindowScreenLayout>,
        transaction: u64,
        revision: u64,
        screen: usize,
        placements: Vec<(WindowId, Rect)>,
        gap: f64,
        strict: bool,
    },
    EndEdit {
        transaction: u64,
        commit: bool,
    },
    Adjust {
        target: WindowId,
        change: WindowChange,
        group: u64,
    },
    Tile {
        target: WindowId,
        gap: f64,
        group: u64,
    },
    Undo,
    Redo,
    /// Restore windows changed in this session to their first observed native state.
    ResetInitial {
        group: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowScreenLayout {
    pub screen: usize,
    pub placements: Vec<(WindowId, Rect)>,
}

impl WindowOperation {
    pub fn precedes_inventory(&self) -> bool {
        matches!(
            self,
            Self::Tabs(_)
                | Self::BeginEdit { .. }
                | Self::ApplyLayout { .. }
                | Self::EndEdit { .. }
                | Self::Select(_)
                | Self::Close(_)
                | Self::Cycle
                | Self::CyclePrevious
                | Self::Tile { .. }
                | Self::Undo
                | Self::Redo
                | Self::ResetInitial { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowRequest {
    pub scope: Option<WindowScope>,
    pub session: u64,
    pub id: u64,
    pub operation: WindowOperation,
}

/// Inventory selection shared by numbering, cycling and layout operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowScope {
    pub screen: Option<usize>,
    pub include_minimized: bool,
}
impl WindowScope {
    pub fn contains(self, window: &WindowInfo) -> bool {
        self.screen.is_none_or(|screen| screen == window.screen)
            && (self.include_minimized || !window.minimized)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowResult {
    pub tabs: Option<super::window_tabs::TabState>,
    pub session: u64,
    pub id: u64,
    pub target: Option<WindowInfo>,
    pub windows: Option<Vec<WindowInfo>>,
    /// Retired native identities, distinct from hidden/minimized windows.
    pub closed: Vec<WindowId>,
    pub pointer: Option<Point>,
    pub changed: usize,
    pub skipped: usize,
    pub message: Option<String>,
    pub edit: Option<Box<WindowEditResult>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowEditResult {
    Started {
        transaction: u64,
        minimums: Vec<(WindowId, Point)>,
        gap_scale: f64,
        full_inventory: bool,
    },
    Applied {
        transaction: u64,
        revision: u64,
        accepted: bool,
        minimums: Vec<(WindowId, Point)>,
    },
    Ended {
        transaction: u64,
        committed: bool,
    },
}

#[cfg(test)]
mod state_cycle_tests {
    use super::WindowAction;
    #[test]
    fn size_cycle_is_the_only_supported_state_cycle_name() {
        assert_eq!(
            WindowAction::parse("size_cycle"),
            Some(WindowAction::CycleState)
        );
        assert_eq!(WindowAction::parse("window_maximize"), None);
        assert_eq!(WindowAction::parse("window_cycle_state"), None);
        assert_eq!(WindowAction::CycleState.name(), "size_cycle");
    }
}
