//! The only assembly point for built-in modes.
//!
//! Configuration DTOs stop here. Each mode receives only its own immutable,
//! strongly typed settings and remains unaware of TOML or the application
//! configuration document.

use crate::api::{Mode, Plugin};
use crate::config::Config;
use crate::modes::{self, GridMode, HintMode, IdleMode, NormalMode, RecursiveGridMode};
use crate::plugins::BundledSettings;

pub(crate) fn normal_settings(config: &Config) -> modes::normal::Settings {
    modes::normal::Settings {
        pointer: modes::normal::PointerSettings {
            initial_speed: config.pointer.initial_speed,
            max_speed: config.pointer.max_speed,
            acceleration: config.pointer.acceleration,
            smooth_acceleration: config.pointer.smooth_acceleration,
            tap_distance: config.pointer.tap_distance,
            slow_multiplier: config.pointer.slow_multiplier,
            precision_multiplier: config.pointer.precision_multiplier,
            fast_multiplier: config.pointer.fast_multiplier,
        },
        scroll: modes::normal::ScrollSettings {
            scroll_step: config.scroll.scroll_step,
            scroll_step_half: config.scroll.scroll_step_half,
            scroll_step_full: config.scroll.scroll_step_full,
        },
        passthrough_unbound_keys: config.normal.passthrough_unbound_keys,
    }
}

pub(crate) fn grid_settings(config: &Config) -> modes::grid::Settings {
    modes::grid::Settings {
        grid_cols: config.grid.grid_cols,
        grid_rows: config.grid.grid_rows,
        keys: config.grid.keys.clone(),
        max_depth: config.grid.max_depth,
        cursor_follow_selection: config.grid.cursor_follow_selection,
        lifecycle: config.grid.lifecycle.clone(),
        ui: modes::grid::VisualSettings {
            label: config.grid.ui.label.clone(),
            matched_background_color: config.grid.ui.matched_background_color.clone(),
            matched_border_color: config.grid.ui.matched_border_color.clone(),
        },
    }
}

pub(crate) fn recursive_grid_settings(config: &Config) -> modes::recursive_grid::Settings {
    modes::recursive_grid::Settings {
        grid_cols: config.recursive_grid.grid_cols,
        grid_rows: config.recursive_grid.grid_rows,
        keys: config.recursive_grid.keys.clone(),
        min_size_width: config.recursive_grid.min_size_width,
        min_size_height: config.recursive_grid.min_size_height,
        max_depth: config.recursive_grid.max_depth,
        cursor_follow_selection: config.recursive_grid.cursor_follow_selection,
        lifecycle: config.recursive_grid.lifecycle.clone(),
        layers: config
            .recursive_grid
            .layers
            .iter()
            .map(|layer| modes::recursive_grid::LayerSettings {
                depth: layer.depth,
                grid_cols: layer.grid_cols,
                grid_rows: layer.grid_rows,
                keys: layer.keys.clone(),
            })
            .collect(),
        ui: modes::recursive_grid::VisualSettings {
            label: config.recursive_grid.ui.label.clone(),
            line_width: config.recursive_grid.ui.line_width,
            line_color: config.recursive_grid.ui.line_color.clone(),
            highlight_color: config.recursive_grid.ui.highlight_color.clone(),
            label_background: config.recursive_grid.ui.label_background,
            label_background_color: config.recursive_grid.ui.label_background_color.clone(),
            label_char: config.recursive_grid.ui.label_char.clone(),
            label_min_font_size: config.recursive_grid.ui.label_min_font_size,
            label_autohide_multiplier: config.recursive_grid.ui.label_autohide_multiplier,
            sub_key_preview: config.recursive_grid.ui.sub_key_preview,
            sub_key_preview_font_size: config.recursive_grid.ui.sub_key_preview_font_size,
            sub_key_preview_text_color: config.recursive_grid.ui.sub_key_preview_text_color.clone(),
            sub_key_preview_autohide_multiplier: config
                .recursive_grid
                .ui
                .sub_key_preview_autohide_multiplier,
        },
    }
}

pub(crate) fn hint_settings(config: &Config) -> modes::hint::Settings {
    modes::hint::Settings {
        strategy: config.ui_hint.strategy,
        vision: config.ui_hint.vision.clone(),
        hint_characters: config.ui_hint.hint_characters.clone(),
        label_direction: config.ui_hint.label_direction,
        max_depth: config.ui_hint.max_depth,
        scan_timeout_ms: config.ui_hint.scan_timeout_ms,
        scan_retry_count: config.ui_hint.scan_retry_count,
        scan_retry_delay_ms: config.ui_hint.scan_retry_delay_ms,
        clickable_roles: config.ui_hint.clickable_roles.clone(),
        ignore_clickable_check: config.ui_hint.ignore_clickable_check,
        visible_check_enabled: config.ui_hint.visible_check_enabled,
        placement: config.ui_hint.placement,
        label_x_offset: config.ui_hint.label_x_offset,
        label_y_offset: config.ui_hint.label_y_offset,
        ui: config.ui_hint.ui.clone(),
        boundary_highlight: config.ui_hint.boundary_highlight.clone(),
        search_input_ui: config.ui_hint.search_input_ui.clone(),
        lifecycle: config.ui_hint.lifecycle.clone(),
        overlap_cycle_key: config.ui_hint.overlap_cycle_key.clone(),
        app_overrides: config
            .ui_hint
            .app_configs
            .iter()
            .map(|entry| modes::hint::AppStrategyOverride {
                pattern: entry.bundle_id.clone(),
                strategy: entry.strategy,
            })
            .collect(),
    }
}

/// Instantiate the built-in modes enabled by a validated configuration.
#[doc(hidden)]
pub fn normal(config: &Config) -> NormalMode {
    NormalMode::new(normal_settings(config))
}

#[doc(hidden)]
pub fn grid(config: &Config) -> GridMode {
    GridMode::new(grid_settings(config))
}

#[doc(hidden)]
pub fn recursive_grid(config: &Config) -> RecursiveGridMode {
    RecursiveGridMode::new(recursive_grid_settings(config))
}

#[doc(hidden)]
pub fn hint(config: &Config) -> HintMode {
    HintMode::new(hint_settings(config))
}

#[doc(hidden)]
pub fn built_in(config: &Config) -> Vec<Box<dyn Mode>> {
    let mut catalog: Vec<Box<dyn Mode>> = vec![Box::new(IdleMode::new()), Box::new(normal(config))];
    if config.grid.enabled {
        catalog.push(Box::new(grid(config)));
    }
    if config.recursive_grid.enabled {
        catalog.push(Box::new(recursive_grid(config)));
    }
    if config.ui_hint.enabled {
        catalog.push(Box::new(hint(config)));
    }
    catalog
}

pub(crate) fn bundled_plugin_settings(config: &Config) -> BundledSettings {
    BundledSettings {
        key_aliases: config.resolved_key_aliases().clone(),
        screen_selector_preserve: config
            .plugin_setting_bool("plugin:screen-selector", "preserve")
            .unwrap_or(true),
    }
}

#[doc(hidden)]
pub fn bundled_plugins(config: &Config) -> Result<Vec<Box<dyn Plugin>>, String> {
    crate::plugins::bundled(bundled_plugin_settings(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModeId;
    use std::collections::BTreeSet;

    #[test]
    fn default_catalog_has_unique_ids_and_all_built_ins() {
        let modes = built_in(&Config::default());
        let ids: Vec<_> = modes.iter().map(|mode| mode.id()).collect();
        let unique: BTreeSet<_> = ids.iter().cloned().collect();
        assert_eq!(ids.len(), unique.len());
        for expected in [
            ModeId::idle(),
            ModeId::normal(),
            ModeId::grid(),
            ModeId::recursive_grid(),
            ModeId::ui_hint(),
        ] {
            assert!(unique.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn enable_flags_only_remove_optional_modes() {
        let mut config = Config::default();
        config.grid.enabled = false;
        config.recursive_grid.enabled = false;
        config.ui_hint.enabled = false;
        let ids: Vec<_> = built_in(&config).iter().map(|mode| mode.id()).collect();
        assert_eq!(ids, vec![ModeId::idle(), ModeId::normal()]);
    }

    #[test]
    fn default_capture_policy_is_owned_by_each_mode() {
        for mode in built_in(&Config::default()) {
            let expected = !matches!(mode.id().as_str(), "idle" | "normal");
            assert_eq!(mode.captures_keyboard(), expected, "{}", mode.id());
        }
    }
}
