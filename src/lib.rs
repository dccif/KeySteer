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

#[allow(dead_code, unused_imports)]
pub(crate) mod api;
pub mod app;
#[allow(dead_code, unused_imports)]
pub(crate) mod config;
#[allow(dead_code, unused_imports)]
pub(crate) mod modes;
#[allow(dead_code, unused_imports)]
pub(crate) mod platform;
#[allow(dead_code, unused_imports)]
pub(crate) mod plugins;
pub(crate) mod presentation;
pub(crate) mod support;

/// Doc-hidden access for the repository's standalone release-profile benchmark.
#[cfg(feature = "benchmark-hooks")]
#[doc(hidden)]
pub mod benchmark {
    pub use crate::api::{
        Appearance, Backend, BackendEvent, Binding, ButtonAction, Command, CommandBatch, Direction,
        FocusedApp, HostContext, InputEvent, Key, KeyDisposition, KeyState, LabelDirection, Mode,
        ModeEvent, MouseButton, OverlayScene, Point, Rect, Screen, UiScanRequest, UiScanResult,
        UiScanStatus, UiTarget,
    };
    pub use crate::config::Config;
    pub use crate::presentation::COMPOSER;

    #[cfg(target_os = "windows")]
    pub use crate::platform::windows::{CharacterCapture, observe_character_unfiltered};

    #[cfg(target_os = "macos")]
    pub use crate::platform::macos::{CharacterCaptureProbe, observe_character_unfiltered};

    pub fn engine(config: &Config) -> Result<crate::app::runtime::Engine, String> {
        crate::app::runtime::Engine::from_plan(
            crate::app::configuration::compile(config)?,
            Appearance::Dark,
        )
    }

    pub fn hint(config: &Config) -> crate::modes::HintMode {
        crate::app::mode_catalog::hint(config)
    }

    pub fn normal(config: &Config) -> crate::modes::NormalMode {
        crate::app::mode_catalog::normal(config)
    }
}

#[cfg(test)]
extern crate self as keysteer;

#[cfg(test)]
#[global_allocator]
pub(crate) static TEST_ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> =
    &stats_alloc::INSTRUMENTED_SYSTEM;

#[cfg(test)]
mod tests;
