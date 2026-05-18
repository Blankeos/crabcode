use crate::chunk::ChunkType;
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::tool::Tool;
use async_trait::async_trait;
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

        let byte_stream = response.bytes_stream();
        let line_stream = bytes_to_lines(byte_stream);
        let stream = line_stream
            .map(|line| process_sse_data(&line))
            .flat_map(|v| stream::iter(v))
            .boxed();

        Ok(stream)
    }
}

fn debug_log(msg: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/crabcode_sse_debug.log")
        .and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{}", msg)
        });
}

fn process_sse_data(data: &str) -> Vec<Result<ChunkType>> {
    // [DONE] is ignored — the HTTP stream end signals completion.
    if data == "[DONE]" || data.is_empty() {
        debug_log(&format!("[SSE] Ignored: [DONE] or empty"));
        return vec![];
    }

    debug_log(&format!("[SSE] Raw data: {}", data));

    let value: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            debug_log(&format!("[SSE] JSON parse error: {} | data: {}", e, data));
            return vec![Ok(ChunkType::Failed(format!("Invalid SSE data: {}", e)))];
        }
    };

    if let Some(error) = value["error"].as_object() {
        let msg = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        debug_log(&format!("[SSE] API error: {}", msg));
        return vec![Ok(ChunkType::Failed(msg.to_string()))];
    }

    let Some(choices) = value["choices"].as_array() else {
        debug_log(&format!("[SSE] No choices array. JSON keys: {:?}", value.as_object().map(|o| o.keys().collect::<Vec<_>>())));
        return vec![];
    };

    if choices.is_empty() {
        debug_log("[SSE] choices array is empty");
        return vec![];
    }

    let choice = &choices[0];
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("");
    let mut chunks = Vec::new();

    // Log the full choice structure for debugging
    debug_log(&format!("[SSE] Choice JSON: {}", serde_json::to_string(choice).unwrap_or_default()));

    // Emit text delta first (may coexist with finish_reason)
    // Try standard delta.content, then fallbacks for non-standard providers
    let text = choice["delta"]["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| choice["delta"]["text"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| choice["message"]["content"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| choice["text"].as_str().filter(|s| !s.is_empty()));

    if let Some(delta) = text {
        debug_log(&format!("[SSE] Text chunk: {}", delta));
        chunks.push(Ok(ChunkType::Text(delta.to_string())));
    }

    // Emit reasoning delta
    let reasoning = choice["delta"]["reasoning_content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| choice["delta"]["reasoning"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| choice["reasoning_content"].as_str().filter(|s| !s.is_empty()));

    if let Some(reasoning) = reasoning {
        debug_log(&format!("[SSE] Reasoning chunk: {}", reasoning));
        chunks.push(Ok(ChunkType::Reasoning(reasoning.to_string())));
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

    if chunks.is_empty() {
        debug_log(&format!("[SSE] No chunks produced. finish_reason='{}'", finish_reason));
    }

    chunks
}

/// Convert a byte stream into a stream of lines, handling both SSE (`data: ...`) and raw NDJSON.
fn bytes_to_lines<S>(byte_stream: S) -> impl futures::Stream<Item = String>
where
    S: futures::Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let buffer: Vec<u8> = Vec::new();
    stream::unfold((byte_stream, buffer), |(mut stream, mut buffer)| async move {
        loop {
            if let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                let line = line.trim_end_matches('\n').trim_end_matches('\r');
                if line.is_empty() {
                    continue;
                }
                let data = if let Some(stripped) = line.strip_prefix("data:") {
                    stripped.trim_start().to_string()
                } else {
                    line.to_string()
                };
                if data == "[DONE]" || data.is_empty() {
                    continue;
                }
                debug_log(&format!("[LINE] Extracted: {}", data));
                return Some((data, (stream, buffer)));
            }
            match stream.next().await {
                Some(Ok(bytes)) => {
                    debug_log(&format!("[BYTES] Received {} bytes", bytes.len()));
                    buffer.extend_from_slice(&bytes);
                }
                Some(Err(e)) => {
                    debug_log(&format!("[BYTES] Error: {}", e));
                    return None;
                }
                None => {
                    let remaining = String::from_utf8_lossy(&buffer).trim().to_string();
                    buffer.clear();
                    if remaining.is_empty() || remaining == "[DONE]" {
                        debug_log("[LINE] Stream ended, no remaining data");
                        return None;
                    }
                    let data = if let Some(stripped) = remaining.strip_prefix("data:") {
                        stripped.trim_start().to_string()
                    } else {
                        remaining
                    };
                    debug_log(&format!("[LINE] Remaining at EOF: {}", data));
                    return Some((data, (stream, buffer)));
                }
            }
        }
    })
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
