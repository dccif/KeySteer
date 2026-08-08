#![forbid(unsafe_code)]

//! The public API: the single vocabulary shared by the engine, the built-in
//! modes, the platform backends and third-party plugins.
//!
//! Everything a mode may do is expressed here. There is no privileged
//! back-channel for built-in modes, which is what makes the five shipped modes
//! (`idle`, `normal`, `grid`, `recursive_grid`, `ui_hint`) ordinary consumers of
//! this API and lets a plugin build a full-screen grid of its own.

pub mod autostart;
pub mod backend;
pub mod binding;
pub mod command;
pub mod geometry;
pub mod hint;
pub mod input;
pub mod overlay;
pub mod plugin;
pub mod theme;

pub use autostart::Autostart;
pub use backend::{
    Appearance, Backend, BackendEvent, KeyDisposition, UpdateCheckResult, UpdateProgress,
};
pub use binding::{
    Action, ActionPhase, ActionSequence, Binding, Button, DEFAULT_WAIT_MS, Direction, InputTarget,
    ScrollAmount, Speed,
};
pub use command::{
    ButtonAction, Command, FinishCause, FocusedApp, HostContext, HostSettings, Mode, ModeEvent,
    MouseButton, UiScanRequest, UiScanResult, UiScanStatus, UiScanStrategy, VisionOptions,
};
pub use geometry::{Point, Rect, Screen, UiTarget};
pub use hint::LabelDirection;
pub use input::{InputEvent, Key, KeyChord, KeyState, ModeId};
pub use overlay::{
    Color, Indicator, LabelStyle, OverlayLabel, OverlayScene, OverlayShape, Placement,
};
pub use plugin::{API_VERSION, Manifest, Plugin};
pub use theme::Palette;
