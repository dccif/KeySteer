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
#[allow(dead_code, unused_imports)]
pub(crate) mod runtime;
pub(crate) mod support;

/// Feature-gated access for the repository's standalone performance harness.
#[cfg(feature = "perf-probe")]
#[doc(hidden)]
pub mod benchmark {
    pub use crate::api::{
        Appearance, Binding, Command, CommandBatch, Direction, HostContext, Key, KeyState,
        LabelDirection, Mode, ModeEvent, Point, Rect, Screen, UiScanResult, UiScanStatus, UiTarget,
    };
    pub use crate::config::Config;
    pub use crate::modes::hint::labeling::{Hint, assign_into};

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
