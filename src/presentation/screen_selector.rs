//! Central multi-display selection composition, also used by bundled plugins.
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape, Placement};
use crate::api::presentation::ScreenSelectorView;
use crate::api::{HostContext, Palette};
impl ScreenSelectorView<'_> {
    fn style(palette: &Palette) -> LabelStyle {
        LabelStyle {
            background: palette.surface_label(),
            text_color: palette.text,
            border_color: palette.accent,
            font_size: 72.0,
            padding_x: 30.0,
            padding_y: 20.0,
            border_radius: 14.0,
            bold: true,
            ..LabelStyle::default()
        }
    }

    pub(crate) fn scene(&self, ctx: &HostContext<'_>) -> OverlayScene {
        let palette = ctx.palette;
        let style = Self::style(palette);
        let mut scene = OverlayScene::new().with_backdrop(Color::rgba(0, 0, 0, 0x60));
        for (label, _, bounds) in self.cells {
            scene.push_shape(OverlayShape::Rect {
                rect: bounds.inset(8.0, 8.0),
                fill: palette.highlight(),
                stroke: palette.accent,
                stroke_width: 4.0,
                corner_radius: 8.0,
                z_index: 0,
            });
            let size = style.font_size * 2.2;
            scene.push_label(
                OverlayLabel::new(
                    label.clone(),
                    Placement::Center.place(bounds, size, size),
                    style.clone(),
                )
                .with_matched_prefix(if label.starts_with(self.input) {
                    self.input.chars().count()
                } else {
                    0
                })
                .with_z_index(1),
            );
        }
        scene
    }
}
