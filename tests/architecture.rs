//! Mechanical module-layout and dependency-boundary guardrails.

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
fn modern_module_layout_does_not_use_mod_rs() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&source_root, &mut files)?;
    let legacy: Vec<_> = files
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "mod.rs"))
        .collect();
    assert!(legacy.is_empty(), "legacy module roots found: {legacy:?}");
    Ok(())
}

#[test]
fn lower_layers_do_not_depend_on_app() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for layer in [
        "api", "config", "modes", "platform", "plugins", "runtime", "support",
    ] {
        let mut files = vec![source_root.join(format!("{layer}.rs"))];
        let directory = source_root.join(layer);
        if directory.is_dir() {
            rust_files(&directory, &mut files)?;
        }
        for path in files {
            let source = std::fs::read_to_string(&path)?;
            let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
            assert!(
                !production.contains("crate::app::") && !production.contains("super::app::"),
                "{layer} must not depend on the composition layer: {}",
                path.display()
            );
        }
    }
    Ok(())
}
