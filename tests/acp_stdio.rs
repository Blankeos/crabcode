use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn initialize_over_stdio_and_shutdown_on_eof() {
    let workspace = tempfile::tempdir().expect("workspace");
    let home = tempfile::tempdir().expect("home");
    let config = tempfile::tempdir().expect("config");
    let state = tempfile::tempdir().expect("state");
    let mut child = Command::new(env!("CARGO_BIN_EXE_crabcode"))
        .args(["acp", "--cwd"])
        .arg(workspace.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", config.path())
        .env("XDG_STATE_HOME", state.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn crabcode acp");

    let stdout = child.stdout.take().expect("stdout");
    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = line_tx.send(result);
    });

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {}
        }
    });
    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{request}").expect("write initialize");
    stdin.flush().expect("flush initialize");

    let line = match line_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(line)) => line,
        Ok(Err(error)) => {
            let _ = child.kill();
            panic!("failed reading ACP response: {error}");
        }
        Err(_) => {
            let _ = child.kill();
            panic!("timed out waiting for ACP initialize response");
        }
    };
    let response: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_else(|error| {
        let _ = child.kill();
        panic!("invalid protocol response {line:?}: {error}");
    });
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

    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll ACP process") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("ACP process did not shut down after stdin EOF");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "ACP exited with {status}");
}
