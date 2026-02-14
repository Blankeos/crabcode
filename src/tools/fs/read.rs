use crate::tools::{
    get_integer_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
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

    fn file_path_param(params: &Value) -> Option<String> {
        get_string_param(params, "file_path").or_else(|| get_string_param(params, "filePath"))
    }

    fn is_binary(data: &[u8]) -> bool {
        data.iter().take(BINARY_CHECK_SIZE).any(|b| *b == 0)
    }

    fn read_directory(path: &Path, offset: usize, limit: usize) -> Result<String, ToolError> {
        let mut entries: Vec<String> = std::fs::read_dir(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                let with_marker = if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    format!("{}/", name)
                } else {
                    name
                };
                Some(with_marker)
            })
            .collect();

        entries.sort();

        if offset >= entries.len() {
            return Ok(format!(
                "<path>{}</path>\n<type>directory</type>\n<entries>\n\n({} entries)\n</entries>",
                path.display(),
                entries.len()
            ));
        }

        let end = (offset + limit).min(entries.len());
        let selected = &entries[offset..end];
        let truncated = end < entries.len();

        let mut output = String::new();
        output.push_str(&format!("<path>{}</path>\n", path.display()));
        output.push_str("<type>directory</type>\n");
        output.push_str("<entries>\n");
        output.push_str(&selected.join("\n"));

        if truncated {
            output.push_str(&format!(
                "\n\n(Showing {} of {} entries. Use offset {} to continue)\n",
                selected.len(),
                entries.len(),
                end
            ));
        } else {
            output.push_str(&format!("\n\n({} entries)\n", entries.len()));
        }

        output.push_str("</entries>");
        Ok(output)
    }
}

#[async_trait]
impl ToolHandler for ReadTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "read".to_string(),
            description: "Read file or directory contents with pagination. Detects binary files automatically."
                .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "file_path".to_string(),
                    description: "Path to the file or directory to read".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "filePath".to_string(),
                    description: "Alias of file_path for compatibility".to_string(),
                    required: false,
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
        let has_snake_case = get_string_param(params, "file_path").is_some();
        let has_camel_case = get_string_param(params, "filePath").is_some();

        if has_snake_case || has_camel_case {
            Ok(())
        } else {
            validate_required(params, &["file_path"])
        }
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let file_path = Self::file_path_param(&params)
            .ok_or_else(|| ToolError::Validation("file_path is required".to_string()))?;

        let offset = get_integer_param(&params, "offset")
            .map(|v| v.max(0) as usize)
            .unwrap_or(0);

        let limit = get_integer_param(&params, "limit")
            .map(|v| if v <= 0 { DEFAULT_LIMIT } else { v as usize })
            .unwrap_or(DEFAULT_LIMIT);

        let path = Path::new(&file_path);

        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "File not found: {}",
                file_path
            )));
        }

        if path.is_dir() {
            let output = Self::read_directory(path, offset, limit)?;
            return Ok(ToolResult::new(format!("Read: {}", file_path), output));
        }

        if !path.is_file() {
            return Err(ToolError::Validation(format!(
                "Path is not readable: {}",
                file_path
            )));
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
                "[Binary file - contents not displayed]".to_string(),
            ));
        }

        let text = String::from_utf8_lossy(&content);
        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();

        if offset >= total_lines {
            return Ok(ToolResult::new(
                format!("Read: {}", file_path),
                format!(
                    "[File has {} lines, offset {} is beyond end]",
                    total_lines, offset
                ),
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
            output.push_str(&format!(
                "\n\n... {} more lines (showing {}-{} of {})",
                total_lines - end,
                offset + 1,
                end,
                total_lines
            ));
        }

        Ok(ToolResult::new(format!("Read: {}", file_path), output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!("{}_{}", prefix, nanos))
    }

    #[test]
    fn read_directory_includes_hidden_and_directory_markers() {
        let dir = unique_temp_dir("crabcode_read_tool_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");

        let env_path = dir.join(".env");
        let file_path = dir.join("README.md");
        let nested_dir = dir.join("config");

        std::fs::write(&env_path, "API_KEY=test").expect(".env should be written");
        std::fs::write(&file_path, "# test").expect("README should be written");
        std::fs::create_dir_all(&nested_dir).expect("nested directory should be created");

        let output = ReadTool::read_directory(&dir, 0, 100).expect("directory read should work");

        assert!(output.contains(".env"));
        assert!(output.contains("README.md"));
        assert!(output.contains("config/"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
