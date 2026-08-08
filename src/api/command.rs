//! The command and event vocabulary shared by the engine, the built-in modes
//! and every plugin.
//!
//! A mode is a pure state machine: it receives [`ModeEvent`]s and returns
//! [`Command`]s. It never touches a platform API, which is what makes the
//! built-in modes and third-party plugins interchangeable.

use std::any::Any;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::binding::{ActionSequence, Binding};
use super::geometry::{Point, Rect, Screen, UiTarget};
use super::input::{Key, KeyState, ModeId};
use super::overlay::{Color, OverlayScene};
use super::theme::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonAction {
    Press,
    Release,
    Click,
    DoubleClick,
}

/// Why the current targeting session is being completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishCause {
    Selection,
    Click,
    Explicit,
}

/// Everything a mode can ask the host to do.
///
/// This is the *entire* outward surface of a mode. Built-in modes are
/// restricted to it, which guarantees a plugin can express anything they can —
/// including drawing its own grid or full-screen overlay.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Dispatch high-level actions through the same path used by config.
    DispatchActions(ActionSequence),
    /// Move the pointer by a relative delta, in pixels.
    MovePointer {
        dx: f64,
        dy: f64,
    },
    /// Move the pointer to an absolute position.
    WarpPointer {
        x: f64,
        y: f64,
    },
    /// Press, release or click a mouse button at the current position.
    MouseButton {
        button: MouseButton,
        action: ButtonAction,
    },
    /// Complete the active targeting session without re-entering its mode.
    FinishMode {
        cause: FinishCause,
    },
    /// Reset the active mode's session while preserving its return destination.
    RestartMode,
    /// Scroll by a delta in pixels.
    Scroll {
        dx: f64,
        dy: f64,
    },

    /// Start or stop display-synchronised frame events for continuous motion.
    /// This is driven by the native display link, not a periodic timer.
    SetFrameClock(bool),

    /// Present a frame. Replaces whatever was on screen.
    ShowOverlay(OverlayScene),
    /// Tear down the overlay.
    HideOverlay,

    /// Inject a single key event into the focused application.
    SendKey {
        key: Key,
        state: KeyState,
    },
    /// Inject a chord: press in order, release in reverse.
    SendChord {
        keys: Vec<Key>,
    },

    /// Ask the platform to walk the accessibility tree. The result arrives as
    /// [`ModeEvent::UiScanned`]. `bounds` of `None` means the focused window.
    ScanUi(UiScanRequest),

    /// Activate another mode. The engine deactivates the current one first.
    /// Plugins use this to hand control back to `idle` or to chain modes.
    SwitchMode(ModeId),
    /// Temporarily place a modal plugin above the current mode without losing
    /// the current mode's navigation state.
    PushMode(ModeId),
    /// Close the current modal mode and restore the suspended mode.
    PopMode,
    /// Ask the active mode to move its state to a numbered display.
    RetargetScreen {
        index: usize,
        preserve: bool,
    },

    /// Request a [`ModeEvent::Timer`] after `delay`. Re-arming an existing
    /// `id` replaces it. Available to plugins and other deferred work; built-in
    /// pointer movement uses native display frames rather than this timer.
    SetTimer {
        id: String,
        delay: Duration,
        /// Fire continuously until cancelled.
        repeating: bool,
    },
    CancelTimer {
        id: String,
    },

    /// Copy the config value at a dotted path, e.g. `grid.characters`.
    SetConfigValue {
        path: String,
        value: String,
    },
    /// Re-read the configuration from disk.
    ReloadConfig,

    /// Run a shell command, detached.
    Exec {
        program: String,
        args: Vec<String>,
    },

    /// Shut the application down.
    Quit,
}

impl Command {
    /// Convenience for the overwhelmingly common exit path.
    pub fn dismiss_to_idle() -> Vec<Command> {
        vec![Command::HideOverlay, Command::SwitchMode(ModeId::idle())]
    }

    pub fn click(button: MouseButton) -> Command {
        Command::MouseButton {
            button,
            action: ButtonAction::Click,
        }
    }

    pub fn warp_to(p: Point) -> Command {
        Command::WarpPointer { x: p.x, y: p.y }
    }
}

/// Everything the host tells a mode.
#[derive(Debug, Clone, PartialEq)]
pub enum ModeEvent {
    /// This mode just became active. Build the first overlay here.
    Activated {
        /// Mode that was active before, if any.
        previous: Option<ModeId>,
    },
    /// This mode was pushed modally and must later return with `PopMode`.
    Pushed { previous: ModeId },
    /// This mode is about to be torn down.
    Deactivated,
    /// A modal mode temporarily covered this mode without destroying state.
    Suspended,
    /// The modal mode closed; redraw from the preserved state.
    Resumed,
    /// Reset this mode's current session without changing its identity or
    /// return destination.
    Restarted,
    /// A targeting session should enter its completed state.
    FinishRequested { cause: FinishCause },
    /// A semantic KeySteer click completed successfully.
    Clicked {
        button: MouseButton,
        action: ButtonAction,
    },

    /// A key was pressed or released. Already filtered: injected events and
    /// events consumed by global hotkeys never reach a mode.
    ///
    /// Modes that read raw characters (grid labels, hint labels, search text)
    /// use this. Modes driven by verbs use [`ModeEvent::Binding`] instead.
    Key {
        key: Key,
        state: KeyState,
        repeat: bool,
    },

    /// A configured binding fired.
    ///
    /// The engine resolves the key through the active mode's binding table and
    /// delivers the verb, so a mode never has to re-implement key lookup. Held
    /// bindings ([`Binding::is_held`]) are delivered on both press and release;
    /// the rest only on press.
    ///
    /// This is the same event a plugin receives, which is what lets a plugin
    /// reuse `move_left` and friends rather than inventing its own vocabulary.
    Binding {
        binding: Binding,
        state: KeyState,
        /// The key that triggered it, for modes that need to track holds.
        key: Key,
    },
    /// A parameterized verb exported by a plugin was invoked.
    Invoked { verb: String, args: Vec<String> },

    /// The pointer moved, whoever moved it.
    PointerMoved(Point),

    /// A native display refresh occurred. The measured interval keeps motion
    /// speed stable across 60 Hz, 120 Hz, ProMotion and external displays.
    Frame { elapsed: Duration },

    /// The focused application changed.
    FocusChanged(Option<FocusedApp>),

    /// Display topology changed. Grid-like modes must recompute here.
    ScreensChanged(Vec<Screen>),
    /// Move this mode's state to another display. Grid-like modes can replay
    /// their logical selection path when `preserve` is true.
    ScreenRetargeted { screen: Screen, preserve: bool },

    /// A [`Command::ScanUi`] completed.
    UiScanned(UiScanResult),

    /// A timer armed with [`Command::SetTimer`] elapsed. `elapsed` is measured
    /// by the runtime so animation and movement stay independent of display
    /// refresh rate and scheduler jitter.
    Timer { id: String, elapsed: Duration },

    /// The configuration was reloaded; re-read anything cached.
    ConfigReloaded,
}

/// Identity of the focused application, for per-app configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FocusedApp {
    /// macOS bundle id, Linux `WM_CLASS`/`app_id`, Windows executable name.
    pub bundle_id: String,
    pub window_title: String,
    pub process_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiScanStrategy {
    #[serde(rename = "axtree")]
    AxTree,
    #[default]
    Vision,
    /// Run accessibility and visual detection concurrently and merge their
    /// incremental results in the hint mode.
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VisionOptions {
    pub detect_text: bool,
    pub detect_rectangles: bool,
    pub request_timeout_ms: u64,
    pub minimum_confidence: f64,
    pub merge_iou_threshold: f64,
    pub rectangle_max_candidates: usize,
    pub rectangle_min_size: f64,
    pub rectangle_min_aspect: f64,
    pub rectangle_max_aspect: f64,
    pub button_min_confidence: f64,
    pub button_min_aspect: f64,
    pub button_max_aspect: f64,
    pub button_icon_max_size: f64,
    pub link_min_aspect: f64,
    pub link_max_height: f64,
    pub link_min_width: f64,
    pub image_min_size: f64,
    pub checkbox_max_size: f64,
    pub generic_clickable_min_confidence: f64,
}

impl Default for VisionOptions {
    fn default() -> Self {
        Self {
            detect_text: true,
            detect_rectangles: true,
            request_timeout_ms: 5_000,
            minimum_confidence: 0.0,
            merge_iou_threshold: 0.5,
            rectangle_max_candidates: 100,
            rectangle_min_size: 0.01,
            rectangle_min_aspect: 0.3,
            rectangle_max_aspect: 10.0,
            button_min_confidence: 0.3,
            button_min_aspect: 0.8,
            button_max_aspect: 8.0,
            button_icon_max_size: 48.0,
            link_min_aspect: 5.0,
            link_max_height: 40.0,
            link_min_width: 50.0,
            image_min_size: 48.0,
            checkbox_max_size: 32.0,
            generic_clickable_min_confidence: 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiScanRequest {
    pub id: u64,
    /// Soft accessibility traversal budget. Platform providers may spend up
    /// to this long in one native transaction before returning control.
    pub timeout_ms: u64,
    pub bounds: Option<Rect>,
    pub roles: Vec<String>,
    pub max_depth: u32,
    pub visible_only: bool,
    pub clickable_only: bool,
    pub strategy: UiScanStrategy,
    pub vision: VisionOptions,
    pub app: Option<FocusedApp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiScanStatus {
    /// An incremental batch; more results for the same scan id will follow.
    Partial,
    Success,
    PermissionDenied(String),
    Unsupported(String),
    ContextChanged,
    TimedOut,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiScanResult {
    pub id: u64,
    pub targets: Vec<UiTarget>,
    pub status: UiScanStatus,
}

/// Type-erased, read-only application settings exposed to modes.
///
/// The public API does not depend on a concrete configuration format. Built-in
/// modes may downcast to the host's settings type during reconfiguration, while
/// third-party plugins can remain independent of it.
pub trait HostSettings: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Send + Sync> HostSettings for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl dyn HostSettings + '_ {
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref()
    }
}

/// Read-only view of host state, passed to a mode on every dispatch.
#[derive(Clone)]
pub struct HostContext<'a> {
    pub screens: &'a [Screen],
    pub cursor: Point,
    pub focused_app: Option<&'a FocusedApp>,
    pub palette: &'a Palette,
    pub config: &'a dyn HostSettings,
}

impl std::fmt::Debug for HostContext<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostContext")
            .field("screens", &self.screens)
            .field("cursor", &self.cursor)
            .field("focused_app", &self.focused_app)
            .field("palette", &self.palette)
            .field("config", &"<host settings>")
            .finish()
    }
}

impl HostContext<'_> {
    /// Screen under the cursor, falling back to the primary screen.
    pub fn active_screen(&self) -> Option<&Screen> {
        Screen::containing(self.screens, &self.cursor)
    }

    /// Area a full-screen mode should cover: the active screen, or the whole
    /// virtual desktop when the cursor is not on any known screen.
    pub fn active_bounds(&self) -> Rect {
        self.active_screen()
            .map(|s| s.bounds)
            .unwrap_or_else(|| Screen::virtual_bounds(self.screens))
    }

    pub fn scale(&self) -> f64 {
        self.active_screen().map(|s| s.scale).unwrap_or(1.0)
    }
}

/// A mode: the unit of behaviour. Built-in modes and plugin modes implement
/// exactly this trait, so the engine cannot tell them apart.
pub trait Mode: Send {
    /// Stable identifier used to activate this mode from config and hotkeys.
    fn id(&self) -> ModeId;

    /// Human-readable name for the mode indicator badge.
    fn display_name(&self) -> String {
        self.id().as_str().replace('_', " ")
    }

    /// Handle one event and return the commands it implies.
    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> Vec<Command>;

    /// Whether this mode wants exclusive use of the keyboard. When true the
    /// host swallows keys instead of passing them to the focused app — which
    /// is what grid and hint modes need, and idle does not.
    fn captures_keyboard(&self) -> bool {
        true
    }

    /// Color of this mode's indicator badge; `None` uses the theme accent.
    fn indicator_color(&self, _palette: &Palette) -> Option<Color> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismiss_hides_before_switching() {
        assert_eq!(
            Command::dismiss_to_idle(),
            vec![Command::HideOverlay, Command::SwitchMode(ModeId::idle())]
        );
    }
}
