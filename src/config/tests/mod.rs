#![cfg(test)]

pub(crate) mod filesystem;

use super::*;
use crate::api::binding::{Button, Direction, ScrollAmount, Speed};
use crate::api::{ButtonAction, ModeId, MouseButton, VisionOptions};

#[test]
fn search_highlight_defaults_overrides_and_validation_roundtrip() {
    assert!(
        !Config::default()
            .to_toml()
            .unwrap()
            .contains("[ui_hint.search_highlight]")
    );
    let source = "[ui_hint.search_highlight]\nbackground_color = { light = '#E8F6F080', dark = '#284D4480' }\ntext_color = '#123456FF'\nborder_color = '#68BCA380'\nborder_width = 0.0";
    let config = Config::parse(source).unwrap();
    config.validate().unwrap();
    let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(
        restored.ui_hint.search_highlight,
        config.ui_hint.search_highlight
    );
    for invalid in [
        "background_color = 'bad'",
        "text_color = { light = '#000000FF' }",
        "border_color = 'bad'",
        "border_width = -1",
        "border_width = 21",
        "border_width = nan",
        "unknown = 1",
    ] {
        let result = Config::parse(&format!("[ui_hint.search_highlight]\n{invalid}"));
        assert!(
            result.is_err() || result.unwrap().validate().is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn recursive_grid_font_size_is_read_from_flat_ui_config() {
    for size in [17, 20, 36] {
        let config = Config::parse(&format!("[recursive_grid.ui]\nfont_size = {size}")).unwrap();
        assert_eq!(config.recursive_grid.ui.label.font_size, size);
    }
}

#[test]
fn usage_threshold_and_quick_switch_export_and_validate() {
    let source = r##"
[mode_usage]
save_after_entries = 37
[quick_switch]
position = "mouse"
hint_characters = "ASDF"
blacklist = ["idle", "window_tab"]
[quick_switch.ui]
text_color = { light = "#123456FF", dark = "#FFEEDDFF" }
"##;
    let config = Config::parse(source).unwrap();
    config.validate().unwrap();
    let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(restored.mode_usage.save_after_entries, 37);
    assert_eq!(restored.quick_switch, config.quick_switch);
    assert_eq!(Config::default().quick_switch.hint_characters, "wertyuiop");
    let plan = crate::app::configuration::compile(&config).unwrap();
    assert_eq!(
        plan.settings.quick_switch.alphabet,
        ["a", "s", "d", "f"].map(|key| crate::api::Key::new(key).unwrap())
    );
    for invalid in [
        "[mode_usage]\nsave_after_entries = 0",
        "[quick_switch.bindings]\nw = 'cancel_selection'",
        "[quick_switch]\nhold_ms = 0",
        "[quick_switch]\nkey = 'shift'",
        "[quick_switch]\nhint_characters = ''",
        "[quick_switch]\nhint_characters = 'w'",
        "[quick_switch]\nhint_characters = 'wW'",
        "[quick_switch]\nhint_characters = 'qwe'",
        "[quick_switch]\nhint_characters = 'we '",
        "[quick_switch]\nposition = 'desktop'",
        "[quick_switch.ui]\ntext_color = '#bad'",
    ] {
        assert!(
            Config::parse(invalid)
                .and_then(|config| config.validate())
                .is_err()
        );
    }
}

#[test]
fn editor_card_only_overrides_position() {
    let config = Config::parse("[window.card]\napp_font_size = 24\n[window_editor.card]\nposition = ['20%', '50%', '80%', '50%']").unwrap();
    config.validate().unwrap();
    let editor = config.window_editor.card.apply(&config.window.card);
    assert_eq!(editor.app_font_size, 24.0);
    assert_eq!(editor.position, ["20%", "50%", "80%", "50%"]);
    assert_eq!(config.window.card.position, ["50%", "50%", "50%", "50%"]);
    let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(restored.window_editor.card, config.window_editor.card);
}

#[test]
fn window_card_style_is_shared_validated_and_exported() {
    for mode in ["window"] {
        let config = Config::parse(&format!(
            r##"[{mode}.card]
app_font_size = 22.0
title_font_size = 30.0
app_font_family = "Example Sans"
title_bold = true
app_color = {{ light = "#123456FF", dark = "#FEDCBAFF" }}
selected_background_color = {{ light = "#E0F2E9FF", dark = "#285245E0" }}
selected_border_color = "#68BCA3FF"
selected_border_width = 0.0
text_width = 320.0
padding_y = 8.0
"##
        ))
        .unwrap();
        config.validate().unwrap();
        let reparsed = Config::parse(&config.to_toml().unwrap()).unwrap();
        assert_eq!(config.window.card, reparsed.window.card);
        for mode in [
            "window_quick",
            "window_editor",
            "window_restore",
            "window_tab",
        ] {
            assert!(Config::parse(&format!("[{mode}.card]\ntext_width = 300")).is_err());
        }
        for invalid in [
            "text_width = 0",
            "line_height = 0.5",
            "padding_y = -1",
            "app_font_size = nan",
            "title_font_size = 257",
            "title_color = 'invalid'",
            "selected_background_color = 'invalid'",
            "selected_border_color = { light = '#60B49CFF', dark = 'invalid' }",
            "selected_border_width = -1",
            "selected_border_width = 21",
            "selected_border_width = nan",
        ] {
            let invalid = Config::parse(&format!("[{mode}.card]\n{invalid}")).unwrap();
            assert!(invalid.validate().is_err());
        }
    }
}

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
        "[\"nan\"]",
        "[\"inf\"]",
        "[\"0.0\"]",
        "[\"1.0\"]",
        "[\"1/2/3\"]",
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
        ("[\"0.25\", \" 1 / 2 \", 0.5]", vec![0.25, 0.5]),
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
fn symlink_profiles_share_filename_priority_and_keep_the_logical_source_path() {
    use filesystem::{TestDirectory, symlink_file};

    let directory = TestDirectory::new("symlink-discovery");
    let dotfiles = directory.0.join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    std::fs::write(
        dotfiles.join("real.toml"),
        "[pointer]\ninitial_speed = 33\n",
    )
    .unwrap();
    let link = directory.0.join("keysteer.zulu.toml");
    symlink_file(&Path::new("dotfiles").join("real.toml"), &link);
    let ordinary = directory.0.join("keysteer.alpha.toml");
    std::fs::write(&ordinary, "[pointer]\ninitial_speed = 44\n").unwrap();
    let default = directory.0.join("keysteer.default.toml");
    std::fs::write(&default, "").unwrap();

    assert_eq!(
        Config::discover_in(&directory.0).unwrap(),
        Some(ordinary.clone())
    );
    std::fs::remove_file(&ordinary).unwrap();
    assert_eq!(
        Config::discover_in(&directory.0).unwrap(),
        Some(link.clone())
    );
    let loaded = Config::load_with_source(&link).unwrap();
    assert_eq!(loaded.path, link);
    assert_eq!(loaded.config.pointer.initial_speed, 33.0);

    let later = directory.0.join("keysteer.zzz.toml");
    std::fs::write(&later, "").unwrap();
    assert_eq!(
        Config::discover_in(&directory.0).unwrap(),
        Some(link.clone())
    );
    std::fs::remove_file(&link).unwrap();
    assert_eq!(
        Config::discover_in(&directory.0).unwrap(),
        Some(later.clone())
    );
    std::fs::remove_file(&later).unwrap();
    assert_eq!(Config::discover_in(&directory.0).unwrap(), Some(default));
}

#[test]
fn discovery_ignores_directory_links_and_diagnoses_selected_broken_links() {
    use filesystem::{TestDirectory, symlink_dir, symlink_file};

    let directory = TestDirectory::new("invalid-config-links");
    let child = directory.0.join("child");
    std::fs::create_dir(&child).unwrap();
    symlink_dir(&child, &directory.0.join("keysteer.aaa.toml"));
    symlink_file(
        Path::new("missing.toml"),
        &directory.0.join("unrelated.toml"),
    );
    let ordinary = directory.0.join("keysteer.profile.toml");
    std::fs::write(&ordinary, "").unwrap();
    let broken = directory.0.join("keysteer.zulu.toml");
    symlink_file(Path::new("missing.toml"), &broken);

    assert_eq!(
        Config::discover_in(&directory.0).unwrap(),
        Some(ordinary.clone())
    );
    std::fs::remove_file(ordinary).unwrap();
    let error = Config::discover_in(&directory.0).unwrap_err().to_string();
    assert!(error.contains("keysteer.zulu.toml"));
    assert!(
        std::fs::symlink_metadata(&broken)
            .unwrap()
            .file_type()
            .is_symlink()
    );
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
        "[key_aliases]\nspace = 'x'\nx = ''",
    ] {
        assert!(Config::parse(text).is_err(), "{text}");
    }
}

#[test]
fn empty_config_preserves_defaults() {
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
    assert_eq!(config.scroll.speed, 500.0);
    assert_eq!(config.grid.keys, Grid::default().keys);
}

#[test]
fn config_groups_preserve_surrounding_fields_and_custom_serde_hooks() {
    // Deliberately omit the Deserialize import and the last field's comma.
    mod schemas {
        use crate::config::macros::{mode_config, window_config};

        mode_config! {
            #[derive(Eq)]
            pub struct Example {
                before: u32 = 7,
                ..routing {
                    omit_empty: temporary,
                    inherits: vec!["normal".into()],
                    temporary_mode: None,
                    temporary_mode_keys: vec!["ctrl".into()],
                    temporary_mode_passthrough_keys: vec!["ctrl+x".into()],
                }
                #[serde(alias = "old_name")]
                name: String = "example".into()
            }
        }
        mode_config! {
            pub struct NewMode {
                ..routing {}
                speed: f64 = 300.0,
            }
        }
        window_config!(pub struct Window {
            common: crate::config::WindowRestore::default().common,
            #[serde(alias = "old_step")]
            step: u32 = 17
        });
    }

    let parsed: schemas::Example = toml::from_str("old_name = 'custom'").unwrap();
    assert_eq!(parsed.before, 7);
    assert_eq!(parsed.routing().inherits, ["normal"]);
    assert_eq!(parsed.name, "custom");
    assert_eq!(parsed.temporary_mode_keys, ["ctrl"]);
    assert_eq!(parsed.temporary_mode_passthrough_keys, ["ctrl+x"]);
    assert_eq!(
        toml::from_str::<schemas::Example>("").unwrap(),
        schemas::Example::default()
    );
    let mode: schemas::NewMode = toml::from_str("speed = 250").unwrap();
    assert_eq!(mode.speed, 250.0);
    assert_eq!(mode.inherits, ["hotkeys", "normal"]);
    assert_eq!(mode.routing().temporary_mode, Some("normal"));
    assert!(mode.temporary_mode_keys.is_empty());
    assert!(mode.temporary_mode_passthrough_keys.is_empty());

    let window: schemas::Window =
        toml::from_str("old_step = 9\n[lifecycle]\nafter_click = 'idle'").unwrap();
    assert_eq!(window.step, 9);
    assert_eq!(
        window.lifecycle.after_finish,
        LifecycleAction::Mode(ModeId::window_editor())
    );
    assert_eq!(
        window.lifecycle.after_click,
        LifecycleAction::Mode(ModeId::idle())
    );
    assert_eq!(toml::from_str::<schemas::Window>("").unwrap().step, 17);
    for exported in [
        toml::to_string(&parsed).unwrap(),
        toml::to_string(&window).unwrap(),
    ] {
        assert!(!exported.contains("old_"));
        assert!(!exported.contains("[common]"));
    }
}

#[test]
fn mode_schemas_preserve_defaults_explicit_overrides_and_flat_roundtrips() {
    macro_rules! check_modes {
        ($($mode:ty => ($omit_inherits:expr, $omit_temporary:expr)),+ $(,)?) => {$(
            let defaults = <$mode>::default();
            assert_eq!(toml::from_str::<$mode>("").unwrap(), defaults);
            let mut expected = defaults;
            expected.inherits.clear();
            expected.temporary_mode = Some("grid".into());
            expected.temporary_mode_keys = vec!["ctrl".into()];
            expected.temporary_mode_passthrough_keys = vec!["ctrl+f8".into()];
            expected.bindings.clear();
            for bindings in ["", "f8 = 'none'"] {
                let source = format!(
                    "inherits = []\ntemporary_mode = 'grid'\ntemporary_mode_keys = ['ctrl']\n\
                     temporary_mode_passthrough_keys = ['ctrl+f8']\n[bindings]\n{bindings}"
                );
                if !bindings.is_empty() {
                    expected.bindings = builtin_bindings(&[("f8", "none")]);
                }
                let parsed: $mode = toml::from_str(&source).unwrap();
                assert_eq!(parsed, expected, "{}", stringify!($mode));
                let exported = toml::to_string(&parsed).unwrap();
                assert!(!exported.contains("[common]"));
                assert_eq!(toml::from_str::<$mode>(&exported).unwrap(), parsed);
            }
            assert!(toml::from_str::<$mode>("unknown_setting = true").is_err());
            assert!(toml::from_str::<$mode>("temporary_mode_keys = 1").is_err());
            expected.temporary_mode = None;
            expected.temporary_mode_keys.clear();
            expected.temporary_mode_passthrough_keys.clear();
            let exported = serde_json::to_value(&expected).unwrap();
            for (field, omitted) in [
                ("inherits", $omit_inherits),
                ("temporary_mode", $omit_temporary),
                ("temporary_mode_keys", $omit_temporary),
                ("temporary_mode_passthrough_keys", $omit_temporary),
            ] {
                assert_eq!(exported.get(field).is_none(), omitted, "{}.{field}", stringify!($mode));
            }
        )+};
    }
    check_modes!(
        Normal => (false, true),
        TextInput => (false, false),
        TextSelect => (true, true),
        UiHint => (false, false),
        Grid => (false, false),
        RecursiveGrid => (false, false),
        Bisect => (false, false),
        Window => (false, false),
        WindowQuick => (false, false),
        WindowEditor => (false, false),
        WindowRestore => (false, false),
        WindowTab => (false, false),
        QuickSwitch => (true, true),
        PluginModeConfig => (false, false),
    );
}

#[test]
fn scroll_speed_accepts_fractional_values_and_survives_export() {
    assert!(Config::parse("[scroll]\nsteps_per_second = 10").is_err());
    for rate in [0.0, 0.5, 250.0, 500.0, 1000.0, 6000.0] {
        let config = Config::parse(&format!("[scroll]\nspeed = {rate:?}")).unwrap();
        config.validate().unwrap();
        assert_eq!(config.scroll.speed, rate);
        assert!(!config.to_toml().unwrap().contains("steps_per_second"));
        assert_eq!(
            Config::parse(&config.to_toml().unwrap())
                .unwrap()
                .scroll
                .speed,
            rate
        );
    }
    for rate in ["-1", "nan", "inf", "-inf"] {
        let config = Config::parse(&format!("[scroll]\nspeed = {rate}")).unwrap();
        let error = config.validate().unwrap_err();
        assert!(error.to_string().contains("scroll.speed"));
    }
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
    let (config, warnings) = Config::parse_with_warnings(
        r#"
            [platform.macos.scroll]
            invert_horizontal = true
            invert_vertical = false
            "#,
    )
    .unwrap();
    assert_eq!(config.macos_scroll_invert(), (true, false));
    assert!(warnings.is_empty());
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
    let (config, warnings) = Config::parse_with_warnings(
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
    assert_eq!(warnings.len(), 1);
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
    // Built-ins need not appear in the user alias map. Compile them through
    // the same parser as runtime keymaps, including the Windows override.
    let config = Config::default();
    let aliases = config.resolved_key_aliases();
    let primary = KeyChord::parse_with_aliases("primary", aliases).unwrap();
    for chord in config.hotkeys.keys() {
        let compiled = KeyChord::parse_with_aliases(chord, aliases).unwrap();
        assert!(!compiled.canonical().contains("primary"), "{chord:?}");
        assert_eq!(compiled.keys().len(), 2);
        if compiled.activation_key().as_str() == "e" {
            assert!(compiled.keys().contains(&primary.keys()[0]));
        }
    }
}

#[test]
fn default_bindings_avoid_platform_reserved_chords() {
    // Regression: `alt+e` never fired on macOS because Option+E is a
    // dead key. No default may have that problem on any platform.
    let config = Config::default();
    for (mode, bindings) in [
        ("idle", &config.hotkeys),
        ("normal", &config.normal.bindings),
    ] {
        for chord in bindings.keys() {
            let parsed = KeyChord::parse(chord).unwrap();
            assert_eq!(
                platform_warning(&parsed),
                None,
                "default {mode} binding {chord:?} is problematic on this platform"
            );
        }
    }
}

#[test]
fn normal_defaults_cover_the_requested_controls_and_targeting_modes() {
    let config = Config::default();
    let normal = &config.normal.bindings;
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
    assert_eq!(normal["f"], Binding::Mode(ModeId::ui_hint()));
    let primary = config
        .resolved_key_aliases()
        .get("primary")
        .map_or("primary", String::as_str);
    for (key, mode) in [("g", ModeId::recursive_grid()), ("f", ModeId::bisect())] {
        assert_eq!(
            normal.get(&format!("{primary}+{key}")),
            Some(&Binding::Mode(mode))
        );
    }
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
fn whitespace_separates_single_keys_and_chords_in_binding_groups() {
    let config = Config::parse(
        r#"
            [normal.bindings]
            "v b" = "fast"
            "  f8  " = "none"
            "é 中" = "fast"
            [text_select.bindings]
            "mod+c enter" = "text_copy"
            "#,
    )
    .unwrap();
    for key in ["v", "b", "é", "中"] {
        assert_eq!(config.normal.bindings[key], Binding::Speed(Speed::Fast));
    }
    assert_eq!(config.normal.bindings["  f8  "], Binding::Disabled);
    assert!(!config.normal.bindings.contains_key("v b"));
    let copy_chord = KeyChord::parse("mod+c").unwrap().canonical();
    let action = Binding::TextSelect(crate::api::text_select::Action::Copy);
    assert_eq!(config.text_select.bindings[&copy_chord], action);
    assert_eq!(config.text_select.bindings["enter"], action);
    assert!(!config.text_select.bindings.contains_key("mod+c enter"));
    let roundtrip = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(roundtrip.text_select.bindings, config.text_select.bindings);
    config.validate().unwrap();
    let alias = Config::parse("[key_aliases]\nCopyMod = 'right_ctrl'\n[text_select.bindings]\n'CopyMod+c enter' = 'text_copy'").unwrap();
    assert_eq!(alias.text_select.bindings["right_ctrl+c"], action);
    assert_eq!(alias.text_select.bindings["enter"], action);
    alias.validate().unwrap();
    for source in [
        "[text_select.bindings]\n'mod+c enter' = 'text_copy'\nenter = 'text_copy'",
        "[text_select.bindings]\n'ctrl+shift enter' = 'text_copy'",
        "[normal.bindings]\n'v v' = 'fast'",
    ] {
        assert!(Config::parse(source).is_err(), "{source}");
    }
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
        assert_eq!(table.get("esc"), Some(&Binding::SelectionCancel));
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
fn builtin_inventory_covers_config_sections_and_binding_validation() {
    let mut config = Config::default();
    let document = serde_json::to_value(&config).unwrap();
    let configured: BTreeSet<_> = document
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, section)| section.get("bindings").is_some())
        .map(|(name, _)| name.clone())
        .collect();
    let known: BTreeSet<_> = ModeId::BUILT_IN
        .into_iter()
        .filter(|name| *name != "idle") // Idle uses the root hotkeys table.
        .map(str::to_owned)
        .collect();
    assert_eq!(configured, known);
    let routing: BTreeSet<_> = config
        .mode_routing()
        .filter(|(name, _)| !config.plugin_modes.contains_key(*name))
        .map(|(name, _)| name.to_owned())
        .collect();
    assert_eq!(routing, configured);
    let mutable: BTreeSet<_> = config
        .temporary_key_lists_mut()
        .map(|(name, _)| name.to_owned())
        .collect();
    assert_eq!(mutable, configured);

    for name in configured {
        let mut config = Config::parse(&format!(
            "[key_aliases]\nModeTest = 'f8'\n[{name}.bindings]\nModeTest = 'none'"
        ))
        .unwrap();
        config.validate().unwrap();
        let document = serde_json::to_value(&config).unwrap();
        assert_eq!(document[&name]["bindings"]["f8"], "none", "{name}");
        let plan = crate::app::configuration::compile(&config).unwrap();
        let id = ModeId::new(&name).unwrap();
        assert_eq!(
            plan.route(&id).unwrap().bindings.get("f8"),
            Some(&Binding::Disabled),
            "{name}"
        );
        let (_, bindings) = builtin_mode_fields!(config, [&mut], |mode| &mut mode.bindings)
            .into_iter()
            .find(|(id, _)| *id == name)
            .unwrap();
        bindings.insert("invalid+".into(), Binding::Disabled);
        assert!(config.validate().is_err(), "{name}");

        let duplicate = Config::parse(&format!(
            "[key_aliases]\nModeTest = 'f8'\n[{name}.bindings]\nModeTest = 'none'\nf8 = 'none'"
        ))
        .unwrap_err()
        .to_string();
        assert!(
            duplicate.contains(&format!("[{name}.bindings] binding"))
                && duplicate.contains("duplicate chord \"f8\""),
            "{duplicate}"
        );

        if document[&name].get("app_configs").is_some() {
            let source = format!(
                "[key_aliases]\nModeTest = 'f8'\n\
                 [[{name}.app_configs]]\nbundle_id = 'org.example.Editor'\n\
                 [{name}.app_configs.bindings]\nModeTest = 'none'"
            );
            let config = Config::parse(&source).unwrap();
            config.validate().unwrap();
            let plan = crate::app::configuration::compile(&config).unwrap();
            assert_eq!(
                plan.route(&id).unwrap().app_overrides[0].bindings.get("f8"),
                Some(&Binding::Disabled),
                "{name}.app_configs"
            );
            let document = serde_json::to_value(&config).unwrap();
            assert_eq!(
                document[&name]["app_configs"][0]["bindings"]["f8"], "none",
                "{name}"
            );
            match Config::parse(&source.replace("ModeTest = 'none'", "'invalid+' = 'none'")) {
                Err(_) => {}
                Ok(config) => assert!(config.validate().is_err(), "{name}.app_configs"),
            }
            let duplicate = Config::parse(&format!("{source}\nf8 = 'none'"))
                .unwrap_err()
                .to_string();
            assert!(
                duplicate.contains(&format!("[[{name}.app_configs]]"))
                    && duplicate.contains("duplicate chord \"f8\""),
                "{duplicate}"
            );
        }
    }
}

#[test]
fn all_modes_default_to_empty_temporary_triggers() {
    let mut defaults = Config::default();
    for (mode, keys) in defaults.temporary_key_lists_mut() {
        assert!(keys.is_empty(), "{mode}");
    }
    assert!(
        super::PluginModeConfig::default()
            .temporary_mode_keys
            .is_empty()
    );
}

#[test]
fn temporary_mode_keys_must_be_modifiers() {
    let config = Config::parse("[grid]\ntemporary_mode_keys = [\"h\"]").unwrap();
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("temporary_mode_keys"), "{error}");
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

    let mixed_cycle = Config::parse(
        "[grid]\ninherits = ['window_quick']\n\
         [window_quick]\ninherits = ['plugin:picker']\n\
         [plugin_modes.'plugin:picker']\ninherits = ['grid']",
    )
    .unwrap();
    assert!(
        mixed_cycle
            .validate()
            .unwrap_err()
            .to_string()
            .contains("inheritance cycle contains \"grid\"")
    );

    // Invalid targets are still checked when temporary triggers are empty,
    // and the first error retains the established mode order.
    let invalid_targets = Config::parse(
        "[window]\ntemporary_mode = 'missing'\n\
         [recursive_grid]\ntemporary_mode = 'also_missing'",
    )
    .unwrap();
    assert!(
        invalid_targets
            .validate()
            .unwrap_err()
            .to_string()
            .contains("window.temporary_mode names unknown mode \"missing\"")
    );
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
fn parses_style_theme_and_ui_sections() {
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
    for path in ["ui_hint.ui", "window.ui", "ui_hint.search_highlight"] {
        let config = Config::parse(&format!("[{path}]\ntext_color = '#112233'")).unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains(&format!("{path}.text_color")), "{error}");
        assert!(error.contains("#RRGGBBAA for every appearance"), "{error}");
    }
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
fn text_select_copy_lifecycle_defaults_roundtrip_and_validate() {
    for source in [
        "",
        "[text_select.lifecycle]",
        include_str!("../../../keysteer.default.toml"),
    ] {
        let config = Config::parse(source).unwrap();
        let copy = if cfg!(target_os = "macos") {
            "win+c"
        } else {
            "ctrl+c"
        };
        assert_eq!(
            KeyChord::parse_with_aliases("mod+c", config.resolved_key_aliases())
                .unwrap()
                .canonical(),
            copy
        );
        for expected in [copy, "enter"] {
            assert!(config.text_select.bindings.iter().any(|(key, binding)| {
                KeyChord::parse_with_aliases(key, config.resolved_key_aliases())
                    .unwrap()
                    .canonical()
                    == expected
                    && *binding == Binding::TextSelect(crate::api::text_select::Action::Copy)
            }));
        }
        assert_eq!(
            config.text_select.bindings.get("ctrl+x"),
            Some(&Binding::TextSelect(
                crate::api::text_select::Action::ClearInput
            ))
        );
        assert_eq!(
            config.text_select.lifecycle.after_copy,
            LifecycleAction::Mode(ModeId::normal())
        );
    }
    for action in [
        "normal",
        "idle",
        "keep",
        "restart",
        "return",
        "example:picker",
    ] {
        let config = Config::parse(&format!(
            "[plugin_modes.\"example:picker\"]\n[text_select.lifecycle]\nafter_copy = '{action}'"
        ))
        .unwrap();
        config.validate().unwrap();
        let roundtrip = Config::parse(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(
            roundtrip.text_select.lifecycle,
            config.text_select.lifecycle
        );
    }
    for action in ["finish", "left_click", "double_click", "missing_mode"] {
        let config =
            Config::parse(&format!("[text_select.lifecycle]\nafter_copy = '{action}'")).unwrap();
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("text_select.lifecycle.after_copy")
        );
    }
}

#[test]
fn text_select_copy_binding_can_be_replaced_disabled_and_exported() {
    let config =
        Config::parse("[text_select.bindings]\n'mod+c' = 'none'\nf9 = 'text_copy'\n'ctrl+x' = 'none'\nf8 = 'text_clear_input'").unwrap();
    let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(restored.text_select.bindings, config.text_select.bindings);
    assert_eq!(
        restored.text_select.bindings.get("mod+c"),
        Some(&Binding::Disabled)
    );
    assert_eq!(
        restored.text_select.bindings.get("f9"),
        Some(&Binding::TextSelect(crate::api::text_select::Action::Copy))
    );
    assert!(!restored.text_select.bindings.contains_key("enter"));
    assert_eq!(
        restored.text_select.bindings.get("ctrl+x"),
        Some(&Binding::Disabled)
    );
    assert_eq!(
        restored.text_select.bindings.get("f8"),
        Some(&Binding::TextSelect(
            crate::api::text_select::Action::ClearInput
        ))
    );
}

#[test]
fn text_select_outline_and_pointer_offset_validate_and_roundtrip() {
    for source in [
        "",
        "[text_select.ui]",
        include_str!("../../../keysteer.default.toml"),
    ] {
        let config = Config::parse(source).unwrap();
        assert_eq!(config.text_select.ui.pointer_offset, [12, 12]);
        assert_eq!(config.text_select.ui.padding, [2, 2]);
        assert_eq!(config.text_select.ui.border_width, 1.5);
        assert_eq!(config.text_select.ui.word_min_width, 48);
        assert_eq!(config.text_select.ui.char_min_width, 40);
        assert_eq!(config.text_select.ui.panel_width, 800);
    }
    let config = Config::parse(
        r##"
[text_select.ui]
pointer_offset = [20, -8]
border_color = {light = "#112233FF", dark = "#AABBCCFF"}
border_width = 2.5
padding = [5, 3]
word_min_width = 60
char_min_width = 32
panel_width = 960
"##,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(
        Config::parse(&toml::to_string(&config).unwrap())
            .unwrap()
            .text_select,
        config.text_select
    );
    for value in ["[1]", "[1, 2, 3]", "[1.5, 2]", "[32768, 0]"] {
        assert!(Config::parse(&format!("[text_select.ui]\npointer_offset = {value}")).is_err());
    }
    for field in [
        "border_width = -1",
        "border_width = 21",
        "border_width = nan",
        "padding = [-1, 0]",
        "padding = [101, 0]",
        "padding = [1]",
        "padding = [1.5, 2]",
        "border_color = 'invalid'",
        "panel_width = 319",
        "panel_width = 2401",
        "panel_width = 800.5",
        "word_min_width = 23",
        "char_min_width = 241",
        "char_min_width = 42.5",
    ] {
        let parsed = Config::parse(&format!("[text_select.ui]\n{field}"));
        assert!(
            parsed.is_err() || parsed.unwrap().validate().is_err(),
            "{field}"
        );
    }
}

#[test]
fn text_select_rejects_unreleased_legacy_appearance_fields() {
    for source in [
        "[text_select]\npointer_offset = [12, 12]",
        "[text_select.selection_outline]\npadding = [2, 2]",
    ] {
        assert!(Config::parse(source).is_err(), "{source}");
    }
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
    let indicator: ModeIndicator = ModeIndicator::default();
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
fn contour_strategy_round_trips_and_can_override_hybrid_per_app() {
    let config = Config::parse(
        r#"
        [ui_hint]
        strategy = "hybrid"
        [[ui_hint.app_configs]]
        bundle_id = "com.example.canvas"
        strategy = "contour"
    "#,
    )
    .unwrap();
    let app = FocusedApp {
        bundle_id: "com.example.canvas".into(),
        window_title: String::new(),
        process_id: 9,
    };
    assert_eq!(
        config.ui_hint.strategy_for(Some(&app)),
        UiScanStrategy::Contour
    );
    let standalone = Config::parse("[ui_hint]\nstrategy = \"contour\"").unwrap();
    assert_eq!(standalone.ui_hint.strategy, UiScanStrategy::Contour);
    let encoded = serde_json::to_string(&standalone.ui_hint.strategy).unwrap();
    assert_eq!(encoded, "\"contour\"");
    assert_eq!(
        serde_json::from_str::<UiScanStrategy>(&encoded).unwrap(),
        UiScanStrategy::Contour
    );
}

#[test]
fn scan_strategy_defaults_to_hybrid_and_allows_per_app_overrides() {
    let vision = VisionOptions::default();
    assert!(vision.detect_text && vision.detect_rectangles);
    assert_eq!(vision.request_timeout_ms, 5_000);
    assert_eq!(vision.minimum_confidence, 0.0);
    assert_eq!(vision.merge_iou_threshold, 0.5);
    assert_eq!(vision.rectangle_max_candidates, 0);
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
            indicator_offset = [-12, -24]
            background_color = "#112233FF"

            [mode_indicator.modes.normal]
            text = "Temp Normal"

            [mode_indicator.modes.normal.cursor]
            radius = 18
            fill_color = "#44556677"

            [mode_indicator.modes.normal.ui]
            font_size = 14
            indicator_offset = [20, -24]
            text_color = "#FFFFFFFF"
            "##,
    )
    .unwrap();
    let cursor = config
        .mode_indicator
        .cursor_for_mode_ref("normal")
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
    assert_eq!(ui.indicator_offset, [20, -24]);
    let (_, inherited) = config.mode_indicator.for_mode("grid", "Grid").unwrap();
    assert_eq!(inherited.indicator_offset, [-12, -24]);
    assert!(ui.label.background_color.is_some());
    assert!(ui.label.text_color.is_some());
}

#[test]
fn mode_indicator_rejects_removed_placement_fields() {
    for section in ["mode_indicator.ui", "mode_indicator.modes.normal.ui"] {
        for field in [
            "position = 'bottom_left'",
            "indicator_x_offset = -12",
            "indicator_y_offset = 18",
        ] {
            assert!(Config::parse(&format!("[{section}]\n{field}")).is_err());
        }
    }
}

#[test]
fn mode_indicator_offset_is_a_bounded_pair_and_inherits_per_mode() {
    let config = Config::parse("[mode_indicator.ui]\nindicator_offset = [-12, 18]\n[mode_indicator.modes.normal.ui]\nindicator_offset = [40, 30]").unwrap();
    assert_eq!(
        config
            .mode_indicator
            .for_mode("normal", "Normal")
            .unwrap()
            .1
            .indicator_offset,
        [40, 30]
    );
    assert_eq!(
        config
            .mode_indicator
            .for_mode("grid", "Grid")
            .unwrap()
            .1
            .indicator_offset,
        [-12, 18]
    );
    for pair in ["[-32768, 32767]", "[0, 0]"] {
        assert!(Config::parse(&format!("[mode_indicator.ui]\nindicator_offset = {pair}")).is_ok());
    }
    for section in ["mode_indicator.ui", "mode_indicator.modes.normal.ui"] {
        for pair in ["[-32769, 0]", "[0, 32768]", "[1]", "[1, 2, 3]", "[1.5, 2]"] {
            assert!(Config::parse(&format!("[{section}]\nindicator_offset = {pair}")).is_err());
        }
    }
    let sparse = Config::parse("[mode_indicator.ui]\nfont_size = 18").unwrap();
    assert_eq!(sparse.mode_indicator.ui.indicator_offset, [-12, 18]);
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
        .cursor_for_mode_ref("normal")
        .expect("normal cursor");
    assert_eq!(
        cursor.left_pressed_color,
        Some(&ThemedColor::Both("#123456FF".into()))
    );
    assert_eq!(
        cursor.middle_pressed_color,
        Some(&ThemedColor::Both("#BB33CCFF".into()))
    );
    assert_eq!(
        cursor.right_pressed_color,
        Some(&ThemedColor::Both("#44DDEEFF".into()))
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

            # Isolate this warning from the intentional default Alt+C advisory.
            [text_select.bindings]
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
fn default_platform_warnings_cover_only_the_intentional_option_refinement_chord() {
    let config = Config::default();
    assert_eq!(
        config.text_select.bindings.get("alt+c"),
        Some(&Binding::TextSelect(
            crate::api::text_select::Action::Refine
        )),
    );
    let warnings = config.platform_warnings();
    // Alt+C is an intentional physical-key binding. Keep the conservative
    // layout advisory without allowing additional default warnings unnoticed.
    if cfg!(target_os = "macos") {
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("[text_select.bindings] binding \"alt+c\": on macOS, Option+C"),
            "{warnings:?}"
        );
    } else {
        assert!(warnings.is_empty(), "{warnings:?}");
    }
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
        "window_saved_presets",
        "window_cancel",
        "window_exit",
    ] {
        let error = Config::parse(&format!("[window.bindings]\nx = '{action}'")).unwrap_err();
        assert!(error.to_string().contains(action), "{error}");
    }
}

#[test]
fn sparse_window_lifecycles_preserve_each_modes_defaults() {
    let defaults = Config::default();
    for (name, mode) in defaults.window_modes() {
        for field in [None, Some("after_click"), Some("after_finish")] {
            let mut expected = mode.lifecycle.clone();
            let mut source = format!("[{name}.lifecycle]\n");
            if let Some(field) = field {
                source.push_str(&format!("{field} = 'idle'"));
                let action = if field == "after_click" {
                    &mut expected.after_click
                } else {
                    &mut expected.after_finish
                };
                *action = LifecycleAction::Mode(ModeId::idle());
            }
            let config = Config::parse(&source).unwrap();
            let (_, parsed) = config
                .window_modes()
                .into_iter()
                .find(|(key, _)| *key == name)
                .unwrap();
            assert_eq!(parsed.lifecycle, expected, "{source}");
        }
        assert!(Config::parse(&format!("[{name}.lifecycle]\nunknown = 'keep'")).is_err());
    }
}

#[test]
fn text_select_inheritance_and_temporary_mode_use_shared_validation() {
    let defaults = Config::default();
    assert!(defaults.text_select.inherits.is_empty());
    assert!(defaults.text_select.temporary_mode.is_none());
    assert!(defaults.text_select.temporary_mode_keys.is_empty());
    assert!(defaults.ui_hint.temporary_mode_keys.is_empty());
    let config = Config::parse("[key_aliases]\nBorrow = 'right_alt'\n[text_select]\ninherits = ['normal']\ntemporary_mode = 'grid'\ntemporary_mode_keys = ['Borrow']\ntemporary_mode_passthrough_keys = ['Borrow+f8']").unwrap();
    assert_eq!(config.text_select.temporary_mode_keys, ["right_alt"]);
    assert_eq!(
        config.text_select.temporary_mode_passthrough_keys,
        ["right_alt+f8"]
    );
    assert_eq!(
        Config::parse(&config.to_toml().unwrap())
            .unwrap()
            .text_select,
        config.text_select
    );
    for source in [
        "[text_select]\ninherits = ['missing']",
        "[text_select]\ninherits = ['text_select']",
        "[normal]\ninherits = ['text_select']\n[text_select]\ninherits = ['normal']",
        "[text_select]\ntemporary_mode = 'missing'",
        "[text_select]\ntemporary_mode_keys = ['h']",
        "[text_select]\ntemporary_mode_passthrough_keys = ['ctrl+']",
    ] {
        assert!(
            Config::parse(source)
                .and_then(|config| config.validate())
                .is_err(),
            "{source}"
        );
    }
    let disabled = Config::parse("[text_select]\ninherits = []\ntemporary_mode_keys = []").unwrap();
    assert!(disabled.text_select.inherits.is_empty());
    assert!(disabled.text_select.temporary_mode_keys.is_empty());
}

#[test]
fn text_select_scan_scope_is_independent_and_round_trips() {
    use crate::api::UiScanScope;
    assert_eq!(Config::default().text_select.scan_scope, UiScanScope::All);
    for source in [
        "[ui_hint]\nscan_scope = 'screen'",
        "[ui_hint]\nscan_scope = 'screen'\n[text_select]",
    ] {
        let config = Config::parse(source).unwrap();
        assert_eq!(config.text_select.scan_scope, UiScanScope::All);
        assert_eq!(config.ui_hint.scan_scope, UiScanScope::Screen);
    }
    for (value, scope) in [
        ("window", UiScanScope::Window),
        ("active", UiScanScope::Active),
        ("screen", UiScanScope::Screen),
        ("all", UiScanScope::All),
    ] {
        let config = Config::parse(&format!(
            "[ui_hint]\nscan_scope = 'active'\n[text_select]\nscan_scope = '{value}'"
        ))
        .unwrap();
        let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
        assert_eq!(restored.text_select.scan_scope, scope);
        assert_eq!(restored.ui_hint.scan_scope, UiScanScope::Active);
    }
    for value in ["'invalid'", "'inherit'", "true", "1", "[]"] {
        assert!(Config::parse(&format!("[text_select]\nscan_scope = {value}")).is_err());
    }
}

#[test]
fn ui_hint_scan_scope_defaults_and_round_trips() {
    assert_eq!(
        Config::default().ui_hint.scan_scope,
        crate::api::UiScanScope::All
    );
    let config = Config::parse("[ui_hint]\nscan_scope = 'screen'\n").unwrap();
    assert_eq!(config.ui_hint.scan_scope, crate::api::UiScanScope::Screen);
    let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(restored.ui_hint.scan_scope, crate::api::UiScanScope::Screen);
    let active = Config::parse("[ui_hint]\nscan_scope = 'active'\n").unwrap();
    assert_eq!(active.ui_hint.scan_scope, crate::api::UiScanScope::Active);
    active.validate().unwrap();
    assert_eq!(
        Config::parse(&active.to_toml().unwrap())
            .unwrap()
            .ui_hint
            .scan_scope,
        crate::api::UiScanScope::Active
    );
    let all = Config::parse("[ui_hint]\nscan_scope = 'all'\n").unwrap();
    all.validate().unwrap();
    assert_eq!(all.ui_hint.scan_scope, crate::api::UiScanScope::All);
    assert_eq!(
        Config::parse(&all.to_toml().unwrap())
            .unwrap()
            .ui_hint
            .scan_scope,
        crate::api::UiScanScope::All
    );
    assert!(Config::parse("[ui_hint]\nscreens = 'all'\n").is_err());
}

#[test]
fn restore_owns_delete_action_without_a_separate_mode_or_config() {
    use crate::api::window::WindowAction;
    let config = Config::default();
    assert_eq!(
        config.window_restore.bindings.get("x"),
        Some(&Binding::Window(WindowAction::DeletePreset))
    );
    assert_eq!(
        Binding::parse("window_delete").unwrap(),
        Binding::Window(WindowAction::DeletePreset)
    );
    assert!(!ModeId::BUILT_IN.contains(&"window_delete"));
    assert!(Config::parse("[window_delete]\nenabled = true").is_err());
}

#[test]
fn exported_window_audio_bindings_preserve_defaults_and_custom_system_mute() {
    use crate::api::window::WindowAction as W;
    let mut config = Config::default();
    for bindings in [
        &mut config.window.bindings,
        &mut config.window_quick.bindings,
        &mut config.window_editor.bindings,
    ] {
        assert_eq!(
            bindings.get("shift+v+m"),
            Some(&Binding::Window(W::SystemVolumeMute))
        );
        bindings.remove("shift+v+m");
        bindings.insert("f9".into(), Binding::Window(W::SystemVolumeMute));
    }
    let exported = config.to_toml().unwrap();
    let restored = Config::parse(&exported).unwrap();
    restored.validate().unwrap();
    for bindings in [
        &restored.window.bindings,
        &restored.window_quick.bindings,
        &restored.window_editor.bindings,
    ] {
        assert_eq!(
            bindings.get("f9"),
            Some(&Binding::Window(W::SystemVolumeMute))
        );
        assert!(!bindings.contains_key("shift+v+m"));
        assert_eq!(bindings.get("v+m"), Some(&Binding::Window(W::VolumeMute)));
        assert_eq!(
            bindings.get("shift+v+k"),
            Some(&Binding::Window(W::SystemVolumeUp))
        );
    }
}

#[test]
fn temporary_passthrough_chords_round_trip_and_resolve_aliases() {
    let config = Config::parse(
        r#"
        [key_aliases]
        shield = "right_ctrl"
        [grid]
        temporary_mode_passthrough_keys = ["q", "shield+x"]
        [window]
        temporary_mode_passthrough_keys = ["q"]
        [plugin_modes."plugin:screen-selector"]
        temporary_mode_passthrough_keys = ["shield+x"]
    "#,
    )
    .unwrap();
    assert_eq!(
        config.grid.temporary_mode_passthrough_keys,
        ["q", "right_ctrl+x"]
    );
    assert_eq!(config.window.temporary_mode_passthrough_keys, ["q"]);
    let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(
        restored.grid.temporary_mode_passthrough_keys,
        config.grid.temporary_mode_passthrough_keys
    );
    assert_eq!(
        restored.window.temporary_mode_passthrough_keys,
        config.window.temporary_mode_passthrough_keys
    );
    assert_eq!(restored.plugin_modes, config.plugin_modes);
    assert!(Config::parse("[grid]\ntemporary_mode_passthrough_keys = [\"q+q\"]").is_err());
}

#[test]
fn visual_candidate_limit_defaults_to_unlimited_and_accepts_explicit_limits() {
    assert_eq!(Config::default().ui_hint.vision.rectangle_max_candidates, 0);
    for count in [0, 1, 2000, 10_000, 100_025] {
        assert!(
            Config::parse(&format!(
                "[ui_hint.vision]\nrectangle_max_candidates = {count}"
            ))
            .and_then(|config| config.validate())
            .is_ok()
        );
    }
    assert!(Config::parse("[ui_hint.vision]\nrectangle_max_candidates = -1").is_err());
}

#[test]
fn window_multi_selection_bindings_expand_space_aliases_and_round_trip() {
    let config = Config::parse(
        r#"
[window]
target = "active"
[window.bindings]
ctrl = "window_multi_select"
"ctrl+x" = "window_multi_clear"
[window.multi_select.bindings]
"ctrl enter" = "window_multi_confirm"
"f2 f3" = "window_multi_clear"
"alt+h" = "arrow_left"
"alt+l" = "arrow_right"
"#,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(
        config.window.target,
        Some(crate::api::window::WindowTarget::Active)
    );
    for key in ["ctrl", "enter"] {
        assert_eq!(
            config.window.multi_select.bindings[key],
            Binding::Window(crate::api::window::WindowAction::MultiConfirm)
        );
    }
    let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
    assert_eq!(config.window.multi_select, restored.window.multi_select);
    assert_eq!(
        config.window.bindings["ctrl"],
        Binding::Window(crate::api::window::WindowAction::MultiSelect)
    );
    let shipped = Config::parse(include_str!("../../../keysteer.default.toml")).unwrap();
    assert_eq!(
        shipped.window.multi_select,
        Config::default().window.multi_select
    );
    for value in ["left_click", "move_left", "send ctrl+c"] {
        let invalid =
            Config::parse(&format!("[window.multi_select.bindings]\nf8 = '{value}'")).unwrap();
        assert!(
            invalid.validate().is_err(),
            "{value} is not a multi-selection edit"
        );
    }
}

#[test]
fn search_keys_expand_platform_aliases_before_validation_and_compilation() {
    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let config = Config::parse(&format!(
        r#"[key_aliases.{platform}]
Primary = "left_alt"
Point = "right_ctrl"
[ui_hint.search_bindings]
Point = "point_toggle"
"Point+shift+4" = "color_next"
[ui_hint.search_edit_keys]
accept = "enter / Primary+q"
"#
    ))
    .unwrap();
    config.validate().unwrap();
    let compiled = crate::app::mode_catalog::hint_settings(&config);
    assert_eq!(
        compiled.search_copy_keys[2],
        KeyChord::parse("ctrl+3").unwrap()
    );
    assert!(compiled.search_edit_keys.iter().any(|(key, action)| *action
        == crate::api::text_edit::EditAction::Accept
        && *key == KeyChord::parse("left_alt+q").unwrap()));
    assert!(compiled.search_point.keys.iter().any(|(key, action)| *key
        == KeyChord::parse("right_ctrl").unwrap()
        && *action == crate::api::point_sample::Action::PointToggle));
    let exported = config.to_toml().unwrap();
    assert!(!exported.contains("marker_radius"));
    assert!(!exported.contains("color_formats"));
}

#[test]
fn search_copy_keys_preserve_independent_chords() {
    let config =
        Config::parse("[ui_hint]\nsearch_copy_keys = ['ctrl+9', 'cmd+2', 'alt+3', 'ctrl+alt+4']")
            .unwrap();
    config.validate().unwrap();
    let compiled = crate::app::mode_catalog::hint_settings(&config);
    for (actual, expected) in
        compiled
            .search_copy_keys
            .iter()
            .zip(["ctrl+9", "cmd+2", "alt+3", "ctrl+alt+4"])
    {
        assert_eq!(*actual, KeyChord::parse(expected).unwrap());
    }
    let exported = config.to_toml().unwrap();
    assert_eq!(
        Config::parse(&exported).unwrap().ui_hint.search_copy_keys,
        config.ui_hint.search_copy_keys
    );
}

#[test]
fn search_point_field_modes_compile_and_round_trip_with_implicit_defaults() {
    use crate::api::point_sample::{Action, DEFAULT_FIELD_MODES, FieldMode};
    let defaults = Config::default();
    assert_eq!(
        defaults.ui_hint.search_point.field_modes,
        DEFAULT_FIELD_MODES
    );
    assert!(!defaults.to_toml().unwrap().contains("field_modes"));
    let config = Config::parse("[ui_hint.search_point]\nfield_modes = ['switch', 'concat', 'concat', 'switch']\n[ui_hint.search_bindings]\nctrl = 'point_toggle'\n'Primary+f9' = 'point_next'").unwrap();
    config.validate().unwrap();
    let compiled = crate::app::mode_catalog::hint_settings(&config);
    assert_eq!(
        compiled.search_point.field_modes,
        [
            FieldMode::Switch,
            FieldMode::Concat,
            FieldMode::Concat,
            FieldMode::Switch
        ]
    );
    assert!(
        compiled
            .search_point
            .keys
            .iter()
            .any(|(key, action)| *action == Action::PointNext && key.canonical().ends_with("f9"))
    );
    let exported = config.to_toml().unwrap();
    assert_eq!(
        Config::parse(&exported)
            .unwrap()
            .ui_hint
            .search_point
            .field_modes,
        config.ui_hint.search_point.field_modes
    );
    for values in [
        "[]",
        "['concat']",
        "['concat', 'switch', 'concat', 'switch', 'concat']",
        "['concat', 'switch', 'all', 'switch']",
    ] {
        assert!(
            Config::parse(&format!("[ui_hint.search_point]\nfield_modes = {values}")).is_err(),
            "{values}"
        );
    }
}

#[test]
fn search_point_rejects_duplicate_formats_and_conflicting_or_invalid_keys() {
    for entry in [
        "color_formats = []",
        "color_formats = [\"hex\", \"hex\"]",
        "color_formats = [\"cmyk\"]",
        "marker_radius = 0",
        "marker_width = 11",
        "marker_color = \"bad\"",
    ] {
        let source = format!("[ui_hint.search_point]\n{entry}");
        assert!(
            Config::parse(&source).and_then(|c| c.validate()).is_err(),
            "{entry}"
        );
    }
}

#[test]
fn search_bindings_reject_conflicts_and_support_replacement_and_aliases() {
    for entry in [
        "\"ctrl+k\" = \"point_toggle\"",
        "\"ctrl+4\" = \"color_next\"",
        "enter = \"point_toggle\"",
        "ctrl = \"bad\"",
    ] {
        let source = format!("[ui_hint.search_bindings]\n{entry}");
        assert!(
            Config::parse(&source).and_then(|c| c.validate()).is_err(),
            "{entry}"
        );
    }
    let config =
        Config::parse("[ui_hint.search_bindings]\nf8 = \"point_toggle\"\nf9 = \"color_next\"")
            .unwrap();
    config.validate().unwrap();
    assert_eq!(config.ui_hint.search_bindings.len(), 2);
    assert!(!config.ui_hint.search_bindings.contains_key("ctrl"));
    assert!(
        Config::parse("[ui_hint.search_bindings]")
            .unwrap()
            .ui_hint
            .search_bindings
            .is_empty()
    );
}

#[test]
fn search_point_color_preview_defaults_stay_implicit() {
    assert!(
        !Config::default()
            .to_toml()
            .unwrap()
            .contains("color_preview")
    );
    let config = Config::parse("[ui_hint.search_point.color_preview]\nwidth = 24").unwrap();
    config.validate().unwrap();
    let style = config.ui_hint.search_point.color_preview;
    assert_eq!(
        (
            style.width,
            style.height,
            style.x_offset,
            style.y_offset,
            style.border_width
        ),
        (24, 16, 4, 0, 1)
    );
    let exported = config.to_toml().unwrap();
    let tree: toml::Value = toml::from_str(&exported).unwrap();
    assert_eq!(
        tree["ui_hint"]["search_point"]["color_preview"]
            .as_table()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        Config::parse(&exported)
            .unwrap()
            .ui_hint
            .search_point
            .color_preview,
        style
    );
    for entry in [
        "width = 0",
        "height = 65",
        "x_offset = -201",
        "y_offset = 201",
        "border_width = 11",
        "enabled = 1",
        "unknown = 1",
    ] {
        assert!(
            Config::parse(&format!("[ui_hint.search_point.color_preview]\n{entry}"))
                .and_then(|c| c.validate())
                .is_err(),
            "{entry}"
        );
    }
}

#[test]
fn point_input_colors_inherit_border_and_compile_optional_theme_overrides() {
    use crate::api::Appearance;
    let compiled_defaults = crate::app::mode_catalog::hint_settings(&Config::default());
    for (appearance, expected) in [
        (Appearance::Light, "#E8F6F0FF"),
        (Appearance::Dark, "#284D44FF"),
    ] {
        let panel = &compiled_defaults.search_input_ui;
        let normal = &panel.for_appearance(appearance).panel;
        assert_eq!(
            panel.point_input_colors(appearance, normal),
            (
                crate::api::Color::parse(expected).unwrap(),
                normal.border_color
            )
        );
    }
    let defaults = Config::default().to_toml().unwrap();
    assert!(!defaults.contains("input_background_color"));
    assert!(!defaults.contains("input_border_color"));
    let config = Config::parse(
        r##"[ui_hint.search_point]
input_background_color = { light = "#11223344", dark = "#55667788" }
input_border_color = { light = "#99AABBCC", dark = "#DDEEFFAA" }
"##,
    )
    .unwrap();
    config.validate().unwrap();
    let compiled = crate::app::mode_catalog::hint_settings(&config);
    for (appearance, bg, border) in [
        (Appearance::Light, "#11223344", "#99AABBCC"),
        (Appearance::Dark, "#55667788", "#DDEEFFAA"),
    ] {
        let panel = &compiled.search_input_ui;
        let (background, stroke) =
            panel.point_input_colors(appearance, &panel.for_appearance(appearance).panel);
        assert_eq!(background, crate::api::Color::parse(bg).unwrap());
        assert_eq!(stroke, crate::api::Color::parse(border).unwrap());
    }
    let exported = config.to_toml().unwrap();
    assert_eq!(
        Config::parse(&exported).unwrap().ui_hint.search_point,
        config.ui_hint.search_point
    );
    for field in ["input_background_color", "input_border_color"] {
        assert!(
            Config::parse(&format!("[ui_hint.search_point]\n{field} = 'invalid'"))
                .and_then(|c| c.validate())
                .is_err()
        );
    }
}

#[test]
fn search_match_priority_defaults_compiles_and_round_trips_with_strict_validation() {
    use crate::api::hint::SearchMatchKind::{Label, Pinyin, Text};
    assert_eq!(
        Config::default().ui_hint.search_match_priority,
        [Pinyin, Text, Label]
    );
    let config =
        Config::parse("[ui_hint]\nsearch_match_priority = ['label', 'text', 'pinyin']").unwrap();
    config.validate().unwrap();
    assert_eq!(
        crate::app::mode_catalog::hint_settings(&config).search_match_priority,
        crate::api::hint::CompiledSearchPriority::new([Label, Text, Pinyin])
    );
    assert_eq!(
        Config::parse(&config.to_toml().unwrap())
            .unwrap()
            .ui_hint
            .search_match_priority,
        [Label, Text, Pinyin]
    );
    for values in [
        "[]",
        "['label']",
        "['label', 'text', 'pinyin', 'label']",
        "['label', 'label', 'pinyin']",
        "['label', 'text', 'unknown']",
    ] {
        assert!(
            Config::parse(&format!("[ui_hint]\nsearch_match_priority = {values}"))
                .and_then(|config| config.validate())
                .is_err(),
            "{values}"
        );
    }
}

#[test]
fn grid_region_ui_hint_defaults_can_be_rebound_or_disabled() {
    for config in [Config::default(), Config::parse("").unwrap()] {
        for bindings in [
            &config.grid.bindings,
            &config.recursive_grid.bindings,
            &config.bisect.bindings,
        ] {
            assert_eq!(
                bindings.get("u"),
                Some(&Binding::parse("area_hint").unwrap())
            );
            assert_eq!(
                bindings.get("i"),
                Some(&Binding::parse("area_text").unwrap())
            );
        }
    }
    for mode in ["grid", "recursive_grid", "bisect"] {
        let config = Config::parse(&format!(
            "[{mode}.bindings]\nu = 'none'\nf8 = 'area_hint'\n"
        ))
        .unwrap();
        let bindings = match mode {
            "grid" => &config.grid.bindings,
            "recursive_grid" => &config.recursive_grid.bindings,
            _ => &config.bisect.bindings,
        };
        assert_eq!(bindings.get("u"), Some(&Binding::Disabled));
        assert_eq!(
            bindings.get("f8"),
            Some(&Binding::parse("area_hint").unwrap())
        );
        let restored = Config::parse(&config.to_toml().unwrap()).unwrap();
        let restored_bindings = match mode {
            "grid" => &restored.grid.bindings,
            "recursive_grid" => &restored.recursive_grid.bindings,
            _ => &restored.bisect.bindings,
        };
        assert_eq!(restored_bindings.get("u"), Some(&Binding::Disabled));
        assert_eq!(
            restored_bindings.get("f8"),
            Some(&Binding::parse("area_hint").unwrap())
        );
    }
}

#[test]
fn grid_region_ui_hint_does_not_take_existing_labels_or_inherited_user_keys() {
    let inherited = Config::parse("[normal.bindings]\nu = 'mouse_left'").unwrap();
    for bindings in [
        &inherited.grid.bindings,
        &inherited.recursive_grid.bindings,
        &inherited.bisect.bindings,
    ] {
        assert!(!bindings.contains_key("u"));
    }
    let hotkey = Config::parse("[hotkeys]\nu = 'mouse_left'").unwrap();
    assert!(!hotkey.grid.bindings.contains_key("u"));
    let labels = Config::parse(
        "[grid]\nkeys = 'u2345qwertasdfgzxcvb'\n[recursive_grid]\nkeys = 'uweasdzxc'",
    )
    .unwrap();
    assert!(!labels.grid.bindings.contains_key("u"));
    assert!(!labels.recursive_grid.bindings.contains_key("u"));
    let layer = Config::parse("[[recursive_grid.layers]]\ndepth = 0\nkeys = 'uweasdzxc'").unwrap();
    assert!(!layer.recursive_grid.bindings.contains_key("u"));
    let explicit =
        Config::parse("[normal.bindings]\nu = 'mouse_left'\n[grid.bindings]\nu = 'area_hint'")
            .unwrap();
    assert_eq!(
        explicit.grid.bindings.get("u"),
        Some(&Binding::parse("area_hint").unwrap())
    );
    assert!(!explicit.bisect.bindings.contains_key("u"));
    let restored = Config::parse(&inherited.to_toml().unwrap()).unwrap();
    assert!(!restored.grid.bindings.contains_key("u"));

    let text =
        Config::parse("[grid]\nkeys = 'i2345qwertasdfgzxcvb'\n[normal.bindings]\ni = 'move_up'")
            .unwrap();
    for bindings in [
        &text.grid.bindings,
        &text.recursive_grid.bindings,
        &text.bisect.bindings,
    ] {
        assert!(!bindings.contains_key("i"));
        assert!(bindings.contains_key("u"));
    }
    let app_override =
        Config::parse("[[normal.app_configs]]\nbundle_id = 'editor'\nbindings = { i = 'move_up' }")
            .unwrap();
    assert!(!app_override.grid.bindings.contains_key("i"));
    let explicit =
        Config::parse("[normal.bindings]\ni = 'move_up'\n[grid.bindings]\ni = 'area_text'")
            .unwrap();
    assert_eq!(
        explicit.grid.bindings.get("i"),
        Some(&Binding::parse("area_text").unwrap())
    );
    let remapped = Config::parse("[grid.bindings]\ni = 'none'\nf9 = 'area_text'").unwrap();
    assert_eq!(remapped.grid.bindings.get("i"), Some(&Binding::Disabled));
    assert_eq!(
        remapped.grid.bindings.get("f9"),
        Some(&Binding::parse("area_text").unwrap())
    );
    let alias =
        Config::parse("[key_aliases]\ni = 'f9'\n[normal.bindings]\nf9 = 'move_up'").unwrap();
    assert!(!alias.grid.bindings.contains_key("f9"));
    let alias_label = Config::parse("[key_aliases]\ni = 'x'").unwrap();
    assert!(!alias_label.grid.bindings.contains_key("x"));
    let alias_explicit =
        Config::parse("[key_aliases]\ni = 'f9'\n[grid.bindings]\nf9 = 'area_text'").unwrap();
    assert_eq!(
        alias_explicit.grid.bindings.get("f9"),
        Some(&Binding::parse("area_text").unwrap())
    );
}

#[test]
fn formerly_fixed_controls_keep_defaults_in_sparse_configs_and_allow_none() {
    let config = Config::parse(
        r#"
[grid.bindings]
f8 = "first_level"
space = "none"
[bisect.bindings]
h = "none"
[ui_hint.bindings]
[window.multi_select.bindings]
f2 = "window_multi_confirm"
[window_editor.bindings]
[window_restore.bindings]
[plugin_modes."plugin:screen-selector".settings]
preserve = false
"#,
    )
    .unwrap();
    let restore = &config.window_restore.bindings;
    let multi = &config.window.multi_select.bindings;
    let screen = &config.plugin_modes["plugin:screen-selector"].bindings;
    for (binding, action) in [
        (&config.grid.bindings["space"], "none"),
        (&config.grid.bindings["backspace"], "previous_level"),
        (&config.bisect.bindings["h"], "none"),
        (&config.bisect.bindings["j"], "bisect_down"),
        (&config.bisect.bindings["enter"], "confirm_selection"),
        (&config.ui_hint.bindings["tab"], "reopen_selection"),
        (&config.window_editor.bindings["`"], "window_area_number"),
        (&restore["page_up"], "window_previous_page"),
        (&multi["backspace"], "send backspace"),
        (&screen["esc"], "cancel_selection"),
    ] {
        assert_eq!(binding, &Binding::parse(action).unwrap(), "{action}");
    }
    assert!(!config.grid.bindings.contains_key("ctrl+backspace"));
    // A complete table takes the fast path and must also preserve explicit none.
    let mut bindings = config.grid.bindings;
    bindings
        .values_mut()
        .for_each(|binding| *binding = Binding::Disabled);
    let expected = bindings.clone();
    apply_control_defaults("grid", &mut bindings, &BTreeMap::new()).unwrap();
    assert_eq!(bindings, expected);
}

#[test]
fn older_configs_enable_text_modes_and_supply_unoccupied_default_entrances() {
    let config = Config::parse("[normal.bindings]\nh = 'move_left'").unwrap();
    assert!(config.text_select.enabled);
    assert_eq!(
        config.text_select.bindings,
        Config::default().text_select.bindings
    );
    assert_eq!(
        config.text_input.bindings,
        crate::config::settings::TextInput::default().bindings
    );
    assert_eq!(
        config.normal.bindings.get("c"),
        Some(&Binding::Mode(ModeId::text_select()))
    );
    assert_eq!(
        config.normal.bindings.get("\\"),
        Some(&Binding::Mode(ModeId::text_input()))
    );
    assert!(
        config
            .text_select
            .bindings
            .values()
            .any(|b| *b == Binding::TextSelect(crate::api::text_select::Action::Refine))
    );
}

#[test]
fn upgrade_keeps_explicit_text_mode_disabling_and_remapped_or_occupied_entries() {
    for source in [
        "[normal.bindings]\nc = 'none'\n'\\' = 'none'",
        "[normal.bindings]\nc = 'grid'\n'\\' = 'grid'",
        "[normal.bindings]\nf8 = 'text_select'\nf9 = 'text_input'",
        "[normal.bindings]\n[hotkeys]\nc = 'none'\n'\\' = 'none'",
        "[normal.bindings]\n'c+h' = 'arrow_left'\n'\\+h' = 'arrow_right'",
        "[normal.bindings]\n[ text_select ]\nenabled = false\n[text_input]",
    ] {
        let config = Config::parse(source).unwrap();
        assert_ne!(
            config.normal.bindings.get("c"),
            Some(&Binding::Mode(ModeId::text_select())),
            "{source}"
        );
        assert_ne!(
            config.normal.bindings.get("\\"),
            Some(&Binding::Mode(ModeId::text_input())),
            "{source}"
        );
    }
}

#[test]
fn text_select_uses_one_cyclic_page_action_without_retired_page_bindings() {
    let config = Config::default();
    assert_eq!(
        config.text_select.bindings.get("tab"),
        Some(&Binding::TextSelect(
            crate::api::text_select::Action::CyclePage
        ))
    );
    assert!(!config.text_select.bindings.contains_key("page_up"));
    assert!(!config.text_select.bindings.contains_key("page_down"));
    for action in ["text_next_page", "text_previous_page"] {
        assert!(Binding::parse(action).is_err(), "{action}");
    }
}

#[test]
fn bisect_has_independent_validated_config_and_remappable_direction_bindings() {
    let config = Config::parse(
        r##"
[bisect]
max_depth = 30
min_size_width = 3
min_size_height = 4
[bisect.bindings]
h = "none"
"primary+a" = "bisect_left"
[bisect.ui]
line_color = "#12345678"
highlight_color = { light = "#11223344", dark = "#55667788" }
[[bisect.app_configs]]
bundle_id = "test.*"
[bisect.app_configs.bindings]
f9 = "bisect_down"
"##,
    )
    .unwrap();
    config.validate().unwrap();
    assert_eq!(config.recursive_grid, Config::default().recursive_grid);
    assert_eq!(Config::parse(&config.to_toml().unwrap()).unwrap(), config);
    for source in [
        "[bisect]\nkeys = 'hjkl'",
        "[bisect]\ngrid_cols = 2",
        "[bisect]\nmax_depth = 0",
        "[bisect]\nmax_depth = 41",
        "[bisect]\nmin_size_width = 0",
        "[bisect.ui]\nline_width = -1",
        "[bisect.ui]\nline_color = 'bad'",
        "[bisect.lifecycle]\nafter_click = 'left_click'",
        "[bisect]\ninherits = ['missing']",
    ] {
        assert!(
            Config::parse(source).and_then(|c| c.validate()).is_err(),
            "{source}"
        );
    }
    for action in ["bisect_left", "bisect_down", "bisect_up", "bisect_right"] {
        let binding = Binding::parse(action).unwrap();
        assert_eq!(binding.canonical(), action);
        assert!(!binding.is_held());
        assert!(!binding.repeats_on_key_down());
    }
}
