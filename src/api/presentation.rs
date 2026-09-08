//! Borrowed presentation models and the host's platform-independent composer port.
//! Modes describe content here; layout, typography and overlay primitives belong
//! to the host implementation. Views are consumed synchronously, never retained.

use super::hint::CompactHint;
use super::style::LabelUi;
use super::style::{BoundaryHighlight, HintPlacement, SearchInputUi};
use super::theme::ThemedColor;
use super::window::{WindowId, WindowInfo};
use super::window_layout::LayoutTree;
use super::{HostContext, OverlayScene, Rect};
use std::collections::BTreeMap;
pub mod hint_cache;
pub use hint_cache::VisualLayerPlan;

/// Stateless scene-composition port supplied by the host. Implementations must
/// consume borrowed views within the call and keep native resources in Backend.
pub trait Presenter: Send + Sync {
    fn compose(&self, view: View<'_>, context: &HostContext<'_>) -> OverlayScene;
    fn prepare_hints(
        &self,
        content: HintContent<'_>,
        layers: &mut VisualLayerPlan,
        workspace: &mut Option<Vec<(usize, Rect)>>,
        context: &HostContext<'_>,
    );
}

pub enum View<'a> {
    Empty,
    Grid(GridView<'a>),
    RecursiveGrid(RecursiveGridView<'a>),
    Window(WindowView<'a>),
    ScreenSelector(ScreenSelectorView<'a>),
    Hints(HintView<'a>),
    HintSelection(HintSelectionView<'a>),
    Status(StatusView<'a>),
}

#[derive(Clone, Copy)]
pub struct GridLayout<'a> {
    pub rows: usize,
    pub cols: usize,
    pub keys: &'a [char],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GridStyle {
    pub label: LabelUi,
    pub matched_background_color: Option<ThemedColor>,
    pub matched_border_color: Option<ThemedColor>,
}

pub struct GridView<'a> {
    pub layout: GridLayout<'a>,
    pub ui: &'a GridStyle,
    pub current: Option<Rect>,
    pub root: Option<Rect>,
    pub terminal: bool,
    pub depth: u32,
    pub max_depth: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecursiveGridStyle {
    pub label: LabelUi,
    pub line_width: i32,
    pub line_color: Option<ThemedColor>,
    pub highlight_color: Option<ThemedColor>,
    pub label_background: bool,
    pub label_background_color: Option<ThemedColor>,
    pub label_char: String,
    pub label_min_font_size: i32,
    pub label_autohide_multiplier: f64,
    pub sub_key_preview: bool,
    pub sub_key_preview_font_size: i32,
    pub sub_key_preview_text_color: Option<ThemedColor>,
    pub sub_key_preview_autohide_multiplier: f64,
}

pub struct RecursiveGridView<'a> {
    pub layout: GridLayout<'a>,
    pub next_layout: GridLayout<'a>,
    pub ui: &'a RecursiveGridStyle,
    pub current: Option<Rect>,
    pub root: Option<Rect>,
    pub terminal: bool,
    pub can_descend: bool,
}

pub struct WindowView<'a> {
    pub ui: &'a LabelUi,
    pub border_width: f64,
    pub target: Option<&'a WindowInfo>,
    pub screen: usize,
    pub inventory: &'a BTreeMap<WindowId, WindowInfo>,
    pub visible: &'a [WindowId],
    pub numbers: &'a BTreeMap<WindowId, u32>,
    pub tree: Option<&'a LayoutTree>,
    pub gap: f64,
}

pub struct ScreenSelectorView<'a> {
    pub cells: &'a [(String, usize, Rect)],
    pub input: &'a str,
}

#[derive(Clone, Copy)]
pub struct HintStyle<'a> {
    pub ui: &'a LabelUi,
    pub placement: HintPlacement,
    pub label_x_offset: i32,
    pub label_y_offset: i32,
    pub boundary_highlight: &'a BoundaryHighlight,
    pub search_input_ui: &'a SearchInputUi,
}

#[derive(Clone, Copy)]
pub struct HintContent<'a> {
    pub hints: &'a [CompactHint<usize>],
    pub prefix: &'a str,
    pub search: Option<&'a str>,
    pub scan_bounds: Option<Rect>,
    pub style: HintStyle<'a>,
}

pub struct HintView<'a> {
    pub content: HintContent<'a>,
    pub layers: &'a VisualLayerPlan,
    pub active_layer: Option<usize>,
}

pub struct HintSelectionView<'a> {
    pub target: Option<Rect>,
    pub scan_bounds: Option<Rect>,
    pub boundary: &'a BoundaryHighlight,
}

pub struct StatusView<'a> {
    pub text: &'a str,
    pub ui: &'a LabelUi,
    pub clip: Option<Rect>,
}
