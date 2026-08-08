#![forbid(unsafe_code)]

//! Fallback backend for targets without a native implementation.
//!
//! It compiles and reports a clear error rather than failing the build, so the
//! platform-independent core stays testable and cross-checkable everywhere.

use std::sync::Arc;
use std::time::Duration;

use crate::api::backend::{Backend, BackendEvent, KeyDisposition};
use crate::api::command::{ButtonAction, FocusedApp, MouseButton};
use crate::api::geometry::{Point, Screen};
use crate::api::input::{Key, KeyState};
use crate::api::overlay::OverlayScene;

pub struct UnsupportedBackend;

impl UnsupportedBackend {
    pub fn new() -> Result<Self, String> {
        Err(format!(
            "KeySteer has no backend for {}; supported targets are macOS and Windows",
            std::env::consts::OS
        ))
    }
}

impl Backend for UnsupportedBackend {
    fn poll(&mut self, _timeout: Duration) -> Result<Option<BackendEvent>, String> {
        Ok(Some(BackendEvent::Quit))
    }
    fn dispose_key(&mut self, _d: KeyDisposition) -> Result<(), String> {
        Ok(())
    }
    fn screens(&self) -> Result<Vec<Screen>, String> {
        Ok(Vec::new())
    }
    fn pointer(&self) -> Result<Point, String> {
        Ok(Point::default())
    }
    fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
        Ok(None)
    }
    fn warp_pointer(&self, _to: Point) -> Result<(), String> {
        Ok(())
    }
    fn move_pointer(&self, _from: Point, _dx: f64, _dy: f64) -> Result<(), String> {
        Ok(())
    }
    fn mouse_button(&self, _b: MouseButton, _a: ButtonAction) -> Result<(), String> {
        Ok(())
    }
    fn scroll(&self, _dx: f64, _dy: f64) -> Result<(), String> {
        Ok(())
    }
    fn send_key(&self, _k: &Key, _s: KeyState) -> Result<(), String> {
        Ok(())
    }
    fn present(&mut self, _scene: Arc<OverlayScene>) -> Result<(), String> {
        Ok(())
    }
    fn dismiss(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn request_ui_scan(&mut self, _request: crate::api::UiScanRequest) -> Result<(), String> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "unsupported"
    }
}
