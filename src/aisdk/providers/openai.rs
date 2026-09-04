use crate::chunk::{ChunkType, MessagePhase, ReasoningReplayItem};
use crate::error::{Error, Result};
use crate::message::{is_prefixed_response_item_id, Message};
use crate::provider::{Provider, ProviderStream};
use crate::retry::RetryError;
use crate::tool::Tool;
use async_trait::async_trait;
use eventsource_stream::{EventStreamError, Eventsource};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::atomic::{AtomicBool, Ordering};
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
const OPENAI_RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const OPENAI_RESPONSES_LITE_WS_METADATA_KEY: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";
const OPENAI_SESSION_ID_HEADER: &str = "session-id";
const OPENAI_THREAD_ID_HEADER: &str = "thread-id";
const OPENAI_CLIENT_REQUEST_ID_HEADER: &str = "x-client-request-id";
const OPENAI_CODEX_WINDOW_ID_HEADER: &str = "x-codex-window-id";
const OPENAI_WEBSOCKET_IDLE_MAX: Duration = Duration::from_secs(60);
const OPENAI_WEBSOCKET_IO_TIMEOUT: Duration = Duration::from_secs(300);
const OPENAI_WEBSOCKET_STREAM_RETRIES: usize = 1;
const OPENAI_WEBSOCKET_FAILURES_BEFORE_FALLBACK: usize = 5;

fn responses_incomplete_chunk(value: &serde_json::Value) -> ChunkType {
    let reason = value
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(serde_json::Value::as_str);
    match reason {
        Some("max_output_tokens" | "max_tokens") => ChunkType::End {
            reason: Some(crate::chunk::FinishReason::Length),
        },
        Some("content_filter" | "refusal" | "safety") => ChunkType::End {
            reason: Some(crate::chunk::FinishReason::Refusal),
        },
        _ => ChunkType::RetryableFailure(RetryError::from_message(responses_incomplete_message(
            value,
        ))),
    }
}

#[async_trait]
pub trait HttpResponseRetryPolicy: Send + Sync + std::fmt::Debug {
    async fn retry_headers(
        &self,
        status: reqwest::StatusCode,
    ) -> Option<reqwest::header::HeaderMap>;
}

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
    responses_lite: bool,
    prompt_cache_key: Option<String>,

    response_retry_policy: Option<Arc<dyn HttpResponseRetryPolicy>>,
    websocket_state: Arc<Mutex<OpenAIWebsocketState>>,
}

fn openai_chunk_is_terminal(chunk: &Result<ChunkType>) -> bool {
    matches!(
        chunk,
        Ok(ChunkType::End { .. })
            | Ok(ChunkType::ResponseCompleted { .. })
            | Ok(ChunkType::RetryableFailure(_))
            | Ok(ChunkType::Failed(_))
            | Ok(ChunkType::Incomplete(_))
            | Err(_)
    )
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
    responses_lite: bool,
    prompt_cache_key: Option<String>,

    response_retry_policy: Option<Arc<dyn HttpResponseRetryPolicy>>,
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

    pub fn responses_lite(mut self, enabled: bool) -> Self {
        self.responses_lite = enabled;
        self
    }

    pub fn prompt_cache_key(mut self, key: impl Into<String>) -> Self {
        self.prompt_cache_key = Some(key.into());
        self
    }

    pub fn response_retry_policy(mut self, policy: Arc<dyn HttpResponseRetryPolicy>) -> Self {
        self.response_retry_policy = Some(policy);
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
                // Gateways often ship base URLs that already include a version
                // segment (e.g. https://opencode.ai/zen/go/v1). Appending the
                // default `/v1/responses` there produces a duplicated
                // `/v1/v1/...` path that 404s.
                if super::base_url_has_version_segment(base_url.trim_end_matches('/')) {
                    "/responses".to_string()
                } else {
                    "/v1/responses".to_string()
                }
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
            responses_lite: self.responses_lite,
            prompt_cache_key: self.prompt_cache_key,

            response_retry_policy: self.response_retry_policy,
            websocket_state: Arc::new(Mutex::new(OpenAIWebsocketState::default())),
        })
    }
}

#[derive(Debug, Default)]
struct OpenAIWebsocketState {
    disabled: bool,
    consecutive_failures: usize,
    connection: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    last_used_at: Option<Instant>,
    last_request: Option<OpenAIRequestSnapshot>,
    last_response: Option<OpenAIResponseSnapshot>,
}

impl OpenAIWebsocketState {
    fn discard_idle_connection(&mut self) {
        if websocket_connection_is_idle(self.last_used_at, OPENAI_WEBSOCKET_IDLE_MAX) {
            self.connection = None;
            self.last_used_at = None;
        }
    }

    fn clear_connection(&mut self) {
        self.connection = None;
        self.last_used_at = None;
    }

    fn disable(&mut self) {
        self.disabled = true;
        self.consecutive_failures = 0;
        self.connection = None;
        self.last_used_at = None;
        self.last_request = None;
        self.last_response = None;
    }

    fn record_failure(&mut self) -> bool {
        self.clear_connection();
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures < OPENAI_WEBSOCKET_FAILURES_BEFORE_FALLBACK {
            return false;
        }

        self.disabled = true;
        self.last_request = None;
        self.last_response = None;
        true
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
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

/// Whether the outbound websocket request reuses the live socket that produced
/// `last_response`, or opens/sends on a different socket (reconnect, idle eviction, retry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebsocketContinuationMode {
    SameLiveSocket,
    FreshSocket,
}

#[derive(Debug, Default)]
struct WebsocketStreamProgress {
    emitted_non_replayable_output: bool,
}

impl WebsocketStreamProgress {
    fn record_chunk(&mut self, chunk: &ChunkType) {
        if matches!(
            chunk,
            ChunkType::Text(_)
                | ChunkType::Reasoning(_)
                | ChunkType::ReasoningItem(_)
                | ChunkType::ToolCall(_)
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
        add_responses_lite_header(&mut request_headers, self.responses_lite);
        if self.responses_lite {
            if let Some(session_id) = self
                .prompt_cache_key
                .as_deref()
                .map(str::trim)
                .filter(|session_id| !session_id.is_empty())
            {
                for name in [
                    OPENAI_SESSION_ID_HEADER,
                    OPENAI_THREAD_ID_HEADER,
                    OPENAI_CLIENT_REQUEST_ID_HEADER,
                    OPENAI_CODEX_WINDOW_ID_HEADER,
                ] {
                    if let (Ok(name), Ok(value)) = (
                        reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                        reqwest::header::HeaderValue::from_str(session_id),
                    ) {
                        request_headers.insert(name, value);
                    }
                }
            }
        }

        let input = build_openai_messages(
            messages,
            self.strip_system_and_developer_messages,
            self.responses_lite,
        );
        let body = self.build_responses_body(input.clone(), tools);

        let mut fallback_warning = None;
        if self.responses_websocket {
            match self
                .stream_text_websocket(body.clone(), &request_headers)
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    if !is_websocket_transport_disabled(&err) {
                        let mut state = self.websocket_state.lock().await;
                        state.disable();
                        drop(state);
                        fallback_warning = Some(websocket_fallback_warning(&err));
                    }
                    eprintln!(
                        "[AISDK_OPENAI] websocket transport failed; falling back to HTTP Responses: {}",
                        err
                    );
                }
            }
        }

        let request_diagnostics =
            openai_request_diagnostics(self, &input, tools, &body, &request_headers);

        if let Ok(dir) = std::env::var("CRABCODE_DUMP_REQUEST_DIR") {
            let path = std::path::PathBuf::from(dir).join(format!(
                "openai-responses-{}.json",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or_default()
            ));
            if let Ok(pretty) = serde_json::to_string_pretty(&body) {
                let _ = std::fs::write(&path, pretty);
            }
        }

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(
                OPENAI_STREAM_CONNECT_TIMEOUT_SECS,
            ))
            .build()
            .map_err(|e| Error::Provider(format!("Failed to build client: {}", e)))?;
        let mut response = client
            .post(&url)
            .headers(request_headers.clone())
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

        if let Some(policy) = &self.response_retry_policy {
            if let Some(retry_headers) = policy.retry_headers(response.status()).await {
                request_headers.extend(retry_headers);
                response = client
                    .post(&url)
                    .headers(request_headers)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|err| {
                        Error::RetryableProvider(RetryError::from_message(
                            format_openai_request_error(
                                "send_after_response_retry_policy",
                                &url,
                                &err,
                                Some(&request_diagnostics),
                            ),
                        ))
                    })?;
            }
        }

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
        let saw_terminal_event = Arc::new(AtomicBool::new(false));
        let saw_terminal_event_in_stream = saw_terminal_event.clone();
        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(move |ev| match ev {
                Ok(event) => {
                    let chunk = response_sse_data_to_chunk(&event.data);
                    if chunk
                        .as_ref()
                        .is_some_and(|chunk| openai_chunk_is_terminal(chunk))
                    {
                        saw_terminal_event_in_stream.store(true, Ordering::Relaxed);
                    }
                    futures::future::ready(chunk)
                }
                Err(e) => {
                    let err = format_openai_sse_error(&e, &request_url);
                    saw_terminal_event_in_stream.store(true, Ordering::Relaxed);
                    futures::future::ready(Some(Ok(ChunkType::RetryableFailure(
                        RetryError::from_message(err),
                    ))))
                }
            })
            .chain(
                futures::stream::once(async move {
                    (!saw_terminal_event.load(Ordering::Relaxed)).then(|| {
                        Ok(ChunkType::RetryableFailure(RetryError::from_message(
                            "OpenAI Responses stream closed before response.completed",
                        )))
                    })
                })
                .filter_map(futures::future::ready),
            )
            .boxed();

        if let Some(warning) = fallback_warning {
            let stream =
                futures::stream::once(futures::future::ready(Ok(ChunkType::Warning(warning))))
                    .chain(stream)
                    .boxed();
            return Ok(stream);
        }

        Ok(stream)
    }
}

fn is_websocket_transport_disabled(err: &Error) -> bool {
    err.to_string()
        .to_ascii_lowercase()
        .contains("websocket transport disabled")
}

fn websocket_fallback_warning(err: &Error) -> String {
    format!("Falling back from WebSockets to HTTPS transport. {err}")
}

fn stream_disconnected_before_completion(reason: &str) -> String {
    let reason = reason.trim();
    if reason
        .to_ascii_lowercase()
        .contains("stream disconnected before completion")
    {
        reason.to_string()
    } else {
        format!("stream disconnected before completion: {reason}")
    }
}

fn add_responses_lite_header(headers: &mut reqwest::header::HeaderMap, enabled: bool) {
    if enabled {
        headers.insert(
            OPENAI_RESPONSES_LITE_HEADER,
            reqwest::header::HeaderValue::from_static("true"),
        );
    }
}

impl OpenAI {
    fn build_responses_body(
        &self,
        mut input: Vec<serde_json::Value>,
        tools: &[Tool],
    ) -> serde_json::Value {
        let mut tool_params: Vec<serde_json::Value> = Vec::new();
        for t in tools {
            match &t.transport {
                crate::aisdk::tool::ToolTransport::ProviderNative(value) => {
                    tool_params.push(value.clone());
                }
                crate::aisdk::tool::ToolTransport::OpenRouterPlugin(_) => {
                    // Plugins are not Responses `tools` entries.
                }
                crate::aisdk::tool::ToolTransport::ClientFunction => {
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
                    tool_params.push(tool);
                }
            }
        }

        if self.responses_lite {
            let mut prefix = vec![serde_json::json!({
                "type": "additional_tools",
                "role": "developer",
                "tools": tool_params,
            })];
            if let Some(instructions) = self
                .default_instructions
                .as_deref()
                .map(str::trim)
                .filter(|instructions| !instructions.is_empty())
            {
                prefix.push(serde_json::json!({
                    "type": "message",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": instructions,
                    }],
                }));
            }
            prefix.append(&mut input);
            input = prefix;
        }

        let mut body = serde_json::json!({
            "model": self.model_name,
            "input": input,
            "stream": true,
            // Responses only returns `encrypted_content` when this include is set.
            "include": serde_json::json!(["reasoning.encrypted_content"]),
        });

        if self.responses_lite {
            body["tool_choice"] = serde_json::Value::String("auto".to_string());
            body["parallel_tool_calls"] = serde_json::Value::Bool(false);
            body["instructions"] = serde_json::Value::String(String::new());
            body["text"] = serde_json::json!({ "verbosity": "low" });

            let mut client_metadata = serde_json::Map::from_iter([(
                OPENAI_RESPONSES_LITE_WS_METADATA_KEY.to_string(),
                serde_json::Value::String("true".to_string()),
            )]);
            if let Some(session_id) = self
                .prompt_cache_key
                .as_deref()
                .map(str::trim)
                .filter(|session_id| !session_id.is_empty())
            {
                for name in ["session_id", "thread_id", OPENAI_CODEX_WINDOW_ID_HEADER] {
                    client_metadata.insert(
                        name.to_string(),
                        serde_json::Value::String(session_id.to_string()),
                    );
                }
            }
            body["client_metadata"] = serde_json::Value::Object(client_metadata);
        } else if !tool_params.is_empty() {
            body["tools"] = serde_json::Value::Array(tool_params);
            body["tool_choice"] = serde_json::Value::String("auto".to_string());
            body["parallel_tool_calls"] = serde_json::Value::Bool(true);
        }

        if !self.responses_lite {
            if let Some(instructions) = &self.default_instructions {
                body["instructions"] = serde_json::Value::String(instructions.clone());
            }
        }

        if let Some(store) = self.store_override {
            body["store"] = serde_json::Value::Bool(store);
        }

        if self.responses_lite {
            let mut reasoning = serde_json::json!({
                "context": "all_turns",
            });
            if let Some(effort) = &self.reasoning_effort {
                reasoning["effort"] = serde_json::Value::String(effort.clone());
            }
            body["reasoning"] = reasoning;
        } else if let Some(effort) = &self.reasoning_effort {
            body["reasoning"] = serde_json::json!({ "effort": effort });
        }

        if let Some(key) = &self.prompt_cache_key {
            if !key.is_empty() {
                body["prompt_cache_key"] = serde_json::Value::String(key.clone());
            }
        }

        body
    }

    async fn stream_text_websocket(
        &self,
        full_body: serde_json::Value,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<ProviderStream> {
        let ws_url = websocket_url(self.base_url.trim_end_matches('/'), &self.responses_path)?;
        let (mut sent_request_body, mut ws, reused_connection) = {
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

        let mut request_text = serde_json::to_string(&sent_request_body)
            .map_err(|err| Error::Provider(format!("failed to encode websocket request: {err}")))?;
        if let Err(err) = send_openai_websocket_text(&mut ws, request_text.clone()).await {
            if !reused_connection {
                return Err(Error::Provider(format!("websocket send failed: {err}")));
            }

            {
                let mut state = self.websocket_state.lock().await;
                state.clear_connection();
            }

            let mut fresh_ws = connect_openai_websocket(ws_url.clone(), headers).await?;
            sent_request_body = fresh_websocket_request_body(&full_body);
            request_text = serde_json::to_string(&sent_request_body).map_err(|err| {
                Error::Provider(format!("failed to encode websocket request: {err}"))
            })?;
            send_openai_websocket_text(&mut fresh_ws, request_text.clone()).await?;
            ws = fresh_ws;
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let _ = tx.send(Ok(ChunkType::Metadata(format!(
            "openai_transport=responses_websocket previous_response_id={} input_items={}",
            sent_request_body.get("previous_response_id").is_some(),
            sent_request_body
                .get("input")
                .and_then(|value| value.as_array())
                .map(|items| items.len())
                .unwrap_or(0)
        ))));
        let websocket_state = Arc::clone(&self.websocket_state);
        let request_snapshot = request_snapshot_from_body(&full_body);
        let retry_full_body = full_body.clone();
        let retry_ws_url = ws_url.clone();
        let retry_headers = headers.clone();
        tokio::spawn(async move {
            let mut retry_count = 0usize;

            loop {
                let mut response_id = None;
                let mut items_added = Vec::new();
                let mut progress = WebsocketStreamProgress::default();

                let failure = loop {
                    let next_message =
                        tokio::time::timeout(OPENAI_WEBSOCKET_IO_TIMEOUT, ws.next()).await;
                    match next_message {
                        Err(_) => {
                            break format!(
                                "websocket idle timeout after {} seconds",
                                OPENAI_WEBSOCKET_IO_TIMEOUT.as_secs()
                            );
                        }
                        Ok(Some(Ok(WsMessage::Text(text)))) => {
                            collect_websocket_response_state(
                                &text,
                                &mut response_id,
                                &mut items_added,
                            );
                            if let Some(chunk) = response_sse_data_to_chunk(&text) {
                                let is_terminal = matches!(
                                    chunk,
                                    Ok(ChunkType::ResponseCompleted { .. })
                                        | Ok(ChunkType::RetryableFailure(_))
                                        | Ok(ChunkType::Failed(_))
                                        | Ok(ChunkType::Incomplete(_))
                                );
                                let is_completed =
                                    matches!(chunk, Ok(ChunkType::ResponseCompleted { .. }));
                                if let Ok(ref chunk) = chunk {
                                    progress.record_chunk(chunk);
                                }
                                if tx.send(chunk).is_err() {
                                    return;
                                }
                                if is_completed {
                                    let mut state = websocket_state.lock().await;
                                    state.record_success();
                                    if let Some(response_id) = response_id {
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
                                if is_terminal {
                                    websocket_state.lock().await.clear_connection();
                                    return;
                                }
                            }
                        }
                        Ok(Some(Ok(WsMessage::Ping(_)))) | Ok(Some(Ok(WsMessage::Pong(_)))) => {}
                        Ok(Some(Ok(WsMessage::Close(_)))) => {
                            break "websocket closed before response.completed".to_string();
                        }
                        Ok(Some(Ok(WsMessage::Binary(_)))) | Ok(Some(Ok(WsMessage::Frame(_)))) => {}
                        Ok(Some(Err(err))) => {
                            break format!("websocket stream error: {}", err);
                        }
                        Ok(None) => {
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
                            let fallback_to_http = websocket_state.lock().await.record_failure();
                            let disconnected = stream_disconnected_before_completion(&format!(
                                "{}; websocket retry connect failed: {}",
                                failure, err
                            ));
                            if fallback_to_http {
                                let _ = tx.send(Ok(ChunkType::Warning(format!(
                                    "Falling back from WebSockets to HTTPS transport. {disconnected}"
                                ))));
                            }
                            let _ = tx.send(Ok(ChunkType::RetryableFailure(
                                RetryError::from_message(disconnected),
                            )));
                            return;
                        }
                    };

                    let retry_request_text = match serde_json::to_string(
                        &fresh_websocket_request_body(&retry_full_body),
                    ) {
                        Ok(text) => text,
                        Err(err) => {
                            websocket_state.lock().await.disable();
                            let disconnected = stream_disconnected_before_completion(&format!(
                                "{}; websocket retry encode failed: {}",
                                failure, err
                            ));
                            let _ = tx.send(Ok(ChunkType::Warning(format!(
                                "Falling back from WebSockets to HTTPS transport. {disconnected}"
                            ))));
                            let _ = tx.send(Ok(ChunkType::RetryableFailure(
                                RetryError::from_message(disconnected),
                            )));
                            return;
                        }
                    };

                    if let Err(err) =
                        send_openai_websocket_text(&mut fresh_ws, retry_request_text).await
                    {
                        let fallback_to_http = websocket_state.lock().await.record_failure();
                        let disconnected = stream_disconnected_before_completion(&format!(
                            "{}; websocket retry send failed: {}",
                            failure, err
                        ));
                        if fallback_to_http {
                            let _ = tx.send(Ok(ChunkType::Warning(format!(
                                "Falling back from WebSockets to HTTPS transport. {disconnected}"
                            ))));
                        }
                        let _ = tx.send(Ok(ChunkType::RetryableFailure(RetryError::from_message(
                            disconnected,
                        ))));
                        return;
                    }

                    ws = fresh_ws;
                    continue;
                }

                let fallback_to_http = websocket_state.lock().await.record_failure();
                let disconnected = stream_disconnected_before_completion(&failure);
                if fallback_to_http {
                    let _ = tx.send(Ok(ChunkType::Warning(format!(
                        "Falling back from WebSockets to HTTPS transport. {disconnected}"
                    ))));
                }
                let _ = tx.send(Ok(ChunkType::RetryableFailure(RetryError::from_message(
                    disconnected,
                ))));
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

    tokio::time::timeout(
        Duration::from_secs(OPENAI_STREAM_CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .map_err(|_| {
        Error::Provider(format!(
            "websocket connect timed out after {} seconds",
            OPENAI_STREAM_CONNECT_TIMEOUT_SECS
        ))
    })?
    .map(|(ws, _)| ws)
    .map_err(|err| Error::Provider(format!("websocket connect failed: {err}")))
}

async fn send_openai_websocket_text(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    text: String,
) -> Result<()> {
    tokio::time::timeout(OPENAI_WEBSOCKET_IO_TIMEOUT, ws.send(WsMessage::Text(text)))
        .await
        .map_err(|_| {
            Error::Provider(format!(
                "websocket send timed out after {} seconds",
                OPENAI_WEBSOCKET_IO_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|err| Error::Provider(format!("websocket send failed: {err}")))
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

fn websocket_connection_is_idle(last_used_at: Option<Instant>, idle_max: Duration) -> bool {
    last_used_at
        .map(|last_used_at| last_used_at.elapsed() > idle_max)
        .unwrap_or(false)
}

fn websocket_continuation_mode_for_live_connection(
    has_live_connection: bool,
) -> WebsocketContinuationMode {
    if has_live_connection {
        WebsocketContinuationMode::SameLiveSocket
    } else {
        WebsocketContinuationMode::FreshSocket
    }
}

/// Continuation mode after idle policy: a socket that would be evicted is not live.
#[cfg(test)]
fn websocket_continuation_mode_after_idle_policy(
    has_connection: bool,
    last_used_at: Option<Instant>,
    idle_max: Duration,
) -> WebsocketContinuationMode {
    let connection_survives_idle =
        has_connection && !websocket_connection_is_idle(last_used_at, idle_max);
    websocket_continuation_mode_for_live_connection(connection_survives_idle)
}

fn websocket_continuation_mode_from_state(
    state: &OpenAIWebsocketState,
) -> WebsocketContinuationMode {
    websocket_continuation_mode_for_live_connection(state.connection.is_some())
}

fn compute_incremental_websocket_input(
    state: &OpenAIWebsocketState,
    full_body: &serde_json::Value,
) -> Option<(String, Vec<serde_json::Value>)> {
    let input = full_body
        .get("input")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let body_without_input = body_without_input(full_body);

    state
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
        })
}

fn build_websocket_request_body(
    state: &OpenAIWebsocketState,
    full_body: &serde_json::Value,
    continuation_mode: WebsocketContinuationMode,
) -> serde_json::Value {
    let mut request_body = full_body.clone();
    if continuation_mode == WebsocketContinuationMode::SameLiveSocket {
        if let Some((previous_response_id, delta_input)) =
            compute_incremental_websocket_input(state, full_body)
        {
            request_body["previous_response_id"] = serde_json::Value::String(previous_response_id);
            request_body["input"] = serde_json::Value::Array(delta_input);
        }
    }
    request_body["type"] = serde_json::Value::String("response.create".to_string());
    request_body
}

fn websocket_request_body_from_state(
    state: &OpenAIWebsocketState,
    full_body: &serde_json::Value,
) -> serde_json::Value {
    build_websocket_request_body(
        state,
        full_body,
        websocket_continuation_mode_from_state(state),
    )
}

/// Request body for send-failure reconnect and stream retry (always full replay, no delta).
fn fresh_websocket_request_body(full_body: &serde_json::Value) -> serde_json::Value {
    let mut request_body = full_body.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.remove("previous_response_id");
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
            if value
                .get("response")
                .and_then(|resp| resp.get("error"))
                .is_some_and(|error| !error.is_null())
            {
                return Some(Ok(responses_error_chunk(&value, event_type)));
            }
            let resp = &value["response"];
            log_openai_responses_completed(resp);
            Some(Ok(ChunkType::ResponseCompleted {
                end_turn: resp.get("end_turn").and_then(|value| value.as_bool()),
                reasoning_items: reasoning_items_from_response_output(resp),
                doom_loop_triggers: doom_loop_triggers_from(resp),
                usage: resp.get("usage").and_then(openai_responses_usage),
            }))
        }
        // Grok Build / cli-chat-proxy: `response.doom_loop_check` with
        // `doom_loop_check.triggers` like `tail_repetition:8@thinking`.
        // Also present on `response.completed` (`doom_loop_check` field).
        // `.devrefs/references/xai-org/grok-build/crates/codegen/xai-grok-sampling-types/src/doom_loop.rs`
        "response.doom_loop_check" => {
            let triggers = doom_loop_triggers_from(&value).join(",");
            Some(Ok(ChunkType::Metadata(format!(
                "doom_loop_check triggers={triggers}"
            ))))
        }
        "response.incomplete" => Some(Ok(responses_incomplete_chunk(&value))),
        "response.failed" | "error" => Some(Ok(responses_error_chunk(&value, event_type))),
        _ => {
            if let Some(reasoning_item) = responses_reasoning_item_chunk(&value) {
                Some(Ok(ChunkType::ReasoningItem(reasoning_item)))
            } else if let Some(message_phase) = responses_assistant_message_phase_chunk(&value) {
                Some(Ok(message_phase))
            } else if let Some(payload) = responses_hosted_search_chunk(&value) {
                // Provider-executed hosted search — UI only, never client execute.
                Some(Ok(ChunkType::ProviderToolCall(payload)))
            } else if let Some(tool_call) = responses_function_call_chunk(&value) {
                // Only known client function_call shapes.
                Some(Ok(ChunkType::ToolCall(tool_call)))
            } else {
                None
            }
        }
    }
}

/// Log Responses API usage for prompt-cache visibility.
/// Looks for `input_tokens_details.cached_tokens` (OpenAI/xAI shape).
fn openai_responses_usage(usage: &serde_json::Value) -> Option<crate::chunk::TokenUsage> {
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let output = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(|v| v.as_u64());
    let cached = usage
        .pointer("/input_tokens_details/cached_tokens")
        .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
        .and_then(|v| v.as_u64())
        .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    if input.is_none() && output.is_none() && cached == 0 {
        return None;
    }

    let input_v = input.unwrap_or(0);
    let hit_pct = if input_v > 0 {
        (cached as f64 * 100.0) / input_v as f64
    } else {
        0.0
    };

    crate::log::log(&format!(
        "[prompt-cache] openai-responses input={} output={} cached_tokens={} hit_pct={:.1}",
        input.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        output.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
        cached,
        hit_pct
    ));

    Some(crate::chunk::TokenUsage {
        input: input_v.saturating_sub(cached),
        output: output.unwrap_or(0),
        cache_read: cached,
        cache_write: 0,
    })
}

/// Attribute a `response.completed` payload: status, incomplete reason, and
/// output item types. Needed when a turn finishes with no error after
/// reasoning-only / empty assistant output.
fn log_openai_responses_completed(response: &serde_json::Value) {
    let status = response
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let end_turn = response.get("end_turn").and_then(|value| value.as_bool());
    let incomplete_reason = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let (output_count, output_types) = summarize_response_output_types(response);

    let doom_loop_triggers = doom_loop_triggers_from(response);
    let doom_loop = if doom_loop_triggers.is_empty() {
        "none".to_string()
    } else {
        doom_loop_triggers.join(",")
    };

    crate::log::log(&format!(
        "openai-responses completed status={status} end_turn={end_turn:?} incomplete_reason={incomplete_reason} output_count={output_count} output_types=[{output_types}] doom_loop_check={doom_loop}"
    ));
}

fn doom_loop_triggers_from(value: &serde_json::Value) -> Vec<String> {
    value
        .pointer("/doom_loop_check/triggers")
        .or_else(|| value.pointer("/response/doom_loop_check/triggers"))
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(str::to_string)
        .collect()
}

fn summarize_response_output_types(response: &serde_json::Value) -> (usize, String) {
    let Some(output) = response.get("output").and_then(|value| value.as_array()) else {
        return (0, String::new());
    };

    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for item in output {
        let item_type = item
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        *counts.entry(item_type).or_default() += 1;
    }

    let output_types = counts
        .into_iter()
        .map(|(item_type, count)| format!("{item_type}={count}"))
        .collect::<Vec<_>>()
        .join(",");
    (output.len(), output_types)
}

fn responses_provider_error_message(value: &serde_json::Value, fallback: &str) -> String {
    let code = response_error_field(value, "code");
    let message = response_error_field(value, "message");

    match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (None, Some(message)) => message.to_string(),
        (Some(code), None) => code.to_string(),
        (None, None) => fallback.to_string(),
    }
}

fn responses_error_chunk(value: &serde_json::Value, event_type: &str) -> ChunkType {
    let fallback = match event_type {
        "response.failed" => "Response failed",
        "response.completed" => "Response completed with error",
        _ => "OpenAI Responses stream error",
    };
    let message = responses_provider_error_message(value, fallback);
    let retry_error = RetryError::from_message(message.clone());

    if !responses_error_is_permanent(value)
        && (responses_error_is_retryable(value)
            || crate::retry::retryable(&retry_error)
            || matches!(
                event_type,
                "response.failed" | "response.completed" | "error"
            ))
    {
        ChunkType::RetryableFailure(retry_error)
    } else {
        ChunkType::Failed(message)
    }
}

fn responses_error_is_permanent(value: &serde_json::Value) -> bool {
    if response_error_status(value)
        .is_some_and(|status| matches!(status, 400 | 401 | 403 | 404 | 409 | 413 | 422))
    {
        return true;
    }

    let code = response_error_field(value, "code")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let error_type = response_error_field(value, "type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    [code.as_str(), error_type.as_str()].iter().any(|value| {
        matches!(
            *value,
            "authentication_error"
                | "authorization_error"
                | "bio_policy"
                | "content_filter"
                | "context_length_exceeded"
                | "cyber_policy"
                | "forbidden"
                | "insufficient_quota"
                | "invalid_api_key"
                | "invalid_prompt"
                | "invalid_request_error"
                | "invalid_token"
                | "usage_not_included"
        )
    })
}

fn responses_error_is_retryable(value: &serde_json::Value) -> bool {
    let code = response_error_field(value, "code").unwrap_or_default();
    let error_type = response_error_field(value, "type").unwrap_or_default();
    let status = response_error_status(value);

    if status.is_some_and(|status| status == 429 || status >= 500) {
        return true;
    }

    [code, error_type].iter().any(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "server_error"
                | "internal_server_error"
                | "rate_limit_exceeded"
                | "too_many_requests"
                | "temporarily_unavailable"
                | "websocket_connection_limit_reached"
        )
    })
}

fn response_error_status(value: &serde_json::Value) -> Option<u64> {
    value
        .get("status")
        .or_else(|| value.get("status_code"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            value
                .get("error")
                .and_then(|error| error.get("status").or_else(|| error.get("status_code")))
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("status").or_else(|| error.get("status_code")))
                .and_then(serde_json::Value::as_u64)
        })
}

fn responses_incomplete_message(value: &serde_json::Value) -> String {
    let reason = value
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    format!("Incomplete response returned, reason: {reason}")
}

fn response_error_field<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(|field_value| field_value.as_str())
        .or_else(|| {
            value
                .get("error")
                .and_then(|error| error.get(field))
                .and_then(|field_value| field_value.as_str())
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get(field))
                .and_then(|field_value| field_value.as_str())
        })
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

fn responses_reasoning_item_chunk(value: &serde_json::Value) -> Option<ReasoningReplayItem> {
    let event_type = value.get("type").and_then(|v| v.as_str())?;
    if !matches!(
        event_type,
        "response.output_item.added" | "response.output_item.done"
    ) {
        return None;
    }
    reasoning_replay_from_output_item(value.get("item")?)
}

fn reasoning_items_from_response_output(response: &serde_json::Value) -> Vec<ReasoningReplayItem> {
    response
        .get("output")
        .and_then(|output| output.as_array())
        .into_iter()
        .flatten()
        .filter_map(reasoning_replay_from_output_item)
        .collect()
}

fn reasoning_replay_from_output_item(item: &serde_json::Value) -> Option<ReasoningReplayItem> {
    if item.get("type").and_then(|v| v.as_str()) != Some("reasoning") {
        return None;
    }
    let id = item
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let encrypted_content = item
        .get("encrypted_content")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(str::to_string);
    let summary = reasoning_summary_text(item);
    let item = ReasoningReplayItem {
        id,
        summary,
        encrypted_content,
    };
    (!item.is_empty()).then_some(item)
}

fn reasoning_summary_text(item: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(summary) = item.get("summary").and_then(|value| value.as_array()) {
        for part in summary {
            if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    if let Some(content) = item.get("content").and_then(|value| value.as_array()) {
        for part in content {
            if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

fn responses_reasoning_input_item(
    reasoning: &crate::message::ReasoningMessage,
) -> Option<serde_json::Value> {
    if reasoning.is_empty() {
        return None;
    }
    let mut item = serde_json::json!({
        "type": "reasoning",
        "summary": reasoning_summary_parts(&reasoning.summary),
    });
    if let Some(id) = reasoning
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        item["id"] = serde_json::Value::String(id.to_string());
    }
    if let Some(encrypted) = reasoning
        .encrypted_content
        .as_deref()
        .map(str::trim)
        .filter(|content| !content.is_empty())
    {
        item["encrypted_content"] = serde_json::Value::String(encrypted.to_string());
    }
    Some(item)
}

fn reasoning_summary_parts(summary: &str) -> serde_json::Value {
    if summary.is_empty() {
        return serde_json::json!([]);
    }
    serde_json::json!([{
        "type": "summary_text",
        "text": summary,
    }])
}

/// True when an SSE event type is a provider-hosted search lifecycle event.
fn is_hosted_search_event(event_type: &str) -> bool {
    let lower = event_type.to_ascii_lowercase();
    lower.contains("web_search")
        || lower.contains("x_search")
        || lower.contains("custom_tool")
        || lower.contains("file_search")
}

/// Client-executed function tool events (not provider-hosted search).
///
/// Hosted search SSE names also contain `"tool_call"` and must not enter the
/// client tool accumulator:
/// - `response.web_search_call.*`
/// - `response.custom_tool_call_*` (xAI `x_search` streams as custom_tool_call)
/// - `response.file_search_call.*`
fn is_client_tool_call_event(event_type: &str) -> bool {
    if !event_type.contains("tool_call") {
        return false;
    }
    let lower = event_type.to_ascii_lowercase();
    // Hosted search + other provider-executed tools stay out of the client loop.
    !(is_hosted_search_event(event_type) || lower.contains("code_interpreter"))
}

fn hosted_search_item_name(item_type: &str) -> Option<&'static str> {
    let lower = item_type.to_ascii_lowercase();
    // xAI streams x_search as `custom_tool_call`.
    if lower.contains("x_search") || lower.contains("custom_tool") {
        Some("x_search")
    } else if lower.contains("web_search") {
        Some("web_search")
    } else if lower.contains("file_search") {
        Some("file_search")
    } else {
        None
    }
}

fn hosted_search_status_from_event(event_type: &str) -> &'static str {
    let lower = event_type.to_ascii_lowercase();
    if lower.contains("failed") || lower.contains("error") {
        "failed"
    } else if lower.contains("completed") || lower.ends_with(".done") {
        "completed"
    } else {
        "running"
    }
}

/// Collapse xAI dual SSE id namespaces onto one tool-call id.
///
/// `x_search` emits both:
/// - `response.output_item.*` with `item.id` like `xs_call-<uuid>-N`
/// - `response.custom_tool_call_input.*` with `item_id` like `ctc_<uuid>_call-<uuid>-N`
///
/// Those share the `call-<uuid>-N` suffix; without normalizing, the UI paints
/// two cards for one provider-executed search.
fn normalize_hosted_search_tool_id(id: &str) -> String {
    if let Some(idx) = id.find("call-") {
        return id[idx..].to_string();
    }
    id.to_string()
}

/// Build a display-only ProviderToolCall payload from hosted-search SSE.
fn responses_hosted_search_chunk(value: &serde_json::Value) -> Option<String> {
    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if !is_hosted_search_event(event_type)
        && !matches!(
            event_type,
            "response.output_item.added" | "response.output_item.done"
        )
    {
        return None;
    }

    // Prefer nested item (output_item.added/done); else top-level fields.
    let item = value.get("item").unwrap_or(value);
    let item_type = item.get("type").and_then(|v| v.as_str()).or_else(|| {
        // custom_tool_call_input.* may not nest under item
        if is_hosted_search_event(event_type) {
            Some(event_type)
        } else {
            None
        }
    })?;

    let name = if let Some(n) = item.get("name").and_then(|v| v.as_str()) {
        if n.eq_ignore_ascii_case("x_search") || n.eq_ignore_ascii_case("web_search") {
            n.to_string()
        } else if is_hosted_search_event(item_type) || is_hosted_search_event(event_type) {
            hosted_search_item_name(item_type)
                .or_else(|| hosted_search_item_name(event_type))
                .unwrap_or("web_search")
                .to_string()
        } else {
            return None;
        }
    } else {
        hosted_search_item_name(item_type)
            .or_else(|| hosted_search_item_name(event_type))?
            .to_string()
    };

    // Must look like hosted search — don't mis-classify client function_call items.
    if !is_hosted_search_event(item_type)
        && !is_hosted_search_event(event_type)
        && !matches!(name.as_str(), "x_search" | "web_search" | "file_search")
    {
        return None;
    }
    if item_type == "function_call" {
        return None;
    }

    // Prefer call_id (shared across event families) over item_id / item.id
    // (`ctc_*` vs `xs_*` namespaces), then strip prefixes to the shared suffix.
    let id = item
        .get("call_id")
        .or_else(|| value.get("call_id"))
        .or_else(|| item.get("id"))
        .or_else(|| value.get("item_id"))
        .or_else(|| value.get("id"))
        .and_then(|v| v.as_str())
        .map(normalize_hosted_search_tool_id)
        .unwrap_or_else(|| "hosted_search".to_string());

    let status = if event_type == "response.output_item.done" {
        "completed"
    } else if event_type == "response.output_item.added" {
        "running"
    } else {
        hosted_search_status_from_event(event_type)
    };

    let mut payload = serde_json::Map::new();
    payload.insert("id".into(), serde_json::Value::String(id));
    payload.insert("name".into(), serde_json::Value::String(name));
    payload.insert("status".into(), serde_json::Value::String(status.into()));
    payload.insert("provider_executed".into(), serde_json::Value::Bool(true));

    // Arguments / query from various shapes
    let args = item
        .get("arguments")
        .cloned()
        .or_else(|| item.get("input").cloned())
        .or_else(|| item.get("action").cloned())
        .or_else(|| value.get("input").cloned())
        .or_else(|| value.get("delta").cloned());
    if let Some(args) = args {
        let args_val = match args {
            serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(&s)
                .unwrap_or(serde_json::Value::String(s)),
            other => other,
        };
        payload.insert("arguments".into(), args_val);
    }

    if let Some(output) = item
        .get("output")
        .cloned()
        .or_else(|| item.get("result").cloned())
    {
        payload.insert("output".into(), output);
    }

    serde_json::to_string(&serde_json::Value::Object(payload)).ok()
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

fn build_openai_messages(
    messages: &[Message],
    strip_system: bool,
    responses_lite: bool,
) -> Vec<serde_json::Value> {
    let items = messages
        .iter()
        .filter_map(|msg| {
            if strip_system {
                if let Message::System(_) = msg {
                    return None;
                }
            }
            match msg {
                Message::System(s) => Some(if responses_lite {
                    responses_lite_message("developer", "input_text", s.content.clone())
                } else {
                    serde_json::json!({
                        "role": "system",
                        "content": s.content,
                    })
                }),
                Message::User(u) => {
                    let content = openai_responses_user_content(u);
                    Some(if responses_lite {
                        responses_lite_message_with_content("user", "input_text", content)
                    } else {
                        serde_json::json!({
                            "role": "user",
                            "content": content,
                        })
                    })
                }
                Message::Assistant(a) => Some(if responses_lite {
                    responses_lite_message("assistant", "output_text", a.content.clone())
                } else {
                    serde_json::json!({
                        "role": "assistant",
                        "content": a.content,
                    })
                }),
                Message::Reasoning(r) => responses_reasoning_input_item(r),
                Message::ToolCall(t) => {
                    let mut item = serde_json::json!({
                        "type": "function_call",
                        "call_id": t.call_id,
                        "name": t.name,
                        "arguments": t.arguments,
                    });
                    if let Some(item_id) = &t.item_id {
                        if is_prefixed_response_item_id(item_id) {
                            item["id"] = serde_json::Value::String(item_id.clone());
                        }
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
        .collect();
    dedupe_responses_call_ids(items)
}

/// Rewrite duplicate `function_call` / `function_call_output` call ids in the
/// Responses `input` items.
///
/// Some OpenAI-compatible relays mint per-request call ids (`call_1`,
/// `call_2`, ...) that restart on every request. The Responses API requires
/// every call_id to be unique across the conversation input, so a history that
/// reuses an id fails with "Duplicate function_call_output". Remap each
/// repeated call — and its matching output — to a unique id; ids are opaque.
fn dedupe_responses_call_ids(mut items: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    // Occurrence k of a call_id pairs with occurrence k of its output, so
    // track counts per call_id instead of assuming strict call/output order.
    let mut call_occurrences: HashMap<String, usize> = HashMap::new();
    let mut output_occurrences: HashMap<String, usize> = HashMap::new();

    for item in items.iter_mut() {
        match item.get("type").and_then(|value| value.as_str()) {
            Some("function_call") => {
                let Some(call_id) = item
                    .get("call_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let occurrence = call_occurrences.entry(call_id.clone()).or_insert(0);
                *occurrence += 1;
                if *occurrence == 1 {
                    continue;
                }
                let new_id = format!("{call_id}_dup{occurrence}");
                item["call_id"] = serde_json::Value::String(new_id);
                // The item id belongs to the original response; drop it so the
                // rewritten pair cannot collide on item ids either.
                if let Some(obj) = item.as_object_mut() {
                    obj.remove("id");
                }
            }
            Some("function_call_output") => {
                let Some(call_id) = item
                    .get("call_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let occurrence = output_occurrences.entry(call_id.clone()).or_insert(0);
                *occurrence += 1;
                if *occurrence > 1 {
                    item["call_id"] =
                        serde_json::Value::String(format!("{call_id}_dup{occurrence}"));
                }
            }
            _ => {}
        }
    }

    items
}

fn responses_lite_message(role: &str, text_type: &str, text: String) -> serde_json::Value {
    responses_lite_message_with_content(role, text_type, serde_json::Value::String(text))
}

fn responses_lite_message_with_content(
    role: &str,
    text_type: &str,
    content: serde_json::Value,
) -> serde_json::Value {
    let content = match content {
        serde_json::Value::String(text) => vec![serde_json::json!({
            "type": text_type,
            "text": text,
        })],
        serde_json::Value::Array(parts) => parts,
        _ => Vec::new(),
    };
    serde_json::json!({
        "type": "message",
        "role": role,
        "content": content,
    })
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
            // xAI Build / Responses requires `detail`; omitting it can yield
            // successful responses that ignore the image (hallucinated vision).
            "detail": "auto",
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
            "detail": "auto",
        })
    }));
    serde_json::Value::Array(parts)
}

#[cfg(test)]
mod tests {
    use super::{
        add_responses_lite_header, build_openai_messages, build_websocket_request_body,
        fresh_websocket_request_body, is_client_tool_call_event, openai_chunk_is_terminal,
        request_snapshot_from_body, response_sse_data_to_chunk, responses_function_call_chunk,
        summarize_response_output_types, websocket_connection_is_idle,
        websocket_continuation_mode_after_idle_policy, websocket_continuation_mode_from_state,
        OpenAI, OpenAIResponseSnapshot, OpenAIWebsocketState, WebsocketContinuationMode,
        WebsocketStreamProgress, OPENAI_CODEX_WINDOW_ID_HEADER, OPENAI_RESPONSES_LITE_HEADER,
        OPENAI_RESPONSES_LITE_WS_METADATA_KEY, OPENAI_WEBSOCKET_FAILURES_BEFORE_FALLBACK,
        OPENAI_WEBSOCKET_IDLE_MAX,
    };
    use crate::chunk::{ChunkType, MessagePhase};
    use crate::message::Message;
    use crate::tool::{Tool, ToolExecute};
    use schemars::Schema;
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
    fn default_responses_path_does_not_duplicate_v1_in_base_url() {
        // Models.dev routes some gateway models (e.g. opencode-go /
        // muse-spark-1.3-contributor) through `@ai-sdk/openai` while their
        // base URL already ends in a version segment. The default Responses
        // path must not append another `/v1` (which produced
        // `/v1/v1/responses` 404s).
        let provider = OpenAI::builder()
            .base_url("https://opencode.ai/zen/go/v1")
            .model_name("muse-spark-1.3-contributor")
            .build()
            .unwrap();
        assert_eq!(provider.responses_path, "/responses");

        let provider = OpenAI::builder()
            .base_url("https://api.openai.com")
            .model_name("gpt-test")
            .build()
            .unwrap();
        assert_eq!(provider.responses_path, "/v1/responses");
    }

    #[test]
    fn default_responses_path_handles_non_v1_version_segments() {
        let provider = OpenAI::builder()
            .base_url("https://gateway.example.com/v2")
            .model_name("gpt-test")
            .build()
            .unwrap();
        assert_eq!(provider.responses_path, "/responses");

        let provider = OpenAI::builder()
            .base_url("https://gateway.example.com/v2/")
            .model_name("gpt-test")
            .build()
            .unwrap();
        assert_eq!(provider.responses_path, "/responses");
    }

    #[test]
    fn openai_messages_strip_unprefixed_response_item_ids() {
        let messages = vec![
            Message::tool_call_with_item_id("index:0", "call_1", "read", "{}"),
            Message::tool_call_with_item_id("fc_valid", "call_2", "read", "{}"),
            Message::tool_call_with_item_id("future_valid", "call_3", "read", "{}"),
        ];

        let input = build_openai_messages(&messages, false, false);

        assert!(input[0].get("id").is_none());
        assert_eq!(input[1]["id"], "fc_valid");
        assert_eq!(input[2]["id"], "future_valid");
    }

    #[test]
    fn websocket_falls_back_after_consecutive_failures() {
        let mut state = OpenAIWebsocketState::default();

        for _ in 1..OPENAI_WEBSOCKET_FAILURES_BEFORE_FALLBACK {
            assert!(!state.record_failure());
            assert!(!state.disabled);
        }

        assert!(state.record_failure());
        assert!(state.disabled);
    }

    #[test]
    fn websocket_success_resets_failure_budget() {
        let mut state = OpenAIWebsocketState::default();

        assert!(!state.record_failure());
        state.record_success();

        assert_eq!(state.consecutive_failures, 0);
        assert!(!state.disabled);
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
                end_turn: Some(false),
                ..
            })
        ));
    }

    #[test]
    fn response_completed_captures_usage_minus_cached_tokens() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.completed","response":{"id":"resp_123","usage":{"input_tokens":120,"output_tokens":30,"input_tokens_details":{"cached_tokens":40}}}}"#,
        )
        .expect("expected terminal chunk");

        match chunk {
            Ok(ChunkType::ResponseCompleted { usage, .. }) => {
                assert_eq!(
                    usage,
                    Some(crate::chunk::TokenUsage {
                        input: 80,
                        output: 30,
                        cache_read: 40,
                        cache_write: 0,
                    })
                );
            }
            other => panic!("expected ResponseCompleted, got {other:?}"),
        }
    }

    #[test]
    fn response_incomplete_safety_reasons_emit_refusal() {
        for reason in ["refusal", "content_filter", "safety"] {
            let chunk = response_sse_data_to_chunk(&format!(
                r#"{{"type":"response.incomplete","response":{{"incomplete_details":{{"reason":"{reason}"}}}}}}"#
            ))
            .expect("expected incomplete chunk");
            assert!(matches!(
                chunk,
                Ok(ChunkType::End {
                    reason: Some(crate::chunk::FinishReason::Refusal)
                })
            ));
        }
    }

    #[test]
    fn response_incomplete_max_output_tokens_emits_terminal_reason() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .expect("expected incomplete chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::End {
                reason: Some(crate::chunk::FinishReason::Length)
            })
        ));
    }

    #[test]
    fn retryable_failure_is_terminal_for_sse_eof_tracking() {
        let chunk = Ok(ChunkType::RetryableFailure(
            crate::retry::RetryError::from_message("stream error"),
        ));

        assert!(openai_chunk_is_terminal(&chunk));
    }

    #[test]
    fn nested_transient_error_event_is_retryable() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"error","error":{"type":"server_error","code":"server_error","message":"The server encountered an error"}}"#,
        )
        .expect("expected failure chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::RetryableFailure(error))
                if error.message == "server_error: The server encountered an error"
        ));
    }

    #[test]
    fn nested_permanent_error_event_is_not_retryable() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"error","error":{"type":"invalid_request_error","code":"invalid_prompt","message":"Invalid prompt"}}"#,
        )
        .expect("expected failure chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::Failed(message)) if message == "invalid_prompt: Invalid prompt"
        ));
    }

    #[test]
    fn response_incomplete_is_retryable_with_reason() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"server_error"}}}"#,
        )
        .expect("expected incomplete chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::RetryableFailure(error))
                if error.message == "Incomplete response returned, reason: server_error"
        ));
    }

    #[test]
    fn response_failed_includes_nested_provider_error() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.failed","response":{"error":{"code":"invalid_token","message":"OAuth token expired"}}}"#,
        )
        .expect("expected failure chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::Failed(message))
                if message == "invalid_token: OAuth token expired"
        ));
    }

    #[test]
    fn response_error_event_includes_top_level_provider_error() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"error","code":"forbidden","message":"Codex access denied"}"#,
        )
        .expect("expected failure chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::Failed(message))
                if message == "forbidden: Codex access denied"
        ));
    }

    #[test]
    fn response_completed_rate_limit_error_is_retryable() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.completed","response":{"error":{"code":"rate_limit_exceeded"}}}"#,
        )
        .expect("expected failure chunk");

        assert!(matches!(
            chunk,
            Ok(ChunkType::RetryableFailure(error)) if error.message == "rate_limit_exceeded"
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
    fn surfaces_hosted_search_sse_as_provider_tool_call() {
        // xAI x_search streams as custom_tool_call_*; must not enter client tool loop,
        // but should surface as ProviderToolCall for host observability.
        for event_type in [
            "response.custom_tool_call_input.delta",
            "response.custom_tool_call_input.done",
            "response.web_search_call.in_progress",
            "response.web_search_call.completed",
            "response.web_search_call.searching",
        ] {
            let chunk = response_sse_data_to_chunk(
                &serde_json::json!({
                    "type": event_type,
                    "item_id": "ws_1",
                    "delta": "{\"query\":\"carlo_taleon\"}"
                })
                .to_string(),
            );
            match chunk {
                Some(Ok(ChunkType::ProviderToolCall(payload))) => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(&payload).expect("valid provider tool payload");
                    assert_eq!(parsed["id"], "ws_1");
                    assert!(parsed["provider_executed"].as_bool().unwrap_or(false));
                    assert!(
                        matches!(
                            parsed["name"].as_str(),
                            Some("x_search") | Some("web_search")
                        ),
                        "unexpected name for {event_type}: {}",
                        parsed["name"]
                    );
                }
                other => panic!("expected ProviderToolCall for {event_type}, got {other:?}"),
            }
        }

        let chunk = response_sse_data_to_chunk(
            &serde_json::json!({
                "type": "response.output_item.done",
                "item": {
                    "id": "ws_done",
                    "type": "web_search_call",
                    "status": "completed",
                    "action": {"query": "rust async"}
                }
            })
            .to_string(),
        );
        match chunk {
            Some(Ok(ChunkType::ProviderToolCall(payload))) => {
                let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
                assert_eq!(parsed["name"], "web_search");
                assert_eq!(parsed["status"], "completed");
                assert_eq!(parsed["id"], "ws_done");
            }
            other => panic!("expected completed ProviderToolCall, got {other:?}"),
        }

        assert!(!is_client_tool_call_event(
            "response.custom_tool_call_input.delta"
        ));
        assert!(!is_client_tool_call_event(
            "response.web_search_call.in_progress"
        ));
        // function_call_arguments.delta is handled by responses_function_call_chunk,
        // not the tool_call catch-all (it doesn't contain "tool_call").
        assert!(!is_client_tool_call_event(
            "response.function_call_arguments.delta"
        ));
        assert!(is_client_tool_call_event("chat.completion.tool_call.delta"));
    }

    #[test]
    fn collapses_x_search_dual_sse_id_namespaces() {
        // xAI emits both output_item (xs_*) and custom_tool_call_input (ctc_*).
        // They must share one tool id so the UI paints a single card.
        let shared = "call-aadee343-63eb-9a1b-8000-0b77246e7421-21";
        let output_item = response_sse_data_to_chunk(
            &serde_json::json!({
                "type": "response.output_item.added",
                "item": {
                    "id": format!("xs_{shared}"),
                    "type": "custom_tool_call",
                    "name": "x_search",
                    "status": "in_progress",
                    "arguments": {"post_id": "2093050916953903451"}
                }
            })
            .to_string(),
        );
        let custom_input = response_sse_data_to_chunk(
            &serde_json::json!({
                "type": "response.custom_tool_call_input.done",
                "item_id": format!("ctc_f47ac10b-58cc-4372-a567-0e02b2c3d479_{shared}"),
                "delta": "{\"post_id\":\"2093050916953903451\"}"
            })
            .to_string(),
        );

        let id_from = |chunk: Option<Result<ChunkType, _>>| -> String {
            match chunk {
                Some(Ok(ChunkType::ProviderToolCall(payload))) => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(&payload).expect("valid payload");
                    parsed["id"].as_str().unwrap().to_string()
                }
                other => panic!("expected ProviderToolCall, got {other:?}"),
            }
        };

        let a = id_from(output_item);
        let b = id_from(custom_input);
        assert_eq!(a, shared);
        assert_eq!(b, shared);
        assert_eq!(
            super::normalize_hosted_search_tool_id("ws_1"),
            "ws_1",
            "ids without call- suffix stay unchanged"
        );
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
    fn dedupes_repeated_call_ids_across_turns_for_responses_input() {
        // Relays that mint per-request ids (call_1, ...) restart numbering on
        // every request; the stored history then repeats ids across turns.
        let input = build_openai_messages(
            &[
                Message::user("list files"),
                Message::tool_call("call_1", "glob", "{}"),
                Message::tool_output("call_1", "glob", "src/main.rs", false),
                Message::tool_call("call_1", "read", "{}"),
                Message::tool_output("call_1", "read", "fn main() {}", false),
                Message::tool_call("call_1", "grep", "{}"),
                Message::tool_output("call_1", "grep", "no matches", false),
            ],
            false,
            false,
        );

        let call_ids: Vec<&str> = input
            .iter()
            .filter(|item| item.get("call_id").is_some())
            .map(|item| item["call_id"].as_str().expect("call_id"))
            .collect();
        assert_eq!(
            call_ids,
            [
                "call_1",
                "call_1",
                "call_1_dup2",
                "call_1_dup2",
                "call_1_dup3",
                "call_1_dup3"
            ]
        );
        // Rewritten calls must not keep a stale response item id.
        assert!(input[3].get("id").is_none());
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
            false,
        );

        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_image");
        let output = input[0]["output"].as_array().expect("content items");
        assert_eq!(output[0]["type"], "input_text");
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(output[1]["image_url"], "data:image/png;base64,AAA");
        assert_eq!(output[1]["detail"], "auto");
    }

    #[test]
    fn serializes_user_image_input_with_detail_for_responses() {
        let input = build_openai_messages(
            &[Message::user_with_images(
                "what is this?",
                vec![crate::message::ImageContent {
                    data_url: "data:image/png;base64,AAA".to_string(),
                    media_type: "image/png".to_string(),
                }],
            )],
            false,
            false,
        );

        assert_eq!(input[0]["role"], "user");
        let content = input[0]["content"].as_array().expect("content items");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "data:image/png;base64,AAA");
        assert_eq!(content[1]["detail"], "auto");
    }

    #[test]
    fn responses_body_omits_tool_options_without_tools() {
        let provider = OpenAI::builder()
            .base_url("https://api.openai.com")
            .api_key("test-key")
            .model_name("gpt-test")
            .build()
            .unwrap();

        let body = provider.build_responses_body(
            vec![serde_json::json!({"role": "user", "content": "summarize"})],
            &[],
        );

        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn responses_body_includes_prompt_cache_key_and_store_false() {
        let provider = OpenAI::builder()
            .base_url("https://api.x.ai")
            .api_key("test-key")
            .model_name("grok-composer-2.5-fast")
            .prompt_cache_key("session-abc")
            .store_override(false)
            .build()
            .unwrap();

        let body = provider.build_responses_body(
            vec![serde_json::json!({"role": "user", "content": "hi"})],
            &[],
        );

        assert_eq!(body["prompt_cache_key"], "session-abc");
        assert_eq!(body["store"], false);
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn responses_body_always_includes_encrypted_reasoning() {
        let provider = OpenAI::builder()
            .base_url("https://api.openai.com")
            .api_key("test-key")
            .model_name("gpt-test")
            .build()
            .unwrap();

        let body = provider.build_responses_body(
            vec![serde_json::json!({"role": "user", "content": "hi"})],
            &[],
        );

        assert_eq!(body["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn responses_input_replays_reasoning_item_with_encrypted_content() {
        let input = build_openai_messages(
            &[
                Message::user("inspect"),
                Message::reasoning(
                    Some("rs_1".to_string()),
                    "inspect the file",
                    Some("enc_abc".to_string()),
                ),
                Message::tool_call("call_1", "read", r#"{"file_path":"src/lib.rs"}"#),
            ],
            false,
            false,
        );

        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_1");
        assert_eq!(input[1]["encrypted_content"], "enc_abc");
        assert_eq!(input[1]["summary"][0]["text"], "inspect the file");
        assert_eq!(input[2]["type"], "function_call");
    }

    #[test]
    fn response_completed_captures_encrypted_reasoning_items() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.completed","response":{"output":[{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"plan"}],"encrypted_content":"enc_abc"},{"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}]}}"#,
        )
        .expect("expected terminal chunk");

        match chunk {
            Ok(ChunkType::ResponseCompleted {
                reasoning_items, ..
            }) => {
                assert_eq!(reasoning_items.len(), 1);
                assert_eq!(reasoning_items[0].id.as_deref(), Some("rs_1"));
                assert_eq!(
                    reasoning_items[0].encrypted_content.as_deref(),
                    Some("enc_abc")
                );
                assert_eq!(reasoning_items[0].summary, "plan");
            }
            other => panic!("expected ResponseCompleted, got {other:?}"),
        }
    }

    #[test]
    fn response_completed_captures_terminal_doom_loop_triggers() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.completed","response":{"output":[{"type":"reasoning","id":"rs_1"}],"doom_loop_check":{"triggers":["tail_repetition:8@thinking"]}}}"#,
        )
        .expect("expected terminal chunk");

        match chunk {
            Ok(ChunkType::ResponseCompleted {
                doom_loop_triggers, ..
            }) => {
                assert_eq!(
                    doom_loop_triggers,
                    vec!["tail_repetition:8@thinking".to_string()]
                );
            }
            other => panic!("expected ResponseCompleted, got {other:?}"),
        }
    }

    #[test]
    fn summarize_response_output_types_counts_reasoning_and_function_calls() {
        let response = serde_json::json!({
            "output": [
                {"type": "reasoning", "id": "rs_1"},
                {"type": "function_call", "call_id": "call_1", "name": "read"},
                {"type": "function_call", "call_id": "call_2", "name": "read"},
            ]
        });
        let (count, types) = summarize_response_output_types(&response);
        assert_eq!(count, 3);
        assert_eq!(types, "function_call=2,reasoning=1");
    }

    #[test]
    fn summarize_response_output_types_empty_when_missing_output() {
        let response = serde_json::json!({"status": "completed"});
        assert_eq!(
            summarize_response_output_types(&response),
            (0, String::new())
        );
    }

    #[test]
    fn doom_loop_check_sse_becomes_metadata() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.doom_loop_check","doom_loop_check":{"triggers":["tail_repetition:8@thinking"]}}"#,
        )
        .expect("expected metadata chunk");
        match chunk {
            Ok(ChunkType::Metadata(message)) => {
                assert!(message.contains("doom_loop_check"));
                assert!(message.contains("tail_repetition:8@thinking"));
            }
            other => panic!("expected Metadata, got {other:?}"),
        }
    }

    #[test]
    fn output_item_done_emits_reasoning_item_chunk() {
        let chunk = response_sse_data_to_chunk(
            r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","encrypted_content":"enc_abc"}}"#,
        )
        .expect("expected reasoning item chunk");

        match chunk {
            Ok(ChunkType::ReasoningItem(item)) => {
                assert_eq!(item.id.as_deref(), Some("rs_1"));
                assert_eq!(item.encrypted_content.as_deref(), Some("enc_abc"));
            }
            other => panic!("expected ReasoningItem, got {other:?}"),
        }
    }

    #[test]
    fn responses_body_includes_tool_options_with_tools() {
        let provider = OpenAI::builder()
            .base_url("https://api.openai.com")
            .api_key("test-key")
            .model_name("gpt-test")
            .build()
            .unwrap();
        let tools = vec![Tool::builder()
            .name("read")
            .description("Read a file")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(|_| async { Ok("ok") }))
            .build()
            .unwrap()];

        let body = provider.build_responses_body(
            vec![serde_json::json!({"role": "user", "content": "read Cargo.toml"})],
            &tools,
        );

        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn responses_lite_uses_input_items_and_lite_reasoning_contract() {
        let provider = OpenAI::builder()
            .base_url("https://chatgpt.com")
            .api_key("oauth-token")
            .model_name("gpt-5.6-sol")
            .responses_lite(true)
            .default_instructions("system guidance")
            .reasoning_effort("high")
            .store_override(false)
            .prompt_cache_key("session-abc")
            .build()
            .unwrap();
        let tools = vec![Tool::builder()
            .name("read")
            .description("Read a file")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(|_| async { Ok("ok") }))
            .build()
            .unwrap()];
        let input = build_openai_messages(&[Message::user("inspect")], true, true);

        let body = provider.build_responses_body(input, &tools);

        assert_eq!(body["instructions"], "");
        assert!(body.get("tools").is_none());
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["text"]["verbosity"], "low");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["context"], "all_turns");
        assert_eq!(body["include"][0], "reasoning.encrypted_content");
        assert_eq!(body["store"], false);
        assert_eq!(
            body["client_metadata"][OPENAI_RESPONSES_LITE_WS_METADATA_KEY],
            "true"
        );
        assert_eq!(body["client_metadata"]["session_id"], "session-abc");
        assert_eq!(body["client_metadata"]["thread_id"], "session-abc");
        assert_eq!(
            body["client_metadata"][OPENAI_CODEX_WINDOW_ID_HEADER],
            "session-abc"
        );

        let items = body["input"].as_array().expect("Responses input items");
        assert_eq!(items[0]["type"], "additional_tools");
        assert_eq!(items[0]["role"], "developer");
        assert_eq!(items[0]["tools"][0]["name"], "read");
        assert_eq!(items[1]["type"], "message");
        assert_eq!(items[1]["role"], "developer");
        assert_eq!(items[1]["content"][0]["type"], "input_text");
        assert_eq!(items[1]["content"][0]["text"], "system guidance");
        assert_eq!(items[2]["type"], "message");
        assert_eq!(items[2]["role"], "user");
        assert_eq!(items[2]["content"][0]["type"], "input_text");
        assert_eq!(items[2]["content"][0]["text"], "inspect");
    }

    #[test]
    fn responses_lite_header_is_model_option_scoped() {
        let mut headers = reqwest::header::HeaderMap::new();

        add_responses_lite_header(&mut headers, false);
        assert!(!headers.contains_key(OPENAI_RESPONSES_LITE_HEADER));

        add_responses_lite_header(&mut headers, true);
        assert_eq!(
            headers
                .get(OPENAI_RESPONSES_LITE_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
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
        let ws_body = build_websocket_request_body(
            &state,
            &next_body,
            WebsocketContinuationMode::SameLiveSocket,
        );

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
        let ws_body = build_websocket_request_body(
            &state,
            &next_body,
            WebsocketContinuationMode::SameLiveSocket,
        );

        assert_eq!(ws_body["previous_response_id"], "resp_1");
        assert_eq!(ws_body["input"], serde_json::json!([]));
    }

    #[test]
    fn websocket_connection_is_idle_detects_expired_last_used() {
        assert!(!websocket_connection_is_idle(
            None,
            OPENAI_WEBSOCKET_IDLE_MAX
        ));
        assert!(websocket_connection_is_idle(
            Some(Instant::now() - Duration::from_secs(120)),
            OPENAI_WEBSOCKET_IDLE_MAX,
        ));
        assert!(!websocket_connection_is_idle(
            Some(Instant::now() - Duration::from_secs(10)),
            OPENAI_WEBSOCKET_IDLE_MAX,
        ));
    }

    #[test]
    fn websocket_idle_policy_drops_live_socket_continuation_eligibility() {
        let expired = Instant::now() - Duration::from_secs(120);
        let recent = Instant::now() - Duration::from_secs(10);

        assert_eq!(
            websocket_continuation_mode_after_idle_policy(
                true,
                Some(expired),
                OPENAI_WEBSOCKET_IDLE_MAX,
            ),
            WebsocketContinuationMode::FreshSocket,
        );
        assert_eq!(
            websocket_continuation_mode_after_idle_policy(
                true,
                Some(recent),
                OPENAI_WEBSOCKET_IDLE_MAX,
            ),
            WebsocketContinuationMode::SameLiveSocket,
        );
        assert_eq!(
            websocket_continuation_mode_after_idle_policy(
                false,
                Some(recent),
                OPENAI_WEBSOCKET_IDLE_MAX,
            ),
            WebsocketContinuationMode::FreshSocket,
        );
    }

    #[test]
    fn websocket_continuation_mode_requires_live_connection() {
        let mut state = OpenAIWebsocketState::default();
        assert_eq!(
            websocket_continuation_mode_from_state(&state),
            WebsocketContinuationMode::FreshSocket
        );

        state.last_response = Some(OpenAIResponseSnapshot {
            response_id: "resp_1".to_string(),
            items_added: vec![],
        });
        assert_eq!(
            websocket_continuation_mode_from_state(&state),
            WebsocketContinuationMode::FreshSocket
        );
    }

    #[tokio::test]
    async fn websocket_request_after_idle_eviction_replays_full_input_not_delta() {
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

        let mut next_input = previous_input;
        next_input.push(function_call);
        next_input.push(function_output);
        let next_body = provider.build_responses_body(next_input, &[]);

        {
            let mut state = provider.websocket_state.lock().await;
            state.last_request = Some(request_snapshot_from_body(&previous_body));
            state.last_response = Some(OpenAIResponseSnapshot {
                response_id: "resp_1".to_string(),
                items_added: vec![serde_json::json!({
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "read",
                    "arguments": "{\"file_path\":\"Cargo.toml\"}"
                })],
            });
        }

        let state = provider.websocket_state.lock().await;
        let live_delta_body = build_websocket_request_body(
            &state,
            &next_body,
            WebsocketContinuationMode::SameLiveSocket,
        );
        assert_eq!(live_delta_body["previous_response_id"], "resp_1");
        assert_ne!(live_delta_body["input"], next_body["input"]);

        let after_idle_mode = websocket_continuation_mode_after_idle_policy(
            true,
            Some(Instant::now() - Duration::from_secs(120)),
            OPENAI_WEBSOCKET_IDLE_MAX,
        );
        assert_eq!(after_idle_mode, WebsocketContinuationMode::FreshSocket);

        let ws_body = build_websocket_request_body(&state, &next_body, after_idle_mode);
        assert!(ws_body.get("previous_response_id").is_none());
        assert_eq!(ws_body["input"], next_body["input"]);
    }

    #[test]
    fn fresh_websocket_request_body_strips_previous_response_id_and_preserves_full_input() {
        let mut body = serde_json::json!({
            "model": "gpt-test",
            "previous_response_id": "resp_synthetic",
            "input": [
                {"role": "user", "content": "read the file"},
                {"type": "function_call_output", "call_id": "call_1", "output": "ok"}
            ]
        });
        let expected_input = body["input"].clone();

        let send_reconnect_body = fresh_websocket_request_body(&body);
        let stream_retry_body = fresh_websocket_request_body(&body);

        assert_eq!(send_reconnect_body, stream_retry_body);
        assert_eq!(send_reconnect_body["type"], "response.create");
        assert!(send_reconnect_body.get("previous_response_id").is_none());
        assert_eq!(send_reconnect_body["input"], expected_input);

        body["previous_response_id"] = serde_json::Value::String("resp_again".to_string());
        let fresh = fresh_websocket_request_body(&body);
        assert!(fresh.get("previous_response_id").is_none());
        assert_eq!(fresh["input"], expected_input);
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
        let ws_body = build_websocket_request_body(
            &state,
            &next_body,
            WebsocketContinuationMode::SameLiveSocket,
        );

        assert!(ws_body.get("previous_response_id").is_none());
        assert_eq!(ws_body["input"], next_body["input"]);
    }
}
