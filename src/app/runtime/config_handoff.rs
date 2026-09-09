//! One-shot handoff of active TOML and optional compact layouts to the web simulator.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use miniz_oxide::deflate::compress_to_vec_zlib;
use std::time::Duration;

const SIMULATOR_URL: &str = "https://dccif.github.io/KeySteer/simulator";
const PROTOCOL_VERSION: &str = "v1";
const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_FRAGMENT_BYTES: usize = 24 * 1024;
const MAX_WORKSPACE_BYTES: usize = 2 * 1024 * 1024;
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

pub(super) fn url_for_workspace(source: &str, layouts: Result<Option<Vec<u8>>, String>) -> String {
    if matches!(&layouts, Ok(None)) {
        return url_for_config(source);
    }
    if source.len() > MAX_SOURCE_BYTES {
        return format!("{SIMULATOR_URL}#ks-config-error=too-large");
    }
    #[derive(serde::Serialize)]
    struct Workspace<'a> {
        source: &'a str,
        layouts: Option<String>,
        layout_error: Option<String>,
    }
    let (layouts, layout_error) = match layouts {
        Ok(Some(bytes)) => (Some(URL_SAFE_NO_PAD.encode(bytes)), None),
        Err(error) => (None, Some(error)),
        Ok(None) => (None, None),
    };
    let Ok(bytes) = serde_json::to_vec(&Workspace {
        source,
        layouts,
        layout_error,
    }) else {
        return format!("{SIMULATOR_URL}#ks-config-error=too-large");
    };
    if bytes.len() > MAX_WORKSPACE_BYTES {
        return format!("{SIMULATOR_URL}#ks-config-error=too-large");
    }
    let encoded = URL_SAFE_NO_PAD.encode(compress_to_vec_zlib(&bytes, 6));
    let fragment = format!("ks-config=v2.{}.{encoded}", bytes.len());
    if fragment.len() > MAX_FRAGMENT_BYTES {
        return format!("{SIMULATOR_URL}#ks-config-error=too-large");
    }
    format!("{SIMULATOR_URL}#{fragment}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::inflate::decompress_to_vec_zlib;
    #[test]
    fn workspace_handoff_carries_the_exact_layout_file_and_config_together() {
        let file = include_bytes!("../../../tests/fixtures/window-layouts-v1.kslayout");
        let source = "# preserved comment\n[normal.bindings]\nh = \"move_left\"\n";
        let url = url_for_workspace(source, Ok(Some(file.to_vec())));
        let payload = url
            .split_once("#ks-config=v2.")
            .unwrap()
            .1
            .split_once('.')
            .unwrap()
            .1;
        let json = decompress_to_vec_zlib(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(document["source"], source);
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(document["layouts"].as_str().unwrap())
                .unwrap(),
            file
        );
        assert!(document["layout_error"].is_null());
        assert_eq!(url_for_workspace(source, Ok(None)), url_for_config(source));
    }

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
