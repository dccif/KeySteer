#[test]
fn quick_switch_freezes_ranking_and_consumes_both_chord_edges() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &["grid", "scroll", "plugin:screen-selector", "plugin:window-mover"]);
    let mut store = crate::app::preset_store::PresetStore::default();
    for _ in 0..10 {
        store.record_mode_entry("grid", 100);
    }
    engine.attach_preset_store(Box::new(store));
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine.start_runtime(&mut backend).unwrap();
    for event in enter_normal() {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    engine
        .handle_backend_event(key_down("q"), &mut backend)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::idle());
    engine.quick_switch.pending.as_mut().unwrap().deadline = Instant::now();
    engine.fire_quick_switch(&mut backend).unwrap();
    assert!(engine.quick_switch.pending.as_ref().unwrap().visible);
    assert!(!log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.text.as_str().starts_with("plugin:")));
    assert!(!log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.text.as_str() == "normal"));
    assert!(
        log.lock()
            .unwrap()
            .scenes
            .last()
            .unwrap()
            .labels
            .iter()
            .any(|l| l.text.as_str().eq("grid"))
    );
    for _ in 0..20 {
        engine.window_presets.store.record_mode_entry("scroll", 100);
    }
    for event in [key_down("1"), key_up("q"), key_up("1")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode().as_str(), "grid");
    assert!(engine.input.pressed.is_empty());
    assert!(
        log.lock()
            .unwrap()
            .dispositions
            .iter()
            .rev()
            .take(4)
            .all(|d| *d == KeyDisposition::Consume)
    );
}

#[test]
fn quick_switch_idle_blacklist_and_same_mode_count_semantics() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &["grid"]);
    engine.attach_preset_store(Box::new(crate::app::preset_store::PresetStore::default()));
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine.start_runtime(&mut backend).unwrap();
    for event in [key_down("q"), key_up("q")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(engine.quick_switch.pending.is_none());
    assert!(
        log.lock()
            .unwrap()
            .dispositions
            .iter()
            .all(|d| *d == KeyDisposition::Forward)
    );
    engine.settings.quick_switch.blacklist = vec!["grid".into(), "idle".into()];
    engine.activate(ModeId::grid(), None, &mut backend).unwrap(); // Blacklist doesn't ban ordinary entry.
    engine.set_active(ModeId::grid());
    assert_eq!(engine.window_presets.store.mode_usage()["grid"], 1);
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    for event in [key_down("q"), key_down("1"), key_up("1"), key_up("q")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::idle());
    assert!(
        !log.lock()
            .unwrap()
            .scenes
            .iter()
            .any(|scene| scene.labels.iter().any(|label| label.z_index == 10000)),
        "immediate Q+digit must not flash the switcher"
    );
}

#[test]
fn quick_switch_cancelled_by_capture_loss_can_be_used_again() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    let (mut backend, _) = FakeBackend::new(vec![]);
    engine.start_runtime(&mut backend).unwrap();
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("q"), &mut backend)
        .unwrap();
    engine.quick_switch.pending.as_mut().unwrap().deadline = Instant::now();
    engine.fire_quick_switch(&mut backend).unwrap();
    engine.cancel_quick_switch(true);
    engine.input.forget_physical_capture();
    engine.activate(ModeId::normal(), None, &mut backend).unwrap();
    engine
        .handle_backend_event(key_down("q"), &mut backend)
        .unwrap();
    assert!(engine.quick_switch.pending.is_some());
    engine
        .handle_backend_event(key_up("q"), &mut backend)
        .unwrap();
    assert_eq!(engine.active_mode(), &ModeId::idle());
}

#[test]
fn system_checkpoint_acknowledges_after_saving_below_threshold() {
    let path = std::env::temp_dir().join(format!(
        "keysteer-usage-shutdown-{}-{}.ksw",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    engine.attach_preset_store(Box::new(crate::app::preset_store::PresetStore::persistent(
        path.clone(),
        |source, target| std::fs::rename(source, target),
    )));
    let (mut backend, _) = FakeBackend::new(vec![]);
    engine.start_runtime(&mut backend).unwrap();
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    assert!(!path.exists());
    let (reply, saved) = std::sync::mpsc::channel();
    engine
        .handle_backend_event(BackendEvent::SaveWorkspace(reply), &mut backend)
        .unwrap();
    saved.try_recv().unwrap();
    let restored =
        crate::app::preset_store::PresetStore::persistent(path.clone(), |source, target| {
            std::fs::rename(source, target)
        });
    assert_eq!(restored.mode_usage()["normal"], 1);
    assert!(!engine.should_quit()); // A cancelled system shutdown must not terminate the engine.
    std::fs::remove_file(path).unwrap();
}

#[test]
fn quick_switch_trigger_delivers_down_repeat_and_up_without_waiting_or_replay() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_probes(&seen, "q", "send left");
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    engine
        .handle_backend_event(key_down("q"), &mut backend)
        .unwrap();
    assert!(engine.quick_switch.pending.is_some());
    let expected = vec![
        ("arrow_left".to_string(), KeyState::Down),
        ("arrow_left".to_string(), KeyState::Up),
    ];
    assert_eq!(log.lock().unwrap().sent, expected);
    let mut repeated = match key_down("q") {
        BackendEvent::Input(input) => input,
        _ => unreachable!(),
    };
    repeated.repeat = true;
    engine
        .handle_backend_event(BackendEvent::Input(repeated), &mut backend)
        .unwrap();
    assert_eq!(
        log.lock().unwrap().sent,
        [expected.clone(), expected.clone()].concat()
    );
    engine
        .handle_backend_event(key_up("q"), &mut backend)
        .unwrap();
    assert_eq!(
        log.lock().unwrap().sent,
        [expected.clone(), expected.clone()].concat()
    );
    assert!(engine.quick_switch.pending.is_none());
}

#[test]
fn quick_switch_observes_raw_grid_selection_without_delaying_or_replaying_it() {
    for mode in [ModeId::grid(), ModeId::recursive_grid()] {
        let mut config = Config::default();
        config.grid.cursor_follow_selection = true;
        config.recursive_grid.cursor_follow_selection = true;
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        for mode in crate::app::mode_catalog::built_in(&config) {
            engine.register(mode);
        }
        let (mut backend, log) = FakeBackend::new(vec![]);
        engine.start_runtime(&mut backend).unwrap();
        engine.activate(mode.clone(), None, &mut backend).unwrap();
        let before = log.lock().unwrap().warps.len();
        engine
            .handle_backend_event(key_down("q"), &mut backend)
            .unwrap();
        let selected = log.lock().unwrap().warps.len();
        assert!(
            selected > before,
            "{mode} must select on Down before hold_ms"
        );
        assert!(engine.quick_switch.pending.is_some());
        engine
            .handle_backend_event(key_up("q"), &mut backend)
            .unwrap();
        assert_eq!(
            log.lock().unwrap().warps.len(),
            selected,
            "Up must not repeat selection"
        );
    }
}

#[test]
fn quick_switch_blocks_trigger_repeats_after_hold_and_cancel_until_release() {
    for mode in [ModeId::grid(), ModeId::recursive_grid()] {
        for trigger in ["q", "a"] {
            let mut config = Config::default();
            config.quick_switch.key = trigger.into();
            config.grid.cursor_follow_selection = true;
            config.recursive_grid.cursor_follow_selection = true;
            let mut engine = Engine::from_plan(
                crate::app::configuration::compile(&config).unwrap(),
                Appearance::Dark,
            )
            .unwrap();
            let (mut backend, log) = FakeBackend::new(vec![]);
            engine.start_runtime(&mut backend).unwrap();
            engine.activate(mode.clone(), None, &mut backend).unwrap();
            let before = log.lock().unwrap().warps.len();
            engine
                .handle_backend_event(key_down(trigger), &mut backend)
                .unwrap();
            let selected = log.lock().unwrap().warps.len();
            assert!(selected > before);
            let repeat = || {
                let BackendEvent::Input(mut event) = key_down(trigger) else {
                    unreachable!()
                };
                event.repeat = true;
                BackendEvent::Input(event)
            };
            engine.quick_switch.pending.as_mut().unwrap().deadline = Instant::now();
            // A repeat may arrive before the scheduler delivers the deadline.
            engine.handle_backend_event(repeat(), &mut backend).unwrap();
            assert_eq!(log.lock().unwrap().warps.len(), selected);
            engine.fire_quick_switch(&mut backend).unwrap();
            assert!(engine.quick_switch.pending.as_ref().unwrap().visible);
            for _ in 0..20 {
                engine.handle_backend_event(repeat(), &mut backend).unwrap();
            }
            assert_eq!(engine.active_mode(), &mode);
            assert_eq!(log.lock().unwrap().warps.len(), selected);
            for event in [key_down("esc"), key_up("esc")] {
                engine.handle_backend_event(event, &mut backend).unwrap();
            }
            assert!(engine.quick_switch.pending.is_none());
            for _ in 0..20 {
                engine.handle_backend_event(repeat(), &mut backend).unwrap();
            }
            assert_eq!(log.lock().unwrap().warps.len(), selected);
            engine
                .handle_backend_event(key_up(trigger), &mut backend)
                .unwrap();
            assert!(engine.input.pressed.is_empty());
            engine
                .handle_backend_event(key_down(trigger), &mut backend)
                .unwrap();
            assert!(
                log.lock().unwrap().warps.len() > selected,
                "fresh Down must work"
            );
        }
    }
}

#[test]
fn quick_switch_selection_blocks_held_trigger_in_destination_mode() {
    let mut engine = Engine::from_plan(
        crate::app::configuration::compile(&Config::default()).unwrap(),
        Appearance::Dark,
    )
    .unwrap();
    let mut store = crate::app::preset_store::PresetStore::default();
    for _ in 0..10 {
        store.record_mode_entry("grid", 100);
    }
    engine.attach_preset_store(Box::new(store));
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine.start_runtime(&mut backend).unwrap();
    engine
        .activate(ModeId::normal(), None, &mut backend)
        .unwrap();
    for event in [key_down("q"), key_down("1"), key_up("1")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::grid());
    let before = log.lock().unwrap().warps.len();
    for _ in 0..20 {
        let BackendEvent::Input(mut input) = key_down("q") else {
            unreachable!()
        };
        input.repeat = true;
        engine
            .handle_backend_event(BackendEvent::Input(input), &mut backend)
            .unwrap();
    }
    assert_eq!(log.lock().unwrap().warps.len(), before);
    assert_eq!(engine.active_mode(), &ModeId::grid());
    engine
        .handle_backend_event(key_up("q"), &mut backend)
        .unwrap();
    assert!(engine.input.pressed.is_empty());
    engine
        .handle_backend_event(key_down("q"), &mut backend)
        .unwrap();
    assert!(log.lock().unwrap().warps.len() > before);
}
