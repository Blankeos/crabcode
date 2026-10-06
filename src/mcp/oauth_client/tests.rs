use super::*;
use oauth2::TokenResponse;
use rmcp::transport::auth::{CredentialStore, InMemoryCredentialStore, StoredCredentials};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{transport::StreamableHttpClientTransport, ServiceExt};
use serde_json::{json, Value};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Default)]
struct State {
    refreshes: usize,
    requests: Vec<(String, String)>,
    reject_all: bool,
    reject_refresh: bool,
    required_token: Option<String>,
    rotate: bool,
    reject_status: u16,
    secret_seen: bool,
    refresh_status: u16,
    delay_refresh: bool,
}

#[tokio::test]
async fn startup_distinguishes_revoked_credentials_from_refresh_outages() {
    for transient in [false, true] {
        let server = Server::new().await;
        {
            let mut state = server.state.lock().unwrap();
            state.reject_refresh = !transient;
            state.refresh_status = if transient { 503 } else { 0 };
        }
        let client = server.client(store(true, true).await).await;
        let result = ()
            .serve(StreamableHttpClientTransport::with_client(
                client,
                StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", server.url)),
            ))
            .await;
        let error: anyhow::Error = result.err().unwrap().into();
        assert_eq!(is_auth_connect_error(&error), !transient);
    }
}

#[tokio::test]
async fn transient_refresh_failures_do_not_require_login_and_can_recover() {
    for expired in [false, true] {
        let server = Server::new().await;
        {
            let mut state = server.state.lock().unwrap();
            state.refresh_status = 503;
            state.required_token = Some("Bearer fresh-token".into());
        }
        let client = server.client(store(expired, true).await).await;
        assert!(matches!(
            post(&client, &server).await,
            Err(StreamableHttpError::Auth(AuthError::InternalError(_)))
        ));
        server.state.lock().unwrap().refresh_status = 0;
        assert!(post(&client, &server).await.is_ok());
        assert_eq!(server.state.lock().unwrap().refreshes, 2);
    }
}

#[tokio::test]
async fn cancelled_refresh_finishes_persistence_before_another_manager_refreshes() {
    let server = Server::new().await;
    {
        let mut state = server.state.lock().unwrap();
        state.rotate = true;
        state.delay_refresh = true;
    }
    let temp = tempfile::tempdir().unwrap();
    let remote = server.remote();
    let new_store = || {
        crate::mcp::credentials::FileCredentialStore::for_test(
            "test",
            &remote.url,
            temp.path().join("auth.json"),
        )
    };
    new_store().save(credentials(true, true)).await.unwrap();
    let first = RefreshingHttpClient::new(
        crate::mcp::oauth::stored_authorization_manager_with_store(&remote, new_store())
            .await
            .unwrap(),
    )
    .unwrap()
    .with_refresh_lock(temp.path().join("refresh.lock"));
    let second = RefreshingHttpClient::new(
        crate::mcp::oauth::stored_authorization_manager_with_store(&remote, new_store())
            .await
            .unwrap(),
    )
    .unwrap()
    .with_refresh_lock(temp.path().join("refresh.lock"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), post(&first, &server))
            .await
            .is_err()
    );
    assert!(post(&second, &server).await.is_ok());
    assert_eq!(server.state.lock().unwrap().refreshes, 1);
    assert_eq!(
        new_store()
            .load()
            .await
            .unwrap()
            .unwrap()
            .token_response
            .unwrap()
            .refresh_token()
            .unwrap()
            .secret(),
        "rotated-refresh"
    );
}

#[tokio::test]
async fn auth_error_type_survives_running_service_transport() {
    let server = Server::new().await;
    let client = server.client(store(false, true).await).await;
    let service = ()
        .serve(StreamableHttpClientTransport::with_client(
            client,
            StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", server.url)),
        ))
        .await
        .unwrap();
    server.state.lock().unwrap().reject_all = true;
    let error = service.list_tools(None).await.err().unwrap();
    assert!(is_auth_service_error(&error));
}

#[tokio::test]
async fn refreshed_tokens_persist_to_disk_and_survive_reconnect() {
    let server = Server::new().await;
    server.state.lock().unwrap().rotate = true;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mcp-auth.json");
    let remote = server.remote();
    let new_store = || {
        crate::mcp::credentials::FileCredentialStore::for_test("test", &remote.url, path.clone())
    };
    let unrelated = crate::mcp::credentials::FileCredentialStore::for_test(
        "other",
        "http://other.test/mcp",
        path.clone(),
    );
    unrelated.save(credentials(false, true)).await.unwrap();
    new_store().save(credentials(true, true)).await.unwrap();
    let manager = crate::mcp::oauth::stored_authorization_manager_with_store(&remote, new_store())
        .await
        .unwrap();
    let client = RefreshingHttpClient::new(manager)
        .unwrap()
        .with_refresh_lock(temp.path().join("refresh.lock"));
    assert!(post(&client, &server).await.is_ok());
    let stored = new_store().load().await.unwrap().unwrap();
    let token = stored.token_response.unwrap();
    assert_eq!(token.access_token().secret(), "fresh-token");
    assert_eq!(token.refresh_token().unwrap().secret(), "rotated-refresh");
    assert!(unrelated.load().await.unwrap().is_some());
    let manager = crate::mcp::oauth::stored_authorization_manager_with_store(&remote, new_store())
        .await
        .unwrap();
    let client = RefreshingHttpClient::new(manager)
        .unwrap()
        .with_refresh_lock(temp.path().join("refresh.lock"));
    assert!(post(&client, &server).await.is_ok());
    assert_eq!(server.state.lock().unwrap().refreshes, 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn separate_managers_serialize_rotating_refresh_tokens() {
    let server = Server::new().await;
    server.state.lock().unwrap().rotate = true;
    let store = store(true, true).await;
    let locks = tempfile::tempdir().unwrap();
    let lock = locks.path().join("refresh.lock");
    let first = server
        .client(store.clone())
        .await
        .with_refresh_lock(lock.clone());
    let second = server.client(store.clone()).await.with_refresh_lock(lock);
    let (first, second) = tokio::join!(post(&first, &server), post(&second, &server));
    assert!(first.is_ok() && second.is_ok());
    assert_eq!(server.state.lock().unwrap().refreshes, 1);
    assert_eq!(
        store
            .load()
            .await
            .unwrap()
            .unwrap()
            .token_response
            .unwrap()
            .refresh_token()
            .unwrap()
            .secret(),
        "rotated-refresh"
    );
}

#[tokio::test]
async fn redirects_do_not_replay_custom_credentials() {
    let server = Server::new().await;
    let client = server.client(store(false, true).await).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri: Arc<str> = format!("http://{}/mcp", listener.local_addr().unwrap()).into();
    let target = format!("{}/mcp", server.url);
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        stream.read(&mut request).await.unwrap();
        stream.write_all(format!("HTTP/1.1 307 Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    });
    let headers = HashMap::from([(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_static("fake-sensitive-header"),
    )]);
    let result = client
        .post_message(
            uri,
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap(),
            None,
            None,
            headers,
        )
        .await;
    assert!(result.is_err());
    task.await.unwrap();
    assert!(
        server.state.lock().unwrap().requests.is_empty(),
        "redirect target must receive no request"
    );
    assert_eq!(server.state.lock().unwrap().refreshes, 0);
}

#[test]
fn only_transport_auth_errors_require_reauthentication() {
    use rmcp::service::ServiceError;
    use rmcp::transport::DynamicTransportError;
    let application_error = ServiceError::McpError(rmcp::model::ErrorData::invalid_params(
        "author not found",
        None,
    ));
    assert!(!is_auth_service_error(&application_error));
    let transport_error = ServiceError::TransportSend(DynamicTransportError::from_parts(
        "test",
        std::any::TypeId::of::<()>(),
        Box::new(HttpError::Auth(AuthError::AuthorizationRequired)),
    ));
    assert!(is_auth_service_error(&transport_error));
    let ordinary_error = ServiceError::TransportSend(DynamicTransportError::from_parts(
        "test",
        std::any::TypeId::of::<()>(),
        Box::new(HttpError::UnexpectedServerResponse(
            "downstream auth configuration failed".into(),
        )),
    ));
    assert!(!is_auth_service_error(&ordinary_error));
}

struct Server {
    url: String,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let origin = url.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let state = shared.clone();
                let origin = origin.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let header_end = loop {
                        let mut bytes = [0u8; 4096];
                        let count = stream.read(&mut bytes).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        request.extend_from_slice(&bytes[..count]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    while request.len() < header_end + content_length {
                        let mut bytes = [0u8; 4096];
                        let count = stream.read(&mut bytes).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        request.extend_from_slice(&bytes[..count]);
                    }
                    let first = headers.lines().next().unwrap();
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap();
                    let path = parts.next().unwrap();
                    let body = String::from_utf8_lossy(&request[header_end..]).to_string();
                    let delay = path == "/token" && state.lock().unwrap().delay_refresh;
                    if delay {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    let (status, content_type, response) =
                        respond(&state, &origin, method, path, &headers, &body);
                    let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len());
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Self { url, state, task }
    }

    fn remote(&self) -> crate::config::configuration::McpRemoteConfig {
        crate::config::configuration::McpRemoteConfig {
            capability: Default::default(),
            url: format!("{}/mcp", self.url),
            headers: HashMap::new(),
            enabled: true,
            timeout_ms: None,
            oauth_enabled: true,
            oauth_client_id: Some("test-client".into()),
            oauth_client_secret: Some("test-secret".into()),
            oauth_scope: None,
        }
    }

    async fn client(&self, store: InMemoryCredentialStore) -> RefreshingHttpClient {
        let manager =
            crate::mcp::oauth::stored_authorization_manager_with_store(&self.remote(), store)
                .await
                .unwrap();
        // Discovery probes /mcp; count only requests sent by the live client.
        self.state.lock().unwrap().requests.clear();
        RefreshingHttpClient::new(manager).unwrap()
    }
}

fn respond(
    state: &Mutex<State>,
    origin: &str,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
) -> (u16, &'static str, String) {
    if path.contains("oauth-authorization-server") {
        return (200, "application/json", json!({"issuer": origin, "authorization_endpoint": format!("{origin}/authorize"), "token_endpoint": format!("{origin}/token"), "scopes_supported": ["tools:read", "offline_access"], "response_types_supported": ["code"], "token_endpoint_auth_methods_supported": ["client_secret_post"]}).to_string());
    }
    let mut state = state.lock().unwrap();
    if path == "/token" {
        state.refreshes += 1;
        let params: HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(
            params.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        assert!(params.contains_key("refresh_token"));
        if state.rotate
            && state.refreshes > 1
            && params.get("refresh_token").map(String::as_str) == Some("old-refresh")
        {
            return (
                400,
                "application/json",
                json!({"error":"invalid_grant"}).to_string(),
            );
        }
        state.secret_seen = params.get("client_secret").map(String::as_str) == Some("test-secret");
        if state.refresh_status != 0 {
            return (
                state.refresh_status,
                "application/json",
                json!({"error":"temporarily_unavailable"}).to_string(),
            );
        }
        if state.reject_refresh {
            return (
                400,
                "application/json",
                json!({"error":"invalid_grant"}).to_string(),
            );
        }
        let mut token =
            json!({"access_token":"fresh-token", "token_type":"Bearer", "expires_in":3600});
        if state.rotate {
            token["refresh_token"] = json!("rotated-refresh");
        }
        return (200, "application/json", token.to_string());
    }
    if path != "/mcp" {
        return (404, "application/json", "{}".into());
    }
    let token = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_owned())
        })
        .unwrap_or_default();
    state.requests.push((method.to_owned(), token.clone()));
    if state.reject_all
        || state
            .required_token
            .as_ref()
            .is_some_and(|required| &token != required)
    {
        let status = if state.reject_status == 0 {
            401
        } else {
            state.reject_status
        };
        let id = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|msg| msg.get("id").cloned())
            .unwrap_or(json!(1));
        // Deliberately no WWW-Authenticate, and a valid JSON-RPC error body.
        return (
            status,
            "application/json",
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"rejected"}})
                .to_string(),
        );
    }
    match method {
        "GET" => (
            200,
            "text/event-stream",
            "event: message\ndata: {}\n\n".into(),
        ),
        "DELETE" => (204, "application/json", String::new()),
        _ => {
            let msg: Value = serde_json::from_str(body).unwrap();
            if msg["method"]
                .as_str()
                .unwrap()
                .starts_with("notifications/")
            {
                return (202, "application/json", String::new());
            }
            let result = if msg["method"] == "initialize" {
                json!({"protocolVersion":msg["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}})
            } else {
                json!({"tools":[]})
            };
            (
                200,
                "application/json",
                json!({"jsonrpc":"2.0","id":msg["id"],"result":result}).to_string(),
            )
        }
    }
}

fn credentials(expired: bool, refresh: bool) -> StoredCredentials {
    let mut token = json!({"access_token":"old-token","token_type":"Bearer","expires_in":3600});
    if refresh {
        token["refresh_token"] = json!("old-refresh");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    StoredCredentials::new(
        "test-client".into(),
        Some(serde_json::from_value(token).unwrap()),
        Vec::new(),
        Some(if expired { now - 3601 } else { now }),
    )
}

async fn store(expired: bool, refresh: bool) -> InMemoryCredentialStore {
    let store = InMemoryCredentialStore::new();
    store.save(credentials(expired, refresh)).await.unwrap();
    store
}

async fn post(
    client: &RefreshingHttpClient,
    server: &Server,
) -> Result<StreamableHttpPostResponse, HttpError> {
    client
        .post_message(
            format!("{}/mcp", server.url).into(),
            serde_json::from_value(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
            )
            .unwrap(),
            None,
            None,
            HashMap::new(),
        )
        .await
}

#[tokio::test]
async fn refreshes_expired_tokens_and_persists_rotation_with_secret() {
    for rotate in [false, true] {
        let server = Server::new().await;
        server.state.lock().unwrap().rotate = rotate;
        let store = store(true, true).await;
        let client = server.client(store.clone()).await;
        assert!(post(&client, &server).await.is_ok());
        let state = server.state.lock().unwrap();
        assert_eq!(state.refreshes, 1);
        assert!(state.secret_seen);
        assert_eq!(state.requests[0].1, "Bearer fresh-token");
        drop(state);
        let stored = store.load().await.unwrap().unwrap();
        let token = stored.token_response.unwrap();
        assert_eq!(
            token.refresh_token().unwrap().secret(),
            if rotate {
                "rotated-refresh"
            } else {
                "old-refresh"
            }
        );
        assert_eq!(token.access_token().secret(), "fresh-token");
    }
}

#[tokio::test]
async fn proactive_refresh_continues_after_transport_initialization() {
    let server = Server::new().await;
    let store = store(false, true).await;
    let client = server.client(store.clone()).await;
    let service = ()
        .serve(StreamableHttpClientTransport::with_client(
            client,
            StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", server.url)),
        ))
        .await
        .unwrap();
    service.list_tools(None).await.unwrap();
    store.save(credentials(true, true)).await.unwrap();
    service.list_tools(None).await.unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.refreshes, 1);
    assert_eq!(state.requests.last().unwrap().1, "Bearer fresh-token");
}

#[tokio::test]
async fn unauthorized_post_refreshes_once_without_challenge_header() {
    let server = Server::new().await;
    server.state.lock().unwrap().required_token = Some("Bearer fresh-token".into());
    let client = server.client(store(false, true).await).await;
    assert!(post(&client, &server).await.is_ok());
    let state = server.state.lock().unwrap();
    assert_eq!(state.refreshes, 1);
    assert_eq!(state.requests.len(), 2);
}

#[tokio::test]
async fn unauthorized_get_and_delete_refresh_and_retry() {
    for method in ["GET", "DELETE"] {
        let server = Server::new().await;
        server.state.lock().unwrap().required_token = Some("Bearer fresh-token".into());
        let client = server.client(store(false, true).await).await;
        let uri: Arc<str> = format!("{}/mcp", server.url).into();
        if method == "GET" {
            assert!(client
                .get_stream(
                    uri,
                    "session".into(),
                    Some("last-event".into()),
                    None,
                    HashMap::new()
                )
                .await
                .is_ok());
        } else {
            assert!(client
                .delete_session(uri, "session".into(), None, HashMap::new())
                .await
                .is_ok());
        }
        let state = server.state.lock().unwrap();
        assert_eq!(state.refreshes, 1);
        assert_eq!(state.requests.len(), 2);
    }
}

#[tokio::test]
async fn persistent_401_stops_after_one_refresh_and_one_retry() {
    let server = Server::new().await;
    server.state.lock().unwrap().reject_all = true;
    let client = server.client(store(false, true).await).await;
    assert!(matches!(
        post(&client, &server).await,
        Err(StreamableHttpError::Auth(AuthError::AuthorizationRequired))
    ));
    let state = server.state.lock().unwrap();
    assert_eq!(state.refreshes, 1);
    assert_eq!(state.requests.len(), 2);
}

#[tokio::test]
async fn missing_or_rejected_refresh_token_requests_reauthentication() {
    for has_refresh in [false, true] {
        let server = Server::new().await;
        {
            let mut state = server.state.lock().unwrap();
            state.reject_all = true;
            state.reject_refresh = true;
        }
        let client = server.client(store(false, has_refresh).await).await;
        assert!(matches!(
            post(&client, &server).await,
            Err(StreamableHttpError::Auth(AuthError::AuthorizationRequired))
        ));
        let state = server.state.lock().unwrap();
        assert_eq!(state.refreshes, usize::from(has_refresh));
        assert_eq!(state.requests.len(), 1);
    }
}

#[tokio::test]
async fn concurrent_rejections_share_the_refreshed_token() {
    let server = Server::new().await;
    server.state.lock().unwrap().required_token = Some("Bearer fresh-token".into());
    let client = server.client(store(false, true).await).await;
    let (first, second) = tokio::join!(post(&client, &server), post(&client, &server));
    assert!(first.is_ok() && second.is_ok());
    assert_eq!(server.state.lock().unwrap().refreshes, 1);
}

#[tokio::test]
async fn non_auth_errors_are_not_refreshed_or_retried() {
    for status in [403, 500] {
        let server = Server::new().await;
        {
            let mut state = server.state.lock().unwrap();
            state.reject_all = true;
            state.reject_status = status;
        }
        let client = server.client(store(false, true).await).await;
        assert!(matches!(
            post(&client, &server).await,
            Ok(StreamableHttpPostResponse::Json(
                ServerJsonRpcMessage::Error(_),
                _
            ))
        ));
        let state = server.state.lock().unwrap();
        assert_eq!(state.refreshes, 0);
        assert_eq!(state.requests.len(), 1);
    }
}

#[tokio::test]
async fn changed_configured_client_requires_fresh_login() {
    let server = Server::new().await;
    let mut remote = server.remote();
    remote.oauth_client_id = Some("different-client".into());
    let error = crate::mcp::oauth::stored_authorization_manager_with_store(
        &remote,
        store(false, true).await,
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("authorization required"));
    assert_eq!(server.state.lock().unwrap().refreshes, 0);
}
