#[test]
fn switching_modes_drops_the_previous_modes_timers() {
    struct Ticker(ModeId);
    impl Mode for Ticker {
        fn id(&self) -> ModeId {
            self.0.clone()
        }
        fn handle(&mut self, event: &ModeEvent, _c: &HostContext<'_>) -> CommandBatch {
            CommandBatch::from(match event {
                ModeEvent::Activated { .. } => vec![Command::SetTimer {
                    id: "tick".into(),
                    delay: Duration::from_millis(1),
                    repeating: true,
                }],
                _ => Vec::new(),
            })
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_probes(&seen, &[]);
    // Replace idle with a mode that arms a timer on entry.
    engine.register(Box::new(Ticker(ModeId::idle())));

    let (mut backend, _) = FakeBackend::new(enter_normal());
    engine.run(&mut backend).unwrap();

    assert!(
        engine.scheduler.timers.is_empty(),
        "idle's timer outlived idle"
    );
}

#[test]
fn scan_requests_fall_back_to_configured_roles() {
    struct Scanner(ModeId);
    impl Mode for Scanner {
        fn id(&self) -> ModeId {
            self.0.clone()
        }
        fn handle(&mut self, event: &ModeEvent, _c: &HostContext<'_>) -> CommandBatch {
            CommandBatch::from(match event {
                ModeEvent::Activated { .. } => vec![Command::scan_ui(UiScanRequest {
                    id: 1,
                    timeout_ms: 2_500,
                    bounds: None,
                    roles: Vec::new(),
                    max_depth: 0,
                    visible_only: false,
                    clickable_only: false,
                    strategy: crate::api::command::UiScanStrategy::AxTree,
                    vision: crate::api::command::VisionOptions::default(),
                    app: None,
                })],
                _ => Vec::new(),
            })
        }
    }

    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(Scanner(ModeId::idle())));
    let (mut backend, log) = FakeBackend::new(vec![]);
    engine.run(&mut backend).unwrap();
    assert_eq!(log.lock().unwrap().scans, 1);
}

#[test]
fn focus_changes_rebuild_only_when_the_effective_binding_profile_changes() {
    let mut config = Config::default();
    config.normal.app_configs = vec![
        crate::config::AppOverride {
            bundle_id: "com.example.editor".into(),
            bindings: Bindings::from([("j".into(), Binding::parse("move_left").unwrap())]),
        },
        crate::config::AppOverride {
            bundle_id: "Writer".into(),
            bindings: Bindings::from([("j".into(), Binding::parse("move_left").unwrap())]),
        },
        crate::config::AppOverride {
            bundle_id: "Admin".into(),
            bindings: Bindings::from([("j".into(), Binding::parse("move_right").unwrap())]),
        },
    ];
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(config, Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("idle", seen.clone())));
    engine.register(Box::new(ProbeMode::new("normal", seen.clone())));
    let (mut backend, _) = FakeBackend::new(Vec::new());
    let initial_rebuilds = engine.registry.table_rebuild_count;
    let app = |bundle_id: &str, title: &str, process_id| FocusedApp {
        bundle_id: bundle_id.into(),
        window_title: title.into(),
        process_id,
    };
    fn focus(engine: &mut Engine, backend: &mut FakeBackend, app: FocusedApp) {
        engine
            .handle_backend_event(BackendEvent::FocusChanged(Some(app)), backend)
            .unwrap();
    }

    focus(
        &mut engine,
        &mut backend,
        app("com.example.browser", "Home", 1),
    );
    focus(
        &mut engine,
        &mut backend,
        app("com.example.browser", "Article", 1),
    );
    focus(
        &mut engine,
        &mut backend,
        app("com.example.mail", "Inbox", 2),
    );
    assert_eq!(
        engine.registry.table_rebuild_count, initial_rebuilds,
        "identity and title changes with the same empty profile must reuse tables"
    );

    focus(
        &mut engine,
        &mut backend,
        app("com.example.editor", "Document", 3),
    );
    assert_eq!(engine.registry.table_rebuild_count, initial_rebuilds + 1);
    focus(
        &mut engine,
        &mut backend,
        app("COM.EXAMPLE.EDITOR", "Another document", 4),
    );
    assert_eq!(
        engine.registry.table_rebuild_count,
        initial_rebuilds + 1,
        "different processes matching the same override must reuse tables"
    );
    let writer = app("com.example.browser", "Writer document", 5);
    assert_eq!(
        engine.binding_profile_key_for(Some(&writer)),
        engine.registry.binding_profile_key,
        "different matching entries with the same patch must produce the same profile"
    );
    focus(&mut engine, &mut backend, writer);
    assert_eq!(
        engine.registry.table_rebuild_count,
        initial_rebuilds + 1,
        "different overrides resolving to the same bindings must reuse tables"
    );

    focus(
        &mut engine,
        &mut backend,
        app("com.example.browser", "Admin console", 5),
    );
    assert_eq!(engine.registry.table_rebuild_count, initial_rebuilds + 2);
    focus(
        &mut engine,
        &mut backend,
        app("com.example.browser", "Public console", 5),
    );
    assert_eq!(
        engine.registry.table_rebuild_count,
        initial_rebuilds + 3,
        "a title change that changes the resolved override must rebuild"
    );
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|event| event.ends_with(":focus"))
            .count(),
        8,
        "all focus events must still reach the active mode"
    );
}

#[test]
fn bootstrap_registration_compiles_binding_tables_once() {
    let config = Config::default();
    let modes = crate::app::mode_catalog::built_in(&config);
    let plugins = crate::app::mode_catalog::bundled_plugins(&config).unwrap();
    let mut engine = Engine::new(config, Appearance::Dark);

    for mode in modes {
        engine.register_deferred(mode);
    }
    for plugin in plugins {
        engine.register_plugin_dyn_deferred(plugin).unwrap();
    }
    assert_eq!(engine.registry.table_rebuild_count, 0);

    engine.rebuild_tables();
    assert_eq!(engine.registry.table_rebuild_count, 1);
    assert_eq!(
        engine.registry.table_count(),
        engine
            .binding_mode_ids()
            .into_iter()
            .filter(|id| engine.registry.contains_key(id))
            .count()
    );
}

#[test]
fn excluded_apps_forward_every_key() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config::default();
    config.general.excluded_apps = vec!["com.example.game".into()];
    let mut engine = Engine::new(config, Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("idle", seen.clone())));
    engine.register(Box::new(ProbeMode::new("normal", seen.clone())));

    let mut script = vec![BackendEvent::FocusChanged(Some(FocusedApp {
        bundle_id: "com.example.game".into(),
        window_title: String::new(),
        process_id: 1,
    }))];
    script.extend(enter_normal());
    let (mut backend, log) = FakeBackend::new(script);
    engine.run(&mut backend).unwrap();

    assert_eq!(engine.active_mode().as_str(), "idle");
    assert!(engine.is_excluded_app());
    assert!(
        log.lock()
            .unwrap()
            .dispositions
            .iter()
            .all(|d| *d == KeyDisposition::Forward)
    );
}

#[test]
fn a_release_is_delivered_even_though_the_chord_no_longer_matches() {
    // Regression: resolving the release by looking the chord up again fails
    // once the keys are up, which left movement running forever.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = engine_with_normal_probes(&seen, "alt+l", "move_right");

    run_in_normal(
        &mut engine,
        vec![
            key_down("left_alt"),
            key_down("l"),
            // Alt goes up first, so `alt+l` cannot match any more.
            key_up("left_alt"),
            key_up("l"),
        ],
    );

    let log = seen.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|e| e.contains("binding(move_right)"))
            .count(),
        2,
        "the release must still arrive: {log:?}"
    );
    assert!(engine.input.active_gestures.is_empty());
}

#[test]
fn leaving_a_mode_releases_gestures_still_held() {
    // Otherwise the pointer would keep moving after the mode changed.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config::default();
    config.normal.bindings.clear();
    config
        .normal
        .bindings
        .insert("l".into(), Binding::parse("move_right").unwrap());
    config.normal.bindings.insert("esc".into(), Binding::Escape);

    let mut engine = Engine::new(config, Appearance::Dark);
    let mut idle = ProbeMode::new("idle", seen.clone());
    idle.captures = false;
    engine.register(Box::new(idle));
    engine.register(Box::new(ProbeMode::new("normal", seen.clone())));

    // Hold `l`, then escape out of the mode without releasing it.
    run_in_normal(&mut engine, vec![key_down("l"), key_down("esc")]);

    let log = seen.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|e| e.contains("binding(move_right)"))
            .count(),
        2,
        "switching modes should release the gesture: {log:?}"
    );
    assert!(engine.input.active_gestures.is_empty());
    assert_eq!(engine.active_mode().as_str(), "idle");
}

#[test]
fn an_unavailable_keyboard_is_reported_once_at_startup() {
    // Every mode depends on the keyboard, so a silent failure here looks
    // like the whole program is broken for no reason.
    struct Deaf {
        inner: FakeBackend,
    }
    impl Backend for Deaf {
        fn poll(&mut self, t: Duration) -> Result<Option<BackendEvent>, String> {
            self.inner.poll(t)
        }
        fn dispose_key(&mut self, d: KeyDisposition) -> Result<(), String> {
            self.inner.dispose_key(d)
        }
        fn screens(&self) -> Result<Vec<Screen>, String> {
            self.inner.screens()
        }
        fn pointer(&self) -> Result<Point, String> {
            self.inner.pointer()
        }
        fn focused_app(&self) -> Result<Option<FocusedApp>, String> {
            self.inner.focused_app()
        }
        fn warp_pointer(&self, p: Point) -> Result<(), String> {
            self.inner.warp_pointer(p)
        }
        fn move_pointer(&self, from: Point, x: f64, y: f64) -> Result<(), String> {
            self.inner.move_pointer(from, x, y)
        }
        fn mouse_button(&self, b: MouseButton, a: ButtonAction) -> Result<(), String> {
            self.inner.mouse_button(b, a)
        }
        fn scroll(&self, x: f64, y: f64) -> Result<(), String> {
            self.inner.scroll(x, y)
        }
        fn send_key(&self, k: &Key, s: KeyState) -> Result<(), String> {
            self.inner.send_key(k, s)
        }
        fn present(&mut self, s: Arc<OverlayScene>) -> Result<(), String> {
            self.inner.present(s)
        }
        fn dismiss(&mut self) -> Result<(), String> {
            self.inner.dismiss()
        }
        fn request_ui_scan(&mut self, request: crate::api::UiScanRequest) -> Result<(), String> {
            self.inner.request_ui_scan(request)
        }
        fn name(&self) -> &'static str {
            "deaf"
        }
        fn keyboard_available(&self) -> bool {
            false
        }
        fn keyboard_unavailable_reason(&self) -> Option<String> {
            Some("no permission".into())
        }
    }

    let (inner, _) = FakeBackend::new(vec![]);
    let mut backend = Deaf { inner };
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(ProbeMode::new(
        "idle",
        Arc::new(Mutex::new(vec![])),
    )));

    // Must not fail: the program still controls the pointer.
    engine.run(&mut backend).unwrap();
    assert!(!backend.keyboard_available());
}

#[test]
fn a_working_backend_reports_the_keyboard_as_available() {
    let (backend, _) = FakeBackend::new(vec![]);
    assert!(backend.keyboard_available());
    assert_eq!(backend.keyboard_unavailable_reason(), None);
}

#[test]
fn automatic_reload_discovers_files_created_after_startup() {
    let directory = std::env::temp_dir().join(format!(
        "keysteer-runtime-reload-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();

    let defaults = Config::default();
    let default_write_path = directory.join("keysteer.user.toml");
    let store = ConfigStore::open(
        &default_write_path,
        &defaults,
        crate::platform::atomic_replace,
    )
    .unwrap();
    let source = defaults.to_toml().unwrap();
    let mut engine = Engine::new(defaults.clone(), Appearance::Dark);
    attach_test_repository(
        &mut engine,
        defaults,
        Some(store),
        Some(directory.clone()),
        source,
    );

    let discovered_path = directory.join("keysteer.created-later.toml");
    std::fs::write(&discovered_path, "[pointer]\ninitial_speed = 321\n").unwrap();
    let (mut backend, _) = FakeBackend::new(vec![]);
    engine.reload_config(&mut backend).unwrap();

    let active = Config::parse(
        &engine
            .configuration
            .as_ref()
            .unwrap()
            .source_text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active.pointer.initial_speed, 321.0);
    assert_eq!(engine.configuration_source_path().unwrap(), discovered_path);

    std::fs::remove_file(&discovered_path).unwrap();
    engine.reload_config(&mut backend).unwrap();
    let active = Config::parse(
        &engine
            .configuration
            .as_ref()
            .unwrap()
            .source_text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        active.pointer.initial_speed,
        Config::default().pointer.initial_speed
    );
    assert_eq!(
        engine.configuration_source_path().unwrap(),
        default_write_path
    );

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn explicit_config_reload_remains_pinned_to_its_path() {
    let directory = std::env::temp_dir().join(format!(
        "keysteer-runtime-explicit-reload-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();

    let explicit_path = directory.join("keysteer.explicit.toml");
    std::fs::write(&explicit_path, "[pointer]\ninitial_speed = 200\n").unwrap();
    let config = Config::load(&explicit_path).unwrap();
    let store =
        ConfigStore::open(&explicit_path, &config, crate::platform::atomic_replace).unwrap();
    let source = config.to_toml().unwrap();
    let mut engine = Engine::new(config.clone(), Appearance::Dark);
    attach_test_repository(&mut engine, config, Some(store), None, source);

    std::fs::write(
        directory.join("keysteer.aaa.toml"),
        "[pointer]\ninitial_speed = 999\n",
    )
    .unwrap();
    std::fs::write(&explicit_path, "[pointer]\ninitial_speed = 456\n[hotkeys]\nf8 = \"normal\"\n").unwrap();
    let (mut backend, _) = FakeBackend::new(vec![]);
    engine.handle_backend_event(BackendEvent::ReloadConfig, &mut backend).unwrap();

    let active = Config::parse(
        &engine
            .configuration
            .as_ref()
            .unwrap()
            .source_text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active.pointer.initial_speed, 456.0);
    assert_eq!(engine.configuration_source_path().unwrap(), explicit_path);
    assert!(engine.registry.routes.contains_key(&ModeId::idle()));
    assert!(engine.registry.routes.contains_key(&ModeId::normal()));
    for event in [key_down("f8"), key_up("f8")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::normal());

    // Repeated status-menu reloads replace both mode bindings and plugin verbs.
    for speed in ["slow_toggle", "fast_toggle"] {
        std::fs::write(&explicit_path, format!(
            "[hotkeys]\nf8 = \"normal\"\n[normal.bindings]\nshift = \"{speed}\"\n\"primary+x\" = \"move_window next\"\n"
        )).unwrap();
        engine.handle_backend_event(BackendEvent::ReloadConfig, &mut backend).unwrap();
        assert_eq!(engine.active_mode(), &ModeId::idle());
        assert_eq!(engine.overlay.speed_toggle, None);
        assert_eq!(engine.bindings_in(&ModeId::normal()).len(), 2);
        for event in [key_down("f8"), key_up("f8"), key_down("left_shift"), key_up("left_shift")] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        let expected = if speed == "slow_toggle" { crate::api::binding::Speed::Slow } else { crate::api::binding::Speed::Fast };
        assert_eq!(engine.overlay.speed_toggle, Some(expected));
        for event in [key_down("left_shift"), key_up("left_shift")] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        assert_eq!(engine.overlay.speed_toggle, None);
    }

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runtime_reload_cancels_pending_mouse_only_after_candidate_validation() {
    let directory = std::env::temp_dir().join(format!(
        "keysteer-runtime-pending-reload-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();

    let path = directory.join("keysteer.user.toml");
    let mut engine = engine_with_normal_action("x", Binding::Click(Button::Left));
    let mut config = active_config(&engine);
    std::fs::write(&path, config.to_toml().unwrap()).unwrap();
    let store = ConfigStore::open(&path, &config, crate::platform::atomic_replace).unwrap();
    let source = config.to_toml().unwrap();
    attach_test_repository(&mut engine, config.clone(), Some(store), None, source);
    engine.registry.active = ModeId::normal();
    let (mut backend, log) = FakeBackend::new(Vec::new());

    engine
        .handle_backend_event(key_down("x"), &mut backend)
        .unwrap();
    std::fs::write(&path, "not valid toml = [").unwrap();
    assert!(engine.reload_config(&mut backend).is_err());
    assert_eq!(engine.input.pending_long_press_toggles.len(), 1);
    assert_eq!(
        log.lock().unwrap().buttons,
        [(MouseButton::Left, ButtonAction::Press)]
    );

    config.pointer.initial_speed = 456.0;
    std::fs::write(&path, config.to_toml().unwrap()).unwrap();
    engine.reload_config(&mut backend).unwrap();
    assert!(engine.input.pending_long_press_toggles.is_empty());
    assert!(engine.input.latched.is_empty());
    let active = Config::parse(
        &engine
            .configuration
            .as_ref()
            .unwrap()
            .source_text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active.pointer.initial_speed, 456.0);
    assert_eq!(
        log.lock().unwrap().buttons,
        [
            (MouseButton::Left, ButtonAction::Press),
            (MouseButton::Left, ButtonAction::Release),
        ]
    );

    engine
        .handle_backend_event(key_up("x"), &mut backend)
        .unwrap();
    assert_eq!(log.lock().unwrap().buttons.len(), 2);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn an_idle_binding_to_an_unregistered_plugin_mode_is_ignored() {
    // A namespaced id parses fine but may not be registered; the engine
    // must not switch to a mode that does not exist.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config::default();
    config
        .hotkeys
        .insert("alt+z".into(), Binding::parse("plugin:missing").unwrap());
    let mut engine = Engine::new(config, Appearance::Dark);
    engine.register(Box::new(ProbeMode::new("idle", seen.clone())));

    let (mut backend, _) = FakeBackend::new(vec![key_down("left_alt"), key_down("z")]);
    engine.run(&mut backend).unwrap();
    assert_eq!(engine.active_mode().as_str(), "idle");
}
