use crate::tools::{
    get_string_param, validate_required, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
    ParameterSchema, ParameterType,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub struct ListTool;

impl ListTool {
    pub fn new() -> Self {
        Self
    }

    fn list_directory(
        path: &Path,
        ignore_patterns: &[String],
        prefix: &str,
        is_last: bool,
        output: &mut Vec<String>,
        depth: usize,
    ) -> Result<(), ToolError> {
        const MAX_DEPTH: usize = 10;
        
        if depth > MAX_DEPTH {
            return Ok(());
        }

        let connector = if is_last { "└── " } else { "├── " };
        
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            output.push(format!("{}{}{}", prefix, connector, name));
        }

        if !path.is_dir() {
            return Ok(());
        }

        let entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|e| e.ok())
            .collect();

        let mut filtered: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                !name.starts_with('.') && !ignore_patterns.iter().any(|p| name.contains(p))
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
            Self::list_directory(
                &entry.path(),
                ignore_patterns,
                &new_prefix,
                is_last_entry,
                output,
                depth + 1,
            )?;
        }

        Ok(())
    }
}

#[async_trait]
impl ToolHandler for ListTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "list".to_string(),
            description: "List directory contents in a tree format. Shows files and subdirectories with visual tree connectors.".to_string(),
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
            return Err(ToolError::NotFound(format!("Directory not found: {}", path_str)));
        }

        if !path.is_dir() {
            return Err(ToolError::Validation(format!("Path is not a directory: {}", path_str)));
        }

        let mut output = Vec::new();
        
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            output.push(name.to_string());
        } else {
            output.push(path_str.clone());
        }

        let entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|e| e.ok())
            .collect();

        let mut filtered: Vec<_> = entries
            .into_iter()
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                !name.starts_with('.') && !ignore_patterns.iter().any(|p| name.contains(p))
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
            Self::list_directory(
                &entry.path(),
                &ignore_patterns,
                "",
                is_last,
                &mut output,
                1,
            )?;
        }

        let result_text = if output.len() <= 1 {
            format!("{}\n(empty directory)", output.join("\n"))
        } else {
            output.join("\n")
        };

        Ok(ToolResult::new(
            format!("List: {}", path_str),
            result_text
        ))
    }
}
