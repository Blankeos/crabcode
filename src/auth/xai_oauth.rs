use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::auth::OAuthCredentials;

// Public Grok-CLI OAuth client. xAI's auth server rejects loopback OAuth from
// non-allowlisted clients, so we reuse the Grok-CLI client_id xAI ships for
// desktop OAuth flows. This mirrors OpenCode's xAI integration.
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const AUTHORIZE_URL: &str = "https://auth.x.ai/oauth2/authorize";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.x.ai/oauth2/device/code";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const OAUTH_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";

const OAUTH_HOST: &str = "127.0.0.1";
const OAUTH_PORT: u16 = 56_121;
const OAUTH_REDIRECT_PATH: &str = "/callback";
const DEVICE_CODE_DEFAULT_INTERVAL_MS: u64 = 5_000;
const DEVICE_CODE_MIN_INTERVAL_MS: u64 = 1_000;
const DEVICE_CODE_SLOW_DOWN_INCREMENT_MS: u64 = 5_000;
const DEVICE_CODE_DEFAULT_EXPIRES_MS: u64 = 5 * 60 * 1_000;
const OAUTH_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;
const ACCESS_TOKEN_REFRESH_SKEW_MS: i64 = 120_000;

#[derive(Debug, Clone)]
struct PkceCodes {
    verifier: String,
    challenge: String,
}

fn cors_headers(origin: Option<&str>) -> String {
    let Some(origin @ ("https://accounts.x.ai" | "https://auth.x.ai")) = origin else {
        return String::new();
    };

    format!(
        "Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Private-Network: true\r\nVary: Origin\r\n"
    )
}

#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    interval: Option<i64>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct DeviceTokenErrorBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn build_user_agent() -> String {
    format!(
        "crabcode/{} ({} {}; {})",
        crate::version::CURRENT,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY
    )
}

pub fn access_token_is_expiring(access_token: &str) -> bool {
    access_token_expiry_ms(access_token)
        .map(|expires| expires <= now_unix_ms() + ACCESS_TOKEN_REFRESH_SKEW_MS)
        .unwrap_or(false)
}

pub async fn authorize_browser() -> Result<OAuthCredentials> {
    let listener = TcpListener::bind((OAUTH_HOST, OAUTH_PORT))
        .await
        .with_context(|| {
            format!(
                "failed to bind xAI oauth callback listener on {}:{}",
                OAUTH_HOST, OAUTH_PORT
            )
        })?;

    let redirect_uri = redirect_uri();
    let pkce = generate_pkce();
    let state = generate_state();
    let nonce = generate_state();
    let authorize_url = build_authorize_url(&pkce, &state, &nonce)?;

    open_browser(&authorize_url).with_context(|| {
        format!(
            "failed to open browser. open this url manually: {}",
            authorize_url
        )
    })?;

    let code = wait_for_oauth_callback(listener, &state)
        .await
        .context("did not receive xAI oauth callback")?;

    let client = reqwest::Client::new();
    let token_response =
        exchange_authorization_code(&client, &code, &redirect_uri, &pkce.verifier).await?;

    credentials_from_token_response(token_response, None)
}

pub async fn authorize_headless<F>(mut on_code: F) -> Result<OAuthCredentials>
where
    F: FnMut(String, String) + Send,
{
    let client = reqwest::Client::new();
    let device = request_device_code(&client).await?;
    let browser_url = device
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| device.verification_uri.clone());

    on_code(device.user_code.clone(), browser_url);

    let token_response = poll_device_code_token(&client, &device).await?;
    credentials_from_token_response(token_response, None)
}

pub async fn refresh_access_token(refresh_token: &str) -> Result<OAuthCredentials> {
    let client = reqwest::Client::new();
    let response = form_headers(client.post(TOKEN_URL))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .context("failed to refresh xAI access token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        bail!("xAI token refresh failed: {status} {body}");
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .context("failed to parse xAI refresh token response")?;

    credentials_from_token_response(token_response, Some(refresh_token.to_string()))
}

fn redirect_uri() -> String {
    format!("http://{OAUTH_HOST}:{OAUTH_PORT}{OAUTH_REDIRECT_PATH}")
}

fn credentials_from_token_response(
    token_response: TokenResponse,
    fallback_refresh: Option<String>,
) -> Result<OAuthCredentials> {
    let refresh = token_response
        .refresh_token
        .clone()
        .or(fallback_refresh)
        .ok_or_else(|| anyhow!("missing refresh token in xAI oauth response"))?;

    let expires = now_unix_ms() + token_response.expires_in.unwrap_or(3600) * 1_000;

    Ok(OAuthCredentials {
        refresh,
        access: token_response.access_token,
        expires,
        account_id: None,
        enterprise_url: None,
    })
}

async fn request_device_code(client: &reqwest::Client) -> Result<DeviceCodeResponse> {
    let response = form_headers(client.post(DEVICE_AUTHORIZATION_URL))
        .form(&[("client_id", CLIENT_ID), ("scope", OAUTH_SCOPE)])
        .send()
        .await
        .context("failed to request xAI device code")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        bail!("xAI device code request failed: {status} {body}");
    }

    let device: DeviceCodeResponse = response
        .json()
        .await
        .context("failed to parse xAI device code response")?;

    if device.device_code.is_empty()
        || device.user_code.is_empty()
        || device.verification_uri.is_empty()
    {
        bail!("xAI device code response is missing required fields");
    }

    Ok(device)
}

async fn poll_device_code_token(
    client: &reqwest::Client,
    device: &DeviceCodeResponse,
) -> Result<TokenResponse> {
    let expires_ms = positive_seconds_to_ms(device.expires_in, DEVICE_CODE_DEFAULT_EXPIRES_MS);
    let deadline = Instant::now() + Duration::from_millis(expires_ms);
    let mut interval_ms = std::cmp::max(
        positive_seconds_to_ms(device.interval, DEVICE_CODE_DEFAULT_INTERVAL_MS),
        DEVICE_CODE_MIN_INTERVAL_MS,
    );

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("xAI device authorization timed out");
        }

        let response = form_headers(client.post(TOKEN_URL))
            .form(&[
                ("grant_type", DEVICE_CODE_GRANT_TYPE),
                ("client_id", CLIENT_ID),
                ("device_code", device.device_code.as_str()),
            ])
            .send()
            .await
            .context("failed to poll xAI device authorization")?;

        if response.status().is_success() {
            return response
                .json::<TokenResponse>()
                .await
                .context("failed to parse xAI device token response");
        }

        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        let body: DeviceTokenErrorBody = serde_json::from_str(&body_text).unwrap_or_default();
        let error = body.error.as_deref().unwrap_or_default();

        match error {
            "authorization_pending" => {}
            "slow_down" => {
                interval_ms = interval_ms.saturating_add(DEVICE_CODE_SLOW_DOWN_INCREMENT_MS);
            }
            "access_denied" | "authorization_denied" => {
                bail!("xAI device authorization was denied");
            }
            "expired_token" => {
                bail!("xAI device code expired - please re-run login");
            }
            _ => {
                let detail = body
                    .error_description
                    .or(body.error)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(body_text);
                bail!("xAI device token exchange failed: {status} {detail}");
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        let sleep_ms = std::cmp::min(
            interval_ms.saturating_add(OAUTH_POLLING_SAFETY_MARGIN_MS),
            remaining.as_millis() as u64,
        );
        if sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }
    }
}

async fn wait_for_oauth_callback(listener: TcpListener, expected_state: &str) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(5 * 60);

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("xAI oauth callback timeout");
        }

        let remaining = deadline.saturating_duration_since(now);
        let (mut socket, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .context("timed out waiting for xAI oauth callback connection")?
            .context("failed to accept xAI oauth callback connection")?;

        let mut buffer = vec![0_u8; 8 * 1024];
        let read_count = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
            .await
            .context("timed out reading xAI oauth callback request")?
            .context("failed to read xAI oauth callback request")?;

        if read_count == 0 {
            continue;
        }

        let request = String::from_utf8_lossy(&buffer[..read_count]);
        let Some(first_line) = request.lines().next() else {
            continue;
        };

        let mut request_parts = first_line.split_whitespace();
        let method = request_parts.next().unwrap_or_default();
        let raw_target = request_parts.next().unwrap_or("/");
        let origin = request_header(&request, "origin");
        let parsed_url =
            match reqwest::Url::parse(&format!("http://{OAUTH_HOST}:{OAUTH_PORT}{raw_target}")) {
                Ok(url) => url,
                Err(_) => {
                    write_html_response(
                        &mut socket,
                        400,
                        "Authorization Failed",
                        "Invalid callback request.",
                        origin.as_deref(),
                    )
                    .await;
                    continue;
                }
            };

        if method == "OPTIONS" {
            write_options_response(&mut socket, origin.as_deref()).await;
            continue;
        }

        if parsed_url.path() != OAUTH_REDIRECT_PATH {
            write_html_response(
                &mut socket,
                404,
                "Not Found",
                "Not found.",
                origin.as_deref(),
            )
            .await;
            continue;
        }

        if method != "GET" {
            write_html_response(
                &mut socket,
                405,
                "Authorization Failed",
                "Unsupported callback method.",
                origin.as_deref(),
            )
            .await;
            continue;
        }

        if let Some(error) = parsed_url
            .query_pairs()
            .find_map(|(k, v)| (k == "error").then_some(v.into_owned()))
        {
            let error_description = parsed_url
                .query_pairs()
                .find_map(|(k, v)| (k == "error_description").then_some(v.into_owned()))
                .unwrap_or(error);
            write_html_response(
                &mut socket,
                400,
                "Authorization Failed",
                &error_description,
                origin.as_deref(),
            )
            .await;
            bail!("xAI oauth authorization failed: {error_description}");
        }

        let code = parsed_url
            .query_pairs()
            .find_map(|(k, v)| (k == "code").then_some(v.into_owned()))
            .ok_or_else(|| anyhow!("missing authorization code in xAI callback"))?;

        let state = parsed_url
            .query_pairs()
            .find_map(|(k, v)| (k == "state").then_some(v.into_owned()))
            .ok_or_else(|| anyhow!("missing oauth state in xAI callback"))?;

        if state != expected_state {
            write_html_response(
                &mut socket,
                400,
                "Authorization Failed",
                "Invalid oauth state.",
                origin.as_deref(),
            )
            .await;
            bail!("invalid xAI oauth state received");
        }

        write_html_response(
            &mut socket,
            200,
            "Authorization Successful",
            "You can close this window and return to crabcode.",
            origin.as_deref(),
        )
        .await;

        return Ok(code);
    }
}

async fn write_options_response(socket: &mut TcpStream, origin: Option<&str>) {
    let cors_headers = cors_headers(origin);

    let response = format!(
        "HTTP/1.1 204 No Content\r\n{cors_headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}

async fn write_html_response(
    socket: &mut TcpStream,
    status: u16,
    title: &str,
    body: &str,
    origin: Option<&str>,
) {
    let page = format!(
        "<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1><p>{body}</p></body></html>"
    );
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\n{}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        cors_headers(origin),
        page.len(),
        page
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}

async fn exchange_authorization_code(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<TokenResponse> {
    let response = form_headers(client.post(TOKEN_URL))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .context("failed to exchange xAI authorization code")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        bail!("xAI token exchange failed: {status} {body}");
    }

    response
        .json::<TokenResponse>()
        .await
        .context("failed to parse xAI token exchange response")
}

fn build_authorize_url(pkce: &PkceCodes, state: &str, nonce: &str) -> Result<String> {
    let mut url =
        reqwest::Url::parse(AUTHORIZE_URL).context("failed to build xAI authorize url")?;

    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", &redirect_uri())
        .append_pair("scope", OAUTH_SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("plan", "generic")
        .append_pair("referrer", "crabcode");

    Ok(url.to_string())
}

fn form_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .header("User-Agent", build_user_agent())
}

fn request_header(request: &str, name: &str) -> Option<String> {
    request.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = Command::new("open");
        cmd.arg(url);
        cmd
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url);
        cmd
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    };

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("unsupported platform for automatic browser launch")
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let status = command
            .status()
            .context("failed to launch browser command")?;
        if status.success() {
            return Ok(());
        }
        bail!("browser command returned non-zero exit status")
    }
}

fn generate_pkce() -> PkceCodes {
    let verifier = generate_random_string(64);
    let challenge = {
        let digest = Sha256::digest(verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    };

    PkceCodes {
        verifier,
        challenge,
    }
}

fn generate_state() -> String {
    let bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_random_string(length: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn positive_seconds_to_ms(value: Option<i64>, default_ms: u64) -> u64 {
    value
        .and_then(|seconds| (seconds > 0).then_some(seconds as u64 * 1_000))
        .unwrap_or(default_ms)
}

fn access_token_expiry_ms(token: &str) -> Option<i64> {
    let claims = parse_jwt_claims(token)?;
    claims
        .get("exp")
        .and_then(|value| value.as_i64())
        .map(|seconds| seconds * 1_000)
}

fn parse_jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;

    serde_json::from_slice::<serde_json::Value>(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_authorize_url_with_xai_required_params() {
        let pkce = PkceCodes {
            verifier: "verifier".to_string(),
            challenge: "challenge".to_string(),
        };

        let url = build_authorize_url(&pkce, "state", "nonce").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();

        assert_eq!(parsed.as_str().split('?').next().unwrap(), AUTHORIZE_URL);
        assert_eq!(params.get("client_id").map(String::as_str), Some(CLIENT_ID));
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:56121/callback")
        );
        assert_eq!(params.get("scope").map(String::as_str), Some(OAUTH_SCOPE));
        assert_eq!(params.get("plan").map(String::as_str), Some("generic"));
        assert_eq!(params.get("referrer").map(String::as_str), Some("crabcode"));
    }

    #[test]
    fn detects_expiring_jwt_access_token() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!("{{\"exp\":{}}}", (now_unix_ms() / 1_000) + 30));
        let token = format!("{header}.{payload}.sig");

        assert!(access_token_is_expiring(&token));
    }

    #[test]
    fn cors_headers_allow_xai_auth_origins_for_loopback_callback() {
        let headers = cors_headers(Some("https://accounts.x.ai"));
        assert!(headers.contains("Access-Control-Allow-Origin: https://accounts.x.ai"));
        assert!(headers.contains("Access-Control-Allow-Private-Network: true"));
        assert!(cors_headers(Some("https://example.com")).is_empty());
        assert!(cors_headers(None).is_empty());
    }
}
