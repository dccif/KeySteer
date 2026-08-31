#![forbid(unsafe_code)]

//! Cross-cutting, platform-neutral infrastructure.
//!
//! These modules are intentionally below `app`, `runtime`, and the native
//! backends so worker ownership, diagnostics, and error aggregation do not
//! create a reverse dependency on the composition root.

pub(crate) mod errors;
pub(crate) mod logging;
pub(crate) mod perf_probe;
pub(crate) mod worker;
