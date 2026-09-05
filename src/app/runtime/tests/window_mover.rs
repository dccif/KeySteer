fn chord_test_engine(config: &Config) -> Engine {
    Engine::from_plan(crate::app::configuration::compile(config).unwrap(), Appearance::Dark).unwrap()
}

fn display_primary() -> String {
    KeyChord::parse(&normal_launcher()).unwrap().keys()[0].as_str().to_owned()
}

#[test]
fn default_long_chord_moves_the_window_without_moving_the_pointer_first() {
    let primary = display_primary();
    use crate::api::command::WindowScreenTarget;
    let mut engine = chord_test_engine(&Config::default());
    let mut script = enter_normal();
    script.extend([
        key_down(&primary),
        key_down("s"),
        key_down("d"),
        key_up("d"),
        key_up("s"),
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
fn custom_nested_chords_use_the_same_arbitration_for_host_actions() {
    let config = Config::parse(r#"
        [normal.bindings]
        "ctrl+x" = "send home"
        "ctrl+x+c" = "send end"
        "ctrl+x+c+v" = "send page_down"
    "#).unwrap();
    for (letters, expected) in [(vec!["x"], "home"), (vec!["x", "c"], "end"), (vec!["x", "c", "v"], "page_down")] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.push(key_down("left_ctrl"));
        script.extend(letters.iter().map(|key| key_down(key)));
        script.extend(letters.iter().rev().map(|key| key_up(key)));
        script.push(key_up("left_ctrl"));
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert_eq!(log.lock().unwrap().sent, [(expected.into(), KeyState::Down), (expected.into(), KeyState::Up)]);
    }
}

#[test]
fn disabled_continuations_do_not_delay_the_short_chord() {
    let primary = display_primary();
    let config = Config::parse(r#"
        [normal.bindings]
        "primary+s+d" = "none"
    "#).unwrap();
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
    let config = Config::parse(r#"
        [key_aliases.windows]
        primary = "right_alt"
        [normal.bindings]
        "primary+x" = "screen next"
        "primary+x+z" = "move_window 2"
    "#).unwrap();
    let mut engine = chord_test_engine(&config);
    let script = vec![key_down("right_alt"), key_down("e"), key_up("e"), key_down("x"), key_down("z"), key_up("z"), key_up("x"), key_up("right_alt")];
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.window_moves, [WindowScreenTarget::Index(1)]);
    assert!(log.warps.is_empty());
}

#[test]
fn inherited_continuations_respect_a_local_disabled_binding() {
    for disabled in [false, true] {
        let mut config = Config::parse(r#"
            [normal.bindings]
            g = "grid"
            "ctrl+f1+f2" = "move_window next"
            [grid.bindings]
            "ctrl+f1" = "send home"
        "#).unwrap();
        if disabled { config.grid.bindings.insert("ctrl+f1+f2".into(), Binding::Disabled); }
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend([key_down("g"), key_up("g"), key_down("left_ctrl"), key_down("f1")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert_eq!(log.lock().unwrap().sent.len(), 2 * usize::from(disabled), "active={} grid={:?} pending={:?}", engine.active_mode(), engine.bindings_in(&ModeId::grid()), engine.input.pending_chords);
    }
}

#[test]
fn pending_chords_are_cancelled_when_modes_or_profiles_change() {
    let config = Config::parse(r#"
        [normal.bindings]
        "ctrl+x" = "send home"
        "ctrl+x+c" = "send end"
        "ctrl+q" = "idle"
        [[normal.app_configs]]
        bundle_id = "changed-app"
        bindings = { "ctrl+x+c" = "none" }
    "#).unwrap();
    for change in [key_down("q"), BackendEvent::ToggleEnabled, BackendEvent::FocusChanged(Some(FocusedApp {
        bundle_id: "changed-app".into(), window_title: "Changed".into(), process_id: 42,
    }))] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend([key_down("left_ctrl"), key_down("x"), change, key_up("x"), key_up("left_ctrl")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        assert!(log.lock().unwrap().sent.is_empty());
    }
}

#[test]
fn opposite_side_continuations_do_not_delay_a_generic_short_chord() {
    let config = Config::parse(r#"
        [normal.bindings]
        "alt+x" = "send home"
        "left_alt+x+c" = "send end"
    "#).unwrap();
    let mut engine = chord_test_engine(&config);
    let mut script = enter_normal();
    script.extend([key_down("right_alt"), key_down("x")]);
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();
    assert_eq!(log.lock().unwrap().sent, [("home".into(), KeyState::Down), ("home".into(), KeyState::Up)]);
}

#[test]
fn click_prefix_is_not_injected_when_a_longer_chord_wins() {
    let config = Config::parse(r#"
        [normal.bindings]
        "ctrl+x" = "left_click"
        "ctrl+x+c" = "move_window next"
    "#).unwrap();
    for complete_long in [false, true] {
        let mut engine = chord_test_engine(&config);
        let mut script = enter_normal();
        script.extend([key_down("left_ctrl"), key_down("x")]);
        if complete_long { script.extend([key_down("c"), key_up("c")]); }
        script.extend([key_up("x"), key_up("left_ctrl")]);
        let (mut backend, log) = FakeBackend::new(script);
        engine.run(&mut backend).unwrap();
        let log = log.lock().unwrap();
        if complete_long { assert!(log.buttons.is_empty()); assert_eq!(log.window_moves.len(), 1); }
        else { assert_eq!(log.buttons, [(MouseButton::Left, ButtonAction::Click)]); }
    }
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
    engine.execute_for(&ModeId::normal(), [Command::MoveWindowToScreen(WindowScreenTarget::Next)], &mut backend).unwrap();
    assert_eq!(engine.cursor, Point::new(1250.0, 200.0));
    assert_eq!(log.lock().unwrap().warps, [engine.cursor]);
    engine.execute_for(&ModeId::normal(), [Command::MovePointer { dx: 5.0, dy: 0.0 }], &mut backend).unwrap();
    assert_eq!(engine.cursor, Point::new(1255.0, 200.0));
}

#[test]
fn window_noop_failure_or_failed_warp_does_not_update_cursor() {
    use crate::api::command::WindowScreenTarget;
    for (destination, fail_window, fail_warp) in [(None, false, false), (Some(Point::new(200.0, 100.0)), true, false), (Some(Point::new(200.0, 100.0)), false, true)] {
        let mut engine = chord_test_engine(&Config::default());
        let (mut backend, log) = FakeBackend::new(vec![]);
        engine.screens = backend.screens().unwrap();
        engine.cursor = Point::new(10.0, 10.0);
        backend.window_move_pointer = destination;
        backend.fail_window_move = fail_window;
        backend.fail_warp = fail_warp;
        let result = engine.execute_for(&ModeId::normal(), [Command::MoveWindowToScreen(WindowScreenTarget::Next)], &mut backend);
        assert_eq!(result.is_err(), fail_window || fail_warp);
        assert!(log.lock().unwrap().warps.is_empty());
        assert_eq!(engine.cursor, Point::new(10.0, 10.0));
    }
}
