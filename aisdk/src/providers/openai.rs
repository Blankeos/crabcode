use crate::chunk::{ChunkType, MessagePhase};
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::tool::Tool;
use async_trait::async_trait;
use eventsource_stream::{EventStreamError, Eventsource};
use futures::StreamExt;
use std::collections::HashMap;
use std::error::Error as StdError;

const OPENAI_STREAM_CONNECT_TIMEOUT_SECS: u64 = 30;
const OPENAI_ERROR_BODY_MAX_CHARS: usize = 2048;

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
    reasoning_effort: Option<String>,
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
    reasoning_effort: Option<String>,
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

    pub fn reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
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
            reasoning_effort: self.reasoning_effort,
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
        request_headers.insert(
            reqwest::header::ACCEPT,
            "text/event-stream".parse().unwrap(),
        );
        request_headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            "identity".parse().unwrap(),
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

        if let Some(effort) = &self.reasoning_effort {
            body["reasoning"] = serde_json::json!({ "effort": effort });
        }

        let request_diagnostics =
            openai_request_diagnostics(self, &input, tools, &body, &request_headers);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(
                OPENAI_STREAM_CONNECT_TIMEOUT_SECS,
            ))
            .build()
            .map_err(|e| Error::Provider(format!("Failed to build client: {}", e)))?;
        let response = client
            .post(&url)
            .headers(request_headers)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                Error::Provider(format_openai_request_error(
                    "send",
                    &url,
                    &err,
                    Some(&request_diagnostics),
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let response_url = sanitized_url(response.url());
            let text = match response.text().await {
                Ok(text) => truncate_log_value(&text, OPENAI_ERROR_BODY_MAX_CHARS),
                Err(err) => format!(
                    "<failed to read error body: {}>",
                    format_reqwest_error("read_error_body", &err)
                ),
            };
            return Err(Error::Provider(format!(
                "OpenAI API error: status={} url={} body={}",
                status, response_url, text
            )));
        }

        let request_url = url.clone();
        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(move |ev| match ev {
                Ok(event) => futures::future::ready(response_sse_data_to_chunk(&event.data)),
                Err(e) => {
                    let err = format_openai_sse_error(&e, &request_url);
                    futures::future::ready(Some(Ok(ChunkType::Failed(err))))
                }
            })
            .boxed();

        Ok(stream)
    }
}

fn format_openai_sse_error(err: &EventStreamError<reqwest::Error>, request_url: &str) -> String {
    match err {
        EventStreamError::Transport(source) => {
            format!(
                "SSE transport error: stream_connect_timeout_secs={} stream_body_timeout=disabled request_url={} {}",
                OPENAI_STREAM_CONNECT_TIMEOUT_SECS,
                sanitized_url_str(request_url),
                format_reqwest_error("stream_body", source),
            )
        }
        EventStreamError::Parser(source) => {
            format!("SSE parser error: source={} debug={:?}", source, source)
        }
        EventStreamError::Utf8(source) => {
            format!("SSE UTF-8 error: source={} debug={:?}", source, source)
        }
    }
}

fn format_openai_request_error(
    stage: &str,
    request_url: &str,
    err: &reqwest::Error,
    request_diagnostics: Option<&str>,
) -> String {
    let request_diagnostics = request_diagnostics
        .map(|diagnostics| format!(" request_diagnostics={}", diagnostics))
        .unwrap_or_default();

    format!(
        "OpenAI request error: stream_connect_timeout_secs={} stream_body_timeout=disabled request_url={} {}{}",
        OPENAI_STREAM_CONNECT_TIMEOUT_SECS,
        sanitized_url_str(request_url),
        format_reqwest_error(stage, err),
        request_diagnostics,
    )
}

#[derive(Debug, Default)]
struct OpenAIInputLogSummary {
    system_items: usize,
    user_items: usize,
    assistant_items: usize,
    unknown_items: usize,
    text_bytes: usize,
    image_count: usize,
    max_item_role: &'static str,
    max_item_bytes: usize,
    last_item_role: &'static str,
    last_item_bytes: usize,
    last_item_images: usize,
}

fn openai_request_diagnostics(
    provider: &OpenAI,
    input: &[serde_json::Value],
    tools: &[Tool],
    body: &serde_json::Value,
    headers: &reqwest::header::HeaderMap,
) -> String {
    let input_summary = summarize_openai_input(input);
    let input_json_bytes = json_bytes(input);
    let tool_json_bytes = body.get("tools").map(json_bytes).unwrap_or(0);
    let body_json_bytes = json_bytes(body);
    let instructions_bytes = provider
        .default_instructions
        .as_ref()
        .map(|instructions| instructions.len())
        .unwrap_or(0);
    let store = provider
        .store_override
        .map(|store| store.to_string())
        .unwrap_or_else(|| "default".to_string());
    let reasoning_effort = provider.reasoning_effort.as_deref().unwrap_or("none");

    format!(
        "model={} responses_path={} stream=true store={} reasoning_effort={} instructions_bytes={} input_items={} input_roles[system={},user={},assistant={},unknown={}] input_text_bytes={} input_images={} input_json_bytes={} max_input[role={},bytes={}] last_input[role={},bytes={},images={}] tools={} tool_names=[{}] tool_json_bytes={} body_json_bytes={} header_names=[{}]",
        provider.model_name,
        provider.responses_path,
        store,
        reasoning_effort,
        instructions_bytes,
        input.len(),
        input_summary.system_items,
        input_summary.user_items,
        input_summary.assistant_items,
        input_summary.unknown_items,
        input_summary.text_bytes,
        input_summary.image_count,
        input_json_bytes,
        input_summary.max_item_role,
        input_summary.max_item_bytes,
        input_summary.last_item_role,
        input_summary.last_item_bytes,
        input_summary.last_item_images,
        tools.len(),
        compact_tool_names(tools),
        tool_json_bytes,
        body_json_bytes,
        header_names(headers),
    )
}

fn summarize_openai_input(input: &[serde_json::Value]) -> OpenAIInputLogSummary {
    let mut summary = OpenAIInputLogSummary {
        max_item_role: "none",
        last_item_role: "none",
        ..OpenAIInputLogSummary::default()
    };

    for item in input {
        let role = input_role(item);
        let (text_bytes, image_count) = input_content_size(item.get("content"));

        match role {
            "system" => summary.system_items += 1,
            "user" => summary.user_items += 1,
            "assistant" => summary.assistant_items += 1,
            _ => summary.unknown_items += 1,
        }

        summary.text_bytes += text_bytes;
        summary.image_count += image_count;
        summary.last_item_role = role;
        summary.last_item_bytes = text_bytes;
        summary.last_item_images = image_count;

        if text_bytes > summary.max_item_bytes {
            summary.max_item_role = role;
            summary.max_item_bytes = text_bytes;
        }
    }

    summary
}

fn input_role(item: &serde_json::Value) -> &'static str {
    match item.get("role").and_then(|role| role.as_str()) {
        Some("system") => "system",
        Some("user") => "user",
        Some("assistant") => "assistant",
        _ => "unknown",
    }
}

fn input_content_size(content: Option<&serde_json::Value>) -> (usize, usize) {
    match content {
        Some(serde_json::Value::String(text)) => (text.len(), 0),
        Some(serde_json::Value::Array(parts)) => parts.iter().fold((0, 0), |mut acc, part| {
            match part.get("type").and_then(|value| value.as_str()) {
                Some("input_text") => {
                    acc.0 += part
                        .get("text")
                        .and_then(|value| value.as_str())
                        .map(|text| text.len())
                        .unwrap_or(0);
                }
                Some("input_image") => acc.1 += 1,
                _ => acc.0 += json_bytes(part),
            }
            acc
        }),
        Some(value) => (json_bytes(value), 0),
        None => (0, 0),
    }
}

fn json_bytes<T: serde::Serialize + ?Sized>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

fn compact_tool_names(tools: &[Tool]) -> String {
    const MAX_TOOL_NAMES: usize = 16;

    let mut names = tools
        .iter()
        .take(MAX_TOOL_NAMES)
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>()
        .join(",");

    if tools.len() > MAX_TOOL_NAMES {
        if !names.is_empty() {
            names.push(',');
        }
        names.push_str(&format!("+{}", tools.len() - MAX_TOOL_NAMES));
    }

    names
}

fn header_names(headers: &reqwest::header::HeaderMap) -> String {
    let mut names = headers
        .keys()
        .map(|name| name.as_str().to_ascii_lowercase())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names.join(",")
}

fn format_reqwest_error(stage: &str, err: &reqwest::Error) -> String {
    format!(
        "stage={} is_timeout={} is_connect={} is_request={} is_body={} is_decode={} status={} url={} source_chain={} debug={:?}",
        stage,
        err.is_timeout(),
        err.is_connect(),
        err.is_request(),
        err.is_body(),
        err.is_decode(),
        err.status()
            .map(|status| status.as_u16().to_string())
            .unwrap_or_else(|| "none".to_string()),
        sanitized_reqwest_error_url(err),
        error_source_chain(err),
        err,
    )
}

fn sanitized_reqwest_error_url(err: &reqwest::Error) -> String {
    err.url()
        .map(sanitized_url)
        .unwrap_or_else(|| "none".to_string())
}

fn sanitized_url_str(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|url| sanitized_url(&url))
        .unwrap_or_else(|_| "<invalid-url>".to_string())
}

fn sanitized_url(url: &reqwest::Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn truncate_log_value(value: &str, max_chars: usize) -> String {
    let single_line = value
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");

    if single_line.chars().count() <= max_chars {
        single_line
    } else {
        let truncated = single_line.chars().take(max_chars).collect::<String>();
        format!("{}...<truncated>", truncated)
    }
}

fn error_source_chain(err: &(dyn StdError + 'static)) -> String {
    let mut parts = Vec::new();
    let mut source = err.source();
    while let Some(err) = source {
        parts.push(err.to_string());
        source = err.source();
    }

    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(" <- ")
    }
}

fn response_sse_data_to_chunk(data: &str) -> Option<Result<ChunkType>> {
    if data == "[DONE]" {
        return Some(Ok(ChunkType::End(String::new())));
    }
    if data.is_empty() {
        return None;
    }

    let value = match serde_json::from_str::<serde_json::Value>(data) {
        Ok(value) => value,
        Err(err) => {
            return Some(Ok(ChunkType::Failed(format!("Invalid SSE data: {}", err))));
        }
    };

    let event_type = value["type"].as_str().unwrap_or("");
    match event_type {
        "response.output_text.delta" => {
            let delta = value["delta"].as_str().unwrap_or("");
            Some(Ok(ChunkType::Text(delta.to_string())))
        }
        "response.reasoning_summary_text.delta" => {
            let delta = value["delta"].as_str().unwrap_or("");
            Some(Ok(ChunkType::Reasoning(delta.to_string())))
        }
        "response.completed" => {
            let resp = &value["response"];
            if let Some(error) = resp.get("error") {
                if let Some(code) = error.get("code") {
                    return Some(Ok(ChunkType::Failed(code.to_string())));
                }
            }
            Some(Ok(ChunkType::ResponseCompleted {
                end_turn: resp.get("end_turn").and_then(|value| value.as_bool()),
            }))
        }
        "response.incomplete" => Some(Ok(ChunkType::Incomplete("Response incomplete".to_string()))),
        "response.failed" => Some(Ok(ChunkType::Failed("Response failed".to_string()))),
        _ => {
            if let Some(message_phase) = responses_assistant_message_phase_chunk(&value) {
                Some(Ok(message_phase))
            } else if let Some(tool_call) = responses_function_call_chunk(&value) {
                Some(Ok(ChunkType::ToolCall(tool_call)))
            } else if event_type.contains("tool_call") {
                Some(Ok(ChunkType::ToolCall(data.to_string())))
            } else {
                None
            }
        }
    }
}

fn responses_assistant_message_phase_chunk(value: &serde_json::Value) -> Option<ChunkType> {
    let event_type = value.get("type").and_then(|v| v.as_str())?;
    if !matches!(
        event_type,
        "response.output_item.added" | "response.output_item.done"
    ) {
        return None;
    }

    let item = value.get("item")?;
    if item.get("type").and_then(|v| v.as_str())? != "message"
        || item.get("role").and_then(|v| v.as_str()) != Some("assistant")
    {
        return None;
    }

    Some(ChunkType::AssistantMessagePhase {
        phase: item
            .get("phase")
            .and_then(|phase| phase.as_str())
            .and_then(parse_message_phase),
    })
}

fn parse_message_phase(phase: &str) -> Option<MessagePhase> {
    match phase {
        "commentary" => Some(MessagePhase::Commentary),
        "final_answer" => Some(MessagePhase::FinalAnswer),
        _ => None,
    }
}

fn responses_function_call_chunk(value: &serde_json::Value) -> Option<String> {
    let event_type = value.get("type").and_then(|v| v.as_str())?;

    let chunk = match event_type {
        "response.output_item.added" => {
            let item = value.get("item")?;
            if item.get("type").and_then(|v| v.as_str())? != "function_call" {
                return None;
            }

            response_function_call_item_chunk(value, item, false)?
        }
        "response.output_item.done" => {
            let item = value.get("item")?;
            if item.get("type").and_then(|v| v.as_str())? != "function_call" {
                return None;
            }

            response_function_call_item_chunk(value, item, true)?
        }
        "response.function_call_arguments.delta" => {
            let mut function = serde_json::Map::new();
            function.insert(
                "arguments".to_string(),
                value
                    .get("delta")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            response_function_call_chunk_base(value, function)?
        }
        "response.function_call_arguments.done" => {
            let mut function = serde_json::Map::new();
            function.insert(
                "arguments_done".to_string(),
                value
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            response_function_call_chunk_base(value, function)?
        }
        _ => return None,
    };

    serde_json::to_string(&vec![serde_json::Value::Object(chunk)]).ok()
}

fn response_function_call_item_chunk(
    value: &serde_json::Value,
    item: &serde_json::Value,
    include_final_arguments: bool,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut function = serde_json::Map::new();

    if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
        function.insert(
            "name".to_string(),
            serde_json::Value::String(name.to_string()),
        );
    }

    if include_final_arguments {
        if let Some(arguments) = item.get("arguments") {
            function.insert("arguments_done".to_string(), arguments.clone());
        }
    }

    response_function_call_chunk_base_with_item(value, item, function)
}

fn response_function_call_chunk_base(
    value: &serde_json::Value,
    function: serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut chunk = serde_json::Map::new();

    if let Some(index) = value.get("output_index").and_then(|v| v.as_u64()) {
        chunk.insert(
            "index".to_string(),
            serde_json::Value::Number(serde_json::Number::from(index)),
        );
    }

    if let Some(id) = value
        .get("item_id")
        .or_else(|| value.get("call_id"))
        .and_then(|v| v.as_str())
    {
        chunk.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    }

    chunk.insert(
        "type".to_string(),
        serde_json::Value::String("function".to_string()),
    );
    chunk.insert("function".to_string(), serde_json::Value::Object(function));

    Some(chunk)
}

fn response_function_call_chunk_base_with_item(
    value: &serde_json::Value,
    item: &serde_json::Value,
    function: serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut chunk = response_function_call_chunk_base(value, function)?;

    if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
        chunk.insert(
            "call_id".to_string(),
            serde_json::Value::String(call_id.to_string()),
        );
    }

    if !chunk.contains_key("id") {
        if let Some(id) = item
            .get("id")
            .or_else(|| item.get("call_id"))
            .and_then(|v| v.as_str())
        {
            chunk.insert("id".to_string(), serde_json::Value::String(id.to_string()));
        }
    }

    Some(chunk)
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
                    "content": openai_responses_user_content(u),
                })),
                Message::Assistant(a) => Some(serde_json::json!({
                    "role": "assistant",
                    "content": a.content,
                })),
                Message::ToolCall(t) => Some(serde_json::json!({
                    "type": "function_call",
                    "call_id": t.call_id,
                    "name": t.name,
                    "arguments": t.arguments,
                })),
                Message::ToolOutput(t) => Some(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": t.call_id,
                    "output": t.output,
                })),
            }
        })
        .collect()
}

fn openai_responses_user_content(user: &crate::message::UserMessage) -> serde_json::Value {
    if user.images.is_empty() {
        return serde_json::json!(user.content);
    }

    let mut parts = Vec::new();
    if !user.content.is_empty() {
        parts.push(serde_json::json!({
            "type": "input_text",
            "text": user.content,
        }));
    }
    parts.extend(user.images.iter().map(|image| {
        serde_json::json!({
            "type": "input_image",
            "image_url": image.data_url,
        })
    }));
    serde_json::Value::Array(parts)
}

#[cfg(test)]
mod tests {
    use super::{build_openai_messages, response_sse_data_to_chunk, responses_function_call_chunk};
    use crate::chunk::{ChunkType, MessagePhase};
    use crate::message::Message;

    #[test]
    fn done_marker_emits_terminal_chunk() {
        let chunk = response_sse_data_to_chunk("[DONE]").expect("expected terminal chunk");

        assert!(matches!(chunk, Ok(ChunkType::End(_))));
    }

    #[test]
    fn response_completed_emits_terminal_chunk() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.completed","response":{"id":"resp_123","end_turn":false}}"#,
        )
        .expect("expected terminal chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::ResponseCompleted {
                end_turn: Some(false)
            })
        ));
    }

    #[test]
    fn maps_responses_assistant_message_phase() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.output_item.done","item":{"type":"message","role":"assistant","phase":"commentary"}}"#,
        )
        .expect("expected message phase chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::AssistantMessagePhase {
                phase: Some(MessagePhase::Commentary)
            })
        ));
    }

    #[test]
    fn maps_responses_function_call_item_to_tool_call_shape() {
        let event = serde_json::json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "id": "fc_123",
                "call_id": "call_123",
                "type": "function_call",
                "name": "read",
                "arguments": ""
            }
        });

        let chunk = responses_function_call_chunk(&event).expect("expected function call chunk");
        let parsed: serde_json::Value = serde_json::from_str(&chunk).unwrap();

        assert_eq!(parsed[0]["index"], 0);
        assert_eq!(parsed[0]["id"], "fc_123");
        assert_eq!(parsed[0]["call_id"], "call_123");
        assert_eq!(parsed[0]["function"]["name"], "read");
    }

    #[test]
    fn maps_responses_function_call_argument_delta_to_tool_call_shape() {
        let event = serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "item_id": "fc_123",
            "delta": "{\"file_path\":\"Cargo.toml\"}"
        });

        let chunk = responses_function_call_chunk(&event).expect("expected argument chunk");
        let parsed: serde_json::Value = serde_json::from_str(&chunk).unwrap();

        assert_eq!(parsed[0]["index"], 0);
        assert_eq!(parsed[0]["id"], "fc_123");
        assert_eq!(
            parsed[0]["function"]["arguments"],
            "{\"file_path\":\"Cargo.toml\"}"
        );
    }

    #[test]
    fn serializes_structured_tool_history_for_responses_input() {
        let input = build_openai_messages(
            &[
                Message::tool_call("call_edit", "edit", "{\"file_path\":\"src/lib.rs\"}"),
                Message::tool_output("call_edit", "edit", "Replaced at line 7", false),
            ],
            false,
        );

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_edit");
        assert_eq!(input[0]["name"], "edit");
        assert_eq!(input[0]["arguments"], "{\"file_path\":\"src/lib.rs\"}");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_edit");
        assert_eq!(input[1]["output"], "Replaced at line 7");
    }
}
