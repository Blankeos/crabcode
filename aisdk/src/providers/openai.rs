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
pub struct OpenAI {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
    responses_path: String,
    headers: HashMap<String, String>,
    store_override: Option<bool>,
    strip_system_and_developer_messages: bool,
    tool_strict_override: Option<bool>,
    default_instructions: Option<String>,
}

impl OpenAI {
    pub fn builder() -> OpenAIBuilder {
        OpenAIBuilder::default()
    }
}

#[derive(Default)]
pub struct OpenAIBuilder {
    base_url: Option<String>,
    api_key: Option<String>,
    model_name: Option<String>,
    provider_name: Option<String>,
    responses_path: String,
    headers: HashMap<String, String>,
    store_override: Option<bool>,
    strip_system_and_developer_messages: bool,
    tool_strict_override: Option<bool>,
    default_instructions: Option<String>,
}

impl OpenAIBuilder {
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

    pub fn responses_path(mut self, path: impl Into<String>) -> Self {
        self.responses_path = path.into();
        self
    }

    pub fn headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub fn store_override(mut self, store: bool) -> Self {
        self.store_override = Some(store);
        self
    }

    pub fn strip_system_and_developer_messages(mut self, enabled: bool) -> Self {
        self.strip_system_and_developer_messages = enabled;
        self
    }

    pub fn tool_strict_override(mut self, strict: bool) -> Self {
        self.tool_strict_override = Some(strict);
        self
    }

    pub fn default_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.default_instructions = Some(instructions.into());
        self
    }

    pub fn build(self) -> Result<OpenAI> {
        let base_url = self
            .base_url
            .ok_or(Error::MissingField("base_url".into()))?;
        let api_key = self.api_key.ok_or(Error::MissingField("api_key".into()))?;
        let model_name = self
            .model_name
            .ok_or(Error::MissingField("model_name".into()))?;
        let provider_name = self.provider_name.unwrap_or_else(|| "openai".to_string());

        let responses_path = {
            let trimmed = self.responses_path.trim();
            if trimmed.is_empty() {
                "/v1/responses".to_string()
            } else if trimmed.starts_with('/') {
                trimmed.to_string()
            } else {
                format!("/{trimmed}")
            }
        };

        Ok(OpenAI {
            base_url,
            api_key,
            model_name,
            provider_name,
            responses_path,
            headers: self.headers,
            store_override: self.store_override,
            strip_system_and_developer_messages: self.strip_system_and_developer_messages,
            tool_strict_override: self.tool_strict_override,
            default_instructions: self.default_instructions,
        })
    }
}

#[async_trait]
impl Provider for OpenAI {
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
        headers: &HashMap<String, String>,
    ) -> Result<ProviderStream> {
        let url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.responses_path
        );

        let mut request_headers = reqwest::header::HeaderMap::new();
        request_headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        if !self.api_key.is_empty() {
            request_headers.insert(
                "Authorization",
                format!("Bearer {}", self.api_key).parse().unwrap(),
            );
        }

        for (k, v) in &self.headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                request_headers.insert(name, value);
            }
        }

        for (k, v) in headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                request_headers.insert(name, value);
            }
        }

        let input = build_openai_messages(messages, self.strip_system_and_developer_messages);

        let tool_params: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                let schema = serde_json::to_value(&t.input_schema).unwrap_or_default();
                let mut tool = serde_json::json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": schema,
                });

                if let Some(strict) = self.tool_strict_override {
                    tool = serde_json::json!({
                        "type": "function",
                        "name": t.name,
                        "strict": strict,
                        "parameters": schema,
                        "description": t.description,
                    });
                }

                tool
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model_name,
            "input": input,
            "stream": true,
        });

        if !tool_params.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_params);
        }

        if let Some(instructions) = &self.default_instructions {
            body["instructions"] = serde_json::Value::String(instructions.clone());
        }

        if let Some(store) = self.store_override {
            body["store"] = serde_json::Value::Bool(store);
        }

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
                "OpenAI API error {}: {}",
                status, text
            )));
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(|ev| {
                match ev {
                    Ok(event) => {
                        let data = &event.data;
                        // [DONE] / empty data → stream exhausts naturally
                        if data == "[DONE]" || data.is_empty() {
                            return futures::future::ready(None);
                        }

                        match serde_json::from_str::<serde_json::Value>(data) {
                            Ok(value) => {
                                let event_type = value["type"].as_str().unwrap_or("");

                                match event_type {
                                    "response.output_text.delta" => {
                                        let delta = value["delta"].as_str().unwrap_or("");
                                        futures::future::ready(Some(Ok(ChunkType::Text(
                                            delta.to_string(),
                                        ))))
                                    }
                                    "response.reasoning_summary_text.delta" => {
                                        let delta = value["delta"].as_str().unwrap_or("");
                                        futures::future::ready(Some(Ok(ChunkType::Reasoning(
                                            delta.to_string(),
                                        ))))
                                    }
                                    "response.completed" => {
                                        let resp = &value["response"];
                                        if let Some(error) = resp.get("error") {
                                            if let Some(code) = error.get("code") {
                                                return futures::future::ready(Some(Ok(
                                                    ChunkType::Failed(code.to_string()),
                                                )));
                                            }
                                        }
                                        // Stream exhausts naturally — no End chunk forwarded
                                        futures::future::ready(None)
                                    }
                                    "response.incomplete" => futures::future::ready(Some(Ok(
                                        ChunkType::Incomplete("Response incomplete".to_string()),
                                    ))),
                                    "response.failed" => futures::future::ready(Some(Ok(
                                        ChunkType::Failed("Response failed".to_string()),
                                    ))),
                                    _ => {
                                        if event_type.contains("tool_call") {
                                            futures::future::ready(Some(Ok(ChunkType::ToolCall(
                                                data.clone(),
                                            ))))
                                        } else {
                                            futures::future::ready(None)
                                        }
                                    }
                                }
                            }
                            Err(e) => futures::future::ready(Some(Ok(ChunkType::Failed(format!(
                                "Invalid SSE data: {}",
                                e
                            ))))),
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

fn build_openai_messages(messages: &[Message], strip_system: bool) -> Vec<serde_json::Value> {
    messages
        .iter()
        .filter_map(|msg| {
            if strip_system {
                if let Message::System(_) = msg {
                    return None;
                }
            }
            match msg {
                Message::System(s) => Some(serde_json::json!({
                    "role": "system",
                    "content": s.content,
                })),
                Message::User(u) => Some(serde_json::json!({
                    "role": "user",
                    "content": u.content,
                })),
                Message::Assistant(a) => Some(serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                })),
            }
        })
        .collect()
}
