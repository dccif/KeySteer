#![forbid(unsafe_code)]

//! Application assembly and runtime orchestration.

mod bootstrap;
mod cli;
pub(crate) mod configuration;
pub(crate) mod mode_catalog;
pub(crate) mod paths;
#[allow(dead_code, unused_imports)]
pub(crate) mod runtime;

pub use cli::{prepare_console_for_cli, run_cli};
