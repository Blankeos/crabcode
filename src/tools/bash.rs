use crate::tools::{
    get_bool_param, get_integer_param, get_string_param, validate_required, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult, ParameterSchema, ParameterType,
};
use async_trait::async_trait;
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_OUTPUT_SIZE: usize = 51200; // 50KB

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }

    fn is_dangerous(command: &str) -> Option<String> {
        let dangerous_patterns = [
            "rm -rf /",
            "rm -rf /*",
            ":(){ :|: & };:",
            "> /dev/sda",
            "mkfs",
            "dd if=/dev/zero",
            "chmod -R 777 /",
        ];

        for pattern in &dangerous_patterns {
            if command.contains(pattern) {
                return Some(format!("Command contains dangerous pattern: {}", pattern));
            }
        }

        None
    }
}

#[async_trait]
impl ToolHandler for BashTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "bash".to_string(),
            description: "Execute shell commands with timeout and output streaming.".to_string(),
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
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["command"])
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command_str = get_string_param(&params, "command")
            .ok_or_else(|| ToolError::Validation("command is required".to_string()))?;

        let timeout_seconds = get_integer_param(&params, "timeout")
            .map(|v| if v <= 0 { DEFAULT_TIMEOUT_SECONDS } else { v as u64 })
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);

        let workdir = get_string_param(&params, "path")
            .or_else(|| get_string_param(&params, "workdir"));

        let description = get_string_param(&params, "description")
            .unwrap_or_else(|| command_str.clone());

        if let Some(reason) = Self::is_dangerous(&command_str) {
            return Err(ToolError::Permission(reason));
        }

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&command_str);

        if let Some(dir) = workdir {
            cmd.current_dir(dir);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Execution(format!("Failed to spawn process: {}", e)))?;

        let stdout = child.stdout.take().expect("stdout should be piped");
        let stderr = child.stderr.take().expect("stderr should be piped");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_lines: Vec<String> = Vec::new();
        let mut stderr_lines: Vec<String> = Vec::new();

        let timeout_duration = Duration::from_secs(timeout_seconds);

        let result = timeout(timeout_duration, async {
            loop {
                if ctx.is_aborted() {
                    let _ = child.kill().await;
                    return Err(ToolError::Execution("Command aborted".to_string()));
                }

                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(l)) => {
                                if stdout_lines.len() < MAX_OUTPUT_SIZE {
                                    stdout_lines.push(l);
                                }
                            }
                            Ok(None) => {}
                            Err(e) => return Err(ToolError::Execution(format!("Error reading stdout: {}", e))),
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(l)) => {
                                if stderr_lines.len() < MAX_OUTPUT_SIZE {
                                    stderr_lines.push(l);
                                }
                            }
                            Ok(None) => {}
                            Err(e) => return Err(ToolError::Execution(format!("Error reading stderr: {}", e))),
                        }
                    }
                    status = child.wait() => {
                        return match status {
                            Ok(exit_status) => Ok(exit_status),
                            Err(e) => Err(ToolError::Execution(format!("Process error: {}", e))),
                        };
                    }
                }
            }
        }).await;

        let exit_status = match result {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                let _ = child.kill().await;
                return Err(ToolError::Execution(format!(
                    "Command timed out after {} seconds",
                    timeout_seconds
                )));
            }
        };

        let mut output_parts = Vec::new();

        if !stdout_lines.is_empty() {
            output_parts.push(stdout_lines.join("\n"));
        }

        if !stderr_lines.is_empty() {
            if !output_parts.is_empty() {
                output_parts.push("\n--- stderr ---".to_string());
            }
            output_parts.push(stderr_lines.join("\n"));
        }

        let output = if output_parts.is_empty() {
            "(no output)".to_string()
        } else {
            output_parts.join("\n")
        };

        let truncated = stdout_lines.len() >= MAX_OUTPUT_SIZE || stderr_lines.len() >= MAX_OUTPUT_SIZE;
        let final_output = if truncated {
            format!("{}\n\n[Output truncated to {} bytes]", output, MAX_OUTPUT_SIZE)
        } else {
            output
        };

        let exit_code = exit_status.code().unwrap_or(-1);

        Ok(ToolResult::new(
            format!("Bash: {}", description),
            final_output
        )
        .with_metadata("exit_code", serde_json::json!(exit_code))
        .with_metadata("command", serde_json::json!(command_str)))
    }
}
