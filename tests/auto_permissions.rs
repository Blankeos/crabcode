//! Offline, real-process permission tests. No credentials or public services are used.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CONTENT: &str = "written by the offline permission probe\n";
const ANSWER: &str = "offline permission probe finished";
const TIMEOUT: Duration = Duration::from_secs(30);

struct MockProvider {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockProvider {
    fn start(target: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker_stop = stop.clone();
        let worker_requests = requests.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &target, &worker_requests),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept mock request: {error}"),
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn tool_result(&self) -> String {
        let requests = self.requests.lock().unwrap();
        let results: Vec<_> = requests
            .iter()
            .flat_map(|request| request["messages"].as_array().into_iter().flatten())
            .filter(|message| message["role"] == "tool")
            .collect();
        assert!(
            !results.is_empty(),
            "provider never received a tool result: {requests:#?}"
        );
        results
            .iter()
            .map(|message| message.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.expect("mock provider thread");
            }
        }
    }
}

fn serve(mut stream: TcpStream, target: &Path, requests: &Mutex<Vec<Value>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    reader.read_line(&mut first).expect("HTTP request line");
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).expect("HTTP header") > 0,
            "truncated HTTP headers"
        );
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
    }
    assert!(length < 8 * 1024 * 1024, "unexpectedly large mock request");
    let (mime, body) = if first.starts_with("GET /v1/models ") {
        (
            "application/json",
            json!({"data": [{"id": "permission-probe"}]}).to_string(),
        )
    } else {
        assert!(
            first.starts_with("POST /v1/chat/completions "),
            "unexpected request: {first}"
        );
        let mut body = vec![0; length];
        reader.read_exact(&mut body).expect("HTTP body");
        let request: Value = serde_json::from_slice(&body).expect("provider JSON");
        assert_eq!(request["model"], "permission-probe");
        let has_result = request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .take_while(|message| message["role"] != "user")
            .any(|message| message["role"] == "tool");
        let has_write = request["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == "write"));
        let (delta, reason) = if !has_result && has_write {
            (
                json!({"role": "assistant", "tool_calls": [{"index": 0, "id": "permission-probe-call", "type": "function", "function": {
                    "name": "write", "arguments": json!({"file_path": target, "content": CONTENT}).to_string()
                }}]}),
                "tool_calls",
            )
        } else {
            (json!({"role": "assistant", "content": ANSWER}), "stop")
        };
        requests.lock().unwrap().push(request);
        let chunks = [
            json!({"id": "offline-probe", "object": "chat.completion.chunk", "created": 1, "model": "permission-probe", "choices": [{"index": 0, "delta": delta, "finish_reason": null}]}),
            json!({"id": "offline-probe", "object": "chat.completion.chunk", "created": 1, "model": "permission-probe", "choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}),
        ];
        (
            "text/event-stream",
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunks[0], chunks[1]
            ),
        )
    };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("mock response");
    stream.flush().unwrap();
}

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    target: PathBuf,
    provider: MockProvider,
}

impl Fixture {
    fn new(deny: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let target = workspace.join("auto-marker.txt");
        let provider = MockProvider::start(target.clone());
        // A path-specific deny keeps `write` advertised so the test exercises
        // permission preflight rather than merely hiding the tool from the model.
        let edit = if deny {
            json!({"*": "ask", "*/auto-marker.txt": "deny"})
        } else {
            json!("ask")
        };
        std::fs::write(workspace.join("crabcode.json"), json!({
            "model": "offline-auto/permission-probe",
            "small_model": "offline-auto/permission-probe",
            "enabled_providers": ["offline-auto"],
            "provider": {"offline-auto": {
                "npm": "@ai-sdk/openai-compatible",
                "options": {"baseURL": provider.url, "apiKey": "offline-test-not-a-secret"},
                "models": {"permission-probe": {"tool_call": true, "limit": {"context": 1000000, "output": 1024}}}
            }},
            "permission": {"edit": edit},
            "compaction": false,
            "watcher": false,
            "notifications": false
        }).to_string()).unwrap();
        let cache = root.path().join("state/crabcode/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("models_dev_cache.json"), json!({
            "data": {}, "schema_version": 5,
            "timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        }).to_string()).unwrap();
        for dir in ["home", "config", "cache"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        Self {
            root,
            workspace,
            target,
            provider,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crabcode"));
        // Inherited test-mode flags select shared /tmp paths; inherited proxy
        // settings can even send loopback traffic through a public proxy.
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("TERM", "dumb")
            .current_dir(&self.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// Always reap the real binary, including on assertion failures and timeouts.
struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

impl Process {
    fn spawn(command: &mut Command) -> Self {
        let mut child = command.spawn().expect("spawn crabcode");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let stderr_pipe = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.expect("stdout line")).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr_pipe).read_to_string(&mut text);
            *captured.lock().unwrap() = text;
        });
        Self {
            child,
            stdin,
            lines,
            stderr,
        }
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
        messages: &mut Vec<Value>,
    ) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let response = self.response(id, messages);
        assert!(
            response.get("error").is_none() && response.get("result").is_some(),
            "ACP {method} failed: {response:#?}"
        );
        response["result"].clone()
    }

    fn response(&mut self, id: u64, messages: &mut Vec<Value>) -> Value {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!(
                        "waiting for ACP response {id}: {error}; stderr: {}",
                        self.stderr.lock().unwrap()
                    )
                });
            let message: Value =
                serde_json::from_str(&line).expect("ACP JSON (stdout must be protocol-only)");
            // Respond even in --auto tests: an accidental permission request
            // should fail assertions, not leave a prompt blocked forever.
            if message["method"] == "session/request_permission" {
                self.send(json!({"jsonrpc": "2.0", "id": message["id"], "result": {"outcome": {"outcome": "cancelled"}}}));
            }
            if message["id"] == id && message.get("method").is_none() {
                return message;
            }
            messages.push(message);
        }
    }

    fn finish(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "crabcode exited {status}; stderr: {}",
                    self.stderr.lock().unwrap()
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "crabcode did not exit after stdin EOF"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn acp_probe(args: &[&str], deny: bool, expect_permission: bool) {
    let fixture = Fixture::new(deny);
    let mut process = Process::spawn(fixture.command().args(args));
    let mut messages = Vec::new();
    process.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}}));
    assert!(process.response(1, &mut messages).get("result").is_some());
    process.send(json!({"jsonrpc": "2.0", "id": 2, "method": "session/new", "params": {"cwd": fixture.workspace, "mcpServers": []}}));
    let response = process.response(2, &mut messages);
    let session = response["result"]["sessionId"]
        .as_str()
        .expect("new session id");
    process.send(json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt", "params": {"sessionId": session, "prompt": [{"type": "text", "text": "Run the offline write permission probe."}]}}));
    let response = process.response(3, &mut messages);
    assert_eq!(
        response["result"]["stopReason"], "end_turn",
        "{response:#?}"
    );
    process.finish();
    // Also inspect messages queued before EOF (including late notifications).
    messages.extend(
        process
            .lines
            .iter()
            .map(|line| serde_json::from_str::<Value>(&line).unwrap()),
    );
    let permissions = messages
        .iter()
        .filter(|message| message["method"] == "session/request_permission")
        .count();
    assert_eq!(permissions, usize::from(expect_permission), "{messages:#?}");
    assert!(
        messages
            .iter()
            .any(|message| message["params"]["update"]["content"]["text"]
                .as_str()
                .is_some_and(|text| text.contains(ANSWER))),
        "missing final answer: {messages:#?}"
    );
    let result = fixture.provider.tool_result();
    if deny || expect_permission {
        assert!(
            !fixture.target.exists(),
            "denied write reached the filesystem"
        );
        assert!(
            result.to_lowercase().contains("permission"),
            "expected permission failure: {result}"
        );
    } else {
        assert_eq!(std::fs::read_to_string(&fixture.target).unwrap(), CONTENT);
        assert!(
            result.contains("Created file"),
            "expected successful tool result: {result}"
        );
    }
}

#[test]
fn acp_auto_after_subcommand_executes_without_permission_requests() {
    acp_probe(&["acp", "--auto"], false, false);
}

#[test]
fn acp_auto_before_subcommand_executes_without_permission_requests() {
    acp_probe(&["--auto", "acp"], false, false);
}

#[test]
fn acp_without_auto_requests_permission_and_honors_rejection() {
    acp_probe(&["acp"], false, true);
}

#[test]
fn acp_auto_preserves_explicit_config_deny() {
    acp_probe(&["acp", "--auto"], true, false);
}

#[test]
fn acp_auto_preserved_across_close_load_resume_and_fork() {
    let fixture = Fixture::new(false);
    let mut process = Process::spawn(fixture.command().args(["acp", "--auto"]));
    let mut messages = Vec::new();
    process.request(
        1,
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
        &mut messages,
    );
    let result = process.request(
        2,
        "session/new",
        json!({"cwd": fixture.workspace, "mcpServers": []}),
        &mut messages,
    );
    let session = result["sessionId"].as_str().expect("new session id");
    let mut active_session = session.to_owned();
    for (turn, transition) in ["new", "load", "resume", "fork"].into_iter().enumerate() {
        let id = 3 + turn as u64 * 3;
        if transition == "load" || transition == "resume" {
            process.request(
                id,
                "session/close",
                json!({"sessionId": session}),
                &mut messages,
            );
            process.request(
                id + 1,
                &format!("session/{transition}"),
                json!({"sessionId": session, "cwd": fixture.workspace, "mcpServers": []}),
                &mut messages,
            );
        } else if transition == "fork" {
            let fork = process.request(
                id,
                "session/fork",
                json!({"sessionId": session, "cwd": fixture.workspace, "mcpServers": []}),
                &mut messages,
            );
            active_session = fork["sessionId"]
                .as_str()
                .expect("fork session id")
                .to_owned();
            assert_ne!(active_session, session);
        }
        let start = messages.len();
        let result = process.request(
            id + 2,
            "session/prompt",
            json!({"sessionId": active_session, "prompt": [{"type": "text", "text": "Run the offline write permission probe."}]}),
            &mut messages,
        );
        assert_eq!(
            result["stopReason"], "end_turn",
            "{transition}: {result:#?}"
        );
        assert!(
            messages[start..]
                .iter()
                .all(|message| message["method"] != "session/request_permission"),
            "{transition} requested permission: {:#?}",
            &messages[start..]
        );
        assert!(
            messages[start..]
                .iter()
                .any(|message| message["params"]["update"]["content"]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(ANSWER))),
            "{transition} missing final answer: {:#?}",
            &messages[start..]
        );
        assert_eq!(
            std::fs::read_to_string(&fixture.target)
                .unwrap_or_else(|error| panic!("{transition} did not write marker: {error}")),
            CONTENT,
            "{transition}"
        );
        // Every turn must execute a fresh write, not reuse an earlier result.
        std::fs::remove_file(&fixture.target).unwrap();
    }
    process.finish();
    messages.extend(
        process
            .lines
            .iter()
            .map(|line| serde_json::from_str::<Value>(&line).unwrap()),
    );
    assert!(
        messages
            .iter()
            .all(|message| message["method"] != "session/request_permission"),
        "unexpected permission request: {messages:#?}"
    );
}

#[test]
fn print_auto_executes_configured_ask_offline() {
    let fixture = Fixture::new(false);
    let mut process = Process::spawn(fixture.command().args([
        "--auto",
        "-p",
        "Run the offline write permission probe.",
    ]));
    process.finish();
    let output = process.lines.iter().collect::<Vec<_>>().join("\n");
    assert!(output.contains(ANSWER), "{output}");
    assert_eq!(std::fs::read_to_string(&fixture.target).unwrap(), CONTENT);
    assert!(fixture.provider.tool_result().contains("Created file"));
}
