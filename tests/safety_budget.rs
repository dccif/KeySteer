//! Mechanical guardrails for the ongoing native safety-boundary migration.

use std::path::{Path, PathBuf};

// Window close adds one pointer-free WM_CLOSE post and one retained, type-checked
// AXCloseButton timeout/AXPress block. Neither terminates processes.
// Keep the current audited native surface from growing. Portable layers are
// checked separately below and remain entirely safe Rust.
// Window Mover adds four Win32 placement calls and five bounded AX operations.
// Fullscreen transitions add one bounded timeout call for the retained AX window.
// Input recovery adds owned SetTimer/KillTimer and WTS registration pairs.
// Literal-character input adds one bounded read-only FFI block per platform:
// CGEvent Unicode extraction and Win32 ToUnicodeEx with no-state-change flags.
// Candidate filtering adds a foreground HKL read, cold translation enumeration,
// and an explicitly requested read-only loaded-layout test probe on Windows.
// Window mode adds eight bounded Win32 identity/placement operations, two blocks
// for an explicitly run disposable-window probe, and 10 retained/type-checked
// AX/Quartz operations. Its state machine and shared worker remain safe Rust.
// Modeless note entry adds six bounded Win32 dialog operations, one explicit
// disposable-dialog Unicode probe, and one AppKit
// action-construction block, owned by existing tray/main-thread lifecycles.
// Inline input adds one audited AppKit superclass initialization for the
// borderless NSPanel subclass; its retained owner and main-thread lifetime stay unchanged.
// Window state cycle adds one bounded ShowWindowAsync minimization request.
// Active-only grouping keeps applications as independent top-level windows.
// WinEvent/AX observers enqueue identities, separate Win32/AppKit strips own only
// their own controls, and adapters restore visibility leases before releasing IDs.
// Native acceptance uses only disposable windows and checks no visible repositioning
// of incoming members. Portable grouping and placement remain safe Rust.
// Three bounded worker message-queue operations replace tab polling on Windows.
// Tab drag feedback invalidates only the existing owned strip.
// The strip changes only its own popup owner to follow application stacking.
// Opacity leases preserve content; separate read/reset blocks validate visibility
// and restore only the adapter's own added compositing style bit.
// A disposable child-process probe reads its own show-event counter.
// One bounded disposable-strip probe verifies mouse drop and cancellation.
// A disposable-window minimize message verifies native group synchronization.
// Application audio adds two worker-owned blocks and one owned-session test: a closed process snapshot and
// apartment-scoped Core Audio interfaces. Explicit Shift audio actions add six
// endpoint/property blocks and one non-disruptive native policy probe. The
// versioned policy adapter adds three exact ABI slots and four scoped call blocks.
const MAX_UNSAFE_EXPRESSIONS: usize = 367;
const MAX_UNSAFE_FILES: usize = 31;
const PER_FILE_BUDGET: &[(&str, usize)] = &[
    // macOS audio owns, changes, maintains and destroys native state,
    // with one bounded diagnostic callback into centralized logging.
    ("src/platform/macos/window_audio.rs", 6),
    ("src/platform/windows/window_audio.rs", 10),
    ("src/platform/windows/audio_policy.rs", 7),
    ("src/platform/macos/accessibility/window_tabs.rs", 12),
    ("src/platform/macos/window_tabs.rs", 2),
    ("src/platform/windows/window_tabs/strip.rs", 11),
    ("src/platform/macos/accessibility.rs", 18),
    ("src/platform/macos/accessibility/window_manager.rs", 12),
    ("src/platform/macos/autostart.rs", 5),
    ("src/platform/macos/display_link.rs", 4),
    ("src/platform/macos/native.rs", 7),
    ("src/platform/macos/overlay.rs", 6),
    ("src/platform/macos/permissions.rs", 5),
    ("src/platform/macos/status_item.rs", 6),
    ("src/platform/windows/text_prompt.rs", 7),
    ("src/platform/macos/vision.rs", 5),
    ("src/platform/windows/accessibility.rs", 31),
    ("src/platform/windows/autostart.rs", 4),
    ("src/platform/windows/gpu_overlay.rs", 28),
    ("src/platform/windows/hook.rs", 8),
    ("src/platform/windows/input.rs", 8),
    ("src/platform/windows/overlay.rs", 9),
    ("src/platform/windows/screens.rs", 5),
    ("src/platform/windows/window_mover.rs", 4),
    ("src/platform/windows/window_manager.rs", 27),
    ("src/platform/windows/window_tabs.rs", 7),
    ("src/platform/windows/status_item.rs", 14),
    ("src/platform/windows/update_installer/candidate.rs", 4),
    ("src/platform/windows/update_installer/mod.rs", 11),
    ("src/platform/windows/update_installer/signature.rs", 9),
    ("src/platform/windows/native/mod.rs", 84),
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
            !source.contains("transmute"),
            "transmute and transmute_copy are forbidden: {}",
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
        assert!(
            !source.contains("allow(clippy::undocumented_unsafe_blocks)"),
            "platform modules must document each unsafe block: {}",
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
    for relative in [
        "api",
        "app",
        "config",
        "modes",
        "plugins",
        "presentation",
        "support",
        "platform/common",
    ] {
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

#[test]
fn overlay_capture_affinity_cannot_be_reintroduced() -> Result<(), Box<dyn std::error::Error>> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&source_root, &mut files)?;
    for path in files {
        let source = std::fs::read_to_string(&path)?;
        for forbidden in ["SetWindowDisplayAffinity", "WDA_EXCLUDEFROMCAPTURE"] {
            assert!(
                !source.contains(forbidden),
                "capture-affinity API {forbidden} is forbidden: {}",
                path.display()
            );
        }
    }
    Ok(())
}
