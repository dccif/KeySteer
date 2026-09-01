//! Keyboard-driven mouse control.
//!
//! # Architecture
//!
//! The crate is a platform-independent core plus interchangeable backends:
//!
//! ```text
//!   config  ──►  engine  ──►  modes (idle / grid / recursive_grid / ui_hint)
//!                  │             │
//!                  │             └─ return Command, receive ModeEvent
//!                  ▼
//!            api::Backend  ──►  platform::{macos, windows, ...}
//! ```
//!
//! [`api`] is the *only* vocabulary in the system. A mode receives
//! [`ModeEvent`]s and returns [`Command`]s; it
//! cannot reach a native API even if it wanted to. Consequently the four
//! built-in modes are ordinary API consumers, and a plugin implementing
//! [`api::Mode`] has exactly the same powers — including drawing its own
//! full-screen grid.
//!
//! Backends implement [`api::Backend`] and are chosen by `cfg(target_os)` in
//! [`platform`], so cross-compiling needs no feature flags or config edits.

pub mod api;
pub mod app;
pub mod config;
pub mod modes;
pub mod platform;
pub mod plugins;
pub mod runtime;
pub(crate) mod support;

// Compatibility alias retained while callers migrate to `runtime::Engine`.
pub use runtime as engine;

pub use api::{
    Action, ActionPhase, ActionSequence, Backend, BackendEvent, Color, Command, CommandBatch,
    FinishCause, HostContext, Key, KeyChord, Mode, ModeEvent, ModeId, OverlayScene, Point, Rect,
    Screen,
};
pub use config::{Config, ConfigError, ConfigFile, Palette, Theme};
pub use engine::Engine;
