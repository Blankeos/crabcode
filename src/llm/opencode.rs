use std::collections::HashMap;

const SESSION_HEADER: &str = "x-opencode-session";
const REQUEST_HEADER: &str = "x-opencode-request";
const CLIENT_HEADER: &str = "x-opencode-client";
const CLIENT_VALUE: &str = "cli";

/// OpenCode Go/Zen require a stable conversation id in `x-opencode-session`.
///
/// Matches OpenCode's own client (`providerID.startsWith("opencode")`), but
/// crabcode's request config stores the *display* name (`OpenCode Go`), so
/// this is ASCII-case-insensitive. Also matches zen/go base URLs.
/// `.devrefs/references/anomalyco/opencode/packages/opencode/src/session/llm/request.ts`
pub(crate) fn is_opencode_provider(provider_name: &str) -> bool {
    let name = provider_name.trim().to_ascii_lowercase();
    if name == "opencode" {
        return true;
    }
    name.starts_with("opencode-")
        || name.starts_with("opencode ")
        || name.starts_with("opencode_")
        || name.starts_with("opencode/")
        || name.starts_with("opencode:")
}

pub(crate) fn is_opencode_endpoint(base_url: &str) -> bool {
    base_url
        .trim()
        .to_ascii_lowercase()
        .contains("opencode.ai/zen")
}

pub(crate) fn should_attach_session_headers(provider_name: &str, base_url: &str) -> bool {
    is_opencode_provider(provider_name) || is_opencode_endpoint(base_url)
}

fn remove_header_case_insensitive(headers: &mut HashMap<String, String>, name: &str) {
    headers.retain(|k, _| !k.eq_ignore_ascii_case(name));
}

fn insert_header_case_insensitive(
    headers: &mut HashMap<String, String>,
    name: &str,
    value: String,
) {
    remove_header_case_insensitive(headers, name);
    headers.insert(name.to_string(), value);
}

fn merge_headers_case_insensitive(
    dst: &mut HashMap<String, String>,
    src: &HashMap<String, String>,
) {
    for (k, v) in src {
        remove_header_case_insensitive(dst, k);
        dst.insert(k.clone(), v.clone());
    }
}

/// Merge base headers with per-call overrides, then stamp OpenCode Go session routing.
///
/// Precedence: `base` < `overrides` < sticky session headers. Overrides win
/// over base for generic headers, but the stable `x-opencode-session` stamp
/// always wins so callers can't break conversation stickiness by accident.
/// Uses `session_id` when present (sticky per conversation); otherwise mints a
/// one-off id so auxiliary calls (title, compaction) still satisfy Console Go.
pub(crate) fn ensure_session_headers(
    provider_name: &str,
    base_url: &str,
    base_headers: &HashMap<String, String>,
    session_id: Option<&str>,
    overrides: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut headers = base_headers.clone();
    merge_headers_case_insensitive(&mut headers, overrides);
    if !should_attach_session_headers(provider_name, base_url) {
        return headers;
    }
    let session = session_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| id.to_string())
        .unwrap_or_else(cuid2::create_id);
    inject_session_headers(&mut headers, &session);
    headers
}

/// Stamp OpenCode Go/Zen routing headers onto an outbound request.
///
/// OpenCode sends `x-opencode-session` (conversation), `x-opencode-request`
/// (per invocation), `x-opencode-client`, and a non-generic `User-Agent`.
/// Case-insensitive so `X-Opencode-Session` / `user-agent` variants can't
/// duplicate and collapse nondeterministically in the `HeaderMap`.
pub(crate) fn inject_session_headers(headers: &mut HashMap<String, String>, session_id: &str) {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return;
    }
    insert_header_case_insensitive(headers, SESSION_HEADER, session_id.to_string());
    insert_header_case_insensitive(headers, REQUEST_HEADER, cuid2::create_id());
    insert_header_case_insensitive(headers, CLIENT_HEADER, CLIENT_VALUE.to_string());
    if !headers.keys().any(|k| k.eq_ignore_ascii_case("User-Agent")) {
        headers.insert(
            "User-Agent".to_string(),
            format!("crabcode/{}", env!("CARGO_PKG_VERSION")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_opencode_provider_ids() {
        assert!(is_opencode_provider("opencode-go"));
        assert!(is_opencode_provider("opencode-zen"));
        assert!(is_opencode_provider("opencode"));
        // Request config stores the models.dev display name, not the id.
        assert!(is_opencode_provider("OpenCode Go"));
        assert!(is_opencode_provider("OpenCode Zen"));
        assert!(!is_opencode_provider("openai"));
        assert!(!is_opencode_provider("crof"));
        assert!(!is_opencode_provider("opencodefoo"));
    }

    #[test]
    fn detects_opencode_zen_endpoints() {
        assert!(is_opencode_endpoint("https://opencode.ai/zen/go/v1"));
        assert!(is_opencode_endpoint("https://opencode.ai/zen/v1"));
        assert!(!is_opencode_endpoint("https://api.openai.com/v1"));
        assert!(should_attach_session_headers(
            "OpenCode Go",
            "https://opencode.ai/zen/go/v1"
        ));
    }

    #[test]
    fn injects_stable_session_and_identifying_ua() {
        let mut headers = HashMap::new();
        inject_session_headers(&mut headers, "sess-1");
        assert_eq!(
            headers.get(SESSION_HEADER).map(String::as_str),
            Some("sess-1")
        );
        assert!(headers.get(REQUEST_HEADER).is_some_and(|id| !id.is_empty()));
        assert_eq!(
            headers.get(CLIENT_HEADER).map(String::as_str),
            Some(CLIENT_VALUE)
        );
        assert_eq!(
            headers.get("User-Agent").map(String::as_str),
            Some(concat!("crabcode/", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn inject_is_case_insensitive() {
        let mut headers = HashMap::from([
            ("X-Opencode-Session".to_string(), "stale".to_string()),
            ("user-agent".to_string(), "keep".to_string()),
        ]);
        inject_session_headers(&mut headers, "sess-1");
        assert_eq!(
            headers.get(SESSION_HEADER).map(String::as_str),
            Some("sess-1")
        );
        assert!(headers.keys().all(|k| k != "X-Opencode-Session"));
        assert_eq!(headers.get("user-agent").map(String::as_str), Some("keep"));
    }

    #[test]
    fn skips_blank_session_id() {
        let mut headers = HashMap::new();
        inject_session_headers(&mut headers, "  ");
        assert!(headers.is_empty());
    }

    #[test]
    fn ensure_headers_is_noop_for_other_providers() {
        let existing = HashMap::from([("x-grok-session-id".to_string(), "keep".to_string())]);
        let headers = ensure_session_headers(
            "xai",
            "https://api.x.ai",
            &existing,
            Some("sess-1"),
            &HashMap::new(),
        );
        assert_eq!(headers, existing);
        assert!(!headers.contains_key(SESSION_HEADER));
    }

    #[test]
    fn ensure_headers_uses_session_id_when_present() {
        let headers = ensure_session_headers(
            "OpenCode Go",
            "https://opencode.ai/zen/go/v1",
            &HashMap::new(),
            Some("sticky-sess"),
            &HashMap::new(),
        );
        assert_eq!(
            headers.get(SESSION_HEADER).map(String::as_str),
            Some("sticky-sess")
        );
    }

    #[test]
    fn ensure_headers_mints_session_when_missing() {
        let headers = ensure_session_headers(
            "opencode-go",
            "https://opencode.ai/zen/go/v1",
            &HashMap::new(),
            None,
            &HashMap::new(),
        );
        assert!(headers.get(SESSION_HEADER).is_some_and(|id| !id.is_empty()));
    }

    #[test]
    fn overrides_win_over_base_but_session_stays_sticky() {
        let base = HashMap::from([("x-custom".to_string(), "base".to_string())]);
        let overrides = HashMap::from([
            ("x-custom".to_string(), "override".to_string()),
            (SESSION_HEADER.to_string(), "spoof".to_string()),
        ]);
        let headers = ensure_session_headers(
            "OpenCode Go",
            "https://opencode.ai/zen/go/v1",
            &base,
            Some("sticky-sess"),
            &overrides,
        );
        assert_eq!(
            headers.get("x-custom").map(String::as_str),
            Some("override")
        );
        assert_eq!(
            headers.get(SESSION_HEADER).map(String::as_str),
            Some("sticky-sess")
        );
    }

    #[test]
    fn child_session_overwrites_parent_session_header() {
        let mut headers = HashMap::new();
        inject_session_headers(&mut headers, "parent");
        inject_session_headers(&mut headers, "child");
        assert_eq!(
            headers.get(SESSION_HEADER).map(String::as_str),
            Some("child")
        );
    }
}
