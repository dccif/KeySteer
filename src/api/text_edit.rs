//! Shared editing vocabulary for overlay input; no native widget owns the text.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditAction {
    Accept,
    Cancel,
    Paste,
    Copy,
    Cut,
    SelectAll,
    Left,
    Right,
    Home,
    End,
    SelectLeft,
    SelectRight,
    SelectHome,
    SelectEnd,
    Backspace,
    Delete,
}

/// Editing keys that a mode-owned prompt may consume instead of injecting.
pub fn navigation_action(chord: &crate::api::KeyChord) -> Option<EditAction> {
    use EditAction::*;
    let keys = chord.keys();
    let selecting = keys.len() == 2
        && keys
            .iter()
            .any(|key| matches!(key.as_str(), "shift" | "left_shift" | "right_shift"));
    if keys.len() != 1 && !selecting {
        return None;
    }
    Some(match (chord.activation_key().as_str(), selecting) {
        ("arrow_left", false) => Left,
        ("arrow_right", false) => Right,
        ("home", false) => Home,
        ("end", false) => End,
        ("arrow_left", true) => SelectLeft,
        ("arrow_right", true) => SelectRight,
        ("home", true) => SelectHome,
        ("end", true) => SelectEnd,
        ("backspace", false) => Backspace,
        ("delete", false) => Delete,
        _ => return None,
    })
}

pub fn default_keys() -> std::collections::BTreeMap<EditAction, String> {
    use EditAction::*;
    [
        (Accept, "enter / primary+q"),
        (Cancel, "esc"),
        (Paste, "primary+v"),
        (Copy, "primary+c"),
        (Cut, "primary+x"),
        (SelectAll, "primary+a"),
        (Left, "left"),
        (Right, "right"),
        (Home, "home"),
        (End, "end"),
        (SelectLeft, "shift+left"),
        (SelectRight, "shift+right"),
        (SelectHome, "shift+home"),
        (SelectEnd, "shift+end"),
        (Backspace, "backspace"),
        (Delete, "delete"),
    ]
    .into_iter()
    .map(|(action, key)| (action, key.into()))
    .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub cursor: usize,
    pub anchor: usize,
}

impl Selection {
    pub fn range(self) -> std::ops::Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }
    /// Returns whether the query changed; capacity is retained even when empty.
    pub fn insert(&mut self, text: &mut String, value: &str) -> bool {
        let range = self.range();
        // Most queries fit even measured in bytes. Count Unicode only near
        // the limit, rather than walking the whole query on every keystroke.
        let end = if text.len() - range.len() + value.len() <= 4096 {
            value.len()
        } else {
            let remaining = 4096usize.saturating_sub(
                text[..range.start].chars().count() + text[range.end..].chars().count(),
            );
            value
                .char_indices()
                .nth(remaining)
                .map_or(value.len(), |(i, _)| i)
        };
        let changed = text[range.clone()] != value[..end];
        if changed {
            text.replace_range(range.clone(), &value[..end]);
        }
        self.cursor = range.start + end;
        self.anchor = self.cursor;
        changed
    }
    pub fn edit(&mut self, text: &mut String, action: EditAction) -> bool {
        use EditAction::*;
        let previous = text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        let next = self.cursor + text[self.cursor..].chars().next().map_or(0, char::len_utf8);
        if matches!(action, Backspace | Delete) {
            if self.cursor == self.anchor {
                self.anchor = if action == Backspace { previous } else { next };
            }
            return self.insert(text, "");
        }
        if action == SelectAll {
            self.anchor = 0;
            self.cursor = text.len();
            return false;
        }
        self.cursor = match action {
            Left if self.cursor != self.anchor => self.range().start,
            Right if self.cursor != self.anchor => self.range().end,
            Left | SelectLeft => previous,
            Right | SelectRight => next,
            Home | SelectHome => 0,
            End | SelectEnd => text.len(),
            _ => return false,
        };
        if !matches!(action, SelectLeft | SelectRight | SelectHome | SelectEnd) {
            self.anchor = self.cursor;
        }
        false
    }
}

/// Horizontal viewport offset, using an already measured insertion position.
pub fn scroll_offset(width: f64, font_size: f64, cursor_x: f64) -> f64 {
    let caret_width = (font_size / 14.0).max(1.0);
    (cursor_x - (width - caret_width).max(0.0)).max(0.0)
}

/// Space for the query after reserving an already measured trailing counter.
pub fn query_width(width: f64, font_size: f64, trailing_width: f64) -> f64 {
    if trailing_width > 0.0 {
        (width - trailing_width - font_size * 0.2).max(0.0)
    } else {
        width
    }
}

/// Shared physical geometry; native text engines supply measured insertion offsets.
pub fn decoration_rects(
    area: crate::api::Rect,
    font_size: f64,
    cursor_x: f64,
    anchor_x: f64,
) -> (crate::api::Rect, Option<crate::api::Rect>) {
    let width = (font_size / 14.0).max(1.0).min(area.width.max(0.0));
    let height = (font_size * 1.1).min(area.height).max(0.0);
    let y = area.y + (area.height - height) / 2.0;
    let cursor_x = cursor_x.clamp(0.0, (area.width - width).max(0.0));
    let anchor_x = anchor_x.clamp(0.0, (area.width - width).max(0.0));
    (
        crate::api::Rect::new(area.x + cursor_x, y, width, height),
        (cursor_x != anchor_x).then(|| {
            crate::api::Rect::new(
                area.x + cursor_x.min(anchor_x),
                y,
                (cursor_x - anchor_x).abs(),
                height,
            )
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caret_and_selection_share_clipped_centered_geometry() {
        let area = crate::api::Rect::new(10.0, 20.0, 200.0, 36.0);
        for scale in [1.0, 1.5, 2.0] {
            let (caret, selection) = decoration_rects(area, 14.0 * scale, 21.0, 7.0);
            assert_eq!(caret.x, 31.0);
            assert!((caret.center().y - area.center().y).abs() < 0.001);
            assert!(caret.height < area.height);
            let selected = selection.unwrap();
            assert_eq!(selected.x, 17.0);
            assert_eq!(selected.width, 14.0);
            let (clipped, _) = decoration_rects(area, 14.0 * scale, 900.0, -20.0);
            assert!(clipped.right() <= area.right());
            assert_eq!(scroll_offset(area.width, 14.0 * scale, 21.0), 0.0);
            let scroll = scroll_offset(area.width, 14.0 * scale, 900.0);
            let (visible, _) = decoration_rects(area, 14.0 * scale, 900.0 - scroll, -scroll);
            assert!((visible.right() - area.right()).abs() < 0.001);
            assert_eq!(query_width(area.width, 14.0 * scale, 0.0), area.width);
            assert_eq!(
                query_width(area.width, 14.0 * scale, 42.0),
                area.width - 42.0 - 2.8 * scale
            );
        }
    }

    #[test]
    #[ignore = "allocation measurement; run alone with --test-threads=1"]
    fn warmed_editor_mutations_do_not_allocate() {
        let mut text = String::with_capacity(256);
        let mut selection = Selection::default();
        let region = stats_alloc::Region::new(crate::TEST_ALLOCATOR);
        for _ in 0..10_000 {
            assert!(selection.insert(&mut text, "复制🦀"));
            selection.edit(&mut text, EditAction::SelectAll);
            assert!(selection.edit(&mut text, EditAction::Backspace));
            assert!(!selection.edit(&mut text, EditAction::Backspace));
        }
        let stats = region.change();
        assert_eq!(stats.allocations + stats.reallocations, 0, "{stats:?}");
    }
    #[test]
    fn unicode_selection_deletion_and_capacity_reuse() {
        let mut text = String::with_capacity(256);
        let mut selection = Selection::default();
        selection.insert(&mut text, "中🦀ab");
        let storage = text.as_ptr();
        selection.edit(&mut text, EditAction::Home);
        selection.edit(&mut text, EditAction::SelectRight);
        selection.edit(&mut text, EditAction::SelectRight);
        assert_eq!(&text[selection.range()], "中🦀");
        selection.insert(&mut text, "复制");
        assert_eq!(text, "复制ab");
        selection.edit(&mut text, EditAction::Backspace);
        assert_eq!(text, "复ab");
        selection.edit(&mut text, EditAction::Delete);
        assert_eq!(text, "复b");
        assert_eq!(text.as_ptr(), storage);
        selection.edit(&mut text, EditAction::SelectAll);
        selection.insert(&mut text, &"🦀".repeat(5000));
        assert_eq!(text.chars().count(), 4096);
        assert_eq!(selection.cursor, text.len());
    }
    #[test]
    fn key_action_tables_combine_alternatives_and_preserve_sparse_defaults() {
        let config = crate::config::Config::parse(
            "[ui_hint.search_edit_keys]\nf8 = 'accept'\nf9 = 'accept'\n'alt+v' = 'paste'",
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.ui_hint.search_edit_keys[&EditAction::Accept],
            "f8 f9"
        );
        assert_eq!(config.ui_hint.search_edit_keys[&EditAction::Paste], "alt+v");
        assert_eq!(config.ui_hint.search_edit_keys[&EditAction::Cancel], "esc");
        let exported = config.to_toml().unwrap();
        let table: toml::Value = toml::from_str(&exported).unwrap();
        let bindings = table["ui_hint"]["search_edit_keys"].as_table().unwrap();
        assert_eq!(bindings["f8 f9"].as_str(), Some("accept"));
        assert_eq!(bindings["alt+v"].as_str(), Some("paste"));
        assert!(!bindings.contains_key("paste"));
        assert_eq!(
            crate::config::Config::parse(&exported)
                .unwrap()
                .ui_hint
                .search_edit_keys,
            config.ui_hint.search_edit_keys,
        );
        assert!(
            crate::config::Config::parse("[ui_hint.search_edit_keys]\nf8 = 'missing_action'")
                .is_err()
        );
        for source in [
            "'enter ctrl+1' = 'accept'",
            "'enter enter' = 'accept'",
            "'enter primary+v' = 'accept'",
        ] {
            let invalid =
                crate::config::Config::parse(&format!("[ui_hint.search_edit_keys]\n{source}"))
                    .unwrap();
            assert!(invalid.validate().is_err(), "{source}");
        }
    }

    #[test]
    fn legacy_edit_keys_export_in_key_action_format_with_disabled_actions_intact() {
        let config = crate::config::Config::parse(
            "[ui_hint.search_edit_keys]\naccept = 'f9'\ncopy = ''\ncut = ''",
        )
        .unwrap();
        config.validate().unwrap();
        let exported = config.to_toml().unwrap();
        let table: toml::Value = toml::from_str(&exported).unwrap();
        let bindings = table["ui_hint"]["search_edit_keys"].as_table().unwrap();
        assert_eq!(bindings["f9"].as_str(), Some("accept"));
        assert!(!bindings.contains_key("accept"));
        let restored = crate::config::Config::parse(&exported).unwrap();
        restored.validate().unwrap();
        assert_eq!(
            restored.ui_hint.search_edit_keys,
            config.ui_hint.search_edit_keys
        );
    }

    #[test]
    fn sparse_configuration_keeps_defaults_and_rejects_shortcut_collisions() {
        let config =
            crate::config::Config::parse("[ui_hint.search_edit_keys]\npaste = 'alt+v'").unwrap();
        config.validate().unwrap();
        assert_eq!(config.ui_hint.search_edit_keys[&EditAction::Paste], "alt+v");
        assert_eq!(config.ui_hint.search_edit_keys[&EditAction::Cancel], "esc");
        let invalid =
            crate::config::Config::parse("[ui_hint.search_edit_keys]\npaste = 'ctrl+1'").unwrap();
        assert!(invalid.validate().is_err());
        for value in ["enter ctrl+1", "enter enter", "enter primary+v"] {
            let invalid = crate::config::Config::parse(&format!(
                "[ui_hint.search_edit_keys]\naccept = '{value}'"
            ))
            .unwrap();
            assert!(invalid.validate().is_err(), "{value}");
        }
    }
}
