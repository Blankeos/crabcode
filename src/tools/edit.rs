use crate::tools::mutation::FileMutation;
use crate::tools::{
    get_bool_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

fn count_occurrences(content: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }

    let mut count = 0;
    let mut offset = 0;
    while let Some(relative) = content[offset..].find(needle) {
        count += 1;
        offset += relative + needle.len();
    }
    count
}

fn find_exact_occurrence(content: &str, needle: &str) -> Option<usize> {
    content.find(needle)
}

#[async_trait]
impl ToolHandler for EditTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "edit".to_string(),
            description: "Replace exact text in a file. Fails if the old text is missing, or if it appears multiple times unless replace_all is true.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "file_path".to_string(),
                    description: "Path to the file to edit".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "old_string".to_string(),
                    description: "Exact text to replace, including whitespace and indentation".to_string(),
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
                    description: "Replace all exact occurrences (default: false)".to_string(),
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

        FileMutation::with_lock_path(path, |locked| {
            if !locked.exists() {
                return Err(ToolError::NotFound(format!(
                    "File not found: {}",
                    file_path
                )));
            }

            if !locked.is_file() {
                return Err(ToolError::Validation(format!(
                    "Path is not a file: {}",
                    file_path
                )));
            }

            if old_string.is_empty() {
                return Err(ToolError::Validation(
                    "old_string must not be empty".to_string(),
                ));
            }

            if old_string == new_string {
                return Err(ToolError::Validation(
                    "new_string must differ from old_string".to_string(),
                ));
            }

            let original = locked.read()?;
            let content = String::from_utf8(original.clone()).map_err(|e| {
                ToolError::Execution(format!("Failed to decode file as UTF-8: {}", e))
            })?;

            let count = count_occurrences(&content, &old_string);
            if count == 0 {
                return Err(ToolError::NotFound(
                    "Could not find old_string in the file. It must match exactly, including whitespace and indentation.".to_string(),
                ));
            }

            if !replace_all && count > 1 {
                return Err(ToolError::Validation(
                    "Found multiple exact matches for old_string. Provide more surrounding context or set replace_all to true.".to_string(),
                ));
            }

            let (new_content, line_num) = if replace_all {
                (content.replace(&old_string, &new_string), None)
            } else {
                let start = find_exact_occurrence(&content, &old_string)
                    .expect("count was nonzero, so an exact occurrence must exist");
                let mut new_content =
                    String::with_capacity(content.len() - old_string.len() + new_string.len());
                new_content.push_str(&content[..start]);
                new_content.push_str(&new_string);
                new_content.push_str(&content[start + old_string.len()..]);
                let line_num = content[..start].chars().filter(|c| *c == '\n').count() + 1;
                (new_content, Some(line_num))
            };

            locked.write_if_unchanged(&original, new_content.as_bytes())?;

            let mut result = ToolResult::new(
                format!("Edit: {}", file_path),
                if replace_all {
                    format!("Replaced {} occurrence(s)", count)
                } else {
                    format!("Replaced at line {}", line_num.unwrap_or(1))
                },
            )
            .with_metadata("replace_count", serde_json::json!(count));

            if let Some(line_num) = line_num {
                result = result.with_metadata("line_number", serde_json::json!(line_num));
            }

            Ok(result)
        })
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
    async fn edit_rejects_multiple_matches_without_replace_all() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("matches.txt");
        std::fs::write(&file, "same same").unwrap();

        let err = EditTool::new()
            .execute(
                serde_json::json!({
                    "file_path": file,
                    "old_string": "same",
                    "new_string": "after"
                }),
                &test_context(),
            )
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("Found multiple exact matches for old_string"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "same same");
    }

    #[tokio::test]
    async fn edit_replace_all_replaces_exact_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("matches.txt");
        std::fs::write(&file, "same same").unwrap();

        let result = EditTool::new()
            .execute(
                serde_json::json!({
                    "file_path": file,
                    "old_string": "same",
                    "new_string": "after",
                    "replace_all": true
                }),
                &test_context(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(file).unwrap(), "after after");
        assert_eq!(result.metadata["replace_count"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn edit_does_not_fuzzy_replace_tweaked_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tweaked.txt");
        std::fs::write(&file, "alpha\nbravo changed\ngamma\n").unwrap();

        let err = EditTool::new()
            .execute(
                serde_json::json!({
                    "file_path": file,
                    "old_string": "alpha\nbravo\ngamma",
                    "new_string": "replacement"
                }),
                &test_context(),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Could not find old_string"));
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "alpha\nbravo changed\ngamma\n"
        );
    }
}
