#[test]
fn finish_click_chain_is_idempotent_and_does_not_click_recursively() {
    let mut config = Config::default();
    config.grid.max_depth = 1;
    config.grid.lifecycle.after_finish = crate::config::LifecycleAction::Click {
        button: MouseButton::Left,
        action: ButtonAction::Click,
    };
    // The resulting Clicked event asks to finish again. Since the grid is
    // already finished this must be a no-op, not another click.
    config.grid.lifecycle.after_click = crate::config::LifecycleAction::Finish;
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let mut script = enter_normal();
    script.extend([key_down("g"), key_up("g"), key_down("1"), key_up("1")]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Click)]
    );
    assert_eq!(engine.active_mode(), &ModeId::grid());
}

#[test]
fn clicking_before_grid_max_depth_finishes_and_returns_to_normal_by_default() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let mut script = enter_normal();
    script.extend([key_down("g"), key_up("g"), key_down(";"), key_up(";")]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert!(log.lock().unwrap().dismissals > 0);
}

#[test]
fn bindings_are_scoped_to_the_mode_that_declares_them() {
    // `g` belongs to normal, so it must do nothing while idle. This is what
    // keeps the program silent until it is asked for.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &["grid"]);

    let (mut backend, log) = FakeBackend::new(vec![key_down("g")]);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode().as_str(), "idle");
    assert!(
        log.lock()
            .unwrap()
            .dispositions
            .iter()
            .all(|d| *d == KeyDisposition::Forward),
        "idle must not swallow keys"
    );
}

#[test]
fn pressing_an_active_modes_own_key_returns_to_idle() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);

    // Enter normal, then use its bare q exit binding.
    let mut script = enter_normal();
    script.push(key_down("q"));
    let (mut backend, _) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode().as_str(), "idle");
}

#[test]
fn escape_binding_returns_to_idle() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);

    let mut script = enter_normal();
    script.push(key_down("q"));
    let (mut backend, _) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode().as_str(), "idle");
}

#[test]
fn temporary_modifier_changes_only_the_display_mode() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::grid();
    assert_eq!(engine.display_mode(), ModeId::grid());

    let primary = Key::new(&config.grid.temporary_mode_keys[0]).unwrap();
    engine.input.pressed.insert(primary.clone());
    assert_eq!(engine.display_mode(), ModeId::normal());
    assert_eq!(
        engine.registry.active,
        ModeId::grid(),
        "grid state remains active"
    );

    engine.input.pressed.remove(&primary);
    assert_eq!(engine.display_mode(), ModeId::grid());
    assert_eq!(engine.registry.active, ModeId::grid());
}

#[test]
fn temporary_modifier_repaints_badge_without_leaving_grid() {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let primary = Key::new(&config.grid.temporary_mode_keys[0]).unwrap();
    let input = |state| {
        BackendEvent::Input(InputEvent {
            character: None,
            key: primary.clone(),
            state,
            repeat: false,
            injected: false,
            timestamp_millis: 0,
        })
    };
    let mut script = enter_normal();
    script.extend([key_down("g"), key_up("g")]);
    script.extend([input(KeyState::Down), input(KeyState::Up)]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode(), &ModeId::grid());
    let indicators: Vec<String> = log
        .lock()
        .unwrap()
        .scenes
        .iter()
        .filter_map(|scene| scene.indicator.as_ref().map(|item| item.text.clone()))
        .collect();
    assert!(
        indicators
            .windows(3)
            .any(|values| { values[0] == "Grid" && values[1] == "Normal" && values[2] == "Grid" }),
        "temporary indicator sequence missing: {indicators:?}"
    );
}

#[test]
fn primary_q_returns_every_configurable_targeting_mode_to_normal() {
    for target in [ModeId::grid(), ModeId::recursive_grid(), ModeId::ui_hint()] {
        let config = Config::default();
        let primary = Key::new(&config.grid.temporary_mode_keys[0]).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(config, Appearance::Dark);
        let mut idle = ProbeMode::new("idle", seen.clone());
        idle.captures = false;
        engine.register(Box::new(idle));
        engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
        engine.register(Box::new(ProbeMode::new(target.as_str(), seen)));
        let (mut backend, _) = FakeBackend::new(Vec::new());
        engine
            .activate(target.clone(), Some(ModeId::normal()), &mut backend)
            .unwrap();

        engine
            .handle_backend_event(
                BackendEvent::Input(InputEvent {
                    character: None,
                    key: primary.clone(),
                    state: KeyState::Down,
                    repeat: false,
                    injected: false,
                    timestamp_millis: 0,
                }),
                &mut backend,
            )
            .unwrap();
        engine
            .handle_backend_event(key_down("q"), &mut backend)
            .unwrap();

        assert_eq!(
            engine.active_mode(),
            &ModeId::normal(),
            "Primary+Q should leave {target} for normal"
        );
    }
}

#[test]
fn label_modes_use_normal_pointer_controls_only_when_allowed_or_temporary() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let log = Arc::new(Mutex::new(Vec::new()));
    engine.register(Box::new(ProbeMode::new("normal", log.clone())));
    engine.register(Box::new(ProbeMode::new("grid", log.clone())));
    engine.register(Box::new(ProbeMode::new("recursive_grid", log)));

    engine.registry.active = ModeId::grid();
    engine.input.pressed.insert(Key::new("g").unwrap());
    assert!(
        engine.lookup(&Key::new("g").unwrap()).is_none(),
        "the grid's raw label must beat inherited normal bindings"
    );
    engine.input.pressed.clear();
    engine.input.pressed.insert(Key::new("h").unwrap());
    let inherited = engine.lookup(&Key::new("h").unwrap()).unwrap();
    assert_eq!(inherited.owner, ModeId::normal());

    engine.input.pressed.insert(Key::new("left_ctrl").unwrap());
    let resolved = engine.lookup(&Key::new("h").unwrap()).unwrap();
    assert_eq!(resolved.owner, ModeId::normal());
    assert_eq!(resolved.binding.as_ref(), &Binding::Move(Direction::Left));

    engine.input.pressed.clear();
    engine.input.pressed.insert(Key::new("h").unwrap());
    engine.registry.active = ModeId::recursive_grid();
    let resolved = engine.lookup(&Key::new("h").unwrap()).unwrap();
    assert_eq!(resolved.owner, ModeId::normal());
    assert_eq!(resolved.binding.as_ref(), &Binding::Move(Direction::Left));
}

#[test]
fn ui_hint_overlap_key_wins_over_inheritance_and_temporary_mode() {
    let mut config = Config::default();
    config.ui_hint.overlap_cycle_key = "alt".into();
    config.ui_hint.temporary_mode_keys = vec!["alt".into()];
    config.normal.bindings.insert(
        "left_alt".into(),
        Binding::Click(crate::api::binding::Button::Middle),
    );
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    let key = Key::new("left_alt").unwrap();
    engine.input.pressed.insert(key.clone());

    engine.registry.active = ModeId::ui_hint();
    assert!(
        engine.lookup(&key).is_none(),
        "the configured key must reach UI Hint as a raw hold event"
    );
    assert_eq!(
        engine.display_mode(),
        ModeId::ui_hint(),
        "the same temporary key must not hide overlap cycling"
    );

    engine.registry.active = ModeId::normal();
    assert!(
        engine.lookup(&key).is_some(),
        "the key must retain its Normal binding outside UI Hint"
    );
}

#[test]
fn a_longer_chord_wins_over_a_bare_key() {
    // A configured longer chord must win over its bare-key sibling.
    use crate::api::binding::{Direction, ScrollAmount};
    let mut config = Config::default();
    config.normal.bindings.insert(
        "e".into(),
        Binding::Scroll(Direction::Down, ScrollAmount::Step),
    );
    config.normal.bindings.insert(
        "shift+e".into(),
        Binding::Scroll(Direction::Down, ScrollAmount::Half),
    );
    let mut engine = Engine::new(config, Appearance::Dark);
    engine.register(Box::new(ProbeMode::new(
        "normal",
        Arc::new(Mutex::new(vec![])),
    )));
    engine.registry.active = ModeId::normal();

    engine.input.pressed.insert(Key::new("left_shift").unwrap());
    engine.input.pressed.insert(Key::new("e").unwrap());
    assert_eq!(
        engine
            .lookup(&Key::new("e").unwrap())
            .map(|resolved| resolved.binding.as_ref().clone()),
        Some(Binding::Scroll(Direction::Down, ScrollAmount::Half))
    );

    engine
        .input
        .pressed
        .remove(&Key::new("left_shift").unwrap());
    assert_eq!(
        engine
            .lookup(&Key::new("e").unwrap())
            .map(|resolved| resolved.binding.as_ref().clone()),
        Some(Binding::Scroll(Direction::Down, ScrollAmount::Step))
    );
}

#[test]
fn a_click_binding_is_executed_by_the_engine_not_the_mode() {
    let mut engine = engine_with_normal_binding("f", "left_click");
    let log = run_in_normal(&mut engine, vec![key_down("f"), key_up("f")]);
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn two_physical_taps_are_never_coalesced_or_suppressed_by_the_engine() {
    let mut engine = engine_with_normal_binding(";", "left_click");
    let log = run_in_normal(
        &mut engine,
        vec![key_down(";"), key_up(";"), key_down(";"), key_up(";")],
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn two_clicks_in_one_binding_sequence_reach_the_backend_in_order() {
    let mut engine = engine_with_normal_action(
        ";",
        Binding::Sequence(vec![
            Binding::Click(crate::api::binding::Button::Left),
            Binding::Click(crate::api::binding::Button::Left),
        ]),
    );
    let log = run_in_normal(&mut engine, vec![key_down(";")]);
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Click),
            (MouseButton::Left, ButtonAction::Click),
        ]
    );
}

#[test]
fn a_double_click_reaches_the_backend_as_one_native_action() {
    let mut engine = engine_with_normal_binding("f", "double_click");
    let log = run_in_normal(&mut engine, vec![key_down("f"), key_up("f")]);
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
            (MouseButton::Left, ButtonAction::Click),
        ]
    );
}

#[test]
fn a_send_binding_injects_the_keystroke() {
    let mut engine = engine_with_normal_binding("t", "home");
    let log = run_in_normal(&mut engine, vec![key_down("t")]);

    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("home".to_string(), KeyState::Down),
            ("home".into(), KeyState::Up)
        ]
    );
}

#[test]
fn held_bindings_reach_the_mode_on_both_edges() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_probes(&seen, "l", "move_right");
    run_in_normal(&mut engine, vec![key_down("l"), key_up("l")]);

    let log = seen.lock().unwrap().clone();
    let deliveries = log
        .iter()
        .filter(|e| e.contains("binding(move_right)"))
        .count();
    assert_eq!(deliveries, 2, "press and release both matter: {log:?}");
}

#[test]
fn discrete_bindings_ignore_auto_repeat() {
    let mut engine = engine_with_normal_binding("f", "left_click");
    let log = run_in_normal(
        &mut engine,
        vec![
            key_down("f"),
            BackendEvent::Input(InputEvent {
                character: None,
                key: Key::new("f").unwrap(),
                state: KeyState::Down,
                repeat: true,
                injected: false,
                timestamp_millis: 0,
            }),
            key_up("f"),
        ],
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ],
        "repeat must not re-click"
    );
}

#[test]
fn held_sequence_repeat_does_not_repeat_discrete_actions() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config.normal.bindings.insert(
        "f".into(),
        Binding::Sequence(vec![
            Binding::parse("move_right").unwrap(),
            Binding::parse("left_click").unwrap(),
        ]),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));
    let log = run_in_normal(
        &mut engine,
        vec![
            key_down("f"),
            BackendEvent::Input(InputEvent {
                character: None,
                key: Key::new("f").unwrap(),
                state: KeyState::Down,
                repeat: true,
                injected: false,
                timestamp_millis: 0,
            }),
            key_up("f"),
        ],
    );
    assert_eq!(log.lock().unwrap().clicks, 1);
}

#[test]
fn held_sequence_rejects_mode_changes_before_starting() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, _) = FakeBackend::new(Vec::new());
    let resolved = ResolvedBinding {
        binding: Arc::new(Binding::Sequence(vec![
            Binding::parse("move_right").unwrap(),
            Binding::Mode(ModeId::grid()),
        ])),
        owner: ModeId::normal(),
    };
    let input = InputEvent {
        character: None,
        key: Key::new("f").unwrap(),
        state: KeyState::Down,
        repeat: false,
        injected: false,
        timestamp_millis: 0,
    };
    assert!(
        engine
            .apply_binding(resolved, &input, &mut backend)
            .unwrap_err()
            .contains("mode-changing")
    );
}

#[test]
fn follow_binding_is_dispatched_once_to_the_active_mode() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_probes(&seen, "`", "follow");
    run_in_normal(
        &mut engine,
        vec![
            key_down("`"),
            BackendEvent::Input(InputEvent {
                character: None,
                key: Key::new("`").unwrap(),
                state: KeyState::Down,
                repeat: true,
                injected: false,
                timestamp_millis: 0,
            }),
        ],
    );

    let log = seen.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|entry| entry.as_str() == "normal:binding(follow)")
            .count(),
        1,
        "follow must be a single mode binding, got {log:?}"
    );
}
