//! Platform backends.
//!
//! Exactly one backend is compiled, chosen by `cfg(target_os)`. Adding a
//! platform means adding a module here and one `cfg` arm — nothing above this
//! line changes, and no feature flag or build-config edit is ever required:
//!
//! ```text
//! cargo build                                  # host
//! cargo build --target x86_64-pc-windows-msvc  # Windows
//! cargo build --target aarch64-apple-darwin    # macOS
//! ```

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(any(target_os = "macos", test))]
mod multi_click;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod disposition_mailbox;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub mod unsupported;

use crate::api::Backend;

#[cfg(target_os = "windows")]
pub(crate) fn prepare_console_for_cli() {
    windows::prepare_console_for_cli();
}

pub(crate) fn atomic_replace(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows::atomic_replace(source, destination)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::rename(source, destination)
    }
}

/// Create the backend for the target this binary was built for.
pub fn backend() -> Result<Box<dyn Backend>, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacOsBackend::new()?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WindowsBackend::new()?))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(Box::new(unsupported::UnsupportedBackend::new()?))
    }
}

/// Name of the compiled-in backend, for diagnostics.
pub const fn backend_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "unsupported"
    }
}
