//! Borrowed presentation models and the host's platform-independent composer port.
//! Modes describe content here; layout, typography and overlay primitives belong
//! to the host implementation. Views are consumed synchronously, never retained.

use super::hint::CompactHint;
use super::style::compiled::{BoundaryHighlight, LabelUi};
use super::style::{CompiledSearchPanel, HintPlacement};
use super::theme::CompiledColor as ThemedColor;
use super::window::{WindowId, WindowInfo};
use super::window_layout::LayoutTree;
use super::{HostContext, OverlayScene, Rect};
use std::collections::BTreeMap;
pub mod hint_cache;
pub use hint_cache::VisualLayerPlan;

/// Stateless scene-composition port supplied by the host. Implementations must
/// consume borrowed views within the call and keep native resources in Backend.
pub trait Presenter: Send + Sync {
    /// Bounded preparation after the first scene has been submitted.
    fn warm_hint_scene(
        &self,
        _cache: &HintSceneCache,
        _content: HintContent<'_>,
        _layers: &VisualLayerPlan,
        _context: &HostContext<'_>,
        _limit: usize,
    ) -> bool {
        true
    }
    fn input_panel(
        &self,
        ui: &CompiledSearchPanel,
        window: Option<Rect>,
        context: &HostContext<'_>,
    ) -> (Rect, crate::api::overlay::SharedLabelStyle);
    fn compose(&self, view: View<'_>, context: &HostContext<'_>) -> OverlayScene;
    fn prepare_hints(
        &self,
        content: HintContent<'_>,
        layers: &mut VisualLayerPlan,
        workspace: &mut Option<Vec<(usize, Rect)>>,
        context: &HostContext<'_>,
    );
}

// Views are synchronous borrowed descriptions. Boxing the largest variant
// would add an allocation to every Hint frame just to reduce the enum's stack.
#[allow(clippy::large_enum_variant)]
pub enum View<'a> {
    Empty,
    Grid(GridView<'a>),
    RecursiveGrid(RecursiveGridView<'a>),
    Bisect(BisectView<'a>),
    Window(WindowView<'a>),
    ScreenSelector(ScreenSelectorView<'a>),
    Hints(HintView<'a>),
    TextSelect(TextSelectView<'a>),
    QuickSwitch(QuickSwitchView<'a>),
    HintSelection(HintSelectionView<'a>),
    Status(StatusView<'a>),
}

pub struct QuickSwitchRow {
    pub label: super::overlay::OverlayText,
    pub mode: super::ModeId,
}

pub struct QuickSwitchView<'a> {
    pub rows: &'a [QuickSwitchRow],
    pub prefix: &'a str,
    pub styles: &'a super::style::QuickSwitchStyles,
    pub position: super::style::QuickSwitchPosition,
    pub window: Option<Rect>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct BisectStyle {
    pub line_width: i32,
    pub line_color: Option<ThemedColor>,
    pub highlight_color: Option<ThemedColor>,
}

pub struct BisectView<'a> {
    pub ui: &'a BisectStyle,
    pub current: Option<Rect>,
    pub root: Option<Rect>,
    pub terminal: bool,
    /// Whether horizontal / vertical narrowing is still possible.
    pub axes: (bool, bool),
}

#[derive(Default)]
pub struct WindowTextCache {
    pub(crate) entries: std::cell::RefCell<BTreeMap<WindowId, std::sync::Arc<[String]>>>,
}
impl WindowTextCache {
    pub fn clear(&mut self) {
        self.entries.get_mut().clear();
    }
}

#[derive(Clone, Copy)]
pub struct WindowView<'a> {
    pub selected: &'a [WindowId],
    pub text_cache: Option<&'a WindowTextCache>,
    pub configurable_position: bool,
    pub tabs: &'a super::window_tabs::TabState,
    pub group_input: bool,
    pub styles: &'a super::style::WindowStyles,
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
    pub search_input_ui: &'a CompiledSearchPanel,
    pub search_highlight: &'a super::style::compiled::SearchHighlightUi,
}

#[derive(Clone, Copy)]
pub struct HintLabelBounds {
    pub bounds: Rect,
    pub scale: f64,
}

impl HintLabelBounds {
    pub fn new(bounds: Rect, screen_scale: f64) -> Self {
        Self {
            bounds,
            scale: if cfg!(target_os = "windows") && screen_scale.is_finite() {
                screen_scale.max(1.0)
            } else {
                1.0
            },
        }
    }
}

#[derive(Clone, Copy)]
pub struct HintContent<'a> {
    pub hints: &'a [CompactHint<usize>],
    /// Character count shared by the retained label code space, if uniform.
    /// This is not inferred from the size of a filtered or streamed batch.
    pub uniform_label_chars: Option<usize>,
    pub prefix: &'a str,
    pub search: Option<&'a str>,
    pub search_selection: crate::api::text_edit::Selection,
    pub scan_bounds: Option<Rect>,
    /// Optional display bounds for region-scan labels, independent of capture.
    pub label_bounds: Option<HintLabelBounds>,
    pub style: HintStyle<'a>,
}

pub struct HintView<'a> {
    pub cache: Option<&'a HintSceneCache>,
    /// Current text-only preview, independent of UIHint's information panel.
    pub highlighted_hint: Option<&'a CompactHint<usize>>,
    pub point: Option<HintPointView<'a>>,
    pub info: Option<HintInfoView<'a>>,
    pub window_bounds: &'a Option<Rect>,
    pub content: HintContent<'a>,
    pub layers: &'a VisualLayerPlan,
    pub active_layer: Option<usize>,
}

pub struct TextSelectView<'a> {
    pub hints: HintView<'a>,
    pub ui: &'a CompiledSearchPanel,
    /// Page-local hints index the original units and selection, without a
    /// temporary array of borrowed chip data on each redraw.
    pub chips: &'a [super::hint::CompactHint<usize>],
    pub unit_text: &'a str,
    pub units: &'a [std::ops::Range<usize>],
    pub grid_min_width: f64,
    pub text: &'a str,
    pub status: &'a str,
    pub summary: &'a str,
    pub page: (usize, usize),
    pub page_keys: &'a str,
    pub region_bounds: &'a [Rect],
    pub selected: &'a [bool],
    pub selection_ui: &'a super::text_select::Ui<ThemedColor>,
}

/// Session-owned portable presentation data, never native drawing resources.
#[derive(Default)]
pub struct HintSceneCache {
    pub(crate) state: std::cell::RefCell<HintSceneState>,
}

#[derive(Default)]
pub(crate) struct HintSceneState {
    pub scene: Option<std::sync::Arc<OverlayScene>>,
    pub palette: Option<super::Palette>,
    pub scale: u64,
    pub active_layer: Option<usize>,
    pub prefix: String,
    pub highlighted: Option<super::overlay::SharedLabelStyle>,
    pub warm_cursor: usize,
    pub rank_offsets: Vec<usize>,
    pub prepared: usize,
    /// Scene slots follow hints with monotonically increasing target indices.
    /// Filtered hints themselves serve as the index; no second map is needed.
    pub target_order: bool,
    /// Target-to-slot positions recorded during emission or bounded prewarming. Empty
    /// for ordinary first scenes and directly addressable, unstacked results.
    pub draw_slots: Vec<usize>,
}

impl HintSceneCache {
    pub fn clear(&mut self) {
        *self.state.get_mut() = HintSceneState::default();
    }
    /// Invalidate content within a scan without reallocating its lookup buffer
    /// or recompiling the same highlight style on the next filtered scene.
    pub(crate) fn invalidate(&mut self) {
        let state = self.state.get_mut();
        state.scene = None;
        state.warm_cursor = 0;
        state.rank_offsets.clear();
        state.draw_slots.clear();
        state.prepared = 0;
    }
    pub fn needs_warm(&self) -> bool {
        let state = self.state.borrow();
        state
            .scene
            .as_ref()
            .is_some_and(|scene| state.prepared < scene.labels.len() || state.highlighted.is_none())
    }
    #[inline(never)]
    pub fn capture(
        &self,
        scene: std::sync::Arc<OverlayScene>,
        palette: &super::Palette,
        scale: f64,
        active_layer: Option<usize>,
        prefix: &str,
        target_order: bool,
    ) {
        let mut state = self.state.borrow_mut();
        if state.palette.as_ref() != Some(palette) {
            state.palette = Some(palette.clone());
            state.highlighted = None;
        }
        state.scene = Some(scene);
        state.scale = scale.to_bits();
        state.active_layer = active_layer;
        state.prefix.clear();
        state.prefix.push_str(prefix);
        state.target_order = target_order;
        state.warm_cursor = 0;
        state.rank_offsets.clear();
        state.draw_slots.clear();
        state.prepared = 0;
    }
}

pub struct HintInfoView<'a> {
    pub point: Option<HintPointInfo<'a>>,
    pub field_modes: &'a [crate::api::point_sample::FieldMode; 4],
    pub preview: &'a HintInfoPreview,
    pub targets: &'a [crate::api::UiTarget],
    pub hints: &'a [CompactHint<usize>],
    pub multiple: bool,
    pub ui: &'a CompiledSearchPanel,
    pub titles: &'a [String; 4],
}

pub struct HintPointView<'a> {
    pub adjusting: bool,
    /// Zero means the initial collection center; 1..=count selects a member.
    pub position: usize,
    pub count: usize,
    pub point: &'a crate::api::Point,
    pub color: Option<crate::api::theme::CompiledColor>,
    pub radius: u16,
    pub width: u16,
}

pub struct HintPointInfo<'a> {
    pub point: &'a crate::api::Point,
    pub target: Option<usize>,
    pub color: Option<crate::api::Color>,
    pub format: crate::api::point_sample::ColorFormat,
    pub colors: &'a [crate::api::point_sample::SampledColor],
}

/// Session-owned preview storage; closing the editor does not discard capacity.
#[derive(Default)]
pub struct HintInfoPreview(std::cell::RefCell<[String; 4]>);

impl HintInfoView<'_> {
    pub fn field_count(&self) -> usize {
        if self.multiple && self.point.is_none() {
            3
        } else {
            4
        }
    }

    #[cfg(test)]
    pub fn values(&self) -> [String; 4] {
        std::array::from_fn(|field| self.field_text(field, usize::MAX))
    }

    pub fn preview_values(&self) -> std::cell::Ref<'_, [String; 4]> {
        {
            let mut values = self.preview.0.borrow_mut();
            for (field, value) in values.iter_mut().enumerate() {
                self.write_field(field, 512, value);
            }
        }
        self.preview.0.borrow()
    }

    pub fn copy_value(&self, field: usize) -> String {
        self.field_text(field, usize::MAX)
    }

    fn field_text(&self, field: usize, limit: usize) -> String {
        let mut text = String::new();
        self.write_field(field, limit, &mut text);
        text
    }

    fn write_field(&self, field: usize, limit: usize, text: &mut String) {
        use std::fmt::Write;
        if let Some(point) = &self.point
            && (!self.multiple
                || self.field_modes.get(field)
                    == Some(&crate::api::point_sample::FieldMode::Switch))
        {
            text.clear();
            match field {
                0 | 1 => {
                    if let Some(target) = point.target.and_then(|i| self.targets.get(i)) {
                        let mut out = InfoText {
                            text,
                            position: 0,
                            remaining: limit,
                        };
                        if field == 0 {
                            let _ = write_ocr_text(&mut out, target.ocr_text().chars());
                        } else if target.details.is_none()
                            || !target.accessibility_text().is_empty()
                        {
                            let _ =
                                write!(out, "{} · {}", target.accessibility_text(), target.role);
                        }
                    }
                }
                2 => {
                    let _ = write!(text, "{:.0}, {:.0}", point.point.x, point.point.y);
                }
                3 => {
                    if let Some(color) = point.color {
                        let _ = point.format.write(text, color);
                    }
                }
                _ => {}
            }
            return;
        }
        if field >= self.field_count() {
            text.clear();
            return;
        }
        let mut out = InfoText {
            text,
            position: 0,
            remaining: limit,
        };
        let mut present = false;
        for (index, hint) in self.hints.iter().enumerate() {
            if index > 0 && out.write_str("\n").is_err() {
                break;
            }
            let target = &self.targets[hint.value];
            let result = match field {
                0 => {
                    present |= !target.ocr_text().is_empty();
                    write_ocr_text(&mut out, target.ocr_text().chars())
                }
                1 if target.details.is_none() || !target.accessibility_text().is_empty() => {
                    present = true;
                    write!(out, "{} · {}", target.accessibility_text(), target.role)
                }
                2 => {
                    present = true;
                    let center = target.rect.center();
                    write!(out, "{:.0}, {:.0}", center.x, center.y)
                }
                3 => {
                    if let Some(point) = &self.point {
                        if let Some(color) = point.colors.get(index).and_then(|sample| sample.color)
                        {
                            present = true;
                            point.format.write(&mut out, color)
                        } else {
                            Ok(())
                        }
                    } else if let Some(color) = target.details.as_ref().and_then(|d| d.color) {
                        present = true;
                        write!(
                            out,
                            "#{:02X}{:02X}{:02X}{:02X}",
                            color.r, color.g, color.b, color.a
                        )
                    } else {
                        Ok(())
                    }
                }
                _ => Ok(()),
            };
            if result.is_err() {
                break;
            }
        }
        out.text.truncate(if present { out.position } else { 0 });
    }
}

/// Stream OCR whitespace cleanup into the existing preview/clipboard buffer.
/// Keep word boundaries and line breaks, but remove recognition gaps between
/// Chinese characters and around Chinese punctuation. No temporary String.
/// A character stream also lets Text Select join OCR rows before this same cleanup.
pub(crate) fn write_ocr_text(
    out: &mut impl std::fmt::Write,
    text: impl IntoIterator<Item = char>,
) -> std::fmt::Result {
    let chinese = |c: char| {
        matches!(c as u32,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
    };
    let punctuation = |c: char| "，。！？；：、（）【】《》〈〉「」『』“”‘’".contains(c);
    let mut previous = None;
    let mut space = false;
    let mut chars = text.into_iter().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.write_char('\n')?;
            previous = None;
            space = false;
        } else if c.is_whitespace() {
            space = true;
        } else {
            if space
                && let Some(left) = previous
                && !(chinese(left) && chinese(c) || punctuation(left) || punctuation(c))
            {
                out.write_char(' ')?;
            }
            out.write_char(c)?;
            previous = Some(c);
            space = false;
        }
    }
    Ok(())
}

/// Stops preview formatting at a Unicode boundary, without building full clipboard data.
struct InfoText<'a> {
    text: &'a mut String,
    position: usize,
    remaining: usize,
}

impl InfoText<'_> {
    fn append(&mut self, value: &str) {
        let end = self.position + value.len();
        if self.text.get(self.position..end) != Some(value) {
            self.text.truncate(self.position);
            self.text.push_str(value);
        }
        self.position = end;
    }
}

impl std::fmt::Write for InfoText<'_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        if self.remaining == usize::MAX {
            self.append(value);
            return Ok(());
        }
        if let Some((end, _)) = value.char_indices().nth(self.remaining) {
            self.append(&value[..end]);
            self.append("…");
            self.remaining = 0;
            return Err(std::fmt::Error);
        }
        self.remaining -= value.chars().count();
        self.append(value);
        Ok(())
    }
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

#[cfg(test)]
mod info_preview_tests {
    use super::*;

    #[test]
    fn ocr_cleanup_preserves_words_lines_and_unicode_in_both_outputs() {
        for (input, expected) in [
            ("OCR 和 辅 助 功 能 内 容", "OCR 和辅助功能内容"),
            (
                "  复\t制\u{3000}文\u{a0}字 ， 测 试 。  ",
                "复制文字，测试。",
            ),
            (
                "  macOS   Gatekeeper  拒 绝 打 开  ",
                "macOS Gatekeeper 拒绝打开",
            ),
            (
                "中 文  \r\n  English   words\n下 一 项",
                "中文\nEnglish words\n下一项",
            ),
            ("𠀀 𠀁 🦀 test", "𠀀𠀁 🦀 test"),
        ] {
            for limit in [512, usize::MAX] {
                let mut text = String::new();
                let mut out = InfoText {
                    text: &mut text,
                    position: 0,
                    remaining: limit,
                };
                write_ocr_text(&mut out, input.chars()).unwrap();
                assert_eq!(text, expected);
            }
        }
        let mut text = String::new();
        let mut out = InfoText {
            text: &mut text,
            position: 0,
            remaining: 2,
        };
        assert!(write_ocr_text(&mut out, "中 文 字 后 续".chars()).is_err());
        assert_eq!(text, "中文…");
    }

    #[test]
    #[ignore = "allocation measurement; run alone with --test-threads=1"]
    fn search_preview_memory_is_bounded_for_large_multiselections() {
        let config = crate::config::Config::default();
        let settings = crate::app::mode_catalog::hint_settings(&config);
        let target = crate::api::UiTarget {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            name: "按钮".into(),
            role: crate::api::SemanticRole::Button,
            details: Some(Box::new(crate::api::geometry::UiTargetDetails {
                ocr: "中文🦀".repeat(20_000),
                accessibility: "按钮".into(),
                color: None,
            })),
        };
        let targets = [target];
        let hints: Vec<_> = (0..10_000)
            .map(|_| CompactHint {
                label: Default::default(),
                bounds: targets[0].rect,
                value: 0,
            })
            .collect();
        let preview = HintInfoPreview::default();
        let mut view = HintInfoView {
            point: None,
            field_modes: &crate::api::point_sample::DEFAULT_FIELD_MODES,
            preview: &preview,
            targets: &targets,
            hints: &hints,
            multiple: true,
            ui: &settings.search_info_ui,
            titles: &settings.search_titles,
        };
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        let values = view.preview_values();
        let stats = region.change();
        assert!(values.iter().all(|value| value.chars().count() <= 513));
        assert!(stats.bytes_allocated < 32_768, "{stats:?}");
        assert!(values[0].ends_with('…'));
        assert!(values[3].is_empty());
        println!(
            "10000-target preview: {} allocated bytes, {} allocations, {} reallocations",
            stats.bytes_allocated, stats.allocations, stats.reallocations
        );
        drop(values);
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for index in 0..100 {
            view.hints = if index % 2 == 0 { &hints[..1] } else { &hints };
            let values = view.preview_values();
            assert!(values[0].ends_with('…'));
        }
        let stats = region.change();
        assert_eq!(stats.allocations + stats.reallocations, 0, "{stats:?}");
        println!("100 warmed previews: {stats:?}");
    }

    #[test]
    fn preview_reuses_storage_across_changed_unicode_empty_and_multiple_fields() {
        let config = crate::config::Config::default();
        let settings = crate::app::mode_catalog::hint_settings(&config);
        let preview = HintInfoPreview::default();
        let hints = [CompactHint {
            label: Default::default(),
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
            value: 0,
        }];
        let mut targets = [crate::api::UiTarget {
            rect: hints[0].bounds,
            name: String::new(),
            role: crate::api::SemanticRole::Button,
            details: Some(Box::new(crate::api::geometry::UiTargetDetails {
                ocr: "长文字🦀".repeat(200),
                accessibility: String::new(),
                color: Some(crate::api::Color::rgb(1, 2, 3)),
            })),
        }];
        let mut capacity = 0;
        for (index, text) in [
            "长文字🦀".repeat(200),
            "🙂短".into(),
            String::new(),
            "新内容".into(),
        ]
        .into_iter()
        .enumerate()
        {
            targets[0].details.as_mut().unwrap().ocr = text;
            let view = HintInfoView {
                point: None,
                field_modes: &crate::api::point_sample::DEFAULT_FIELD_MODES,
                preview: &preview,
                targets: &targets,
                hints: &hints,
                multiple: index % 2 != 0,
                ui: &settings.search_info_ui,
                titles: &settings.search_titles,
            };
            let values = view.preview_values();
            assert_eq!(
                *values,
                std::array::from_fn(|field| view.field_text(field, 512))
            );
            if index == 0 {
                capacity = values[0].capacity();
            }
            assert_eq!(values[0].capacity(), capacity);
        }
    }
}
