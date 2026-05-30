use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub struct WriteTool;
pub struct WriteFilesTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl WriteFilesTool {
    pub fn new() -> Self {
        Self
    }
}

fn write_one_file(file_path: &str, content: &str) -> Result<(bool, u64), ToolError> {
    let path = Path::new(file_path);
    let is_new = !path.exists();

    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::Execution(format!("Failed to create directories: {}", e))
            })?;
        }
    }

    let temp_path = path.with_extension("tmp");

    std::fs::write(&temp_path, content)
        .map_err(|e| ToolError::Execution(format!("Failed to write temp file: {}", e)))?;

    std::fs::rename(&temp_path, path)
        .map_err(|e| ToolError::Execution(format!("Failed to rename file: {}", e)))?;

    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok((is_new, bytes))
}

#[async_trait]
impl ToolHandler for WriteTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "write".to_string(),
            description: "Create or overwrite a file. Creates parent directories if needed."
                .to_string(),
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

        let (is_new, bytes) = write_one_file(&file_path, &content)?;

        Ok(ToolResult::new(
            format!("Write: {}", file_path),
            if is_new {
                format!("Created file with {} bytes", bytes)
            } else {
                format!("Updated file with {} bytes", bytes)
            },
        ))
    }
}

#[async_trait]
impl ToolHandler for WriteFilesTool {
    fn definition(&self) -> Tool {
        let mut file_props = HashMap::new();
        file_props.insert("file_path".to_string(), ParameterType::String);
        file_props.insert("content".to_string(), ParameterType::String);

        Tool {
            id: "write_files".to_string(),
            description: "Create or overwrite multiple files in one call. Prefer this over repeated write calls when replacing complete contents of 2 or more files.".to_string(),
            parameters: vec![ParameterSchema {
                name: "files".to_string(),
                description: "Array of files to write, each with file_path and content.".to_string(),
                required: true,
                param_type: ParameterType::Array(Box::new(ParameterType::Object(file_props))),
            }],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["files"])?;
        let files = params
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::Validation("files must be an array".to_string()))?;

        if files.is_empty() {
            return Err(ToolError::Validation(
                "files must include at least one file".to_string(),
            ));
        }

        for (index, file) in files.iter().enumerate() {
            let Some(obj) = file.as_object() else {
                return Err(ToolError::Validation(format!(
                    "files[{index}] must be an object"
                )));
            };
            if !obj.get("file_path").is_some_and(Value::is_string) {
                return Err(ToolError::Validation(format!(
                    "files[{index}].file_path is required"
                )));
            }
            if !obj.get("content").is_some_and(Value::is_string) {
                return Err(ToolError::Validation(format!(
                    "files[{index}].content is required"
                )));
            }
        }

        Ok(())
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let files = params
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::Validation("files must be an array".to_string()))?;

        let mut summaries = Vec::with_capacity(files.len());
        for file in files {
            let file_path = file
                .get("file_path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Validation("file_path is required".to_string()))?;
            let content = file
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Validation("content is required".to_string()))?;
            let (is_new, bytes) = write_one_file(file_path, content)?;
            let action = if is_new { "created" } else { "updated" };
            summaries.push(format!("{file_path}: {action} {bytes} bytes"));
        }

        Ok(ToolResult::new(
            format!("Write files: {}", summaries.len()),
            summaries.join("\n"),
        )
        .with_metadata("file_count", serde_json::json!(summaries.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolHandler;

    fn test_context() -> ToolContext {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        ToolContext::new("session", "message", "build", rx)
    }

    #[tokio::test]
    async fn write_files_creates_and_updates_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.txt");
        let second = dir.path().join("nested/b.txt");
        std::fs::write(&first, "old").unwrap();

        let result = WriteFilesTool::new()
            .execute(
                serde_json::json!({
                    "files": [
                        { "file_path": first.to_string_lossy(), "content": "new" },
                        { "file_path": second.to_string_lossy(), "content": "created" }
                    ]
                }),
                &test_context(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(first).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(second).unwrap(), "created");
        assert!(result.output.contains("updated 3 bytes"));
        assert!(result.output.contains("created 7 bytes"));
        assert_eq!(result.metadata["file_count"], serde_json::json!(2));
    }
}
