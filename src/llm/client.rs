use crate::agent::config::OpenAIRequestOptions;
use crate::aisdk::core::{
    chunk::{ChunkType, MessagePhase},
    response::{stream_with_hosted_tools, LanguageModelStream, StreamTextResponse},
    stop::StopReason,
    HostedTool, Message as AisdkMessage, Tool,
};
use crate::aisdk::message::ImageContent;
use crate::aisdk::{Anthropic, OpenAI, OpenAICompatible};
use futures::StreamExt;
use std::{collections::HashMap, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::tools::aisdk_bridge::convert_to_aisdk_tools;

pub(crate) const MAX_STEPS_REACHED_PROMPT: &str = r#"CRITICAL - MAXIMUM STEPS REACHED

The maximum number of steps allowed for this task has been reached. Tools are disabled until next user input. Respond with text only.

STRICT REQUIREMENTS:
1. Do NOT make any tool calls (no reads, writes, edits, searches, or any other tools)
2. MUST provide a text response summarizing work done so far
3. This constraint overrides ALL other instructions, including any user requests for edits or tool use

Response must include:
- Statement that maximum steps for this agent have been reached
- Summary of what has been accomplished so far
- List of any remaining tasks that were not completed
- Recommendations for what should be done next

Any attempt to use tools is a critical violation. Respond with text ONLY."#;

const TOOL_HISTORY_ARGUMENTS_MAX_CHARS: usize = 4_000;

type DynError = Box<dyn std::error::Error>;

#[derive(Clone, Debug)]
struct ProviderRequestConfig {
    kind: ProviderKind,
    provider_name: String,
    base_url: String,
    model_name: String,
    api_key: Option<String>,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    supports_image_input: bool,
    openai_options: OpenAIRequestOptions,
}

impl ProviderRequestConfig {
    fn new(
        kind: ProviderKind,
        provider_name: String,
        base_url: String,
        model_name: String,
        api_key: Option<String>,
        reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
        supports_image_input: bool,
    ) -> Self {
        Self {
            kind,
            provider_name,
            base_url,
            model_name,
            api_key,
            reasoning_effort,
            supports_image_input,
            openai_options: OpenAIRequestOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamRelayOutcome {
    Ended,
    Exhausted,
}

fn stream_outcome_label(
    outcome: StreamRelayOutcome,
    stop_reason: Option<&StopReason>,
) -> &'static str {
    match (outcome, stop_reason) {
        (StreamRelayOutcome::Ended, _) => "Ended",
        (StreamRelayOutcome::Exhausted, Some(StopReason::Finish)) => "Finished",
        (StreamRelayOutcome::Exhausted, Some(StopReason::Hook)) => "StepLimit",
        (StreamRelayOutcome::Exhausted, _) => "Exhausted",
    }
}

#[derive(Clone, Copy, Debug)]
struct StreamLogContext<'a> {
    phase: &'a str,
    provider_name: &'a str,
    provider_kind: ProviderKind,
    base_url: &'a str,
    model_name: &'a str,
    message_count: usize,
    tool_count: usize,
    agent_max_steps: Option<usize>,
}

impl<'a> StreamLogContext<'a> {
    fn new(
        phase: &'a str,
        config: &'a ProviderRequestConfig,
        message_count: usize,
        tool_count: usize,
        agent_max_steps: Option<usize>,
    ) -> Self {
        Self {
            phase,
            provider_name: &config.provider_name,
            provider_kind: config.kind,
            base_url: &config.base_url,
            model_name: &config.model_name,
            message_count,
            tool_count,
            agent_max_steps,
        }
    }

    fn describe(self) -> String {
        format!(
            "phase={} provider={} provider_kind={:?} base_url={} model={} messages={} tools={} agent_max_steps={:?}",
            self.phase,
            self.provider_name,
            self.provider_kind,
            self.base_url,
            self.model_name,
            self.message_count,
            self.tool_count,
            self.agent_max_steps,
        )
    }
}

#[derive(Clone, Debug, Default)]
struct RelayStats {
    start_chunks: usize,
    text_chunks: usize,
    reasoning_chunks: usize,
    tool_call_chunks: usize,
    assistant_phase_chunks: usize,
    metadata_chunks: usize,
    response_completed_chunks: usize,
    failed_chunks: usize,
    incomplete_chunks: usize,
    not_supported_chunks: usize,
    text_chars: usize,
    commentary_text_chars: usize,
    final_answer_text_chars: usize,
    unphased_text_chars: usize,
    reasoning_chars: usize,
    tool_call_bytes: usize,
    tool_call_argument_chars: usize,
    tool_call_arguments_done_chars: usize,
    last_chunk: Option<&'static str>,
    last_progress_chunk: Option<&'static str>,
    current_assistant_phase: Option<&'static str>,
    last_metadata: Option<String>,
    last_tool_call_names: Option<String>,
    first_chunk_elapsed_ms: Option<u128>,
    last_progress_elapsed_ms: Option<u128>,
    last_text_elapsed_ms: Option<u128>,
    last_tool_call_elapsed_ms: Option<u128>,
}

impl RelayStats {
    fn record_chunk(&mut self, name: &'static str, elapsed_ms: u128) {
        if self.first_chunk_elapsed_ms.is_none() {
            self.first_chunk_elapsed_ms = Some(elapsed_ms);
        }
        self.last_chunk = Some(name);
        self.last_progress_chunk = Some(name);
        self.last_progress_elapsed_ms = Some(elapsed_ms);
    }

    fn record_failed_chunk(&mut self) {
        self.failed_chunks += 1;
        self.last_chunk = Some("Failed");
    }

    fn record_text(&mut self, len: usize, elapsed_ms: u128) {
        self.last_text_elapsed_ms = Some(elapsed_ms);
        match self.current_assistant_phase {
            Some("commentary") => self.commentary_text_chars += len,
            Some("final_answer") => self.final_answer_text_chars += len,
            _ => self.unphased_text_chars += len,
        }
    }

    fn record_assistant_phase(&mut self, phase: Option<MessagePhase>) {
        self.assistant_phase_chunks += 1;
        self.current_assistant_phase = Some(message_phase_label(phase));
    }

    fn record_metadata(&mut self, message: &str) {
        self.metadata_chunks += 1;
        self.last_metadata = Some(truncate_log_value(message, 120));

        if let Some(phase) = message.strip_prefix("assistant_message_phase=") {
            self.current_assistant_phase = Some(match phase {
                "commentary" => "commentary",
                "final_answer" => "final_answer",
                _ => "unknown",
            });
        }
    }

    fn record_tool_call(&mut self, info: &ToolCallLogInfo, elapsed_ms: u128) {
        self.last_tool_call_elapsed_ms = Some(elapsed_ms);
        self.tool_call_argument_chars += info.argument_chars;
        self.tool_call_arguments_done_chars += info.arguments_done_chars;
        if !info.names.is_empty() {
            self.last_tool_call_names = Some(info.names.join(","));
        }
    }

    fn describe_at(&self, elapsed_ms: Option<u128>) -> String {
        let idle_since_progress_ms = elapsed_ms
            .zip(self.last_progress_elapsed_ms)
            .map(|(now, last)| now.saturating_sub(last));
        format!(
            "chunks[start={}, text={} text_chars={} text_by_phase[commentary={}, final_answer={}, unphased={}], reasoning={} reasoning_chars={}, tool_calls={} tool_call_bytes={} tool_arg_chars={} tool_arg_done_chars={}, assistant_phase={}, metadata={}, response_completed={}, failed={}, incomplete={}, not_supported={}, last={}, last_progress={}] timing[first_chunk_ms={}, last_progress_ms={}, idle_since_progress_ms={}, last_text_ms={}, last_tool_call_ms={}] current_phase={} last_tool_names={} last_metadata={}",
            self.start_chunks,
            self.text_chunks,
            self.text_chars,
            self.commentary_text_chars,
            self.final_answer_text_chars,
            self.unphased_text_chars,
            self.reasoning_chunks,
            self.reasoning_chars,
            self.tool_call_chunks,
            self.tool_call_bytes,
            self.tool_call_argument_chars,
            self.tool_call_arguments_done_chars,
            self.assistant_phase_chunks,
            self.metadata_chunks,
            self.response_completed_chunks,
            self.failed_chunks,
            self.incomplete_chunks,
            self.not_supported_chunks,
            self.last_chunk.unwrap_or("none"),
            self.last_progress_chunk.unwrap_or("none"),
            optional_u128(self.first_chunk_elapsed_ms),
            optional_u128(self.last_progress_elapsed_ms),
            optional_u128(idle_since_progress_ms),
            optional_u128(self.last_text_elapsed_ms),
            optional_u128(self.last_tool_call_elapsed_ms),
            self.current_assistant_phase.unwrap_or("none"),
            self.last_tool_call_names.as_deref().unwrap_or("none"),
            self.last_metadata.as_deref().unwrap_or("none"),
        )
    }
}

#[derive(Clone, Debug, Default)]
struct ToolCallLogInfo {
    names: Vec<String>,
    ids: Vec<String>,
    argument_chars: usize,
    arguments_done_chars: usize,
}

impl ToolCallLogInfo {
    fn names_label(&self) -> String {
        if self.names.is_empty() {
            "unknown".to_string()
        } else {
            self.names.join(",")
        }
    }

    fn ids_label(&self) -> String {
        if self.ids.is_empty() {
            "unknown".to_string()
        } else {
            self.ids.join(",")
        }
    }
}

fn tool_call_log_info(tool_call: &str) -> ToolCallLogInfo {
    let mut info = ToolCallLogInfo::default();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(tool_call) else {
        return info;
    };

    let Some(items) = value.as_array() else {
        return info;
    };

    for item in items {
        if let Some(id) = item.get("id").and_then(|id| id.as_str()) {
            info.ids.push(id.to_string());
        }

        let Some(function) = item.get("function") else {
            continue;
        };

        if let Some(name) = function.get("name").and_then(|name| name.as_str()) {
            info.names.push(name.to_string());
        }
        if let Some(arguments) = function.get("arguments").and_then(|args| args.as_str()) {
            info.argument_chars += arguments.len();
        }
        if let Some(arguments_done) = function
            .get("arguments_done")
            .and_then(|args| args.as_str())
        {
            info.arguments_done_chars += arguments_done.len();
        }
    }

    info
}

fn message_phase_label(phase: Option<MessagePhase>) -> &'static str {
    match phase {
        Some(MessagePhase::Commentary) => "commentary",
        Some(MessagePhase::FinalAnswer) => "final_answer",
        None => "unknown",
    }
}

fn optional_u128(value: Option<u128>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn truncate_log_value(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

#[derive(Clone, Debug)]
struct StreamRelayResult {
    outcome: StreamRelayOutcome,
    stats: RelayStats,
}

pub async fn stream_llm_with_cancellation(
    cancel_token: CancellationToken,
    session_id: String,
    provider_name: String,
    model: String,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    agent_mode: String,
    agent_max_steps: Option<usize>,
    agent_registry: crate::agent::definition::AgentRegistry,
    tool_permissions: crate::tools::ToolPermissions,
    websearch_config: crate::config::configuration::WebsearchConfig,
    mcp_config: crate::config::configuration::McpConfig,
    workspace: String,
    messages: Vec<crate::session::types::Message>,
    sender: crate::llm::ChunkSender,
) -> Result<(), DynError> {
    struct SessionConfigGuard(crate::agent::config::LlmSessionRegistration);
    impl Drop for SessionConfigGuard {
        fn drop(&mut self) {
            crate::agent::config::remove_llm_session_if_owned(&self.0);
        }
    }
    crate::emit_log!(
        "GOING TO STREAM session_id={} provider={} model={} agent_mode={} agent_max_steps={:?} input_messages={}",
        session_id,
        provider_name,
        model,
        agent_mode,
        agent_max_steps,
        messages.len()
    );
    let request_config =
        prepare_request_config(&provider_name, model, reasoning_effort, &sender).await?;
    let mut request_config = request_config;
    // Sticky prompt-cache routing: same key for every tool step in this session.
    request_config.openai_options.prompt_cache_key = Some(session_id.clone());

    let tool_registry = crate::tools::initialize_tool_registry_with_dynamic_config(
        Some(sender.clone()),
        tool_permissions.clone(),
        agent_registry.clone(),
        cancel_token.clone(),
        Some(&request_config.provider_name),
        &websearch_config,
        &mcp_config,
        &workspace,
    )
    .await;
    // Set LLM session config for subagent use
    let llm_session = crate::agent::config::LlmSessionConfig {
        provider_name: request_config.provider_name.clone(),
        model: request_config.model_name.clone(),
        api_key: request_config.api_key.clone(),
        provider_kind: match request_config.kind {
            ProviderKind::OpenAI => crate::agent::config::ProviderKind::OpenAI,
            ProviderKind::OpenAICompatible => crate::agent::config::ProviderKind::OpenAICompatible,
            ProviderKind::Anthropic => crate::agent::config::ProviderKind::Anthropic,
        },
        base_url: request_config.base_url.clone(),
        reasoning_effort: request_config.reasoning_effort,
        supports_image_input: request_config.supports_image_input,
        openai_options: request_config.openai_options.clone(),
        prompt_cache_key: Some(session_id.clone()),
        websearch: websearch_config.clone(),
    };
    crate::agent::config::set_llm_session(llm_session.clone());
    let session_registration =
        crate::agent::config::set_llm_session_for(session_id.clone(), llm_session);
    let _session_config_guard = SessionConfigGuard(session_registration);

    let show_vlm_agent_hint = !request_config.supports_image_input
        && vlm_agent_has_model(&agent_registry)
        && messages_have_user_images(&messages);
    let text_only_image_turn =
        !request_config.supports_image_input && messages_have_user_images(&messages);

    if text_only_image_turn && !show_vlm_agent_hint {
        send_warning(
            &sender,
            "This model cannot receive images directly. Configure agent.vlm-agent.model to enable the built-in vision subagent."
                .to_string(),
        );
    }

    let aisdk_messages = convert_messages_for_model(
        &messages,
        request_config.supports_image_input,
        show_vlm_agent_hint,
    );

    let mut aisdk_tools = convert_to_aisdk_tools(
        &tool_registry,
        Some(sender.clone()),
        agent_mode,
        tool_permissions,
        Some(session_id.clone()),
        None,
        request_config.supports_image_input,
        cancel_token.clone(),
    )
    .await;
    if text_only_image_turn {
        aisdk_tools.retain(|tool| tool.name != "view_image");
    }
    let hosted_tools = resolve_websearch_tools(
        &request_config.provider_name,
        &websearch_config,
        &mut aisdk_tools,
    );

    let message_count = aisdk_messages.len();
    let tool_count = aisdk_tools.len();
    let primary_log_context = StreamLogContext::new(
        "primary",
        &request_config,
        message_count,
        tool_count,
        agent_max_steps,
    );
    log_stream_request(primary_log_context, &request_config);

    let mut response = stream_provider_request_with_hosted_tools(
        &request_config,
        aisdk_messages,
        aisdk_tools,
        hosted_tools,
        agent_max_steps,
        Some(cancel_token.clone()),
    )
    .await?;

    let start_time = Instant::now();
    let mut token_count: usize = 0;

    let relay_result = match relay_stream_to_sender(
        &mut response.stream,
        &cancel_token,
        &sender,
        &mut token_count,
        &start_time,
        primary_log_context,
    )
    .await
    .map_err(|err| err.to_string())
    {
        Ok(result) => result,
        Err(error) => {
            let stop_reason = response.stop_reason().await;
            log_stream_summary(
                primary_log_context,
                "Error",
                stop_reason.as_ref(),
                token_count,
                start_time.elapsed().as_millis(),
                None,
                Some(&error),
            );
            return Err(anyhow::anyhow!(error).into());
        }
    };

    let stop_reason = response.stop_reason().await;
    let stream_outcome = relay_result.outcome;
    let primary_outcome_label = stream_outcome_label(stream_outcome, stop_reason.as_ref());
    crate::emit_log!(
        "Stream completed: session_id={session_id} outcome={stream_outcome:?}, effective_outcome={primary_outcome_label}, stop_reason={stop_reason:?}, agent_max_steps={agent_max_steps:?}",
    );
    log_stream_summary(
        primary_log_context,
        primary_outcome_label,
        stop_reason.as_ref(),
        token_count,
        start_time.elapsed().as_millis(),
        Some(&relay_result.stats),
        None,
    );

    if stream_outcome == StreamRelayOutcome::Ended {
        return Ok(());
    }

    let hit_step_limit = reached_step_limit(agent_max_steps, &response).await;
    if !hit_step_limit {
        return Ok(());
    }

    send_warning(
        &sender,
        "Maximum configured steps reached. Sending text-only summary.",
    );

    let mut follow_up_messages = response.messages().await;
    follow_up_messages.push(AisdkMessage::assistant(MAX_STEPS_REACHED_PROMPT));
    let summary_message_count = follow_up_messages.len();
    let summary_log_context = StreamLogContext::new(
        "max_steps_summary",
        &request_config,
        summary_message_count,
        0,
        None,
    );
    log_stream_request(summary_log_context, &request_config);

    let mut summary_response = stream_provider_request(
        &request_config,
        follow_up_messages,
        Vec::new(),
        None,
        Some(cancel_token.clone()),
    )
    .await?;

    match relay_stream_to_sender(
        &mut summary_response.stream,
        &cancel_token,
        &sender,
        &mut token_count,
        &start_time,
        summary_log_context,
    )
    .await
    .map_err(|err| err.to_string())
    {
        Ok(result) => {
            let stop_reason = summary_response.stop_reason().await;
            log_stream_summary(
                summary_log_context,
                stream_outcome_label(result.outcome, stop_reason.as_ref()),
                stop_reason.as_ref(),
                token_count,
                start_time.elapsed().as_millis(),
                Some(&result.stats),
                None,
            );
        }
        Err(error) => {
            let stop_reason = summary_response.stop_reason().await;
            log_stream_summary(
                summary_log_context,
                "Error",
                stop_reason.as_ref(),
                token_count,
                start_time.elapsed().as_millis(),
                None,
                Some(&error),
            );
            return Err(anyhow::anyhow!(error).into());
        }
    }

    Ok(())
}

pub async fn configure_subagent_llm_session(
    provider_name: &str,
    model: String,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    sender: &crate::llm::ChunkSender,
) -> Result<(), DynError> {
    let session =
        build_subagent_llm_session(provider_name, model, reasoning_effort, sender).await?;
    crate::agent::config::set_llm_session(session);
    Ok(())
}

pub async fn build_subagent_llm_session(
    provider_name: &str,
    model: String,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    sender: &crate::llm::ChunkSender,
) -> Result<crate::agent::config::LlmSessionConfig, DynError> {
    let request_config =
        prepare_request_config(provider_name, model, reasoning_effort, sender).await?;
    Ok(crate::agent::config::LlmSessionConfig {
        provider_name: request_config.provider_name,
        model: request_config.model_name,
        api_key: request_config.api_key,
        provider_kind: match request_config.kind {
            ProviderKind::OpenAI => crate::agent::config::ProviderKind::OpenAI,
            ProviderKind::OpenAICompatible => crate::agent::config::ProviderKind::OpenAICompatible,
            ProviderKind::Anthropic => crate::agent::config::ProviderKind::Anthropic,
        },
        base_url: request_config.base_url,
        reasoning_effort: request_config.reasoning_effort,
        supports_image_input: request_config.supports_image_input,
        openai_options: request_config.openai_options,
        prompt_cache_key: None,
        websearch: crate::config::configuration::WebsearchConfig::default(),
    })
}

fn resolve_api_key(
    auth_config: Option<&crate::persistence::AuthConfig>,
    custom_provider_api_key: Option<String>,
) -> Option<String> {
    configured_api_key(auth_config).or(custom_provider_api_key)
}

pub async fn summarize_for_compaction(
    provider_name: String,
    model: String,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    prompt: String,
) -> Result<String, DynError> {
    let (warning_sender, _warning_receiver) = tokio::sync::mpsc::unbounded_channel();
    let request_config =
        prepare_request_config(&provider_name, model, reasoning_effort, &warning_sender).await?;
    let messages = vec![AisdkMessage::user(prompt)];
    let mut response =
        stream_provider_request(&request_config, messages, Vec::new(), None, None).await?;

    let mut summary = String::new();
    while let Some(chunk) = response.stream.next().await {
        match chunk {
            ChunkType::Text(text) => summary.push_str(&text),
            ChunkType::Failed(err) => {
                return Err(anyhow::anyhow!("Compaction failed: {}", err).into());
            }
            ChunkType::NotSupported(msg) => {
                return Err(anyhow::anyhow!("Compaction unsupported: {}", msg).into());
            }
            ChunkType::Reasoning(_)
            | ChunkType::ToolCall(_)
            | ChunkType::End { .. }
            | ChunkType::AssistantMessagePhase { .. }
            | ChunkType::ResponseCompleted { .. }
            | ChunkType::Retry(_)
            | ChunkType::RetryableFailure(_)
            | ChunkType::Warning(_)
            | ChunkType::Metadata(_)
            | ChunkType::Start
            | ChunkType::Incomplete(_) => {}
            ChunkType::StreamRollback { text, .. } => {
                if summary.ends_with(&text) {
                    summary.truncate(summary.len() - text.len());
                }
            }
        }
    }

    let summary = summary.trim().to_string();
    if summary.is_empty() {
        return Err(anyhow::anyhow!("Compaction returned an empty summary").into());
    }

    Ok(summary)
}

pub async fn generate_session_title(
    provider_name: String,
    model: String,
    user_message: String,
) -> Result<String, DynError> {
    let (warning_sender, _warning_receiver) = tokio::sync::mpsc::unbounded_channel();
    let request_config =
        prepare_request_config(&provider_name, model, None, &warning_sender).await?;
    let prompt = format!(
        "Generate a concise chat title for this user request.\n\nRules:\n- Return only the title, no quotes or punctuation wrapper.\n- 3 to 7 words.\n- Use title case only when natural.\n- Do not end with a period.\n\nUser request:\n{}",
        user_message.trim()
    );
    let messages = vec![AisdkMessage::user(prompt)];
    let mut response =
        stream_provider_request(&request_config, messages, Vec::new(), None, None).await?;

    let mut title = String::new();
    while let Some(chunk) = response.stream.next().await {
        match chunk {
            ChunkType::Text(text) => title.push_str(&text),
            ChunkType::Failed(err) => {
                return Err(anyhow::anyhow!("Title generation failed: {}", err).into());
            }
            ChunkType::NotSupported(msg) => {
                return Err(anyhow::anyhow!("Title generation unsupported: {}", msg).into());
            }
            ChunkType::Reasoning(_)
            | ChunkType::ToolCall(_)
            | ChunkType::End { .. }
            | ChunkType::AssistantMessagePhase { .. }
            | ChunkType::ResponseCompleted { .. }
            | ChunkType::Retry(_)
            | ChunkType::RetryableFailure(_)
            | ChunkType::Warning(_)
            | ChunkType::Metadata(_)
            | ChunkType::Start
            | ChunkType::Incomplete(_) => {}
            ChunkType::StreamRollback { text, .. } => {
                if title.ends_with(&text) {
                    title.truncate(title.len() - text.len());
                }
            }
        }
    }

    let title = sanitize_generated_title(&title);
    if title.is_empty() {
        return Err(anyhow::anyhow!("Title generation returned an empty title").into());
    }
    Ok(title)
}

fn sanitize_generated_title(raw: &str) -> String {
    let mut title = raw
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*' | '#' | ':' | '-' | '–' | '—'))
        .trim()
        .to_string();
    title = title.lines().next().unwrap_or("").trim().to_string();
    while title.ends_with('.') {
        title.pop();
        title = title.trim_end().to_string();
    }
    if title.chars().count() > 80 {
        title = title
            .chars()
            .take(80)
            .collect::<String>()
            .trim()
            .to_string();
    }
    title
}

async fn prepare_request_config(
    provider_name: &str,
    model: String,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    sender: &crate::llm::ChunkSender,
) -> Result<ProviderRequestConfig, DynError> {
    let auth_dao = crate::persistence::AuthDAO::new()?;
    let auth_config = auth_dao.get_provider(provider_name)?;

    let discovery = crate::model::discovery::Discovery::new()?;
    let custom_provider_api_key = discovery.custom_provider_api_key(provider_name);
    let provider = if let Some(provider) =
        crate::model::extensions::ModelExtensions::provider_for_request(provider_name)
    {
        provider
    } else {
        let providers = discovery.fetch_providers().await?;

        providers
            .get(provider_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?
    };

    let supports_image_input = model_supports_image_input(&model, provider.models.get(&model));
    let model_route = resolve_model_route(&provider, model);
    let provider_kind = ProviderKind::from_provider(provider_name, &model_route.npm_package);
    let base_url = if provider_name == "xai" && model_route.api.trim().is_empty() {
        "https://api.x.ai".to_string()
    } else {
        provider_kind.normalize_base_url(&model_route.api)
    };

    let mut request_config = ProviderRequestConfig::new(
        provider_kind,
        provider.name.clone(),
        base_url,
        model_route.model_name.clone(),
        resolve_api_key(auth_config.as_ref(), custom_provider_api_key),
        reasoning_effort,
        supports_image_input,
    );
    apply_provider_request_defaults(provider_name, &mut request_config);

    maybe_apply_openai_oauth_overrides(
        provider_name,
        &auth_dao,
        auth_config.as_ref(),
        &mut request_config,
        sender,
    )
    .await?;
    maybe_apply_xai_oauth_overrides(
        provider_name,
        &auth_dao,
        auth_config.as_ref(),
        &mut request_config,
        sender,
    )
    .await;

    maybe_apply_unauthenticated_free_provider_key(
        provider_name,
        provider.models.get(&model_route.model_name),
        &mut request_config,
    );

    if request_config.api_key.is_none()
        && !crate::model::extensions::ModelExtensions::is_runtime_provider(provider_name)
    {
        send_warning(
            sender,
            format!(
                "No API key configured for '{}'. Trying anyway.",
                provider_name
            ),
        );
    }

    crate::emit_log!(
        "Provider: {}, NPM: {}, Base URL: {}, Model: {}, Image Input: {}",
        provider_name,
        model_route.npm_package,
        request_config.base_url,
        request_config.model_name,
        if request_config.supports_image_input {
            "supported"
        } else {
            "unsupported"
        }
    );

    Ok(request_config)
}

fn apply_provider_request_defaults(
    provider_name: &str,
    request_config: &mut ProviderRequestConfig,
) {
    if provider_name == "xai" {
        // Ask xAI not to persist Responses, including subagent requests.
        request_config.openai_options.force_store_false = true;
    }
}

fn maybe_apply_unauthenticated_free_provider_key(
    provider_id: &str,
    model: Option<&crate::model::discovery::Model>,
    request_config: &mut ProviderRequestConfig,
) {
    if request_config.api_key.is_some()
        || !crate::model::extensions::ModelExtensions::is_unauthenticated_free_provider(provider_id)
    {
        return;
    }

    let Some(model) = model else {
        return;
    };

    if !matches!(model.status.as_deref(), Some("alpha" | "deprecated"))
        && model.cost.as_ref().is_some_and(|cost| cost.input == 0.0)
    {
        request_config.api_key = Some("public".to_string());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedModelRoute {
    npm_package: String,
    api: String,
    model_name: String,
}

fn resolve_model_route(
    provider: &crate::model::discovery::Provider,
    requested_model: String,
) -> ResolvedModelRoute {
    let model = provider.models.get(&requested_model);

    let npm_package = model
        .and_then(|model| model.provider.as_ref())
        .and_then(|provider| provider.npm.as_deref())
        .filter(|npm| !npm.trim().is_empty())
        .unwrap_or(provider.npm.as_str())
        .to_string();

    let api = model
        .and_then(|model| model.provider.as_ref())
        .and_then(|provider| provider.api.as_deref())
        .filter(|api| !api.trim().is_empty())
        .unwrap_or(provider.api.as_str())
        .to_string();

    let model_name = model
        .map(|model| model.id.as_str())
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(requested_model.as_str())
        .to_string();

    ResolvedModelRoute {
        npm_package,
        api,
        model_name,
    }
}

fn model_supports_image_input(
    requested_model: &str,
    model: Option<&crate::model::discovery::Model>,
) -> bool {
    if is_text_only_image_model(requested_model) {
        return false;
    }

    let Some(model) = model else {
        return false;
    };

    if is_text_only_image_model(&model.id) || is_text_only_image_model(&model.name) {
        return false;
    }

    if let Some(modalities) = model.modalities.as_ref() {
        return modalities.input.iter().any(|item| item == "image");
    }

    model.attachment
}

fn is_text_only_image_model(model: &str) -> bool {
    const TEXT_ONLY_IMAGE_MODELS: &[&str] = &[
        "gpt-5.3-codex-spark",
        "grok-composer-2.5-fast",
        "deepseek-v4-flash",
        "mimo-v2.5-pro",
        "mimo-2.5-pro",
    ];

    let normalized = model.trim().to_ascii_lowercase();
    let slug = normalized.replace([' ', '_'], "-");

    TEXT_ONLY_IMAGE_MODELS.iter().any(|model_id| {
        normalized == *model_id
            || normalized.ends_with(&format!("/{model_id}"))
            || slug == *model_id
            || slug.ends_with(&format!("/{model_id}"))
    })
}

fn vlm_agent_has_model(agent_registry: &crate::agent::definition::AgentRegistry) -> bool {
    agent_registry
        .task_target(crate::agent::definition::VLM_AGENT_NAME)
        .and_then(|agent| agent.model.as_deref())
        .is_some_and(|model| !model.trim().is_empty())
}

fn messages_have_user_images(messages: &[crate::session::types::Message]) -> bool {
    messages.iter().any(|message| {
        message.role == crate::session::types::MessageRole::User
            && !message.local_image_paths.is_empty()
    })
}

fn configured_api_key(auth_config: Option<&crate::persistence::AuthConfig>) -> Option<String> {
    auth_config.and_then(|config| match config {
        crate::persistence::AuthConfig::Api { key } => Some(key.clone()),
        crate::persistence::AuthConfig::Local => None,
        crate::persistence::AuthConfig::OAuth { access, .. } => Some(access.clone()),
    })
}

async fn maybe_apply_openai_oauth_overrides(
    provider_name: &str,
    auth_dao: &crate::persistence::AuthDAO,
    auth_config: Option<&crate::persistence::AuthConfig>,
    request_config: &mut ProviderRequestConfig,
    sender: &crate::llm::ChunkSender,
) -> Result<(), DynError> {
    if request_config.kind != ProviderKind::OpenAI || provider_name != "openai" {
        return Ok(());
    }

    let Some(crate::persistence::AuthConfig::OAuth {
        refresh,
        access,
        expires,
        account_id,
        enterprise_url,
    }) = auth_config.cloned()
    else {
        return Ok(());
    };

    let mut oauth_refresh = refresh;
    let mut oauth_access = access;
    let mut oauth_expires = expires;
    let mut oauth_account_id = account_id;
    let mut oauth_enterprise_url = enterprise_url;

    let now = crate::auth::openai_oauth::now_unix_ms();
    let refresh_required = oauth_expires <= now;
    if oauth_expires <= now + 60_000 {
        match crate::auth::openai_oauth::refresh_access_token(&oauth_refresh).await {
            Ok(refreshed) => {
                oauth_refresh = refreshed.refresh;
                oauth_access = refreshed.access;
                oauth_expires = refreshed.expires;

                if refreshed.account_id.is_some() {
                    oauth_account_id = refreshed.account_id;
                }
                if refreshed.enterprise_url.is_some() {
                    oauth_enterprise_url = refreshed.enterprise_url;
                }

                let _ = auth_dao.set_provider(
                    provider_name.to_string(),
                    crate::persistence::AuthConfig::OAuth {
                        refresh: oauth_refresh.clone(),
                        access: oauth_access.clone(),
                        expires: oauth_expires,
                        account_id: oauth_account_id.clone(),
                        enterprise_url: oauth_enterprise_url.clone(),
                    },
                );
            }
            Err(err) => {
                if refresh_required {
                    return Err(anyhow::anyhow!(
                        "OpenAI OAuth token refresh failed: {}. Please reconnect OpenAI OAuth.",
                        err
                    )
                    .into());
                }
                send_warning(
                    sender,
                    format!("Failed to refresh OpenAI OAuth token: {}", err),
                );
            }
        }
    }

    request_config.api_key = Some(oauth_access.clone());
    request_config.base_url = "https://chatgpt.com".to_string();

    request_config.openai_options.response_path = Some("/backend-api/codex/responses".to_string());
    request_config.openai_options.force_store_false = true;
    request_config.openai_options.default_instructions =
        Some("You are Codex, a coding assistant focused on high-quality code changes.".to_string());
    request_config.openai_options.disallow_system_messages = true;
    request_config.openai_options.force_tool_strict_false = true;

    request_config
        .openai_options
        .additional_headers
        .insert("originator".to_string(), "crabcode".to_string());
    request_config.openai_options.additional_headers.insert(
        "User-Agent".to_string(),
        crate::auth::openai_oauth::build_user_agent(),
    );

    if let Some(account_id) = oauth_account_id {
        request_config
            .openai_options
            .additional_headers
            .insert("ChatGPT-Account-Id".to_string(), account_id);
    }

    crate::emit_log!("Configured OpenAI OAuth Codex transport");

    if !is_openai_oauth_model_allowed(&request_config.model_name) {
        let fallback_model = "gpt-5.3-codex".to_string();
        send_warning(
            sender,
            format!(
                "Model '{}' is not supported for OpenAI OAuth. Falling back to '{}'.",
                request_config.model_name, fallback_model
            ),
        );
        request_config.model_name = fallback_model;
    }

    Ok(())
}

async fn maybe_apply_xai_oauth_overrides(
    provider_name: &str,
    auth_dao: &crate::persistence::AuthDAO,
    auth_config: Option<&crate::persistence::AuthConfig>,
    request_config: &mut ProviderRequestConfig,
    sender: &crate::llm::ChunkSender,
) {
    if request_config.kind != ProviderKind::OpenAI || provider_name != "xai" {
        return;
    }

    let Some(crate::persistence::AuthConfig::OAuth {
        refresh,
        access,
        expires,
        account_id,
        enterprise_url,
    }) = auth_config.cloned()
    else {
        return;
    };

    let mut oauth_refresh = refresh;
    let mut oauth_access = access;
    let mut oauth_expires = expires;
    let oauth_account_id = account_id;
    let oauth_enterprise_url = enterprise_url;

    let expires_soon = oauth_expires <= crate::auth::xai_oauth::now_unix_ms() + 120_000
        || crate::auth::xai_oauth::access_token_is_expiring(&oauth_access);

    if expires_soon {
        match crate::auth::xai_oauth::refresh_access_token(&oauth_refresh).await {
            Ok(refreshed) => {
                oauth_refresh = refreshed.refresh;
                oauth_access = refreshed.access;
                oauth_expires = refreshed.expires;

                let _ = auth_dao.set_provider(
                    provider_name.to_string(),
                    crate::persistence::AuthConfig::OAuth {
                        refresh: oauth_refresh.clone(),
                        access: oauth_access.clone(),
                        expires: oauth_expires,
                        account_id: oauth_account_id.clone(),
                        enterprise_url: oauth_enterprise_url.clone(),
                    },
                );
            }
            Err(err) => {
                send_warning(
                    sender,
                    format!("Failed to refresh xAI OAuth token: {}", err),
                );
            }
        }
    }

    let overrides = super::xai_build::request_overrides(oauth_access).await;
    request_config.api_key = Some(overrides.api_key);
    request_config.base_url = overrides.base_url.to_string();
    request_config.model_name = overrides.model.to_string();
    request_config.openai_options.force_store_false = true;
    request_config
        .openai_options
        .additional_headers
        .extend(overrides.headers);

    crate::emit_log!("Configured xAI Grok Build OAuth transport");
}

fn send_warning(sender: &crate::llm::ChunkSender, warning: impl Into<String>) {
    let _ = sender.send(crate::llm::ChunkMessage::Warning(warning.into()));
}

fn provider_supports_native_websearch(provider_name: &str) -> bool {
    matches!(
        provider_name.trim().to_ascii_lowercase().as_str(),
        "openai" | "xai" | "openrouter"
    )
}

pub(crate) fn resolve_websearch_tools(
    provider_name: &str,
    websearch: &crate::config::configuration::WebsearchConfig,
    tools: &mut Vec<Tool>,
) -> Vec<HostedTool> {
    use crate::config::configuration::WebsearchStrategy;

    if websearch.enabled == Some(false) || !tools.iter().any(|tool| tool.name == "websearch") {
        return Vec::new();
    }

    let use_native = match websearch.strategy {
        WebsearchStrategy::NativeFirst | WebsearchStrategy::NativeOnly => {
            provider_supports_native_websearch(provider_name)
        }
        WebsearchStrategy::ExternalOnly => false,
    };

    if use_native {
        tools.retain(|tool| tool.name != "websearch");
        vec![HostedTool::WebSearch]
    } else if websearch.strategy == WebsearchStrategy::NativeOnly {
        tools.retain(|tool| tool.name != "websearch");
        Vec::new()
    } else {
        Vec::new()
    }
}

async fn stream_provider_request(
    config: &ProviderRequestConfig,
    messages: Vec<AisdkMessage>,
    tools: Vec<Tool>,
    max_steps: Option<usize>,
    cancel_token: Option<CancellationToken>,
) -> Result<StreamTextResponse, DynError> {
    stream_provider_request_with_hosted_tools(
        config,
        messages,
        tools,
        Vec::new(),
        max_steps,
        cancel_token,
    )
    .await
}

async fn stream_provider_request_with_hosted_tools(
    config: &ProviderRequestConfig,
    messages: Vec<AisdkMessage>,
    tools: Vec<Tool>,
    hosted_tools: Vec<HostedTool>,
    max_steps: Option<usize>,
    cancel_token: Option<CancellationToken>,
) -> Result<StreamTextResponse, DynError> {
    let headers = HashMap::new();
    match config.kind {
        ProviderKind::OpenAICompatible => {
            let mut builder = OpenAICompatible::builder()
                .base_url(&config.base_url)
                .model_name(&config.model_name)
                .provider_name(&config.provider_name);
            if let Some(effort) = config.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            if let Some(key) = config.api_key.as_deref() {
                builder = builder.api_key(key);
            }
            if let Some(cache_key) = config.openai_options.prompt_cache_key.as_deref() {
                builder = builder.prompt_cache_key(cache_key);
            }
            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_hosted_tools(
                provider,
                messages,
                tools,
                hosted_tools,
                max_steps,
                None,
                headers,
                cancel_token,
            )
            .await
            .map_err(|e| Box::new(e) as DynError)
        }
        ProviderKind::Anthropic => {
            let mut builder = Anthropic::builder()
                .base_url(&config.base_url)
                .model_name(&config.model_name)
                .provider_name(&config.provider_name);
            if let Some(effort) = config.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            if let Some(key) = config.api_key.as_deref() {
                builder = builder.api_key(key);
            }
            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_hosted_tools(
                provider,
                messages,
                tools,
                hosted_tools,
                max_steps,
                None,
                headers,
                cancel_token,
            )
            .await
            .map_err(|e| Box::new(e) as DynError)
        }
        ProviderKind::OpenAI => {
            let mut builder = OpenAI::builder()
                .base_url(&config.base_url)
                .model_name(&config.model_name)
                .provider_name(&config.provider_name);
            if let Some(effort) = config.reasoning_effort {
                builder = builder.reasoning_effort(effort.as_str());
            }
            if let Some(key) = config.api_key.as_deref() {
                builder = builder.api_key(key);
            }

            if let Some(responses_path) = &config.openai_options.response_path {
                builder = builder.responses_path(responses_path);
            }
            if config.openai_options.force_store_false {
                builder = builder.store_override(false);
            }
            if let Some(instructions) =
                openai_request_instructions(&config.openai_options, &messages)
            {
                builder = builder.default_instructions(instructions);
            }
            if config.openai_options.disallow_system_messages {
                builder = builder.strip_system_and_developer_messages(true);
            }
            if config.openai_options.force_tool_strict_false {
                builder = builder.tool_strict_override(false);
            }
            if config.openai_options.disallow_system_messages {
                builder = builder.responses_websocket(true);
            }
            if let Some(cache_key) = config.openai_options.prompt_cache_key.as_deref() {
                builder = builder.prompt_cache_key(cache_key);
            }
            if !config.openai_options.additional_headers.is_empty() {
                builder = builder.headers(config.openai_options.additional_headers.clone());
            }
            if let Some(policy) =
                super::xai_build::retry_policy_for(&config.openai_options.additional_headers)
            {
                builder = builder.response_retry_policy(policy);
            }

            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_hosted_tools(
                provider,
                messages,
                tools,
                hosted_tools,
                max_steps,
                None,
                headers,
                cancel_token,
            )
            .await
            .map_err(|e| Box::new(e) as DynError)
        }
    }
}

fn openai_request_instructions(
    options: &OpenAIRequestOptions,
    messages: &[AisdkMessage],
) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(instructions) = options
        .default_instructions
        .as_deref()
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
    {
        parts.push(instructions.to_string());
    }

    if options.disallow_system_messages {
        parts.extend(messages.iter().filter_map(|message| {
            let AisdkMessage::System(system) = message else {
                return None;
            };

            let content = system.content.trim();
            (!content.is_empty()).then(|| content.to_string())
        }));
    }

    (!parts.is_empty()).then(|| parts.join("\n\n---\n\n"))
}

fn log_stream_request(context: StreamLogContext<'_>, config: &ProviderRequestConfig) {
    if !crate::logging::enabled() {
        return;
    }

    let reasoning_effort = config
        .reasoning_effort
        .map(|effort| effort.as_str())
        .unwrap_or("none");
    let mut header_names = config
        .openai_options
        .additional_headers
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    header_names.sort_unstable();
    crate::emit_log!(
        "[STREAM_REQUEST] {} reasoning_effort={} responses_path={:?} force_store_false={} prompt_cache_key={} disallow_system_messages={} force_tool_strict_false={} extra_header_names=[{}]",
        context.describe(),
        reasoning_effort,
        config.openai_options.response_path,
        config.openai_options.force_store_false,
        config
            .openai_options
            .prompt_cache_key
            .as_deref()
            .unwrap_or("-"),
        config.openai_options.disallow_system_messages,
        config.openai_options.force_tool_strict_false,
        header_names.join(","),
    );
}

fn log_stream_summary(
    context: StreamLogContext<'_>,
    relay_result: &str,
    stop_reason: Option<&StopReason>,
    token_count: usize,
    elapsed_ms: u128,
    stats: Option<&RelayStats>,
    error: Option<&str>,
) {
    if !crate::logging::enabled() {
        return;
    }

    let stats = stats
        .map(|stats| stats.describe_at(Some(elapsed_ms)))
        .unwrap_or_else(|| "chunks=unavailable".to_string());
    let error = error
        .map(|err| format!(" error={}", err))
        .unwrap_or_default();
    crate::emit_log!(
        "[STREAM_SUMMARY] {} relay_result={} stop_reason={:?} token_estimate={} elapsed_ms={} {}{}",
        context.describe(),
        relay_result,
        stop_reason,
        token_count,
        elapsed_ms,
        stats,
        error,
    );
}

fn is_transport_or_request_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    ((lower.contains("sse error") || lower.contains("sse transport error"))
        && (lower.contains("transport")
            || lower.contains("decoding response body")
            || lower.contains("body")))
        || (lower.contains("request error")
            && (lower.contains("is_timeout=true")
                || lower.contains("is_connect=true")
                || lower.contains("error sending request")))
        || lower.contains("http error: error sending request")
}

async fn relay_stream_to_sender(
    stream: &mut LanguageModelStream,
    cancel_token: &CancellationToken,
    sender: &crate::llm::ChunkSender,
    token_count: &mut usize,
    start_time: &Instant,
    context: StreamLogContext<'_>,
) -> Result<StreamRelayResult, DynError> {
    let mut stats = RelayStats::default();
    crate::emit_log!(
        "[RELAY] relay_stream_to_sender started {}",
        context.describe()
    );
    loop {
        let chunk = tokio::select! {
            _ = cancel_token.cancelled() => {
                let elapsed_ms = start_time.elapsed().as_millis();
                let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
                crate::emit_log!(
                    "[STREAM_CANCELLED] {} elapsed_ms={} token_estimate={} {}",
                    context.describe(),
                    elapsed_ms,
                    *token_count,
                    stats.describe_at(Some(elapsed_ms)),
                );
                return Err(anyhow::anyhow!("Streaming cancelled by user").into());
            }
            chunk = stream.next() => chunk,
        };

        let chunk = match chunk {
            Some(c) => c,
            None => break,
        };

        match chunk {
            ChunkType::Text(text) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Text", elapsed_ms);
                stats.text_chunks += 1;
                stats.text_chars += text.len();
                stats.record_text(text.len(), elapsed_ms);
                *token_count += estimate_tokens(&text);
                crate::emit_log!("[RELAY] Text chunk ({} chars)", text.len());
                let _ = sender.send(crate::llm::ChunkMessage::Text(text));
            }
            ChunkType::Reasoning(reasoning) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Reasoning", elapsed_ms);
                stats.reasoning_chunks += 1;
                stats.reasoning_chars += reasoning.len();
                *token_count += estimate_tokens(&reasoning);
                crate::emit_log!("[RELAY] Reasoning chunk ({} chars)", reasoning.len());
                let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
            }
            ChunkType::ToolCall(tool_call) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("ToolCall", elapsed_ms);
                stats.tool_call_chunks += 1;
                stats.tool_call_bytes += tool_call.len();
                let info = tool_call_log_info(&tool_call);
                stats.record_tool_call(&info, elapsed_ms);
                crate::emit_log!(
                    "[RELAY] ToolCall chunk received names={} ids={} arg_chars={} arg_done_chars={} bytes={}",
                    info.names_label(),
                    info.ids_label(),
                    info.argument_chars,
                    info.arguments_done_chars,
                    tool_call.len(),
                );
            }
            ChunkType::End { reason } => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("End", elapsed_ms);
                let reason = reason
                    .as_ref()
                    .map(|reason| reason.label())
                    .unwrap_or("unknown");
                crate::emit_log!(
                    "[RELAY] End chunk reason={reason} — returning Ended {}",
                    stats.describe_at(Some(elapsed_ms)),
                );
                let duration_ms = elapsed_ms as u64;
                let _ = sender.send(crate::llm::ChunkMessage::Metrics {
                    token_count: *token_count,
                    duration_ms,
                });
                let _ = sender.send(crate::llm::ChunkMessage::End);
                return Ok(StreamRelayResult {
                    outcome: StreamRelayOutcome::Ended,
                    stats,
                });
            }
            ChunkType::ResponseCompleted { end_turn } => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("ResponseCompleted", elapsed_ms);
                stats.response_completed_chunks += 1;
                crate::emit_log!(
                    "[RELAY] ResponseCompleted chunk end_turn={end_turn:?} — returning Ended {}",
                    stats.describe_at(Some(elapsed_ms))
                );
                let duration_ms = elapsed_ms as u64;
                let _ = sender.send(crate::llm::ChunkMessage::Metrics {
                    token_count: *token_count,
                    duration_ms,
                });
                let _ = sender.send(crate::llm::ChunkMessage::End);
                return Ok(StreamRelayResult {
                    outcome: StreamRelayOutcome::Ended,
                    stats,
                });
            }
            ChunkType::AssistantMessagePhase { phase } => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("AssistantMessagePhase", elapsed_ms);
                stats.record_assistant_phase(phase);
                crate::emit_log!("[RELAY] AssistantMessagePhase chunk phase={phase:?}");
            }
            ChunkType::Metadata(message) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Metadata", elapsed_ms);
                stats.record_metadata(&message);
                crate::emit_log!("[RELAY] Metadata {}", message);
            }
            ChunkType::Retry(status) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Retry", elapsed_ms);
                crate::emit_log!(
                    "[RELAY] Retry attempt={} delay_ms={} next_epoch_ms={} message={}",
                    status.attempt,
                    status.delay_ms,
                    status.next_epoch_ms,
                    status.message,
                );
                let _ = sender.send(crate::llm::ChunkMessage::Retry(status));
            }
            ChunkType::StreamRollback { text, reasoning } => {
                let rolled_back_tokens = estimate_tokens(&text) + estimate_tokens(&reasoning);
                *token_count = token_count.saturating_sub(rolled_back_tokens);
                crate::emit_log!(
                    "[RELAY] StreamRollback text_chars={} reasoning_chars={}",
                    text.len(),
                    reasoning.len(),
                );
                let _ = sender.send(crate::llm::ChunkMessage::StreamRollback { text, reasoning });
            }
            ChunkType::Warning(message) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Warning", elapsed_ms);
                crate::emit_log!("[RELAY] Warning {}", message);
                let _ = sender.send(crate::llm::ChunkMessage::Warning(message));
            }
            ChunkType::Start => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Start", elapsed_ms);
                stats.start_chunks += 1;
                crate::emit_log!("[RELAY] Start chunk received");
            }
            ChunkType::RetryableFailure(retry_error) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_failed_chunk();
                let err = retry_error.message.clone();
                let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                crate::emit_log!("Stream Chunk RetryableFailure without retry loop {}", err);
                crate::emit_log!(
                    "[STREAM_ERROR] {} elapsed_ms={} token_estimate={} {} error={}",
                    context.describe(),
                    elapsed_ms,
                    *token_count,
                    stats.describe_at(Some(elapsed_ms)),
                    err,
                );
                return Err(anyhow::anyhow!("Streaming failed: {}", err).into());
            }
            ChunkType::Failed(err) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_failed_chunk();
                let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                crate::emit_log!("Stream Chunk Failed {}", err);
                crate::emit_log!(
                    "[STREAM_ERROR] {} elapsed_ms={} token_estimate={} {} error={}",
                    context.describe(),
                    elapsed_ms,
                    *token_count,
                    stats.describe_at(Some(elapsed_ms)),
                    err,
                );
                if is_transport_or_request_error(&err) {
                    crate::emit_log!("[STREAM_ERROR_HINT] Request/stream transport failure. This happened below the model layer while sending or reading provider HTTP data; if it repeats, compare network/proxy/VPN state and provider status with the request and provider_step context above.");
                }
                return Err(anyhow::anyhow!("Streaming failed: {}", err).into());
            }
            ChunkType::Incomplete(msg) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Incomplete", elapsed_ms);
                stats.incomplete_chunks += 1;
                crate::emit_log!("[RELAY] Incomplete chunk received: {}", msg);
            }
            ChunkType::NotSupported(msg) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("NotSupported", elapsed_ms);
                stats.not_supported_chunks += 1;
                crate::emit_log!("[RELAY] NotSupported chunk received: {}", msg);
            }
        }
    }

    let elapsed_ms = start_time.elapsed().as_millis();
    crate::emit_log!(
        "[RELAY] stream exhausted — returning Exhausted {} token_estimate={} {}",
        context.describe(),
        *token_count,
        stats.describe_at(Some(elapsed_ms)),
    );
    Ok(StreamRelayResult {
        outcome: StreamRelayOutcome::Exhausted,
        stats,
    })
}

async fn reached_step_limit(agent_max_steps: Option<usize>, response: &StreamTextResponse) -> bool {
    agent_max_steps.is_some() && matches!(response.stop_reason().await, Some(StopReason::Hook))
}

fn estimate_tokens(content: &str) -> usize {
    content.chars().count().max(1) / 4
}

fn convert_messages(messages: &[crate::session::types::Message]) -> Vec<AisdkMessage> {
    convert_messages_for_model(messages, true, false)
}

fn convert_messages_for_model(
    messages: &[crate::session::types::Message],
    supports_image_input: bool,
    show_vlm_agent_hint: bool,
) -> Vec<AisdkMessage> {
    let mut aisdk_messages = Vec::new();

    for msg in messages {
        if crate::session::compaction::is_compaction_marker(msg) {
            continue;
        }

        match msg.role {
            crate::session::types::MessageRole::System => {
                aisdk_messages.push(AisdkMessage::system(
                    crate::utils::sanitize::strip_legacy_image_descriptions(&msg.content),
                ));
            }
            crate::session::types::MessageRole::User => {
                let content = crate::utils::sanitize::strip_legacy_image_descriptions(&msg.content);
                if !supports_image_input && !msg.local_image_paths.is_empty() {
                    if show_vlm_agent_hint {
                        aisdk_messages.push(AisdkMessage::user(content_with_vlm_agent_hint(
                            &content,
                            &msg.local_image_paths,
                        )));
                    } else {
                        aisdk_messages.push(AisdkMessage::user(
                            content_with_unsupported_image_note(
                                &content,
                                msg.local_image_paths.len(),
                            ),
                        ));
                    }
                    continue;
                }

                let images = msg
                    .local_image_paths
                    .iter()
                    .filter_map(|path| {
                        let path = std::path::Path::new(path);
                        match crate::utils::image_attachment::prompt_image_for_path(path, false) {
                            Ok(image) => Some(ImageContent {
                                data_url: image.data_url,
                                media_type: image.media_type,
                            }),
                            Err(err) => {
                                crate::emit_log!(
                                    "failed to attach image {}: {}",
                                    path.display(),
                                    err
                                );
                                None
                            }
                        }
                    })
                    .collect::<Vec<_>>();

                if images.is_empty() {
                    aisdk_messages.push(AisdkMessage::user(content));
                } else {
                    aisdk_messages.push(AisdkMessage::user_with_images(content, images));
                }
            }
            crate::session::types::MessageRole::Assistant => {
                if msg.parts.iter().any(|part| {
                    matches!(
                        part.part_type.as_str(),
                        "text" | "reasoning" | "tool_call" | "tool_result"
                    )
                }) {
                    append_assistant_parts_for_model(
                        &mut aisdk_messages,
                        msg,
                        supports_image_input,
                    );
                } else if !msg.content.trim().is_empty() {
                    let content =
                        crate::utils::sanitize::strip_legacy_image_descriptions(&msg.content);
                    if !content.trim().is_empty() {
                        aisdk_messages.push(AisdkMessage::assistant(content));
                    }
                }
            }
            crate::session::types::MessageRole::Tool => {
                if let Some(tool_messages) =
                    tool_messages_for_model(&msg.content, supports_image_input)
                {
                    aisdk_messages.extend(tool_messages);
                } else {
                    aisdk_messages.push(AisdkMessage::user(tool_message_observation(&msg.content)));
                }
            }
        }
    }

    aisdk_messages
}

fn append_assistant_parts_for_model(
    aisdk_messages: &mut Vec<AisdkMessage>,
    msg: &crate::session::types::Message,
    supports_image_input: bool,
) {
    let mut emitted_text = false;
    let mut pending_tools = AssistantToolReplayGroup::default();

    for part in &msg.parts {
        match part.part_type.as_str() {
            "text" => {
                pending_tools.flush_complete_pairs(aisdk_messages);

                let Some(text) = part.text_value().filter(|text| !text.trim().is_empty()) else {
                    continue;
                };

                let text = crate::utils::sanitize::strip_legacy_image_descriptions(text);
                if text.trim().is_empty() {
                    continue;
                }

                emitted_text = true;
                aisdk_messages.push(AisdkMessage::assistant(text));
            }
            "tool_call" => {
                if pending_tools.is_complete() {
                    pending_tools.flush_complete_pairs(aisdk_messages);
                }

                let Some(obj) = part.data.as_object() else {
                    continue;
                };
                if let Some(message) = tool_call_message_from_model_obj(obj) {
                    if let Some(id) = part.tool_id() {
                        pending_tools.add_call(id.to_string(), message);
                    }
                }
            }
            "tool_result" => {
                let Some(obj) = part.data.as_object() else {
                    continue;
                };

                let Some(id) = part.tool_id().map(str::to_string) else {
                    continue;
                };

                if !pending_tools.has_call(&id) {
                    if pending_tools.is_complete() {
                        pending_tools.flush_complete_pairs(aisdk_messages);
                    }

                    if let Some(call) = tool_call_message_from_model_obj(obj) {
                        pending_tools.add_call(id.clone(), call);
                    }
                }

                if let Some(output) = tool_output_message_from_model_obj(obj, supports_image_input)
                {
                    pending_tools.add_output(id, output);
                }

                if pending_tools.is_complete() {
                    pending_tools.flush_complete_pairs(aisdk_messages);
                }
            }
            _ => {}
        }
    }

    pending_tools.flush_complete_pairs(aisdk_messages);

    if !emitted_text && msg.parts.is_empty() && !msg.content.trim().is_empty() {
        aisdk_messages.push(AisdkMessage::assistant(msg.content.clone()));
    }
}

#[derive(Default)]
struct AssistantToolReplayGroup {
    calls: Vec<(String, AisdkMessage)>,
    outputs: HashMap<String, AisdkMessage>,
}

impl AssistantToolReplayGroup {
    fn has_call(&self, id: &str) -> bool {
        self.calls.iter().any(|(call_id, _)| call_id == id)
    }

    fn is_complete(&self) -> bool {
        !self.calls.is_empty()
            && self
                .calls
                .iter()
                .all(|(call_id, _)| self.outputs.contains_key(call_id))
    }

    fn add_call(&mut self, id: String, call: AisdkMessage) {
        if !self.has_call(&id) {
            self.calls.push((id, call));
        }
    }

    fn add_output(&mut self, id: String, output: AisdkMessage) {
        self.outputs.insert(id, output);
    }

    fn flush_complete_pairs(&mut self, aisdk_messages: &mut Vec<AisdkMessage>) {
        let mut calls = Vec::new();
        let mut outputs = Vec::new();

        for (call_id, call) in &self.calls {
            if let Some(output) = self.outputs.remove(call_id) {
                calls.push(call.clone());
                outputs.push(output);
            }
        }

        aisdk_messages.extend(calls);
        aisdk_messages.extend(outputs);
        self.calls.clear();
        self.outputs.clear();
    }
}

fn tool_messages_for_model(content: &str, supports_image_input: bool) -> Option<Vec<AisdkMessage>> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let obj = value.as_object()?;

    Some(vec![
        tool_call_message_from_model_obj(obj)?,
        tool_output_message_from_model_obj(obj, supports_image_input)?,
    ])
}

fn tool_call_message_from_model_obj(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<AisdkMessage> {
    let call_id = obj
        .get("id")
        .or_else(|| obj.get("call_id"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())?;

    let arguments = obj
        .get("args")
        .map(|args| serde_json::to_string(args).unwrap_or_else(|_| args.to_string()))
        .unwrap_or_else(|| "{}".to_string());

    Some(AisdkMessage::tool_call(call_id, name, arguments))
}

fn tool_output_message_from_model_obj(
    obj: &serde_json::Map<String, serde_json::Value>,
    supports_image_input: bool,
) -> Option<AisdkMessage> {
    let call_id = obj
        .get("id")
        .or_else(|| obj.get("call_id"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())?;
    let output = obj
        .get("output_preview")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())?;

    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("ok");
    let is_error = status.eq_ignore_ascii_case("error");

    let images = if name == "view_image" && !is_error && supports_image_input {
        view_image_tool_images(obj)
    } else {
        Vec::new()
    };
    let output = if name == "view_image" && !is_error && !supports_image_input {
        content_with_unsupported_image_note(output, 1)
    } else {
        crate::utils::sanitize::strip_legacy_image_descriptions(output)
    };

    Some(AisdkMessage::tool_output_with_images(
        call_id, name, output, images, is_error,
    ))
}

fn content_with_unsupported_image_note(content: &str, image_count: usize) -> String {
    let image_label = if image_count == 1 { "image" } else { "images" };
    let note = format!(
        "ERROR: Cannot read {image_label} (this model does not support image input). Inform the user."
    );

    if content.trim().is_empty() {
        note
    } else {
        format!("{content}\n\n{note}")
    }
}

fn view_image_tool_images(obj: &serde_json::Map<String, serde_json::Value>) -> Vec<ImageContent> {
    let path = obj
        .get("metadata")
        .and_then(|metadata| metadata.get("path"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            obj.get("args")
                .and_then(|args| args.get("path"))
                .and_then(|value| value.as_str())
        });
    let Some(path) = path else {
        return Vec::new();
    };

    let preserve_original = obj
        .get("metadata")
        .and_then(|metadata| metadata.get("detail"))
        .and_then(|value| value.as_str())
        .map(|detail| detail == "original")
        .unwrap_or(false);

    match crate::utils::image_attachment::prompt_image_for_path(
        std::path::Path::new(path),
        preserve_original,
    ) {
        Ok(image) => vec![ImageContent {
            data_url: image.data_url,
            media_type: image.media_type,
        }],
        Err(err) => {
            crate::emit_log!(
                "failed to reattach viewed image {} from tool history: {}",
                path,
                err
            );
            Vec::new()
        }
    }
}

fn tool_message_observation(content: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return format!("Tool result:\n{}", content);
    };

    let Some(obj) = value.as_object() else {
        return format!("Tool result:\n{}", content);
    };

    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("ok");
    let title = obj.get("title").and_then(|v| v.as_str());
    let output = obj
        .get("output_preview")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("");

    let mut observation = format!("Tool `{}` result ({})", name, status);
    if let Some(title) = title {
        observation.push_str(&format!(": {}", title));
    }
    if let Some(args) = obj.get("args") {
        push_tool_arguments_for_observation(&mut observation, args);
    }
    if !output.is_empty() {
        observation.push_str("\n\nTool output:\n");
        observation.push_str(output);
    }

    observation
}

fn push_tool_arguments_for_observation(out: &mut String, args: &serde_json::Value) {
    out.push_str("\n\nTool call arguments:\n```json\n");
    out.push_str(&truncate_for_tool_observation(
        &serde_json::to_string(args).unwrap_or_else(|_| args.to_string()),
        TOOL_HISTORY_ARGUMENTS_MAX_CHARS,
    ));
    out.push_str("\n```");
}

fn truncate_for_tool_observation(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}\n[truncated]", truncated)
    } else {
        truncated
    }
}

fn is_openai_oauth_model_allowed(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.contains("codex") || is_openai_oauth_gpt5_model(&model)
}

fn is_openai_oauth_gpt5_model(model: &str) -> bool {
    let model = model.strip_prefix("openai/").unwrap_or(model);
    if model.contains("-chat") {
        return false;
    }

    model == "gpt-5" || model.starts_with("gpt-5.") || model.starts_with("gpt-5-")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderKind {
    OpenAI,
    OpenAICompatible,
    Anthropic,
}

impl ProviderKind {
    fn from_provider(_provider_name: &str, npm_package: &str) -> Self {
        match npm_package {
            "@ai-sdk/openai-compatible" => Self::OpenAICompatible,
            "@ai-sdk/anthropic" => Self::Anthropic,
            _ => Self::OpenAI,
        }
    }

    fn normalize_base_url(self, base_url: &str) -> String {
        match self {
            ProviderKind::Anthropic => normalize_anthropic_base_url(base_url),
            ProviderKind::OpenAI => {
                if base_url.trim().is_empty() {
                    "https://api.openai.com".to_string()
                } else {
                    base_url.to_string()
                }
            }
            _ => base_url.to_string(),
        }
    }
}

fn normalize_anthropic_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.trim_end_matches("/v1").to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_provider_request_defaults, convert_messages, convert_messages_for_model,
        is_openai_oauth_model_allowed, maybe_apply_unauthenticated_free_provider_key,
        model_supports_image_input, openai_request_instructions, resolve_api_key,
        resolve_model_route, resolve_websearch_tools, vlm_agent_has_model, AisdkMessage,
        OpenAIRequestOptions, ProviderKind, ProviderRequestConfig,
    };

    use crate::persistence::AuthConfig;

    #[test]
    fn stored_auth_takes_precedence_over_custom_provider_api_key() {
        assert_eq!(
            resolve_api_key(
                Some(&AuthConfig::Api {
                    key: "stored-key".to_string(),
                }),
                Some("config-key".to_string()),
            )
            .as_deref(),
            Some("stored-key")
        );
        assert_eq!(
            resolve_api_key(Some(&AuthConfig::Local), Some("config-key".to_string())).as_deref(),
            Some("config-key")
        );
    }

    #[test]
    fn openai_oauth_instructions_preserve_stripped_system_prompt() {
        let options = OpenAIRequestOptions {
            default_instructions: Some("base codex instructions".to_string()),
            disallow_system_messages: true,
            ..OpenAIRequestOptions::default()
        };
        let messages = vec![
            AisdkMessage::system("rich system prompt with AGENTS.md"),
            AisdkMessage::user("Go ahead"),
        ];

        let instructions = openai_request_instructions(&options, &messages)
            .expect("instructions should be present");

        assert!(instructions.contains("base codex instructions"));
        assert!(instructions.contains("rich system prompt with AGENTS.md"));
    }

    #[test]
    fn openai_instructions_do_not_duplicate_system_when_not_stripping() {
        let options = OpenAIRequestOptions {
            default_instructions: Some("base codex instructions".to_string()),
            disallow_system_messages: false,
            ..OpenAIRequestOptions::default()
        };
        let messages = vec![AisdkMessage::system("system stays in input")];

        assert_eq!(
            openai_request_instructions(&options, &messages).as_deref(),
            Some("base codex instructions")
        );
    }

    #[test]
    fn openai_oauth_allows_versioned_gpt5_models() {
        assert!(is_openai_oauth_model_allowed("gpt-5.4"));
        assert!(is_openai_oauth_model_allowed("gpt-5.5"));
        assert!(is_openai_oauth_model_allowed("openai/gpt-5.6"));
    }

    #[test]
    fn openai_oauth_allows_codex_named_models() {
        assert!(is_openai_oauth_model_allowed("gpt-5.3-codex"));
        assert!(is_openai_oauth_model_allowed("codex-mini-latest"));
    }

    #[test]
    fn openai_oauth_rejects_known_non_codex_chat_models() {
        assert!(!is_openai_oauth_model_allowed("gpt-5-chat-latest"));
        assert!(!is_openai_oauth_model_allowed("gpt-4o"));
    }

    #[test]
    fn model_provider_override_selects_anthropic_route() {
        let provider: crate::model::discovery::Provider =
            serde_json::from_value(serde_json::json!({
                "id": "opencode-go",
                "name": "OpenCode Go",
                "api": "https://opencode.ai/zen/go/v1",
                "npm": "@ai-sdk/openai-compatible",
                "env": ["OPENCODE_API_KEY"],
                "models": {
                    "qwen3.7-max": {
                        "id": "qwen3.7-max",
                        "name": "Qwen3.7 Max",
                        "release_date": "2026-05-21",
                        "last_updated": "2026-05-21",
                        "provider": {
                            "npm": "@ai-sdk/anthropic"
                        }
                    }
                }
            }))
            .unwrap();

        let route = resolve_model_route(&provider, "qwen3.7-max".to_string());
        assert_eq!(route.npm_package, "@ai-sdk/anthropic");
        assert_eq!(route.api, "https://opencode.ai/zen/go/v1");
        assert_eq!(route.model_name, "qwen3.7-max");
        assert_eq!(
            ProviderKind::from_provider("opencode-go", &route.npm_package),
            ProviderKind::Anthropic
        );
        assert_eq!(
            ProviderKind::Anthropic.normalize_base_url(&route.api),
            "https://opencode.ai/zen/go"
        );
    }

    #[test]
    fn model_route_falls_back_to_provider_transport() {
        let provider: crate::model::discovery::Provider =
            serde_json::from_value(serde_json::json!({
                "id": "opencode-go",
                "name": "OpenCode Go",
                "api": "https://opencode.ai/zen/go/v1",
                "npm": "@ai-sdk/openai-compatible",
                "env": ["OPENCODE_API_KEY"],
                "models": {
                    "kimi-k2.6": {
                        "id": "kimi-k2.6",
                        "name": "Kimi K2.6",
                        "release_date": "2026-04-21",
                        "last_updated": "2026-04-21"
                    }
                }
            }))
            .unwrap();

        let route = resolve_model_route(&provider, "kimi-k2.6".to_string());
        assert_eq!(route.npm_package, "@ai-sdk/openai-compatible");
        assert_eq!(route.api, "https://opencode.ai/zen/go/v1");
        assert_eq!(route.model_name, "kimi-k2.6");
    }

    #[test]
    fn unauthenticated_opencode_free_model_uses_public_key() {
        let model: crate::model::discovery::Model = serde_json::from_value(serde_json::json!({
            "id": "big-pickle",
            "name": "Big Pickle",
            "cost": { "input": 0.0, "output": 0.0, "cache_read": 0.0 }
        }))
        .unwrap();
        let mut config = test_request_config(None);

        maybe_apply_unauthenticated_free_provider_key("opencode", Some(&model), &mut config);

        assert_eq!(config.api_key.as_deref(), Some("public"));
    }

    #[test]
    fn unauthenticated_opencode_paid_model_does_not_use_public_key() {
        let model: crate::model::discovery::Model = serde_json::from_value(serde_json::json!({
            "id": "gpt-5.2",
            "name": "GPT-5.2",
            "cost": { "input": 1.75, "output": 14.0 }
        }))
        .unwrap();
        let mut config = test_request_config(None);

        maybe_apply_unauthenticated_free_provider_key("opencode", Some(&model), &mut config);

        assert_eq!(config.api_key, None);
    }

    #[test]
    fn unauthenticated_opencode_deprecated_free_model_does_not_use_public_key() {
        let model: crate::model::discovery::Model = serde_json::from_value(serde_json::json!({
            "id": "kimi-k2.5-free",
            "name": "Kimi K2.5 Free",
            "status": "deprecated",
            "cost": { "input": 0.0, "output": 0.0 }
        }))
        .unwrap();
        let mut config = test_request_config(None);

        maybe_apply_unauthenticated_free_provider_key("opencode", Some(&model), &mut config);

        assert_eq!(config.api_key, None);
    }

    #[test]
    fn configured_opencode_key_is_not_overwritten_by_public_key() {
        let model: crate::model::discovery::Model = serde_json::from_value(serde_json::json!({
            "id": "big-pickle",
            "name": "Big Pickle",
            "cost": { "input": 0.0, "output": 0.0 }
        }))
        .unwrap();
        let mut config = test_request_config(Some("real-key".to_string()));

        maybe_apply_unauthenticated_free_provider_key("opencode", Some(&model), &mut config);

        assert_eq!(config.api_key.as_deref(), Some("real-key"));
    }

    #[test]
    fn xai_request_defaults_disable_server_storage() {
        let mut config = test_request_config(Some("xai-key".to_string()));

        apply_provider_request_defaults("xai", &mut config);

        assert!(config.openai_options.force_store_false);
    }

    #[test]
    fn non_xai_request_defaults_leave_storage_unchanged() {
        let mut config = test_request_config(Some("other-key".to_string()));

        apply_provider_request_defaults("openai", &mut config);

        assert!(!config.openai_options.force_store_false);
    }

    fn test_request_config(api_key: Option<String>) -> ProviderRequestConfig {
        ProviderRequestConfig::new(
            ProviderKind::OpenAICompatible,
            "OpenCode Zen".to_string(),
            "https://opencode.ai/zen/v1".to_string(),
            "big-pickle".to_string(),
            api_key,
            None,
            false,
        )
    }

    #[test]
    fn xai_provider_uses_openai_responses_transport() {
        let provider: crate::model::discovery::Provider =
            serde_json::from_value(serde_json::json!({
                "id": "xai",
                "name": "xAI",
                "api": "",
                "npm": "@ai-sdk/xai",
                "env": ["XAI_API_KEY"],
                "models": {
                    "grok-build-0.1": {
                        "id": "grok-build-0.1",
                        "name": "Grok Build 0.1"
                    }
                }
            }))
            .unwrap();

        let route = resolve_model_route(&provider, "grok-build-0.1".to_string());
        assert_eq!(route.npm_package, "@ai-sdk/xai");
        assert_eq!(route.api, "");
        assert_eq!(route.model_name, "grok-build-0.1");
        assert_eq!(
            ProviderKind::from_provider("xai", &route.npm_package),
            ProviderKind::OpenAI
        );
    }

    #[test]
    fn xai_grok_composer_model_uses_openai_responses_transport() {
        let provider: crate::model::discovery::Provider =
            serde_json::from_value(serde_json::json!({
                "id": "xai",
                "name": "xAI",
                "api": "",
                "npm": "@ai-sdk/xai",
                "env": ["XAI_API_KEY"],
                "models": {
                    "grok-composer-2.5-fast": {
                        "id": "grok-composer-2.5-fast",
                        "name": "Composer 2.5",
                        "family": "grok-build"
                    }
                }
            }))
            .unwrap();

        let route = resolve_model_route(&provider, "grok-composer-2.5-fast".to_string());
        assert_eq!(route.npm_package, "@ai-sdk/xai");
        assert_eq!(route.api, "");
        assert_eq!(route.model_name, "grok-composer-2.5-fast");
        assert_eq!(
            ProviderKind::from_provider("xai", &route.npm_package),
            ProviderKind::OpenAI
        );
    }

    #[test]
    fn grok_composer_is_text_only_for_raw_image_transport() {
        let stale_image_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "grok-composer-2.5-fast",
                "name": "Composer 2.5",
                "attachment": true,
                "modalities": {
                    "input": ["text", "image"],
                    "output": ["text"]
                }
            }))
            .unwrap();

        assert!(!model_supports_image_input(
            "grok-composer-2.5-fast",
            Some(&stale_image_model)
        ));
        assert!(!model_supports_image_input(
            "xai/grok-composer-2.5-fast",
            None
        ));
    }

    #[test]
    fn tool_history_replays_structured_tool_call_and_output() {
        let tool_message = crate::session::types::Message::tool(
            serde_json::json!({
                "name": "edit",
                "status": "ok",
                "id": "call_edit",
                "title": "Edit: src/lib.rs",
                "args": {
                    "file_path": "src/lib.rs",
                    "old_string": "old line",
                    "new_string": "new line"
                },
                "output_preview": "Replaced at line 7"
            })
            .to_string(),
        );

        let messages = convert_messages(&[tool_message]);

        assert_eq!(messages.len(), 2);
        match &messages[0] {
            AisdkMessage::ToolCall(call) => {
                assert_eq!(call.call_id, "call_edit");
                assert_eq!(call.name, "edit");
                assert!(call.arguments.contains("\"old_string\":\"old line\""));
                assert!(call.arguments.contains("\"new_string\":\"new line\""));
            }
            other => panic!("expected tool call, got {other:?}"),
        }
        match &messages[1] {
            AisdkMessage::ToolOutput(output) => {
                assert_eq!(output.call_id, "call_edit");
                assert_eq!(output.name, "edit");
                assert_eq!(output.output, "Replaced at line 7");
                assert!(!output.is_error);
            }
            other => panic!("expected tool output, got {other:?}"),
        }
    }

    #[test]
    fn assistant_ordered_parts_flatten_for_provider_replay() {
        let mut assistant = crate::session::types::Message::incomplete("");
        assistant.append("I will inspect.");
        assistant.add_tool_call_part(
            "call_edit",
            "edit",
            serde_json::json!({
                "file_path": "src/lib.rs",
                "old_string": "old line",
                "new_string": "new line"
            }),
        );
        assistant.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_edit",
            "name": "edit",
            "status": "ok",
            "args": {
                "file_path": "src/lib.rs",
                "old_string": "old line",
                "new_string": "new line"
            },
            "output_preview": "Replaced at line 7"
        }));
        assistant.append("Done.");

        let messages = convert_messages(&[assistant]);

        assert_eq!(messages.len(), 4);
        match &messages[0] {
            AisdkMessage::Assistant(message) => assert_eq!(message.content, "I will inspect."),
            other => panic!("expected assistant text, got {other:?}"),
        }
        match &messages[1] {
            AisdkMessage::ToolCall(call) => {
                assert_eq!(call.call_id, "call_edit");
                assert_eq!(call.name, "edit");
                assert!(call.arguments.contains("\"old_string\":\"old line\""));
            }
            other => panic!("expected tool call, got {other:?}"),
        }
        match &messages[2] {
            AisdkMessage::ToolOutput(output) => {
                assert_eq!(output.call_id, "call_edit");
                assert_eq!(output.output, "Replaced at line 7");
            }
            other => panic!("expected tool output, got {other:?}"),
        }
        match &messages[3] {
            AisdkMessage::Assistant(message) => assert_eq!(message.content, "Done."),
            other => panic!("expected assistant text, got {other:?}"),
        }
    }

    #[test]
    fn assistant_interleaved_tool_results_replay_as_valid_concurrent_group() {
        let mut assistant = crate::session::types::Message::incomplete("");
        assistant.append("I will inspect.");
        assistant.add_tool_call_part(
            "call_5",
            "bash",
            serde_json::json!({
                "command": "ls src",
            }),
        );
        assistant.add_tool_call_part(
            "call_6",
            "read",
            serde_json::json!({
                "file_path": "vite.config.ts",
            }),
        );
        assistant.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_6",
            "name": "read",
            "status": "ok",
            "args": {
                "file_path": "vite.config.ts",
            },
            "output_preview": "vite config"
        }));
        assistant.add_tool_call_part(
            "call_7",
            "read",
            serde_json::json!({
                "file_path": "tsconfig.json",
            }),
        );
        assistant.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_7",
            "name": "read",
            "status": "ok",
            "args": {
                "file_path": "tsconfig.json",
            },
            "output_preview": "tsconfig"
        }));
        assistant.add_tool_call_part(
            "call_8",
            "read",
            serde_json::json!({
                "file_path": "biome.json",
            }),
        );
        assistant.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_8",
            "name": "read",
            "status": "ok",
            "args": {
                "file_path": "biome.json",
            },
            "output_preview": "biome"
        }));
        assistant.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_5",
            "name": "bash",
            "status": "ok",
            "args": {
                "command": "ls src",
            },
            "output_preview": "src listing"
        }));
        assistant.append("Done.");

        let messages = convert_messages(&[assistant]);
        let order = messages
            .iter()
            .map(|message| match message {
                AisdkMessage::Assistant(_) => "assistant".to_string(),
                AisdkMessage::ToolCall(call) => format!("call:{}", call.call_id),
                AisdkMessage::ToolOutput(output) => format!("output:{}", output.call_id),
                other => panic!("unexpected message: {other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            order,
            vec![
                "assistant",
                "call:call_5",
                "call:call_6",
                "call:call_7",
                "call:call_8",
                "output:call_5",
                "output:call_6",
                "output:call_7",
                "output:call_8",
                "assistant",
            ]
        );
    }

    #[test]
    fn empty_assistant_messages_are_not_sent_to_provider() {
        let messages = convert_messages(&[
            crate::session::types::Message::system("system"),
            crate::session::types::Message::user("prompt"),
            crate::session::types::Message::assistant(""),
            crate::session::types::Message::assistant("   \n\t"),
            crate::session::types::Message::assistant("answer"),
        ]);

        assert_eq!(messages.len(), 3);
        match &messages[0] {
            AisdkMessage::System(message) => assert_eq!(message.content, "system"),
            other => panic!("expected system message, got {other:?}"),
        }
        match &messages[1] {
            AisdkMessage::User(message) => assert_eq!(message.content, "prompt"),
            other => panic!("expected user message, got {other:?}"),
        }
        match &messages[2] {
            AisdkMessage::Assistant(message) => assert_eq!(message.content, "answer"),
            other => panic!("expected assistant message, got {other:?}"),
        }
    }

    #[test]
    fn user_images_become_text_note_for_text_only_model() {
        let mut user_message = crate::session::types::Message::user("what is in this?");
        user_message.local_image_paths = vec!["/tmp/example.png".to_string()];

        let messages = convert_messages_for_model(&[user_message], false, false);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            AisdkMessage::User(message) => {
                assert!(message.images.is_empty());
                assert!(message.content.contains("what is in this?"));
                assert!(message
                    .content
                    .contains("this model does not support image input"));
            }
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[test]
    fn text_only_model_with_vlm_agent_configured_shows_paths_in_hint() {
        let mut user_message = crate::session::types::Message::user("what is this?");
        user_message.local_image_paths =
            vec!["/var/folders/tq/crabcode-clipboard-3vCHpv.png".to_string()];

        let messages = convert_messages_for_model(&[user_message], false, true);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            AisdkMessage::User(message) => {
                assert!(message.images.is_empty());
                assert!(message.content.contains("what is this?"));
                assert!(message.content.contains("crabcode-clipboard-3vCHpv.png"));
                assert!(message.content.contains("subagent_type: \"vlm-agent\""));
                assert!(message
                    .content
                    .contains("description: \"Analyze attached image(s)\""));
                assert!(message
                    .content
                    .contains("Use view_image on every image path below"));
                assert!(message
                    .content
                    .contains("must not call view_image directly"));
                assert!(message
                    .content
                    .contains("After the vlm-agent subagent returns, use its result"));
            }
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[test]
    fn vlm_hint_strips_legacy_image_description_blocks_but_keeps_paths() {
        let mut user_message = crate::session::types::Message::user(
            "[Image #1]\n\n<image_description source=\"vlm-agent\">\nPermission denied - path outside working directory\n</image_description>\nwhat is this?",
        );
        user_message.local_image_paths =
            vec!["/var/folders/tq/crabcode-clipboard-gYi30o.png".to_string()];

        let messages = convert_messages_for_model(&[user_message], false, true);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            AisdkMessage::User(message) => {
                assert!(message.content.contains("[Image #1]"));
                assert!(message.content.contains("what is this?"));
                assert!(message.content.contains("crabcode-clipboard-gYi30o.png"));
                assert!(message.content.contains("subagent_type: \"vlm-agent\""));
                assert!(!message.content.contains("<image_description"));
                assert!(!message.content.contains("Permission denied"));
            }
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[test]
    fn legacy_image_description_blocks_are_stripped_from_history_text() {
        let stripped = crate::utils::sanitize::strip_legacy_image_descriptions(
            "before\n<image_description source=\"vlm-agent\">\nstale\n</image_description>\nafter",
        );
        assert_eq!(stripped, "before\n\nafter");

        let assistant = crate::session::types::Message::assistant(
            "visible\n<image_description source=\"vlm-agent\">hidden</image_description>",
        );
        let messages = convert_messages(&[assistant]);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            AisdkMessage::Assistant(message) => {
                assert_eq!(message.content, "visible");
            }
            other => panic!("expected assistant message, got {other:?}"),
        }
    }

    #[test]
    fn vlm_agent_model_check_requires_runnable_task_target() {
        let config = serde_json::json!({
            "vlm-agent": {
                "model": "xai/grok-4.3",
                "mode": "primary"
            }
        });
        let mut warnings = Vec::new();
        let registry = crate::agent::definition::AgentRegistry::with_definitions(
            None,
            crate::agent::definition::parse_agent_definitions_from_config(
                Some(&config),
                &mut warnings,
            ),
        );

        assert!(warnings.is_empty());
        assert!(!vlm_agent_has_model(&registry));
    }

    #[test]
    fn vlm_agent_model_check_accepts_configured_subagent() {
        let config = serde_json::json!({
            "vlm-agent": {
                "model": "xai/grok-4.3"
            }
        });
        let mut warnings = Vec::new();
        let registry = crate::agent::definition::AgentRegistry::with_definitions(
            None,
            crate::agent::definition::parse_agent_definitions_from_config(
                Some(&config),
                &mut warnings,
            ),
        );

        assert!(warnings.is_empty());
        assert!(vlm_agent_has_model(&registry));
    }

    #[test]
    fn view_image_tool_history_becomes_text_note_for_text_only_model() {
        let tool_message = crate::session::types::Message::tool(
            serde_json::json!({
                "name": "view_image",
                "status": "ok",
                "id": "call_view_image",
                "args": {
                    "path": "/tmp/example.png"
                },
                "metadata": {
                    "path": "/tmp/example.png"
                },
                "output_preview": "Viewed image /tmp/example.png (2x1, image/png)"
            })
            .to_string(),
        );

        let messages = convert_messages_for_model(&[tool_message], false, false);

        assert_eq!(messages.len(), 2);
        match &messages[1] {
            AisdkMessage::ToolOutput(output) => {
                assert!(output.images.is_empty());
                assert!(output.output.contains("Viewed image /tmp/example.png"));
                assert!(output
                    .output
                    .contains("this model does not support image input"));
            }
            other => panic!("expected tool output, got {other:?}"),
        }
    }

    #[test]
    fn model_image_input_support_uses_modalities_then_attachment() {
        let image_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "vision",
                "name": "Vision",
                "attachment": false,
                "modalities": {
                    "input": ["text", "image"],
                    "output": ["text"]
                }
            }))
            .unwrap();
        let text_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "text",
                "name": "Text",
                "attachment": true,
                "modalities": {
                    "input": ["text"],
                    "output": ["text"]
                }
            }))
            .unwrap();
        let attachment_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "legacy-vision",
                "name": "Legacy Vision",
                "attachment": true
            }))
            .unwrap();
        let no_attachment_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "legacy-text",
                "name": "Legacy Text",
                "attachment": false
            }))
            .unwrap();

        assert!(model_supports_image_input("vision", Some(&image_model)));
        assert!(!model_supports_image_input("text", Some(&text_model)));
        assert!(model_supports_image_input(
            "legacy-vision",
            Some(&attachment_model)
        ));
        assert!(!model_supports_image_input(
            "legacy-text",
            Some(&no_attachment_model)
        ));
        assert!(!model_supports_image_input("unknown", None));
    }

    #[test]
    fn codex_spark_is_text_only_for_image_input_even_with_missing_or_stale_metadata() {
        let stale_image_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "gpt-5.3-codex-spark",
                "name": "GPT-5.3 Codex Spark",
                "attachment": true,
                "modalities": {
                    "input": ["text", "image"],
                    "output": ["text"]
                }
            }))
            .unwrap();

        assert!(!model_supports_image_input(
            "gpt-5.3-codex-spark",
            Some(&stale_image_model)
        ));
        assert!(!model_supports_image_input(
            "openai/gpt-5.3-codex-spark",
            None
        ));
    }

    #[test]
    fn deepseek_v4_flash_is_text_only_for_image_input_even_with_missing_or_stale_metadata() {
        let stale_image_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "deepseek-v4-flash",
                "name": "DeepSeek V4 Flash",
                "attachment": true,
                "modalities": {
                    "input": ["text", "image"],
                    "output": ["text"]
                }
            }))
            .unwrap();

        assert!(!model_supports_image_input(
            "deepseek-v4-flash",
            Some(&stale_image_model)
        ));
        assert!(!model_supports_image_input(
            "deepseek/deepseek-v4-flash",
            None
        ));
    }

    #[test]
    fn mimo_v25_pro_is_text_only_for_image_input_even_with_missing_or_stale_metadata() {
        let stale_image_model: crate::model::discovery::Model =
            serde_json::from_value(serde_json::json!({
                "id": "mimo-v2.5-pro",
                "name": "MiMo V2.5 Pro",
                "attachment": true,
                "modalities": {
                    "input": ["text", "image"],
                    "output": ["text"]
                }
            }))
            .unwrap();

        assert!(!model_supports_image_input(
            "mimo-v2.5-pro",
            Some(&stale_image_model)
        ));
        assert!(!model_supports_image_input("mimo v2.5 pro", None));
        assert!(!model_supports_image_input("minimax/mimo-v2.5-pro", None));
    }

    #[test]
    fn compaction_marker_is_not_sent_to_model() {
        let stats = crate::session::types::CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 8,
            after_messages: 2,
        };
        let marker = crate::session::compaction::compaction_marker(stats);

        let messages = convert_messages(&[crate::session::types::Message::user("tail"), marker]);

        assert_eq!(messages.len(), 1);
        match &messages[0] {
            AisdkMessage::User(message) => assert_eq!(message.content, "tail"),
            other => panic!("expected user message, got {other:?}"),
        }
    }

    fn websearch_tool_for_test() -> crate::aisdk::core::Tool {
        crate::aisdk::core::Tool::builder()
            .name("websearch")
            .description("Search the web")
            .input_schema(schemars::schema_for!(bool))
            .execute(crate::aisdk::tool::ToolExecute::new(|_| async {
                Ok::<_, String>("ok")
            }))
            .build()
            .expect("test tool")
    }

    #[test]
    fn native_first_replaces_external_websearch_for_openai() {
        let mut tools = vec![websearch_tool_for_test()];
        let hosted = resolve_websearch_tools(
            "openai",
            &crate::config::configuration::WebsearchConfig::default(),
            &mut tools,
        );
        assert!(tools.is_empty());
        assert_eq!(hosted, vec![crate::aisdk::core::HostedTool::WebSearch]);
    }

    #[test]
    fn native_first_keeps_external_websearch_for_unsupported_provider() {
        let mut tools = vec![websearch_tool_for_test()];
        let hosted = resolve_websearch_tools(
            "anthropic",
            &crate::config::configuration::WebsearchConfig::default(),
            &mut tools,
        );
        assert_eq!(tools.len(), 1);
        assert!(hosted.is_empty());
    }

    #[test]
    fn external_only_keeps_external_websearch_for_openrouter() {
        let mut config = crate::config::configuration::WebsearchConfig::default();
        config.strategy = crate::config::configuration::WebsearchStrategy::ExternalOnly;
        let mut tools = vec![websearch_tool_for_test()];
        let hosted = resolve_websearch_tools("openrouter", &config, &mut tools);
        assert_eq!(tools.len(), 1);
        assert!(hosted.is_empty());
    }

    #[test]
    fn native_only_hides_external_websearch_for_unsupported_provider() {
        let mut config = crate::config::configuration::WebsearchConfig::default();
        config.strategy = crate::config::configuration::WebsearchStrategy::NativeOnly;
        let mut tools = vec![websearch_tool_for_test()];
        let hosted = resolve_websearch_tools("anthropic", &config, &mut tools);
        assert!(tools.is_empty());
        assert!(hosted.is_empty());
    }
}
fn content_with_vlm_agent_hint(content: &str, image_paths: &[String]) -> String {
    let paths = image_paths
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let user_request = if content.trim().is_empty() {
        "No additional user text was provided. Analyze the image(s)."
    } else {
        content
    };
    let hint = format!(
        "The attached image(s) cannot be sent to this model directly.\n\nYou must not call view_image directly in this parent turn. Before answering the user's image request, call the task tool with:\n- subagent_type: \"{}\"\n- description: \"Analyze attached image(s)\"\n- prompt: \"Use view_image on every image path below. Return the visual findings needed to answer the user's request.\n\nImage paths:\n{paths}\n\nUser request:\n{user_request}\"\n\nAfter the vlm-agent subagent returns, use its result to answer the user.",
        crate::agent::definition::VLM_AGENT_NAME
    );
    if content.trim().is_empty() {
        hint
    } else {
        format!("{content}\n\n{hint}")
    }
}
