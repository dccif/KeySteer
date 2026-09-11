//! Persistent window groups. Only opaque identities cross the native boundary.
use super::Rect;
use super::window::WindowId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabGroupId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowTarget {
    Window(WindowId),
    Group(TabGroupId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabGroup {
    pub id: TabGroupId,
    pub members: Vec<WindowId>,
    pub active: WindowId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabState {
    pub groups: Vec<TabGroup>,
    /// Session reservations, including temporarily ineligible windows.
    /// Consumers build selectable numbers from the current scoped inventory.
    pub numbers: Vec<(WindowId, u32)>,
    /// The current continuous composition, cleared by the end-group action.
    pub target: Option<WindowTarget>,
    pub active: Option<WindowId>,
}

impl TabState {
    pub fn group(&self, id: TabGroupId) -> Option<&TabGroup> {
        self.groups.iter().find(|group| group.id == id)
    }
    pub fn containing(&self, id: WindowId) -> Option<&TabGroup> {
        self.groups.iter().find(|group| group.members.contains(&id))
    }
    pub fn representative(&self, id: WindowId) -> WindowId {
        self.containing(id).map_or(id, |group| group.members[0])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TabOperation {
    /// Refresh eligible windows and automatically group matching applications.
    Enter {
        screen: usize,
    },
    Choose(WindowTarget),
    EndGroup,
    Activate(WindowId),
    RemoveActive,
    Dissolve,
    Cycle {
        backwards: bool,
    },
    Reorder {
        backwards: bool,
    },
    Undo,
    Redo,
    /// Resolve all selected identities before changing any native placement.
    Restore {
        members: Vec<WindowId>,
        region: Rect,
        screen: usize,
        active: usize,
    },
}

/// Lightweight retained native tab-bar content, separate from mode overlays.
#[derive(Debug, Clone, PartialEq)]
pub struct TabBar {
    pub visible: bool,
    pub group: TabGroupId,
    pub bounds: Rect,
    pub screen: usize,
    pub active: WindowId,
    /// Zero means this member has no number in the current inventory scope.
    /// It remains present and clickable in its persistent native tab strip.
    pub tabs: Vec<(WindowId, u32, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabDrop {
    pub source: WindowTarget,
    pub target: TabGroupId,
    /// Insert before this member, or append at the end.
    pub before: Option<WindowId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabNativeEvent {
    Changed(WindowId),
    Focused(WindowId),
    Closed(WindowId),
    Activate(WindowId),
    /// Closing the independent tab strip dissolves its group and reveals members.
    Dissolve(TabGroupId),
    Drop(TabDrop),
    VisibilityChanged,
}
