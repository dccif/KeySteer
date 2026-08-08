#![forbid(unsafe_code)]

//! Bundled example plugins.
//!
//! These are ordinary [`Plugin`] implementations built
//! against the public API. They prove the point of the architecture: a plugin
//! composes the same primitives the built-in modes use, so it can create its
//! own grid or full-screen overlay without any special support from the host.

pub mod builtin;

pub use builtin::ScreenSelector;

use crate::api::Plugin;
use crate::config::Config;

/// Instantiate the bundled plugins.
pub fn bundled(config: &Config) -> Result<Vec<Box<dyn Plugin>>, String> {
    Ok(vec![Box::new(ScreenSelector::with_key_aliases(
        config.resolved_key_aliases(),
    )?)])
}
