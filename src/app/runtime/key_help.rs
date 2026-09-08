//! Read-only help derived from the same routing decisions as real input.
use super::*;
use crate::api::geometry::Rect;
use crate::api::overlay::{Color, LabelStyle, OverlayItems, OverlayLabel, TextAlignment};

/// One visible-session cache. Source and decorated labels share the existing
/// scene storage; there is no history or map that can grow across inputs.
pub(super) struct KeyHelpCache {
    source_labels: OverlayItems<OverlayLabel>,
    labels: OverlayItems<OverlayLabel>,
    source_clip: Option<Rect>,
    clip: Option<Rect>,
    bounds: Rect,
    work_area: Rect,
    scale: f64,
    active: ModeId,
    display: ModeId,
    palette: Palette,
}

impl KeyHelpCache {
    pub(super) fn matches_screen(&self, screen: &Screen) -> bool {
        self.bounds == screen.bounds
            && self.work_area == screen.work_area
            && self.scale == screen.scale
    }
}

impl Engine {
    pub(super) fn key_help_entries(&self) -> Vec<String> {
        let mut entries = std::collections::BTreeSet::new();
        // Enumerate configured chords, then resolve each through the actual
        // active/inherited/temporary route. Disabled and shadowed entries vanish.
        for id in self.registry.keys() {
            let Some(table) = self.registry.table(id) else {
                continue;
            };
            for entry in table.iter_entries() {
                if matches!(entry.binding.as_ref(), Binding::Disabled) {
                    continue;
                }
                let mut pressed: Vec<Key> = self.input.pressed.iter().cloned().collect();
                for key in entry.chord.keys() {
                    if !pressed
                        .iter()
                        .any(|physical| Self::keys_match(key, physical))
                    {
                        pressed.push(key.clone());
                    }
                }
                let Some(resolved) =
                    self.lookup_for_pressed(entry.chord.activation_key(), &pressed)
                else {
                    continue;
                };
                if resolved.owner != *id || resolved.binding != entry.binding {
                    continue;
                }
                // While a non-modifier chord prefix is held, show its continuations.
                if !self.input.pending_chords.is_empty()
                    && !self
                        .input
                        .pressed
                        .iter()
                        .filter(|key| !key.is_modifier())
                        .all(|key| {
                            entry
                                .chord
                                .keys()
                                .iter()
                                .any(|part| Self::keys_match(part, key))
                        })
                {
                    continue;
                }
                entries.insert(format!(
                    "{}  ·  {}",
                    entry.chord.canonical(),
                    entry.binding.canonical()
                ));
            }
        }
        if self.display_mode() == self.registry.active
            && let Some(mode) = self.registry.get(&self.registry.active)
        {
            for (key, action) in mode.available_keys() {
                let Ok(chord) = KeyChord::parse(&key) else {
                    continue;
                };
                let mut pressed: Vec<_> = self.input.pressed.iter().cloned().collect();
                pressed.extend(chord.keys().iter().cloned());
                if self
                    .lookup_for_pressed(chord.activation_key(), &pressed)
                    .is_none()
                {
                    entries.insert(format!("{key}  ·  {action}"));
                }
            }
        }
        entries.into_iter().collect()
    }

    pub(super) fn decorate_key_help(&mut self, scene: &mut OverlayScene) {
        if !self.overlay.key_help_visible || self.registry.active == ModeId::idle() {
            self.overlay.key_help_cache = None;
            return;
        }
        let Some(screen) = Screen::containing(&self.screens, &self.cursor) else {
            self.overlay.key_help_cache = None;
            return;
        };
        let display = self.display_mode();
        if let Some(cache) = &self.overlay.key_help_cache
            && cache.active == self.registry.active
            && cache.display == display
            && cache.palette == self.palette
            && cache.matches_screen(screen)
            && cache.source_clip == scene.clip
            && (cache.source_labels.shares_storage_with(&scene.labels)
                || (cache.source_labels.is_empty() && scene.labels.is_empty()))
        {
            scene.labels = cache.labels.clone();
            scene.clip = cache.clip;
            return;
        }
        let (bounds, work_area, scale) = (screen.bounds, screen.work_area, screen.scale);
        let source_labels = scene.labels.clone();
        let source_clip = scene.clip;
        self.build_key_help(scene);
        self.overlay.key_help_cache = Some(Box::new(KeyHelpCache {
            source_labels,
            labels: scene.labels.clone(),
            source_clip,
            clip: scene.clip,
            bounds,
            work_area,
            scale,
            active: self.registry.active.clone(),
            display,
            palette: self.palette.clone(),
        }));
    }

    fn build_key_help(&self, scene: &mut OverlayScene) {
        if !self.overlay.key_help_visible || self.registry.active == ModeId::idle() {
            return;
        }
        let Some(screen) = Screen::containing(&self.screens, &self.cursor) else {
            return;
        };
        scene.clip = Some(
            scene
                .clip
                .map_or(screen.bounds, |clip| clip.union(&screen.bounds)),
        );
        // Group equivalent actions instead of repeating a badge for every key.
        let mut grouped = BTreeMap::<String, Vec<String>>::new();
        for entry in self.key_help_entries() {
            if let Some((keys, action)) = entry.split_once("  ·  ") {
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
        let ui = &self.settings.key_help;
        let color = |value: &Option<crate::api::theme::ThemedColor>, fallback| {
            crate::api::style::resolve(value.as_ref(), self.palette.appearance, fallback)
        };
        let area = screen.work_area;
        let available_width = (area.width - 12.0 * 2.0 * scale).max(1.0);
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
        let squeeze = ((width
            - padding * 2.0
            - (columns - 1) as f64 * column_gap
            - columns as f64 * key_gap)
            / widths
                .iter()
                .map(|(keys, actions)| keys + actions)
                .sum::<f64>())
        .clamp(0.01, 1.0);
        for (keys, actions) in &mut widths {
            *keys *= squeeze;
            *actions *= squeeze;
        }
        let header_height = (ui.font_size * 7.0 / 3.0).max((ui.font_size * 1.25) * 1.4) * scale;
        let row_height = ((ui.font_size * 2.0) * scale).min(
            ((area.height * 0.6 - header_height - vertical_padding * 2.0) / rows as f64).max(8.0),
        );
        let height = header_height + rows as f64 * row_height + vertical_padding * 2.0;
        let panel = Rect::new(
            area.x + (area.width - width) / 2.0,
            area.bottom() - 12.0 * scale - height,
            width,
            height,
        );
        let display_mode = self.display_mode();
        let indicator_style = self
            .build_indicator(&display_mode)
            .map(|(indicator, _)| indicator.style);
        let derived_background = indicator_style
            .as_ref()
            .map_or(self.palette.accent, |style| style.background);
        let derived_foreground = indicator_style.as_ref().map_or_else(
            || self.palette.readable_on(derived_background),
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
        let title = format!(
            "{} · Available keys",
            self.registry
                .get(&display_mode)
                .map_or_else(|| display_mode.to_string(), |mode| mode.display_name())
        );
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
        let close_width =
            (close_hint.chars().count() as f64 * (ui.font_size * 1.25) * 0.75 * scale)
                .min(width * 0.45);
        push_help_text(
            scene,
            title,
            Rect::new(
                panel.x + padding,
                panel.y + vertical_padding,
                (width - padding * 2.0 - close_width - key_gap).max(1.0),
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
                panel.y + vertical_padding + header_height + (index % rows) as f64 * row_height,
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
