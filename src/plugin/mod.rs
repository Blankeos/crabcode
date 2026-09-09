mod protocol;

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

use crate::config::configuration::PluginSpec;
use protocol::{Request, Response, PROTOCOL_VERSION};

// Integration tests include this module without the CLI dispatcher, so its
// public entry points otherwise appear unused in that separate test crate.
#[allow(dead_code)]
pub mod installer;

const SIDECAR_SOURCE: &str = include_str!("sidecar.mjs");
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PluginHost {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_request_id: u64,
    request_timeout: Duration,
}

impl PluginHost {
    pub async fn start(cache_dir: &Path, workspace: &Path) -> Result<Self> {
        Self::start_with_runtime(cache_dir, workspace, "bun").await
    }

    async fn start_with_runtime(cache_dir: &Path, workspace: &Path, runtime: &str) -> Result<Self> {
        let sidecar_path = install_sidecar(cache_dir).await?;
        let mut child = Command::new(runtime)
            .arg("run")
            .arg(&sidecar_path)
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start plugin runtime `{runtime}`"))?;
        let stdin = child
            .stdin
            .take()
            .context("plugin host stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("plugin host stdout unavailable")?;
        let mut host = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_request_id: 1,
            request_timeout: DEFAULT_TIMEOUT,
        };
        host.call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "workspace": workspace,
            }),
        )
        .await?;
        Ok(host)
    }

    pub async fn load_plugins(&mut self, plugins: &[PluginSpec]) -> Result<Value> {
        let specs: Vec<Value> = plugins
            .iter()
            .map(|plugin| {
                json!({
                    "source": plugin.source,
                    "options": plugin.options,
                })
            })
            .collect();
        self.call("load_plugins", json!({ "plugins": specs })).await
    }

    pub async fn ping(&mut self) -> Result<()> {
        self.call("ping", Value::Null).await.map(|_| ())
    }

    pub async fn invoke_hook(&mut self, hook: &str, input: Value, output: Value) -> Result<Value> {
        self.call(
            "invoke_hook",
            json!({
                "hook": hook,
                "input": input,
                "output": output,
            }),
        )
        .await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let _ = self.call("shutdown", Value::Null).await;
        match timeout(Duration::from_secs(1), self.child.wait()).await {
            Ok(status) => {
                status.context("failed waiting for plugin host")?;
            }
            Err(_) => {
                self.child
                    .kill()
                    .await
                    .context("failed to kill plugin host")?;
            }
        }
        Ok(())
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let request = Request { id, method, params };
        let mut encoded =
            serde_json::to_vec(&request).context("failed to encode plugin request")?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .context("failed to write plugin request")?;
        self.stdin
            .flush()
            .await
            .context("failed to flush plugin request")?;

        let line = match timeout(self.request_timeout, self.stdout.next_line()).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = self.child.kill().await;
                bail!("plugin request `{method}` timed out");
            }
        }
        .ok_or_else(|| anyhow!("plugin host exited during `{method}`"))?;
        let response: Response =
            serde_json::from_str(&line).context("invalid response from plugin host")?;
        if response.id != id {
            bail!(
                "plugin response id mismatch: expected {id}, got {}",
                response.id
            );
        }
        if let Some(error) = response.error {
            bail!(
                "plugin host error {}: {} ({})",
                error.code,
                error.message,
                error.data
            );
        }
        Ok(response.result)
    }

    #[cfg(test)]
    pub(crate) fn set_request_timeout(&mut self, timeout: Duration) {
        self.request_timeout = timeout;
    }

    #[cfg(test)]
    pub(crate) fn process_id(&self) -> Option<u32> {
        self.child.id()
    }
}

async fn install_sidecar(cache_dir: &Path) -> Result<PathBuf> {
    let plugin_dir = cache_dir.join("plugin-host");
    tokio::fs::create_dir_all(&plugin_dir)
        .await
        .context("failed to create plugin host cache directory")?;
    let path = plugin_dir.join(format!("sidecar-v{PROTOCOL_VERSION}.mjs"));
    let needs_write = match tokio::fs::read_to_string(&path).await {
        Ok(existing) => existing != SIDECAR_SOURCE,
        Err(_) => true,
    };
    if needs_write {
        tokio::fs::write(&path, SIDECAR_SOURCE)
            .await
            .context("failed to install plugin host sidecar")?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sidecar_round_trip_when_bun_is_available() {
        if Command::new("bun").arg("--version").output().await.is_err() {
            return;
        }
        let temp = tempfile::tempdir().expect("temp dir");
        let plugin_path = temp.path().join("plugin.mjs");
        tokio::fs::write(
            &plugin_path,
            "export default async ({ options }) => { if (!options.enabled) throw new Error('missing options'); return { 'test.echo': async (input, output) => { output.value = input.value; } }; };",
        )
        .await
        .expect("write plugin fixture");
        let mut host = PluginHost::start(temp.path(), temp.path())
            .await
            .expect("start plugin host");
        let loaded = host
            .load_plugins(&[PluginSpec {
                source: plugin_path.to_string_lossy().into_owned(),
                options: json!({ "enabled": true }),
            }])
            .await
            .expect("load plugin");
        assert_eq!(
            loaded["loaded"][0]["source"],
            plugin_path.to_string_lossy().as_ref()
        );
        let output = host
            .invoke_hook("test.echo", json!({ "value": "ok" }), json!({}))
            .await
            .expect("invoke plugin hook");
        assert_eq!(output["value"], "ok");
        host.ping().await.expect("ping plugin host");
        host.shutdown().await.expect("shutdown plugin host");
    }
}
