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
    let cli = source_root.join("app/cli.rs");
    let mut files = Vec::new();
    rust_files(&source_root, &mut files)?;

    for path in files {
        let source = std::fs::read_to_string(&path)?;
        assert!(
            !source.contains("logging::error("),
            "use logging::report_error for unconditional error reporting: {}",
            path.display()
        );
        if path != logging && path != cli {
            assert!(
                !source.contains("eprintln!(") && !source.contains("std::io::stderr("),
                "diagnostics must go through app::logging: {}",
                path.display()
            );
        }
    }
    Ok(())
}
