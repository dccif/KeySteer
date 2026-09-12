#![cfg(test)]

use super::*;
use crate::api::Appearance;
use crate::api::window::WindowResult;

#[test]
fn tabs_use_move_verbs_and_overview_lists_every_member() {
    use crate::api::{
        Screen,
        window_tabs::{TabGroup, TabGroupId},
    };
    use std::sync::Arc;
    let config = crate::config::Config::default();
    assert!(!config.window_tab.bindings.contains_key("ctrl+left"));
    assert!(!config.window_tab.bindings.contains_key("ctrl+right"));
    let palette = config.palette(Appearance::Dark);
    let screens = [Screen {
        name: None,
        bounds: Rect::new(0.0, 0.0, 1200.0, 800.0),
        work_area: Rect::new(0.0, 0.0, 1200.0, 800.0),
        scale: 1.0,
        is_primary: true,
    }];
    let ctx = HostContext {
        presenter: &crate::presentation::COMPOSER,
        screens: &screens,
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let mut mode = crate::app::mode_catalog::window(&config);
    mode.session = 1;
    mode.kind = WindowKind::Tab;
    for (key, expected) in [
        (
            "h",
            crate::api::window_tabs::TabOperation::Reorder { backwards: true },
        ),
        (
            "l",
            crate::api::window_tabs::TabOperation::Reorder { backwards: false },
        ),
        (
            "k",
            crate::api::window_tabs::TabOperation::Cycle { backwards: true },
        ),
        (
            "j",
            crate::api::window_tabs::TabOperation::Cycle { backwards: false },
        ),
    ] {
        let commands = mode.handle(
            &ModeEvent::Binding {
                binding: Arc::new(config.window_tab.bindings[key].clone()),
                state: KeyState::Down,
                key: Key::new(key).unwrap(),
            },
            &ctx,
        );
        assert!(commands.iter().any(|c| matches!(c, Command::WindowRequest(request) if request.operation == WindowOperation::Tabs(expected.clone()))));
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, Command::SetFrameClock(true)))
        );
        mode.tabs.in_flight = None;
    }
    mode.kind = WindowKind::Move;
    for (id, app) in [(1, "Explorer"), (2, "Zed"), (3, "Browser")] {
        let id = WindowId(id);
        mode.inventory.insert(
            id,
            WindowInfo {
                id,
                app: app.into(),
                title: format!("{app} document"),
                bounds: Rect::new(100.0, 100.0, 500.0, 400.0),
                screen: 0,
                resizable: true,
                maximized: false,
                minimized: false,
                fullscreen: false,
            },
        );
    }
    mode.rebuild_numbers();
    mode.tabs.state.groups.push(TabGroup {
        id: TabGroupId(1),
        members: vec![WindowId(1), WindowId(2), WindowId(3)],
        active: WindowId(2),
    });
    let Command::ShowOverlay(scene) = ctx.present(mode.view()) else {
        panic!("missing overview");
    };
    for app in ["Explorer", "Zed", "Browser"] {
        assert!(
            scene.labels.iter().any(|label| label.text.contains(app)),
            "missing {app}"
        );
    }
    assert!(
        scene
            .labels
            .iter()
            .any(|label| label.text.contains("●") && label.text.contains("Zed"))
    );
}

#[test]
fn tab_composition_input_waits_for_native_ack_and_t_only_ends_each_batch() {
    use crate::api::window_tabs::{TabOperation, WindowTarget};
    let config = crate::config::Config::default();
    let palette = config.palette(Appearance::Dark);
    let ctx = HostContext {
        presenter: &crate::presentation::COMPOSER,
        screens: &[],
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let mut mode = crate::app::mode_catalog::window(&config);
    mode.session = 1;
    mode.visible = (1..=4).map(WindowId).collect();
    mode.numbers = (1..=4).map(|id| (WindowId(id), id as u32)).collect();
    mode.window_index = NumberIndex::new(1..=4);
    let mut out = CommandBatch::new();
    mode.enter_tabs(&mut out);
    for key in "12t34t".chars() {
        mode.tab_input(
            if key == 't' {
                tabs::Input::Action(W::TabEnd)
            } else {
                tabs::Input::Digit(key)
            },
            &ctx,
            &mut out,
        );
    }
    assert_eq!(mode.tabs.queue.len(), 6);
    let mut operations = Vec::new();
    loop {
        for command in &out {
            if let Command::WindowRequest(request) = command
                && let WindowOperation::Tabs(operation) = &request.operation
            {
                operations.push(operation.clone());
            }
        }
        out = CommandBatch::new();
        let Some(id) = mode.tabs.in_flight else {
            break;
        };
        mode.tab_result(id, &ctx, &mut out);
    }
    assert_eq!(
        operations,
        vec![
            TabOperation::Enter { screen: 0 },
            TabOperation::Choose(WindowTarget::Window(WindowId(1))),
            TabOperation::Choose(WindowTarget::Window(WindowId(2))),
            TabOperation::EndGroup,
            TabOperation::Choose(WindowTarget::Window(WindowId(3))),
            TabOperation::Choose(WindowTarget::Window(WindowId(4))),
            TabOperation::EndGroup
        ]
    );
}

#[test]
fn tab_space_finishes_ambiguous_number_and_prefix_immediately_selects_group_input() {
    let config = crate::config::Config::default();
    let palette = config.palette(Appearance::Dark);
    let ctx = HostContext {
        presenter: &crate::presentation::COMPOSER,
        screens: &[],
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let mut mode = crate::app::mode_catalog::window(&config);
    mode.session = 1;
    mode.visible = (1..=23).map(WindowId).collect();
    mode.numbers = (1..=23).map(|id| (WindowId(id), id as u32)).collect();
    mode.window_index = NumberIndex::new(1..=23);
    let mut out = CommandBatch::new();
    mode.tab_input(tabs::Input::Digit('1'), &ctx, &mut out);
    assert!(mode.tabs.in_flight.is_none());
    mode.tab_input(tabs::Input::Action(W::TabSeparator), &ctx, &mut out);
    assert!(mode.tabs.in_flight.is_some());
    let id = mode.tabs.in_flight.unwrap();
    mode.tab_result(id, &ctx, &mut out);
    mode.tab_input(tabs::Input::Action(W::TabPrefix), &ctx, &mut out);
    assert!(mode.number.slot);
    assert!(mode.tab_detail().contains("choose a tab group"));
}

#[test]
#[ignore = "run alone with --test-threads=1 so allocator counts are isolated"]
fn window_idle_inventory_and_number_input_do_not_allocate() {
    use stats_alloc::Region;
    use std::hint::black_box;
    let config = crate::config::Config::default();
    let palette = config.palette(Appearance::Dark);
    let context = HostContext {
        presenter: &crate::presentation::COMPOSER,
        screens: &[],
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let w = &config.window;
    let mut mode = WindowSession::new(Settings {
        all_screens: w.screens == crate::config::WindowScreens::All,
        include_minimized: w.include_minimized,
        lifecycle: crate::api::TargetingLifecycle::default(),
        split_ratios: crate::api::window_layout::RATIOS.to_vec(),
        ratio_ticks: crate::config::WindowQuick::default()
            .split_ratio_ticks()
            .into(),
        number_timeout_ms: w.number_timeout_ms,
        move_step: w.move_step,
        move_speed: w.move_speed,
        resize_step: w.resize_step,
        resize_speed: w.resize_speed,
        gap: config.window_editor.gap,
        border_width: w.border_width,
        ui: w.ui.clone(),
    });
    mode.session = 1;
    let windows: Vec<_> = (1..=23)
        .map(|id| WindowInfo {
            id: WindowId(id),
            title: format!("Window {id}"),
            app: "test".into(),
            bounds: Rect::new(0.0, 0.0, 200.0, 100.0),
            screen: 0,
            resizable: true,
            maximized: false,
            minimized: false,
            fullscreen: false,
        })
        .collect();
    let event = |id| {
        ModeEvent::WindowResult(Box::new(WindowResult {
            tabs: None,
            closed: Vec::new(),
            session: 1,
            id,
            target: Some(windows[0].clone()),
            windows: Some(windows.clone()),
            pointer: None,
            changed: 0,
            skipped: 0,
            message: None,
            edit: None,
        }))
    };
    mode.handle_owned(event(1), &context);
    // Incoming owned native results are prepared before counting mode work.
    let events: Vec<_> = (2..=1001).map(event).collect();
    let region = Region::new(crate::TEST_ALLOCATOR);
    for event in events {
        assert!(black_box(mode.handle_owned(event, &context)).is_empty());
    }
    let inventory = region.change();

    let windows = NumberIndex::new(1..=30);
    let slots = NumberIndex::new(1..=12);
    let mut input = NumberInput::default();
    input.digit('1', &windows, &slots);
    input.digit('2', &windows, &slots);
    let region = Region::new(crate::TEST_ALLOCATOR);
    for _ in 0..10_000 {
        black_box(input.digit('1', &windows, &slots));
        black_box(input.digit('2', &windows, &slots));
    }
    let digits = region.change();
    println!("window idle inventory: {inventory:?}; number input: {digits:?}");
    assert_eq!(inventory.allocations, 0, "unchanged inventory allocated");
    assert_eq!(digits.allocations, 0, "number input allocated");
}

#[test]
fn divider_hold_uses_elapsed_pixels_and_one_undo_checkpoint() {
    use crate::api::{Direction, Screen};
    let config = crate::config::Config::default();
    let palette = config.palette(Appearance::Dark);
    let screens = [Screen {
        bounds: Rect::new(0.0, 0.0, 1000.0, 700.0),
        work_area: Rect::new(0.0, 0.0, 1000.0, 700.0),
        scale: 1.0,
        is_primary: true,
        name: None,
    }];
    let ctx = HostContext {
        presenter: &crate::presentation::COMPOSER,
        screens: &screens,
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let mut mode = crate::app::mode_catalog::window(&config);
    let mut tree = LayoutTree::import(&[], None, screens[0].work_area);
    tree.split(Direction::Right);
    mode.target = Some(WindowInfo {
        id: crate::api::window::WindowId(1),
        app: "test".into(),
        title: "Test".into(),
        bounds: screens[0].work_area,
        screen: 0,
        resizable: true,
        maximized: false,
        minimized: false,
        fullscreen: false,
    });
    mode.edit = Some(LiveEdit {
        additional_trees: BTreeMap::new(),
        transaction: 1,
        screen: 0,
        model: EditModel::Tree(tree.clone()),
        accepted: EditModel::Tree(tree),
        history: Vec::new(),
        redo: Vec::new(),
        divider_gesture: false,
        entry_layout: false,
        minimums: BTreeMap::new(),
        gap_scale: 1.0,
        ready: true,
        revision: 0,
        in_flight: None,
        dirty: false,
        finishing: None,
        ending: false,
        deferred: Vec::new(),
    });
    let mut out = CommandBatch::default();
    let key = Key::new("l").unwrap();
    mode.action(
        W::Ratio(Direction::Right),
        KeyState::Down,
        &key,
        &ctx,
        &mut out,
    );
    assert!(
        out.iter()
            .any(|c| matches!(c, Command::SetFrameClock(true)))
    );
    // Native key repeats do not add another short-press step.
    mode.action(
        W::Ratio(Direction::Right),
        KeyState::Down,
        &key,
        &ctx,
        &mut out,
    );
    for _ in 0..5 {
        mode.motion(Some(0.02), &ctx, &mut out);
    }
    let edit = mode.edit.as_ref().unwrap();
    let EditModel::Tree(tree) = &edit.model else {
        panic!()
    };
    let expected = 0.5 + (config.window.resize_step + 0.1 * config.window.resize_speed) / 1000.0;
    assert!((tree.slots()[0].rect.width - expected).abs() < 1e-9);
    assert_eq!(edit.history.len(), 1);
    // Backpressure retains one submitted revision and the latest desired tree.
    assert_eq!(out.iter().filter(|c| matches!(c, Command::WindowRequest(r) if matches!(r.operation, WindowOperation::ApplyLayout { .. }))).count(), 1);
    mode.action(
        W::Ratio(Direction::Right),
        KeyState::Up,
        &key,
        &ctx,
        &mut out,
    );
    assert!(mode.held.is_empty());
    assert!(!mode.edit.as_ref().unwrap().divider_gesture);
    mode.action(
        W::Undo,
        KeyState::Down,
        &Key::new("z").unwrap(),
        &ctx,
        &mut out,
    );
    assert!(out.iter().any(|c| matches!(c, Command::WindowRequest(r) if matches!(r.operation, WindowOperation::EndEdit { commit: false, .. }))));
}
