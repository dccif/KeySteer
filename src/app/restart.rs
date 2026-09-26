//! Validated configuration handoff to a fresh process. No runtime state is transferred.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use super::runtime::PreparedRestart;

pub(crate) const ARGUMENT: &str = "--internal-reload";

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Snapshot {
    pub source: String,
    pub write_path: Option<PathBuf>,
    pub loaded_from_file: bool,
    pub discovery_directory: Option<PathBuf>,
}

pub(crate) fn receive() -> Result<Snapshot, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid reload handoff: {error}"))
}

struct Replacement {
    child: Child,
    payload: Vec<u8>,
    committed: bool,
}

pub(crate) fn prepare(snapshot: Snapshot) -> Result<Box<dyn PreparedRestart>, String> {
    let payload = serde_json::to_vec(&snapshot).map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command
        .arg(ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(Box::new(spawn(&mut command, payload)?))
}

fn spawn(command: &mut Command, payload: Vec<u8>) -> Result<Replacement, String> {
    let child = command
        .spawn()
        .map_err(|error| format!("cannot prepare reload process: {error}"))?;
    Ok(Replacement {
        child,
        payload,
        committed: false,
    })
}

impl PreparedRestart for Replacement {
    fn commit(mut self: Box<Self>) -> Result<(), String> {
        let mut input = self
            .child
            .stdin
            .take()
            .ok_or("reload handoff pipe unavailable")?;
        input
            .write_all(&self.payload)
            .map_err(|error| format!("cannot start reload process: {error}"))?;
        // EOF is the commit signal. The replacement cannot initialize any native
        // resources until the old runtime and backend have both been dropped.
        drop(input);
        self.committed = true;
        Ok(())
    }
}

impl Drop for Replacement {
    fn drop(&mut self) {
        if !self.committed {
            // Kill before closing stdin: an aborted handoff must not start up.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    #[ignore = "subprocess entry point for reload pipe tests"]
    fn reload_receiver_probe() {
        if std::env::var_os("KEYSTEER_RELOAD_TEST_PROBE").is_none() {
            return;
        }
        println!("reload-ready");
        std::io::stdout().flush().unwrap();
        let snapshot = receive().unwrap();
        assert_eq!(snapshot.source, "[pointer]\ninitial_speed = 456\n");
        assert_eq!(
            snapshot.write_path,
            Some(PathBuf::from("keysteer.explicit.toml"))
        );
        assert!(snapshot.discovery_directory.is_none());
        assert!(snapshot.loaded_from_file);
        println!("reload-received");
    }

    fn probe() -> (
        Box<Replacement>,
        mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        let snapshot = Snapshot {
            source: "[pointer]\ninitial_speed = 456\n".into(),
            write_path: Some("keysteer.explicit.toml".into()),
            loaded_from_file: true,
            discovery_directory: None,
        };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env("KEYSTEER_RELOAD_TEST_PROBE", "1");
        command
            .args([
                "--ignored",
                "--exact",
                "app::restart::tests::reload_receiver_probe",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut replacement =
            Box::new(spawn(&mut command, serde_json::to_vec(&snapshot).unwrap()).unwrap());
        let output = replacement.child.stdout.take().unwrap();
        let (sender, received) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if line.starts_with("reload-") {
                    let _ = sender.send(line);
                }
            }
        });
        assert_eq!(
            received.recv_timeout(Duration::from_secs(10)).unwrap(),
            "reload-ready"
        );
        (replacement, received, reader)
    }

    #[test]
    fn reload_process_waits_for_commit_then_receives_exact_snapshot() {
        let (replacement, received, reader) = probe();
        assert!(received.try_recv().is_err());
        replacement.commit().unwrap();
        assert_eq!(
            received.recv_timeout(Duration::from_secs(10)).unwrap(),
            "reload-received"
        );
        reader.join().unwrap();
    }

    #[test]
    fn abandoned_reload_kills_waiting_process_without_starting_it() {
        let (replacement, received, reader) = probe();
        drop(replacement);
        assert!(received.recv_timeout(Duration::from_secs(10)).is_err());
        reader.join().unwrap();
    }
}
