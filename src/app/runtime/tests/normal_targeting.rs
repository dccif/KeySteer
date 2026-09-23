fn blind_config(method: &str, reset_on: &str) -> Config {
    Config::parse(&format!(
        r#"
[normal]
long_press_toggle_ms = 0
[normal.targeting]
method = "{method}"
reset_on = {reset_on}
[normal.bindings]
h = "move_left"
j = "move_down"
k = "move_up"
l = "move_right"
";" = "left_click"
esc = "idle"
"ctrl+a" = "send ctrl+c"
[grid]
grid_cols = 2
grid_rows = 2
keys = "asdf"
max_depth = 3
[recursive_grid]
grid_cols = 2
grid_rows = 2
keys = "asdf"
max_depth = 10
[quick_switch]
enabled = false
"#
    ))
    .unwrap()
}

fn blind_engine(config: &Config) -> (Engine, FakeBackend, Arc<Mutex<Recorder>>) {
    let mut engine = Engine::from_plan(
        crate::app::configuration::compile(config).unwrap(),
        Appearance::Dark,
    )
    .unwrap();
    engine.rebuild_tables();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = vec![Screen {
        bounds: Rect::new(0.0, 0.0, 1024.0, 1024.0),
        work_area: Rect::new(0.0, 0.0, 1024.0, 1024.0),
        is_primary: true,
        scale: 1.0,
        name: None,
    }];
    engine.cursor = Point::new(512.0, 512.0);
    engine
        .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
        .unwrap();
    (engine, backend, log)
}

fn blind_tap(engine: &mut Engine, backend: &mut FakeBackend, key: &str) {
    for event in [key_down(key), key_up(key)] {
        engine.handle_backend_event(event, backend).unwrap();
    }
}

#[test]
fn normal_targeting_reuses_grid_back_reset_and_keeps_normal_without_grid_scenes() {
    for method in ["grid", "recursive_grid"] {
        let (mut engine, mut backend, log) = blind_engine(&blind_config(method, "[]"));
        for (key, expected) in [
            ("a", 256.0),
            ("a", 128.0),
            ("tab", 128.0),
            ("a", 128.0),
            ("backspace", 128.0),
            ("space", 128.0),
            ("a", 256.0),
        ] {
            blind_tap(&mut engine, &mut backend, key);
            assert_eq!(
                engine.cursor,
                Point::new(expected, expected),
                "{method} {key}"
            );
            assert_eq!(engine.active_mode(), &ModeId::normal());
        }
        blind_tap(&mut engine, &mut backend, "space");
        blind_tap(&mut engine, &mut backend, "tab");
        assert_eq!(
            engine.active_mode(),
            &ModeId::normal(),
            "root Tab must not exit"
        );
        let log = log.lock().unwrap();
        assert!(
            log.dispositions
                .iter()
                .all(|value| *value == KeyDisposition::Consume)
        );
        assert!(
            log.scenes.iter().all(|scene| scene.labels.is_empty()),
            "no grid labels may be composed"
        );
    }
}

#[test]
fn normal_targeting_reset_policy_separates_movement_click_and_grid_warp() {
    for method in ["grid", "recursive_grid"] {
        for (policy, move_reset, click_reset) in [
            ("[]", false, false),
            (r#"["move"]"#, true, false),
            (r#"["click"]"#, false, true),
            (r#"["move", "click"]"#, true, true),
        ] {
            for (action, reset) in [("h", move_reset), (";", click_reset)] {
                let (mut engine, mut backend, _) = blind_engine(&blind_config(method, policy));
                blind_tap(&mut engine, &mut backend, "a");
                blind_tap(&mut engine, &mut backend, action);
                let before = engine.cursor;
                assert_eq!(before.y, 256.0, "reset itself must not warp");
                blind_tap(&mut engine, &mut backend, "a");
                let expected = if reset { 256.0 } else { 128.0 };
                assert_eq!(
                    engine.cursor,
                    Point::new(expected, expected),
                    "{method} {policy} {action}"
                );
            }
        }
    }
}

#[test]
fn normal_targeting_compilation_is_optional_defaults_to_grid_and_roundtrips() {
    let config = Config::default();
    assert!(config.normal.targeting.is_none());
    assert!(!config.to_toml().unwrap().contains("normal.targeting"));
    let plan = crate::app::configuration::compile(&config).unwrap();
    assert!(plan.modes.iter().all(|spec| {
        !spec
            .route
            .bindings
            .values()
            .any(|binding| matches!(binding, Binding::TargetingKey(_)))
    }));
    let config = Config::parse("[normal.targeting]\n[normal.bindings]\nh = 'move_left'").unwrap();
    assert_eq!(
        config.normal.targeting.as_ref().unwrap().method,
        crate::config::TargetingMethod::Grid
    );
    crate::app::configuration::compile(&config).unwrap();
    let exported = config.to_toml().unwrap();
    assert_eq!(
        Config::parse(&exported).unwrap().normal.targeting,
        config.normal.targeting
    );
    for input in [
        "method = 'unknown'",
        "reset_on = ['pointer_move']",
        "show_overlay = true",
    ] {
        assert!(Config::parse(&format!("[normal.targeting]\n{input}")).is_err());
    }
}

#[test]
fn normal_targeting_reports_binding_alias_and_app_conflicts_before_installation() {
    let mut config = blind_config("grid", "[]");
    config
        .normal
        .bindings
        .insert("a".into(), Binding::parse("left_click").unwrap());
    let error = crate::app::configuration::compile(&config).err().unwrap();
    assert!(error.contains("a = left_click"), "{error}");
    config.normal.bindings.insert("a".into(), Binding::Disabled);
    crate::app::configuration::compile(&config).unwrap();
    let mut config = blind_config("grid", "[]");
    config
        .hotkeys
        .insert("a".into(), Binding::parse("idle").unwrap());
    assert!(crate::app::configuration::compile(&config).is_err());
    config.normal.bindings.insert("a".into(), Binding::Disabled);
    crate::app::configuration::compile(&config).unwrap();
    let mut config = blind_config("grid", "[]");
    config.normal.app_configs.push(crate::config::AppOverride {
        bundle_id: "test.*".into(),
        bindings: std::collections::BTreeMap::from([(
            "a".into(),
            Binding::parse("left_click").unwrap(),
        )]),
    });
    assert!(
        crate::app::configuration::compile(&config)
            .err()
            .unwrap()
            .contains("test.*")
    );
    let mut config = blind_config("grid", "[]");
    config
        .normal
        .bindings
        .insert("aim".into(), Binding::parse("left_click").unwrap());
    let source = config.to_toml().unwrap();
    let config = Config::parse(&format!("{source}\n[key_aliases]\naim = 'a'\n")).unwrap();
    assert!(
        crate::app::configuration::compile(&config)
            .err()
            .unwrap()
            .contains("a = left_click")
    );
}

#[test]
fn normal_targeting_full_chords_unbound_keys_and_terminal_behavior() {
    let (mut engine, mut backend, log) = blind_engine(&blind_config("grid", "[]"));
    for event in tap_chord("left_ctrl+a") {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(log.lock().unwrap().warps.is_empty());
    blind_tap(&mut engine, &mut backend, "p");
    assert_eq!(
        &log.lock().unwrap().dispositions[4..],
        &[KeyDisposition::Forward; 2]
    );
    for _ in 0..4 {
        blind_tap(&mut engine, &mut backend, "a");
    }
    assert_eq!(engine.cursor, Point::new(64.0, 64.0));
    assert_eq!(engine.active_mode(), &ModeId::normal());
    blind_tap(&mut engine, &mut backend, "tab");
    blind_tap(&mut engine, &mut backend, "f");
    assert_eq!(engine.cursor, Point::new(192.0, 192.0));
}

#[test]
fn normal_targeting_does_not_take_over_standalone_grid_navigation() {
    for method in ["grid", "recursive_grid"] {
        let (mut engine, mut backend, _) = blind_engine(&blind_config(method, "[]"));
        engine
            .activate(
                ModeId::parse_borrowed(method).unwrap(),
                Some(ModeId::normal()),
                &mut backend,
            )
            .unwrap();
        for key in ["a", "a", "tab", "f"] {
            blind_tap(&mut engine, &mut backend, key);
        }
        assert_eq!(engine.cursor, Point::new(384.0, 384.0));
        blind_tap(&mut engine, &mut backend, "space");
        blind_tap(&mut engine, &mut backend, "a");
        assert_eq!(engine.cursor, Point::new(256.0, 256.0));
    }
}

#[test]
fn normal_targeting_validates_referenced_geometry_even_when_standalone_mode_is_disabled() {
    for method in ["grid", "recursive_grid"] {
        let mut config = blind_config(method, "[]");
        config.grid.enabled = false;
        config.recursive_grid.enabled = false;
        crate::app::configuration::compile(&config).unwrap();
        if method == "grid" {
            config.grid.grid_cols = 0;
        } else {
            config.recursive_grid.grid_cols = 0;
        }
        assert!(crate::app::configuration::compile(&config).is_err());
    }
}

#[test]
fn normal_targeting_documented_default_example_compiles_after_relinquishing_conflicts() {
    let source = include_str!("../../../../keysteer.default.toml");
    let example = source
        .split("# [normal.targeting]")
        .nth(1)
        .unwrap()
        .lines()
        .skip(1)
        .take(2)
        .map(|line| line.strip_prefix("# ").unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for method in ["grid", "recursive_grid"] {
        let mut config = Config::parse(&format!(
            "{source}\n[normal.targeting]\n{}",
            example.replace("method = \"grid\"", &format!("method = {method:?}"))
        ))
        .unwrap();
        assert!(crate::app::configuration::compile(&config).is_err());
        let conflicts: &[&str] = if method == "grid" {
            &["q", "t", "f", "g", "v", "b"]
        } else {
            &["q"]
        };
        for key in conflicts {
            config
                .normal
                .bindings
                .insert((*key).into(), Binding::Disabled);
        }
        crate::app::configuration::compile(&config).unwrap();
        assert_eq!(config.normal.bindings["esc"], Binding::Mode(ModeId::idle()));
    }
}

#[test]
fn normal_targeting_replays_recursive_layers_across_screens_without_rendering() {
    let mut config = blind_config("recursive_grid", "[]");
    config.recursive_grid.layers.push(crate::config::GridLayer {
        depth: 1,
        grid_cols: Some(2),
        grid_rows: Some(1),
        keys: Some("zx".into()),
    });
    let (mut engine, mut backend, log) = blind_engine(&config);
    blind_tap(&mut engine, &mut backend, "a");
    blind_tap(&mut engine, &mut backend, "x");
    assert_eq!(engine.cursor, Point::new(384.0, 256.0));
    let mut screen = engine.screens[0].clone();
    screen.bounds.x = -1024.0;
    screen.work_area = screen.bounds;
    engine.screens.push(screen.clone());
    // Screen-selector changes the cursor before delivering ScreenRetargeted.
    engine.cursor = Point::new(-512.0, 512.0);
    engine
        .dispatch(
            ModeEvent::ScreenRetargeted {
                screen: screen.clone(),
                preserve: true,
            },
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.cursor, Point::new(-640.0, 256.0));
    blind_tap(&mut engine, &mut backend, "tab");
    blind_tap(&mut engine, &mut backend, "z");
    assert_eq!(engine.cursor, Point::new(-896.0, 256.0));
    engine
        .dispatch(
            ModeEvent::ScreenRetargeted {
                screen,
                preserve: false,
            },
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.cursor, Point::new(-512.0, 512.0));
    blind_tap(&mut engine, &mut backend, "a");
    assert_eq!(engine.cursor, Point::new(-768.0, 256.0));
    assert!(
        log.lock()
            .unwrap()
            .scenes
            .iter()
            .all(|scene| scene.labels.is_empty())
    );
}

#[test]
fn normal_targeting_ignores_repeat_and_press_release_and_retains_enter_binding() {
    let mut config = blind_config("grid", r#"["click"]"#);
    config
        .normal
        .bindings
        .insert("x".into(), Binding::parse("press mouse_left").unwrap());
    config
        .normal
        .bindings
        .insert("z".into(), Binding::parse("release mouse_left").unwrap());
    config
        .normal
        .bindings
        .insert("enter".into(), Binding::parse("send enter").unwrap());
    let (mut engine, mut backend, log) = blind_engine(&config);
    engine
        .handle_backend_event(key_down("a"), &mut backend)
        .unwrap();
    let BackendEvent::Input(mut repeat) = key_down("a") else {
        unreachable!()
    };
    repeat.repeat = true;
    engine
        .handle_backend_event(BackendEvent::Input(repeat), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("a"), &mut backend)
        .unwrap();
    assert_eq!(engine.cursor, Point::new(256.0, 256.0));
    for key in ["x", "z", "enter", "a"] {
        blind_tap(&mut engine, &mut backend, key);
    }
    assert_eq!(engine.cursor, Point::new(128.0, 128.0));
    assert!(
        log.lock()
            .unwrap()
            .sent
            .contains(&("enter".into(), KeyState::Down))
    );
}

#[test]
fn screen_plugin_preserves_grid_path_while_temporary_normal_is_held() {
    for recursive in [false, true] {
        for preserve in [false, true] {
            let mut config = blind_config("grid", "[]");
            config.normal.targeting = None;
            config.grid.cursor_follow_selection = true;
            config.recursive_grid.cursor_follow_selection = true;
            config.normal.bindings.insert("primary+s".into(), Binding::parse("screen next").unwrap());
            config.plugin_modes.entry("plugin:screen-selector".into()).or_default().settings.insert("preserve".into(), toml::Value::Boolean(preserve));
            let primary = config.resolved_key_aliases()["primary"].clone();
            let (mut engine, mut backend, _) = blind_engine(&config);
            let mut second = engine.screens[0].clone();
            second.bounds.x = 1024.0;
            second.work_area.x = 1024.0;
            second.is_primary = false;
            engine.screens.push(second);
            let mode = if recursive { ModeId::recursive_grid() } else { ModeId::grid() };
            engine.activate(mode.clone(), Some(ModeId::normal()), &mut backend).unwrap();
            blind_tap(&mut engine, &mut backend, "a");
            engine.handle_backend_event(key_down(&primary), &mut backend).unwrap();
            blind_tap(&mut engine, &mut backend, "s");
            engine.handle_backend_event(key_up(&primary), &mut backend).unwrap();
            assert_eq!(engine.active_mode(), &mode);
            assert_eq!(engine.cursor, if preserve { Point::new(1280.0, 256.0) } else { Point::new(1536.0, 512.0) });
            blind_tap(&mut engine, &mut backend, "a");
            assert_eq!(engine.cursor, if preserve { Point::new(1152.0, 128.0) } else { Point::new(1280.0, 256.0) });
        }
    }
}

#[test]
fn single_level_targeting_leaves_normal_navigation_bindings_available() {
    for method in ["grid", "recursive_grid"] {
        let mut config = blind_config(method, "[]");
        config.normal.targeting.as_mut().unwrap().max_depth = Some(1);
        for key in ["esc", "enter", "tab", "backspace", "space"] {
            config.normal.bindings.insert(key.into(), Binding::parse("send enter").unwrap());
        }
        let (mut engine, mut backend, log) = blind_engine(&config);
        for key in ["esc", "enter", "tab", "backspace", "space"] {
            blind_tap(&mut engine, &mut backend, key);
        }
        assert_eq!(log.lock().unwrap().sent.iter().filter(|(key, state)| key == "enter" && *state == KeyState::Down).count(), 5);
        config.normal.targeting.as_mut().unwrap().max_depth = Some(3);
        assert!(crate::app::configuration::compile(&config).is_err());
        config.normal.targeting.as_mut().unwrap().max_depth = Some(1);
        config.normal.bindings.insert("a".into(), Binding::parse("send enter").unwrap());
        assert!(crate::app::configuration::compile(&config).is_err());
    }
}

#[test]
fn normal_targeting_single_level_repeatedly_selects_root_cells() {
    for method in ["grid", "recursive_grid"] {
        for reset_on in ["[]", "[\"move\", \"click\"]"] {
            let mut config = blind_config(method, reset_on);
            let targeting = config.normal.targeting.as_mut().unwrap();
            targeting.grid_cols = Some(3);
            targeting.grid_rows = Some(2);
            targeting.keys = Some("asdzxc".into());
            targeting.max_depth = Some(1);
            let (mut engine, mut backend, _) = blind_engine(&config);
            for key in ["a", "c", "s", "s", "z", "x", "d", "tab", "space", "a"] {
                let previous = engine.cursor;
                blind_tap(&mut engine, &mut backend, key);
                if let Some(index) = "asdzxc".find(key) {
                    let x = (index % 3) as f64 * (1024.0 / 3.0) + 1024.0 / 6.0;
                    let y = (index / 3) as f64 * 512.0 + 256.0;
                    assert!((engine.cursor.x - x).abs() < 1.0e-9);
                    assert_eq!(engine.cursor.y, y);
                } else {
                    assert_eq!(engine.cursor, previous);
                }
                assert_eq!(engine.active_mode(), &ModeId::normal());
            }
        }
    }
}

#[test]
fn normal_targeting_geometry_overrides_are_local_and_drive_input_compilation() {
    for method in ["grid", "recursive_grid"] {
        let mut config = blind_config(method, "[]");
        let original_grid = config.grid.clone();
        let original_recursive = config.recursive_grid.clone();
        let targeting = config.normal.targeting.as_mut().unwrap();
        targeting.grid_cols = Some(4);
        targeting.grid_rows = Some(1);
        targeting.keys = Some("1234".into());
        targeting.max_depth = Some(1);
        let exported = config.to_toml().unwrap();
        let config = Config::parse(&exported).unwrap();
        assert_eq!(config.grid, original_grid);
        assert_eq!(config.recursive_grid, original_recursive);
        let (mut engine, mut backend, _) = blind_engine(&config);
        blind_tap(&mut engine, &mut backend, "1");
        assert_eq!(engine.cursor, Point::new(128.0, 512.0));
        blind_tap(&mut engine, &mut backend, "2");
        assert_eq!(
            engine.cursor,
            Point::new(384.0, 512.0),
            "a single level must keep targeting the root"
        );
        blind_tap(&mut engine, &mut backend, "space");
        blind_tap(&mut engine, &mut backend, "a");
        assert_eq!(
            engine.cursor,
            Point::new(384.0, 512.0),
            "old alphabet must not be generated"
        );
        blind_tap(&mut engine, &mut backend, "4");
        assert_eq!(engine.cursor, Point::new(896.0, 512.0));
    }
}

#[test]
fn normal_targeting_partial_overrides_inherit_and_layers_replace_or_clear() {
    let mut config = blind_config("recursive_grid", "[]");
    config.recursive_grid.layers.push(crate::config::GridLayer {
        depth: 0,
        grid_cols: None,
        grid_rows: None,
        keys: Some("hjkl".into()),
    });
    assert!(
        crate::app::configuration::compile(&config).is_err(),
        "inherited keys conflict"
    );
    config.normal.targeting.as_mut().unwrap().layers = Some(vec![crate::config::GridLayer {
        depth: 0,
        grid_cols: Some(4),
        grid_rows: Some(1),
        keys: Some("1234".into()),
    }]);
    let (mut engine, mut backend, _) = blind_engine(&config);
    blind_tap(&mut engine, &mut backend, "1");
    assert_eq!(engine.cursor, Point::new(128.0, 512.0));
    blind_tap(&mut engine, &mut backend, "a");
    assert_eq!(
        engine.cursor,
        Point::new(64.0, 256.0),
        "unoverridden depth inherits base layout"
    );
    config.normal.targeting.as_mut().unwrap().layers = Some(vec![]);
    let exported = config.to_toml().unwrap();
    let config = Config::parse(&exported).unwrap();
    assert_eq!(
        config.normal.targeting.as_ref().unwrap().layers,
        Some(vec![])
    );
    let (mut engine, mut backend, _) = blind_engine(&config);
    blind_tap(&mut engine, &mut backend, "a");
    assert_eq!(engine.cursor, Point::new(256.0, 256.0));
}

#[test]
fn normal_targeting_min_size_override_stops_recursion_without_changing_source() {
    let mut config = blind_config("recursive_grid", "[]");
    config.normal.targeting.as_mut().unwrap().min_size_width = Some(600);
    let (mut engine, mut backend, _) = blind_engine(&config);
    blind_tap(&mut engine, &mut backend, "a");
    assert_eq!(engine.cursor, Point::new(256.0, 256.0));
    blind_tap(&mut engine, &mut backend, "a");
    assert_eq!(engine.cursor, Point::new(256.0, 256.0));
    assert_eq!(config.recursive_grid.min_size_width, 1);
}

#[test]
fn normal_targeting_validates_effective_overrides_and_rejects_ui() {
    for method in ["grid", "recursive_grid"] {
        let mut config = blind_config(method, "[]");
        config.grid.enabled = false;
        config.recursive_grid.enabled = false;
        config.grid.grid_cols = 0;
        config.recursive_grid.grid_cols = 0;
        config.normal.targeting.as_mut().unwrap().grid_cols = Some(2);
        config.validate().unwrap();
        crate::app::configuration::compile(&config).unwrap();
        config.normal.targeting.as_mut().unwrap().grid_cols = Some(3);
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("normal.targeting.keys")
        );
        config.normal.targeting.as_mut().unwrap().grid_cols = Some(2);
        config.normal.targeting.as_mut().unwrap().keys = Some("hjkl".into());
        assert!(
            crate::app::configuration::compile(&config)
                .err()
                .unwrap()
                .contains("normal.targeting conflicts")
        );
    }
    for field in ["layers = []", "min_size_width = 1", "min_size_height = 1"] {
        assert!(
            Config::parse(&format!("[normal.targeting]\n{field}"))
                .unwrap()
                .validate()
                .is_err()
        );
    }
    assert!(Config::parse("[normal.targeting.ui]\nfont_size = 20").is_err());
}
