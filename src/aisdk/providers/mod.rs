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

/// Returns true when a provider base URL already contains a `/vN` path segment
/// (e.g. `https://opencode.ai/zen/go/v1`, `.../v4`). Providers join their
/// endpoint path onto the base URL, so callers must not prepend another
/// version segment when one is already present (which produced
/// `/v1/v1/responses`-style 404s).
pub(crate) fn base_url_has_version_segment(base_url: &str) -> bool {
    // Check if the URL path already contains a /vN segment (e.g., /v4, /v1)
    if let Some(pos) = base_url.find("://") {
        let after_scheme = &base_url[pos + 3..];
        if let Some(path_start) = after_scheme.find('/') {
            let path = &after_scheme[path_start..];
            // Match /vN where N is one or more digits, followed by / or end of string
            let bytes = path.as_bytes();
            for i in 0..bytes.len().saturating_sub(2) {
                if bytes[i] == b'/'
                    && bytes[i + 1] == b'v'
                    && bytes[i + 2].is_ascii_digit()
                    && (i + 3 >= bytes.len() || bytes[i + 3] == b'/')
                {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_version_segment_in_base_url() {
        assert!(base_url_has_version_segment(
            "https://opencode.ai/zen/go/v1"
        ));
        assert!(base_url_has_version_segment(
            "https://opencode.ai/zen/go/v1/"
        ));
        assert!(base_url_has_version_segment("http://localhost:11434/v1"));
        assert!(!base_url_has_version_segment("https://api.openai.com"));
        assert!(!base_url_has_version_segment("https://api.anthropic.com"));
    }
}
