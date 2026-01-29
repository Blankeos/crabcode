use async_trait::async_trait;
use serde_json::Value;

pub mod bash;
pub mod context;
pub mod edit;
pub mod fs;
pub mod init;
pub mod registry;
pub mod types;

pub use bash::BashTool;
pub use context::ToolContext;
pub use edit::EditTool;
pub use init::initialize_tool_registry;
pub use registry::ToolRegistry;
pub use types::{ParameterSchema, ParameterType, Tool, ToolError, ToolId, ToolResult};

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn definition(&self) -> Tool;
    fn validate(&self, params: &Value) -> Result<(), ToolError>;
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError>;
}

pub fn validate_required(params: &Value, required: &[&str]) -> Result<(), ToolError> {
    let obj = params
        .as_object()
        .ok_or_else(|| ToolError::Validation("Parameters must be an object".to_string()))?;

    for field in required {
        if !obj.contains_key(*field) {
            return Err(ToolError::Validation(format!(
                "Missing required parameter: {}",
                field
            )));
        }
    }

    Ok(())
}

pub fn get_string_param(params: &Value, name: &str) -> Option<String> {
    params
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn get_integer_param(params: &Value, name: &str) -> Option<i64> {
    params.get(name).and_then(|v| v.as_i64())
}

pub fn get_bool_param(params: &Value, name: &str, default: bool) -> bool {
    params
        .get(name)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}
