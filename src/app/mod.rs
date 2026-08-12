#![forbid(unsafe_code)]

//! Application assembly and runtime orchestration.

pub(crate) mod about;
mod bootstrap;
mod cli;
pub(crate) mod logging;
pub(crate) mod paths;
pub(crate) mod perf_probe;
pub mod runtime;
pub(crate) mod update;

pub use cli::{prepare_console_for_cli, run_cli};
