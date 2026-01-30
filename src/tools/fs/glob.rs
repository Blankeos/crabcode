use crate::tools::{
    get_string_param, validate_required, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
    ParameterSchema, ParameterType,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for GlobTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "glob".to_string(),
            description: "Find files by glob pattern. Returns file paths sorted by modification time.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "pattern".to_string(),
                    description: "Glob pattern to match files (e.g., '**/*.rs', '*.md')".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "path".to_string(),
                    description: "Base directory to search from (default: current working directory)".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["pattern"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern = get_string_param(&params, "pattern")
            .ok_or_else(|| ToolError::Validation("pattern is required".to_string()))?;

        let base_path = get_string_param(&params, "path")
            .unwrap_or_else(|| ".".to_string());

        let pattern_path = Path::new(&base_path).join(&pattern);
        let pattern_str = pattern_path
            .to_str()
            .ok_or_else(|| ToolError::Execution("Invalid path encoding".to_string()))?;

        let mut entries: Vec<(glob::Paths, String)> = Vec::new();
        
        match glob::glob(pattern_str) {
            Ok(paths) => {
                let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
                
                for entry in paths {
                    match entry {
                        Ok(path) => {
                            if let Ok(metadata) = std::fs::metadata(&path) {
                                if let Ok(modified) = metadata.modified() {
                                    files.push((path, modified));
                                } else {
                                    files.push((path, std::time::SystemTime::UNIX_EPOCH));
                                }
                            }
                        }
                        Err(e) => {
                            return Err(ToolError::Execution(format!("Glob error: {}", e)));
                        }
                    }
                }

                files.sort_by(|a, b| b.1.cmp(&a.1));

                let limit = 100;
                let total = files.len();
                let truncated = total > limit;
                
                let output: Vec<String> = files
                    .into_iter()
                    .take(limit)
                    .map(|(path, _)| path.display().to_string())
                    .collect();

                let result_text = if output.is_empty() {
                    "No files found matching pattern.".to_string()
                } else {
                    let mut text = output.join("\n");
                    if truncated {
                        text.push_str(&format!("\n\n... and {} more files (showing first {})", total - limit, limit));
                    }
                    text
                };

                Ok(ToolResult::new(format!("Glob: {}", pattern), result_text)
                    .with_metadata("match_count", serde_json::Value::Number((total as i64).into()))
                    .with_metadata("shown_count", serde_json::Value::Number(((total.min(limit)) as i64).into()))
                    .with_metadata("limit", serde_json::Value::Number((limit as i64).into()))
                    .with_metadata("truncated", serde_json::Value::Bool(truncated)))
            }
            Err(e) => Err(ToolError::Execution(format!("Invalid glob pattern: {}", e))),
        }
    }
}
