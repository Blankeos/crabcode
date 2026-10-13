pub mod anthropic;
pub mod compatible;
pub mod hosted_search;
pub mod openai;

#[allow(unused_imports)]
pub use hosted_search::{
    default_tools_for, should_register_local_websearch, tools_for, HostedSearchSelection,
};

pub use anthropic::Anthropic;
pub use compatible::OpenAICompatible;
pub use openai::OpenAI;

use std::collections::HashMap;

/// Merge caller-supplied HTTP headers onto an outbound request map.
pub(crate) fn apply_extra_headers(
    request_headers: &mut reqwest::header::HeaderMap,
    extra: &HashMap<String, String>,
) {
    for (k, v) in extra {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            request_headers.insert(name, value);
        }
    }
}

/// Returns true when a provider base URL already contains a `/vN` path segment
/// (e.g. `https://opencode.ai/zen/go/v1`, `.../v4`). Providers join their
/// endpoint path onto the base URL, so callers must not prepend another
/// version segment when one is already present (which produced
/// `/v1/v1/responses`-style 404s).
pub(crate) fn base_url_has_version_segment(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).ok().is_some_and(|url| {
        url.path().split('/').any(|segment| {
            segment.strip_prefix('v').is_some_and(|version| {
                !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn apply_extra_headers_inserts_valid_names() {
        let mut map = reqwest::header::HeaderMap::new();
        let extra = HashMap::from([
            ("x-opencode-session".to_string(), "sess-1".to_string()),
            ("not a header".to_string(), "nope".to_string()),
        ]);
        apply_extra_headers(&mut map, &extra);
        assert_eq!(
            map.get("x-opencode-session").and_then(|v| v.to_str().ok()),
            Some("sess-1")
        );
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn detects_version_segment_in_base_url() {
        assert!(base_url_has_version_segment(
            "https://opencode.ai/zen/go/v1"
        ));
        assert!(base_url_has_version_segment(
            "https://opencode.ai/zen/go/v1/"
        ));
        assert!(base_url_has_version_segment("http://localhost:11434/v1"));
        assert!(base_url_has_version_segment(
            "https://gateway.example/api/v4"
        ));
        assert!(base_url_has_version_segment(
            "https://gateway.example/v10/openai"
        ));
        assert!(!base_url_has_version_segment(
            "https://gateway.example?v=/v1"
        ));
        assert!(!base_url_has_version_segment(
            "https://gateway.example/v1beta"
        ));
        assert!(!base_url_has_version_segment("https://api.openai.com"));
        assert!(!base_url_has_version_segment("https://api.anthropic.com"));
    }
}
