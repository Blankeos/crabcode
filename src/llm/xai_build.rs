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
        format!("crabcode/{}", crate::version::CURRENT),
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
        assert!(retry_policy_for(&HashMap::new()).is_none());
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
            Some(format!("crabcode/{}", crate::version::CURRENT).as_str())
        );
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
