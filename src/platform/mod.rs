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

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) mod common;

#[cfg(target_os = "macos")]
pub(crate) use macos::{latest_point_mailbox, multi_click};

// Keep portable unit coverage for macOS-only state machines on non-macOS CI.
#[cfg(all(test, not(target_os = "macos")))]
#[path = "macos/latest_point_mailbox.rs"]
mod latest_point_mailbox_tests;
#[cfg(all(test, not(target_os = "macos")))]
#[path = "macos/multi_click.rs"]
mod multi_click_tests;
#[cfg(all(test, not(target_os = "macos")))]
#[path = "macos/window_move.rs"]
mod window_move_tests;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub mod unsupported;

use crate::api::{Backend, UiScanStrategy};

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) const fn redundant_button_action(
    held: bool,
    action: crate::api::command::ButtonAction,
) -> bool {
    held && matches!(action, crate::api::command::ButtonAction::Press)
}

#[cfg(target_os = "windows")]
pub(crate) fn prepare_console_for_cli() {
    windows::prepare_console_for_cli();
}

#[cfg(target_os = "windows")]
pub(crate) fn run_internal_wechat_ocr_helper(
    bridge: std::path::PathBuf,
    component: std::path::PathBuf,
    runtime: std::path::PathBuf,
) -> Result<(), String> {
    windows::run_internal_wechat_ocr_helper(bridge, component, runtime)
}

#[cfg(target_os = "windows")]
pub(crate) fn run_internal_update_helper() -> Option<Result<(), String>> {
    windows::run_internal_update_helper()
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_vision_diagnostics() -> Vec<String> {
    windows::vision_diagnostics()
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
    backend_with_ui_scan_strategy(None)
}

/// Create the application backend and prewarm providers required by the
/// configured UI scan strategy.
pub fn backend_for_ui_scan(strategy: UiScanStrategy) -> Result<Box<dyn Backend>, String> {
    backend_with_ui_scan_strategy(Some(strategy))
}

fn backend_with_ui_scan_strategy(
    strategy: Option<UiScanStrategy>,
) -> Result<Box<dyn Backend>, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = strategy;
        Ok(Box::new(macos::MacOsBackend::new()?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(
            windows::WindowsBackend::new_with_ui_scan_strategy(strategy)?,
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = strategy;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::command::ButtonAction;

    #[test]
    fn an_already_held_button_never_receives_another_press() {
        assert!(redundant_button_action(true, ButtonAction::Press));
        assert!(!redundant_button_action(false, ButtonAction::Press));
        assert!(!redundant_button_action(true, ButtonAction::Release));
        assert!(!redundant_button_action(true, ButtonAction::Click));
        assert!(!redundant_button_action(true, ButtonAction::DoubleClick));
    }
}
