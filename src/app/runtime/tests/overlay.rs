#[test]
fn identical_scenes_are_presented_only_once() {
    struct Redrawer(ModeId);
    impl Mode for Redrawer {
        fn id(&self) -> ModeId {
            self.0.clone()
        }
        fn handle(&mut self, event: &ModeEvent, _c: &HostContext<'_>) -> CommandBatch {
            CommandBatch::from(match event {
                ModeEvent::Activated { .. } | ModeEvent::Key { .. } => {
                    vec![Command::show_overlay(OverlayScene::new())]
                }
                _ => Vec::new(),
            })
        }
    }

    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(Redrawer(ModeId::idle())));
    let (mut backend, log) = FakeBackend::new(vec![key_down("a"), key_down("b")]);
    engine.run(&mut backend).unwrap();

    // Activation draws once; the two identical redraws are suppressed.
    assert_eq!(log.lock().unwrap().presents, 1);
}

fn visible_normal_overlay() -> (Engine, FakeBackend, Arc<Mutex<Recorder>>) {
    let config = Config::default();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    for mode in crate::app::mode_catalog::built_in(&config) {
        engine.register(mode);
    }
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.cursor = backend.pointer().unwrap();
    engine
        .show_overlay(Arc::new(OverlayScene::new()), &mut backend)
        .unwrap();
    (engine, backend, log)
}

fn visible_dense_overlay() -> (Engine, FakeBackend, Arc<Mutex<Recorder>>) {
    let (mut engine, mut backend, log) = visible_normal_overlay();
    let mut scene = OverlayScene::with_capacity(0, 1_000);
    for index in 0..1_000 {
        scene.labels.push(crate::api::overlay::OverlayLabel::new(
            format!("{index:03}"),
            Rect::new(
                (index % 40) as f64 * 24.0,
                (index / 40) as f64 * 24.0,
                22.0,
                22.0,
            ),
            crate::api::overlay::LabelStyle::default(),
        ));
    }
    engine.show_overlay(Arc::new(scene), &mut backend).unwrap();
    backend.accept_position_updates = true;
    (engine, backend, log)
}

#[test]
fn pointer_motion_uses_position_only_backend_update() {
    let (mut engine, mut backend, log) = visible_normal_overlay();
    backend.accept_position_updates = true;
    engine.cursor = Point::new(200.0, 300.0);

    engine.refresh_overlay_positions(&mut backend).unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.presents, 1, "movement rebuilt the complete scene");
    assert_eq!(log.positions.len(), 1);
    assert_eq!(log.positions[0].0, Some(Point::new(200.0, 300.0)));
    assert!(log.positions[0].1.is_some());
}

#[test]
#[ignore = "allocation probe; run alone with --test-threads=1"]
fn overlay_position_path_is_allocation_free() {
    let (mut engine, mut backend, log) = visible_dense_overlay();
    engine.cursor = Point::new(100.0, 100.0);
    engine.refresh_overlay_positions(&mut backend).unwrap();
    log.lock().unwrap().positions.clear();

    let region = Region::new(TEST_ALLOCATOR);
    for index in 0..10_000 {
        engine.cursor = Point::new(100.0 + (index % 500) as f64, 100.0);
        engine.refresh_overlay_positions(&mut backend).unwrap();
        log.lock().unwrap().positions.clear();
    }
    let change = region.change();
    assert_eq!(change.allocations, 0, "position path allocated: {change:?}");
    assert_eq!(change.deallocations, 0, "position path freed: {change:?}");
    assert_eq!(
        change.bytes_allocated, 0,
        "position path allocated: {change:?}"
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn overlay_position_performance_probe() {
    const WARMUP: usize = 1_000;
    const SAMPLES: usize = 10_000;
    fn measure(mut operation: impl FnMut(usize)) -> (u128, u128, u128) {
        for index in 0..WARMUP {
            operation(index);
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for index in 0..SAMPLES {
            let started = Instant::now();
            operation(index);
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    let (mut fast, mut fast_backend, fast_log) = visible_dense_overlay();
    let position = measure(|index| {
        fast.cursor = Point::new(100.0 + (index % 500) as f64, 100.0);
        fast.refresh_overlay_positions(&mut fast_backend).unwrap();
        fast_log.lock().unwrap().positions.clear();
    });

    let (mut full, mut full_backend, full_log) = visible_dense_overlay();
    let complete = measure(|index| {
        full.cursor = Point::new(100.0 + (index % 500) as f64, 100.0);
        full.refresh_overlay(&mut full_backend).unwrap();
        let mut log = full_log.lock().unwrap();
        log.scenes.clear();
        log.timeline.clear();
    });
    println!(
        "overlay_probe samples={SAMPLES} position_p50={}ns position_p95={}ns position_p99={}ns complete_p50={}ns complete_p95={}ns complete_p99={}ns",
        position.0, position.1, position.2, complete.0, complete.1, complete.2
    );
}

#[test]
#[ignore = "allocation probe; run alone with --test-threads=1"]
fn pending_long_press_wait_is_allocation_free() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    engine.registry.active = ModeId::normal();
    engine
        .input
        .pending_long_press_toggles
        .push(PendingLongPressToggle {
            fires_at: Instant::now() + Duration::from_secs(60),
            key: Key::new("n").unwrap(),
            target: InputTarget::Key(Key::new("n").unwrap()),
            short_action: None,
        });
    let (mut backend, _) = FakeBackend::new(Vec::new());

    let region = Region::new(TEST_ALLOCATOR);
    for _ in 0..10_000 {
        engine.fire_due_long_press_toggles(&mut backend).unwrap();
    }
    let change = region.change();
    assert_eq!(change.allocations, 0, "waiting allocated: {change:?}");
    assert_eq!(change.deallocations, 0, "waiting freed: {change:?}");
    assert_eq!(change.bytes_allocated, 0, "waiting allocated: {change:?}");
}

#[test]
#[ignore = "allocation probe; run alone with --test-threads=1"]
fn pending_short_press_queue_is_allocation_free() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    let pending = PendingLongPressToggle {
        fires_at: Instant::now() + Duration::from_secs(60),
        key: Key::new("x").unwrap(),
        target: InputTarget::Mouse(Button::Left),
        short_action: Some(ButtonAction::Click),
    };

    let region = Region::new(TEST_ALLOCATOR);
    for _ in 0..10_000 {
        engine
            .input
            .pending_long_press_toggles
            .push(pending.clone());
        std::hint::black_box(engine.take_pending_long_press_toggle(&pending.key));
    }
    let change = region.change();
    assert_eq!(change.allocations, 0, "queue allocated: {change:?}");
    assert_eq!(change.deallocations, 0, "queue freed: {change:?}");
    assert_eq!(change.bytes_allocated, 0, "queue allocated: {change:?}");
}

#[test]
#[ignore = "allocation probe; run alone with --test-threads=1"]
fn common_toggle_transaction_is_allocation_free() {
    let mut engine = engine_with_normal_binding("n", "toggle");
    let (mut backend, log) = FakeBackend::new(Vec::new());
    let targets = [
        InputTarget::Mouse(Button::Left),
        InputTarget::Mouse(Button::Right),
    ];
    engine.toggle_targets(&targets, &mut backend).unwrap();
    engine.toggle_targets(&targets, &mut backend).unwrap();
    {
        let mut log = log.lock().unwrap();
        log.buttons.clear();
        log.timeline.clear();
    }

    let region = Region::new(TEST_ALLOCATOR);
    for _ in 0..10_000 {
        engine.toggle_targets(&targets, &mut backend).unwrap();
        engine.toggle_targets(&targets, &mut backend).unwrap();
        let mut log = log.lock().unwrap();
        log.buttons.clear();
        log.timeline.clear();
    }
    let change = region.change();
    assert_eq!(change.allocations, 0, "toggle allocated: {change:?}");
    assert_eq!(change.deallocations, 0, "toggle freed: {change:?}");
    assert_eq!(change.bytes_allocated, 0, "toggle allocated: {change:?}");
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn deadline_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn measure(mut operation: impl FnMut()) -> (u128, u128, u128) {
        for _ in 0..WARMUP {
            operation();
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            for _ in 0..CALLS_PER_SAMPLE {
                operation();
            }
            samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
        }
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    fn engine_with_timers(count: usize) -> Engine {
        let mut engine = Engine::new(Config::default(), Appearance::Dark);
        let now = Instant::now();
        for index in 0..count {
            engine.scheduler.timers.insert(
                format!("timer-{index}"),
                Timer {
                    fires_at: now + Duration::from_secs(30),
                    last_fired: now,
                    interval: None,
                    owner: ModeId::idle(),
                },
            );
        }
        engine
    }

    let no_tasks = engine_with_timers(0);
    let one_timer = engine_with_timers(1);
    let simultaneous = engine_with_timers(8);
    let no_tasks_result = measure(|| {
        std::hint::black_box(no_tasks.next_timeout());
    });
    let one_timer_result = measure(|| {
        std::hint::black_box(one_timer.next_timeout());
    });
    let simultaneous_result = measure(|| {
        std::hint::black_box(simultaneous.next_timeout());
    });

    let mut idle = engine_with_timers(0);
    let (mut backend, _) = FakeBackend::new(Vec::new());
    let service_result = measure(|| {
        idle.fire_due_long_press_toggles(&mut backend).unwrap();
        idle.fire_due_timers(&mut backend).unwrap();
        idle.fire_due_sequences(&mut backend).unwrap();
    });

    println!(
        "deadline_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} no_tasks_p50={}ns no_tasks_p95={}ns no_tasks_p99={}ns one_timer_p50={}ns one_timer_p95={}ns one_timer_p99={}ns simultaneous_p50={}ns simultaneous_p95={}ns simultaneous_p99={}ns idle_service_p50={}ns idle_service_p95={}ns idle_service_p99={}ns",
        no_tasks_result.0,
        no_tasks_result.1,
        no_tasks_result.2,
        one_timer_result.0,
        one_timer_result.1,
        one_timer_result.2,
        simultaneous_result.0,
        simultaneous_result.1,
        simultaneous_result.2,
        service_result.0,
        service_result.1,
        service_result.2,
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn toggle_performance_probe() {
    const WARMUP: usize = 1_000;
    const SAMPLES: usize = 10_000;
    fn measure(mut operation: impl FnMut()) -> (u128, u128, u128) {
        for _ in 0..WARMUP {
            operation();
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            operation();
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }
    fn clear_log(log: &Arc<Mutex<Recorder>>) {
        let mut log = log.lock().unwrap();
        log.scenes.clear();
        log.positions.clear();
        log.timeline.clear();
        log.dispositions.clear();
        log.buttons.clear();
        log.sent.clear();
    }

    let mut bare = engine_with_normal_binding("n", "toggle");
    bare.registry.active = ModeId::normal();
    let (mut bare_backend, bare_log) = FakeBackend::new(Vec::new());
    let bare_result = measure(|| {
        bare.handle_backend_event(key_down("n"), &mut bare_backend)
            .unwrap();
        bare.handle_backend_event(key_up("n"), &mut bare_backend)
            .unwrap();
        clear_log(&bare_log);
    });

    let mut repeated = engine_with_normal_binding("n", "toggle");
    repeated.registry.active = ModeId::normal();
    let (mut repeated_backend, repeated_log) = FakeBackend::new(Vec::new());
    let mut repeat_down = match key_down("n") {
        BackendEvent::Input(input) => input,
        _ => unreachable!(),
    };
    repeat_down.repeat = true;
    let repeat_result = measure(|| {
        repeated
            .handle_backend_event(key_down("n"), &mut repeated_backend)
            .unwrap();
        for _ in 0..100 {
            repeated
                .handle_key(repeat_down.clone(), &mut repeated_backend)
                .unwrap();
        }
        repeated
            .handle_backend_event(key_up("n"), &mut repeated_backend)
            .unwrap();
        clear_log(&repeated_log);
    });

    let mut chord = engine_with_normal_binding("n", "toggle");
    chord.registry.active = ModeId::normal();
    let (mut chord_backend, chord_log) = FakeBackend::new(Vec::new());
    let chord_result = measure(|| {
        for event in [
            key_down("left_ctrl"),
            key_down("left_shift"),
            key_down("e"),
            key_down("n"),
            key_up("n"),
            key_up("e"),
            key_up("left_shift"),
            key_up("left_ctrl"),
            key_down("n"),
            key_up("n"),
        ] {
            chord
                .handle_backend_event(event, &mut chord_backend)
                .unwrap();
        }
        clear_log(&chord_log);
    });

    let mut mouse_config = Config::default();
    mouse_config.normal.bindings.clear();
    mouse_config
        .normal
        .bindings
        .insert("n".into(), Binding::Toggle(Vec::new()));
    mouse_config.normal.bindings.insert(
        ";".into(),
        Binding::Click(crate::api::binding::Button::Left),
    );
    let mut mouse = Engine::new(mouse_config, Appearance::Dark);
    let seen = Arc::new(Mutex::new(Vec::new()));
    mouse.register(Box::new(ProbeMode::new("normal", seen)));
    mouse.registry.active = ModeId::normal();
    let (mut mouse_backend, mouse_log) = FakeBackend::new(Vec::new());
    let mouse_result = measure(|| {
        for event in [
            key_down("n"),
            key_down(";"),
            key_up(";"),
            key_up("n"),
            key_down("n"),
            key_up("n"),
        ] {
            mouse
                .handle_backend_event(event, &mut mouse_backend)
                .unwrap();
        }
        clear_log(&mouse_log);
    });

    let left = InputTarget::Key(Key::new("left_ctrl").unwrap());
    let right = InputTarget::Key(Key::new("e").unwrap());
    let rollback_targets = [left, right];
    let mut rollback = engine_with_normal_binding("n", "toggle");
    rollback.registry.active = ModeId::normal();
    let (mut rollback_backend, rollback_log) = FakeBackend::new(Vec::new());
    rollback
        .press_targets(&rollback_targets, &mut rollback_backend)
        .unwrap();
    let rollback_result = measure(|| {
        rollback_log.lock().unwrap().fail_next_key_up = true;
        let _ = rollback.toggle_targets(&rollback_targets, &mut rollback_backend);
        clear_log(&rollback_log);
    });

    println!(
        "toggle_probe samples={SAMPLES} bare_p50={}ns bare_p95={}ns bare_p99={}ns repeat100_p50={}ns repeat100_p95={}ns repeat100_p99={}ns chord_p50={}ns chord_p95={}ns chord_p99={}ns mouse_p50={}ns mouse_p95={}ns mouse_p99={}ns rollback_p50={}ns rollback_p95={}ns rollback_p99={}ns",
        bare_result.0,
        bare_result.1,
        bare_result.2,
        repeat_result.0,
        repeat_result.1,
        repeat_result.2,
        chord_result.0,
        chord_result.1,
        chord_result.2,
        mouse_result.0,
        mouse_result.1,
        mouse_result.2,
        rollback_result.0,
        rollback_result.1,
        rollback_result.2,
    );
}

#[test]
fn unsupported_or_failed_position_update_falls_back_to_full_scene() {
    for fail in [false, true] {
        let (mut engine, mut backend, log) = visible_normal_overlay();
        backend.fail_position_updates = fail;
        engine.cursor = Point::new(200.0, 300.0);

        engine.refresh_overlay_positions(&mut backend).unwrap();
        engine.cursor = Point::new(300.0, 300.0);
        engine.refresh_overlay_positions(&mut backend).unwrap();

        let log = log.lock().unwrap();
        assert_eq!(log.presents, 3, "fail={fail}: no complete fallback");
        assert_eq!(
            log.position_update_attempts, 1,
            "fail={fail}: rejected fast path was retried"
        );
        let scene = log.scenes.last().unwrap();
        assert_eq!(
            scene.cursor_marker.as_ref().map(|marker| marker.center),
            Some(Point::new(300.0, 300.0))
        );
    }
}

#[test]
fn crossing_a_screen_edge_rebuilds_the_cursor_only_overlay() {
    let (mut engine, mut backend, log) = visible_normal_overlay();
    backend.accept_position_updates = true;
    engine.screens.push(Screen {
        bounds: Rect::new(1000.0, 0.0, 1000.0, 800.0),
        work_area: Rect::new(1000.0, 0.0, 1000.0, 800.0),
        is_primary: false,
        scale: 2.0,
        name: None,
    });
    engine.cursor = Point::new(1200.0, 300.0);

    engine.refresh_overlay_positions(&mut backend).unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.presents, 2);
    assert!(log.positions.is_empty());
    assert_eq!(
        log.scenes.last().and_then(|scene| scene.clip),
        Some(Rect::new(1000.0, 0.0, 1000.0, 800.0))
    );
}

#[test]
fn one_command_batch_submits_only_its_final_overlay_state() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.cursor = backend.pointer().unwrap();

    let mut first = OverlayScene::new();
    first.backdrop = Some(crate::api::Color::rgb(1, 2, 3));
    let mut final_scene = OverlayScene::new();
    final_scene.backdrop = Some(crate::api::Color::rgb(4, 5, 6));
    engine
        .execute(
            vec![
                Command::show_overlay(first),
                Command::warp_to(Point::new(30.0, 40.0)),
                Command::show_overlay(final_scene),
            ],
            &mut backend,
        )
        .unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.warps, vec![Point::new(30.0, 40.0)]);
    assert_eq!(log.presents, 1);
    assert_eq!(
        log.scenes[0].backdrop,
        Some(crate::api::Color::rgb(4, 5, 6))
    );
}

