#![cfg(test)]

use super::*;
use crate::api::binding::{Button, Direction, ScrollAmount, Speed};
use crate::api::{ButtonAction, ModeId, MouseButton, VisionOptions};

#[test]
fn window_split_ratios_validate_and_round_trip() {
    let config =
        Config::parse("[window_quick]\nsplit_ratios = [\"1/5\", \"2/5\", \"3/5\", \"4/5\"]")
            .unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.window_quick.parsed_split_ratios().unwrap(),
        vec![0.2, 0.4, 0.6, 0.8]
    );
    let reparsed = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(
        reparsed.window_quick.split_ratios,
        config.window_quick.split_ratios
    );
    assert_eq!(
        Config::parse("")
            .unwrap()
            .window_quick
            .parsed_split_ratios()
            .unwrap(),
        crate::api::window_layout::DEFAULT_SPLIT_RATIOS
    );
    for values in [
        "[]",
        "[\"0/4\"]",
        "[\"1/0\"]",
        "[\"1/1\"]",
        "[0.0]",
        "[1.0]",
        "[-0.1]",
        "[nan]",
        "[inf]",
        "[\"0.5\"]",
        "[\"nan\"]",
        "[\"1/4294967296\"]",
    ] {
        let config = Config::parse(&format!("[window_quick]\nsplit_ratios = {values}")).unwrap();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("window_quick.split_ratios"),
            "{values}"
        );
    }
}

#[test]
fn window_split_ratios_sort_and_deduplicate_mixed_inputs_without_rewriting_source() {
    for (source, expected) in [
        (
            "[\"3/4\", 0.3, \"1/2\", 0.4, \"2/4\", 0.5]",
            vec![0.3, 0.4, 0.5, 0.75],
        ),
        ("[0.4, 0.3, 0.4]", vec![0.3, 0.4]),
        ("[\"3/5\", \"1/5\", \"2/10\"]", vec![0.2, 0.6]),
    ] {
        let config = Config::parse(&format!("[window_quick]\nsplit_ratios = {source}")).unwrap();
        config.validate().unwrap();
        assert_eq!(config.window_quick.parsed_split_ratios().unwrap(), expected);
        let reparsed = Config::parse(&config.to_toml().unwrap()).unwrap();
        assert_eq!(
            config.window_quick.split_ratios,
            reparsed.window_quick.split_ratios
        );
    }
}

#[test]
fn portable_config_names_require_a_profile() {
    assert!(Config::is_portable_config_name(
        "keysteer.user.toml".as_ref()
    ));
    assert!(Config::is_portable_config_name(
        "KEYSTEER.Work.TOML".as_ref()
    ));
    assert!(!Config::is_portable_config_name("keysteer.toml".as_ref()));
    assert!(!Config::is_portable_config_name("keysteer..toml".as_ref()));
    assert!(!Config::is_portable_config_name("config.toml".as_ref()));
}

#[test]
fn default_write_path_uses_the_application_data_directory() {
    let expected = crate::app::paths::data_file("keysteer.user.toml").unwrap();
    assert_eq!(Config::default_write_path(), Some(expected));
}

#[test]
fn portable_discovery_is_filtered_and_deterministic() {
    let directory = std::env::temp_dir().join(format!(
        "keysteer-config-discovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    for name in [
        "keysteer.zebra.toml",
        "keysteer.Alpha.toml",
        "KEYSTEER.DEFAULT.TOML",
        "keysteer..toml",
        "config.toml",
    ] {
        std::fs::write(directory.join(name), "").unwrap();
    }

    assert_eq!(
        Config::discover_in(&directory).unwrap(),
        Some(directory.join("keysteer.Alpha.toml"))
    );

    std::fs::remove_file(directory.join("keysteer.Alpha.toml")).unwrap();
    std::fs::remove_file(directory.join("keysteer.zebra.toml")).unwrap();
    assert_eq!(
        Config::discover_in(&directory).unwrap(),
        Some(directory.join("KEYSTEER.DEFAULT.TOML")),
        "the annotated default should remain a fallback when no user profile exists"
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn user_key_alias_can_override_primary_with_one_physical_side() {
    let config = Config::parse(
        r#"
            [key_aliases]
            Primary = "left_alt"

            [hotkeys]
            "Primary+e" = "normal"
            "alt+f" = "grid"
            "right_alt+g" = "recursive_grid"
            "#,
    )
    .unwrap();

    assert!(config.hotkeys.contains_key("left_alt+e"));
    assert!(config.hotkeys.contains_key("alt+f"));
    assert!(config.hotkeys.contains_key("right_alt+g"));
    assert_eq!(config.resolved_key_aliases()["primary"], "left_alt");
}

#[test]
fn current_platform_aliases_override_global_aliases_only_here() {
    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let text = format!(
        r#"
            [key_aliases]
            Primary = "alt"
            Hyper = "right_ctrl"

            [key_aliases.{platform}]
            Primary = "left_shift"

            [hotkeys]
            "Primary+e" = "normal"
            "Hyper+g" = "grid"
            "#
    );
    let config = Config::parse(&text).unwrap();

    assert!(config.hotkeys.contains_key("left_shift+e"));
    assert!(config.hotkeys.contains_key("right_ctrl+g"));
    assert_eq!(config.resolved_key_aliases()["primary"], "left_shift");
}

#[test]
fn inactive_platform_aliases_do_not_change_this_platform() {
    let inactive = if cfg!(target_os = "windows") {
        "macos"
    } else {
        "windows"
    };
    let text = format!(
        r#"
            [key_aliases]
            Primary = "alt"

            [key_aliases.{inactive}]
            Primary = "left_shift"

            [hotkeys]
            "Primary+e" = "normal"
            "#
    );
    let config = Config::parse(&text).unwrap();

    assert!(config.hotkeys.contains_key("alt+e"));
    assert!(!config.hotkeys.contains_key("left_shift+e"));
}

#[test]
fn platform_key_aliases_round_trip_through_toml() {
    let config = Config::parse(
        r#"
            [key_aliases]
            Hyper = "right_ctrl"

            [key_aliases.windows]
            Primary = "left_alt"

            [key_aliases.macos]
            Primary = "left_cmd"
            "#,
    )
    .unwrap();
    let reparsed = Config::parse(&config.to_toml().unwrap()).unwrap();

    assert_eq!(reparsed.key_aliases, config.key_aliases);
    assert_eq!(
        reparsed.resolved_key_aliases(),
        config.resolved_key_aliases()
    );
}

#[test]
fn custom_key_aliases_chain_and_apply_to_send_actions() {
    let config = Config::parse(
        r#"
            [key_aliases]
            Primary = "Hyper"
            Hyper = "right_ctrl"

            [normal.bindings]
            h = "send Primary+x"
            "#,
    )
    .unwrap();

    match &config.normal.bindings["h"] {
        Binding::Send(chord) => assert_eq!(chord.canonical(), "right_ctrl+x"),
        other => panic!("expected send binding, got {other:?}"),
    }
}

#[test]
fn invalid_key_aliases_are_rejected() {
    for text in [
        "[key_aliases]\nPrimary = 'missing_key'",
        "[key_aliases]\nPrimary = 'left_alt+right_alt'",
        "[key_aliases]\nPrimary = 'Hyper'\nHyper = 'Primary'",
    ] {
        assert!(Config::parse(text).is_err(), "{text}");
    }
}

#[test]
fn empty_config_is_valid_and_equals_the_defaults() {
    let config = Config::parse("").unwrap();
    assert_eq!(config, Config::default());
    config.validate().unwrap();
}

#[test]
fn partial_config_keeps_defaults_for_omitted_fields() {
    let config = Config::parse(
        r#"
            [scroll]
            scroll_step = 25
            "#,
    )
    .unwrap();
    assert_eq!(config.scroll.scroll_step, 25);
    // Untouched fields keep their documented defaults.
    assert_eq!(config.scroll.scroll_step_half, 500);
    assert_eq!(config.grid.keys, Grid::default().keys);
}

#[test]
fn pointer_smooth_acceleration_defaults_on_and_can_be_disabled() {
    let defaults = Config::default().pointer;
    assert_eq!(defaults.initial_speed, 1000.0);
    assert_eq!(defaults.max_speed, 2200.0);
    assert_eq!(defaults.acceleration, 3000.0);
    assert!(defaults.smooth_acceleration);

    let config = Config::parse("[pointer]\nsmooth_acceleration = false").unwrap();
    assert!(!config.pointer.smooth_acceleration);
    assert!(
        config
            .to_toml()
            .unwrap()
            .contains("smooth_acceleration = false")
    );
}

#[test]
fn macos_scroll_axes_have_independent_defaults() {
    assert_eq!(Config::default().macos_scroll_invert(), (false, true));
    let config = Config::parse(
        r#"
            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = false
            "#,
    )
    .unwrap();
    assert_eq!(config.macos_scroll_invert(), (true, false));
    assert!(config.deprecation_warnings().is_empty());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_ignores_macos_and_legacy_scroll_inversion() {
    let config = Config::parse(
        r#"
            [scroll]
            invert_scroll = true

            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = true
            "#,
    )
    .unwrap();

    assert_eq!(config.effective_scroll_invert(), (false, false));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_effective_scroll_uses_macos_axis_settings() {
    let config = Config::parse(
        r#"
            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = false
            "#,
    )
    .unwrap();

    assert_eq!(config.effective_scroll_invert(), (true, false));
}

#[test]
fn all_macos_scroll_axis_combinations_are_preserved() {
    for horizontal in [false, true] {
        for vertical in [false, true] {
            let mut config = Config::default();
            config.platform.macos.scroll.invert_horizontal = Some(horizontal);
            config.platform.macos.scroll.invert_vertical = Some(vertical);
            assert_eq!(config.macos_scroll_invert(), (horizontal, vertical));
        }
    }
}

#[test]
fn explicit_axis_settings_override_legacy_values() {
    let config = Config::parse(
        r#"
            [scroll]
            invert_scroll = true

            [platform.macos.scroll]
            invert_horizontal = false
            invert_vertical = true
            "#,
    )
    .unwrap();
    assert_eq!(config.macos_scroll_invert(), (false, true));
    assert_eq!(config.deprecation_warnings().len(), 1);
}

#[test]
fn exported_legacy_scroll_setting_is_migrated_to_both_axes() {
    let config = Config::parse(
        r#"
            [platform.macos.scroll]
            invert = true
            "#,
    )
    .unwrap();
    let exported = config.to_toml().unwrap();
    assert!(!exported.contains("invert ="), "{exported}");
    let reparsed = Config::parse(&exported).unwrap();
    assert_eq!(reparsed.platform.macos.scroll.invert_horizontal, Some(true));
    assert_eq!(reparsed.platform.macos.scroll.invert_vertical, Some(true));
    assert_eq!(reparsed.macos_scroll_invert(), (true, true));
}

#[test]
fn idle_binds_only_mode_launchers() {
    let config = Config::default();
    // The silence guarantee: idle does nothing except launch a mode.
    assert!(
        config.hotkeys.values().all(|b| b.mode().is_some()),
        "idle should only enter modes, got {:?}",
        config.hotkeys
    );
    // And `normal` must be among them, or the program is unreachable.
    assert_eq!(config.hotkeys.len(), 2);
    assert!(
        config
            .hotkeys
            .values()
            .any(|b| b.mode() == Some(&ModeId::normal()))
    );
}

#[test]
fn idle_launchers_resolve_the_platform_neutral_primary_modifier() {
    // The source default uses `primary`; runtime tables contain its
    // concrete key so matching never needs to resolve aliases again.
    let config = Config::default();
    for chord in config.hotkeys.keys() {
        assert!(
            !chord.starts_with("primary+"),
            "unresolved launcher {chord:?}"
        );
        assert_eq!(KeyChord::parse(chord).unwrap().keys().len(), 2);
    }
}

#[test]
fn idle_launchers_avoid_platform_reserved_chords() {
    // Regression: `alt+e` never fired on macOS because Option+E is a
    // dead key. No default may have that problem on any platform.
    for chord in Config::default().hotkeys.keys() {
        let parsed = KeyChord::parse(chord).unwrap();
        assert_eq!(
            platform_warning(&parsed),
            None,
            "default idle binding {chord:?} is problematic on this platform"
        );
    }
}

#[test]
fn normal_defaults_avoid_platform_reserved_chords() {
    for chord in Config::default().normal.bindings.keys() {
        let parsed = KeyChord::parse(chord).unwrap();
        assert_eq!(
            platform_warning(&parsed),
            None,
            "default normal binding {chord:?} is problematic on this platform"
        );
    }
}

#[test]
fn normal_defaults_cover_the_requested_controls_and_targeting_modes() {
    let normal = &Config::default().normal.bindings;
    assert_eq!(normal["h"], Binding::Move(Direction::Left));
    assert_eq!(normal["j"], Binding::Move(Direction::Down));
    assert_eq!(normal["k"], Binding::Move(Direction::Up));
    assert_eq!(normal["l"], Binding::Move(Direction::Right));
    assert_eq!(normal["left_shift"], Binding::Speed(Speed::Slow));
    assert_eq!(normal["caps_lock"], Binding::Speed(Speed::Precision));
    assert_eq!(normal["v"], Binding::Speed(Speed::Fast));
    assert_eq!(normal["b"], Binding::Speed(Speed::Fast));
    assert!(!normal.contains_key("e"));
    assert!(!normal.contains_key("r"));
    assert_eq!(
        normal["m"],
        Binding::Scroll(Direction::Down, ScrollAmount::Step)
    );
    assert_eq!(
        normal[","],
        Binding::Scroll(Direction::Up, ScrollAmount::Step)
    );
    assert_eq!(normal[";"], Binding::Click(Button::Left));
    assert_eq!(normal["'"], Binding::Click(Button::Right));
    assert_eq!(normal["n"], Binding::Toggle(Vec::new()));
    assert_eq!(normal["g"], Binding::Mode(ModeId::grid()));
    assert_eq!(normal["f"], Binding::Mode(ModeId::recursive_grid()));
    assert!(
        normal
            .values()
            .any(|binding| binding == &Binding::Mode(ModeId::ui_hint()))
    );
    assert_eq!(normal["q"], Binding::Mode(ModeId::idle()));
}

#[test]
fn normal_long_press_toggle_threshold_is_configurable() {
    assert_eq!(Config::default().normal.long_press_toggle_ms, 500);

    let configured = Config::parse("[normal]\nlong_press_toggle_ms = 750").unwrap();
    assert_eq!(configured.normal.long_press_toggle_ms, 750);

    let disabled = Config::parse("[normal]\nlong_press_toggle_ms = 0").unwrap();
    assert_eq!(disabled.normal.long_press_toggle_ms, 0);

    let dumped = toml::to_string(&Config::default()).unwrap();
    assert!(dumped.contains("long_press_toggle_ms = 500"));

    let invalid = Config::parse("[normal]\nlong_press_toggle_ms = 60001").unwrap();
    let error = invalid.validate().unwrap_err();
    assert!(error.to_string().contains("normal.long_press_toggle_ms"));
}

#[test]
fn normal_auto_release_threshold_is_configurable() {
    assert_eq!(Config::default().normal.auto_release_ms, 0);

    let legacy = Config::parse("[normal]\nlong_press_toggle_ms = 750").unwrap();
    assert_eq!(legacy.normal.auto_release_ms, 0);

    let configured = Config::parse("[normal]\nauto_release_ms = 750").unwrap();
    configured.validate().unwrap();
    assert_eq!(configured.normal.auto_release_ms, 750);

    let dumped = toml::to_string(&Config::default()).unwrap();
    assert!(dumped.contains("auto_release_ms = 0"));

    let invalid = Config::parse("[normal]\nauto_release_ms = 60001").unwrap();
    let error = invalid.validate().unwrap_err();
    assert!(error.to_string().contains("normal.auto_release_ms"));
}

#[test]
fn normal_unbound_passthrough_defaults_on_and_round_trips() {
    assert!(Config::default().normal.passthrough_unbound_keys);

    let legacy = Config::parse("[normal]\nlong_press_toggle_ms = 750").unwrap();
    assert!(legacy.normal.passthrough_unbound_keys);

    let default_dumped = toml::to_string(&Config::default()).unwrap();
    assert!(default_dumped.contains("passthrough_unbound_keys = true"));

    let exclusive = Config::parse("[normal]\npassthrough_unbound_keys = false").unwrap();
    assert!(!exclusive.normal.passthrough_unbound_keys);

    let dumped = toml::to_string(&exclusive).unwrap();
    assert!(dumped.contains("passthrough_unbound_keys = false"));
    let reparsed = Config::parse(&dumped).unwrap();
    assert!(!reparsed.normal.passthrough_unbound_keys);
}

#[test]
fn navigation_keys_are_bound_as_synthetic_keystrokes() {
    let normal = &Config::default().normal.bindings;
    for (chord, key) in [
        ("u", "page_down"),
        ("i", "page_up"),
        ("t", "home"),
        ("y", "end"),
    ] {
        match &normal[chord] {
            Binding::Send(sent) => assert_eq!(sent.canonical(), key),
            other => panic!("{chord} should send {key}, got {other:?}"),
        }
    }
}

#[test]
fn every_mode_can_be_reached_from_the_defaults() {
    // Either directly from idle, or from normal.
    let config = Config::default();
    let reachable: Vec<&str> = config
        .hotkeys
        .values()
        .chain(config.normal.bindings.values())
        .filter_map(|b| b.mode())
        .map(|id| id.as_str())
        .collect();
    for mode in ["normal", "grid", "recursive_grid", "ui_hint"] {
        assert!(
            reachable.contains(&mode),
            "{mode} unreachable: {reachable:?}"
        );
    }
}

#[test]
fn grid_modes_bind_follow_like_every_other_mode_action() {
    let config = Config::parse(
        r#"
            [grid.bindings]
            "`" = "follow"

            [recursive_grid.bindings]
            "`" = "follow"
            "#,
    )
    .unwrap();
    assert_eq!(
        config.grid.bindings["`"],
        Binding::ToggleCursorFollowSelection
    );
    assert_eq!(
        config.recursive_grid.bindings["`"],
        Binding::ToggleCursorFollowSelection
    );
    config.validate().unwrap();
}

#[test]
fn recursive_grid_defaults_are_the_qweasdzxc_nine_cell_layout() {
    let grid = &Config::default().recursive_grid;
    assert_eq!((grid.grid_cols, grid.grid_rows), (3, 3));
    assert_eq!(grid.keys, "qweasdzxc");
    assert_eq!(grid.max_depth, 10);
    assert_eq!(grid.bindings["`"], Binding::ToggleCursorFollowSelection);
}

#[test]
fn bindings_need_no_action_prefix() {
    let config = Config::parse(
        r#"
            [normal.bindings]
            h = "move_left"
            g = "grid"
            t = "home"
            z = "plugin:screen-selector"
            "#,
    )
    .unwrap();
    let b = &config.normal.bindings;
    assert_eq!(b["h"], Binding::Move(Direction::Left));
    assert_eq!(b["g"], Binding::Mode(ModeId::grid()));
    assert!(matches!(b["t"], Binding::Send(_)));
    assert_eq!(
        b["z"],
        Binding::Mode(ModeId::new("plugin:screen-selector").unwrap())
    );
}

#[test]
fn whitespace_separates_multiple_single_key_binding_aliases() {
    let config = Config::parse(
        r#"
            [normal.bindings]
            "v b" = "fast"
            "#,
    )
    .unwrap();
    assert_eq!(config.normal.bindings["v"], Binding::Speed(Speed::Fast));
    assert_eq!(config.normal.bindings["b"], Binding::Speed(Speed::Fast));
    assert!(!config.normal.bindings.contains_key("v b"));
    config.validate().unwrap();
}

#[test]
fn grid_like_modes_do_not_steal_label_keys_with_exit_bindings() {
    let config = Config::default();
    for table in [
        &config.grid.bindings,
        &config.recursive_grid.bindings,
        &config.ui_hint.bindings,
    ] {
        assert!(!table.contains_key("q"));
        assert!(!table.contains_key("esc"));
        let exit = table
            .iter()
            .find(|(_, binding)| binding == &&Binding::Mode(ModeId::normal()))
            .map(|(chord, _)| chord)
            .expect("grid-like modes should expose a configurable exit");
        assert_eq!(
            KeyChord::parse(exit).unwrap().activation_key().as_str(),
            "q"
        );
    }
}

#[test]
fn temporary_mode_keys_must_be_modifiers() {
    let config = Config::parse("[grid]\ntemporary_mode_keys = [\"h\"]").unwrap();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("temporary_mode_keys"), "{error}");
}

#[test]
fn ui_hint_overlap_cycle_key_defaults_to_shift_and_is_configurable() {
    let default = Config::default();
    assert_eq!(default.ui_hint.overlap_cycle_key, "shift");
    assert!(
        default
            .ui_hint
            .overlap_cycle_matches(&Key::new("left_shift").unwrap())
    );
    assert!(
        default
            .ui_hint
            .overlap_cycle_matches(&Key::new("right_shift").unwrap())
    );

    let config = Config::parse("[ui_hint]\noverlap_cycle_key = \"option\"").unwrap();
    config.validate().unwrap();
    assert!(
        config
            .ui_hint
            .overlap_cycle_matches(&Key::new("left_alt").unwrap())
    );
    assert!(config.ui_hint.overlap_cycle_conflicts_with("alt"));
}

#[test]
fn ui_hint_scan_timeout_and_retry_are_configurable() {
    let defaults = Config::default();
    assert_eq!(defaults.ui_hint.scan_timeout_ms, 2_500);
    assert_eq!(defaults.ui_hint.scan_retry_count, 1);
    assert_eq!(defaults.ui_hint.scan_retry_delay_ms, 200);

    let config = Config::parse(
        r#"
            [ui_hint]
            scan_timeout_ms = 8000
            scan_retry_count = 3
            scan_retry_delay_ms = 500
            "#,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(config.ui_hint.scan_timeout_ms, 8_000);
    assert_eq!(config.ui_hint.scan_retry_count, 3);
    assert_eq!(config.ui_hint.scan_retry_delay_ms, 500);
}

#[test]
fn ui_hint_overlap_cycle_key_must_be_a_modifier() {
    let config = Config::parse("[ui_hint]\noverlap_cycle_key = \"b\"").unwrap();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("overlap_cycle_key"), "{error}");
}

#[test]
fn inheritance_rejects_unknown_sources_and_cycles() {
    let unknown = Config::parse("[grid]\ninherits = [\"missing\"]").unwrap();
    assert!(
        unknown
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unknown source")
    );

    let cycle =
        Config::parse("[normal]\ninherits = [\"grid\"]\n[grid]\ninherits = [\"normal\"]").unwrap();
    assert!(cycle.validate().unwrap_err().to_string().contains("cycle"));
}

#[test]
fn scroll_bindings_replace_the_old_scroll_mode() {
    let config = Config::parse(
        r#"
            [normal.bindings]
            e = "scroll_down"
            "alt+e" = "scroll_half_down"
            "#,
    )
    .unwrap();
    assert_eq!(
        config.normal.bindings["e"],
        Binding::Scroll(Direction::Down, ScrollAmount::Step)
    );
    config.validate().unwrap();
}

#[test]
fn scroll_is_rejected_as_a_mode_target() {
    // It used to be a mode; make the migration explicit rather than silent.
    let err = Config::parse(
        r#"
            [hotkeys]
            "alt+e" = "normal"
            "alt+s" = "scroll"
            "#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("unknown binding"), "{err}");
    assert!(err.contains("scroll"), "{err}");
}

#[test]
fn a_config_that_cannot_reach_normal_is_valid_but_diagnosable() {
    let config = Config::parse(
        r#"
            [hotkeys]
            "alt+g" = "grid"
            "#,
    )
    .unwrap();
    config.validate().unwrap();
}

#[test]
fn plugin_modes_get_a_binding_table_like_built_ins() {
    let config = Config::parse(
        r#"
            [plugin_modes."plugin:screen-selector".bindings]
            "1" = "left_click"
            esc = "escape"
            "#,
    )
    .unwrap();
    let table = &config.plugin_modes["plugin:screen-selector"].bindings;
    assert_eq!(table["1"], Binding::Click(Button::Left));
    config.validate().unwrap();
}

#[test]
fn parses_neru_style_theme_and_ui_sections() {
    let config = Config::parse(
        r##"
            [theme.dark]
            surface       = "#0A1338FF"
            accent        = "#6E82D6FF"
            accent_alt    = "#8FA2F0FF"
            on_accent_alt = "#081022FF"
            text          = "#E8EEFFFF"

            [ui_hint]
            placement = "top"
            label_x_offset = 3
            label_y_offset = -8

            [ui_hint.ui]
            font_size = 14
            background_color = { light = "#FFFFFFFF", dark = "#000000FF" }
            text_color = "#E8EEFFFF"
            matched_text_color = "#E4B400FF"

            [recursive_grid.ui]
            label_min_font_size = 5
            sub_key_preview = true
            label_char = "\u00B7"
            "##,
    )
    .unwrap();
    assert_eq!(config.ui_hint.placement, HintPlacement::Top);
    assert_eq!(config.ui_hint.label_x_offset, 3);
    assert_eq!(config.ui_hint.label_y_offset, -8);
    assert_eq!(config.ui_hint.ui.font_size, 14);
    assert!(config.ui_hint.ui.text_color.is_some());
    assert!(config.ui_hint.ui.matched_text_color.is_some());
    assert_eq!(config.recursive_grid.ui.label_min_font_size, 5);
    assert!(config.recursive_grid.ui.sub_key_preview);
    assert_eq!(config.recursive_grid.ui.label_char, "\u{B7}");
    config.validate().unwrap();
}

#[test]
fn rejects_non_rgba_component_colors() {
    let config = Config::parse(
        r##"
            [ui_hint.ui]
            text_color = "#112233"
            "##,
    )
    .unwrap();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("ui_hint.ui.text_color"), "{error}");
    assert!(error.contains("#RRGGBBAA"), "{error}");
}

#[test]
fn recursive_grid_key_count_must_match_the_grid() {
    let config = Config::parse(
        r#"
            [recursive_grid]
            grid_cols = 2
            grid_rows = 2
            keys = "abc"
            "#,
    )
    .unwrap();
    let err = config.validate().unwrap_err().to_string();
    assert!(err.contains("needs 4"), "{err}");
}

#[test]
fn layers_override_the_parent_shape() {
    let config = Config::parse(
        r#"
            [recursive_grid]
            layers = [
              { depth = 0, grid_cols = 2, grid_rows = 2, keys = "crtn" },
            ]
            "#,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(config.recursive_grid.layers[0].grid_cols, Some(2));
}

#[test]
fn rejects_unparseable_chords() {
    let config = Config::parse(
        r#"
            [normal.bindings]
            "ctrl+shift" = "grid"
            "#,
    )
    .unwrap();
    assert!(config.validate().is_err());
}

#[test]
fn a_binding_typo_is_rejected_at_parse_time() {
    // `gird` must not be silently sent as four keystrokes.
    let err = Config::parse("[normal.bindings]\ng = \"gird\"").unwrap_err();
    assert!(err.to_string().contains("unknown binding"), "{err}");
}

#[test]
fn none_disables_an_inherited_binding() {
    let config = Config::parse("[normal.bindings]\nh = \"none\"").unwrap();
    assert_eq!(config.normal.bindings["h"], Binding::Disabled);
}

#[test]
fn targeting_lifecycle_accepts_actions_and_known_modes() {
    let config = Config::parse(
        r#"
            [grid.lifecycle]
            after_finish = "left_click"
            after_click = "recursive_grid"
            "#,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.grid.lifecycle.after_finish,
        LifecycleAction::Click {
            button: MouseButton::Left,
            action: ButtonAction::Click,
        }
    );
    assert_eq!(
        config.grid.lifecycle.after_click,
        LifecycleAction::Mode(ModeId::recursive_grid())
    );
}

#[test]
fn targeting_lifecycle_rejects_unknown_modes_and_recursive_clicks() {
    let config = Config::parse(
        r#"
            [ui_hint.lifecycle]
            after_click = "does_not_exist"
            "#,
    )
    .unwrap();
    assert!(config.validate().is_err());

    let config = Config::parse("[grid.lifecycle]\nafter_click = \"left_click\"").unwrap();
    assert!(config.validate().is_err());

    let config = Config::parse("[grid.lifecycle]\nafter_finish = \"finish\"").unwrap();
    assert!(config.validate().is_err());
}

#[test]
fn obsolete_after_click_mode_is_rejected() {
    assert!(Config::parse("[grid]\nafter_click_mode = \"normal\"").is_err());
}

#[test]
fn targeting_lifecycle_defaults_match_the_shipped_experience() {
    let config = Config::default();
    assert_eq!(
        config.ui_hint.lifecycle,
        TargetingLifecycle {
            after_finish: LifecycleAction::Mode(ModeId::normal()),
            after_click: LifecycleAction::Mode(ModeId::normal()),
        }
    );
    assert_eq!(
        config.grid.lifecycle,
        TargetingLifecycle {
            after_finish: LifecycleAction::Mode(ModeId::normal()),
            after_click: LifecycleAction::Finish,
        }
    );
    assert_eq!(
        config.recursive_grid.lifecycle,
        TargetingLifecycle {
            after_finish: LifecycleAction::Keep,
            after_click: LifecycleAction::Keep,
        }
    );
}

#[test]
fn targeting_lifecycle_can_switch_to_a_configured_plugin_mode() {
    let config = Config::parse(
        r#"
            [plugin_modes."example:picker"]

            [grid.lifecycle]
            after_click = "example:picker"
            "#,
    )
    .unwrap();
    config.validate().unwrap();
}

#[test]
fn typos_in_field_names_are_rejected_rather_than_ignored() {
    // deny_unknown_fields turns a silent no-op into a visible error.
    assert!(Config::parse("[grid]\ncharacterz = \"abc\"").is_err());
    // The old mode-scoped `hotkeys` name is gone; catch stale configs.
    assert!(Config::parse("[grid]\nhotkeys = {}").is_err());
}

#[test]
fn per_mode_indicator_entries_parse_alongside_the_shared_ui() {
    // Regression: `flatten` plus `deny_unknown_fields` rejected `ui`.
    let config = Config::parse(
        r#"
            [mode_indicator.ui]
            font_size = 13

            [mode_indicator.modes.normal]
            enabled = true
            text = "Normal"
            "#,
    )
    .unwrap();
    assert_eq!(config.mode_indicator.ui.label.font_size, 13);
    let (text, _) = config
        .mode_indicator
        .for_mode("normal", "Normal")
        .expect("normal should have a badge");
    assert_eq!(text, "Normal");
    // Unlisted active modes stay visible, while idle remains silent.
    assert!(config.mode_indicator.for_mode("grid", "Grid").is_some());
    assert!(config.mode_indicator.for_mode("idle", "Idle").is_none());
}

#[test]
fn mode_indicator_only_builds_a_display_name_when_needed() {
    let indicator = ModeIndicator::default();
    let calls = std::cell::Cell::new(0);
    let (normal, _) = indicator
        .for_mode_with("normal", || {
            calls.set(calls.get() + 1);
            "unused".into()
        })
        .expect("normal indicator");
    assert_eq!(normal, "Normal");
    assert_eq!(calls.get(), 0);

    let (grid, _) = indicator
        .for_mode_with("grid", || {
            calls.set(calls.get() + 1);
            "Grid".into()
        })
        .expect("grid indicator");
    assert_eq!(grid, "Grid");
    assert_eq!(calls.get(), 1);

    assert!(
        indicator
            .for_mode_with("idle", || {
                calls.set(calls.get() + 1);
                "Idle".into()
            })
            .is_none()
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn scan_strategy_defaults_to_hybrid_and_allows_per_app_overrides() {
    let vision = VisionOptions::default();
    assert!(vision.detect_text && vision.detect_rectangles);
    assert_eq!(vision.request_timeout_ms, 5_000);
    assert_eq!(vision.minimum_confidence, 0.0);
    assert_eq!(vision.merge_iou_threshold, 0.5);
    assert_eq!(vision.rectangle_max_candidates, 100);
    assert_eq!(vision.rectangle_min_size, 0.01);
    assert_eq!(vision.button_icon_max_size, 48.0);
    assert_eq!(vision.checkbox_max_size, 32.0);
    assert_eq!(vision.generic_clickable_min_confidence, 0.5);

    assert_eq!(UiScanStrategy::default(), UiScanStrategy::Hybrid);
    assert_eq!(Config::default().ui_hint.strategy, UiScanStrategy::Hybrid);

    let config = Config::parse(
        r#"
            [ui_hint]
            strategy = "axtree"

            [[ui_hint.app_configs]]
            bundle_id = "com.example.editor"
            strategy = "hybrid"
            "#,
    )
    .unwrap();
    let app = FocusedApp {
        bundle_id: "com.example.editor".into(),
        window_title: String::new(),
        process_id: 7,
    };
    assert_eq!(
        config.ui_hint.strategy_for(Some(&app)),
        UiScanStrategy::Hybrid
    );
    assert_eq!(config.ui_hint.strategy_for(None), UiScanStrategy::AxTree);
}

#[test]
fn mode_indicator_merges_per_mode_cursor_and_badge_styles() {
    let config = Config::parse(
        r##"
            [mode_indicator.cursor]
            radius = 12
            stroke_width = 1

            [mode_indicator.ui]
            font_size = 11
            background_color = "#112233FF"

            [mode_indicator.modes.normal]
            text = "Temp Normal"

            [mode_indicator.modes.normal.cursor]
            radius = 18
            fill_color = "#44556677"

            [mode_indicator.modes.normal.ui]
            font_size = 14
            text_color = "#FFFFFFFF"
            "##,
    )
    .unwrap();
    let cursor = config
        .mode_indicator
        .cursor_for_mode("normal")
        .expect("cursor");
    assert_eq!(cursor.radius, 18);
    assert_eq!(cursor.stroke_width, 1);
    assert!(cursor.fill_color.is_some());
    let (text, ui) = config
        .mode_indicator
        .for_mode("normal", "Normal")
        .expect("badge");
    assert_eq!(text, "Temp Normal");
    assert_eq!(ui.label.font_size, 14);
    assert!(ui.label.background_color.is_some());
    assert!(ui.label.text_color.is_some());
}

#[test]
fn cursor_pressed_colors_are_configurable_and_inherit_into_modes() {
    let config = Config::parse(
        r##"
            [mode_indicator.cursor]
            left_pressed_color = "#11AA22FF"
            middle_pressed_color = "#BB33CCFF"
            right_pressed_color = "#44DDEEFF"

            [mode_indicator.modes.normal.cursor]
            left_pressed_color = "#123456FF"
            "##,
    )
    .unwrap();
    config.validate().unwrap();

    let cursor = config
        .mode_indicator
        .cursor_for_mode("normal")
        .expect("normal cursor");
    assert_eq!(
        cursor.left_pressed_color,
        Some(ThemedColor::Both("#123456FF".into()))
    );
    assert_eq!(
        cursor.middle_pressed_color,
        Some(ThemedColor::Both("#BB33CCFF".into()))
    );
    assert_eq!(
        cursor.right_pressed_color,
        Some(ThemedColor::Both("#44DDEEFF".into()))
    );
}

#[test]
fn round_trips_through_toml() {
    let config = Config::default();
    let reparsed = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(config, reparsed);
}

#[test]
fn scroll_amounts_resolve_to_configured_pixels() {
    let scroll = Scroll::default();
    assert_eq!(scroll.pixels(ScrollAmount::Step), 50.0);
    assert_eq!(scroll.pixels(ScrollAmount::Half), 500.0);
    assert_eq!(scroll.pixels(ScrollAmount::Full), 1_000_000.0);
}

fn warn(chord: &str) -> Option<String> {
    platform_warning(&KeyChord::parse(chord).unwrap())
}

#[test]
#[cfg(target_os = "macos")]
fn macos_option_letter_chords_are_flagged() {
    // The exact bug the user hit: alt+e silently never fires.
    let warning = warn("alt+e").expect("alt+e should be flagged on macOS");
    assert!(warning.contains("dead-key"), "{warning}");
    assert!(warning.contains("primary+shift"), "should suggest a fix");

    // Adding Cmd or Ctrl removes the text-composition behaviour.
    assert_eq!(warn("primary+shift+e"), None);
    assert_eq!(warn("ctrl+alt+e"), None);
    assert_eq!(
        warn("alt+w"),
        None,
        "Window uses the physical Option+W chord"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn macos_system_reserved_chords_are_flagged() {
    for chord in ["win+space", "win+tab"] {
        assert!(warn(chord).is_some(), "{chord} should be flagged");
    }
    // Cmd+Q is intentionally usable as grid/hint exit, while Shift also
    // disambiguates the other system shortcuts.
    assert_eq!(warn("win+q"), None);
    assert_eq!(warn("win+shift+space"), None);
}

#[test]
#[cfg(target_os = "macos")]
fn macos_rejects_function_keys_it_does_not_have() {
    assert!(warn("f21").is_some(), "F21 does not exist on macOS");
    assert_eq!(warn("f20"), None);
}

#[test]
#[cfg(not(target_os = "macos"))]
fn terminal_clipboard_chords_are_flagged() {
    for chord in ["ctrl+shift+c", "ctrl+shift+v"] {
        let warning = warn(chord).expect("{chord} should be flagged");
        assert!(warning.contains("clipboard"), "{warning}");
    }
    // Our own default is deliberately not one of them.
    assert_eq!(warn("ctrl+shift+e"), None);
    assert_eq!(warn("ctrl+shift+g"), None);
}

#[test]
#[cfg(not(target_os = "macos"))]
fn option_letter_chords_are_fine_off_macos() {
    // Only macOS composes text from Option+letter.
    assert_eq!(warn("alt+e"), None);
}

#[test]
fn a_flagged_chord_warns_but_still_validates() {
    // The user may know their layout better than we do, so this must not
    // be a hard error.
    let config = Config::parse(
        r#"
            [hotkeys]
            "primary+shift+e" = "normal"

            [normal.bindings]
            "alt+e" = "move_left"
            "#,
    )
    .unwrap();
    config
        .validate()
        .expect("a warning must not fail validation");

    // But it must be reported, and exactly once.
    let warnings = config.platform_warnings();
    if cfg!(target_os = "macos") {
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("alt+e"), "{warnings:?}");
    } else {
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn the_default_config_produces_no_platform_warnings() {
    // Nothing we ship may warn on the platform it runs on.
    let warnings = Config::default().platform_warnings();
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn window_modes_have_independent_sparse_defaults_and_migration_errors() {
    let config = Config::parse(
        "[window_editor]\ngap = 16\n[window_quick.bindings]\nv = 'window_layout_left'\nq = 'grid'",
    )
    .unwrap();
    assert_eq!(config.window_editor.gap, 16.0);
    assert_eq!(
        config.window_editor.bindings.get("q"),
        Some(&Binding::Mode(ModeId::window()))
    );
    assert_eq!(config.window_quick.bindings.len(), 2);
    assert_eq!(
        config.window_quick.bindings.get("q"),
        Some(&Binding::Mode(ModeId::grid()))
    );
    assert!(config.window.bindings.contains_key("h"));
    assert_eq!(
        config.window_restore.lifecycle.after_finish,
        crate::api::LifecycleAction::Mode(ModeId::window_editor())
    );
    for property in [
        "exit_mode = 'idle'",
        "gap = 8",
        "split_ratios = [0.5]",
        "layout_keys = 'asdfghjklqwe'",
        "double_tap_ms = 200",
    ] {
        assert!(
            Config::parse(&format!("[window]\n{property}")).is_err(),
            "{property}"
        );
    }
    for action in [
        "window_layout",
        "window_edit",
        "window_saved_layouts",
        "window_cancel",
        "window_exit",
    ] {
        let error = Config::parse(&format!("[window.bindings]\nx = '{action}'")).unwrap_err();
        assert!(error.to_string().contains(action), "{error}");
    }
}

#[test]
fn sparse_restore_lifecycle_preserves_default_finish_destination() {
    let config = Config::parse("[window_restore.lifecycle]\nafter_click = 'idle'").unwrap();
    assert_eq!(
        config.window_restore.lifecycle.after_finish,
        crate::api::LifecycleAction::Mode(ModeId::window_editor())
    );
    assert_eq!(
        config.window_restore.lifecycle.after_click,
        crate::api::LifecycleAction::Mode(ModeId::idle())
    );
}
