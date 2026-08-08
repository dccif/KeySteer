#![forbid(unsafe_code)]

//! Application assembly and runtime orchestration.

mod bootstrap;
mod cli;
pub(crate) mod logging;
pub(crate) mod paths;
pub mod runtime;
pub(crate) mod update;

pub use cli::{prepare_console_for_cli, run_cli};
