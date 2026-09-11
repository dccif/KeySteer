#[test]
fn window_number_and_configurable_cycle_keys_include_group_members_only_in_window_mode() {
    use crate::api::window::{WindowAction as W, WindowOperation as O, WindowInfo, WindowId, WindowResult};
    use crate::api::window_tabs::{TabState, TabGroup, TabGroupId};
    for custom in [false, true] {
        let mut config = Config::default();
        if custom {
            config.window.bindings.remove("tab");
            config.window.bindings.remove("shift+tab");
            config.window.bindings.insert("n".into(), Binding::Window(W::Select));
            config.window.bindings.insert("p".into(), Binding::Window(W::SelectPrevious));
        }
        let (mut engine, mut backend, log) = window_test_engine(&config);
        enter_window(&mut engine, &mut backend, &log);
        engine.dispatch_to(&ModeId::window(), ModeEvent::Timer { id: "window_inventory".into(), elapsed: Duration::from_millis(500) }, &mut backend).unwrap();
        let request = log.lock().unwrap().window_requests.last().unwrap().clone();
        let windows: Vec<_> = [77, 88, 99].into_iter().map(|id| WindowInfo {
            id: WindowId(id), title: format!("Window {id}"), app: "test".into(), bounds: Rect::new(100.0, 100.0, 400.0, 300.0),
            screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false,
        }).collect();
        engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult {
            tabs: Some(TabState { groups: vec![TabGroup { id: TabGroupId(1), members: vec![WindowId(77), WindowId(88)], active: WindowId(77) }],
                numbers: vec![(WindowId(77), 1), (WindowId(88), 2), (WindowId(99), 3)], target: None, active: Some(WindowId(77)) }),
            session: request.session, id: request.id, target: Some(windows[0].clone()), windows: Some(windows),
            pointer: None, closed: Vec::new(), changed: 0, skipped: 0, message: None, edit: None,
        })), &mut backend).unwrap();
        for event in [key_down("2"), key_up("2")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::Select(WindowId(88))));
        let next = if custom { "n" } else { "tab" };
        for event in [key_down(next), key_up(next)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::Cycle));
        if !custom { engine.handle_backend_event(key_down("left_shift"), &mut backend).unwrap(); }
        let previous = if custom { "p" } else { "tab" };
        for event in [key_down(previous), key_up(previous)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        if !custom { engine.handle_backend_event(key_up("left_shift"), &mut backend).unwrap(); }
        let actual = log.lock().unwrap().window_requests.last().unwrap().operation.clone();
        assert!(matches!(actual, O::CyclePrevious), "custom={custom} actual={actual:?}");
        for event in [key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &ModeId::idle());
        let count = log.lock().unwrap().window_requests.len();
        for event in [key_down("tab"), key_up("tab"), key_down("2"), key_up("2")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(log.lock().unwrap().window_requests.len(), count);
    }
}

#[test]
fn window_help_shows_restore_and_complete_descriptions_from_effective_bindings() {
    for rebound in [false, true] {
        let mut config = Config::default();
        if rebound {
            config.window.bindings.insert("r".into(), Binding::Disabled);
            config.window.bindings.insert("v".into(), Binding::Mode(ModeId::window_restore()));
        }
        let (mut engine, mut backend, log) = window_test_engine(&config);
        enter_window(&mut engine, &mut backend, &log);
        {
            let recorded = log.lock().unwrap();
            let labels = &recorded.scenes.last().unwrap().labels;
            for text in ["Restore", "Edit", "Maximize / minimize / restore", if rebound { "V" } else { "R" }] {
                assert!(labels.iter().map(|label| label.text.as_str()).collect::<Vec<_>>().join(" ").contains(text), "missing {text}");
            }
            assert!(!labels.iter().any(|label| label.text == "Save layout"));
            if rebound { assert!(!labels.iter().any(|label| label.text == "R")); }
        }
        for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.text == "Save layout"));
    }
}

#[test]
fn saved_layout_pages_live_inside_one_bottom_panel_at_each_dpi() {
    use crate::api::overlay::scaled_label_geometry;
    use crate::api::window_presets::RegionTemplate;
    for scale in [1.0, 1.5, 2.0] {
        let config = Config::default();
        let (mut engine, mut backend, log) = window_test_engine(&config);
        engine.screens[0].scale = scale;
        engine.screens[0].bounds = Rect::new(0.0, 0.0, 1920.0 * scale, 1080.0 * scale);
        engine.screens[0].work_area = engine.screens[0].bounds;
        for n in 1..=7 { engine.window_presets.store.save(RegionTemplate::Slot { id: 1 }.into(), 1, format!("Saved layout {n}")).unwrap(); }
        enter_window(&mut engine, &mut backend, &log);
        for key in ["r", "page_down"] {
            for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            let recorded = log.lock().unwrap();
            let scene = recorded.scenes.last().unwrap();
            let panels: Vec<_> = scene.labels.iter().filter(|l| l.z_index == i32::MAX - 1).collect();
            assert_eq!(panels.len(), 1);
            let panel = panels[0].rect;
            assert!((panel.bottom() - (engine.screens[0].work_area.bottom() - 12.0 * scale)).abs() < 1.0);
            assert!(panel.y > engine.screens[0].work_area.height / 2.0);
            for label in &scene.labels {
                let rect = scaled_label_geometry(&label.text, label.rect, &label.style, scale).0;
                assert!(rect.x >= panel.x - 1.0 && rect.right() <= panel.right() + 1.0 && rect.y >= panel.y - 1.0 && rect.bottom() <= panel.bottom() + 1.0, "{label:?} outside {panel:?}");
            }
            let expected = if key == "r" { "Next page" } else { "Previous page" };
            let hidden = if key == "r" { "Previous page" } else { "Next page" };
            assert!(scene.labels.iter().any(|label| label.text == expected));
            assert!(!scene.labels.iter().any(|label| label.text == hidden));
            assert_eq!(scene.labels.iter().filter(|label| label.text.contains("Saved layout ")).count(), if key == "r" { 6 } else { 1 });
        }
    }
}

#[test]
fn window_card_text_stays_inside_its_background_at_each_dpi_without_font_shrinking() {
    use crate::api::overlay::scaled_label_geometry;
    for scale in [1.0, 1.5, 2.0] {
        let config = Config::default();
        let (mut engine, mut backend, log) = window_test_engine(&config);
        engine.screens[0].scale = scale;
        enter_window(&mut engine, &mut backend, &log);
        let log = log.lock().unwrap();
        let scene = log.scenes.last().unwrap();
        let physical = |label: &crate::api::overlay::OverlayLabel| {
            scaled_label_geometry(&label.text, label.rect, &label.style, scale).0
        };
        let card = physical(scene.labels.iter().find(|l| l.z_index == 19).unwrap());
        for label in scene.labels.iter().filter(|l| (20..=21).contains(&l.z_index)) {
            let rect = physical(label);
            assert!(rect.x >= card.x - 1.0 && rect.right() <= card.right() + 1.0, "{label:?} outside {card:?}");
            assert!(rect.y >= card.y - 1.0 && rect.bottom() <= card.bottom() + 1.0);
        }
        let app = scene.labels.iter().find(|l| l.z_index == 21 && l.text == "test").unwrap();
        assert!((app.style.font_size - (f64::from(config.window.ui.font_size) * 0.6).max(14.0)).abs() < 0.001);
        let key = scene.labels.iter().find(|l| l.z_index == i32::MAX && l.text == "A").unwrap();
        assert_eq!(key.style.font_size, config.key_help.font_size);
    }
}

#[test]
fn window_root_e_and_independent_rebound_mode_destinations() {
    use crate::api::window::WindowOperation as O;
    let mut config = Config::default();
    config.window.bindings.insert("q".into(), Binding::Disabled);
    config.window.bindings.insert("v".into(), Binding::Mode(ModeId::idle()));
    config.window_editor.bindings.insert("q".into(), Binding::Disabled);
    config.window_editor.bindings.insert("v".into(), Binding::Mode(ModeId::window()));
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    assert!(engine.key_help_entries().iter().any(|e| e.starts_with("e ") && e.contains("window_editor")));
    for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::ApplyLayout { .. })));
    for event in [key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::window_editor());
    for event in [key_down("v"), key_up("v")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(engine.active_mode(), &ModeId::window());
    for event in [key_down("v"), key_up("v")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::idle());
}

#[test]
fn window_saved_layout_note_forwards_input_then_r_one_restores_the_saved_regions() {
    let mut config = Config::default();
    config.window.temporary_mode_keys = vec!["ctrl".into()];
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a"), key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); acknowledge_window_edit(&mut engine, &mut backend, &log); }
    for event in [key_down("left_ctrl"), key_down("s"), key_up("s"), key_up("left_ctrl")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let prompt = log.lock().unwrap().text_prompts.last().expect("Ctrl+S owns its chord even with Ctrl temporary mode").clone();
    assert!(prompt.bounds.bottom() <= engine.screens[0].work_area.bottom());
    assert!(prompt.bounds.y > engine.screens[0].work_area.center().y);
    assert!(!log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.z_index == i32::MAX - 1));
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text == "`1"));
    let before = log.lock().unwrap().window_requests.len();
    for event in [key_down("x"), key_up("x"), key_down("esc"), key_up("esc")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(log.lock().unwrap().window_requests.len(), before, "typing notes must not edit the layout");
    assert!(log.lock().unwrap().dispositions.iter().rev().take(4).all(|d| *d == KeyDisposition::Forward));
    engine.handle_backend_event(BackendEvent::TextPromptResult { id: prompt.id, value: Ok(Some("工作 · 写代码".into())) }, &mut backend).unwrap();
    assert!(engine.window_presets.pending.is_none());
    for event in [key_down("r"), key_up("r")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("工作 · 写代码")));
    let before = log.lock().unwrap().window_requests.len();
    for event in [key_down("1"), key_up("1")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().window_requests[before..].iter().any(|r| matches!(r.operation, crate::api::window::WindowOperation::ApplyLayout { .. })));
}

#[test]
fn leaving_window_cancels_note_and_ignores_its_late_completion() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a"), key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); acknowledge_window_edit(&mut engine, &mut backend, &log); }
    for event in [key_down("left_ctrl"), key_down("s"), key_up("s"), key_up("left_ctrl")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let id = log.lock().unwrap().text_prompts.last().unwrap().id;
    engine.handle_backend_event(BackendEvent::ToggleEnabled, &mut backend).unwrap();
    assert!(log.lock().unwrap().cancelled_text_prompts.contains(&id));
    engine.handle_backend_event(BackendEvent::TextPromptResult { id, value: Ok(Some("stale".into())) }, &mut backend).unwrap();
    assert!(engine.window_presets.store.list().unwrap().is_empty());
}

#[test]
fn reopening_r_reads_layouts_replaced_outside_the_program() {
    use crate::api::window_presets::RegionTemplate;
    let path = std::env::temp_dir().join(format!("keysteer-r-refresh-{}-{}.ksw", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let mut external = crate::app::preset_store::PresetStore::persistent(path.clone(), crate::platform::atomic_replace);
    external.save(RegionTemplate::Slot { id: 1 }, 1, "Original layout".into()).unwrap();
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    engine.attach_preset_store(Box::new(crate::app::preset_store::PresetStore::persistent(path.clone(), crate::platform::atomic_replace))); enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("r"), key_up("r")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Original layout")));
    for event in [key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    external.save(RegionTemplate::Slot { id: 1 }, 1, "Browser edited layout".into()).unwrap();
    for event in [key_down("r"), key_up("r")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Browser edited layout")));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn restoring_while_quick_start_is_pending_keeps_the_saved_template_for_tree_start() {
    use crate::api::window_presets::RegionTemplate as T;
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    engine.window_presets.store.save(T::Split { axis: crate::api::window_layout::Axis::X, ratio: 0.25, first: Box::new(T::Slot { id: 1 }), second: Box::new(T::Slot { id: 2 }) }.into(), 1, "Quarter".into()).unwrap();
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a"), key_down("r"), key_up("r")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::window_quick());
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(engine.active_mode(), &ModeId::window_restore());
    for event in [key_down("1"), key_up("1")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(&r.operation, crate::api::window::WindowOperation::ApplyLayout { placements, .. } if placements.iter().any(|(id, bounds)| *id == crate::api::window::WindowId(77) && (bounds.width - 0.25).abs() < 0.001))));
}

fn window_test_engine(config: &Config) -> (Engine, FakeBackend, Arc<Mutex<Recorder>>) {
    let mut engine = Engine::from_plan(crate::app::configuration::compile(config).unwrap(), Appearance::Dark).unwrap();
    engine.attach_preset_store(Box::new(crate::app::preset_store::PresetStore::default()));
    let (backend, log) = FakeBackend::new(Vec::new());
    engine.screens = backend.screens().unwrap();
    engine.set_active(ModeId::normal());
    engine.rebuild_tables();
    (engine, backend, log)
}

#[test]
fn entering_tab_consumes_t_once_and_shifted_prefix_targets_group_identity() {
    use crate::api::window::{WindowId, WindowInfo, WindowOperation, WindowResult};
    use crate::api::window_tabs::{TabGroup, TabGroupId, TabOperation, TabState, WindowTarget};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("t"), key_up("t")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::window_tab());
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    assert!(matches!(request.operation, WindowOperation::Tabs(TabOperation::Enter { .. })));
    let windows: Vec<_> = [77, 88].into_iter().map(|id| WindowInfo { id: WindowId(id), app: "test".into(), title: format!("Window {id}"), bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false }).collect();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult {
        session: request.session, id: request.id, target: Some(windows[0].clone()), windows: Some(windows), pointer: None, changed: 1, skipped: 0, message: None, edit: None, closed: Vec::new(),
        tabs: Some(TabState { groups: vec![TabGroup { id: TabGroupId(1), members: vec![WindowId(77), WindowId(88)], active: WindowId(77) }], numbers: vec![(WindowId(77), 1), (WindowId(88), 2)], target: None, active: Some(WindowId(77)) }),
    })), &mut backend).unwrap();
    assert!(!log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, WindowOperation::Tabs(TabOperation::EndGroup))));
    for event in [key_down("left_shift"), character_down("`", '~'), key_up("`"), key_up("left_shift"), key_down("1"), key_up("1")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, WindowOperation::Tabs(TabOperation::Choose(WindowTarget::Group(TabGroupId(1)))))));
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
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { tabs: None, closed: Vec::new(),
        session: request.session, id: request.id, target: Some(crate::api::window::WindowInfo {
            id: crate::api::window::WindowId(77), title: "Test window".into(), app: "test".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0,
            resizable: true, maximized: false, minimized: false, fullscreen: false,
        }), windows: Some(vec![crate::api::window::WindowInfo {
            id: crate::api::window::WindowId(77), title: "Test window".into(), app: "test".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0,
            resizable: true, maximized: false, minimized: false, fullscreen: false,
        }]), pointer: None, changed: 0, skipped: 0, message: None, edit: None,
    })), backend).unwrap();
    request
}

#[test]
fn window_all_screen_editor_submits_separate_display_layouts() {
    use crate::api::window::{WindowEditResult, WindowId, WindowInfo, WindowOperation, WindowResult};
    let mut config = Config::default();
    config.window_editor.screens = crate::config::WindowScreens::All;
    let (mut engine, mut backend, log) = window_test_engine(&config);
    let mut second = engine.screens[0].clone();
    second.bounds.x += second.bounds.width;
    second.work_area.x += second.bounds.width;
    second.is_primary = false;
    engine.screens.push(second);
    let acquired = enter_window(&mut engine, &mut backend, &log);
    assert_eq!(acquired.scope.unwrap().screen, Some(0));
    assert!(!acquired.scope.unwrap().include_minimized);
    for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    assert_eq!(request.scope.unwrap().screen, None);
    let WindowOperation::BeginEdit { transaction, .. } = request.operation else { panic!("missing edit request") };
    let windows: Vec<_> = [(77, 0), (88, 1)].into_iter().map(|(id, screen)| WindowInfo {
        id: WindowId(id), title: format!("Window {id}"), app: "test".into(),
        bounds: Rect::new(engine.screens[screen].work_area.x + 100.0, 100.0, 400.0, 300.0), screen,
        resizable: true, maximized: false, minimized: false, fullscreen: false,
    }).collect();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult {
        tabs: None, closed: Vec::new(), session: request.session, id: request.id,
        target: Some(windows[0].clone()), windows: Some(windows), pointer: None,
        changed: 0, skipped: 0, message: None,
        edit: Some(Box::new(WindowEditResult::Started { transaction,
            minimums: vec![(WindowId(77), Point::new(100.0, 80.0)), (WindowId(88), Point::new(100.0, 80.0))],
            gap_scale: 1.0, full_inventory: true })),
    })), &mut backend).unwrap();
    let log = log.lock().unwrap();
    let WindowOperation::ApplyLayout { screen, placements, additional_screens, .. } = &log.window_requests.last().unwrap().operation else { panic!("missing layout") };
    assert_eq!(*screen, 0);
    assert_eq!(placements.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [WindowId(77)]);
    assert_eq!(additional_screens.len(), 1);
    assert_eq!(additional_screens[0].screen, 1);
    assert_eq!(additional_screens[0].placements.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [WindowId(88)]);
}

#[test]
fn coincident_window_cards_avoid_the_actual_bottom_help_panel() {
    use crate::api::window::{WindowId, WindowInfo, WindowResult};
    for scale in [1.0, 1.5, 2.0] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        engine.screens[0].scale = scale;
        engine.screens[0].bounds.width *= scale;
        engine.screens[0].bounds.height *= scale;
        engine.screens[0].work_area.width *= scale;
        engine.screens[0].work_area.height *= scale;
        let request = enter_window(&mut engine, &mut backend, &log);
        engine.dispatch_to(&ModeId::window(), ModeEvent::Timer { id: "window_inventory".into(), elapsed: Duration::from_millis(500) }, &mut backend).unwrap();
        let refresh = log.lock().unwrap().window_requests.last().unwrap().clone();
        let work = engine.screens[0].work_area;
        let windows: Vec<_> = [77, 88].into_iter().map(|id| WindowInfo {
            id: WindowId(id), title: "Overlapping Explorer".into(), app: "explorer".into(),
            bounds: Rect::new(work.center().x - 300.0, work.bottom() - 300.0, 600.0, 300.0),
            screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false,
        }).collect();
        engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult {
            tabs: None, closed: Vec::new(), session: request.session, id: refresh.id,
            target: Some(windows[0].clone()), windows: Some(windows), pointer: None,
            changed: 0, skipped: 0, message: None, edit: None,
        })), &mut backend).unwrap();
        let log = log.lock().unwrap();
        let scene = log.scenes.last().unwrap();
        let panel = scene.labels.iter().find(|l| l.z_index == i32::MAX - 1).unwrap().rect;
        let cards: Vec<_> = scene.labels.iter().filter(|l| l.z_index == 19).map(|l| l.rect).collect();
        assert_eq!(cards.len(), 2);
        for label in scene.labels.iter().filter(|l| matches!(l.z_index, 20 | 21)) {
            assert!(cards.iter().any(|card| card.contains(&label.rect.center())), "text detached: {} {:?}", label.text, label.rect);
            assert!(!panel.contains(&label.rect.center()), "text overlaps help: {}", label.text);
        }
        assert!(cards[0].intersect(&cards[1]).is_none(), "scale={scale}: {cards:?}");
        assert!(cards.iter().all(|card| card.intersect(&panel).is_none()), "scale={scale}: {panel:?} {cards:?}");
    }
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
    for event in [key_down("q"), key_up("q")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(engine.active_mode(), &ModeId::window());
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit: true, .. })));
    assert!(!log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit: false, .. })));
}

#[test]
fn window_mode_locks_target_and_uses_normal_movement_and_size() {
    use crate::api::window::{WindowOperation as O, WindowChange as C};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(BackendEvent::PointerMoved(Point::new(900.0, 700.0)), &mut backend).unwrap();
    for event in [key_down("l"), key_up("l"), key_down("s"), key_up("s"), key_down("k"), key_up("k")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    let log = log.lock().unwrap();
    assert!(log.window_requests.iter().any(|r| matches!(r.operation, O::Adjust { target: crate::api::window::WindowId(77), change: C::Move { dx, dy: 0.0 }, .. } if dx > 0.0)));
    assert!(log.window_requests.iter().any(|r| matches!(r.operation, O::Adjust { target: crate::api::window::WindowId(77), change: C::Resize { dw: 0.0, dh }, .. } if dh > 0.0)));
    assert!(log.moves.is_empty());
    assert!(log.scenes.iter().any(|s| s.labels.iter().any(|l| l.text == "Resize" && l.style.border_width > 0.0)));
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
    assert!(log.scenes.last().unwrap().labels.iter().any(|l| l.text == "Resize" && l.style.border_width > 0.0));
}

#[test]
fn window_a_opens_quick_without_any_double_tap_tiling() {
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
    assert_eq!(log.lock().unwrap().window_requests.iter().filter(|r| matches!(r.operation, O::Tile { .. })).count(), 0);
}

#[test]
fn layout_q_waits_for_commit_then_uses_ordinary_mode_binding() {
    use crate::api::window::WindowOperation as O;
    let config = Config::default();
    let (mut engine, mut backend, log) = window_test_engine(&config);
    let seen = Arc::new(Mutex::new(Vec::new()));
    engine.register(Box::new(ProbeMode::new("grid", seen.clone())));
    engine.set_active(ModeId::grid());
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a"), key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::window_quick());
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::EndEdit { commit: true, .. })));
    assert_eq!(engine.active_mode(), &ModeId::window());
    for event in [key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::idle());
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|e| e == "grid:deactivated"));
    assert!(!seen.iter().any(|e| e == "grid:resumed"));
}

#[test]
fn window_exit_cancels_and_late_results_cannot_warp_pointer() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    let request = enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("q"), &mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::idle());
    assert!(log.lock().unwrap().cancelled_window_sessions.contains(&request.session));
    let before = log.lock().unwrap().warps.len();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { tabs: None, closed: Vec::new(),
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
    let bounds = panel(&initial);
    let work = engine.screens[0].work_area;
    assert!(bounds.x >= work.x && bounds.y >= work.y && bounds.right() <= work.right() && bounds.bottom() <= work.bottom());
    for label in initial.labels.iter().filter(|l| l.z_index == i32::MAX && l.text == "A") { assert_eq!(label.style.font_size, 12.0); }
    assert!(!initial.labels.iter().any(|l| l.text.contains("Select window number")));
    assert!(initial.labels.iter().any(|l| l.text == "1" && l.style.font_size == 28.0));
    engine.handle_backend_event(BackendEvent::PointerMoved(Point::new(950.0, 750.0)), &mut backend).unwrap();
    assert_eq!(panel(log.lock().unwrap().scenes.last().unwrap()), panel(&initial));
    for event in [key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    let scene = log.lock().unwrap().scenes.last().unwrap().clone();
    let bounds = panel(&scene);
    assert!(bounds.x >= 0.0 && bounds.y >= 0.0 && bounds.right() <= 1000.0 && bounds.bottom() <= 800.0);
    assert_eq!(scene.labels.iter().filter(|l| l.text.is_empty() && l.z_index == i32::MAX - 1).count(), 1);
    assert!(!scene.labels.iter().any(|l| l.text == "1 · Full"));
    assert!(scene.labels.iter().any(|l| l.text.contains("Quick")));
    assert!(scene.labels.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ").contains("Layout ←↓↑→"));
    for event in [key_down("h"), key_up("h")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Left 1/2")));
}

// Runtime tests acknowledge the worker boundary; geometry and native rollback
// are exercised separately against Session and platform probes.
fn acknowledge_window_edit(engine: &mut Engine, backend: &mut FakeBackend, log: &Arc<Mutex<Recorder>>) {
    acknowledge_window_edit_with_acceptance(engine, backend, log, true);
}
fn acknowledge_window_edit_with_acceptance(engine: &mut Engine, backend: &mut FakeBackend, log: &Arc<Mutex<Recorder>>, accepted: bool) {
    use crate::api::window::{WindowEditResult as E, WindowOperation as O, WindowInfo, WindowId, WindowResult};
    let mut acknowledged = 0;
    for _ in 0..20 {
        let request = log.lock().unwrap().window_requests.last().unwrap().clone();
        if request.id == acknowledged { break; }
        acknowledged = request.id;
        let mut target = WindowInfo { id: WindowId(77), title: "Test window".into(), app: "test".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0,
            resizable: true, maximized: false, minimized: false, fullscreen: false };
        let edit = match request.operation {
            O::BeginEdit { transaction, targets, screen, .. } => E::Started { transaction, minimums: if screen.is_some() { vec![(target.id, Point::new(100.0, 80.0))] } else { targets.iter().map(|id| (*id, Point::new(100.0, 80.0))).collect() }, gap_scale: 1.0, full_inventory: screen.is_some() },
            O::ApplyLayout { transaction, revision, ref placements, gap, .. } => {
                if let Some((_, rect)) = placements.iter().find(|(id, _)| *id == target.id) {
                    target.bounds = crate::api::window_layout::placed_rect(engine.screens[0].work_area, *rect, gap);
                }
                E::Applied { transaction, revision, accepted, minimums: Vec::new() }
            }
            O::EndEdit { transaction, commit } => E::Ended { transaction, committed: commit },
            _ => break,
        };
        engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { tabs: None, closed: Vec::new(),
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
        config.window_quick.bindings.insert(key.into(), Binding::Window(crate::api::window::WindowAction::Navigate(direction)));
        config.window_editor.bindings.insert(key.into(), Binding::Window(crate::api::window::WindowAction::Navigate(direction)));
        config.window_editor.bindings.insert(format!("shift+{key}"), Binding::Window(crate::api::window::WindowAction::Split(direction)));
        config.window_editor.bindings.insert(format!("ctrl+{key}"), Binding::Window(crate::api::window::WindowAction::Ratio(direction)));
    }
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    // All layout directions apply immediately, including a rebound A.
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
    assert!(requests.iter().any(|r| matches!(&r.operation, O::ApplyLayout { placements, strict: true, .. } if (placements[0].1.width - (0.5 + config.window.resize_step / engine.screens[0].work_area.width)).abs() < 1e-6)));
}

#[test]
fn window_number_deadline_only_exists_for_a_live_ambiguous_prefix() {
    use crate::api::window::{WindowOperation as O, WindowResult, WindowInfo, WindowId};
    for count in [1, 9, 10, 20, 23, 30] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        let request = enter_window(&mut engine, &mut backend, &log);
        let windows: Vec<_> = (1..=count).map(|id| WindowInfo { id: WindowId(id), title: format!("Window {id}"), app: "test".into(),
            bounds: Rect::new(0.0, 0.0, 300.0, 200.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false }).collect();
        // Supply the inventory on a real outstanding enumerate request.
        engine.dispatch_to(&ModeId::window(), ModeEvent::Timer { id: "window_inventory".into(), elapsed: Duration::from_millis(500) }, &mut backend).unwrap();
        let id = log.lock().unwrap().window_requests.last().unwrap().id;
        engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { tabs: None, closed: Vec::new(),
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
        bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { tabs: None, closed: Vec::new(),
        session: request.session, id: request.id + 1, target: Some(target), windows: None,
        pointer: None, changed: 0, skipped: 0, message: Some("Nothing to undo".into()), edit: None,
    })), &mut backend).unwrap();
    engine.handle_backend_event(key_up("x"), &mut backend).unwrap();
    let log = log.lock().unwrap();
    let labels = &log.scenes.last().unwrap().labels;
    assert!(labels.iter().any(|l| l.text == "Nothing to undo"));
    assert!(labels.iter().any(|l| l.text == "X / SHIFT+Z"));
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
        bounds: Rect::new(500.0, 200.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { tabs: None, closed: Vec::new(),
        session, id: request.id, pointer: Some(target.bounds.center()), target: Some(target),
        windows: None, changed: 0, skipped: 0, message: None, edit: None,
    })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().warps.last(), Some(&Point::new(700.0, 350.0)));
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Next window")), "Tab shows the locked target in the help panel");
    engine.handle_backend_event(key_down("l"), &mut backend).unwrap();
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation,
        WindowOperation::Adjust { target: WindowId(88), change: crate::api::window::WindowChange::Resize { .. }, .. }));
}

#[test]
fn temporary_normal_cancels_pending_window_motion_and_ignores_its_late_pointer() {
    let config = Config::default();
    let primary = &config.window.temporary_mode_keys[0];
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("l"), &mut backend).unwrap();
    let pending = log.lock().unwrap().window_requests.last().unwrap().clone();
    engine.handle_backend_event(key_down(primary), &mut backend).unwrap();
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation,
        crate::api::window::WindowOperation::CancelPending));
    let before = log.lock().unwrap().warps.len();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { tabs: None, closed: Vec::new(),
        session: pending.session, id: pending.id, target: None, windows: None,
        pointer: Some(Point::new(1.0, 2.0)), changed: 1, skipped: 0, message: None, edit: None,
    })), &mut backend).unwrap();
    assert_eq!(log.lock().unwrap().warps.len(), before);
    engine.handle_backend_event(key_up(primary), &mut backend).unwrap();
    engine.handle_backend_event(key_up("l"), &mut backend).unwrap();
    assert_eq!(engine.active_mode(), &ModeId::window());
}

#[test]
fn closing_target_stops_window_motion_and_waits_for_explicit_tab() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    engine.handle_backend_event(key_down("l"), &mut backend).unwrap();
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    let before = log.lock().unwrap().window_requests.len();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult { tabs: None, closed: Vec::new(),
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
    assert_eq!(engine.active_mode(), &ModeId::idle());
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
    let target = WindowInfo { id: WindowId(77), title: "Test window".into(), app: "test".into(), bounds: Rect::new(100.0,100.0,400.0,300.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { tabs: None, closed: Vec::new(), session: initial.session, id, target: Some(target.clone()), windows: Some(vec![target]), pointer: None, changed:0, skipped:0, message:None, edit:None })), &mut backend).unwrap();
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
    assert_eq!(engine.active_mode(), &ModeId::window_quick());
}

#[test]
fn window_base_motion_ignores_rebound_normal_keys_and_ignores_default_arrows() {
    use crate::api::window::WindowOperation as O;
    let mut config = Config::default();
    config.normal.bindings.insert("h".into(), Binding::Disabled);
    config.normal.bindings.insert("v".into(), Binding::Move(Direction::Left));
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for key in ["left", "v"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    }
    assert!(!log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::Adjust { .. })));
    for event in [key_down("h"), key_up("h")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::Adjust { .. })));
    let log = log.lock().unwrap();
    assert!(log.moves.is_empty());
    assert!(log.scenes.last().unwrap().labels.iter().any(|l| l.text == "H/J/K/L"));
}

#[test]
fn window_tree_labels_remain_large_and_avoid_window_cards() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    for key in ["a", "e"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
    }
    let log = log.lock().unwrap();
    let scene = log.scenes.last().unwrap();
    let number = scene.labels.iter().find(|l| l.text == "1").unwrap();
    let area = scene.labels.iter().find(|l| l.text == "`1").unwrap();
    assert!(area.rect.y > number.rect.bottom());
    assert!((area.rect.center().x - engine.screens[0].work_area.center().x).abs() < 1.0);
    assert!(engine.screens[0].work_area.contains(&area.rect.center()));
    assert!(scene.labels.iter().filter(|label| label.placement.is_some_and(|p| p.role == crate::api::overlay::LabelPlacementRole::Background)).all(|card| card.rect.intersect(&area.rect).is_none()));
    assert_eq!(area.style.font_size, 28.0);
}

#[test]
fn backtick_opens_auto_layout_and_pending_region_input_focuses_window() {
    use crate::api::window::{WindowOperation as O, WindowId};
    for quick in [false, true] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        enter_window(&mut engine, &mut backend, &log);
        if quick {
            for event in [key_down("a"), key_up("a")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            acknowledge_window_edit(&mut engine, &mut backend, &log);
        }
        for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        for key in ["`", "1"] {
            for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        let log = log.lock().unwrap();
        assert!(log.window_requests.iter().any(|r| matches!(r.operation, O::ApplyLayout { strict: true, .. })));
        assert!(!log.window_requests.iter().any(|r| matches!(r.operation, O::Tile { .. })));
        assert!(log.window_requests.iter().any(|r| matches!(r.operation, O::Select(WindowId(77)))));
        assert!(log.scenes.last().unwrap().labels.iter().any(|l| l.text == "Input: `1"));
    }
}

#[test]
fn x_deletes_a_tree_region_without_closing_its_window_and_z_restores_it() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    for key in ["a", "e"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
    }
    for event in [key_down("left_shift"), key_down("l"), key_up("l"), key_up("left_shift")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for key in ["`", "2", "x"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(!log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text == "`2"));
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text == "1"));
    for event in [key_down("z"), key_up("z")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text == "`2"));
}

#[test]
fn every_window_mode_launches_from_idle_and_q_uses_its_binding() {
    for (mode, back) in [(ModeId::window(), ModeId::idle()), (ModeId::window_quick(), ModeId::window()), (ModeId::window_editor(), ModeId::window()), (ModeId::window_restore(), ModeId::window())] {
        let mut config = Config::default();
        config.hotkeys.insert("alt+v".into(), Binding::Mode(mode.clone()));
        let (mut engine, mut backend, log) = window_test_engine(&config);
        engine.set_active(ModeId::idle());
        for event in [key_down("left_alt"), key_down("v"), key_up("v"), key_up("left_alt")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &mode);
        for event in [key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        assert_eq!(engine.active_mode(), &back);
    }
}

#[test]
fn custom_editor_q_commits_then_enters_the_configured_mode() {
    for destination in [ModeId::idle(), ModeId::normal(), ModeId::grid(), ModeId::window_restore()] {
        let mut config = Config::default();
        config.window_editor.bindings.insert("q".into(), Binding::Mode(destination.clone()));
        let (mut engine, mut backend, log) = window_test_engine(&config);
        let session = enter_window(&mut engine, &mut backend, &log).session;
        for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        for event in [key_down("left_shift"), key_down("l"), key_up("l"), key_up("left_shift"), key_down("q"), key_up("q")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &ModeId::window_editor());
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        assert_eq!(engine.active_mode(), &destination);
        assert_eq!(log.lock().unwrap().cancelled_window_sessions.contains(&session), !destination.is_window());
    }
}

#[test]
fn restore_finish_uses_configured_lifecycle_after_matching_apply() {
    for destination in [crate::api::LifecycleAction::Keep, crate::api::LifecycleAction::Mode(ModeId::window_editor()), crate::api::LifecycleAction::Mode(ModeId::idle())] {
        let mut config = Config::default(); config.window_restore.lifecycle.after_finish = destination.clone();
        let (mut engine, mut backend, log) = window_test_engine(&config);
        engine.window_presets.store.save(crate::api::window_presets::RegionTemplate::Slot { id: 1 }.into(), 1, "One".into()).unwrap();
        enter_window(&mut engine, &mut backend, &log);
        for event in [key_down("r"), key_up("r"), key_down("1"), key_up("1")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &ModeId::window_restore());
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        let expected = match destination { crate::api::LifecycleAction::Mode(mode) => mode, _ => ModeId::window_restore() };
        assert_eq!(engine.active_mode(), &expected);
    }
}

#[test]
fn delete_confirmation_cancel_and_empty_library_at_multiple_dpi() {
    for scale in [1.0, 1.5, 2.0] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        engine.screens[0].scale = scale;
        engine.screens[0].work_area = Rect::new(0.0, 0.0, 1920.0 * scale, 1080.0 * scale);
        engine.screens[0].bounds = engine.screens[0].work_area;
        for i in 1..=7 { engine.window_presets.store.save(crate::api::window_presets::RegionTemplate::Slot { id: 1 }.into(), 1, format!("Delete me {i}")).unwrap(); }
        enter_window(&mut engine, &mut backend, &log);
        for key in ["r", "x", "7"] { for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); } }
        assert_eq!(engine.window_presets.store.list().unwrap().len(), 7);
        {
            let recorded = log.lock().unwrap(); let scene = recorded.scenes.last().unwrap();
            let panel = scene.labels.iter().find(|l| l.z_index == i32::MAX - 1).unwrap().rect;
            assert!(scene.labels.iter().any(|l| l.text.contains("Delete me 7")));
            for label in &scene.labels {
                let rect = crate::api::overlay::scaled_label_geometry(&label.text, label.rect, &label.style, scale).0;
                assert!(rect.x >= panel.x - 1.0 && rect.right() <= panel.right() + 1.0 && rect.y >= panel.y - 1.0 && rect.bottom() <= panel.bottom() + 1.0);
            }
        }
        for key in ["x", "x", "enter"] { for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); } }
        assert_eq!(engine.window_presets.store.list().unwrap().len(), 7);
        for i in (1..=7).rev() {
            for key in [i.to_string(), "enter".into()] { for event in [key_down(&key), key_up(&key)] { engine.handle_backend_event(event, &mut backend).unwrap(); } }
            assert_eq!(engine.active_mode(), &ModeId::window_restore());
            let layouts = engine.window_presets.store.list().unwrap();
            assert_eq!(layouts.iter().map(|p| p.id).collect::<Vec<_>>(), (1..i).collect::<Vec<_>>());
        }
    }
}

#[test]
fn inherited_window_actions_and_app_overrides_use_the_active_session() {
    let mut config = Config::default();
    config.window_quick.bindings.insert("v".into(), Binding::Window(crate::api::window::WindowAction::Navigate(Direction::Right)));
    config.window_editor.inherits = vec!["window_quick".into()];
    config.window_editor.bindings.insert("h".into(), Binding::Disabled);
    config.window_editor.app_configs.push(crate::config::AppOverride { bundle_id: "override-app".into(), bindings: Bindings::from([("v".into(), Binding::Window(crate::api::window::WindowAction::Split(Direction::Right)))]) });
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    let help = engine.key_help_entries();
    assert!(help.iter().any(|entry| entry.contains("v")), "{help:?}");
    assert!(!help.iter().any(|entry| entry == "h"));
    engine.handle_backend_event(BackendEvent::FocusChanged(Some(FocusedApp { bundle_id: "override-app".into(), window_title: "example".into(), process_id: 9 })), &mut backend).unwrap();
    for event in [key_down("v"), key_up("v")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.text == "`2"));
    engine.handle_backend_event(BackendEvent::FocusChanged(None), &mut backend).unwrap();
    for event in [key_down("v"), key_up("v")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert_eq!(engine.active_mode(), &ModeId::window_editor());
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|label| label.text.contains("area `2")));
}

#[test]
fn failed_restore_stays_in_restore_and_can_be_retried() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    engine.window_presets.store.save(crate::api::window_presets::RegionTemplate::Slot { id: 1 }.into(), 1, "Retry".into()).unwrap();
    enter_window(&mut engine, &mut backend, &log);
    for key in ["r", "1"] { for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); } }
    acknowledge_window_edit_with_acceptance(&mut engine, &mut backend, &log, false);
    assert_eq!(engine.active_mode(), &ModeId::window_restore());
    for event in [key_down("1"), key_up("1")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(engine.active_mode(), &ModeId::window_editor());
}

#[test]
fn quick_can_select_a_window_when_entry_has_no_pointer_target() {
    use crate::api::window::{WindowInfo, WindowId, WindowResult, WindowOperation as O};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    engine.dispatch_to(&ModeId::window(), ModeEvent::Timer { id: "window_inventory".into(), elapsed: Duration::from_millis(500) }, &mut backend).unwrap();
    let window = WindowInfo { id: WindowId(77), title: "Selectable".into(), app: "test".into(), bounds: Rect::new(0.0, 0.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false };
    let request = log.lock().unwrap().window_requests.last().unwrap().clone();
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { tabs: None, session: request.session, id: request.id, target: None, windows: Some(vec![window.clone()]), pointer: None, closed: Vec::new(), changed: 0, skipped: 0, message: None, edit: None })), &mut backend).unwrap();
    for key in ["a", "1"] { for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); } }
    let select = log.lock().unwrap().window_requests.last().unwrap().clone();
    assert!(matches!(select.operation, O::Select(WindowId(77))));
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult { tabs: None, session: select.session, id: select.id, target: Some(window), windows: None, pointer: None, closed: Vec::new(), changed: 0, skipped: 0, message: None, edit: None })), &mut backend).unwrap();
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::BeginEdit { .. }));
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for event in [key_down("h"), key_up("h")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::ApplyLayout { .. }));
}


#[test]
fn window_history_bindings_are_configurable_and_c_still_centers() {
    use crate::api::window::{WindowAction as W, WindowOperation as O, WindowChange as C};
    for custom in [false, true] {
        let mut config = Config::default();
        let (modifier, redo_key, reset_key) = if custom { ("left_ctrl", "y", "b") } else { ("left_shift", "z", "c") };
        if custom {
            config.window.bindings.remove("shift+z"); config.window.bindings.remove("shift+c");
            config.window.bindings.insert("ctrl+y".into(), Binding::Window(W::Redo));
            config.window.bindings.insert("ctrl+b".into(), Binding::Window(W::ResetInitial));
        }
        let (mut engine, mut backend, log) = window_test_engine(&config);
        enter_window(&mut engine, &mut backend, &log);
        for event in [key_down("c"), key_up("c")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::Adjust { change: C::Center, .. }));
        for (key, redo) in [(redo_key, true), (reset_key, false)] {
            for event in [key_down(modifier), key_down(key), key_up(key), key_up(modifier)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            let operation = log.lock().unwrap().window_requests.last().unwrap().operation.clone();
            assert!(if redo { matches!(operation, O::Redo) } else { matches!(operation, O::ResetInitial { .. }) });
        }
    }
}

#[test]
fn quick_and_editor_redo_survive_undoing_the_entry_layout() {
    use crate::api::window::WindowOperation as O;
    for entry in ["a", "e"] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        enter_window(&mut engine, &mut backend, &log);
        for event in [key_down(entry), key_up(entry)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        if entry == "a" {
            for event in [key_down("h"), key_up("h")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            acknowledge_window_edit(&mut engine, &mut backend, &log);
        }
        let expected = log.lock().unwrap().window_requests.iter().rev().find_map(|request| match &request.operation {
            O::ApplyLayout { placements, .. } => Some(placements.clone()), _ => None,
        }).unwrap();
        for _ in 0..2 {
            for event in [key_down("z"), key_up("z")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            acknowledge_window_edit(&mut engine, &mut backend, &log);
        }
        let before_redo = log.lock().unwrap().window_requests.len();
        for event in [key_down("left_shift"), key_down("z"), key_up("z"), key_up("left_shift")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        acknowledge_window_edit(&mut engine, &mut backend, &log);
        let requests = log.lock().unwrap().window_requests.clone();
        let applied: Vec<_> = requests[before_redo..].iter().filter_map(|request| match &request.operation {
            O::ApplyLayout { placements, .. } => Some(placements), _ => None,
        }).collect();
        assert_eq!(applied, vec![&expected], "entry {entry} must redo the undone placement");
    }
}

#[test]
fn initial_reset_waits_for_the_latest_edit_and_reopens_without_auto_arranging() {
    use crate::api::window::{WindowOperation as O, WindowResult, WindowInfo, WindowId};
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for event in [key_down("left_shift"), key_down("l"), key_up("l"), key_down("c"), key_up("c"), key_up("left_shift")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(!log.lock().unwrap().window_requests.iter().any(|r| matches!(r.operation, O::ResetInitial { .. })));
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    let reset = log.lock().unwrap().window_requests.last().unwrap().clone();
    assert!(matches!(reset.operation, O::ResetInitial { .. }));
    let original = WindowInfo { id: WindowId(77), title: "Test window".into(), app: "test".into(), bounds: Rect::new(100.0, 100.0, 400.0, 300.0), screen: 0, resizable: true, maximized: false, minimized: false, fullscreen: false };
    engine.handle_backend_event(BackendEvent::WindowResult(Box::new(WindowResult {
        tabs: None,
        session: reset.session, id: reset.id, target: Some(original.clone()), windows: Some(vec![original]), closed: Vec::new(), pointer: None, changed: 1, skipped: 0, message: None, edit: None,
    })), &mut backend).unwrap();
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert_eq!(engine.active_mode(), &ModeId::window_editor());
    assert!(log.lock().unwrap().window_requests.iter().filter(|r| r.id > reset.id).all(|r| !matches!(r.operation, O::ApplyLayout { .. })));
    for event in [key_down("z"), key_up("z")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    assert!(matches!(log.lock().unwrap().window_requests.last().unwrap().operation, O::Undo));
}


#[test]
fn restore_help_refreshes_on_release_without_waiting_for_inventory() {
    let (mut engine, mut backend, log) = window_test_engine(&Config::default());
    enter_window(&mut engine, &mut backend, &log);
    assert!(!engine.overlay.key_help_visible);
    for event in [key_down("r"), key_up("r")] {
        engine.handle_backend_event(event, &mut backend).unwrap();
    }
    assert_eq!(engine.active_mode(), &ModeId::window_restore());
    let immediate = log.lock().unwrap().scenes.last().unwrap().clone();
    assert!(immediate.labels.iter().any(|l| l.text == "Normal"));
    engine.overlay.key_help_cache = None;
    engine.refresh_overlay(&mut backend).unwrap();
    let later = log.lock().unwrap().scenes.last().unwrap().clone();
    assert_eq!(immediate, later, "background refresh must not reveal stale omitted shortcuts");
    assert_eq!(engine.active_mode(), &ModeId::window_restore());
}

#[test]
fn restore_help_and_input_share_arbitrary_configured_aliases() {
    for (modifier, exit, launcher) in [("right_ctrl", "f10", "f9"), ("left_shift", "f8", "f7")] {
        let mut config = Config::default();
        for (alias, physical) in [("Modifier", modifier), ("ExitKey", exit), ("LaunchKey", launcher), ("Leave", "ExitKey")] {
            config.key_aliases.all.insert(alias.into(), physical.into());
        }
        config.hotkeys.retain(|_, binding| *binding != Binding::Mode(ModeId::normal()));
        config.hotkeys.insert("Modifier+LaunchKey".into(), Binding::Mode(ModeId::normal()));
        config.window_restore.bindings.retain(|_, binding| *binding != Binding::Mode(ModeId::idle()));
        config.window_restore.bindings.insert("Modifier+Leave".into(), Binding::Mode(ModeId::idle()));
        let config = Config::parse(&toml::to_string(&config).unwrap()).unwrap();
        let (mut engine, mut backend, log) = window_test_engine(&config);
        enter_window(&mut engine, &mut backend, &log);
        for event in [key_down("r"), key_up("r")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        let labels = log.lock().unwrap().scenes.last().unwrap().labels.clone();
        for key in [exit, launcher] {
            let expected = format!("{modifier}+{key}").replace('_', " ").to_uppercase();
            assert!(labels.iter().any(|l| l.text == expected.as_str()), "missing {expected}");
        }
        assert!(!labels.iter().any(|l| l.text.contains("PRIMARY") || l.text.contains("MODIFIER") || l.text.contains("LEAVE")));
        for event in [key_down(modifier), key_down(launcher), key_up(launcher), key_up(modifier)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &ModeId::normal());
        enter_window(&mut engine, &mut backend, &log);
        for event in [key_down("r"), key_up("r"), key_down(modifier), key_down(exit), key_up(exit), key_up(modifier)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &ModeId::idle());
    }
}


#[test]
fn region_resize_uses_configured_alias_chord_and_updates_its_hint() {
    use crate::api::window::{WindowAction as W, WindowOperation as O};
    let mut config = Config::default();
    config.key_aliases.all.insert("RegionGrow".into(), "f9".into());
    config.window_editor.bindings.retain(|_, binding| *binding != Binding::Window(W::Ratio(Direction::Right)));
    config.window_editor.bindings.insert("RegionGrow".into(), Binding::Window(W::Ratio(Direction::Right)));
    let config = Config::parse(&toml::to_string(&config).unwrap()).unwrap();
    let (mut engine, mut backend, log) = window_test_engine(&config);
    enter_window(&mut engine, &mut backend, &log);
    for event in [key_down("e"), key_up("e")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    for event in [key_down("left_shift"), key_down("l"), key_up("l"), key_up("left_shift")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    acknowledge_window_edit(&mut engine, &mut backend, &log);
    let width = |recorded: &Recorder| recorded.window_requests.iter().rev().find_map(|request| match &request.operation {
        O::ApplyLayout { placements, .. } => placements.first().map(|(_, rect)| rect.width), _ => None,
    }).unwrap();
    let before = width(&log.lock().unwrap());
    assert!(!engine.key_help_entries().iter().any(|entry| entry.starts_with("ctrl+l  ") && entry.ends_with("window_ratio_right")));
    let scene = log.lock().unwrap().scenes.last().unwrap().clone();
    assert!(scene.labels.iter().any(|l| l.text == "F9"));
    assert!(scene.labels.iter().any(|l| l.text == "Grow region width"));
    for event in [key_down("f9"), key_up("f9")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    assert!(width(&log.lock().unwrap()) > before);
}

#[test]
fn window_close_uses_default_or_configured_alias_and_editor_keeps_remove_region() {
    use crate::api::window::{WindowAction as W, WindowOperation as O};
    for custom in [false, true] {
        let mut config = Config::default();
        if custom {
            config.window.bindings.remove("x");
            config.key_aliases.all.insert("CloseKey".into(), "f9".into());
            config.window.bindings.insert("CloseKey".into(), Binding::Window(W::Close));
        }
        let config = Config::parse(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(config.window_editor.bindings.get("x"), Some(&Binding::Window(W::RemoveRegion)));
        for bindings in [&config.window_quick.bindings, &config.window_editor.bindings, &config.window_tab.bindings] {
            assert!(!bindings.values().any(|binding| *binding == Binding::Window(W::Close)));
        }
        let (mut engine, mut backend, log) = window_test_engine(&config);
        enter_window(&mut engine, &mut backend, &log);
        let labels = log.lock().unwrap().scenes.last().unwrap().labels.clone();
        assert!(labels.iter().any(|l| l.text == "Close window"));
        assert!(labels.iter().any(|l| l.text == if custom { "F9" } else { "X" }));
        let before = log.lock().unwrap().window_requests.len();
        if custom {
            for event in [key_down("x"), key_up("x")] {
                engine.handle_backend_event(event, &mut backend).unwrap();
            }
            assert!(!log.lock().unwrap().window_requests[before..].iter().any(|r| matches!(r.operation, O::Close(_))));
        }
        let key = if custom { "f9" } else { "x" };
        for event in [key_down(key), key_up(key)] {
            engine.handle_backend_event(event, &mut backend).unwrap();
        }
        let requests = log.lock().unwrap().window_requests.clone();
        let closes: Vec<_> = requests[before..].iter().filter_map(|r| match r.operation { O::Close(id) => Some(id), _ => None }).collect();
        assert_eq!(closes, vec![crate::api::window::WindowId(77)]);
    }
}

fn grouped_help_sample(entry: Option<&str>, scale: f64, font_size: f64) -> OverlayScene {
    let mut config = Config::default();
    config.key_help.font_size = font_size;
    let (mut engine, mut backend, log) = window_test_engine(&config);
    engine.screens[0].scale = scale;
    engine.screens[0].bounds = Rect::new(-1280.0 * scale, 0.0, 1280.0 * scale, 800.0 * scale);
    engine.screens[0].work_area = engine.screens[0].bounds;
    enter_window(&mut engine, &mut backend, &log);
    if let Some(key) = entry {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        if matches!(key, "a" | "e") { acknowledge_window_edit(&mut engine, &mut backend, &log); }
    }
    log.lock().unwrap().scenes.last().unwrap().clone()
}

#[test]
fn grouped_window_help_keeps_two_semantic_columns_and_header_exit_at_each_dpi() {
    use crate::api::overlay::scaled_label_geometry;
    for scale in [1.0, 1.5, 2.0] {
        for font in [12.0, 16.0] {
            for entry in [None, Some("a"), Some("e"), Some("t"), Some("r")] {
                let scene = grouped_help_sample(entry, scale, font);
                let panel = scene.labels.iter().find(|l| l.z_index == i32::MAX - 1).unwrap().rect;
                let labels: Vec<_> = scene.labels.iter().filter(|l| l.z_index == i32::MAX && !l.text.is_empty()).map(|l| (l, scaled_label_geometry(&l.text, l.rect, &l.style, scale).0)).collect();
                for (label, rect) in &labels {
                    assert!(rect.x >= panel.x - 1.0 && rect.right() <= panel.right() + 1.0 && rect.y >= panel.y - 1.0 && rect.bottom() <= panel.bottom() + 1.0, "entry={entry:?} scale={scale} {label:?} outside {panel:?}");
                }
                for (i, (label, rect)) in labels.iter().enumerate() {
                    for (other, bounds) in &labels[i + 1..] {
                        assert!(rect.intersect(bounds).is_none_or(|r| r.width < 1.0 || r.height < 1.0), "entry={entry:?} scale={scale} overlap {:?} {rect:?} with {:?} {bounds:?}", label.text, other.text);
                    }
                }
                assert!(panel.y >= -1.0 && panel.bottom() <= 800.0 * scale);
                let common = labels.iter().find(|(l, _)| l.text == "COMMON").unwrap().1;
                if entry != Some("r") {
                    let operation = labels.iter().find(|(l, _)| l.text.ends_with("ACTIONS") || matches!(l.text.as_str(), "EDIT LAYOUT" | "QUICK LAYOUT" | "TAB GROUPS")).unwrap().1;
                    assert!(operation.x < common.x, "mode actions stay in the left column");
                }
                assert!(labels.iter().any(|(l, _)| l.text == if entry.is_none() { "Exit \u{2192} Idle" } else { "Back \u{2192} Window" }));
                let exit = labels.iter().find(|(l, _)| l.text == if entry.is_none() { "Exit \u{2192} Idle" } else { "Back \u{2192} Window" }).unwrap().1;
                assert!(exit.right() > panel.center().x && exit.y < common.y);
                let title = labels.iter().find(|(l, _)| l.text == "Window").unwrap().1;
                assert!((exit.center().y - title.center().y).abs() <= 1.0);
                for (label, bounds) in &labels {
                    if label.style.background.a > 0 && bounds.y < title.bottom() {
                        assert!((bounds.center().y - title.center().y).abs() <= 1.0,
                            "header {:?} must share the title center: {bounds:?} vs {title:?}", label.text);
                    }
                }
                if entry == Some("t") {
                    assert!(labels.iter().any(|(l, _)| l.text == "H/J/K/L"));
                    assert!(labels.iter().any(|(l, _)| l.text == "Move / switch tab"));
                    assert!(!labels.iter().any(|(l, _)| l.text.starts_with("move_")));
                }

                if entry.is_none() {
                    assert!(labels.iter().any(|(l, _)| l.text == "H/J/K/L"));
                    assert!(labels.iter().any(|(l, _)| l.text == "Maximize / minimize / restore"));
                    let key_rects: Vec<_> = ["H/J/K/L", "S", "C", "D", "F", "X"].iter().map(|key| labels.iter().find(|(l, _)| l.text == *key).unwrap().1).collect();
                    assert!(key_rects.windows(2).all(|pair| (pair[0].right() - pair[1].right()).abs() < 1.0));

                    let keys: Vec<_> = ["A", "E", "R", "T"].into_iter().map(|key| labels.iter().find(|(l, _)| l.text == key).unwrap().1).collect();
                    assert!(keys.windows(2).all(|pair| (pair[0].y - pair[1].y).abs() < 1.0 && pair[0].x < pair[1].x));
                }
            }
        }
    }
}

#[test]
#[ignore = "exports current native compositor geometry for visual inspection"]
fn export_grouped_window_help_visual_samples() {
    use crate::api::overlay::scaled_label_geometry;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/window-help-v14");
    std::fs::create_dir_all(&directory).unwrap();
    let scale = 1.5;
    for (name, entry) in [("window", None), ("quick", Some("a")), ("editor", Some("e")), ("tabs", Some("t")), ("restore", Some("r"))] {
        let scene = grouped_help_sample(entry, scale, 12.0);
        let labels: Vec<_> = scene.labels.iter().filter(|l| l.z_index >= i32::MAX - 1).map(|l| serde_json::json!({ "text": l.text, "rect": scaled_label_geometry(&l.text, l.rect, &l.style, scale).0, "style": l.style, "scale": scale })).collect();
        std::fs::write(directory.join(format!("{name}.json")), serde_json::to_string_pretty(&labels).unwrap()).unwrap();
    }
}

#[test]
fn window_return_hint_uses_local_configured_destination_and_physical_alias() {
    for entry in ["a", "e", "r", "t"] {
        let mut config = Config::default();
        config.key_aliases.all.insert("ReturnKey".into(), "f9".into());
        let bindings = match entry {
            "a" => &mut config.window_quick.bindings,
            "e" => &mut config.window_editor.bindings,
            "r" => &mut config.window_restore.bindings,
            _ => &mut config.window_tab.bindings,
        };
        bindings.insert("q".into(), Binding::Disabled);
        bindings.insert("ReturnKey".into(), Binding::Mode(ModeId::normal()));
        let config = Config::parse(&toml::to_string(&config).unwrap()).unwrap();
        let (mut engine, mut backend, log) = window_test_engine(&config);
        enter_window(&mut engine, &mut backend, &log);
        for event in [key_down(entry), key_up(entry)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        if matches!(entry, "a" | "e") { acknowledge_window_edit(&mut engine, &mut backend, &log); }
        let scene = log.lock().unwrap().scenes.last().unwrap().clone();
        let caption = scene.labels.iter().find(|label| label.text == "Back \u{2192} Normal").unwrap();
        let common = scene.labels.iter().find(|label| label.text == "COMMON").unwrap();
        assert!(caption.rect.y < common.rect.y);
        assert!(scene.labels.iter().any(|label| label.text.contains("F9")));
        assert!(!scene.labels.iter().any(|label| label.text == "Back \u{2192} Window"));
        for event in [key_down("f9"), key_up("f9")] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        if matches!(entry, "a" | "e") { acknowledge_window_edit(&mut engine, &mut backend, &log); }
        if entry == "t" {
            let request = log.lock().unwrap().window_requests.last().unwrap().clone();
            engine.handle_backend_event(BackendEvent::WindowResult(Box::new(crate::api::window::WindowResult {
                session: request.session, id: request.id, tabs: Some(crate::api::window_tabs::TabState::default()),
                target: None, windows: None, closed: Vec::new(), pointer: None, changed: 0, skipped: 0, message: None, edit: None,
            })), &mut backend).unwrap();
        }
        assert_eq!(engine.active_mode(), &ModeId::normal());
    }
}

#[test]
fn window_normal_launcher_hint_stays_visible_across_unrelated_key_edges() {
    for entry in [None, Some("a"), Some("e"), Some("r"), Some("t")] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        enter_window(&mut engine, &mut backend, &log);
        if let Some(key) = entry {
            for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            if matches!(key, "a" | "e") { acknowledge_window_edit(&mut engine, &mut backend, &log); }
        }
        let launcher = engine.key_help_entries().into_iter().find(|hint| hint.ends_with("  \u{b7}  normal")).unwrap_or_else(|| panic!("entry={entry:?} missing launcher: {:?}", engine.key_help_entries()));
        for key in ["h", "j", "k", "l", "left_shift", "left_ctrl"] {
            for (edge, event) in [("down", key_down(key)), ("up", key_up(key))] {
                engine.handle_backend_event(event, &mut backend).unwrap();
                assert!(engine.key_help_entries().contains(&launcher), "entry={entry:?} key={key} edge={edge} lost {launcher}");
            }
        }
    }
}

#[test]
fn window_family_shortcut_plan_survives_input_and_scene_refresh_until_routes_change() {
    for entry in [None, Some("a"), Some("e"), Some("r"), Some("t")] {
        let (mut engine, mut backend, log) = window_test_engine(&Config::default());
        enter_window(&mut engine, &mut backend, &log);
        if let Some(key) = entry {
            for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        }
        let plan = Arc::clone(&engine.overlay.window_help_plan.as_ref().unwrap().entries);
        if entry == Some("e") {
            assert!(plan.iter().any(|hint| hint.ends_with("window_save_layout")));
            acknowledge_window_edit(&mut engine, &mut backend, &log);
        }
        for key in ["h", "left_shift", "left_ctrl"] {
            for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
            engine.overlay.key_help_cache = None;
            engine.refresh_overlay(&mut backend).unwrap();
            assert!(Arc::ptr_eq(&plan, &engine.overlay.window_help_plan.as_ref().unwrap().entries),
                "mode={entry:?}: shortcut table must be reused even when the scene is recomposed");
        }
        engine.rebuild_tables();
        assert!(engine.overlay.window_help_plan.is_none());
        engine.refresh_overlay(&mut backend).unwrap();
        assert!(!Arc::ptr_eq(&plan, &engine.overlay.window_help_plan.as_ref().unwrap().entries));
    }
}

#[test]
fn restore_delete_toggle_is_rebindable_keeps_page_and_cached_help() {
    use crate::api::window::WindowAction;
    let mut config = Config::default();
    config.window_restore.bindings.remove("x");
    config.window_restore.bindings.insert("v".into(), Binding::Window(WindowAction::DeletePreset));
    let (mut engine, mut backend, log) = window_test_engine(&config);
    for i in 1..=7 {
        engine.window_presets.store.save(crate::api::window_presets::RegionTemplate::Slot { id: 1 }.into(), 1, format!("Preset {i}")).unwrap();
    }
    enter_window(&mut engine, &mut backend, &log);
    for key in ["r", "page_down"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    }
    let plan = engine.overlay.window_help_plan.as_ref().unwrap().entries.clone();
    for key in ["v", "7", "v", "enter"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
        assert_eq!(engine.active_mode(), &ModeId::window_restore());
        assert!(std::sync::Arc::ptr_eq(&plan, &engine.overlay.window_help_plan.as_ref().unwrap().entries));
    }
    assert_eq!(engine.window_presets.store.list().unwrap().len(), 7);
    assert!(log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("page 2 / 2")));
    for key in ["v", "7", "enter", "q", "r"] {
        for event in [key_down(key), key_up(key)] { engine.handle_backend_event(event, &mut backend).unwrap(); }
    }
    assert_eq!(engine.window_presets.store.list().unwrap().len(), 6);
    assert_eq!(engine.active_mode(), &ModeId::window_restore());
    assert!(!log.lock().unwrap().scenes.last().unwrap().labels.iter().any(|l| l.text.contains("Delete layouts")));
}
