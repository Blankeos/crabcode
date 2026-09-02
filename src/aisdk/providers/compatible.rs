use crate::chunk::{ChunkType, FinishReason};
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::retry::RetryError;
use crate::tool::Tool;
use async_trait::async_trait;
use futures::stream;
use futures::StreamExt;
use std::collections::HashMap;

const COMPATIBLE_STREAM_CONNECT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct OpenAICompatible {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
    reasoning_effort: Option<String>,
    prompt_cache_key: Option<String>,

    /// Vercel AI Gateway: set `providerOptions.gateway.caching = "auto"` so
    /// Anthropic (and MiniMax) models get explicit cache breakpoints.
    gateway_caching_auto: bool,
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
    reasoning_effort: Option<String>,
    prompt_cache_key: Option<String>,

    gateway_caching_auto: bool,
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

    pub fn reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    pub fn prompt_cache_key(mut self, key: impl Into<String>) -> Self {
        self.prompt_cache_key = Some(key.into());
        self
    }

    pub fn gateway_caching_auto(mut self, enabled: bool) -> Self {
        self.gateway_caching_auto = enabled;
        self
    }

    pub fn build(self) -> Result<OpenAICompatible> {
        Ok(OpenAICompatible {
            base_url: self
                .base_url
                .ok_or(Error::MissingField("base_url".into()))?,
            api_key: self.api_key.unwrap_or_default(),
            model_name: self
                .model_name
                .ok_or(Error::MissingField("model_name".into()))?,
            provider_name: self
                .provider_name
                .unwrap_or_else(|| "openai-compatible".to_string()),
            reasoning_effort: self.reasoning_effort,
            prompt_cache_key: self.prompt_cache_key,

            gateway_caching_auto: self.gateway_caching_auto,
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

        let include_empty_tool_call_reasoning =
            openai_compatible_requires_tool_call_reasoning_content(self);
        let chat_messages = openai_compatible_messages(messages, include_empty_tool_call_reasoning);

        let mut tool_params: Vec<serde_json::Value> = Vec::new();
        let mut plugins: Vec<serde_json::Value> = Vec::new();
        for t in tools {
            match &t.transport {
                crate::aisdk::tool::ToolTransport::ClientFunction => {
                    let schema = serde_json::to_value(&t.input_schema).unwrap_or_default();
                    tool_params.push(serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": schema,
                        }
                    }));
                }
                crate::aisdk::tool::ToolTransport::ProviderNative(value) => {
                    // Some OpenAI-compatible gateways accept Responses-style tools.
                    tool_params.push(value.clone());
                }
                crate::aisdk::tool::ToolTransport::OpenRouterPlugin(value) => {
                    plugins.push(value.clone());
                }
            }
        }

        let mut body = serde_json::json!({
            "model": self.model_name,
            "messages": chat_messages,
            "stream": true,
        });

        if !tool_params.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_params);
        }

        if let Some(effort) = &self.reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(effort.clone());
        }

        if let Some(key) = &self.prompt_cache_key {
            if !key.is_empty() {
                body["prompt_cache_key"] = serde_json::Value::String(key.clone());
            }
        }

        if !plugins.is_empty() {
            body["plugins"] = serde_json::Value::Array(plugins);
        }

        // AI Gateway Chat Completions: enable automatic prompt caching for
        // providers that need explicit markers (Anthropic / MiniMax).
        // https://vercel.com/docs/ai-gateway/models-and-providers/automatic-caching
        if self.gateway_caching_auto {
            body["providerOptions"] = serde_json::json!({
                "gateway": { "caching": "auto" }
            });
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
            .connect_timeout(std::time::Duration::from_secs(
                COMPATIBLE_STREAM_CONNECT_TIMEOUT_SECS,
            ))
            .build()
            .map_err(|e| Error::Provider(format!("Failed to build client: {}", e)))?;
        let response = client
            .post(&url)
            .headers(request_headers)
            .json(&body)
            .send()
            .await
            .map_err(|err| Error::RetryableProvider(RetryError::from_message(err.to_string())))?;

        if !response.status().is_success() {
            let status = response.status();
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            let retry_error = RetryError::new(format!("API error {}: {}", status, text))
                .with_status(status.as_u16())
                .with_headers(&headers);
            if crate::retry::retryable(&retry_error) {
                return Err(Error::RetryableProvider(retry_error));
            }
            return Err(Error::Provider(retry_error.message));
        }

        let byte_stream = response.bytes_stream();
        let line_stream = bytes_to_lines(byte_stream);
        let stream = line_stream
            .flat_map(|line| match line {
                Ok(line) => stream::iter(process_sse_data(&line)),
                Err(err) => stream::iter(vec![Err(err)]),
            })
            .boxed();

        Ok(stream)
    }
}

fn openai_compatible_user_content(user: &crate::message::UserMessage) -> serde_json::Value {
    if user.images.is_empty() && user.audios.is_empty() {
        return serde_json::json!(user.content);
    }

    let mut parts = Vec::new();
    if !user.content.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": user.content,
        }));
    }
    parts.extend(user.images.iter().map(|image| {
        serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": image.data_url,
            },
        })
    }));
    parts.extend(user.audios.iter().map(|audio| {
        serde_json::json!({
            "type": "input_audio",
            "input_audio": {
                "data": audio.data,
                "format": audio.format,
            },
        })
    }));
    serde_json::Value::Array(parts)
}

fn openai_compatible_messages(
    messages: &[Message],
    include_empty_tool_call_reasoning: bool,
) -> Vec<serde_json::Value> {
    let mut chat_messages = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        match &messages[index] {
            Message::System(s) => {
                chat_messages.push(serde_json::json!({
                    "role": "system",
                    "content": s.content,
                }));
                index += 1;
            }
            Message::User(u) => {
                chat_messages.push(serde_json::json!({
                    "role": "user",
                    "content": openai_compatible_user_content(u),
                }));
                index += 1;
            }
            Message::Assistant(a) => {
                chat_messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                }));
                index += 1;
            }
            Message::Reasoning(_) => {
                // Chat Completions has no reasoning item; the following
                // tool-call group copies summary into `reasoning_content`.
                index += 1;
            }
            Message::ToolCall(_) => {
                let mut tool_calls = Vec::new();
                let mut reasoning_content = None;
                if index > 0 {
                    if let Some(Message::Reasoning(reasoning)) = messages.get(index - 1) {
                        if !reasoning.summary.is_empty() {
                            reasoning_content = Some(reasoning.summary.clone());
                        }
                    }
                }

                while let Some(Message::ToolCall(tool)) = messages.get(index) {
                    if reasoning_content.is_none() {
                        reasoning_content = tool.reasoning_content.clone();
                    }
                    tool_calls.push(openai_compatible_tool_call(tool));
                    index += 1;
                }

                chat_messages.push(openai_compatible_tool_call_message_from_calls(
                    tool_calls,
                    reasoning_content,
                    include_empty_tool_call_reasoning,
                ));
            }
            Message::ToolOutput(t) => {
                chat_messages.extend(openai_compatible_tool_output_messages(t));
                index += 1;
            }
        }
    }

    chat_messages
}

fn openai_compatible_tool_call(tool: &crate::message::ToolCallMessage) -> serde_json::Value {
    serde_json::json!({
        "id": tool.call_id,
        "type": "function",
        "function": {
            "name": tool.name,
            "arguments": tool.arguments,
        }
    })
}

fn openai_compatible_tool_call_message_from_calls(
    tool_calls: Vec<serde_json::Value>,
    reasoning_content: Option<String>,
    include_empty_reasoning_content: bool,
) -> serde_json::Value {
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": serde_json::Value::Null,
        "tool_calls": tool_calls,
    });

    if let Some(reasoning_content) = reasoning_content {
        message["reasoning_content"] = serde_json::Value::String(reasoning_content);
    } else if include_empty_reasoning_content {
        message["reasoning_content"] = serde_json::Value::String(String::new());
    }

    message
}

fn openai_compatible_requires_tool_call_reasoning_content(provider: &OpenAICompatible) -> bool {
    let model = provider.model_name.to_ascii_lowercase();
    let provider_name = provider.provider_name.to_ascii_lowercase();
    let base_url = provider.base_url.to_ascii_lowercase();

    model.contains("kimi") || provider_name.contains("moonshot") || base_url.contains("moonshot")
}

fn openai_compatible_tool_output_messages(
    tool: &crate::message::ToolOutputMessage,
) -> Vec<serde_json::Value> {
    let mut messages = vec![serde_json::json!({
        "role": "tool",
        "tool_call_id": tool.call_id,
        "name": tool.name,
        "content": tool.output,
    })];

    if !tool.images.is_empty() {
        messages.push(serde_json::json!({
            "role": "user",
            "content": openai_compatible_image_content(
                &format!("Image returned by tool `{}`.", tool.name),
                &tool.images,
            ),
        }));
    }

    messages
}

fn openai_compatible_image_content(
    text: &str,
    images: &[crate::message::ImageContent],
) -> serde_json::Value {
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": text,
        }));
    }
    parts.extend(images.iter().map(|image| {
        serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": image.data_url,
            },
        })
    }));
    serde_json::Value::Array(parts)
}

fn debug_log(msg: &str) {
    #[cfg(feature = "aisdk-sse-debug")]
    {
        use std::env;
        use std::io::Write;
        let path = env::var("AISDK_SSE_DEBUG_LOG")
            .unwrap_or_else(|_| "/tmp/aisdk_sse_debug.log".to_string());
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{}", msg));
    }
    #[cfg(not(feature = "aisdk-sse-debug"))]
    {
        let _ = msg;
    }
}

/// Log OpenAI-compatible / AI Gateway usage via the host logger.
/// Looks for `prompt_tokens_details.cached_tokens` and Anthropic-style fields
/// that some gateways forward.
fn openai_compatible_usage(usage: &serde_json::Value) -> Option<crate::chunk::LanguageModelUsage> {
    let prompt = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_u64());
    let completion = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|v| v.as_u64());
    let cached = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.get("cache_write_input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if prompt.is_none()
        && completion.is_none()
        && cached == 0
        && cache_read == 0
        && cache_creation == 0
    {
        return None;
    }

    // Prefer OpenAI-style cached_tokens; fall back to Anthropic-style cache_read.
    let effective_cached = if cached > 0 { cached } else { cache_read };
    let prompt_v = prompt.unwrap_or(0);
    let hit_pct = if prompt_v > 0 {
        (effective_cached as f64 * 100.0) / prompt_v as f64
    } else if effective_cached > 0 || cache_creation > 0 {
        let total = effective_cached.saturating_add(cache_creation);
        if total > 0 {
            (effective_cached as f64 * 100.0) / total as f64
        } else {
            0.0
        }
    } else {
        0.0
    };

    crate::log::log(&format!(
        "[prompt-cache] openai-compatible prompt={} completion={} cached_tokens={} cache_read={} cache_creation={} hit_pct={:.1}",
        prompt.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        completion
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into()),
        cached,
        cache_read,
        cache_creation,
        hit_pct
    ));

    let anthropic_shape = cached == 0 && (cache_read > 0 || cache_creation > 0);
    let input_tokens = if anthropic_shape {
        prompt
            .unwrap_or(0)
            .saturating_add(cache_read)
            .saturating_add(cache_creation)
    } else {
        prompt.unwrap_or(0)
    };
    Some(crate::chunk::LanguageModelUsage {
        input_tokens,
        output_tokens: completion.unwrap_or(0),
        cache_read_tokens: effective_cached,
        cache_write_tokens: cache_creation,
    })
}

fn process_sse_data(data: &str) -> Vec<Result<ChunkType>> {
    let data = data.trim();
    let mut chunks = Vec::new();

    if data == "[DONE]" {
        debug_log("[SSE] Terminal: [DONE]");
        return vec![Ok(ChunkType::End { reason: None })];
    }

    if data.is_empty() || is_sse_metadata_line(data) {
        debug_log("[SSE] Ignored: empty or metadata/comment");
        return chunks;
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

    // Final usage often arrives on a choices-empty (or choices-missing) chunk.
    // Log cache-related fields so gateway Anthropic hits are verifiable.
    let usage = value.get("usage").and_then(openai_compatible_usage);
    if let Some(usage) = usage {
        chunks.push(Ok(ChunkType::Usage(usage)));
    }

    let Some(choices) = value["choices"].as_array() else {
        debug_log(&format!(
            "[SSE] No choices array. JSON keys: {:?}",
            value.as_object().map(|o| o.keys().collect::<Vec<_>>())
        ));
        return chunks;
    };

    if choices.is_empty() {
        debug_log("[SSE] choices array is empty");
        return chunks;
    }

    let choice = &choices[0];
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("");
    // Log the full choice structure for debugging
    debug_log(&format!(
        "[SSE] Choice JSON: {}",
        serde_json::to_string(choice).unwrap_or_default()
    ));

    // Emit text delta first (may coexist with finish_reason)
    // Try standard delta.content, then fallbacks for non-standard providers
    let text = choice["delta"]["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| choice["delta"]["text"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| {
            choice["message"]["content"]
                .as_str()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| choice["text"].as_str().filter(|s| !s.is_empty()));

    if let Some(delta) = text {
        debug_log(&format!("[SSE] Text chunk: {}", delta));
        chunks.push(Ok(ChunkType::Text(delta.to_string())));
    }

    // Emit reasoning delta
    let reasoning = choice["delta"]["reasoning_content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            choice["delta"]["reasoning"]
                .as_str()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            choice["reasoning_content"]
                .as_str()
                .filter(|s| !s.is_empty())
        });

    if let Some(reasoning) = reasoning {
        debug_log(&format!("[SSE] Reasoning chunk: {}", reasoning));
        chunks.push(Ok(ChunkType::Reasoning(reasoning.to_string())));
    }

    if let Some(tool_calls) = choice["delta"]["tool_calls"].as_array() {
        if !tool_calls.is_empty() {
            let json = serde_json::to_string(tool_calls).unwrap_or_default();
            debug_log(&format!(
                "[SSE] Tool call delta: count={} finish_reason='{}'",
                tool_calls.len(),
                finish_reason
            ));
            chunks.push(Ok(ChunkType::ToolCall(json)));
        }
    }

    match finish_reason {
        "" => {}
        _ => chunks.push(Ok(ChunkType::End {
            reason: Some(FinishReason::from_openai_compatible(finish_reason)),
        })),
    }

    if chunks.is_empty() {
        debug_log(&format!(
            "[SSE] No chunks produced. finish_reason='{}'",
            finish_reason
        ));
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call_chunks(data: &str) -> Vec<String> {
        process_sse_data(data)
            .into_iter()
            .filter_map(|chunk| match chunk.expect("chunk should parse") {
                ChunkType::ToolCall(value) => Some(value),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn builder_allows_missing_api_key() {
        let provider = OpenAICompatible::builder()
            .base_url("http://localhost:11434/v1")
            .model_name("llama3.2:latest")
            .provider_name("ollama")
            .build()
            .expect("api key should be optional");

        assert!(provider.api_key.is_empty());
    }

    #[test]
    fn serializes_audio_input_content_part() {
        let user = crate::message::UserMessage {
            content: "Describe this".to_string(),
            images: Vec::new(),
            audios: vec![crate::message::AudioContent {
                data: "YXVkaW8=".to_string(),
                format: "wav".to_string(),
                media_type: "audio/wav".to_string(),
            }],
        };

        assert_eq!(
            openai_compatible_user_content(&user),
            serde_json::json!([
                {"type": "text", "text": "Describe this"},
                {"type": "input_audio", "input_audio": {"data": "YXVkaW8=", "format": "wav"}}
            ])
        );
    }

    #[test]
    fn responses_style_usage_aliases_are_normalized() {
        let chunks = process_sse_data(
            r#"{"choices":[],"usage":{"input_tokens":120,"output_tokens":30,"cached_tokens":80,"cache_write_input_tokens":10}}"#,
        );
        assert!(matches!(
            chunks.as_slice(),
            [Ok(ChunkType::Usage(crate::chunk::LanguageModelUsage {
                input_tokens: 120,
                output_tokens: 30,
                cache_read_tokens: 80,
                cache_write_tokens: 10,
            }))]
        ));
    }

    #[test]
    fn usage_only_chunk_emits_normalized_usage() {
        let chunks = process_sse_data(
            r#"{"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":30,"prompt_tokens_details":{"cached_tokens":80}}}"#,
        );

        assert!(matches!(
            chunks.as_slice(),
            [Ok(ChunkType::Usage(crate::chunk::LanguageModelUsage {
                input_tokens: 120,
                output_tokens: 30,
                cache_read_tokens: 80,
                cache_write_tokens: 0,
            }))]
        ));
    }

    #[test]
    fn emits_tool_call_delta_without_finish_reason() {
        let data = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"id":"tool-1","index":0,"type":"function","function":{"name":"question","arguments":"{\"questions\":[{\"header\":\"Hobbies\",\"options\":[]}]}"}}]}}]}"#;

        let chunks = tool_call_chunks(data);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("\"name\":\"question\""));
    }

    #[test]
    fn emits_no_tool_call_for_empty_final_tool_call_chunk() {
        let data = r#"{"choices":[{"index":0,"finish_reason":"tool_calls","delta":{"role":"assistant","content":""}}]}"#;

        let chunks = tool_call_chunks(data);

        assert!(chunks.is_empty());
    }

    #[test]
    fn tool_call_message_preserves_reasoning_content() {
        let message = Message::tool_call_with_reasoning(
            "call_1",
            "read",
            r#"{"file_path":"src/lib.rs"}"#,
            "plan",
        );
        let Message::ToolCall(tool) = message else {
            panic!("expected tool call message");
        };

        let payload = openai_compatible_tool_call_message_from_calls(
            vec![openai_compatible_tool_call(&tool)],
            tool.reasoning_content.clone(),
            false,
        );

        assert_eq!(payload["reasoning_content"], "plan");
    }

    #[test]
    fn kimi_tool_call_message_includes_empty_reasoning_content_fallback() {
        let provider = OpenAICompatible::builder()
            .base_url("https://api.example.com/v1")
            .model_name("kimi-k2.6")
            .provider_name("Example")
            .build()
            .unwrap();
        let message = Message::tool_call("call_1", "read", r#"{"file_path":"src/lib.rs"}"#);
        let Message::ToolCall(tool) = message else {
            panic!("expected tool call message");
        };

        let payload = openai_compatible_tool_call_message_from_calls(
            vec![openai_compatible_tool_call(&tool)],
            tool.reasoning_content.clone(),
            openai_compatible_requires_tool_call_reasoning_content(&provider),
        );

        assert_eq!(payload["reasoning_content"], "");
    }

    #[test]
    fn groups_adjacent_tool_calls_before_tool_outputs() {
        let messages = vec![
            Message::system("system"),
            Message::user("user"),
            Message::tool_call("glob:0", "glob", r#"{"pattern":"**/*.jpg"}"#),
            Message::tool_call("glob:1", "glob", r#"{"pattern":"**/*.png"}"#),
            Message::tool_output("glob:0", "glob", "jpg result", false),
            Message::tool_output("glob:1", "glob", "png result", false),
        ];

        let payload = openai_compatible_messages(&messages, false);

        assert_eq!(payload.len(), 5);
        assert_eq!(payload[2]["role"], "assistant");
        assert_eq!(payload[2]["tool_calls"][0]["id"], "glob:0");
        assert_eq!(payload[2]["tool_calls"][1]["id"], "glob:1");
        assert_eq!(payload[3]["role"], "tool");
        assert_eq!(payload[3]["tool_call_id"], "glob:0");
        assert_eq!(payload[4]["role"], "tool");
        assert_eq!(payload[4]["tool_call_id"], "glob:1");
    }

    #[test]
    fn reasoning_sibling_becomes_tool_call_reasoning_content() {
        let messages = vec![
            Message::user("inspect"),
            Message::reasoning(Some("rs_1".to_string()), "plan", Some("enc".to_string())),
            Message::tool_call("call_1", "read", r#"{"file_path":"src/lib.rs"}"#),
            Message::tool_output("call_1", "read", "ok", false),
        ];

        let payload = openai_compatible_messages(&messages, false);

        assert_eq!(payload.len(), 3);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[1]["role"], "assistant");
        assert_eq!(payload[1]["reasoning_content"], "plan");
        assert_eq!(payload[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(payload[2]["role"], "tool");
    }

    #[test]
    fn done_marker_emits_terminal_chunk() {
        let chunks = process_sse_data("[DONE]");

        assert!(matches!(
            chunks.as_slice(),
            [Ok(ChunkType::End { reason: None })]
        ));
    }

    #[test]
    fn finish_reason_emits_terminal_chunk() {
        let data = r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{"role":"assistant","content":""}}]}"#;

        let chunks = process_sse_data(data);

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            Ok(ChunkType::End {
                reason: Some(FinishReason::Stop)
            })
        )));
    }

    #[test]
    fn length_finish_reason_emits_terminal_reason() {
        let data = r#"{"choices":[{"index":0,"finish_reason":"length","delta":{"role":"assistant","content":""}}]}"#;

        let chunks = process_sse_data(data);

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            Ok(ChunkType::End {
                reason: Some(FinishReason::Length)
            })
        )));
    }

    #[test]
    fn ignores_sse_comments_and_metadata() {
        for data in [
            ": OPENROUTER PROCESSING",
            "event: ping",
            "id: chatcmpl-123",
            "retry: 1000",
        ] {
            assert!(process_sse_data(data).is_empty());
        }
    }

    #[test]
    fn bytes_to_lines_skips_sse_comments_and_metadata() {
        let byte_stream = stream::iter(vec![
            Ok::<_, reqwest::Error>(bytes::Bytes::from_static(b": OPENROUTER PROCESSING\n")),
            Ok::<_, reqwest::Error>(bytes::Bytes::from_static(b"event: ping\n")),
            Ok::<_, reqwest::Error>(bytes::Bytes::from_static(
                br#"data: {"choices":[{"delta":{"content":"hello"}}]}
"#,
            )),
        ]);

        let lines = futures::executor::block_on(bytes_to_lines(byte_stream).collect::<Vec<_>>())
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .expect("byte stream should parse");

        assert_eq!(
            lines,
            vec![r#"{"choices":[{"delta":{"content":"hello"}}]}"#.to_string()]
        );
    }

    #[test]
    fn bytes_to_lines_preserves_done_marker() {
        let byte_stream = stream::iter(vec![Ok::<_, reqwest::Error>(bytes::Bytes::from_static(
            b"data: [DONE]\n",
        ))]);

        let lines = futures::executor::block_on(bytes_to_lines(byte_stream).collect::<Vec<_>>())
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .expect("byte stream should parse");

        assert_eq!(lines, vec!["[DONE]".to_string()]);
    }

    #[test]
    fn gateway_caching_auto_is_off_by_default() {
        let provider = OpenAICompatible::builder()
            .base_url("https://ai-gateway.vercel.sh/v1")
            .model_name("anthropic/claude-sonnet-4.5")
            .provider_name("vercel")
            .build()
            .unwrap();
        assert!(!provider.gateway_caching_auto);
    }

    #[test]
    fn gateway_caching_auto_can_be_enabled() {
        let provider = OpenAICompatible::builder()
            .base_url("https://ai-gateway.vercel.sh/v1")
            .model_name("anthropic/claude-sonnet-4.5")
            .provider_name("vercel")
            .gateway_caching_auto(true)
            .build()
            .unwrap();
        assert!(provider.gateway_caching_auto);
    }
}

/// Convert a byte stream into a stream of lines, handling both SSE (`data: ...`) and raw NDJSON.
fn bytes_to_lines<S>(byte_stream: S) -> impl futures::Stream<Item = Result<String>>
where
    S: futures::Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let buffer: Vec<u8> = Vec::new();
    stream::unfold(
        (byte_stream, buffer),
        |(mut stream, mut buffer)| async move {
            loop {
                if let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                    let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line_bytes);
                    let line = line.trim_end_matches('\n').trim_end_matches('\r');
                    if line.is_empty() || is_sse_metadata_line(line.trim()) {
                        continue;
                    }
                    let data = if let Some(stripped) = line.strip_prefix("data:") {
                        stripped.trim_start().to_string()
                    } else {
                        line.to_string()
                    };
                    if data.is_empty() {
                        continue;
                    }
                    debug_log(&format!("[LINE] Extracted: {}", data));
                    return Some((Ok(data), (stream, buffer)));
                }
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        debug_log(&format!("[BYTES] Received {} bytes", bytes.len()));
                        buffer.extend_from_slice(&bytes);
                    }
                    Some(Err(e)) => {
                        debug_log(&format!("[BYTES] Error: {}", e));
                        return Some((Err(Error::Http(e)), (stream, buffer)));
                    }
                    None => {
                        let remaining = String::from_utf8_lossy(&buffer).trim().to_string();
                        buffer.clear();
                        if remaining.is_empty() || is_sse_metadata_line(&remaining) {
                            debug_log("[LINE] Stream ended, no remaining data");
                            return None;
                        }
                        let data = if let Some(stripped) = remaining.strip_prefix("data:") {
                            stripped.trim_start().to_string()
                        } else {
                            remaining
                        };
                        debug_log(&format!("[LINE] Remaining at EOF: {}", data));
                        return Some((Ok(data), (stream, buffer)));
                    }
                }
            }
        },
    )
}

fn is_sse_metadata_line(line: &str) -> bool {
    line.starts_with(':')
        || line.starts_with("event:")
        || line.starts_with("id:")
        || line.starts_with("retry:")
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
