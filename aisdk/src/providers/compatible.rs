use crate::chunk::ChunkType;
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::tool::Tool;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream;
use futures::StreamExt;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct OpenAICompatible {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
}

impl OpenAICompatible {
    pub fn builder() -> OpenAICompatibleBuilder {
        OpenAICompatibleBuilder::default()
    }
}

#[derive(Default)]
pub struct OpenAICompatibleBuilder {
    base_url: Option<String>,
    api_key: Option<String>,
    model_name: Option<String>,
    provider_name: Option<String>,
}

impl OpenAICompatibleBuilder {
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

    pub fn build(self) -> Result<OpenAICompatible> {
        Ok(OpenAICompatible {
            base_url: self.base_url.ok_or(Error::MissingField("base_url".into()))?,
            api_key: self.api_key.ok_or(Error::MissingField("api_key".into()))?,
            model_name: self.model_name.ok_or(Error::MissingField("model_name".into()))?,
            provider_name: self
                .provider_name
                .unwrap_or_else(|| "openai-compatible".to_string()),
        })
    }
}

#[async_trait]
impl Provider for OpenAICompatible {
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
        let base = self.base_url.trim_end_matches('/');
        let url = if has_version_segment(base) {
            format!("{}/chat/completions", base)
        } else {
            format!("{}/v1/chat/completions", base)
        };

        let chat_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| match m {
                Message::System(s) => serde_json::json!({
                    "role": "system",
                    "content": s.content,
                }),
                Message::User(u) => serde_json::json!({
                    "role": "user",
                    "content": u.content,
                }),
                Message::Assistant(a) => serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                }),
            })
            .collect();

        let tool_params: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                let schema = serde_json::to_value(&t.input_schema).unwrap_or_default();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": schema,
                    }
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model_name,
            "messages": chat_messages,
            "stream": true,
        });

        if !tool_params.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_params);
        }

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
            return Err(Error::Provider(format!("API error {}: {}", status, text)));
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .map(|ev| {
                match ev {
                    Ok(event) => process_sse_data(&event.data),
                    Err(e) => vec![Ok(ChunkType::Failed(format!("SSE error: {}", e)))],
                }
            })
            .flat_map(|v| stream::iter(v))
            .boxed();

        Ok(stream)
    }
}

fn process_sse_data(data: &str) -> Vec<Result<ChunkType>> {
    // [DONE] is ignored — the HTTP stream end signals completion.
    if data == "[DONE]" || data.is_empty() {
        return vec![];
    }

    let value: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => return vec![Ok(ChunkType::Failed(format!("Invalid SSE data: {}", e)))],
    };

    if let Some(error) = value["error"].as_object() {
        let msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return vec![Ok(ChunkType::Failed(msg.to_string()))];
    }

    let Some(choices) = value["choices"].as_array() else {
        return vec![];
    };

    if choices.is_empty() {
        return vec![];
    }

    let choice = &choices[0];
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("");
    let mut chunks = Vec::new();

    // Emit text delta first (may coexist with finish_reason)
    if let Some(delta) = choice["delta"]["content"].as_str() {
        if !delta.is_empty() {
            chunks.push(Ok(ChunkType::Text(delta.to_string())));
        }
    }

    // Emit reasoning delta
    if let Some(reasoning) = choice["delta"]["reasoning_content"].as_str() {
        if !reasoning.is_empty() {
            chunks.push(Ok(ChunkType::Reasoning(reasoning.to_string())));
        }
    }

    // Emit tool calls on tool_calls finish_reason. Stream exhausts naturally
    // for all other finish_reasons — no explicit End chunk needed.
    if finish_reason == "tool_calls" || finish_reason == "function_call" {
        if let Some(tool_calls) = choice["delta"]["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                let json = serde_json::to_string(tool_calls).unwrap_or_default();
                chunks.push(Ok(ChunkType::ToolCall(json)));
            }
        }
    }

    chunks
}

fn has_version_segment(base_url: &str) -> bool {
    // Check if the URL path already contains a /vN segment (e.g., /v4, /v1)
    if let Some(pos) = base_url.find("://") {
        let after_scheme = &base_url[pos + 3..];
        if let Some(path_start) = after_scheme.find('/') {
            let path = &after_scheme[path_start..];
            // Match /vN where N is one or more digits, followed by / or end of string
            let bytes = path.as_bytes();
            for i in 0..bytes.len().saturating_sub(2) {
                if bytes[i] == b'/'
                    && bytes[i + 1] == b'v'
                    && bytes[i + 2].is_ascii_digit()
                    && (i + 3 >= bytes.len() || bytes[i + 3] == b'/')
                {
                    return true;
                }
            }
        }
    }
    false
}
