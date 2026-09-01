//! TOML DTOs for built-in mode sections.

use serde::{Deserialize, Serialize};

use crate::api::{
    Binding, BoundaryHighlight, FocusedApp, HintPlacement, Key, KeyChord, LabelDirection, LabelUi,
    LifecycleAction, ModeId, SearchInputUi, TargetingLifecycle, ThemedColor, UiScanStrategy,
    VisionOptions,
};

use super::{AppOverride, Bindings, UiHintAppOverride, app_override_matches, builtin_bindings};

// ---------------------------------------------------------------------------
// [normal]
// ---------------------------------------------------------------------------

/// The working mode: move the pointer, click, scroll, and enter the other
/// modes.
///
/// Everything here is a plain binding table, so a user can rebind `hjkl` to
/// anything, add `t = "home"`, or point a key at a plugin mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Normal {
    pub inherits: Vec<String>,
    /// Forward keyboard input that does not match a complete KeySteer binding.
    pub passthrough_unbound_keys: bool,
    /// Hold a physical key bound to click/double-click for this many
    /// milliseconds to toggle that mouse button. The same threshold lets a
    /// parameterless toggle activation key latch itself. Zero disables both.
    pub long_press_toggle_ms: u64,
    /// Release a direct click/double-click button latched by long press after
    /// a physical-modifier drag has remained still. Zero disables it.
    pub auto_release_ms: u64,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Default for Normal {
    fn default() -> Self {
        Self {
            inherits: vec!["hotkeys".into()],
            passthrough_unbound_keys: true,
            long_press_toggle_ms: 500,
            auto_release_ms: 0,
            bindings: default_normal_bindings(),
            app_configs: Vec::new(),
        }
    }
}

/// Defaults for `normal`: vim-style movement, clicks, scrolling, the three
/// targeting modes, and the navigation keys.
///
/// Bare letters are free for pointer control, while unbound keys pass through
/// by default. Complete modifier combinations take precedence over bare-key
/// bindings so external shortcuts remain usable.
fn default_normal_bindings() -> Bindings {
    let entries: &[(&str, &str)] = &[
        // Movement and speed modifiers, held alongside a direction.
        ("h", "move_left"),
        ("j", "move_down"),
        ("k", "move_up"),
        ("l", "move_right"),
        ("caps_lock", "precision"),
        ("left_shift", "slow"),
        ("v", "fast"),
        ("b", "fast"),
        // Scroll takes effect immediately on a tap and repeats while held.
        ("m", "wheel_down"),
        (",", "wheel_up"),
        // Pointer buttons.
        (";", "left_click"),
        ("'", "right_click"),
        ("right_shift", "middle_click"),
        ("n", "toggle"),
        // Targeting modes available directly from normal.
        ("g", "grid"),
        ("f", "recursive_grid"),
        ("primary+f", "ui_hint"),
        ("primary+s", "screen next"),
        // Navigation keys sent to the focused application.
        ("u", "page_down"),
        ("i", "page_up"),
        ("t", "home"),
        ("y", "end"),
        // Normal has no label-key conflict, so bare q exits immediately.
        ("q", "idle"),
        ("esc", "idle"),
    ];
    builtin_bindings(entries)
}

// ---------------------------------------------------------------------------
// [ui_hint]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiHint {
    pub enabled: bool,
    pub strategy: UiScanStrategy,
    pub vision: VisionOptions,
    /// Characters used to build hint labels.
    pub hint_characters: String,
    pub label_direction: LabelDirection,
    /// Accessibility tree depth limit; 0 means unlimited.
    pub max_depth: u32,
    /// Soft budget for one accessibility scan. Incremental results remain
    /// visible while the scan continues.
    pub scan_timeout_ms: u64,
    /// Automatic retries when a scan times out or returns no targets.
    pub scan_retry_count: u32,
    /// Delay before an automatic retry, allowing a busy UI provider to settle.
    pub scan_retry_delay_ms: u64,
    /// Semantic roles that receive a hint.
    pub clickable_roles: Vec<String>,
    /// Skip the clickability heuristic and hint every matching role.
    pub ignore_clickable_check: bool,
    /// Hit-test each element; slower but removes occluded hints.
    pub visible_check_enabled: bool,
    pub placement: HintPlacement,
    /// Pixel offsets applied after placing each label relative to its element.
    pub label_x_offset: i32,
    pub label_y_offset: i32,
    pub ui: LabelUi,
    pub boundary_highlight: BoundaryHighlight,
    pub search_input_ui: SearchInputUi,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub lifecycle: TargetingLifecycle,
    /// Modifier held to expose the next label in each overlapping group.
    pub overlap_cycle_key: String,
    pub bindings: Bindings,
    pub app_configs: Vec<UiHintAppOverride>,
}

impl UiHint {
    pub fn strategy_for(&self, app: Option<&FocusedApp>) -> UiScanStrategy {
        let Some(app) = app else {
            return self.strategy;
        };
        self.app_configs
            .iter()
            .find(|over| app_override_matches(&over.bundle_id, app))
            .and_then(|over| over.strategy)
            .unwrap_or(self.strategy)
    }

    /// Side-agnostic modifiers such as `shift` match either physical key.
    pub fn overlap_cycle_matches(&self, key: &Key) -> bool {
        KeyChord::parse(&self.overlap_cycle_key).is_ok_and(|chord| chord.activation_matches(key))
    }

    /// Whether a temporary-mode key denotes the same physical modifier family
    /// as the overlap key. In UI Hint, overlap cycling deliberately wins.
    pub fn overlap_cycle_conflicts_with(&self, temporary_key: &str) -> bool {
        let (Ok(overlap), Ok(temporary)) = (
            KeyChord::parse(&self.overlap_cycle_key),
            KeyChord::parse(temporary_key),
        ) else {
            return false;
        };
        overlap.activation_matches(temporary.activation_key())
            || temporary.activation_matches(overlap.activation_key())
    }
}

impl Default for UiHint {
    fn default() -> Self {
        let ui = LabelUi {
            font_size: 17,
            ..Default::default()
        };
        let search_input_ui = SearchInputUi {
            label: LabelUi {
                font_size: 14,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            enabled: true,
            strategy: UiScanStrategy::Hybrid,
            vision: VisionOptions::default(),
            hint_characters: "asdfghjkl".into(),
            label_direction: LabelDirection::Normal,
            max_depth: 50,
            scan_timeout_ms: 2_500,
            scan_retry_count: 1,
            scan_retry_delay_ms: 200,
            clickable_roles: default_clickable_roles(),
            ignore_clickable_check: false,
            visible_check_enabled: false,
            placement: HintPlacement::Bottom,
            label_x_offset: 0,
            label_y_offset: -8,
            ui,
            boundary_highlight: BoundaryHighlight::default(),
            search_input_ui,
            inherits: vec!["hotkeys".into(), "normal".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            lifecycle: TargetingLifecycle {
                after_finish: LifecycleAction::Mode(ModeId::normal()),
                after_click: LifecycleAction::Mode(ModeId::normal()),
            },
            overlap_cycle_key: "shift".into(),
            bindings: Bindings::from([
                ("primary+r".into(), Binding::RescanUi),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]),
            app_configs: Vec::new(),
        }
    }
}

/// Neru's semantic role vocabulary. Backends map these to native roles.
fn default_clickable_roles() -> Vec<String> {
    [
        "button",
        "menu_button",
        "popup_button",
        "combo_box",
        "link",
        "checkbox",
        "radio",
        "switch",
        "text_field",
        "text_area",
        "search_field",
        "slider",
        "stepper",
        "tab",
        "menu_item",
        "cell",
        "list_item",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// [grid]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grid {
    pub enabled: bool,
    /// Columns in every selection layer.
    pub grid_cols: u32,
    /// Rows in every selection layer.
    pub grid_rows: u32,
    /// One selection key per cell, in row-major keyboard order.
    pub keys: String,
    /// Number of selection layers before the current cell becomes the target.
    pub max_depth: u32,
    /// Move the pointer to each selected cell's centre while drilling down.
    pub cursor_follow_selection: bool,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub lifecycle: TargetingLifecycle,
    pub ui: GridUi,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Default for Grid {
    fn default() -> Self {
        let ui = GridUi {
            label: LabelUi {
                font_size: 20,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            enabled: true,
            grid_cols: 5,
            grid_rows: 4,
            keys: "12345qwertasdfgzxcvb".into(),
            max_depth: 3,
            cursor_follow_selection: true,
            inherits: vec!["hotkeys".into(), "normal".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            lifecycle: TargetingLifecycle {
                after_finish: LifecycleAction::Mode(ModeId::normal()),
                after_click: LifecycleAction::Finish,
            },
            ui,
            bindings: Bindings::from([
                ("`".into(), Binding::ToggleCursorFollowSelection),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]),
            app_configs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GridUi {
    #[serde(flatten)]
    pub label: LabelUi,
    pub matched_background_color: Option<ThemedColor>,
    pub matched_border_color: Option<ThemedColor>,
}

// ---------------------------------------------------------------------------
// [recursive_grid]
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecursiveGrid {
    pub enabled: bool,
    pub grid_cols: u32,
    pub grid_rows: u32,
    /// Cell keys; must hold `grid_cols * grid_rows` characters.
    pub keys: String,
    pub min_size_width: u32,
    pub min_size_height: u32,
    /// Recursion limit, 1..=20.
    pub max_depth: u32,
    /// Move the pointer to each selected cell's centre while drilling down.
    pub cursor_follow_selection: bool,
    pub inherits: Vec<String>,
    pub temporary_mode: Option<String>,
    pub temporary_mode_keys: Vec<String>,
    pub lifecycle: TargetingLifecycle,
    /// Per-depth layout overrides.
    pub layers: Vec<GridLayer>,
    pub ui: RecursiveGridUi,
    pub bindings: Bindings,
    pub app_configs: Vec<AppOverride>,
}

impl Default for RecursiveGrid {
    fn default() -> Self {
        let ui = RecursiveGridUi {
            label: LabelUi {
                font_size: 20,
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            enabled: true,
            grid_cols: 3,
            grid_rows: 3,
            keys: "qweasdzxc".into(),
            min_size_width: 1,
            min_size_height: 1,
            max_depth: 10,
            cursor_follow_selection: true,
            inherits: vec!["hotkeys".into(), "normal".into()],
            temporary_mode: Some("normal".into()),
            temporary_mode_keys: vec!["primary".into()],
            lifecycle: TargetingLifecycle::default(),
            layers: Vec::new(),
            ui,
            bindings: Bindings::from([
                ("`".into(), Binding::ToggleCursorFollowSelection),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]),
            app_configs: Vec::new(),
        }
    }
}

/// Overrides the grid shape at one recursion depth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridLayer {
    /// Zero-based depth this entry applies to.
    pub depth: u32,
    pub grid_cols: Option<u32>,
    pub grid_rows: Option<u32>,
    pub keys: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecursiveGridUi {
    #[serde(flatten)]
    pub label: LabelUi,
    pub line_width: i32,
    pub line_color: Option<ThemedColor>,
    pub highlight_color: Option<ThemedColor>,
    /// Draw a filled pill behind each cell key.
    pub label_background: bool,
    pub label_background_color: Option<ThemedColor>,
    /// Replace every cell label with this character, e.g. `·`.
    pub label_char: String,
    /// Hide labels when fitting them would require a smaller font.
    pub label_min_font_size: i32,
    /// Hide labels once a cell is smaller than the fitted font size times this
    /// multiplier. `0` disables this additional threshold.
    pub label_autohide_multiplier: f64,
    pub sub_key_preview: bool,
    pub sub_key_preview_font_size: i32,
    pub sub_key_preview_text_color: Option<ThemedColor>,
    pub sub_key_preview_autohide_multiplier: f64,
}

impl Default for RecursiveGridUi {
    fn default() -> Self {
        Self {
            label: LabelUi::default(),
            line_width: 1,
            line_color: None,
            highlight_color: None,
            label_background: false,
            label_background_color: None,
            label_char: String::new(),
            label_min_font_size: 6,
            label_autohide_multiplier: 1.5,
            sub_key_preview: false,
            sub_key_preview_font_size: 8,
            sub_key_preview_text_color: None,
            sub_key_preview_autohide_multiplier: 1.5,
        }
    }
}

// ---------------------------------------------------------------------------
// [scroll] and [pointer]
//
// These are tuning parameters, not modes: scrolling is a set of bindings that
// any mode can use, so there is no `scroll` mode to enter.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scroll {
    /// Pixels for `scroll_up` and friends.
    pub scroll_step: i32,
    /// Pixels for `scroll_half_up` and friends.
    pub scroll_step_half: i32,
    /// Pixels for `scroll_full_up` and friends.
    pub scroll_step_full: i32,
    /// Deprecated compatibility input. Use `platform.macos.scroll.invert`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invert_scroll: Option<bool>,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            scroll_step: 50,
            scroll_step_half: 500,
            scroll_step_full: 1_000_000,
            invert_scroll: None,
        }
    }
}

impl Scroll {
    /// Pixels for one scroll binding.
    pub fn pixels(&self, amount: crate::api::binding::ScrollAmount) -> f64 {
        use crate::api::binding::ScrollAmount as A;
        match amount {
            A::Step => self.scroll_step as f64,
            A::Half => self.scroll_step_half as f64,
            A::Full => self.scroll_step_full as f64,
        }
    }
}

/// Keyboard-driven pointer acceleration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pointer {
    /// Initial pointer velocity in pixels per second.
    pub initial_speed: f64,
    /// Maximum pointer velocity in pixels per second.
    pub max_speed: f64,
    /// Velocity added per second while held, in pixels per second².
    pub acceleration: f64,
    /// Ease acceleration at both ends instead of changing velocity linearly.
    pub smooth_acceleration: bool,
    /// Immediate pixel distance for a tap shorter than the next display update.
    pub tap_distance: f64,
    pub slow_multiplier: f64,
    pub precision_multiplier: f64,
    pub fast_multiplier: f64,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            initial_speed: 1000.0,
            max_speed: 2200.0,
            acceleration: 3000.0,
            smooth_acceleration: true,
            tap_distance: 2.5,
            slow_multiplier: 0.35,
            precision_multiplier: 0.12,
            fast_multiplier: 2.0,
        }
    }
}
