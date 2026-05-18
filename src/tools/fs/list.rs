use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const MAX_DEPTH: usize = 3;
const MAX_OUTPUT_LINES: usize = 1_000;
const DEFAULT_SKIPPED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".next",
    ".turbo",
    ".cache",
    "dist",
    "build",
    "coverage",
];

pub struct ListTool;

impl ListTool {
    pub fn new() -> Self {
        Self
    }

    fn should_skip_entry(name: &str, ignore_patterns: &[String]) -> bool {
        // Keep expensive generated/dependency trees out of default recursive
        // output while still surfacing ordinary dotfiles such as .env.
        if DEFAULT_SKIPPED_DIRS.contains(&name) {
            return true;
        }

        ignore_patterns.iter().any(|p| name.contains(p))
    }

    fn push_output(output: &mut Vec<String>, line: String, truncated: &mut bool) -> bool {
        if output.len() >= MAX_OUTPUT_LINES {
            *truncated = true;
            return false;
        }

        output.push(line);
        true
    }

    fn list_directory(
        path: &Path,
        ignore_patterns: &[String],
        prefix: &str,
        is_last: bool,
        output: &mut Vec<String>,
        depth: usize,
        truncated: &mut bool,
    ) -> Result<bool, ToolError> {
        if depth > MAX_DEPTH {
            return Ok(true);
        }

        let connector = if is_last { "└── " } else { "├── " };

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if !Self::push_output(
                output,
                format!("{}{}{}", prefix, connector, name),
                truncated,
            ) {
                return Ok(false);
            }
        }

        if !path.is_dir() {
            return Ok(true);
        }

        let entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|e| e.ok())
            .collect();

        let mut filtered: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                !Self::should_skip_entry(&name, ignore_patterns)
            })
            .collect();

        filtered.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);

            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        let new_prefix = if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        let count = filtered.len();
        for (i, entry) in filtered.iter().enumerate() {
            let is_last_entry = i == count - 1;
            if !Self::list_directory(
                &entry.path(),
                ignore_patterns,
                &new_prefix,
                is_last_entry,
                output,
                depth + 1,
                truncated,
            )? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

#[async_trait]
impl ToolHandler for ListTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "list".to_string(),
            description: "List directory contents in a bounded tree format. Includes hidden files, while skipping common generated/dependency directories unless listed directly."
                .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "path".to_string(),
                    description: "Directory path to list".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "ignore".to_string(),
                    description: "Patterns to ignore (e.g., ['node_modules', 'target'])".to_string(),
                    required: false,
                    param_type: ParameterType::Array(Box::new(ParameterType::String)),
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["path"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = get_string_param(&params, "path")
            .ok_or_else(|| ToolError::Validation("path is required".to_string()))?;

        let ignore_patterns: Vec<String> = params
            .get("ignore")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let path = Path::new(&path_str);

        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Directory not found: {}",
                path_str
            )));
        }

        if !path.is_dir() {
            return Err(ToolError::Validation(format!(
                "Path is not a directory: {}",
                path_str
            )));
        }

        let mut output = Vec::new();
        let mut truncated = false;

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            Self::push_output(&mut output, name.to_string(), &mut truncated);
        } else {
            Self::push_output(&mut output, path_str.clone(), &mut truncated);
        }

        let entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|e| e.ok())
            .collect();

        let mut filtered: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                !Self::should_skip_entry(&name, &ignore_patterns)
            })
            .collect();

        filtered.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);

            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        let count = filtered.len();
        for (i, entry) in filtered.iter().enumerate() {
            let is_last = i == count - 1;
            if !Self::list_directory(
                &entry.path(),
                &ignore_patterns,
                "",
                is_last,
                &mut output,
                1,
                &mut truncated,
            )? {
                break;
            }
        }

        let mut result_text = if output.len() <= 1 {
            format!("{}\n(empty directory)", output.join("\n"))
        } else {
            output.join("\n")
        };

        if truncated {
            result_text.push_str(&format!(
                "\n\n... output truncated after {} entries. Narrow the path or add ignore patterns for more.",
                MAX_OUTPUT_LINES
            ));
        }

        Ok(ToolResult::new(format!("List: {}", path_str), result_text)
            .with_metadata("truncated", serde_json::json!(truncated))
            .with_metadata("limit", serde_json::json!(MAX_OUTPUT_LINES))
            .with_metadata("max_depth", serde_json::json!(MAX_DEPTH)))
    }
}

#[cfg(test)]
mod tests {
    use super::{ListTool, MAX_OUTPUT_LINES};
    use crate::tools::{ToolContext, ToolHandler};
    use serde_json::json;

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!("{}_{}", prefix, nanos))
    }

    fn tool_context() -> ToolContext {
        let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        ToolContext::new("session", "message", "Plan", abort_rx)
    }

    #[test]
    fn should_skip_entry_keeps_dotenv_visible() {
        assert!(!ListTool::should_skip_entry(".env", &[]));
        assert!(!ListTool::should_skip_entry(".env.local", &[]));
    }

    #[test]
    fn should_skip_entry_hides_git_metadata_directory() {
        assert!(ListTool::should_skip_entry(".git", &[]));
    }

    #[test]
    fn should_skip_entry_hides_generated_directories() {
        assert!(ListTool::should_skip_entry("target", &[]));
        assert!(ListTool::should_skip_entry("node_modules", &[]));
    }

    #[test]
    fn list_output_is_bounded() {
        let dir = unique_temp_dir("crabcode_list_tool_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");

        for idx in 0..(MAX_OUTPUT_LINES + 25) {
            std::fs::write(dir.join(format!("file_{idx:04}.txt")), "x")
                .expect("test file should be written");
        }

        let tool = ListTool::new();
        let result = tokio_test::block_on(tool.execute(
            json!({ "path": dir.to_string_lossy().to_string() }),
            &tool_context(),
        ))
        .expect("list should succeed");

        assert!(result.output.contains("output truncated"));
        assert_eq!(
            result.metadata.get("truncated").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(result.output.lines().count() <= MAX_OUTPUT_LINES + 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
