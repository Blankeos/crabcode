use crate::tools::{
    ParameterSchema, ParameterType, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

const PLAN_UPDATED_MESSAGE: &str = "Plan updated";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PlanItem {
    step: String,
    status: String,
}

#[derive(Debug, Clone)]
struct PlanUpdate {
    explanation: Option<String>,
    plan: Vec<PlanItem>,
}

pub struct UpdatePlanTool;

impl UpdatePlanTool {
    pub fn new() -> Self {
        Self
    }
}

fn plan_item_param_type() -> ParameterType {
    let mut props = HashMap::new();
    props.insert("step".to_string(), ParameterType::String);
    props.insert("status".to_string(), ParameterType::String);
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
        // Legacy todo lists could mark an item cancelled. The Codex plan UI has
        // only three states, so preserve the item without implying it completed.
        "cancelled" | "canceled" => "pending".to_string(),
        value => value.to_string(),
    }
}

fn item_from_plain(step: &str, status: &str) -> PlanItem {
    PlanItem {
        step: step.trim().to_string(),
        status: status.to_string(),
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

fn parse_checkbox_line(line: &str) -> Option<PlanItem> {
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
    } else if let Some(rest) = line.strip_prefix("✔") {
        ("completed", rest)
    } else if let Some(rest) = line.strip_prefix("[•]") {
        ("in_progress", rest)
    } else if let Some(rest) = line.strip_prefix("□") {
        ("pending", rest)
    } else {
        return None;
    };

    let step = rest.trim();
    if step.is_empty() {
        None
    } else {
        Some(item_from_plain(step, status))
    }
}

fn parse_plain_plan(raw: &str) -> Vec<PlanItem> {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            parse_checkbox_line(trimmed).or_else(|| {
                let step = strip_list_marker(trimmed);
                if step.is_empty() {
                    None
                } else {
                    Some(item_from_plain(step, "pending"))
                }
            })
        })
        .collect()
}

fn parse_plan_item(value: &Value) -> Result<PlanItem, ToolError> {
    match value {
        Value::Object(obj) => {
            let step = obj
                .get("step")
                .or_else(|| obj.get("content"))
                .or_else(|| obj.get("todo"))
                .or_else(|| obj.get("task"))
                .or_else(|| obj.get("title"))
                .or_else(|| obj.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            Ok(PlanItem {
                step: step.trim().to_string(),
                status: normalize_status(obj.get("status").and_then(|v| v.as_str())),
            })
        }
        Value::String(step) => Ok(item_from_plain(step, "pending")),
        _ => Err(ToolError::Validation(
            "Each plan item must be an object or string".to_string(),
        )),
    }
}

fn parse_plan_items(value: &Value) -> Result<Vec<PlanItem>, ToolError> {
    match value {
        Value::Array(items) => items.iter().map(parse_plan_item).collect(),
        Value::Object(_) => Ok(vec![parse_plan_item(value)?]),
        Value::String(raw) => parse_plan_string(raw),
        _ => Err(ToolError::Validation(
            "plan must be an array, object, string, or JSON string".to_string(),
        )),
    }
}

fn parse_plan_string(raw: &str) -> Result<Vec<PlanItem>, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::Validation(
            "Plan parameter cannot be empty".to_string(),
        ));
    }

    if trimmed
        .lines()
        .any(|line| parse_checkbox_line(line).is_some())
    {
        return Ok(parse_plain_plan(trimmed));
    }

    let starts_like_json = trimmed.starts_with('[') || trimmed.starts_with('{');
    if !starts_like_json {
        return Ok(parse_plain_plan(trimmed));
    }

    let parsed = serde_json::from_str::<Value>(trimmed)
        .map_err(|e| ToolError::Validation(format!("Invalid plan JSON: {}", e)))?;
    parse_plan_items(&parsed)
}

fn parse_update_plan(params: &Value) -> Result<PlanUpdate, ToolError> {
    let obj = params
        .as_object()
        .ok_or_else(|| ToolError::Validation("Parameters must be an object".to_string()))?;

    let explanation = obj
        .get("explanation")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    let plan_value = obj.get("plan").or_else(|| obj.get("todos"));
    let Some(plan_value) = plan_value else {
        return Err(ToolError::Validation(
            "Missing required parameter: plan".to_string(),
        ));
    };

    let plan = parse_plan_items(plan_value)?;
    validate_plan_items(&plan)?;

    Ok(PlanUpdate { explanation, plan })
}

fn validate_plan_items(plan: &[PlanItem]) -> Result<(), ToolError> {
    if plan.is_empty() {
        return Err(ToolError::Validation(
            "Plan must contain at least one item".to_string(),
        ));
    }

    let mut in_progress_count = 0;

    for (idx, item) in plan.iter().enumerate() {
        if item.step.trim().is_empty() {
            return Err(ToolError::Validation(format!(
                "Plan item {} has empty step",
                idx + 1
            )));
        }

        if !matches!(
            item.status.as_str(),
            "pending" | "in_progress" | "completed"
        ) {
            return Err(ToolError::Validation(format!(
                "Plan item '{}' has invalid status: {}. Must be one of: pending, in_progress, completed",
                item.step, item.status
            )));
        }

        if item.status == "in_progress" {
            in_progress_count += 1;
        }
    }

    if in_progress_count > 1 {
        return Err(ToolError::Validation(
            "Plan must contain at most one in_progress item".to_string(),
        ));
    }

    Ok(())
}

#[async_trait]
impl ToolHandler for UpdatePlanTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "update_plan".to_string(),
            description: "Update the current task plan. Use this for non-trivial, multi-step work. Provide an optional explanation and a plan array with step/status items. Status must be pending, in_progress, or completed. At most one step can be in_progress at a time.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "explanation".to_string(),
                    description: "Optional short explanation for this plan update".to_string(),
                    required: false,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "plan".to_string(),
                    description: "Array of plan items, each with step and status (pending, in_progress, completed). At most one item may be in_progress.".to_string(),
                    required: true,
                    param_type: ParameterType::Array(Box::new(plan_item_param_type())),
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        parse_update_plan(params).map(|_| ())
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let update = parse_update_plan(&params)?;

        Ok(ToolResult::new("Plan updated", PLAN_UPDATED_MESSAGE)
            .with_metadata("explanation", serde_json::json!(update.explanation))
            .with_metadata("plan", serde_json::json!(update.plan)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_update_plan_accepts_codex_shape() {
        let params = json!({
            "explanation": "Now implementing.",
            "plan": [{
                "step": "Implement rendering",
                "status": "in_progress"
            }]
        });

        let update = parse_update_plan(&params).unwrap();

        assert_eq!(update.explanation.as_deref(), Some("Now implementing."));
        assert_eq!(update.plan.len(), 1);
        assert_eq!(update.plan[0].step, "Implement rendering");
        assert_eq!(update.plan[0].status, "in_progress");
    }

    #[test]
    fn parse_update_plan_accepts_legacy_todos_shape() {
        let params = json!({
            "todos": [{
                "content": "Choose rendering file",
                "status": "pending",
                "priority": "medium"
            }]
        });

        let update = parse_update_plan(&params).unwrap();

        assert_eq!(update.plan.len(), 1);
        assert_eq!(update.plan[0].step, "Choose rendering file");
        assert_eq!(update.plan[0].status, "pending");
    }

    #[test]
    fn parse_update_plan_accepts_plain_checkbox_text() {
        let params = json!({
            "plan": "[ ] Define table data\n[•] Implement rendering\n[✓] Verify output"
        });

        let update = parse_update_plan(&params).unwrap();

        assert_eq!(update.plan.len(), 3);
        assert_eq!(update.plan[0].status, "pending");
        assert_eq!(update.plan[1].status, "in_progress");
        assert_eq!(update.plan[2].status, "completed");
    }

    #[test]
    fn parse_update_plan_rejects_multiple_in_progress_items() {
        let params = json!({
            "plan": [
                {"step": "Implement rendering", "status": "in_progress"},
                {"step": "Validate rendering", "status": "in_progress"}
            ]
        });

        let err = parse_update_plan(&params).unwrap_err();

        assert!(err.to_string().contains("at most one in_progress item"));
    }

    #[tokio::test]
    async fn execute_returns_codex_style_ack_with_structured_metadata() {
        let params = json!({
            "explanation": "Now implementing.",
            "plan": [
                {"step": "Implement rendering", "status": "in_progress"},
                {"step": "Validate", "status": "pending"}
            ]
        });
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let ctx = ToolContext::new("session", "message", "Build", rx);

        let result = UpdatePlanTool::new().execute(params, &ctx).await.unwrap();

        assert_eq!(result.title, "Plan updated");
        assert_eq!(result.output, PLAN_UPDATED_MESSAGE);
        assert!(result.metadata.contains_key("plan"));
        assert!(result.metadata.contains_key("explanation"));
    }
}
