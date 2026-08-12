use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use keysteer::api::{
    Appearance, Binding, Direction, HostContext, KeyState, LabelDirection, Mode, Rect,
};
use keysteer::domain::hints::assign_into;
use keysteer::modes::normal::NormalMode;
use keysteer::{Config, Key, ModeEvent, Point};

const SAMPLES: usize = 20_000;
const CALLS_PER_SAMPLE: usize = 100;

fn main() -> Result<(), String> {
    benchmark_normal_frame()?;
    benchmark_hint_assignment()?;
    Ok(())
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
