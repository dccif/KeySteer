//! Key-help panel composition; effective routing and cache ownership stay in the host.
use crate::api::overlay::{Color, LabelStyle, OverlayLabel, TextAlignment};
use crate::api::style::KeyHelp;
use crate::api::{OverlayScene, Palette, Rect, Screen};
use std::collections::BTreeMap;

mod window;

pub(crate) struct KeyHelpView<'a> {
    pub screen: &'a Screen,
    pub ui: &'a KeyHelp,
    pub palette: &'a Palette,
    pub entries: std::sync::Arc<[String]>,
    pub extra_entries: Vec<String>,
    pub return_target: Option<String>,
    pub window_help: bool,
    pub display_name: String,
    pub previews: Vec<(String, String, Rect, bool)>,
    pub detail: Option<String>,
    pub anchor: Option<Rect>,
    pub indicator_style: Option<LabelStyle>,
}

pub(crate) fn compose(scene: &mut OverlayScene, input: KeyHelpView<'_>) {
    compose_columns(scene, input, 2);
}

fn compose_columns(scene: &mut OverlayScene, input: KeyHelpView<'_>, max_columns: usize) {
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
    for entry in input.entries.iter().chain(&input.extra_entries) {
        if let Some((keys, action)) = entry.split_once("  ·  ") {
            if window_help && action == "Window number" {
                continue;
            }
            grouped
                .entry(action.into())
                .or_default()
                .push(keys.replace('_', " ").to_uppercase());
        }
    }
    let (mut entries, sections) = if window_help {
        (
            Vec::new(),
            Some(window::sections(
                grouped,
                &input.display_name,
                input.return_target.as_deref(),
                input
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.starts_with("Resize")),
            )),
        )
    } else {
        let mut entries: Vec<_> = grouped
            .into_iter()
            .map(|(action, keys)| (keys.join(" / "), action))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        (entries, None)
    };
    let exit_action = sections
        .as_ref()
        .map(|s| s.exit_label.clone())
        .unwrap_or_default();
    let mode_entries = sections
        .as_ref()
        .map(|s| s.modes.clone())
        .unwrap_or_default();
    let exit_keys = sections
        .as_ref()
        .map(|s| s.exit.clone())
        .unwrap_or_default();
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
    let available_width = (area.width - 12.0 * 2.0 * scale).max(1.0);
    let padding = (ui.padding_x * scale).min(available_width / 4.0);
    let vertical_padding = ui.padding_y * scale;
    let column_gap = if window_help { 20.0 } else { 6.0 } * scale;
    let key_gap = 8.0 * scale;
    let key_padding = if window_help { 2.0 } else { 5.0 };
    let columns = if let Some(sections) = sections {
        let count = if max_columns > 1
            && available_width >= 620.0 * scale
            && !sections.left.is_empty()
            && !sections.right.is_empty()
        {
            2
        } else {
            1
        };
        entries = sections.entries(count);
        count
    } else if available_width >= 560.0 * scale && entries.len() > 8 {
        2
    } else {
        1
    };
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
            (key_chars as f64 * ui.font_size * 0.75 + key_padding * 2.0) * scale,
            (action_chars as f64 * ui.font_size * 0.75 + ui.font_size * 0.5) * scale,
        ));
    }
    let detail = &input.detail;
    let (detail_title, detail_status) = detail
        .as_deref()
        .unwrap_or("")
        .split_once('\n')
        .unwrap_or((detail.as_deref().unwrap_or(""), ""));
    let mut detail_status = detail_status.to_owned();
    if window_help
        && matches!(
            input.display_name.as_str(),
            "window" | "window_quick" | "window_editor"
        )
    {
        let lines: Vec<_> = detail_status.lines().collect();
        if lines.len() >= 2 {
            detail_status = std::iter::once(format!("{}  \u{b7}  {}", lines[0], lines[1]))
                .chain(lines[2..].iter().map(|line| (*line).to_owned()))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    let detail_title = if let Some((title, instruction)) = detail_title.split_once(" \u{b7} ") {
        detail_status = format!("{instruction}\n{detail_status}");
        title
    } else {
        detail_title
    };
    let content_width: f64 = widths
        .iter()
        .map(|(keys, actions)| keys + key_gap + actions)
        .sum::<f64>()
        + (columns - 1) as f64 * column_gap;
    let header_width = (ui.font_size * 1.75 * 5.0
        + detail_title.chars().count() as f64 * ui.font_size * 0.75
        + 16.0
        + exit_keys.chars().count() as f64 * ui.font_size * 0.75
        + key_padding * 2.0
        + super::text_units(&exit_action) * ui.font_size
        + 2.0)
        * scale
        + key_gap * 3.0;
    let status_width = detail_status
        .lines()
        .map(super::text_units)
        .fold(0.0, f64::max)
        * ui.font_size
        * scale;
    let natural_width = (if window_help {
        content_width.max(header_width).max(status_width)
    } else {
        content_width
    } + padding * 2.0)
        .max(if window_help && columns == 2 {
            620.0 * scale
        } else {
            340.0 * scale
        });
    if window_help && natural_width > available_width && columns > 1 {
        return compose_columns(scene, input, 1);
    }
    // A single font ratio is the last resort on a screen too narrow for the
    // complete rows; never truncate a key or wrap half of a chord.
    if window_help && natural_width > available_width && ui.font_size > 0.5 {
        let mut fitted_ui = ui.clone();
        fitted_ui.font_size = (ui.font_size * (available_width - padding * 2.0).max(1.0)
            / (natural_width - padding * 2.0).max(1.0)
            * 0.98)
            .max(0.5);
        return compose_columns(
            scene,
            KeyHelpView {
                ui: &fitted_ui,
                ..input
            },
            max_columns,
        );
    }
    let width = natural_width.min(available_width);
    let squeeze =
        ((width - padding * 2.0 - (columns - 1) as f64 * column_gap - columns as f64 * key_gap)
            / widths
                .iter()
                .map(|(keys, actions)| keys + actions)
                .sum::<f64>())
        .clamp(0.01, 1.0);
    for (keys, actions) in &mut widths {
        if window_help {
            // Natural column widths preserve every row on one line.
        } else {
            *keys *= squeeze;
            *actions *= squeeze;
        }
    }
    let entry_widths: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, _)| widths[index / rows])
        .collect();
    let key_lines: Vec<Vec<String>> = entries.iter().map(|(keys, _)| vec![keys.clone()]).collect();
    let action_lines: Vec<Vec<String>> = entries
        .iter()
        .map(|(_, action)| vec![action.clone()])
        .collect();
    let previews = input.previews;
    let preview_rows = previews.len().div_ceil(3);
    let preview_height = (preview_rows as f64 * 56.0 * scale).min(area.height * 0.35);
    let title_font = ui.font_size * if window_help { 1.75 } else { 1.25 };
    let exit_width = if exit_keys.is_empty() {
        0.0
    } else {
        (exit_keys.chars().count() as f64 * ui.font_size * 0.75 + key_padding * 2.0)
            .min(width / scale * 0.42)
            * scale
    };
    let exit_lines = [exit_keys.clone()];
    let title_height = (ui.font_size * 7.0 / 3.0).max(title_font * 1.4).max(
        if window_help && !exit_keys.is_empty() {
            exit_lines.len() as f64 * (ui.font_size * 1.4 + 4.0)
        } else {
            0.0
        },
    ) * scale;
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
    let row_heights: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, (keys, action))| {
            if !window_help {
                return row_height;
            }
            if keys.is_empty() && action.is_empty() {
                return 0.0;
            }
            let lines = action_lines[index].len().max(key_lines[index].len());
            (ui.font_size * (1.4 * lines as f64 + 0.6) + 4.0 * lines.saturating_sub(1) as f64)
                * scale
        })
        .collect();
    let body_height = row_heights
        .chunks(rows)
        .map(|column| column.iter().sum::<f64>())
        .fold(0.0, f64::max);
    let footer_line_height = ui.font_size * 2.2 * scale;
    let mut footer = Vec::new();
    let mut footer_x = 0.0;
    let mut footer_y = 0.0;
    let footer_columns = mode_entries.len().clamp(1, 4);
    let footer_cell_width = (width - padding * 2.0) / footer_columns as f64;
    for (keys, action) in mode_entries {
        let key_width =
            (keys.chars().count() as f64 * ui.font_size * 0.75 + key_padding * 2.0) * scale;
        let action_width = (super::text_units(&action) * ui.font_size + 4.0) * scale;
        let item_width = key_width + key_gap + action_width;
        if footer_x > 0.0 && footer_x + item_width > width - padding * 2.0 {
            footer_x = 0.0;
            footer_y += footer_line_height;
        }
        footer.push((keys, action, footer_x, footer_y, key_width, action_width));
        footer_x += footer_cell_width.max(item_width + 16.0 * scale);
    }
    let footer_height = if footer.is_empty() {
        0.0
    } else {
        footer_y + footer_line_height + 4.0 * scale
    };
    let height =
        header_height + preview_height + body_height + footer_height + vertical_padding * 2.0;
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
    if window_help {
        super::label_placement::avoid_overlaps(scene, screen, &[panel]);
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
    let exit_action_width = (super::text_units(&exit_action) * ui.font_size + 2.0) * scale;
    let close_width = if window_help {
        if exit_keys.is_empty() {
            0.0
        } else {
            exit_width + key_gap + exit_action_width
        }
    } else {
        (close_hint.chars().count() as f64 * (ui.font_size * 1.25) * 0.75 * scale).min(width * 0.45)
    };
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
            .min((panel.right() - padding - close_width - key_gap - left) / scale)
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
            if window_help {
                status.into()
            } else {
                super::elide_width(status, (width - padding * 2.0) / (ui.font_size * scale))
            },
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
    if !window_help {
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
    }
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
        padding_x: key_padding,
        padding_y: 1.0,
        bold: true,
        ..base.clone()
    }
    .scaled(body_scale);
    if window_help && !exit_keys.is_empty() {
        push_sized_help_text(
            scene,
            exit_action,
            Rect::new(
                panel.right() - padding - exit_action_width,
                panel.y + vertical_padding,
                exit_action_width,
                title_height,
            ),
            &base,
            scale,
        );
        for (index, line) in exit_lines.iter().enumerate() {
            push_sized_help_text(
                scene,
                line.clone(),
                Rect::new(
                    panel.right() - padding - close_width,
                    panel.y + vertical_padding + index as f64 * (ui.font_size * 1.4 + 4.0) * scale,
                    exit_width,
                    title_height,
                ),
                &key_style,
                scale,
            );
        }
    }
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
        let (key_width, action_width) = entry_widths[index];
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
                + row_heights[column * rows..index].iter().sum::<f64>(),
            key_width + key_gap + action_width,
            (row_heights[index] - 2.0 * scale).max(1.0),
        );
        if window_help && keys.is_empty() {
            if !_action.is_empty() {
                for (line_index, line) in action_lines[index].iter().enumerate() {
                    push_sized_help_text(
                        scene,
                        line.clone(),
                        Rect::new(
                            cell.x,
                            cell.y + line_index as f64 * ui.font_size * 1.4 * scale,
                            cell.width,
                            ui.font_size * 1.8 * scale,
                        ),
                        &LabelStyle {
                            bold: true,
                            text_color: foreground.with_opacity(0.65),
                            ..action_style.clone()
                        },
                        scale,
                    );
                }
            }
            continue;
        }
        for (line_index, line) in key_lines[index].iter().enumerate() {
            let line_height = (ui.font_size * 1.4 + 4.0) * scale;
            push_sized_help_text(
                scene,
                line.clone(),
                Rect::new(
                    cell.x,
                    cell.center().y - key_lines[index].len() as f64 * line_height / 2.0
                        + line_index as f64 * line_height,
                    key_width,
                    line_height,
                ),
                &key_style,
                scale,
            );
        }
        for (line_index, line) in action_lines[index].iter().enumerate() {
            push_sized_help_text(
                scene,
                line.clone(),
                Rect::new(
                    cell.x + key_width + key_gap,
                    cell.center().y
                        - action_lines[index].len() as f64
                            * (action_style.font_size * 1.4 + 1.0)
                            * scale
                            / 2.0
                        + line_index as f64 * (action_style.font_size * 1.4 + 1.0) * scale,
                    action_width,
                    (action_style.font_size * 1.4 + 1.0) * scale,
                ),
                &action_style,
                scale,
            );
        }
    }
    if !footer.is_empty() {
        scene.labels.push(
            OverlayLabel::new(
                "",
                Rect::new(
                    panel.x + padding,
                    panel.y + vertical_padding + header_height + preview_height + body_height,
                    width - padding * 2.0,
                    scale,
                ),
                LabelStyle {
                    background: foreground.with_opacity(0.18),
                    font_size: 1.0,
                    ..base.clone()
                },
            )
            .with_z_index(i32::MAX),
        );
    }
    for (keys, action, x, y, key_width, action_width) in footer {
        let y = panel.y
            + vertical_padding
            + header_height
            + preview_height
            + body_height
            + 4.0 * scale
            + y;
        push_sized_help_text(
            scene,
            keys,
            Rect::new(panel.x + padding + x, y, key_width, footer_line_height),
            &key_style,
            scale,
        );
        push_sized_help_text(
            scene,
            action,
            Rect::new(
                panel.x + padding + x + key_width + key_gap,
                y,
                action_width,
                footer_line_height,
            ),
            &action_style,
            scale,
        );
    }
    scene.sort_in_place();
}

fn window_action_label(action: &str) -> Option<&'static str> {
    Some(match action {
        "window_left" => "Left",
        "window_right" => "Right",
        "window_up" => "Up",
        "window_down" => "Down",
        "window_size" => "Move / resize",
        "window_editor" => "Edit layout tree",
        "window_quick" => "Quick layout",
        "window_restore" => "Restore layout",
        "window_delete" => "Restore / delete",
        "window_tab" => "Group windows as tabs",
        "window" => "Window",
        "idle" => "Idle",
        "normal" => "Normal",
        "grid" => "Grid",
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
        "window_ratio_left" => "Shrink region width",
        "window_ratio_right" => "Grow region width",
        "window_ratio_up" => "Shrink region height",
        "window_ratio_down" => "Grow region height",
        "window_tile" => "Tile windows",
        "window_screen_next" => "Next screen",
        "window_screen_previous" => "Previous screen",
        "size_cycle" => "Maximize / minimize / restore",
        "window_center" => "Center window",
        "window_close" => "Close window",
        "window_select" => "Next window",
        "window_select_previous" => "Previous window",
        "window_undo" => "Undo",
        "window_redo" => "Redo",
        "window_reset_initial" => "Restore initial state",
        "window_remove_region" => "Delete region",
        "window_confirm" => "Confirm",
        "window_tab_end" => "Next group",
        "window_tab_group" => "Group number",
        "window_number_end" => "End number",
        "window_tab_remove" => "Remove current tab",
        "window_tab_dissolve" => "Dissolve group",
        "window_tab_next" => "Next tab",
        "window_tab_previous" => "Previous tab",
        "window_tab_move_left" => "Move tab left",
        "window_tab_move_right" => "Move tab right",
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
        (estimated_width + style.font_size * 0.5).min(cell.width / scale)
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
                        return_target: None,
                        extra_entries: Vec::new(),
                        entries: vec!["H  ·  window_left".into(), "1  ·  Window number".into()]
                            .into(),
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
