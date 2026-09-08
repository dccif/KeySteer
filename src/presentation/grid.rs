//! Central grid composition from a borrowed semantic view.
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, OverlayScene, OverlayShape};
use crate::api::presentation::GridView;
use crate::api::{Palette, Point, Rect};

impl GridView<'_> {
    pub(crate) fn style(&self, palette: &Palette) -> LabelStyle {
        self.ui.label.resolve(
            palette,
            palette.surface_cell(),
            palette.text,
            palette.accent_border(),
        )
    }

    fn previews_second_layer(&self) -> bool {
        !self.terminal && self.depth == 0 && self.max_depth > 1
    }

    fn push_rulings(
        scene: &mut OverlayScene,
        area: Rect,
        rows: usize,
        cols: usize,
        color: Color,
        width: f64,
        z_index: i32,
    ) {
        if width <= 0.0 || color.is_transparent() {
            return;
        }
        let cell_width = area.width / cols as f64;
        let cell_height = area.height / rows as f64;
        for column in 1..cols {
            let x = area.x + column as f64 * cell_width;
            scene.push_shape(OverlayShape::Line {
                from: Point::new(x, area.top()),
                to: Point::new(x, area.bottom()),
                color,
                width,
                z_index,
            });
        }
        for row in 1..rows {
            let y = area.y + row as f64 * cell_height;
            scene.push_shape(OverlayShape::Line {
                from: Point::new(area.left(), y),
                to: Point::new(area.right(), y),
                color,
                width,
                z_index,
            });
        }
    }

    fn push_second_layer_preview(
        &self,
        scene: &mut OverlayScene,
        area: Rect,
        style: &LabelStyle,
        palette: &Palette,
    ) {
        // Because every depth uses the same layout, the nested rulings align
        // into one fine grid. Draw only the fine lines that are not already an
        // outer boundary, avoiding per-cell duplicate edges and alpha blending.
        let preview_rows = self.layout.rows.saturating_mul(self.layout.rows);
        let preview_cols = self.layout.cols.saturating_mul(self.layout.cols);
        let line_color = crate::api::style::resolve(
            self.ui.matched_border_color.as_ref(),
            palette.appearance,
            style.border_color,
        )
        .with_opacity(0.35);
        let line_width = style.border_width * 0.7;
        if line_width > 0.0 && !line_color.is_transparent() {
            let cell_width = area.width / preview_cols as f64;
            let cell_height = area.height / preview_rows as f64;
            for column in 1..preview_cols {
                if column % self.layout.cols == 0 {
                    continue;
                }
                let x = area.x + column as f64 * cell_width;
                scene.push_shape(OverlayShape::Line {
                    from: Point::new(x, area.top()),
                    to: Point::new(x, area.bottom()),
                    color: line_color,
                    width: line_width,
                    z_index: 1,
                });
            }
            for row in 1..preview_rows {
                if row % self.layout.rows == 0 {
                    continue;
                }
                let y = area.y + row as f64 * cell_height;
                scene.push_shape(OverlayShape::Line {
                    from: Point::new(area.left(), y),
                    to: Point::new(area.right(), y),
                    color: line_color,
                    width: line_width,
                    z_index: 1,
                });
            }
        }

        let preview_color = style.text_color.with_opacity(0.62);
        let preview_style = LabelStyle {
            background: Color::TRANSPARENT,
            text_color: preview_color,
            matched_text_color: preview_color,
            border_color: Color::TRANSPARENT,
            border_width: 0.0,
            border_radius: 0.0,
            bold: false,
            ..style.clone()
        };
        for outer_index in 0..self.layout.keys.len() {
            let Some(outer) = area.subdivision(self.layout.rows, self.layout.cols, outer_index)
            else {
                break;
            };
            for (inner_index, suffix) in self.layout.keys.iter().copied().enumerate() {
                let Some(rect) = outer.subdivision(self.layout.rows, self.layout.cols, inner_index)
                else {
                    break;
                };
                let text = suffix.to_string();
                let label_style = preview_style.fit_to(&text, rect);
                scene.push_label(
                    OverlayLabel::new(text, rect, label_style)
                        .fitted()
                        .with_z_index(2),
                );
            }
        }

        // The large prefix sits above the faint suffix grid, matching the
        // visual hierarchy of Mousemaster's nested decorations. Its size is
        // relative to the outer cell rather than capped by the ordinary label
        // font, so the first key remains readable at a glance on any display.
        let primary_color = style.matched_text_color.with_opacity(0.92);
        for (outer_index, prefix) in self.layout.keys.iter().copied().enumerate() {
            let Some(outer) = area.subdivision(self.layout.rows, self.layout.cols, outer_index)
            else {
                break;
            };
            let text = prefix.to_string();
            let primary_style = LabelStyle {
                font_size: style.font_size.max(outer.width.min(outer.height) * 0.62),
                text_color: primary_color,
                matched_text_color: primary_color,
                background: Color::TRANSPARENT,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                border_radius: 0.0,
                padding_x: 0.0,
                padding_y: 0.0,
                bold: false,
                ..style.clone()
            }
            .fit_to(&text, outer);
            scene.push_label(
                OverlayLabel::new(text, outer, primary_style)
                    .fitted()
                    .with_z_index(3),
            );
        }
    }

    pub(crate) fn scene(&self, palette: &Palette) -> OverlayScene {
        let preview_second_layer = self.previews_second_layer();
        let outer_rulings = self.layout.rows.saturating_sub(1) + self.layout.cols.saturating_sub(1);
        let preview_rulings = if preview_second_layer {
            self.layout
                .rows
                .saturating_mul(self.layout.rows.saturating_sub(1))
                + self
                    .layout
                    .cols
                    .saturating_mul(self.layout.cols.saturating_sub(1))
        } else {
            0
        };
        let shape_capacity = if self.terminal {
            1
        } else {
            1 + outer_rulings + preview_rulings
        };
        let label_capacity = if preview_second_layer {
            self.layout
                .keys
                .len()
                .saturating_mul(self.layout.keys.len())
                .saturating_add(self.layout.keys.len())
        } else {
            usize::from(!self.terminal) * self.layout.keys.len()
        };
        let mut scene = OverlayScene::with_capacity(shape_capacity, label_capacity);
        let style = self.style(palette);

        if self.terminal {
            if let Some(rect) = self.current {
                scene.push_shape(OverlayShape::Rect {
                    rect,
                    fill: palette.highlight().with_opacity(0.55),
                    stroke: palette.accent_border(),
                    stroke_width: style.border_width.max(1.0),
                    corner_radius: 0.0,
                    z_index: 1,
                });
            }
        } else {
            let Some(area) = self.current else {
                return scene;
            };

            // Paint the common cell background once and each ruling once.
            // Drawing a filled/stroked rectangle per cell makes the software
            // Windows backend traverse the whole overlay multiple times and
            // alpha-blend shared edges twice. A grid is the same geometry as
            // one background rectangle plus O(rows + columns) straight lines.
            scene.push_shape(OverlayShape::Rect {
                rect: area,
                fill: style.background.with_opacity(0.55),
                stroke: style.border_color,
                stroke_width: style.border_width,
                corner_radius: 0.0,
                z_index: 0,
            });

            Self::push_rulings(
                &mut scene,
                area,
                self.layout.rows,
                self.layout.cols,
                style.border_color,
                style.border_width,
                1,
            );

            let text_style = LabelStyle {
                background: Color::TRANSPARENT,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
                border_radius: 0.0,
                ..style.clone()
            };
            if preview_second_layer {
                self.push_second_layer_preview(&mut scene, area, &style, palette);
            } else {
                for (index, key) in self.layout.keys.iter().copied().enumerate() {
                    let Some(rect) = area.subdivision(self.layout.rows, self.layout.cols, index)
                    else {
                        break;
                    };
                    let text = key.to_string();
                    let label_style = text_style.fit_to(&text, rect);
                    scene.push_label(
                        OverlayLabel::new(text, rect, label_style)
                            .fitted()
                            .with_z_index(2),
                    );
                }
            }
        }

        scene.backdrop = Some(Color::rgba(0, 0, 0, 0x40));
        scene.clip = self.root;
        scene
    }
}
