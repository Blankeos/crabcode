//! Provider-neutral wire tests for account capability recovery in MCP.

use super::*;
use crate::config::mcp_capability::{McpCapabilityPolicy, Recovery, RecoveryKind, RecoveryRule};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

const ENDPOINT_KEY: &str = "test-endpoint-key";
const SERVER_NAME: &str = "account-tools";
const SETTINGS_URL: &str = "https://example.test/account/consent";

#[derive(Clone, Copy)]
enum ResponseMode {
    Consent { structured: bool },
    Unsupported,
    NeverRespond,
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

struct LocalServer {
    base_url: String,
    consent: Arc<AtomicBool>,
    requests: Arc<std::sync::Mutex<Vec<Request>>>,
    task: JoinHandle<()>,
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        // Dropping the accept task also drops its JoinSet, aborting held sockets.
        self.task.abort();
    }
}

impl LocalServer {
    async fn start(mode: ResponseMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let consent = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let task = tokio::spawn({
            let consent = consent.clone();
            let requests = requests.clone();
            async move {
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (socket, _) = accepted.unwrap();
                            connections.spawn(handle_request(socket, mode, consent.clone(), requests.clone()));
                        }
                        finished = connections.join_next(), if !connections.is_empty() => {
                            finished.unwrap().unwrap();
                        }
                    }
                }
            }
        });
        Self {
            base_url,
            consent,
            requests,
            task,
        }
    }

    fn config(&self) -> McpServerConfig {
        McpServerConfig::Remote(McpRemoteConfig {
            url: format!("{}/mcp", self.base_url),
            headers: HashMap::from([("Authorization".into(), format!("Bearer {ENDPOINT_KEY}"))]),
            enabled: true,
            timeout_ms: Some(5_000),
            oauth_enabled: false,
            oauth_client_id: None,
            oauth_client_secret: None,
            oauth_scope: None,
            capability: McpCapabilityPolicy {
                authorization: None,
                recovery_rules: vec![
                    RecoveryRule {
                        markers: vec!["needs_consent".into()],
                        recovery: Recovery {
                            kind: RecoveryKind::Consent,
                            message: format!("Grant account access at {SETTINGS_URL}"),
                        },
                    },
                    RecoveryRule {
                        markers: vec!["404".into()],
                        recovery: Recovery {
                            kind: RecoveryKind::Unavailable,
                            message: "Capability unavailable".into(),
                        },
                    },
                ],
            },
        })
    }

    fn assert_requests(&self, methods: &[&str]) {
        assert!(!self.task.is_finished(), "local HTTP server failed");
        let requests = self.requests.lock().unwrap();
        let posts: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
        let actual: Vec<_> = posts
            .iter()
            .map(|r| r.body["method"].as_str().unwrap())
            .collect();
        assert_eq!(actual, methods);
        for request in requests.iter() {
            assert_eq!(request.path, "/v1/mcp");
            assert!(matches!(request.method.as_str(), "POST" | "GET"));
            let authorization: Vec<_> = request
                .headers
                .iter()
                .filter(|(name, _)| name == "authorization")
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(authorization, vec![format!("Bearer {ENDPOINT_KEY}")]);
            assert!(!request.headers.iter().any(|(name, _)| name == "cookie"));
        }
        for request in posts.iter().filter(|r| r.body["method"] == "tools/call") {
            assert_eq!(request.body["params"]["name"], "read_design");
            assert_eq!(
                request.body["params"]["arguments"],
                json!({"id": "local-design"})
            );
        }
    }
}

async fn handle_request(
    mut socket: TcpStream,
    mode: ResponseMode,
    consent: Arc<AtomicBool>,
    requests: Arc<std::sync::Mutex<Vec<Request>>>,
) {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await.unwrap();
        if count == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() < 64 * 1024, "unexpectedly large MCP request");
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap().to_string();
    let path = first.next().unwrap().to_string();
    let headers: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length < 64 * 1024);
    while bytes.len() < header_end + length {
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0, "incomplete HTTP request body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
    };
    requests.lock().unwrap().push(Request {
        method: method.clone(),
        path,
        headers,
        body: body.clone(),
    });

    let (status, response) = if method == "GET" {
        // Stateless MCP: no session header, and no standalone SSE stream.
        ("405 Method Not Allowed", String::new())
    } else if matches!(mode, ResponseMode::NeverRespond) {
        std::future::pending::<()>().await;
        unreachable!()
    } else if matches!(mode, ResponseMode::Unsupported) {
        ("404 Not Found", String::new())
    } else if body.get("id").is_none() {
        ("202 Accepted", String::new())
    } else {
        let result = match body["method"].as_str().unwrap() {
            "initialize" => json!({
                "protocolVersion": body["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "local-account-tools", "version": "1.0"}
            }),
            "tools/list" => json!({"tools": [{
                "name": "read_design",
                "description": "Read a account capability project",
                "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}
            }]}),
            "tools/call" if consent.load(Ordering::SeqCst) => json!({
                "isError": false,
                "content": [{"type": "text", "text": "local design contents"}]
            }),
            "tools/call" => {
                if matches!(mode, ResponseMode::Consent { structured: true }) {
                    json!({"isError": true, "content": [], "structuredContent": {"error": "needs_consent"}})
                } else {
                    json!({"isError": true, "content": [{"type": "text", "text": "{\"error\":\"needs_consent\"}"}]})
                }
            }
            method => panic!("unexpected MCP method: {method}"),
        };
        (
            "200 OK",
            json!({"jsonrpc": "2.0", "id": body["id"], "result": result}).to_string(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
    socket.write_all(response.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn capability_consent_preserves_tools_and_retry_recovers_over_http() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for structured in [false, true] {
            let server = LocalServer::start(ResponseMode::Consent { structured }).await;
            let config = server.config();
            let (client, tools) = open_and_list_tools(Path::new("."), SERVER_NAME, &config)
                .await
                .unwrap_or_else(|status| panic!("handshake failed: {status:?}"));
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "read_design");
            assert_eq!(tools[0].server, SERVER_NAME);
            assert_eq!(tools[0].input_schema["required"], json!(["id"]));
            let tool_id = tools[0].tool_id.clone();
            let mut manager = McpManager {
                workspace: PathBuf::from("."),
                servers: BTreeMap::from([(
                    SERVER_NAME.to_string(),
                    McpServerState {
                        config,
                        status: McpStatus::Connected,
                        client: Some(client),
                        tools,
                    },
                )]),
            };
            let error = manager
                .call_tool(SERVER_NAME, "read_design", json!({"id": "local-design"}))
                .await
                .unwrap_err();
            assert!(error.to_string().contains(SETTINGS_URL));
            let view = &manager.views()[0];
            assert_eq!(view.status, "needs_consent");
            let detail = view.detail.as_deref().unwrap();
            assert!(detail.contains(SETTINGS_URL));
            assert!(detail.contains("Grant account access"));
            assert_eq!(manager.tools().len(), 1);
            assert_eq!(manager.tools()[0].tool_id, tool_id);
            assert!(manager.servers[SERVER_NAME].client.is_some());

            server.consent.store(true, Ordering::SeqCst);
            let result = manager
                .call_tool(SERVER_NAME, "read_design", json!({"id": "local-design"}))
                .await
                .unwrap();
            assert_eq!(result.output, "local design contents");
            assert_eq!(manager.views()[0].status, "connected");
            assert!(manager.views()[0].detail.is_none());
            assert_eq!(manager.tools()[0].tool_id, tool_id);
            server.assert_requests(&[
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/call",
                "tools/call",
            ]);
        }
    })
    .await
    .expect("local account capability consent test exceeded five seconds");
}

#[tokio::test]
async fn capability_unsupported_handshake_is_unavailable() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = LocalServer::start(ResponseMode::Unsupported).await;
        let status = match open_and_list_tools(Path::new("."), SERVER_NAME, &server.config()).await
        {
            Err(status) => status,
            Ok(_) => panic!("404 handshake unexpectedly succeeded"),
        };
        assert_eq!(status.as_str(), "unavailable");
        assert!(status.detail().unwrap().contains("Capability unavailable"));
        server.assert_requests(&["initialize"]);
    })
    .await
    .expect("local unsupported account capability test exceeded five seconds");
}

#[tokio::test]
async fn capability_never_responding_handshake_is_bounded() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let server = LocalServer::start(ResponseMode::NeverRespond).await;
        let mut config = server.config();
        let McpServerConfig::Remote(remote) = &mut config else { unreachable!() };
        remote.timeout_ms = Some(200);
        let started = tokio::time::Instant::now();
        let status = match open_and_list_tools(Path::new("."), SERVER_NAME, &config).await {
            Err(status) => status,
            Ok(_) => panic!("silent handshake unexpectedly succeeded"),
        };
        assert!(matches!(&status, McpStatus::Failed(message) if message.contains("timed out after 200 ms while connecting")));
        assert!(started.elapsed() < Duration::from_secs(2));
        server.assert_requests(&["initialize"]);
    })
    .await
    .expect("local silent account capability handshake test exceeded five seconds");
}
