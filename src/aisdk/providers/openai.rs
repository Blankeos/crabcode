use crate::chunk::{ChunkType, MessagePhase};
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::retry::RetryError;
use crate::tool::Tool;
use async_trait::async_trait;
use eventsource_stream::{EventStreamError, Eventsource};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const OPENAI_STREAM_CONNECT_TIMEOUT_SECS: u64 = 30;
const OPENAI_ERROR_BODY_MAX_CHARS: usize = 2048;
const OPENAI_BETA_HEADER: &str = "OpenAI-Beta";
const RESPONSES_WEBSOCKETS_V2_BETA_HEADER_VALUE: &str = "responses_websockets=2026-02-06";
const OPENAI_WEBSOCKET_IDLE_MAX: Duration = Duration::from_secs(60);
const OPENAI_WEBSOCKET_STREAM_RETRIES: usize = 1;

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
    responses_websocket: bool,
    websocket_state: Arc<Mutex<OpenAIWebsocketState>>,
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
    responses_websocket: bool,
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

    pub fn responses_websocket(mut self, enabled: bool) -> Self {
        self.responses_websocket = enabled;
        self
    }

    pub fn build(self) -> Result<OpenAI> {
        let base_url = self
            .base_url
            .ok_or(Error::MissingField("base_url".into()))?;
        let api_key = self.api_key.unwrap_or_default();
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
            responses_websocket: self.responses_websocket,
            websocket_state: Arc::new(Mutex::new(OpenAIWebsocketState::default())),
        })
    }
}

#[derive(Debug, Default)]
struct OpenAIWebsocketState {
    disabled: bool,
    connection: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    last_used_at: Option<Instant>,
    last_request: Option<OpenAIRequestSnapshot>,
    last_response: Option<OpenAIResponseSnapshot>,
}

impl OpenAIWebsocketState {
    fn discard_idle_connection(&mut self) {
        let is_idle = self
            .last_used_at
            .map(|last_used_at| last_used_at.elapsed() > OPENAI_WEBSOCKET_IDLE_MAX)
            .unwrap_or(false);
        if is_idle {
            self.connection = None;
            self.last_used_at = None;
        }
    }

    fn clear_connection(&mut self) {
        self.connection = None;
        self.last_used_at = None;
    }
}

#[derive(Debug, Clone)]
struct OpenAIRequestSnapshot {
    body_without_input: serde_json::Value,
    input: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct OpenAIResponseSnapshot {
    response_id: String,
    items_added: Vec<serde_json::Value>,
}

#[derive(Debug, Default)]
struct WebsocketStreamProgress {
    emitted_non_replayable_output: bool,
}

impl WebsocketStreamProgress {
    fn record_chunk(&mut self, chunk: &ChunkType) {
        if matches!(
            chunk,
            ChunkType::Text(_) | ChunkType::Reasoning(_) | ChunkType::ToolCall(_)
        ) {
            self.emitted_non_replayable_output = true;
        }
    }

    fn can_retry_without_duplicate_output(&self) -> bool {
        !self.emitted_non_replayable_output
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
        let body = self.build_responses_body(input.clone(), tools);

        if self.responses_websocket {
            match self
                .stream_text_websocket(body.clone(), &request_headers)
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    let mut state = self.websocket_state.lock().await;
                    state.disabled = true;
                    state.last_request = None;
                    state.last_response = None;
                    drop(state);
                    eprintln!(
                        "[AISDK_OPENAI] websocket transport failed; falling back to HTTP Responses: {}",
                        err
                    );
                }
            }
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
                Error::RetryableProvider(RetryError::from_message(format_openai_request_error(
                    "send",
                    &url,
                    &err,
                    Some(&request_diagnostics),
                )))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let response_url = sanitized_url(response.url());
            let headers = response.headers().clone();
            let text = match response.text().await {
                Ok(text) => truncate_log_value(&text, OPENAI_ERROR_BODY_MAX_CHARS),
                Err(err) => format!(
                    "<failed to read error body: {}>",
                    format_reqwest_error("read_error_body", &err)
                ),
            };
            let message = format!(
                "OpenAI API error: status={} url={} body={}",
                status, response_url, text
            );
            let retry_error = RetryError::new(message)
                .with_status(status.as_u16())
                .with_headers(&headers);
            if crate::retry::retryable(&retry_error) {
                return Err(Error::RetryableProvider(retry_error));
            }
            return Err(Error::Provider(retry_error.message));
        }

        let request_url = url.clone();
        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(move |ev| match ev {
                Ok(event) => futures::future::ready(response_sse_data_to_chunk(&event.data)),
                Err(e) => {
                    let err = format_openai_sse_error(&e, &request_url);
                    futures::future::ready(Some(Ok(ChunkType::RetryableFailure(
                        RetryError::from_message(err),
                    ))))
                }
            })
            .boxed();

        Ok(stream)
    }
}

impl OpenAI {
    fn build_responses_body(
        &self,
        input: Vec<serde_json::Value>,
        tools: &[Tool],
    ) -> serde_json::Value {
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
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "include": [],
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

        body
    }

    async fn stream_text_websocket(
        &self,
        full_body: serde_json::Value,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<ProviderStream> {
        let ws_url = websocket_url(self.base_url.trim_end_matches('/'), &self.responses_path)?;
        let (request_body, mut ws, reused_connection) = {
            let mut state = self.websocket_state.lock().await;
            if state.disabled {
                return Err(Error::Provider("websocket transport disabled".to_string()));
            }
            state.discard_idle_connection();
            let request_body = websocket_request_body_from_state(&state, &full_body);
            if let Some(ws) = state.connection.take() {
                state.last_used_at = None;
                (request_body, ws, true)
            } else {
                drop(state);
                let ws = connect_openai_websocket(ws_url.clone(), headers).await?;
                (request_body, ws, false)
            }
        };

        let request_text = serde_json::to_string(&request_body)
            .map_err(|err| Error::Provider(format!("failed to encode websocket request: {err}")))?;
        if let Err(err) = ws.send(WsMessage::Text(request_text.clone())).await {
            if !reused_connection {
                return Err(Error::Provider(format!("websocket send failed: {err}")));
            }

            {
                let mut state = self.websocket_state.lock().await;
                state.clear_connection();
            }

            let mut fresh_ws = connect_openai_websocket(ws_url.clone(), headers).await?;
            fresh_ws
                .send(WsMessage::Text(request_text.clone()))
                .await
                .map_err(|err| Error::Provider(format!("websocket send failed: {err}")))?;
            ws = fresh_ws;
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(Ok(ChunkType::Metadata(format!(
            "openai_transport=responses_websocket previous_response_id={} input_items={}",
            request_body.get("previous_response_id").is_some(),
            request_body
                .get("input")
                .and_then(|value| value.as_array())
                .map(|items| items.len())
                .unwrap_or(0)
        ))));
        let websocket_state = Arc::clone(&self.websocket_state);
        let request_snapshot = request_snapshot_from_body(&full_body);
        let retry_ws_url = ws_url.clone();
        let retry_headers = headers.clone();
        tokio::spawn(async move {
            let mut retry_count = 0usize;

            loop {
                let mut response_id = None;
                let mut items_added = Vec::new();
                let mut progress = WebsocketStreamProgress::default();

                let failure = loop {
                    match ws.next().await {
                        Some(Ok(WsMessage::Text(text))) => {
                            collect_websocket_response_state(
                                &text,
                                &mut response_id,
                                &mut items_added,
                            );
                            if let Some(chunk) = response_sse_data_to_chunk(&text) {
                                let is_completed =
                                    matches!(chunk, Ok(ChunkType::ResponseCompleted { .. }));
                                if let Ok(ref chunk) = chunk {
                                    progress.record_chunk(chunk);
                                }
                                if tx.send(chunk).is_err() {
                                    return;
                                }
                                if is_completed {
                                    if let Some(response_id) = response_id {
                                        let mut state = websocket_state.lock().await;
                                        state.connection = Some(ws);
                                        state.last_used_at = Some(Instant::now());
                                        state.last_request = Some(request_snapshot.clone());
                                        state.last_response = Some(OpenAIResponseSnapshot {
                                            response_id,
                                            items_added,
                                        });
                                    }
                                    return;
                                }
                            }
                        }
                        Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => {}
                        Some(Ok(WsMessage::Close(_))) => {
                            break "websocket closed before response.completed".to_string();
                        }
                        Some(Ok(WsMessage::Binary(_))) | Some(Ok(WsMessage::Frame(_))) => {}
                        Some(Err(err)) => {
                            break format!("websocket stream error: {}", err);
                        }
                        None => {
                            break "websocket stream ended before response.completed".to_string();
                        }
                    }
                };

                websocket_state.lock().await.clear_connection();

                if retry_count < OPENAI_WEBSOCKET_STREAM_RETRIES
                    && progress.can_retry_without_duplicate_output()
                {
                    retry_count += 1;
                    if tx
                        .send(Ok(ChunkType::Metadata(format!(
                            "openai_transport=responses_websocket_retry attempt={} reason={}",
                            retry_count, failure
                        ))))
                        .is_err()
                    {
                        return;
                    }

                    let mut fresh_ws = match connect_openai_websocket(
                        retry_ws_url.clone(),
                        &retry_headers,
                    )
                    .await
                    {
                        Ok(ws) => ws,
                        Err(err) => {
                            let _ = tx.send(Ok(ChunkType::Failed(format!(
                                "{}; websocket retry connect failed: {}",
                                failure, err
                            ))));
                            return;
                        }
                    };

                    if let Err(err) = fresh_ws.send(WsMessage::Text(request_text.clone())).await {
                        let _ = tx.send(Ok(ChunkType::Failed(format!(
                            "{}; websocket retry send failed: {}",
                            failure, err
                        ))));
                        return;
                    }

                    ws = fresh_ws;
                    continue;
                }

                let _ = tx.send(Ok(ChunkType::Failed(failure)));
                return;
            }
        });

        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        })))
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

async fn connect_openai_websocket(
    ws_url: reqwest::Url,
    headers: &reqwest::header::HeaderMap,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let mut request = ws_url
        .as_str()
        .into_client_request()
        .map_err(|err| Error::Provider(format!("failed to build websocket request: {err}")))?;
    request.headers_mut().extend(headers.clone());
    request.headers_mut().insert(
        OPENAI_BETA_HEADER,
        RESPONSES_WEBSOCKETS_V2_BETA_HEADER_VALUE
            .parse()
            .map_err(|err| Error::Provider(format!("invalid websocket beta header: {err}")))?,
    );

    connect_async(request)
        .await
        .map(|(ws, _)| ws)
        .map_err(|err| Error::Provider(format!("websocket connect failed: {err}")))
}

fn websocket_url(base_url: &str, responses_path: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("{base_url}{responses_path}"))
        .map_err(|err| Error::Provider(format!("failed to build websocket URL: {err}")))?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => return Ok(url),
        other => {
            return Err(Error::Provider(format!(
                "unsupported websocket URL scheme: {other}"
            )));
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| Error::Provider("failed to set websocket URL scheme".to_string()))?;
    Ok(url)
}

fn websocket_request_body_from_state(
    state: &OpenAIWebsocketState,
    full_body: &serde_json::Value,
) -> serde_json::Value {
    let input = full_body
        .get("input")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let body_without_input = body_without_input(full_body);

    let incremental_input = state
        .last_request
        .as_ref()
        .zip(state.last_response.as_ref())
        .and_then(|(last_request, last_response)| {
            if last_request.body_without_input != body_without_input {
                return None;
            }

            let mut baseline = last_request.input.clone();
            baseline.extend(last_response.items_added.clone());
            if input_starts_with(&input, &baseline) {
                Some((
                    last_response.response_id.clone(),
                    input[baseline.len()..].to_vec(),
                ))
            } else {
                None
            }
        });

    let mut request_body = full_body.clone();
    if let Some((previous_response_id, delta_input)) = incremental_input {
        request_body["previous_response_id"] = serde_json::Value::String(previous_response_id);
        request_body["input"] = serde_json::Value::Array(delta_input);
    }
    request_body["type"] = serde_json::Value::String("response.create".to_string());
    request_body
}

fn body_without_input(body: &serde_json::Value) -> serde_json::Value {
    let mut body = body.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.remove("input");
        obj.remove("previous_response_id");
        obj.remove("type");
    }
    body
}

fn request_snapshot_from_body(body: &serde_json::Value) -> OpenAIRequestSnapshot {
    OpenAIRequestSnapshot {
        body_without_input: body_without_input(body),
        input: body
            .get("input")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default(),
    }
}

fn input_starts_with(input: &[serde_json::Value], baseline: &[serde_json::Value]) -> bool {
    input.len() >= baseline.len()
        && input
            .iter()
            .zip(baseline.iter())
            .all(|(left, right)| input_items_equivalent(left, right))
}

fn input_items_equivalent(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    normalize_input_item_for_prefix(left) == normalize_input_item_for_prefix(right)
}

fn normalize_input_item_for_prefix(item: &serde_json::Value) -> serde_json::Value {
    if item.get("type").and_then(|value| value.as_str()) == Some("message") {
        if let Some(role) = item.get("role").and_then(|value| value.as_str()) {
            if let Some(content) = response_message_content_as_text(item.get("content")) {
                return serde_json::json!({
                    "role": role,
                    "content": content,
                });
            }
        }
    }

    let mut normalized = item.clone();
    if normalized.get("type").and_then(|value| value.as_str()) == Some("function_call") {
        if let Some(obj) = normalized.as_object_mut() {
            obj.remove("id");
            obj.remove("status");
        }
    }
    normalized
}

fn response_message_content_as_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                let part_type = part.get("type").and_then(|value| value.as_str());
                if matches!(
                    part_type,
                    Some("output_text") | Some("text") | Some("input_text")
                ) {
                    if let Some(part_text) = part.get("text").and_then(|value| value.as_str()) {
                        text.push_str(part_text);
                    }
                }
            }
            Some(text)
        }
        _ => None,
    }
}

fn collect_websocket_response_state(
    text: &str,
    response_id: &mut Option<String>,
    items_added: &mut Vec<serde_json::Value>,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    match value.get("type").and_then(|value| value.as_str()) {
        Some("response.created") => {
            if let Some(id) = value
                .get("response")
                .and_then(|response| response.get("id"))
                .and_then(|id| id.as_str())
            {
                *response_id = Some(id.to_string());
            }
        }
        Some("response.output_item.done") => {
            if let Some(item) = value.get("item") {
                items_added.push(item.clone());
            }
        }
        Some("response.completed") => {
            if response_id.is_none() {
                if let Some(id) = value
                    .get("response")
                    .and_then(|response| response.get("id"))
                    .and_then(|id| id.as_str())
                {
                    *response_id = Some(id.to_string());
                }
            }
        }
        _ => {}
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
        return Some(Ok(ChunkType::End { reason: None }));
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
                Message::ToolCall(t) => {
                    let mut item = serde_json::json!({
                        "type": "function_call",
                        "call_id": t.call_id,
                        "name": t.name,
                        "arguments": t.arguments,
                    });
                    if let Some(item_id) = &t.item_id {
                        item["id"] = serde_json::Value::String(item_id.clone());
                    }
                    Some(item)
                }
                Message::ToolOutput(t) => Some(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": t.call_id,
                    "output": openai_tool_output_content(t),
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

fn openai_tool_output_content(tool: &crate::message::ToolOutputMessage) -> serde_json::Value {
    if tool.images.is_empty() {
        return serde_json::json!(tool.output);
    }

    let mut parts = Vec::new();
    if !tool.output.is_empty() {
        parts.push(serde_json::json!({
            "type": "input_text",
            "text": tool.output,
        }));
    }
    parts.extend(tool.images.iter().map(|image| {
        serde_json::json!({
            "type": "input_image",
            "image_url": image.data_url,
        })
    }));
    serde_json::Value::Array(parts)
}

#[cfg(test)]
mod tests {
    use super::{
        build_openai_messages, request_snapshot_from_body, response_sse_data_to_chunk,
        responses_function_call_chunk, websocket_request_body_from_state, OpenAI,
        OpenAIResponseSnapshot, OpenAIWebsocketState, WebsocketStreamProgress,
    };
    use crate::chunk::{ChunkType, MessagePhase};
    use crate::message::Message;
    use std::time::{Duration, Instant};

    #[test]
    fn builder_allows_missing_api_key() {
        let provider = OpenAI::builder()
            .base_url("http://localhost:11434/v1")
            .model_name("local-model")
            .provider_name("local-openai")
            .build()
            .expect("api key should be optional");

        assert!(provider.api_key.is_empty());
    }

    #[test]
    fn done_marker_emits_terminal_chunk() {
        let chunk = response_sse_data_to_chunk("[DONE]").expect("expected terminal chunk");

        assert!(matches!(chunk, Ok(ChunkType::End { .. })));
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
    fn websocket_stream_progress_allows_retry_before_output() {
        let mut progress = WebsocketStreamProgress::default();

        progress.record_chunk(&ChunkType::Metadata(
            "openai_transport=responses_websocket".to_string(),
        ));
        progress.record_chunk(&ChunkType::AssistantMessagePhase {
            phase: Some(MessagePhase::Commentary),
        });

        assert!(progress.can_retry_without_duplicate_output());
    }

    #[test]
    fn websocket_stream_progress_blocks_retry_after_replay_unsafe_chunks() {
        for chunk in [
            ChunkType::Text("partial".to_string()),
            ChunkType::Reasoning("thinking".to_string()),
            ChunkType::ToolCall(r#"[{"id":"call_1"}]"#.to_string()),
        ] {
            let mut progress = WebsocketStreamProgress::default();
            progress.record_chunk(&chunk);

            assert!(!progress.can_retry_without_duplicate_output());
        }
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
                Message::tool_call_with_item_id(
                    "fc_edit",
                    "call_edit",
                    "edit",
                    "{\"file_path\":\"src/lib.rs\"}",
                ),
                Message::tool_output("call_edit", "edit", "Replaced at line 7", false),
            ],
            false,
        );

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["id"], "fc_edit");
        assert_eq!(input[0]["call_id"], "call_edit");
        assert_eq!(input[0]["name"], "edit");
        assert_eq!(input[0]["arguments"], "{\"file_path\":\"src/lib.rs\"}");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_edit");
        assert_eq!(input[1]["output"], "Replaced at line 7");
    }

    #[test]
    fn serializes_tool_image_output_for_responses_input() {
        let input = build_openai_messages(
            &[Message::tool_output_with_images(
                "call_image",
                "view_image",
                "Viewed image assets/screenshot_1.png",
                vec![crate::message::ImageContent {
                    data_url: "data:image/png;base64,AAA".to_string(),
                    media_type: "image/png".to_string(),
                }],
                false,
            )],
            false,
        );

        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_image");
        let output = input[0]["output"].as_array().expect("content items");
        assert_eq!(output[0]["type"], "input_text");
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(output[1]["image_url"], "data:image/png;base64,AAA");
    }

    #[tokio::test]
    async fn websocket_request_uses_previous_response_id_for_append_only_delta() {
        let provider = OpenAI::builder()
            .base_url("https://chatgpt.com")
            .api_key("")
            .model_name("gpt-test")
            .build()
            .unwrap();
        let previous_input = vec![serde_json::json!({
            "role": "user",
            "content": "read the file"
        })];
        let previous_body = provider.build_responses_body(previous_input.clone(), &[]);
        let function_call = serde_json::json!({
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "read",
            "arguments": "{\"file_path\":\"Cargo.toml\"}"
        });
        let function_output = serde_json::json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "00001| [package]"
        });

        {
            let mut state = provider.websocket_state.lock().await;
            state.last_request = Some(request_snapshot_from_body(&previous_body));
            state.last_response = Some(OpenAIResponseSnapshot {
                response_id: "resp_1".to_string(),
                items_added: vec![function_call.clone()],
            });
        }

        let mut next_input = previous_input;
        next_input.push(function_call);
        next_input.push(function_output.clone());
        let next_body = provider.build_responses_body(next_input, &[]);

        let state = provider.websocket_state.lock().await;
        let ws_body = websocket_request_body_from_state(&state, &next_body);

        assert_eq!(ws_body["type"], "response.create");
        assert_eq!(ws_body["previous_response_id"], "resp_1");
        assert_eq!(ws_body["input"], serde_json::json!([function_output]));
    }

    #[tokio::test]
    async fn websocket_request_uses_previous_response_id_for_assistant_message_shape_delta() {
        let provider = OpenAI::builder()
            .base_url("https://chatgpt.com")
            .api_key("")
            .model_name("gpt-test")
            .build()
            .unwrap();
        let previous_input = vec![serde_json::json!({
            "role": "user",
            "content": "inspect the code"
        })];
        let previous_body = provider.build_responses_body(previous_input.clone(), &[]);
        let response_assistant_message = serde_json::json!({
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "status": "completed",
            "content": [
                { "type": "output_text", "text": "I'll inspect the code." }
            ]
        });

        {
            let mut state = provider.websocket_state.lock().await;
            state.last_request = Some(request_snapshot_from_body(&previous_body));
            state.last_response = Some(OpenAIResponseSnapshot {
                response_id: "resp_1".to_string(),
                items_added: vec![response_assistant_message],
            });
        }

        let mut next_input = previous_input;
        next_input.push(serde_json::json!({
            "role": "assistant",
            "content": "I'll inspect the code."
        }));
        let next_body = provider.build_responses_body(next_input, &[]);

        let state = provider.websocket_state.lock().await;
        let ws_body = websocket_request_body_from_state(&state, &next_body);

        assert_eq!(ws_body["previous_response_id"], "resp_1");
        assert_eq!(ws_body["input"], serde_json::json!([]));
    }

    #[test]
    fn websocket_connection_clear_preserves_response_history() {
        let mut state = OpenAIWebsocketState {
            last_used_at: Some(Instant::now() - Duration::from_secs(120)),
            last_response: Some(OpenAIResponseSnapshot {
                response_id: "resp_1".to_string(),
                items_added: vec![],
            }),
            ..OpenAIWebsocketState::default()
        };

        state.clear_connection();

        assert!(state.last_used_at.is_none());
        assert_eq!(
            state
                .last_response
                .as_ref()
                .map(|response| response.response_id.as_str()),
            Some("resp_1")
        );
    }

    #[tokio::test]
    async fn websocket_request_uses_full_input_when_not_append_only() {
        let provider = OpenAI::builder()
            .base_url("https://chatgpt.com")
            .api_key("")
            .model_name("gpt-test")
            .build()
            .unwrap();
        let previous_body = provider.build_responses_body(
            vec![serde_json::json!({"role": "user", "content": "first"})],
            &[],
        );
        {
            let mut state = provider.websocket_state.lock().await;
            state.last_request = Some(request_snapshot_from_body(&previous_body));
            state.last_response = Some(OpenAIResponseSnapshot {
                response_id: "resp_1".to_string(),
                items_added: vec![],
            });
        }
        let next_body = provider.build_responses_body(
            vec![serde_json::json!({"role": "user", "content": "different"})],
            &[],
        );

        let state = provider.websocket_state.lock().await;
        let ws_body = websocket_request_body_from_state(&state, &next_body);

        assert!(ws_body.get("previous_response_id").is_none());
        assert_eq!(ws_body["input"], next_body["input"]);
    }
}
