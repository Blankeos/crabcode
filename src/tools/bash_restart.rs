use crate::tools::process_registry::ProcessRegistry;
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct BashRestartTool {
    registry: Arc<ProcessRegistry>,
}

impl BashRestartTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self { registry }
    }

    pub fn with_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.registry = registry;
        self
    }
}

#[async_trait]
impl ToolHandler for BashRestartTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "bash_restart".to_string(),
            description: "Restart a background job by id (same command/cwd). Use when a server \
died or needs reload. Reuses the same task_id and appends a restart marker to the log. Prefer \
this over killing + re-running when you have a task_id."
                .to_string(),
            parameters: vec![ParameterSchema {
                name: "task_id".to_string(),
                description: "Task id returned by bash in background mode (e.g. job_01HXYZ…)"
                    .to_string(),
                required: true,
                param_type: ParameterType::String,
            }],
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

        let meta = self
            .registry
            .restart(&task_id)
            .await
            .map_err(ToolError::Execution)?;

        Ok(ToolResult::new(
            format!(
                "Restarted: {} (pid={}, name={})",
                meta.id, meta.pid, meta.name
            ),
            format!("Restarted task {}", meta.id),
        )
        .with_metadata("task_id", serde_json::json!(meta.id))
        .with_metadata("pid", serde_json::json!(meta.pid))
        .with_metadata("name", serde_json::json!(meta.name))
        .with_metadata("status", serde_json::json!(meta.status.as_str())))
    }
}
