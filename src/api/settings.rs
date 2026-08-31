//! Immutable settings snapshot exposed to the runtime and modes.
//!
//! The alias is the migration boundary between the persisted TOML model and
//! consumers. Callers use this name rather than type erasure; the underlying
//! representation can be split without changing the Mode contract again.

pub type RuntimeSettings = crate::config::Config;
