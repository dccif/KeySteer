#![forbid(unsafe_code)]

//! The built-in modes.
//!
//! Each is an ordinary [`Mode`] implementation with no
//! privileged access: they receive [`ModeEvent`](crate::api::ModeEvent)s and
//! return [`Command`]s, exactly as a plugin does. They are
//! therefore also worked examples of how to build a mode.
//!
//! The flow between them:
//!
//! ```text
//!   idle ──Primary+e──► normal ──g─────────► grid
//!                        │   ──f─────────► ui_hint
//!                        │   ──Primary+g─► recursive_grid
//!                        └───Primary+f───► bisect
//!   Esc returns targeting modes to their source; q/esc returns normal to idle.
//! ```
//!
//! `idle` is silent, `normal` does the work, and the four targeting modes each
//! pick a point and hand control back.

pub mod grid;
pub mod hint;
pub mod idle;
pub mod normal;
pub(crate) mod normal_targeting;
pub mod quick_switch;
mod scan_region;
pub(crate) mod targeting;
pub mod text_input;
pub mod text_select;
pub mod window;

pub(crate) use grid::GridMode;
pub use hint::HintMode;
pub use idle::IdleMode;
pub use normal::NormalMode;
