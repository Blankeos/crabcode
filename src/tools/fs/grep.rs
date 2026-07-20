use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const BINARY_CHECK_SIZE: usize = 8192;
const RESULT_LIMIT: usize = 200;

pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }

    fn is_binary(data: &[u8]) -> bool {
        data.iter().take(BINARY_CHECK_SIZE).any(|b| *b == 0)
    }

    fn include_matches(include: &Option<glob::Pattern>, path: &Path, base: &Path) -> bool {
        let Some(include) = include else {
            return true;
        };

        let rel = path.strip_prefix(base).unwrap_or(path);
        include.matches_path(rel) || include.matches_path(path)
    }
}

fn no_matches_result(pattern: &str) -> ToolResult {
    ToolResult::new(format!("Grep: {}", pattern), "No matches found.")
        .with_metadata("match_count", serde_json::json!(0))
        .with_metadata("file_count", serde_json::json!(0))
        .with_metadata("truncated", serde_json::json!(false))
        .with_metadata("limit", serde_json::json!(RESULT_LIMIT))
}

#[async_trait]
impl ToolHandler for GrepTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "grep".to_string(),
            description: "Search file contents using regex and return matching lines with file paths and line numbers.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "pattern".to_string(),
                    description: "Regex pattern to search for".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "path".to_string(),
                    description:
                        "Directory or file to search (default: current workspace; blank also uses it)"
                            .to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "include".to_string(),
                    description: "Optional glob filter for files (for example *.rs, *.{ts,tsx})"
                        .to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["pattern"])
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern = get_string_param(&params, "pattern")
            .ok_or_else(|| ToolError::Validation("pattern is required".to_string()))?;
        let path_str = get_string_param(&params, "path");
        let include = get_string_param(&params, "include");

        let regex = regex::Regex::new(&pattern)
            .map_err(|e| ToolError::Validation(format!("Invalid regex pattern: {}", e)))?;

        let base = super::resolve_path(path_str.as_deref(), ctx);
        if !base.exists() {
            return Ok(no_matches_result(&pattern));
        }

        let include_pattern = if let Some(ref include_glob) = include {
            Some(glob::Pattern::new(include_glob).map_err(|e| {
                ToolError::Validation(format!("Invalid include glob pattern: {}", e))
            })?)
        } else {
            None
        };

        let mut output = Vec::new();
        let mut total_matches = 0usize;
        let mut matched_files = 0usize;

        if base.is_file() {
            if Self::include_matches(&include_pattern, &base, &base.parent().unwrap_or(&base)) {
                let content = std::fs::read(&base)
                    .map_err(|e| ToolError::Execution(format!("Failed to read file: {}", e)))?;

                if !Self::is_binary(&content) {
                    let text = String::from_utf8_lossy(&content);
                    let mut file_had_match = false;
                    for (idx, line) in text.lines().enumerate() {
                        if regex.is_match(line) {
                            total_matches += 1;
                            file_had_match = true;
                            if output.len() < RESULT_LIMIT {
                                output.push(format!(
                                    "{}:{}: {}",
                                    base.display(),
                                    idx + 1,
                                    line.trim_end()
                                ));
                            }
                        }
                    }
                    if file_had_match {
                        matched_files += 1;
                    }
                }
            }
        } else {
            let mut walker = ignore::WalkBuilder::new(&base);
            walker
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .parents(true)
                .standard_filters(true);

            for entry in walker.build() {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                if !Self::include_matches(&include_pattern, path, &base) {
                    continue;
                }

                let content = match std::fs::read(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                if Self::is_binary(&content) {
                    continue;
                }

                let text = String::from_utf8_lossy(&content);
                let mut file_had_match = false;
                for (idx, line) in text.lines().enumerate() {
                    if regex.is_match(line) {
                        total_matches += 1;
                        file_had_match = true;
                        if output.len() < RESULT_LIMIT {
                            output.push(format!(
                                "{}:{}: {}",
                                path.display(),
                                idx + 1,
                                line.trim_end()
                            ));
                        }
                    }
                }

                if file_had_match {
                    matched_files += 1;
                }
            }
        }

        let truncated = total_matches > RESULT_LIMIT;
        let result_text = if output.is_empty() {
            "No matches found.".to_string()
        } else {
            let mut text = output.join("\n");
            if truncated {
                text.push_str(&format!(
                    "\n\n... and {} more matches (showing first {})",
                    total_matches - RESULT_LIMIT,
                    RESULT_LIMIT
                ));
            }
            text
        };

        Ok(ToolResult::new(format!("Grep: {}", pattern), result_text)
            .with_metadata("match_count", serde_json::json!(total_matches))
            .with_metadata("file_count", serde_json::json!(matched_files))
            .with_metadata("truncated", serde_json::json!(truncated))
            .with_metadata("limit", serde_json::json!(RESULT_LIMIT)))
    }
}

#[cfg(test)]
mod tests {
    use super::GrepTool;
    use crate::tools::{ToolContext, ToolHandler};
    use serde_json::json;
    use std::path::Path;

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!("{}_{}", prefix, nanos))
    }

    fn tool_context_in(path: &Path) -> ToolContext {
        let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        ToolContext::new("session", "message", "Plan", abort_rx).with_workdir(path)
    }

    #[test]
    fn grep_blank_path_uses_workspace() {
        let dir = unique_temp_dir("crabcode_grep_tool_blank_path_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        std::fs::write(dir.join("job.rs"), "const LIMIT: usize = 100;")
            .expect("test file should be written");

        let result = tokio_test::block_on(GrepTool::new().execute(
            json!({ "path": "", "pattern": "LIMIT", "include": "*.rs" }),
            &tool_context_in(&dir),
        ))
        .expect("blank path should search the workspace");

        assert!(result.output.contains("job.rs:1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_missing_root_returns_no_matches() {
        let dir = unique_temp_dir("crabcode_grep_tool_missing_path_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");

        let result = tokio_test::block_on(GrepTool::new().execute(
            json!({ "path": "missing", "pattern": "LIMIT" }),
            &tool_context_in(&dir),
        ))
        .expect("speculative missing roots should not fail discovery");

        assert_eq!(result.output, "No matches found.");
        assert_eq!(result.metadata["match_count"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
