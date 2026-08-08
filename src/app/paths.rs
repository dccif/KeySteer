//! Runtime paths for portable binaries and packaged macOS applications.

use std::path::{Path, PathBuf};

/// Directory used for mutable application data.
///
/// A packaged macOS app must not write inside its signed bundle, so its
/// configuration and diagnostics live in the conventional per-user
/// Application Support directory. A directly launched binary, including the
/// Windows portable build, keeps the original beside-the-executable policy.
pub(crate) fn data_dir() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    data_dir_for(&executable, home.as_deref(), cfg!(target_os = "macos"))
}

pub(crate) fn data_file(file_name: &str) -> Option<PathBuf> {
    data_dir().map(|directory| directory.join(file_name))
}

/// Resolve an explicit `--config` argument.
///
/// A bare file name retains the portable, beside-the-executable behaviour.
/// Once the user supplies a directory, normal command-line path semantics
/// apply: absolute paths are kept and relative paths start at the process
/// working directory.
pub(crate) fn explicit_config_file(requested: &Path) -> Result<PathBuf, String> {
    if requested.is_absolute() {
        return Ok(requested.to_path_buf());
    }

    if requested.components().count() == 1 {
        let directory =
            data_dir().ok_or_else(|| "cannot determine the KeySteer data directory".to_string())?;
        return Ok(directory.join(requested));
    }

    let working_directory = std::env::current_dir()
        .map_err(|error| format!("cannot determine the working directory: {error}"))?;
    Ok(working_directory.join(requested))
}

fn data_dir_for(executable: &Path, home: Option<&Path>, macos: bool) -> Option<PathBuf> {
    if macos && is_macos_app_executable(executable) {
        return home.map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("KeySteer")
        });
    }
    executable.parent().map(PathBuf::from)
}

fn is_macos_app_executable(executable: &Path) -> bool {
    let Some(macos_directory) = executable.parent() else {
        return false;
    };
    if macos_directory.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return false;
    }
    let Some(contents_directory) = macos_directory.parent() else {
        return false;
    };
    if contents_directory
        .file_name()
        .and_then(|name| name.to_str())
        != Some("Contents")
    {
        return false;
    }
    contents_directory
        .parent()
        .and_then(Path::extension)
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_macos_app_uses_application_support() {
        let executable = Path::new("/Applications/KeySteer.app/Contents/MacOS/keysteer");
        assert_eq!(
            data_dir_for(executable, Some(Path::new("/Users/tester")), true),
            Some(PathBuf::from(
                "/Users/tester/Library/Application Support/KeySteer"
            ))
        );
    }

    #[test]
    fn bare_macos_binary_stays_portable() {
        let executable = Path::new("/Users/tester/tools/keysteer");
        assert_eq!(
            data_dir_for(executable, Some(Path::new("/Users/tester")), true),
            Some(PathBuf::from("/Users/tester/tools"))
        );
    }

    #[test]
    fn non_macos_build_stays_portable_even_inside_app_shaped_path() {
        let executable = Path::new("C:/KeySteer.app/Contents/MacOS/keysteer.exe");
        assert_eq!(
            data_dir_for(executable, Some(Path::new("C:/Users/tester")), false),
            Some(PathBuf::from("C:/KeySteer.app/Contents/MacOS"))
        );
    }

    #[test]
    fn explicit_bare_config_name_uses_the_application_data_directory() {
        let requested = Path::new("keysteer.default.toml");
        let directory = data_dir().unwrap();

        assert_eq!(
            explicit_config_file(requested).unwrap(),
            directory.join(requested)
        );
    }

    #[test]
    fn explicit_relative_config_path_uses_the_working_directory() {
        let requested = Path::new(".").join("keysteer.default.toml");

        assert_eq!(
            explicit_config_file(&requested).unwrap(),
            std::env::current_dir().unwrap().join(requested)
        );
    }

    #[test]
    fn explicit_absolute_config_path_is_unchanged() {
        let requested = std::env::temp_dir().join("keysteer.default.toml");

        assert_eq!(explicit_config_file(&requested).unwrap(), requested);
    }
}
