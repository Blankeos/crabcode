use crate::chunk::{ChunkType, FinishReason};
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
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

        let user_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(serde_json::json!({
                    "role": "user",
                    "content": anthropic_user_content(u),
                })),
                Message::Assistant(a) => Some(serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                })),
                Message::ToolCall(t) => Some(serde_json::json!({
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": t.call_id,
                        "name": t.name,
                        "input": serde_json::from_str::<serde_json::Value>(&t.arguments)
                            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
                    }],
                })),
                Message::ToolOutput(t) => Some(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": t.call_id,
                        "content": anthropic_tool_output_content(t),
                        "is_error": t.is_error,
                    }],
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

        if let Some(effort) = &self.reasoning_effort {
            body["output_config"] = serde_json::json!({ "effort": effort });
        }

        let mut request_headers = reqwest::header::HeaderMap::new();
        request_headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        if !self.api_key.is_empty() {
            request_headers.insert("x-api-key", self.api_key.parse().unwrap());
        }
        request_headers.insert("anthropic-version", "2023-06-01".parse().unwrap());

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
                Err(e) => {
                    let err = format!("SSE error: {}", e);
                    futures::future::ready(Some(Ok(ChunkType::Failed(err))))
                }
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
        "content_block_start" => anthropic_tool_call_start(value)
            .map(ChunkType::ToolCall)
            .map(Ok),
        "content_block_delta" => anthropic_content_block_delta(value).map(Ok),
        "message_delta" => anthropic_message_delta(value).map(Ok),
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

    match stop_reason {
        "max_tokens" => Some(ChunkType::Incomplete("stop_reason=max_tokens".to_string())),
        "refusal" => Some(ChunkType::Failed("stop_reason=refusal".to_string())),
        reason => Some(ChunkType::End {
            reason: Some(FinishReason::from_anthropic(reason)),
        }),
    }
}

fn anthropic_tool_call_start(value: &serde_json::Value) -> Option<String> {
    let content_block = value.get("content_block")?;
    if content_block
        .get("type")
        .and_then(|block_type| block_type.as_str())
        != Some("tool_use")
    {
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
    fn max_tokens_stop_reason_emits_incomplete_chunk() {
        let value = serde_json::json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "max_tokens",
            },
        });
        let chunk = anthropic_stream_chunk("message_delta", &value)
            .expect("event should produce a chunk")
            .expect("chunk should parse");

        assert!(matches!(chunk, ChunkType::Incomplete(_)));
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
