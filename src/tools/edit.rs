use crate::tools::{
    get_bool_param, get_string_param, validate_required, Tool, ToolContext, ToolError,
    ToolHandler, ToolResult, ParameterSchema, ParameterType,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const SIMILARITY_THRESHOLD: f64 = 0.8;

pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }

    fn levenshtein_similarity(a: &str, b: &str) -> f64 {
        let distance = strsim::levenshtein(a, b);
        let max_len = a.len().max(b.len());
        if max_len == 0 {
            return 1.0;
        }
        1.0 - (distance as f64 / max_len as f64)
    }

    fn find_best_match<'a>(content: &str, old_string: &str) -> Option<(usize, usize)> {
        if let Some(pos) = content.find(old_string) {
            return Some((pos, pos + old_string.len()));
        }

        let old_trimmed = old_string.trim();
        if let Some(pos) = content.find(old_trimmed) {
            return Some((pos, pos + old_trimmed.len()));
        }

        let lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_string.lines().collect();

        if old_lines.len() > 1 {
            for i in 0..lines.len() {
                if i + old_lines.len() <= lines.len() {
                    let candidate: String = lines[i..i + old_lines.len()].join("\n");
                    let similarity = Self::levenshtein_similarity(&candidate, old_string);
                    
                    if similarity >= SIMILARITY_THRESHOLD {
                        let start = lines[..i].join("\n").len();
                        let start = if i > 0 { start + 1 } else { start };
                        return Some((start, start + candidate.len()));
                    }
                }
            }
        }

        None
    }
}

#[async_trait]
impl ToolHandler for EditTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "edit".to_string(),
            description: "Replace text in files with smart matching. Supports exact match, fuzzy match, and line-trimmed match.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "file_path".to_string(),
                    description: "Path to the file to edit".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "old_string".to_string(),
                    description: "Text to replace".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "new_string".to_string(),
                    description: "Replacement text".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "replace_all".to_string(),
                    description: "Replace all occurrences (default: false)".to_string(),
                    required: false,
                    param_type: ParameterType::Boolean,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["file_path", "old_string", "new_string"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let file_path = get_string_param(&params, "file_path")
            .ok_or_else(|| ToolError::Validation("file_path is required".to_string()))?;

        let old_string = get_string_param(&params, "old_string")
            .ok_or_else(|| ToolError::Validation("old_string is required".to_string()))?;

        let new_string = get_string_param(&params, "new_string")
            .ok_or_else(|| ToolError::Validation("new_string is required".to_string()))?;

        let replace_all = get_bool_param(&params, "replace_all", false);

        let path = Path::new(&file_path);

        if !path.exists() {
            return Err(ToolError::NotFound(format!("File not found: {}", file_path)));
        }

        if !path.is_file() {
            return Err(ToolError::Validation(format!("Path is not a file: {}", file_path)));
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read file: {}", e)))?;

        if replace_all {
            if !content.contains(&old_string) {
                return Err(ToolError::NotFound(format!(
                    "Text not found in file: {}",
                    old_string.chars().take(50).collect::<String>()
                )));
            }

            let new_content = content.replace(&old_string, &new_string);
            let count = content.matches(&old_string).count();

            std::fs::write(path, new_content)
                .map_err(|e| ToolError::Execution(format!("Failed to write file: {}", e)))?;

            return Ok(ToolResult::new(
                format!("Edit: {}", file_path),
                format!("Replaced {} occurrence(s)", count)
            ));
        }

        match Self::find_best_match(&content, &old_string) {
            Some((start, end)) => {
                let mut new_content = String::with_capacity(content.len() - (end - start) + new_string.len());
                new_content.push_str(&content[..start]);
                new_content.push_str(&new_string);
                new_content.push_str(&content[end..]);

                std::fs::write(path, new_content)
                    .map_err(|e| ToolError::Execution(format!("Failed to write file: {}", e)))?;

                let line_num = content[..start].chars().filter(|c| *c == '\n').count() + 1;

                Ok(ToolResult::new(
                    format!("Edit: {}", file_path),
                    format!("Replaced at line {}", line_num)
                ))
            }
            None => Err(ToolError::NotFound(format!(
                "Could not find text to replace: {}",
                old_string.chars().take(50).collect::<String>()
            ))),
        }
    }
}
