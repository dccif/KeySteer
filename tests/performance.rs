use std::alloc::System;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use keysteer::api::{
    Appearance, Binding, Command, CommandBatch, Direction, HostContext, KeyState, LabelDirection,
    Mode, Rect,
};
use keysteer::domain::hints::assign_into;
use keysteer::modes::normal::NormalMode;
use keysteer::{Config, Key, ModeEvent, Point};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[test]
fn steady_normal_frames_do_not_allocate() {
    let config = Config::default();
    let palette = config.palette(Appearance::Dark);
    let context = HostContext {
        screens: &[],
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
        config: &config,
    };
    let mut mode = NormalMode::new(&config);
    let key = Key::new("l").unwrap();
    let _ = mode.handle(
        &ModeEvent::Binding {
            binding: Arc::new(Binding::Move(Direction::Right)),
            state: KeyState::Down,
            key,
        },
        &context,
    );

    // Warm every lazily initialised branch before opening the allocation
    // region. The gate covers the steady display-frame path users feel.
    let _ = mode.handle(
        &ModeEvent::Frame {
            elapsed: Duration::from_micros(8_333),
        },
        &context,
    );

    let region = Region::new(GLOBAL);
    for _ in 0..10_000 {
        black_box(mode.handle(
            &ModeEvent::Frame {
                elapsed: Duration::from_micros(8_333),
            },
            &context,
        ));
    }
    let change = region.change();
    assert_eq!(change.allocations, 0, "steady frames allocated: {change:?}");
    assert_eq!(change.deallocations, 0, "steady frames freed: {change:?}");
    assert_eq!(
        change.bytes_allocated, 0,
        "steady frames allocated bytes: {change:?}"
    );
    inline_command_batches_do_not_allocate();
    warmed_hint_assignment_reuses_two_thousand_labels();
}

fn inline_command_batches_do_not_allocate() {
    let region = Region::new(GLOBAL);
    for _ in 0..10_000 {
        let mut batch = CommandBatch::new();
        batch.push(Command::HideOverlay);
        batch.push(Command::ReloadConfig);
        black_box(batch);
    }
    let change = region.change();
    assert_eq!(
        change.allocations, 0,
        "inline batches allocated: {change:?}"
    );
    assert_eq!(
        change.bytes_allocated, 0,
        "inline batches allocated bytes: {change:?}"
    );
}

fn warmed_hint_assignment_reuses_two_thousand_labels() {
    const TARGETS: usize = 2_000;
    let alphabet: Vec<char> = "arstneioqwfpjluy".chars().collect();
    let targets = (0..TARGETS).map(|index| {
        (
            Rect::new((index % 100) as f64, (index / 100) as f64, 8.0, 8.0),
            index,
        )
    });
    let mut output = Vec::new();
    assert!(
        assign_into(
            &mut output,
            targets.clone(),
            &alphabet,
            LabelDirection::Normal,
        )
        .is_ok(),
        "valid hint alphabet must assign"
    );

    let region = Region::new(GLOBAL);
    for _ in 0..100 {
        assert!(
            assign_into(
                &mut output,
                targets.clone(),
                &alphabet,
                LabelDirection::Normal,
            )
            .is_ok(),
            "valid hint alphabet must assign"
        );
        black_box(&output);
    }
    let change = region.change();
    assert_eq!(change.allocations, 0, "hint relabel allocated: {change:?}");
    assert_eq!(
        change.bytes_allocated, 0,
        "hint relabel allocated bytes: {change:?}"
    );
}
