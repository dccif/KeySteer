fn window_test_engine(config: &Config) -> (Engine, FakeBackend, Arc<Mutex<Recorder>>) {
    let mut engine = Engine::from_plan(crate::app::configuration::compile(config).unwrap(), Appearance::Dark).unwrap();
    let (backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.set_active(ModeId::normal());
    engine.rebuild_tables();
    (engine, backend, log)
}

fn enter_window(engine: &mut Engine, backend: &mut FakeBackend, log: &Arc<Mutex<Recorder>>) -> crate::api::window::WindowRequest {
    for event in [key_down("left_alt"), key_down("w"), key_up("w"), key_up("left_alt")] {
        engine.handle_backend_event(event, backend).unwrap();
    }
    {
        let log = log.lock().unwrap();
        assert_eq!(engine.active_mode(), &ModeId::window(), "requests={:?} dispositions={:?} pressed={:?}", log.window_requests, log.dispositions, engine.input.pressed);
    }
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { closed: Vec::new(),
        session: request.session, id: request.id, target: Some(crate::api::window::WindowInfo {
            id: crate::api::window::WindowId(77), title: "Test window".into(), app: "test".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0,
            resizable: true, maximized: false, fullscreen: false,
        }), windows: Some(vec![crate::api::window::WindowInfo {
            id: crate::api::window::WindowId(77), title: "Test window".into(), app: "test".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0,
            resizable: true, maximized: false, fullscreen: false,
        }]), pointer: None, changed: 0, skipped: 0, message: None, edit: None,
    })), backend).unwrap();
    request
}

#[test]
fn window_defaults_use_only_d_and_never_require_enter() {
    use crate::api::window::{WindowOperation as O, WindowChange as C};
    let config = Config::default();
    assert!(!config.window.bindings.contains_key("enter"));
    assert!(!config.window.bindings.contains_key("shift+d"));
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    let initial = log.lock().unwrap().window_requests.len();
    for event in [key_down("enter"), key_up("enter")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::window());
    assert_eq!(log.lock().unwrap().window_requests.len(), initial);
    for event in [key_down("d"), key_up("d")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::Adjust { change: C::Screen(crate::api::command::WindowScreenTarget::Next), .. })));
    for event in [key_down("a"), key_up("a")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for event in [key_down("h"), key_up("h")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::ApplyLayout { .. })));
    for event in [key_down("esc"), key_up("esc")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(engine.active_mode(), &ModeId::window());
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit: true, .. })));
    assert!(!log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit: false, .. })));
}

#[test]
fn window_mode_locks_target_and_uses_independent_arrows_and_size() {
    use crate::api::window::{WindowOperation as O, WindowChange as C};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(BackendEvent::PointerMoved(Point::new(900.0, 700.0)), &mut backend).unwrap();
    for event in [key_down("right"), key_up("right"), key_down("s"), key_up("s"), key_down("up"), key_up("up")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    let log = log.lock().unwrap();
    assert!(log.window_requests.iter().any(|r| matches!(r.operation, O::Adjust { target: crate::api::window::WindowId(77), change: C::Move { dx, dy: 0.0 }, .. } if dx > 0.0)));
    assert!(log.window_requests.iter().any(|r| matches!(r.operation, O::Adjust { target: crate::api::window::WindowId(77), change: C::Resize { dw: 0.0, dh }, .. } if dh > 0.0)));
    assert!(log.moves.is_empty());
    assert!(log.scenes.iter().any(|s| s.labels.iter().any(|l| l.text.starts_with("Window · size"))));
}

#[test]
fn window_temporary_normal_uses_custom_movement_and_preserves_size_state() {
    let mut config = Config::default();
    config.normal.bindings.insert("a".into(), Binding::Move(Direction::Left));
    let primary = config.resolved_key_aliases()["primary"].clone();
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("s"), key_up("s"), key_down(&primary), key_down("a"), key_up("a"), key_up(&primary)] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::window());
    let log = log.lock().unwrap();
    assert!(log.moves.iter().any(|(dx, _)| *dx < 0.0));
    assert!(!log.window_requests.iter().any(|r| matches!(r.operation,
        crate::api::window::WindowOperation::Adjust { .. } | crate::api::window::WindowOperation::Tile { .. })),
        "temporary Normal must not resize or lay out windows");
    assert!(log.scenes.last().unwrap().labels.iter().any(|l| l.text.starts_with("Window · size")));
}

#[test]
fn window_single_a_previews_and_double_a_tiles_once_ignoring_repeat() {
    use crate::api::window::WindowOperation as O;
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("a"), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().window_requests.len(), 2);
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    let mut repeat = key_down("a");
    if let BackendEvent::Input(input) = &mut repeat { input.repeat = true; }
    engine.handle_backend_event(repeat, &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().window_requests.len(), 2);
    for event in [key_up("a"), key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(log.lock().unwrap().window_requests.iter().filter(|r| matches!(r.operation, O::Tile { .. })).count(), 1);
}

#[test]
fn layout_q_keeps_adjustments_before_returning_to_preserved_grid() {
    use crate::api::window::WindowOperation as O;
    let config = Config::default();
    let (mut engine, mut backend, log) = window_test_engine(&config);
    let seen = Arc::new(Mutex::new(Vec::new()));
    engine.register(Box::new(ProbeMode::new("grid", seen.clone())));
    engine.set_active(ModeId::grid());
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a"), key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::window());
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit: true, .. })));
    assert_eq!(engine.active_mode(), &ModeId::grid());
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|e| e == "grid:resumed"));
    assert!(!seen.iter().any(|e| e == "grid:restarted" || e == "grid:deactivated"));
}

#[test]
fn window_exit_cancels_and_late_results_cannot_warp_pointer() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    let request = enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("q"), &mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert!(log.lock().unwrap().cancelled_window_sessions.contains(&request.session));
    let before = log.lock().unwrap().warps.len();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { closed: Vec::new(),
        session: request.session, id: request.id + 1, target: None, windows: None,
        pointer: Some(Point::new(700.0, 500.0)), changed: 1, skipped: 0, message: None, edit: None,
    })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().warps.len(), before);
}

#[test]
fn window_rebinding_and_undo_use_actual_configured_actions() {
    let mut config = Config::default();
    config.window.bindings.insert("x".into(), Binding::Window(crate::api::window::WindowAction::Undo));
    config.window.bindings.insert("z".into(), Binding::Disabled);
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("z"), key_up("z"), key_down("x"), key_up("x")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(log.lock().unwrap().window_requests.iter().filter(|r| matches!(r.operation, crate::api::window::WindowOperation::Undo)).count(), 1);
    let entries = engine.key_help_entries();
    assert!(entries.iter().any(|e| e.starts_with("x ") && e.contains("window_undo")));
    assert!(!entries.iter().any(|e| e.starts_with("z ")));
}

#[test]
fn window_request_is_submitted_after_key_disposition() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    let timeline = &log.lock().unwrap().timeline;
    let index = timeline.iter().position(|e| *e == "window").unwrap();
    assert_eq!(timeline[index - 1], "dispose");
}

#[test]
fn window_help_follows_target_not_pointer_and_contains_quick_layout_in_one_panel() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    let panel = |scene: &OverlayScene| scene.labels.iter().find(|l| l.text.is_empty()
        && l.z_index == i32::MAX - 1).expect("one rounded help background").rect;
    let initial = log.lock().unwrap().scenes.last().unwrap().clone();
    assert!(initial.indicator.is_none());
    assert!((panel(&initial).y - 410.0).abs() < 1.0);
    engine.handle_backend_event(BackendEvent::PointerMoved(Point::new(950.0, 750.0)), &mut backend).unwrap();
    assert_eq!(panel(log.lock().unwrap().scenes.last().unwrap()), panel(&initial));
    for event in [key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let scene = log.lock().unwrap().scenes.last().unwrap().clone();
    let bounds = panel(&scene);
    assert!(bounds.x >= 0.0 && bounds.y >= 0.0 && bounds.right() <= 1000.0 && bounds.bottom() <= 800.0);
    assert_eq!(scene.labels.iter().filter(|l| l.text.is_empty() && l.z_index == i32::MAX - 1).count(), 1);
    assert!(!scene.labels.iter().any(|l| l.text == "1 · Full"));
    assert!(scene.labels.iter().any(|l| l.text.contains("Quick")));
    assert!(scene.labels.iter().any(|l| l.text == "Layout left"));
    for event in [key_down("h"), key_up("h")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Left 1/2")));
}

// Runtime tests acknowledge the worker boundary; geometry and native rollback
// are exercised separately against Session and platform probes.
fn acknowledge_window_edit(engine: &mut Engine, backend: &mut FakeBackend, log: &Arc<Mutex<Recorder>>) {
    use crate::api::window::{WindowEditResult as E, WindowOperation as O, WindowInfo, WindowId, WindowResult};
    let mut acknowledged = 0;
    for _ in 0..20 {
        let request = log.lock().unwrap().window_requests.last().unwrap().clone();
        if request.id == acknowledged { break; }
        acknowledged = request.id;
        let mut target = WindowInfo { id: WindowId(77), title: "Test window".into(), app: "test".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0,
            resizable: true, maximized: false, fullscreen: false };
        let edit = match request.operation {
            O::BeginEdit { transaction, targets, screen, .. } => E::Started { transaction, minimums: if screen.is_some() { vec![(target.id, Point::new(100.0, 80.0))] } else { targets.iter().map(|id| (*id, Point::new(100.0, 80.0))).collect() }, gap_scale: 1.0, full_inventory: screen.is_some() },
            O::ApplyLayout { transaction, revision, ref placements, gap, .. } => {
                if let Some((_, rect)) = placements.iter().find(|(id, _)| *id == target.id) {
                    target.bounds = crate::api::window_layout::placed_rect(engine.screens[0].work_area, *rect, gap);
                }
                E::Applied { transaction, revision, accepted: true, minimums: Vec::new() }
            }
            O::EndEdit { transaction, commit } => E::Ended { transaction, committed: commit },
            _ => break,
        };
        engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { closed: Vec::new(),
            session: request.session, id: request.id, target: Some(target.clone()), windows: Some(vec![target]),
            pointer: None, changed: 0, skipped: 0, message: None, edit: Some(Box::new(edit)),
        })), backend).unwrap();
    }
}

#[test]
fn window_direction_language_uses_effective_wasd_and_modifiers_without_pointer_motion() {
    use crate::api::window::WindowOperation as O;
    let mut config = Config::default();
    for (key, direction) in [("a", Direction::Left), ("s", Direction::Down), ("w", Direction::Up), ("d", Direction::Right)] {
        config.normal.bindings.insert(key.into(), Binding::Move(direction));
    }
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    // W is a different action, so the AA entrance deadline is cancelled.
    for key in ["w", "a"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
    }
    let requests = log.lock().unwrap().window_requests.clone();
    assert!(requests.iter().any(|r| matches!(&r.operation, O::ApplyLayout { placements, strict: false, .. } if placements[0].1 == Rect::new(0.0, 0.0, 0.5, 0.5))));
    assert!(log.lock().unwrap().moves.is_empty(), "layout directions must not reach Normal's pointer movement");
    for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for event in [key_down("left_shift"), key_down("d"), key_up("d"), key_up("left_shift")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text == "`2"));
    for event in [key_down("left_ctrl"), key_down("d"), key_up("d"), key_up("left_ctrl")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let requests = log.lock().unwrap().window_requests.clone();
    assert!(requests.iter().any(|r| matches!(&r.operation, O::ApplyLayout { placements, strict: true, .. } if (placements[0].1.width - 2.0 / 3.0).abs() < 1e-6)));
}

#[test]
fn window_number_deadline_only_exists_for_a_live_ambiguous_prefix() {
    use crate::api::window::{WindowOperation as O, WindowResult, WindowInfo, WindowId};
    for count in [1, 9, 10, 20, 23, 30] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        let request = enter_window(&mut engine, &mut backend, &log);
        let windows: Vec<_> = (1..=count).map(|id| WindowInfo { id: WindowId(id), title: format!("Window {id}"), app: "test".into(),
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0), screen: 0, resizable: true, maximized: false, fullscreen: false }).collect();
        // Supply the inventory on a real outstanding enumerate request.
        engine.dispatch_to(&ModeId::window(), ModeEvent::Timer { id: "window_inventory".into(), elapsed: Duration::from_millis(500) }, &mut backend).unwrap();
        let id = log.lock().unwrap().window_requests.last().unwrap().id;
        engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { closed: Vec::new(),
            session: request.session, id, target: Some(windows[0].clone()), windows: Some(windows), pointer: None,
            changed: 0, skipped: 0, message: None, edit: None,
        })), &mut backend).unwrap();
        // The original fixture already owns number 1; it is deliberately not
        // recycled. Current windows receive 2..=count+1.
        for event in [key_down("2"), key_up("2")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        let ambiguous = count + 1 >= 20;
        assert_eq!(engine.scheduler.timers.keys().any(|id| id.contains("window_number")), ambiguous, "count={count}");
        if !ambiguous { assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::Select(WindowId(1)))); }
    }
}

#[test]
fn window_help_shows_rebound_keys_and_retains_result_status_after_key_up() {
    let mut config = Config::default();
    config.window.bindings.insert("z".into(), Binding::Disabled);
    config.window.bindings.insert("x".into(), Binding::Window(crate::api::window::WindowAction::Undo));
    let (mut engine, mut backend, log) = window_test_engine(&config);
    let request = enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("x"), &mut backend).unwrap();
    let target = crate::api::window::WindowInfo { id: crate::api::window::WindowId(77), title: "Test window".into(), app: "test".into(),
        bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { closed: Vec::new(),
        session: request.session, id: request.id + 1, target: Some(target), windows: None,
        pointer: None, changed: 0, skipped: 0, message: Some("Nothing to undo".into()), edit: None,
    })), &mut backend).unwrap();
    engine.handle_backend_event(key_up("x"), &mut backend).unwrap();
    let log = log.lock().unwrap();
    let labels = &log.scenes.last().unwrap().labels;
    assert!(labels.iter().any(|l| l.text == "Nothing to undo"));
    assert!(labels.iter().any(|l| l.text == "X"));
    assert!(!labels.iter().any(|l| l.text == "Z"));
}

#[test]
fn window_tab_cycles_immediately_and_centers_pointer_without_confirmation() {
    use crate::api::window::{WindowOperation, WindowResult, WindowId, WindowInfo};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    let session = enter_window(&mut engine, &mut backend, &log).session;
    for event in [key_down("s"), key_up("s"), key_down("tab"), key_up("tab")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    assert!(matches!(request.operation, WindowOperation::Cycle));
    let target = WindowInfo { id: WindowId(88), title: "Next window".into(), app: "test".into(),
        bounds: Rect::new(500.0, 200.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { closed: Vec::new(),
        session, id: request.id, pointer: Some(target.bounds.center()), target: Some(target),
        windows: None, changed: 0, skipped: 0, message: None, edit: None,
    })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().warps.last(), Some(&Point::new(700.0, 350.0)));
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Next window")), "Tab shows the locked target in the help panel");
    engine.handle_backend_event(key_down("right"), &mut backend).unwrap();
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation,
        WindowOperation::Adjust { target: WindowId(88), change: crate::api::window::WindowChange::Resize { .. }, .. }));
}

#[test]
fn temporary_normal_cancels_pending_window_motion_and_ignores_its_late_pointer() {
    let config = Config::default();
    let primary = &config.window.temporary_mode_keys[0];
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("right"), &mut backend).unwrap();
    let pending = log.lock().unwrap().window_requests.last().unwrap().clone();
    engine.handle_backend_event(key_down(primary), &mut backend).unwrap();
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation,
        crate::api::window::WindowOperation::CancelPending));
    let before = log.lock().unwrap().warps.len();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { closed: Vec::new(),
        session: pending.session, id: pending.id, target: None, windows: None,
        pointer: Some(Point::new(1.0, 2.0)), changed: 1, skipped: 0, message: None, edit: None,
    })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().warps.len(), before);
    engine.handle_backend_event(key_up(primary), &mut backend).unwrap();
    engine.handle_backend_event(key_up("right"), &mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::window());
}

#[test]
fn closing_target_stops_window_motion_and_waits_for_explicit_tab() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("right"), &mut backend).unwrap();
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    let before = log.lock().unwrap().window_requests.len();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { closed: Vec::new(),
        session: request.session, id: request.id, target: None, windows: None,
        pointer: None, changed: 0, skipped: 0, message: Some("window was closed".into()), edit: None,
    })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().window_requests.len(), before + 1);
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation,
        crate::api::window::WindowOperation::CancelPending));
    assert_eq!(log.lock().unwrap().frame_clock_states.last(), Some(&false));
}

#[test]
fn entering_window_stops_previous_normal_motion_without_restarting_it_on_return() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    engine.handle_backend_event(key_down("h"), &mut backend).unwrap();
    assert!(!engine.input.active_gestures.is_empty());
    enter_window(&mut engine, &mut backend, &log);
    assert!(engine.input.active_gestures.is_empty());
    engine.handle_backend_event(key_down("q"), &mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::normal());
    assert_ne!(engine.scheduler.frame_clock_owner, Some(ModeId::normal()));
    engine.handle_backend_event(key_up("h"), &mut backend).unwrap();
}

#[test]
fn unchanged_window_inventory_does_not_redraw_and_refresh_is_coalesced() {
    use crate::api::window::{WindowResult, WindowInfo, WindowId};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    let initial = enter_window(&mut engine, &mut backend, &log);
    let before = log.lock().unwrap().scenes.len();
    let requests = log.lock().unwrap().window_requests.len();
    for _ in 0..3 {
        engine.dispatch_to(&ModeId::window(), ModeEvent::Timer { id: "window_inventory".into(), elapsed: Duration::from_millis(500) }, &mut backend).unwrap();
    }
    assert_eq!(log.lock().unwrap().window_requests.len(), requests + 1);
    let id = log.lock().unwrap().window_requests.last().unwrap().id;
    let target = WindowInfo { id: WindowId(77), title: "Test window".into(), app: "test".into(), bounds: Rect::new(100.0,100.0,400.0,300.0), screen: 0, resizable: true, maximized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { closed: Vec::new(), session: initial.session, id, target: Some(target.clone()), windows: Some(vec![target]), pointer: None, changed:0, skipped:0, message:None, edit:None })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().scenes.len(), before);
}

#[test]
fn quick_input_coalesces_then_undo_restores_entry_transaction() {
    use crate::api::window::WindowOperation as O;
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for key in ["h", "h", "h"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    }
    assert_eq!(log.lock().unwrap().window_requests.iter().filter(|r| matches!(r.operation, O::ApplyLayout { .. })).count(), 1);
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    let requests = log.lock().unwrap().window_requests.clone();
    let placements: Vec<_> = requests.iter().filter_map(|r| match &r.operation { O::ApplyLayout { placements, .. } => Some(placements[0].1.width), _ => None }).collect();
    assert_eq!(placements, vec![0.5, 0.25]);
    for _ in 0..3 {
        for event in [key_down("z"), key_up("z")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
    }
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit:false, .. })));
    assert_eq!(engine.active_mode(), &ModeId::window());
}
