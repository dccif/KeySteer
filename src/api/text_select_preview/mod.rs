//! The macOS OCR preview is isolated from the other platform builds.
#[cfg(any(target_os = "macos", test))]
#[path = "macos.rs"]
mod implementation;
#[cfg(not(any(target_os = "macos", test)))]
#[path = "stub.rs"]
mod implementation;

pub(crate) use implementation::*;

/// A portable snapshot whose provisional rows can be reconciled when enabled.
pub(crate) trait PreviewFrame {
    fn reconcile_preview(&mut self, result: &super::command::UiScanResult) -> bool;
}
