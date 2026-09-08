#[test]
fn toggle_holds_then_releases_the_button() {
    let mut engine = engine_with_normal_binding("space", "toggle_left");
    let log = run_in_normal(
        &mut engine,
        vec![key_down("space"), key_up("space"), key_down("space")],
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
fn modifier_plus_bare_toggle_latches_that_modifier() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    let log = run_in_normal(
        &mut engine,
        vec![
            key_down("left_shift"),
            key_down("n"),
            key_up("n"),
            key_up("left_shift"),
        ],
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.sent,
        vec![
            ("left_shift".into(), KeyState::Down),
            ("left_shift".into(), KeyState::Up),
        ]
    );
    assert!(log.buttons.is_empty());
    assert!(log.scenes.iter().any(|scene| {
        scene
            .indicator
            .as_ref()
            .is_some_and(|indicator| indicator.held_text.as_deref() == Some("● LEFT SHIFT"))
    }));
}

#[test]
fn every_toggle_target_survives_its_forwarded_physical_key_release() {
    for (target, chord, binding, injected) in [
        ("left_ctrl", "ctrl+n", "toggle ctrl", "left_ctrl"),
        (
            "right_shift",
            "right_shift+n",
            "toggle right_shift",
            "right_shift",
        ),
        ("left_alt", "alt+n", "toggle alt", "left_alt"),
        ("left_win", "left_win+n", "toggle left_win", "left_win"),
        ("e", "e+n", "toggle e", "e"),
    ] {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert(chord.into(), Binding::parse(binding).unwrap());
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::app::mode_catalog::built_in(&config) {
            engine.register(mode);
        }
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        for event in [key_down(target), key_down("n"), key_up("n")] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert!(
            engine
                .matching_latched_target(&InputTarget::Key(Key::new(target).unwrap()))
                .is_some(),
            "target={target}"
        );
        assert_eq!(
            log.lock().unwrap().sent,
            vec![(injected.into(), KeyState::Down)],
            "target={target}"
        );

        engine
            .handle_backend_event(key_up(target), &mut backend)
            .unwrap();
        assert!(
            engine
                .matching_latched_target(&InputTarget::Key(Key::new(target).unwrap()))
                .is_some(),
            "target={target}"
        );
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                (injected.into(), KeyState::Down),
                (injected.into(), KeyState::Down),
            ],
            "the forwarded physical Up must be followed by one synthetic Down; target={target}"
        );

        for event in [key_down(target), key_down("n"), key_up("n"), key_up(target)] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert!(engine.input.latched.is_empty(), "target={target}");
        assert_eq!(
            log.lock().unwrap().sent.last(),
            Some(&(injected.into(), KeyState::Up)),
            "an explicit toggle-off must not be reasserted; target={target}"
        );
    }
}

#[test]
fn parameterless_toggle_uses_the_same_forwarded_release_reassertion() {
    for activation in ["n", "t", "f8"] {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert(activation.into(), Binding::Toggle(Vec::new()));
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::app::mode_catalog::built_in(&config) {
            engine.register(mode);
        }
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down("left_ctrl"), &mut backend)
            .unwrap();
        assert!(log.lock().unwrap().sent.is_empty());

        engine
            .handle_backend_event(key_down(activation), &mut backend)
            .unwrap();
        assert!(engine.input.pending_long_press_toggles.is_empty());
        assert!(
            engine
                .input
                .latched
                .contains(&InputTarget::Key(Key::new("left_ctrl").unwrap()))
        );
        assert_eq!(
            log.lock().unwrap().sent,
            vec![("left_ctrl".into(), KeyState::Down)],
            "the already-held companion must latch on the configurable toggle KeyDown; activation={activation}"
        );

        engine
            .handle_backend_event(key_up(activation), &mut backend)
            .unwrap();
        engine
            .handle_backend_event(key_up("left_ctrl"), &mut backend)
            .unwrap();

        assert!(
            engine
                .input
                .latched
                .contains(&InputTarget::Key(Key::new("left_ctrl").unwrap()))
        );
        assert_eq!(
            log.lock().unwrap().sent,
            vec![
                ("left_ctrl".into(), KeyState::Down),
                ("left_ctrl".into(), KeyState::Down),
            ],
            "activation={activation}"
        );
    }
}

#[test]
fn parameterless_toggle_latches_every_companion_in_both_orders() {
    const ACTIVATION: &str = "t";
    const PARTNERS: [&str; 5] = ["left_ctrl", "left_shift", "left_alt", "s", "d"];

    for partners_first in [true, false] {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert(ACTIVATION.into(), Binding::Toggle(Vec::new()));
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::app::mode_catalog::built_in(&config) {
            engine.register(mode);
        }
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        if partners_first {
            for partner in PARTNERS {
                engine
                    .handle_backend_event(key_down(partner), &mut backend)
                    .unwrap();
            }
            engine
                .handle_backend_event(key_down(ACTIVATION), &mut backend)
                .unwrap();
        } else {
            engine
                .handle_backend_event(key_down(ACTIVATION), &mut backend)
                .unwrap();
            for partner in PARTNERS {
                engine
                    .handle_backend_event(key_down(partner), &mut backend)
                    .unwrap();
            }
        }

        assert!(engine.input.pending_long_press_toggles.is_empty());
        for partner in PARTNERS {
            assert!(
                engine
                    .input
                    .latched
                    .contains(&InputTarget::Key(Key::new(partner).unwrap())),
                "partner={partner} partners_first={partners_first}"
            );
            assert_eq!(
                log.lock()
                    .unwrap()
                    .sent
                    .iter()
                    .filter(|(key, state)| key == partner && *state == KeyState::Down)
                    .count(),
                1,
                "partner={partner} partners_first={partners_first}"
            );
        }
    }
}

#[test]
fn parameterless_toggle_maps_normal_click_bindings_in_both_orders() {
    use crate::api::binding::Button;

    for (partner, button, mouse, companions) in [
        (
            ";",
            Button::Left,
            MouseButton::Left,
            &["left_shift", "left_ctrl"][..],
        ),
        (
            "'",
            Button::Right,
            MouseButton::Right,
            &["left_shift", "left_ctrl"][..],
        ),
        (
            "right_shift",
            Button::Middle,
            MouseButton::Middle,
            &["left_ctrl"][..],
        ),
    ] {
        for partners_first in [true, false] {
            let mut config = Config::default();
            config.normal.bindings.clear();
            config
                .normal
                .bindings
                .insert("n".into(), Binding::Toggle(Vec::new()));
            config
                .normal
                .bindings
                .insert(partner.into(), Binding::Click(button));
            let mut engine = Engine::new(config.clone(), Appearance::Dark);
            for mode in crate::app::mode_catalog::built_in(&config) {
                engine.register(mode);
            }
            engine.registry.active = ModeId::normal();
            let (mut backend, log) = FakeBackend::new(Vec::new());

            if !partners_first {
                engine
                    .handle_backend_event(key_down("n"), &mut backend)
                    .unwrap();
            }
            for companion in companions {
                engine
                    .handle_backend_event(key_down(companion), &mut backend)
                    .unwrap();
            }
            engine
                .handle_backend_event(key_down(partner), &mut backend)
                .unwrap();
            if partners_first {
                engine
                    .handle_backend_event(key_down("n"), &mut backend)
                    .unwrap();
            }

            assert!(engine.input.latched.contains(&InputTarget::Mouse(button)));
            assert!(
                engine
                    .matching_latched_target(&InputTarget::Key(Key::new(partner).unwrap()))
                    .is_none(),
                "the Normal binding must replace the physical partner; partner={partner} partners_first={partners_first}"
            );
            for companion in companions {
                assert!(
                    engine
                        .matching_latched_target(&InputTarget::Key(Key::new(*companion).unwrap()))
                        .is_some(),
                    "companion={companion} partner={partner} partners_first={partners_first}"
                );
            }
            assert_eq!(
                log.lock().unwrap().buttons,
                [(mouse, ButtonAction::Press)],
                "partner={partner} partners_first={partners_first}"
            );

            engine
                .handle_backend_event(key_up(partner), &mut backend)
                .unwrap();
            for companion in companions.iter().rev() {
                engine
                    .handle_backend_event(key_up(companion), &mut backend)
                    .unwrap();
            }
            engine
                .handle_backend_event(key_up("n"), &mut backend)
                .unwrap();
            for event in [key_down("n"), key_up("n")] {
                engine.handle_backend_event(event, &mut backend).unwrap();
            }

            assert!(engine.input.latched.is_empty());
            assert_eq!(
                log.lock().unwrap().buttons,
                [(mouse, ButtonAction::Press), (mouse, ButtonAction::Release),],
                "partner={partner} partners_first={partners_first}"
            );
        }
    }
}

#[test]
fn parameterless_toggle_uses_latched_modifiers_for_later_normal_chords() {
    use crate::api::binding::Button;

    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("n".into(), Binding::Toggle(Vec::new()));
    config
        .normal
        .bindings
        .insert("x".into(), Binding::Click(Button::Left));
    config
        .normal
        .bindings
        .insert("ctrl+x".into(), Binding::Click(Button::Right));
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    for event in [
        key_down("n"),
        key_down("left_ctrl"),
        key_up("left_ctrl"),
        key_down("x"),
        key_up("x"),
        key_up("n"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }

    assert!(
        engine
            .matching_latched_target(&InputTarget::Key(Key::new("left_ctrl").unwrap()))
            .is_some()
    );
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Right))
    );
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Mouse(Button::Left))
    );
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Right, ButtonAction::Press)]
    );
}

#[test]
fn parameterless_toggle_accumulates_duplicate_semantic_targets_once() {
    use crate::api::binding::Button;

    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("n".into(), Binding::Toggle(Vec::new()));
    for key in ["x", "y"] {
        config
            .normal
            .bindings
            .insert(key.into(), Binding::Click(Button::Left));
    }
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    for event in [
        key_down("n"),
        key_down("x"),
        key_up("x"),
        key_down("y"),
        key_up("y"),
        key_up("n"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );

    for event in [key_down("n"), key_up("n")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
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
fn parameterless_toggle_consumes_every_unbound_partner_edge() {
    let mut engine = engine_with_normal_binding("t", "toggle");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    for event in [
        key_down("t"),
        key_down("e"),
        key_up("e"),
        key_up("t"),
        key_down("t"),
        key_up("t"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }

    let log = log.lock().unwrap();
    assert_eq!(log.dispositions, [KeyDisposition::Consume; 6]);
    assert_eq!(
        log.sent,
        [("e".into(), KeyState::Down), ("e".into(), KeyState::Up)]
    );
    assert!(engine.input.latched.is_empty());
}

#[test]
fn parameterless_toggle_consumes_a_bound_partner_without_running_its_action() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("t".into(), Binding::Toggle(Vec::new()));
    config.normal.bindings.insert(
        "x".into(),
        Binding::Click(crate::api::binding::Button::Left),
    );
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    for event in [
        key_down("t"),
        key_down("x"),
        key_up("x"),
        key_up("t"),
        key_down("t"),
        key_up("t"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }

    let log = log.lock().unwrap();
    assert_eq!(log.dispositions, [KeyDisposition::Consume; 6]);
    assert_eq!(
        log.buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert_eq!(log.clicks, 0, "the partner's click action must not run");
    assert!(engine.input.latched.is_empty());
}

#[test]
fn parameterless_toggle_treats_speed_keys_as_physical_partners_in_both_orders() {
    for order in [["n", "left_shift"], ["left_shift", "n"]] {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("n".into(), Binding::Toggle(Vec::new()));
        config.normal.bindings.insert(
            "left_shift".into(),
            Binding::SpeedToggle(crate::api::Speed::Slow),
        );

        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::app::mode_catalog::built_in(&config) {
            engine.register(mode);
        }
        engine.registry.active = ModeId::normal();
        let (mut backend, _log) = FakeBackend::new(Vec::new());

        for key in order {
            engine.handle_backend_event(key_down(key), &mut backend).unwrap();
        }
        for key in order.iter().rev() {
            engine.handle_backend_event(key_up(key), &mut backend).unwrap();
        }

        assert!(engine
            .input
            .latched
            .contains(&InputTarget::Key(Key::new("left_shift").unwrap())), "order={order:?}");
        assert_eq!(
            engine.overlay.speed_toggle,
            (order[0] == "left_shift").then_some(crate::api::Speed::Slow),
            "toggle capture must not execute the speed binding: order={order:?}"
        );
    }
}

#[test]
fn configurable_parameterless_toggle_self_latches_only_after_the_threshold() {
    for activation in ["t", "f8", "caps_lock"] {
        let mut engine = engine_with_normal_binding(activation, "toggle");
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        engine
            .handle_backend_event(key_down(activation), &mut backend)
            .unwrap();
        assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
        assert!(log.lock().unwrap().sent.is_empty());

        engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
        engine.fire_due_long_press_toggles(&mut backend).unwrap();
        assert!(
            engine
                .input
                .latched
                .contains(&InputTarget::Key(Key::new(activation).unwrap())),
            "activation={activation}"
        );
        assert_eq!(
            log.lock().unwrap().sent,
            vec![(activation.into(), KeyState::Down)],
            "activation={activation}"
        );
    }
}

#[test]
fn activation_key_then_modifier_upgrades_bare_toggle_without_mouse_fallback() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    let log = run_in_normal(
        &mut engine,
        vec![
            key_down("n"),
            key_down("left_shift"),
            key_up("n"),
            key_up("left_shift"),
        ],
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.sent,
        vec![
            ("left_shift".into(), KeyState::Down),
            ("left_shift".into(), KeyState::Up),
        ]
    );
    assert!(log.buttons.is_empty());
}

#[test]
fn parameterless_toggle_captures_three_keyboard_partners_in_either_order() {
    for events in [
        vec![
            key_down("left_win"),
            key_down("left_shift"),
            key_down("e"),
            key_down("n"),
        ],
        vec![
            key_down("n"),
            key_down("left_win"),
            key_down("left_shift"),
            key_down("e"),
        ],
    ] {
        let mut engine = engine_with_normal_binding("n", "toggle");
        engine.registry.active = ModeId::normal();
        let (mut backend, log) = FakeBackend::new(Vec::new());

        for event in events {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }

        let expected = BTreeSet::from([
            InputTarget::Key(Key::new("left_win").unwrap()),
            InputTarget::Key(Key::new("left_shift").unwrap()),
            InputTarget::Key(Key::new("e").unwrap()),
        ]);
        assert_eq!(engine.input.latched, expected);
        assert!(
            engine.input.pending_long_press_toggles.is_empty(),
            "a partner must cancel the activation key's self-latch deadline"
        );
        let sent_down: BTreeSet<_> = log
            .lock()
            .unwrap()
            .sent
            .iter()
            .filter(|(_, state)| *state == KeyState::Down)
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(
            sent_down,
            BTreeSet::from(["left_win".into(), "left_shift".into(), "e".into()])
        );
    }
}

#[test]
fn separate_toggle_chords_accumulate_instead_of_clearing_existing_targets() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    let log = run_in_normal(
        &mut engine,
        vec![
            key_down("left_shift"),
            key_down("n"),
            key_up("n"),
            key_up("left_shift"),
            key_down("n"),
            key_down("left_ctrl"),
            key_up("n"),
            key_up("left_ctrl"),
        ],
    );
    let log = log.lock().unwrap();
    assert_eq!(
        &log.sent[..2],
        &[
            ("left_shift".into(), KeyState::Down),
            ("left_ctrl".into(), KeyState::Down),
        ]
    );
    assert!(log.buttons.is_empty());
}

#[test]
fn bare_parameterless_toggle_does_nothing_when_nothing_is_latched() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    let log = run_in_normal(
        &mut engine,
        vec![key_down("n"), key_up("n"), key_down("n"), key_up("n")],
    );
    let log = log.lock().unwrap();
    assert!(log.sent.is_empty());
    assert!(log.buttons.is_empty());
}

#[test]
fn long_press_parameterless_toggle_latches_its_activation_key_until_a_short_tap() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    let n = Key::new("n").unwrap();
    let target = InputTarget::Key(n.clone());

    engine
        .handle_backend_event(key_down("n"), &mut backend)
        .unwrap();
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    assert_eq!(engine.input.pending_long_press_toggles[0].target, target);
    assert!(log.lock().unwrap().sent.is_empty());

    engine.input.pending_long_press_toggles[0].fires_at = Instant::now();
    engine.fire_due_long_press_toggles(&mut backend).unwrap();
    assert!(engine.input.latched.contains(&InputTarget::Key(n.clone())));
    assert_eq!(log.lock().unwrap().sent, vec![("n".into(), KeyState::Down)]);

    engine
        .handle_backend_event(key_up("n"), &mut backend)
        .unwrap();
    assert!(
        engine.input.latched.contains(&InputTarget::Key(n.clone())),
        "the physical release must not undo the long-press latch"
    );
    assert_eq!(log.lock().unwrap().sent.len(), 1);

    engine
        .handle_backend_event(key_down("n"), &mut backend)
        .unwrap();
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    engine
        .handle_backend_event(key_up("n"), &mut backend)
        .unwrap();
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        vec![("n".into(), KeyState::Down), ("n".into(), KeyState::Up),]
    );

    let mut config = active_config(&engine);
    config.normal.long_press_toggle_ms = 0;
    engine.apply_config(config).unwrap();
    engine
        .handle_backend_event(key_down("n"), &mut backend)
        .unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    engine
        .handle_backend_event(key_up("n"), &mut backend)
        .unwrap();
}

#[test]
fn bare_parameterless_toggle_releases_all_accumulated_targets() {
    let mut config = Config::default();
    config
        .normal
        .bindings
        .insert("n".into(), Binding::Toggle(Vec::new()));
    config.normal.bindings.insert(
        ";".into(),
        Binding::Click(crate::api::binding::Button::Left),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .activate(ModeId::normal(), Some(ModeId::idle()), &mut backend)
        .unwrap();
    for event in [
        key_down("left_shift"),
        key_down("n"),
        key_up("n"),
        key_up("left_shift"),
        key_down("n"),
        key_down(";"),
        key_up(";"),
        key_up("n"),
        key_down("n"),
        key_up("n"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    let log = log.lock().unwrap();
    assert_eq!(
        log.sent,
        vec![
            ("left_shift".into(), KeyState::Down),
            ("left_shift".into(), KeyState::Up),
        ]
    );
    assert_eq!(
        log.buttons,
        vec![
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
    assert!(engine.input.latched.is_empty());
}

#[test]
fn caps_lock_uses_the_same_implicit_toggle_partner_rule_as_every_other_key() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("caps_lock"), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("n"), &mut backend)
        .unwrap();

    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Key(Key::new("caps_lock").unwrap()))
    );
    assert_eq!(
        log.lock().unwrap().sent,
        vec![("caps_lock".into(), KeyState::Down)]
    );
}

#[test]
fn toggle_plus_click_key_latches_the_mapped_mouse_button_without_clicking() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("n".into(), Binding::Toggle(Vec::new()));
    config.normal.bindings.insert(
        ";".into(),
        Binding::Click(crate::api::binding::Button::Left),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let log = run_in_normal(
        &mut engine,
        vec![key_down("n"), key_down(";"), key_up(";"), key_up("n")],
    );
    let log = log.lock().unwrap();
    assert_eq!(log.clicks, 0);
    assert_eq!(
        log.buttons,
        vec![
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn generic_press_send_and_release_preserve_a_latched_modifier() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config.normal.bindings.insert(
        "n".into(),
        Binding::Sequence(vec![
            Binding::parse("press shift mouse_left").unwrap(),
            Binding::parse("send shift+x").unwrap(),
            Binding::parse("release shift mouse_left").unwrap(),
        ]),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let log = run_in_normal(&mut engine, vec![key_down("n")]);
    let log = log.lock().unwrap();
    assert_eq!(
        log.sent,
        vec![
            ("left_shift".into(), KeyState::Down),
            ("x".into(), KeyState::Down),
            ("x".into(), KeyState::Up),
            ("left_shift".into(), KeyState::Up),
        ]
    );
    assert_eq!(
        log.buttons,
        vec![
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );
}

#[test]
fn inferred_side_modifier_satisfies_a_generic_modifier_send() {
    let mut config = Config::default();
    config.normal.bindings.clear();
    config.normal.bindings.insert(
        "n".into(),
        Binding::Sequence(vec![
            Binding::parse("toggle").unwrap(),
            Binding::parse("send shift+x").unwrap(),
        ]),
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen)));

    let log = run_in_normal(&mut engine, vec![key_down("left_shift"), key_down("n")]);
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("left_shift".into(), KeyState::Down),
            ("x".into(), KeyState::Down),
            ("x".into(), KeyState::Up),
            ("left_shift".into(), KeyState::Up),
        ]
    );
}

#[test]
fn failed_chord_release_is_retried_during_cleanup() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    log.lock().unwrap().fail_next_key_up = true;
    let error = engine
        .send_chord(&KeyChord::parse("home").unwrap(), &mut backend)
        .unwrap_err();
    assert!(error.contains("key-up"));
    assert_eq!(
        engine.input.latched,
        BTreeSet::from([InputTarget::Key(Key::new("home").unwrap())])
    );
    engine.release_latched(&mut backend).unwrap();
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("home".into(), KeyState::Down),
            ("home".into(), KeyState::Up),
        ]
    );
}

#[test]
fn generic_modifier_release_matches_a_latched_side() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .press_targets(
            &[InputTarget::Key(Key::new("right_shift").unwrap())],
            &mut backend,
        )
        .unwrap();
    engine
        .release_targets(
            &[InputTarget::Key(Key::new("shift").unwrap())],
            true,
            &mut backend,
        )
        .unwrap();
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("right_shift".into(), KeyState::Down),
            ("right_shift".into(), KeyState::Up),
        ]
    );
}

#[test]
fn explicit_modifier_sides_can_be_latched_independently() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .press_targets(
            &[
                InputTarget::Key(Key::new("left_shift").unwrap()),
                InputTarget::Key(Key::new("right_shift").unwrap()),
            ],
            &mut backend,
        )
        .unwrap();
    assert_eq!(engine.input.latched.len(), 2);
    engine
        .release_targets(
            &[InputTarget::Key(Key::new("right_shift").unwrap())],
            true,
            &mut backend,
        )
        .unwrap();
    assert!(
        engine
            .input
            .latched
            .contains(&InputTarget::Key(Key::new("left_shift").unwrap()))
    );
    assert!(
        !engine
            .input
            .latched
            .contains(&InputTarget::Key(Key::new("right_shift").unwrap()))
    );
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("left_shift".into(), KeyState::Down),
            ("right_shift".into(), KeyState::Down),
            ("right_shift".into(), KeyState::Up),
        ]
    );
}

#[test]
fn generic_modifier_toggle_releases_a_latched_side() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .press_targets(
            &[InputTarget::Key(Key::new("left_ctrl").unwrap())],
            &mut backend,
        )
        .unwrap();
    engine
        .toggle_targets(&[InputTarget::Key(Key::new("ctrl").unwrap())], &mut backend)
        .unwrap();
    assert!(engine.input.latched.is_empty());
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("left_ctrl".into(), KeyState::Down),
            ("left_ctrl".into(), KeyState::Up),
        ]
    );
}

#[test]
fn explicit_release_emits_an_up_edge_even_when_not_tracked() {
    let mut engine = engine_with_normal_binding("n", "release shift");
    let log = run_in_normal(&mut engine, vec![key_down("n")]);
    assert_eq!(
        log.lock().unwrap().sent,
        vec![("left_shift".into(), KeyState::Up)]
    );
}

#[test]
fn repeated_key_send_uses_the_same_interval_as_default_wait() {
    let engine = Engine::new(Config::default(), Appearance::Dark);
    let home = Binding::parse("home").unwrap();
    assert_eq!(
        engine.flatten_sequence(&[home.clone(), home.clone()]),
        engine.flatten_sequence(&[home.clone(), Binding::parse("wait").unwrap(), home])
    );
}

#[test]
fn wait_pauses_and_resumes_a_sequence_without_blocking() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    let input = InputEvent {
        character: None,
        key: Key::new("n").unwrap(),
        state: KeyState::Down,
        repeat: false,
        injected: false,
        timestamp_millis: 0,
    };
    let actions = VecDeque::from(vec![
        Binding::parse("home").unwrap(),
        Binding::parse("wait 1 1").unwrap(),
        Binding::parse("home").unwrap(),
    ]);
    engine
        .continue_sequence(actions, ModeId::normal(), input, &mut backend)
        .unwrap();
    assert_eq!(log.lock().unwrap().sent.len(), 2);
    assert_eq!(engine.scheduler.sequences.len(), 1);
    engine.scheduler.sequences[0].fires_at = Instant::now();
    engine.fire_due_sequences(&mut backend).unwrap();
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("home".into(), KeyState::Down),
            ("home".into(), KeyState::Up),
            ("home".into(), KeyState::Down),
            ("home".into(), KeyState::Up),
        ]
    );
    assert!(engine.scheduler.sequences.is_empty());
}

#[test]
fn delayed_sequence_failure_does_not_drop_other_due_sequences() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    let now = Instant::now();
    let input = InputEvent {
        character: None,
        key: Key::new("n").unwrap(),
        state: KeyState::Down,
        repeat: false,
        injected: false,
        timestamp_millis: 0,
    };
    engine.scheduler.sequences = vec![
        PendingSequence {
            fires_at: now,
            actions: VecDeque::from([Binding::Warp { x: 1, y: 2 }]),
            owner: ModeId::normal(),
            input: input.clone(),
        },
        PendingSequence {
            fires_at: now,
            actions: VecDeque::from([Binding::parse("home").unwrap()]),
            owner: ModeId::normal(),
            input,
        },
    ];
    backend.fail_warp = true;
    engine.fire_due_sequences(&mut backend).unwrap();
    assert_eq!(
        log.lock().unwrap().sent,
        vec![
            ("home".into(), KeyState::Down),
            ("home".into(), KeyState::Up),
        ]
    );
}

#[test]
fn random_wait_stays_inside_the_inclusive_range() {
    for _ in 0..128 {
        assert!((50..=100).contains(&random_wait_ms(50, 100)));
    }
    assert_eq!(random_wait_ms(25, 25), 25);
}

