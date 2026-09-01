//! Dependency guardrails for the stable inner layers.

use std::path::{Path, PathBuf};

fn rust_files(root: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn production_source(path: &Path) -> std::io::Result<String> {
    let source = std::fs::read_to_string(path)?.replace("\r\n", "\n");
    Ok(source
        .split_once("#[cfg(test)]\nmod tests")
        .map_or(source.as_str(), |(production, _)| production)
        .to_owned())
}

fn assert_forbidden(root: &str, forbidden: &[&str]) -> std::io::Result<()> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root);
    let mut files = Vec::new();
    rust_files(&source_root, &mut files)?;
    for path in files {
        let source = production_source(&path)?;
        for dependency in forbidden {
            assert!(
                !source.contains(&format!("crate::{dependency}")),
                "{root} must not depend on {dependency}: {}",
                path.display()
            );
        }
    }
    Ok(())
}

#[test]
fn api_is_the_dependency_floor() -> std::io::Result<()> {
    assert_forbidden(
        "src/api",
        &[
            "app", "config", "modes", "platform", "plugins", "runtime", "support",
        ],
    )
}

#[test]
fn support_is_business_agnostic() -> std::io::Result<()> {
    assert_forbidden(
        "src/support",
        &["app", "config", "modes", "platform", "plugins", "runtime"],
    )
}

#[test]
fn config_owns_documents_without_reaching_outward() -> std::io::Result<()> {
    assert_forbidden(
        "src/config",
        &["app", "modes", "platform", "plugins", "runtime", "support"],
    )
}

#[test]
fn runtime_consumes_only_api_and_support() -> std::io::Result<()> {
    assert_forbidden(
        "src/runtime",
        &["app", "config", "modes", "platform", "plugins"],
    )
}

#[test]
fn modes_and_compiled_plugins_only_consume_the_api() -> std::io::Result<()> {
    let forbidden = ["app", "config", "platform", "runtime", "support"];
    assert_forbidden("src/modes", &forbidden)?;
    assert_forbidden("src/plugins", &forbidden)
}

#[test]
fn native_platform_code_cannot_reach_business_layers() -> std::io::Result<()> {
    assert_forbidden(
        "src/platform",
        &["app", "config", "modes", "plugins", "runtime"],
    )
}
