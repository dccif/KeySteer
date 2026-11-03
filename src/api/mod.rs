#![forbid(unsafe_code)]

//! The public API: the single vocabulary shared by the engine, the built-in
//! modes, the platform backends and third-party plugins.
//!
//! Everything a mode may do is expressed here. There is no privileged
//! back-channel for built-in modes, which is what makes the five shipped modes
//! (`idle`, `normal`, `grid`, `recursive_grid`, `ui_hint`) ordinary consumers of
//! this API and lets a plugin build a full-screen grid of its own.

pub mod audio;
pub mod autostart;
pub mod backend;
pub mod binding;
pub mod command;
pub mod geometry;
pub mod hint;
pub mod input;
pub mod lifecycle;
pub mod overlay;
pub mod plugin;
pub mod point_sample;
pub mod presentation;
pub mod scroll;
pub mod style;
pub mod text_edit;
pub mod text_select;
pub(crate) mod text_select_preview;
pub mod theme;
pub mod window;
pub mod window_layout;
pub mod window_presets;
pub mod window_tabs;

pub use autostart::Autostart;
#[cfg(test)]
pub use backend::UpdateCheckResult;
pub use backend::{Appearance, Backend, BackendEvent, UpdateProgress};
#[cfg(test)]
pub use binding::Speed;
pub use binding::{Binding, Direction};
pub use command::{
    ButtonAction, Command, CommandBatch, FinishCause, FocusedApp, HostContext, Mode, ModeEvent,
    MouseButton, UiScanRequest, UiScanScope, UiScanStrategy, VisionOptions,
};
pub use geometry::{Point, Rect, Screen, SemanticRole, UiTarget};
pub use hint::LabelDirection;
pub use input::{Key, KeyChord, KeyState, ModeId};
pub use lifecycle::{LifecycleAction, TargetingLifecycle};
#[cfg(test)]
pub use overlay::OverlayLabel;
pub use overlay::{Color, OverlayScene, OverlayShape};
pub use plugin::Plugin;
pub use style::{BoundaryHighlight, HintPlacement, LabelUi, SearchInputUi};
pub use theme::{Palette, ThemedColor};

#[cfg(test)]
pub use command::UiScanResult;
#[cfg(any(target_os = "windows", test))]
pub use command::UiScanStatus;
