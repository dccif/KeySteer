#[test]
fn native_event_loop_keeps_grid_selection_across_dispatch_boundaries() {
    for mode in ["grid", "recursive_grid"] {
        let mut config = blind_config("grid", "[]");
        config.hotkeys.insert("f6".into(), Binding::parse(mode).unwrap());
        let mut engine = Engine::from_plan(
            crate::app::configuration::compile(&config).unwrap(), Appearance::Dark,
        ).unwrap();
        let (mut backend, log) = FakeBackend::new(Vec::new());
        backend.native_batches = Some(vec![
            Ok(tap_chord("f6")), Ok(tap_chord("a")),
            Ok(Vec::new()), Ok(tap_chord("a")),
        ]);
        engine.run(&mut backend).unwrap();
        assert_eq!(engine.active_mode().as_str(), mode);
        let log = log.lock().unwrap();
        assert_eq!(log.warps.last(), Some(&Point::new(125.0, 100.0)), "{mode} must keep its first selection");
        assert_eq!(log.timeline.iter().filter(|&&event| event == "native-yield").count(), 4);
        assert_eq!(log.shutdowns, 1);
    }
}

#[test]
fn native_event_loop_keeps_frame_movement_and_click_pairing_alive() {
    let config = Config::default();
    let mut engine = Engine::from_plan(
        crate::app::configuration::compile(&config).unwrap(), Appearance::Dark,
    ).unwrap();
    let (mut backend, log) = FakeBackend::new(Vec::new());
    backend.native_batches = Some(vec![
        Ok(enter_normal()), Ok(vec![key_down("h")]),
        Ok(vec![BackendEvent::Frame(Duration::from_millis(20))]),
        Ok(Vec::new()),
        Ok(vec![BackendEvent::Frame(Duration::from_millis(20)), key_up("h")]),
        Ok(vec![key_down(";")]), Ok(Vec::new()), Ok(vec![key_up(";")]),
    ]);
    engine.run(&mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert!(!engine.input_failure_active);
    let log = log.lock().unwrap();
    assert!(log.moves.iter().filter(|(dx, _)| *dx < 0.0).count() >= 2);
    assert_eq!(log.buttons, [
        (MouseButton::Left, ButtonAction::Press),
        (MouseButton::Left, ButtonAction::Release),
    ]);
}

#[test]
fn native_event_loop_error_still_releases_held_input_and_shuts_down() {
    let mut engine = engine_with_normal_binding("x", "left_click");
    let (mut backend, log) = FakeBackend::new(Vec::new());
    backend.native_batches = Some(vec![
        Ok(enter_normal()), Ok(vec![key_down("x")]), Err("native driver failed".into()),
    ]);
    assert!(engine.run(&mut backend).unwrap_err().contains("native driver failed"));
    let log = log.lock().unwrap();
    assert_eq!(log.buttons, [
        (MouseButton::Left, ButtonAction::Press),
        (MouseButton::Left, ButtonAction::Release),
    ]);
    assert_eq!(log.shutdowns, 1);
}

#[test]
fn native_event_loop_services_due_timers_and_async_scan_results() {
    struct Scanner(Arc<Mutex<Vec<&'static str>>>);
    impl Mode for Scanner {
        fn id(&self) -> ModeId { ModeId::idle() }
        fn handle(&mut self, event: &ModeEvent, _: &HostContext<'_>) -> CommandBatch {
            match event {
                ModeEvent::Activated { .. } => CommandBatch::two(
                    Command::SetTimer { id: "native-timer".into(), delay: Duration::ZERO, repeating: false },
                    Command::scan_ui(crate::api::UiScanRequest {
                        id: 77, scope: crate::api::UiScanScope::Active,
                        timeout_ms: 2_500, bounds: None, roles: Vec::new(), max_depth: 0,
                        visible_only: false, clickable_only: false,
                        strategy: crate::api::UiScanStrategy::AxTree,
                        vision: crate::api::VisionOptions::default(), app: None,
                    }),
                ),
                ModeEvent::Timer { .. } => { self.0.lock().unwrap().push("timer"); CommandBatch::new() }
                ModeEvent::UiScanned(_) => { self.0.lock().unwrap().push("scan"); CommandBatch::new() }
                _ => CommandBatch::new(),
            }
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(Scanner(Arc::clone(&seen))));
    let (mut backend, log) = FakeBackend::new(Vec::new());
    backend.native_batches = Some(vec![
        Ok(Vec::new()),
        Ok(vec![BackendEvent::UiScanned(crate::api::UiScanResult {
            id: 77, targets: Vec::new(), retired: Vec::new(), status: UiScanStatus::Success,
        })]),
    ]);
    engine.run(&mut backend).unwrap();
    assert_eq!(*seen.lock().unwrap(), ["timer", "scan"]);
    assert_eq!(log.lock().unwrap().shutdowns, 1);
}

#[test]
fn zero_delay_work_yields_to_input_between_dispatches() {
    struct Preparation { seen: Arc<Mutex<Vec<&'static str>>>, remaining: usize }
    impl Mode for Preparation {
        fn id(&self) -> ModeId { ModeId::idle() }
        fn handle(&mut self, event: &ModeEvent, _: &HostContext<'_>) -> CommandBatch {
            let schedule = || CommandBatch::one(Command::SetTimer {
                id: "cooperative-prewarm".into(), delay: Duration::ZERO, repeating: false,
            });
            match event {
                ModeEvent::Activated { .. } => schedule(),
                ModeEvent::Timer { .. } => {
                    self.seen.lock().unwrap().push("prepare");
                    self.remaining -= 1;
                    if self.remaining > 0 { schedule() } else { CommandBatch::new() }
                }
                ModeEvent::Key { state: KeyState::Down, .. } => {
                    self.seen.lock().unwrap().push("input");
                    CommandBatch::new()
                }
                _ => CommandBatch::new(),
            }
        }
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(Config::default(), Appearance::Dark);
    engine.register(Box::new(Preparation { seen: Arc::clone(&seen), remaining: 3 }));
    let (mut backend, log) = FakeBackend::new(Vec::new());
    backend.native_batches = Some(vec![Ok(Vec::new()), Ok(vec![key_down("a")]), Ok(Vec::new())]);
    engine.run(&mut backend).unwrap();
    assert_eq!(*seen.lock().unwrap(), ["prepare", "input", "prepare", "prepare"]);
    assert_eq!(log.lock().unwrap().shutdowns, 1);
}
