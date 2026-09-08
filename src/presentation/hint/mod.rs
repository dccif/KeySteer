//! UI Hint typography, placement and cached overlap planning.
pub(crate) mod layers;
use crate::api::hint::CompactHint;
use crate::api::overlay::{
    Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape, SharedLabelStyle,
};
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

pub(crate) fn placed_hint_rect(
    config: &HintStyle<'_>,
    hint: &CompactHint<usize>,
    style: &LabelStyle,
) -> Rect {
    let width =
        style.font_size * 0.75 * hint.label.as_str().chars().count() as f64 + style.padding_x * 2.0;
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
    let visual_scale = visual_layer_scale(ctx, content.scan_bounds);
    let visual_padding_x = (style.padding_x * visual_scale).round();
    let visual_padding_y = (style.padding_y * visual_scale).round();
    let visible = content
        .hints
        .iter()
        .filter(|hint| hint.label.as_str().starts_with(content.prefix))
        .count();
    let stacked = |left, right| visually_stacked(left, right, visual_padding_x, visual_padding_y);
    if visible > 128 {
        let mut placements = workspace.take().unwrap_or_default();
        placements.clear();
        placements.extend(
            content
                .hints
                .iter()
                .enumerate()
                .filter(|(_, hint)| hint.label.as_str().starts_with(content.prefix))
                .map(|(index, hint)| {
                    let rect = placed_hint_rect(&content.style, hint, &style);
                    (index, visual_layer_rect(rect, visual_scale))
                }),
        );
        build_visual_layer_plan(&placements, content.hints.len(), stacked, layers);
        *workspace = Some(placements);
    } else {
        let placements: SmallVec<[(usize, Rect); 128]> = content
            .hints
            .iter()
            .enumerate()
            .filter(|(_, hint)| hint.label.as_str().starts_with(content.prefix))
            .map(|(index, hint)| {
                let rect = placed_hint_rect(&content.style, hint, &style);
                (index, visual_layer_rect(rect, visual_scale))
            })
            .collect();
        build_visual_layer_plan(&placements, content.hints.len(), stacked, layers);
    }
}
impl HintView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let visible_count = self
            .content
            .hints
            .iter()
            .filter(|hint| hint.label.as_str().starts_with(self.content.prefix))
            .count();
        let shape_capacity = if self.content.style.boundary_highlight.enabled {
            visible_count
        } else {
            0
        };
        let label_capacity = visible_count + usize::from(self.content.search.is_some());
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
        let label_style = SharedLabelStyle::from(label_style);

        // Optional outlines behind only the currently visible candidates.
        if self.content.style.boundary_highlight.enabled {
            let bh = &self.content.style.boundary_highlight;
            for hint in self
                .content
                .hints
                .iter()
                .filter(|hint| hint.label.as_str().starts_with(self.content.prefix))
            {
                scene.push_shape(OverlayShape::Rect {
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
        let matched_prefix_len = typed.chars().count();
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
        for z_index in HINT_LAYER_Z_BASE..=final_z {
            for (hint_index, hint) in self
                .content
                .hints
                .iter()
                .enumerate()
                .filter(|(_, hint)| hint.label.as_str().starts_with(self.content.prefix))
            {
                if z_for(hint_index) != z_index {
                    continue;
                }
                let rect = placed_hint_rect(&self.content.style, hint, &label_style);
                scene.push_label(
                    OverlayLabel::new(hint.label.as_str(), rect, label_style.clone())
                        .with_matched_prefix(matched_prefix_len)
                        .with_z_index(z_index),
                );
            }
        }

        // Search box, shown only while searching.
        if let Some(query) = self.content.search {
            let cfg = &self.content.style.search_input_ui;
            let style = cfg.label.resolve(
                palette,
                palette.surface_label(),
                palette.text,
                palette.accent,
            );
            let height = style.font_size * 1.8 + style.padding_y * 2.0;
            let rect = cfg.position.place(
                ctx.active_bounds(),
                cfg.width.max(1) as f64,
                height,
                cfg.x_offset as f64,
                cfg.y_offset as f64,
            );
            scene.push_label(
                OverlayLabel::new(format!("/{query}"), rect, style)
                    .with_z_index(SEARCH_INPUT_Z_INDEX),
            );
        }

        scene
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
