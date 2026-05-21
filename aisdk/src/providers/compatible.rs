use crate::chunk::ChunkType;
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
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

    pub fn build(self) -> Result<OpenAICompatible> {
        Ok(OpenAICompatible {
            base_url: self
                .base_url
                .ok_or(Error::MissingField("base_url".into()))?,
            api_key: self.api_key.ok_or(Error::MissingField("api_key".into()))?,
            model_name: self
                .model_name
                .ok_or(Error::MissingField("model_name".into()))?,
            provider_name: self
                .provider_name
                .unwrap_or_else(|| "openai-compatible".to_string()),
            reasoning_effort: self.reasoning_effort,
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
                    "content": openai_compatible_user_content(u),
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

        if let Some(effort) = &self.reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(effort.clone());
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
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Provider(format!("API error {}: {}", status, text)));
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
    let data = data.trim();

    if data == "[DONE]" {
        debug_log("[SSE] Terminal: [DONE]");
        return vec![Ok(ChunkType::End(String::new()))];
    }

    if data.is_empty() || is_sse_metadata_line(data) {
        debug_log("[SSE] Ignored: empty or metadata/comment");
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
        debug_log(&format!(
            "[SSE] No choices array. JSON keys: {:?}",
            value.as_object().map(|o| o.keys().collect::<Vec<_>>())
        ));
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
        "length" => chunks.push(Ok(ChunkType::Incomplete(
            "finish_reason=length".to_string(),
        ))),
        "content_filter" => chunks.push(Ok(ChunkType::Failed(
            "finish_reason=content_filter".to_string(),
        ))),
        _ => chunks.push(Ok(ChunkType::End(String::new()))),
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
    fn done_marker_emits_terminal_chunk() {
        let chunks = process_sse_data("[DONE]");

        assert!(matches!(chunks.as_slice(), [Ok(ChunkType::End(_))]));
    }

    #[test]
    fn finish_reason_emits_terminal_chunk() {
        let data = r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{"role":"assistant","content":""}}]}"#;

        let chunks = process_sse_data(data);

        assert!(chunks
            .iter()
            .any(|chunk| matches!(chunk, Ok(ChunkType::End(_)))));
    }

    #[test]
    fn length_finish_reason_emits_incomplete_chunk() {
        let data = r#"{"choices":[{"index":0,"finish_reason":"length","delta":{"role":"assistant","content":""}}]}"#;

        let chunks = process_sse_data(data);

        assert!(chunks
            .iter()
            .any(|chunk| matches!(chunk, Ok(ChunkType::Incomplete(_)))));
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
