use aisdk::{
    core::{
        capabilities::ToolCallSupport,
        language_model::{LanguageModelStream, StopReason},
        utils::step_count_is,
        DynamicModel, LanguageModel, LanguageModelRequest, LanguageModelStreamChunkType,
        Message as AisdkMessage, StreamTextResponse, Tool,
    },
    providers::{Anthropic, OpenAI, OpenAICompatible},
};
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
    let aisdk_tools = convert_to_aisdk_tools(
        &tool_registry,
        Some(sender.clone()),
        agent_mode,
        tool_permissions,
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
    follow_up_messages.push(AisdkMessage::Assistant(
        MAX_STEPS_REACHED_PROMPT.to_string().into(),
    ));

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
    match config.kind {
        ProviderKind::OpenAICompatible => {
            let provider = build_openai_compatible_provider(config)?;
            stream_with_model(provider, messages, tools, max_steps).await
        }
        ProviderKind::Anthropic => {
            let provider = build_anthropic_provider(config)?;
            stream_with_model(provider, messages, tools, max_steps).await
        }
        ProviderKind::OpenAI => {
            let provider = build_openai_provider(config)?;
            stream_with_model(provider, messages, tools, max_steps).await
        }
    }
}

async fn stream_with_model<M>(
    model: M,
    messages: Vec<AisdkMessage>,
    tools: Vec<Tool>,
    max_steps: Option<usize>,
) -> Result<StreamTextResponse, DynError>
where
    M: LanguageModel + ToolCallSupport,
{
    let mut builder = LanguageModelRequest::builder()
        .model(model)
        .messages(messages);

    if let Some(max_steps) = max_steps {
        builder = builder.stop_when(step_count_is(max_steps));
    }

    for tool in tools {
        builder = builder.with_tool(tool);
    }

    let mut request = builder.build();
    request
        .stream_text()
        .await
        .map_err(|e| Box::new(e) as DynError)
}

fn build_openai_compatible_provider(
    config: &ProviderRequestConfig,
) -> Result<OpenAICompatible<DynamicModel>, DynError> {
    let mut provider_builder = OpenAICompatible::<DynamicModel>::builder()
        .base_url(&config.base_url)
        .model_name(&config.model_name)
        .provider_name(&config.provider_name);

    if let Some(key) = config.api_key.as_deref() {
        provider_builder = provider_builder.api_key(key);
    }

    provider_builder
        .build()
        .map_err(|e| Box::new(e) as DynError)
}

fn build_anthropic_provider(
    config: &ProviderRequestConfig,
) -> Result<Anthropic<DynamicModel>, DynError> {
    let mut provider_builder = Anthropic::<DynamicModel>::builder()
        .base_url(&config.base_url)
        .model_name(&config.model_name)
        .provider_name(&config.provider_name);

    if let Some(key) = config.api_key.as_deref() {
        provider_builder = provider_builder.api_key(key);
    }

    provider_builder
        .build()
        .map_err(|e| Box::new(e) as DynError)
}

fn build_openai_provider(config: &ProviderRequestConfig) -> Result<OpenAI<DynamicModel>, DynError> {
    let mut provider_builder = OpenAI::<DynamicModel>::builder()
        .base_url(&config.base_url)
        .model_name(&config.model_name)
        .provider_name(&config.provider_name);

    if let Some(key) = config.api_key.as_deref() {
        provider_builder = provider_builder.api_key(key);
    }

    if let Some(response_path) = &config.openai_options.response_path {
        provider_builder = provider_builder.response_path(response_path);
    }

    if config.openai_options.force_store_false {
        provider_builder = provider_builder.force_store_false(true);
    }

    if let Some(instructions) = &config.openai_options.default_instructions {
        provider_builder = provider_builder.default_instructions(instructions.clone());
    }

    if config.openai_options.disallow_system_messages {
        provider_builder = provider_builder.disallow_system_messages(true);
    }

    if config.openai_options.force_tool_strict_false {
        provider_builder = provider_builder.force_tool_strict_false(true);
    }

    if !config.openai_options.additional_headers.is_empty() {
        provider_builder =
            provider_builder.additional_headers(config.openai_options.additional_headers.clone());
    }

    provider_builder
        .build()
        .map_err(|e| Box::new(e) as DynError)
}

async fn relay_stream_to_sender(
    stream: &mut LanguageModelStream,
    cancel_token: &CancellationToken,
    sender: &crate::llm::ChunkSender,
    token_count: &mut usize,
    start_time: &Instant,
) -> Result<StreamRelayOutcome, DynError> {
    while let Some(chunk) = stream.next().await {
        if cancel_token.is_cancelled() {
            let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
            return Err(anyhow::anyhow!("Streaming cancelled by user").into());
        }

        match chunk {
            LanguageModelStreamChunkType::Text(text) => {
                *token_count += estimate_tokens(&text);
                let _ = sender.send(crate::llm::ChunkMessage::Text(text));
            }
            LanguageModelStreamChunkType::Reasoning(reasoning) => {
                *token_count += estimate_tokens(&reasoning);
                let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
            }
            LanguageModelStreamChunkType::ToolCall(_tool_call) => {
                // Tool execution is handled internally by aisdk::stream_text().
                // We intentionally don't surface argument deltas here.
            }
            LanguageModelStreamChunkType::End(_msg) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                let _ = sender.send(crate::llm::ChunkMessage::Metrics {
                    token_count: *token_count,
                    duration_ms,
                });
                let _ = sender.send(crate::llm::ChunkMessage::End);
                return Ok(StreamRelayOutcome::Ended);
            }
            LanguageModelStreamChunkType::Start => {}
            LanguageModelStreamChunkType::Failed(err) => {
                let _ = sender.send(crate::llm::ChunkMessage::Failed(err.clone()));
                let _ = log(&format!("Stream Chunk Failed {}", err));
                return Err(anyhow::anyhow!("Streaming failed: {}", err).into());
            }
            LanguageModelStreamChunkType::Incomplete(_msg) => {}
            LanguageModelStreamChunkType::NotSupported(_msg) => {}
        }
    }

    Ok(StreamRelayOutcome::Exhausted)
}

async fn reached_step_limit(agent_max_steps: Option<usize>, response: &StreamTextResponse) -> bool {
    agent_max_steps.is_some() && matches!(response.stop_reason().await, Some(StopReason::Hook))
}

fn estimate_tokens(content: &str) -> usize {
    // Estimate tokens: ~4 characters per token on average
    content.chars().count().max(1) / 4
}

fn convert_messages(messages: &[crate::session::types::Message]) -> Vec<AisdkMessage> {
    use aisdk::core::Message::{Assistant, System, User};

    let mut aisdk_messages = Vec::new();

    for msg in messages {
        match msg.role {
            crate::session::types::MessageRole::System => {
                aisdk_messages.push(System(msg.content.clone().into()));
            }
            crate::session::types::MessageRole::User => {
                aisdk_messages.push(User(msg.content.clone().into()));
            }
            crate::session::types::MessageRole::Assistant => {
                aisdk_messages.push(Assistant(msg.content.clone().into()));
            }
            crate::session::types::MessageRole::Tool => {
                continue;
            }
        }
    }

    aisdk_messages
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
        // Dirty: But add any workaround/overrides here in case npm_package can be treated differently.
        // if provider_name == "kimi-for-coding" {
        //     return Self::OpenAICompatible;
        // }

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
