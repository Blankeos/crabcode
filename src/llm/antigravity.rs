use crate::aisdk::chunk::{ChunkType, FinishReason, ReasoningReplayItem, TokenUsage};
use crate::aisdk::error::{Error, Result};
use crate::aisdk::message::{ImageContent, Message};
use crate::aisdk::provider::{Provider, ProviderStream};
use crate::aisdk::retry::RetryError;
use crate::aisdk::tool::{Tool, ToolTransport};
use async_trait::async_trait;
use base64::Engine;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

pub(crate) const REPLAY_PREFIX: &str = "antigravity:gemini-parts:v1:";

pub(crate) fn persist_replay(
    message: &mut crate::session::types::Message,
    item: ReasoningReplayItem,
) {
    if !item
        .encrypted_content
        .as_deref()
        .is_some_and(|value| value.starts_with(REPLAY_PREFIX))
    {
        return;
    }
    // The item arrives after this step's text but before its tools execute.
    let index = message
        .parts
        .iter()
        .rposition(|part| matches!(part.part_type.as_str(), "tool_result" | "provider_replay"))
        .map_or(0, |index| index + 1);
    message.parts.insert(
        index,
        crate::session::types::MessagePart {
            part_type: "provider_replay".into(),
            data: json!({
                "id": item.id,
                "summary": item.summary,
                "encrypted_content": item.encrypted_content,
            }),
        },
    );
}

#[derive(Clone)]
pub struct Antigravity {
    base_url: String,
    model: String,
    access: String,
    project: String,
    stored_credentials: bool,
}

impl std::fmt::Debug for Antigravity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Antigravity")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl Antigravity {
    pub fn new(base_url: String, model: String, access: String, project: String) -> Self {
        Self {
            base_url,
            model,
            access,
            project,
            stored_credentials: false,
        }
    }

    pub fn with_stored_credentials(mut self) -> Self {
        self.stored_credentials = true;
        self
    }

    fn request_body(
        &self,
        messages: &[Message],
        tools: &[Tool],
        headers: &HashMap<String, String>,
    ) -> Result<Value> {
        let (contents, system) = gemini_messages(messages)?;
        let mut declarations = Vec::new();
        for tool in tools {
            if !matches!(tool.transport, ToolTransport::ClientFunction) {
                return Err(Error::InvalidInput(
                    "Antigravity only supports client function tools".into(),
                ));
            }
            let schema = serde_json::to_value(&tool.input_schema)?;
            declarations.push(json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": gemini_schema(&schema, &schema, 0)?,
            }));
        }
        let session = [
            "x-opencode-session",
            "session-id",
            "x-session-id",
            "x-crabcode-session",
            "thread-id",
        ]
        .into_iter()
        .find_map(|name| {
            headers
                .iter()
                .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.trim().is_empty())
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_else(cuid2::create_id);
        let mut request = json!({"sessionId": session, "contents": contents});
        if !system.is_empty() {
            request["systemInstruction"] = json!({"parts": system});
        }
        if !declarations.is_empty() {
            request["tools"] = json!([{"functionDeclarations": declarations}]);
        }
        Ok(json!({
            "project": self.project,
            "model": self.model,
            "userAgent": "antigravity",
            "requestType": "agent",
            "requestId": format!("agent-{}", cuid2::create_id()),
            "request": request,
        }))
    }
}

// The SDK's language-model interface is named Provider. Its opaque reasoning
// replay items survive stream_with_tools_options and persisted Message history.
#[async_trait]
impl Provider for Antigravity {
    fn name(&self) -> &str {
        "antigravity"
    }
    fn model_name(&self) -> &str {
        &self.model
    }

    async fn stream_text(
        &self,
        messages: &[Message],
        tools: &[Tool],
        headers: &HashMap<String, String>,
    ) -> Result<ProviderStream> {
        let mut request = self.clone();
        if self.stored_credentials {
            let credentials = crate::auth::antigravity_oauth::load_request_credentials()
                .await
                .map_err(|_| {
                    Error::Provider("Google Antigravity credentials are unavailable".into())
                })?;
            request.access = credentials.access;
            request.project = credentials.project_id.ok_or_else(|| {
                Error::Provider("Google Antigravity project is unavailable".into())
            })?;
        }
        let body = request.request_body(messages, tools, headers)?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Provider("Could not initialize Antigravity HTTP client".into()))?;
        // Do not forward arbitrary provider headers (especially API keys or an
        // Authorization override) to this OAuth-only endpoint.
        let response = client
            .post(format!(
                "{}/v1internal:streamGenerateContent?alt=sse",
                self.base_url.trim_end_matches('/')
            ))
            .bearer_auth(&request.access)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header(
                reqwest::header::USER_AGENT,
                crate::auth::antigravity_oauth::user_agent(),
            )
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::RetryableProvider(RetryError::new("Antigravity request failed")))?;
        if !response.status().is_success() {
            let status = response.status();
            // Error bodies may echo credentials or request contents. Keep only
            // the status and retry timing, never the body or request URL.
            let error = RetryError::new(format!("Antigravity HTTP error {}", status.as_u16()))
                .with_status(status.as_u16())
                .with_headers(response.headers());
            return if crate::aisdk::retry::retryable(&error) {
                Err(Error::RetryableProvider(error))
            } else {
                Err(Error::Provider(error.message))
            };
        }
        Ok(gemini_event_stream(
            response.bytes_stream().eventsource(),
            Duration::from_secs(90),
            Duration::from_millis(250),
        ))
    }
}

fn gemini_event_stream<S, E>(events: S, idle: Duration, grace: Duration) -> ProviderStream
where
    S: futures::Stream<Item = std::result::Result<eventsource_stream::Event, E>> + Send + 'static,
    E: Send + 'static,
{
    let state = (
        Box::pin(events),
        GeminiStream::default(),
        VecDeque::new(),
        None::<tokio::time::Instant>,
        false,
    );
    Box::pin(futures::stream::unfold(
        state,
        move |(mut events, mut parser, mut pending, mut deadline, mut done)| async move {
            loop {
                if let Some(chunk) = pending.pop_front() {
                    return Some((chunk, (events, parser, pending, deadline, done)));
                }
                if done {
                    return None;
                }
                // The terminal grace is absolute: keepalives must not extend it.
                let until = deadline.unwrap_or_else(|| tokio::time::Instant::now() + idle);
                match tokio::time::timeout_at(until, events.next()).await {
                    Ok(Some(Ok(event))) if event.data.trim() == "[DONE]" => {
                        pending.extend(parser.finish());
                        done = true;
                    }
                    Ok(Some(Ok(event))) => {
                        pending.extend(parser.data(&event.data));
                        if parser.failed {
                            done = true;
                        } else if parser.reason.is_some() && deadline.is_none() {
                            deadline = Some(tokio::time::Instant::now() + grace);
                        }
                    }
                    Ok(None) => {
                        pending.extend(parser.finish());
                        done = true;
                    }
                    Err(_) if deadline.is_some() => {
                        pending.extend(parser.finish());
                        done = true;
                    }
                    result => {
                        parser.failed = true;
                        let message = if result.is_err() {
                            "Antigravity SSE read timed out"
                        } else {
                            "Antigravity SSE transport failed"
                        };
                        pending.push_back(Err(Error::Stream(message.into())));
                        done = true;
                    }
                }
            }
        },
    ))
}

#[derive(Serialize, Deserialize)]
struct Replay {
    parts: Vec<Value>,
    call_ids: Vec<String>,
}

fn push_parts(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut().filter(|last| last["role"] == role) {
        last["parts"]
            .as_array_mut()
            .expect("constructed parts array")
            .extend(parts);
    } else {
        contents.push(json!({"role": role, "parts": parts}));
    }
}

pub(super) fn gemini_messages(messages: &[Message]) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut contents = Vec::new();
    let mut system = Vec::new();
    let mut replay_calls = HashSet::new();
    let mut replay_text = None::<String>;
    for message in messages {
        match message {
            Message::System(message) => system.push(json!({"text": message.content})),
            Message::User(message) => {
                replay_text = None;
                replay_calls.clear();
                if !message.audios.is_empty() {
                    return Err(Error::InvalidInput(
                        "Antigravity audio input is not supported".into(),
                    ));
                }
                let mut parts = Vec::new();
                if !message.content.is_empty() {
                    parts.push(json!({"text": message.content}));
                }
                for image in &message.images {
                    parts.push(image_part(image)?);
                }
                push_parts(&mut contents, "user", parts);
            }
            Message::Reasoning(message) => {
                if let Some(payload) = message
                    .encrypted_content
                    .as_deref()
                    .and_then(|value| value.strip_prefix(REPLAY_PREFIX))
                {
                    let replay: Replay = serde_json::from_str(payload).map_err(|_| {
                        Error::InvalidInput("Invalid Antigravity replay metadata".into())
                    })?;
                    replay_text = Some(
                        replay
                            .parts
                            .iter()
                            .filter(|part| part["thought"] != true)
                            .filter_map(|part| part["text"].as_str())
                            .collect::<String>()
                            .trim()
                            .to_owned(),
                    );
                    replay_calls = replay.call_ids.into_iter().collect();
                    push_parts(&mut contents, "model", replay.parts);
                }
            }
            Message::Assistant(message) => {
                if let Some(remaining) = replay_text.take() {
                    let text = message.content.trim();
                    if let Some(rest) = remaining.trim_start().strip_prefix(text) {
                        if !rest.trim().is_empty() {
                            replay_text = Some(rest.to_owned());
                        }
                        continue;
                    }
                }
                push_parts(
                    &mut contents,
                    "model",
                    vec![json!({"text": message.content})],
                );
            }
            Message::ToolCall(message) => {
                if replay_calls.remove(&message.call_id) {
                    continue;
                }
                let args: Value = serde_json::from_str(&message.arguments).map_err(|_| {
                    Error::InvalidInput("Invalid Antigravity tool arguments in history".into())
                })?;
                if !args.is_object() {
                    return Err(Error::InvalidInput(
                        "Antigravity tool arguments must be an object".into(),
                    ));
                }
                push_parts(
                    &mut contents,
                    "model",
                    vec![
                        json!({"functionCall": {"name": message.name, "args": args, "id": message.call_id}}),
                    ],
                );
            }
            Message::ToolOutput(message) => {
                replay_text = None;
                replay_calls.clear();
                let response = if message.is_error {
                    json!({"error": message.output})
                } else {
                    json!({"output": message.output})
                };
                let mut parts = vec![
                    json!({"functionResponse": {"name": message.name, "id": message.call_id, "response": response}}),
                ];
                for image in &message.images {
                    parts.push(image_part(image)?);
                }
                push_parts(&mut contents, "user", parts);
            }
        }
    }
    Ok((contents, system))
}

fn image_part(image: &ImageContent) -> Result<Value> {
    let data = if let Some((prefix, data)) = image.data_url.split_once(',') {
        if !prefix.starts_with("data:") || !prefix.ends_with(";base64") {
            return Err(Error::InvalidInput(
                "Antigravity images must contain base64 data".into(),
            ));
        }
        data
    } else {
        image.data_url.as_str()
    };
    if !image.media_type.starts_with("image/")
        || base64::engine::general_purpose::STANDARD
            .decode(data)
            .is_err()
        || data.is_empty()
    {
        return Err(Error::InvalidInput(
            "Invalid Antigravity image data or media type".into(),
        ));
    }
    Ok(json!({"inlineData": {"mimeType": image.media_type, "data": data}}))
}

// Internal Gemini endpoints accept the Schema subset, not arbitrary JSON Schema.
// Resolve local references before stripping unsupported validation keywords;
// validation remains the tool executor's responsibility.
fn gemini_schema(schema: &Value, root: &Value, depth: usize) -> Result<Value> {
    if depth > 32 {
        return Err(Error::InvalidInput(
            "Recursive or deeply nested Antigravity tool schema".into(),
        ));
    }
    if schema == &Value::Bool(true) {
        return Ok(json!({"type": "OBJECT"}));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| Error::InvalidInput("Unsupported Antigravity tool schema".into()))?;
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let target = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
            .ok_or_else(|| {
                Error::InvalidInput("Unresolved Antigravity tool schema reference".into())
            })?;
        return gemini_schema(target, root, depth + 1);
    }
    let mut out = serde_json::Map::new();
    for (key, value) in object {
        match key.as_str() {
            "type" => {
                let types: Vec<&str> = match value {
                    Value::String(kind) => vec![kind],
                    Value::Array(types) => types.iter().filter_map(Value::as_str).collect(),
                    _ => {
                        return Err(Error::InvalidInput(
                            "Invalid Antigravity schema type".into(),
                        ))
                    }
                };
                if types.contains(&"null") {
                    out.insert("nullable".into(), json!(true));
                }
                let kinds: Vec<_> = types.into_iter().filter(|kind| *kind != "null").collect();
                if kinds.len() > 1 {
                    return Err(Error::InvalidInput(
                        "Antigravity does not support union schema types".into(),
                    ));
                }
                if let Some(kind) = kinds.first() {
                    out.insert("type".into(), json!(kind.to_uppercase()));
                }
            }
            "properties" => {
                let properties = value.as_object().ok_or_else(|| {
                    Error::InvalidInput("Invalid Antigravity schema properties".into())
                })?;
                let mut converted = serde_json::Map::new();
                for (name, property) in properties {
                    converted.insert(name.clone(), gemini_schema(property, root, depth + 1)?);
                }
                out.insert(key.clone(), Value::Object(converted));
            }
            "items" => {
                out.insert(key.clone(), gemini_schema(value, root, depth + 1)?);
            }
            "anyOf" => {
                let branches = value.as_array().ok_or_else(|| {
                    Error::InvalidInput("Invalid Antigravity schema alternatives".into())
                })?;
                out.insert(
                    key.clone(),
                    Value::Array(
                        branches
                            .iter()
                            .map(|branch| gemini_schema(branch, root, depth + 1))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                );
            }
            "description" | "enum" | "required" | "nullable" | "format" => {
                out.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    Ok(Value::Object(out))
}

#[derive(Default)]
struct GeminiStream {
    parts: Vec<Value>,
    call_ids: Vec<String>,
    usage: Option<TokenUsage>,
    reason: Option<FinishReason>,
    failed: bool,
    finished: bool,
    part_bytes: usize,
}

impl GeminiStream {
    fn data(&mut self, data: &str) -> Vec<Result<ChunkType>> {
        if self.failed || self.finished || data.trim().is_empty() || data.trim() == "[DONE]" {
            return Vec::new();
        }
        if data.len() > 8 * 1024 * 1024 {
            self.failed = true;
            return vec![Err(Error::Stream(
                "Antigravity SSE event exceeds limit".into(),
            ))];
        }
        let result = serde_json::from_str::<Value>(data)
            .map_err(|_| Error::Stream("Malformed Antigravity SSE JSON".into()))
            .and_then(|value| self.response(&value));
        match result {
            Ok(chunks) => chunks.into_iter().map(Ok).collect(),
            Err(error) => {
                self.failed = true;
                vec![Err(error)]
            }
        }
    }

    fn response(&mut self, envelope: &Value) -> Result<Vec<ChunkType>> {
        if envelope.get("error").is_some() {
            return Err(Error::Provider(
                "Antigravity returned a stream error".into(),
            ));
        }
        let response = envelope.get("response").unwrap_or(envelope);
        if response.get("error").is_some() {
            return Err(Error::Provider(
                "Antigravity returned a stream error".into(),
            ));
        }
        if let Some(usage) = response.get("usageMetadata") {
            let token = |key| -> Result<u64> {
                match usage.get(key) {
                    None => Ok(0),
                    Some(value) => value
                        .as_u64()
                        .ok_or_else(|| Error::Stream("Invalid Antigravity token usage".into())),
                }
            };
            let cached = token("cachedContentTokenCount")?;
            self.usage = Some(TokenUsage {
                input: token("promptTokenCount")?.saturating_sub(cached),
                output: token("candidatesTokenCount")?.saturating_add(token("thoughtsTokenCount")?),
                cache_read: cached,
                cache_write: 0,
            });
        }
        if response
            .pointer("/promptFeedback/blockReason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason != "BLOCK_REASON_UNSPECIFIED")
        {
            self.reason = Some(FinishReason::Refusal);
            return Ok(Vec::new());
        }
        let Some(candidates) = response.get("candidates") else {
            if response.get("usageMetadata").is_some() {
                return Ok(Vec::new());
            }
            return Err(Error::Stream(
                "Antigravity SSE response has no candidates".into(),
            ));
        };
        let candidates = candidates
            .as_array()
            .ok_or_else(|| Error::Stream("Invalid Antigravity candidates".into()))?;
        if candidates.len() > 1 {
            return Err(Error::Stream(
                "Multiple Antigravity candidates are unsupported".into(),
            ));
        }
        let mut chunks = Vec::new();
        for candidate in candidates {
            if let Some(content) = candidate.get("content") {
                let parts = content
                    .get("parts")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::Stream("Invalid Antigravity content parts".into()))?;
                for part in parts {
                    if !part.is_object() {
                        return Err(Error::Stream("Invalid Antigravity part".into()));
                    }
                    if let Some(signature) = part.get("thoughtSignature") {
                        if !signature.is_string() {
                            return Err(Error::Stream(
                                "Invalid Antigravity thought signature".into(),
                            ));
                        }
                    }
                    if let Some(text) = part.get("text") {
                        let text = text
                            .as_str()
                            .ok_or_else(|| Error::Stream("Invalid Antigravity text".into()))?;
                        if part["thought"] == true {
                            chunks.push(ChunkType::Reasoning(text.into()));
                        } else {
                            chunks.push(ChunkType::Text(text.into()));
                        }
                    }
                    if let Some(call) = part.get("functionCall") {
                        let name = call
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|name| !name.is_empty())
                            .ok_or_else(|| {
                                Error::Stream("Antigravity function call has no name".into())
                            })?;
                        let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                        if !args.is_object() {
                            return Err(Error::Stream(
                                "Invalid Antigravity function arguments".into(),
                            ));
                        }
                        let id = match call.get("id") {
                            Some(value) => value
                                .as_str()
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                                .ok_or_else(|| {
                                    Error::Stream("Invalid Antigravity function ID".into())
                                })?,
                            None => format!("ag_{}", cuid2::create_id()),
                        };
                        if self.call_ids.contains(&id) {
                            return Err(Error::Stream("Duplicate Antigravity function ID".into()));
                        }
                        chunks.push(ChunkType::ToolCall(
                            json!([{
                                "index": self.call_ids.len(), "id": id, "type": "function",
                                "function": {"name": name, "arguments_done": args.to_string()}
                            }])
                            .to_string(),
                        ));
                        self.call_ids.push(id);
                    }
                    if part.get("text").is_none()
                        && part.get("functionCall").is_none()
                        && part.get("thoughtSignature").is_none()
                    {
                        return Err(Error::Stream(
                            "Unsupported Antigravity response part".into(),
                        ));
                    }
                    self.part_bytes = self.part_bytes.saturating_add(part.to_string().len());
                    if self.parts.len() >= 65_536 || self.part_bytes > 16 * 1024 * 1024 {
                        return Err(Error::Stream("Antigravity replay exceeds limit".into()));
                    }
                    self.parts.push(part.clone());
                }
            }
            if let Some(reason) = candidate.get("finishReason") {
                let reason = reason
                    .as_str()
                    .ok_or_else(|| Error::Stream("Invalid Antigravity finish reason".into()))?;
                self.reason = Some(match reason {
                    "STOP" => {
                        if self.call_ids.is_empty() {
                            FinishReason::Stop
                        } else {
                            FinishReason::ToolCalls
                        }
                    }
                    "MAX_TOKENS" => FinishReason::Length,
                    "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
                    | "IMAGE_SAFETY" => FinishReason::Refusal,
                    // Tool execution must never proceed after a failed generation.
                    _ => {
                        return Err(Error::Stream(
                            "Antigravity generation did not finish successfully".into(),
                        ))
                    }
                });
            }
        }
        Ok(chunks)
    }

    fn finish(&mut self) -> Vec<Result<ChunkType>> {
        if self.failed || self.finished {
            return Vec::new();
        }
        self.finished = true;
        let Some(reason) = self.reason.take() else {
            return vec![Err(Error::Stream(
                "Antigravity stream closed before finishReason".into(),
            ))];
        };
        let mut chunks = Vec::new();
        // Store the *entire* ordered model turn, not just a signature string:
        // signatures bind to their original parts, including text/thought parts.
        // The SDK reorders reasoning/text/calls in history; this opaque sibling
        // restores the exact wire parts and suppresses their flattened duplicates.
        if self
            .parts
            .iter()
            .any(|part| part.get("thoughtSignature").is_some())
        {
            let replay = Replay {
                parts: std::mem::take(&mut self.parts),
                call_ids: self.call_ids.clone(),
            };
            match serde_json::to_string(&replay) {
                Ok(payload) => chunks.push(Ok(ChunkType::ReasoningItem(ReasoningReplayItem {
                    id: Some(format!("ag_replay_{}", cuid2::create_id())),
                    summary: String::new(),
                    encrypted_content: Some(format!("{REPLAY_PREFIX}{payload}")),
                }))),
                Err(_) => {
                    return vec![Err(Error::Stream(
                        "Could not preserve Antigravity thought signatures".into(),
                    ))]
                }
            }
        }
        if let Some(usage) = self.usage.take() {
            chunks.push(Ok(ChunkType::Usage(usage)));
        }
        chunks.push(Ok(ChunkType::End {
            reason: Some(reason),
        }));
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Antigravity {
        Antigravity::new(
            "http://localhost".into(),
            "gemini-test".into(),
            "secret-token".into(),
            "project".into(),
        )
    }

    #[test]
    fn serializes_messages_images_tools_and_stable_session() {
        let tool = Tool::builder().name("read").description("Read a file")
            .input_schema(serde_json::from_value(json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})).unwrap())
            .execute(crate::aisdk::tool::ToolExecute::new(|_| async { Ok::<_, String>("done") })).build().unwrap();
        let messages = vec![
            Message::system("Use tools"),
            Message::user_with_images(
                "look",
                vec![ImageContent {
                    data_url: "data:image/png;base64,aGk=".into(),
                    media_type: "image/png".into(),
                }],
            ),
            Message::assistant("reading"),
            Message::tool_call("call", "read", r#"{"path":"a"}"#),
            Message::tool_output("call", "read", "contents", false),
        ];
        let headers = HashMap::from([
            ("X-Opencode-Session".into(), "session".into()),
            ("x-api-key".into(), "never-send".into()),
        ]);
        let body = model().request_body(&messages, &[tool], &headers).unwrap();
        assert_eq!(body["model"], "gemini-test");
        assert_eq!(body["request"]["sessionId"], "session");
        assert_eq!(
            body["request"]["systemInstruction"]["parts"][0]["text"],
            "Use tools"
        );
        assert_eq!(
            body["request"]["contents"][0]["parts"][1]["inlineData"]["data"],
            "aGk="
        );
        assert_eq!(body["request"]["contents"][1]["role"], "model");
        assert_eq!(
            body["request"]["contents"][1]["parts"][1]["functionCall"]["args"]["path"],
            "a"
        );
        assert_eq!(
            body["request"]["contents"][2]["parts"][0]["functionResponse"]["response"]["output"],
            "contents"
        );
        assert_eq!(
            body["request"]["tools"][0]["functionDeclarations"][0]["parameters"]["type"],
            "OBJECT"
        );
        assert!(!body.to_string().contains("secret-token"));
        assert!(!body.to_string().contains("never-send"));
        assert!(!format!("{:?}", model()).contains("secret-token"));
    }

    #[test]
    fn signatures_restore_exact_parts_without_duplicating_calls_or_text() {
        let parts = json!([
            {"text":"thinking", "thought":true,"thoughtSignature":"thought-signature"},
            {"text":"inspect", "thoughtSignature":"text-signature"},
            {"functionCall":{"name":"read","args":{"path":"a"}},"thoughtSignature":"call-signature"},
            {"functionCall":{"name":"read","args":{"path":"b"}}}
        ]);
        let mut state = GeminiStream::default();
        let chunks = state.response(&json!({"response":{"candidates":[{"content":{"role":"model","parts":parts},"finishReason":"STOP"}]}})).unwrap();
        assert!(matches!(&chunks[0], ChunkType::Reasoning(text) if text == "thinking"));
        let calls = state.call_ids.clone();
        let terminal = state.finish();
        let replay = terminal
            .into_iter()
            .find_map(|chunk| match chunk.unwrap() {
                ChunkType::ReasoningItem(item) => Some(item),
                _ => None,
            })
            .unwrap();
        let messages = vec![
            Message::user("start"),
            Message::reasoning(replay.id, replay.summary, replay.encrypted_content),
            Message::assistant("inspect"),
            Message::tool_call(&calls[0], "read", r#"{"path":"a"}"#),
            Message::tool_call(&calls[1], "read", r#"{"path":"b"}"#),
            Message::tool_output(&calls[0], "read", "a", false),
            Message::tool_output(&calls[1], "read", "b", false),
        ];
        let (contents, _) = gemini_messages(&messages).unwrap();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[1]["parts"], parts);
        assert_eq!(contents[2]["parts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn persisted_replay_consumes_split_sanitized_assistant_text() {
        for (first, second) in [("first", "second"), (" \n first  ", " \tsecond\n ")] {
            let parts = json!([
                {"text":first,"thoughtSignature":"signed-first"},
                {"text":"thinking","thought":true,"thoughtSignature":"signed-thought"},
                {"text":second,"thoughtSignature":"signed-second"}
            ]);
            let mut state = GeminiStream::default();
            state
                .response(
                    &json!({"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}),
                )
                .unwrap();
            let item = state
                .finish()
                .into_iter()
                .find_map(|chunk| match chunk.unwrap() {
                    ChunkType::ReasoningItem(item) => Some(item),
                    _ => None,
                })
                .unwrap();
            let mut stored = crate::session::types::Message::new(
                crate::session::types::MessageRole::Assistant,
                "",
            );
            stored.append(first);
            stored.append_reasoning("thinking");
            stored.append(second);
            persist_replay(&mut stored, item);
            assert_eq!(stored.parts[0].part_type, "provider_replay");
            let replay = &stored.parts[0].data;
            // Match the host's flattened, trimmed text messages without calling its private converter.
            let mut messages = vec![Message::reasoning(
                replay["id"].as_str().map(str::to_owned),
                "",
                replay["encrypted_content"].as_str().map(str::to_owned),
            )];
            for part in &stored.parts {
                if part.part_type == "text" {
                    messages.push(Message::assistant(
                        part.data["text"].as_str().unwrap().trim(),
                    ));
                }
            }
            messages.push(Message::assistant("new text"));
            let (contents, _) = gemini_messages(&messages).unwrap();
            let mut expected = parts.as_array().unwrap().clone();
            expected.push(json!({"text":"new text"}));
            assert_eq!(contents[0]["parts"], json!(expected));
        }
    }

    async fn held_open_server(
        first: String,
        trailing: Option<String>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..index]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .map(|value| value.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if request.len() >= index + 4 + length {
                        break;
                    }
                }
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n").await.unwrap();
            socket.write_all(first.as_bytes()).await.unwrap();
            if let Some(trailing) = trailing {
                tokio::time::sleep(Duration::from_millis(10)).await;
                socket.write_all(trailing.as_bytes()).await.unwrap();
            }
            std::future::pending::<()>().await;
        });
        (base, task)
    }

    fn terminal_event() -> String {
        format!(
            "data: {}\n\n",
            json!({"candidates":[{"content":{"parts":[{"text":"first"},{"text":"second"}]},"finishReason":"STOP"}]})
        )
    }

    #[tokio::test]
    async fn held_open_terminal_preserves_chunks_and_trailing_usage() {
        let (base, server) = held_open_server(
            terminal_event(),
            Some(format!(
                "data: {}\n\n",
                json!({"usageMetadata":{"candidatesTokenCount":7}})
            )),
        )
        .await;
        let response = reqwest::get(base).await.unwrap();
        let chunks = tokio::time::timeout(
            Duration::from_secs(2),
            gemini_event_stream(
                response.bytes_stream().eventsource(),
                Duration::from_secs(1),
                Duration::from_millis(80),
            )
            .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        server.abort();
        assert_eq!(chunks.len(), 4);
        assert!(matches!(&chunks[0], Ok(ChunkType::Text(text)) if text == "first"));
        assert!(matches!(&chunks[1], Ok(ChunkType::Text(text)) if text == "second"));
        assert!(matches!(&chunks[2], Ok(ChunkType::Usage(usage)) if usage.output == 7));
        assert!(matches!(&chunks[3], Ok(ChunkType::End { .. })));
    }

    #[tokio::test]
    async fn done_terminates_immediately_and_failures_do_not_finish() {
        for (event, success) in [
            (format!("{}data: [DONE]\n\n", terminal_event()), true),
            ("data: malformed\n\n".into(), false),
            ("data: [DONE]\n\n".into(), false),
        ] {
            let (base, server) = held_open_server(event, None).await;
            let response = reqwest::get(base).await.unwrap();
            let chunks = tokio::time::timeout(
                Duration::from_secs(1),
                gemini_event_stream(
                    response.bytes_stream().eventsource(),
                    Duration::from_secs(30),
                    Duration::from_secs(30),
                )
                .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
            server.abort();
            if success {
                assert_eq!(chunks.len(), 3);
                assert_eq!(
                    chunks
                        .iter()
                        .filter(|chunk| matches!(chunk, Ok(ChunkType::End { .. })))
                        .count(),
                    1
                );
            } else {
                assert_eq!(chunks.len(), 1);
                assert!(chunks[0].is_err());
            }
        }
    }

    #[tokio::test]
    async fn terminal_grace_is_not_extended_by_more_events() {
        let events = futures::stream::unfold(0, |index| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let data = if index == 0 {
                json!({"candidates":[{"finishReason":"STOP"}]}).to_string()
            } else {
                json!({"usageMetadata":{"candidatesTokenCount":index}}).to_string()
            };
            Some((
                Ok::<_, std::io::Error>(eventsource_stream::Event {
                    data,
                    ..Default::default()
                }),
                index + 1,
            ))
        });
        let chunks = tokio::time::timeout(
            Duration::from_secs(1),
            gemini_event_stream(events, Duration::from_secs(30), Duration::from_millis(40))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(matches!(&chunks[0], Ok(ChunkType::Usage(usage)) if usage.output > 0));
        assert!(matches!(&chunks[1], Ok(ChunkType::End { .. })));
    }

    #[tokio::test]
    async fn eof_and_transport_errors_finalize_only_once() {
        for fail in [false, true] {
            let mut events = vec![Ok(eventsource_stream::Event {
                data: json!({"candidates":[{"finishReason":"STOP"}]}).to_string(),
                ..Default::default()
            })];
            if fail {
                events.push(Err(std::io::Error::other("disconnected")));
            }
            let chunks = gemini_event_stream(
                futures::stream::iter(events),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .collect::<Vec<_>>()
            .await;
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].is_err(), fail);
        }
        let mut parser = GeminiStream::default();
        parser
            .response(&json!({"candidates":[{"finishReason":"STOP"}]}))
            .unwrap();
        assert_eq!(parser.finish().len(), 1);
        assert!(parser.finish().is_empty());
        assert!(parser.data("malformed").is_empty());
    }

    #[tokio::test]
    async fn stalled_reads_are_bounded_even_inside_incomplete_sse_frames() {
        for request_timeout in [false, true] {
            let (base, server) = held_open_server("data: {".into(), None).await;
            let client = reqwest::Client::builder()
                .read_timeout(if request_timeout {
                    Duration::from_millis(40)
                } else {
                    Duration::from_secs(30)
                })
                .build()
                .unwrap();
            let response = client.get(base).send().await.unwrap();
            let chunks = tokio::time::timeout(
                Duration::from_secs(2),
                gemini_event_stream(
                    response.bytes_stream().eventsource(),
                    Duration::from_millis(100),
                    Duration::from_millis(20),
                )
                .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
            server.abort();
            assert_eq!(chunks.len(), 1);
            assert!(chunks[0].is_err());
        }
    }

    #[tokio::test]
    async fn sdk_cancellation_interrupts_a_stalled_provider() {
        let event = format!(
            "data: {}\n\n",
            json!({"candidates":[{"content":{"parts":[{"text":"ready"}]}}]})
        );
        let (base, server) = held_open_server(event, None).await;
        let provider = Antigravity::new(base, "test".into(), "token".into(), "project".into());
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut response = crate::aisdk::response::stream_with_tools_options(
            provider,
            vec![Message::user("hello")],
            vec![],
            Some(1),
            None,
            HashMap::new(),
            Some(cancel.clone()),
            crate::aisdk::response::StreamWithToolsOptions::default(),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(chunk) = response.stream.next().await {
                if matches!(chunk, ChunkType::Text(_)) {
                    break;
                }
            }
            cancel.cancel();
            while response.stream.next().await.is_some() {}
        })
        .await
        .unwrap();
        server.abort();
    }

    #[test]
    fn cumulative_usage_is_emitted_once_including_reasoning_and_cache() {
        let mut state = GeminiStream::default();
        state.response(&json!({"response":{"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":40,"candidatesTokenCount":3}}})).unwrap();
        state.response(&json!({"response":{"candidates":[{"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":40,"candidatesTokenCount":10,"thoughtsTokenCount":5}}})).unwrap();
        let chunks = state.finish();
        assert!(matches!(
            &chunks[0],
            Ok(ChunkType::Usage(TokenUsage {
                input: 60,
                output: 15,
                cache_read: 40,
                cache_write: 0
            }))
        ));
        assert!(matches!(
            &chunks[1],
            Ok(ChunkType::End {
                reason: Some(FinishReason::Length)
            })
        ));
    }

    #[test]
    fn malformed_errors_and_truncation_fail_without_echoing_payloads() {
        for data in [
            "secret-token",
            r#"{"error":{"message":"secret-token"}}"#,
            r#"{"response":{"error":{"message":"secret-token"}}}"#,
            r#"{"response":{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":[]}}]}}]}}"#,
        ] {
            let mut state = GeminiStream::default();
            let chunks = state.data(data);
            assert!(chunks[0].is_err());
            assert!(!chunks[0]
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("secret-token"));
            assert!(state.finish().is_empty());
        }
        assert!(GeminiStream::default().finish()[0].is_err());
        let mut state = GeminiStream::default();
        state
            .response(&json!({"response":{"candidates":[{"finishReason":"SAFETY"}]}}))
            .unwrap();
        assert!(matches!(
            &state.finish()[0],
            Ok(ChunkType::End {
                reason: Some(FinishReason::Refusal)
            })
        ));
    }

    #[tokio::test]
    async fn sse_parser_handles_split_bytes_and_multiline_data() {
        let bytes = futures::stream::iter(vec![
            Ok::<_, std::io::Error>(b"data: {\"response\":{\"candi".to_vec()),
            Ok(b"dates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]},\n".to_vec()),
            Ok(b"data: \"finishReason\":\"STOP\"}]}}\n\n".to_vec()),
        ]);
        let mut events = bytes.eventsource();
        let mut state = GeminiStream::default();
        let event = events.next().await.unwrap().unwrap();
        assert!(
            matches!(&state.data(&event.data)[0], Ok(ChunkType::Text(text)) if text == "hello")
        );
        assert!(events.next().await.is_none());
        assert!(matches!(
            &state.finish()[0],
            Ok(ChunkType::End {
                reason: Some(FinishReason::Stop)
            })
        ));
    }
    #[tokio::test]
    async fn http_tool_loop_replays_signatures_and_never_forwards_api_keys() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for step in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut buffer = [0u8; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..index]).to_lowercase();
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|value| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (index + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut buffer = [0u8; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
                assert!(headers.starts_with("post /v1internal:streamgeneratecontent?alt=sse "));
                assert!(headers.contains("authorization: bearer secret-token"));
                assert!(!headers.contains("api-key"));
                assert!(!headers.contains("never-send"));
                requests.push(
                    serde_json::from_slice::<Value>(&bytes[header_end..header_end + length])
                        .unwrap(),
                );
                let parts = if step == 0 {
                    json!([{ "functionCall": {"name":"read", "args":{}, "id":"call-1"}, "thoughtSignature":"opaque-signature" }])
                } else {
                    json!([{ "text":"done" }])
                };
                let event = format!(
                    "data: {}\n\n",
                    json!({"response":{"candidates":[{"content":{"role":"model","parts":parts},"finishReason":"STOP"}]}})
                );
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", event.len()).as_bytes()).await.unwrap();
                for chunk in event.as_bytes().chunks(7) {
                    socket.write_all(chunk).await.unwrap();
                }
            }
            requests
        });
        let tool = Tool::builder()
            .name("read")
            .description("Read")
            .input_schema(serde_json::from_value(json!({"type":"object","properties":{}})).unwrap())
            .execute(crate::aisdk::tool::ToolExecute::new(|_| async {
                Ok::<_, String>("result")
            }))
            .build()
            .unwrap();
        let provider = Antigravity::new(
            base,
            "gemini-test".into(),
            "secret-token".into(),
            "project".into(),
        );
        let mut response = crate::aisdk::response::stream_with_tools_options(
            provider,
            vec![Message::user("read")],
            vec![tool],
            Some(2),
            None,
            HashMap::from([
                ("x-api-key".into(), "never-send".into()),
                ("x-opencode-session".into(), "stable".into()),
            ]),
            None,
            crate::aisdk::response::StreamWithToolsOptions::default(),
        )
        .await
        .unwrap();
        let mut text = String::new();
        while let Some(chunk) = response.stream.next().await {
            match chunk {
                ChunkType::Text(value) => text.push_str(&value),
                ChunkType::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        assert_eq!(text, "done");
        let requests = server.await.unwrap();
        assert_eq!(
            requests[0]["request"]["sessionId"],
            requests[1]["request"]["sessionId"]
        );
        let contents = &requests[1]["request"]["contents"];
        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            "opaque-signature"
        );
        assert_eq!(contents[1]["parts"].as_array().unwrap().len(), 1);
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["output"],
            "result"
        );
    }
}
