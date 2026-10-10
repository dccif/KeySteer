fn text_input_engine(config: Config) -> Engine {
    let mut engine = Engine::from_plan(
        crate::app::configuration::compile(&config).unwrap(),
        Appearance::Dark,
    )
    .unwrap();
    engine.rebuild_tables();
    engine
}

#[test]
fn hint_overlay_search_routes_editing_copy_and_stale_results_by_owner() {
    let mut engine = text_input_engine(Config::default());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.set_active(ModeId::normal());
    engine.activate(ModeId::ui_hint(), Some(ModeId::normal()), &mut backend).unwrap();
    let scan_id = log.lock().unwrap().scan_requests.last().unwrap().id;
    engine.handle_backend_event(BackendEvent::UiScanned(crate::api::UiScanResult {
        id: scan_id, status: UiScanStatus::Success, retired: Vec::new(),
        targets: vec![crate::api::UiTarget {
            rect: Rect::new(10.0, 10.0, 50.0, 30.0), name: "复制".into(), role: crate::api::SemanticRole::Button,
            details: Some(Box::new(crate::api::geometry::UiTargetDetails { ocr: "复制文本".into(), accessibility: "Copy".into(), color: None })),
        }],
    }), &mut backend).unwrap();
    for event in [key_down("/"), key_up("/")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().text_prompts.is_empty(), "search must not create a native editor");
    let old_id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
    assert!(log.lock().unwrap().text_capture);
    log.lock().unwrap().clipboard_input = "复制".into();
    // Native activation can briefly report no foreground application.
    engine.handle_backend_event(BackendEvent::FocusChanged(None), &mut backend).unwrap();
    assert_eq!(engine.scheduler.text_prompt.as_ref().unwrap().1.id, old_id);
    log.lock().unwrap().dispositions.clear();
    let modifier = if cfg!(target_os = "windows") { "left_alt" } else if cfg!(target_os = "macos") { "left_win" } else { "left_ctrl" };
    for event in [key_down(modifier), key_down("v"), key_up("v"), key_up(modifier)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(log.lock().unwrap().dispositions, [KeyDisposition::Consume; 4]);
    engine.handle_backend_event(BackendEvent::TextPromptChanged { id: old_id, text: "fzwb".into() }, &mut backend).unwrap();
    log.lock().unwrap().dispositions.clear();
    let copy_modifier = "left_ctrl";
    log.lock().unwrap().fail_copy = true;
    for event in [key_down(copy_modifier), key_down("1"), key_up("1"), key_up(copy_modifier)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.scheduler.text_prompt.as_ref().unwrap().1.id, old_id);
    assert!(log.lock().unwrap().copied_text.is_empty());
    log.lock().unwrap().fail_copy = false;
    log.lock().unwrap().dispositions.clear();
    for event in [key_down(copy_modifier), key_down("1"), key_up("1"), key_up(copy_modifier)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(log.lock().unwrap().dispositions, [KeyDisposition::Consume; 4]);
    assert_eq!(log.lock().unwrap().copied_text, ["复制文本"]);
    assert!(engine.scheduler.text_prompt.is_none());
    assert!(!log.lock().unwrap().text_capture);
    assert_eq!(engine.active_mode(), &ModeId::ui_hint());
    assert!(log.lock().unwrap().warps.is_empty());
    engine.handle_backend_event(BackendEvent::TextPromptResult { id: old_id, value: Ok(None) }, &mut backend).unwrap();
    for event in [key_down("/"), key_up("/")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
    assert_ne!(id, old_id);
    engine.handle_backend_event(BackendEvent::TextPromptResult { id: old_id, value: Ok(Some("stale".into())) }, &mut backend).unwrap();
    assert_eq!(engine.scheduler.text_prompt.as_ref().unwrap().1.id, id);
    engine.handle_backend_event(BackendEvent::TextPromptResult { id, value: Ok(Some("fzwb".into())) }, &mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::ui_hint());
    assert_eq!(log.lock().unwrap().warps.last(), Some(&Point::new(35.0, 25.0)));
    assert!(engine.scheduler.text_prompt.is_none());
    assert!(!log.lock().unwrap().text_capture);
    assert!(log.lock().unwrap().released_text_prompts >= 1);
}

#[test]
fn text_input_forwards_typing_and_consumes_enter_to_restore_normal() {
    let mut engine = text_input_engine(Config::default());
    engine.set_active(ModeId::normal());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [key_down("\\"), key_up("\\")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::text_input());
    assert_eq!(
        log.lock().unwrap().dispositions,
        [KeyDisposition::Consume; 2]
    );
    log.lock().unwrap().dispositions.clear();
    for key in ["h", "j", "q", "i", "left_shift", "left_ctrl"] {
        engine
            .handle_backend_event(key_down(key), &mut backend)
            .unwrap();
        assert!(engine.quick_switch.pending.is_none());
        engine
            .handle_backend_event(key_up(key), &mut backend)
            .unwrap();
    }
    engine
        .handle_backend_event(key_down("enter"), &mut backend)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
    engine
        .handle_backend_event(key_up("enter"), &mut backend)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert_eq!(
        &log.lock().unwrap().dispositions[..12],
        [KeyDisposition::Forward; 12]
    );
    assert_eq!(
        &log.lock().unwrap().dispositions[12..14],
        [KeyDisposition::Consume; 2]
    );
    assert!(log.lock().unwrap().sent.is_empty());
    assert!(log.lock().unwrap().moves.is_empty());
    for event in [key_down("h"), key_up("h")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(
        &log.lock().unwrap().dispositions[14..],
        [KeyDisposition::Consume; 2]
    );
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn text_input_custom_return_bindings_roundtrip() {
    let config = Config::parse(
        r#"
[key_aliases]
typing_key = "f8"
[normal.bindings]
typing_key = "text_input"
[text_input.bindings]
"ctrl+typing_key" = "normal"
"#,
    )
    .unwrap();
    let config = Config::parse(&config.to_toml().unwrap()).unwrap();
    let mut engine = text_input_engine(config);
    engine.set_active(ModeId::normal());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [key_down("f8"), key_up("f8")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    log.lock().unwrap().dispositions.clear();
    for key in ["enter", "esc", "\\", "f2", "f8"] {
        for event in [key_down(key), key_up(key)] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(engine.active_mode(), &ModeId::text_input());
    }
    assert_eq!(
        log.lock().unwrap().dispositions,
        [KeyDisposition::Forward; 10]
    );
    for event in tap_chord("left_ctrl+f8") {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn text_input_return_bindings_consume_both_edges_and_ignore_extra_modifiers() {
    for exit_key in ["enter", "\\", "esc"] {
        let mut engine = text_input_engine(Config::default());
        engine.set_active(ModeId::text_input());
        let (mut backend, log) = FakeBackend::new(Vec::new());
        for event in tap_chord(&format!("left_ctrl+{exit_key}")) {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(engine.active_mode(), &ModeId::text_input());
        assert_eq!(
            log.lock().unwrap().dispositions,
            [KeyDisposition::Forward; 4]
        );
        log.lock().unwrap().dispositions.clear();
        for event in [key_down(exit_key), key_up(exit_key)] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(engine.active_mode(), &ModeId::normal());
        assert_eq!(
            log.lock().unwrap().dispositions,
            [KeyDisposition::Consume; 2]
        );
    }
}

#[test]
fn text_input_entry_stops_movement_and_releases_latched_inputs() {
    let mut config = Config::default();
    config
        .normal
        .bindings
        .insert("f3".into(), Binding::parse("press mouse_left").unwrap());
    let mut engine = text_input_engine(config);
    engine.set_active(ModeId::normal());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [
        key_down("f3"),
        key_up("f3"),
        key_down("h"),
        key_down("\\"),
        key_up("\\"),
        key_up("h"),
    ] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::text_input());
    assert!(engine.input.latched.is_empty());
    assert!(engine.input.active_gestures.is_empty());
    assert_eq!(
        log.lock().unwrap().buttons.last(),
        Some(&(MouseButton::Left, ButtonAction::Release))
    );
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn text_input_home_row_mappings_support_modified_navigation_and_repeats() {
    // Exercise the shipped examples as explicit user bindings, not defaults.
    let shipped = include_str!("../../../../keysteer.default.toml");
    let examples: Vec<_> = shipped
        .split("[text_input.bindings]")
        .nth(1)
        .unwrap()
        .split("[window]")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("# \""))
        .filter(|line| line.contains("primary+"))
        .map(|line| format!("\"{line}"))
        .collect();
    assert_eq!(examples.len(), 36);
    let config = Config::parse(&format!(
        "[text_input.bindings]\n'\\' = 'normal'\nesc = 'normal'\n{}",
        examples.join("\n")
    ))
    .unwrap();
    config.validate().unwrap();
    let primary = config.text_input.temporary_mode_keys[0].clone();
    let mut engine = text_input_engine(config);
    engine.set_active(ModeId::text_input());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for (source, target) in [
        ("h", "arrow_left"),
        ("j", "arrow_down"),
        ("k", "arrow_up"),
        ("l", "arrow_right"),
        ("u", "backspace"),
        ("i", "delete"),
        ("o", "insert"),
        ("t", "home"),
        ("y", "end"),
    ] {
        for modifiers in ["", "shift+", "ctrl+", "ctrl+shift+"] {
            let source_chord = format!("{modifiers}{primary}+{source}");
            let keys = KeyChord::parse(&source_chord).unwrap();
            let resolved = engine
                .lookup_for_pressed(&Key::new(source).unwrap(), keys.keys())
                .unwrap();
            assert_eq!(
                *resolved.binding,
                Binding::parse(&format!("send {modifiers}{target}")).unwrap()
            );
            let before = log.lock().unwrap().sent.len();
            for event in tap_chord(&source_chord) {
                engine.handle_backend_event(event, &mut backend).unwrap();
            }
            assert_eq!(engine.active_mode(), &ModeId::text_input());
            let recorded = log.lock().unwrap();
            assert!(recorded.sent[before..].contains(&(target.into(), KeyState::Down)));
            assert!(recorded.sent[before..].contains(&(target.into(), KeyState::Up)));
        }
    }
    engine
        .handle_backend_event(key_down(&primary), &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("h"), &mut backend)
        .unwrap();
    let mut repeat = key_down("h");
    if let BackendEvent::Input(ref mut input) = repeat {
        input.repeat = true;
    }
    let before = log.lock().unwrap().sent.len();
    engine
        .handle_backend_event(repeat.clone(), &mut backend)
        .unwrap();
    assert!(log.lock().unwrap().sent.len() > before);
    engine
        .handle_backend_event(key_up(&primary), &mut backend)
        .unwrap();
    let before = log.lock().unwrap().sent.len();
    engine.handle_backend_event(repeat, &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().sent.len(), before);
    engine
        .handle_backend_event(key_up("h"), &mut backend)
        .unwrap();
    assert!(engine.input.key_dispositions.is_empty());
}

#[test]
fn text_input_borrows_normal_and_resumes_typing_when_modifier_is_released() {
    let config = Config::default();
    let primary = config.text_input.temporary_mode_keys[0].clone();
    let mut engine = text_input_engine(config);
    engine.set_active(ModeId::text_input());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine
        .handle_backend_event(key_down(&primary), &mut backend)
        .unwrap();
    assert_eq!(engine.display_mode(), ModeId::normal());
    engine
        .handle_backend_event(key_down("h"), &mut backend)
        .unwrap();
    assert_eq!(
        engine
            .input
            .active_gestures
            .get(&Key::new("h").unwrap())
            .unwrap()
            .owner,
        ModeId::normal()
    );
    engine
        .handle_backend_event(key_up(&primary), &mut backend)
        .unwrap();
    assert!(engine.input.active_gestures.is_empty());
    engine
        .handle_backend_event(key_up("h"), &mut backend)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::text_input());
    assert_eq!(engine.display_mode(), ModeId::text_input());
    assert_eq!(
        log.lock().unwrap().dispositions,
        [KeyDisposition::Consume; 4]
    );
    log.lock().unwrap().dispositions.clear();
    for event in [key_down("h"), key_up("h")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(
        log.lock().unwrap().dispositions,
        [KeyDisposition::Forward; 2]
    );
}

#[test]
fn text_input_inherits_normal_with_local_overrides_and_validates_layers() {
    let config = Config::parse(
        r#"
[text_input]
inherits = ["normal"]
temporary_mode = "normal"
temporary_mode_keys = ["right_ctrl"]
temporary_mode_passthrough_keys = ["h"]
[text_input.bindings]
h = "backspace"
'\' = "normal"
"#,
    )
    .unwrap();
    let config = Config::parse(&config.to_toml().unwrap()).unwrap();
    let mut engine = text_input_engine(config);
    engine.set_active(ModeId::text_input());
    let lookup = |key: &str, pressed: &[&str]| {
        engine
            .lookup_for_pressed(
                &Key::new(key).unwrap(),
                &pressed
                    .iter()
                    .map(|k| Key::new(k).unwrap())
                    .collect::<Vec<_>>(),
            )
            .map(|r| (*r.binding).clone())
    };
    assert_eq!(
        lookup("h", &["h"]),
        Some(Binding::parse("backspace").unwrap())
    );
    assert_eq!(lookup("j", &["j"]), Some(Binding::Move(Direction::Down)));
    assert_eq!(
        lookup("h", &["right_ctrl", "h"]),
        Some(Binding::parse("backspace").unwrap())
    );
    assert!(
        Config::parse("[text_input]\ninherits=['text_input']")
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(
        Config::parse("[text_input]\ntemporary_mode='missing'")
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(
        Config::parse("[text_input]\ntemporary_mode_keys=['h']")
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn text_input_default_editing_chords_are_opt_in_and_primary_is_aliased() {
    let defaults = Config::default();
    assert_eq!(defaults.text_input.bindings.len(), 3);
    for physical in ["left_alt", "right_ctrl", "left_win"] {
        let config = Config::parse(&format!("[key_aliases]\nprimary = '{physical}'\n[key_aliases.windows]\nprimary = '{physical}'\n[key_aliases.macos]\nprimary = '{physical}'")).unwrap();
        config.validate().unwrap();
        assert_eq!(config.text_input.temporary_mode_keys, [physical]);
        let mut engine = text_input_engine(config);
        engine.set_active(ModeId::text_input());
        let resolved = engine
            .lookup_for_pressed(
                &Key::new("h").unwrap(),
                &[Key::new(physical).unwrap(), Key::new("h").unwrap()],
            )
            .unwrap();
        assert_eq!(*resolved.binding, Binding::Move(Direction::Left));
    }
}

#[test]
fn text_input_can_submit_enter_with_an_ordinary_action_sequence() {
    let shipped = include_str!("../../../../keysteer.default.toml");
    let bindings: Vec<_> = shipped
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .filter(|line| line.starts_with("enter = [") || line.starts_with("'\\ esc' ="))
        .collect();
    assert_eq!(bindings.len(), 2);
    let config =
        Config::parse(&format!("[text_input.bindings]\n{}\n", bindings.join("\n"))).unwrap();
    let mut engine = text_input_engine(config);
    engine.set_active(ModeId::text_input());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for event in [key_down("enter"), key_up("enter")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert_eq!(
        log.lock().unwrap().sent,
        [
            ("enter".into(), KeyState::Down),
            ("enter".into(), KeyState::Up)
        ]
    );
    assert_eq!(
        log.lock().unwrap().dispositions,
        [KeyDisposition::Consume; 2]
    );
}

#[test]
fn text_input_temporary_normal_routes_plugin_screen_commands_to_normal() {
    for literal in [false, true] {
        let mut config = Config::default();
        config.normal.bindings.clear();
        config
            .normal
            .bindings
            .insert("s".into(), Binding::parse("screen next").unwrap());
        let primary = config.resolved_key_aliases()["primary"].clone();
        let mut engine = text_input_engine(config);
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine.screens = backend.screens().unwrap();
        engine.screens.push(Screen {
            bounds: Rect::new(1000.0, 0.0, 1000.0, 800.0),
            work_area: Rect::new(1000.0, 0.0, 1000.0, 800.0),
            is_primary: false,
            scale: 1.0,
            name: None,
        });
        engine.cursor = Point::new(500.0, 400.0);
        engine.set_active(ModeId::text_input());
        engine
            .handle_backend_event(key_down(&primary), &mut backend)
            .unwrap();
        let physical = if literal { "x" } else { "s" };
        for target_screen in [1, 0] {
            let down = if literal {
                character_down(physical, 's')
            } else {
                key_down(physical)
            };
            engine.handle_backend_event(down, &mut backend).unwrap();
            engine
                .handle_backend_event(key_up(physical), &mut backend)
                .unwrap();
            assert!(
                engine.screens[target_screen]
                    .bounds
                    .contains(&engine.cursor),
                "literal={literal}, expected screen={target_screen}, cursor={:?}",
                engine.cursor
            );
            assert_eq!(engine.active_mode(), &ModeId::text_input());
        }
        assert_eq!(log.lock().unwrap().warps.len(), 2);
        engine
            .handle_backend_event(key_up(&primary), &mut backend)
            .unwrap();
        log.lock().unwrap().dispositions.clear();
        for event in [key_down("s"), key_up("s")] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(
            log.lock().unwrap().dispositions,
            [KeyDisposition::Forward; 2]
        );
        assert_eq!(log.lock().unwrap().warps.len(), 2);
    }
}

#[test]
fn text_input_default_indicator_is_disabled_in_code_and_shipped_config() {
    for config in [
        Config::default(),
        Config::parse(include_str!("../../../../keysteer.default.toml")).unwrap(),
    ] {
        assert_eq!(
            config.mode_indicator.modes["text_input"].enabled,
            Some(false)
        );
        let engine = text_input_engine(config);
        assert!(engine.build_indicator(&ModeId::text_input()).is_none());
        assert!(engine.build_indicator(&ModeId::normal()).is_some());
    }
}

#[test]
fn text_input_default_backslash_leaves_f2_available_to_applications() {
    let config = Config::default();
    assert!(!config.normal.bindings.contains_key("f2"));
    assert!(!config.text_input.bindings.contains_key("f2"));
    let mut engine = text_input_engine(config);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    for mode in [ModeId::normal(), ModeId::text_input()] {
        engine.set_active(mode.clone());
        log.lock().unwrap().dispositions.clear();
        for event in [key_down("f2"), key_up("f2")] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(engine.active_mode(), &mode);
        assert_eq!(
            log.lock().unwrap().dispositions,
            [KeyDisposition::Forward; 2]
        );
    }
}

#[test]
fn overlay_editor_uses_layout_text_custom_paste_and_paired_capture() {
    let mut config = Config::default();
    config.ui_hint.search_edit_keys.insert(crate::api::text_edit::EditAction::Paste, "f8".into());
    let mut engine = text_input_engine(config);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.set_active(ModeId::normal());
    engine.activate(ModeId::ui_hint(), Some(ModeId::normal()), &mut backend).unwrap();
    for event in [key_down("/"), key_up("/")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let scan_id = log.lock().unwrap().scan_requests.last().unwrap().id;
    engine.handle_backend_event(BackendEvent::UiScanActivationExpected { id: scan_id, process_id: 700 }, &mut backend).unwrap();
    engine.handle_backend_event(BackendEvent::FocusChanged(None), &mut backend).unwrap();
    engine.handle_backend_event(BackendEvent::FocusChanged(Some(crate::api::FocusedApp { process_id: 700, bundle_id: "test".into(), window_title: "target".into() })), &mut backend).unwrap();
    assert!(engine.scheduler.text_prompt.is_some(), "expected activation must not close search");
    assert_eq!(log.lock().unwrap().scan_requests.len(), 1);
    let mut event = key_down("z");
    if let BackendEvent::Input(input) = &mut event { input.character = Some('Z'); }
    engine.handle_backend_event(event, &mut backend).unwrap();
    engine.handle_backend_event(key_up("z"), &mut backend).unwrap();
    log.lock().unwrap().clipboard_input = "复制🦀".into();
    for event in [key_down("f8"), key_up("f8")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let query = |log: &Recorder| log.scenes.last().unwrap().labels.iter().find(|label| label.z_index == 10_002).unwrap().text.to_string();
    assert_eq!(query(&log.lock().unwrap()), "Z复制🦀");
    for event in [key_down("left"), key_up("left"), key_down("backspace"), key_up("backspace")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(query(&log.lock().unwrap()), "Z复🦀");
    assert!(log.lock().unwrap().text_prompts.is_empty());
    assert!(log.lock().unwrap().dispositions.iter().all(|d| *d == KeyDisposition::Consume));
    for event in [key_down("enter"), key_up("enter")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::ui_hint());
    assert!(engine.scheduler.text_prompt.is_none());
    assert!(!log.lock().unwrap().text_capture);
}

#[test]
fn overlay_search_reentry_and_mode_exit_retire_capture_without_native_windows() {
    let mut engine = text_input_engine(Config::default());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.set_active(ModeId::normal());
    engine.activate(ModeId::ui_hint(), Some(ModeId::normal()), &mut backend).unwrap();
    for _ in 0..32 {
        for event in [key_down("/"), key_up("/"), key_down("a"), key_up("a"), key_down("esc"), key_up("esc")] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert!(engine.scheduler.text_prompt.is_none());
        assert!(!log.lock().unwrap().text_capture);
        assert_eq!(engine.active_mode(), &ModeId::ui_hint());
    }
    for event in [key_down("/"), key_up("/")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    engine.activate(ModeId::normal(), Some(ModeId::ui_hint()), &mut backend).unwrap();
    assert!(!log.lock().unwrap().text_capture);
    assert!(engine.scheduler.text_prompt.is_none());
    assert!(log.lock().unwrap().text_prompts.is_empty());
}

#[test]
fn search_completion_error_and_capture_loss_retire_input_before_owner_is_dropped() {
    for outcome in [Ok(None), Err("editor failed".to_string())] {
        let mut engine = text_input_engine(Config::default());
        let (mut backend, log) = FakeBackend::new(Vec::new());
        engine.screens = backend.screens().unwrap();
        engine.activate(ModeId::ui_hint(), Some(ModeId::normal()), &mut backend).unwrap();
        for event in [key_down("/"), key_up("/")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptResult { id, value: outcome }, &mut backend).unwrap();
        assert!(!log.lock().unwrap().text_capture);
        assert!(engine.scheduler.text_prompt.is_none());
        assert!(!engine.scheduler.text_prompt_returning_focus);
        for event in [key_down("/"), key_up("/")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        let current = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptResult { id, value: Ok(None) }, &mut backend).unwrap();
        assert!(log.lock().unwrap().text_capture, "stale completion must not close the new editor");
        assert_eq!(engine.scheduler.text_prompt.as_ref().unwrap().1.id, current);
        engine.handle_backend_event(BackendEvent::InputCaptureLost("test capture lost".into()), &mut backend).unwrap();
        assert!(!log.lock().unwrap().text_capture);
        assert!(engine.scheduler.text_prompt.is_none());
    }
}

#[test]
fn search_accept_alternatives_close_capture_and_keep_uihint() {
    for binding in ["enter", "/", "primary+q", "f9"] {
        let config = if binding == "f9" { Config::parse("[ui_hint.search_edit_keys]\nf9 = 'accept'").unwrap() } else { Config::default() };
        let primary = if cfg!(target_os = "macos") { "left_win" } else if cfg!(target_os = "windows") { "left_alt" } else { "left_ctrl" };
        let (mut engine, mut backend, log) = point_search_fixture(config);
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "button".into() }, &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["tab"]);
        assert!(log.lock().unwrap().warps.is_empty());
        let modified = binding == "primary+q";
        if modified { engine.handle_backend_event(key_down(primary), &mut backend).unwrap(); }
        let key = if modified { "q" } else { binding };
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        if modified { engine.handle_backend_event(key_up(primary), &mut backend).unwrap(); }
        assert!(engine.scheduler.text_prompt.is_none(), "{binding}");
        assert!(!log.lock().unwrap().text_capture, "{binding}");
        assert_eq!(engine.active_mode(), &ModeId::ui_hint());
        assert_eq!(log.lock().unwrap().warps, [Point::new(40.0, 20.0)], "{binding}");
    }
}


fn point_search_fixture(mut config: Config) -> (Engine, FakeBackend, Arc<Mutex<Recorder>>) {
    config.pointer.tap_distance = 20.0;
    let mut engine = text_input_engine(config);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.activate(ModeId::ui_hint(), Some(ModeId::normal()), &mut backend).unwrap();
    let id = log.lock().unwrap().scan_requests.last().unwrap().id;
    engine.handle_backend_event(BackendEvent::UiScanned(crate::api::UiScanResult {
        id, status: UiScanStatus::Success, retired: Vec::new(),
        targets: ["alpha$", "bravo"].into_iter().enumerate().map(|(i, name)| crate::api::UiTarget {
            rect: Rect::new(10.0 + i as f64 * 20.0, 10.0, 20.0, 20.0), name: name.into(), role: crate::api::SemanticRole::Button,
            details: Some(Box::new(crate::api::geometry::UiTargetDetails { ocr: name.into(), accessibility: name.into(), color: None })),
        }).collect(),
    }), &mut backend).unwrap();
    assert!(log.lock().unwrap().point_requests.is_empty(), "label scanning must not sample pixels");
    point_keys(&mut engine, &mut backend, &["/"]);
    assert!(log.lock().unwrap().point_requests.is_empty(), "empty search must not sample pixels");
    let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
    engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "alpha".into() }, &mut backend).unwrap();
    (engine, backend, log)
}

fn point_keys(engine: &mut Engine, backend: &mut FakeBackend, keys: &[&str]) {
    for key in keys { engine.handle_backend_event(key_down(key), backend).unwrap(); }
    for key in keys.iter().rev() { engine.handle_backend_event(key_up(key), backend).unwrap(); }
}

#[test]
fn ordinary_modes_and_empty_search_do_not_touch_native_sampling() {
    let mut engine = text_input_engine(Config::default());
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    for _ in 0..3 {
        engine.activate(ModeId::ui_hint(), Some(ModeId::normal()), &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["/"]);
        point_keys(&mut engine, &mut backend, &["esc"]);
        engine.activate(ModeId::normal(), None, &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["h"]);
    }
    assert!(log.lock().unwrap().point_requests.is_empty());
    assert_eq!(log.lock().unwrap().point_cancels, 0);
}

#[test]
fn ambiguous_search_shows_first_panel_and_routes_configured_cycles_while_editing() {
    for (source, next) in [
        ("", vec!["tab"]),
        ("[ui_hint.search_bindings]\nctrl = 'point_toggle'\nf9 = 'point_next'", vec!["f9"]),
        ("[ui_hint.search_bindings]\nctrl = 'point_toggle'\n'alt+f9' = 'point_next'", vec!["left_alt", "f9"]),
    ] {
        let (mut engine, mut backend, log) = point_search_fixture(Config::parse(source).unwrap());
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "button".into() }, &mut backend).unwrap();
        assert!(engine.scheduler.point_input.available);
        assert_eq!(log.lock().unwrap().point_requests.last().unwrap().point, Point::new(20.0, 20.0));
        let before_empty = log.lock().unwrap().point_requests.len();
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "".into() }, &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &next);
        assert_eq!(log.lock().unwrap().point_requests.len(), before_empty);
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "button".into() }, &mut backend).unwrap();
        for (expected, name) in [(Point::new(40.0, 20.0), "bravo"), (Point::new(20.0, 20.0), "alpha$"), (Point::new(40.0, 20.0), "bravo")] {
            point_keys(&mut engine, &mut backend, &next);
            assert!(engine.scheduler.point_input.available);
            assert!(!engine.scheduler.point_input.adjusting);
            let recorder = log.lock().unwrap();
            assert_eq!(recorder.point_requests.last().unwrap().point, expected);
            assert!(recorder.scenes.last().unwrap().labels.iter().any(|label| label.text.as_str() == name));
            assert!(recorder.warps.is_empty());
        }
        if next == ["tab"] {
            let requests = log.lock().unwrap().point_requests.len();
            point_keys(&mut engine, &mut backend, &["left_ctrl", "tab"]);
            let mut repeat = key_down("tab");
            if let BackendEvent::Input(input) = &mut repeat { input.repeat = true; }
            engine.handle_backend_event(repeat, &mut backend).unwrap();
            engine.handle_backend_event(key_up("tab"), &mut backend).unwrap();
            assert_eq!(log.lock().unwrap().point_requests.len(), requests);
        }
        point_keys(&mut engine, &mut backend, &next);
        point_keys(&mut engine, &mut backend, &["left_ctrl", "1"]);
        assert_eq!(log.lock().unwrap().copied_text, ["alpha$"]);
        assert!(engine.scheduler.text_prompt.is_none());
        // Result browsing does not enter point adjustment or accept on copy.
        assert!(log.lock().unwrap().warps.is_empty());
        point_keys(&mut engine, &mut backend, &["/"]);
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "button".into() }, &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &next);
        point_keys(&mut engine, &mut backend, &["x"]);
        assert!(!engine.scheduler.point_input.available);
        assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.edit.is_some() && label.text.as_str() == "buttonx"));
    }
}

#[test]
fn multiple_point_search_routes_default_and_custom_cycles_and_copies_joined_ocr() {
    for (config, next) in [(Config::default(), "tab"), (Config::parse("[ui_hint.search_bindings]\nctrl = 'point_toggle'\nf9 = 'point_next'").unwrap(), "f9")] {
        let (mut engine, mut backend, log) = point_search_fixture(config);
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "bravo alpha".into() }, &mut backend).unwrap();
        assert!(engine.scheduler.point_input.available);
        assert_eq!(log.lock().unwrap().point_requests.last().unwrap().point, Point::new(30.0, 20.0));
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        for expected in [Point::new(40.0, 20.0), Point::new(20.0, 20.0), Point::new(40.0, 20.0)] {
            point_keys(&mut engine, &mut backend, &[next]);
            assert_eq!(log.lock().unwrap().point_requests.last().unwrap().point, expected);
            assert!(engine.scheduler.text_prompt.is_some());
            assert!(engine.scheduler.point_input.adjusting);
        }
        point_keys(&mut engine, &mut backend, &["left_ctrl", "1"]);
        assert_eq!(log.lock().unwrap().copied_text, ["bravo\nalpha$"]);
        assert_eq!(log.lock().unwrap().warps, [Point::new(40.0, 20.0)]);
        assert!(engine.scheduler.text_prompt.is_none());
        assert!(!log.lock().unwrap().text_capture);
    }
}

#[test]
fn multiple_point_coordinate_and_color_copies_follow_tab_and_the_current_sample() {
    for field in ["3", "4"] {
        let (mut engine, mut backend, log) = point_search_fixture(Config::default());
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "alpha bravo".into() }, &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.as_str() == "30, 20"));
        point_keys(&mut engine, &mut backend, &["tab"]);
        point_keys(&mut engine, &mut backend, &["tab"]);
        let request = *log.lock().unwrap().point_requests.last().unwrap();
        assert_eq!(request.point, Point::new(40.0, 20.0));
        assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.as_str() == "40, 20"));
        engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request, color: Some(crate::api::Color::rgb(1, 2, 3)) }), &mut backend).unwrap();
        assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.as_str() == "#010203"));
        point_keys(&mut engine, &mut backend, &["left_ctrl", field]);
        let expected = if field == "3" { "40, 20" } else { "#010203" };
        assert_eq!(log.lock().unwrap().copied_text, [expected]);
        assert_eq!(log.lock().unwrap().warps, [Point::new(40.0, 20.0)]);
    }
}

#[test]
fn point_sampling_starts_on_unique_search_result_without_confirmation() {
    let (mut engine, mut backend, log) = point_search_fixture(Config::default());
    assert_eq!(log.lock().unwrap().point_requests.len(), 1);
    assert!(!engine.scheduler.point_input.adjusting);
    let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
    for query in ["missing", ""] {
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: query.into() }, &mut backend).unwrap();
        assert!(engine.scheduler.point_sample.is_none(), "{query}");
        assert!(!engine.scheduler.timers.contains_key("ui_hint.point_sample"));
        assert_eq!(log.lock().unwrap().point_requests.len(), 1);
    }
    engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "bravo".into() }, &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().point_requests.len(), 2);
    assert!(!engine.scheduler.point_input.adjusting);
    point_keys(&mut engine, &mut backend, &["left_ctrl"]);
    point_keys(&mut engine, &mut backend, &["h"]);
    assert_eq!(log.lock().unwrap().point_requests.last().unwrap().point, Point::new(20.0, 20.0));
}

#[test]
fn primary_alt_accept_remains_separate_from_ctrl_coordinate_copy() {
    let platform = if cfg!(target_os = "windows") { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" };
    for key in ["q", "3"] {
        let config = Config::parse(&format!("[key_aliases.{platform}]\nPrimary = \"left_alt\"\n")).unwrap();
        let (mut engine, mut backend, log) = point_search_fixture(config);
        point_keys(&mut engine, &mut backend, &[if key == "q" { "left_alt" } else { "left_ctrl" }, key]);
        assert!(engine.scheduler.text_prompt.is_none(), "Alt+{key}");
        assert!(!log.lock().unwrap().text_capture);
        assert_eq!(engine.active_mode(), &ModeId::ui_hint());
        if key == "3" { assert_eq!(log.lock().unwrap().copied_text, ["20, 20"]); }
        else { assert_eq!(log.lock().unwrap().warps, [Point::new(20.0, 20.0)]); }
    }
}

#[test]
fn search_point_taps_copy_chords_dollar_auto_cycle_and_fresh_color_copy() {
    let config = Config::default();
    let (mut engine, mut backend, log) = point_search_fixture(config);
    assert!(engine.scheduler.point_input.available);
    log.lock().unwrap().fail_copy = true;
    point_keys(&mut engine, &mut backend, &["left_ctrl", "3"]);
    assert!(!engine.scheduler.point_input.adjusting, "copy must disarm the modifier tap");
    log.lock().unwrap().fail_copy = false;
    engine.handle_backend_event(key_down("left_shift"), &mut backend).unwrap();
    let mut dollar = key_down("4");
    if let BackendEvent::Input(input) = &mut dollar { input.character = Some('$'); }
    engine.handle_backend_event(dollar, &mut backend).unwrap();
    for key in ["4", "left_shift"] { engine.handle_backend_event(key_up(key), &mut backend).unwrap(); }
    assert!(!engine.scheduler.point_input.adjusting);
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.as_str() == "alpha$" && l.edit.is_some()));
    point_keys(&mut engine, &mut backend, &["left_ctrl", "left_shift", "4"]);
    assert!(engine.scheduler.point_input.adjusting, "operation automatically enters adjustment");
    let previous = *log.lock().unwrap().point_requests.last().unwrap();
    point_keys(&mut engine, &mut backend, &["l"]);
    assert_eq!(log.lock().unwrap().point_requests.last().unwrap().point, Point::new(40.0, 20.0));
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.as_str() == "bravo" && l.fixed_bounds));
    point_keys(&mut engine, &mut backend, &["left_ctrl", "4"]);
    let fresh = *log.lock().unwrap().point_requests.last().unwrap();
    assert!(fresh.id > previous.id);
    engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request: previous, color: Some(crate::api::Color::rgb(0, 0, 255)) }), &mut backend).unwrap();
    assert!(log.lock().unwrap().copied_text.is_empty());
    engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request: fresh, color: Some(crate::api::Color::rgb(1, 2, 3)) }), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().copied_text, ["rgb(1, 2, 3)"]);
    assert!(engine.scheduler.text_prompt.is_none());
    assert!(engine.scheduler.point_sample.is_none());
    assert!(engine.scheduler.frame_clock_owner.is_none());
    assert!(!log.lock().unwrap().text_capture);
    engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request: fresh, color: Some(crate::api::Color::rgb(9, 9, 9)) }), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().copied_text.len(), 1);
}

#[test]
fn search_point_toggle_back_and_cancel_release_held_movement_and_samples() {
    let (mut engine, mut backend, log) = point_search_fixture(Config::default());
    point_keys(&mut engine, &mut backend, &["left_ctrl"]);
    assert!(engine.scheduler.point_input.adjusting);
    point_keys(&mut engine, &mut backend, &["left_ctrl"]);
    assert!(!engine.scheduler.point_input.adjusting);
    point_keys(&mut engine, &mut backend, &["right_ctrl"]);
    engine.handle_backend_event(key_down("l"), &mut backend).unwrap();
    assert_eq!(engine.scheduler.frame_clock_owner, Some(ModeId::ui_hint()));
    point_keys(&mut engine, &mut backend, &["esc"]);
    assert!(engine.scheduler.point_input.held.is_empty());
    assert!(engine.scheduler.frame_clock_owner.is_none());
    assert!(engine.scheduler.point_sample.is_none());
    let samples = log.lock().unwrap().point_requests.len();
    engine.handle_backend_event(key_up("l"), &mut backend).unwrap();
    engine.handle_backend_event(BackendEvent::Frame(Duration::from_millis(16)), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().point_requests.len(), samples);
    assert!(!engine.scheduler.timers.contains_key("ui_hint.point_sample"));
}

#[test]
fn customized_point_keys_and_all_four_copy_fields_follow_the_adjusted_target() {
    for (key, expected) in [("1", "bravo"), ("2", "bravo · button"), ("3", "40, 20"), ("4", "#010203")] {
        let config = Config::parse("[ui_hint.search_bindings]\nf8 = 'point_toggle'\nf9 = 'color_next'\n").unwrap();
        let (mut engine, mut backend, log) = point_search_fixture(config);
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        assert!(!engine.scheduler.point_input.adjusting, "explicit bindings replace defaults");
        point_keys(&mut engine, &mut backend, &["f8"]);
        assert!(engine.scheduler.point_input.adjusting);
        engine.handle_backend_event(key_down("l"), &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["left_ctrl", key]);
        if key == "4" {
            assert!(log.lock().unwrap().warps.is_empty(), "wait for the current color before accepting");
            let request = *log.lock().unwrap().point_requests.last().unwrap();
            engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request, color: Some(crate::api::Color::rgb(1, 2, 3)) }), &mut backend).unwrap();
        }
        assert_eq!(log.lock().unwrap().copied_text, [expected], "field {key}");
        assert_eq!(log.lock().unwrap().warps, [Point::new(40.0, 20.0)], "copy accepts the adjusted point without Enter");
        assert!(engine.scheduler.text_prompt.is_none());
        assert!(engine.scheduler.point_sample.is_none());
        assert!(engine.scheduler.frame_clock_owner.is_none());
        engine.handle_backend_event(key_up("l"), &mut backend).unwrap();
        assert_eq!(log.lock().unwrap().warps.len(), 1);
    }
}

#[test]
fn failed_point_copy_preserves_adjustment_until_success() {
    let config = Config::default();
    let (mut engine, mut backend, log) = point_search_fixture(config);
    point_keys(&mut engine, &mut backend, &["left_ctrl"]);
    point_keys(&mut engine, &mut backend, &["l"]);
    log.lock().unwrap().fail_copy = true;
    point_keys(&mut engine, &mut backend, &["left_ctrl", "3"]);
    assert!(engine.scheduler.text_prompt.is_some());
    assert!(engine.scheduler.point_input.adjusting);
    assert!(log.lock().unwrap().warps.is_empty());
    log.lock().unwrap().fail_copy = false;
    point_keys(&mut engine, &mut backend, &["left_ctrl", "3"]);
    assert_eq!(log.lock().unwrap().copied_text, ["40, 20"]);
    assert_eq!(log.lock().unwrap().warps, [Point::new(40.0, 20.0)]);
    assert!(engine.scheduler.text_prompt.is_none());
}

#[test]
fn held_speed_modifier_remains_part_of_the_color_cycle_chord() {
    let config = Config::default();
    let (mut engine, mut backend, log) = point_search_fixture(config);
    point_keys(&mut engine, &mut backend, &["left_ctrl"]);
    engine.handle_backend_event(key_down("left_shift"), &mut backend).unwrap();
    point_keys(&mut engine, &mut backend, &["left_ctrl", "4"]);
    engine.handle_backend_event(key_up("left_shift"), &mut backend).unwrap();
    assert!(engine.scheduler.point_input.adjusting);
    assert!(engine.scheduler.point_input.held.is_empty());
    assert!(log.lock().unwrap().copied_text.is_empty());
    let request = *log.lock().unwrap().point_requests.last().unwrap();
    engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request, color: Some(crate::api::Color::rgb(1, 2, 3)) }), &mut backend).unwrap();
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.text.as_str() == "rgb(1, 2, 3)"));
}

#[test]
fn point_status_preserves_search_text_and_original_caret() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let (mut engine, mut backend, log) = point_search_fixture(Config::default());
        engine.handle_backend_event(BackendEvent::AppearanceChanged(appearance), &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["left"]);
        let before_scene = log.lock().unwrap().scenes.last().unwrap().clone();
        let before = before_scene.labels.iter().find(|l| l.edit.is_some()).unwrap().clone();
        let panel = |scene: &crate::api::OverlayScene| scene.shapes.iter().find_map(|shape| match shape {
            crate::api::OverlayShape::Rect { rect, fill, stroke, .. } if rect.contains(&before.rect.center()) => Some((*fill, *stroke)),
            _ => None,
        }).unwrap();
        let before_colors = panel(&before_scene);
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        let during_scene = log.lock().unwrap().scenes.last().unwrap().clone();
        let during = during_scene.labels.iter().find(|l| l.z_index == 10_002 && l.text.as_str() == "alpha").unwrap();
        assert!(during.edit.is_none(), "Point mode has no text caret or selection");
        assert_eq!(during.style, before.style);
        assert!(!during_scene.labels.iter().any(|l| l.text.as_str().contains("Point")));
        assert!(during_scene.shapes.iter().any(|s| matches!(s, crate::api::OverlayShape::Rect { z_index: 10_004, .. })));
        let during_colors = panel(&during_scene);
        assert_ne!(during_colors.0, before_colors.0, "Point background identifies movement mode");
        assert_eq!(during_colors.1, before_colors.1, "Point border inherits search by default");
        assert_eq!((during.rect.x, during.rect.y, during.rect.height), (before.rect.x, before.rect.y, before.rect.height));
        assert_eq!(during.style.text_alignment, crate::api::overlay::TextAlignment::Left);
        assert_eq!(during.style.text_alignment, before.style.text_alignment);
        assert_eq!(during.style.font_size, before.style.font_size);
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        let after_scene = log.lock().unwrap().scenes.last().unwrap().clone();
        let after = after_scene.labels.iter().find(|l| l.edit.is_some()).unwrap();
        assert_eq!(after.text, before.text);
        assert_eq!(after.edit, before.edit);
        assert_eq!(after.style, before.style);
        assert_eq!(panel(&after_scene), before_colors);
    }
}

#[test]
fn point_copy_uses_visible_color_and_custom_keys_then_clears_search() {
    for (cycles, expected) in [(0, "#010203"), (1, "rgb(1, 2, 3)"), (2, "hsl(210, 50%, 1%)")] {
        let config = Config::parse("[ui_hint]\nsearch_copy_keys = ['ctrl+9', 'cmd+2', 'alt+3', 'alt+4']\n").unwrap();
        let (mut engine, mut backend, log) = point_search_fixture(config);
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        for _ in 0..cycles { point_keys(&mut engine, &mut backend, &["left_ctrl", "left_shift", "4"]); }
        let request = *log.lock().unwrap().point_requests.last().unwrap();
        engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request, color: Some(crate::api::Color::rgb(1, 2, 3)) }), &mut backend).unwrap();
        assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.as_str() == expected));
        let requests = log.lock().unwrap().point_requests.len();
        point_keys(&mut engine, &mut backend, &["left_alt", "4"]);
        assert_eq!(log.lock().unwrap().copied_text, [expected]);
        assert_eq!(log.lock().unwrap().point_requests.len(), requests, "visible color copies immediately");
        assert!(engine.scheduler.text_prompt.is_none());
        assert!(engine.scheduler.point_sample.is_none());
        assert!(!log.lock().unwrap().text_capture);
        point_keys(&mut engine, &mut backend, &["/"]);
        let state = log.lock().unwrap();
        assert!(state.scenes.last().unwrap().labels.iter().any(|l| l.edit.is_some() && l.text.is_empty()), "copy cleared the old query and Point status");
    }
}

#[test]
fn point_operation_accepts_before_click_and_pairs_held_release() {
    let (mut engine, mut backend, log) = point_search_fixture(Config::default());
    point_keys(&mut engine, &mut backend, &["left_ctrl"]);
    engine.handle_backend_event(key_down("l"), &mut backend).unwrap();
    point_keys(&mut engine, &mut backend, &[";"]);
    assert_eq!(log.lock().unwrap().warps.first(), Some(&Point::new(40.0, 20.0)));
    assert_eq!(log.lock().unwrap().buttons, [(MouseButton::Left, ButtonAction::Press), (MouseButton::Left, ButtonAction::Release)]);
    assert!(engine.scheduler.text_prompt.is_none());
    assert!(engine.scheduler.frame_clock_owner.is_none());
    engine.handle_backend_event(key_up("l"), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().buttons, [(MouseButton::Left, ButtonAction::Press), (MouseButton::Left, ButtonAction::Release)]);
}

#[test]
fn search_point_color_swatch_tracks_sample_and_optional_geometry() {
    for source in ["", "[ui_hint.search_point.color_preview]\nwidth = 24\nheight = 18\nx_offset = -3\ny_offset = 2\nborder_width = 0", "[ui_hint.search_point.color_preview]\nenabled = false"] {
        let config = Config::parse(source).unwrap();
        config.validate().unwrap();
        let preview = config.ui_hint.search_point.color_preview;
        let (mut engine, mut backend, log) = point_search_fixture(config);
        let color = crate::api::Color::rgb(1, 2, 3);
        let chip = |scene: &crate::api::OverlayScene| scene.shapes.iter().find_map(|shape| match shape {
            crate::api::OverlayShape::Rect { rect, fill, stroke_width, .. } if *fill == color => Some((*rect, *stroke_width)),
            _ => None,
        });
        assert!(chip(log.lock().unwrap().scenes.last().unwrap()).is_none());
        let request = *log.lock().unwrap().point_requests.last().unwrap();
        engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request, color: Some(color) }), &mut backend).unwrap();
        {
            let state = log.lock().unwrap();
            let scene = state.scenes.last().unwrap();
            if preview.enabled {
                let (rect, border) = chip(scene).unwrap();
                let title = scene.labels.iter().find(|label| label.text.as_str() == "Color").unwrap();
                assert_eq!(rect.width, f64::from(preview.width));
                assert_eq!(rect.height, f64::from(preview.height));
                assert_eq!(rect.center().y, title.rect.center().y + f64::from(preview.y_offset));
                assert!(rect.x > title.rect.x);
                assert_eq!(border, f64::from(preview.border_width));
            } else { assert!(chip(scene).is_none()); }
        }
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        point_keys(&mut engine, &mut backend, &["l"]);
        assert!(chip(log.lock().unwrap().scenes.last().unwrap()).is_none(), "moving clears the old color immediately");
    }
}

#[test]
fn point_search_reopens_at_new_target_after_copy_accept_or_cancel() {
    for exit_keys in [&["left_ctrl", "3"][..], &["enter"][..], &["esc"][..]] {
        let (mut engine, mut backend, log) = point_search_fixture(Config::default());
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        point_keys(&mut engine, &mut backend, &["l"]);
        point_keys(&mut engine, &mut backend, &["l"]);
        let old = *log.lock().unwrap().point_requests.last().unwrap();
        assert_eq!(old.point, Point::new(60.0, 20.0));
        point_keys(&mut engine, &mut backend, exit_keys);
        assert!(engine.scheduler.text_prompt.is_none());
        assert!(!engine.scheduler.point_input.available);
        assert!(engine.scheduler.point_sample.is_none());
        point_keys(&mut engine, &mut backend, &["/"]);
        let id = engine.scheduler.text_prompt.as_ref().unwrap().1.id;
        engine.handle_backend_event(BackendEvent::TextPromptChanged { id, text: "bravo".into() }, &mut backend).unwrap();
        point_keys(&mut engine, &mut backend, &["left_ctrl"]);
        let fresh = *log.lock().unwrap().point_requests.last().unwrap();
        assert_eq!(fresh.point, Point::new(40.0, 20.0));
        assert_ne!(fresh.id, old.id);
        engine.handle_backend_event(BackendEvent::PointSampled(crate::api::point_sample::Sample { request: old, color: Some(crate::api::Color::rgb(255, 0, 0)) }), &mut backend).unwrap();
        {
            let state = log.lock().unwrap();
            let scene = state.scenes.last().unwrap();
            assert!(scene.cursor_marker.is_none(), "the previous confirmed cursor must not appear as a second inspection point");
            let points: Vec<_> = scene.shapes.iter().filter_map(|shape| match shape {
                crate::api::OverlayShape::Rect { rect, z_index: 10_003, .. } => Some(rect.center()),
                _ => None,
            }).collect();
            assert_eq!(points, [Point::new(40.0, 20.0)]);

            assert!(scene.labels.iter().any(|l| l.text.as_str() == "40, 20"));
            assert!(scene.labels.iter().any(|l| l.text.as_str() == "bravo"));
            assert!(!scene.labels.iter().any(|l| l.text.as_str() == "#FF0000"));
        }
        point_keys(&mut engine, &mut backend, &["h"]);
        assert_eq!(log.lock().unwrap().point_requests.last().unwrap().point, Point::new(20.0, 20.0));
    }
}
