fn drag_auto_release_engine() -> Engine {
    let mut config = Config::default();
    config.normal.auto_release_ms = 300;
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::normal();
    engine.cursor = Point::new(400.0, 300.0);
    engine.screens = vec![Screen {
        bounds: Rect::new(0.0, 0.0, 1_000.0, 800.0),
        work_area: Rect::new(0.0, 0.0, 1_000.0, 800.0),
        is_primary: true,
        scale: 1.0,
        name: None,
    }];
    engine
}

fn latch_drag_button(engine: &mut Engine, key: &str, backend: &mut dyn Backend) {
    engine.handle_backend_event(key_down(key), backend).unwrap();
    engine
        .input
        .pending_long_press_toggles
        .last_mut()
        .expect("pending long press")
        .fires_at = Instant::now();
    engine.fire_due_long_press_toggles(backend).unwrap();
    engine.handle_backend_event(key_up(key), backend).unwrap();
}

#[test]
fn drag_auto_release_requires_a_forwarded_modifier_and_real_movement() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);

    assert_eq!(
        engine.input.drag_auto_release.buttons,
        drag_button_bit(Button::Left)
    );
    assert!(engine.input.drag_auto_release.fires_at.is_none());

    engine
        .execute([Command::MovePointer { dx: 10.0, dy: 0.0 }], &mut backend)
        .unwrap();
    assert!(
        engine.input.drag_auto_release.fires_at.is_none(),
        "movement without a forwarded modifier must keep manual release"
    );

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    assert_ne!(engine.input.drag_auto_release.modifiers, 0);
    assert!(engine.input.drag_auto_release.fires_at.is_none());

    engine
        .handle_backend_event(BackendEvent::PointerMoved(engine.cursor), &mut backend)
        .unwrap();
    engine
        .execute(
            [Command::WarpPointer {
                x: engine.cursor.x,
                y: engine.cursor.y,
            }],
            &mut backend,
        )
        .unwrap();
    assert!(
        engine.input.drag_auto_release.fires_at.is_none(),
        "a coordinate report or warp without displacement must not arm release"
    );

    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(450.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.drag_auto_release.fires_at.is_some());
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );
}

#[test]
fn drag_auto_release_forwards_multiple_modifiers_and_moves_with_a_bare_binding() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);

    const MODIFIERS: [&str; 8] = [
        "left_shift",
        "right_shift",
        "left_ctrl",
        "right_ctrl",
        "left_alt",
        "right_alt",
        "left_win",
        "right_win",
    ];
    for key in MODIFIERS {
        engine
            .handle_backend_event(key_down(key), &mut backend)
            .unwrap();
    }
    assert_eq!(engine.input.drag_auto_release.modifiers.count_ones(), 8);

    engine
        .handle_backend_event(key_down("h"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("h"), &mut backend)
        .unwrap();
    assert!(
        !log.lock().unwrap().moves.is_empty(),
        "forwarded modifiers must not hide the effective Normal movement binding"
    );
    assert!(engine.input.drag_auto_release.fires_at.is_some());

    engine
        .handle_backend_event(key_up("left_alt"), &mut backend)
        .unwrap();
    assert_eq!(engine.input.drag_auto_release.modifiers.count_ones(), 7);
    for key in MODIFIERS.into_iter().filter(|key| *key != "left_alt") {
        engine
            .handle_backend_event(key_up(key), &mut backend)
            .unwrap();
    }
    assert_eq!(engine.input.drag_auto_release.modifiers, 0);
    assert!(
        engine.input.drag_auto_release.fires_at.is_some(),
        "an armed deadline survives an early release of every modifier"
    );

    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(430.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    assert!(
        engine
            .input
            .drag_auto_release
            .fires_at
            .is_some_and(|deadline| deadline > Instant::now()),
        "movement keeps resetting an already-armed deadline after every modifier is up"
    );

    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine.fire_due_drag_auto_release(&mut backend).unwrap();
    assert!(engine.input.drag_auto_release.fires_at.is_none());
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );

    let log = log.lock().unwrap();
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert_eq!(log.clicks, 0);
    assert_eq!(
        log.dispositions
            .iter()
            .filter(|disposition| **disposition == KeyDisposition::Forward)
            .count(),
        16,
        "every concrete modifier Down/Up pair must be forwarded"
    );
}

#[test]
fn drag_auto_release_refreshes_held_text_and_pressed_color() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);

    let pressed = crate::api::overlay::Color::rgb(0, 255, 0);
    {
        let log = log.lock().unwrap();
        let scene = log.scenes.last().expect("latched drag scene");
        assert_eq!(
            scene
                .indicator
                .as_ref()
                .and_then(|indicator| indicator.held_text.as_deref()),
            Some("● MOUSE LEFT")
        );
        assert_eq!(
            scene
                .cursor_marker
                .as_ref()
                .expect("latched cursor marker")
                .stroke,
            pressed
        );
    }

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    let scenes_before_release = log.lock().unwrap().scenes.len();
    engine.fire_due_drag_auto_release(&mut backend).unwrap();

    assert!(engine.input.latched.is_empty());
    let log = log.lock().unwrap();
    assert_eq!(log.scenes.len(), scenes_before_release + 1);
    let scene = log.scenes.last().expect("auto-released drag scene");
    assert_eq!(
        scene
            .indicator
            .as_ref()
            .and_then(|indicator| indicator.held_text.as_deref()),
        None
    );
    assert_ne!(
        scene
            .cursor_marker
            .as_ref()
            .expect("released cursor marker")
            .stroke,
        pressed
    );
    assert_eq!(log.clicks, 0);
}

#[test]
fn auto_release_clears_feedback_while_activation_key_is_still_down() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down(";"), &mut backend)
        .unwrap();
    engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
    engine.fire_due_long_press_toggles(&mut backend).unwrap();
    assert!(engine.input.active_click_indicators.is_empty());
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine.fire_due_drag_auto_release(&mut backend).unwrap();

    assert!(engine.input.active_click_indicators.is_empty());
    assert!(engine.input.latched.is_empty());
    let pressed = crate::api::overlay::Color::rgb(0, 255, 0);
    assert_ne!(
        log.lock()
            .unwrap()
            .scenes
            .last()
            .and_then(|scene| scene.cursor_marker.as_ref())
            .expect("released cursor marker")
            .stroke,
        pressed
    );

    engine
        .handle_backend_event(key_up(";"), &mut backend)
        .unwrap();
    let log = log.lock().unwrap();
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert_eq!(log.clicks, 0);
}

#[test]
fn auto_release_cancels_an_overlapping_late_toggle_for_the_same_button() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);
    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    engine
        .handle_backend_event(key_up("left_ctrl"), &mut backend)
        .unwrap();

    engine
        .handle_backend_event(key_down(";"), &mut backend)
        .unwrap();
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    assert!(
        engine.input.pending_long_press_toggles[0]
            .short_action
            .is_none()
    );
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine.fire_due_drag_auto_release(&mut backend).unwrap();

    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(engine.input.active_click_indicators.is_empty());
    engine
        .handle_backend_event(key_up(";"), &mut backend)
        .unwrap();
    assert!(engine.input.latched.is_empty());
    let log = log.lock().unwrap();
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert_eq!(log.clicks, 0);
}

#[test]
fn empty_drag_auto_release_does_not_redraw() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    let scenes_before = log.lock().unwrap().scenes.len();

    engine.fire_due_drag_auto_release(&mut backend).unwrap();

    assert_eq!(log.lock().unwrap().scenes.len(), scenes_before);
}

#[test]
fn excluded_focus_clears_pending_and_latched_click_feedback() {
    for long_pressed in [false, true] {
        let mut engine = drag_auto_release_engine();
        let mut config = active_config(&engine);
        config.general.excluded_apps = vec!["com.example.excluded".into()];
        engine.apply_config(config).unwrap();
        let (mut backend, log) = FakeBackend::new(Vec::new());
        if long_pressed {
            latch_drag_button(&mut engine, ";", &mut backend);
        } else {
            engine
                .handle_backend_event(key_down(";"), &mut backend)
                .unwrap();
        }

        engine
            .handle_backend_event(
                BackendEvent::FocusChanged(Some(FocusedApp {
                    bundle_id: "com.example.excluded".into(),
                    window_title: String::new(),
                    process_id: 1,
                })),
                &mut backend,
            )
            .unwrap();

        assert!(engine.input.pending_long_press_toggles.is_empty());
        assert!(engine.input.active_click_indicators.is_empty());
        assert!(engine.input.latched.is_empty());
        let log = log.lock().unwrap();
        let scene = log.scenes.last().expect("released excluded-app scene");
        assert_eq!(
            scene
                .indicator
                .as_ref()
                .and_then(|indicator| indicator.held_text.as_deref()),
            None,
            "long_pressed={long_pressed}"
        );
        assert_ne!(
            scene
                .cursor_marker
                .as_ref()
                .expect("released cursor marker")
                .stroke,
            crate::api::overlay::Color::rgb(0, 255, 0),
            "long_pressed={long_pressed}"
        );
        assert_eq!(
            log.buttons,
            [
                (MouseButton::Left, ButtonAction::Press),
                (MouseButton::Left, ButtonAction::Release),
            ],
            "long_pressed={long_pressed}"
        );
    }
}

#[test]
fn excluded_focus_clears_immediate_click_feedback_without_a_pending_toggle() {
    let mut engine = drag_auto_release_engine();
    let mut config = active_config(&engine);
    config.normal.long_press_toggle_ms = 0;
    config.general.excluded_apps = vec!["com.example.excluded".into()];
    engine.apply_config(config).unwrap();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down(";"), &mut backend)
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(!engine.input.active_click_indicators.is_empty());
    let scenes_before_focus = log.lock().unwrap().scenes.len();

    engine
        .handle_backend_event(
            BackendEvent::FocusChanged(Some(FocusedApp {
                bundle_id: "com.example.excluded".into(),
                window_title: String::new(),
                process_id: 1,
            })),
            &mut backend,
        )
        .unwrap();

    assert!(engine.input.active_click_indicators.is_empty());
    let log = log.lock().unwrap();
    assert_eq!(log.scenes.len(), scenes_before_focus + 1);
    assert_ne!(
        log.scenes
            .last()
            .and_then(|scene| scene.cursor_marker.as_ref())
            .expect("released cursor marker")
            .stroke,
        crate::api::overlay::Color::rgb(0, 255, 0)
    );
    assert_eq!(log.clicks, 1);
}

#[test]
fn drag_auto_release_preserves_keyboard_latch_feedback() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .press_targets(
            &[InputTarget::Key(Key::new("left_ctrl").unwrap())],
            &mut backend,
        )
        .unwrap();
    latch_drag_button(&mut engine, ";", &mut backend);
    engine
        .handle_backend_event(key_down("left_shift"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    engine.input.drag_auto_release.fires_at = Some(Instant::now());

    engine.fire_due_drag_auto_release(&mut backend).unwrap();

    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Key(Key::new("left_ctrl").unwrap()))
    );
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert_eq!(
        log.lock()
            .unwrap()
            .scenes
            .last()
            .and_then(|scene| scene.indicator.as_ref())
            .and_then(|indicator| indicator.held_text.as_deref()),
        Some("● LEFT CTRL")
    );
}

#[test]
fn drag_auto_release_keeps_specific_modifier_chords_ahead_of_move_fallback() {
    let mut engine = drag_auto_release_engine();
    let mut config = active_config(&engine);
    config
        .normal
        .bindings
        .insert("ctrl+shift+h".into(), Binding::parse("send home").unwrap());
    engine.apply_config(config).unwrap();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("left_shift"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("h"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("h"), &mut backend)
        .unwrap();

    let log = log.lock().unwrap();
    assert!(log.moves.is_empty());
    assert!(log.sent.contains(&("home".into(), KeyState::Down)));
    assert!(log.sent.contains(&("home".into(), KeyState::Up)));
}

#[test]
fn drag_auto_release_respects_an_exact_disabled_chord() {
    let mut engine = drag_auto_release_engine();
    let mut config = active_config(&engine);
    config
        .normal
        .bindings
        .insert("ctrl+h".into(), Binding::Disabled);
    engine.apply_config(config).unwrap();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("h"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("h"), &mut backend)
        .unwrap();

    assert!(
        log.lock().unwrap().moves.is_empty(),
        "an explicit `none` chord must not fall back to the bare movement binding"
    );
}

#[test]
fn drag_auto_release_respects_a_disabled_chord_from_normal_inheritance() {
    let mut engine = drag_auto_release_engine();
    let mut config = active_config(&engine);
    config.hotkeys.insert("ctrl+h".into(), Binding::Disabled);
    engine.apply_config(config).unwrap();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("h"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("h"), &mut backend)
        .unwrap();

    assert!(
        log.lock().unwrap().moves.is_empty(),
        "an inherited `none` chord must also block bare movement fallback"
    );
}

#[test]
fn drag_auto_release_releases_every_long_pressed_click_candidate() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);
    latch_drag_button(&mut engine, "'", &mut backend);
    assert_eq!(engine.input.drag_auto_release.buttons.count_ones(), 2);

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine.fire_due_drag_auto_release(&mut backend).unwrap();

    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Right, ButtonAction::Press),
            (MouseButton::Right, ButtonAction::Release),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn a_new_drag_button_waits_for_movement_instead_of_reusing_an_old_deadline() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, _) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);
    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine
        .handle_backend_event(key_up("left_ctrl"), &mut backend)
        .unwrap();

    latch_drag_button(&mut engine, "'", &mut backend);

    assert_eq!(engine.input.drag_auto_release.buttons.count_ones(), 2);
    assert!(
        engine.input.drag_auto_release.fires_at.is_none(),
        "a newly owned button must not inherit a nearly-expired deadline"
    );
    engine.fire_due_drag_auto_release(&mut backend).unwrap();
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Right))
    );

    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(440.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.drag_auto_release.fires_at.is_some());
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine.fire_due_drag_auto_release(&mut backend).unwrap();
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Right))
    );
}

#[test]
fn explicit_press_takes_over_an_existing_auto_drag_button() {
    let mut engine = drag_auto_release_engine();
    let mut config = active_config(&engine);
    config
        .normal
        .bindings
        .insert("ctrl+x".into(), Binding::parse("press mouse_left").unwrap());
    engine.apply_config(config).unwrap();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);
    latch_drag_button(&mut engine, "'", &mut backend);
    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.drag_auto_release.fires_at.is_some());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();

    assert_eq!(
        engine.input.drag_auto_release.buttons,
        drag_button_bit(Button::Right),
        "explicit ownership removes only the matching auto button"
    );
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Right))
    );
    engine.input.drag_auto_release.fires_at = Some(Instant::now());
    engine.fire_due_drag_auto_release(&mut backend).unwrap();
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Right))
    );
    assert_eq!(
        log.lock()
            .unwrap()
            .scenes
            .last()
            .and_then(|scene| scene.indicator.as_ref())
            .and_then(|indicator| indicator.held_text.as_deref()),
        Some("● MOUSE LEFT"),
        "automatic release must keep feedback for an explicitly owned button"
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Right, ButtonAction::Press),
            (MouseButton::Right, ButtonAction::Release),
        ],
        "the old deadline may release only the button it still owns"
    );
}

#[test]
fn parameterless_toggle_partner_takes_over_an_existing_auto_drag_button() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);
    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(420.0, 300.0)),
            &mut backend,
        )
        .unwrap();

    engine
        .handle_backend_event(key_down("n"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down(";"), &mut backend)
        .unwrap();

    assert_eq!(engine.input.drag_auto_release.buttons, 0);
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    engine.fire_due_drag_auto_release(&mut backend).unwrap();
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );
}

#[test]
fn config_reload_forgets_auto_release_without_releasing_the_existing_latch() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    latch_drag_button(&mut engine, ";", &mut backend);
    engine
        .handle_backend_event(key_down("left_ctrl"), &mut backend)
        .unwrap();
    assert_ne!(engine.input.drag_auto_release.buttons, 0);

    let mut replacement = active_config(&engine);
    replacement.normal.auto_release_ms = 50;
    engine.apply_config(replacement).unwrap();

    assert_eq!(engine.input.drag_auto_release.buttons, 0);
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    engine
        .handle_backend_event(key_up("left_ctrl"), &mut backend)
        .unwrap();
    engine
        .release_targets(&[InputTarget::Mouse(Button::Left)], false, &mut backend)
        .unwrap();
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ],
        "reload must leave an existing native latch under manual ownership until explicit release"
    );
    assert_eq!(
        log.dispositions
            .iter()
            .filter(|disposition| **disposition == KeyDisposition::Forward)
            .count(),
        2,
        "reload must preserve the forwarded modifier Down/Up disposition pair"
    );
}

#[test]
fn leaving_normal_releases_only_the_auto_managed_drag_candidate() {
    let mut engine = drag_auto_release_engine();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .press_targets(
            &[InputTarget::Key(Key::new("left_ctrl").unwrap())],
            &mut backend,
        )
        .unwrap();
    latch_drag_button(&mut engine, ";", &mut backend);

    engine
        .activate(ModeId::grid(), Some(ModeId::normal()), &mut backend)
        .unwrap();

    assert_eq!(engine.input.drag_auto_release.buttons, 0);
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Key(Key::new("left_ctrl").unwrap())),
        "an unrelated explicit latch keeps its existing targeting lifetime"
    );
    assert!(
        log.lock()
            .unwrap()
            .buttons
            .contains(&(MouseButton::Left, ButtonAction::Release))
    );
}

#[test]
fn explicit_mouse_toggle_never_becomes_an_auto_release_candidate() {
    let mut config = Config::default();
    config.normal.auto_release_ms = 1;
    config
        .normal
        .bindings
        .insert("x".into(), Binding::parse("toggle mouse_left").unwrap());
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::normal();
    let (mut backend, _) = FakeBackend::new(Vec::new());

    for event in [key_down("x"), key_up("x"), key_down("left_ctrl")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }

    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert_eq!(engine.input.drag_auto_release.buttons, 0);
    assert!(engine.input.drag_auto_release.fires_at.is_none());
}

#[test]
#[ignore = "allocation probe; run alone with --test-threads=1"]
fn disabled_drag_auto_release_pointer_check_allocates_nothing() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let region = Region::new(TEST_ALLOCATOR);
    for _ in 0..1_000 {
        engine.note_drag_pointer_moved();
    }
    assert_eq!(region.change().allocations, 0);
}

#[test]
fn second_long_press_releases_latched_button_without_an_extra_click() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );
    engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
    engine.fire_due_long_press_toggles(&mut backend).unwrap();
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert_eq!(
        log.lock().unwrap().buttons,
        vec![(MouseButton::Left, ButtonAction::Press)],
        "a latched button must not receive an atomic click"
    );
    engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
    engine.fire_due_long_press_toggles(&mut backend).unwrap();
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();

    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        vec![
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn due_pending_mouse_press_releases_when_its_physical_key_disappeared() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    engine.input.pressed.remove(&Key::new("x").unwrap());
    engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
    engine.fire_due_long_press_toggles(&mut backend).unwrap();

    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn one_long_press_cancels_other_pending_keys_for_the_same_button() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("x".into(), Binding::Click(Button::Left));
    config
        .normal
        .bindings
        .insert("y".into(), Binding::Click(Button::Left));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("y"), &mut backend)
        .unwrap();
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    for pending in &mut engine.input.pending_long_press_toggles {
        pending.fires_at = Instant::now();
    }
    engine.fire_due_long_press_toggles(&mut backend).unwrap();

    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_up("y"), &mut backend)
        .unwrap();
    assert_eq!(log.lock().unwrap().buttons.len(), 1);
}

#[test]
fn short_click_cancels_long_press_and_zero_disables_it() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );

    let mut config = active_config(&engine);
    config.normal.long_press_toggle_ms = 0;
    engine.apply_config(config).unwrap();
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
            (MouseButton::Left, ButtonAction::Click),
        ]
    );
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    assert_eq!(log.lock().unwrap().clicks, 1);
}

#[test]
fn long_press_toggle_ignores_non_click_bindings() {
    let mut engine = engine_with_normal_binding("x", "move_left");
    engine.registry.active = ModeId::normal();
    let (mut backend, _) = FakeBackend::new(Vec::new());
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
}

#[test]
fn explicit_parameterless_toggle_cancels_matching_long_press() {
    for (key, button) in [
        (";", MouseButton::Left),
        ("'", MouseButton::Right),
        ("right_shift", MouseButton::Middle),
    ] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = engine_with_probes(&seen, &[]);
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down(key), &mut backend)
            .unwrap();
        assert_eq!(
            engine.input.pending_long_press_toggles.len(),
            1,
            "key={key}"
        );
        engine
            .handle_backend_event(key_down("n"), &mut backend)
            .unwrap();
        assert!(
            engine.input.pending_long_press_toggles.is_empty(),
            "key={key}"
        );
        engine.fire_due_long_press_toggles(&mut backend).unwrap();
        assert_eq!(
            log.lock().unwrap().buttons,
            vec![(button, ButtonAction::Press)],
            "key={key}"
        );
    }
}

#[test]
fn newest_held_click_key_controls_color_then_release_falls_back() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config.normal.bindings.insert(
        "x".into(),
        Binding::Click(crate::api::binding::Button::Left),
    );
    config.normal.bindings.insert(
        "y".into(),
        Binding::Click(crate::api::binding::Button::Right),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let log = run_in_normal(
        &mut engine,
        vec![key_down("x"), key_down("y"), key_up("y"), key_up("x")],
    );
    let log = log.lock().unwrap();
    let left = crate::api::overlay::Color::rgb(0, 255, 0);
    let right = crate::api::overlay::Color::rgb(0, 255, 255);
    let mut colors = log
        .scenes
        .iter()
        .filter_map(|scene| scene.cursor_marker.as_ref().map(|marker| marker.stroke))
        .filter(|color| *color == left || *color == right)
        .collect::<Vec<_>>();
    colors.dedup();
    assert_eq!(colors, vec![left, right, left]);
    assert_ne!(
        log.scenes
            .last()
            .and_then(|scene| scene.cursor_marker.as_ref())
            .expect("released cursor marker")
            .stroke,
        left
    );
}

#[test]
fn latched_mouse_button_color_beats_ordinary_click_feedback() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config.normal.bindings.insert(
        "x".into(),
        Binding::Toggle(vec![InputTarget::Mouse(crate::api::binding::Button::Left)]),
    );
    config.normal.bindings.insert(
        "y".into(),
        Binding::Click(crate::api::binding::Button::Right),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let log = run_in_normal(
        &mut engine,
        vec![key_down("x"), key_up("x"), key_down("y"), key_up("y")],
    );
    let log = log.lock().unwrap();
    let right = crate::api::overlay::Color::rgb(0, 255, 255);
    assert!(log.scenes.iter().all(|scene| {
        scene
            .cursor_marker
            .as_ref()
            .is_none_or(|marker| marker.stroke != right)
    }));
}

#[test]
fn click_indicator_survives_mode_switch_until_its_key_is_released() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.register(Box::new(ProbeMode::new("grid", seen)));
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert!(!engine.input.active_click_indicators.is_empty());
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    engine
        .activate(ModeId::grid(), Some(ModeId::normal()), &mut backend)
        .unwrap();
    assert!(!engine.input.active_click_indicators.is_empty());
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    assert!(engine.input.active_click_indicators.is_empty());
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn failed_or_source_less_clicks_do_not_leave_click_indicators() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, _) = FakeBackend::new(Vec::new());
    backend.fail_mouse = true;
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert!(engine.input.active_click_indicators.is_empty());
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    assert!(engine.input.active_click_indicators.is_empty());

    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, _) = FakeBackend::new(Vec::new());
    engine
        .execute(
            vec![Command::DispatchActions(vec![Binding::Click(
                crate::api::binding::Button::Left,
            )])],
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.active_click_indicators.is_empty());

    let delayed_input = InputEvent {
        key: Key::new("x").unwrap(),
        state: KeyState::Down,
        repeat: false,
        injected: false,
        timestamp_millis: 0,
    };
    engine
        .continue_sequence(
            VecDeque::from([Binding::Click(crate::api::binding::Button::Left)]),
            ModeId::normal(),
            delayed_input,
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.active_click_indicators.is_empty());
}

#[test]
fn disabling_clears_click_indicators() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    assert!(!engine.input.active_click_indicators.is_empty());
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    engine
        .handle_backend_event(BackendEvent::ToggleEnabled, &mut backend)
        .unwrap();
    assert!(engine.input.active_click_indicators.is_empty());
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn apply_config_completes_and_capture_loss_cancels_pending_mouse_presses() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    engine.apply_config(active_config(&engine)).unwrap();
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );

    let mut engine = engine_with_normal_binding("x", "right_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(
            BackendEvent::InputCaptureLost("injected capture loss".into()),
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Right, ButtonAction::Press),
            (MouseButton::Right, ButtonAction::Release),
        ]
    );
}

#[test]
fn failed_pending_mouse_release_remains_owned_until_recovery_retries_it() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    log.lock().unwrap().fail_next_mouse_release = true;
    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();

    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ],
        "the failed first MouseUp must be retried by input recovery"
    );
}

#[test]
fn shutdown_cancels_a_pending_click_and_clears_its_indicator() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    let log = run_in_normal(&mut engine, vec![key_down("x")]);
    assert!(engine.input.active_click_indicators.is_empty());
    assert!(engine.input.pending_long_press_toggles.is_empty());
    let log = log.lock().unwrap();
    assert_eq!(log.clicks, 0);
    assert!(log.scenes.iter().any(|scene| {
        scene
            .cursor_marker
            .as_ref()
            .is_some_and(|marker| marker.stroke == crate::api::overlay::Color::rgb(0, 255, 0))
    }));
}

#[test]
fn latched_mouse_buttons_recolor_the_transparent_cursor_marker() {
    for (target, color) in [
        ("mouse_left", (0, 255, 0)),
        ("mouse_middle", (255, 0, 255)),
        ("mouse_right", (0, 255, 255)),
    ] {
        let mut engine = engine_with_normal_binding("x", &format!("toggle {target}"));
        let log = run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
        let expected_fill = crate::api::overlay::Color::rgba(color.0, color.1, color.2, 51);
        let expected_stroke = crate::api::overlay::Color::rgba(color.0, color.1, color.2, 255);
        assert!(
            log.lock().unwrap().scenes.iter().any(|scene| {
                scene.cursor_marker.as_ref().is_some_and(|marker| {
                    marker.fill == expected_fill && marker.stroke == expected_stroke
                })
            }),
            "{target} never presented its pressed cursor color"
        );
    }
}

#[test]
fn left_mouse_color_wins_when_multiple_buttons_are_latched() {
    let mut engine = engine_with_normal_binding("x", "toggle mouse_right mouse_middle mouse_left");
    let log = run_in_normal(&mut engine, vec![key_down("x"), key_up("x")]);
    let expected = crate::api::overlay::Color::rgb(0, 255, 0);
    assert!(log.lock().unwrap().scenes.iter().any(|scene| {
        scene
            .cursor_marker
            .as_ref()
            .is_some_and(|marker| marker.stroke == expected)
    }));
}

#[test]
fn releasing_the_mouse_button_restores_the_mode_cursor_color() {
    let mut engine = engine_with_normal_binding("x", "toggle mouse_left");
    let log = run_in_normal(
        &mut engine,
        vec![key_down("x"), key_up("x"), key_down("x"), key_up("x")],
    );
    let log = log.lock().unwrap();
    let marker = log
        .scenes
        .last()
        .and_then(|scene| scene.cursor_marker.as_ref())
        .expect("released cursor marker");
    assert_ne!(marker.stroke, crate::api::overlay::Color::rgb(0, 255, 0));
    assert_eq!(marker.fill.a, 34);
    assert_eq!(marker.stroke.a, 210);
}

