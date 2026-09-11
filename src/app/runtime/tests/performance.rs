#[test]
fn simulator_menu_uses_the_active_config_source() {
    let source = "# current profile\n[normal]\nlong_press_toggle_ms = 777\n";
    let mut engine = Engine::new(Config::parse(source).unwrap(), Appearance::Dark);
    attach_test_repository(
        &mut engine,
        Config::parse(source).unwrap(),
        Some(ConfigStore::from_validated_text(
            "keysteer.profile.toml",
            source.to_string(),
            crate::platform::atomic_replace,
        )),
        None,
        source.to_string(),
    );
    let (mut backend, log) = FakeBackend::new(vec![BackendEvent::OpenConfigSimulator]);

    engine.run(&mut backend).unwrap();

    assert_eq!(
        log.lock().unwrap().opened_urls,
        [config_handoff::url_for_config(source)]
    );
}

#[test]
fn simulator_menu_includes_current_saved_presets_without_manual_import() {
    let config = Config::default(); let source = config.to_toml().unwrap();
    let mut engine = Engine::new(config, Appearance::Dark);
    engine.attach_preset_store(Box::new(crate::app::preset_store::PresetStore::default()));
    engine.window_presets.store.save(crate::api::window_presets::RegionTemplate::Slot { id: 1 }.into(), 1, "Coding".into()).unwrap();
    let expected = config_handoff::url_for_workspace(&source, engine.window_presets.store.export_file());
    let (mut backend, log) = FakeBackend::new(vec![BackendEvent::OpenConfigSimulator]);
    engine.run(&mut backend).unwrap();
    assert_eq!(log.lock().unwrap().opened_urls, [expected]);
}

#[test]
fn simulator_menu_serializes_config_without_a_store() {
    let config = Config::default();
    let expected = config_handoff::url_for_config(&config.to_toml().unwrap());
    let mut engine = Engine::new(config, Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(vec![BackendEvent::OpenConfigSimulator]);

    engine.run(&mut backend).unwrap();

    assert_eq!(log.lock().unwrap().opened_urls, [expected]);
}

#[test]
fn simulator_menu_debounces_rapid_repeated_clicks() {
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let (mut backend, log) = FakeBackend::new(vec![
        BackendEvent::OpenConfigSimulator,
        BackendEvent::OpenConfigSimulator,
        BackendEvent::OpenConfigSimulator,
    ]);

    engine.run(&mut backend).unwrap();

    assert_eq!(log.lock().unwrap().opened_urls.len(), 1);
}

#[test]
fn rejected_mouse_injection_does_not_stop_the_engine() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    let mut normal = ProbeMode::new("normal", seen);
    normal.on_key = vec![Command::click(MouseButton::Right)];
    engine.register(Box::new(idle));
    engine.register(Box::new(normal));

    let events = enter_normal().into_iter().chain([key_down("x")]).collect();
    let (mut backend, log) = FakeBackend::new(events);
    backend.fail_mouse = true;

    assert!(engine.run(&mut backend).is_ok());
    assert!(engine.input_failure_active);
    assert_eq!(engine.active_mode(), &ModeId::idle());
    assert_eq!(log.lock().unwrap().clicks, 0);
    assert_eq!(log.lock().unwrap().shutdowns, 1);
}

#[test]
fn leaving_a_scan_owner_cancels_native_work_without_stopping_the_backend() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("idle", Arc::clone(&seen))));
    engine.register(Box::new(ProbeMode::new("ui_hint", seen)));
    engine.registry.active = ModeId::ui_hint();
    engine.scan_owners.insert(41, ModeId::ui_hint());
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .activate(ModeId::idle(), Some(ModeId::ui_hint()), &mut backend)
        .unwrap();

    assert!(engine.scan_owners.is_empty());
    assert_eq!(log.lock().unwrap().cancelled_scans, [41]);
    assert_eq!(log.lock().unwrap().shutdowns, 0);
}

#[test]
fn capture_loss_discards_physical_keys_that_can_no_longer_receive_key_up() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    engine.registry.active = ModeId::normal();
    let key = Key::new("h").unwrap();
    engine.input.pressed.insert(key.clone());
    engine
        .input
        .key_dispositions
        .insert(key, KeyDisposition::Consume);
    engine.scheduler.frame_clock_owner = Some(ModeId::normal());
    let (mut backend, _) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(
            BackendEvent::InputCaptureLost("Accessibility permission removed".into()),
            &mut backend,
        )
        .unwrap();

    assert!(engine.input.pressed.is_empty());
    assert!(engine.input.key_dispositions.is_empty());
    assert!(engine.scheduler.frame_clock_owner.is_none());
    assert_eq!(engine.active_mode(), &ModeId::idle());
}

#[test]
fn finish_cancels_scan_even_when_the_mode_lifecycle_keeps_it_active() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("ui_hint", seen)));
    engine.registry.active = ModeId::ui_hint();
    engine.scan_owners.insert(73, ModeId::ui_hint());
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .execute(
            [Command::FinishMode {
                cause: FinishCause::Explicit,
            }],
            &mut backend,
        )
        .unwrap();

    assert_eq!(engine.active_mode(), &ModeId::ui_hint());
    assert!(engine.scan_owners.is_empty());
    assert_eq!(log.lock().unwrap().cancelled_scans, [73]);
}

#[test]
fn pointer_interest_skips_only_the_mode_event() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mode = ProbeMode::new("normal", Arc::clone(&seen));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(mode));
    engine.registry.active = ModeId::normal();
    engine.screens = vec![Screen {
        bounds: Rect::new(0.0, 0.0, 1_000.0, 800.0),
        work_area: Rect::new(0.0, 0.0, 1_000.0, 800.0),
        is_primary: true,
        scale: 1.0,
        name: None,
    }];
    let (mut backend, _) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(
            BackendEvent::PointerMoved(Point::new(400.0, 300.0)),
            &mut backend,
        )
        .unwrap();

    assert_eq!(engine.cursor, Point::new(400.0, 300.0));
    assert!(!seen.lock().unwrap().iter().any(|event| event == "pointer"));
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn pointer_interest_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn configured_idle_engine() -> Engine {
        let config = Config::default();
        let mut engine = Engine::new(config.clone(), Appearance::Dark);
        engine.register(Box::new(crate::modes::idle::IdleMode::new()));
        engine.registry.active = ModeId::idle();
        engine.screens = vec![Screen {
            bounds: Rect::new(0.0, 0.0, 1_000.0, 800.0),
            work_area: Rect::new(0.0, 0.0, 1_000.0, 800.0),
            is_primary: true,
            scale: 1.0,
            name: None,
        }];
        engine
    }

    fn legacy_pointer(engine: &mut Engine, backend: &mut dyn Backend, reported: Point) {
        let point = engine
            .constrain_absolute_pointer(reported)
            .unwrap_or(engine.cursor);
        let changed = engine.cursor != point;
        engine.cursor = point;
        engine
            .dispatch(ModeEvent::PointerMoved(point), backend)
            .unwrap();
        if changed {
            engine.refresh_overlay_positions(backend).unwrap();
        }
    }

    fn measure(mut operation: impl FnMut(usize)) -> (u128, u128, u128) {
        for index in 0..WARMUP {
            operation(index);
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for index in 0..SAMPLES {
            let started = Instant::now();
            for offset in 0..CALLS_PER_SAMPLE {
                operation(index * CALLS_PER_SAMPLE + offset);
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

    let mut fast = configured_idle_engine();
    let (mut fast_backend, _) = FakeBackend::new(Vec::new());
    let fast_result = measure(|index| {
        let point = Point::new((index % 999) as f64, (index % 799) as f64);
        fast.handle_backend_event(BackendEvent::PointerMoved(point), &mut fast_backend)
            .unwrap();
    });

    let mut legacy = configured_idle_engine();
    let (mut legacy_backend, _) = FakeBackend::new(Vec::new());
    let legacy_result = measure(|index| {
        let point = Point::new((index % 999) as f64, (index % 799) as f64);
        legacy_pointer(&mut legacy, &mut legacy_backend, point);
    });

    println!(
        "pointer_interest_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} fast_p50={}ns fast_p95={}ns fast_p99={}ns legacy_p50={}ns legacy_p95={}ns legacy_p99={}ns",
        fast_result.0,
        fast_result.1,
        fast_result.2,
        legacy_result.0,
        legacy_result.1,
        legacy_result.2,
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn ui_hint_overlap_routing_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn measure(mut operation: impl FnMut() -> bool) -> (u128, u128, u128) {
        for _ in 0..WARMUP {
            std::hint::black_box(operation());
        }
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            for _ in 0..CALLS_PER_SAMPLE {
                std::hint::black_box(operation());
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

    let config = Config::default();
    let key = Key::new("left_shift").unwrap();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    engine.register(Box::new(crate::modes::idle::IdleMode::new()));
    let compiled = measure(|| engine.ui_hint_overlap_matches(&key));
    let parsed = measure(|| config.ui_hint.overlap_cycle_matches(&key));

    println!(
        "ui_hint_overlap_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} compiled_p50={}ns compiled_p95={}ns compiled_p99={}ns parsed_p50={}ns parsed_p95={}ns parsed_p99={}ns",
        compiled.0, compiled.1, compiled.2, parsed.0, parsed.1, parsed.2,
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn click_indicator_inline_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;

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

    let key = Key::new("x").unwrap();
    let inline = measure(|| {
        let mut indicators = ActiveClickIndicators::default();
        indicators.activate(key.clone(), Button::Left);
        std::hint::black_box(indicators.release(&key));
    });
    let heap = measure(|| {
        let mut indicators = Vec::new();
        indicators.push((key.clone(), Button::Left));
        std::hint::black_box(indicators.remove(0));
    });

    let inline_region = Region::new(TEST_ALLOCATOR);
    let mut indicators = ActiveClickIndicators::default();
    indicators.activate(key.clone(), Button::Left);
    indicators.release(&key);
    let inline_allocations = inline_region.change().allocations;
    let heap_region = Region::new(TEST_ALLOCATOR);
    let mut indicators = Vec::new();
    indicators.push((key.clone(), Button::Left));
    indicators.remove(0);
    let heap_allocations = heap_region.change().allocations;

    println!(
        "click_indicator_probe samples={SAMPLES} inline_p50={}ns inline_p95={}ns inline_p99={}ns heap_p50={}ns heap_p95={}ns heap_p99={}ns inline_allocations={inline_allocations} heap_allocations={heap_allocations}",
        inline.0, inline.1, inline.2, heap.0, heap.1, heap.2,
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn binding_ownership_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn direct(binding: &Arc<Binding>) {
        for _ in 0..CALLS_PER_SAMPLE {
            std::hint::black_box(binding.as_ref());
        }
    }

    fn cloned(binding: &Arc<Binding>) {
        for _ in 0..CALLS_PER_SAMPLE {
            drop(std::hint::black_box(Arc::clone(binding)));
        }
    }

    fn percentile(samples: &mut [u128], percent: usize) -> u128 {
        samples.sort_unstable();
        samples[(samples.len() - 1) * percent / 100]
    }

    let binding = Arc::new(Binding::Move(Direction::Left));
    for _ in 0..WARMUP {
        direct(&binding);
        cloned(&binding);
    }

    let mut direct_samples = Vec::with_capacity(SAMPLES);
    let mut cloned_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let measure = |operation: fn(&Arc<Binding>), samples: &mut Vec<u128>| {
            let started = Instant::now();
            operation(&binding);
            samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
        };
        if sample % 2 == 0 {
            measure(direct, &mut direct_samples);
            measure(cloned, &mut cloned_samples);
        } else {
            measure(cloned, &mut cloned_samples);
            measure(direct, &mut direct_samples);
        }
    }

    let direct_p50 = percentile(&mut direct_samples, 50);
    let direct_p95 = percentile(&mut direct_samples, 95);
    let direct_p99 = percentile(&mut direct_samples, 99);
    let cloned_p50 = percentile(&mut cloned_samples, 50);
    let cloned_p95 = percentile(&mut cloned_samples, 95);
    let cloned_p99 = percentile(&mut cloned_samples, 99);
    println!(
        "binding_ownership_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} direct_p50={direct_p50}ns direct_p95={direct_p95}ns direct_p99={direct_p99}ns cloned_p50={cloned_p50}ns cloned_p95={cloned_p95}ns cloned_p99={cloned_p99}ns"
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn native_role_move_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;

    fn make_target(native_role: String, clone_role: bool) -> crate::api::geometry::UiTarget {
        let retained_role = if clone_role {
            native_role.clone()
        } else {
            native_role
        };
        crate::api::geometry::UiTarget {
            rect: Rect::new(0.0, 0.0, 40.0, 20.0),
            name: String::new(),
            role: "button".to_owned(),
            native_role: Some(retained_role),
        }
    }

    fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    for _ in 0..WARMUP {
        drop(std::hint::black_box(make_target("AXButton".into(), false)));
        drop(std::hint::black_box(make_target("AXButton".into(), true)));
    }
    let mut moved_samples = Vec::with_capacity(SAMPLES);
    let mut cloned_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let measure = |clone_role: bool, samples: &mut Vec<u128>| {
            let native_role = String::from("AXButton");
            let started = Instant::now();
            drop(std::hint::black_box(make_target(native_role, clone_role)));
            samples.push(started.elapsed().as_nanos());
        };
        if sample % 2 == 0 {
            measure(false, &mut moved_samples);
            measure(true, &mut cloned_samples);
        } else {
            measure(true, &mut cloned_samples);
            measure(false, &mut moved_samples);
        }
    }

    let native_role = String::from("AXButton");
    let moved_region = Region::new(TEST_ALLOCATOR);
    drop(make_target(native_role, false));
    let moved_allocations = moved_region.change().allocations;
    let native_role = String::from("AXButton");
    let cloned_region = Region::new(TEST_ALLOCATOR);
    drop(make_target(native_role, true));
    let cloned_allocations = cloned_region.change().allocations;
    println!(
        "native_role_probe samples={SAMPLES} moved={:?} cloned={:?} moved_allocations={moved_allocations} cloned_allocations={cloned_allocations}",
        percentiles(&mut moved_samples),
        percentiles(&mut cloned_samples),
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn excluded_app_cache_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    fn measure_pair(
        mut cached: impl FnMut(),
        mut scanned: impl FnMut(),
    ) -> ((u128, u128, u128), (u128, u128, u128)) {
        for _ in 0..WARMUP {
            cached();
            scanned();
        }
        let mut cached_samples = Vec::with_capacity(SAMPLES);
        let mut scanned_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: &mut dyn FnMut(), samples: &mut Vec<u128>| {
                let started = Instant::now();
                for _ in 0..CALLS_PER_SAMPLE {
                    operation();
                }
                samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
            };
            if sample % 2 == 0 {
                measure(&mut cached, &mut cached_samples);
                measure(&mut scanned, &mut scanned_samples);
            } else {
                measure(&mut scanned, &mut scanned_samples);
                measure(&mut cached, &mut cached_samples);
            }
        }
        (
            percentiles(&mut cached_samples),
            percentiles(&mut scanned_samples),
        )
    }

    let app = FocusedApp {
        bundle_id: "com.example.target".into(),
        window_title: String::new(),
        process_id: 1,
    };
    let empty = Config::default();
    let mut one = Config::default();
    one.general.excluded_apps = vec!["com.example.target".into()];
    let mut sixteen = Config::default();
    sixteen.general.excluded_apps = (0..15)
        .map(|index| format!("com.example.other{index}"))
        .chain([app.bundle_id.clone()])
        .collect();

    let compare = |config: &Config, cached_value: bool| {
        measure_pair(
            || {
                std::hint::black_box(cached_value);
            },
            || {
                std::hint::black_box(
                    config
                        .general
                        .excluded_apps
                        .iter()
                        .any(|entry| entry.eq_ignore_ascii_case(&app.bundle_id)),
                );
            },
        )
    };
    let empty_result = compare(&empty, false);
    let one_result = compare(&one, true);
    let sixteen_result = compare(&sixteen, true);
    println!(
        "excluded_app_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} empty_cached={:?} empty_scanned={:?} one_cached={:?} one_scanned={:?} sixteen_cached={:?} sixteen_scanned={:?}",
        empty_result.0,
        empty_result.1,
        one_result.0,
        one_result.1,
        sixteen_result.0,
        sixteen_result.1,
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn mode_registry_lookup_baseline_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    let ids = [
        ModeId::idle(),
        ModeId::normal(),
        ModeId::grid(),
        ModeId::recursive_grid(),
        ModeId::ui_hint(),
    ];
    let tree: BTreeMap<ModeId, usize> = ids
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let slots = [0_usize, 1, 2, 3, 4];
    let active = ModeId::ui_hint();
    let tree_lookup = || {
        std::hint::black_box(
            std::hint::black_box(&tree)
                .get(std::hint::black_box(&active))
                .copied()
                .unwrap(),
        )
    };
    let slot_lookup =
        || std::hint::black_box(std::hint::black_box(&slots)[std::hint::black_box(4)]);
    for _ in 0..WARMUP {
        tree_lookup();
        slot_lookup();
    }
    let mut tree_samples = Vec::with_capacity(SAMPLES);
    let mut slot_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let measure = |operation: &dyn Fn() -> usize, samples: &mut Vec<u128>| {
            let started = Instant::now();
            for _ in 0..CALLS_PER_SAMPLE {
                std::hint::black_box(operation());
            }
            samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
        };
        if sample % 2 == 0 {
            measure(&tree_lookup, &mut tree_samples);
            measure(&slot_lookup, &mut slot_samples);
        } else {
            measure(&slot_lookup, &mut slot_samples);
            measure(&tree_lookup, &mut tree_samples);
        }
    }
    println!(
        "mode_registry_lookup_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} tree={:?} slot={:?}",
        percentiles(&mut tree_samples),
        percentiles(&mut slot_samples),
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn mode_registry_dispatch_performance_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    let config = Config::default();
    let mut registry = ModeRegistry::default();
    for mode in crate::app::mode_catalog::built_in(&config) {
        let id = mode.id();
        let pointer_events = mode.wants_pointer_events();
        registry.insert(id, mode, pointer_events);
    }
    let active = ModeId::ui_hint();
    let active_slot = registry.index_of(&active).unwrap();
    let mut slot_samples = Vec::with_capacity(SAMPLES);
    let mut tree_samples = Vec::with_capacity(SAMPLES);

    let slot = |registry: &mut ModeRegistry| {
        std::hint::black_box(
            registry
                .get_index_mut(active_slot)
                .unwrap()
                .captures_keyboard(),
        );
    };
    let tree = |registry: &mut ModeRegistry| {
        std::hint::black_box(registry.get_mut(&active).unwrap().captures_keyboard());
    };
    for _ in 0..WARMUP {
        slot(&mut registry);
        tree(&mut registry);
    }
    for sample in 0..SAMPLES {
        let mut measure = |operation: &dyn Fn(&mut ModeRegistry), samples: &mut Vec<u128>| {
            let started = Instant::now();
            for _ in 0..CALLS_PER_SAMPLE {
                operation(&mut registry);
            }
            samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
        };
        if sample % 2 == 0 {
            measure(&slot, &mut slot_samples);
            measure(&tree, &mut tree_samples);
        } else {
            measure(&tree, &mut tree_samples);
            measure(&slot, &mut slot_samples);
        }
    }
    println!(
        "mode_registry_dispatch_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} slot={:?} tree={:?}",
        percentiles(&mut slot_samples),
        percentiles(&mut tree_samples),
    );
}

#[test]
#[ignore = "microbenchmark probe; run in release with --test-threads=1"]
fn compiled_route_slot_baseline_probe() {
    const WARMUP: usize = 2_000;
    const SAMPLES: usize = 20_000;
    const CALLS_PER_SAMPLE: usize = 100;

    fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
        samples.sort_unstable();
        let last = samples.len() - 1;
        (
            samples[last * 50 / 100],
            samples[last * 95 / 100],
            samples[last * 99 / 100],
        )
    }

    let config = Config::default();
    let build = || {
        CompiledKeymap::compile(
            vec![("a".into(), Binding::Move(Direction::Left))],
            config.resolved_key_aliases(),
        )
    };
    let mode = ModeId::normal();
    let external = BTreeMap::from([(mode.clone(), build())]);
    let slot = [build()];
    let key = Key::new("a").unwrap();
    let pressed = PressedKeys::default();
    let external_lookup = || {
        std::hint::black_box(
            external
                .get(&mode)
                .unwrap()
                .lookup_with_specificity(&key, &pressed),
        );
    };
    let slot_lookup = || {
        std::hint::black_box(slot[0].lookup_with_specificity(&key, &pressed));
    };
    for _ in 0..WARMUP {
        external_lookup();
        slot_lookup();
    }
    let mut external_samples = Vec::with_capacity(SAMPLES);
    let mut slot_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let measure = |operation: &dyn Fn(), samples: &mut Vec<u128>| {
            let started = Instant::now();
            for _ in 0..CALLS_PER_SAMPLE {
                operation();
            }
            samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
        };
        if sample % 2 == 0 {
            measure(&slot_lookup, &mut slot_samples);
            measure(&external_lookup, &mut external_samples);
        } else {
            measure(&external_lookup, &mut external_samples);
            measure(&slot_lookup, &mut slot_samples);
        }
    }
    println!(
        "compiled_route_slot_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} slot={:?} external={:?}",
        percentiles(&mut slot_samples),
        percentiles(&mut external_samples),
    );
}

