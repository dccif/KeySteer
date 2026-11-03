//! Text preview uses UIHint's input, panel positioning and compiled appearance.
use crate::api::overlay::{Color, OverlayLabel, OverlayText, SharedLabelStyle};
use crate::api::presentation::TextSelectView;
use crate::api::{HostContext, OverlayScene, OverlayShape, Point, Rect};
use unicode_segmentation::UnicodeSegmentation;

struct ChipStyles {
    background: Color,
    border_color: Color,
    border_width: f64,
    code: SharedLabelStyle,
    text: SharedLabelStyle,
}

impl TextSelectView<'_> {
    fn chips(&self) -> impl ExactSizeIterator<Item = (&str, &str, bool)> {
        self.chips.iter().map(|hint| {
            (
                hint.label.as_str(),
                &self.unit_text[self.units[hint.value].clone()],
                self.selected[hint.value],
            )
        })
    }

    pub(crate) fn scene(self, ctx: &HostContext<'_>) -> OverlayScene {
        let (input, _) = super::hint::input_panel(
            self.hints.content.style.search_input_ui,
            *self.hints.window_bounds,
            ctx,
        );
        let scale = super::label_scale(ctx.scale());
        let styles = self.ui.for_appearance(ctx.palette.appearance);
        let base = &styles.panel;
        let font = base.font_size * scale;
        let line = font * 1.4;
        let padding = (
            base.padding_x.max(10.0) * scale,
            // The compact text preview needs half the multi-field info panel's
            // vertical inset, while still following its configured spacing.
            base.padding_y * 0.5 * scale,
        );
        let preview = if self.text.is_empty() {
            self.status
        } else {
            self.text
        };
        let preliminary = self.ui.place(
            ctx.active_bounds(),
            *self.hints.window_bounds,
            Some(input),
            line,
            scale,
        );
        let width = (preliminary.width - padding.0 * 2.0).max(1.0);
        // Reserve a bounded preview area; only native typography decides where
        // the prose wraps. Estimated character widths must not insert line breaks.
        let preview_height = line * preview_lines(preview, width / font).take(4).count() as f64;
        let code_height = font;
        let word_height = font * 1.9;
        let chip_height = code_height + word_height + 8.0 * scale;
        let summary_height = if self.summary.is_empty() {
            0.0
        } else {
            line * 0.8
        };
        let pages_height = if self.page.1 > 1 && !self.chips.is_empty() {
            line + 6.0 * scale
        } else {
            0.0
        };
        let divider_height = if self.chips.is_empty() {
            0.0
        } else {
            16.0 * scale
        };
        let fixed_height =
            preview_height + summary_height + divider_height + pages_height + padding.1 * 2.0;
        let max_rows = ((ctx.active_bounds().height - input.height - fixed_height - 12.0 * scale)
            / chip_height)
            .floor()
            .max(1.0) as usize;
        let cells = chip_layout(
            self.chips(),
            width,
            font,
            scale,
            self.grid_min_width,
            chip_height,
            max_rows,
        );
        let grid_height = cells.last().map_or(0.0, |rect| rect.bottom());
        let height = preview_height
            + summary_height
            + divider_height
            + grid_height
            + pages_height
            + padding.1 * 2.0;
        let panel = self.ui.place(
            ctx.active_bounds(),
            *self.hints.window_bounds,
            Some(input),
            height,
            scale,
        );
        let highlight = self.hints.content.style.search_highlight;
        let mut scene = self.hints.scene(ctx);
        for (rect, _) in self
            .region_bounds
            .iter()
            .zip(self.selected)
            .filter(|(rect, selected)| **selected && !rect.is_empty())
        {
            let [pad_x, pad_y] = self
                .selection_ui
                .padding
                .map(|value| f64::from(value) * scale);
            let rect = Rect::new(
                rect.x - pad_x,
                rect.y - pad_y,
                rect.width + pad_x * 2.0,
                rect.height + pad_y * 2.0,
            );
            scene.push_shape(OverlayShape::Rect {
                rect,
                fill: Color::TRANSPARENT,
                stroke: self
                    .selection_ui
                    .border_color
                    .and_then(|color| color.resolve(ctx.palette.appearance))
                    .unwrap_or(ctx.palette.accent),
                stroke_width: self.selection_ui.border_width * scale,
                corner_radius: 2.0 * scale,
                z_index: 0,
            });
        }
        scene.push_shape(OverlayShape::Rect {
            rect: panel,
            fill: base.background,
            stroke: base.border_color,
            stroke_width: base.border_width * scale,
            corner_radius: base.border_radius * scale,
            z_index: 10_000,
        });
        let x = panel.x + padding.0;
        let mut y = panel.y + padding.1;
        let text_style = self.ui.input_style(ctx.palette.appearance).clone();
        let mut preview_style = (*text_style).clone();
        preview_style.wrap = true;
        // Keep shaping bounded even when the clipboard selection is very large.
        let preview = bounded_preview(preview);
        scene.push_label(
            OverlayLabel::new(
                preview,
                Rect::new(x, y, width, preview_height),
                preview_style,
            )
            .with_fixed_bounds()
            .with_z_index(10_002),
        );
        y += preview_height;
        if !self.summary.is_empty() {
            let mut style = (*text_style).clone();
            style.font_size *= 0.8;
            scene.push_label(
                OverlayLabel::new(self.summary, Rect::new(x, y, width, summary_height), style)
                    .with_fixed_bounds()
                    .with_z_index(10_002),
            );
            y += summary_height;
        }
        if divider_height > 0.0 {
            scene.push_shape(OverlayShape::Line {
                from: Point::new(x, y + divider_height * 0.5),
                to: Point::new(x + width, y + divider_height * 0.5),
                color: base.border_color.with_alpha(100),
                width: scale,
                z_index: 10_001,
                placement_group: None,
            });
            y += divider_height;
        }
        // Each word takes its own width; one long word never widens the whole grid.
        // Share styles only within this scene, creating each checked state on demand.
        let mut chip_styles = [None, None];
        for ((code, text, selected), cell) in self.chips().zip(cells) {
            let rect = Rect::new(
                x + cell.x,
                y + cell.y,
                cell.width - 4.0 * scale,
                chip_height - 4.0 * scale,
            );
            let styles = chip_styles[usize::from(selected)].get_or_insert_with(|| {
                let mut style = (*text_style).clone();
                style.background = base.background;
                if selected {
                    highlight.apply(&mut style, ctx.palette);
                }
                let background = style.background;
                let border_color = style.border_color;
                let border_width = style.border_width.max(1.0) * scale;
                style.padding_x = 0.0;
                style.padding_y = 0.0;
                style.background = Color::TRANSPARENT;
                style.border_width = 0.0;
                let mut code = style.clone();
                code.font_size *= 0.7;
                style.font_size *= 1.35;
                ChipStyles {
                    background,
                    border_color,
                    border_width,
                    code: code.into(),
                    text: style.into(),
                }
            });
            scene.push_shape(OverlayShape::Rect {
                rect,
                fill: styles.background,
                stroke: styles.border_color,
                stroke_width: styles.border_width,
                corner_radius: 3.0 * scale,
                z_index: 10_001,
            });
            let mut label = OverlayText::from(if selected { "✓ " } else { "" });
            for ch in code.chars().flat_map(char::to_uppercase) {
                label.push(ch);
            }
            scene.push_label(
                OverlayLabel::new(
                    label,
                    Rect::new(
                        rect.x + 4.0 * scale,
                        rect.y + 2.0 * scale,
                        rect.width - 8.0 * scale,
                        code_height,
                    ),
                    styles.code.clone(),
                )
                .with_fixed_bounds()
                .with_z_index(10_002),
            );
            let (first, more) = preview_lines(text, (rect.width - 8.0 * scale) / (font * 1.35))
                .next()
                .unwrap_or_default();
            let mut text = OverlayText::from(first);
            if more {
                text.push('…');
            }
            scene.push_label(
                OverlayLabel::new(
                    text,
                    Rect::new(
                        rect.x + 4.0 * scale,
                        rect.y + 2.0 * scale + code_height,
                        rect.width - 8.0 * scale,
                        word_height,
                    ),
                    styles.text.clone(),
                )
                .with_fixed_bounds()
                .with_z_index(10_002),
            );
        }
        if pages_height > 0.0 {
            scene.push_label(
                OverlayLabel::new(
                    if self.page_keys.is_empty() {
                        format!("Page {} / {}", self.page.0, self.page.1)
                    } else {
                        format!(
                            "Page {} / {}  ·  {}: next page",
                            self.page.0, self.page.1, self.page_keys
                        )
                    },
                    Rect::new(x, y + grid_height + 6.0 * scale, width, line),
                    text_style,
                )
                .with_fixed_bounds()
                .with_z_index(10_002),
            );
        }
        scene
    }
}

fn bounded_preview(text: &str) -> &str {
    // Bytes bound the grapheme count, so short text needs no segmentation.
    if text.len() <= 4096 {
        return text;
    }
    // Include the following byte to prove a boundary: a combining mark must
    // not be cut from an ASCII base. CR may instead form a CRLF cluster.
    let prefix = &text.as_bytes()[..=4096];
    if prefix.is_ascii() && !prefix[..4096].contains(&b'\r') {
        return &text[..4096];
    }
    text.grapheme_indices(true)
        .nth(4096)
        .map_or(text, |(end, _)| &text[..end])
}

/// Ordered row packing, bounded to the screen's available height. On short
/// screens only oversized cells are capped; their text is elided when painted.
fn chip_layout<'a>(
    chips: impl ExactSizeIterator<Item = (&'a str, &'a str, bool)>,
    width: f64,
    font: f64,
    scale: f64,
    min_width: f64,
    height: f64,
    max_rows: usize,
) -> Vec<Rect> {
    let advance = |text: &str| {
        text.graphemes(true)
            .map(|g| if g.is_ascii() { 0.75 } else { 1.0 })
            .sum::<f64>()
    };
    let count = chips.len();
    let mut cells: Vec<_> = chips
        .map(|(code, text, _)| {
            // Reserve the checkmark even for unchecked cells, so toggling is stable.
            let natural = ((advance(text) * font * 1.35).max((advance(code) + 1.5) * font * 0.7)
                + 16.0 * scale)
                .max(min_width * scale)
                .min(width);
            Rect::new(0.0, 0.0, natural, height)
        })
        .collect();
    let pack = |cells: &mut [Rect], cap: f64| {
        let mut x = 0.0;
        let mut y = 0.0;
        for cell in cells {
            cell.width = cell.width.min(cap);
            if x > 0.0 && x + cell.width > width + 0.001 {
                x = 0.0;
                y += height;
            }
            cell.x = x;
            cell.y = y;
            x += cell.width;
        }
    };
    pack(&mut cells, width);
    if cells
        .last()
        .is_some_and(|rect| rect.bottom() > height * max_rows as f64 + 0.001)
    {
        pack(
            &mut cells,
            width / count.div_ceil(max_rows.max(1)).max(1) as f64,
        );
    }
    cells
}

/// Borrow estimated lines without allocating text just to count them. The bool
/// marks a following line, including an empty paragraph after a trailing break.
fn preview_lines(text: &str, capacity: f64) -> impl Iterator<Item = (&str, bool)> {
    let mut remaining = Some(text);
    std::iter::from_fn(move || {
        let text = remaining.take()?;
        let mut width = 0.0;
        for (index, g) in text.grapheme_indices(true) {
            let newline = g.contains('\n') || g == "\r";
            let advance = if g == "\t" {
                3.0
            } else if g.is_ascii() {
                0.75
            } else {
                1.0
            };
            if newline || width + advance > capacity && index != 0 {
                remaining = Some(&text[index + if newline { g.len() } else { 0 }..]);
                return Some((&text[..index], true));
            }
            width += advance;
        }
        Some((text, false))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_cards_pack_individually_and_checks_do_not_move_them() {
        let mut chips: Vec<_> = [
            "a",
            "to",
            "of",
            "in",
            "on",
            "is",
            "longerword",
            "a",
            "中",
            "e\u{301}",
        ]
        .into_iter()
        .map(|text| ("a", text, true))
        .collect();
        let layout = chip_layout(chips.iter().copied(), 480., 14., 1., 48., 58., 8);
        assert!(layout.iter().take_while(|cell| cell.y == 0.).count() > 3);
        assert!(layout[6].width > layout[0].width * 2.);
        for cell in &layout {
            assert!(cell.x >= 0. && cell.right() <= 480.001);
        }
        chips[0].2 = false;
        assert_eq!(
            layout,
            chip_layout(chips.iter().copied(), 480., 14., 1., 48., 58., 8)
        );
    }

    #[test]
    fn character_density_scales_and_short_screens_keep_every_card() {
        let chips: Vec<_> = (0..24).map(|_| ("a", "中", true)).collect();
        let columns = |layout: &[Rect]| layout.iter().take_while(|cell| cell.y == 0.).count();
        let compact = chip_layout(chips.iter().copied(), 480., 14., 1., 40., 58., 8);
        assert_eq!(columns(&compact), 12);
        let roomy = chip_layout(chips.iter().copied(), 480., 14., 1., 80., 58., 8);
        assert_eq!(columns(&roomy), 6);
        let scaled = chip_layout(chips.iter().copied(), 960., 28., 2., 40., 116., 8);
        assert_eq!(columns(&scaled), columns(&compact));
        let short = chip_layout(chips.iter().copied(), 480., 14., 1., 240., 58., 2);
        assert_eq!(short.len(), 24);
        assert!(
            short
                .iter()
                .all(|cell| cell.right() <= 480.001 && cell.bottom() <= 116.001)
        );
    }
    #[test]
    fn preview_limit_keeps_combining_marks_and_crlf_at_the_cut() {
        for text in [
            String::new(),
            "a".repeat(4096),
            "a".repeat(4097),
            "中".repeat(2000),
            "e\u{301}".repeat(4097),
            format!("{}\r\nz", "a".repeat(4095)),
            format!("{}\rX", "a".repeat(4095)),
            format!("{}\u{301}z", "a".repeat(4096)),
            format!("{}b\u{301}", "a".repeat(4096)),
            format!("{}👨‍👩‍👧‍👦z", "a".repeat(4095)),
        ] {
            let end = text
                .grapheme_indices(true)
                .nth(4096)
                .map_or(text.len(), |(end, _)| end);
            assert_eq!(bounded_preview(&text), &text[..end]);
        }
    }

    #[test]
    fn preview_keeps_paragraph_breaks_and_does_not_split_graphemes() {
        assert_eq!(
            preview_lines("first\n\nsecond", 50.).collect::<Vec<_>>(),
            [("first", true), ("", true), ("second", false)]
        );
        assert_eq!(
            preview_lines("e\u{301}👨‍👩‍👧‍👦a", 1.).take(2).collect::<Vec<_>>(),
            [("e\u{301}", true), ("👨‍👩‍👧‍👦", true)]
        );
        assert_eq!(preview_lines("", 0.).collect::<Vec<_>>(), [("", false)]);
        assert_eq!(
            preview_lines("a\r\n", 1.).collect::<Vec<_>>(),
            [("a", true), ("", false)]
        );
        assert_eq!(
            preview_lines("\t中", 0.).collect::<Vec<_>>(),
            [("\t", true), ("中", false)]
        );
    }
}
