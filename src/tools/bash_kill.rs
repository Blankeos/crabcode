use crate::tools::process_registry::ProcessRegistry;
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct BashKillTool {
    registry: Arc<ProcessRegistry>,
}

impl BashKillTool {
    pub fn new(registry: Arc<ProcessRegistry>) -> Self {
        Self { registry }
    }

    pub fn with_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.registry = registry;
        self
    }
}

#[async_trait]
impl ToolHandler for BashKillTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "bash_kill".to_string(),
            description: "Kill a background bash job previously started with bash \
mode=\"background\". Prefer this over killing via shell when you have a task_id. \
Background jobs survive crabcode quit — humans can also stop them with `crabcode jobs stop <id>`."
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

        self.registry
            .kill(&task_id)
            .await
            .map_err(ToolError::Execution)?;

        Ok(ToolResult::new(
            format!("Killed: {task_id}"),
            format!("Killed task {task_id}"),
        )
        .with_metadata("task_id", serde_json::json!(task_id))
        .with_metadata("status", serde_json::json!("killed")))
    }
}
