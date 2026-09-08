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
                    continue;
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
    let ui = input.ui;
    let color = |value: &Option<crate::api::theme::ThemedColor>, fallback| {
        crate::api::style::resolve(value.as_ref(), palette.appearance, fallback)
    };
    let area = screen.work_area;
    let available_width = (area.width - 12.0 * 2.0 * scale)
        .max(1.0)
        .min(if window_help { 760.0 * scale } else { f64::MAX });
    let padding = (ui.padding_x * scale).min(available_width / 4.0);
    let vertical_padding = ui.padding_y * scale;
    let column_gap = 6.0 * scale;
    let key_gap = 8.0 * scale;
    let columns = if available_width >= 560.0 * scale && entries.len() > 8 {
        2_usize.min(entries.len()).max(1)
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
            (key_chars as f64 * ui.font_size * 0.75 + 5.0 * 2.0) * scale,
            (action_chars as f64 * ui.font_size * 0.6 + ui.font_size / 3.0) * scale,
        ));
    }
    let content_width: f64 = widths
        .iter()
        .map(|(keys, actions)| keys + key_gap + actions)
        .sum::<f64>()
        + (columns - 1) as f64 * column_gap;
    let width = (content_width + padding * 2.0)
        .max(340.0 * scale)
        .min(available_width);
    let squeeze =
        ((width - padding * 2.0 - (columns - 1) as f64 * column_gap - columns as f64 * key_gap)
            / widths
                .iter()
                .map(|(keys, actions)| keys + actions)
                .sum::<f64>())
        .clamp(0.01, 1.0);
    for (keys, actions) in &mut widths {
        *keys *= squeeze;
        *actions *= squeeze;
    }
    let previews = input.previews;
    let preview_rows = previews.len().div_ceil(3);
    let preview_height = (preview_rows as f64 * 56.0 * scale).min(area.height * 0.35);
    let detail = input.detail;
    let (detail_title, detail_status) = detail
        .as_deref()
        .unwrap_or("")
        .split_once('\n')
        .unwrap_or((detail.as_deref().unwrap_or(""), ""));
    let title_height = (ui.font_size * 7.0 / 3.0).max((ui.font_size * 1.25) * 1.4) * scale;
    let header_height = title_height
        + if detail_status.is_empty() {
            0.0
        } else {
            ui.font_size * 1.8 * scale
        };
    let row_height = ((ui.font_size * 2.0) * scale).min(
        ((area.height * 0.75 - header_height - preview_height - vertical_padding * 2.0)
            / rows as f64)
            .max(8.0),
    );
    let height = header_height + preview_height + rows as f64 * row_height + vertical_padding * 2.0;
    let mut panel = Rect::new(
        area.x + (area.width - width) / 2.0,
        area.bottom() - 12.0 * scale - height,
        width,
        height,
    );
    if window_help && let Some(anchor) = input.anchor {
        panel.x =
            (anchor.center().x - width / 2.0).clamp(area.x, (area.right() - width).max(area.x));
        // Prefer below the locked window; when it reaches the work-area
        // bottom, keep the complete panel visible over its lower portion.
        panel.y = (anchor.bottom() + 10.0 * scale)
            .clamp(area.y, (area.bottom() - height - 8.0 * scale).max(area.y));
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
    let title = if window_help {
        format!("Window · {detail_title}")
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
    push_help_text(
        scene,
        title,
        Rect::new(
            panel.x + padding,
            panel.y + vertical_padding,
            (width - padding * 2.0 - close_width - key_gap).max(1.0),
            title_height,
        ),
        &LabelStyle {
            font_size: (ui.font_size * 1.25),
            text_color: foreground,
            bold: true,
            ..base.clone()
        },
        scale,
    );
    if !detail_status.is_empty() {
        push_help_text(
            scene,
            detail_status.into(),
            Rect::new(
                panel.x + padding,
                panel.y + vertical_padding + title_height,
                width - padding * 2.0,
                header_height - title_height,
            ),
            &base,
            scale,
        );
    }
    push_help_text(
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
    let body_scale = squeeze
        .min((row_height / scale - 2.0) / (ui.font_size * 1.4 + 1.0 * 2.0))
        .max(0.01);
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
        push_help_text(
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
    for (index, (keys, action)) in entries.into_iter().enumerate() {
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
            keys,
            Rect::new(cell.x, cell.y, key_width, cell.height),
            &key_style,
            scale,
        );
        push_sized_help_text(
            scene,
            action,
            Rect::new(
                cell.x + key_width + key_gap,
                cell.y,
                action_width,
                cell.height,
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
        "window_layout" => "Quick layout / double tap to tile",
        "window_edit" => "Edit layout tree",
        "Window number" => "Select window number",
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
        "window_confirm" => "Apply / exit",
        "window_cancel" => "Back / exit",
        "window_exit" => "Exit",
        _ => return None,
    })
}

fn push_help_text(
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

fn push_sized_help_text(
    scene: &mut OverlayScene,
    text: String,
    cell: Rect,
    style: &LabelStyle,
    scale: f64,
) {
    let estimated_width =
        style.font_size * 0.75 * text.chars().count().max(1) as f64 + style.padding_x * 2.0;
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
