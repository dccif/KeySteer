//! One-shot handoff of the active TOML source to the static web simulator.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use miniz_oxide::deflate::compress_to_vec_zlib;
use std::time::Duration;

const SIMULATOR_URL: &str = "https://dccif.github.io/KeySteer/simulator";
const PROTOCOL_VERSION: &str = "v1";
const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_FRAGMENT_BYTES: usize = 24 * 1024;
pub(super) const OPEN_DEBOUNCE: Duration = Duration::from_secs(2);

pub(super) fn url_for_config(source: &str) -> String {
    if source.len() > MAX_SOURCE_BYTES {
        return format!("{SIMULATOR_URL}#ks-config-error=too-large");
    }

    let compressed = compress_to_vec_zlib(source.as_bytes(), 6);
    let encoded = URL_SAFE_NO_PAD.encode(compressed);
    let fragment = format!("ks-config={PROTOCOL_VERSION}.{}.{}", source.len(), encoded);
    if fragment.len() > MAX_FRAGMENT_BYTES {
        return format!("{SIMULATOR_URL}#ks-config-error=too-large");
    }
    format!("{SIMULATOR_URL}#{fragment}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::inflate::decompress_to_vec_zlib;

    fn decode(url: &str) -> Vec<u8> {
        let fragment = url.split_once('#').unwrap().1;
        let payload = fragment
            .strip_prefix("ks-config=v1.")
            .unwrap()
            .split_once('.')
            .unwrap()
            .1;
        let compressed = URL_SAFE_NO_PAD.decode(payload).unwrap();
        decompress_to_vec_zlib(&compressed).unwrap()
    }

    #[test]
    fn config_handoff_round_trips_unicode_and_comments() {
        let source = "# 注释\n[normal.bindings]\n空格 = \"left_click\"\n";
        let url = url_for_config(source);
        assert_eq!(decode(&url), source.as_bytes());
        assert!(!url.contains('?'));
    }

    #[test]
    fn shipped_configuration_fits_the_browser_handoff() {
        let source = include_str!("../../../keysteer.default.toml");
        let url = url_for_config(source);
        assert!(url.starts_with(&format!("{SIMULATOR_URL}#ks-config=v1.")));
        assert!(url.len() <= SIMULATOR_URL.len() + 1 + MAX_FRAGMENT_BYTES);
        assert_eq!(decode(&url), source.as_bytes());
    }

    #[test]
    fn oversized_source_uses_non_secret_error_fragment() {
        let source = "x".repeat(MAX_SOURCE_BYTES + 1);
        assert_eq!(
            url_for_config(&source),
            format!("{SIMULATOR_URL}#ks-config-error=too-large")
        );
    }
}
