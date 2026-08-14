use std::alloc::System;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use keysteer::api::{
    Appearance, Binding, Direction, HostContext, KeyState, LabelDirection, Mode, Rect, Screen,
    UiScanResult, UiScanStatus, UiTarget,
};
use keysteer::domain::hints::assign_into;
use keysteer::modes::hint::HintMode;
use keysteer::modes::normal::NormalMode;
use keysteer::{Config, Key, ModeEvent, Point};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const SAMPLES: usize = 20_000;
const CALLS_PER_SAMPLE: usize = 100;

fn main() -> Result<(), String> {
    benchmark_normal_frame()?;
    benchmark_hint_assignment()?;
    benchmark_hint_owned_delivery()?;
    Ok(())
}

fn benchmark_hint_owned_delivery() -> Result<(), String> {
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
        config: &config,
    };

    for targets in [24, 64, 100, 128, 500, 2_000] {
        let samples = if targets <= 128 { 5_000 } else { 500 };
        let mut owned = Vec::with_capacity(samples);
        let mut borrowed = Vec::with_capacity(samples);
        for sample in 0..samples {
            if sample % 2 == 0 {
                owned.push(measure_hint_delivery(&config, &context, targets, true));
                borrowed.push(measure_hint_delivery(&config, &context, targets, false));
            } else {
                borrowed.push(measure_hint_delivery(&config, &context, targets, false));
                owned.push(measure_hint_delivery(&config, &context, targets, true));
            }
        }
        owned.sort_unstable();
        borrowed.sort_unstable();
        let p = |values: &[u128], percentile: usize| values[(values.len() - 1) * percentile / 100];
        println!(
            "hint_delivery targets={targets} samples={samples} owned_p50={}ns owned_p95={}ns owned_p99={}ns borrowed_p50={}ns borrowed_p95={}ns borrowed_p99={}ns",
            p(&owned, 50),
            p(&owned, 95),
            p(&owned, 99),
            p(&borrowed, 50),
            p(&borrowed, 95),
            p(&borrowed, 99),
        );
        let (owned_allocations, owned_bytes) =
            measure_hint_delivery_allocations(&config, &context, targets, true);
        let (borrowed_allocations, borrowed_bytes) =
            measure_hint_delivery_allocations(&config, &context, targets, false);
        println!(
            "hint_delivery_alloc targets={targets} owned_allocations={owned_allocations} owned_bytes={owned_bytes} borrowed_allocations={borrowed_allocations} borrowed_bytes={borrowed_bytes}"
        );
    }
    Ok(())
}

fn measure_hint_delivery_allocations(
    config: &Config,
    context: &HostContext<'_>,
    count: usize,
    owned: bool,
) -> (usize, usize) {
    let values = hint_targets(count);
    let mut mode = HintMode::new(config);
    black_box(mode.handle(&ModeEvent::Activated { previous: None }, context));
    let region = Region::new(GLOBAL);
    if owned {
        black_box(mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: 1,
                targets: values,
                status: UiScanStatus::Partial,
            }),
            context,
        ));
    } else {
        let event = ModeEvent::UiScanned(UiScanResult {
            id: 1,
            targets: values,
            status: UiScanStatus::Partial,
        });
        black_box(mode.handle(&event, context));
    }
    let change = region.change();
    (change.allocations, change.bytes_allocated)
}

fn measure_hint_delivery(
    config: &Config,
    context: &HostContext<'_>,
    count: usize,
    owned: bool,
) -> u128 {
    let values = hint_targets(count);
    let mut mode = HintMode::new(config);
    black_box(mode.handle(&ModeEvent::Activated { previous: None }, context));
    let started = Instant::now();
    if owned {
        black_box(mode.handle_owned(
            ModeEvent::UiScanned(UiScanResult {
                id: 1,
                targets: values,
                status: UiScanStatus::Partial,
            }),
            context,
        ));
    } else {
        let event = ModeEvent::UiScanned(UiScanResult {
            id: 1,
            targets: values,
            status: UiScanStatus::Partial,
        });
        black_box(mode.handle(&event, context));
    }
    started.elapsed().as_nanos()
}

fn hint_targets(count: usize) -> Vec<UiTarget> {
    (0..count)
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
        .collect()
}

fn benchmark_normal_frame() -> Result<(), String> {
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
    let _ = mode.handle(
        &ModeEvent::Binding {
            binding: Arc::new(Binding::Move(Direction::Right)),
            state: KeyState::Down,
            key: Key::new("l")?,
        },
        &context,
    );

    for _ in 0..10_000 {
        black_box(mode.handle(
            &ModeEvent::Frame {
                elapsed: Duration::from_micros(8_333),
            },
            &context,
        ));
    }

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..CALLS_PER_SAMPLE {
            black_box(mode.handle(
                &ModeEvent::Frame {
                    elapsed: Duration::from_micros(8_333),
                },
                &context,
            ));
        }
        samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
    }
    samples.sort_unstable();
    let percentile = |numerator: usize| samples[(SAMPLES - 1) * numerator / 100];
    println!(
        "normal_frame samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} p50={}ns p95={}ns p99={}ns",
        percentile(50),
        percentile(95),
        percentile(99)
    );
    Ok(())
}

fn benchmark_hint_assignment() -> Result<(), String> {
    const TARGETS: usize = 2_000;
    const HINT_SAMPLES: usize = 10_000;
    let alphabet: Vec<char> = "arstneioqwfpjluy".chars().collect();
    let targets = (0..TARGETS).map(|index| {
        (
            Rect::new((index % 100) as f64, (index / 100) as f64, 8.0, 8.0),
            index,
        )
    });
    let mut output = Vec::new();
    for _ in 0..100 {
        assign_into(
            &mut output,
            targets.clone(),
            &alphabet,
            LabelDirection::Normal,
        )?;
    }
    let mut samples = Vec::with_capacity(HINT_SAMPLES);
    for _ in 0..HINT_SAMPLES {
        let started = Instant::now();
        assign_into(
            &mut output,
            targets.clone(),
            &alphabet,
            LabelDirection::Normal,
        )?;
        black_box(&output);
        samples.push(started.elapsed().as_nanos());
    }
    samples.sort_unstable();
    let percentile = |numerator: usize| samples[(HINT_SAMPLES - 1) * numerator / 100];
    println!(
        "hint_assign targets={TARGETS} samples={HINT_SAMPLES} p50={}ns p95={}ns p99={}ns",
        percentile(50),
        percentile(95),
        percentile(99)
    );
    Ok(())
}
