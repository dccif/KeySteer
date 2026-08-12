//! Mechanical guardrails for the ongoing native safety-boundary migration.

use std::path::{Path, PathBuf};

// Keep the current audited native surface from growing. Portable layers are
// checked separately below and remain entirely safe Rust.
const MAX_UNSAFE_EXPRESSIONS: usize = 304;
const MAX_UNSAFE_FILES: usize = 23;
const PER_FILE_BUDGET: &[(&str, usize)] = &[
    ("src/platform/macos/accessibility.rs", 15),
    ("src/platform/macos/autostart.rs", 5),
    ("src/platform/macos/display_link.rs", 4),
    ("src/platform/macos/hook.rs", 2),
    ("src/platform/macos/input.rs", 11),
    ("src/platform/macos/native.rs", 2),
    ("src/platform/macos/overlay.rs", 6),
    ("src/platform/macos/permissions.rs", 5),
    ("src/platform/macos/screens.rs", 4),
    ("src/platform/macos/status_item.rs", 4),
    ("src/platform/macos/vision.rs", 5),
    ("src/platform/macos/workspace.rs", 4),
    ("src/platform/windows/accessibility.rs", 64),
    ("src/platform/windows/autostart.rs", 4),
    ("src/platform/windows/console_control.rs", 5),
    ("src/platform/windows/gpu_overlay.rs", 43),
    ("src/platform/windows/hook.rs", 20),
    ("src/platform/windows/input.rs", 6),
    ("src/platform/windows/overlay.rs", 26),
    ("src/platform/windows/screens.rs", 8),
    ("src/platform/windows/status_item.rs", 27),
    ("src/platform/windows/system_events.rs", 4),
    ("src/platform/windows/native/mod.rs", 30),
];

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let entries = std::fs::read_dir(directory)?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn unsafe_expressions(source: &str) -> usize {
    source
        .match_indices("unsafe")
        .filter(|(index, _)| {
            let before = source[..*index].chars().next_back();
            let after_index = index + "unsafe".len();
            let after = source[after_index..].chars().next();
            let boundary = |character: Option<char>| {
                character
                    .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
            };
            if !boundary(before) || !boundary(after) {
                return false;
            }
            let tail = source[after_index..].trim_start();
            tail.starts_with('{')
                || tail.starts_with("fn ")
                || tail.starts_with("extern ")
                || tail.starts_with("impl ")
                || tail.starts_with("trait ")
        })
        .count()
}

#[test]
fn unsafe_surface_does_not_regress() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&source_root, &mut files)?;

    let mut expression_count = 0;
    let mut unsafe_files = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path)?;
        let count = unsafe_expressions(&source);
        let relative = path
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))?
            .to_string_lossy()
            .replace('\\', "/");
        let budget = PER_FILE_BUDGET
            .iter()
            .find_map(|(candidate, budget)| (*candidate == relative).then_some(*budget))
            .unwrap_or(0);
        assert!(
            count <= budget,
            "unsafe budget regressed in {relative}: {count} > {budget}"
        );
        if count > 0 {
            expression_count += count;
        }
        assert!(
            !source.contains("transmute("),
            "transmute is forbidden: {}",
            path.display()
        );
        assert!(
            !source.contains("static mut "),
            "static mut is forbidden: {}",
            path.display()
        );
        assert!(
            !source.contains("get_unchecked"),
            "unchecked indexing is forbidden: {}",
            path.display()
        );
        assert!(
            !source.contains("unsafe impl Send") && !source.contains("unsafe impl Sync"),
            "unsafe Send/Sync requires an explicit architecture review: {}",
            path.display()
        );
        if count > 0 {
            unsafe_files.push(path);
        }
    }

    assert!(
        expression_count <= MAX_UNSAFE_EXPRESSIONS,
        "unsafe expression budget regressed: {expression_count} > {MAX_UNSAFE_EXPRESSIONS}"
    );
    assert!(
        unsafe_files.len() <= MAX_UNSAFE_FILES,
        "unsafe file budget regressed: {} > {MAX_UNSAFE_FILES}: {unsafe_files:?}",
        unsafe_files.len()
    );
    Ok(())
}

#[test]
fn portable_layers_are_safe_rust() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for relative in ["api", "app", "config", "domain", "modes", "plugins"] {
        let mut files = Vec::new();
        rust_files(&source_root.join(relative), &mut files)?;
        for path in files {
            let source = std::fs::read_to_string(&path)?;
            assert_eq!(
                unsafe_expressions(&source),
                0,
                "portable layer contains unsafe code: {}",
                path.display()
            );
        }
    }
    Ok(())
}
