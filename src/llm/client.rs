use aisdk::{
    core::{LanguageModelRequest, LanguageModelStreamChunkType, Message as AisdkMessage},
    providers::{OpenAI, OpenAICompatible},
};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::logging::log;

pub struct LLMClient {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
    npm_package: String,
}

impl LLMClient {
    pub fn new(
        base_url: String,
        api_key: String,
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

    fn uses_openai_compatible(&self) -> bool {
        // Check if the npm package indicates OpenAI-compatible API
        self.npm_package == "@ai-sdk/openai-compatible"
    }

    pub async fn stream_chat(
        &self,
        messages: &[crate::session::types::Message],
        mut on_chunk: impl FnMut(LanguageModelStreamChunkType),
    ) -> Result<(), Box<dyn std::error::Error>> {
        let aisdk_messages = self.convert_messages(messages);

        let response = if self.uses_openai_compatible() {
            let provider = OpenAICompatible::<aisdk::core::DynamicModel>::builder()
                .base_url(&self.base_url)
                .api_key(&self.api_key)
                .model_name(&self.model_name)
                .provider_name(&self.provider_name)
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            LanguageModelRequest::builder()
                .model(provider)
                .messages(aisdk_messages)
                .build()
                .stream_text()
                .await?
        } else {
            let provider = OpenAI::<aisdk::core::DynamicModel>::builder()
                .base_url(&self.base_url)
                .api_key(&self.api_key)
                .model_name(&self.model_name)
                .provider_name(&self.provider_name)
                .build()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            LanguageModelRequest::builder()
                .model(provider)
                .messages(aisdk_messages)
                .build()
                .stream_text()
                .await?
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
    messages: Vec<crate::session::types::Message>,
    sender: crate::llm::ChunkSender,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    let auth_dao = crate::persistence::AuthDAO::new()?;

    let api_key = auth_dao
        .get_api_key(&provider_name)?
        .ok_or_else(|| anyhow::anyhow!("No API key found for {}", provider_name))?;

    let discovery = crate::model::discovery::Discovery::new()?;

    let providers = discovery.fetch_providers().await?;

    let provider = providers
        .get(&provider_name)
        .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_name))?;

    let base_url = &provider.api;
    let npm_package = &provider.npm;

    let _ = log(&format!(
        "Provider: {}, NPM: {}, Base URL: {}",
        provider_name, npm_package, base_url
    ));

    // Determine which provider to use based on npm package
    let uses_openai_compatible = npm_package == "@ai-sdk/openai-compatible";

    let aisdk_messages = convert_messages(&messages);

    let response = if uses_openai_compatible {
        let provider_config = OpenAICompatible::<aisdk::core::DynamicModel>::builder()
            .base_url(base_url)
            .api_key(&api_key)
            .model_name(&model)
            .provider_name(&provider.name)
            .build()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        LanguageModelRequest::builder()
            .model(provider_config)
            .messages(aisdk_messages)
            .build()
            .stream_text()
            .await?
    } else {
        let provider_config = OpenAI::<aisdk::core::DynamicModel>::builder()
            .base_url(base_url)
            .api_key(&api_key)
            .model_name(&model)
            .provider_name(&provider.name)
            .build()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        LanguageModelRequest::builder()
            .model(provider_config)
            .messages(aisdk_messages)
            .build()
            .stream_text()
            .await?
    };

    let mut stream = response.stream;
    let start_time = Instant::now();
    let mut token_count: usize = 0;

    while let Some(chunk) = stream.next().await {
        if cancel_token.is_cancelled() {
            let _ = sender.send(crate::llm::ChunkMessage::Cancelled);
            return Err(anyhow::anyhow!("Streaming cancelled by user").into());
        }

        match chunk {
            LanguageModelStreamChunkType::Text(text) => {
                token_count += text.split_whitespace().count();
                let _ = sender.send(crate::llm::ChunkMessage::Text(text));
            }
            LanguageModelStreamChunkType::Reasoning(reasoning) => {
                token_count += reasoning.split_whitespace().count();
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
