use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

struct AcpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<std::io::Result<String>>,
    _home: tempfile::TempDir,
    _config: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl AcpProcess {
    fn spawn(workspace: &std::path::Path) -> Self {
        let home = tempfile::tempdir().expect("home");
        let config = tempfile::tempdir().expect("config");
        let state = tempfile::tempdir().expect("state");
        let mut child = Command::new(env!("CARGO_BIN_EXE_crabcode"))
            .args(["acp", "--cwd"])
            .arg(workspace)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", config.path())
            .env("XDG_STATE_HOME", state.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn crabcode acp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (line_tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            lines,
            _home: home,
            _config: config,
            _state: state,
        }
    }

    fn send(&mut self, request: serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("open stdin");
        writeln!(stdin, "{request}").expect("write ACP request");
        stdin.flush().expect("flush ACP request");
    }

    fn recv(&mut self) -> serde_json::Value {
        let line = match self.lines.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                let _ = self.child.kill();
                panic!("failed reading ACP response: {error}");
            }
            Err(_) => {
                let _ = self.child.kill();
                panic!("timed out waiting for ACP message");
            }
        };
        serde_json::from_str(&line).unwrap_or_else(|error| {
            let _ = self.child.kill();
            panic!("invalid protocol message {line:?}: {error}");
        })
    }

    fn recv_response(&mut self, id: u64) -> (serde_json::Value, Vec<serde_json::Value>) {
        let mut notifications = Vec::new();
        loop {
            let message = self.recv();
            if message.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
                return (message, notifications);
            }
            notifications.push(message);
        }
    }

    fn close_and_wait(mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll ACP process") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("ACP process did not shut down after stdin EOF");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success(), "ACP exited with {status}");
    }
}

fn initialize(process: &mut AcpProcess) -> serde_json::Value {
    process.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {}
        }
    }));
    process.recv_response(1).0
}

#[test]
fn initialize_over_stdio_and_shutdown_on_eof() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut process = AcpProcess::spawn(workspace.path());
    let response = initialize(&mut process);

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert_eq!(response["result"]["agentInfo"]["name"], "crabcode");
    assert_eq!(
        response["result"]["agentInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        response["result"]["agentCapabilities"]["promptCapabilities"]["audio"],
        true
    );

    process.close_and_wait();
}

#[test]
fn advertises_and_dispatches_commands_over_stdio() {
    let workspace = tempfile::tempdir().expect("workspace");
    let command_dir = workspace.path().join(".crabcode/commands");
    std::fs::create_dir_all(&command_dir).expect("command dir");
    std::fs::write(
        command_dir.join("wire-probe.md"),
        r#"---
description: Verify ACP custom command rendering
agent: missing-agent
---
Probe $ARGUMENTS !`printf wired > command-marker`
"#,
    )
    .expect("custom command");
    let skill_dir = workspace.path().join(".crabcode/skills/wire-skill");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: wire-skill
description: Verify ACP skill discovery
---
Wire skill instructions.
"#,
    )
    .expect("skill");
    let mut process = AcpProcess::spawn(workspace.path());
    initialize(&mut process);

    process.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": workspace.path(),
            "mcpServers": []
        }
    }));
    let (new_session, mut notifications) = process.recv_response(2);
    let session_id = new_session["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string();
    while !notifications
        .iter()
        .any(|message| message["params"]["update"]["sessionUpdate"] == "available_commands_update")
    {
        notifications.push(process.recv());
    }
    let command_update = notifications
        .iter()
        .find(|message| message["params"]["update"]["sessionUpdate"] == "available_commands_update")
        .expect("available commands update");
    let names = command_update["params"]["update"]["availableCommands"]
        .as_array()
        .expect("commands")
        .iter()
        .filter_map(|command| command["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"btw"));
    assert!(names.contains(&"compact"));
    assert!(names.contains(&"mcp"));
    assert!(names.contains(&"skills"));
    assert!(names.contains(&"wire-probe"));
    assert!(names.contains(&"wire-skill"));

    for (id, command, expected_text) in [
        (3, "/skills", "wire-skill"),
        (4, "/mcp", "No MCP servers are configured"),
    ] {
        process.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": command }]
            }
        }));
        let (response, notifications) = process.recv_response(id);
        assert_eq!(response["result"]["stopReason"], "end_turn");
        assert!(notifications.iter().any(|message| {
            message["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                && message["params"]["update"]["content"]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(expected_text))
        }));
    }

    process.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": "/wire-probe custom args" }]
        }
    }));
    let (response, _) = process.recv_response(5);
    assert_eq!(response["error"]["code"], -32602);
    assert!(response["error"]["data"]
        .as_str()
        .is_some_and(|data| data.contains("unknown agent")));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("command-marker")).expect("command marker"),
        "wired"
    );

    process.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": "/compact" }]
        }
    }));
    let (response, _) = process.recv_response(6);
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["data"], "Nothing to compact");

    process.close_and_wait();
}
