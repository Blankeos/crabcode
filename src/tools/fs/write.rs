use crate::tools::{
    get_string_param, validate_required, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
    ParameterSchema, ParameterType,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const BLOCKED_FILES: [&str; 3] = [".env", ".env.local", ".env.production"];

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }

    fn is_blocked(path: &Path) -> bool {
        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
            BLOCKED_FILES.contains(&file_name)
        } else {
            false
        }
    }
}

#[async_trait]
impl ToolHandler for WriteTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "write".to_string(),
            description: "Create or overwrite a file. Creates parent directories if needed.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "file_path".to_string(),
                    description: "Path to the file to write".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "content".to_string(),
                    description: "Content to write to the file".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["file_path", "content"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let file_path = get_string_param(&params, "file_path")
            .ok_or_else(|| ToolError::Validation("file_path is required".to_string()))?;

        let content = get_string_param(&params, "content")
            .ok_or_else(|| ToolError::Validation("content is required".to_string()))?;

        let path = Path::new(&file_path);

        if Self::is_blocked(path) {
            return Err(ToolError::Permission(format!(
                "Writing to {} is blocked for security reasons",
                file_path
            )));
        }

        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| ToolError::Execution(format!("Failed to create directories: {}", e)))?;
            }
        }

        let temp_path = path.with_extension("tmp");
        
        std::fs::write(&temp_path, content)
            .map_err(|e| ToolError::Execution(format!("Failed to write temp file: {}", e)))?;

        std::fs::rename(&temp_path, path)
            .map_err(|e| ToolError::Execution(format!("Failed to rename file: {}", e)))?;

        let is_new = !path.exists();
        
        Ok(ToolResult::new(
            format!("Write: {}", file_path),
            if is_new {
                format!("Created file with {} bytes", std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
            } else {
                format!("Updated file with {} bytes", std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
            }
        ))
    }
}
