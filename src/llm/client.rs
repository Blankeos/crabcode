use aisdk::core::{
    chunk::ChunkType,
    response::{stream_with_tools, LanguageModelStream, StreamTextResponse},
    stop::{step_count_is, StopReason},
    Message as AisdkMessage, Tool,
};
use aisdk::message::ImageContent;
use aisdk::{Anthropic, OpenAI, OpenAICompatible};
use futures::StreamExt;
use std::{collections::HashMap, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::logging::log;
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
    openai_options: OpenAIRequestOptions,
}

impl ProviderRequestConfig {
    fn new(
        kind: ProviderKind,
        provider_name: String,
        base_url: String,
        model_name: String,
        api_key: Option<String>,
    ) -> Self {
        Self {
            kind,
            provider_name,
            base_url,
            model_name,
            api_key,
            openai_options: OpenAIRequestOptions::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamRelayOutcome {
    Ended,
    Exhausted,
}

pub async fn stream_llm_with_cancellation(
    cancel_token: CancellationToken,
    session_id: String,
    provider_name: String,
    model: String,
    agent_mode: String,
    agent_max_steps: Option<usize>,
    tool_permissions: crate::tools::ToolPermissions,
    messages: Vec<crate::session::types::Message>,
    sender: crate::llm::ChunkSender,
) -> Result<(), DynError> {
    let _ = log("GOING TO STREAM");
    let request_config = prepare_request_config(&provider_name, model, &sender).await?;

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
    });

    let aisdk_tools = convert_to_aisdk_tools(
        &tool_registry,
        Some(sender.clone()),
        agent_mode,
        tool_permissions,
        Some(session_id),
        None,
    )
    .await;

    let mut response = stream_provider_request(
        &request_config,
        aisdk_messages,
        aisdk_tools,
        agent_max_steps,
    )
    .await?;

    let start_time = Instant::now();
    let mut token_count: usize = 0;

    let stream_outcome = relay_stream_to_sender(
        &mut response.stream,
        &cancel_token,
        &sender,
        &mut token_count,
        &start_time,
    )
    .await?;

    let stop_reason = response.stop_reason().await;
    let _ = log(&format!(
        "Stream completed: outcome={stream_outcome:?}, stop_reason={stop_reason:?}, agent_max_steps={agent_max_steps:?}",
    ));

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

    let mut summary_response =
        stream_provider_request(&request_config, follow_up_messages, Vec::new(), None).await?;

    let _ = relay_stream_to_sender(
        &mut summary_response.stream,
        &cancel_token,
        &sender,
        &mut token_count,
        &start_time,
    )
    .await?;

    Ok(())
}

pub async fn summarize_for_compaction(
    provider_name: String,
    model: String,
    prompt: String,
) -> Result<String, DynError> {
    let (warning_sender, _warning_receiver) = tokio::sync::mpsc::unbounded_channel();
    let request_config = prepare_request_config(&provider_name, model, &warning_sender).await?;
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
    sender: &crate::llm::ChunkSender,
) -> Result<ProviderRequestConfig, DynError> {
    let auth_dao = crate::persistence::AuthDAO::new()?;
    let auth_config = auth_dao.get_provider(provider_name)?;

    let discovery = crate::model::discovery::Discovery::new()?;
    let providers = discovery.fetch_providers().await?;

    let provider = providers
        .get(provider_name)
        .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?;

    let provider_kind = ProviderKind::from_provider(provider_name, &provider.npm);
    let mut request_config = ProviderRequestConfig::new(
        provider_kind,
        provider.name.clone(),
        provider_kind.normalize_base_url(&provider.api),
        model,
        configured_api_key(auth_config.as_ref()),
    );

    maybe_apply_openai_oauth_overrides(
        provider_name,
        &auth_dao,
        auth_config.as_ref(),
        &mut request_config,
        sender,
    )
    .await;

    if request_config.api_key.is_none() {
        send_warning(
            sender,
            format!(
                "No API key configured for '{}'. Trying anyway.",
                provider_name
            ),
        );
    }

    let _ = log(&format!(
        "Provider: {}, NPM: {}, Base URL: {}, Model: {}",
        provider_name, provider.npm, request_config.base_url, request_config.model_name
    ));

    Ok(request_config)
}

fn configured_api_key(auth_config: Option<&crate::persistence::AuthConfig>) -> Option<String> {
    auth_config.and_then(|config| match config {
        crate::persistence::AuthConfig::Api { key } => Some(key.clone()),
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

    let _ = log("Configured OpenAI OAuth Codex transport");

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
    let stop_when = max_steps.map(|s| step_count_is(s));
    let headers = HashMap::new();

    match config.kind {
        ProviderKind::OpenAICompatible => {
            let mut builder = OpenAICompatible::builder()
                .base_url(&config.base_url)
                .model_name(&config.model_name)
                .provider_name(&config.provider_name);
            if let Some(key) = config.api_key.as_deref() {
                builder = builder.api_key(key);
            }
            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_tools(provider, messages, tools, max_steps, stop_when, headers)
                .await
                .map_err(|e| Box::new(e) as DynError)
        }
        ProviderKind::Anthropic => {
            let mut builder = Anthropic::builder()
                .base_url(&config.base_url)
                .model_name(&config.model_name)
                .provider_name(&config.provider_name);
            if let Some(key) = config.api_key.as_deref() {
                builder = builder.api_key(key);
            }
            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_tools(provider, messages, tools, max_steps, stop_when, headers)
                .await
                .map_err(|e| Box::new(e) as DynError)
        }
        ProviderKind::OpenAI => {
            let mut builder = OpenAI::builder()
                .base_url(&config.base_url)
                .model_name(&config.model_name)
                .provider_name(&config.provider_name);
            if let Some(key) = config.api_key.as_deref() {
                builder = builder.api_key(key);
            }

            if let Some(responses_path) = &config.openai_options.response_path {
                builder = builder.responses_path(responses_path);
            }
            if config.openai_options.force_store_false {
                builder = builder.store_override(false);
            }
            if let Some(instructions) = &config.openai_options.default_instructions {
                builder = builder.default_instructions(instructions.clone());
            }
            if config.openai_options.disallow_system_messages {
                builder = builder.strip_system_and_developer_messages(true);
            }
            if config.openai_options.force_tool_strict_false {
                builder = builder.tool_strict_override(false);
            }
            if !config.openai_options.additional_headers.is_empty() {
                builder = builder.headers(config.openai_options.additional_headers.clone());
            }

            let provider = builder.build().map_err(|e| -> DynError { Box::new(e) })?;
            stream_with_tools(provider, messages, tools, max_steps, stop_when, headers)
                .await
                .map_err(|e| Box::new(e) as DynError)
        }
    }
}

async fn relay_stream_to_sender(
    stream: &mut LanguageModelStream,
    cancel_token: &CancellationToken,
    sender: &crate::llm::ChunkSender,
    token_count: &mut usize,
    start_time: &Instant,
) -> Result<StreamRelayOutcome, DynError> {
    let _ = log("[RELAY] relay_stream_to_sender started");
    loop {
        let chunk = tokio::select! {
            _ = cancel_token.cancelled() => {
                let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
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
                *token_count += estimate_tokens(&text);
                let _ = log(&format!("[RELAY] Text chunk ({} chars)", text.len()));
                let _ = sender.send(crate::llm::ChunkMessage::Text(text));
            }
            ChunkType::Reasoning(reasoning) => {
                *token_count += estimate_tokens(&reasoning);
                let _ = log(&format!(
                    "[RELAY] Reasoning chunk ({} chars)",
                    reasoning.len()
                ));
                let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
            }
            ChunkType::ToolCall(tool_call) => {
                let names = serde_json::from_str::<serde_json::Value>(&tool_call)
                    .ok()
                    .and_then(|value| {
                        value.as_array().map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    item.get("function")
                                        .and_then(|function| function.get("name"))
                                        .and_then(|name| name.as_str())
                                })
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                    })
                    .filter(|names| !names.is_empty())
                    .unwrap_or_else(|| "unknown".to_string());
                let _ = log(&format!(
                    "[RELAY] ToolCall chunk received names={} bytes={}",
                    names,
                    tool_call.len()
                ));
            }
            ChunkType::End(_msg) => {
                let _ = log("[RELAY] End chunk — returning Ended");
                let duration_ms = start_time.elapsed().as_millis() as u64;
                let _ = sender.send(crate::llm::ChunkMessage::Metrics {
                    token_count: *token_count,
                    duration_ms,
                });
                let _ = sender.send(crate::llm::ChunkMessage::End);
                return Ok(StreamRelayOutcome::Ended);
            }
            ChunkType::Start => {
                let _ = log("[RELAY] Start chunk received");
            }
            ChunkType::Failed(err) => {
                let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                let _ = log(&format!("Stream Chunk Failed {}", err));
                return Err(anyhow::anyhow!("Streaming failed: {}", err).into());
            }
            ChunkType::Incomplete(_msg) => {}
            ChunkType::NotSupported(_msg) => {}
        }
    }

    let _ = log("[RELAY] stream exhausted — returning Exhausted");
    Ok(StreamRelayOutcome::Exhausted)
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
                        match crate::utils::image_attachment::data_url_for_path(path) {
                            Ok(data_url) => Some(ImageContent {
                                data_url,
                                media_type: crate::utils::image_attachment::mime_type_for_path(
                                    path,
                                )
                                .to_string(),
                            }),
                            Err(err) => {
                                let _ = log(&format!(
                                    "failed to attach image {}: {}",
                                    path.display(),
                                    err
                                ));
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
                aisdk_messages.push(AisdkMessage::user(tool_message_observation(&msg.content)));
            }
        }
    }

    aisdk_messages
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
    if !output.is_empty() {
        observation.push_str("\n");
        observation.push_str(output);
    }

    observation
}

fn is_openai_oauth_model_allowed(model: &str) -> bool {
    matches!(
        model,
        "gpt-5.1-codex-max"
            | "gpt-5.1-codex-mini"
            | "gpt-5.2"
            | "gpt-5.2-codex"
            | "gpt-5.3-codex"
            | "gpt-5.1-codex"
            | "codex-mini-latest"
    ) || model.contains("codex")
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
