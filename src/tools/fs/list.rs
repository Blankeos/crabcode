use crate::tools::{
    get_integer_param, get_string_param, validate_required, ParameterSchema, ParameterType, Tool,
    ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

const DEFAULT_LIMIT: usize = 2_000;

pub struct ListTool;

impl ListTool {
    pub fn new() -> Self {
        Self
    }

    fn entry_name(path: &Path, entry: &std::fs::DirEntry) -> Option<String> {
        let name = entry.file_name().to_string_lossy().to_string();
        let kind = entry.file_type().ok()?;
        let is_dir = if kind.is_dir() {
            true
        } else if kind.is_symlink() {
            std::fs::metadata(path.join(&name))
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        } else {
            false
        };

        Some(if is_dir { format!("{}/", name) } else { name })
    }

    fn list_entries(path: &Path) -> Result<Vec<String>, ToolError> {
        let mut entries: Vec<String> = std::fs::read_dir(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                Self::entry_name(path, &entry)
            })
            .collect();

        entries.sort();
        Ok(entries)
    }

    fn format_entries(path: &Path, entries: &[String], offset: usize, limit: usize) -> String {
        let start = offset.min(entries.len());
        let end = start.saturating_add(limit).min(entries.len());
        let selected = &entries[start..end];
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
        output
    }
}

#[async_trait]
impl ToolHandler for ListTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "list".to_string(),
            description:
                "List direct directory entries with pagination. Directories are suffixed with `/`."
                    .to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "path".to_string(),
                    description:
                        "Directory path to list (default: current workspace; blank also uses it)"
                            .to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "offset".to_string(),
                    description: "Entry offset to start from (0-based, default: 0)".to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
                ParameterSchema {
                    name: "limit".to_string(),
                    description: "Maximum number of entries to return (default: 2000)".to_string(),
                    required: false,
                    param_type: ParameterType::Integer,
                },
            ],
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &[])
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = get_string_param(&params, "path");
        let offset = get_integer_param(&params, "offset")
            .map(|value| value.max(0) as usize)
            .unwrap_or(0);
        let limit = get_integer_param(&params, "limit")
            .map(|value| {
                if value <= 0 {
                    DEFAULT_LIMIT
                } else {
                    value as usize
                }
            })
            .unwrap_or(DEFAULT_LIMIT);

        let path = super::resolve_path(path_str.as_deref(), ctx);

        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "Directory not found: {}. Current workspace: {}",
                path.display(),
                ctx.workdir().display()
            )));
        }

        if !path.is_dir() {
            return Err(ToolError::Validation(format!(
                "Path is not a directory: {}",
                path.display()
            )));
        }

        let entries = Self::list_entries(&path)?;
        let end = offset.saturating_add(limit).min(entries.len());
        let truncated = end < entries.len();
        let result_text = Self::format_entries(&path, &entries, offset, limit);
        let preview = entries
            .iter()
            .skip(offset)
            .take(limit)
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        Ok(
            ToolResult::new(format!("List: {}", path.display()), result_text)
                .with_metadata("truncated", serde_json::json!(truncated))
                .with_metadata("count", serde_json::json!(entries.len()))
                .with_metadata("offset", serde_json::json!(offset))
                .with_metadata("limit", serde_json::json!(limit))
                .with_metadata("preview", serde_json::json!(preview)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ListTool;
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

    fn tool_context_in(path: &std::path::Path) -> ToolContext {
        tool_context().with_workdir(path)
    }

    #[test]
    fn list_blank_path_uses_workspace() {
        let dir = unique_temp_dir("crabcode_list_tool_blank_path_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        std::fs::write(dir.join("workspace.txt"), "x").expect("test file should be written");

        let result = tokio_test::block_on(
            ListTool::new().execute(json!({ "path": "" }), &tool_context_in(&dir)),
        )
        .expect("blank path should list the workspace");

        assert!(result.output.contains("workspace.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_missing_path_remains_an_error() {
        let dir = unique_temp_dir("crabcode_list_tool_missing_path_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");

        let result = tokio_test::block_on(
            ListTool::new().execute(json!({ "path": "missing" }), &tool_context_in(&dir)),
        );

        assert!(matches!(result, Err(crate::tools::ToolError::NotFound(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_outputs_direct_entries_sorted_with_directory_markers() {
        let dir = unique_temp_dir("crabcode_list_tool_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        std::fs::create_dir_all(dir.join("src")).expect("child dir should be created");
        std::fs::write(dir.join("README.md"), "x").expect("test file should be written");
        std::fs::write(dir.join("src").join("nested.rs"), "x")
            .expect("nested file should be written");

        let tool = ListTool::new();
        let result = tokio_test::block_on(tool.execute(
            json!({ "path": dir.to_string_lossy().to_string() }),
            &tool_context(),
        ))
        .expect("list should succeed");

        assert!(result.output.contains("<type>directory</type>"));
        assert!(result.output.contains("README.md"));
        assert!(result.output.contains("src/"));
        assert!(!result.output.contains("nested.rs"));
        assert!(result.output.contains("(2 entries)"));
        assert_eq!(
            result.metadata.get("truncated").and_then(|v| v.as_bool()),
            Some(false)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_supports_offset_and_limit() {
        let dir = unique_temp_dir("crabcode_list_tool_page_test");
        std::fs::create_dir_all(&dir).expect("temp dir should be created");

        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.join(name), "x").expect("test file should be written");
        }

        let tool = ListTool::new();
        let result = tokio_test::block_on(tool.execute(
            json!({
                "path": dir.to_string_lossy().to_string(),
                "offset": 1,
                "limit": 1,
            }),
            &tool_context(),
        ))
        .expect("list should succeed");

        assert!(!result.output.contains("a.txt"));
        assert!(result.output.contains("b.txt"));
        assert!(!result.output.contains("c.txt"));
        assert!(result.output.contains("Showing 1 of 3 entries"));
        assert_eq!(
            result.metadata.get("truncated").and_then(|v| v.as_bool()),
            Some(true)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
