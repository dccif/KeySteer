#![cfg(test)]

use super::*;
use crate::api::Appearance;
use crate::api::window::WindowResult;

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
    let mut mode = WindowMode::new(Settings {
        double_tap_ms: w.double_tap_ms,
        number_timeout_ms: w.number_timeout_ms,
        split_ratios: w.parsed_split_ratios().unwrap(),
        move_step: w.move_step,
        move_speed: w.move_speed,
        resize_step: w.resize_step,
        resize_speed: w.resize_speed,
        gap: w.gap,
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
            fullscreen: false,
        })
        .collect();
    let event = |id| {
        ModeEvent::WindowResult(Box::new(WindowResult {
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
