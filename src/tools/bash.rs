use crate::llm::ChunkSender;
use crate::tools::process_registry::{JobStatus, ProcessRegistry};
use crate::tools::terminal_session::TerminalSessionTool;
use crate::tools::{
    get_integer_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
/// Cap bash output sent toward the model. Aligns with Grok Build's ~20k-char
/// bash limit (OpenCode defaults to 50KiB; Codex model-facing truncates nearer
/// ~10k tokens). Tighter caps cut SuperGrok / long-session token burn.
const MAX_OUTPUT_BYTES: usize = 20_000;
const READ_CHUNK_SIZE: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BashMode {
    Foreground,
    Background,
    Interactive,
}

impl BashMode {
    fn parse(raw: &str) -> Result<Self, ToolError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "foreground" => Ok(Self::Foreground),
            "background" => Ok(Self::Background),
            "interactive" => Ok(Self::Interactive),
            other => Err(ToolError::Validation(format!(
                "Unknown bash mode '{other}'. Expected foreground|background|interactive"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Interactive => "interactive",
        }
    }
}

pub struct BashTool {
    chunk_tx: Option<ChunkSender>,
    registry: Option<Arc<ProcessRegistry>>,
}

impl BashTool {
    pub fn new() -> Self {
        Self {
            chunk_tx: None,
            registry: None,
        }
    }

    pub fn with_sender(mut self, sender: ChunkSender) -> Self {
        self.chunk_tx = Some(sender);
        self
    }

    pub fn with_sender_opt(mut self, sender: Option<ChunkSender>) -> Self {
        self.chunk_tx = sender;
        self
    }

    pub fn with_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    fn resolve_registry<'a>(&'a self, ctx: &'a ToolContext) -> Option<&'a Arc<ProcessRegistry>> {
        self.registry.as_ref().or(ctx.process_registry.as_ref())
    }

    async fn execute_foreground(
        &self,
        command_str: String,
        description: String,
        workdir: Option<String>,
        timeout_seconds: u64,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&command_str);
            c
        } else {
            let mut c = Command::new("bash");
            c.arg("-c").arg(&command_str);
            c
        };

        if let Some(dir) = workdir {
            cmd.current_dir(dir);
        } else {
            cmd.current_dir(ctx.workdir());
        }

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Execution(format!("Failed to spawn process: {}", e)))?;
        let process_group_id = child.id();
        let mut process_group_guard = ProcessGroupGuard::new(process_group_id);

        let stdout = child.stdout.take().expect("stdout should be piped");
        let stderr = child.stderr.take().expect("stderr should be piped");

        let mut stdout_reader = BufReader::new(stdout);
        let mut stderr_reader = BufReader::new(stderr);

        let mut stdout_buf: Vec<u8> = Vec::new();
        let mut stderr_buf: Vec<u8> = Vec::new();
        let mut stdout_truncated = false;
        let mut stderr_truncated = false;

        let timeout_duration = Duration::from_secs(timeout_seconds);

        let result = timeout(timeout_duration, async {
            let mut stdout_done = false;
            let mut stderr_done = false;
            let mut exit_status = None;
            let mut stdout_chunk = vec![0u8; READ_CHUNK_SIZE];
            let mut stderr_chunk = vec![0u8; READ_CHUNK_SIZE];

            loop {
                if ctx.is_aborted() {
                    terminate_child(&mut child).await;
                    return Err(ToolError::Execution("Command aborted".to_string()));
                }

                if stdout_done && stderr_done {
                    return if let Some(exit_status) = exit_status {
                        Ok(exit_status)
                    } else {
                        match child.wait().await {
                            Ok(exit_status) => {
                                process_group_guard.kill();
                                Ok(exit_status)
                            }
                            Err(e) => Err(ToolError::Execution(format!("Process error: {}", e))),
                        }
                    };
                }

                tokio::select! {
                    read = stdout_reader.read(&mut stdout_chunk), if !stdout_done => {
                        match read {
                            Ok(0) => stdout_done = true,
                            Ok(n) => append_capped(&mut stdout_buf, &stdout_chunk[..n], &mut stdout_truncated),
                            Err(e) => return Err(ToolError::Execution(format!("Error reading stdout: {}", e))),
                        }
                    }
                    read = stderr_reader.read(&mut stderr_chunk), if !stderr_done => {
                        match read {
                            Ok(0) => stderr_done = true,
                            Ok(n) => append_capped(&mut stderr_buf, &stderr_chunk[..n], &mut stderr_truncated),
                            Err(e) => return Err(ToolError::Execution(format!("Error reading stderr: {}", e))),
                        }
                    }
                    status = child.wait(), if exit_status.is_none() => {
                        match status {
                            Ok(status) => {
                                exit_status = Some(status);
                                // A shell can exit successfully while background descendants
                                // keep running and retain the output pipes. Kill the process group
                                // so those descendants cannot leak beyond this tool invocation.
                                process_group_guard.kill();
                            }
                            Err(e) => return Err(ToolError::Execution(format!("Process error: {}", e))),
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(25)) => {}
                }
            }
        })
        .await;

        let exit_status = match result {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                terminate_child(&mut child).await;
                if let Some(stdout) = child.stdout.take() {
                    drain_reader(stdout).await;
                }
                if let Some(stderr) = child.stderr.take() {
                    drain_reader(stderr).await;
                }
                let _ = child.wait().await;
                return Err(ToolError::Execution(format!(
                    "Command timed out after {} seconds",
                    timeout_seconds
                )));
            }
        };

        let stdout_text = String::from_utf8_lossy(&stdout_buf).into_owned();
        let stderr_text = String::from_utf8_lossy(&stderr_buf).into_owned();

        let mut output_parts = Vec::new();
        if !stdout_text.is_empty() {
            output_parts.push(stdout_text);
        }
        if !stderr_text.is_empty() {
            if !output_parts.is_empty() {
                output_parts.push("\n--- stderr ---".to_string());
            }
            output_parts.push(stderr_text);
        }

        let output = if output_parts.is_empty() {
            "(no output)".to_string()
        } else {
            output_parts.join("\n")
        };

        let truncated = stdout_truncated || stderr_truncated;
        let final_output = if truncated {
            format!(
                "{}\n\n[Output truncated to {} bytes]",
                output, MAX_OUTPUT_BYTES
            )
        } else {
            output
        };

        let exit_code = exit_status.code().unwrap_or(-1);

        Ok(
            ToolResult::new(format!("Bash: {}", description), final_output)
                .with_metadata("exit_code", serde_json::json!(exit_code))
                .with_metadata("command", serde_json::json!(command_str))
                .with_metadata("mode", serde_json::json!(BashMode::Foreground.as_str())),
        )
    }

    async fn execute_background(
        &self,
        command_str: String,
        description: String,
        workdir: PathBuf,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let registry = self.resolve_registry(ctx).ok_or_else(|| {
            ToolError::Execution(
                "Background mode requires a ProcessRegistry (not available in this context)"
                    .to_string(),
            )
        })?;

        let session_id = (!ctx.session_id.is_empty()).then(|| ctx.session_id.clone());
        let spawned = registry
            .spawn_background(
                command_str.clone(),
                description.clone(),
                &workdir,
                session_id,
                ctx.cancel_token.child_token(),
            )
            .await
            .map_err(ToolError::Execution)?;

        let output = format!(
            "Background job started.\n\
task_id: {}\n\
command: {}\n\
workdir: {}\n\n\
Poll output with bash_output (task_id=\"{}\").\n\
Kill with bash_kill (task_id=\"{}\").",
            spawned.task_id,
            command_str,
            workdir.display(),
            spawned.task_id,
            spawned.task_id
        );

        Ok(
            ToolResult::new(format!("Background: {}", description), output)
                .with_metadata("task_id", serde_json::json!(spawned.task_id))
                .with_metadata("mode", serde_json::json!(BashMode::Background.as_str()))
                .with_metadata("command", serde_json::json!(command_str))
                .with_metadata("description", serde_json::json!(description))
                .with_metadata("workdir", serde_json::json!(workdir.display().to_string())),
        )
    }

    async fn execute_interactive(
        &self,
        command_str: String,
        description: String,
        workdir: PathBuf,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if self.chunk_tx.is_none() {
            return Err(ToolError::Execution(
                "Interactive bash requires a live UI session (chunk sender unavailable)"
                    .to_string(),
            ));
        }

        let registry = self.resolve_registry(ctx).cloned();
        let job_id = if let Some(ref registry) = registry {
            Some(
                registry
                    .register_interactive(command_str.clone(), description.clone(), &workdir)
                    .await,
            )
        } else {
            None
        };

        // Reuse the same PTY + UI dialog path as terminal_session.
        // Pass job_id so terminal_session does not double-register.
        let term = TerminalSessionTool::new().with_sender_opt(self.chunk_tx.clone());
        let mut params = serde_json::json!({
            "command": command_str,
            "workdir": workdir.display().to_string(),
            "description": description,
        });
        if let Some(ref id) = job_id {
            params["job_id"] = serde_json::json!(id);
        }

        let mut result = match term.execute(params, ctx).await {
            Ok(mut result) => {
                if let (Some(registry), Some(job_id)) = (registry.as_ref(), job_id.as_ref()) {
                    let stopped = result
                        .metadata
                        .get("stopped_by_user")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let exit_code = result
                        .metadata
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32);
                    let status = if stopped {
                        JobStatus::Killed
                    } else {
                        JobStatus::Exited
                    };
                    registry
                        .mark_interactive_status(job_id, status, exit_code)
                        .await;
                    result = result.with_metadata("task_id", serde_json::json!(job_id));
                }
                result.with_metadata("mode", serde_json::json!(BashMode::Interactive.as_str()))
            }
            Err(err) => {
                if let (Some(registry), Some(job_id)) = (registry.as_ref(), job_id.as_ref()) {
                    registry
                        .mark_interactive_status(job_id, JobStatus::Failed, None)
                        .await;
                }
                return Err(err);
            }
        };

        result.title = result
            .title
            .replacen("Terminal session:", "Interactive:", 1);
        Ok(result)
    }
}

#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        unsafe {
            let _ = libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
}

struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    fn kill(&mut self) {
        kill_process_group(self.pid.take());
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(unix)]
async fn terminate_child(child: &mut tokio::process::Child) {
    kill_process_group(child.id());
    let _ = child.kill().await;
}

#[cfg(not(unix))]
fn kill_process_group(_pid: Option<u32>) {}

#[cfg(not(unix))]
async fn terminate_child(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

async fn drain_reader(mut reader: impl tokio::io::AsyncRead + Unpin) {
    let mut buffer = vec![0u8; READ_CHUNK_SIZE];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

fn append_capped(buffer: &mut Vec<u8>, chunk: &[u8], truncated: &mut bool) {
    if *truncated {
        return;
    }
    let remaining = MAX_OUTPUT_BYTES.saturating_sub(buffer.len());
    if remaining == 0 {
        *truncated = true;
        return;
    }
    let take = chunk.len().min(remaining);
    buffer.extend_from_slice(&chunk[..take]);
    if take < chunk.len() {
        *truncated = true;
    }
}

#[async_trait]
impl ToolHandler for BashTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "bash".to_string(),
            description: "Run shell commands with hybrid modes:\n\
- mode=\"foreground\" (default): short non-interactive commands with timeout; stdin is closed (EOF on prompts).\n\
- mode=\"background\": long-running servers/watchers (e.g. bun dev, cargo watch). NEVER use interactive for these. Returns a task_id immediately; manage with bash_output / bash_kill / bash_restart. Background jobs survive crabcode quit — humans can inspect them with `crabcode jobs list|logs|stop|restart`.\n\
- mode=\"interactive\": only when the user must type (npx prompts, ssh, password entry, pagers). Opens an embedded TTY dialog (Esc minimizes / parks the session; ctrl+] stops).\n\
Prefer bash over the legacy terminal_session alias. Use bash_output/bash_kill/bash_restart to manage background jobs. Open the jobs list via WhichKey `j` (ctrl+x then j), ctrl+p \"Background Jobs\", or the bottom-right jobs chip.\n\
For mode=background, pass description as a short 2–4 word name (e.g. \"Dev server\")."
                .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "command".to_string(),
                    description: "Command to execute".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "mode".to_string(),
                    description: "Run mode: foreground (default) | background | interactive"
                        .to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "timeout".to_string(),
                    description: "Timeout in seconds for foreground mode (default: 120)"
                        .to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
                ParameterSchema {
                    name: "workdir".to_string(),
                    description: "Working directory for the command".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "description".to_string(),
                    description: "Short 2–4 word job name for Jobs UI / `crabcode jobs list` (e.g. \"Dev server\"). Especially useful for mode=background.".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["command"])?;
        if let Some(mode) = get_string_param(params, "mode") {
            BashMode::parse(&mode)?;
        }
        Ok(())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command_str = get_string_param(&params, "command")
            .ok_or_else(|| ToolError::Validation("command is required".to_string()))?;

        let mode = BashMode::parse(
            &get_string_param(&params, "mode").unwrap_or_else(|| "foreground".to_string()),
        )?;

        let timeout_seconds = get_integer_param(&params, "timeout")
            .map(|v| {
                if v <= 0 {
                    DEFAULT_TIMEOUT_SECONDS
                } else {
                    v as u64
                }
            })
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);

        let workdir_param =
            get_string_param(&params, "path").or_else(|| get_string_param(&params, "workdir"));

        let description =
            get_string_param(&params, "description").unwrap_or_else(|| command_str.clone());

        let workdir = workdir_param
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.workdir().to_path_buf());

        match mode {
            BashMode::Foreground => {
                self.execute_foreground(
                    command_str,
                    description,
                    Some(workdir.display().to_string()),
                    timeout_seconds,
                    ctx,
                )
                .await
            }
            BashMode::Background => {
                self.execute_background(command_str, description, workdir, ctx)
                    .await
            }
            BashMode::Interactive => {
                self.execute_interactive(command_str, description, workdir, ctx)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn append_capped_respects_byte_limit() {
        let mut buf = Vec::new();
        let mut truncated = false;
        append_capped(&mut buf, &[b'a'; MAX_OUTPUT_BYTES + 10], &mut truncated);
        assert_eq!(buf.len(), MAX_OUTPUT_BYTES);
        assert!(truncated);
    }

    #[test]
    fn mode_validation_rejects_unknown() {
        let tool = BashTool::new();
        let err = tool
            .validate(&serde_json::json!({
                "command": "echo hi",
                "mode": "wat"
            }))
            .expect_err("unknown mode should fail");
        assert!(err.to_string().contains("Unknown bash mode"));
    }

    #[test]
    fn mode_validation_accepts_known() {
        let tool = BashTool::new();
        for mode in ["foreground", "background", "interactive", "BACKGROUND"] {
            tool.validate(&serde_json::json!({
                "command": "echo hi",
                "mode": mode
            }))
            .unwrap_or_else(|_| panic!("mode {mode} should be valid"));
        }
    }

    #[tokio::test]
    async fn interactive_read_receives_eof_instead_of_hanging() {
        let ctx =
            ToolContext::from_cancel_token("session", "message", "Build", CancellationToken::new());
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            BashTool::new().execute(
                serde_json::json!({
                    "command": "if IFS= read -r value; then echo unexpected; else echo eof; fi"
                }),
                &ctx,
            ),
        )
        .await
        .expect("non-interactive bash should not wait for terminal input")
        .expect("bash command should execute");

        assert!(result.output.contains("eof"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_command_kills_background_processes() {
        let temp_dir = tempfile::tempdir().expect("temp directory should be created");
        let pid_file = temp_dir.path().join("background.pid");
        let command = format!(
            "sleep 30 & echo $! > {}",
            pid_file.to_string_lossy().replace(' ', "\\ ")
        );
        let ctx =
            ToolContext::from_cancel_token("session", "message", "Build", CancellationToken::new());

        tokio::time::timeout(
            Duration::from_secs(3),
            BashTool::new().execute(
                serde_json::json!({
                    "command": command,
                    "timeout": 2
                }),
                &ctx,
            ),
        )
        .await
        .expect("bash tool should not wait for background process pipes")
        .expect("foreground command should succeed");

        let pid: i32 = std::fs::read_to_string(pid_file)
            .expect("background pid should be written")
            .trim()
            .parse()
            .expect("background pid should be numeric");
        let mut still_running = true;
        for _ in 0..20 {
            still_running = unsafe { libc::kill(pid, 0) == 0 };
            if !still_running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!still_running, "background process {pid} was not killed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborted_tool_future_kills_node_descendants() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let temp_dir = tempfile::tempdir().expect("temp directory should be created");
        let pid_file = temp_dir.path().join("node.pid");
        let escaped_pid_file = pid_file.to_string_lossy().replace('\'', "'\\''");
        let command = format!(
            "node -e 'setInterval(() => {{}}, 1000)' & echo $! > '{escaped_pid_file}'; wait"
        );

        let handle = tokio::spawn(async move {
            let ctx = ToolContext::from_cancel_token(
                "session",
                "message",
                "Build",
                CancellationToken::new(),
            );
            BashTool::new()
                .execute(
                    serde_json::json!({
                        "command": command,
                        "timeout": 30
                    }),
                    &ctx,
                )
                .await
        });

        let pid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = contents.trim().parse::<i32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("node pid should be written");

        handle.abort();
        let _ = handle.await;

        let mut still_running = true;
        for _ in 0..40 {
            still_running = unsafe { libc::kill(pid, 0) == 0 };
            if !still_running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!still_running, "node descendant {pid} survived tool abort");
    }
}
