//! Opt-in process markers for end-to-end benchmark runners.

#[cfg(feature = "perf-probe")]
mod enabled {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::sync::mpsc::{self, SyncSender, TrySendError};
    use std::sync::{Mutex, OnceLock};
    use std::thread::JoinHandle;
    use std::time::Instant;

    const EVENT_CAPACITY: usize = 4_096;

    struct Record {
        event: &'static str,
        elapsed_ns: u128,
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
                            let _ = writeln!(
                                output,
                                "{{\"event\":\"{}\",\"elapsed_ns\":{},\"pid\":{}}}",
                                record.event,
                                record.elapsed_ns,
                                std::process::id()
                            );
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

    pub(super) fn mark(event: &'static str) {
        let Some(probe) = PROBE.get_or_init(open) else {
            return;
        };
        let record = Message::Record(Record {
            event,
            elapsed_ns: probe.started.elapsed().as_nanos(),
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
}

#[inline]
pub(crate) fn mark(event: &'static str) {
    #[cfg(feature = "perf-probe")]
    enabled::mark(event);
    #[cfg(not(feature = "perf-probe"))]
    let _ = event;
}
