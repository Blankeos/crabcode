use crate::agent::definition::AgentRegistry;
use crate::agent::subagent;
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolRegistry, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct TaskTool {
    tool_registry: Arc<ToolRegistry>,
    sender: Option<crate::llm::ChunkSender>,
    permissions: Option<crate::tools::ToolPermissions>,
    agent_registry: AgentRegistry,
    cancel_token: CancellationToken,
}

fn stream_chunk_can_batch(chunk: &crate::llm::ChunkMessage) -> bool {
    matches!(
        chunk,
        crate::llm::ChunkMessage::Text(_)
            | crate::llm::ChunkMessage::Reasoning(_)
            | crate::llm::ChunkMessage::Retry(_)
    )
}

fn merge_adjacent_stream_chunks(
    current: &mut crate::llm::ChunkMessage,
    next: crate::llm::ChunkMessage,
) -> Result<(), crate::llm::ChunkMessage> {
    match (current, next) {
        (crate::llm::ChunkMessage::Text(current), crate::llm::ChunkMessage::Text(next)) => {
            current.push_str(&next);
            Ok(())
        }
        (
            crate::llm::ChunkMessage::Reasoning(current),
            crate::llm::ChunkMessage::Reasoning(next),
        ) => {
            current.push_str(&next);
            Ok(())
        }
        (crate::llm::ChunkMessage::Retry(current), crate::llm::ChunkMessage::Retry(next)) => {
            *current = next;
            Ok(())
        }
        (_, next) => Err(next),
    }
}

fn send_subagent_chunk(
    sender: &crate::llm::ChunkSender,
    session_id: &str,
    chunk: crate::llm::ChunkMessage,
) -> Result<(), tokio::sync::mpsc::error::SendError<crate::llm::ChunkMessage>> {
    sender.send(crate::llm::ChunkMessage::SubagentChunk {
        session_id: session_id.to_string(),
        chunk: Box::new(chunk),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::test_scoped_llm_session_lock;

    #[test]
    fn child_stream_batching_merges_only_adjacent_compatible_chunks() {
        let mut text = crate::llm::ChunkMessage::Text("hello".to_string());
        assert!(merge_adjacent_stream_chunks(
            &mut text,
            crate::llm::ChunkMessage::Text(" world".to_string())
        )
        .is_ok());
        assert!(
            matches!(text, crate::llm::ChunkMessage::Text(ref value) if value == "hello world")
        );

        let boundary = merge_adjacent_stream_chunks(
            &mut text,
            crate::llm::ChunkMessage::Reasoning("think".to_string()),
        )
        .expect_err("reasoning must remain an ordering boundary for text");
        assert!(matches!(
            boundary,
            crate::llm::ChunkMessage::Reasoning(ref value) if value == "think"
        ));
    }

    #[test]
    fn child_stream_batching_keeps_latest_retry_status() {
        let first = crate::aisdk::retry::RetryStatus {
            attempt: 1,
            message: "first".to_string(),
            delay_ms: 10,
            next_epoch_ms: 20,
        };
        let second = crate::aisdk::retry::RetryStatus {
            attempt: 2,
            message: "second".to_string(),
            delay_ms: 30,
            next_epoch_ms: 40,
        };
        let mut retry = crate::llm::ChunkMessage::Retry(first);

        assert!(
            merge_adjacent_stream_chunks(&mut retry, crate::llm::ChunkMessage::Retry(second))
                .is_ok()
        );
        assert!(matches!(
            retry,
            crate::llm::ChunkMessage::Retry(status)
                if status.attempt == 2 && status.message == "second"
        ));
    }

    #[test]
    fn task_requires_parent_session_scoped_llm_config() {
        let _lock = test_scoped_llm_session_lock();
        let _reg = crate::agent::config::set_llm_session_for(
            "other-session",
            crate::agent::config::LlmSessionConfig {
                provider_name: "other-provider".to_string(),
                model: "other-model".to_string(),
                api_key: None,
                provider_kind: crate::agent::config::ProviderKind::OpenAICompatible,
                base_url: "https://example.test".to_string(),
                reasoning_effort: None,
                supports_image_input: false,
                openai_options: crate::agent::config::OpenAIRequestOptions::default(),
            },
        );

        let task = TaskTool::new(ToolRegistry::new()).with_runtime_options(
            crate::tools::ToolPermissions::new("."),
            AgentRegistry::default(),
            CancellationToken::new(),
        );
        let params = serde_json::json!({
            "subagent_type": "explore",
            "description": "test",
            "prompt": "look around"
        });
        let ctx = ToolContext::from_cancel_token(
            "parent-session",
            "message",
            "Build",
            CancellationToken::new(),
        );

        let result = tokio_test::block_on(task.execute(params, &ctx));
        assert!(
            matches!(result, Err(ToolError::Execution(ref msg)) if msg.contains("LLM session not configured")),
            "expected missing scoped config error, got {:?}",
            result
        );

        crate::agent::config::remove_llm_session_for("other-session");
    }

    #[test]
    fn plan_parent_cannot_invoke_general_subagent() {
        let task = TaskTool::new(ToolRegistry::new()).with_runtime_options(
            crate::tools::ToolPermissions::new("."),
            AgentRegistry::default(),
            CancellationToken::new(),
        );
        let params = serde_json::json!({
            "subagent_type": "general",
            "description": "test",
            "prompt": "try to write"
        });
        let ctx =
            ToolContext::from_cancel_token("session", "message", "Plan", CancellationToken::new());

        let result = tokio_test::block_on(task.execute(params, &ctx));
        assert!(matches!(result, Err(ToolError::Permission(_))));
    }

    #[test]
    fn explore_subagent_policy_denies_mutating_tools() {
        let registry = AgentRegistry::default();
        let mut policies = crate::tools::AgentToolPolicies::default();
        for (agent, tools) in registry.tool_policy_map() {
            policies = policies.with_custom_tools(agent, tools);
        }
        let permissions = crate::tools::ToolPermissions::new(".").with_agent_policies(policies);

        assert!(permissions.is_tool_allowed_for_agent("explore", "read"));
        assert!(!permissions.is_tool_allowed_for_agent("explore", "bash"));
        assert!(!permissions.is_tool_allowed_for_agent("explore", "apply_patch"));
        assert!(!permissions.is_tool_allowed_for_agent("explore", "write"));
        assert!(!permissions.is_tool_allowed_for_agent("explore", "edit"));
    }

    #[test]
    fn subagent_display_model_prefers_agent_model_override() {
        let _lock = test_scoped_llm_session_lock();
        let parent_session = crate::agent::config::LlmSessionConfig {
            provider_name: "parent-provider".to_string(),
            model: "parent-model".to_string(),
            api_key: None,
            provider_kind: crate::agent::config::ProviderKind::OpenAICompatible,
            base_url: "https://example.test".to_string(),
            reasoning_effort: None,
            supports_image_input: false,
            openai_options: crate::agent::config::OpenAIRequestOptions::default(),
        };

        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "vlm-agent": {
                    "mode": "subagent",
                    "model": "opencode-go/kimi-k2.6"
                }
            })),
            &mut warnings,
        );
        let agent = defs.first().expect("agent definition");

        assert!(warnings.is_empty());
        assert_eq!(
            subagent_display_provider(agent, &parent_session).as_deref(),
            Some("opencode-go")
        );
        assert_eq!(
            subagent_display_model(agent, &parent_session).as_deref(),
            Some("kimi-k2.6")
        );
    }

    #[test]
    fn subagent_display_metadata_falls_back_to_parent_session() {
        let _lock = test_scoped_llm_session_lock();
        let parent_session = crate::agent::config::LlmSessionConfig {
            provider_name: "parent-provider".to_string(),
            model: "parent-model".to_string(),
            api_key: None,
            provider_kind: crate::agent::config::ProviderKind::OpenAICompatible,
            base_url: "https://example.test".to_string(),
            reasoning_effort: None,
            supports_image_input: false,
            openai_options: crate::agent::config::OpenAIRequestOptions::default(),
        };
        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "explore": {
                    "mode": "subagent"
                }
            })),
            &mut warnings,
        );
        let agent = defs.first().expect("agent definition");

        assert!(warnings.is_empty());
        assert_eq!(
            subagent_display_provider(agent, &parent_session).as_deref(),
            Some("parent-provider")
        );
        assert_eq!(
            subagent_display_model(agent, &parent_session).as_deref(),
            Some("parent-model")
        );
    }
}

impl TaskTool {
    pub fn new(tool_registry: ToolRegistry) -> Self {
        Self {
            tool_registry: Arc::new(tool_registry),
            sender: None,
            permissions: None,
            agent_registry: AgentRegistry::default(),
            cancel_token: CancellationToken::new(),
        }
    }

    pub fn with_sender_opt(mut self, sender: Option<crate::llm::ChunkSender>) -> Self {
        self.sender = sender;
        self
    }

    pub fn with_runtime_options(
        mut self,
        permissions: crate::tools::ToolPermissions,
        agent_registry: AgentRegistry,
        cancel_token: CancellationToken,
    ) -> Self {
        self.permissions = Some(permissions);
        self.agent_registry = agent_registry;
        self.cancel_token = cancel_token;
        self
    }
}

#[async_trait]
impl ToolHandler for TaskTool {
    fn definition(&self) -> Tool {
        let available = self
            .agent_registry
            .visible_subagents()
            .into_iter()
            .map(|agent| format!("- {}: {}", agent.name, agent.description))
            .collect::<Vec<_>>()
            .join("\n");
        let available = if available.is_empty() {
            "No visible subagent types are currently configured.".to_string()
        } else {
            available
        };

        Tool {
            id: "task".to_string(),
            description: format!("Launch a new agent to handle complex, multistep tasks autonomously.\n\nWhen using the Task tool, you must specify a subagent_type parameter to select which agent type to use.\n\nWhen to use the Task tool:\n- When you are instructed to execute custom slash commands. Use the Task tool with the slash command invocation as the entire prompt.\n\nWhen NOT to use the Task tool:\n- If you want to read a specific file path, use the Read or Glob tool instead\n- If you are searching for a specific class definition, use the Glob tool instead\n- If you are searching for code within a specific file or set of 2-3 files, use the Read tool instead\n- Other tasks that are not related to the agent descriptions above\n\nUsage notes:\n1. Launch multiple agents concurrently whenever possible, to maximize performance; do that by using multiple tool calls in a single message\n2. When the agent is done, it will return a single message back to you. The result is not visible to the user. To show the user the result, you should send a text message back to the user with a concise summary of the result.\n3. Each agent invocation starts with a fresh context\n4. The agent's outputs should generally be trusted\n5. Clearly tell the agent whether you expect it to write code or just to do research (search, file reads, web fetches, etc.)\n\nAvailable subagent types:\n{}", available),
            parameters: vec![
                ParameterSchema {
                    name: "subagent_type".to_string(),
                    description: "The type of specialized agent to use for this task".to_string(),
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
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["subagent_type", "description", "prompt"])?;

        let subagent_type = get_string_param(params, "subagent_type").unwrap_or_default();
        if self.agent_registry.task_target(&subagent_type).is_none() {
            return Err(ToolError::Validation(format!(
                "Invalid subagent_type: '{}'. Must be a configured subagent",
                subagent_type
            )));
        }

        Ok(())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let subagent_type_str = get_string_param(&params, "subagent_type").unwrap_or_default();
        let description = get_string_param(&params, "description").unwrap_or_default();
        let prompt = get_string_param(&params, "prompt").unwrap_or_default();

        let subagent = self
            .agent_registry
            .task_target(&subagent_type_str)
            .cloned()
            .ok_or_else(|| {
                ToolError::Validation(format!("Unknown subagent type: {}", subagent_type_str))
            })?;

        if !self
            .agent_registry
            .can_agent_invoke(&ctx.agent, &subagent.name)
        {
            return Err(ToolError::Permission(format!(
                "Agent '{}' is not allowed to invoke subagent '{}'",
                ctx.agent, subagent.name
            )));
        }

        if ctx.is_aborted() {
            return Err(ToolError::Execution("Subagent cancelled".to_string()));
        }
        let subagent_cancel_token = ctx.cancel_token.clone();
        let permissions = self
            .permissions
            .clone()
            .unwrap_or_else(|| {
                crate::tools::ToolPermissions::new(crate::utils::cwd::current_dir_or_dot())
            })
            .with_agent_permission_rules(self.agent_registry.permission_rules_map());
        let max_steps = subagent.max_steps;

        let child_session_id = cuid2::create_id();
        let parent_llm_session = crate::agent::config::get_llm_session_for(&ctx.session_id)
            .ok_or_else(|| ToolError::Execution("LLM session not configured".to_string()))?;
        let title = format!(
            "{} (@{} subagent)",
            if description.trim().is_empty() {
                "Task"
            } else {
                description.trim()
            },
            subagent.name
        );

        crate::emit_log!(
            "[TASK] start parent_session_id={} child_session_id={} subagent_type={} title={:?} description_bytes={} prompt_bytes={} sender_present={}",
            ctx.session_id,
            child_session_id,
            subagent.name,
            title,
            description.len(),
            prompt.len(),
            self.sender.is_some()
        );

        let child_sender = self.start_child_session_stream(
            ctx.session_id.clone(),
            child_session_id.clone(),
            title.clone(),
            subagent.name.clone(),
            subagent_display_provider(&subagent, &parent_llm_session),
            subagent_display_model(&subagent, &parent_llm_session),
            description.clone(),
            prompt.clone(),
        );

        let started_at = std::time::Instant::now();
        let result = match subagent::run_subagent(
            subagent.clone(),
            parent_llm_session,
            &description,
            &prompt,
            &self.tool_registry,
            child_sender.clone(),
            child_session_id.clone(),
            subagent_cancel_token,
            permissions,
            max_steps,
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                crate::emit_log!(
                    "[TASK] error parent_session_id={} child_session_id={} subagent_type={} duration_ms={} error={}",
                    ctx.session_id,
                    child_session_id,
                    subagent.name,
                    started_at.elapsed().as_millis(),
                    e
                );
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

        crate::emit_log!(
            "[TASK] finish parent_session_id={} child_session_id={} subagent_type={} duration_ms={} output_bytes={} child_tool_call_count={}",
            ctx.session_id,
            child_session_id,
            subagent.name,
            duration_ms,
            result.output.len(),
            result.tool_call_count
        );

        Ok(ToolResult::new(
            format!("Subagent ({}) result", subagent.name),
            result.output,
        )
        .with_metadata("subagent_type", serde_json::json!(subagent.name))
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
        provider: Option<String>,
        model: Option<String>,
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
            model,
            provider,
            description,
            prompt,
        });

        tokio::spawn(async move {
            const CHILD_STREAM_BATCH_INTERVAL: std::time::Duration =
                std::time::Duration::from_millis(4);

            crate::emit_log!("[TASK] child_forwarder_start session_id={}", session_id);
            while let Some(chunk) = child_rx.recv().await {
                let mut chunk = chunk;
                let batch_started = std::time::Instant::now();

                loop {
                    let remaining =
                        CHILD_STREAM_BATCH_INTERVAL.saturating_sub(batch_started.elapsed());
                    if remaining.is_zero() || !stream_chunk_can_batch(&chunk) {
                        break;
                    }

                    let next = match tokio::time::timeout(remaining, child_rx.recv()).await {
                        Ok(Some(next)) => next,
                        Ok(None) | Err(_) => break,
                    };

                    match merge_adjacent_stream_chunks(&mut chunk, next) {
                        Ok(()) => {}
                        Err(next) => {
                            let _ = send_subagent_chunk(&ui_sender, &session_id, chunk);
                            chunk = next;
                            if !stream_chunk_can_batch(&chunk) {
                                break;
                            }
                        }
                    }
                }

                let _ = send_subagent_chunk(&ui_sender, &session_id, chunk);
            }
            crate::emit_log!("[TASK] child_forwarder_closed session_id={}", session_id);
        });

        Some(child_tx)
    }
}

fn subagent_display_model(
    agent: &crate::agent::definition::AgentDefinition,
    parent_session: &crate::agent::config::LlmSessionConfig,
) -> Option<String> {
    agent
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model_ref| !model_ref.is_empty())
        .map(|model_ref| {
            model_ref
                .split_once('/')
                .map(|(_, model)| model.trim())
                .unwrap_or(model_ref)
                .to_string()
        })
        .or_else(|| Some(parent_session.model.clone()))
}

fn subagent_display_provider(
    agent: &crate::agent::definition::AgentDefinition,
    parent_session: &crate::agent::config::LlmSessionConfig,
) -> Option<String> {
    agent
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model_ref| !model_ref.is_empty())
        .and_then(|model_ref| {
            model_ref
                .split_once('/')
                .map(|(provider, _)| provider.trim().to_string())
        })
        .or_else(|| Some(parent_session.provider_name.clone()))
}
