//! One-shot handoff of the active TOML to the static web simulator.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use miniz_oxide::deflate::compress_to_vec_zlib;
use std::time::Duration;

const SIMULATOR_URL: &str = "https://dccif.github.io/KeySteer/simulator";
const PROTOCOL_VERSION: &str = "v1";
const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_FRAGMENT_BYTES: usize = 24 * 1024;
pub(crate) const OPEN_DEBOUNCE: Duration = Duration::from_secs(2);

pub(crate) fn url_for_config(source: &str) -> String {
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
    fn empty_config_is_supported() {
        assert_eq!(decode(&url_for_config("")), b"");
    }

    #[test]
    fn shipped_configuration_fits_the_browser_handoff() {
        let source = include_str!("../../keysteer.default.toml");
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

    #[test]
    fn source_at_the_raw_limit_is_supported_when_compressible() {
        let source = "x".repeat(MAX_SOURCE_BYTES);
        assert_eq!(decode(&url_for_config(&source)), source.as_bytes());
    }

    #[test]
    fn incompressible_fragment_uses_the_bounded_error_url() {
        let mut state = 0x1234_5678_u32;
        let source = (0..64 * 1024)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                char::from_u32((state >> 24) % 95 + 32).unwrap()
            })
            .collect::<String>();
        assert_eq!(
            url_for_config(&source),
            format!("{SIMULATOR_URL}#ks-config-error=too-large")
        );
    }
}
