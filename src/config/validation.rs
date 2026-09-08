//! Semantic validation for parsed configuration documents.

use super::*;

fn validate_optional_color(path: &str, color: Option<&ThemedColor>) -> Result<(), ConfigError> {
    if color.is_some_and(|value| !value.is_valid()) {
        return Err(ConfigError::Invalid(format!(
            "{path} must use #RRGGBBAA for every appearance"
        )));
    }
    Ok(())
}

fn validate_label_colors(path: &str, label: &LabelUi) -> Result<(), ConfigError> {
    for (name, value) in [
        ("background_color", label.background_color.as_ref()),
        ("text_color", label.text_color.as_ref()),
        ("matched_text_color", label.matched_text_color.as_ref()),
        ("border_color", label.border_color.as_ref()),
    ] {
        validate_optional_color(&format!("{path}.{name}"), value)?;
    }
    Ok(())
}

impl ConfigFile {
    /// Deprecated settings that should be moved to their replacement path.
    pub fn deprecation_warnings(&self) -> Vec<String> {
        self.scroll
            .invert_scroll
            .map(|_| {
                "`scroll.invert_scroll` is deprecated; use \
                 `platform.macos.scroll.invert` instead"
                    .to_string()
            })
            .into_iter()
            .collect()
    }

    /// Chords that will not behave as expected on this platform.
    ///
    /// Separate from [`Self::validate`] because these are advisory, and because
    /// a pure function can be tested and reported exactly once by the caller.
    pub fn platform_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for (label, table) in self.binding_tables() {
            for chord in table.keys() {
                let Ok(parsed) = KeyChord::parse(chord) else {
                    continue; // `validate` reports unparseable chords.
                };
                if let Some(problem) = platform_warning(&parsed) {
                    warnings.push(format!("{label} binding {chord:?}: {problem}"));
                }
            }
        }

        warnings
    }

    /// Reject configurations that would misbehave at runtime.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: String| ConfigError::Invalid(m);

        let help = &self.key_help;
        for (name, value) in [
            ("font_size", help.font_size),
            ("border_width", help.border_width),
            ("border_radius", help.border_radius),
            ("padding_x", help.padding_x),
            ("padding_y", help.padding_y),
        ] {
            if !value.is_finite() || !(0.0..=4096.0).contains(&value) {
                return Err(bad(format!("key_help.{name} must be finite and 0..=4096")));
            }
        }
        if help.font_size < 1.0 {
            return Err(bad("key_help.font_size must be at least 1".into()));
        }
        validate_optional_color("key_help.background_color", help.background_color.as_ref())?;
        validate_optional_color("key_help.text_color", help.text_color.as_ref())?;
        validate_optional_color("key_help.border_color", help.border_color.as_ref())?;

        if self.normal.long_press_toggle_ms > 60_000 {
            return Err(bad("normal.long_press_toggle_ms must be 0..=60000".into()));
        }
        if self.normal.auto_release_ms > 60_000 {
            return Err(bad("normal.auto_release_ms must be 0..=60000".into()));
        }

        for (appearance, colors) in [("light", &self.theme.light), ("dark", &self.theme.dark)] {
            for (name, value) in [
                ("surface", &colors.surface),
                ("accent", &colors.accent),
                ("accent_alt", &colors.accent_alt),
                ("on_accent_alt", &colors.on_accent_alt),
                ("text", &colors.text),
            ] {
                if crate::api::Color::parse(value).is_none() {
                    return Err(bad(format!("theme.{appearance}.{name} must use #RRGGBBAA")));
                }
            }
        }

        validate_label_colors("grid.ui", &self.grid.ui.label)?;
        validate_optional_color(
            "grid.ui.matched_background_color",
            self.grid.ui.matched_background_color.as_ref(),
        )?;
        validate_optional_color(
            "grid.ui.matched_border_color",
            self.grid.ui.matched_border_color.as_ref(),
        )?;
        validate_label_colors("recursive_grid.ui", &self.recursive_grid.ui.label)?;
        if self.recursive_grid.ui.label_min_font_size <= 0 {
            return Err(bad(
                "recursive_grid.ui.label_min_font_size must be positive".into(),
            ));
        }
        for (name, multiplier) in [
            (
                "label_autohide_multiplier",
                self.recursive_grid.ui.label_autohide_multiplier,
            ),
            (
                "sub_key_preview_autohide_multiplier",
                self.recursive_grid.ui.sub_key_preview_autohide_multiplier,
            ),
        ] {
            if !multiplier.is_finite() || multiplier < 0.0 {
                return Err(bad(format!(
                    "recursive_grid.ui.{name} must be finite and non-negative"
                )));
            }
        }
        for (name, value) in [
            ("line_color", self.recursive_grid.ui.line_color.as_ref()),
            (
                "highlight_color",
                self.recursive_grid.ui.highlight_color.as_ref(),
            ),
            (
                "label_background_color",
                self.recursive_grid.ui.label_background_color.as_ref(),
            ),
            (
                "sub_key_preview_text_color",
                self.recursive_grid.ui.sub_key_preview_text_color.as_ref(),
            ),
        ] {
            validate_optional_color(&format!("recursive_grid.ui.{name}"), value)?;
        }
        validate_label_colors("ui_hint.ui", &self.ui_hint.ui)?;
        validate_optional_color(
            "ui_hint.boundary_highlight.background_color",
            self.ui_hint.boundary_highlight.background_color.as_ref(),
        )?;
        validate_optional_color(
            "ui_hint.boundary_highlight.border_color",
            self.ui_hint.boundary_highlight.border_color.as_ref(),
        )?;
        validate_label_colors(
            "ui_hint.search_input_ui",
            &self.ui_hint.search_input_ui.label,
        )?;
        validate_label_colors("mode_indicator.ui", &self.mode_indicator.ui.label)?;
        for (name, value) in [
            ("fill_color", self.mode_indicator.cursor.fill_color.as_ref()),
            (
                "stroke_color",
                self.mode_indicator.cursor.stroke_color.as_ref(),
            ),
            (
                "left_pressed_color",
                self.mode_indicator.cursor.left_pressed_color.as_ref(),
            ),
            (
                "middle_pressed_color",
                self.mode_indicator.cursor.middle_pressed_color.as_ref(),
            ),
            (
                "right_pressed_color",
                self.mode_indicator.cursor.right_pressed_color.as_ref(),
            ),
        ] {
            validate_optional_color(&format!("mode_indicator.cursor.{name}"), value)?;
        }
        if self.mode_indicator.cursor.radius <= 0 || self.mode_indicator.cursor.stroke_width < 0 {
            return Err(bad(
                "mode_indicator.cursor radius must be positive and stroke_width non-negative"
                    .into(),
            ));
        }
        for (mode, entry) in &self.mode_indicator.modes {
            for (name, value) in [
                ("cursor.fill_color", entry.cursor.fill_color.as_ref()),
                ("cursor.stroke_color", entry.cursor.stroke_color.as_ref()),
                (
                    "cursor.left_pressed_color",
                    entry.cursor.left_pressed_color.as_ref(),
                ),
                (
                    "cursor.middle_pressed_color",
                    entry.cursor.middle_pressed_color.as_ref(),
                ),
                (
                    "cursor.right_pressed_color",
                    entry.cursor.right_pressed_color.as_ref(),
                ),
                ("ui.background_color", entry.ui.background_color.as_ref()),
                ("ui.text_color", entry.ui.text_color.as_ref()),
                (
                    "ui.matched_text_color",
                    entry.ui.matched_text_color.as_ref(),
                ),
                ("ui.border_color", entry.ui.border_color.as_ref()),
            ] {
                validate_optional_color(&format!("mode_indicator.modes.{mode}.{name}"), value)?;
            }
            if entry.cursor.radius.is_some_and(|value| value <= 0)
                || entry.cursor.stroke_width.is_some_and(|value| value < 0)
            {
                return Err(bad(format!(
                    "mode_indicator.modes.{mode}.cursor has invalid dimensions"
                )));
            }
        }

        if self.ui_hint.enabled {
            if self.ui_hint.hint_characters.chars().count() < 2 {
                return Err(bad(
                    "ui_hint.hint_characters needs at least 2 characters".into()
                ));
            }
            let mut canonical = BTreeSet::new();
            for character in self.ui_hint.hint_characters.chars() {
                let key = Key::new(character.to_string()).map_err(|error| {
                    bad(format!(
                        "ui_hint.hint_characters contains an invalid key: {error}"
                    ))
                })?;
                let Some(character) = key.as_char() else {
                    return Err(bad(format!(
                        "ui_hint.hint_characters contains non-character key `{character}`"
                    )));
                };
                if !canonical.insert(character) {
                    return Err(bad(format!(
                        "ui_hint.hint_characters contains duplicate canonical key `{character}`"
                    )));
                }
            }
        }
        if !(250..=30_000).contains(&self.ui_hint.scan_timeout_ms) {
            return Err(bad("ui_hint.scan_timeout_ms must be 250..=30000".into()));
        }
        if self.ui_hint.scan_retry_count > 5 {
            return Err(bad("ui_hint.scan_retry_count must be 0..=5".into()));
        }
        if self.ui_hint.scan_retry_delay_ms > 5_000 {
            return Err(bad("ui_hint.scan_retry_delay_ms must be 0..=5000".into()));
        }
        let overlap_cycle_key = Key::new(&self.ui_hint.overlap_cycle_key).map_err(|error| {
            bad(format!(
                "ui_hint.overlap_cycle_key contains an invalid key: {error}"
            ))
        })?;
        if !overlap_cycle_key.is_modifier() {
            return Err(bad(
                "ui_hint.overlap_cycle_key must be a modifier key".into()
            ));
        }
        let vision = &self.ui_hint.vision;
        if !vision.detect_text && !vision.detect_rectangles {
            return Err(bad(
                "ui_hint.vision must enable detect_text or detect_rectangles".into(),
            ));
        }
        if !(1..=30_000).contains(&vision.request_timeout_ms) {
            return Err(bad(
                "ui_hint.vision.request_timeout_ms must be 1..=30000".into()
            ));
        }
        if !(1..=2_000).contains(&vision.rectangle_max_candidates) {
            return Err(bad(
                "ui_hint.vision.rectangle_max_candidates must be 1..=2000".into(),
            ));
        }
        for (name, value) in [
            ("minimum_confidence", vision.minimum_confidence),
            ("merge_iou_threshold", vision.merge_iou_threshold),
            ("rectangle_min_size", vision.rectangle_min_size),
            ("button_min_confidence", vision.button_min_confidence),
            (
                "generic_clickable_min_confidence",
                vision.generic_clickable_min_confidence,
            ),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(bad(format!(
                    "ui_hint.vision.{name} must be between 0 and 1"
                )));
            }
        }
        for (name, value) in [
            ("rectangle_min_aspect", vision.rectangle_min_aspect),
            ("rectangle_max_aspect", vision.rectangle_max_aspect),
            ("button_min_aspect", vision.button_min_aspect),
            ("button_max_aspect", vision.button_max_aspect),
            ("button_icon_max_size", vision.button_icon_max_size),
            ("link_min_aspect", vision.link_min_aspect),
            ("link_max_height", vision.link_max_height),
            ("link_min_width", vision.link_min_width),
            ("image_min_size", vision.image_min_size),
            ("checkbox_max_size", vision.checkbox_max_size),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(bad(format!("ui_hint.vision.{name} must be positive")));
            }
        }
        if vision.rectangle_min_aspect > vision.rectangle_max_aspect
            || vision.button_min_aspect > vision.button_max_aspect
        {
            return Err(bad(
                "ui_hint.vision minimum aspect ratios must not exceed maximums".into(),
            ));
        }
        for (mode, keys) in [
            ("grid", &self.grid.temporary_mode_keys),
            ("recursive_grid", &self.recursive_grid.temporary_mode_keys),
            ("ui_hint", &self.ui_hint.temporary_mode_keys),
        ] {
            for key in keys {
                let key = Key::new(key).map_err(|error| {
                    bad(format!(
                        "{mode}.temporary_mode_keys contains an invalid key: {error}"
                    ))
                })?;
                if !key.is_modifier() {
                    return Err(bad(format!(
                        "{mode}.temporary_mode_keys may contain only modifier keys"
                    )));
                }
            }
        }
        if self.grid.enabled {
            let grid = &self.grid;
            let cells = (grid.grid_cols as usize) * (grid.grid_rows as usize);
            if grid.grid_cols == 0 || grid.grid_rows == 0 || cells < 2 {
                return Err(bad(
                    "grid needs at least 2 cells (grid_cols * grid_rows)".into()
                ));
            }
            if grid.keys.chars().count() != cells {
                return Err(bad(format!(
                    "grid.keys has {} characters but the {}x{} grid needs {cells}",
                    grid.keys.chars().count(),
                    grid.grid_cols,
                    grid.grid_rows,
                )));
            }
            if grid
                .keys
                .chars()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != cells
            {
                return Err(bad("grid.keys must not contain duplicate characters".into()));
            }
            if !(1..=20).contains(&grid.max_depth) {
                return Err(bad("grid.max_depth must be 1..=20".into()));
            }
        }

        if self.recursive_grid.enabled {
            let rg = &self.recursive_grid;
            let cells = (rg.grid_cols as usize) * (rg.grid_rows as usize);
            if rg.grid_cols == 0 || rg.grid_rows == 0 || cells < 2 {
                return Err(bad(
                    "recursive_grid needs at least 2 cells (grid_cols * grid_rows)".into(),
                ));
            }
            if rg.keys.chars().count() != cells {
                return Err(bad(format!(
                    "recursive_grid.keys has {} characters but the {}x{} grid needs {cells}",
                    rg.keys.chars().count(),
                    rg.grid_cols,
                    rg.grid_rows,
                )));
            }
            if rg
                .keys
                .chars()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != cells
            {
                return Err(bad(
                    "recursive_grid.keys must not contain duplicate characters".into(),
                ));
            }
            if !(1..=20).contains(&rg.max_depth) {
                return Err(bad("recursive_grid.max_depth must be 1..=20".into()));
            }
            for layer in &rg.layers {
                let cols = layer.grid_cols.unwrap_or(rg.grid_cols) as usize;
                let rows = layer.grid_rows.unwrap_or(rg.grid_rows) as usize;
                if cols == 0 || rows == 0 || cols * rows < 2 {
                    return Err(bad(format!(
                        "recursive_grid.layers[depth={}] needs at least 2 cells",
                        layer.depth
                    )));
                }
                if let Some(keys) = &layer.keys
                    && keys.chars().count() != cols * rows
                {
                    return Err(bad(format!(
                        "recursive_grid.layers[depth={}].keys has {} characters but needs {}",
                        layer.depth,
                        keys.chars().count(),
                        cols * rows,
                    )));
                }
            }
        }

        for (mode, lifecycle) in [
            ("grid", &self.grid.lifecycle),
            ("recursive_grid", &self.recursive_grid.lifecycle),
            ("ui_hint", &self.ui_hint.lifecycle),
        ] {
            if lifecycle.after_finish == LifecycleAction::Finish {
                return Err(bad(format!(
                    "{mode}.lifecycle.after_finish cannot trigger finish recursively"
                )));
            }
            if matches!(lifecycle.after_click, LifecycleAction::Click { .. }) {
                return Err(bad(format!(
                    "{mode}.lifecycle.after_click cannot trigger another click"
                )));
            }
            for (field, action) in [
                ("after_finish", &lifecycle.after_finish),
                ("after_click", &lifecycle.after_click),
            ] {
                if let LifecycleAction::Mode(target) = action {
                    let known = ModeId::BUILT_IN.contains(&target.as_str())
                        || self.plugin_modes.contains_key(target.as_str());
                    if !known {
                        return Err(bad(format!(
                            "{mode}.lifecycle.{field} names unknown mode {:?}",
                            target.as_str()
                        )));
                    }
                }
                if action.canonical() == "unsupported_click" {
                    return Err(bad(format!(
                        "{mode}.lifecycle.{field} contains an unsupported click"
                    )));
                }
            }
        }

        // Every binding chord must parse, or it would silently never fire.
        for (label, table) in self.binding_tables() {
            for chord in table.keys() {
                KeyChord::parse(chord).map_err(|e| {
                    bad(format!(
                        "{label} binding {chord:?} is not a valid chord: {e}"
                    ))
                })?;
            }
        }

        validate_inheritance(self).map_err(bad)?;

        if self.pointer.max_speed <= 0.0 {
            return Err(bad("pointer.max_speed must be positive".into()));
        }
        if self.pointer.initial_speed <= 0.0 {
            return Err(bad("pointer.initial_speed must be positive".into()));
        }
        if self.pointer.acceleration < 0.0 {
            return Err(bad("pointer.acceleration must not be negative".into()));
        }
        if self.pointer.tap_distance < 0.0 {
            return Err(bad("pointer.tap_distance must not be negative".into()));
        }
        if self.pointer.precision_multiplier <= 0.0
            || self.pointer.slow_multiplier <= 0.0
            || self.pointer.fast_multiplier <= 0.0
        {
            return Err(bad(
                "pointer precision/slow/fast multipliers must be positive".into(),
            ));
        }

        Ok(())
    }

    /// Every binding table with a label for error messages.
    fn binding_tables(&self) -> Vec<(String, &Bindings)> {
        let mut tables: Vec<(String, &Bindings)> = vec![
            ("[hotkeys]".into(), &self.hotkeys),
            ("[normal.bindings]".into(), &self.normal.bindings),
            ("[grid.bindings]".into(), &self.grid.bindings),
            (
                "[recursive_grid.bindings]".into(),
                &self.recursive_grid.bindings,
            ),
            ("[ui_hint.bindings]".into(), &self.ui_hint.bindings),
        ];
        for (id, mode) in &self.plugin_modes {
            tables.push((format!("[plugin_modes.{id:?}.bindings]"), &mode.bindings));
        }
        for over in &self.app_configs {
            tables.push((
                format!("[[app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.normal.app_configs {
            tables.push((
                format!("[[normal.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.grid.app_configs {
            tables.push((
                format!("[[grid.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.recursive_grid.app_configs {
            tables.push((
                format!("[[recursive_grid.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for over in &self.ui_hint.app_configs {
            tables.push((
                format!("[[ui_hint.app_configs]] {:?}", over.bundle_id),
                &over.bindings,
            ));
        }
        for (id, mode) in &self.plugin_modes {
            for over in &mode.app_configs {
                tables.push((
                    format!("[[plugin_modes.{id:?}.app_configs]] {:?}", over.bundle_id),
                    &over.bindings,
                ));
            }
        }
        tables
    }
}

fn validate_inheritance(config: &Config) -> Result<(), String> {
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::from([
        ("normal".into(), config.normal.inherits.clone()),
        ("grid".into(), config.grid.inherits.clone()),
        (
            "recursive_grid".into(),
            config.recursive_grid.inherits.clone(),
        ),
        ("ui_hint".into(), config.ui_hint.inherits.clone()),
    ]);
    for (id, mode) in &config.plugin_modes {
        graph.insert(id.clone(), mode.inherits.clone());
    }
    let known = |name: &str| name == "hotkeys" || graph.contains_key(name);
    for (mode, sources) in &graph {
        for source in sources {
            if !known(source) {
                return Err(format!(
                    "{mode}.inherits contains unknown source {source:?}"
                ));
            }
        }
    }
    for (mode, source) in [
        ("grid", config.grid.temporary_mode.as_deref()),
        (
            "recursive_grid",
            config.recursive_grid.temporary_mode.as_deref(),
        ),
        ("ui_hint", config.ui_hint.temporary_mode.as_deref()),
    ] {
        if let Some(source) = source
            && !known(source)
        {
            return Err(format!(
                "{mode}.temporary_mode names unknown mode {source:?}"
            ));
        }
    }

    fn visit(
        mode: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut std::collections::BTreeSet<String>,
        done: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        if done.contains(mode) || mode == "hotkeys" {
            return Ok(());
        }
        if !visiting.insert(mode.to_string()) {
            return Err(format!("inheritance cycle contains {mode:?}"));
        }
        if let Some(sources) = graph.get(mode) {
            for source in sources {
                visit(source, graph, visiting, done)?;
            }
        }
        visiting.remove(mode);
        done.insert(mode.to_string());
        Ok(())
    }

    let mut done = std::collections::BTreeSet::new();
    for mode in graph.keys() {
        visit(
            mode,
            &graph,
            &mut std::collections::BTreeSet::new(),
            &mut done,
        )?;
    }
    Ok(())
}
