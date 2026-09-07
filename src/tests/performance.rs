use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use keysteer::api::{
    Appearance, Binding, Command, CommandBatch, Direction, HostContext, KeyState, LabelDirection,
    Mode, Rect, Screen, UiScanResult, UiScanStatus, UiTarget,
};
use keysteer::api::{Key, ModeEvent, Point};
use keysteer::config::Config;
use keysteer::modes::hint::labeling::assign_compact_into;
use stats_alloc::Region;

#[test]
#[ignore = "run alone with --test-threads=1 so other tests cannot pollute allocator counts"]
fn steady_normal_frames_do_not_allocate() {
    let config = Config::default();
    let palette = config.palette(Appearance::Dark);
    let context = HostContext {
        screens: &[],
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let mut mode = keysteer::app::mode_catalog::normal(&config);
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

    let region = Region::new(keysteer::TEST_ALLOCATOR);
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
    warmed_compact_hint_assignment_reuses_two_thousand_labels();
    owned_hint_delivery_stays_within_allocation_budget();
}

fn inline_command_batches_do_not_allocate() {
    let region = Region::new(keysteer::TEST_ALLOCATOR);
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

fn warmed_compact_hint_assignment_reuses_two_thousand_labels() {
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
        assign_compact_into(
            &mut output,
            targets.clone(),
            &alphabet,
            LabelDirection::Normal,
        )
        .is_ok(),
        "valid hint alphabet must assign"
    );

    let region = Region::new(keysteer::TEST_ALLOCATOR);
    for _ in 0..100 {
        assert!(
            assign_compact_into(
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

fn owned_hint_delivery_stays_within_allocation_budget() {
    const TARGETS: usize = 2_000;
    const MAX_ALLOCATIONS: usize = 15;
    // The first owned batch no longer allocates a second target array.
    const MAX_BYTES: usize = 840_920 - TARGETS * std::mem::size_of::<UiTarget>();

    let config = Config::default();
    let palette = config.palette(Appearance::Dark);
    let screens = [Screen {
        bounds: Rect::new(0.0, 0.0, 4_000.0, 4_000.0),
        work_area: Rect::new(0.0, 0.0, 4_000.0, 4_000.0),
        is_primary: true,
        scale: 1.0,
        name: None,
    }];
    let context = HostContext {
        screens: &screens,
        cursor: Point::default(),
        focused_app: None,
        palette: &palette,
    };
    let targets = (0..TARGETS)
        .map(|index| UiTarget {
            rect: Rect::new(
                (index % 50) as f64 * 70.0,
                (index / 50) as f64 * 30.0,
                64.0,
                24.0,
            ),
            name: format!("Control {index} 设置"),
            role: "button".into(),
            native_role: Some("native button".into()),
        })
        .collect();
    let mut mode = keysteer::app::mode_catalog::hint(&config);
    black_box(mode.handle(&ModeEvent::Activated { previous: None }, &context));

    let region = Region::new(keysteer::TEST_ALLOCATOR);
    let commands = mode.handle_owned(
        ModeEvent::UiScanned(UiScanResult {
            id: 1,
            targets,
            status: UiScanStatus::Partial,
        }),
        &context,
    );
    black_box(&commands);
    let change = region.change();
    assert!(
        change.allocations <= MAX_ALLOCATIONS,
        "owned Hint delivery exceeded {MAX_ALLOCATIONS} allocations: {change:?}"
    );
    assert!(
        change.bytes_allocated <= MAX_BYTES,
        "owned Hint delivery exceeded {MAX_BYTES} allocated bytes: {change:?}"
    );
}
