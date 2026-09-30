use crate::config::configuration::McpRemoteConfig;
use anyhow::{anyhow, Context, Result};
use rmcp::transport::auth::{AuthorizationManager, OAuthClientConfig};
use std::collections::HashMap;
use std::future::Future;
use std::io::{self, IsTerminal};
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

/// CLI PKCE login: print the URL and open the default browser only on Enter.
/// Tokens are persisted to mcp-auth.json.
pub async fn authenticate(name: &str, remote: &McpRemoteConfig) -> Result<()> {
    authenticate_with_url_action(name, remote, |url| {
        eprintln!("Authenticate MCP server \"{name}\" by opening this URL in your preferred browser:\n");
        eprintln!("{url}\n");
        let interactive = io::stdin().is_terminal();
        if interactive {
            eprintln!("Press Enter to open your default browser, or copy the URL above and open it manually.");
        }
        eprintln!("Waiting for authorization in the browser...");
        let url = url.to_owned();
        async move {
            if interactive {
                open_browser_on_enter(
                    &url,
                    wait_for_enter(),
                    crate::utils::image_attachment::open_url,
                )
                .await;
            }
        }
    })
    .await
}

async fn open_browser_on_enter(
    url: &str,
    enter: impl Future<Output = io::Result<()>>,
    open_url: impl FnOnce(&str) -> Result<()>,
) {
    match enter.await {
        Ok(()) => match open_url(url) {
            Ok(()) => eprintln!("Opening default browser..."),
            Err(err) => eprintln!("Failed to open browser ({err}). Open the URL above manually."),
        },
        Err(err) => {
            eprintln!("Unable to read terminal input ({err}). Open the URL above manually.")
        }
    }
}

async fn wait_for_enter() -> io::Result<()> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // A bounded poll avoids Tokio's uncancellable stdin read, which would keep
    // the command alive after a manually opened browser completes OAuth.
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            while !sender.is_closed() {
                if poll_enter()? {
                    return Ok(());
                }
            }
            Ok(())
        })();
        let _ = sender.send(result);
    });
    receiver.await.map_err(io::Error::other)?
}

#[cfg(unix)]
fn poll_enter() -> io::Result<bool> {
    poll_enter_fd(libc::STDIN_FILENO)
}

#[cfg(unix)]
fn poll_enter_fd(fd: std::os::fd::RawFd) -> io::Result<bool> {
    // Keep canonical mode and mouse selection intact. Read just one byte after
    // readiness, including EOF (which Crossterm's event reader loops on).
    let mut input = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: input is a valid pollfd and remains alive for the call.
    let ready = unsafe { libc::poll(&mut input, 1, 100) };
    if ready == 0 {
        return Ok(false);
    }
    if ready < 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(err)
        };
    }
    let mut byte = 0u8;
    // SAFETY: byte is a writable one-byte buffer; fd was reported ready above.
    match unsafe { libc::read(fd, (&mut byte as *mut u8).cast(), 1) } {
        0 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed")),
        1 => Ok(byte == b'\n' || byte == b'\r'),
        _ => {
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => Ok(false),
                _ => Err(err),
            }
        }
    }
}

#[cfg(not(unix))]
fn poll_enter() -> io::Result<bool> {
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
    Ok(event::poll(Duration::from_millis(100))?
        && matches!(event::read()?, Event::Key(key) if key.code == KeyCode::Enter && key.kind == KeyEventKind::Press))
}

pub async fn authenticate_with_url_callback(
    name: &str,
    remote: &McpRemoteConfig,
    on_url: impl FnOnce(&str),
) -> Result<()> {
    authenticate_with_url_action(name, remote, |url| {
        on_url(url);
        std::future::ready(())
    })
    .await
}

async fn authenticate_with_url_action<F: Future<Output = ()>>(
    name: &str,
    remote: &McpRemoteConfig,
    on_url: impl FnOnce(&str) -> F,
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

    let callback = tokio::time::timeout(
        BROWSER_AUTH_TIMEOUT,
        accept_callback_with_action(listener, on_url(&auth_url)),
    )
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

async fn accept_callback_with_action(
    listener: TcpListener,
    action: impl Future<Output = ()>,
) -> Result<OAuthCallback> {
    let callback = accept_callback(listener);
    // Retain an in-progress callback read when the browser action finishes.
    tokio::pin!(callback);
    tokio::select! {
        biased;
        result = &mut callback => result,
        _ = action => callback.await,
    }
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
    use std::cell::Cell;
    use tokio::net::TcpStream;

    async fn send_callback(addr: std::net::SocketAddr, query: &str) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
    }

    #[tokio::test]
    async fn manual_callback_completes_without_enter_or_browser() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let opened = Cell::new(false);
        let (enter_sender, enter_receiver) = tokio::sync::oneshot::channel::<()>();
        let action = open_browser_on_enter(
            "https://auth.example/authorize",
            async { enter_receiver.await.map_err(io::Error::other) },
            |_| {
                opened.set(true);
                Ok(())
            },
        );
        let (callback, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                accept_callback_with_action(listener, action),
                send_callback(addr, "code=manual&state=s1")
            )
        })
        .await
        .unwrap();
        assert_eq!(callback.unwrap().code, "manual");
        assert!(!opened.get());
        assert!(enter_sender.is_closed(), "pending input must be cancelled");
    }

    #[tokio::test]
    async fn enter_opens_browser_once_and_keeps_waiting_for_callback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let opens = Cell::new(0);
        let (enter_sender, enter_receiver) = tokio::sync::oneshot::channel();
        let action = open_browser_on_enter(
            "https://auth.example/authorize",
            async { enter_receiver.await.map_err(io::Error::other) },
            |url| {
                assert_eq!(url, "https://auth.example/authorize");
                opens.set(opens.get() + 1);
                Ok(())
            },
        );
        let callback = accept_callback_with_action(listener, action);
        tokio::pin!(callback);
        assert!(futures::poll!(&mut callback).is_pending());
        assert_eq!(opens.get(), 0);
        enter_sender.send(()).unwrap();
        assert!(futures::poll!(&mut callback).is_pending());
        assert_eq!(opens.get(), 1);
        let (callback, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(callback, send_callback(addr, "code=browser&state=s1"))
        })
        .await
        .unwrap();
        assert_eq!(callback.unwrap().code, "browser");
        assert_eq!(opens.get(), 1);
    }

    #[tokio::test]
    async fn browser_open_failure_still_allows_manual_auth() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let action = open_browser_on_enter(
            "https://auth.example/authorize",
            std::future::ready(Ok(())),
            |_| anyhow::bail!("browser unavailable"),
        );
        let callback = accept_callback_with_action(listener, action);
        tokio::pin!(callback);
        assert!(futures::poll!(&mut callback).is_pending());
        let (callback, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(callback, send_callback(addr, "code=manual&state=s1"))
        })
        .await
        .unwrap();
        assert_eq!(callback.unwrap().code, "manual");
    }

    #[tokio::test]
    async fn closed_input_does_not_open_browser() {
        let opened = Cell::new(false);
        open_browser_on_enter(
            "https://auth.example/authorize",
            std::future::ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stdin closed",
            ))),
            |_| {
                opened.set(true);
                Ok(())
            },
        )
        .await;
        assert!(!opened.get());
    }

    #[tokio::test]
    async fn callback_errors_do_not_wait_for_enter() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let action = async {
            let _ = receiver.await;
        };
        let (callback, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                accept_callback_with_action(listener, action),
                send_callback(addr, "error=access_denied")
            )
        })
        .await
        .unwrap();
        assert!(callback.unwrap_err().to_string().contains("access_denied"));
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn timeout_cancels_pending_browser_input() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let action = async {
            let _ = receiver.await;
        };
        assert!(tokio::time::timeout(
            Duration::from_millis(10),
            accept_callback_with_action(listener, action),
        )
        .await
        .is_err());
        assert!(sender.is_closed());
    }

    #[cfg(unix)]
    #[test]
    fn terminal_input_only_opens_on_enter() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        let (input, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"x\n").unwrap();
        assert!(!poll_enter_fd(input.as_raw_fd()).unwrap());
        assert!(poll_enter_fd(input.as_raw_fd()).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn terminal_eof_does_not_loop_or_open_browser() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        let (input, writer) = UnixStream::pair().unwrap();
        drop(writer);
        assert_eq!(
            poll_enter_fd(input.as_raw_fd()).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

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
