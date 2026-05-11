use crate::agent::subagent::{self, SubAgentType};
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult, ToolRegistry,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct TaskTool {
    tool_registry: Arc<ToolRegistry>,
}

impl TaskTool {
    pub fn new(tool_registry: ToolRegistry) -> Self {
        Self {
            tool_registry: Arc::new(tool_registry),
        }
    }
}

#[async_trait]
impl ToolHandler for TaskTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "task".to_string(),
            description: "Launch a new agent to handle complex, multistep tasks autonomously.\n\nWhen using the Task tool, you must specify a subagent_type parameter to select which agent type to use.\n\nWhen to use the Task tool:\n- When you are instructed to execute custom slash commands. Use the Task tool with the slash command invocation as the entire prompt.\n\nWhen NOT to use the Task tool:\n- If you want to read a specific file path, use the Read or Glob tool instead\n- If you are searching for a specific class definition, use the Glob tool instead\n- If you are searching for code within a specific file or set of 2-3 files, use the Read tool instead\n- Other tasks that are not related to the agent descriptions above\n\nUsage notes:\n1. Launch multiple agents concurrently whenever possible, to maximize performance; do that by using multiple tool calls in a single message\n2. When the agent is done, it will return a single message back to you. The result is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.\n3. Each agent invocation starts with a fresh context\n4. The agent's outputs should generally be trusted\n5. Clearly tell the agent whether you expect it to write code or just to do research (search, file reads, web fetches, etc.)\n\nAvailable subagent types:\n- explore: Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns, search code for keywords, or answer questions about the codebase.\n- general: General-purpose agent for researching complex questions and executing multi-step tasks. Use this agent to execute multiple units of work in parallel.".to_string(),
            parameters: vec![
                ParameterSchema {
                    name: "subagent_type".to_string(),
                    description: "The type of specialized agent to use for this task (explore or general)".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "description".to_string(),
                    description: "A short (3-5 words) description of the task".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
                ParameterSchema {
                    name: "prompt".to_string(),
                    description: "The task for the agent to perform".to_string(),
                    required: true,
                    param_type: ParameterType::String,
                },
            ],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["subagent_type", "description", "prompt"])?;

        let subagent_type = get_string_param(params, "subagent_type").unwrap_or_default();
        if SubAgentType::from_str(&subagent_type).is_none() {
            return Err(ToolError::Validation(format!(
                "Invalid subagent_type: '{}'. Must be 'explore' or 'general'",
                subagent_type
            )));
        }

        Ok(())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let subagent_type_str = get_string_param(&params, "subagent_type").unwrap_or_default();
        let description = get_string_param(&params, "description").unwrap_or_default();
        let prompt = get_string_param(&params, "prompt").unwrap_or_default();

        let subagent_type = SubAgentType::from_str(&subagent_type_str)
            .ok_or_else(|| ToolError::Validation(format!(
                "Unknown subagent type: {}", subagent_type_str
            )))?;

        if ctx.is_aborted() {
            return Err(ToolError::Execution("Subagent cancelled".to_string()));
        }

        let result = subagent::run_subagent(
            subagent_type.clone(),
            &description,
            &prompt,
            &self.tool_registry,
        )
        .await
        .map_err(|e| ToolError::Execution(format!("Subagent error: {}", e)))?;

        Ok(ToolResult::new(
            format!("Subagent ({}) result", subagent_type.name()),
            result,
        )
        .with_metadata("subagent_type", serde_json::json!(subagent_type.name())))
    }
}
