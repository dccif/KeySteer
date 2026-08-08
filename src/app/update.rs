//! User-initiated release checks and bounded background downloads.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::backend::{UpdateCheckResult, UpdateProgress};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/dccif/KeySteer/releases/latest";
const CDN_LATEST_MANIFEST: &str = "https://cdn.jsdelivr.net/gh/dccif/KeySteer@latest/Cargo.toml";
const RELEASE_DOWNLOAD_ROOT: &str = "https://github.com/dccif/KeySteer/releases/download";
const GH_PROXY_ROOT: &str = "https://gh-proxy.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const DOWNLOAD_BODY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RELEASE_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;
const DOWNLOAD_BUFFER_BYTES: usize = 32 * 1024;
const UPDATE_THREAD_STACK_BYTES: usize = 512 * 1024;
static CHECK_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct UpdateCheckGuard;

impl UpdateCheckGuard {
    fn acquire() -> Option<Self> {
        CHECK_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for UpdateCheckGuard {
    fn drop(&mut self) {
        CHECK_IN_PROGRESS.store(false, Ordering::Release);
    }
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<LatestAsset>,
}

#[derive(Deserialize)]
struct LatestAsset {
    name: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct CargoManifest {
    package: CargoPackage,
}

#[derive(Deserialize)]
struct CargoPackage {
    version: String,
}

struct ReleaseInfo {
    version: Version,
    asset: Option<ReleaseAsset>,
}

struct ReleaseAsset {
    size: u64,
    sha256: Option<String>,
}

struct DownloadReceipt {
    bytes: u64,
    sha256: String,
}

#[derive(Debug)]
struct FetchFailure {
    source: &'static str,
    timed_out: bool,
    details: String,
}

impl FetchFailure {
    fn network(source: &'static str, error: ureq::Error) -> Self {
        Self {
            source,
            timed_out: matches!(error, ureq::Error::Timeout(_)),
            details: error.to_string(),
        }
    }

    fn content(source: &'static str, details: impl Into<String>) -> Self {
        Self {
            source,
            timed_out: false,
            details: details.into(),
        }
    }
}

pub(crate) fn check_async(
    progress: impl Fn(UpdateProgress) + Send + 'static,
    complete: impl FnOnce(UpdateCheckResult) + Send + 'static,
) -> Result<(), String> {
    let Some(guard) = UpdateCheckGuard::acquire() else {
        return Ok(());
    };
    std::thread::Builder::new()
        .name("keysteer-update-check".into())
        .stack_size(UPDATE_THREAD_STACK_BYTES)
        .spawn(move || {
            let _guard = guard;
            progress(UpdateProgress::Checking);
            let result = check_latest_release(&progress).unwrap_or_else(UpdateCheckResult::Failed);
            complete(result);
        })
        .map(|_| ())
        .map_err(|error| format!("cannot start update check: {error}"))
}

fn check_latest_release(progress: &dyn Fn(UpdateProgress)) -> Result<UpdateCheckResult, String> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("invalid current package version: {error}"))?;
    let target = release_target()?;
    let latest = fetch_latest_release(target)?;

    if latest.version <= current {
        return Ok(UpdateCheckResult::UpToDate {
            current: current.to_string(),
        });
    }

    let latest_version = latest.version.to_string();
    let report_download = |percent| {
        progress(UpdateProgress::Downloading {
            latest: latest_version.clone(),
            percent,
        });
    };
    report_download(0);
    let path = download_release(&latest, target, &report_download)?;
    Ok(UpdateCheckResult::UpdateDownloaded {
        current: current.to_string(),
        latest: latest.version.to_string(),
        path,
    })
}

fn fetch_latest_release(target: &str) -> Result<ReleaseInfo, String> {
    match fetch_github_release(target) {
        Ok(release) => Ok(release),
        Err(github) => match fetch_cdn_version() {
            Ok(version) => Ok(ReleaseInfo {
                version,
                asset: None,
            }),
            Err(cdn) => {
                if github.timed_out && cdn.timed_out {
                    Err(format!(
                        "Update check timed out: GitHub and the CDN fallback each exceeded {} seconds.",
                        REQUEST_TIMEOUT.as_secs()
                    ))
                } else {
                    Err(format!(
                        "Update check failed. {}: {}; CDN retry via {}: {}",
                        github.source, github.details, cdn.source, cdn.details
                    ))
                }
            }
        },
    }
}

fn fetch_github_release(target: &str) -> Result<ReleaseInfo, FetchFailure> {
    let agent: ureq::Agent = metadata_agent_config().into();
    let release: LatestRelease = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("KeySteer/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| FetchFailure::network("GitHub", error))?
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_RESPONSE_BYTES)
        .read_json()
        .map_err(|error| FetchFailure::network("GitHub", error))?;
    let version = parse_release_version(&release.tag_name)
        .map_err(|error| FetchFailure::content("GitHub", error))?;
    let asset_name = release_asset_name(&version, target);
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            FetchFailure::content("GitHub", format!("release asset {asset_name} is missing"))
        })?;
    let sha256 = parse_sha256_digest(asset.digest.as_deref())
        .map_err(|error| FetchFailure::content("GitHub", error))?;
    Ok(ReleaseInfo {
        version,
        asset: Some(ReleaseAsset {
            size: asset.size,
            sha256,
        }),
    })
}

fn fetch_cdn_version() -> Result<Version, FetchFailure> {
    let agent: ureq::Agent = metadata_agent_config().into();
    let manifest = agent
        .get(CDN_LATEST_MANIFEST)
        .header("Accept", "text/plain")
        .header(
            "User-Agent",
            concat!("KeySteer/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| FetchFailure::network("jsDelivr", error))?
        .body_mut()
        .with_config()
        .limit(MAX_RELEASE_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|error| FetchFailure::network("jsDelivr", error))?;
    let manifest: CargoManifest = toml::from_str(&manifest)
        .map_err(|error| FetchFailure::content("jsDelivr", error.to_string()))?;
    Version::parse(&manifest.package.version)
        .map_err(|error| FetchFailure::content("jsDelivr", error.to_string()))
}

fn parse_release_version(tag: &str) -> Result<Version, String> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(version).map_err(|error| format!("invalid release version {tag:?}: {error}"))
}

fn parse_sha256_digest(digest: Option<&str>) -> Result<Option<String>, String> {
    let Some(digest) = digest else {
        return Ok(None);
    };
    let value = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("unsupported release asset digest {digest:?}"))?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid release asset digest {digest:?}"));
    }
    Ok(Some(value.to_ascii_lowercase()))
}

fn release_target() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => Err(format!("no release package is available for {os}/{arch}")),
    }
}

fn release_asset_name(version: &Version, target: &str) -> String {
    format!("KeySteer-v{version}-{target}.zip")
}

fn download_release(
    release: &ReleaseInfo,
    target: &str,
    progress: &dyn Fn(u8),
) -> Result<PathBuf, String> {
    let file_name = release_asset_name(&release.version, target);
    let url = format!("{RELEASE_DOWNLOAD_ROOT}/v{}/{file_name}", release.version);
    let directory = downloads_directory()?;
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "cannot create the Downloads directory {}: {error}",
            directory.display()
        )
    })?;

    let destination = directory.join(&file_name);
    let partial = directory.join(format!(".{file_name}.part"));
    let partial_guard = PartialDownload::new(partial);
    if let Some(asset) = release.asset.as_ref()
        && asset.size > MAX_DOWNLOAD_BYTES
    {
        return Err(format!(
            "release asset is {} bytes, above the {} MiB update limit",
            asset.size,
            MAX_DOWNLOAD_BYTES / (1024 * 1024)
        ));
    }

    if let Err(direct_error) =
        download_and_validate(&url, partial_guard.path(), release.asset.as_ref(), progress)
    {
        let proxy_url = gh_proxy_url(&url);
        progress(0);
        download_and_validate(
            &proxy_url,
            partial_guard.path(),
            release.asset.as_ref(),
            progress,
        )
        .map_err(|proxy_error| {
                format!(
                    "Official GitHub download failed: {direct_error}\n\ngh-proxy retry failed: {proxy_error}"
                )
            })?;
    }
    replace_download(partial_guard.path(), &destination)?;
    Ok(destination)
}

fn gh_proxy_url(original: &str) -> String {
    format!("{GH_PROXY_ROOT}/{original}")
}

fn download_and_validate(
    url: &str,
    path: &Path,
    asset: Option<&ReleaseAsset>,
    progress: &dyn Fn(u8),
) -> Result<(), String> {
    let receipt = download_to(url, path, asset.map(|asset| asset.size), progress)?;
    validate_zip(path)?;
    if let Some(asset) = asset {
        if receipt.bytes != asset.size {
            return Err(format!(
                "download size mismatch: expected {} bytes, received {}",
                asset.size, receipt.bytes
            ));
        }
        if let Some(expected) = asset.sha256.as_deref()
            && receipt.sha256 != expected
        {
            return Err("downloaded update failed GitHub SHA-256 verification".into());
        }
    }
    progress(100);
    Ok(())
}

fn download_to(
    url: &str,
    path: &Path,
    expected_size: Option<u64>,
    progress: &dyn Fn(u8),
) -> Result<DownloadReceipt, String> {
    let agent: ureq::Agent = download_agent_config().into();
    let mut response = agent
        .get(url)
        .header("Accept", "application/octet-stream")
        .header(
            "User-Agent",
            concat!("KeySteer/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|error| download_network_error("cannot start update download", &error))?;
    let response_size = response.body().content_length();
    if let Some(size) = response_size
        && size > MAX_DOWNLOAD_BYTES
    {
        return Err(format!(
            "release asset is {size} bytes, above the {} MiB update limit",
            MAX_DOWNLOAD_BYTES / (1024 * 1024)
        ));
    }
    let total = expected_size.filter(|size| *size > 0).or(response_size);
    let mut file = File::create(path).map_err(|error| {
        format!(
            "cannot create temporary download {}: {error}",
            path.display()
        )
    })?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .reader();
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut last_percent = 0_u8;
    let mut buffer = [0_u8; DOWNLOAD_BUFFER_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| download_io_error("update download failed", &error))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|error| {
            format!(
                "cannot write temporary download {}: {error}",
                path.display()
            )
        })?;
        hasher.update(&buffer[..read]);
        bytes += read as u64;
        if let Some(total) = total {
            let percent = download_percent(bytes, total).min(99);
            if percent > last_percent {
                last_percent = percent;
                progress(percent);
            }
        }
    }
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "cannot finish temporary download {}: {error}",
                path.display()
            )
        })?;
    drop(file);
    Ok(DownloadReceipt {
        bytes,
        sha256: hex_encode(hasher.finalize().as_ref()),
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn download_percent(received: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((u128::from(received) * 100 / u128::from(total)).min(100)) as u8
}

fn validate_zip(path: &Path) -> Result<(), String> {
    let mut file = File::open(path)
        .map_err(|error| format!("cannot validate download {}: {error}", path.display()))?;
    let mut signature = [0_u8; 4];
    file.read_exact(&mut signature)
        .map_err(|error| format!("downloaded update is incomplete: {error}"))?;
    if matches!(signature, [b'P', b'K', 3, 4] | [b'P', b'K', 5, 6]) {
        Ok(())
    } else {
        Err("downloaded update is not a valid ZIP archive".into())
    }
}

fn download_network_error(context: &str, error: &ureq::Error) -> String {
    if matches!(error, ureq::Error::Timeout(_)) {
        format!(
            "Update download timed out after {} seconds while connecting to GitHub.",
            REQUEST_TIMEOUT.as_secs()
        )
    } else {
        format!("{context}: {error}")
    }
}

fn download_io_error(context: &str, error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::TimedOut || error.to_string().contains("timeout") {
        format!(
            "Update download timed out after {} seconds while receiving the file.",
            DOWNLOAD_BODY_TIMEOUT.as_secs()
        )
    } else {
        format!("{context}: {error}")
    }
}

struct PartialDownload {
    path: PathBuf,
}

impl PartialDownload {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PartialDownload {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn metadata_agent_config() -> ureq::config::Config {
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

fn download_agent_config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_resolve(Some(REQUEST_TIMEOUT))
        .timeout_connect(Some(REQUEST_TIMEOUT))
        .timeout_send_request(Some(REQUEST_TIMEOUT))
        .timeout_recv_response(Some(REQUEST_TIMEOUT))
        .timeout_recv_body(Some(DOWNLOAD_BODY_TIMEOUT))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
}

#[cfg(target_os = "windows")]
fn downloads_directory() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .ok_or_else(|| {
            "cannot locate the user Downloads folder: USERPROFILE and HOME are not set".into()
        })
}

#[cfg(not(target_os = "windows"))]
fn downloads_directory() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .ok_or_else(|| "cannot locate the user Downloads folder: HOME is not set".into())
}

fn replace_download(from: &Path, to: &Path) -> Result<(), String> {
    crate::platform::atomic_replace(from, to)
        .map_err(|error| format!("cannot replace {}: {error}", to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_agents_use_the_enabled_native_tls_provider() {
        for config in [metadata_agent_config(), download_agent_config()] {
            assert_eq!(
                config.tls_config().provider(),
                ureq::tls::TlsProvider::NativeTls
            );
            assert!(matches!(
                config.tls_config().root_certs(),
                &ureq::tls::RootCerts::PlatformVerifier
            ));
        }
    }

    #[test]
    fn update_checks_are_single_flight_and_release_the_guard() {
        let first = UpdateCheckGuard::acquire().expect("first check should acquire the guard");
        assert!(UpdateCheckGuard::acquire().is_none());

        drop(first);
        assert!(UpdateCheckGuard::acquire().is_some());
    }

    #[test]
    fn release_tags_accept_the_optional_v_prefix() {
        assert_eq!(
            parse_release_version("v0.2.0").unwrap(),
            Version::new(0, 2, 0)
        );
        assert_eq!(
            parse_release_version("0.2.0").unwrap(),
            Version::new(0, 2, 0)
        );
    }

    #[test]
    fn release_asset_names_match_the_packaging_scripts() {
        assert_eq!(
            release_asset_name(&Version::new(0, 5, 0), "aarch64-apple-darwin"),
            "KeySteer-v0.5.0-aarch64-apple-darwin.zip"
        );
    }

    #[test]
    fn gh_proxy_wraps_the_original_release_url() {
        assert_eq!(
            gh_proxy_url("https://github.com/dccif/KeySteer/releases/download/v0.5.0/a.zip"),
            "https://gh-proxy.com/https://github.com/dccif/KeySteer/releases/download/v0.5.0/a.zip"
        );
    }

    #[test]
    fn github_sha256_digest_is_strictly_validated() {
        let digest = format!("sha256:{}", "aB".repeat(32));
        assert_eq!(
            parse_sha256_digest(Some(&digest)).unwrap(),
            Some("ab".repeat(32))
        );
        assert!(parse_sha256_digest(Some("md5:abcd")).is_err());
        assert!(parse_sha256_digest(Some("sha256:short")).is_err());
        assert_eq!(parse_sha256_digest(None).unwrap(), None);
    }

    #[test]
    fn automatic_update_downloads_are_bounded_to_ten_mib() {
        assert_eq!(MAX_DOWNLOAD_BYTES, 10 * 1024 * 1024);
        assert_eq!(DOWNLOAD_BUFFER_BYTES, 32 * 1024);
        assert_eq!(UPDATE_THREAD_STACK_BYTES, 512 * 1024);
    }

    #[test]
    fn download_percentage_is_bounded_and_monotonic() {
        assert_eq!(download_percent(0, 10), 0);
        assert_eq!(download_percent(1, 10), 10);
        assert_eq!(download_percent(5, 10), 50);
        assert_eq!(download_percent(10, 10), 100);
        assert_eq!(download_percent(11, 10), 100);
        assert_eq!(download_percent(u64::MAX, u64::MAX), 100);
        assert_eq!(download_percent(10, 0), 0);
    }

    #[test]
    fn sha256_hex_encoding_is_lowercase_and_zero_padded() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn prerelease_order_uses_semver() {
        assert!(parse_release_version("v0.2.0").unwrap() > Version::parse("0.2.0-beta.1").unwrap());
    }

    #[test]
    fn malformed_release_tag_is_rejected() {
        assert!(parse_release_version("latest").is_err());
    }

    #[test]
    fn invalid_download_body_is_not_accepted_as_a_zip() {
        let path = std::env::temp_dir().join(format!(
            "keysteer-invalid-update-{}.zip",
            std::process::id()
        ));
        fs::write(&path, b"<html>gateway error</html>").unwrap();

        assert!(validate_zip(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn partial_download_guard_removes_failed_downloads() {
        let path = std::env::temp_dir().join(format!(
            "keysteer-partial-update-{}.zip.part",
            std::process::id()
        ));
        {
            let guard = PartialDownload::new(path.clone());
            fs::write(guard.path(), b"partial").unwrap();
        }

        assert!(!path.exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn replacing_a_download_preserves_the_new_complete_file() {
        let directory = std::env::temp_dir().join(format!(
            "keysteer-update-replace-test-{}",
            std::process::id()
        ));
        let old = directory.join("KeySteer.zip");
        let new = directory.join("KeySteer.zip.part");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&old, b"old").unwrap();
        fs::write(&new, b"new complete download").unwrap();

        replace_download(&new, &old).unwrap();

        assert_eq!(fs::read(&old).unwrap(), b"new complete download");
        assert!(!new.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
