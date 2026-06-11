use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::auth::OAuthCredentials;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
const OAUTH_SCOPE: &str = "openid profile email offline_access";
const OAUTH_PORT: u16 = 1455;
const OAUTH_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;

#[derive(Debug, Clone)]
struct PkceCodes {
    verifier: String,
    challenge: String,
}

#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct DeviceAuthStartResponse {
    device_auth_id: String,
    user_code: String,
    interval: String,
}

#[derive(Debug, serde::Deserialize)]
struct DeviceAuthTokenResponse {
    authorization_code: String,
    code_verifier: String,
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
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY
    )
}

pub async fn authorize_browser() -> Result<OAuthCredentials> {
    let listener = TcpListener::bind(("127.0.0.1", OAUTH_PORT))
        .await
        .with_context(|| {
            format!(
                "failed to bind oauth callback listener on port {}",
                OAUTH_PORT
            )
        })?;

    let redirect_uri = format!("http://localhost:{}/auth/callback", OAUTH_PORT);
    let pkce = generate_pkce();
    let state = generate_state();
    let authorize_url = build_authorize_url(&redirect_uri, &pkce, &state)?;

    open_browser(&authorize_url).with_context(|| {
        format!(
            "failed to open browser. open this url manually: {}",
            authorize_url
        )
    })?;

    let code = wait_for_oauth_callback(listener, &state)
        .await
        .context("did not receive oauth callback")?;

    let client = reqwest::Client::new();
    let token_response = exchange_authorization_code(
        &client,
        &code,
        &redirect_uri,
        &pkce.verifier,
        Some(build_user_agent()),
    )
    .await?;

    credentials_from_token_response(token_response, None)
}

pub async fn authorize_headless<F>(mut on_code: F) -> Result<OAuthCredentials>
where
    F: FnMut(String, String) + Send,
{
    let client = reqwest::Client::new();
    let user_agent = build_user_agent();

    let start_response = client
        .post(format!("{ISSUER}/api/accounts/deviceauth/usercode"))
        .header("Content-Type", "application/json")
        .header("User-Agent", user_agent.clone())
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await
        .context("failed to initiate device authorization")?;

    if !start_response.status().is_success() {
        let status = start_response.status();
        let body = start_response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        bail!("device authorization start failed: {status} {body}");
    }

    let device_data: DeviceAuthStartResponse = start_response
        .json()
        .await
        .context("failed to parse device authorization response")?;

    let interval_ms = std::cmp::max(device_data.interval.parse::<u64>().unwrap_or(5), 1) * 1_000;
    let code_url = format!("{ISSUER}/codex/device");
    on_code(device_data.user_code.clone(), code_url);

    let deadline = Instant::now() + Duration::from_secs(10 * 60);

    loop {
        if Instant::now() >= deadline {
            bail!("timed out while waiting for device authorization");
        }

        let token_response = client
            .post(format!("{ISSUER}/api/accounts/deviceauth/token"))
            .header("Content-Type", "application/json")
            .header("User-Agent", user_agent.clone())
            .json(&serde_json::json!({
                "device_auth_id": device_data.device_auth_id,
                "user_code": device_data.user_code,
            }))
            .send()
            .await
            .context("failed to poll device authorization")?;

        if token_response.status().is_success() {
            let device_token: DeviceAuthTokenResponse = token_response
                .json()
                .await
                .context("failed to parse device authorization token response")?;

            let token_response = exchange_authorization_code(
                &client,
                &device_token.authorization_code,
                &format!("{ISSUER}/deviceauth/callback"),
                &device_token.code_verifier,
                Some(user_agent.clone()),
            )
            .await?;

            return credentials_from_token_response(token_response, None);
        }

        if token_response.status() != reqwest::StatusCode::FORBIDDEN
            && token_response.status() != reqwest::StatusCode::NOT_FOUND
        {
            let status = token_response.status();
            let body = token_response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read body>".to_string());
            bail!("device authorization polling failed: {status} {body}");
        }

        tokio::time::sleep(Duration::from_millis(
            interval_ms + OAUTH_POLLING_SAFETY_MARGIN_MS,
        ))
        .await;
    }
}

pub async fn refresh_access_token(refresh_token: &str) -> Result<OAuthCredentials> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .context("failed to refresh access token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        bail!("token refresh failed: {status} {body}");
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .context("failed to parse refresh token response")?;

    credentials_from_token_response(token_response, Some(refresh_token.to_string()))
}

fn credentials_from_token_response(
    token_response: TokenResponse,
    fallback_refresh: Option<String>,
) -> Result<OAuthCredentials> {
    let refresh = token_response
        .refresh_token
        .clone()
        .or(fallback_refresh)
        .ok_or_else(|| anyhow!("missing refresh token in oauth response"))?;

    let account_id = extract_account_id(&token_response);
    let expires = now_unix_ms() + token_response.expires_in.unwrap_or(3600) * 1_000;

    Ok(OAuthCredentials {
        refresh,
        access: token_response.access_token,
        expires,
        account_id,
        enterprise_url: None,
    })
}

async fn wait_for_oauth_callback(listener: TcpListener, expected_state: &str) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(5 * 60);

    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("oauth callback timeout")
        }

        let remaining = deadline.saturating_duration_since(now);
        let (mut socket, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .context("timed out waiting for oauth callback connection")?
            .context("failed to accept oauth callback connection")?;

        let mut buffer = vec![0_u8; 8 * 1024];
        let read_count = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
            .await
            .context("timed out reading oauth callback request")?
            .context("failed to read oauth callback request")?;

        if read_count == 0 {
            continue;
        }

        let request = String::from_utf8_lossy(&buffer[..read_count]);
        let Some(first_line) = request.lines().next() else {
            continue;
        };

        let raw_target = first_line.split_whitespace().nth(1).unwrap_or("/");
        let parsed_url = match reqwest::Url::parse(&format!("http://localhost{}", raw_target)) {
            Ok(url) => url,
            Err(_) => {
                write_html_response(
                    &mut socket,
                    400,
                    "Authorization Failed",
                    "Invalid callback request.",
                )
                .await;
                continue;
            }
        };

        if parsed_url.path() != "/auth/callback" {
            write_html_response(&mut socket, 404, "Not Found", "Not found.").await;
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
            write_html_response(&mut socket, 400, "Authorization Failed", &error_description).await;
            bail!("oauth authorization failed: {error_description}");
        }

        let code = parsed_url
            .query_pairs()
            .find_map(|(k, v)| (k == "code").then_some(v.into_owned()))
            .ok_or_else(|| anyhow!("missing authorization code in callback"))?;

        let state = parsed_url
            .query_pairs()
            .find_map(|(k, v)| (k == "state").then_some(v.into_owned()))
            .ok_or_else(|| anyhow!("missing oauth state in callback"))?;

        if state != expected_state {
            write_html_response(
                &mut socket,
                400,
                "Authorization Failed",
                "Invalid oauth state.",
            )
            .await;
            bail!("invalid oauth state received")
        }

        write_html_response(
            &mut socket,
            200,
            "Authorization Successful",
            "You can close this window and return to crabcode.",
        )
        .await;

        return Ok(code);
    }
}

async fn write_html_response(socket: &mut TcpStream, status: u16, title: &str, body: &str) {
    let page = format!(
        "<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1><p>{body}</p></body></html>"
    );
    let status_text = if status == 200 { "OK" } else { "Bad Request" };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
    user_agent: Option<String>,
) -> Result<TokenResponse> {
    let mut request = client
        .post(format!("{ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded");

    if let Some(agent) = user_agent {
        request = request.header("User-Agent", agent);
    }

    let response = request
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .context("failed to exchange authorization code")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        bail!("token exchange failed: {status} {body}");
    }

    response
        .json::<TokenResponse>()
        .await
        .context("failed to parse token exchange response")
}

fn build_authorize_url(redirect_uri: &str, pkce: &PkceCodes, state: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("{ISSUER}/oauth/authorize"))
        .context("failed to build authorize url")?;

    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", OAUTH_SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "opencode")
        .append_pair("state", state);

    Ok(url.to_string())
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
    let verifier = generate_random_string(43);
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

fn extract_account_id(tokens: &TokenResponse) -> Option<String> {
    if let Some(ref id_token) = tokens.id_token {
        if let Some(claims) = parse_jwt_claims(id_token) {
            if let Some(account_id) = extract_account_id_from_claims(&claims) {
                return Some(account_id);
            }
        }
    }

    if let Some(claims) = parse_jwt_claims(&tokens.access_token) {
        return extract_account_id_from_claims(&claims);
    }

    None
}

fn parse_jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;

    serde_json::from_slice::<serde_json::Value>(&decoded).ok()
}

fn extract_account_id_from_claims(claims: &serde_json::Value) -> Option<String> {
    claims
        .get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|v| v.get("chatgpt_account_id"))
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|org| org.get("id"))
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
        })
}
