//! Signed, transactional Windows self-update support.
//!
//! The running process never overwrites itself. It stages a verified candidate
//! on the install volume, launches a signed temporary copy as a helper, then
//! requests the normal engine shutdown. The helper atomically swaps the file,
//! waits for the new backend to signal readiness, and rolls back on failure.

mod candidate;
mod signature;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, EVENT_MODIFY_STATE, OpenEventW, OpenProcess,
    PROCESS_SYNCHRONIZE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
use windows::core::PCWSTR;

const INTERNAL_ARG: &str = "--internal-apply-update";
const READY_EVENT_ENV: &str = "KEYSTEER_UPDATE_READY_EVENT";
const HELPER_WAIT: Duration = Duration::from_secs(30);
const READY_WAIT: Duration = Duration::from_secs(15);
const ARM_WAIT: Duration = Duration::from_secs(5);
const TEMP_DIRECTORY: &str = "KeySteer";
const HELPER_PREFIX: &str = "update-helper-";
static NONCE: AtomicU64 = AtomicU64::new(0);

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns a successful Win32 handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn prepare_and_launch(download_path: &Path, latest: &str) -> Result<(), String> {
    let latest = Version::parse(latest)
        .map_err(|error| format!("release version {latest:?} is invalid: {error}"))?;
    let target = std::env::current_exe()
        .map_err(|error| format!("cannot locate the running KeySteer executable: {error}"))?;
    let install_dir = target
        .parent()
        .ok_or_else(|| format!("{} has no install directory", target.display()))?;
    let nonce = unique_nonce();
    let staged = install_dir.join(format!(".keysteer-update-{nonce}.exe"));
    let backup = install_dir.join(format!(".keysteer-backup-{nonce}.exe"));
    let temp_dir = std::env::temp_dir().join(TEMP_DIRECTORY);
    fs::create_dir_all(&temp_dir).map_err(|error| {
        format!(
            "cannot create the KeySteer update helper directory {}: {error}",
            temp_dir.display()
        )
    })?;
    cleanup_old_helpers(&temp_dir);
    let helper = temp_dir.join(format!("{HELPER_PREFIX}{nonce}.exe"));

    let result = (|| {
        let current_signer = signature::verified_signer(&target).map_err(|error| {
            format!(
                "Automatic installation requires the running KeySteer executable to have a valid code signature. {error}"
            )
        })?;
        candidate::validate_machine(download_path)?;
        candidate::validate_version(download_path, &latest)?;
        let downloaded_signer = signature::verified_signer(download_path)?;
        if downloaded_signer != current_signer {
            return Err(
                "the downloaded update is signed by a different publisher certificate".into(),
            );
        }
        candidate::stage(download_path, &staged)?;
        candidate::validate_machine(&staged)?;
        candidate::validate_version(&staged, &latest)?;
        let candidate_signer = signature::verified_signer(&staged)?;
        if candidate_signer != current_signer {
            return Err(
                "the downloaded update is signed by a different publisher certificate".into(),
            );
        }
        fs::copy(&target, &helper).map_err(|error| {
            format!(
                "cannot create temporary signed update helper {}: {error}",
                helper.display()
            )
        })?;
        launch_helper(&helper, &target, &staged, &backup, download_path, &latest)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
        let _ = fs::remove_file(&helper);
    }
    result
}

fn launch_helper(
    helper: &Path,
    target: &Path,
    staged: &Path,
    backup: &Path,
    download: &Path,
    latest: &Version,
) -> Result<(), String> {
    let armed_name = format!("Local\\KeySteerUpdateArmed-{}", unique_nonce());
    let armed_wide = wide(&armed_name);
    // SAFETY: the event name is NUL-terminated and live for the call.
    let armed = OwnedHandle(
        unsafe { CreateEventW(None, true, false, PCWSTR(armed_wide.as_ptr())) }
            .map_err(|error| format!("cannot create update-helper handshake: {error}"))?,
    );
    let mut command = Command::new(helper);
    command
        .arg(INTERNAL_ARG)
        .arg(std::process::id().to_string())
        .arg(&armed_name)
        .arg(target)
        .arg(staged)
        .arg(backup)
        .arg(download)
        .arg(latest.to_string())
        .arg("--")
        .args(std::env::args_os().skip(1))
        .creation_flags(CREATE_NO_WINDOW.0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot launch the update helper: {error}"))?;
    // SAFETY: the owned event remains live while the helper opens the current
    // process and signals that it is safe for the engine to begin shutdown.
    let wait = unsafe { WaitForSingleObject(armed.0, millis(ARM_WAIT)) };
    if wait != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        return if wait == WAIT_TIMEOUT {
            Err("update helper did not become ready; KeySteer was not changed".into())
        } else {
            Err(format!(
                "update-helper handshake failed (wait code {})",
                wait.0
            ))
        };
    }
    Ok(())
}

pub(crate) fn run_internal_helper() -> Option<Result<(), String>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(INTERNAL_ARG)) {
        return None;
    }
    let result = parse_helper_args(&mut args).and_then(apply_update);
    if let Err(error) = &result {
        show_helper_error(error);
    }
    Some(result)
}

struct HelperPlan {
    parent_pid: u32,
    armed_event: OsString,
    target: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    download: PathBuf,
    latest: Version,
    restart_args: Vec<OsString>,
}

fn parse_helper_args(args: &mut impl Iterator<Item = OsString>) -> Result<HelperPlan, String> {
    let parent_pid = required(args, "parent process id")?
        .to_string_lossy()
        .parse::<u32>()
        .map_err(|_| "update helper received an invalid parent process id".to_string())?;
    let armed_event = required(args, "armed event")?;
    let target = PathBuf::from(required(args, "target executable")?);
    let staged = PathBuf::from(required(args, "staged executable")?);
    let backup = PathBuf::from(required(args, "backup executable")?);
    let download = PathBuf::from(required(args, "downloaded update")?);
    let latest = required(args, "release version")?
        .to_string_lossy()
        .parse::<Version>()
        .map_err(|error| format!("update helper received an invalid release version: {error}"))?;
    if required(args, "argument separator")? != OsStr::new("--") {
        return Err("update helper received an invalid argument separator".into());
    }
    Ok(HelperPlan {
        parent_pid,
        armed_event,
        target,
        staged,
        backup,
        download,
        latest,
        restart_args: args.collect(),
    })
}

fn required(args: &mut impl Iterator<Item = OsString>, name: &str) -> Result<OsString, String> {
    args.next()
        .ok_or_else(|| format!("update helper is missing {name}"))
}

fn apply_update(plan: HelperPlan) -> Result<(), String> {
    // SAFETY: access is limited to waiting for the exact parent PID supplied
    // by the running process; the handle is closed by OwnedHandle.
    let parent = OwnedHandle(
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, plan.parent_pid) }
            .map_err(|error| format!("cannot open the running KeySteer process: {error}"))?,
    );
    signal_named_event(&plan.armed_event)?;
    // SAFETY: parent is a live process handle held until the wait completes.
    let wait = unsafe { WaitForSingleObject(parent.0, millis(HELPER_WAIT)) };
    if wait != WAIT_OBJECT_0 {
        return if wait == WAIT_TIMEOUT {
            Err("KeySteer did not finish its orderly shutdown; the update was not installed".into())
        } else {
            Err(format!(
                "waiting for KeySteer shutdown failed (wait code {})",
                wait.0
            ))
        };
    }
    drop(parent);

    validate_helper_plan(&plan)?;

    replace_file(&plan.target, &plan.staged, &plan.backup).map_err(|error| {
        format!(
            "cannot replace {} with the downloaded update: {error}",
            plan.target.display()
        )
    })?;

    match start_and_wait_ready(&plan) {
        Ok(()) => {
            let _ = fs::remove_file(&plan.backup);
            let _ = fs::remove_file(&plan.download);
            Ok(())
        }
        Err(start_error) => {
            let failed = plan
                .target
                .with_file_name(format!(".keysteer-failed-{}.exe", unique_nonce()));
            replace_file(&plan.target, &plan.backup, &failed).map_err(|rollback_error| {
                format!(
                    "{start_error}\n\nRollback also failed: {rollback_error}\nThe previous executable is preserved at {}.",
                    plan.backup.display()
                )
            })?;
            let _ = fs::remove_file(failed);
            Command::new(&plan.target)
                .args(&plan.restart_args)
                .spawn()
                .map_err(|error| {
                    format!(
                        "{start_error}\n\nThe previous version was restored but could not be restarted: {error}"
                    )
                })?;
            Err(format!(
                "{start_error}\n\nThe previous KeySteer version was restored and restarted."
            ))
        }
    }
}

fn validate_helper_plan(plan: &HelperPlan) -> Result<(), String> {
    let helper = std::env::current_exe()
        .map_err(|error| format!("cannot locate the running update helper: {error}"))?;
    let helper_signer = signature::verified_signer(&helper)?;
    let current_signer = signature::verified_signer(&plan.target)?;
    let candidate_signer = signature::verified_signer(&plan.staged)?;
    if current_signer != helper_signer || candidate_signer != helper_signer {
        return Err(
            "update helper, installed executable and staged executable are not signed by the same certificate"
                .into(),
        );
    }
    candidate::validate_machine(&plan.staged)?;
    candidate::validate_version(&plan.staged, &plan.latest)
}

fn start_and_wait_ready(plan: &HelperPlan) -> Result<(), String> {
    let ready_name = format!("Local\\KeySteerUpdateReady-{}", unique_nonce());
    let ready_wide = wide(&ready_name);
    // SAFETY: the event name is NUL-terminated and live for the call.
    let ready = OwnedHandle(
        unsafe { CreateEventW(None, true, false, PCWSTR(ready_wide.as_ptr())) }
            .map_err(|error| format!("cannot create update readiness event: {error}"))?,
    );
    let mut child = Command::new(&plan.target)
        .args(&plan.restart_args)
        .env(READY_EVENT_ENV, &ready_name)
        .spawn()
        .map_err(|error| format!("updated KeySteer could not be launched: {error}"))?;
    let process = HANDLE(child.as_raw_handle());
    // SAFETY: the event is owned above and Child owns a live process handle for
    // the complete wait. Waiting does not transfer ownership of either handle.
    let wait = unsafe { WaitForMultipleObjects(&[ready.0, process], false, millis(READY_WAIT)) };
    if wait == WAIT_OBJECT_0 {
        return Ok(());
    }
    let _ = child.kill();
    let _ = child.wait();
    if wait == WAIT_TIMEOUT {
        Err("updated KeySteer did not report a successful backend startup in time".into())
    } else if wait.0 == WAIT_OBJECT_0.0 + 1 {
        Err("updated KeySteer exited before its backend became ready".into())
    } else {
        Err(format!(
            "waiting for updated KeySteer failed (wait code {})",
            wait.0
        ))
    }
}

pub(super) fn signal_update_ready() -> Result<(), String> {
    let Some(name) = std::env::var_os(READY_EVENT_ENV) else {
        return Ok(());
    };
    signal_named_event(&name)
        .map_err(|error| format!("cannot acknowledge successful update startup: {error}"))
}

fn signal_named_event(name: &OsStr) -> Result<(), String> {
    let wide: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
    // SAFETY: event name is NUL-terminated. The returned handle is uniquely
    // owned and used only to set the event before it is closed.
    let event = OwnedHandle(
        unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(wide.as_ptr())) }
            .map_err(|error| format!("cannot open update handshake event: {error}"))?,
    );
    // SAFETY: event is a valid event handle with EVENT_MODIFY_STATE access.
    unsafe { SetEvent(event.0) }
        .map_err(|error| format!("cannot signal update handshake event: {error}"))
}

fn replace_file(target: &Path, replacement: &Path, backup: &Path) -> Result<(), String> {
    let target = wide_path(target);
    let replacement = wide_path(replacement);
    let backup = wide_path(backup);
    // SAFETY: all path buffers are NUL-terminated and live for the synchronous
    // operation. Replacement and backup were staged on the target volume.
    unsafe {
        ReplaceFileW(
            PCWSTR(target.as_ptr()),
            PCWSTR(replacement.as_ptr()),
            PCWSTR(backup.as_ptr()),
            REPLACEFILE_WRITE_THROUGH,
            None,
            None,
        )
    }
    .map_err(|error| error.to_string())
}

fn cleanup_old_helpers(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(24 * 60 * 60))
        .unwrap_or(UNIX_EPOCH);
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(HELPER_PREFIX) && name.ends_with(".exe"))
        {
            continue;
        }
        if entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < cutoff)
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn show_helper_error(error: &str) {
    let title = wide("KeySteer Update Failed");
    let message = wide(error);
    // SAFETY: both strings are NUL-terminated and remain live for the modal
    // call. A null owner is appropriate because the helper has no window.
    let _ = unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        )
    };
}

fn unique_nonce() -> u64 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    time ^ (u64::from(std::process::id()) << 32) ^ NONCE.fetch_add(1, Ordering::Relaxed)
}

fn millis(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX)) as u32
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_arguments_preserve_restart_arguments() {
        let mut values = [
            OsString::from("42"),
            OsString::from("Local\\armed"),
            OsString::from("C:\\KeySteer.exe"),
            OsString::from("C:\\staged.exe"),
            OsString::from("C:\\backup.exe"),
            OsString::from("C:\\update.exe"),
            OsString::from("0.9.13"),
            OsString::from("--"),
            OsString::from("--config"),
            OsString::from("C:\\配置\\keysteer.work.toml"),
        ]
        .into_iter();
        let plan = parse_helper_args(&mut values);
        assert!(plan.is_ok());
        let plan = plan.unwrap_or_else(|error| panic!("unexpected parse failure: {error}"));
        assert_eq!(plan.parent_pid, 42);
        assert_eq!(plan.latest, Version::new(0, 9, 13));
        assert_eq!(plan.restart_args.len(), 2);
        assert_eq!(
            plan.restart_args[1],
            OsStr::new("C:\\配置\\keysteer.work.toml")
        );
    }

    #[test]
    fn replacement_keeps_a_recoverable_backup() -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!(
            "keysteer-replace-test-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir(&directory)?;
        let target = directory.join("KeySteer.exe");
        let candidate = directory.join("candidate.exe");
        let backup = directory.join("backup.exe");
        fs::write(&target, b"old")?;
        fs::write(&candidate, b"new")?;

        replace_file(&target, &candidate, &backup)?;

        assert_eq!(fs::read(&target)?, b"new");
        assert_eq!(fs::read(&backup)?, b"old");
        assert!(!candidate.exists());
        fs::remove_file(target)?;
        fs::remove_file(backup)?;
        fs::remove_dir(directory)?;
        Ok(())
    }
}
