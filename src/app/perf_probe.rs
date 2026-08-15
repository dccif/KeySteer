//! Opt-in process markers for end-to-end benchmark runners.

#[cfg(feature = "perf-probe")]
mod enabled {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    #[cfg(target_os = "windows")]
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc::{self, SyncSender, TrySendError};
    use std::sync::{Mutex, OnceLock};
    use std::thread::JoinHandle;
    use std::time::Instant;

    const EVENT_CAPACITY: usize = 4_096;

    struct Record {
        event: &'static str,
        elapsed_ns: u128,
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
    }

    static PROBE: OnceLock<Option<Probe>> = OnceLock::new();

    fn open() -> Option<Probe> {
        let path = std::env::var_os("KEYSTEER_PERF_PROBE")?;
        let output = BufWriter::new(File::create(path).ok()?);
        let (sender, receiver) = mpsc::sync_channel(EVENT_CAPACITY);
        let writer = std::thread::Builder::new()
            .name("keysteer-perf-probe".into())
            .spawn(move || {
                let mut output = output;
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Record(record) => {
                            if let Some(value) = record.value {
                                let _ = writeln!(
                                    output,
                                    "{{\"event\":\"{}\",\"elapsed_ns\":{},\"value\":{},\"pid\":{}}}",
                                    record.event,
                                    record.elapsed_ns,
                                    value,
                                    std::process::id()
                                );
                            } else {
                                let _ = writeln!(
                                    output,
                                    "{{\"event\":\"{}\",\"elapsed_ns\":{},\"pid\":{}}}",
                                    record.event,
                                    record.elapsed_ns,
                                    std::process::id()
                                );
                            }
                            // The writer thread may flush eagerly so external
                            // runners can observe readiness without putting I/O
                            // on the measured engine or native callback thread.
                            let _ = output.flush();
                        }
                        Message::Stop => break,
                    }
                }
                let _ = output.flush();
            })
            .ok()?;
        Some(Probe {
            started: Instant::now(),
            sender,
            writer: Mutex::new(Some(writer)),
        })
    }

    fn submit(event: &'static str, value: Option<isize>) {
        let Some(probe) = PROBE.get_or_init(open) else {
            return;
        };
        let record = Message::Record(Record {
            event,
            elapsed_ns: probe.started.elapsed().as_nanos(),
            value,
        });
        if event == "shutdown_complete" {
            // Shutdown is outside every measured hot path. Use blocking sends
            // here so the final record and all earlier records are durable.
            let _ = probe.sender.send(record);
            let _ = probe.sender.send(Message::Stop);
            if let Ok(mut writer) = probe.writer.lock()
                && let Some(writer) = writer.take()
            {
                let _ = writer.join();
            }
            return;
        }
        match probe.sender.try_send(record) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                // A benchmark consumer that cannot drain 4096 fixed records
                // must not back-pressure input or rendering.
            }
        }
    }

    pub(super) fn mark(event: &'static str) {
        submit(event, None);
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
        submit(event, Some(counter.fetch_add(1, Ordering::AcqRel) + 1));
    }

    #[cfg(target_os = "windows")]
    pub(super) fn release(kind: super::ResourceKind) {
        let (counter, event) = resource(kind);
        submit(event, Some(counter.fetch_sub(1, Ordering::AcqRel) - 1));
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
