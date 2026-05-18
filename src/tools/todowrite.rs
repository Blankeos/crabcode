use crate::tools::{
    validate_required, ParameterSchema, ParameterType, Tool, ToolContext, ToolError, ToolHandler,
    ToolResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize)]
struct TodoItem {
    content: String,
    status: String,
    priority: String,
}

fn todo_item_param_type() -> ParameterType {
    let mut props = HashMap::new();
    props.insert("content".to_string(), ParameterType::String);
    props.insert("status".to_string(), ParameterType::String);
    props.insert("priority".to_string(), ParameterType::String);
    ParameterType::Object(props)
}

fn normalize_status(status: Option<&str>) -> String {
    match status
        .unwrap_or("pending")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "todo" | "open" | "not_started" | "not-started" => "pending".to_string(),
        "doing" | "active" | "in-progress" | "in progress" => "in_progress".to_string(),
        "done" | "complete" => "completed".to_string(),
        "canceled" => "cancelled".to_string(),
        value => value.to_string(),
    }
}

fn normalize_priority(priority: Option<&str>) -> String {
    match priority
        .unwrap_or("medium")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "normal" => "medium".to_string(),
        value => value.to_string(),
    }
}

fn todo_from_plain(content: &str, status: &str) -> TodoItem {
    TodoItem {
        content: content.trim().to_string(),
        status: status.to_string(),
        priority: "medium".to_string(),
    }
}

fn strip_list_marker(line: &str) -> &str {
    let trimmed = line.trim();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return rest.trim_start();
    }

    if let Some((prefix, rest)) = trimmed.split_once(". ") {
        if !prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit()) {
            return rest.trim_start();
        }
    }

    trimmed
}

fn parse_checkbox_line(line: &str) -> Option<TodoItem> {
    let line = strip_list_marker(line);
    let (status, rest) = if let Some(rest) = line.strip_prefix("[ ]") {
        ("pending", rest)
    } else if let Some(rest) = line.strip_prefix("[x]") {
        ("completed", rest)
    } else if let Some(rest) = line.strip_prefix("[X]") {
        ("completed", rest)
    } else if let Some(rest) = line.strip_prefix("[✓]") {
        ("completed", rest)
    } else if let Some(rest) = line.strip_prefix("[✔]") {
        ("completed", rest)
    } else if let Some(rest) = line.strip_prefix("[•]") {
        ("in_progress", rest)
    } else {
        return None;
    };

    let content = rest.trim();
    if content.is_empty() {
        None
    } else {
        Some(todo_from_plain(content, status))
    }
}

fn parse_plain_todos(raw: &str) -> Vec<TodoItem> {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            parse_checkbox_line(trimmed).or_else(|| {
                let content = strip_list_marker(trimmed);
                if content.is_empty() {
                    None
                } else {
                    Some(todo_from_plain(content, "pending"))
                }
            })
        })
        .collect()
}

fn parse_todo_value(value: &Value) -> Result<TodoItem, ToolError> {
    match value {
        Value::Object(obj) => {
            let content = obj
                .get("content")
                .or_else(|| obj.get("todo"))
                .or_else(|| obj.get("task"))
                .or_else(|| obj.get("title"))
                .or_else(|| obj.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            Ok(TodoItem {
                content: content.trim().to_string(),
                status: normalize_status(obj.get("status").and_then(|v| v.as_str())),
                priority: normalize_priority(obj.get("priority").and_then(|v| v.as_str())),
            })
        }
        Value::String(content) => Ok(todo_from_plain(content, "pending")),
        _ => Err(ToolError::Validation(
            "Each todo must be an object or string".to_string(),
        )),
    }
}

fn parse_todos_value(value: &Value) -> Result<Vec<TodoItem>, ToolError> {
    match value {
        Value::Array(items) => items.iter().map(parse_todo_value).collect(),
        Value::Object(_) | Value::String(_) => Ok(vec![parse_todo_value(value)?]),
        _ => Err(ToolError::Validation(
            "todos must be an array, object, string, or JSON string".to_string(),
        )),
    }
}

fn parse_todos_string(raw: &str) -> Result<Vec<TodoItem>, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::Validation(
            "Todos parameter cannot be empty".to_string(),
        ));
    }

    if trimmed
        .lines()
        .any(|line| parse_checkbox_line(line).is_some())
    {
        return Ok(parse_plain_todos(trimmed));
    }

    let starts_like_json = trimmed.starts_with('[') || trimmed.starts_with('{');
    if !starts_like_json {
        return Ok(parse_plain_todos(trimmed));
    }

    let parsed = serde_json::from_str::<Value>(trimmed)
        .map_err(|e| ToolError::Validation(format!("Invalid todo JSON: {}", e)))?;
    parse_todos_value(&parsed)
}

fn parse_todos_param(params: &Value) -> Result<Vec<TodoItem>, ToolError> {
    let raw = params
        .get("todos")
        .ok_or_else(|| ToolError::Validation("Missing required parameter: todos".to_string()))?;

    match raw {
        Value::String(s) => parse_todos_string(s),
        Value::Array(_) | Value::Object(_) => parse_todos_value(raw),
        _ => Err(ToolError::Validation(
            "todos must be an array, object, string, or JSON string".to_string(),
        )),
    }
}

fn validate_todos(todos: &[TodoItem]) -> Result<(), ToolError> {
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

    Ok(())
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
            description: "Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.\n\n## When to Use This Tool\nUse this tool proactively in these scenarios:\n\n1. Complex multistep tasks - When a task requires 3 or more distinct steps or actions\n2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations\n3. User explicitly requests todo list - When the user directly asks you to use the todo list\n4. User provides multiple tasks - When users provide a list of things to be done\n5. After receiving new instructions - Immediately capture user requirements as todos\n6. After completing a task - Mark it complete and add any new follow-up tasks\n\n## Task States and Management\n\n1. **Task States**: Use these states:\n   - pending: Task not yet started\n   - in_progress: Currently working on (limit to ONE at a time)\n   - completed: Task finished successfully\n   - cancelled: Task no longer needed\n\n2. **Task Management**:\n   - Update task status in real-time as you work\n   - Mark tasks complete IMMEDIATELY after finishing\n   - Only have ONE task in_progress at any time\n   - Complete current tasks before starting new ones\n\nParameters:\n- todos: Array of todo items, each with content, status (pending/in_progress/completed/cancelled), and priority (high/medium/low)".to_string(),
            parameters: vec![ParameterSchema {
                name: "todos".to_string(),
                description: "Array of todo items, each with: content, status, priority. Plain checklist text is also accepted for compatibility.".to_string(),
                required: true,
                param_type: ParameterType::Array(Box::new(todo_item_param_type())),
            }],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["todos"])?;
        let todos = parse_todos_param(params)?;
        validate_todos(&todos)
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let todos = parse_todos_param(&params)?;
        validate_todos(&todos)?;

        let mut output = String::new();

        for todo in &todos {
            let mark = match todo.status.as_str() {
                "completed" => "[✓]",
                "in_progress" => "[•]",
                _ => "[ ]",
            };
            output.push_str(&format!("{} {}\n", mark, todo.content));
        }

        Ok(ToolResult::new("Todo list updated", output.clone())
            .with_metadata("todo_items", serde_json::json!(todos)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_todos_accepts_structured_array() {
        let params = json!({
            "todos": [{
                "content": "Implement rendering",
                "status": "in_progress",
                "priority": "high"
            }]
        });

        let todos = parse_todos_param(&params).unwrap();

        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "Implement rendering");
        assert_eq!(todos[0].status, "in_progress");
        assert_eq!(todos[0].priority, "high");
    }

    #[test]
    fn parse_todos_accepts_json_string_for_compatibility() {
        let params = json!({
            "todos": r#"[{"content":"Choose rendering file","status":"pending","priority":"medium"}]"#
        });

        let todos = parse_todos_param(&params).unwrap();

        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].content, "Choose rendering file");
    }

    #[test]
    fn parse_todos_accepts_plain_checkbox_text() {
        let params = json!({
            "todos": "[ ] Define table data\n[•] Implement rendering\n[✓] Verify output"
        });

        let todos = parse_todos_param(&params).unwrap();

        assert_eq!(todos.len(), 3);
        assert_eq!(todos[0].status, "pending");
        assert_eq!(todos[1].status, "in_progress");
        assert_eq!(todos[2].status, "completed");
        assert_eq!(todos[0].priority, "medium");
    }
}
