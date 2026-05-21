use crate::agent::subagent::{self, SubAgentType};
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolRegistry, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct TaskTool {
    tool_registry: Arc<ToolRegistry>,
    sender: Option<crate::llm::ChunkSender>,
}

impl TaskTool {
    pub fn new(tool_registry: ToolRegistry) -> Self {
        Self {
            tool_registry: Arc::new(tool_registry),
            sender: None,
        }
    }

    pub fn with_sender_opt(mut self, sender: Option<crate::llm::ChunkSender>) -> Self {
        self.sender = sender;
        self
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

        let subagent_type = SubAgentType::from_str(&subagent_type_str).ok_or_else(|| {
            ToolError::Validation(format!("Unknown subagent type: {}", subagent_type_str))
        })?;

        if ctx.is_aborted() {
            return Err(ToolError::Execution("Subagent cancelled".to_string()));
        }

        let child_session_id = cuid2::create_id();
        let title = format!(
            "{} (@{} subagent)",
            if description.trim().is_empty() {
                "Task"
            } else {
                description.trim()
            },
            subagent_type.name()
        );

        let _ = crate::logging::log(&format!(
            "[TASK] start parent_session_id={} child_session_id={} subagent_type={} title={:?} description_bytes={} prompt_bytes={} sender_present={}",
            ctx.session_id,
            child_session_id,
            subagent_type.name(),
            title,
            description.len(),
            prompt.len(),
            self.sender.is_some()
        ));

        let child_sender = self.start_child_session_stream(
            ctx.session_id.clone(),
            child_session_id.clone(),
            title.clone(),
            subagent_type.name().to_string(),
            description.clone(),
            prompt.clone(),
        );

        let started_at = std::time::Instant::now();
        let result = match subagent::run_subagent(
            subagent_type.clone(),
            &description,
            &prompt,
            &self.tool_registry,
            child_sender.clone(),
            child_session_id.clone(),
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                let _ = crate::logging::log(&format!(
                    "[TASK] error parent_session_id={} child_session_id={} subagent_type={} duration_ms={} error={}",
                    ctx.session_id,
                    child_session_id,
                    subagent_type.name(),
                    started_at.elapsed().as_millis(),
                    e
                ));
                if let Some(sender) = child_sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Failed(e.clone()));
                }
                return Err(ToolError::Execution(format!("Subagent error: {}", e)));
            }
        };

        if let Some(sender) = child_sender.as_ref() {
            let _ = sender.send(crate::llm::ChunkMessage::End);
        }
        let duration_ms = started_at.elapsed().as_millis() as u64;

        let _ = crate::logging::log(&format!(
            "[TASK] finish parent_session_id={} child_session_id={} subagent_type={} duration_ms={} output_bytes={} child_tool_call_count={}",
            ctx.session_id,
            child_session_id,
            subagent_type.name(),
            duration_ms,
            result.output.len(),
            result.tool_call_count
        ));

        Ok(ToolResult::new(
            format!("Subagent ({}) result", subagent_type.name()),
            result.output,
        )
        .with_metadata("subagent_type", serde_json::json!(subagent_type.name()))
        .with_metadata("child_session_id", serde_json::json!(child_session_id))
        .with_metadata("child_session_title", serde_json::json!(title))
        .with_metadata(
            "child_tool_call_count",
            serde_json::json!(result.tool_call_count),
        )
        .with_metadata("duration_ms", serde_json::json!(duration_ms)))
    }
}

impl TaskTool {
    fn start_child_session_stream(
        &self,
        parent_session_id: String,
        session_id: String,
        title: String,
        subagent_type: String,
        description: String,
        prompt: String,
    ) -> Option<crate::llm::ChunkSender> {
        let ui_sender = self.sender.as_ref()?.clone();
        let (child_tx, mut child_rx) = tokio::sync::mpsc::unbounded_channel();

        let _ = ui_sender.send(crate::llm::ChunkMessage::SubagentStarted {
            parent_session_id,
            session_id: session_id.clone(),
            title,
            subagent_type,
            description,
            prompt,
        });

        tokio::spawn(async move {
            let _ = crate::logging::log(&format!(
                "[TASK] child_forwarder_start session_id={}",
                session_id
            ));
            while let Some(chunk) = child_rx.recv().await {
                let _ = ui_sender.send(crate::llm::ChunkMessage::SubagentChunk {
                    session_id: session_id.clone(),
                    chunk: Box::new(chunk),
                });
            }
            let _ = crate::logging::log(&format!(
                "[TASK] child_forwarder_closed session_id={}",
                session_id
            ));
        });

        Some(child_tx)
    }
}
