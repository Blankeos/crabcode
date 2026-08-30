use crate::config::configuration::McpRemoteConfig;
use anyhow::{anyhow, Context, Result};
use rmcp::transport::auth::{AuthorizationManager, OAuthClientConfig};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CLIENT_NAME: &str = "crabcode";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const BROWSER_AUTH_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
    pub issuer: Option<String>,
}

pub fn is_auth_error_message(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("auth")
        || lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("authorization required")
}

pub fn has_static_authorization(remote: &McpRemoteConfig) -> bool {
    remote
        .headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"))
}

pub fn should_use_oauth(remote: &McpRemoteConfig) -> bool {
    remote.oauth_enabled && !has_static_authorization(remote)
}

pub fn logout(name: &str, url: &str) -> Result<bool> {
    super::credentials::delete(name, url)
}

/// Browser PKCE login for a remote MCP server. Tokens are persisted to mcp-auth.json.
pub async fn authenticate(name: &str, remote: &McpRemoteConfig) -> Result<()> {
    authenticate_with_url_callback(name, remote, |url| {
        match crate::utils::image_attachment::open_url(url) {
            Ok(()) => eprintln!("Opening browser to authenticate MCP server \"{name}\"..."),
            Err(err) => eprintln!("Failed to open browser ({err}). Open this URL manually:"),
        }
        eprintln!("{url}");
        eprintln!("Waiting for authorization in the browser...");
    })
    .await
}

pub async fn authenticate_with_url_callback(
    name: &str,
    remote: &McpRemoteConfig,
    on_url: impl FnOnce(&str),
) -> Result<()> {
    if !remote.oauth_enabled {
        anyhow::bail!("OAuth is disabled for MCP server '{name}' (set mcp.{name}.oauth)");
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind OAuth loopback port")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let mut manager = AuthorizationManager::new(remote.url.as_str())
        .await
        .map_err(|e| anyhow!("failed to start OAuth client: {e}"))?;
    manager.set_credential_store(super::credentials::FileCredentialStore::new(
        name.to_string(),
        remote.url.clone(),
    ));

    let metadata = tokio::time::timeout(DISCOVERY_TIMEOUT, manager.discover_metadata())
        .await
        .map_err(|_| anyhow!("OAuth metadata discovery timed out"))?
        .map_err(|e| anyhow!("OAuth metadata discovery failed: {e}"))?;
    manager.set_metadata(metadata);

    let scopes: Vec<String> = remote
        .oauth_scope
        .as_deref()
        .map(|scope| {
            scope
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    if let Some(client_id) = remote.oauth_client_id.as_deref() {
        let mut config =
            OAuthClientConfig::new(client_id, redirect_uri.clone()).with_scopes(scopes.clone());
        config.client_secret = remote.oauth_client_secret.clone();
        manager
            .configure_client(config)
            .map_err(|e| anyhow!("failed to configure OAuth client: {e}"))?;
    } else {
        let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        manager
            .register_client(CLIENT_NAME, &redirect_uri, &scope_refs)
            .await
            .map_err(|e| anyhow!("dynamic client registration failed: {e}"))?;
    }

    let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
    let auth_url = manager
        .get_authorization_url(&scope_refs)
        .await
        .map_err(|e| anyhow!("failed to build authorization URL: {e}"))?;

    on_url(&auth_url);

    let callback = tokio::time::timeout(BROWSER_AUTH_TIMEOUT, accept_callback(listener))
        .await
        .map_err(|_| anyhow!("OAuth timed out after {}s", BROWSER_AUTH_TIMEOUT.as_secs()))?
        .context("OAuth callback failed")?;

    manager
        .exchange_code_for_token_with_issuer(
            &callback.code,
            &callback.state,
            callback.issuer.as_deref(),
        )
        .await
        .map_err(|e| anyhow!("token exchange failed: {e}"))?;

    Ok(())
}

pub fn parse_callback_query(query: &str) -> Result<OAuthCallback> {
    let mut params = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        params.insert(key.into_owned(), value.into_owned());
    }
    if let Some(error) = params.get("error") {
        let desc = params
            .get("error_description")
            .cloned()
            .unwrap_or_else(|| "unknown error".to_string());
        anyhow::bail!("OAuth error: {error} — {desc}");
    }
    let code = params
        .get("code")
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| anyhow!("OAuth callback missing code"))?;
    let state = params
        .get("state")
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| anyhow!("OAuth callback missing state"))?;
    Ok(OAuthCallback {
        code,
        state,
        issuer: params.get("iss").cloned(),
    })
}

pub fn parse_callback_request(request: &str) -> Result<OAuthCallback> {
    let first_line = request.lines().next().unwrap_or_default();
    let path = first_line.split_whitespace().nth(1).unwrap_or_default();
    let query = path
        .split_once('?')
        .map(|(_, query)| query.trim_end_matches(|c| c == ' ' || c == '\r'))
        .unwrap_or("");
    parse_callback_query(query)
}

async fn accept_callback(listener: TcpListener) -> Result<OAuthCallback> {
    let (mut stream, _) = listener.accept().await.context("callback accept failed")?;
    let mut buf = vec![0u8; 8192];
    let n = stream
        .read(&mut buf)
        .await
        .context("callback read failed")?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let result = parse_callback_request(&request);

    let body = match &result {
        Ok(_) => CALLBACK_SUCCESS_HTML,
        Err(_) => CALLBACK_FAILURE_HTML,
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    result
}

const CALLBACK_SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html><head><title>crabcode</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;padding:48px;">
<h1>Authenticated</h1>
<p>You can close this window and return to crabcode.</p>
<script>window.close();</script>
</body></html>"#;

const CALLBACK_FAILURE_HTML: &str = r#"<!DOCTYPE html>
<html><head><title>crabcode</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;padding:48px;">
<h1>Authorization failed</h1>
<p>You can close this window and try again from crabcode.</p>
</body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_callback_query() {
        let cb = parse_callback_query("code=abc&state=xyz&iss=https://auth.example").unwrap();
        assert_eq!(cb.code, "abc");
        assert_eq!(cb.state, "xyz");
        assert_eq!(cb.issuer.as_deref(), Some("https://auth.example"));
    }

    #[test]
    fn parses_http_request_line() {
        let req = "GET /callback?code=tok&state=s1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let cb = parse_callback_request(req).unwrap();
        assert_eq!(cb.code, "tok");
        assert_eq!(cb.state, "s1");
    }

    #[test]
    fn rejects_oauth_error() {
        let err = parse_callback_query("error=access_denied&error_description=nope").unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[test]
    fn auth_error_heuristic() {
        assert!(is_auth_error_message("Auth required"));
        assert!(is_auth_error_message("HTTP 401 Unauthorized"));
        assert!(is_auth_error_message("OAuth authorization required"));
        assert!(!is_auth_error_message("connection refused"));
    }
}
