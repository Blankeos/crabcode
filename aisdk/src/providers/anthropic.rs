use crate::chunk::ChunkType;
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::tool::Tool;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Anthropic {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
}

impl Anthropic {
    pub fn builder() -> AnthropicBuilder {
        AnthropicBuilder::default()
    }
}

#[derive(Default)]
pub struct AnthropicBuilder {
    base_url: Option<String>,
    api_key: Option<String>,
    model_name: Option<String>,
    provider_name: Option<String>,
}

impl AnthropicBuilder {
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn model_name(mut self, name: impl Into<String>) -> Self {
        self.model_name = Some(name.into());
        self
    }

    pub fn provider_name(mut self, name: impl Into<String>) -> Self {
        self.provider_name = Some(name.into());
        self
    }

    pub fn build(self) -> Result<Anthropic> {
        Ok(Anthropic {
            base_url: self.base_url.ok_or(Error::MissingField("base_url".into()))?,
            api_key: self.api_key.ok_or(Error::MissingField("api_key".into()))?,
            model_name: self.model_name.ok_or(Error::MissingField("model_name".into()))?,
            provider_name: self.provider_name.unwrap_or_else(|| "anthropic".to_string()),
        })
    }
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    async fn stream_text(
        &self,
        messages: &[Message],
        tools: &[Tool],
        _headers: &HashMap<String, String>,
    ) -> Result<ProviderStream> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));

        let system_prompts: Vec<serde_json::Value> = messages
            .iter()
            .filter_map(|m| match m {
                Message::System(s) => Some(serde_json::json!({
                    "type": "text",
                    "text": s.content,
                })),
                _ => None,
            })
            .collect();

        let user_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(serde_json::json!({
                    "role": "user",
                    "content": u.content,
                })),
                Message::Assistant(a) => Some(serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                })),
                _ => None,
            })
            .collect();

        let tool_params: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                let schema = serde_json::to_value(&t.input_schema).unwrap_or_default();
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": schema,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model_name,
            "messages": user_messages,
            "max_tokens": 32000,
            "stream": true,
        });

        if !system_prompts.is_empty() {
            body["system"] = serde_json::Value::Array(system_prompts);
        }

        if !tool_params.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_params);
        }

        let mut request_headers = reqwest::header::HeaderMap::new();
        request_headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        request_headers.insert("x-api-key", self.api_key.parse().unwrap());
        request_headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Provider(format!("Failed to build client: {}", e)))?;
        let response = client
            .post(&url)
            .headers(request_headers)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Provider(format!(
                "Anthropic API error {}: {}",
                status, text
            )));
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(|ev| {
                match ev {
                    Ok(event) => {
                        let event_type = event.event.as_str();
                        let data = &event.data;

                        if data.is_empty() {
                            return futures::future::ready(None);
                        }

                        match serde_json::from_str::<serde_json::Value>(data) {
                            Ok(value) => match event_type {
                                "content_block_delta" => {
                                    let delta = &value["delta"];
                                    match delta["type"].as_str() {
                                        Some("text_delta") => {
                                            futures::future::ready(
                                                delta["text"].as_str().map(|t| {
                                                    Ok(ChunkType::Text(t.to_string()))
                                                }),
                                            )
                                        }
                                        Some("thinking_delta") => {
                                            futures::future::ready(
                                                delta["thinking"].as_str().map(|t| {
                                                    Ok(ChunkType::Reasoning(t.to_string()))
                                                }),
                                            )
                                        }
                                        Some("input_json_delta") => {
                                            futures::future::ready(
                                                delta["partial_json"].as_str().map(|j| {
                                                    Ok(ChunkType::ToolCall(j.to_string()))
                                                }),
                                            )
                                        }
                                        _ => futures::future::ready(None),
                                    }
                                }
                                "message_delta" => {
                                    // Stream exhausts naturally after message_stop
                                    futures::future::ready(None)
                                }
                                "error" => {
                                    let error_msg = value["error"]["message"]
                                        .as_str()
                                        .unwrap_or("Unknown error");
                                    futures::future::ready(Some(Ok(ChunkType::Failed(
                                        error_msg.to_string(),
                                    ))))
                                }
                                _ => futures::future::ready(None),
                            },
                            Err(e) => futures::future::ready(Some(Ok(ChunkType::Failed(
                                format!("Invalid SSE data: {}", e),
                            )))),
                        }
                    }
                    Err(e) => {
                        let err = format!("SSE error: {}", e);
                        futures::future::ready(Some(Ok(ChunkType::Failed(err))))
                    }
                }
            })
            .boxed();

        Ok(stream)
    }
}
