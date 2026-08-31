use crate::tools::process_registry::ProcessRegistry;
use crate::tools::{
    get_bool_param, get_integer_param, get_string_param, validate_required, ParameterSchema,
    ParameterType, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct BashOutputTool {
    registry: Arc<ProcessRegistry>,
}

impl BashOutputTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self { registry }
    }

    pub fn with_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.registry = registry;
        self
    }
}

#[async_trait]
impl ToolHandler for BashOutputTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "bash_output".to_string(),
            description: "Read output from a background/interactive bash job started with \
bash mode=\"background\". By default returns bytes since the last successful read \
(Grok-style since-last). Pass offset=0 to re-read from the start of the retained log. \
Optionally wait for new output or process exit via wait/timeout. Jobs survive crabcode \
quit; humans can also use `crabcode jobs logs <id>`."
                .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "task_id".to_string(),
                    description: "Task id returned by bash in background mode (e.g. job_01HXYZ…)"
                        .to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "wait".to_string(),
                    description: "If true, wait up to ~30s for new output or process exit \
(ignored if timeout is set)"
                        .to_string(),
                    required: false,
                    param_type: ParameterType::Boolean,
                },
                ParameterSchema {
                    name: "timeout".to_string(),
                    description: "Milliseconds to wait for new output or process exit \
before returning current state"
                        .to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
                ParameterSchema {
                    name: "offset".to_string(),
                    description: "Absolute byte offset into the job's logical output stream. \
Omit for since-last semantics; pass 0 to read from the start of the retained buffer."
                        .to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["task_id"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        self.validate(&params)?;
        let task_id = get_string_param(&params, "task_id")
            .ok_or_else(|| ToolError::Validation("Missing required parameter: task_id".into()))?;

        let wait_ms = if let Some(timeout) = get_integer_param(&params, "timeout") {
            if timeout < 0 {
                return Err(ToolError::Validation(
                    "timeout must be a non-negative integer (milliseconds)".into(),
                ));
            }
            Some(timeout as u64)
        } else if get_bool_param(&params, "wait", false) {
            Some(30_000)
        } else {
            None
        };

        let since_byte =
            get_integer_param(&params, "offset").map(|v| if v < 0 { 0u64 } else { v as u64 });

        let out = self
            .registry
            .output(&task_id, wait_ms, since_byte)
            .await
            .map_err(ToolError::Execution)?;

        let text = if out.text.is_empty() {
            if out.status.is_terminal() {
                "(no new output; process finished)".to_string()
            } else {
                "(no new output yet)".to_string()
            }
        } else {
            out.text
        };

        Ok(ToolResult::new(format!("Output: {task_id}"), text)
            .with_metadata("task_id", serde_json::json!(task_id))
            .with_metadata("status", serde_json::json!(out.status.as_str()))
            .with_metadata("exit_code", serde_json::json!(out.exit_code))
            .with_metadata("bytes_total", serde_json::json!(out.bytes_total))
            .with_metadata("truncated", serde_json::json!(out.truncated))
            .with_metadata("next_offset", serde_json::json!(out.next_offset)))
    }
}
