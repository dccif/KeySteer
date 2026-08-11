use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use keysteer::api::{Appearance, Binding, Direction, HostContext, KeyState, Mode};
use keysteer::modes::normal::NormalMode;
use keysteer::{Config, Key, ModeEvent, Point};

const SAMPLES: usize = 20_000;
const CALLS_PER_SAMPLE: usize = 100;

fn main() -> Result<(), String> {
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
