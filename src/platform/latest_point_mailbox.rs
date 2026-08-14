use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::api::geometry::Point;

/// Latest-only point transfer for one native producer and one engine consumer.
///
/// The sequence counter keeps the two `f64` bit patterns as one consistent
/// snapshot. `pending` coalesces a burst into one wake while retaining the
/// newest coordinates.
#[derive(Debug, Default)]
pub(crate) struct LatestPointMailbox {
    sequence: AtomicU64,
    x: AtomicU64,
    y: AtomicU64,
    pending: AtomicU64,
}

impl LatestPointMailbox {
    /// Store the latest point. Returns true only when the caller must publish
    /// a wake marker.
    pub(crate) fn publish(&self, point: Point) -> bool {
        // The macOS event tap is the sole producer. Release fences keep both
        // coordinate stores inside the odd sequence interval.
        self.sequence.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.x.store(point.x.to_bits(), Ordering::Relaxed);
        self.y.store(point.y.to_bits(), Ordering::Relaxed);
        fence(Ordering::Release);
        self.sequence.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Release) == 0
    }

    /// Consume the newest consistent point and allow a later producer to wake
    /// the engine again.
    pub(crate) fn take(&self) -> Option<Point> {
        if self.pending.swap(0, Ordering::Acquire) == 0 {
            return None;
        }
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let x = self.x.load(Ordering::Relaxed);
            let y = self.y.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            let after = self.sequence.load(Ordering::Relaxed);
            if before == after {
                return Some(Point::new(f64::from_bits(x), f64::from_bits(y)));
            }
        }
    }

    /// Undo a wake reservation after the bounded event queue rejected its
    /// marker. The producer is single-threaded, so no later publish can race
    /// this immediate rollback.
    pub(crate) fn cancel_signal(&self) {
        self.pending.store(0, Ordering::Release);
    }

    pub(crate) fn clear(&self) {
        self.pending.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_coalesces_and_keeps_the_latest_consistent_point() {
        let mailbox = LatestPointMailbox::default();
        assert!(mailbox.publish(Point::new(1.0, 2.0)));
        assert!(!mailbox.publish(Point::new(3.0, 4.0)));
        assert_eq!(mailbox.take(), Some(Point::new(3.0, 4.0)));
        assert_eq!(mailbox.take(), None);
        assert!(mailbox.publish(Point::new(5.0, 6.0)));
    }

    #[test]
    fn failed_signal_can_be_retried() {
        let mailbox = LatestPointMailbox::default();
        assert!(mailbox.publish(Point::new(1.0, 2.0)));
        mailbox.cancel_signal();
        assert!(mailbox.publish(Point::new(3.0, 4.0)));
        mailbox.clear();
        assert_eq!(mailbox.take(), None);
        assert!(mailbox.publish(Point::new(3.0, 4.0)));
        assert_eq!(mailbox.take(), Some(Point::new(3.0, 4.0)));
    }

    #[test]
    #[ignore = "microbenchmark probe; run in release with --test-threads=1"]
    fn latest_point_mailbox_performance_probe() {
        use std::sync::Mutex;
        use std::time::Instant;

        const WARMUP: usize = 2_000;
        const SAMPLES: usize = 20_000;
        const CALLS_PER_SAMPLE: usize = 100;

        fn percentiles(samples: &mut [u128]) -> (u128, u128, u128) {
            samples.sort_unstable();
            let last = samples.len() - 1;
            (
                samples[last * 50 / 100],
                samples[last * 95 / 100],
                samples[last * 99 / 100],
            )
        }

        let atomic = LatestPointMailbox::default();
        let locked = Mutex::new(None);
        let atomic_cycle = || {
            atomic.publish(Point::new(10.0, 20.0));
            std::hint::black_box(atomic.take());
        };
        let locked_cycle = || {
            *locked.lock().unwrap() = Some(Point::new(10.0, 20.0));
            std::hint::black_box(locked.lock().unwrap().take());
        };
        for _ in 0..WARMUP {
            atomic_cycle();
            locked_cycle();
        }
        let mut atomic_samples = Vec::with_capacity(SAMPLES);
        let mut locked_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let measure = |operation: &dyn Fn(), samples: &mut Vec<u128>| {
                let started = Instant::now();
                for _ in 0..CALLS_PER_SAMPLE {
                    operation();
                }
                samples.push(started.elapsed().as_nanos() / CALLS_PER_SAMPLE as u128);
            };
            if sample % 2 == 0 {
                measure(&atomic_cycle, &mut atomic_samples);
                measure(&locked_cycle, &mut locked_samples);
            } else {
                measure(&locked_cycle, &mut locked_samples);
                measure(&atomic_cycle, &mut atomic_samples);
            }
        }
        println!(
            "latest_point_mailbox_probe samples={SAMPLES} calls_per_sample={CALLS_PER_SAMPLE} atomic={:?} mutex={:?}",
            percentiles(&mut atomic_samples),
            percentiles(&mut locked_samples),
        );
    }

    #[test]
    fn concurrent_snapshots_never_mix_coordinate_pairs() {
        use std::sync::Arc;
        use std::thread;

        let mailbox = Arc::new(LatestPointMailbox::default());
        let producer = Arc::clone(&mailbox);
        let writer = thread::spawn(move || {
            for value in 1..=10_000 {
                let value = value as f64;
                producer.publish(Point::new(value, -value));
            }
        });
        while !writer.is_finished() {
            if let Some(point) = mailbox.take() {
                assert_eq!(point.x, -point.y);
            }
        }
        writer.join().unwrap();
        if let Some(point) = mailbox.take() {
            assert_eq!(point.x, -point.y);
        }
    }

    #[test]
    fn loom_model_never_observes_a_torn_pair_or_loses_the_latest_publish() {
        use loom::sync::Arc;
        use loom::sync::atomic::{AtomicU64, Ordering, fence};
        use loom::thread;

        struct ModelMailbox {
            sequence: AtomicU64,
            x: AtomicU64,
            y: AtomicU64,
            pending: AtomicU64,
        }

        impl ModelMailbox {
            fn new() -> Self {
                Self {
                    sequence: AtomicU64::new(0),
                    x: AtomicU64::new(0),
                    y: AtomicU64::new(0),
                    pending: AtomicU64::new(0),
                }
            }

            fn publish(&self, value: u64) {
                self.sequence.fetch_add(1, Ordering::Relaxed);
                fence(Ordering::Release);
                self.x.store(value, Ordering::Relaxed);
                self.y.store(!value, Ordering::Relaxed);
                fence(Ordering::Release);
                self.sequence.fetch_add(1, Ordering::Relaxed);
                self.pending.fetch_add(1, Ordering::Release);
            }

            fn take(&self) -> Option<(u64, u64)> {
                if self.pending.swap(0, Ordering::Acquire) == 0 {
                    return None;
                }
                loop {
                    let before = self.sequence.load(Ordering::Acquire);
                    if before & 1 != 0 {
                        thread::yield_now();
                        continue;
                    }
                    let x = self.x.load(Ordering::Relaxed);
                    let y = self.y.load(Ordering::Relaxed);
                    fence(Ordering::Acquire);
                    let after = self.sequence.load(Ordering::Relaxed);
                    if before == after {
                        return Some((x, y));
                    }
                }
            }
        }

        loom::model(|| {
            let mailbox = Arc::new(ModelMailbox::new());
            mailbox.publish(1);
            let writer_mailbox = Arc::clone(&mailbox);
            let writer = thread::spawn(move || writer_mailbox.publish(2));
            let reader_mailbox = Arc::clone(&mailbox);
            let reader = thread::spawn(move || reader_mailbox.take());

            let first = reader.join().unwrap();
            writer.join().unwrap();
            let second = mailbox.take();
            let mut saw_latest = false;
            for (x, y) in [first, second].into_iter().flatten() {
                assert_eq!(x, !y);
                saw_latest |= x == 2;
            }
            assert!(saw_latest);
        });
    }
}
