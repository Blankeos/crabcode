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

    fn is_in_git_metadata(path: &Path, base: &Path) -> bool {
        let rel = path.strip_prefix(base).unwrap_or(path);
        rel.components().any(|component| component.as_os_str() == ".git")
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
                    description:
                        "Base directory to search from (default: current working directory)"
                            .to_string(),
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

        let base_path = get_string_param(&params, "path").unwrap_or_else(|| ".".to_string());
        let base = PathBuf::from(&base_path);

        if !base.exists() {
            return Err(ToolError::NotFound(format!(
                "Path not found: {}",
                base_path
            )));
        }

        let glob_pattern = glob::Pattern::new(&pattern)
            .map_err(|e| ToolError::Validation(format!("Invalid glob pattern: {}", e)))?;

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
    use std::path::Path;

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
}
