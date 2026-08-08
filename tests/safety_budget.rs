//! Mechanical guardrails for the ongoing native safety-boundary migration.

use std::path::{Path, PathBuf};

// Six reviewed native-boundary expressions implement display synchronisation:
// four Win32/DXGI calls for the compatibility path, plus one C ABI declaration
// and one consolidated bridge call for the Windows 11 compositor clock.
const MAX_UNSAFE_EXPRESSIONS: usize = 311;
const MAX_UNSAFE_FILES: usize = 23;

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
