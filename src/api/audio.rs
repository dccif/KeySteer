//! Platform-independent audio commands. Backends own all native audio state.
use super::window::WindowId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioAction {
    Down,
    Up,
    ToggleMute,
    DevicePrevious,
    DeviceNext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTarget {
    /// Resolve the application owning a retained window (or its active tab).
    Application(WindowId),
    /// The system default output; no window selection is required.
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRequest {
    /// Caller-owned correlation token, independent of window edit transactions.
    pub session: u64,
    pub id: u64,
    pub target: AudioTarget,
    pub action: AudioAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioResult {
    pub session: u64,
    pub id: u64,
    pub outcome: Result<String, String>,
}
