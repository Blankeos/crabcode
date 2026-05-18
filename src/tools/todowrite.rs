use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
struct TodoItem {
    content: String,
    status: String,
    priority: String,
}

pub struct TodowriteTool;

impl TodowriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for TodowriteTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "todowrite".to_string(),
            description: "Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.\n\n## When to Use This Tool\nUse this tool proactively in these scenarios:\n\n1. Complex multistep tasks - When a task requires 3 or more distinct steps or actions\n2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations\n3. User explicitly requests todo list - When the user directly asks you to use the todo list\n4. User provides multiple tasks - When users provide a list of things to be done\n5. After receiving new instructions - Immediately capture user requirements as todos\n6. After completing a task - Mark it complete and add any new follow-up tasks\n\n## Task States and Management\n\n1. **Task States**: Use these states:\n   - pending: Task not yet started\n   - in_progress: Currently working on (limit to ONE at a time)\n   - completed: Task finished successfully\n   - cancelled: Task no longer needed\n\n2. **Task Management**:\n   - Update task status in real-time as you work\n   - Mark tasks complete IMMEDIATELY after finishing\n   - Only have ONE task in_progress at any time\n   - Complete current tasks before starting new ones\n\nParameters:\n- todos: Array of todo items (JSON string) each with content, status (pending/in_progress/completed/cancelled), and priority (high/medium/low)".to_string(),
            parameters: vec![ParameterSchema {
                name: "todos".to_string(),
                description: "JSON string of todo items array, each with: content, status, priority".to_string(),
                required: true,
                param_type: ParameterType::String,
            }],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["todos"])
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let todos_raw = get_string_param(&params, "todos").unwrap_or_default();

        let todos: Vec<TodoItem> = serde_json::from_str(&todos_raw).map_err(|e| {
            ToolError::Validation(format!("Invalid todo JSON: {}", e))
        })?;

        if todos.is_empty() {
            return Err(ToolError::Validation(
                "Todos array must contain at least one item".to_string(),
            ));
        }

        for (i, todo) in todos.iter().enumerate() {
            if todo.content.trim().is_empty() {
                return Err(ToolError::Validation(format!(
                    "Todo item {} has empty content",
                    i
                )));
            }
            if !matches!(
                todo.status.as_str(),
                "pending" | "in_progress" | "completed" | "cancelled"
            ) {
                return Err(ToolError::Validation(format!(
                    "Todo item '{}' has invalid status: {}. Must be one of: pending, in_progress, completed, cancelled",
                    todo.content, todo.status
                )));
            }
            if !matches!(todo.priority.as_str(), "high" | "medium" | "low") {
                return Err(ToolError::Validation(format!(
                    "Todo item '{}' has invalid priority: {}. Must be one of: high, medium, low",
                    todo.content, todo.priority
                )));
            }
        }

        let mut output = String::new();

        for todo in &todos {
            let mark = match todo.status.as_str() {
                "completed" => "[✓]",
                "in_progress" => "[•]",
                _ => "[ ]",
            };
            output.push_str(&format!("{} {}\n", mark, todo.content));
        }

        Ok(ToolResult::new("Todo list updated", output.clone()).with_metadata(
            "todo_items",
            serde_json::json!(todos),
        ))
    }
}
