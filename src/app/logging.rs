//! Dependency-free diagnostic logging.
//!
//! Errors and panics are always persisted because the normal app may have no
//! console. Debug, info and warning records are emitted only while logging is
//! enabled by the active configuration. Files are rotated at a fixed bound.

use std::backtrace::Backtrace;
use std::fmt;
use std::fs::{self, File, OpenOptions};
#[cfg(target_os = "macos")]
use std::io::IsTerminal;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE: &str = "keysteer.log";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const RETAINED_LOGS: usize = 3;

static LOGGER: OnceLock<Logger> = OnceLock::new();
static PANIC_HOOK: Once = Once::new();
static NON_ERROR_ENABLED: AtomicBool = AtomicBool::new(false);
static SESSION_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warning => "WARN",
            Self::Error => "ERROR",
        })
    }
}

struct LoggerState {
    file: Option<File>,
    bytes: u64,
}

struct Logger {
    path: PathBuf,
    state: Mutex<LoggerState>,
}

impl Logger {
    fn open(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        rotate_before_open(&path)?;
        let file = open_append(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            path,
            state: Mutex::new(LoggerState {
                file: Some(file),
                bytes,
            }),
        })
    }

    fn write(&self, level: Level, target: &str, message: &str) {
        let line = format_line(level, target, message);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.file.is_none() {
            match open_append(&self.path) {
                Ok(file) => {
                    state.bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                    state.file = Some(file);
                }
                Err(error) => {
                    write_emergency_stderr(format_args!(
                        "cannot reopen diagnostic log {}: {error}",
                        self.path.display()
                    ));
                    return;
                }
            }
        }
        if state.bytes.saturating_add(line.len() as u64 + 1) > MAX_LOG_BYTES
            && let Err(error) = self.rotate(&mut state)
        {
            write_emergency_stderr(format_args!(
                "cannot rotate diagnostic log {}: {error}",
                self.path.display()
            ));
        }
        let write_result = match state.file.as_mut() {
            Some(file) => {
                let result = writeln!(file, "{line}");
                if result.is_ok()
                    && level == Level::Error
                    && let Err(error) = file.flush()
                {
                    write_emergency_stderr(format_args!(
                        "cannot flush diagnostic log {}: {error}",
                        self.path.display()
                    ));
                }
                result
            }
            None => return,
        };
        if let Err(error) = write_result {
            write_emergency_stderr(format_args!(
                "cannot write diagnostic log {}: {error}",
                self.path.display()
            ));
            state.file = None;
            return;
        }
        state.bytes = state.bytes.saturating_add(line.len() as u64 + 1);
    }

    fn rotate(&self, state: &mut LoggerState) -> io::Result<()> {
        if let Some(file) = state.file.as_mut() {
            file.flush()?;
        }
        drop(state.file.take());
        let rotation = rotate_files(&self.path);
        match open_append(&self.path) {
            Ok(file) => {
                state.bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                state.file = Some(file);
            }
            Err(reopen_error) => {
                return match rotation {
                    Ok(()) => Err(reopen_error),
                    Err(rotation_error) => Err(io::Error::new(
                        reopen_error.kind(),
                        format!(
                            "rotation failed ({rotation_error}); reopening failed ({reopen_error})"
                        ),
                    )),
                };
            }
        }
        rotation
    }

    fn flush(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(file) = state.file.as_mut()
            && let Err(error) = file.flush()
        {
            write_emergency_stderr(format_args!(
                "cannot flush diagnostic log {}: {error}",
                self.path.display()
            ));
        }
    }
}

/// Initialise the logger in the active application data directory, falling
/// back to a process-writable temporary directory when necessary.
///
/// Startup treats failure as non-fatal: a read-only program directory should
/// disable logging, not prevent the tray application from running.
pub fn init() -> Result<&'static Path, String> {
    if let Some(logger) = LOGGER.get() {
        return Ok(&logger.path);
    }
    let mut failures = Vec::new();
    for path in candidate_paths() {
        match Logger::open(path.clone()) {
            Ok(logger) => {
                if LOGGER.set(logger).is_ok() {
                    let Some(installed) = LOGGER.get() else {
                        return Err("logger installation completed without a logger".into());
                    };
                    let path = &installed.path;
                    if !failures.is_empty() {
                        report_error(
                            "logging",
                            format!(
                                "preferred diagnostic log was unavailable; using {}: {}",
                                path.display(),
                                failures.join("; ")
                            ),
                        );
                    }
                    return Ok(path);
                }
                return LOGGER
                    .get()
                    .map(|logger| logger.path.as_path())
                    .ok_or_else(|| "logger installation race produced no logger".into());
            }
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    Err(format!(
        "cannot create a diagnostic log ({})",
        failures.join("; ")
    ))
}

pub fn path() -> Option<&'static Path> {
    LOGGER.get().map(|logger| logger.path.as_path())
}

/// Enable or disable every non-error diagnostic level.
///
/// Errors deliberately bypass this switch. `Relaxed` ordering is sufficient:
/// the value controls observability only and does not publish program state.
pub fn set_non_error_enabled(enabled: bool) {
    NON_ERROR_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Start a diagnostic session after configuration has enabled non-error logs.
pub(crate) fn start_session() {
    if !level_enabled(Level::Info)
        || LOGGER.get().is_none()
        || SESSION_STARTED.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let log_path = path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<unavailable>".into());
    info(
        "session",
        format!(
            "session started version={} pid={} os={} arch={} profile={} backend={} executable={} log={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            crate::platform::backend_name(),
            std::env::current_exe()
                .map(|value| value.display().to_string())
                .unwrap_or_else(|error| format!("<unavailable: {error}>")),
            log_path,
        ),
    );
}

pub(crate) fn end_session() {
    if SESSION_STARTED.swap(false, Ordering::AcqRel) {
        info("session", "session ended normally");
    }
}

/// Lazy variant used by callers that would otherwise build a formatted
/// message before discovering that non-error logging is disabled.
pub(crate) fn debug_args(target: &str, message: fmt::Arguments<'_>) {
    if level_enabled(Level::Debug) {
        log_args(Level::Debug, target, message);
    }
}

pub fn info(target: &str, message: impl AsRef<str>) {
    log(Level::Info, target, message.as_ref());
}

pub(crate) fn info_args(target: &str, message: fmt::Arguments<'_>) {
    if level_enabled(Level::Info) {
        log_args(Level::Info, target, message);
    }
}

fn report(level: Level, target: &str, message: &str) {
    debug_assert!(level >= Level::Warning);
    write_emergency_stderr(format_args!("{message}"));
    if let Some(logger) = LOGGER.get() {
        logger.write(level, target, message);
    }
}

pub fn report_error(target: &str, message: impl AsRef<str>) {
    let message = message.as_ref();
    report(Level::Error, target, message);
}

pub(crate) fn report_error_args(target: &str, message: fmt::Arguments<'_>) {
    let message = message.to_string();
    report(Level::Error, target, &message);
}

/// Central emergency console path used before the persistent logger exists or
/// for CLI-only location hints. Platform and application modules must not
/// write stderr directly.
pub(crate) fn emergency_console(message: impl fmt::Display) {
    write_emergency_stderr(format_args!("{message}"));
}

#[cfg(target_os = "macos")]
pub(crate) fn emergency_stderr_is_terminal() -> bool {
    io::stderr().is_terminal()
}

/// Best-effort emergency output must never panic while reporting another
/// failure. There is deliberately no recursive fallback after stderr itself
/// rejects a write.
fn write_emergency_stderr(message: fmt::Arguments<'_>) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(b"KeySteer: ");
    let _ = stderr.write_fmt(message);
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}

pub(crate) fn report_warning_args(target: &str, message: fmt::Arguments<'_>) {
    if level_enabled(Level::Warning) {
        let message = message.to_string();
        report(Level::Warning, target, &message);
    }
}

/// Formatting macros keep disabled warning/info paths allocation-free. Errors
/// bypass the non-error switch, reach stderr and the file, and flush eagerly.
#[macro_export]
macro_rules! log_info {
    ($target:expr, $($arg:tt)*) => {
        $crate::app::logging::info_args($target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! report_warning {
    ($target:expr, $($arg:tt)*) => {
        $crate::app::logging::report_warning_args($target, format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! report_error {
    ($target:expr, $($arg:tt)*) => {
        $crate::app::logging::report_error_args($target, format_args!($($arg)*))
    };
}

pub fn flush() {
    if let Some(logger) = LOGGER.get() {
        logger.flush();
    }
}

/// Install once for the process. The previous hook remains responsible for
/// normal stderr output after the full diagnostic has been persisted.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let payload = panic
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panic.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic payload");
            let location = panic
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_else(|| "unknown location".into());
            report_error(
                "panic",
                format!(
                    "panic at {location}: {payload}\nbacktrace:\n{}",
                    Backtrace::force_capture()
                ),
            );
            flush();
            previous(panic);
        }));
    });
}

fn level_enabled(level: Level) -> bool {
    level_enabled_with(level, NON_ERROR_ENABLED.load(Ordering::Relaxed))
}

fn level_enabled_with(level: Level, non_error_enabled: bool) -> bool {
    level == Level::Error || non_error_enabled
}

fn log(level: Level, target: &str, message: &str) {
    if !level_enabled(level) {
        return;
    }
    if let Some(logger) = LOGGER.get() {
        logger.write(level, target, message);
    }
}

fn log_args(level: Level, target: &str, message: fmt::Arguments<'_>) {
    if let Some(logger) = LOGGER.get() {
        let message = message.to_string();
        logger.write(level, target, &message);
    }
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = super::paths::data_file(LOG_FILE).into_iter().collect();
    let fallback = std::env::temp_dir().join("KeySteer").join(LOG_FILE);
    if !paths.contains(&fallback) {
        paths.push(fallback);
    }
    paths
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn rotate_before_open(path: &Path) -> io::Result<()> {
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES)
    {
        rotate_files(path)?;
    }
    Ok(())
}

fn rotate_files(path: &Path) -> io::Result<()> {
    let oldest = rotated_path(path, RETAINED_LOGS);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for index in (1..RETAINED_LOGS).rev() {
        let source = rotated_path(path, index);
        if source.exists() {
            fs::rename(source, rotated_path(path, index + 1))?;
        }
    }
    if path.exists() {
        fs::rename(path, rotated_path(path, 1))?;
    }
    Ok(())
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{index}"));
    PathBuf::from(value)
}

fn format_line(level: Level, target: &str, message: &str) -> String {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    let message = message.replace('\n', "\n    ");
    format!(
        "{} [{level}] [{target}] [thread={name} {:?}] {message}",
        utc_timestamp(SystemTime::now()),
        thread.id()
    )
}

fn utc_timestamp(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs();
    let days = (seconds / 86_400) as i64;
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        second_of_day / 3_600,
        second_of_day % 3_600 / 60,
        second_of_day % 60,
        duration.subsec_millis()
    )
}

/// Gregorian civil date from days since 1970-01-01 (Howard Hinnant's
/// public-domain civil calendar algorithm).
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn temporary_log() -> PathBuf {
        std::env::temp_dir().join(format!(
            "keysteer-logging-{}-{}.log",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn timestamp_is_stable_utc_iso_8601() {
        let time = UNIX_EPOCH + std::time::Duration::from_millis(1_704_067_200_123);
        assert_eq!(utc_timestamp(time), "2024-01-01T00:00:00.123Z");
    }

    #[test]
    fn rotated_paths_keep_the_log_name_visible() {
        let path = Path::new("diagnostics/keysteer.log");
        assert_eq!(
            rotated_path(path, 2),
            PathBuf::from("diagnostics/keysteer.log.2")
        );
    }

    #[test]
    fn application_data_directory_is_preferred_over_the_emergency_fallback() {
        let expected = super::super::paths::data_file(LOG_FILE).unwrap();
        let paths = candidate_paths();
        assert_eq!(paths.first(), Some(&expected));
        assert_eq!(
            paths.last(),
            Some(&std::env::temp_dir().join("KeySteer").join(LOG_FILE))
        );
    }

    #[test]
    fn only_errors_bypass_the_configuration_switch() {
        assert!(level_enabled_with(Level::Error, false));
        assert!(!level_enabled_with(Level::Warning, false));
        assert!(!level_enabled_with(Level::Info, false));
        assert!(!level_enabled_with(Level::Debug, false));
        assert!(level_enabled_with(Level::Warning, true));
        assert!(level_enabled_with(Level::Info, true));
        assert!(level_enabled_with(Level::Debug, true));
    }

    #[test]
    fn logger_persists_context_and_message() {
        let path = temporary_log();
        let logger = Logger::open(path.clone()).unwrap();
        logger.write(Level::Error, "test-target", "first line\nsecond line");
        logger.flush();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[ERROR] [test-target]"), "{text}");
        assert!(text.contains("first line\n    second line"), "{text}");
        drop(logger);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn error_is_visible_and_flushed_when_non_error_logging_is_disabled() {
        let path = temporary_log();
        let logger = Logger::open(path.clone()).unwrap();
        set_non_error_enabled(false);
        assert!(level_enabled(Level::Error));
        logger.write(Level::Error, "test-target", "unconditional error");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("unconditional error"), "{text}");
        drop(logger);
        fs::remove_file(path).unwrap();
    }
}
