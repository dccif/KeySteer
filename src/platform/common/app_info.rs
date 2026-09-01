//! Compile-time application identity used by native About dialogs.

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const AUTHORS: &str = env!("CARGO_PKG_AUTHORS");
pub(crate) const BUILD_DATE: &str = env!("KEYSTEER_BUILD_DATE");
pub(crate) const LICENSE: &str = env!("CARGO_PKG_LICENSE");

pub(crate) fn details() -> String {
    format!(
        "Version: {VERSION}\nAuthor: {AUTHORS}\nBuild date: {BUILD_DATE} (UTC)\nLicense: {LICENSE}\nCopyright © 2026 dccif"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_details_use_package_and_build_metadata() {
        let details = details();
        assert!(details.contains(concat!("Version: ", env!("CARGO_PKG_VERSION"))));
        assert!(details.contains("Author: dccif"));
        assert!(details.contains("Build date: "));
        assert!(details.contains("License: GPL-3.0-or-later"));
    }
}
