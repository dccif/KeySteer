//! The contract a platform backend must satisfy.
//!
//! Everything above this line is platform-independent; everything below is
//! `src/platform/<os>`. The trait is deliberately small — input, screens,
//! pointer, injection, overlay presentation and an accessibility scan — so a
//! new OS needs no changes anywhere else in the tree.

use super::command::{ButtonAction, FocusedApp, MouseButton, UiScanRequest, UiScanResult};
use super::geometry::{Point, Screen};
use super::input::{InputEvent, Key};
use super::overlay::OverlayScene;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Something that happened natively and must reach the engine.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// A key was pressed or released.
    Input(InputEvent),
    /// A previously accepted native input request failed during execution.
    ///
    /// Some backends must submit synthetic input asynchronously so their
    /// native input callback never waits on the engine while the engine waits
    /// on that same callback. Successful requests produce no event.
    InputInjectionFailed(String),
    /// The pointer moved.
    PointerMoved(Point),
    /// The display is ready for another animation frame.
    ///
    /// This is sourced from the native display link rather than a fixed-rate
    /// timer, so `elapsed` follows the active monitor's actual refresh cadence.
    Frame(Duration),
    /// The focused application changed.
    FocusChanged(Option<FocusedApp>),
    /// Displays were added, removed or rearranged.
    ScreensChanged(Vec<Screen>),
    /// The operating-system light/dark appearance changed.
    AppearanceChanged(Appearance),
    /// An accessibility scan finished.
    UiScanned(UiScanResult),
    /// The user asked to quit (tray menu, signal, ...).
    Quit,
    /// The user asked to reload the configuration.
    ReloadConfig,
    /// The user asked to edit the active configuration in the web simulator.
    OpenConfigSimulator,
    /// The user toggled the engine on or off.
    ToggleEnabled,
    /// The user toggled whether KeySteer runs when the current user signs in.
    ToggleAutostart,
    /// The user explicitly requested an update check from the native menu.
    CheckForUpdates,
    /// A background update request changed phase or download percentage.
    UpdateProgress(UpdateProgress),
    /// A background update request completed.
    UpdateChecked(UpdateCheckResult),
    /// Non-fatal backend problem worth logging.
    Warning(String),
}

/// Visible progress for a user-requested background update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateProgress {
    Checking,
    Downloading { latest: String, percent: u8 },
}

/// Result of comparing the running package version with GitHub's latest release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckResult {
    UpToDate {
        current: String,
    },
    UpdateDownloaded {
        current: String,
        latest: String,
        path: PathBuf,
    },
    Failed(String),
}

/// Whether a key event should be hidden from the focused application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDisposition {
    /// Swallow it: the active mode consumed it.
    Consume,
    /// Retained for backend API compatibility. The engine no longer defers or
    /// replays physical modifiers; built-in backends treat this as `Forward`.
    Defer,
    /// Let it through to the focused application.
    Forward,
}

/// A native backend. Implementations live in `src/platform/<os>.rs` and are
/// selected by `cfg(target_os)` in `src/platform/mod.rs`.
pub trait Backend {
    /// Block for up to `timeout` for the next event.
    ///
    /// Returning `Ok(None)` on timeout is how the engine gets its chance to
    /// run timers, so backends must honour the timeout even when idle.
    fn poll(&mut self, timeout: std::time::Duration) -> Result<Option<BackendEvent>, String>;

    /// Tell the backend how to dispose of the key event it just delivered.
    ///
    /// Backends that hook the input stream synchronously (Windows
    /// `WH_KEYBOARD_LL`, macOS `CGEventTap`) must block their callback until
    /// this is called, otherwise the keystroke reaches the app before the
    /// engine has decided.
    fn dispose_key(&mut self, disposition: KeyDisposition) -> Result<(), String>;

    fn screens(&self) -> Result<Vec<Screen>, String>;
    fn pointer(&self) -> Result<Point, String>;
    fn focused_app(&self) -> Result<Option<FocusedApp>, String>;

    fn warp_pointer(&self, to: Point) -> Result<(), String>;
    /// Move from the engine's authoritative position by a relative delta.
    /// Supplying `from` lets native backends avoid querying the cursor again.
    fn move_pointer(&self, from: Point, dx: f64, dy: f64) -> Result<(), String>;
    fn mouse_button(&self, button: MouseButton, action: ButtonAction) -> Result<(), String>;
    /// Scroll the native window currently underneath the physical pointer.
    fn scroll(&self, dx: f64, dy: f64) -> Result<(), String>;
    fn send_key(&self, key: &Key, state: super::input::KeyState) -> Result<(), String>;

    /// Inject an owned, ordered keyboard sequence. Ownership lets an
    /// asynchronous backend queue the batch without cloning it first.
    /// Platforms with a native batch API override this; the default preserves
    /// the existing per-event behaviour. The engine records a conservative
    /// cleanup set when this returns an error.
    fn send_keys(&self, events: Vec<(Key, super::input::KeyState)>) -> Result<(), String> {
        for (key, state) in events {
            self.send_key(&key, state)?;
        }
        Ok(())
    }

    /// Press every key in order, then release them in reverse order.
    ///
    /// Built-in backends override this to keep common chords inline. The
    /// default remains source-compatible with external backends and preserves
    /// the owned sequence semantics of [`Self::send_keys`].
    fn send_chord(&self, keys: &[Key]) -> Result<(), String> {
        let events = keys
            .iter()
            .cloned()
            .map(|key| (key, super::input::KeyState::Down))
            .chain(
                keys.iter()
                    .rev()
                    .cloned()
                    .map(|key| (key, super::input::KeyState::Up)),
            )
            .collect();
        self.send_keys(events)
    }

    /// Start or stop native display-synchronised frame delivery.
    ///
    /// Backends without a display-link implementation may reject activation;
    /// normal mode then retains operating-system key repeat as a fallback.
    fn set_frame_clock(&mut self, _active: bool) -> Result<(), String> {
        Err("native display-synchronised frames are unavailable".into())
    }

    /// Present a frame. Called on every visual change, so implementations
    /// should diff against the previous scene rather than rebuild windows.
    /// Submit immutable shared frame data. An asynchronous backend may retain
    /// it without forcing the engine to deep-clone the scene.
    fn present(&mut self, scene: Arc<OverlayScene>) -> Result<(), String>;
    /// Move dynamic layers already presented by [`Self::present`].
    ///
    /// `None` leaves that layer unchanged. `Ok(true)` means the backend
    /// accepted the lightweight update; the default asks the engine to fall
    /// back to a complete scene so third-party backends remain compatible.
    fn update_overlay_positions(
        &mut self,
        _cursor: Option<Point>,
        _indicator: Option<Point>,
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Remove all overlay surfaces from the screen.
    fn dismiss(&mut self) -> Result<(), String>;

    /// Begin an accessibility scan. Results arrive as
    /// [`BackendEvent::UiScanned`] so a slow tree walk cannot stall the engine.
    fn request_ui_scan(&mut self, request: UiScanRequest) -> Result<(), String>;

    /// Cancel a scan that no longer has an active consumer.
    ///
    /// Backends should keep reusable provider/worker state warm while dropping
    /// request-scoped work. The default keeps third-party backends source
    /// compatible when they do not retain asynchronous scans.
    fn cancel_ui_scan(&mut self, _id: u64) -> Result<(), String> {
        Ok(())
    }

    /// Which system appearance is active, for theme selection.
    fn appearance(&self) -> Appearance {
        Appearance::Dark
    }

    /// Keep native controls in sync with the engine's paused state.
    fn set_enabled(&mut self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }

    /// Toggle login-time startup and return the new checked state.
    fn toggle_autostart(&mut self) -> Result<bool, String> {
        Err(format!(
            "{} does not support login-time startup",
            self.name()
        ))
    }

    /// Start a user-requested update check without blocking the engine loop.
    fn check_for_updates(&mut self) -> Result<(), String> {
        Err(format!("{} does not support update checks", self.name()))
    }

    /// Reflect background update progress in the platform status menu.
    fn present_update_progress(&mut self, _progress: &UpdateProgress) -> Result<(), String> {
        Ok(())
    }

    /// Present the result using the platform's native UI.
    fn present_update_result(&mut self, _result: &UpdateCheckResult) -> Result<(), String> {
        Ok(())
    }

    /// Open an HTTPS URL using the user's default browser.
    ///
    /// The default keeps third-party and test backends source compatible.
    fn open_url(&mut self, _url: &str) -> Result<(), String> {
        Err(format!("{} cannot open web pages", self.name()))
    }

    /// Name of the backend, for diagnostics.
    fn name(&self) -> &'static str;

    /// Whether the keyboard can actually be observed.
    ///
    /// `false` means every mode that captures the keyboard will misbehave,
    /// because keystrokes cannot be read or suppressed. On macOS this is what
    /// a missing Accessibility grant looks like. The engine reports it once at
    /// startup instead of leaving the user with a program that silently does
    /// nothing.
    fn keyboard_available(&self) -> bool {
        true
    }

    /// A human-readable explanation of why the keyboard is unavailable, with
    /// the steps to fix it. Only consulted when [`Self::keyboard_available`]
    /// returns `false`.
    fn keyboard_unavailable_reason(&self) -> Option<String> {
        None
    }

    /// Called once before the engine loop starts.
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// Called once after the engine loop ends.
    fn shutdown(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}
