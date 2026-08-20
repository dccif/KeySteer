//! Opt-in process markers for end-to-end benchmark runners.

#[cfg(feature = "perf-probe")]
mod enabled {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::sync::Arc;
    #[cfg(target_os = "windows")]
    use std::sync::atomic::AtomicIsize;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, SyncSender, TrySendError};
    use std::sync::{Mutex, OnceLock};
    use std::thread::JoinHandle;
    use std::time::Instant;

    const EVENT_CAPACITY: usize = 32_768;
    const FLUSH_BATCH: usize = 256;

    struct Record {
        sequence: u64,
        event: &'static str,
        elapsed_ns: u128,
        correlation_id: Option<u64>,
        value: Option<isize>,
    }

    enum Message {
        Record(Record),
        Stop,
    }

    struct Probe {
        started: Instant,
        sender: SyncSender<Message>,
        writer: Mutex<Option<JoinHandle<()>>>,
        writer_error: Arc<Mutex<Option<String>>>,
        dropped: AtomicU64,
        reported_dropped: AtomicU64,
    }

    static PROBE: OnceLock<Option<Probe>> = OnceLock::new();
    static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    static NEXT_CORRELATION_ID: AtomicU64 = AtomicU64::new(1);

    fn write_record(output: &mut BufWriter<File>, record: &Record) -> std::io::Result<()> {
        write!(
            output,
            "{{\"sequence\":{},\"event\":\"{}\",\"elapsed_ns\":{}",
            record.sequence, record.event, record.elapsed_ns
        )?;
        if let Some(correlation_id) = record.correlation_id {
            write!(output, ",\"correlation_id\":{correlation_id}")?;
        }
        if let Some(value) = record.value {
            write!(output, ",\"value\":{value}")?;
        }
        writeln!(output, ",\"pid\":{}}}", std::process::id())
    }

    fn eager(event: &str) -> bool {
        matches!(
            event,
            "backend_started"
                | "engine_ready"
                | "hook_ready"
                | "uia_ready"
                | "ocr_ready"
                | "renderer_ready"
                | "probe_dropped"
                | "shutdown_complete"
        )
    }

    fn open() -> Option<Probe> {
        let path = std::env::var_os("KEYSTEER_PERF_PROBE")?;
        let output = BufWriter::new(File::create(path).ok()?);
        let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        let writer_error = Arc::new(Mutex::new(None));
        let thread_error = Arc::clone(&writer_error);
        let writer = std::thread::Builder::new()
            .name("keysteer-perf-probe".into())
            .spawn(move || {
                let mut output = output;
                let mut buffered = 0usize;
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Record(record) => {
                            if let Err(error) = write_record(&mut output, &record) {
                                if let Ok(mut stored) = thread_error.lock() {
                                    *stored = Some(error.to_string());
                                }
                                break;
                            }
                            buffered += 1;
                            if buffered >= FLUSH_BATCH || eager(record.event) {
                                if let Err(error) = output.flush() {
                                    if let Ok(mut stored) = thread_error.lock() {
                                        *stored = Some(error.to_string());
                                    }
                                    break;
                                }
                                buffered = 0;
                            }
                        }
                        Message::Stop => break,
                    }
                }
                if let Err(error) = output.flush()
                    && let Ok(mut stored) = thread_error.lock()
                {
                    *stored = Some(error.to_string());
                }
            })
            .ok()?;
        Some(Probe {
            started: Instant::now(),
            sender,
            writer: Mutex::new(Some(writer)),
            writer_error,
            dropped: AtomicU64::new(0),
            reported_dropped: AtomicU64::new(0),
        })
    }

    fn record(
        probe: &Probe,
        event: &'static str,
        correlation_id: Option<u64>,
        value: Option<isize>,
    ) -> Message {
        Message::Record(Record {
            sequence: NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            event,
            elapsed_ns: probe.started.elapsed().as_nanos(),
            correlation_id,
            value,
        })
    }

    fn report_recovered_drops(probe: &Probe) {
        let dropped = probe.dropped.load(Ordering::Acquire);
        if dropped == probe.reported_dropped.load(Ordering::Relaxed) {
            return;
        }
        let value = isize::try_from(dropped).unwrap_or(isize::MAX);
        if probe
            .sender
            .try_send(record(probe, "probe_dropped", None, Some(value)))
            .is_ok()
        {
            probe.reported_dropped.store(dropped, Ordering::Release);
        }
    }

    fn submit(event: &'static str, correlation_id: Option<u64>, value: Option<isize>) {
        let Some(probe) = PROBE.get_or_init(open) else {
            return;
        };
        let message = record(probe, event, correlation_id, value);
        if event == "shutdown_complete" {
            // Shutdown is outside every measured hot path. Use blocking sends
            // here so the final record and all earlier records are durable.
            let _ = probe.sender.send(message);
            let dropped =
                isize::try_from(probe.dropped.load(Ordering::Acquire)).unwrap_or(isize::MAX);
            let _ = probe
                .sender
                .send(record(probe, "probe_dropped", None, Some(dropped)));
            let _ = probe.sender.send(Message::Stop);
            if let Ok(mut writer) = probe.writer.lock()
                && let Some(writer) = writer.take()
                && writer.join().is_err()
                && let Ok(mut stored) = probe.writer_error.lock()
            {
                *stored = Some("probe writer panicked".into());
            }
            if let Ok(error) = probe.writer_error.lock()
                && let Some(error) = error.as_deref()
            {
                crate::report_error!("perf-probe", "{error}");
            }
            return;
        }
        report_recovered_drops(probe);
        match probe.sender.try_send(message) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                probe.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub(super) fn mark(event: &'static str) {
        submit(event, None, None);
    }

    pub(super) fn mark_correlated(event: &'static str, correlation_id: u64) {
        submit(event, Some(correlation_id), None);
    }

    pub(super) fn mark_correlated_value(event: &'static str, correlation_id: u64, value: isize) {
        submit(event, Some(correlation_id), Some(value));
    }

    pub(super) fn mark_value(event: &'static str, value: isize) {
        submit(event, None, Some(value));
    }

    pub(super) fn next_correlation_id() -> u64 {
        NEXT_CORRELATION_ID.fetch_add(1, Ordering::Relaxed)
    }

    #[cfg(target_os = "windows")]
    static COORDINATORS: AtomicIsize = AtomicIsize::new(0);
    #[cfg(target_os = "windows")]
    static PROVIDERS: AtomicIsize = AtomicIsize::new(0);
    #[cfg(target_os = "windows")]
    static BITMAPS: AtomicIsize = AtomicIsize::new(0);
    #[cfg(target_os = "windows")]
    static GDI_SURFACES: AtomicIsize = AtomicIsize::new(0);
    #[cfg(target_os = "windows")]
    static HELPERS: AtomicIsize = AtomicIsize::new(0);
    #[cfg(target_os = "windows")]
    static TEMP_FILES: AtomicIsize = AtomicIsize::new(0);

    #[cfg(target_os = "windows")]
    fn resource(kind: super::ResourceKind) -> (&'static AtomicIsize, &'static str) {
        match kind {
            super::ResourceKind::Coordinator => (&COORDINATORS, "resource_coordinator"),
            super::ResourceKind::Provider => (&PROVIDERS, "resource_provider"),
            super::ResourceKind::Bitmap => (&BITMAPS, "resource_bitmap"),
            super::ResourceKind::GdiSurface => (&GDI_SURFACES, "resource_gdi_surface"),
            super::ResourceKind::Helper => (&HELPERS, "resource_helper"),
            super::ResourceKind::TempFile => (&TEMP_FILES, "resource_temp_file"),
        }
    }

    #[cfg(target_os = "windows")]
    pub(super) fn acquire(kind: super::ResourceKind) {
        let (counter, event) = resource(kind);
        submit(
            event,
            None,
            Some(counter.fetch_add(1, Ordering::AcqRel) + 1),
        );
    }

    #[cfg(target_os = "windows")]
    pub(super) fn release(kind: super::ResourceKind) {
        let (counter, event) = resource(kind);
        submit(
            event,
            None,
            Some(counter.fetch_sub(1, Ordering::AcqRel) - 1),
        );
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
pub(crate) enum ResourceKind {
    Coordinator,
    Provider,
    Bitmap,
    GdiSurface,
    Helper,
    TempFile,
}

#[cfg(target_os = "windows")]
#[must_use = "the resource ledger guard must live for the owned resource"]
pub(crate) struct ResourceGuard {
    #[cfg(feature = "perf-probe")]
    kind: ResourceKind,
}

#[cfg(target_os = "windows")]
impl ResourceGuard {
    #[inline]
    pub(crate) fn new(kind: ResourceKind) -> Self {
        #[cfg(feature = "perf-probe")]
        enabled::acquire(kind);
        #[cfg(not(feature = "perf-probe"))]
        let _ = kind;
        Self {
            #[cfg(feature = "perf-probe")]
            kind,
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for ResourceGuard {
    #[inline]
    fn drop(&mut self) {
        #[cfg(feature = "perf-probe")]
        enabled::release(self.kind);
    }
}

#[inline]
pub(crate) fn mark(event: &'static str) {
    #[cfg(feature = "perf-probe")]
    enabled::mark(event);
    #[cfg(not(feature = "perf-probe"))]
    let _ = event;
}

#[inline]
pub(crate) fn mark_correlated(event: &'static str, correlation_id: u64) {
    #[cfg(feature = "perf-probe")]
    enabled::mark_correlated(event, correlation_id);
    #[cfg(not(feature = "perf-probe"))]
    let _ = (event, correlation_id);
}

#[inline]
pub(crate) fn mark_correlated_value(event: &'static str, correlation_id: u64, value: isize) {
    #[cfg(feature = "perf-probe")]
    enabled::mark_correlated_value(event, correlation_id, value);
    #[cfg(not(feature = "perf-probe"))]
    let _ = (event, correlation_id, value);
}

#[inline]
pub(crate) fn mark_value(event: &'static str, value: isize) {
    #[cfg(feature = "perf-probe")]
    enabled::mark_value(event, value);
    #[cfg(not(feature = "perf-probe"))]
    let _ = (event, value);
}

#[inline]
pub(crate) fn next_correlation_id() -> u64 {
    #[cfg(feature = "perf-probe")]
    {
        enabled::next_correlation_id()
    }
    #[cfg(not(feature = "perf-probe"))]
    {
        0
    }
}
