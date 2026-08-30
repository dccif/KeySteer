//! Mechanical guardrails for the application-wide logging contract.

use std::path::{Path, PathBuf};

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

#[test]
fn application_diagnostics_use_the_unified_logger() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let logging = source_root.join("app/logging.rs");
    let mut files = Vec::new();
    rust_files(&source_root, &mut files)?;

    for path in files {
        let source = std::fs::read_to_string(&path)?;
        assert!(
            !source.contains("logging::error("),
            "use logging::report_error for unconditional error reporting: {}",
            path.display()
        );
        if path != logging {
            for forbidden in [
                "eprintln!(",
                "eprint!(",
                "std::io::stderr(",
                "io::stderr(",
                "dbg!(",
                "OutputDebugString",
                "WriteConsole",
                "STD_ERROR_HANDLE",
                "log::",
                "tracing::",
                "env_logger::",
                "slog::",
                "NSLog",
                "os_log(",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "diagnostics must go through app::logging ({forbidden}): {}",
                    path.display()
                );
            }
        }
        assert!(
            !source.contains("log_warning!"),
            "warnings must use report_warning!: {}",
            path.display()
        );
    }
    Ok(())
}

#[test]
fn native_sources_and_dependencies_cannot_bypass_logging() -> Result<(), Box<dyn std::error::Error>>
{
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![source_root];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|extension| matches!(extension.to_str(), Some("m" | "mm" | "c" | "h")))
            {
                continue;
            }
            let source = std::fs::read_to_string(&path)?;
            for forbidden in [
                "NSLog",
                "os_log(",
                "OutputDebugString",
                "WriteConsole",
                "STD_ERROR_HANDLE",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "native diagnostics must be returned to app::logging ({forbidden}): {}",
                    path.display()
                );
            }
        }
    }

    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))?;
    for dependency in ["log", "tracing", "env_logger", "slog"] {
        let prefix = format!("{dependency} =");
        assert!(
            !manifest
                .lines()
                .any(|line| line.trim_start().starts_with(&prefix)),
            "diagnostics dependency {dependency} bypasses src/app/logging.rs"
        );
    }
    Ok(())
}
