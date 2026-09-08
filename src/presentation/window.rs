//! Central window number, selection and BSP region composition.
use crate::api::overlay::{OverlayLabel, SharedLabelStyle};
use crate::api::presentation::WindowView;
use crate::api::window_layout::placed_rect;
use crate::api::{HostContext, OverlayScene, OverlayShape, Rect};
impl WindowView<'_> {
    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let mut scene = OverlayScene::new();
        let style: SharedLabelStyle = self
            .ui
            .resolve(
                ctx.palette,
                ctx.palette.surface_label(),
                ctx.palette.text,
                ctx.palette.accent,
            )
            .into();
        if let Some(target) = &self.target {
            scene.push_shape(OverlayShape::outline(
                target.bounds,
                style.border_color,
                self.border_width,
            ));
        }
        let Some(screen) = ctx.screens.get(self.screen) else {
            return scene;
        };
        let scale = crate::presentation::label_scale(screen.scale);
        let mut occupied: Vec<Rect> = Vec::new();
        for window in self.visible.iter().filter_map(|id| self.inventory.get(id)) {
            let Some(number) = self.numbers.get(&window.id) else {
                continue;
            };
            let text = number.to_string();
            let width =
                (style.font_size * 0.75 * text.len() as f64 + style.padding_x * 2.0).max(28.0);
            let height = (style.font_size * 1.4 + style.padding_y * 2.0).max(24.0);
            let point = window.bounds.center();
            let mut rect = Rect::new(point.x - width / 2.0, point.y - height / 2.0, width, height);
            for _ in 0..occupied.len() {
                let expanded = Rect::new(
                    rect.center().x - width * scale / 2.0,
                    rect.center().y - height * scale / 2.0,
                    width * scale,
                    height * scale,
                );
                if !occupied
                    .iter()
                    .any(|old| old.intersect(&expanded).is_some())
                {
                    break;
                }
                rect.y += (height + 4.0) * scale;
            }
            rect.x = rect.x.clamp(
                screen.work_area.x,
                (screen.work_area.right() - width).max(screen.work_area.x),
            );
            rect.y = rect.y.clamp(
                screen.work_area.y,
                (screen.work_area.bottom() - height).max(screen.work_area.y),
            );
            occupied.push(Rect::new(
                rect.center().x - width * scale / 2.0,
                rect.center().y - height * scale / 2.0,
                width * scale,
                height * scale,
            ));
            scene.push_label(OverlayLabel::new(text, rect, style.clone()).with_z_index(20));
        }
        if let Some(tree) = self.tree {
            for slot in tree.slots() {
                let rect = placed_rect(screen.work_area, slot.rect, self.gap);
                scene.push_shape(OverlayShape::outline(
                    rect,
                    style
                        .border_color
                        .with_opacity(if slot.id == tree.selected { 1.0 } else { 0.4 }),
                    if slot.id == tree.selected { 3.0 } else { 1.0 },
                ));
                scene.push_label(
                    OverlayLabel::new(
                        format!("`{}", slot.id),
                        Rect::new(rect.x + 10.0 * scale, rect.y + 8.0 * scale, 44.0, 26.0),
                        style.clone(),
                    )
                    .with_z_index(21),
                );
            }
        }
        scene
    }
}
