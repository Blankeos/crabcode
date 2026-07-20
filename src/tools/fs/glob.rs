use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }

    fn no_matches_result(pattern: &str) -> ToolResult {
        ToolResult::new(
            format!("Glob: {}", pattern),
            "No files found matching pattern.",
        )
        .with_metadata("match_count", serde_json::json!(0))
        .with_metadata("shown_count", serde_json::json!(0))
        .with_metadata("limit", serde_json::json!(100))
        .with_metadata("truncated", serde_json::json!(false))
    }

    fn is_in_git_metadata(path: &Path, base: &Path) -> bool {
        let rel = path.strip_prefix(base).unwrap_or(path);
        rel.components()
            .any(|component| component.as_os_str() == ".git")
    }
}

#[async_trait]
impl ToolHandler for GlobTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "glob".to_string(),
            description:
                "Find files by glob pattern. Includes hidden/gitignored files, excluding .git internals. Returns paths sorted by modification time."
                    .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "pattern".to_string(),
                    description: "Glob pattern to match files (e.g., '**/*.rs', '*.md')"
                        .to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "path".to_string(),
                    description: "Base directory to search from (default: current workspace; blank also uses it)"
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

        let base_path = get_string_param(&params, "path");
        let base = super::resolve_path(base_path.as_deref(), ctx);

        let glob_pattern = glob::Pattern::new(&pattern)
            .map_err(|e| ToolError::Validation(format!("Invalid glob pattern: {}", e)))?;

        if !base.exists() {
            return Ok(Self::no_matches_result(&pattern));
        }

        let pattern_is_absolute = Path::new(&pattern).is_absolute();

        let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

        if base.is_file() {
            let candidate = base.clone();
            let rel = candidate.strip_prefix(&base).unwrap_or(&candidate);
            let matches = if pattern_is_absolute {
                glob_pattern.matches_path(&candidate)
            } else {
                glob_pattern.matches_path(rel)
            };

            if matches {
                let modified = std::fs::metadata(&candidate)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                files.push((candidate, modified));
            }
        } else {
            let mut walker = ignore::WalkBuilder::new(&base);
            walker
                .hidden(false)
                .ignore(false)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .parents(false)
                .standard_filters(false);

            for entry in walker.build() {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let path = entry.path();
                if Self::is_in_git_metadata(path, &base) {
                    continue;
                }

                if !path.is_file() {
                    continue;
                }

                let rel = path.strip_prefix(&base).unwrap_or(path);
                let matches = if pattern_is_absolute {
                    glob_pattern.matches_path(path)
                } else {
                    glob_pattern.matches_path(rel)
                };

                if !matches {
                    continue;
                }

                let modified = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                files.push((path.to_path_buf(), modified));
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
                text.push_str(&format!(
                    "\n\n... and {} more files (showing first {})",
                    total - limit,
                    limit
                ));
            }
            text
        };

        Ok(ToolResult::new(format!("Glob: {}", pattern), result_text)
            .with_metadata(
                "match_count",
                serde_json::Value::Number((total as i64).into()),
            )
            .with_metadata(
                "shown_count",
                serde_json::Value::Number(((total.min(limit)) as i64).into()),
            )
            .with_metadata("limit", serde_json::Value::Number((limit as i64).into()))
            .with_metadata("truncated", serde_json::Value::Bool(truncated)))
    }
}

#[cfg(test)]
mod tests {
    use super::GlobTool;
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
    fn detects_git_metadata_paths() {
        let base = Path::new("/tmp/workspace");
        assert!(GlobTool::is_in_git_metadata(
            Path::new("/tmp/workspace/.git/config"),
            base
        ));
        assert!(!GlobTool::is_in_git_metadata(
            Path::new("/tmp/workspace/.gitignore"),
            base
        ));
    }

    #[test]
    fn glob_blank_path_uses_workspace() {
        let dir = unique_temp_dir("crabcode_glob_tool_blank_path_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        std::fs::write(dir.join("job.rs"), "x").expect("test file should be written");

        let result = tokio_test::block_on(GlobTool::new().execute(
            json!({ "path": "", "pattern": "**/*job*" }),
            &tool_context_in(&dir),
        ))
        .expect("blank path should search the workspace");

        assert!(result.output.contains("job.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_missing_root_returns_no_matches() {
        let dir = unique_temp_dir("crabcode_glob_tool_missing_path_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");

        let result = tokio_test::block_on(GlobTool::new().execute(
            json!({ "path": "missing", "pattern": "**/*.rs" }),
            &tool_context_in(&dir),
        ))
        .expect("speculative missing roots should not fail discovery");

        assert_eq!(result.output, "No files found matching pattern.");
        assert_eq!(result.metadata["match_count"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
