//! TOML settings DTOs for built-in mode sections.

use super::macros::{config_struct, mode_config};
use serde::{Deserialize, Serialize};

use crate::api::{
    Binding, BoundaryHighlight, FocusedApp, HintPlacement, LabelDirection, LabelUi,
    LifecycleAction, ModeId, SearchInputUi, TargetingLifecycle, ThemedColor, UiScanStrategy,
    VisionOptions,
};

use super::{AppOverride, Bindings, UiHintAppOverride, app_override_matches, builtin_bindings};

// ---------------------------------------------------------------------------
// [normal]
// ---------------------------------------------------------------------------

mode_config! {
    /// The working mode: move the pointer, click, scroll, and enter the other
    /// modes.
    ///
    /// Everything here is a plain binding table, so a user can rebind `hjkl` to
    /// anything, add `t = "home"`, or point a key at a plugin mode.
    pub struct Normal {
        ..routing {
            omit_empty: temporary,
            inherits: vec!["hotkeys".into()],
            temporary_mode: None,
        }
        /// Opt-in blind positioning. Absence keeps the ordinary Normal runtime.
        #[serde(skip_serializing_if = "Option::is_none")]
        targeting: Option<Box<NormalTargeting>>,
        /// Forward keyboard input that does not match a complete KeySteer binding.
        passthrough_unbound_keys: bool = true,
        /// Hold a physical key bound to click/double-click for this many
        /// milliseconds to toggle that mouse button. The same threshold lets a
        /// parameterless toggle activation key latch itself. Zero disables both.
        long_press_toggle_ms: u64 = 500,
        /// Release a direct click/double-click button latched by long press after
        /// a physical-modifier drag has remained still. Zero disables it.
        auto_release_ms: u64 = 0,
        bindings: Bindings = default_normal_bindings(),
        app_configs: Vec<AppOverride>,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetingMethod {
    #[default]
    Grid,
    RecursiveGrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetingReset {
    Move,
    Click,
}

config_struct! {
    pub struct NormalTargeting {
        method: TargetingMethod = TargetingMethod::Grid,
        reset_on: Vec<TargetingReset> = vec![TargetingReset::Move, TargetingReset::Click],
        #[serde(skip_serializing_if = "Option::is_none")]
        grid_cols: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        grid_rows: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        keys: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_depth: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_size_width: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_size_height: Option<u32>,
        /// None inherits all layers; Some replaces the entire list (empty clears it).
        #[serde(skip_serializing_if = "Option::is_none")]
        layers: Option<Vec<GridLayer>>,
    }
}

/// Borrowed geometry only: resolving overrides never clones UI or binding tables.
pub(crate) struct TargetingGeometry<'a> {
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub keys: &'a str,
    pub max_depth: u32,
    pub min_size: Option<(u32, u32)>,
    pub layers: &'a [GridLayer],
}

impl NormalTargeting {
    pub(crate) fn resolve<'a>(&'a self, config: &'a super::Config) -> TargetingGeometry<'a> {
        let (cols, rows, keys, depth, min_size, layers) = match self.method {
            TargetingMethod::Grid => {
                let grid = &config.grid;
                (
                    grid.grid_cols,
                    grid.grid_rows,
                    grid.keys.as_str(),
                    grid.max_depth,
                    None,
                    &[][..],
                )
            }
            TargetingMethod::RecursiveGrid => {
                let grid = &config.recursive_grid;
                (
                    grid.grid_cols,
                    grid.grid_rows,
                    grid.keys.as_str(),
                    grid.max_depth,
                    Some((
                        self.min_size_width.unwrap_or(grid.min_size_width),
                        self.min_size_height.unwrap_or(grid.min_size_height),
                    )),
                    self.layers.as_deref().unwrap_or(&grid.layers),
                )
            }
        };
        TargetingGeometry {
            grid_cols: self.grid_cols.unwrap_or(cols),
            grid_rows: self.grid_rows.unwrap_or(rows),
            keys: self.keys.as_deref().unwrap_or(keys),
            max_depth: self.max_depth.unwrap_or(depth),
            min_size,
            layers,
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
    builtin_bindings(&[
        ("?", "key_help"),
        ("\\", "text_input"),
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
        (".", "scroll_left"),
        ("/", "scroll_right"),
        // Pointer buttons.
        (";", "left_click"),
        ("'", "right_click"),
        ("right_shift", "middle_click"),
        ("n", "toggle"),
        // Targeting modes available directly from normal.
        ("g", "grid"),
        ("f", "ui_hint"),
        ("primary+g", "recursive_grid"),
        ("primary+f", "bisect"),
        ("c", "text_select"),
        ("primary+s", "screen next"),
        ("primary+d", "move_window next"),
        // Navigation keys sent to the focused application.
        ("u", "page_down"),
        ("i", "page_up"),
        ("t", "home"),
        ("y", "end"),
        // Normal has no label-key conflict, so bare q exits immediately.
        ("q", "idle"),
        ("esc", "idle"),
    ])
}

// ---------------------------------------------------------------------------
// [text_input]
// ---------------------------------------------------------------------------

mode_config! {
    pub struct TextInput {
        ..routing {
            inherits: Vec::new(),
            temporary_mode: None,
        }
        bindings: Bindings = builtin_bindings(&[("enter", "normal"), ("\\", "normal"), ("esc", "normal")]),
    }
}

mode_config! {
    /// OCR-only text selection has its own scope and shares UIHint's OCR options and styles.
    pub struct TextSelect {
        enabled: bool = true,
        scan_scope: crate::api::UiScanScope = crate::api::UiScanScope::All,
        ..routing {
            omit_empty: all,
            inherits: Vec::new(),
            temporary_mode: None,
        }
        hint_characters: String = "asdfghjklqwertyuiopzvbnm".into(),
        ui: crate::api::text_select::Ui,
        lifecycle: crate::api::text_select::Lifecycle,
        bindings: Bindings = builtin_bindings(&[
            ("alt+c", "text_refine"),
            ("alt+backspace", "previous_level"),
            ("ctrl+backspace", "first_level"),
            ("tab", "text_cycle_page"),
            ("ctrl+a", "text_select_all"),
            ("backspace", "text_clear_selection"),
            ("ctrl+x", "text_clear_input"),
            ("mod+c enter", "text_copy"),
            ("primary+q", "normal"),
            ("esc", "cancel_selection"),
            ("?", "key_help"),
        ]),
    }
}

// ---------------------------------------------------------------------------
// [ui_hint]
// ---------------------------------------------------------------------------

mode_config! {
    pub struct UiHint {
        enabled: bool = true,
        strategy: UiScanStrategy = UiScanStrategy::Hybrid,
        scan_scope: crate::api::UiScanScope = crate::api::UiScanScope::All,
        vision: VisionOptions,
        /// Characters used to build hint labels.
        hint_characters: String = "asdfghjkl".into(),
        label_direction: LabelDirection = LabelDirection::Normal,
        /// Accessibility tree depth limit; 0 means unlimited.
        max_depth: u32 = 50,
        /// Soft budget for one accessibility scan. Incremental results remain
        /// visible while the scan continues.
        scan_timeout_ms: u64 = 2_500,
        /// Automatic retries when a scan times out or returns no targets.
        scan_retry_count: u32 = 1,
        /// Delay before an automatic retry, allowing a busy UI provider to settle.
        scan_retry_delay_ms: u64 = 200,
        /// Semantic roles that receive a hint.
        clickable_roles: Vec<String> = default_clickable_roles(),
        /// Skip the clickability heuristic and hint every matching role.
        ignore_clickable_check: bool = false,
        /// Hit-test each element; slower but removes occluded hints.
        visible_check_enabled: bool = false,
        placement: HintPlacement = HintPlacement::Bottom,
        /// Pixel offsets applied after placing each label relative to its element.
        label_x_offset: i32 = 0,
        label_y_offset: i32 = -8,
        ui: LabelUi = LabelUi {
            font_size: 17,
            ..Default::default()
        },
        boundary_highlight: BoundaryHighlight,
        #[serde(deserialize_with = "deserialize_search_input_ui")]
        search_input_ui: SearchInputUi = default_search_input_ui(),
        #[serde(deserialize_with = "deserialize_search_info_ui")]
        search_info_ui: SearchInputUi = default_search_info_ui(),
        #[serde(skip_serializing_if = "crate::api::style::SearchHighlightUi::is_default")]
        search_highlight: crate::api::style::SearchHighlightUi,
        /// Independent shortcuts for OCR, accessibility, coordinates and color.
        search_copy_keys: Vec<String> = ["ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4"]
            .map(String::from)
            .to_vec(),
        /// Search-result preview groups, highest priority first; all three are required.
        search_match_priority: [crate::api::hint::SearchMatchKind; 3] =
            crate::api::hint::DEFAULT_SEARCH_MATCH_PRIORITY,
        search_bindings: std::collections::BTreeMap<String, crate::api::point_sample::Action> =
            std::collections::BTreeMap::from([
                ("ctrl".into(), crate::api::point_sample::Action::PointToggle),
                ("tab".into(), crate::api::point_sample::Action::PointNext),
                (
                    "ctrl+shift+4".into(),
                    crate::api::point_sample::Action::ColorNext,
                ),
            ]),
        #[serde(skip_serializing_if = "SearchPoint::is_default")]
        search_point: SearchPoint,
        #[serde(
            deserialize_with = "deserialize_search_edit_keys",
            serialize_with = "serialize_search_edit_keys"
        )]
        search_edit_keys: std::collections::BTreeMap<crate::api::text_edit::EditAction, String> =
            crate::api::text_edit::default_keys(),
        ..routing {}
        lifecycle: TargetingLifecycle = TargetingLifecycle {
            after_finish: LifecycleAction::Mode(ModeId::normal()),
            after_click: LifecycleAction::Mode(ModeId::normal()),
        },
        /// Modifier held to expose the next label in each overlapping group.
        overlap_cycle_key: String = "shift".into(),
        bindings: Bindings = {
            let mut bindings = ui_hint_control_defaults();
            bindings.extend([
                ("primary+r".into(), Binding::RescanUi),
                ("primary+q".into(), Binding::Mode(ModeId::normal())),
            ]);
            bindings
        },
        app_configs: Vec<UiHintAppOverride>,
    }
}

pub(super) fn ui_hint_control_defaults() -> Bindings {
    Bindings::from([
        ("esc".into(), Binding::SelectionCancel),
        ("enter".into(), Binding::SelectionAccept),
        ("backspace".into(), Binding::SelectionParent),
        ("tab".into(), Binding::SelectionReopen),
        ("/".into(), Binding::SelectionSearch),
    ])
}

// A present but partial panel table must inherit that panel's defaults, rather
// than SearchInputUi's generic defaults (especially the info panel anchor).
fn deserialize_search_panel<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    defaults: SearchInputUi,
) -> Result<SearchInputUi, D::Error> {
    let overrides = toml::Table::deserialize(deserializer)?;
    let mut table = toml::Table::try_from(defaults).map_err(serde::de::Error::custom)?;
    table.extend(overrides);
    table.try_into().map_err(serde::de::Error::custom)
}

fn deserialize_search_input_ui<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SearchInputUi, D::Error> {
    deserialize_search_panel(deserializer, default_search_input_ui())
}

fn deserialize_search_info_ui<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SearchInputUi, D::Error> {
    deserialize_search_panel(deserializer, default_search_info_ui())
}

fn default_search_input_ui() -> SearchInputUi {
    SearchInputUi {
        label: LabelUi {
            font_size: 14,
            padding_y: 6,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn default_search_info_ui() -> SearchInputUi {
    SearchInputUi {
        position_mode: crate::api::style::PanelPositionMode::SearchInput,
        position: crate::api::style::PanelPosition::Percentages(
            ["44%", "50%", "56%", "50%"].map(String::from),
        ),
        width: 520,
        label: LabelUi {
            font_size: 14,
            padding_y: 16,
            ..Default::default()
        },
    }
}

impl UiHint {
    pub(crate) fn copy_chords(&self) -> Result<Vec<crate::api::KeyChord>, String> {
        self.search_copy_keys
            .iter()
            .map(|key| crate::api::KeyChord::parse(key))
            .collect()
    }

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
}

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

mode_config! {
    pub struct Grid {
        enabled: bool = true,
        /// Columns in every selection layer.
        grid_cols: u32 = 5,
        /// Rows in every selection layer.
        grid_rows: u32 = 4,
        /// One selection key per cell, in row-major keyboard order.
        keys: String = "12345qwertasdfgzxcvb".into(),
        /// Number of selection layers before the current cell becomes the target.
        max_depth: u32 = 3,
        /// Move the pointer to each selected cell's centre while drilling down.
        cursor_follow_selection: bool = true,
        ..routing {}
        lifecycle: TargetingLifecycle = TargetingLifecycle {
            after_finish: LifecycleAction::Mode(ModeId::normal()),
            after_click: LifecycleAction::Finish,
        },
        ui: GridUi = GridUi {
            label: LabelUi {
                font_size: 20,
                ..Default::default()
            },
            ..Default::default()
        },
        bindings: Bindings = with_grid_shortcuts(grid_control_defaults()),
        app_configs: Vec<AppOverride>,
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

mode_config! {
    pub struct RecursiveGrid {
        enabled: bool = true,
        grid_cols: u32 = 3,
        grid_rows: u32 = 3,
        /// Cell keys; must hold `grid_cols * grid_rows` characters.
        keys: String = "qweasdzxc".into(),
        min_size_width: u32 = 1,
        min_size_height: u32 = 1,
        /// Recursion limit, 1..=20.
        max_depth: u32 = 10,
        /// Move the pointer to each selected cell's centre while drilling down.
        cursor_follow_selection: bool = true,
        ..routing {}
        lifecycle: TargetingLifecycle,
        /// Per-depth layout overrides.
        layers: Vec<GridLayer>,
        ui: RecursiveGridUi,
        bindings: Bindings = with_grid_shortcuts(grid_control_defaults()),
        app_configs: Vec<AppOverride>,
    }
}

pub(super) fn grid_control_defaults() -> Bindings {
    Bindings::from([
        ("enter".into(), Binding::SelectionAccept),
        ("esc".into(), Binding::SelectionCancel),
        ("space".into(), Binding::SelectionReset),
        ("tab".into(), Binding::SelectionParent),
        ("backspace".into(), Binding::SelectionParent),
    ])
}

fn with_grid_shortcuts(mut bindings: Bindings) -> Bindings {
    bindings.extend([
        (
            "u".into(),
            Binding::ScanRegion(crate::api::binding::RegionTarget::UiHint),
        ),
        (
            "i".into(),
            Binding::ScanRegion(crate::api::binding::RegionTarget::TextSelect),
        ),
        ("`".into(), Binding::ToggleCursorFollowSelection),
        ("primary+q".into(), Binding::Mode(ModeId::normal())),
    ]);
    bindings
}

mode_config! {
    /// Directional bisection has no label alphabet or per-depth grid layout.
    pub struct Bisect {
        enabled: bool = true,
        min_size_width: u32 = 2,
        min_size_height: u32 = 2,
        max_depth: u32 = 20,
        cursor_follow_selection: bool = true,
        ..routing {}
        lifecycle: TargetingLifecycle,
        ui: BisectUi,
        bindings: Bindings = with_grid_shortcuts(bisect_control_defaults()),
        app_configs: Vec<AppOverride>,
    }
}

pub(super) fn bisect_control_defaults() -> Bindings {
    use crate::api::Direction;
    let mut bindings = grid_control_defaults();
    bindings.extend([
        ("h".into(), Binding::Bisect(Direction::Left)),
        ("j".into(), Binding::Bisect(Direction::Down)),
        ("k".into(), Binding::Bisect(Direction::Up)),
        ("l".into(), Binding::Bisect(Direction::Right)),
    ]);
    bindings
}

config_struct! {
    pub struct BisectUi {
        line_width: i32 = 1,
        line_color: Option<ThemedColor>,
        highlight_color: Option<ThemedColor>,
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

config_struct! {
    pub struct RecursiveGridUi {
        #[serde(flatten)]
        label: LabelUi = LabelUi {
            font_size: 0,
            ..Default::default()
        },
        line_width: i32 = 1,
        line_color: Option<ThemedColor>,
        highlight_color: Option<ThemedColor>,
        /// Draw a filled pill behind each cell key.
        label_background: bool = false,
        label_background_color: Option<ThemedColor>,
        /// Replace every cell label with this character, e.g. `·`.
        label_char: String,
        /// Hide labels when fitting them would require a smaller font.
        label_min_font_size: i32 = 6,
        /// Hide labels once a cell is smaller than the fitted font size times this
        /// multiplier. `0` disables this additional threshold.
        label_autohide_multiplier: f64 = 1.5,
        sub_key_preview: bool = false,
        sub_key_preview_font_size: i32 = 8,
        sub_key_preview_text_color: Option<ThemedColor>,
        sub_key_preview_autohide_multiplier: f64 = 1.5,
    }
}

// ---------------------------------------------------------------------------
// [scroll] and [pointer]
//
// These are tuning parameters, not modes: scrolling is a set of bindings that
// any mode can use, so there is no `scroll` mode to enter.
// ---------------------------------------------------------------------------

config_struct! {
    pub struct Scroll {
        /// Ordinary held scrolling in pixels per second; zero is tap-only.
        speed: f64 = 500.0,
        /// Pixels for `scroll_up` and friends.
        scroll_step: i32 = 50,
        /// Pixels for `scroll_half_up` and friends.
        scroll_step_half: i32 = 500,
        /// Pixels for `scroll_full_up` and friends.
        scroll_step_full: i32 = 1_000_000,
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

config_struct! {
    /// Keyboard-driven pointer acceleration.
    pub struct Pointer {
        /// Initial pointer velocity in pixels per second.
        initial_speed: f64 = 1000.0,
        /// Maximum pointer velocity in pixels per second.
        max_speed: f64 = 2200.0,
        /// Velocity added per second while held, in pixels per second².
        acceleration: f64 = 3000.0,
        /// Ease acceleration at both ends instead of changing velocity linearly.
        smooth_acceleration: bool = true,
        /// Immediate pixel distance for a tap shorter than the next display update.
        tap_distance: f64 = 2.5,
        slow_multiplier: f64 = 0.35,
        precision_multiplier: f64 = 0.12,
        fast_multiplier: f64 = 2.0,
    }
}

config_struct! {
    pub struct SearchPoint {
        #[serde(skip_serializing_if = "SearchPoint::default_field_modes")]
        field_modes: [crate::api::point_sample::FieldMode; 4] =
            crate::api::point_sample::DEFAULT_FIELD_MODES,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_background_color: Option<ThemedColor>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_border_color: Option<ThemedColor>,
        #[serde(skip_serializing_if = "crate::api::style::SearchColorPreview::is_default")]
        color_preview: crate::api::style::SearchColorPreview,
        /// First entry is the initial format. Cycle order is preserved.
        #[serde(skip_serializing_if = "SearchPoint::default_formats")]
        color_formats: Vec<crate::api::point_sample::ColorFormat> = {
            use crate::api::point_sample::ColorFormat::*;
            vec![Hex, Rgb, Hsl]
        },
        #[serde(skip_serializing_if = "Option::is_none")]
        marker_color: Option<ThemedColor>,
        #[serde(skip_serializing_if = "SearchPoint::default_radius")]
        marker_radius: u16 = 6,
        #[serde(skip_serializing_if = "SearchPoint::default_width")]
        marker_width: u16 = 2,
    }
}

impl SearchPoint {
    fn default_field_modes(value: &[crate::api::point_sample::FieldMode; 4]) -> bool {
        *value == crate::api::point_sample::DEFAULT_FIELD_MODES
    }
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
    fn default_formats(value: &[crate::api::point_sample::ColorFormat]) -> bool {
        use crate::api::point_sample::ColorFormat::*;
        value == [Hex, Rgb, Hsl]
    }
    fn default_radius(value: &u16) -> bool {
        *value == 6
    }
    fn default_width(value: &u16) -> bool {
        *value == 2
    }
}

fn deserialize_search_edit_keys<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<crate::api::text_edit::EditAction, String>, D::Error> {
    use crate::api::text_edit::EditAction;
    // Public tables use key = action. Group alternatives by action only while
    // loading, so the existing compilation and input paths remain unchanged.
    let bindings = std::collections::BTreeMap::<String, EditAction>::deserialize(deserializer)?;
    let mut keys = crate::api::text_edit::default_keys();
    let mut overrides: std::collections::BTreeMap<EditAction, String> = Default::default();
    for (alternatives, action) in bindings {
        overrides
            .entry(action)
            .and_modify(|keys| {
                keys.push(' ');
                keys.push_str(&alternatives);
            })
            .or_insert(alternatives);
    }
    keys.extend(overrides);
    Ok(keys)
}

fn serialize_search_edit_keys<S: serde::Serializer>(
    actions: &std::collections::BTreeMap<crate::api::text_edit::EditAction, String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut table = serializer.serialize_map(Some(actions.len()))?;
    let mut used = std::collections::BTreeSet::new();
    for (action, keys) in actions {
        // Empty groups disable an action. Distinct empty groups keep
        // that behavior representable when exporting several disabled actions.
        let mut keys = std::borrow::Cow::Borrowed(keys.as_str());
        while !used.insert(keys.to_string()) {
            if !keys.trim().is_empty() {
                return Err(serde::ser::Error::custom(
                    "duplicate search editing key group",
                ));
            }
            keys.to_mut().push(' ');
        }
        table.serialize_entry(keys.as_ref(), action)?;
    }
    table.end()
}
