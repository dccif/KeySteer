//! Key-help panel composition; effective routing and cache ownership stay in the host.
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, TextAlignment};
use crate::api::style::KeyHelp;
use crate::api::{OverlayScene, Palette, Rect, Screen};
use std::collections::BTreeMap;

pub(crate) struct KeyHelpView<'a> {
    pub screen: &'a Screen,
    pub ui: &'a KeyHelp,
    pub palette: &'a Palette,
    pub entries: Vec<String>,
    pub window_help: bool,
    pub display_name: String,
    pub previews: Vec<(String, String, Rect, bool)>,
    pub detail: Option<String>,
    pub anchor: Option<Rect>,
    pub indicator_style: Option<LabelStyle>,
}

pub(crate) fn compose(scene: &mut OverlayScene, input: KeyHelpView<'_>) {
    let screen = input.screen;
    let palette = input.palette;
    let window_help = input.window_help;
    scene.clip = Some(
        scene
            .clip
            .map_or(screen.bounds, |clip| clip.union(&screen.bounds)),
    );
    // Group equivalent actions instead of repeating a badge for every key.
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for entry in input.entries {
        if let Some((keys, action)) = entry.split_once("  ·  ") {
            let action = if window_help {
                if let Some(label) = window_action_label(action) {
                    label
                } else {
                    if action == "Window number" {
                        continue;
                    }
                    action
                }
            } else {
                action
            };
            grouped
                .entry(action.into())
                .or_default()
                .push(keys.replace('_', " ").to_uppercase());
        }
    }
    let mut entries: Vec<_> = grouped
        .into_iter()
        .map(|(action, keys)| (keys.join(" / "), action))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    #[cfg(target_os = "windows")]
    let scale = crate::api::overlay::normalized_label_scale(screen.scale);
    #[cfg(not(target_os = "windows"))]
    let scale: f64 = 1.0;
    let push_panel_text =
        |scene: &mut OverlayScene, text: String, cell: Rect, style: &LabelStyle, scale: f64| {
            if window_help {
                push_sized_help_text(scene, text, cell, style, scale);
            } else {
                push_help_text(scene, text, cell, style, scale);
            }
        };
    let ui = input.ui;
    let color = |value: &Option<crate::api::theme::ThemedColor>, fallback| {
        crate::api::style::resolve(value.as_ref(), palette.appearance, fallback)
    };
    // An anchor affects placement only. Font size must not depend on target size.
    let area = screen.work_area;
    let available_width = (area.width - 12.0 * 2.0 * scale)
        .max(1.0)
        .min(if window_help { 760.0 * scale } else { f64::MAX });
    let padding = (ui.padding_x * scale).min(available_width / 4.0);
    let vertical_padding = ui.padding_y * scale;
    let column_gap = 6.0 * scale;
    let key_gap = 8.0 * scale;
    let mut columns = if available_width >= (if window_help { 340.0 } else { 560.0 }) * scale
        && entries.len() > 8
    {
        2_usize.min(entries.len()).max(1)
    } else {
        1
    };
    if window_help && columns == 2 {
        let rows = entries.len().div_ceil(columns);
        let needed = entries
            .chunks(rows)
            .map(|chunk| {
                let keys = chunk
                    .iter()
                    .map(|(key, _)| key.chars().count())
                    .max()
                    .unwrap_or(1) as f64;
                let actions = chunk
                    .iter()
                    .map(|(_, action)| super::text_units(action))
                    .fold(1.0, f64::max);
                (keys * ui.font_size * 0.75 + 10.0 + actions * ui.font_size + ui.font_size / 3.0)
                    * scale
                    + key_gap
            })
            .sum::<f64>()
            + column_gap
            + padding * 2.0;
        if needed > available_width {
            columns = 1;
        }
    }
    let rows = entries.len().div_ceil(columns).max(1);
    // Size each column from its own longest key and action, rather than
    // reserving half of a fixed-width panel for each side of every row.
    let mut widths = Vec::with_capacity(columns);
    for column in 0..columns {
        let chunk = entries.iter().skip(column * rows).take(rows);
        let (key_chars, action_chars) = chunk.fold((1, 1), |(keys, actions), (key, action)| {
            (
                keys.max(key.chars().count()),
                actions.max(action.chars().count()),
            )
        });
        widths.push((
            (key_chars as f64 * ui.font_size * 0.75 + 5.0 * 2.0) * scale,
            (action_chars as f64 * ui.font_size * 0.75 + ui.font_size / 3.0) * scale,
        ));
    }
    let content_width: f64 = widths
        .iter()
        .map(|(keys, actions)| keys + key_gap + actions)
        .sum::<f64>()
        + (columns - 1) as f64 * column_gap;
    let width = (content_width + padding * 2.0)
        .max(
            if window_help
                && input
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.starts_with("Restore\n"))
            {
                560.0 * scale
            } else {
                340.0 * scale
            },
        )
        .min(available_width);
    let squeeze =
        ((width - padding * 2.0 - (columns - 1) as f64 * column_gap - columns as f64 * key_gap)
            / widths
                .iter()
                .map(|(keys, actions)| keys + actions)
                .sum::<f64>())
        .clamp(0.01, 1.0);
    for (keys, actions) in &mut widths {
        if window_help {
            *actions = (*actions - (*keys + *actions) * (1.0 - squeeze)).max(ui.font_size * scale);
        } else {
            *keys *= squeeze;
            *actions *= squeeze;
        }
    }
    let action_lines: Vec<Vec<String>> = entries
        .iter()
        .enumerate()
        .map(|(index, (_, action))| {
            if window_help {
                wrap_action(action, widths[index / rows].1 / (ui.font_size * scale))
            } else {
                vec![action.clone()]
            }
        })
        .collect();
    let previews = input.previews;
    let preview_rows = previews.len().div_ceil(3);
    let preview_height = (preview_rows as f64 * 56.0 * scale).min(area.height * 0.35);
    let detail = input.detail;
    let (detail_title, detail_status) = detail
        .as_deref()
        .unwrap_or("")
        .split_once('\n')
        .unwrap_or((detail.as_deref().unwrap_or(""), ""));
    let title_font = ui.font_size * if window_help { 1.75 } else { 1.25 };
    let title_height = (ui.font_size * 7.0 / 3.0).max(title_font * 1.4) * scale;
    let status_lines = detail_status.lines().count();
    let status_height = ui.font_size * 1.8 * scale;
    let header_height = title_height + status_lines as f64 * status_height;
    let row_height = if window_help {
        ui.font_size
            * (1.4 * action_lines.iter().map(Vec::len).max().unwrap_or(1) as f64 + 0.6)
            * scale
    } else {
        ((ui.font_size * 2.0) * scale).min(
            ((area.height * (if window_help { 1.0 } else { 0.75 })
                - 16.0 * scale * f64::from(window_help)
                - header_height
                - preview_height
                - vertical_padding * 2.0)
                / rows as f64)
                .max(8.0),
        )
    };
    let height = header_height + preview_height + rows as f64 * row_height + vertical_padding * 2.0;
    let mut panel = Rect::new(
        area.x + (area.width - width) / 2.0,
        area.bottom() - 12.0 * scale - height,
        width,
        height,
    );
    if window_help
        && let Some(anchor) = input.anchor.filter(|anchor| {
            anchor.width >= width + 16.0 * scale && anchor.height >= height + 16.0 * scale
        })
    {
        panel.x =
            (anchor.center().x - width / 2.0).clamp(area.x, (area.right() - width).max(area.x));
        // Keep the panel inside the target whenever it fits; tiny or off-screen
        // targets fall back to the visible work area rather than hiding controls.
        panel.y = (anchor.bottom() - height - 8.0 * scale).clamp(
            screen.work_area.y,
            (screen.work_area.bottom() - height).max(screen.work_area.y),
        );
    }
    let indicator_style = input.indicator_style;
    let derived_background = indicator_style
        .as_ref()
        .map_or(palette.accent, |style| style.background);
    let derived_foreground = indicator_style.as_ref().map_or_else(
        || palette.readable_on(derived_background),
        |style| style.text_color,
    );
    let background = color(&ui.background_color, derived_background);
    let foreground = color(&ui.text_color, derived_foreground);
    let base = LabelStyle {
        text_alignment: TextAlignment::Left,
        background: crate::api::overlay::Color::TRANSPARENT,
        text_color: foreground,
        border_color: crate::api::overlay::Color::TRANSPARENT,
        border_width: 0.0,
        padding_x: 0.0,
        padding_y: 0.0,
        font_family: if ui.font_family.is_empty() {
            indicator_style
                .as_ref()
                .map_or_else(String::new, |style| style.font_family.clone())
        } else {
            ui.font_family.clone()
        },
        font_size: ui.font_size,
        bold: false,
        ..LabelStyle::default()
    };
    // A blank label supplies one opaque rounded panel in the label layer,
    // above Grid/Hint text as well as shapes. Its large fixed rect avoids
    // compact-label DPI expansion; all following text is transparent.
    scene.labels.push(
        OverlayLabel::new(
            "",
            panel,
            LabelStyle {
                background,
                border_radius: ui.border_radius,
                border_width: ui.border_width,
                border_color: color(&ui.border_color, Color::TRANSPARENT),
                font_size: 1.0,
                ..base.clone()
            },
        )
        .with_z_index(i32::MAX - 1),
    );
    let mut title = if window_help {
        "Window".into()
    } else {
        format!("{} · Available keys", input.display_name)
    };
    let close_keys: Vec<_> = entries
        .iter()
        .filter(|(_, action)| action == "key_help")
        .map(|(keys, _)| keys.as_str())
        .collect();
    let close_hint = if close_keys.is_empty() {
        String::new()
    } else {
        format!("{}  Close", close_keys.join(" / "))
    };
    let close_width = (close_hint.chars().count() as f64 * (ui.font_size * 1.25) * 0.75 * scale)
        .min(width * 0.45);
    if window_help {
        let capacity = ((width - padding * 2.0 - close_width - key_gap)
            / (title_font * 0.75 * scale))
            .max(1.0) as usize;
        title = super::elide(&title, capacity);
    }
    push_panel_text(
        scene,
        title,
        Rect::new(
            panel.x + padding,
            panel.y + vertical_padding,
            (width - padding * 2.0 - close_width - key_gap).max(1.0),
            title_height,
        ),
        &LabelStyle {
            font_size: title_font,
            text_color: foreground,
            bold: true,
            ..base.clone()
        },
        scale,
    );
    if window_help {
        let left = panel.x + padding + title_font * 5.0 * scale;
        let badge_width = (detail_title.chars().count() as f64 * ui.font_size * 0.75 + 16.0)
            .min((panel.right() - padding - left) / scale)
            .max(1.0)
            * scale;
        push_panel_text(
            scene,
            detail_title.into(),
            Rect::new(left, panel.y + vertical_padding, badge_width, title_height),
            &LabelStyle {
                font_size: ui.font_size,
                bold: true,
                text_alignment: TextAlignment::Center,
                background: Color::rgb(235, 237, 242),
                text_color: Color::rgb(30, 34, 43),
                border_color: Color::rgb(196, 201, 211),
                border_width: 1.0,
                border_radius: 3.0,
                padding_x: 6.0,
                padding_y: 2.0,
                ..base.clone()
            },
            scale,
        );
    }
    for (index, status) in detail_status.lines().enumerate() {
        push_panel_text(
            scene,
            super::elide_width(status, (width - padding * 2.0) / (ui.font_size * scale)),
            Rect::new(
                panel.x + padding,
                panel.y + vertical_padding + title_height + index as f64 * status_height,
                width - padding * 2.0,
                status_height,
            ),
            &LabelStyle {
                bold: status.starts_with("Input:") || (window_help && index == 0),
                ..base.clone()
            },
            scale,
        );
    }
    push_panel_text(
        scene,
        close_hint,
        Rect::new(
            panel.right() - padding - close_width,
            panel.y + vertical_padding,
            close_width,
            header_height,
        ),
        &LabelStyle {
            font_size: (ui.font_size * 1.25),
            text_color: foreground,
            bold: true,
            ..base.clone()
        },
        scale,
    );
    // One font scale for the entire body, including both keycaps and
    // descriptions. Never shrink individual rows according to text length.
    let body_scale = if window_help {
        1.0
    } else {
        squeeze
            .min((row_height / scale - 2.0) / (ui.font_size * 1.4 + 1.0 * 2.0))
            .max(0.01)
    };
    let key_style = LabelStyle {
        font_size: ui.font_size,
        text_alignment: TextAlignment::Center,
        background: Color::rgb(235, 237, 242),
        text_color: Color::rgb(30, 34, 43),
        border_color: Color::rgb(196, 201, 211),
        border_width: 1.0,
        border_radius: 3.0,
        padding_x: 5.0,
        padding_y: 1.0,
        bold: true,
        ..base.clone()
    }
    .scaled(body_scale);
    let action_style = LabelStyle {
        font_size: ui.font_size,
        text_color: foreground.with_opacity(0.85),
        ..base.clone()
    }
    .scaled(body_scale);
    let preview_area = Rect::new(
        panel.x + padding,
        panel.y + vertical_padding + header_height,
        width - padding * 2.0,
        preview_height,
    );
    for (index, (key, name, relative, selected)) in previews.into_iter().enumerate() {
        let Some(cell) = preview_area.subdivision(3, preview_rows, index) else {
            continue;
        };
        let cell = cell.inset(3.0 * scale, 3.0 * scale);
        let mini = Rect::new(
            cell.x + 4.0 * scale,
            cell.y + 3.0 * scale,
            (cell.width - 8.0 * scale).max(1.0),
            (cell.height - 22.0 * scale).max(1.0),
        );
        let frame_style = LabelStyle {
            font_size: 1.0,
            border_width: if selected { 2.0 } else { 1.0 },
            border_color: foreground.with_opacity(if selected { 1.0 } else { 0.4 }),
            border_radius: 2.0,
            ..base.clone()
        };
        scene
            .labels
            .push(OverlayLabel::new("", mini, frame_style.clone()).with_z_index(i32::MAX));
        let fill = Rect::new(
            mini.x + relative.x * mini.width,
            mini.y + relative.y * mini.height,
            relative.width * mini.width,
            relative.height * mini.height,
        )
        .inset(2.0 * scale, 2.0 * scale);
        scene.labels.push(
            OverlayLabel::new(
                "",
                fill,
                LabelStyle {
                    background: foreground.with_opacity(if selected { 0.65 } else { 0.25 }),
                    border_width: 0.0,
                    ..frame_style
                },
            )
            .with_z_index(i32::MAX),
        );
        push_panel_text(
            scene,
            format!("{} · {name}", key.to_uppercase()),
            Rect::new(
                cell.x + 4.0 * scale,
                mini.bottom() + 2.0 * scale,
                cell.width - 8.0 * scale,
                17.0 * scale,
            ),
            &LabelStyle {
                bold: selected,
                ..base.clone()
            },
            scale,
        );
    }
    let body_width = widths
        .iter()
        .map(|(keys, actions)| keys + key_gap + actions)
        .sum::<f64>()
        + (columns - 1) as f64 * column_gap;
    let body_left = panel.x + (panel.width - body_width) / 2.0;
    for (index, (keys, _action)) in entries.into_iter().enumerate() {
        // Read down each column, with one consistent left edge per column.
        let column = index / rows;
        let (key_width, action_width) = widths[column];
        let column_offset: f64 = widths
            .iter()
            .take(column)
            .map(|(keys, actions)| keys + key_gap + actions + column_gap)
            .sum();
        let cell = Rect::new(
            body_left + column_offset,
            panel.y
                + vertical_padding
                + header_height
                + preview_height
                + (index % rows) as f64 * row_height,
            key_width + key_gap + action_width,
            (row_height - 2.0 * scale).max(1.0),
        );
        push_sized_help_text(
            scene,
            if window_help {
                super::elide(
                    &keys,
                    ((key_width / scale - 10.0) / (ui.font_size * 0.75)).max(1.0) as usize,
                )
            } else {
                keys
            },
            Rect::new(cell.x, cell.y, key_width, cell.height),
            &key_style,
            scale,
        );
        for (line_index, line) in action_lines[index].iter().enumerate() {
            push_sized_help_text(
                scene,
                line.clone(),
                Rect::new(
                    cell.x + key_width + key_gap,
                    cell.center().y
                        - action_lines[index].len() as f64 * action_style.font_size * 1.4 * scale
                            / 2.0
                        + line_index as f64 * action_style.font_size * 1.4 * scale,
                    action_width,
                    action_style.font_size * 1.4 * scale,
                ),
                &action_style,
                scale,
            );
        }
    }
    scene.sort_in_place();
}

fn wrap_action(text: &str, capacity: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && super::text_units(&format!("{line}{word}")) > capacity {
            lines.push(std::mem::take(&mut line));
        }
        for ch in word.chars() {
            if !line.is_empty()
                && super::text_units(&line) + super::text_units(&ch.to_string()) > capacity
            {
                lines.push(std::mem::take(&mut line));
            }
            line.push(ch);
        }
        line.push(' ');
    }
    if !line.is_empty() {
        lines.push(line.trim_end().into());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    for line in &mut lines {
        *line = line.trim_end().into();
    }
    lines
}

fn window_action_label(action: &str) -> Option<&'static str> {
    Some(match action {
        "window_left" => "Left",
        "window_right" => "Right",
        "window_up" => "Up",
        "window_down" => "Down",
        "window_size" => "Move / resize",
        "window_layout" => "Quick layout",
        "window_edit" => "Edit layout tree",
        "window_saved_layouts" => "Restore layout",
        "window_save_layout" => "Save layout",
        "Area number" => "Area number prefix",
        "window_layout_left" => "Layout left",
        "window_layout_right" => "Layout right",
        "window_layout_up" => "Layout up",
        "window_layout_down" => "Layout down",
        "window_split_left" => "Split left",
        "window_split_right" => "Split right",
        "window_split_up" => "Split up",
        "window_split_down" => "Split down",
        "window_ratio_left" => "Move divider left",
        "window_ratio_right" => "Move divider right",
        "window_ratio_up" => "Move divider up",
        "window_ratio_down" => "Move divider down",
        "window_tile" => "Tile windows",
        "window_screen_next" => "Next screen",
        "window_screen_previous" => "Previous screen",
        "window_maximize" => "Maximize / restore",
        "window_center" => "Center window",
        "window_select" => "Next window",
        "window_undo" => "Undo",
        "window_remove_region" => "Delete region",
        "window_confirm" => "Apply / exit",
        "window_cancel" => "Back / exit",
        "window_exit" => "Exit",
        _ => return None,
    })
}

pub(crate) fn push_help_text(
    scene: &mut OverlayScene,
    text: String,
    cell: Rect,
    base: &LabelStyle,
    scale: f64,
) {
    let style = base.fit_to(
        &text,
        Rect::new(0.0, 0.0, cell.width / scale, cell.height / scale),
    );
    push_sized_help_text(scene, text, cell, &style, scale);
}

pub(crate) fn push_sized_help_text(
    scene: &mut OverlayScene,
    text: String,
    cell: Rect,
    style: &LabelStyle,
    scale: f64,
) {
    let estimated_width = style.font_size
        * if style.text_alignment == TextAlignment::Left {
            super::text_units(&text).max(0.75)
        } else {
            0.75 * text.chars().count().max(1) as f64
        }
        + style.padding_x * 2.0;
    // Left-aligned prose uses its column's text box, without reserving the
    // wider keycap character estimate as invisible space on the right.
    let width = if style.text_alignment == TextAlignment::Left {
        estimated_width.min(cell.width / scale)
    } else {
        estimated_width
    };
    let height = style.font_size * 1.4 + style.padding_y * 2.0;
    // Keycaps share the column's right edge; prose uses native left alignment
    // so actual glyph advances no longer shift the visible start of each row.
    let left = if style.text_alignment == TextAlignment::Left {
        cell.x
    } else {
        cell.right() - width * scale
    };
    let rect = Rect::new(
        left + width * (scale - 1.0) / 2.0,
        cell.center().y - height / 2.0,
        width,
        height,
    );
    scene
        .labels
        .push(OverlayLabel::new(text, rect, style.clone()).with_z_index(i32::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_help_fits_inside_target_or_visible_work_area_at_multiple_scales() {
        for scale in [1.0, 1.5, 2.0] {
            let screen = Screen {
                bounds: Rect::new(-1600.0, 0.0, 1600.0, 1000.0),
                work_area: Rect::new(-1600.0, 0.0, 1600.0, 960.0),
                scale,
                is_primary: true,
                name: None,
            };
            for anchor in [
                Rect::new(-1400.0, 100.0, 1000.0, 700.0),
                Rect::new(-100.0, 870.0, 140.0, 120.0),
            ] {
                let mut scene = OverlayScene::new();
                compose(
                    &mut scene,
                    KeyHelpView {
                        screen: &screen,
                        ui: &KeyHelp::default(),
                        palette: &Palette::default(),
                        entries: vec!["H  ·  window_left".into(), "1  ·  Window number".into()],
                        window_help: true,
                        display_name: "window".into(),
                        previews: Vec::new(),
                        detail: None,
                        anchor: Some(anchor),
                        indicator_style: None,
                    },
                );
                let panel = scene
                    .labels
                    .iter()
                    .find(|label| label.z_index == i32::MAX - 1)
                    .unwrap()
                    .rect;
                let containing = if anchor.width >= 1000.0 {
                    anchor
                } else {
                    screen.work_area
                };
                assert!(
                    panel.x >= containing.x
                        && panel.y >= containing.y
                        && panel.right() <= containing.right()
                        && panel.bottom() <= containing.bottom(),
                    "scale={scale} panel={panel:?} containing={containing:?}"
                );
                assert!(
                    !scene
                        .labels
                        .iter()
                        .any(|label| label.text == "1" || label.text.contains("window number"))
                );
            }
        }
    }
}
