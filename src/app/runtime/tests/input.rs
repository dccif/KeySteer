#[test]
fn mouse_side_buttons_launch_modes_and_keep_the_release_consumed() {
    let config: Config = toml::from_str("[hotkeys]\nmouse4 = 'normal'").unwrap();
    let mut engine = Engine::from_plan(crate::app::configuration::compile(&config).unwrap(), Appearance::Dark).unwrap();
    let (mut backend, log) = FakeBackend::new(vec![key_down("mouse_x1"), key_up("mouse_x1")]);
    engine.run(&mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert_eq!(log.lock().unwrap().dispositions, [KeyDisposition::Consume; 2]);
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn unbound_mouse_side_buttons_forward_in_capturing_modes() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [key_down("mouse_x1"), key_up("mouse_x1"), key_down("mouse_x2"), key_up("mouse_x2")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(log.lock().unwrap().dispositions, [KeyDisposition::Forward; 4]);
    assert!(!seen.lock().unwrap().iter().any(|event| event.ends_with(":key") || event.ends_with(":clicked")));
}

#[test]
fn mouse_side_button_chord_releases_held_action_after_modifier_release() {
    let mut engine = engine_with_normal_binding("ctrl+mouse_x2", "move_left");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [key_down("left_ctrl"), key_down("mouse_x2")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(engine.input.active_gestures.get(&Key::new("mouse_x2").unwrap()).is_some());
    for event in [key_up("left_ctrl"), key_up("mouse_x2")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(engine.input.active_gestures.is_empty());
    let log = log.lock().unwrap();
    assert_eq!(log.dispositions[1], KeyDisposition::Consume);
    assert_eq!(log.dispositions[3], KeyDisposition::Consume);
}

#[test]
fn disabled_mouse_side_binding_blocks_inheritance_and_preserves_native_behavior() {
    let mut config = Config::default();
    config.normal.bindings.insert("mouse_x2".into(), Binding::parse("key_help").unwrap());
    config.grid.inherits = vec!["normal".into()];
    config.grid.bindings.insert("mouse_x2".into(), Binding::Disabled);
    let mut engine = Engine::from_plan(crate::app::configuration::compile(&config).unwrap(), Appearance::Dark).unwrap();
    engine.registry.active = ModeId::grid();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [key_down("mouse_x2"), key_up("mouse_x2")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(!engine.overlay.key_help_visible);
    assert_eq!(log.lock().unwrap().dispositions, [KeyDisposition::Forward; 2]);
}

#[test]
fn trace_lazy_does_not_build_messages_when_debug_is_disabled() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let builds = std::cell::Cell::new(0);

    engine.trace_lazy(true, "test", || {
        builds.set(builds.get() + 1);
        "disabled globally".to_string()
    });
    assert_eq!(builds.get(), 0);

    engine.settings.debug.enabled = true;
    engine.trace_lazy(false, "test", || {
        builds.set(builds.get() + 1);
        "disabled category".to_string()
    });
    assert_eq!(builds.get(), 0);
}

#[test]
fn normal_run_calls_backend_shutdown_once() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(vec![BackendEvent::Quit]);
    engine.run(&mut backend).unwrap();
    assert_eq!(log.lock().unwrap().shutdowns, 1);
}

#[test]
fn startup_failure_still_calls_backend_shutdown() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    backend.fail_start = true;

    assert!(engine.run(&mut backend).is_err());
    assert_eq!(log.lock().unwrap().shutdowns, 1);
}

#[test]
fn reload_while_idle_updates_grid_max_depth_before_activation() {
    let mut initial = Config::default();
    initial.grid.max_depth = 2;
    let mut engine = Engine::new(initial.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&initial) {
        engine.register(mode);
    }
    engine.screens = vec![Screen {
        bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
        work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
        is_primary: true,
        scale: 1.0,
        name: None,
    }];
    let (mut backend, _) = FakeBackend::new(Vec::new());

    let mut reloaded = initial;
    reloaded.grid.max_depth = 3;
    let plan = crate::app::configuration::compile(&reloaded).unwrap();
    engine.apply_runtime_plan(plan, &mut backend).unwrap();
    engine
        .activate(ModeId::grid(), Some(ModeId::normal()), &mut backend)
        .unwrap();

    for _ in 0..2 {
        engine
            .handle_backend_event(key_down("1"), &mut backend)
            .unwrap();
        engine
            .handle_backend_event(key_up("1"), &mut backend)
            .unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::grid());

    engine
        .handle_backend_event(key_down("1"), &mut backend)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
}

#[test]
fn idle_enters_normal_on_the_configured_chord() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);

    let (mut backend, _) = FakeBackend::new(enter_normal());
    engine.run(&mut backend).unwrap();

    let log = seen.lock().unwrap().clone();
    assert!(log.contains(&"normal:activated".to_string()), "{log:?}");
    assert_eq!(engine.active_mode().as_str(), "normal");
}

#[test]
fn launcher_modifier_release_keeps_the_forwarding_decision_from_its_press() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    let (mut backend, log) = FakeBackend::new(enter_normal());
    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().dispositions,
        [
            KeyDisposition::Forward,
            KeyDisposition::Consume,
            KeyDisposition::Consume,
            KeyDisposition::Forward,
        ]
    );
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn alt_launcher_forwards_both_modifier_edges_without_replay() {
    let mut config = Config::default();
    config.hotkeys.clear();
    config
        .hotkeys
        .insert("alt+e".into(), Binding::Mode(ModeId::normal()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));
    let (mut backend, log) = FakeBackend::new(tap_chord("left_alt+e"));
    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().dispositions,
        [
            KeyDisposition::Forward,
            KeyDisposition::Consume,
            KeyDisposition::Consume,
            KeyDisposition::Forward,
        ]
    );
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn idle_bare_binding_does_not_claim_an_external_alt_chord() {
    let mut config = Config::default();
    config.hotkeys.clear();
    config
        .hotkeys
        .insert("h".into(), Binding::Mode(ModeId::normal()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));
    let (mut backend, log) = FakeBackend::new(tap_chord("left_alt+h"));
    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().dispositions,
        [
            KeyDisposition::Forward,
            KeyDisposition::Forward,
            KeyDisposition::Forward,
            KeyDisposition::Forward,
        ]
    );
    assert_eq!(engine.active_mode(), &ModeId::idle());
}

#[test]
fn normal_passthrough_uses_complete_modifier_combinations() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("h".into(), Binding::Move(Direction::Left));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    let mut normal = ProbeMode::new("normal", seen.clone());
    normal.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(normal));

    let log = run_in_normal(&mut engine, tap_chord("left_alt+h"));
    let recorded = log.lock().unwrap();
    let dispositions = &recorded.dispositions;
    assert_eq!(
        &dispositions[dispositions.len() - 4..],
        [
            KeyDisposition::Forward,
            KeyDisposition::Forward,
            KeyDisposition::Forward,
            KeyDisposition::Forward,
        ]
    );
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|event| event != "normal:binding(move_left)"),
        "Alt+H must not fall back to bare h"
    );
}

#[test]
fn consumed_normal_modifier_can_still_modify_a_bare_binding() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("left_shift".into(), Binding::Speed(crate::api::Speed::Slow));
    config
        .normal
        .bindings
        .insert("h".into(), Binding::Move(Direction::Left));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    let mut normal = ProbeMode::new("normal", seen.clone());
    normal.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(normal));

    let log = run_in_normal(&mut engine, tap_chord("left_shift+h"));
    let recorded = log.lock().unwrap();
    let dispositions = &recorded.dispositions;
    assert_eq!(
        &dispositions[dispositions.len() - 4..],
        [
            KeyDisposition::Consume,
            KeyDisposition::Consume,
            KeyDisposition::Consume,
            KeyDisposition::Consume,
        ]
    );
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|event| event == "normal:binding(slow)"));
    assert!(
        seen.iter()
            .any(|event| event == "normal:binding(move_left)")
    );
}

#[test]
fn disabling_normal_passthrough_restores_exclusive_matching() {
    let mut config = Config::default();
    config.normal.passthrough_unbound_keys = false;
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("h".into(), Binding::Move(Direction::Left));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let log = run_in_normal(&mut engine, tap_chord("left_alt+h"));
    let recorded = log.lock().unwrap();
    let dispositions = &recorded.dispositions;
    assert_eq!(
        &dispositions[dispositions.len() - 4..],
        [
            KeyDisposition::Consume,
            KeyDisposition::Consume,
            KeyDisposition::Consume,
            KeyDisposition::Consume,
        ]
    );
}

#[test]
fn repeat_and_release_keep_the_first_down_disposition() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let key = Key::new("f").unwrap();
    let input = |state, repeat| InputEvent {
        character: None,
        key: key.clone(),
        state,
        repeat,
        injected: false,
        timestamp_millis: 0,
    };

    assert_eq!(
        engine.complete_key_disposition(&input(KeyState::Down, false), KeyOutcome::Forwarded,),
        KeyOutcome::Forwarded
    );
    assert_eq!(
        engine.complete_key_disposition(&input(KeyState::Down, true), KeyOutcome::Consumed),
        KeyOutcome::Forwarded,
        "a repeat after enabling or changing mode must not consume a forwarded lifecycle"
    );
    assert_eq!(
        engine.complete_key_disposition(&input(KeyState::Up, false), KeyOutcome::Consumed),
        KeyOutcome::Forwarded
    );

    assert_eq!(
        engine.complete_key_disposition(&input(KeyState::Down, false), KeyOutcome::Consumed),
        KeyOutcome::Consumed
    );
    assert_eq!(
        engine.complete_key_disposition(&input(KeyState::Down, true), KeyOutcome::Forwarded),
        KeyOutcome::Consumed,
        "a repeat after pausing must not expose a previously consumed lifecycle"
    );
    assert_eq!(
        engine.complete_key_disposition(&input(KeyState::Up, false), KeyOutcome::Forwarded),
        KeyOutcome::Consumed
    );
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn repeat_does_not_start_a_binding_missing_from_the_first_down() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_probes(&seen, "l", "move_right");
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("l"), &mut backend)
        .unwrap();
    engine
        .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::Input(InputEvent {
                character: None,
                key: Key::new("l").unwrap(),
                state: KeyState::Down,
                repeat: true,
                injected: false,
                timestamp_millis: 1,
            }),
            &mut backend,
        )
        .unwrap();
    engine
        .handle_backend_event(key_up("l"), &mut backend)
        .unwrap();

    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|event| !event.contains("binding(move_right)")),
        "a forwarded lifecycle must not acquire a held binding after context changes"
    );
    assert_eq!(
        log.lock().unwrap().dispositions,
        [
            KeyDisposition::Forward,
            KeyDisposition::Forward,
            KeyDisposition::Forward,
        ]
    );
    assert!(engine.input.active_gestures.is_empty());
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn outward_motion_at_every_screen_edge_keeps_normal_active_and_can_reverse() {
    let cases = [
        (Point::new(0.0, 400.0), (-20.0, 0.0), (20.0, 0.0)),
        (Point::new(999.0, 400.0), (20.0, 0.0), (-20.0, 0.0)),
        (Point::new(500.0, 0.0), (0.0, -20.0), (0.0, 20.0)),
        (Point::new(500.0, 799.0), (0.0, 20.0), (0.0, -20.0)),
    ];

    for (edge, outward, inward) in cases {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        engine.registry.active = ModeId::normal();
        engine.cursor = edge;
        engine.screens = vec![Screen {
            bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
            work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        }];
        let (mut backend, log) = FakeBackend::new(Vec::new());

        for _ in 0..20 {
            engine
                .execute(
                    vec![Command::MovePointer {
                        dx: outward.0,
                        dy: outward.1,
                    }],
                    &mut backend,
                )
                .unwrap();
        }
        assert_eq!(engine.cursor, edge);
        assert_eq!(engine.active_mode(), &ModeId::normal());
        assert!(log.lock().unwrap().moves.is_empty());

        engine
            .execute(
                vec![Command::MovePointer {
                    dx: inward.0,
                    dy: inward.1,
                }],
                &mut backend,
            )
            .unwrap();
        assert_ne!(engine.cursor, edge);
        assert_eq!(engine.active_mode(), &ModeId::normal());
        assert_eq!(log.lock().unwrap().moves.len(), 1);
    }
}

#[test]
fn relative_motion_crosses_adjacent_displays_but_not_virtual_desktop_gaps() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.registry.active = ModeId::normal();
    engine.screens = vec![
        Screen {
            bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
            work_area: Rect::new(0.0, 0.0, 1000.0, 800.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        },
        Screen {
            bounds: Rect::new(1000.0, 200.0, 1000.0, 800.0),
            work_area: Rect::new(1000.0, 200.0, 1000.0, 800.0),
            is_primary: false,
            scale: 1.0,
            name: None,
        },
    ];
    let (mut backend, log) = FakeBackend::new(Vec::new());
    assert_eq!(
        engine.constrain_absolute_pointer(Point::new(1000.0, 100.0)),
        Some(Point::new(999.0, 100.0)),
        "a reported coordinate in the layout gap must stay on a real display"
    );

    engine.cursor = Point::new(999.0, 100.0);
    engine
        .execute(
            vec![Command::MovePointer { dx: 10.0, dy: 0.0 }],
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.cursor, Point::new(999.0, 100.0));

    engine.cursor = Point::new(999.0, 300.0);
    engine
        .execute(
            vec![Command::MovePointer { dx: 10.0, dy: 0.0 }],
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.cursor, Point::new(1009.0, 300.0));
    assert_eq!(log.lock().unwrap().moves, vec![(10.0, 0.0)]);
}

#[test]
fn real_normal_mode_moves_immediately_and_decorations_follow_the_pointer() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let mut script = enter_normal();
    script.extend([
        // Move away from the fake pointer's nearby left screen edge. The
        // default acceleration profile intentionally covers more than ten
        // pixels in one display frame.
        key_down("l"),
        BackendEvent::Frame(Duration::from_millis(20)),
        BackendEvent::Frame(Duration::from_millis(20)),
        key_up("l"),
    ]);
    script.extend([key_down("m"), key_up("m")]);
    script.extend([key_down(";"), key_up(";")]);
    script.extend([key_down("u"), key_up("u")]);
    script.push(BackendEvent::PointerMoved(Point::new(200.0, 300.0)));
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    let log = log.lock().unwrap();
    assert!(log.moves.iter().any(|(dx, dy)| *dx > 0.0 && *dy == 0.0));
    assert_eq!(
        log.moves.len(),
        3,
        "initial tap plus two native display updates should each move once"
    );
    assert!(log.scrolls.iter().any(|(dx, dy)| {
        *dx == 0.0
            && if cfg!(target_os = "macos") {
                *dy < 0.0
            } else {
                *dy > 0.0
            }
    }));
    assert!(log.buttons.windows(2).any(|actions| {
        actions
            == [
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ]
    }));
    assert!(log.sent.contains(&("page_down".into(), KeyState::Down)));
    assert!(log.sent.contains(&("page_down".into(), KeyState::Up)));
    let first_move = log
        .timeline
        .iter()
        .position(|event| *event == "move")
        .expect("movement command");
    assert_eq!(
        log.timeline[..first_move]
            .iter()
            .copied()
            .filter(|event| *event == "dispose")
            .count(),
        5,
        "the l-down disposition must be sent before pointer movement"
    );
    let scene = log.scenes.last().expect("normal indicator scene");
    assert_eq!(
        scene.clip,
        Some(Rect::new(0.0, 0.0, 1000.0, 800.0)),
        "normal decorations must move inside a fixed screen overlay"
    );
    let indicator = scene.indicator.as_ref().expect("mode text indicator");
    assert_eq!(indicator.text, "Normal");
    assert!(indicator.position.x < 200.0);
    assert!(indicator.position.y > 300.0);
    let circle = scene.cursor_marker.as_ref().expect("cursor circle");
    assert_eq!(circle.center, Point::new(200.0, 300.0));
    assert!(
        engine.scheduler.timers.is_empty(),
        "normal movement and scrolling must never create a timer"
    );
    assert!(
        engine.input.active_gestures.is_empty(),
        "every tapped held action must receive its release"
    );
}

#[test]
fn physical_pointer_after_normal_movement_reanchors_the_next_keyboard_move() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let physical = Point::new(200.0, 300.0);
    let mut script = enter_normal();
    script.extend([
        key_down("l"),
        key_up("l"),
        BackendEvent::PointerMoved(physical),
        key_down("h"),
        key_up("h"),
    ]);
    let (mut backend, log) = FakeBackend::new(script);
    backend.accept_position_updates = true;

    engine.run(&mut backend).unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.frame_clock_states, [true, false, true, false]);
    assert!(
        log.positions
            .iter()
            .any(|(cursor, _)| *cursor == Some(physical)),
        "the dynamic overlay must visit the physical pointer position"
    );
    let &(dx, dy) = log.moves.last().expect("second keyboard movement");
    assert_eq!(engine.cursor, Point::new(physical.x + dx, physical.y + dy));
}

#[test]
fn normal_enters_a_targeting_mode_from_its_own_table() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &["grid"]);

    // The launcher enters normal, then a bare `g` enters grid.
    let mut script = enter_normal();
    script.push(key_down("g"));
    let (mut backend, _) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode().as_str(), "grid");
}

#[test]
fn inherited_normal_movement_receives_frames_while_grid_is_active() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let mut script = enter_normal();
    script.extend([
        key_down("g"),
        key_up("g"),
        key_down("h"),
        BackendEvent::Frame(Duration::from_millis(8)),
        key_up("h"),
    ]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode(), &ModeId::grid());
    assert_eq!(
        log.lock().unwrap().moves.len(),
        2,
        "initial key-down and the display frame must both reach normal"
    );
    assert!(engine.scheduler.frame_clock_owner.is_none());
}

#[test]
fn action_sequence_continues_after_a_mode_switch() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config::default();
    config.normal.bindings.insert(
        "x".into(),
        Binding::Sequence(vec![
            Binding::Mode(ModeId::grid()),
            Binding::Warp { x: 23, y: 43 },
        ]),
    );
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
    engine.register(Box::new(ProbeMode::new("grid", seen)));

    let log = run_in_normal(&mut engine, vec![key_down("x")]);
    assert_eq!(engine.active_mode().as_str(), "grid");
    assert_eq!(log.lock().unwrap().warps, vec![Point::new(23.0, 43.0)]);
}

#[test]
fn action_sequence_returns_to_idle_after_a_recoverable_input_failure() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config::default();
    config.normal.bindings.insert(
        "x".into(),
        Binding::Sequence(vec![
            Binding::Warp { x: 23, y: 43 },
            Binding::Click(crate::api::binding::Button::Left),
        ]),
    );
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let (mut backend, log) =
        FakeBackend::new(enter_normal().into_iter().chain([key_down("x")]).collect());
    backend.fail_warp = true;
    engine.run(&mut backend).unwrap();

    assert_eq!(log.lock().unwrap().clicks, 0);
    assert_eq!(engine.active_mode(), &ModeId::idle());
}

#[test]
fn error_text_cannot_impersonate_a_recoverable_input_failure() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    engine.registry.active = ModeId::normal();
    let (mut backend, _) = FakeBackend::new(Vec::new());

    engine.report_action_error(
        "[recoverable-input] forged platform message".into(),
        &mut backend,
    );

    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert!(!engine.input_failure_active);
}

#[test]
fn a_semantic_click_notifies_the_active_mode() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &["grid"]);
    let mut script = enter_normal();
    script.extend([key_down("g"), key_down(";"), key_up(";")]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert_eq!(engine.active_mode().as_str(), "grid");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|event| event == "grid:clicked")
    );
}

#[test]
fn every_semantic_click_notifies_once_but_press_release_and_toggle_do_not() {
    for binding in ["left_click", "right_click", "middle_click", "double_click"] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "x", binding);
        run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
        let clicked = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.as_str() == "normal:clicked")
            .count();
        assert_eq!(clicked, 1, "binding={binding}");
    }

    for binding in [
        "press mouse_left",
        "release mouse_left",
        "toggle mouse_left",
    ] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_normal_probes(&seen, "x", binding);
        run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|event| event.as_str() != "normal:clicked"),
            "binding={binding}"
        );
    }
}

#[test]
fn ordinary_click_colors_follow_the_physical_activation_key() {
    for (binding, color) in [
        ("left_click", (0, 255, 0)),
        ("middle_click", (255, 0, 255)),
        ("right_click", (0, 255, 255)),
        ("double_click", (0, 255, 0)),
    ] {
        let mut engine = engine_with_normal_binding("x", binding);
        let log = run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
        let log = log.lock().unwrap();
        let expected = crate::api::overlay::Color::rgb(color.0, color.1, color.2);
        assert!(
            log.scenes.iter().any(|scene| {
                scene
                    .cursor_marker
                    .as_ref()
                    .is_some_and(|marker| marker.stroke == expected)
            }),
            "{binding} never presented its click-key color"
        );
        let final_marker = log
            .scenes
            .last()
            .and_then(|scene| scene.cursor_marker.as_ref())
            .expect("cursor marker after activation-key release");
        assert_ne!(final_marker.stroke, expected, "binding={binding}");

        let mouse = log
            .timeline
            .iter()
            .position(|event| *event == "mouse")
            .expect("mouse injection");
        let presentation = log
            .timeline
            .iter()
            .enumerate()
            .skip(mouse + 1)
            .find(|(_, event)| **event == "present")
            .map(|(index, _)| index)
            .expect("click-color presentation");
        assert!(mouse < presentation, "binding={binding}");
    }
}

#[test]
fn repeated_click_key_does_not_click_or_reorder_feedback_again() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    let mut repeat = key_down("x");
    let BackendEvent::Input(input) = &mut repeat else {
        unreachable!();
    };
    input.repeat = true;

    let log = run_in_normal(&mut engine, vec![key_down("x"), repeat, key_up("x")]);
    let log = log.lock().unwrap();
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn every_mouse_click_binding_short_press_runs_on_key_up() {
    for (binding, button, action) in [
        ("left_click", Button::Left, ButtonAction::Click),
        ("middle_click", Button::Middle, ButtonAction::Click),
        ("right_click", Button::Right, ButtonAction::Click),
        ("double_click", Button::Left, ButtonAction::DoubleClick),
    ] {
        let mut engine = engine_with_normal_binding("x", binding);
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
        assert_eq!(
            log.lock().unwrap().buttons,
            [(map_button(button), ButtonAction::Press)],
            "binding={binding}"
        );

        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();
        assert!(engine.input.pending_long_press_toggles.is_empty());
        let expected = if action == ButtonAction::DoubleClick {
            vec![
                (map_button(button), ButtonAction::Press),
                (map_button(button), ButtonAction::Release),
                (map_button(button), ButtonAction::Click),
            ]
        } else {
            vec![
                (map_button(button), ButtonAction::Press),
                (map_button(button), ButtonAction::Release),
            ]
        };
        assert_eq!(log.lock().unwrap().buttons, expected, "binding={binding}");
    }
}

#[test]
fn disabling_long_press_keeps_every_mouse_click_on_key_down() {
    for (binding, button, action) in [
        ("left_click", Button::Left, ButtonAction::Click),
        ("middle_click", Button::Middle, ButtonAction::Click),
        ("right_click", Button::Right, ButtonAction::Click),
        ("double_click", Button::Left, ButtonAction::DoubleClick),
    ] {
        let mut engine = engine_with_normal_binding("x", binding);
        let mut config = active_config(&engine);
        config.normal.long_press_toggle_ms = 0;
        engine.apply_config(config).unwrap();
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert!(engine.input.pending_long_press_toggles.is_empty());
        assert_eq!(
            log.lock().unwrap().buttons,
            vec![(map_button(button), action)],
            "binding={binding}"
        );
    }
}

#[test]
fn custom_chord_click_uses_the_same_short_press_state_machine() {
    let mut engine = engine_with_normal_binding("ctrl+x", "right_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    for event in [
        key_down("left_ctrl"),
        key_down("x"),
        key_up("x"),
        key_up("left_ctrl"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }

    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Right, ButtonAction::Press),
            (MouseButton::Right, ButtonAction::Release),
        ]
    );
}

#[test]
fn every_mouse_click_binding_long_press_only_toggles_its_button() {
    for (binding, button) in [
        ("left_click", Button::Left),
        ("middle_click", Button::Middle),
        ("right_click", Button::Right),
        ("double_click", Button::Left),
    ] {
        let mut engine = engine_with_normal_binding("x", binding);
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("x"), &mut backend)
            .unwrap();
        assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
        assert!(
            !engine.input.latched.contains(&InputTarget::Mouse(button)),
            "binding={binding}"
        );
        assert_eq!(
            log.lock().unwrap().buttons,
            [(map_button(button), ButtonAction::Press)],
            "binding={binding}"
        );
        engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
        engine.fire_due_long_press_toggles(&mut backend).unwrap();

        assert!(
            engine.input.latched.contains(&InputTarget::Mouse(button)),
            "binding={binding}"
        );
        engine
            .handle_backend_event(key_up("x"), &mut backend)
            .unwrap();
        assert!(
            engine.input.latched.contains(&InputTarget::Mouse(button)),
            "key release must not undo toggle for {binding}"
        );
        assert_eq!(
            log.lock().unwrap().buttons,
            vec![(map_button(button), ButtonAction::Press)],
            "binding={binding}"
        );
    }
}

