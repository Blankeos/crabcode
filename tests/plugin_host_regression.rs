use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

mod config {
    pub mod configuration {
        use serde_json::Value;

        #[derive(Clone, Debug, PartialEq)]
        pub struct PluginSpec {
            pub source: String,
            pub options: Value,
        }
    }
}

#[path = "../src/plugin/mod.rs"]
mod plugin;

use config::configuration::PluginSpec;
use plugin::PluginHost;

async fn bun_available() -> bool {
    Command::new("bun")
        .arg("--version")
        .output()
        .await
        .is_ok_and(|output| output.status.success())
}

async fn write_plugin(root: &Path, name: &str, source: &str) -> PathBuf {
    let path = root.join(name);
    tokio::fs::write(&path, source)
        .await
        .expect("write plugin fixture");
    path
}

fn spec(path: &Path, options: Value) -> PluginSpec {
    PluginSpec {
        source: path.to_string_lossy().into_owned(),
        options,
    }
}

async fn host(root: &Path) -> PluginHost {
    PluginHost::start(root, root)
        .await
        .expect("start Bun plugin host")
}

#[tokio::test]
async fn hooks_chain_in_plugin_order_and_preserve_options() {
    if !bun_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let first = write_plugin(
        temp.path(),
        "first.mjs",
        "export default async ({ options }) => ({ 'test.chain': async (_input, output) => { output.steps.push(options.step); } });",
    )
    .await;
    let second = write_plugin(
        temp.path(),
        "second.mjs",
        "export default async ({ options }) => ({ 'test.chain': async (_input, output) => { output.steps.push(options.step); } });",
    )
    .await;
    let mut host = host(temp.path()).await;

    let loaded = host
        .load_plugins(&[
            spec(&first, json!({ "step": "first" })),
            spec(&second, json!({ "step": "second" })),
        ])
        .await
        .expect("load plugins");
    let output = host
        .invoke_hook("test.chain", Value::Null, json!({ "steps": [] }))
        .await
        .expect("invoke chained hook");

    assert_eq!(loaded["loaded"].as_array().map(Vec::len), Some(2));
    assert_eq!(output, json!({ "steps": ["first", "second"] }));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_hook_is_a_noop() {
    if !bun_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let fixture = write_plugin(temp.path(), "noop.mjs", "export default async () => ({});").await;
    let mut host = host(temp.path()).await;
    host.load_plugins(&[spec(&fixture, Value::Null)])
        .await
        .unwrap();

    let output = host
        .invoke_hook(
            "missing.hook",
            json!({ "ignored": true }),
            json!({ "safe": true }),
        )
        .await
        .unwrap();

    assert_eq!(output, json!({ "safe": true }));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn plugin_factory_and_hook_errors_cross_the_rpc_boundary() {
    if !bun_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let factory_error = write_plugin(
        temp.path(),
        "factory-error.mjs",
        "export default async () => { throw new Error('factory exploded'); };",
    )
    .await;
    let hook_error = write_plugin(
        temp.path(),
        "hook-error.mjs",
        "export default async () => ({ 'test.fail': async () => { throw new Error('hook exploded'); } });",
    )
    .await;
    let mut host = host(temp.path()).await;

    let error = host
        .load_plugins(&[spec(&factory_error, Value::Null)])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("factory exploded"));

    host.load_plugins(&[spec(&hook_error, Value::Null)])
        .await
        .unwrap();
    let error = host
        .invoke_hook("test.fail", Value::Null, Value::Null)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("hook exploded"));
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn hook_timeout_is_bounded_and_shutdown_kills_the_host() {
    if !bun_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let fixture = write_plugin(
        temp.path(),
        "hang.mjs",
        "export default async () => ({ 'test.hang': async () => await new Promise(() => {}) });",
    )
    .await;
    let mut host = host(temp.path()).await;
    host.load_plugins(&[spec(&fixture, Value::Null)])
        .await
        .unwrap();
    host.set_request_timeout(Duration::from_millis(100));
    let pid = host.process_id().expect("plugin host pid");

    let error = host
        .invoke_hook("test.hang", Value::Null, Value::Null)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));

    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .await
        .expect("check plugin host process");
    assert!(
        !status.success(),
        "plugin host process {pid} survived the request timeout"
    );
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn plugin_stdout_does_not_corrupt_rpc_and_process_exit_is_reported() {
    if !bun_available().await {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let corrupt = write_plugin(
        temp.path(),
        "stdout.mjs",
        "export default async () => { console.log('not-json'); return {}; };",
    )
    .await;
    let exit = write_plugin(
        temp.path(),
        "exit.mjs",
        "export default async () => ({ 'test.exit': async () => process.exit(17) });",
    )
    .await;

    let mut corrupt_host = host(temp.path()).await;
    let loaded = corrupt_host
        .load_plugins(&[spec(&corrupt, Value::Null)])
        .await
        .expect("plugin stdout must be isolated from RPC stdout");
    assert_eq!(loaded["loaded"].as_array().map(Vec::len), Some(1));
    corrupt_host.shutdown().await.unwrap();

    let mut exit_host = host(temp.path()).await;
    exit_host
        .load_plugins(&[spec(&exit, Value::Null)])
        .await
        .unwrap();
    let error = exit_host
        .invoke_hook("test.exit", Value::Null, Value::Null)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exited"));
    exit_host.shutdown().await.unwrap();
}
