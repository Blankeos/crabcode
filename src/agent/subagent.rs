use crate::agent::config::ProviderKind;
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

async fn build_subagent_messages(
    agent: &AgentDefinition,
    description: &str,
    prompt: &str,
    permissions: &crate::tools::ToolPermissions,
    scoped_registry: &ToolRegistry,
    skills: &[&crate::skill::SkillInfo],
) -> Vec<crate::aisdk::core::Message> {
    use crate::aisdk::core::Message;

    // Use the tool execution directory, not process cwd or the parent's full
    // conversation. Subagents need workspace rules even with a custom role prompt.
    let working_directory = permissions.workdir().to_string_lossy();
    let is_git_repo = crate::utils::git::is_git_repo(&working_directory).unwrap_or(false);
    let runtime_context = crate::prompt::compose_runtime_context(
        &working_directory,
        is_git_repo,
        std::env::consts::OS,
    )
    .await;
    let role_prompt = agent
        .instructions
        .as_deref()
        .unwrap_or("Complete the delegated task and return a concise, comprehensive result.");
    let skill_guidance = if scoped_registry.get("skill").await.is_some() {
        crate::prompt::render_skill_guidance(
            skills
                .iter()
                .copied()
                .filter(|skill| permissions.is_skill_visible_for_agent(&agent.name, &skill.name)),
        )
    } else {
        String::new()
    };
    let system_prompt = [role_prompt, &runtime_context, &skill_guidance]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    vec![
        Message::system(system_prompt),
        Message::user(format!(
            "## Task Description\n{}\n\n## Task Prompt\n{}",
            description, prompt
        )),
    ]
}

pub async fn run_subagent(
    agent: AgentDefinition,
    parent_session: crate::agent::config::LlmSessionConfig,
    description: &str,
    prompt: &str,
    full_registry: &ToolRegistry,
    sender: Option<crate::llm::ChunkSender>,
    session_id: String,
    cancel_token: tokio_util::sync::CancellationToken,
    permissions: crate::tools::ToolPermissions,
    max_steps: Option<usize>,
    process_registry: Option<std::sync::Arc<crate::tools::ProcessRegistry>>,
) -> Result<SubAgentRunResult, String> {
    use crate::aisdk::core::{
        chunk::ChunkType, response::StreamTextResponse, stop::StopReason, Message as AisdkMessage,
    };
    use futures::StreamExt;
    use std::collections::HashMap;

    let mut session = resolve_subagent_session(&agent, parent_session, sender.as_ref()).await?;
    // Child cache key on purpose: subagents have a different system prompt and tool set,
    // so parent-prefix reuse would miss (and risk sticky-routing pollution). See
    // SessionAffinity::child_session docs.
    session.prompt_cache_key = Some(session_id.clone());
    session.openai_options.prompt_cache_key = Some(session_id.clone());
    if crate::llm::xai_build::is_build_transport(&session.openai_options.additional_headers) {
        let affinity = crate::llm::xai_build::SessionAffinity::child_session(&session_id);
        crate::llm::xai_build::inject_session_affinity_headers(
            &mut session.openai_options.additional_headers,
            &affinity,
        );
        crate::llm::xai_build::inject_compaction_hint_headers(
            &mut session.openai_options.additional_headers,
            &session.model,
            false,
        );
        crate::emit_log!(
            "[prompt-cache] xai-build affinity kind=child session_id={} req_id={}",
            affinity.session_id,
            affinity.req_id
        );
    }

    let scoped_registry = build_scoped_registry(full_registry, &agent).await;
    let skills = crate::skill::get_skill_store()
        .map(|store| store.all())
        .unwrap_or_default();
    let messages = build_subagent_messages(
        &agent,
        description,
        prompt,
        &permissions,
        &scoped_registry,
        &skills,
    )
    .await;

    let mut aisdk_tools = crate::tools::aisdk_bridge::convert_to_aisdk_tools(
        &scoped_registry,
        sender.clone(),
        agent.name.clone(),
        permissions,
        Some(session_id.clone()),
        None,
        session.supports_image_input,
        cancel_token.clone(),
        process_registry,
    )
    .await;
    let hosted_selection = match crate::config::ConfigLoader::load() {
        Ok(loaded) => {
            let ws = &loaded.merged_config.websearch;
            if ws.enabled.unwrap_or(true) {
                Some(
                    crate::aisdk::providers::hosted_search::HostedSearchSelection {
                        web: ws.native.web_enabled(),
                        x: ws.native.x_enabled(),
                    },
                )
            } else {
                None
            }
        }
        Err(_) => Some(crate::aisdk::providers::hosted_search::HostedSearchSelection::DEFAULT),
    };
    if let Some(selection) = hosted_selection {
        if selection.web || selection.x {
            aisdk_tools.extend(crate::aisdk::providers::hosted_search::tools_for(
                &session.provider_name,
                selection,
            ));
        }
    }

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

    let mut response: StreamTextResponse = start_subagent_stream(
        &session,
        messages,
        aisdk_tools,
        max_steps,
        headers,
        Some(cancel_token.clone()),
    )
    .await?;

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
            ChunkType::ProviderToolCall(payload) => {
                tool_call_count = tool_call_count.saturating_add(1);
                if let Some(sender) = sender.as_ref() {
                    let (calls, result) =
                        crate::llm::client::provider_tool_call_ui_events(&payload);
                    if !calls.is_empty() {
                        let _ = sender.send(crate::llm::ChunkMessage::ToolCalls(calls));
                    }
                    if let Some(result) = result {
                        let _ = sender.send(crate::llm::ChunkMessage::ToolResult(result));
                    }
                }
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
            ChunkType::Usage(usage) => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Usage(usage));
                }
            }
            ChunkType::Retry(status) => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Retry(status));
                }
            }
            ChunkType::StreamRollback { text, reasoning } => {
                if collected_text.ends_with(&text) {
                    collected_text.truncate(collected_text.len() - text.len());
                }
                if let Some(sender) = sender.as_ref() {
                    let _ =
                        sender.send(crate::llm::ChunkMessage::StreamRollback { text, reasoning });
                }
            }
            ChunkType::Warning(message) => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Warning(message));
                }
            }
            ChunkType::RetryableFailure(err) => {
                if let Some(sender) = sender.as_ref() {
                    let _ = sender.send(crate::llm::ChunkMessage::Failed(err.message.clone()));
                }
                return Err(format!("Subagent streaming failed: {}", err.message));
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
            Some(cancel_token.clone()),
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
                ChunkType::Usage(usage) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Usage(usage));
                    }
                }
                ChunkType::Retry(status) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Retry(status));
                    }
                }
                ChunkType::StreamRollback { text, reasoning } => {
                    if collected_text.ends_with(&text) {
                        collected_text.truncate(collected_text.len() - text.len());
                    }
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender
                            .send(crate::llm::ChunkMessage::StreamRollback { text, reasoning });
                    }
                }
                ChunkType::Warning(message) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Warning(message));
                    }
                }
                ChunkType::RetryableFailure(err) => {
                    if let Some(sender) = sender.as_ref() {
                        let _ = sender.send(crate::llm::ChunkMessage::Failed(err.message.clone()));
                    }
                    return Err(format!("Subagent max-step summary failed: {}", err.message));
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
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<crate::aisdk::core::response::StreamTextResponse, String> {
    use crate::aisdk::core::response::stream_with_tools;
    use crate::aisdk::{Anthropic, OpenAI, OpenAICompatible};

    let headers = crate::llm::opencode::ensure_session_headers(
        &session.provider_name,
        &session.base_url,
        &session.openai_options.additional_headers,
        session
            .prompt_cache_key
            .as_deref()
            .or(session.openai_options.prompt_cache_key.as_deref()),
        &headers,
    );

    match session.provider_kind {
        ProviderKind::OpenAICompatible => {
            let mut builder = OpenAICompatible::builder()
                .base_url(&session.base_url)
                .model_name(&session.model)
                .provider_name(&session.provider_name)
                .api_key(session.api_key.as_deref().unwrap_or(""))
                .gateway_caching_auto(session.gateway_caching_auto);
            if let Some(effort) = session.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            if let Some(cache_key) = session
                .prompt_cache_key
                .as_deref()
                .or(session.openai_options.prompt_cache_key.as_deref())
            {
                builder = builder.prompt_cache_key(cache_key);
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build OpenAICompatible provider: {}", e))?;

            stream_with_tools(
                provider,
                messages,
                tools,
                max_steps,
                None,
                headers,
                cancel_token,
            )
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

            stream_with_tools(
                provider,
                messages,
                tools,
                max_steps,
                None,
                headers,
                cancel_token,
            )
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
            if let Some(responses_path) = &session.openai_options.response_path {
                builder = builder.responses_path(responses_path);
            }
            if session.openai_options.force_store_false {
                builder = builder.store_override(false);
            }
            if let Some(instructions) =
                crate::llm::client::openai_request_instructions(&session.openai_options, &messages)
            {
                builder = builder.default_instructions(instructions);
            }
            if session.openai_options.disallow_system_messages {
                builder = builder.strip_system_and_developer_messages(true);
                builder = builder.responses_websocket(true);
            }
            if session.openai_options.use_responses_lite {
                builder = builder.responses_lite(true);
            }
            if session.openai_options.force_tool_strict_false {
                builder = builder.tool_strict_override(false);
            }
            if let Some(cache_key) = session
                .prompt_cache_key
                .as_deref()
                .or(session.openai_options.prompt_cache_key.as_deref())
            {
                builder = builder.prompt_cache_key(cache_key);
            }
            let provider = builder
                .build()
                .map_err(|e| format!("Failed to build OpenAI provider: {}", e))?;

            stream_with_tools(
                provider,
                messages,
                tools,
                max_steps,
                None,
                headers,
                cancel_token,
            )
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
    use super::{build_subagent_messages, normalize_subagent_output, resolve_subagent_session};
    use crate::aisdk::core::Message;

    #[tokio::test]
    async fn builtin_subagents_receive_workspace_rules_and_scoped_discovery_guidance() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let working_directory = project.join("src");
        std::fs::create_dir_all(&working_directory).unwrap();
        std::fs::write(project.join("AGENTS.md"), "subagent-project-rule-marker").unwrap();
        std::fs::write(project.join("CLAUDE.md"), "ignored-claude-rule-marker").unwrap();
        let sibling = root.path().join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("AGENTS.md"), "unrelated-sibling-rule-marker").unwrap();
        let permissions = crate::tools::ToolPermissions::new(&working_directory);
        let registry = crate::agent::definition::AgentRegistry::default();

        for name in ["general", "explore"] {
            let agent = registry.task_target(name).unwrap();
            let messages = build_subagent_messages(
                agent,
                "Fix page resolution",
                "Edit owned files only.",
                &permissions,
                &crate::tools::ToolRegistry::new(),
                &[],
            )
            .await;

            assert_eq!(messages.len(), 2);
            let Message::System(system) = &messages[0] else {
                panic!("expected system message");
            };
            assert!(system
                .content
                .starts_with(agent.instructions.as_deref().unwrap()));
            assert!(system.content.contains(&format!(
                "Working directory: {}",
                permissions.workdir().display()
            )));
            assert!(system.content.contains("Platform: "));
            assert!(system.content.contains("Today's date: "));
            assert!(system.content.contains(&format!(
                "Instructions from: {}",
                std::fs::canonicalize(project.join("AGENTS.md"))
                    .unwrap()
                    .display()
            )));
            assert_eq!(
                system
                    .content
                    .matches("subagent-project-rule-marker")
                    .count(),
                1
            );
            assert!(!system.content.contains("ignored-claude-rule-marker"));
            assert!(!system.content.contains("unrelated-sibling-rule-marker"));
            assert!(system
                .content
                .contains("Never recursively search parent or sibling directories"));
            assert!(system
                .content
                .contains("Prefer glob/grep/list/read over shell find"));
            assert!(!system
                .content
                .contains("Plan extensively before each function call"));
            let Message::User(user) = &messages[1] else {
                panic!("expected user message");
            };
            assert_eq!(
                user.content,
                "## Task Description\nFix page resolution\n\n## Task Prompt\nEdit owned files only."
            );
        }
    }

    #[tokio::test]
    async fn custom_and_fallback_subagent_prompts_keep_workspace_context() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "custom-project-rule-marker").unwrap();
        let permissions = crate::tools::ToolPermissions::new(root.path());
        let mut agent = crate::agent::definition::AgentRegistry::default()
            .task_target("general")
            .unwrap()
            .clone();

        for instructions in [Some("Custom role instructions"), None] {
            agent.instructions = instructions.map(str::to_string);
            let messages = build_subagent_messages(
                &agent,
                "Task",
                "Do the task.",
                &permissions,
                &crate::tools::ToolRegistry::new(),
                &[],
            )
            .await;
            let Message::System(system) = &messages[0] else {
                panic!("expected system message");
            };

            let expected_role = instructions.unwrap_or("Complete the delegated task");
            assert!(system.content.starts_with(expected_role));
            assert!(system.content.contains("custom-project-rule-marker"));
            assert!(system
                .content
                .contains("Use the supplied working directory and instructions"));
        }
    }

    #[tokio::test]
    async fn openai_oauth_subagent_instructions_preserve_role_and_workspace_rules() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "oauth-project-rule-marker").unwrap();
        let permissions = crate::tools::ToolPermissions::new(root.path());
        let registry = crate::agent::definition::AgentRegistry::default();
        let agent = registry.task_target("general").unwrap();
        let tools = skill_registry().await;
        let skill = test_skill("oauth-skill");
        let messages = build_subagent_messages(
            agent,
            "Task",
            "Do the task.",
            &permissions,
            &tools,
            &[&skill],
        )
        .await;
        let options = crate::agent::config::OpenAIRequestOptions {
            default_instructions: Some("base oauth instructions".to_string()),
            disallow_system_messages: true,
            ..Default::default()
        };

        let instructions = crate::llm::client::openai_request_instructions(&options, &messages)
            .expect("system prompt must survive stripping");

        assert!(instructions.starts_with("base oauth instructions"));
        assert!(instructions.contains(agent.instructions.as_deref().unwrap()));
        assert!(instructions.contains("Working directory: "));
        assert!(instructions.contains("Never recursively search parent or sibling directories"));
        assert_eq!(instructions.matches("oauth-project-rule-marker").count(), 1);
        assert_eq!(instructions.matches("<available_skills>").count(), 1);
        assert!(instructions.contains("<name>oauth-skill</name>"));
        assert!(!instructions.contains(&skill.content));
        assert!(!instructions.contains("## Task Description"));
    }

    fn test_skill(name: &str) -> crate::skill::SkillInfo {
        crate::skill::SkillInfo {
            name: name.to_string(),
            description: Some(format!("Use {name} for focused testing.")),
            location: std::path::PathBuf::from(format!("/skills/{name}/SKILL.md")),
            content: format!("full-body-marker-{name}"),
        }
    }

    async fn skill_registry() -> crate::tools::ToolRegistry {
        let tools = crate::tools::ToolRegistry::new();
        tools
            .register(std::sync::Arc::new(crate::tools::SkillTool::new()))
            .await;
        tools
    }

    #[tokio::test]
    async fn eligible_subagents_receive_shared_skill_metadata_without_bodies() {
        let root = tempfile::tempdir().unwrap();
        let permissions = crate::tools::ToolPermissions::new(root.path());
        let registry = crate::agent::definition::AgentRegistry::default();
        let mut agent = registry.task_target("general").unwrap().clone();
        let tools = skill_registry().await;
        let skill = test_skill("test-workflow");
        let guidance = crate::prompt::render_skill_guidance([&skill]);

        for custom in [false, true] {
            if custom {
                agent.name = "custom-agent".to_string();
                agent.tools = Some(vec!["skill".to_string()]);
                agent.instructions = Some("Custom role instructions".to_string());
            }
            let scoped = super::build_scoped_registry(&tools, &agent).await;
            let messages = build_subagent_messages(
                &agent,
                "Task",
                "Use test-workflow.",
                &permissions,
                &scoped,
                &[&skill],
            )
            .await;
            let Message::System(system) = &messages[0] else {
                panic!("expected system message")
            };
            assert!(system.content.contains(&guidance));
            assert_eq!(system.content.matches("<available_skills>").count(), 1);
            assert!(system
                .content
                .contains("<description>Use test-workflow for focused testing.</description>"));
            assert!(system
                .content
                .contains("<location>file:///skills/test-workflow/SKILL.md</location>"));
            assert!(system
                .content
                .contains("use the skill tool to load it before responding"));
            assert!(!system.content.contains(&skill.content));
        }
    }

    #[tokio::test]
    async fn subagents_do_not_advertise_unavailable_skill_tools_or_empty_catalogs() {
        use crate::tools::permission::{PermissionPolicyAction, PermissionRule};

        let root = tempfile::tempdir().unwrap();
        let permissions = crate::tools::ToolPermissions::new(root.path());
        let disabled =
            permissions
                .clone()
                .with_global_tool_config(std::collections::HashMap::from([(
                    "skill".to_string(),
                    false,
                )]));
        let denied = permissions
            .clone()
            .with_permission_rules(vec![PermissionRule {
                permission: "skill".to_string(),
                pattern: "*".to_string(),
                action: PermissionPolicyAction::Deny,
            }]);
        let registry = crate::agent::definition::AgentRegistry::default();
        let general = registry.task_target("general").unwrap();
        let explore = registry.task_target("explore").unwrap();
        let tools = skill_registry().await;
        let empty_tools = crate::tools::ToolRegistry::new();
        let skill = test_skill("test-workflow");
        let skills = [&skill];

        for (agent, permissions, tools, skills) in [
            (explore, &permissions, &tools, skills.as_slice()),
            (general, &disabled, &tools, skills.as_slice()),
            (general, &denied, &tools, skills.as_slice()),
            (general, &permissions, &empty_tools, skills.as_slice()),
            (general, &permissions, &tools, &[][..]),
        ] {
            let scoped = super::build_scoped_registry(tools, agent).await;
            let messages = build_subagent_messages(
                agent,
                "Task",
                "Do the task.",
                permissions,
                &scoped,
                skills,
            )
            .await;
            let Message::System(system) = &messages[0] else {
                panic!("expected system message")
            };
            assert!(!system.content.contains("<available_skills>"));
            assert!(!system.content.contains("test-workflow"));
            assert!(!system.content.contains("Use the skill tool"));
        }
    }

    #[tokio::test]
    async fn skill_advertising_respects_agent_overrides_and_keeps_ask_skills_visible() {
        use crate::tools::permission::{PermissionPolicyAction, PermissionRule};

        let root = tempfile::tempdir().unwrap();
        let registry = crate::agent::definition::AgentRegistry::default();
        let agent = registry.task_target("general").unwrap();
        let rule = |name: &str, action| PermissionRule {
            permission: "skill".to_string(),
            pattern: name.to_string(),
            action,
        };
        let permissions = crate::tools::ToolPermissions::new(root.path())
            .with_permission_rules(vec![
                rule("blocked", PermissionPolicyAction::Deny),
                rule("overridden", PermissionPolicyAction::Deny),
            ])
            .with_agent_permission_rules(std::collections::HashMap::from([(
                agent.name.clone(),
                vec![
                    rule("overridden", PermissionPolicyAction::Allow),
                    rule("approval", PermissionPolicyAction::Ask),
                ],
            )]));
        let tools = skill_registry().await;
        let allowed = test_skill("allowed");
        let blocked = test_skill("blocked");
        let overridden = test_skill("overridden");
        let approval = test_skill("approval");
        let messages = build_subagent_messages(
            agent,
            "Task",
            "Do the task.",
            &permissions,
            &tools,
            &[&allowed, &blocked, &overridden, &approval],
        )
        .await;
        let Message::System(system) = &messages[0] else {
            panic!("expected system message")
        };
        for name in ["allowed", "overridden", "approval"] {
            assert!(system.content.contains(&format!("<name>{name}</name>")));
        }
        assert!(!system.content.contains("blocked"));
    }

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

    #[test]
    fn inherited_parent_session_preserves_openai_request_options() {
        let mut warnings = Vec::new();
        let agent = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "explore": {
                    "mode": "subagent"
                }
            })),
            &mut warnings,
        )
        .pop()
        .expect("agent definition");
        let mut parent = test_session(None);
        parent.provider_kind = crate::agent::config::ProviderKind::OpenAI;
        parent.openai_options.response_path = Some("/backend-api/codex/responses".to_string());
        parent.openai_options.disallow_system_messages = true;
        parent.openai_options.force_store_false = true;
        parent.openai_options.force_tool_strict_false = true;
        parent.openai_options.default_instructions = Some("Codex".to_string());
        parent
            .openai_options
            .additional_headers
            .insert("originator".to_string(), "crabcode".to_string());

        let session = tokio_test::block_on(resolve_subagent_session(&agent, parent, None))
            .expect("resolved session");

        assert!(warnings.is_empty());
        assert_eq!(
            session.openai_options.response_path.as_deref(),
            Some("/backend-api/codex/responses")
        );
        assert!(session.openai_options.disallow_system_messages);
        assert!(session.openai_options.force_store_false);
        assert!(session.openai_options.force_tool_strict_false);
        assert_eq!(
            session.openai_options.default_instructions.as_deref(),
            Some("Codex")
        );
        assert_eq!(
            session.openai_options.additional_headers.get("originator"),
            Some(&"crabcode".to_string())
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
            openai_options: crate::agent::config::OpenAIRequestOptions::default(),
            prompt_cache_key: None,
            gateway_caching_auto: false,
        }
    }
}
