//! Opt-in process lifecycle markers for end-to-end benchmark runners.

#[cfg(feature = "perf-probe")]
mod enabled {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    struct Probe {
        started: Instant,
        output: Mutex<BufWriter<File>>,
    }

    static PROBE: OnceLock<Option<Probe>> = OnceLock::new();

    pub(super) fn mark(event: &'static str) {
        let probe = PROBE.get_or_init(|| {
            let path = std::env::var_os("KEYSTEER_PERF_PROBE")?;
            let output = File::create(path).ok()?;
            Some(Probe {
                started: Instant::now(),
                output: Mutex::new(BufWriter::new(output)),
            })
        });
        let Some(probe) = probe else { return };
        let Ok(mut output) = probe.output.lock() else {
            return;
        };
        let _ = writeln!(
            output,
            "{{\"event\":\"{event}\",\"elapsed_ns\":{},\"pid\":{}}}",
            probe.started.elapsed().as_nanos(),
            std::process::id()
        );
        let _ = output.flush();
    }
}

#[inline]
pub(crate) fn mark(event: &'static str) {
    #[cfg(feature = "perf-probe")]
    enabled::mark(event);
    #[cfg(not(feature = "perf-probe"))]
    let _ = event;
}
