use crate::tools::{
    get_integer_param, get_string_param, validate_required, Tool, ToolContext, ToolError,
    ToolHandler, ToolResult, ParameterSchema, ParameterType,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024; // 50MB
const BINARY_CHECK_SIZE: usize = 8192; // 8KB
const DEFAULT_LIMIT: usize = 2000;

pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }

    fn is_binary(data: &[u8]) -> bool {
        data.iter().take(BINARY_CHECK_SIZE).any(|b| *b == 0)
    }
}

#[async_trait]
impl ToolHandler for ReadTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "read".to_string(),
            description: "Read file contents with line numbers and pagination. Detects binary files automatically.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "file_path".to_string(),
                    description: "Path to the file to read".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "offset".to_string(),
                    description: "Line offset to start from (0-based, default: 0)".to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
                ParameterSchema {
                    name: "limit".to_string(),
                    description: "Maximum number of lines to read (default: 2000)".to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["file_path"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let file_path = get_string_param(&params, "file_path")
            .ok_or_else(|| ToolError::Validation("file_path is required".to_string()))?;

        let offset = get_integer_param(&params, "offset")
            .map(|v| v.max(0) as usize)
            .unwrap_or(0);

        let limit = get_integer_param(&params, "limit")
            .map(|v| if v <= 0 { DEFAULT_LIMIT } else { v as usize })
            .unwrap_or(DEFAULT_LIMIT);

        let path = Path::new(&file_path);

        if !path.exists() {
            return Err(ToolError::NotFound(format!("File not found: {}", file_path)));
        }

        if !path.is_file() {
            return Err(ToolError::Validation(format!("Path is not a file: {}", file_path)));
        }

        let metadata = std::fs::metadata(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read file metadata: {}", e)))?;

        let file_size = metadata.len();

        if file_size > MAX_FILE_SIZE {
            return Err(ToolError::Execution(format!(
                "File is too large ({}MB > {}MB limit)",
                file_size / (1024 * 1024),
                MAX_FILE_SIZE / (1024 * 1024)
            )));
        }

        let content = std::fs::read(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read file: {}", e)))?;

        if Self::is_binary(&content) {
            return Ok(ToolResult::new(
                format!("Read: {}", file_path),
                "[Binary file - contents not displayed]".to_string()
            ));
        }

        let text = String::from_utf8_lossy(&content);
        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();

        if offset >= total_lines {
            return Ok(ToolResult::new(
                format!("Read: {}", file_path),
                format!("[File has {} lines, offset {} is beyond end]", total_lines, offset)
            ));
        }

        let end = (offset + limit).min(total_lines);
        let selected_lines = &lines[offset..end];

        let numbered_lines: Vec<String> = selected_lines
            .iter()
            .enumerate()
            .map(|(idx, line)| format!("{:05}| {}", offset + idx + 1, line))
            .collect();

        let mut output = numbered_lines.join("\n");

        if end < total_lines {
            output.push_str(&format!("\n\n... {} more lines (showing {}-{} of {})", 
                total_lines - end, offset + 1, end, total_lines));
        }

        Ok(ToolResult::new(
            format!("Read: {}", file_path),
            output
        ))
    }
}
