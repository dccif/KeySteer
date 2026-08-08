//! User-initiated release checks. No polling or automatic startup request.

use std::time::Duration;

use semver::Version;
use serde::Deserialize;

use crate::api::backend::UpdateCheckResult;

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/dccif/KeySteer/releases/latest";
const LATEST_RELEASE_PAGE: &str = "https://github.com/dccif/KeySteer/releases/latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

pub(crate) fn check_async(
    complete: impl FnOnce(UpdateCheckResult) + Send + 'static,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("keysteer-update-check".into())
        .spawn(move || {
            let result = check_latest_release().unwrap_or_else(UpdateCheckResult::Failed);
            complete(result);
        })
        .map(|_| ())
        .map_err(|error| format!("cannot start update check: {error}"))
}

fn check_latest_release() -> Result<UpdateCheckResult, String> {
    let agent: ureq::Agent = update_agent_config().into();
    let release: LatestRelease = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("KeySteer/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| format!("cannot query GitHub releases: {error}"))?
        .body_mut()
        .read_json()
        .map_err(|error| format!("cannot read GitHub release response: {error}"))?;

    compare_versions(env!("CARGO_PKG_VERSION"), &release.tag_name)
}

fn update_agent_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
}

fn compare_versions(current: &str, latest_tag: &str) -> Result<UpdateCheckResult, String> {
    let current_version = Version::parse(current)
        .map_err(|error| format!("invalid current version {current:?}: {error}"))?;
    let latest_text = latest_tag.strip_prefix('v').unwrap_or(latest_tag);
    let latest_version = Version::parse(latest_text)
        .map_err(|error| format!("invalid release version {latest_tag:?}: {error}"))?;

    if latest_version > current_version {
        Ok(UpdateCheckResult::UpdateAvailable {
            current: current.to_owned(),
            latest: latest_version.to_string(),
            url: LATEST_RELEASE_PAGE.to_owned(),
        })
    } else {
        Ok(UpdateCheckResult::UpToDate {
            current: current.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_agent_uses_the_enabled_native_tls_provider() {
        let config = update_agent_config();

        assert_eq!(
            config.tls_config().provider(),
            ureq::tls::TlsProvider::NativeTls
        );
        assert!(matches!(
            config.tls_config().root_certs(),
            &ureq::tls::RootCerts::PlatformVerifier
        ));
    }

    #[test]
    fn newer_release_is_available() {
        assert_eq!(
            compare_versions("0.1.0", "v0.2.0").unwrap(),
            UpdateCheckResult::UpdateAvailable {
                current: "0.1.0".into(),
                latest: "0.2.0".into(),
                url: LATEST_RELEASE_PAGE.into(),
            }
        );
    }

    #[test]
    fn equal_or_older_release_is_up_to_date() {
        for release in ["v0.1.0", "0.0.9"] {
            assert_eq!(
                compare_versions("0.1.0", release).unwrap(),
                UpdateCheckResult::UpToDate {
                    current: "0.1.0".into(),
                }
            );
        }
    }

    #[test]
    fn prerelease_order_uses_semver() {
        assert!(matches!(
            compare_versions("0.2.0-beta.1", "v0.2.0").unwrap(),
            UpdateCheckResult::UpdateAvailable { .. }
        ));
    }

    #[test]
    fn malformed_release_tag_is_rejected() {
        assert!(compare_versions("0.1.0", "latest").is_err());
    }
}
