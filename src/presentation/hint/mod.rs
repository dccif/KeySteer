//! UI Hint typography, placement and cached overlap planning.
pub(crate) mod layers;
use crate::api::hint::CompactHint;
use crate::api::overlay::{
    Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape, SharedLabelStyle,
};
use crate::api::presentation::hint_cache::INLINE_LABELS;
use crate::api::presentation::{
    HintContent, HintSelectionView, HintStyle, HintView, StatusView, VisualLayerPlan,
};
use crate::api::style::AUTO;
use crate::api::{HostContext, Palette, Rect};
use layers::build_visual_layer_plan;
use smallvec::SmallVec;
const MIN_VISUAL_STACK_AREA_RATIO: f64 = 0.20;
const MIN_TEXT_OCCLUSION_EXTENT: f64 = 0.5;
const HINT_LAYER_Z_BASE: i32 = 1;
const SEARCH_INPUT_Z_INDEX: i32 = 10_000;

fn push_number(text: &mut crate::api::overlay::OverlayText, mut value: usize) {
    let mut digits = [0_u8; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in &digits[start..] {
        text.push(char::from(*digit));
    }
}

pub(super) fn input_panel(
    cfg: &crate::api::style::CompiledSearchPanel,
    window: Option<Rect>,
    ctx: &HostContext<'_>,
) -> (Rect, crate::api::overlay::SharedLabelStyle) {
    let style = cfg.for_appearance(ctx.palette.appearance).panel.clone();
    let scale = super::label_scale(ctx.scale());
    let area = if cfg.position_mode == crate::api::style::PanelPositionMode::Window {
        window.unwrap_or_else(|| ctx.active_bounds())
    } else {
        ctx.active_bounds()
    };
    let height = (style.font_size * 1.8 + style.padding_y * 2.0) * scale;
    let rect = cfg.position.place(
        area,
        (cfg.width * scale).min(area.width),
        height,
        cfg.x_offset * scale,
        cfg.y_offset * scale,
    );
    (rect, style)
}
const AUTO_HINT_PADDING_X_RATIO: f64 = 2.0 / 17.0;
const AUTO_HINT_PADDING_Y_RATIO: f64 = 0.06;
pub(crate) fn resolved_hint_label_style(config: &HintStyle<'_>, palette: &Palette) -> LabelStyle {
    let mut style = config.ui.resolve(
        palette,
        palette.surface_label(),
        palette.text,
        palette.accent,
    );
    // UI Hint labels are deliberately denser than larger grid cells and
    // badges. Keep -1 as a font-relative auto value without changing the
    // shared LabelUi auto rules used by those other components.
    if config.ui.padding_x == AUTO {
        style.padding_x = (style.font_size * AUTO_HINT_PADDING_X_RATIO).round();
    }
    if config.ui.padding_y == AUTO {
        style.padding_y = (style.font_size * AUTO_HINT_PADDING_Y_RATIO).round();
    }
    style
}

fn hint_label_width(style: &LabelStyle, characters: usize) -> f64 {
    style.font_size * 0.75 * characters as f64 + style.padding_x * 2.0
}

// Keep uniform width and placement visible to the per-label planning loops.
#[inline(always)]
pub(crate) fn placed_hint_rect(
    config: &HintStyle<'_>,
    hint: &CompactHint<usize>,
    style: &LabelStyle,
    uniform_width: Option<f64>,
) -> Rect {
    let width = uniform_width
        .unwrap_or_else(|| hint_label_width(style, hint.label.as_str().chars().count()));
    let height = style.font_size * 1.4 + style.padding_y * 2.0;
    let placed = config.placement.place(&hint.bounds, width, height);
    Rect::new(
        placed.x + config.label_x_offset as f64,
        placed.y + config.label_y_offset as f64,
        placed.width,
        placed.height,
    )
}

#[cfg(target_os = "windows")]
pub(crate) fn visual_layer_scale(ctx: &HostContext<'_>, scan_bounds: Option<Rect>) -> f64 {
    let center = scan_bounds.unwrap_or_else(|| ctx.active_bounds()).center();
    let scale = ctx
        .screens
        .iter()
        .find(|screen| screen.bounds.contains(&center))
        .map_or(1.0, |screen| screen.scale);
    crate::api::overlay::normalized_label_scale(scale)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn visual_layer_scale(_ctx: &HostContext<'_>, _scan_bounds: Option<Rect>) -> f64 {
    1.0
}

#[cfg(target_os = "windows")]
pub(crate) fn visual_layer_rect(rect: Rect, scale: f64) -> Rect {
    if scale > 1.0 {
        crate::api::overlay::scaled_compact_label_rect(rect, scale)
    } else {
        rect
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn visual_layer_rect(rect: Rect, _scale: f64) -> Rect {
    rect
}

pub(crate) fn visually_stacked(
    left: Rect,
    right: Rect,
    horizontal_padding: f64,
    vertical_padding: f64,
) -> bool {
    let Some(intersection) = left.intersect(&right) else {
        return false;
    };
    if left.contains(&right.center()) || right.contains(&left.center()) {
        return true;
    }
    let intersection_area = intersection.width * intersection.height;
    let smaller_area = (left.width * left.height).min(right.width * right.height);
    (smaller_area > 0.0 && intersection_area >= smaller_area * MIN_VISUAL_STACK_AREA_RATIO)
        || obscures_label_text(left, right, horizontal_padding, vertical_padding)
        || obscures_label_text(right, left, horizontal_padding, vertical_padding)
}

fn obscures_label_text(
    cover: Rect,
    target: Rect,
    horizontal_padding: f64,
    vertical_padding: f64,
) -> bool {
    let inset_x = horizontal_padding.clamp(0.0, target.width / 2.0);
    let inset_y = vertical_padding.clamp(0.0, target.height / 2.0);
    let content = Rect::new(
        target.x + inset_x,
        target.y + inset_y,
        target.width - inset_x * 2.0,
        target.height - inset_y * 2.0,
    );
    cover.intersect(&content).is_some_and(|intersection| {
        intersection.width > MIN_TEXT_OCCLUSION_EXTENT
            && intersection.height > MIN_TEXT_OCCLUSION_EXTENT
    })
}

pub(crate) fn prepare_hints(
    content: HintContent<'_>,
    layers: &mut VisualLayerPlan,
    workspace: &mut Option<Vec<(usize, Rect)>>,
    ctx: &HostContext<'_>,
) {
    let style = resolved_hint_label_style(&content.style, ctx.palette);
    let uniform_width = content
        .uniform_label_chars
        .map(|chars| hint_label_width(&style, chars));
    let visual_scale = visual_layer_scale(ctx, content.scan_bounds);
    let visual_padding_x = (style.padding_x * visual_scale).round();
    let visual_padding_y = (style.padding_y * visual_scale).round();
    // An empty prefix accepts every label without decoding its inline text.
    let placements = || {
        content
            .hints
            .iter()
            .enumerate()
            .filter(|(_, hint)| {
                content.prefix.is_empty() || hint.label.as_str().starts_with(content.prefix)
            })
            .map(|(index, hint)| {
                let rect = placed_hint_rect(&content.style, hint, &style, uniform_width);
                (index, visual_layer_rect(rect, visual_scale))
            })
    };
    let visible = if content.prefix.is_empty() {
        content.hints.len()
    } else {
        content
            .hints
            .iter()
            .filter(|hint| hint.label.as_str().starts_with(content.prefix))
            .count()
    };
    let stacked = |left, right| visually_stacked(left, right, visual_padding_x, visual_padding_y);
    prepare_hint_plan(
        placements(),
        visible,
        content.hints.len(),
        uniform_width.is_some(),
        stacked,
        layers,
        workspace,
    );
}

#[inline(never)]
fn prepare_hint_plan(
    placements: impl Iterator<Item = (usize, Rect)>,
    visible: usize,
    hint_count: usize,
    uniform_size: bool,
    stacked: impl Fn(Rect, Rect) -> bool,
    layers: &mut VisualLayerPlan,
    workspace: &mut Option<Vec<(usize, Rect)>>,
) {
    if visible > INLINE_LABELS {
        let mut buffer = workspace.take().unwrap_or_default();
        buffer.clear();
        // Filtering loses the iterator's exact lower bound. The visible count
        // is already known, so avoid repeated growth on the first wide scan.
        buffer.reserve(visible);
        buffer.extend(placements);
        build_visual_layer_plan(&buffer, hint_count, uniform_size, stacked, layers);
        *workspace = Some(buffer);
    } else {
        let mut buffer = SmallVec::<[(usize, Rect); INLINE_LABELS]>::new();
        buffer.extend(placements);
        build_visual_layer_plan(&buffer, hint_count, uniform_size, stacked, layers);
    }
}

impl HintView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let visible_count = if self.content.prefix.is_empty() {
            self.content.hints.len()
        } else {
            self.content
                .hints
                .iter()
                .filter(|hint| hint.label.as_str().starts_with(self.content.prefix))
                .count()
        };
        // Appending a small search panel must not grow and copy the entire
        // target buffer. Include its labels, background, swatch and point now.
        let info_fields = self.info.as_ref().map_or(0, |info| info.field_count());
        let shape_capacity = if self.content.style.boundary_highlight.enabled {
            visible_count
        } else {
            0
        } + usize::from(self.point.is_some())
            + usize::from(self.info.is_some()) * 2;
        let label_capacity =
            visible_count + usize::from(self.content.search.is_some()) + info_fields * 3;
        let mut scene = OverlayScene::with_capacity(shape_capacity, label_capacity);
        scene.clip = self
            .content
            .scan_bounds
            .or_else(|| Some(ctx.active_bounds()));

        let mut label_style = resolved_hint_label_style(&self.content.style, palette);
        // This highlight belongs specifically to UI Hint's typed-prefix
        // interaction. Keep the generic overlay/config defaults unchanged.
        if self.content.style.ui.matched_text_color.is_none() {
            label_style.matched_text_color = Color::rgb(0xE4, 0xB4, 0x00);
        }
        let uniform_width = self
            .content
            .uniform_label_chars
            .map(|chars| hint_label_width(&label_style, chars));
        let label_style = SharedLabelStyle::from(label_style);

        // Optional outlines behind only the currently visible candidates.
        if self.content.style.boundary_highlight.enabled {
            let bh = &self.content.style.boundary_highlight;
            let shapes: &mut Vec<OverlayShape> = &mut scene.shapes;
            for hint in self.content.hints.iter().filter(|hint| {
                self.content.prefix.is_empty()
                    || hint.label.as_str().starts_with(self.content.prefix)
            }) {
                shapes.push(OverlayShape::Rect {
                    rect: hint.bounds,
                    fill: bh.fill(palette),
                    stroke: bh.stroke(palette),
                    stroke_width: bh.border_width.max(0) as f64,
                    corner_radius: bh.radius(),
                    z_index: 0,
                });
            }
        }

        // Remove non-matching labels as the prefix narrows. The matched part of
        // each remaining label is painted with `matched_text_color`.
        let typed = self.content.prefix;
        // Every emitted label passed starts_with, so this count already fits.
        let matched_prefix_len = typed.chars().count().min(u32::MAX as usize) as u32;
        let active_overlap_layer = self.active_layer;
        let z_for = |hint_index| {
            active_overlap_layer
                .and_then(|layer| self.layers.draw_rank(hint_index, layer))
                .and_then(|rank| i32::try_from(rank).ok())
                .map_or(HINT_LAYER_Z_BASE + 1, |rank| {
                    HINT_LAYER_Z_BASE.saturating_add(rank)
                })
        };
        // Emit final stable z order directly. Typical components are only
        // 2..=5 layers, so a few allocation-free linear passes are cheaper
        // than sorting and preserve equal-z source order exactly.
        let final_z = HINT_LAYER_Z_BASE
            .saturating_add(i32::try_from(self.layers.layer_count().max(1)).unwrap_or(i32::MAX));
        // Without an active overlap layer every label has the same rank.
        // Avoid traversing the entire hint list for empty z layers.
        let (first_z, final_z) = if active_overlap_layer.is_none() || self.layers.layer_count() == 0
        {
            (HINT_LAYER_Z_BASE + 1, HINT_LAYER_Z_BASE + 1)
        } else {
            (HINT_LAYER_Z_BASE, final_z)
        };
        // Detach shared storage once for the whole batch, not for every label.
        // Hint labels carry no window-placement annotations.
        let labels: &mut Vec<OverlayLabel> = &mut scene.labels;
        if let Some(layer) = active_overlap_layer.filter(|_| self.layers.is_canonical_stack()) {
            self.append_stack_labels(
                labels,
                &label_style,
                uniform_width,
                matched_prefix_len,
                layer,
            );
        } else {
            for z_index in first_z..=final_z {
                for (hint_index, hint) in
                    self.content.hints.iter().enumerate().filter(|(_, hint)| {
                        self.content.prefix.is_empty()
                            || hint.label.as_str().starts_with(self.content.prefix)
                    })
                {
                    if z_for(hint_index) != z_index {
                        continue;
                    }
                    let rect =
                        placed_hint_rect(&self.content.style, hint, &label_style, uniform_width);
                    labels.push(OverlayLabel {
                        matched_prefix_len,
                        z_index,
                        ..OverlayLabel::new(hint.label.as_str(), rect, label_style.clone())
                    });
                }
            }
        }

        if self.content.search.is_some() {
            self.append_search(&mut scene, ctx);
        }

        if let Some(point) = &self.point {
            let radius = f64::from(point.radius) * super::label_scale(ctx.scale());
            scene.push_shape(OverlayShape::Rect {
                rect: Rect::new(
                    point.point.x - radius,
                    point.point.y - radius,
                    radius * 2.0,
                    radius * 2.0,
                ),
                fill: Color::TRANSPARENT,
                stroke: point
                    .color
                    .and_then(|c| c.resolve(ctx.palette.appearance))
                    .unwrap_or(ctx.palette.accent),
                stroke_width: f64::from(point.width),
                corner_radius: radius,
                z_index: SEARCH_INPUT_Z_INDEX + 3,
            });
        }
        scene
    }

    // A verified clique keeps source order except for one raised item. Keep
    // this uncommon layout out of the ordinary label scene's stack frame.
    #[inline(never)]
    fn append_stack_labels(
        &self,
        labels: &mut Vec<OverlayLabel>,
        style: &SharedLabelStyle,
        uniform_width: Option<f64>,
        matched_prefix_len: u32,
        selected_layer: usize,
    ) {
        let top = self.layers.layer_count();
        let mut selected = None;
        let mut append = |hint: &CompactHint<usize>, rank: usize| {
            let rect = placed_hint_rect(&self.content.style, hint, style, uniform_width);
            labels.push(OverlayLabel {
                matched_prefix_len,
                z_index: HINT_LAYER_Z_BASE.saturating_add(i32::try_from(rank).unwrap_or(i32::MAX)),
                ..OverlayLabel::new(hint.label.as_str(), rect, style.clone())
            });
        };
        for (index, hint) in self.content.hints.iter().enumerate().filter(|(_, hint)| {
            self.content.prefix.is_empty() || hint.label.as_str().starts_with(self.content.prefix)
        }) {
            let rank = self.layers.draw_rank(index, selected_layer).unwrap_or(1);
            if rank == top {
                selected = Some(hint);
            } else {
                append(hint, rank);
            }
        }
        if let Some(hint) = selected {
            append(hint, top);
        }
    }

    // Keep panel layout and its larger temporaries out of ordinary label frames.
    #[inline(never)]
    fn append_search(&self, scene: &mut OverlayScene, ctx: &HostContext<'_>) {
        scene.clip = Some(ctx.active_bounds());
        let cfg = &self.content.style.search_input_ui;
        let (rect, style) = input_panel(cfg, *self.window_bounds, ctx);
        if let Some(info) = &self.info {
            self.append_info(info, rect, scene, ctx);
        }
        // Text, selection and caret are shared across native renderers.
        let scale = super::label_scale(ctx.scale());
        let adjusting = self.point.as_ref().is_some_and(|point| point.adjusting);
        let (background, border) = if adjusting {
            cfg.point_input_colors(ctx.palette.appearance, &style)
        } else {
            (style.background, style.border_color)
        };
        scene.push_shape(OverlayShape::Rect {
            rect,
            fill: background,
            stroke: border,
            stroke_width: style.border_width * scale,
            corner_radius: style.border_radius * scale,
            z_index: SEARCH_INPUT_Z_INDEX,
        });
        let text = self.content.search.unwrap_or_default();
        // Point editing suspends text editing. Hide its caret/selection,
        // preserving them so toggling back resumes at the same position.
        let font = style.font_size * scale;
        let area = rect.inset(
            (style.padding_x + style.border_width) * scale,
            (style.padding_y + style.border_width) * scale,
        );
        let mut text_area = area;
        let mut counter = crate::api::overlay::OverlayText::default();
        if adjusting {
            let radius = (2.5 * scale).min(area.width / 2.0);
            let dot = Rect::new(
                area.right() - radius * 2.0,
                area.center().y - radius,
                radius * 2.0,
                radius * 2.0,
            );
            scene.push_shape(OverlayShape::Rect {
                rect: dot,
                fill: match ctx.palette.appearance {
                    crate::api::Appearance::Light => Color::rgb(0x16, 0x85, 0x6B),
                    crate::api::Appearance::Dark => Color::rgb(0x68, 0xD9, 0xB1),
                },
                stroke: Color::TRANSPARENT,
                stroke_width: 0.0,
                corner_radius: radius,
                z_index: SEARCH_INPUT_Z_INDEX + 4,
            });
            let reserved = radius * 2.0 + font * 0.3;
            if let Some(point) = self.point.as_ref().filter(|p| p.count > 1) {
                push_number(&mut counter, point.position);
                counter.push('/');
                push_number(&mut counter, point.count);
            }
            text_area.width = (area.width - reserved).max(0.0);
        }
        let mut visible = crate::api::overlay::OverlayText::default();
        let mut edit = crate::api::text_edit::Selection::default();
        for (offset, character) in text.char_indices() {
            let character = if character.is_control() {
                ' '
            } else {
                character
            };
            visible.push(character);
            if offset < self.content.search_selection.cursor {
                edit.cursor = visible.len();
            }
            if offset < self.content.search_selection.anchor {
                edit.anchor = visible.len();
            }
        }
        visible.push_str(counter.as_str());
        let mut input = OverlayLabel::new(
            visible,
            text_area,
            if counter.is_empty() {
                cfg.input_style(ctx.palette.appearance)
            } else {
                cfg.point_input_style(ctx.palette.appearance)
            }
            .clone(),
        )
        .with_fixed_bounds()
        .with_z_index(SEARCH_INPUT_Z_INDEX + 2);
        input.scroll_to_cursor = true;
        input.trailing_text_len = counter.len() as u8;
        if !adjusting {
            input.edit = crate::api::overlay::LabelEdit::try_from(edit).ok();
        }
        scene.push_label(input);
    }

    #[inline(never)]
    fn append_info(
        &self,
        info: &crate::api::presentation::HintInfoView<'_>,
        rect: Rect,
        scene: &mut OverlayScene,
        ctx: &HostContext<'_>,
    ) {
        let scale = super::label_scale(ctx.scale());
        let (mut panel, base) = input_panel(info.ui, *self.window_bounds, ctx);
        let line = base.font_size * 1.8 * scale;
        let gap = base.font_size * 0.8 * scale;
        let padding = (base.padding_x.max(10.0) * scale, base.padding_y * scale);
        let text_gap = base.font_size * 0.5 * scale;
        let height = line * 4.0 + text_gap * 2.0 + gap + padding.1 * 2.0;
        panel.height = height;
        if info.ui.position_mode == crate::api::style::PanelPositionMode::SearchInput {
            let anchor = match &info.ui.position {
                crate::api::style::CompiledPanelPosition::Percentages(value) => {
                    crate::api::style::percentage_region(rect, *value).center()
                }
                crate::api::style::CompiledPanelPosition::Anchor(anchor) => {
                    anchor.place(rect, 0.0, 0.0, 0.0, 0.0).center()
                }
            };
            panel.x = anchor.x - panel.width / 2.0 + info.ui.x_offset * scale;
            panel.y = if anchor.y <= rect.center().y {
                anchor.y - height - info.ui.y_offset * scale
            } else {
                anchor.y + info.ui.y_offset * scale
            };
        } else {
            let area = if info.ui.position_mode == crate::api::style::PanelPositionMode::Window {
                self.window_bounds.unwrap_or(ctx.active_bounds())
            } else {
                ctx.active_bounds()
            };
            panel = info.ui.position.place(
                area,
                panel.width,
                height,
                info.ui.x_offset * scale,
                info.ui.y_offset * scale,
            );
        }
        let area = ctx.active_bounds();
        panel.x = panel
            .x
            .clamp(area.x, (area.right() - panel.width).max(area.x));
        panel.y = panel.y.clamp(area.y, (area.bottom() - height).max(area.y));
        let styles = info.ui.for_appearance(ctx.palette.appearance);
        // Detach each shared buffer once for the complete information panel.
        let labels: &mut Vec<OverlayLabel> = &mut scene.labels;
        // The search input is appended after this panel. Reserve it too.
        labels.reserve(info.field_count() * 3 + 1);
        let shapes: &mut Vec<OverlayShape> = &mut scene.shapes;
        shapes.push(OverlayShape::Rect {
            rect: panel,
            fill: base.background,
            stroke: base.border_color,
            stroke_width: base.border_width * scale,
            corner_radius: base.border_radius * scale,
            z_index: SEARCH_INPUT_Z_INDEX,
        });
        for (index, (title, value)) in info
            .titles
            .iter()
            .zip(info.preview_values().iter())
            .take(info.field_count())
            .enumerate()
        {
            let column_width = ((panel.width - padding.0 * 2.0 - gap) / 2.0).max(1.0);
            let x = panel.x
                + padding.0
                + if !(info.field_count() == 3 && index == 2) {
                    (index % 2) as f64 * (column_width + gap)
                } else {
                    0.0
                };
            let y = panel.y + padding.1 + (index / 2) as f64 * (line * 2.0 + text_gap + gap);
            let width = if !(info.field_count() == 3 && index == 2) {
                column_width
            } else {
                (panel.width - padding.0 * 2.0).max(1.0)
            };
            let number_width = base.font_size * 1.3 * scale;
            let header_gap = 6.0 * scale;
            if index == 3
                && let Some(point) = &info.point
                && info.ui.color_preview.enabled
                && let Some(color) = point.color
                && (!info.multiple
                    || info.field_modes[3] == crate::api::point_sample::FieldMode::Switch)
            {
                let preview = info.ui.color_preview;
                let chip_width = f64::from(preview.width) * scale;
                let chip_height = f64::from(preview.height) * scale;
                // The short proportional caption is narrower than the
                // conservative text budget used for truncation.
                let title_width = super::text_units(title) * base.font_size * (2.0 / 3.0) * scale;
                shapes.push(OverlayShape::Rect {
                    rect: Rect::new(
                        x + number_width
                            + header_gap
                            + title_width
                            + f64::from(preview.x_offset) * scale,
                        y + (line - chip_height) / 2.0 + f64::from(preview.y_offset) * scale,
                        chip_width,
                        chip_height,
                    ),
                    fill: color,
                    stroke: base.text_color,
                    stroke_width: f64::from(preview.border_width) * scale,
                    corner_radius: 0.0,
                    z_index: SEARCH_INPUT_Z_INDEX + 2,
                });
            }
            for (text, bounds, style) in [
                (
                    ["1", "2", "3", "4"][index],
                    Rect::new(x, y, number_width, line),
                    styles.key.clone(),
                ),
                (
                    title.as_str(),
                    Rect::new(
                        x + number_width + header_gap,
                        y,
                        (width - number_width - header_gap).max(1.0),
                        line,
                    ),
                    styles.caption.clone(),
                ),
            ] {
                labels.push(
                    OverlayLabel::new(text, bounds, style)
                        .with_fixed_bounds()
                        .with_z_index(SEARCH_INPUT_Z_INDEX + 1),
                );
            }
            {
                let text = super::single_line_elide_width(
                    if value.is_empty() { "—" } else { value },
                    width / (base.font_size * scale),
                );
                labels.push(
                    OverlayLabel::new(
                        text,
                        Rect::new(x, y + line + text_gap, width, line),
                        styles.caption.clone(),
                    )
                    .with_fixed_bounds()
                    .with_z_index(SEARCH_INPUT_Z_INDEX + 1),
                );
            }
        }
    }
}
impl HintSelectionView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let mut scene = OverlayScene::new();
        scene.clip = self.scan_bounds.or_else(|| Some(ctx.active_bounds()));
        let Some(target) = self.target else {
            return scene;
        };
        let boundary = self.boundary;
        scene.push_shape(OverlayShape::Rect {
            rect: target,
            fill: boundary.fill(ctx.palette),
            stroke: boundary.stroke(ctx.palette),
            stroke_width: boundary.border_width.max(1) as f64,
            corner_radius: boundary.radius(),
            z_index: 1,
        });
        scene
    }
}
impl StatusView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let style = self.ui.resolve(
            palette,
            palette.surface_label(),
            palette.text,
            palette.accent,
        );
        let text = self.text;
        let width = text.chars().count() as f64 * style.font_size * 0.65 + style.padding_x * 2.0;
        let height = style.font_size * 1.4 + style.padding_y * 2.0;
        let bounds = ctx.active_bounds();
        let rect = Rect::new(
            bounds.center().x - width / 2.0,
            bounds.center().y - height / 2.0,
            width,
            height,
        );
        let mut scene = OverlayScene::new();
        scene.clip = self.clip.or(Some(bounds));
        scene.push_label(OverlayLabel::new(text, rect, style).with_z_index(10));
        scene
    }
}
