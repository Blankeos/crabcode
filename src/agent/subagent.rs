use crate::agent::config::{get_llm_session, ProviderKind};
use crate::agent::definition::AgentDefinition;
use crate::tools::ToolRegistry;

pub struct SubAgentRunResult {
    pub output: String,
    pub tool_call_count: usize,
}

pub async fn build_scoped_registry(
    full_registry: &ToolRegistry,
    agent: &AgentDefinition,
) -> ToolRegistry {
    let scoped = ToolRegistry::new();
    let allowed = agent.tools.as_ref();

    let full_tools = full_registry.list().await;

    for tool_def in &full_tools {
        let tool_allowed = allowed
            .is_none_or(|tools| tools.iter().any(|tool| tool == "*" || tool == &tool_def.id));
        if tool_allowed {
            if let Some(handler) = full_registry.get(&tool_def.id).await {
                scoped.register(handler).await;
            }
        }
    }

    scoped
}

pub async fn run_subagent(
    agent: AgentDefinition,
    description: &str,
    prompt: &str,
    full_registry: &ToolRegistry,
    sender: Option<crate::llm::ChunkSender>,
    session_id: String,
    cancel_token: tokio_util::sync::CancellationToken,
    permissions: crate::tools::ToolPermissions,
    max_steps: Option<usize>,
) -> Result<SubAgentRunResult, String> {
    use crate::aisdk::core::{
        chunk::ChunkType, response::StreamTextResponse, stop::StopReason, Message as AisdkMessage,
    };
    use futures::StreamExt;
    use std::collections::HashMap;

    let parent_session = get_llm_session().ok_or("LLM session not configured")?;
    let session = resolve_subagent_session(&agent, parent_session, sender.as_ref()).await?;

    let scoped_registry = build_scoped_registry(full_registry, &agent).await;

    let aisdk_tools = crate::tools::aisdk_bridge::convert_to_aisdk_tools(
        &scoped_registry,
        sender.clone(),
        agent.name.clone(),
        permissions,
        Some(session_id.clone()),
        None,
        session.supports_image_input,
        cancel_token.clone(),
    )
    .await;

    let system_prompt = agent
        .instructions
        .as_deref()
        .unwrap_or("Complete the delegated task and return a concise, comprehensive result.");
    let user_content = format!(
        "## Task Description\n{}\n\n## Task Prompt\n{}",
        description, prompt
    );

    let messages = vec![
        AisdkMessage::system(system_prompt),
        AisdkMessage::user(user_content),
    ];

    let headers = HashMap::new();
    let stream_started_at = std::time::Instant::now();
    crate::emit_log!(
        "[SUBAGENT] stream_start session_id={} subagent_type={} tools={} description_bytes={} prompt_bytes={} max_steps={:?} sender_present={}",
        session_id,
        agent.name,
        aisdk_tools.len(),
        description.len(),
        prompt.len(),
        max_steps,
        sender.is_some()
    );

    let mut response: StreamTextResponse =
        start_subagent_stream(&session, messages, aisdk_tools, max_steps, headers).await?;

    let mut collected_text = String::new();
    let mut tool_call_count = 0usize;

    loop {
        let chunk = tokio::select! {
            _ = cancel_token.cancelled() => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
                }
                return Err("Subagent cancelled".to_string());
            }
            chunk = response.stream.next() => chunk,
        };

        let Some(chunk) = chunk else {
            break;
        };

        match chunk {
            ChunkType::Text(text) => {
                collected_text.push_str(&text);
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Text(text));
                }
            }
            ChunkType::Reasoning(reasoning) => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
                }
            }
            ChunkType::ToolCall(tool_call) => {
                let calls = serde_json::from_str::<serde_json::Value>(&tool_call)
                    .ok()
                    .and_then(|value| value.as_array().map(|items| items.len()))
                    .unwrap_or(1);
                tool_call_count = tool_call_count.saturating_add(calls);
            }
            ChunkType::Failed(err) => {
                crate::emit_log!(
                    "[SUBAGENT] stream_failed session_id={} subagent_type={} duration_ms={} error={}",
                    session_id,
                    agent.name,
                    stream_started_at.elapsed().as_millis(),
                    err
                );
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                }
                return Err(format!("Subagent streaming failed: {}", err));
            }
            ChunkType::End { .. } => {
                break;
            }
            ChunkType::ResponseCompleted { .. } => {
                break;
            }
            ChunkType::Metadata(message) => {
                crate::emit_log!(
                    "[SUBAGENT_METADATA] session_id={} subagent_type={} {}",
                    session_id,
                    agent.name,
                    message
                );
            }
            _ => {}
        }
    }

    let stop_reason = response.stop_reason().await;
    if max_steps.is_some() && matches!(stop_reason, Some(StopReason::Hook)) {
        if let Some(sender) = sender.as_ref() {
            let _ = sender.send(crate::llm::ChunkMessage::Warning(
                "Maximum configured steps reached. Sending text-only subagent summary.".to_string(),
            ));
        }

        let mut follow_up_messages = response.messages().await;
        follow_up_messages.push(AisdkMessage::assistant(
            crate::llm::client::MAX_STEPS_REACHED_PROMPT,
        ));
        let mut summary_response = start_subagent_stream(
            &session,
            follow_up_messages,
            Vec::new(),
            None,
            HashMap::new(),
        )
        .await?;

        loop {
            let chunk = tokio::select! {
                _ = cancel_token.cancelled() => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
                    }
                    return Err("Subagent cancelled".to_string());
                }
                chunk = summary_response.stream.next() => chunk,
            };

            let Some(chunk) = chunk else {
                break;
            };

            match chunk {
                ChunkType::Text(text) => {
                    collected_text.push_str(&text);
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Text(text));
                    }
                }
                ChunkType::Reasoning(reasoning) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
                    }
                }
                ChunkType::Failed(err) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                    }
                    return Err(format!("Subagent max-step summary failed: {}", err));
                }
                ChunkType::End { .. } | ChunkType::ResponseCompleted { .. } => break,
                ChunkType::Metadata(message) => {
                    crate::emit_log!(
                        "[SUBAGENT_METADATA] session_id={} subagent_type={} {}",
                        session_id,
                        agent.name,
                        message
                    );
                }
                _ => {}
            }
        }
    }
    crate::emit_log!(
        "[SUBAGENT] stream_finish session_id={} subagent_type={} duration_ms={} stop_reason={:?} text_bytes={} tool_call_count={}",
        session_id,
        agent.name,
        stream_started_at.elapsed().as_millis(),
        stop_reason,
        collected_text.len(),
        tool_call_count
    );

    Ok(SubAgentRunResult {
        output: normalize_subagent_output(collected_text),
        tool_call_count,
    })
}

async fn start_subagent_stream(
    session: &crate::agent::config::LlmSessionConfig,
    messages: Vec<crate::aisdk::core::Message>,
    tools: Vec<crate::aisdk::core::Tool>,
    max_steps: Option<usize>,
    headers: std::collections::HashMap<String, String>,
) -> Result<crate::aisdk::core::response::StreamTextResponse, String> {
    use crate::aisdk::core::response::stream_with_tools;
    use crate::aisdk::{Anthropic, OpenAI, OpenAICompatible};

    match session.provider_kind {
        ProviderKind::OpenAICompatible => {
            let mut builder = OpenAICompatible::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""));
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build OpenAICompatible provider: {}", e))?;

            stream_with_tools(provider, messages, tools, max_steps, None, headers)
                .await
                .map_err(|e| format!("Stream error: {}", e))
        }
        ProviderKind::Anthropic => {
            let mut builder = Anthropic::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""));
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build Anthropic provider: {}", e))?;

            stream_with_tools(provider, messages, tools, max_steps, None, headers)
                .await
                .map_err(|e| format!("Stream error: {}", e))
        }
        ProviderKind::OpenAI => {
            let mut builder = OpenAI::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""));
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build OpenAI provider: {}", e))?;

            stream_with_tools(provider, messages, tools, max_steps, None, headers)
                .await
                .map_err(|e| format!("Stream error: {}", e))
        }
    }
}

async fn resolve_subagent_session(
    agent: &AgentDefinition,
    parent_session: crate::agent::config::LlmSessionConfig,
    sender: Option<&crate::llm::ChunkSender>,
) -> Result<crate::agent::config::LlmSessionConfig, String> {
    let Some(model_ref) = agent.model.as_deref() else {
        let mut session = parent_session;
        session.reasoning_effort = agent.reasoning_effort;
        return Ok(session);
    };

    let model_ref = model_ref.trim();
    if model_ref.is_empty() {
        let mut session = parent_session;
        session.reasoning_effort = agent.reasoning_effort;
        return Ok(session);
    }

    let Some((provider, model)) = model_ref.split_once('/') else {
        let mut session = parent_session;
        session.model = model_ref.to_string();
        session.reasoning_effort = agent.reasoning_effort;
        return Ok(session);
    };
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        let mut session = parent_session;
        session.reasoning_effort = agent.reasoning_effort;
        return Ok(session);
    }

    let (fallback_sender, _fallback_rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = sender.unwrap_or(&fallback_sender);
    crate::llm::client::build_subagent_llm_session(
        provider,
        model.to_string(),
        agent.reasoning_effort,
        sender,
    )
    .await
    .map_err(|err| err.to_string())
}

fn normalize_subagent_output(output: String) -> String {
    if output.trim().is_empty() {
        "Subagent completed without a final text response.".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_subagent_output, resolve_subagent_session};

    #[test]
    fn empty_subagent_output_is_not_an_error_payload() {
        assert_eq!(
            normalize_subagent_output("   \n".to_string()),
            "Subagent completed without a final text response."
        );
    }

    #[test]
    fn non_empty_subagent_output_is_preserved() {
        assert_eq!(
            normalize_subagent_output("Hi there".to_string()),
            "Hi there"
        );
    }

    #[test]
    fn subagent_without_model_does_not_inherit_parent_reasoning_effort() {
        let mut warnings = Vec::new();
        let agent = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "frontend-agent": {
                    "mode": "subagent",
                    "reasoningEffort": null
                }
            })),
            &mut warnings,
        )
        .pop()
        .expect("agent definition");
        let parent = test_session(Some(crate::model::reasoning::ReasoningEffort::High));

        let session = tokio_test::block_on(resolve_subagent_session(&agent, parent, None))
            .expect("resolved session");

        assert!(warnings.is_empty());
        assert_eq!(session.reasoning_effort, None);
    }

    #[test]
    fn subagent_model_shorthand_does_not_inherit_parent_reasoning_effort() {
        let mut warnings = Vec::new();
        let agent = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "frontend-agent": {
                    "mode": "subagent",
                    "model": "child-model",
                    "reasoningEffort": null
                }
            })),
            &mut warnings,
        )
        .pop()
        .expect("agent definition");
        let parent = test_session(Some(crate::model::reasoning::ReasoningEffort::High));

        let session = tokio_test::block_on(resolve_subagent_session(&agent, parent, None))
            .expect("resolved session");

        assert!(warnings.is_empty());
        assert_eq!(session.model, "child-model");
        assert_eq!(session.reasoning_effort, None);
    }

    #[test]
    fn explicit_subagent_reasoning_effort_is_applied_to_parent_session() {
        let mut warnings = Vec::new();
        let agent = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "frontend-agent": {
                    "mode": "subagent",
                    "reasoningEffort": "low"
                }
            })),
            &mut warnings,
        )
        .pop()
        .expect("agent definition");
        let parent = test_session(Some(crate::model::reasoning::ReasoningEffort::High));

        let session = tokio_test::block_on(resolve_subagent_session(&agent, parent, None))
            .expect("resolved session");

        assert!(warnings.is_empty());
        assert_eq!(
            session.reasoning_effort,
            Some(crate::model::reasoning::ReasoningEffort::Low)
        );
    }

    fn test_session(
        reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    ) -> crate::agent::config::LlmSessionConfig {
        crate::agent::config::LlmSessionConfig {
            provider_name: "parent-provider".to_string(),
            model: "parent-model".to_string(),
            api_key: None,
            provider_kind: crate::agent::config::ProviderKind::OpenAICompatible,
            base_url: "https://example.test".to_string(),
            reasoning_effort,
            supports_image_input: false,
        }
    }
}
