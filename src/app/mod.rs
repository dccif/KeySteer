#![forbid(unsafe_code)]

//! Application assembly and runtime orchestration.

mod bootstrap;
mod cli;
pub(crate) mod configuration;
pub(crate) mod mode_catalog;
pub(crate) mod paths;

pub use cli::{prepare_console_for_cli, run_cli};
