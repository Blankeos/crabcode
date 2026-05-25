use aisdk::core::{
    chunk::{ChunkType, MessagePhase},
    response::{stream_with_tools, LanguageModelStream, StreamTextResponse},
    stop::StopReason,
    Message as AisdkMessage, Tool,
};
use aisdk::message::ImageContent;
use aisdk::{Anthropic, OpenAI, OpenAICompatible};
use futures::StreamExt;
use std::{collections::HashMap, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::tools::aisdk_bridge::convert_to_aisdk_tools;

const MAX_STEPS_REACHED_PROMPT: &str = r#"CRITICAL - MAXIMUM STEPS REACHED

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

const TOOL_HISTORY_ARGUMENTS_MAX_CHARS: usize = 60_000;

type DynError = Box<dyn std::error::Error>;

#[derive(Clone, Debug, Default)]
struct OpenAIRequestOptions {
    response_path: Option<String>,
    additional_headers: HashMap<String, String>,
    force_store_false: bool,
    default_instructions: Option<String>,
    disallow_system_messages: bool,
    force_tool_strict_false: bool,
}

#[derive(Clone, Debug)]
struct ProviderRequestConfig {
    kind: ProviderKind,
    provider_name: String,
    base_url: String,
    model_name: String,
    api_key: Option<String>,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
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
    ) -> Self {
        Self {
            kind,
            provider_name,
            base_url,
            model_name,
            api_key,
            reasoning_effort,
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
    tool_permissions: crate::tools::ToolPermissions,
    messages: Vec<crate::session::types::Message>,
    sender: crate::llm::ChunkSender,
) -> Result<(), DynError> {
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

    let aisdk_messages = convert_messages(&messages);

    let tool_registry = crate::tools::initialize_tool_registry().await;

    crate::tools::register_dynamic_tools(&tool_registry, Some(sender.clone())).await;

    // Set LLM session config for subagent use
    crate::agent::config::set_llm_session(crate::agent::config::LlmSessionConfig {
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
    });

    let aisdk_tools = convert_to_aisdk_tools(
        &tool_registry,
        Some(sender.clone()),
        agent_mode,
        tool_permissions,
        Some(session_id.clone()),
        None,
    )
    .await;

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

    let mut response = stream_provider_request(
        &request_config,
        aisdk_messages,
        aisdk_tools,
        agent_max_steps,
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

    let mut summary_response =
        stream_provider_request(&request_config, follow_up_messages, Vec::new(), None).await?;

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
    let mut response = stream_provider_request(&request_config, messages, Vec::new(), None).await?;

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
            | ChunkType::End(_)
            | ChunkType::AssistantMessagePhase { .. }
            | ChunkType::ResponseCompleted { .. }
            | ChunkType::Metadata(_)
            | ChunkType::Start
            | ChunkType::Incomplete(_) => {}
        }
    }

    let summary = summary.trim().to_string();
    if summary.is_empty() {
        return Err(anyhow::anyhow!("Compaction returned an empty summary").into());
    }

    Ok(summary)
}

async fn prepare_request_config(
    provider_name: &str,
    model: String,
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,
    sender: &crate::llm::ChunkSender,
) -> Result<ProviderRequestConfig, DynError> {
    let auth_dao = crate::persistence::AuthDAO::new()?;
    let auth_config = auth_dao.get_provider(provider_name)?;

    let provider = if crate::model::ollama::is_ollama_provider(provider_name) {
        crate::model::ollama::provider()
    } else {
        let discovery = crate::model::discovery::Discovery::new()?;
        let providers = discovery.fetch_providers().await?;

        providers
            .get(provider_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?
    };

    let provider_kind = ProviderKind::from_provider(provider_name, &provider.npm);
    let mut request_config = ProviderRequestConfig::new(
        provider_kind,
        provider.name.clone(),
        provider_kind.normalize_base_url(&provider.api),
        model,
        configured_api_key(auth_config.as_ref()),
        reasoning_effort,
    );

    maybe_apply_openai_oauth_overrides(
        provider_name,
        &auth_dao,
        auth_config.as_ref(),
        &mut request_config,
        sender,
    )
    .await;

    if request_config.api_key.is_none() && !crate::model::ollama::is_ollama_provider(provider_name)
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
        "Provider: {}, NPM: {}, Base URL: {}, Model: {}",
        provider_name,
        provider.npm,
        request_config.base_url,
        request_config.model_name
    );

    Ok(request_config)
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
) {
    if request_config.kind != ProviderKind::OpenAI || provider_name != "openai" {
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
    let mut oauth_account_id = account_id;
    let mut oauth_enterprise_url = enterprise_url;

    if oauth_expires <= crate::auth::openai_oauth::now_unix_ms() + 60_000 {
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
}

fn send_warning(sender: &crate::llm::ChunkSender, warning: impl Into<String>) {
    let _ = sender.send(crate::llm::ChunkMessage::Warning(warning.into()));
}

async fn stream_provider_request(
    config: &ProviderRequestConfig,
    messages: Vec<AisdkMessage>,
    tools: Vec<Tool>,
    max_steps: Option<usize>,
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
            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_tools(provider, messages, tools, max_steps, None, headers)
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
            stream_with_tools(provider, messages, tools, max_steps, None, headers)
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
            if !config.openai_options.additional_headers.is_empty() {
                builder = builder.headers(config.openai_options.additional_headers.clone());
            }

            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_tools(provider, messages, tools, max_steps, None, headers)
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
        "[STREAM_REQUEST] {} reasoning_effort={} responses_path={:?} force_store_false={} disallow_system_messages={} force_tool_strict_false={} extra_header_names=[{}]",
        context.describe(),
        reasoning_effort,
        config.openai_options.response_path,
        config.openai_options.force_store_false,
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
            ChunkType::End(_msg) => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("End", elapsed_ms);
                crate::emit_log!(
                    "[RELAY] End chunk — returning Ended {}",
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
            ChunkType::Start => {
                let elapsed_ms = start_time.elapsed().as_millis();
                stats.record_chunk("Start", elapsed_ms);
                stats.start_chunks += 1;
                crate::emit_log!("[RELAY] Start chunk received");
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
    let mut aisdk_messages = Vec::new();

    for msg in messages {
        match msg.role {
            crate::session::types::MessageRole::System => {
                aisdk_messages.push(AisdkMessage::system(msg.content.clone()));
            }
            crate::session::types::MessageRole::User => {
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
                    aisdk_messages.push(AisdkMessage::user(msg.content.clone()));
                } else {
                    aisdk_messages
                        .push(AisdkMessage::user_with_images(msg.content.clone(), images));
                }
            }
            crate::session::types::MessageRole::Assistant => {
                aisdk_messages.push(AisdkMessage::assistant(msg.content.clone()));
            }
            crate::session::types::MessageRole::Tool => {
                if let Some(tool_messages) = tool_messages_for_model(&msg.content) {
                    aisdk_messages.extend(tool_messages);
                } else {
                    aisdk_messages.push(AisdkMessage::user(tool_message_observation(&msg.content)));
                }
            }
        }
    }

    aisdk_messages
}

fn tool_messages_for_model(content: &str) -> Option<Vec<AisdkMessage>> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let obj = value.as_object()?;

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

    let arguments = obj
        .get("args")
        .map(|args| serde_json::to_string(args).unwrap_or_else(|_| args.to_string()))
        .unwrap_or_else(|| "{}".to_string());
    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("ok");
    let is_error = status.eq_ignore_ascii_case("error");

    let images = if name == "view_image" && !is_error {
        view_image_tool_images(obj)
    } else {
        Vec::new()
    };

    Some(vec![
        AisdkMessage::tool_call(call_id, name, arguments),
        AisdkMessage::tool_output_with_images(call_id, name, output, images, is_error),
    ])
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
        &serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string()),
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
        convert_messages, is_openai_oauth_model_allowed, openai_request_instructions, AisdkMessage,
        OpenAIRequestOptions,
    };

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
}
