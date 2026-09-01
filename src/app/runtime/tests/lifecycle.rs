#[test]
fn toggled_inputs_survive_targeting_but_release_on_normal_or_idle_entry() {
    let mut engine = engine_with_normal_binding("n", "toggle left_shift mouse_left");
    engine.register(Box::new(ProbeMode::new(
        "grid",
        Arc::new(Mutex::new(Vec::new())),
    )));
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
        .unwrap();

    engine
        .handle_backend_event(key_down("n"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("n"), &mut backend)
        .unwrap();
    let grid = ModeId::new("grid").unwrap();
    engine
        .activate(grid.clone(), Some(ModeId::normal()), &mut backend)
        .unwrap();
    let screens = backend.screens().unwrap();
    engine
        .handle_backend_event(BackendEvent::ScreensChanged(screens), &mut backend)
        .unwrap();

    assert_eq!(
        log.lock().unwrap().sent,
        [("left_shift".into(), KeyState::Down)],
        "targeting modes and screen changes must preserve the toggle session"
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );
    assert_eq!(engine.input.latched.len(), 2);

    engine
        .activate(ModeId::normal(), Some(grid.clone()), &mut backend)
        .unwrap();
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        [
            ("left_shift".into(), KeyState::Down),
            ("left_shift".into(), KeyState::Up),
        ]
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );

    for event in [key_down("n"), key_up("n")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    engine
        .activate(grid.clone(), Some(ModeId::normal()), &mut backend)
        .unwrap();
    engine
        .activate(ModeId::idle(), Some(grid), &mut backend)
        .unwrap();

    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        [
            ("left_shift".into(), KeyState::Down),
            ("left_shift".into(), KeyState::Up),
            ("left_shift".into(), KeyState::Down),
            ("left_shift".into(), KeyState::Up),
        ]
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
fn popping_a_targeting_mode_to_normal_releases_the_toggle_session() {
    let mut engine = engine_with_normal_binding("n", "toggle left_ctrl mouse_left");
    engine.register(Box::new(ProbeMode::new(
        "grid",
        Arc::new(Mutex::new(Vec::new())),
    )));
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
        .unwrap();

    for event in [key_down("n"), key_up("n")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    engine
        .push_mode(ModeId::new("grid").unwrap(), &mut backend)
        .unwrap();
    assert_eq!(engine.input.latched.len(), 2);
    engine.pop_mode(&mut backend).unwrap();

    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        [
            ("left_ctrl".into(), KeyState::Down),
            ("left_ctrl".into(), KeyState::Up),
        ]
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn entering_idle_releases_a_pending_immediate_mouse_down_without_clicking() {
    let mut engine = engine_with_normal_binding(";", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down(";"), &mut backend)
        .unwrap();
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );

    engine
        .activate(ModeId::idle(), Some(ModeId::normal()), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up(";"), &mut backend)
        .unwrap();

    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(engine.input.latched.is_empty());
    let log = log.lock().unwrap();
    assert_eq!(log.clicks, 0);
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn a_binding_to_an_unregistered_mode_is_ignored() {
    let mut engine = engine_with_normal_binding("z", "plugin:missing");
    run_in_normal(&mut engine, vec![key_down("z")]);
    assert_eq!(engine.active_mode().as_str(), "normal");
}

#[test]
fn unbound_keys_still_reach_the_mode_for_label_input() {
    // Grid and hint modes read raw characters this way.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_probes(&seen, "l", "move_right");
    run_in_normal(&mut engine, vec![key_down("x")]);

    let log = seen.lock().unwrap().clone();
    assert!(log.contains(&"normal:key".to_string()), "{log:?}");
}

#[test]
fn capturing_mode_consumes_keys_and_idle_forwards_them() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));

    let (mut backend, log) = FakeBackend::new(vec![key_down("x")]);
    engine.run(&mut backend).unwrap();
    assert_eq!(
        log.lock().unwrap().dispositions,
        vec![KeyDisposition::Forward]
    );

    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("idle", seen.clone())));
    let (mut backend, log) = FakeBackend::new(vec![key_down("x")]);
    engine.run(&mut backend).unwrap();
    assert_eq!(
        log.lock().unwrap().dispositions,
        vec![KeyDisposition::Consume]
    );
}

#[test]
fn injected_keys_are_never_dispatched_to_modes() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("idle", seen.clone())));

    let (mut backend, _) = FakeBackend::new(vec![BackendEvent::Input(InputEvent {
        key: Key::new("g").unwrap(),
        state: KeyState::Down,
        repeat: false,
        injected: true,
        timestamp_millis: 0,
    })]);
    engine.run(&mut backend).unwrap();

    let log = seen.lock().unwrap().clone();
    assert!(!log.iter().any(|e| e.ends_with(":key")), "{log:?}");
}

