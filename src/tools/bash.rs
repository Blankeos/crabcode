use crate::tools::{
    get_integer_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::process::Stdio;
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

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
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
            description: "Execute non-interactive shell commands with a timeout and captured output. Stdin is closed, so commands that prompt for input will receive EOF; use `terminal_session` when a TTY or user interaction is required."
                .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "command".to_string(),
                    description: "Command to execute".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "timeout".to_string(),
                    description: "Timeout in seconds (default: 120)".to_string(),
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
                    description: "Human-readable description of what the command does".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["command"])
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command_str = get_string_param(&params, "command")
            .ok_or_else(|| ToolError::Validation("command is required".to_string()))?;

        let timeout_seconds = get_integer_param(&params, "timeout")
            .map(|v| {
                if v <= 0 {
                    DEFAULT_TIMEOUT_SECONDS
                } else {
                    v as u64
                }
            })
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);

        let workdir =
            get_string_param(&params, "path").or_else(|| get_string_param(&params, "workdir"));

        let description =
            get_string_param(&params, "description").unwrap_or_else(|| command_str.clone());

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&command_str);

        if let Some(dir) = workdir {
            cmd.current_dir(dir);
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
                                kill_process_group(process_group_id);
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
                                kill_process_group(process_group_id);
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
                .with_metadata("command", serde_json::json!(command_str)),
        )
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
}
