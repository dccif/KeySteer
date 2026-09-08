#![forbid(unsafe_code)]

//! The five built-in modes.
//!
//! Each is an ordinary [`Mode`] implementation with no
//! privileged access: they receive [`ModeEvent`](crate::api::ModeEvent)s and
//! return [`Command`]s, exactly as a plugin does. They are
//! therefore also worked examples of how to build a mode.
//!
//! The flow between them:
//!
//! ```text
//!   idle ──alt+e──► normal ──g───────► grid ───────────┐
//!    ▲               │  ▲ ──f───────► recursive_grid  │
//!    │               │   │  ──Primary+f► ui_hint ──────┤
//!    └──────esc──────┘   └───────────────────esc/pick───┘
//! ```
//!
//! `idle` is silent, `normal` does the work, and the three targeting modes each
//! pick a point and hand control back.

pub mod grid;
pub mod hint;
pub mod idle;
pub mod normal;
pub mod recursive_grid;
pub(crate) mod targeting;
pub mod window;

pub use grid::GridMode;
pub use hint::HintMode;
pub use idle::IdleMode;
pub use normal::NormalMode;
pub use recursive_grid::RecursiveGridMode;
