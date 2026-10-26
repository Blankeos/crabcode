//! Manual-paste Google Antigravity OAuth. No callback listener is started.
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Duration};
use url::Url;

use super::OAuthCredentials;

const CLIENT_ID_ENV: &str = "CRABCODE_ANTIGRAVITY_CLIENT_ID";
const CLIENT_SECRET_ENV: &str = "CRABCODE_ANTIGRAVITY_CLIENT_SECRET";
pub const REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
pub const ANTIGRAVITY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const ANTIGRAVITY_VERSION: &str = "2.24.0";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const MAX_CALLBACK_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

pub fn user_agent() -> &'static str {
    "antigravity/hub/2.24.0 (aidev_client; os_type=darwin; arch=arm64; cl=1000335682)"
}

struct OAuthClient {
    id: String,
    secret: String,
}

impl OAuthClient {
    fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<std::ffi::OsString>) -> Result<Self> {
        let mut required = |name| -> Result<String> {
            lookup(name)
                .and_then(|value| value.into_string().ok())
                .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace)
                    && !value.chars().any(char::is_control))
                .ok_or_else(|| anyhow!("Set {name} to a valid OAuth client value before connecting Google Antigravity"))
        };
        Ok(Self {
            id: required(CLIENT_ID_ENV)?,
            secret: required(CLIENT_SECRET_ENV)?,
        })
    }

    fn token_form(&self) -> Vec<(&str, &str)> {
        vec![
            ("client_id", self.id.as_str()),
            ("client_secret", self.secret.as_str()),
        ]
    }
}

pub struct BrowserAuthorization {
    url: String,
    verifier: String,
    state: String,
    client: OAuthClient,
}

impl BrowserAuthorization {
    pub fn new() -> Result<Self> {
        Self::with_client(OAuthClient::from_env()?)
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::with_client(OAuthClient {
            id: "test-oauth-client".into(),
            secret: "test-oauth-secret".into(),
        })
        .unwrap()
    }

    fn with_client(client: OAuthClient) -> Result<Self> {
        let mut verifier = [0u8; 32];
        let mut state = [0u8; 32];
        OsRng
            .try_fill_bytes(&mut verifier)
            .map_err(|_| anyhow!("OAuth random generation failed"))?;
        OsRng
            .try_fill_bytes(&mut state)
            .map_err(|_| anyhow!("OAuth random generation failed"))?;
        let verifier = URL_SAFE_NO_PAD.encode(verifier);
        let state = URL_SAFE_NO_PAD.encode(state);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", client.id.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ]);
        Ok(Self {
            url: url.into(),
            verifier,
            state,
            client,
        })
    }

    pub fn authorization_url(&self) -> &str {
        &self.url
    }

    pub fn validate_callback(&self, input: &str) -> Result<String> {
        if input.len() > MAX_CALLBACK_BYTES {
            bail!("OAuth callback is too long");
        }
        let input = input.trim();
        if input.chars().any(char::is_control) {
            bail!("Invalid OAuth callback");
        }
        // Compare the original spelling too: URL normalization must not admit another path/origin.
        if input.split('?').next() != Some(REDIRECT_URI) {
            bail!("OAuth callback must use the expected localhost address");
        }
        let url = Url::parse(input).map_err(|_| anyhow!("Invalid OAuth callback URL"))?;
        if url.fragment().is_some() {
            bail!("OAuth callback must not contain a fragment");
        }
        let mut names = HashSet::new();
        let mut code = None;
        let mut state = None;
        let mut failed = false;
        for (name, value) in url.query_pairs() {
            if !names.insert(name.to_string()) {
                bail!("Duplicate OAuth callback field");
            }
            match name.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" | "error_description" | "error_uri" => failed = true,
                _ => {}
            }
        }
        if state.as_deref() != Some(self.state.as_str()) {
            bail!("OAuth callback state is missing or does not match this login");
        }
        if failed {
            bail!("Google authorization was denied or failed; start a new login");
        }
        code.filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
            .ok_or_else(|| anyhow!("OAuth callback is missing a valid authorization code"))
    }

    pub async fn complete(self, callback_url: &str) -> Result<OAuthCredentials> {
        let code = self.validate_callback(callback_url)?;
        let client = http_client()?;
        let started = chrono::Utc::now().timestamp_millis();
        let mut form = self.client.token_form();
        form.extend([
            ("grant_type", "authorization_code"),
            ("redirect_uri", REDIRECT_URI),
            ("code", code.as_str()),
            ("code_verifier", self.verifier.as_str()),
        ]);
        let response = client
            .post(TOKEN_ENDPOINT)
            .form(&form)
            .send()
            .await
            .map_err(|_| anyhow!("Google token exchange request failed"))?;
        let payload: TokenResponse = read_json(response, "Google token exchange").await?;
        let mut credentials = payload.into_credentials(None, started)?;
        credentials.project_id = Some(resolve_project(&credentials.access).await?);
        Ok(credentials)
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(user_agent())
        .build()
        .map_err(|_| anyhow!("Could not create Google HTTP client"))
}

async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    operation: &str,
) -> Result<T> {
    if !response.status().is_success() {
        bail!("{operation} failed (HTTP {})", response.status().as_u16());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("{operation} response could not be read"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            bail!("{operation} response is too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("{operation} returned an invalid response"))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
}
impl TokenResponse {
    fn into_credentials(
        self,
        previous_refresh: Option<&str>,
        started: i64,
    ) -> Result<OAuthCredentials> {
        if self.access_token.trim().is_empty() || self.expires_in <= 0 {
            bail!("Google returned invalid token credentials");
        }
        let refresh = self
            .refresh_token
            .filter(|s| !s.trim().is_empty())
            .or_else(|| previous_refresh.map(str::to_owned))
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                anyhow!("Google did not return a refresh token; restart login and grant consent")
            })?;
        let expires = self
            .expires_in
            .checked_mul(1000)
            .and_then(|duration| started.checked_add(duration))
            .ok_or_else(|| anyhow!("Google returned an invalid token expiry"))?;
        Ok(OAuthCredentials {
            access: self.access_token,
            refresh,
            expires,
            account_id: None,
            enterprise_url: None,
            project_id: None,
        })
    }
}

pub async fn refresh_access_token(refresh_token: &str) -> Result<OAuthCredentials> {
    if refresh_token.trim().is_empty() {
        bail!("Google refresh token is missing");
    }
    let oauth_client = OAuthClient::from_env()?;
    let mut form = oauth_client.token_form();
    form.extend([
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ]);
    let started = chrono::Utc::now().timestamp_millis();
    let response = http_client()?
        .post(TOKEN_ENDPOINT)
        .form(&form)
        .send()
        .await
        .map_err(|_| anyhow!("Google token refresh request failed"))?;
    let payload: TokenResponse = read_json(response, "Google token refresh").await?;
    payload.into_credentials(Some(refresh_token), started)
}

static REQUEST_CREDENTIALS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn load_request_credentials() -> Result<OAuthCredentials> {
    let dao = crate::persistence::AuthDAO::new()
        .map_err(|_| anyhow!("Could not open Google Antigravity OAuth credentials"))?;
    load_request_credentials_with(
        || {
            let config = dao
                .get_provider("google")
                .map_err(|_| anyhow!("Could not read Google Antigravity OAuth credentials"))?;
            let Some(crate::persistence::AuthConfig::OAuth {
                refresh,
                access,
                expires,
                account_id,
                enterprise_url,
                project_id,
            }) = config
            else {
                bail!("Google Antigravity OAuth credentials are missing. Please reconnect Google Antigravity OAuth.");
            };
            Ok(OAuthCredentials {
                refresh,
                access,
                expires,
                account_id,
                enterprise_url,
                project_id,
            })
        },
        |refresh| async move { refresh_access_token(&refresh).await },
        |access| async move { resolve_project(&access).await },
        |credentials| {
            dao.set_provider(
                "google".to_string(),
                crate::persistence::AuthConfig::OAuth {
                    refresh: credentials.refresh,
                    access: credentials.access,
                    expires: credentials.expires,
                    account_id: credentials.account_id,
                    enterprise_url: credentials.enterprise_url,
                    project_id: credentials.project_id,
                },
            )
            .map_err(|_| anyhow!("Could not save Google Antigravity OAuth credentials"))
        },
        || chrono::Utc::now().timestamp_millis(),
    )
    .await
}

fn valid_access(access: &str) -> bool {
    !access.is_empty()
        && !access.chars().any(char::is_whitespace)
        && !access.chars().any(char::is_control)
}

async fn load_request_credentials_with<
    Load,
    Refresh,
    RefreshFuture,
    Resolve,
    ResolveFuture,
    Persist,
    Clock,
>(
    load: Load,
    refresh: Refresh,
    resolve: Resolve,
    persist: Persist,
    now: Clock,
) -> Result<OAuthCredentials>
where
    Load: FnOnce() -> Result<OAuthCredentials>,
    Refresh: FnOnce(String) -> RefreshFuture,
    RefreshFuture: std::future::Future<Output = Result<OAuthCredentials>>,
    Resolve: FnOnce(String) -> ResolveFuture,
    ResolveFuture: std::future::Future<Output = Result<String>>,
    Persist: FnOnce(OAuthCredentials) -> Result<()>,
    Clock: Fn() -> i64,
{
    // Read after acquiring the lock: another generation may have rotated the refresh token.
    let _guard = REQUEST_CREDENTIALS_LOCK.lock().await;
    let mut credentials = load()?;
    let previous = credentials.clone();
    if credentials.expires <= now().saturating_add(60_000) || !valid_access(&credentials.access) {
        match refresh(credentials.refresh.clone()).await {
            Ok(updated) => {
                if !updated.refresh.trim().is_empty() {
                    credentials.refresh = updated.refresh;
                }
                credentials.access = updated.access;
                credentials.expires = updated.expires;
            }
            Err(_) if credentials.expires > now() && valid_access(&credentials.access) => {}
            Err(_) => bail!("Google Antigravity OAuth token refresh failed. Please reconnect Google Antigravity OAuth."),
        }
    }
    if !valid_access(&credentials.access) || credentials.expires <= now() {
        bail!("Google Antigravity OAuth access token is invalid or expired. Please reconnect Google Antigravity OAuth.");
    }
    if credentials
        .project_id
        .as_deref()
        .is_none_or(|id| id.trim().is_empty())
    {
        let resolved = resolve(credentials.access.clone()).await.map_err(|_| {
            anyhow!("Failed to resolve Google Antigravity OAuth project. Please reconnect Google Antigravity OAuth.")
        })?;
        if project_id(&Value::String(resolved.clone())).is_none() {
            bail!("Google Antigravity OAuth project is invalid. Please reconnect Google Antigravity OAuth.");
        }
        credentials.project_id = Some(resolved);
    }
    if credentials.expires <= now() {
        bail!("Google Antigravity OAuth access token expired while resolving the project. Please retry.");
    }
    if credentials.access != previous.access
        || credentials.refresh != previous.refresh
        || credentials.expires != previous.expires
        || credentials.project_id != previous.project_id
    {
        persist(credentials.clone())?;
    }
    Ok(credentials)
}

fn project_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .or_else(|| value.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}

fn tier_id(payload: &Value) -> &str {
    let tiers = payload.get("allowedTiers").and_then(Value::as_array);
    fn valid_id(tier: &Value) -> Option<&str> {
        tier.get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    }
    tiers
        .and_then(|tiers| {
            tiers.iter().find(|tier| {
                tier.get("isDefault").and_then(Value::as_bool) == Some(true)
                    && valid_id(tier).is_some()
            })
        })
        .and_then(valid_id)
        .or_else(|| tiers.and_then(|tiers| tiers.first()).and_then(valid_id))
        .or_else(|| payload.get("currentTier").and_then(valid_id))
        .unwrap_or("free-tier")
}

fn onboard_body(tier: &str) -> Value {
    json!({ "tier_id": tier, "metadata": { "ide_type": "ANTIGRAVITY", "ide_version": ANTIGRAVITY_VERSION, "ide_name": "antigravity" } })
}

pub async fn resolve_project(access: &str) -> Result<String> {
    if access.trim().is_empty() {
        bail!("Google access token is missing");
    }
    tokio::time::timeout(
        Duration::from_secs(120),
        resolve_project_at(
            &http_client()?,
            ANTIGRAVITY_ENDPOINT,
            access,
            Duration::from_secs(5),
        ),
    )
    .await
    .map_err(|_| anyhow!("Antigravity project discovery timed out; retry login"))?
}

async fn resolve_project_at(
    client: &reqwest::Client,
    endpoint: &str,
    access: &str,
    delay: Duration,
) -> Result<String> {
    let response = client
        .post(format!("{endpoint}/v1internal:loadCodeAssist"))
        .bearer_auth(access)
        .json(&json!({ "metadata": { "ideType": "ANTIGRAVITY" } }))
        .send()
        .await
        .map_err(|_| anyhow!("Antigravity project discovery request failed"))?;
    let payload: Value = read_json(response, "Antigravity project discovery").await?;
    if let Some(project) = project_id(&payload["cloudaicompanionProject"]) {
        return Ok(project);
    }
    let body = onboard_body(tier_id(&payload));
    for attempt in 0..10 {
        let response = client
            .post(format!("{endpoint}/v1internal:onboardUser"))
            .bearer_auth(access)
            .header(
                "User-Agent",
                format!("{} google-api-nodejs-client/10.3.0", user_agent()),
            )
            .header("X-Goog-Api-Client", "gl-node/22.21.1")
            .json(&body)
            .send()
            .await
            .map_err(|_| anyhow!("Antigravity onboarding request failed"))?;
        let payload: Value = read_json(response, "Antigravity onboarding").await?;
        if payload["done"].as_bool() == Some(true) {
            return project_id(&payload["response"]["cloudaicompanionProject"])
                .ok_or_else(|| anyhow!("Antigravity onboarding completed without a managed project; check account eligibility"));
        }
        if attempt < 9 {
            tokio::time::sleep(delay).await;
        }
    }
    bail!("Antigravity did not provision a managed project; check account eligibility and retry login")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture_server(
        responses: Vec<(u16, Value)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .unwrap()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let body = body.to_string();
                let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (endpoint, server)
    }

    fn request_credentials() -> OAuthCredentials {
        OAuthCredentials {
            access: "access".into(),
            refresh: "refresh".into(),
            expires: 120_000,
            account_id: Some("account".into()),
            enterprise_url: Some("enterprise".into()),
            project_id: Some("managed".into()),
        }
    }

    #[tokio::test]
    async fn request_credentials_refresh_preserves_metadata_and_saves_only_changes() {
        let credentials = load_request_credentials_with(
            || Ok(request_credentials()),
            |_| async { panic!("fresh token must not refresh") },
            |_| async { panic!("existing project must not resolve") },
            |_| panic!("unchanged credentials must not save"),
            || 0,
        )
        .await
        .unwrap();
        assert_eq!(credentials.access, "access");
        let credentials = load_request_credentials_with(
            || Ok(request_credentials()),
            |token| async move {
                assert_eq!(token, "refresh");
                let mut updated = request_credentials();
                updated.access = "new-access".into();
                updated.refresh.clear();
                updated.project_id = None;
                updated.expires = 240_000;
                Ok(updated)
            },
            |_| async { panic!("refresh must retain project") },
            |saved| {
                assert_eq!(saved.access, "new-access");
                assert_eq!(saved.refresh, "refresh");
                assert_eq!(saved.account_id.as_deref(), Some("account"));
                assert_eq!(saved.enterprise_url.as_deref(), Some("enterprise"));
                assert_eq!(saved.project_id.as_deref(), Some("managed"));
                Ok(())
            },
            || 60_000,
        )
        .await
        .unwrap();
        assert_eq!(credentials.expires, 240_000);
    }

    #[tokio::test]
    async fn request_credentials_resolve_project_and_reject_invalid_refresh() {
        let credentials = load_request_credentials_with(
            || {
                let mut c = request_credentials();
                c.project_id = None;
                Ok(c)
            },
            |_| async { panic!("fresh token must not refresh") },
            |access| async move {
                assert_eq!(access, "access");
                Ok("resolved".into())
            },
            |saved| {
                assert_eq!(saved.project_id.as_deref(), Some("resolved"));
                Ok(())
            },
            || 0,
        )
        .await
        .unwrap();
        assert_eq!(credentials.project_id.as_deref(), Some("resolved"));
        for invalid_access in ["", "bad\ntoken"] {
            let result = load_request_credentials_with(
                || {
                    let mut c = request_credentials();
                    c.access = invalid_access.into();
                    Ok(c)
                },
                |_| async {
                    let mut c = request_credentials();
                    c.expires = 0;
                    Ok(c)
                },
                |_| async { panic!("expired refresh must not resolve") },
                |_| panic!("expired refresh must not save"),
                || 0,
            )
            .await;
            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn failed_proactive_refresh_uses_only_still_valid_access() {
        let c = load_request_credentials_with(
            || Ok(request_credentials()),
            |_| async { bail!("sensitive-upstream-error") },
            |_| async { panic!("existing project must not resolve") },
            |_| panic!("failed refresh must not save"),
            || 60_000,
        )
        .await
        .unwrap();
        assert_eq!(c.access, "access");
        let error = load_request_credentials_with(
            || Ok(request_credentials()),
            |_| async { bail!("sensitive-upstream-error") },
            |_| async { panic!("expired access must not resolve") },
            |_| panic!("failed refresh must not save"),
            || 120_000,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("reconnect Google Antigravity OAuth"));
        assert!(!error.contains("sensitive-upstream-error"));
    }

    #[tokio::test]
    async fn cancelled_refresh_does_not_save_or_hold_lock() {
        let refresh_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let started = refresh_started.clone();
        let task = tokio::spawn(async move {
            load_request_credentials_with(
                || {
                    let mut c = request_credentials();
                    c.expires = 0;
                    Ok(c)
                },
                |_| async move {
                    started.notify_one();
                    std::future::pending::<Result<OAuthCredentials>>().await
                },
                |_| async { panic!("cancelled refresh must not resolve") },
                |_| panic!("cancelled refresh must not save"),
                || 0,
            )
            .await
        });
        refresh_started.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(
            tokio::time::timeout(Duration::from_secs(1), REQUEST_CREDENTIALS_LOCK.lock())
                .await
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn concurrent_generations_read_rotated_credentials_after_lock() {
        let stored = std::sync::Arc::new(std::sync::Mutex::new(request_credentials()));
        let refreshes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let read = stored.clone();
            let save = stored.clone();
            let refreshes = refreshes.clone();
            tasks.push(tokio::spawn(async move {
                load_request_credentials_with(
                    || Ok(read.lock().unwrap().clone()),
                    |_| async move {
                        refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::task::yield_now().await;
                        let mut c = request_credentials();
                        c.access = "rotated".into();
                        c.refresh = "rotated-refresh".into();
                        c.expires = 240_000;
                        Ok(c)
                    },
                    |_| async { panic!("existing project must not resolve") },
                    |c| {
                        *save.lock().unwrap() = c;
                        Ok(())
                    },
                    || 60_000,
                )
                .await
                .unwrap()
                .access
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), "rotated");
        }
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn project_discovery_and_bounded_onboarding() {
        let (endpoint, server) = fixture_server(vec![
            (
                200,
                json!({"allowedTiers":[{"id":"fixture-tier","isDefault":true}]}),
            ),
            (200, json!({"done":false})),
            (
                200,
                json!({"done":true,"response":{"cloudaicompanionProject":{"id":"managed"}}}),
            ),
        ])
        .await;
        assert_eq!(
            resolve_project_at(
                &http_client().unwrap(),
                &endpoint,
                "test-token",
                Duration::ZERO
            )
            .await
            .unwrap(),
            "managed"
        );
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("POST /v1internal:loadCodeAssist "));
        let body: Value =
            serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, onboard_body("fixture-tier"));
        let (endpoint, server) =
            fixture_server(vec![(200, json!({"cloudaicompanionProject":"direct"}))]).await;
        assert_eq!(
            resolve_project_at(&http_client().unwrap(), &endpoint, "test", Duration::ZERO)
                .await
                .unwrap(),
            "direct"
        );
        assert_eq!(server.await.unwrap().len(), 1);
        let (endpoint, server) =
            fixture_server(vec![(403, json!({"error":"sensitive-body"}))]).await;
        let error = resolve_project_at(&http_client().unwrap(), &endpoint, "test", Duration::ZERO)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("403"));
        assert!(!error.contains("sensitive-body"));
        server.await.unwrap();
        let mut responses = vec![(200, json!({}))];
        responses.extend((0..10).map(|_| (200, json!({"done":false}))));
        let (endpoint, server) = fixture_server(responses).await;
        assert!(
            resolve_project_at(&http_client().unwrap(), &endpoint, "test", Duration::ZERO)
                .await
                .is_err()
        );
        assert_eq!(server.await.unwrap().len(), 11);
        let (endpoint, server) =
            fixture_server(vec![(200, json!({})), (200, json!({"done":true}))]).await;
        assert!(
            resolve_project_at(&http_client().unwrap(), &endpoint, "test", Duration::ZERO)
                .await
                .is_err()
        );
        server.await.unwrap();
    }
    fn callback(session: &BrowserAuthorization, fields: &str) -> String {
        format!("{REDIRECT_URI}?state={}&{fields}", session.state)
    }

    #[test]
    fn client_configuration_requires_both_values_without_echoing_them() {
        let error = OAuthClient::from_lookup(|_| None)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(CLIENT_ID_ENV));
        for invalid in ["", " ", "private value", "private\nvalue"] {
            let error = OAuthClient::from_lookup(|name| {
                Some(if name == CLIENT_ID_ENV {
                    "configured-client".into()
                } else {
                    invalid.into()
                })
            })
            .err()
            .unwrap()
            .to_string();
            assert!(error.contains(CLIENT_SECRET_ENV));
            assert!(!error.contains("private"));
        }
        let client = OAuthClient::from_lookup(|name| {
            Some(if name == CLIENT_ID_ENV {
                "configured-client".into()
            } else {
                "configured-secret".into()
            })
        })
        .unwrap();
        assert_eq!(
            client.token_form(),
            vec![
                ("client_id", "configured-client"),
                ("client_secret", "configured-secret"),
            ]
        );
        let authorization = BrowserAuthorization::with_client(client).unwrap();
        let url = Url::parse(authorization.authorization_url()).unwrap();
        assert!(url
            .query_pairs()
            .any(|(key, value)| key == "client_id" && value == "configured-client"));
        assert!(!authorization
            .authorization_url()
            .contains("configured-secret"));
        assert_eq!(authorization.client.token_form()[0].1, "configured-client");
    }
    #[test]
    fn authorization_has_independent_random_state_and_pkce() {
        let a = BrowserAuthorization::for_test();
        let b = BrowserAuthorization::for_test();
        assert_ne!(a.state, b.state);
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.state, a.verifier);
        assert_eq!(URL_SAFE_NO_PAD.decode(&a.state).unwrap().len(), 32);
        assert_eq!(URL_SAFE_NO_PAD.decode(&a.verifier).unwrap().len(), 32);
        let url = Url::parse(a.authorization_url()).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            pairs["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(a.verifier.as_bytes()))
        );
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["redirect_uri"], REDIRECT_URI);
        assert_eq!(pairs["client_id"], "test-oauth-client");
        assert!(!a.authorization_url().contains("test-oauth-secret"));
        assert!(!a.authorization_url().contains(&a.verifier));
    }
    #[test]
    fn callback_validation() {
        let a = BrowserAuthorization::for_test();
        assert_eq!(
            a.validate_callback(&callback(&a, "code=a%2Fb")).unwrap(),
            "a/b"
        );
        for fields in [
            "code=",
            "code=%20",
            "code=%0A",
            "code=x&code=y",
            "code=x&%63ode=y",
            "code=x&state=x",
            "code=x&error=access_denied",
            "error=denied",
            "code=x#error",
            "code=x&error_description=secret",
        ] {
            assert!(
                a.validate_callback(&callback(&a, fields)).is_err(),
                "{fields}"
            );
        }
        let good = callback(&a, "code=secret");
        for prefix in [
            "https://localhost:51121/oauth-callback",
            "http://127.0.0.1:51121/oauth-callback",
            "http://localhost:51122/oauth-callback",
            "http://localhost:51121/other",
            "http://localhost:51121/a/../oauth-callback",
            "http://user@localhost:51121/oauth-callback",
            "http://localhost.evil:51121/oauth-callback",
        ] {
            assert!(a
                .validate_callback(&good.replacen(REDIRECT_URI, prefix, 1))
                .is_err());
        }
        assert!(a
            .validate_callback(&format!("{REDIRECT_URI}?code=secret"))
            .is_err());
        assert!(a
            .validate_callback(&format!("{REDIRECT_URI}?state=wrong&code=secret"))
            .is_err());
        assert!(a
            .validate_callback(&"x".repeat(MAX_CALLBACK_BYTES + 1))
            .is_err());
        assert!(!a
            .validate_callback(&format!("{REDIRECT_URI}?code=secret"))
            .unwrap_err()
            .to_string()
            .contains("secret"));
    }
    #[test]
    fn token_conversion() {
        let payload = || TokenResponse {
            access_token: "access".into(),
            expires_in: 3600,
            refresh_token: None,
        };
        assert!(payload().into_credentials(None, 1000).is_err());
        let credentials = payload()
            .into_credentials(Some("old-refresh"), 1000)
            .unwrap();
        assert_eq!(credentials.refresh, "old-refresh");
        assert_eq!(credentials.expires, 3_601_000);
        assert!(credentials.project_id.is_none());
        let mut value = payload();
        value.refresh_token = Some("new-refresh".into());
        assert_eq!(
            value.into_credentials(Some("old"), 0).unwrap().refresh,
            "new-refresh"
        );
        let mut value = payload();
        value.expires_in = i64::MAX;
        assert!(value.into_credentials(Some("old"), 0).is_err());
        let mut value = payload();
        value.access_token.clear();
        assert!(value.into_credentials(Some("old"), 0).is_err());
    }
    #[test]
    fn project_and_onboarding_fixtures() {
        assert_eq!(project_id(&json!(" managed ")).as_deref(), Some("managed"));
        assert_eq!(
            project_id(&json!({"id":"managed"})).as_deref(),
            Some("managed")
        );
        for value in [
            Value::Null,
            json!(" "),
            json!({"id": 1}),
            json!({"name":"fake"}),
        ] {
            assert!(project_id(&value).is_none());
        }
        assert_eq!(tier_id(&json!({})), "free-tier");
        assert_eq!(tier_id(&json!({"currentTier":{"id":"current"}})), "current");
        assert_eq!(
            tier_id(&json!({"allowedTiers":[{"id":"first"},{"id":"default","isDefault":true}]})),
            "default"
        );
        assert_eq!(
            onboard_body("free-tier"),
            json!({"tier_id":"free-tier","metadata":{"ide_type":"ANTIGRAVITY","ide_version":"2.24.0","ide_name":"antigravity"}})
        );
    }
}
