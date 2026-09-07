use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use keysteer::benchmark::{
    Appearance, Binding, Config, Direction, HostContext, Key, KeyState, Mode, ModeEvent, Point,
    Rect, Screen, UiScanResult, UiScanStatus, UiTarget,
};
const SAMPLES: usize = 20_000;
const CALLS_PER_SAMPLE: usize = 1_000;

#[path = "support/runtime.rs"]
mod runtime;

fn main() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--runtime") {
        return runtime::run();
    }
    benchmark_normal_frame()?;
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
    };

    let small_only = std::env::args().any(|arg| arg == "--hint-small");
    for targets in [24, 64, 100, 128, 500, 2_000]
        .into_iter()
        .filter(|targets| !small_only || *targets <= 64)
    {
        let mut samples = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            samples.push(measure_hint_owned_delivery(&config, &context, targets));
        }
        samples.sort_unstable();
        let p = |values: &[u128], percentile: usize| values[(values.len() - 1) * percentile / 100];
        println!(
            "hint_owned_delivery targets={targets} samples={SAMPLES} p50={}ns p95={}ns p99={}ns",
            p(&samples, 50),
            p(&samples, 95),
            p(&samples, 99),
        );
    }
    Ok(())
}

fn measure_hint_owned_delivery(config: &Config, context: &HostContext<'_>, count: usize) -> u128 {
    let values = hint_targets(count);
    let mut mode = keysteer::benchmark::hint(config);
    black_box(mode.handle(&ModeEvent::Activated { previous: None }, context));
    let started = Instant::now();
    black_box(mode.handle_owned(
        ModeEvent::UiScanned(UiScanResult {
            id: 1,
            targets: values,
            status: UiScanStatus::Partial,
        }),
        context,
    ));
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
    };
    let mut mode = keysteer::benchmark::normal(&config);
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
