use aisdk::{
    core::{
        language_model::StopReason, utils::step_count_is, LanguageModelRequest,
        LanguageModelStreamChunkType, Message as AisdkMessage,
    },
    providers::{Anthropic, OpenAI, OpenAICompatible},
};
use futures::StreamExt;
use std::collections::HashMap;
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

pub struct LLMClient {
    base_url: String,
    api_key: Option<String>,
    model_name: String,
    provider_name: String,
    npm_package: String,
}

impl LLMClient {
    pub fn new(
        base_url: String,
        api_key: Option<String>,
        model_name: String,
        provider_name: String,
        npm_package: String,
    ) -> Self {
        Self {
            base_url,
            api_key,
            model_name,
            provider_name,
            npm_package,
        }
    }

    fn provider_kind(&self) -> ProviderKind {
        ProviderKind::from_provider(&self.provider_name, &self.npm_package)
    }

    pub async fn stream_chat(
        &self,
        messages: &[crate::session::types::Message],
        mut on_chunk: impl FnMut(LanguageModelStreamChunkType),
    ) -> Result<(), Box<dyn std::error::Error>> {
        let aisdk_messages = self.convert_messages(messages);

        let tool_registry = crate::tools::initialize_tool_registry().await;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let permissions = crate::tools::ToolPermissions::new(cwd);
        let aisdk_tools =
            convert_to_aisdk_tools(&tool_registry, None, "Build".to_string(), permissions).await;

        let provider_kind = self.provider_kind();
        let base_url = provider_kind.normalize_base_url(&self.base_url);

        let response = match provider_kind {
            ProviderKind::OpenAICompatible => {
                let mut provider_builder = OpenAICompatible::<aisdk::core::DynamicModel>::builder()
                    .base_url(&base_url)
                    .model_name(&self.model_name)
                    .provider_name(&self.provider_name);

                if let Some(key) = self.api_key.as_deref() {
                    provider_builder = provider_builder.api_key(key);
                }

                let provider = provider_builder
                    .build()
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

                let mut builder = LanguageModelRequest::builder()
                    .model(provider)
                    .messages(aisdk_messages);

                for tool in aisdk_tools {
                    builder = builder.with_tool(tool);
                }

                builder.build().stream_text().await?
            }
            ProviderKind::Anthropic => {
                let mut provider_builder = Anthropic::<aisdk::core::DynamicModel>::builder()
                    .base_url(&base_url)
                    .model_name(&self.model_name)
                    .provider_name(&self.provider_name);

                if let Some(key) = self.api_key.as_deref() {
                    provider_builder = provider_builder.api_key(key);
                }

                let provider = provider_builder
                    .build()
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

                let mut builder = LanguageModelRequest::builder()
                    .model(provider)
                    .messages(aisdk_messages);

                for tool in aisdk_tools {
                    builder = builder.with_tool(tool);
                }

                builder.build().stream_text().await?
            }
            ProviderKind::OpenAI => {
                let mut provider_builder = OpenAI::<aisdk::core::DynamicModel>::builder()
                    .base_url(&base_url)
                    .model_name(&self.model_name)
                    .provider_name(&self.provider_name);

                if let Some(key) = self.api_key.as_deref() {
                    provider_builder = provider_builder.api_key(key);
                }

                let provider = provider_builder
                    .build()
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

                let mut builder = LanguageModelRequest::builder()
                    .model(provider)
                    .messages(aisdk_messages);

                for tool in aisdk_tools {
                    builder = builder.with_tool(tool);
                }

                builder.build().stream_text().await?
            }
        };

        let mut stream = response.stream;

        while let Some(chunk) = stream.next().await {
            on_chunk(chunk.clone());

            match chunk {
                LanguageModelStreamChunkType::Text(_text) => {}
                LanguageModelStreamChunkType::Reasoning(_reasoning) => {}
                LanguageModelStreamChunkType::ToolCall(_tool_call) => {}
                LanguageModelStreamChunkType::End(_msg) => {
                    break;
                }
                LanguageModelStreamChunkType::Start => {}
                LanguageModelStreamChunkType::Failed(_err) => {}
                LanguageModelStreamChunkType::Incomplete(_msg) => {}
                LanguageModelStreamChunkType::NotSupported(_msg) => {}
            }
        }

        Ok(())
    }

    fn convert_messages(&self, messages: &[crate::session::types::Message]) -> Vec<AisdkMessage> {
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
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = log("GOING TO STREAM");
    use std::time::Instant;

    let auth_dao = crate::persistence::AuthDAO::new()?;
    let auth_config = auth_dao.get_provider(&provider_name)?;
    let mut api_key = auth_config.as_ref().and_then(|config| match config {
        crate::persistence::AuthConfig::Api { key } => Some(key.clone()),
        crate::persistence::AuthConfig::OAuth { access, .. } => Some(access.clone()),
    });

    let discovery = crate::model::discovery::Discovery::new()?;

    let providers = discovery.fetch_providers().await?;

    let provider = providers
        .get(&provider_name)
        .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?;

    let npm_package = &provider.npm;
    let provider_kind = ProviderKind::from_provider(&provider_name, npm_package);
    let mut base_url = provider_kind.normalize_base_url(&provider.api);
    let mut effective_model = model.clone();
    let mut openai_response_path: Option<String> = None;
    let mut openai_additional_headers: HashMap<String, String> = HashMap::new();
    let mut openai_force_store_false = false;
    let mut openai_default_instructions: Option<String> = None;
    let mut openai_disallow_system_messages = false;
    let mut openai_force_tool_strict_false = false;

    if provider_kind == ProviderKind::OpenAI && provider_name == "openai" {
        if let Some(crate::persistence::AuthConfig::OAuth {
            refresh,
            access,
            expires,
            account_id,
            enterprise_url,
        }) = auth_config.clone()
        {
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
                            provider_name.clone(),
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
                        let _ = sender.send(crate::llm::ChunkMessage::Warning(format!(
                            "Failed to refresh OpenAI OAuth token: {}",
                            err
                        )));
                    }
                }
            }

            api_key = Some(oauth_access.clone());
            base_url = "https://chatgpt.com".to_string();
            openai_response_path = Some("/backend-api/codex/responses".to_string());
            openai_force_store_false = true;
            openai_default_instructions = Some(
                "You are Codex, a coding assistant focused on high-quality code changes."
                    .to_string(),
            );
            openai_disallow_system_messages = true;
            openai_force_tool_strict_false = true;

            openai_additional_headers.insert("originator".to_string(), "crabcode".to_string());
            openai_additional_headers.insert(
                "User-Agent".to_string(),
                crate::auth::openai_oauth::build_user_agent(),
            );

            if let Some(account_id) = oauth_account_id {
                openai_additional_headers.insert("ChatGPT-Account-Id".to_string(), account_id);
            }

            let _ = log("Configured OpenAI OAuth Codex transport");

            if !is_openai_oauth_model_allowed(&effective_model) {
                let fallback_model = "gpt-5.3-codex".to_string();
                let _ = sender.send(crate::llm::ChunkMessage::Warning(format!(
                    "Model '{}' is not supported for OpenAI OAuth. Falling back to '{}'.",
                    effective_model, fallback_model
                )));
                effective_model = fallback_model;
            }
        }
    }

    if api_key.is_none() {
        let _ = sender.send(crate::llm::ChunkMessage::Warning(format!(
            "No API key configured for '{}'. Trying anyway.",
            provider_name
        )));
    }

    let _ = log(&format!(
        "Provider: {}, NPM: {}, Base URL: {}, Model: {}",
        provider_name, npm_package, base_url, effective_model
    ));

    let aisdk_messages = convert_messages(&messages);

    let tool_registry = crate::tools::initialize_tool_registry().await;
    let aisdk_tools = convert_to_aisdk_tools(
        &tool_registry,
        Some(sender.clone()),
        agent_mode,
        tool_permissions,
    )
    .await;

    let mut response = match provider_kind {
        ProviderKind::OpenAICompatible => {
            let mut provider_builder = OpenAICompatible::<aisdk::core::DynamicModel>::builder()
                .base_url(&base_url)
                .model_name(&effective_model)
                .provider_name(&provider.name);

            if let Some(key) = api_key.as_deref() {
                provider_builder = provider_builder.api_key(key);
            }

            let provider_config = provider_builder
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            let mut builder = LanguageModelRequest::builder()
                .model(provider_config)
                .messages(aisdk_messages);

            if let Some(max_steps) = agent_max_steps {
                builder = builder.stop_when(step_count_is(max_steps));
            }

            for tool in aisdk_tools {
                builder = builder.with_tool(tool);
            }

            builder.build().stream_text().await?
        }
        ProviderKind::Anthropic => {
            let mut provider_builder = Anthropic::<aisdk::core::DynamicModel>::builder()
                .base_url(&base_url)
                .model_name(&effective_model)
                .provider_name(&provider.name);

            if let Some(key) = api_key.as_deref() {
                provider_builder = provider_builder.api_key(key);
            }

            let provider_config = provider_builder
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            let mut builder = LanguageModelRequest::builder()
                .model(provider_config)
                .messages(aisdk_messages);

            if let Some(max_steps) = agent_max_steps {
                builder = builder.stop_when(step_count_is(max_steps));
            }

            for tool in aisdk_tools {
                builder = builder.with_tool(tool);
            }

            builder.build().stream_text().await?
        }
        ProviderKind::OpenAI => {
            let mut provider_builder = OpenAI::<aisdk::core::DynamicModel>::builder()
                .base_url(&base_url)
                .model_name(&effective_model)
                .provider_name(&provider.name);

            if let Some(key) = api_key.as_deref() {
                provider_builder = provider_builder.api_key(key);
            }

            if let Some(response_path) = &openai_response_path {
                provider_builder = provider_builder.response_path(response_path);
            }

            if openai_force_store_false {
                provider_builder = provider_builder.force_store_false(true);
            }

            if let Some(instructions) = &openai_default_instructions {
                provider_builder = provider_builder.default_instructions(instructions.clone());
            }

            if openai_disallow_system_messages {
                provider_builder = provider_builder.disallow_system_messages(true);
            }

            if openai_force_tool_strict_false {
                provider_builder = provider_builder.force_tool_strict_false(true);
            }

            if !openai_additional_headers.is_empty() {
                provider_builder =
                    provider_builder.additional_headers(openai_additional_headers.clone());
            }

            let provider_config = provider_builder
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            let mut builder = LanguageModelRequest::builder()
                .model(provider_config)
                .messages(aisdk_messages);

            if let Some(max_steps) = agent_max_steps {
                builder = builder.stop_when(step_count_is(max_steps));
            }

            for tool in aisdk_tools {
                builder = builder.with_tool(tool);
            }

            builder.build().stream_text().await?
        }
    };

    let start_time = Instant::now();
    let mut token_count: usize = 0;
    let mut completed = false;

    while let Some(chunk) = response.stream.next().await {
        if cancel_token.is_cancelled() {
            let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
            return Err(anyhow::anyhow!("Streaming cancelled by user").into());
        }

        match chunk {
            LanguageModelStreamChunkType::Text(text) => {
                // Estimate tokens: ~4 characters per token on average
                token_count += text.chars().count().max(1) / 4;
                let _ = sender.send(crate::llm::ChunkMessage::Text(text));
            }
            LanguageModelStreamChunkType::Reasoning(reasoning) => {
                // Estimate tokens: ~4 characters per token on average
                token_count += reasoning.chars().count().max(1) / 4;
                let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
            }
            LanguageModelStreamChunkType::ToolCall(_tool_call) => {
                // Tool execution is handled internally by aisdk::stream_text().
                // We intentionally don't surface argument deltas here.
            }
            LanguageModelStreamChunkType::End(_msg) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                let _ = sender.send(crate::llm::ChunkMessage::Metrics {
                    token_count,
                    duration_ms,
                });
                let _ = sender.send(crate::llm::ChunkMessage::End);
                completed = true;
                break;
            }
            LanguageModelStreamChunkType::Start => {}
            LanguageModelStreamChunkType::Failed(err) => {
                let _ = sender.send(crate::llm::ChunkMessage::Failed(format!("{}", err)));
                let _ = log(&format!("Stream Chunk Failed {}", err));
                return Err(anyhow::anyhow!("Streaming failed: {}", err).into());
            }
            LanguageModelStreamChunkType::Incomplete(_msg) => {}
            LanguageModelStreamChunkType::NotSupported(_msg) => {}
        }
    }

    if completed {
        return Ok(());
    }

    let hit_step_limit =
        agent_max_steps.is_some() && matches!(response.stop_reason().await, Some(StopReason::Hook));

    if !hit_step_limit {
        return Ok(());
    }

    let _ = sender.send(crate::llm::ChunkMessage::Warning(
        "Maximum configured steps reached. Sending text-only summary.".to_string(),
    ));

    let mut follow_up_messages = response.messages().await;
    follow_up_messages.push(AisdkMessage::Assistant(
        MAX_STEPS_REACHED_PROMPT.to_string().into(),
    ));

    let mut summary_response = match provider_kind {
        ProviderKind::OpenAICompatible => {
            let mut provider_builder = OpenAICompatible::<aisdk::core::DynamicModel>::builder()
                .base_url(&base_url)
                .model_name(&effective_model)
                .provider_name(&provider.name);

            if let Some(key) = api_key.as_deref() {
                provider_builder = provider_builder.api_key(key);
            }

            let provider_config = provider_builder
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            LanguageModelRequest::builder()
                .model(provider_config)
                .messages(follow_up_messages)
                .build()
                .stream_text()
                .await?
        }
        ProviderKind::Anthropic => {
            let mut provider_builder = Anthropic::<aisdk::core::DynamicModel>::builder()
                .base_url(&base_url)
                .model_name(&effective_model)
                .provider_name(&provider.name);

            if let Some(key) = api_key.as_deref() {
                provider_builder = provider_builder.api_key(key);
            }

            let provider_config = provider_builder
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            LanguageModelRequest::builder()
                .model(provider_config)
                .messages(follow_up_messages)
                .build()
                .stream_text()
                .await?
        }
        ProviderKind::OpenAI => {
            let mut provider_builder = OpenAI::<aisdk::core::DynamicModel>::builder()
                .base_url(&base_url)
                .model_name(&effective_model)
                .provider_name(&provider.name);

            if let Some(key) = api_key.as_deref() {
                provider_builder = provider_builder.api_key(key);
            }

            if let Some(response_path) = &openai_response_path {
                provider_builder = provider_builder.response_path(response_path);
            }

            if openai_force_store_false {
                provider_builder = provider_builder.force_store_false(true);
            }

            if let Some(instructions) = &openai_default_instructions {
                provider_builder = provider_builder.default_instructions(instructions.clone());
            }

            if openai_disallow_system_messages {
                provider_builder = provider_builder.disallow_system_messages(true);
            }

            if openai_force_tool_strict_false {
                provider_builder = provider_builder.force_tool_strict_false(true);
            }

            if !openai_additional_headers.is_empty() {
                provider_builder =
                    provider_builder.additional_headers(openai_additional_headers.clone());
            }

            let provider_config = provider_builder
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            LanguageModelRequest::builder()
                .model(provider_config)
                .messages(follow_up_messages)
                .build()
                .stream_text()
                .await?
        }
    };

    while let Some(chunk) = summary_response.stream.next().await {
        if cancel_token.is_cancelled() {
            let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
            return Err(anyhow::anyhow!("Streaming cancelled by user").into());
        }

        match chunk {
            LanguageModelStreamChunkType::Text(text) => {
                token_count += text.chars().count().max(1) / 4;
                let _ = sender.send(crate::llm::ChunkMessage::Text(text));
            }
            LanguageModelStreamChunkType::Reasoning(reasoning) => {
                token_count += reasoning.chars().count().max(1) / 4;
                let _ = sender.send(crate::llm::ChunkMessage::Reasoning(reasoning));
            }
            LanguageModelStreamChunkType::ToolCall(_tool_call) => {}
            LanguageModelStreamChunkType::End(_msg) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                let _ = sender.send(crate::llm::ChunkMessage::Metrics {
                    token_count,
                    duration_ms,
                });
                let _ = sender.send(crate::llm::ChunkMessage::End);
                break;
            }
            LanguageModelStreamChunkType::Start => {}
            LanguageModelStreamChunkType::Failed(err) => {
                let _ = sender.send(crate::llm::ChunkMessage::Failed(format!("{}", err)));
                let _ = log(&format!("Stream Chunk Failed {}", err));
                return Err(anyhow::anyhow!("Streaming failed: {}", err).into());
            }
            LanguageModelStreamChunkType::Incomplete(_msg) => {}
            LanguageModelStreamChunkType::NotSupported(_msg) => {}
        }
    }

    Ok(())
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
    fn from_provider(provider_name: &str, npm_package: &str) -> Self {
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
