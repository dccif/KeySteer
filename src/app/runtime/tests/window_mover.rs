fn chord_test_engine(config: &Config) -> Engine {
    Engine::from_plan(
        crate::app::configuration::compile(config).unwrap(),
        Appearance::Dark,
    )
    .unwrap()
}

fn display_primary() -> String {
    KeyChord::parse(&normal_launcher()).unwrap().keys()[0]
        .as_str()
        .to_owned()
}

#[test]
fn expired_window_move_disposition_aborts_action_and_recovers_engine() {
    let config = Config::parse(
        r#"
        [key_aliases]
        primary = "left_alt"
        [normal.bindings]
        "primary+d" = "move_window next"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    engine.rebuild_tables();
    let (mut backend, log) = FakeBackend::new(vec![key_down("d")]);
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("left_alt"), &mut backend)
        .unwrap();
    backend.fail_next_disposition = true;
    engine
        .run_runtime_turn(&mut backend, Duration::ZERO)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::idle());
    assert!(engine.input.active_gestures.is_empty());
    assert!(log.lock().unwrap().window_moves.is_empty());
    // Recovery must leave the process usable for the next activation.
    for event in [key_up("d"), key_up("left_alt")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    for event in [
        key_down("left_alt"),
        key_down("d"),
        key_up("d"),
        key_up("left_alt"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(
        log.lock().unwrap().window_moves,
        [crate::api::command::WindowScreenTarget::Next]
    );
}

#[test]
fn normal_alt_d_invokes_window_mover_with_wasd_bindings() {
    let config = Config::parse(
        r#"
        [key_aliases]
        primary = "left_alt"
        [hotkeys]
        "primary+e" = "normal"
        [normal.bindings]
        w = "move_up"
        a = "move_left"
        s = "move_down"
        d = "move_right"
        "primary+s" = "screen next"
        "primary+d" = "move_window next"
    "#,
    )
    .unwrap();
    for fail in [false, true] {
        let mut engine = chord_test_engine(&config);
        let (mut backend, log) = FakeBackend::new(vec![
            key_down("left_alt"),
            key_down("e"),
            key_up("e"),
            key_up("left_alt"),
            key_down("left_alt"),
            key_down("d"),
            key_up("d"),
            key_up("left_alt"),
        ]);
        backend.fail_window_move = fail;
        engine.run(&mut backend).unwrap();
        let log = log.lock().unwrap();
        assert_eq!(
            log.window_moves,
            [crate::api::command::WindowScreenTarget::Next]
        );
        assert_eq!(
            &log.dispositions[5..7],
            &[KeyDisposition::Consume, KeyDisposition::Consume]
        );
        assert!(log.moves.is_empty());
        assert!(log.warps.is_empty());
        assert!(
            log.sent.is_empty(),
            "no fallback to application shortcuts after a failed or no-op window move"
        );
    }
}

#[test]
fn temporary_activation_keys_do_not_shadow_wasd_or_defer_movement() {
    for target in [ModeId::grid(), ModeId::recursive_grid(), ModeId::ui_hint()] {
        for window_chord in ["primary+d", "primary+s+d"] {
            let config = Config::parse(&format!(
                r#"
                [key_aliases]
                primary = "left_alt"
                [normal.bindings]
                w = "move_up"
                a = "move_left"
                s = "move_down"
                d = "move_right"
                "primary+s" = "screen next"
                "{window_chord}" = "move_window next"
            "#
            ))
            .unwrap();
            for key in ["w", "a", "s", "d"] {
                let mut engine = chord_test_engine(&config);
                engine.rebuild_tables();
                let (mut backend, log) = FakeBackend::new(Vec::new());
                engine.screens = backend.screens().unwrap();
                engine.cursor = Point::new(500.0, 400.0);
                engine
                    .activate(target.clone(), Some(ModeId::normal()), &mut backend)
                    .unwrap();
                log.lock().unwrap().warps.clear();
                for event in [
                    key_down("left_alt"),
                    key_down(key),
                    BackendEvent::Frame(Duration::from_millis(20)),
                ] {
                    engine.handle_backend_event(event, &mut backend).unwrap();
                }
                assert!(
                    engine.input.pending_chords.is_empty(),
                    "{target} {window_chord} {key}"
                );
                let move_count = log.lock().unwrap().moves.len();
                assert!(
                    move_count >= 2,
                    "{target} {window_chord} {key} must move immediately and on frames"
                );
                for event in [
                    key_up("left_alt"),
                    key_up(key),
                    BackendEvent::Frame(Duration::from_millis(20)),
                ] {
                    engine.handle_backend_event(event, &mut backend).unwrap();
                }
                assert_eq!(engine.active_mode(), &target);
                let log = log.lock().unwrap();
                assert_eq!(
                    log.moves.len(),
                    move_count,
                    "release must stop the held movement"
                );
                assert!(log.window_moves.is_empty());
                assert!(log.warps.is_empty());
            }
        }
    }
}

#[test]
fn temporary_layer_preserves_local_bindings_other_modifiers_and_normal_shortcuts() {
    let config = Config::parse(
        r#"
        [key_aliases]
        primary = "left_alt"
        [normal.bindings]
        s = "move_down"
        "primary+s" = "screen next"
        "shift+s" = "move_up"
        [grid.bindings]
        "primary+q" = "normal"
        "primary+s" = "none"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    engine.rebuild_tables();
    let s = Key::new("s").unwrap();
    let alt_s = [Key::new("left_alt").unwrap(), s.clone()];
    engine.set_active(ModeId::normal());
    assert_eq!(
        engine
            .lookup_for_pressed(&s, &alt_s)
            .unwrap()
            .binding
            .as_ref(),
        &config.normal.bindings["left_alt+s"]
    );
    engine.set_active(ModeId::grid());
    assert!(engine.lookup_for_pressed(&s, &alt_s).is_none());
    let q = Key::new("q").unwrap();
    assert_eq!(
        engine
            .lookup_for_pressed(&q, &[alt_s[0].clone(), q.clone()])
            .unwrap()
            .owner,
        ModeId::grid()
    );
    engine.set_active(ModeId::recursive_grid());
    assert_eq!(
        engine
            .lookup_for_pressed(&s, &alt_s)
            .unwrap()
            .binding
            .as_ref(),
        &Binding::Move(Direction::Down)
    );
    assert_eq!(
        engine
            .lookup_for_pressed(
                &s,
                &[alt_s[0].clone(), Key::new("left_shift").unwrap(), s.clone()]
            )
            .unwrap()
            .binding
            .as_ref(),
        &Binding::Move(Direction::Up)
    );
}

#[test]
fn wasd_movement_does_not_wait_for_an_unpressed_chord_modifier() {
    // The relevant bindings and pointer settings from keysteer.wasd.toml.
    let config = Config::parse(
        r#"
        [key_aliases]
        primary = "left_alt"
        [normal.bindings]
        w = "move_up"
        a = "move_left"
        s = "move_down"
        d = "move_right"
        "primary+s" = "screen next"
        "primary+s+d" = "move_window next"
        [pointer]
        initial_speed = 1000
        max_speed = 2200
        acceleration = 3000
        tap_distance = 2.5
    "#,
    )
    .unwrap();
    let mut distances = Vec::new();
    for key in ["w", "a", "s", "d"] {
        let mut engine = chord_test_engine(&config);
        let mut script = vec![
            key_down("left_alt"),
            key_down("e"),
            key_up("e"),
            key_up("left_alt"),
            BackendEvent::PointerMoved(Point::new(500.0, 400.0)),
            key_down(key),
        ];
        script.extend([
            BackendEvent::Frame(Duration::from_millis(20)),
            BackendEvent::Frame(Duration::from_millis(20)),
        ]);
        // End while the movement key is held: a deferred tap cannot pass.
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        let log = log.lock().unwrap();
        assert!(
            log.moves.len() >= 3,
            "{key} must move on Down and each frame: {:?}",
            log.moves
        );
        assert!(log.window_moves.is_empty());
        assert!(log.warps.is_empty());
        distances.push(
            log.moves
                .iter()
                .map(|(dx, dy)| dx.abs() + dy.abs())
                .sum::<f64>(),
        );
    }
    for distance in &distances[1..] {
        assert!((distance - distances[0]).abs() < 1e-9, "{distances:?}");
    }
    // The long chord must cancel the short screen action when Alt is held.
    let mut engine = chord_test_engine(&config);
    let (mut backend, log) = FakeBackend::new(vec![
        key_down("left_alt"),
        key_down("e"),
        key_up("e"),
        key_down("s"),
        key_down("d"),
        key_up("d"),
        key_up("s"),
        key_up("left_alt"),
    ]);
    engine.run(&mut backend).unwrap();
    let log = log.lock().unwrap();
    assert_eq!(
        log.window_moves,
        [crate::api::command::WindowScreenTarget::Next]
    );
    assert!(log.moves.is_empty());
    assert!(log.warps.is_empty());
}

#[test]
fn configured_plugin_bindings_do_not_restore_manifest_shortcuts() {
    for source in [
        "[normal.bindings]\n\"primary+x\" = \"move_window next\"\n",
        "[normal.bindings]\n\"primary+s+d\" = \"move_window next\"\n",
        "[normal.bindings]\nshift = \"slow_toggle\"\n",
    ] {
        let config = Config::parse(source).unwrap();
        let mut engine = chord_test_engine(&config);
        engine.rebuild_tables();
        let actual = engine.bindings_in(&ModeId::normal());
        assert_eq!(actual.len(), config.normal.bindings.len(), "{actual:?}");
        for (chord, binding) in &config.normal.bindings {
            assert!(
                actual.contains(&(KeyChord::parse(chord).unwrap().canonical(), binding.clone()))
            );
        }
    }
}

#[test]
fn configured_prefix_availability_is_independent_of_key_names() {
    for (short, completion) in [("q", "r"), ("7", "8"), ("f6", "f7"), ("left", "right")] {
        for modifier in ["left_alt", "right_ctrl", "left_shift", "right_meta"] {
            // Include a modifierless candidate so this also exercises the
            // per-candidate check rather than only the global fast exit.
            let config = Config::parse(&format!(
                "[normal.bindings]\n\"{short}\" = \"send home\"\n\"{modifier}+{short}+{completion}\" = \"send end\"\n\"x+c\" = \"send page_down\"\n"
            )).unwrap();
            let mut engine = chord_test_engine(&config);
            engine.rebuild_tables();
            engine.set_active(ModeId::normal());
            let key = Key::new(short).unwrap();
            engine.input.pressed.insert(key.clone());
            let resolved = engine.lookup(&key).unwrap();
            assert!(
                !engine.defer_prefix_chord(&resolved, &key),
                "{modifier}+{short}"
            );
            engine.input.pressed.insert(Key::new(modifier).unwrap());
            assert!(
                engine.defer_prefix_chord(&resolved, &key),
                "{modifier}+{short}"
            );
        }
    }
}

#[test]
fn custom_long_chords_match_including_modifierless_chords() {
    for (chord, keys) in [
        ("x+c", vec!["x", "c"]),
        ("ctrl+x+c", vec!["left_ctrl", "x", "c"]),
        ("ctrl+x+c+v", vec!["left_ctrl", "x", "c", "v"]),
    ] {
        let config =
            Config::parse(&format!("[normal.bindings]\n\"{chord}\" = \"send home\"\n")).unwrap();
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend(keys.iter().map(|key| key_down(key)));
        script.extend(keys.iter().rev().map(|key| key_up(key)));
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().sent,
            [
                ("home".into(), KeyState::Down),
                ("home".into(), KeyState::Up)
            ]
        );
    }
}

#[test]
#[ignore = "allocation probe; run alone with --test-threads=1"]
fn unrelated_or_unavailable_prefix_checks_do_not_allocate() {
    let config = Config::parse(
        r#"
        [normal.bindings]
        w = "move_up"
        s = "move_down"
        "x+c" = "send home"
        "alt+s+d" = "send end"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    engine.rebuild_tables();
    engine.set_active(ModeId::normal());
    for name in ["w", "s"] {
        engine.input.pressed.clear();
        let key = Key::new(name).unwrap();
        engine.input.pressed.insert(key.clone());
        let resolved = engine.lookup(&key).unwrap();
        let region = Region::new(TEST_ALLOCATOR);
        for _ in 0..10_000 {
            assert!(!engine.defer_prefix_chord(&resolved, &key));
        }
        let change = region.change();
        assert_eq!(change.allocations, 0, "{change:?}");
        assert_eq!(change.deallocations, 0, "{change:?}");
    }
}

#[test]
fn number_keys_select_speed_levels_and_toggle_back_to_normal_speed() {
    use crate::api::Speed;
    let config = Config::parse(
        r#"
        [normal.bindings]
        "1" = "precision_toggle"
        "2" = "slow_toggle"
        "3" = "fast_toggle"
        d = "move_right"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    engine.rebuild_tables();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.cursor = Point::new(100.0, 100.0);
    for event in enter_normal() {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    let mut distances = Vec::new();
    for (key, speed) in [
        ("1", Some(Speed::Precision)),
        ("2", Some(Speed::Slow)),
        ("3", Some(Speed::Fast)),
        ("3", None),
    ] {
        for event in [key_down(key), key_up(key)] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(engine.overlay.speed_toggle, speed);
        log.lock().unwrap().moves.clear();
        for event in [
            key_down("d"),
            BackendEvent::Frame(Duration::from_millis(20)),
            key_up("d"),
        ] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        distances.push(
            log.lock()
                .unwrap()
                .moves
                .iter()
                .map(|(dx, _)| dx)
                .sum::<f64>(),
        );
    }
    assert!(
        distances[0] < distances[1] && distances[1] < distances[3] && distances[3] < distances[2],
        "{distances:?}"
    );
}

#[test]
fn default_chord_moves_the_window_without_moving_the_pointer_first() {
    let primary = display_primary();
    use crate::api::command::WindowScreenTarget;
    let mut engine = chord_test_engine(&Config::default());
    let mut script = enter_normal();
    script.extend([
        key_down(&primary),
        key_down("d"),
        key_up("d"),
        key_up(&primary),
    ]);
    let (mut backend, log) = FakeBackend::new(script);
    backend.window_move_pointer = Some(Point::new(250.0, 200.0));
    engine.run(&mut backend).unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.window_moves, [WindowScreenTarget::Next]);
    assert_eq!(log.warps, [Point::new(250.0, 200.0)]);
    assert!(log.moves.is_empty());
    assert!(log.buttons.is_empty());
    assert_eq!(engine.active_mode(), &ModeId::normal());
}

#[test]
fn disabled_long_chords_do_not_change_the_short_action() {
    let primary = display_primary();
    let config = Config::parse(
        r#"
        [normal.bindings]
        "primary+s" = "screen next"
        "primary+s+d" = "none"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    let mut script = enter_normal();
    script.extend([key_down(&primary), key_down("s")]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();
    assert_eq!(log.lock().unwrap().warps.len(), 1);
}

#[cfg(target_os = "windows")]
#[test]
fn user_aliases_and_remapped_plugin_chords_preserve_completion_keys() {
    use crate::api::command::WindowScreenTarget;
    let config = Config::parse(
        r#"
        [key_aliases.windows]
        primary = "right_alt"
        [normal.bindings]
        "primary+x+z" = "move_window 2"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    let script = vec![
        key_down("right_alt"),
        key_down("e"),
        key_up("e"),
        key_down("x"),
        key_down("z"),
        key_up("z"),
        key_up("x"),
        key_up("right_alt"),
    ];
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.window_moves, [WindowScreenTarget::Index(1)]);
    assert!(log.warps.is_empty());
}

#[test]
fn opposite_side_long_chords_do_not_change_a_generic_short_chord() {
    let config = Config::parse(
        r#"
        [normal.bindings]
        "alt+x" = "send home"
        "left_alt+x+c" = "send end"
    "#,
    )
    .unwrap();
    let mut engine = chord_test_engine(&config);
    let mut script = enter_normal();
    script.extend([key_down("right_alt"), key_down("x")]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();
    assert_eq!(
        log.lock().unwrap().sent,
        [
            ("home".into(), KeyState::Down),
            ("home".into(), KeyState::Up)
        ]
    );
}

#[test]
fn window_follow_updates_authoritative_cursor_for_the_next_keyboard_move() {
    use crate::api::command::WindowScreenTarget;
    let mut engine = chord_test_engine(&Config::default());
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine.screens = backend.screens().unwrap();
    let mut next = engine.screens[0].clone();
    next.bounds.x = 1000.0;
    next.work_area.x = 1000.0;
    next.is_primary = false;
    engine.screens.push(next);
    engine.cursor = Point::new(10.0, 10.0);
    backend.window_move_pointer = Some(Point::new(1250.0, 200.0));
    engine
        .execute_for(
            &ModeId::normal(),
            [Command::MoveWindowToScreen(WindowScreenTarget::Next)],
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.cursor, Point::new(1250.0, 200.0));
    assert_eq!(log.lock().unwrap().warps, [engine.cursor]);
    engine
        .execute_for(
            &ModeId::normal(),
            [Command::MovePointer { dx: 5.0, dy: 0.0 }],
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.cursor, Point::new(1255.0, 200.0));
}

#[test]
fn window_noop_failure_or_failed_warp_does_not_update_cursor() {
    use crate::api::command::WindowScreenTarget;
    for (destination, fail_window, fail_warp) in [
        (None, false, false),
        (Some(Point::new(200.0, 100.0)), true, false),
        (Some(Point::new(200.0, 100.0)), false, true),
    ] {
        let mut engine = chord_test_engine(&Config::default());
        let (mut backend, log) = FakeBackend::new(vec![]);
        engine.screens = backend.screens().unwrap();
        engine.cursor = Point::new(10.0, 10.0);
        backend.window_move_pointer = destination;
        backend.fail_window_move = fail_window;
        backend.fail_warp = fail_warp;
        let result = engine.execute_for(
            &ModeId::normal(),
            [Command::MoveWindowToScreen(WindowScreenTarget::Next)],
            &mut backend,
        );
        assert_eq!(result.is_err(), fail_window || fail_warp);
        assert!(log.lock().unwrap().warps.is_empty());
        assert_eq!(engine.cursor, Point::new(10.0, 10.0));
    }
}

#[test]
fn asynchronous_window_move_warps_and_synchronizes_only_on_success() {
    for fail in [false, true] {
        let mut engine = chord_test_engine(&Config::default());
        let (mut backend, log) = FakeBackend::new(vec![]);
        engine.screens = backend.screens().unwrap();
        engine.cursor = Point::new(10.0, 10.0);
        let result = if fail {
            Err("fullscreen transition failed".into())
        } else {
            Ok(Point::new(250.0, 200.0))
        };
        engine
            .handle_backend_event(BackendEvent::WindowMoveCompleted(result), &mut backend)
            .unwrap();
        let expected = if fail {
            Point::new(10.0, 10.0)
        } else {
            Point::new(250.0, 200.0)
        };
        assert_eq!(engine.cursor, expected);
        assert_eq!(log.lock().unwrap().warps.len(), usize::from(!fail));
        engine
            .execute_for(
                &ModeId::normal(),
                [Command::MovePointer { dx: 5.0, dy: 0.0 }],
                &mut backend,
            )
            .unwrap();
        assert_eq!(engine.cursor, Point::new(expected.x + 5.0, expected.y));
    }
}

#[test]
fn custom_nested_chords_use_the_same_arbitration_for_host_actions() {
    let config = Config::parse(
        r#"
        [normal.bindings]
        "ctrl+x" = "send home"
        "ctrl+x+c" = "send end"
        "ctrl+x+c+v" = "send page_down"
    "#,
    )
    .unwrap();
    for (letters, expected) in [
        (vec!["x"], "home"),
        (vec!["x", "c"], "end"),
        (vec!["x", "c", "v"], "page_down"),
    ] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.push(key_down("left_ctrl"));
        script.extend(letters.iter().map(|key| key_down(key)));
        script.extend(letters.iter().rev().map(|key| key_up(key)));
        script.push(key_up("left_ctrl"));
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().sent,
            [
                (expected.into(), KeyState::Down),
                (expected.into(), KeyState::Up)
            ]
        );
    }
}

#[test]
fn a_single_nonmodifier_prefix_waits_for_completion_or_release() {
    let config = Config::parse(
        r#"
        [normal.bindings]
        x = "send home"
        "x+c" = "send end"
    "#,
    )
    .unwrap();
    for complete in [false, true] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.push(key_down("x"));
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert!(log.lock().unwrap().sent.is_empty());
        assert_eq!(engine.input.pending_chords.len(), 1);
        if complete {
            engine.handle_backend_event(key_down("c"), &mut backend).unwrap();
            engine.handle_backend_event(key_up("c"), &mut backend).unwrap();
        }
        engine.handle_backend_event(key_up("x"), &mut backend).unwrap();
        let expected = if complete { "end" } else { "home" };
        assert_eq!(
            log.lock().unwrap().sent,
            [(expected.into(), KeyState::Down), (expected.into(), KeyState::Up)]
        );
    }
}

#[test]
fn inherited_continuations_respect_a_local_disabled_binding() {
    for disabled in [false, true] {
        let mut config = Config::parse(
            r#"
            [normal.bindings]
            g = "grid"
            "ctrl+f1+f2" = "move_window next"
            [grid.bindings]
            "ctrl+f1" = "send home"
        "#,
        )
        .unwrap();
        if disabled {
            config
                .grid
                .bindings
                .insert("ctrl+f1+f2".into(), Binding::Disabled);
        }
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend([
            key_down("g"),
            key_up("g"),
            key_down("left_ctrl"),
            key_down("f1"),
        ]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().sent.len(),
            2 * usize::from(disabled),
            "active={} grid={:?} pending={:?}",
            engine.active_mode(),
            engine.bindings_in(&ModeId::grid()),
            engine.input.pending_chords
        );
    }
}

#[test]
fn pending_chords_are_cancelled_when_modes_or_profiles_change() {
    let config = Config::parse(
        r#"
        [normal.bindings]
        "ctrl+x" = "send home"
        "ctrl+x+c" = "send end"
        "ctrl+q" = "idle"
        [[normal.app_configs]]
        bundle_id = "changed-app"
        bindings = { "ctrl+x+c" = "none" }
    "#,
    )
    .unwrap();
    for change in [
        key_down("q"),
        BackendEvent::ToggleEnabled,
        BackendEvent::FocusChanged(Some(FocusedApp {
            bundle_id: "changed-app".into(),
            window_title: "Changed".into(),
            process_id: 42,
        })),
    ] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend([
            key_down("left_ctrl"),
            key_down("x"),
            change,
            key_up("x"),
            key_up("left_ctrl"),
        ]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert!(log.lock().unwrap().sent.is_empty());
    }
}

#[test]
fn click_prefix_is_not_injected_when_a_longer_chord_wins() {
    let config = Config::parse(
        r#"
        [normal.bindings]
        "ctrl+x" = "left_click"
        "ctrl+x+c" = "move_window next"
    "#,
    )
    .unwrap();
    for complete_long in [false, true] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend([key_down("left_ctrl"), key_down("x")]);
        if complete_long {
            script.extend([key_down("c"), key_up("c")]);
        }
        script.extend([key_up("x"), key_up("left_ctrl")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        let log = log.lock().unwrap();
        if complete_long {
            assert!(log.buttons.is_empty());
            assert_eq!(log.window_moves.len(), 1);
        } else {
            assert_eq!(log.buttons, [(MouseButton::Left, ButtonAction::Click)]);
        }
    }
}
