#![forbid(unsafe_code)]

//! Cross-cutting, platform-neutral infrastructure.

pub(crate) mod errors;
pub(crate) mod logging;
pub(crate) mod perf_probe;
pub(crate) mod worker;
