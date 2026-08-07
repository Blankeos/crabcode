use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::aisdk::providers::openai::HttpResponseRetryPolicy;

pub(crate) const BASE_URL: &str = "https://cli-chat-proxy.grok.com";
pub(crate) const MODEL: &str = "grok-4.5";
const TOKEN_AUTH_HEADER: &str = "X-XAI-Token-Auth";
const TOKEN_AUTH_VALUE: &str = "xai-grok-cli";
const VERSION_HEADER: &str = "x-grok-client-version";

const PROTOCOL_VERSION_FALLBACK: &str = "0.2.111";
const VERSION_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const VERSION_RETRY_TTL: Duration = Duration::from_secs(5 * 60);
const VERSION_URLS: &[&str] = &[
    "https://x.ai/cli/stable",
    "https://storage.googleapis.com/grok-build-public-artifacts/cli/stable",
];
static VERSION_CACHE: OnceLock<Mutex<Option<CachedVersion>>> = OnceLock::new();

#[derive(Clone)]
struct CachedVersion {
    version: String,
    cached_at: Instant,
    ttl: Duration,
}

#[derive(Debug, Default)]
pub(crate) struct XaiBuildRetryPolicy;

pub(crate) struct RequestOverrides {
    pub(crate) api_key: String,
    pub(crate) base_url: &'static str,
    pub(crate) model: &'static str,
    pub(crate) headers: std::collections::HashMap<String, String>,
}

#[async_trait::async_trait]
impl HttpResponseRetryPolicy for XaiBuildRetryPolicy {
    async fn retry_headers(
        &self,
        status: reqwest::StatusCode,
    ) -> Option<reqwest::header::HeaderMap> {
        if status != reqwest::StatusCode::UPGRADE_REQUIRED {
            return None;
        }

        let version = force_refresh_protocol_version().await?;
        let version = reqwest::header::HeaderValue::from_str(&version).ok()?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(VERSION_HEADER, version);
        Some(headers)
    }
}

pub(crate) fn retry_policy_for(
    headers: &std::collections::HashMap<String, String>,
) -> Option<std::sync::Arc<dyn HttpResponseRetryPolicy>> {
    let is_build_request = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case(TOKEN_AUTH_HEADER) && value == TOKEN_AUTH_VALUE
    });
    is_build_request.then(|| {
        std::sync::Arc::new(XaiBuildRetryPolicy) as std::sync::Arc<dyn HttpResponseRetryPolicy>
    })
}

pub(crate) async fn request_overrides(oauth_access: String) -> RequestOverrides {
    let protocol_version = protocol_version().await;
    request_overrides_with_version(oauth_access, protocol_version)
}

fn request_overrides_with_version(
    oauth_access: String,
    protocol_version: String,
) -> RequestOverrides {
    let mut headers = std::collections::HashMap::new();
    headers.insert(
        "User-Agent".to_string(),
        format!("crabcode/{}", env!("CARGO_PKG_VERSION")),
    );
    headers.insert(TOKEN_AUTH_HEADER.to_string(), TOKEN_AUTH_VALUE.to_string());
    headers.insert("x-grok-model-override".to_string(), MODEL.to_string());
    headers.insert(
        "x-grok-client-identifier".to_string(),
        "crabcode".to_string(),
    );
    headers.insert(VERSION_HEADER.to_string(), protocol_version);
    headers.insert("x-grok-client-mode".to_string(), "default".to_string());

    RequestOverrides {
        api_key: oauth_access,
        base_url: BASE_URL,
        model: MODEL,
        headers,
    }
}

/// True when this request is routed through the Grok Build cli-chat-proxy transport.
pub(crate) fn is_build_transport(headers: &std::collections::HashMap<String, String>) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case(TOKEN_AUTH_HEADER) && value == TOKEN_AUTH_VALUE
    })
}

/// Identity stamped on every cli-chat-proxy request for sticky routing / cache affinity.
///
/// Mirrors Grok Build's main-turn + side-call header set (minus deployment/user which
/// require managed-enterprise context we may not have).
#[derive(Debug, Clone)]
pub(crate) struct SessionAffinity {
    /// Sticky session id (`x-grok-session-id`).
    pub session_id: String,
    /// Sticky conversation id (`x-grok-conv-id`). Usually equals `session_id`.
    /// Side/aux calls that must ride a parent prefix use the **parent** session id here
    /// (and as `prompt_cache_key`) even if telemetry wants a different req id.
    pub conv_id: String,
    /// Unique per model invocation (`x-grok-req-id`).
    pub req_id: String,
    /// 0-based user-turn index within the conversation (`x-grok-turn-idx`).
    pub turn_idx: Option<u32>,
    /// Process-stable agent id (`x-grok-agent-id`). Defaults to [`process_agent_id`].
    pub agent_id: Option<String>,
}

impl SessionAffinity {
    /// Main-turn / tool-loop affinity: session == conv, fresh req id.
    pub fn main_turn(session_id: impl Into<String>, turn_idx: u32) -> Self {
        let session_id = session_id.into();
        Self {
            conv_id: session_id.clone(),
            session_id,
            req_id: new_req_id(),
            turn_idx: Some(turn_idx),
            agent_id: Some(process_agent_id()),
        }
    }

    /// Side/aux call that reuses a parent conversation's cached prefix.
    ///
    /// `prompt_cache_key` and sticky headers stay on `parent_session_id`; `req_id`
    /// is unique so telemetry can still distinguish the aux call.
    pub fn parent_cached_aux(parent_session_id: impl Into<String>, turn_idx: u32) -> Self {
        let parent = parent_session_id.into();
        Self {
            session_id: parent.clone(),
            conv_id: parent,
            req_id: format!("aux-{}", new_req_id()),
            turn_idx: Some(turn_idx),
            agent_id: Some(process_agent_id()),
        }
    }

    /// Child session (subagent) — intentionally **not** parent-cached: different
    /// system prompt / tool set would miss the parent prefix and can pollute routing.
    pub fn child_session(child_session_id: impl Into<String>) -> Self {
        let session_id = child_session_id.into();
        Self {
            conv_id: session_id.clone(),
            session_id,
            req_id: new_req_id(),
            turn_idx: Some(0),
            agent_id: Some(process_agent_id()),
        }
    }
}

/// Sticky session affinity headers for cli-chat-proxy prompt-cache routing.
///
/// `prompt_cache_key` remains a separate body field and should also be set to the
/// sticky session/conv id for Responses sticky routing.
pub(crate) fn inject_session_affinity_headers(
    headers: &mut std::collections::HashMap<String, String>,
    affinity: &SessionAffinity,
) {
    if affinity.session_id.is_empty() {
        return;
    }
    headers.insert("x-grok-session-id".to_string(), affinity.session_id.clone());
    let conv = if affinity.conv_id.is_empty() {
        affinity.session_id.as_str()
    } else {
        affinity.conv_id.as_str()
    };
    headers.insert("x-grok-conv-id".to_string(), conv.to_string());
    if !affinity.req_id.is_empty() {
        headers.insert("x-grok-req-id".to_string(), affinity.req_id.clone());
    }
    if let Some(turn_idx) = affinity.turn_idx {
        headers.insert("x-grok-turn-idx".to_string(), turn_idx.to_string());
    }
    let agent_id = affinity.agent_id.clone().unwrap_or_else(process_agent_id);
    if !agent_id.is_empty() {
        headers.insert("x-grok-agent-id".to_string(), agent_id);
    }
}

/// Convenience for callers that only have session + req (tests / simple paths).
pub(crate) fn inject_session_affinity_headers_simple(
    headers: &mut std::collections::HashMap<String, String>,
    session_id: &str,
    req_id: &str,
) {
    inject_session_affinity_headers(
        headers,
        &SessionAffinity {
            session_id: session_id.to_string(),
            conv_id: session_id.to_string(),
            req_id: req_id.to_string(),
            turn_idx: None,
            agent_id: Some(process_agent_id()),
        },
    );
}

pub(crate) fn new_req_id() -> String {
    format!("crabcode-{}", cuid2::create_id())
}

/// Process-stable agent id (Grok Build `x-grok-agent-id` counterpart).
///
/// Not persisted across restarts — good enough for proxy affinity within a run.
pub(crate) fn process_agent_id() -> String {
    static AGENT_ID: OnceLock<String> = OnceLock::new();
    AGENT_ID
        .get_or_init(|| format!("crabcode-agent-{}", cuid2::create_id()))
        .clone()
}

/// Count 0-based user turns in a converted model message list (for `x-grok-turn-idx`).
pub(crate) fn user_turn_idx_from_aisdk_messages(messages: &[crate::aisdk::core::Message]) -> u32 {
    messages
        .iter()
        .filter(|m| matches!(m, crate::aisdk::core::Message::User(_)))
        .count()
        .saturating_sub(1) as u32
}

pub(crate) async fn protocol_version() -> String {
    if let Some(version) = configured_protocol_version() {
        return version;
    }

    if let Some(version) = cached_protocol_version() {
        return version;
    }

    refresh_protocol_version()
        .await
        .unwrap_or_else(|| cache_fallback_version())
}

pub(crate) async fn force_refresh_protocol_version() -> Option<String> {
    if let Some(version) = configured_protocol_version() {
        return Some(version);
    }

    invalidate_protocol_version_cache();
    refresh_protocol_version().await
}

fn configured_protocol_version() -> Option<String> {
    let version = std::env::var("CRABCODE_XAI_BUILD_VERSION").ok()?;
    let version = version.trim();
    is_valid_protocol_version(version).then(|| version.to_string())
}

fn cached_protocol_version() -> Option<String> {
    let cache = VERSION_CACHE.get_or_init(|| Mutex::new(None));
    let cache = cache.lock().ok()?;
    let cached = cache.as_ref()?;
    (cached.cached_at.elapsed() < cached.ttl).then(|| cached.version.clone())
}

async fn refresh_protocol_version() -> Option<String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(4))
        .build()
        .ok()?;

    for url in VERSION_URLS {
        let fetched = async {
            client
                .get(*url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await
        }
        .await;

        if let Ok(version) = fetched {
            let version = version.trim();
            if is_valid_protocol_version(version) {
                let version = version.to_string();
                cache_protocol_version(version.clone(), VERSION_CACHE_TTL);
                return Some(version);
            }
        }
    }

    None
}

fn cache_fallback_version() -> String {
    let fallback = PROTOCOL_VERSION_FALLBACK.to_string();
    cache_protocol_version(fallback.clone(), VERSION_RETRY_TTL);
    fallback
}

fn cache_protocol_version(version: String, ttl: Duration) {
    let cache = VERSION_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut cache) = cache.lock() {
        *cache = Some(CachedVersion {
            version,
            cached_at: Instant::now(),
            ttl,
        });
    }
}

fn invalidate_protocol_version_cache() {
    let cache = VERSION_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut cache) = cache.lock() {
        *cache = None;
    }
}

fn is_valid_protocol_version(version: &str) -> bool {
    let mut parts = version.split('.');
    parts.next().is_some_and(is_ascii_digits)
        && parts.next().is_some_and(is_ascii_digits)
        && parts.next().is_some_and(is_ascii_digits)
        && parts.next().is_none()
}

fn is_ascii_digits(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_protocol_version, request_overrides_with_version, retry_policy_for,
        HttpResponseRetryPolicy, XaiBuildRetryPolicy, BASE_URL, MODEL, TOKEN_AUTH_HEADER,
        TOKEN_AUTH_VALUE,
    };
    use std::collections::HashMap;

    #[test]
    fn version_validation_accepts_semver_triplets_only() {
        assert!(is_valid_protocol_version("0.2.111"));
        assert!(is_valid_protocol_version("10.20.30"));
        assert!(!is_valid_protocol_version("0.2"));
        assert!(!is_valid_protocol_version("v0.2.111"));
        assert!(!is_valid_protocol_version("0.2.111\n"));
        assert!(!is_valid_protocol_version("0.2.111.4"));
    }

    #[test]
    fn build_request_detection_is_header_scoped() {
        let headers =
            HashMap::from([(TOKEN_AUTH_HEADER.to_string(), TOKEN_AUTH_VALUE.to_string())]);
        assert!(retry_policy_for(&headers).is_some());
        assert!(super::is_build_transport(&headers));
        assert!(retry_policy_for(&HashMap::new()).is_none());
        assert!(!super::is_build_transport(&HashMap::new()));
    }

    #[test]
    fn build_transport_constants_match_proxy_contract() {
        assert_eq!(BASE_URL, "https://cli-chat-proxy.grok.com");
        assert_eq!(MODEL, "grok-4.5");
    }

    #[test]
    fn request_overrides_match_proxy_contract() {
        let overrides =
            request_overrides_with_version("oauth-token".to_string(), "9.8.7".to_string());
        assert_eq!(overrides.api_key, "oauth-token");
        assert_eq!(overrides.base_url, BASE_URL);
        assert_eq!(overrides.model, MODEL);
        assert_eq!(
            overrides.headers.get(TOKEN_AUTH_HEADER).map(String::as_str),
            Some(TOKEN_AUTH_VALUE)
        );
        assert_eq!(
            overrides
                .headers
                .get("x-grok-client-version")
                .map(String::as_str),
            Some("9.8.7")
        );
        assert_eq!(
            overrides.headers.get("User-Agent").map(String::as_str),
            Some(concat!("crabcode/", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn session_affinity_headers_are_sticky_and_req_scoped() {
        let mut headers = HashMap::new();
        super::inject_session_affinity_headers_simple(&mut headers, "sess-1", "req-abc");
        assert_eq!(
            headers.get("x-grok-session-id").map(String::as_str),
            Some("sess-1")
        );
        assert_eq!(
            headers.get("x-grok-conv-id").map(String::as_str),
            Some("sess-1")
        );
        assert_eq!(
            headers.get("x-grok-req-id").map(String::as_str),
            Some("req-abc")
        );
        assert!(headers.get("x-grok-agent-id").is_some());

        // Empty session id is a no-op (never stamp blank affinity).
        let mut empty = HashMap::new();
        super::inject_session_affinity_headers_simple(&mut empty, "", "req");
        assert!(empty.is_empty());
    }

    #[test]
    fn main_turn_affinity_includes_turn_and_agent() {
        let affinity = super::SessionAffinity::main_turn("sess-9", 3);
        let mut headers = HashMap::new();
        super::inject_session_affinity_headers(&mut headers, &affinity);
        assert_eq!(
            headers.get("x-grok-turn-idx").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            headers.get("x-grok-agent-id").map(String::as_str),
            Some(super::process_agent_id().as_str())
        );
        assert!(headers
            .get("x-grok-req-id")
            .is_some_and(|id| id.starts_with("crabcode-")));
    }

    #[test]
    fn parent_cached_aux_keeps_parent_ids_and_unique_req() {
        let affinity = super::SessionAffinity::parent_cached_aux("parent-sess", 1);
        assert_eq!(affinity.session_id, "parent-sess");
        assert_eq!(affinity.conv_id, "parent-sess");
        assert!(affinity.req_id.starts_with("aux-"));
    }

    #[tokio::test]
    async fn retry_policy_ignores_non_upgrade_responses() {
        let policy = XaiBuildRetryPolicy;
        assert!(policy
            .retry_headers(reqwest::StatusCode::BAD_REQUEST)
            .await
            .is_none());
    }
}
