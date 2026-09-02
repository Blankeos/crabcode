use crate::chunk::{ChunkType, FinishReason};
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::retry::RetryError;
use crate::tool::Tool;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use std::collections::HashMap;

const ANTHROPIC_STREAM_CONNECT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct Anthropic {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
    reasoning_effort: Option<String>,
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
    reasoning_effort: Option<String>,
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

    pub fn reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    pub fn build(self) -> Result<Anthropic> {
        Ok(Anthropic {
            base_url: self
                .base_url
                .ok_or(Error::MissingField("base_url".into()))?,
            api_key: self.api_key.unwrap_or_default(),
            model_name: self
                .model_name
                .ok_or(Error::MissingField("model_name".into()))?,
            provider_name: self
                .provider_name
                .unwrap_or_else(|| "anthropic".to_string()),
            reasoning_effort: self.reasoning_effort,
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

        // Anthropic requires adjacent tool_use blocks in one assistant message,
        // immediately followed by tool_result blocks in one user message.
        let user_messages = anthropic_messages(messages);

        let mut tool_params: Vec<serde_json::Value> = Vec::new();
        let mut has_hosted_search = false;
        for t in tools {
            match &t.transport {
                crate::aisdk::tool::ToolTransport::ProviderNative(value) => {
                    has_hosted_search = true;
                    tool_params.push(value.clone());
                }
                crate::aisdk::tool::ToolTransport::OpenRouterPlugin(_) => {}
                crate::aisdk::tool::ToolTransport::ClientFunction => {
                    let schema = serde_json::to_value(&t.input_schema).unwrap_or_default();
                    tool_params.push(serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": schema,
                    }));
                }
            }
        }

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

        if let Some(effort) = &self.reasoning_effort {
            body["output_config"] = serde_json::json!({ "effort": effort });
        }

        // Prompt caching: last tool + last system + latest user content block.
        // Stable prefix first so multi-step tool loops can cache-read
        // tools/system/history.
        apply_anthropic_prompt_caching(&mut body);

        let mut request_headers = reqwest::header::HeaderMap::new();
        request_headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        if !self.api_key.is_empty() {
            request_headers.insert("x-api-key", self.api_key.parse().unwrap());
        }
        request_headers.insert("anthropic-version", "2023-06-01".parse().unwrap());
        if has_hosted_search {
            // Hosted web_search tool requires the anthropic-beta header.
            request_headers.insert("anthropic-beta", "web-search-2025-03-05".parse().unwrap());
        }

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(
                ANTHROPIC_STREAM_CONNECT_TIMEOUT_SECS,
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
            let retry_error = RetryError::new(format!("Anthropic API error {}: {}", status, text))
                .with_status(status.as_u16())
                .with_headers(&headers);
            if crate::retry::retryable(&retry_error) {
                return Err(Error::RetryableProvider(retry_error));
            }
            return Err(Error::Provider(retry_error.message));
        }

        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(|ev| match ev {
                Ok(event) => {
                    let event_type = event.event.as_str();
                    let data = &event.data;

                    if data.is_empty() {
                        return futures::future::ready(None);
                    }

                    match serde_json::from_str::<serde_json::Value>(data) {
                        Ok(value) => {
                            futures::future::ready(anthropic_stream_chunk(event_type, &value))
                        }
                        Err(e) => futures::future::ready(Some(Ok(ChunkType::Failed(format!(
                            "Invalid SSE data: {}",
                            e
                        ))))),
                    }
                }
                Err(e) => futures::future::ready(Some(Ok(ChunkType::RetryableFailure(
                    RetryError::from_message(format!("SSE error: {}", e)),
                )))),
            })
            .boxed();

        Ok(stream)
    }
}

fn anthropic_stream_chunk(
    event_type: &str,
    value: &serde_json::Value,
) -> Option<Result<ChunkType>> {
    match event_type {
        "message_start" => {
            // Partial usage early in the stream (cache fields may already appear).
            if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                log_anthropic_usage(usage);
            }
            None
        }
        "content_block_start" => {
            if let Some(payload) = anthropic_hosted_search_start(value) {
                Some(Ok(ChunkType::ProviderToolCall(payload)))
            } else if let Some(payload) = anthropic_hosted_search_result(value) {
                Some(Ok(ChunkType::ProviderToolCall(payload)))
            } else {
                anthropic_tool_call_start(value)
                    .map(ChunkType::ToolCall)
                    .map(Ok)
            }
        }
        "content_block_delta" => anthropic_content_block_delta(value).map(Ok),
        "message_delta" => {
            // Final usage wins for cache_read / cache_creation.
            if let Some(usage) = value.get("usage") {
                log_anthropic_usage(usage);
            }
            anthropic_message_delta(value).map(Ok)
        }
        "message_stop" => Some(Ok(ChunkType::End { reason: None })),
        "error" => {
            let error_msg = value["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            Some(Ok(ChunkType::Failed(error_msg.to_string())))
        }
        _ => None,
    }
}

/// Log Anthropic usage via the host logger so cache hits are verifiable.
/// Note: `input_tokens` is non-cached only; total input ≈ input + cache_read + cache_creation.
fn log_anthropic_usage(usage: &serde_json::Value) {
    let input = usage.get("input_tokens").and_then(|v| v.as_u64());
    let output = usage.get("output_tokens").and_then(|v| v.as_u64());
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Skip empty/partial early frames with no signal.
    if input.is_none() && output.is_none() && cache_read == 0 && cache_creation == 0 {
        return;
    }

    let input_v = input.unwrap_or(0);
    let total_input = input_v
        .saturating_add(cache_read)
        .saturating_add(cache_creation);
    let hit_pct = if total_input > 0 {
        (cache_read as f64 * 100.0) / total_input as f64
    } else {
        0.0
    };

    crate::log::log(&format!(
        "[prompt-cache] anthropic input={} output={} cache_read={} cache_creation={} total_input={} hit_pct={:.1}",
        input.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        output.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        cache_read,
        cache_creation,
        total_input,
        hit_pct
    ));
}

fn anthropic_content_block_delta(value: &serde_json::Value) -> Option<ChunkType> {
    let delta = value.get("delta")?;

    match delta.get("type").and_then(|delta_type| delta_type.as_str()) {
        Some("text_delta") => delta
            .get("text")
            .and_then(|text| text.as_str())
            .filter(|text| !text.is_empty())
            .map(|text| ChunkType::Text(text.to_string())),
        Some("thinking_delta") => delta
            .get("thinking")
            .and_then(|thinking| thinking.as_str())
            .filter(|thinking| !thinking.is_empty())
            .map(|thinking| ChunkType::Reasoning(thinking.to_string())),
        Some("input_json_delta") => {
            anthropic_tool_call_arguments_delta(value).map(ChunkType::ToolCall)
        }
        _ => None,
    }
}

fn anthropic_message_delta(value: &serde_json::Value) -> Option<ChunkType> {
    let stop_reason = value
        .get("delta")
        .and_then(|delta| delta.get("stop_reason"))
        .and_then(|stop_reason| stop_reason.as_str())?;

    Some(ChunkType::End {
        reason: Some(FinishReason::from_anthropic(stop_reason)),
    })
}

fn anthropic_hosted_search_start(value: &serde_json::Value) -> Option<String> {
    let content_block = value.get("content_block")?;
    if content_block.get("type").and_then(|v| v.as_str()) != Some("server_tool_use") {
        return None;
    }
    let id = content_block
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("hosted_search");
    let name = content_block
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("web_search");
    let args = content_block
        .get("input")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Some(
        serde_json::json!({
            "id": id,
            "name": name,
            "status": "running",
            "provider_executed": true,
            "arguments": args,
        })
        .to_string(),
    )
}

fn anthropic_hosted_search_result(value: &serde_json::Value) -> Option<String> {
    let content_block = value.get("content_block")?;
    let block_type = content_block.get("type").and_then(|v| v.as_str())?;
    let failed = block_type == "web_search_tool_result_error";
    if block_type != "web_search_tool_result" && !failed {
        return None;
    }
    let id = content_block
        .get("tool_use_id")
        .or_else(|| content_block.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("hosted_search");
    let output = content_block
        .get("content")
        .cloned()
        .unwrap_or_else(|| content_block.clone());
    Some(
        serde_json::json!({
            "id": id,
            "name": "web_search",
            "status": if failed { "failed" } else { "completed" },
            "provider_executed": true,
            "output": output,
        })
        .to_string(),
    )
}

fn anthropic_tool_call_start(value: &serde_json::Value) -> Option<String> {
    let content_block = value.get("content_block")?;
    let block_type = content_block
        .get("type")
        .and_then(|block_type| block_type.as_str())?;

    // Hosted web_search runs server-side; ignore those content blocks for the
    // client tool loop (results come back as text / citations).
    if matches!(
        block_type,
        "server_tool_use" | "web_search_tool_result" | "web_search_tool_result_error"
    ) {
        return None;
    }

    if block_type != "tool_use" {
        return None;
    }

    let mut function = serde_json::Map::new();
    if let Some(name) = content_block
        .get("name")
        .and_then(|name| name.as_str())
        .filter(|name| !name.is_empty())
    {
        function.insert(
            "name".to_string(),
            serde_json::Value::String(name.to_string()),
        );
    }

    if let Some(input) = content_block
        .get("input")
        .filter(|input| !anthropic_tool_input_is_empty(input))
    {
        function.insert(
            "arguments_done".to_string(),
            serde_json::Value::String(input.to_string()),
        );
    }

    let mut item = anthropic_tool_call_item_base(value, function);
    if let Some(id) = content_block
        .get("id")
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
    {
        item.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    }
    item.insert(
        "type".to_string(),
        serde_json::Value::String("function".to_string()),
    );

    serde_json::to_string(&vec![serde_json::Value::Object(item)]).ok()
}

fn anthropic_tool_call_arguments_delta(value: &serde_json::Value) -> Option<String> {
    let partial_json = value
        .get("delta")
        .and_then(|delta| delta.get("partial_json"))
        .and_then(|partial_json| partial_json.as_str())
        .filter(|partial_json| !partial_json.is_empty())?;

    let mut function = serde_json::Map::new();
    function.insert(
        "arguments".to_string(),
        serde_json::Value::String(partial_json.to_string()),
    );

    serde_json::to_string(&vec![serde_json::Value::Object(
        anthropic_tool_call_item_base(value, function),
    )])
    .ok()
}

fn anthropic_tool_call_item_base(
    value: &serde_json::Value,
    function: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut item = serde_json::Map::new();

    if let Some(index) = value.get("index").and_then(|index| index.as_u64()) {
        item.insert(
            "index".to_string(),
            serde_json::Value::Number(serde_json::Number::from(index)),
        );
    }

    item.insert("function".to_string(), serde_json::Value::Object(function));
    item
}

fn anthropic_tool_input_is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Object(map) => map.is_empty(),
        serde_json::Value::String(text) => text.trim().is_empty(),
        _ => false,
    }
}

/// Anthropic prompt caching breakpoints:
/// 1. last tool (stable schemas — high value in tool loops)
/// 2. last system block
/// 3. tip of transcript (last markable block; skips thinking)
/// 4. previous user tip (covers turns past the 20-block lookback)
/// Cap at 4 breakpoints; each is `{"type":"ephemeral"}`. The 4th slot stays
/// free when tools or previous-user are missing so gateways can still auto-mark.
fn apply_anthropic_prompt_caching(body: &mut serde_json::Value) {
    let mut remaining = 4usize;

    // 1. Last tool (stable schemas — highest value for tool loops)
    if remaining > 0 {
        if let Some(tools) = body.get_mut("tools").and_then(|v| v.as_array_mut()) {
            if let Some(last) = tools.last_mut().and_then(|t| t.as_object_mut()) {
                last.insert(
                    "cache_control".to_string(),
                    serde_json::json!({ "type": "ephemeral" }),
                );
                remaining -= 1;
            }
        }
    }

    // 2. Last system text block
    if remaining > 0 {
        if let Some(system) = body.get_mut("system").and_then(|v| v.as_array_mut()) {
            if let Some(last) = system.last_mut().and_then(|b| b.as_object_mut()) {
                last.insert(
                    "cache_control".to_string(),
                    serde_json::json!({ "type": "ephemeral" }),
                );
                remaining -= 1;
            }
        }
    }

    // 3–4. Transcript tip + previous user tip (skip thinking blocks).
    if remaining == 0 {
        return;
    }
    let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };

    let tip = (0..messages.len())
        .rev()
        .find(|&i| mark_message_cache_breakpoint(&mut messages[i]));
    if tip.is_some() {
        remaining = remaining.saturating_sub(1);
    }

    // Where the previous request ended: skip the whole trailing user run after
    // the last assistant, then mark that earlier user tip.
    if remaining > 0 {
        if let Some(tip) = tip {
            if let Some(prev) = messages[..tip]
                .iter()
                .rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
                .and_then(|assistant| {
                    messages[..assistant]
                        .iter()
                        .rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                })
            {
                let _ = mark_message_cache_breakpoint(&mut messages[prev]);
            }
        }
    }
}

/// Marks the last content block that can carry a breakpoint, scanning back past
/// `thinking` / `redacted_thinking` which the API rejects.
fn mark_message_cache_breakpoint(message: &mut serde_json::Value) -> bool {
    let Some(obj) = message.as_object_mut() else {
        return false;
    };

    // Plain string content must become a text block to host cache_control.
    if let Some(serde_json::Value::String(text)) = obj.get("content").cloned() {
        obj.insert(
            "content".to_string(),
            serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": { "type": "ephemeral" }
            }]),
        );
        return true;
    }

    let Some(blocks) = obj.get_mut("content").and_then(|c| c.as_array_mut()) else {
        return false;
    };

    for block in blocks.iter_mut().rev() {
        let Some(block_obj) = block.as_object_mut() else {
            continue;
        };
        let block_type = block_obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if block_type == "thinking" || block_type == "redacted_thinking" {
            continue;
        }
        block_obj.insert(
            "cache_control".to_string(),
            serde_json::json!({ "type": "ephemeral" }),
        );
        return true;
    }
    false
}

fn anthropic_user_content(user: &crate::message::UserMessage) -> serde_json::Value {
    if user.images.is_empty() {
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
        let data = image
            .data_url
            .split_once(',')
            .map(|(_, data)| data)
            .unwrap_or(image.data_url.as_str());
        serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image.media_type,
                "data": data,
            },
        })
    }));

    serde_json::Value::Array(parts)
}

/// Convert internal messages into Anthropic Messages API history.
///
/// Adjacent `ToolCall`s are merged into one assistant message with multiple
/// `tool_use` blocks; adjacent `ToolOutput`s become one user message with
/// multiple `tool_result` blocks. Anthropic (and Kimi coding) reject
/// unpaired / non-adjacent multi-tool turns.
fn anthropic_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut pending_tool_uses: Vec<serde_json::Value> = Vec::new();
    let mut pending_tool_results: Vec<serde_json::Value> = Vec::new();

    let flush_tool_uses = |pending: &mut Vec<serde_json::Value>,
                           out: &mut Vec<serde_json::Value>| {
        if pending.is_empty() {
            return;
        }
        out.push(serde_json::json!({
            "role": "assistant",
            "content": std::mem::take(pending),
        }));
    };

    let flush_tool_results = |pending: &mut Vec<serde_json::Value>,
                              out: &mut Vec<serde_json::Value>| {
        if pending.is_empty() {
            return;
        }
        out.push(serde_json::json!({
            "role": "user",
            "content": std::mem::take(pending),
        }));
    };

    for message in messages {
        match message {
            Message::User(u) => {
                flush_tool_uses(&mut pending_tool_uses, &mut out);
                flush_tool_results(&mut pending_tool_results, &mut out);
                out.push(serde_json::json!({
                    "role": "user",
                    "content": anthropic_user_content(u),
                }));
            }
            Message::Assistant(a) => {
                flush_tool_uses(&mut pending_tool_uses, &mut out);
                flush_tool_results(&mut pending_tool_results, &mut out);
                out.push(serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                }));
            }
            Message::ToolCall(t) => {
                flush_tool_results(&mut pending_tool_results, &mut out);
                let input = serde_json::from_str::<serde_json::Value>(&t.arguments)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
                pending_tool_uses.push(serde_json::json!({
                    "type": "tool_use",
                    "id": t.call_id,
                    "name": t.name,
                    "input": input,
                }));
            }
            Message::ToolOutput(t) => {
                flush_tool_uses(&mut pending_tool_uses, &mut out);
                pending_tool_results.push(serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": t.call_id,
                    "content": anthropic_tool_output_content(t),
                    "is_error": t.is_error,
                }));
            }
            Message::Reasoning(_) => {}
            Message::System(_) => {}
        }
    }

    flush_tool_uses(&mut pending_tool_uses, &mut out);
    flush_tool_results(&mut pending_tool_results, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call_json(event_type: &str, value: serde_json::Value) -> serde_json::Value {
        let chunk = anthropic_stream_chunk(event_type, &value)
            .expect("event should produce a chunk")
            .expect("chunk should parse");

        let ChunkType::ToolCall(json) = chunk else {
            panic!("expected tool call chunk");
        };

        serde_json::from_str::<serde_json::Value>(&json).expect("tool call should be json")
    }

    #[test]
    fn emits_tool_call_start_as_openai_style_delta() {
        let json = tool_call_json(
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "read",
                    "input": {},
                },
            }),
        );

        assert_eq!(json[0]["index"], 1);
        assert_eq!(json[0]["id"], "toolu_1");
        assert_eq!(json[0]["type"], "function");
        assert_eq!(json[0]["function"]["name"], "read");
        assert!(json[0]["function"].get("arguments").is_none());
    }

    #[test]
    fn emits_tool_input_delta_as_openai_style_delta() {
        let json = tool_call_json(
            "content_block_delta",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"file_path\"",
                },
            }),
        );

        assert_eq!(json[0]["index"], 0);
        assert_eq!(json[0]["function"]["arguments"], "{\"file_path\"");
    }

    #[test]
    fn ignores_empty_tool_input_delta() {
        let value = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": "",
            },
        });

        assert!(anthropic_stream_chunk("content_block_delta", &value).is_none());
    }

    #[test]
    fn message_stop_emits_terminal_chunk() {
        let chunk = anthropic_stream_chunk("message_stop", &serde_json::json!({}))
            .expect("event should produce a chunk")
            .expect("chunk should parse");

        assert!(matches!(chunk, ChunkType::End { reason: None }));
    }

    #[test]
    fn max_tokens_stop_reason_emits_terminal_reason() {
        let value = serde_json::json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "max_tokens",
            },
        });
        let chunk = anthropic_stream_chunk("message_delta", &value)
            .expect("event should produce a chunk")
            .expect("chunk should parse");

        assert!(matches!(
            chunk,
            ChunkType::End {
                reason: Some(FinishReason::Length)
            }
        ));
    }

    #[test]
    fn refusal_stop_reason_emits_terminal_reason() {
        let value = serde_json::json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "refusal",
            },
        });
        let chunk = anthropic_stream_chunk("message_delta", &value)
            .expect("event should produce a chunk")
            .expect("chunk should parse");

        assert!(matches!(
            chunk,
            ChunkType::End {
                reason: Some(FinishReason::Refusal)
            }
        ));
    }

    #[test]
    fn end_turn_stop_reason_emits_terminal_reason() {
        let value = serde_json::json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "end_turn",
            },
        });
        let chunk = anthropic_stream_chunk("message_delta", &value)
            .expect("event should produce a chunk")
            .expect("chunk should parse");

        assert!(matches!(
            chunk,
            ChunkType::End {
                reason: Some(FinishReason::EndTurn)
            }
        ));
    }

    #[test]
    fn groups_adjacent_tool_calls_and_results() {
        let messages = vec![
            Message::user("hi"),
            Message::tool_call("call_a", "edit", r#"{"file_path":"a.rs"}"#),
            Message::tool_call("call_b", "edit", r#"{"file_path":"b.rs"}"#),
            Message::tool_output("call_a", "edit", "ok a", false),
            Message::tool_output("call_b", "edit", "ok b", false),
            Message::assistant("done"),
        ];

        let encoded = anthropic_messages(&messages);
        assert_eq!(encoded.len(), 4);
        assert_eq!(encoded[0]["role"], "user");
        assert_eq!(encoded[1]["role"], "assistant");
        assert_eq!(encoded[1]["content"].as_array().unwrap().len(), 2);
        assert_eq!(encoded[1]["content"][0]["type"], "tool_use");
        assert_eq!(encoded[1]["content"][0]["id"], "call_a");
        assert_eq!(encoded[1]["content"][1]["id"], "call_b");
        assert_eq!(encoded[2]["role"], "user");
        assert_eq!(encoded[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(encoded[2]["content"][0]["type"], "tool_result");
        assert_eq!(encoded[2]["content"][0]["tool_use_id"], "call_a");
        assert_eq!(encoded[2]["content"][1]["tool_use_id"], "call_b");
        assert_eq!(encoded[3]["role"], "assistant");
        assert_eq!(encoded[3]["content"], "done");
    }

    #[test]
    fn prompt_caching_marks_last_tool_system_tip_and_previous_user() {
        let mut body = serde_json::json!({
            "system": [
                { "type": "text", "text": "sys a" },
                { "type": "text", "text": "sys b" },
            ],
            "tools": [
                { "name": "read", "description": "r", "input_schema": { "type": "object" } },
                { "name": "edit", "description": "e", "input_schema": { "type": "object" } },
            ],
            "messages": [
                { "role": "user", "content": "first" },
                { "role": "assistant", "content": "ok" },
                { "role": "user", "content": "second" },
            ],
        });

        apply_anthropic_prompt_caching(&mut body);

        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(
            body["tools"][1]["cache_control"],
            serde_json::json!({ "type": "ephemeral" })
        );
        assert!(body["system"][0].get("cache_control").is_none());
        assert_eq!(
            body["system"][1]["cache_control"],
            serde_json::json!({ "type": "ephemeral" })
        );
        // tip (latest user) wrapped with cache_control
        assert_eq!(
            body["messages"][2]["content"][0]["cache_control"],
            serde_json::json!({ "type": "ephemeral" })
        );
        // previous user tip also marked
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            serde_json::json!({ "type": "ephemeral" })
        );
    }

    #[test]
    fn prompt_caching_marks_last_tool_result_block() {
        let mut body = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "a", "content": "ok a" },
                    { "type": "tool_result", "tool_use_id": "b", "content": "ok b" },
                ]
            }],
        });

        apply_anthropic_prompt_caching(&mut body);

        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert_eq!(
            body["messages"][0]["content"][1]["cache_control"],
            serde_json::json!({ "type": "ephemeral" })
        );
    }

    #[test]
    fn prompt_caching_skips_thinking_blocks_when_marking_tip() {
        let mut body = serde_json::json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "answer" },
                    { "type": "thinking", "thinking": "secret" },
                ]
            }],
        });

        apply_anthropic_prompt_caching(&mut body);

        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"],
            serde_json::json!({ "type": "ephemeral" })
        );
        assert!(body["messages"][0]["content"][1]
            .get("cache_control")
            .is_none());
    }
}

fn anthropic_tool_output_content(tool: &crate::message::ToolOutputMessage) -> serde_json::Value {
    if tool.images.is_empty() {
        return serde_json::json!(tool.output);
    }

    let mut parts = Vec::new();
    if !tool.output.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": tool.output,
        }));
    }

    parts.extend(tool.images.iter().map(|image| {
        let data = image
            .data_url
            .split_once(',')
            .map(|(_, data)| data)
            .unwrap_or(image.data_url.as_str());
        serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image.media_type,
                "data": data,
            },
        })
    }));

    serde_json::Value::Array(parts)
}
