use crate::chunk::{ChunkType, MessagePhase};
use crate::error::Result;
use crate::message::Message;
use crate::provider::Provider;
use crate::stop::{StopReason, StopWhenFn};
use crate::tool::Tool;
use futures::{future::join_all, StreamExt};
use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct StreamTextResponse {
    pub stream: LanguageModelStream,
    stop_reason: Arc<tokio::sync::Mutex<Option<StopReason>>>,
    messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

pub struct LanguageModelStream {
    rx: mpsc::UnboundedReceiver<ChunkType>,
}

impl futures::Stream for LanguageModelStream {
    type Item = ChunkType;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

impl StreamTextResponse {
    fn create() -> (Self, mpsc::UnboundedSender<ChunkType>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let stop_reason = Arc::new(tokio::sync::Mutex::new(None));
        let messages = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        (
            Self {
                stream: LanguageModelStream { rx },
                stop_reason: stop_reason.clone(),
                messages: messages.clone(),
                _handles: Vec::new(),
            },
            tx,
        )
    }

    pub async fn stop_reason(&self) -> Option<StopReason> {
        self.stop_reason.lock().await.clone()
    }

    pub async fn messages(&self) -> Vec<Message> {
        self.messages.lock().await.clone()
    }

    fn add_handle(&mut self, handle: tokio::task::JoinHandle<()>) {
        self._handles.push(handle);
    }
}

pub async fn stream_with_tools<P: Provider>(
    provider: P,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    max_steps: Option<usize>,
    stop_when: Option<StopWhenFn>,
    headers: HashMap<String, String>,
) -> Result<StreamTextResponse> {
    let (mut response, tx) = StreamTextResponse::create();
    let _ = tx.send(ChunkType::Start);

    let tx_loop = tx.clone();
    let stop_reason_arc = response.stop_reason.clone();
    let messages_arc = response.messages.clone();
    let provider_clone = provider.clone();

    let handle = tokio::spawn(async move {
        let mut current_messages = messages;
        let mut step_idx: usize = 0;
        let max_steps = max_steps.unwrap_or(usize::MAX);
        let mut cached_repeatable_tool_results: HashMap<String, String> = HashMap::new();

        loop {
            step_idx += 1;

            if step_idx > max_steps {
                let _ = tx_loop.send(ChunkType::Incomplete("Max steps reached".to_string()));
                *stop_reason_arc.lock().await = Some(StopReason::Hook);
                break;
            }

            if let Some(ref hook) = stop_when {
                if hook(step_idx) {
                    *stop_reason_arc.lock().await = Some(StopReason::Hook);
                    break;
                }
            }

            let step_summary = provider_step_log_summary(&current_messages, &tools);
            let _ = tx_loop.send(ChunkType::Metadata(format!(
                "provider_step_start step={} messages={} tools={} {}",
                step_idx,
                current_messages.len(),
                tools.len(),
                step_summary
            )));

            let stream_result = provider_clone
                .stream_text(&current_messages, &tools, &headers)
                .await;

            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    let err = format!(
                        "provider_step_error step={} messages={} tools={} {} error={}",
                        step_idx,
                        current_messages.len(),
                        tools.len(),
                        step_summary,
                        e
                    );
                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                    break;
                }
            };

            let mut has_tool_call = false;
            let mut tool_call_accumulator = ToolCallAccumulator::default();
            let mut accumulated_text = String::new();
            let mut saw_terminal_event = false;
            let mut response_end_turn = None;
            let mut last_assistant_message_phase = None;
            let mut current_assistant_message_phase = None;

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(ChunkType::AssistantMessagePhase { phase }) => {
                        current_assistant_message_phase = phase;
                        last_assistant_message_phase = phase;
                        let label = match phase {
                            Some(MessagePhase::Commentary) => "commentary",
                            Some(MessagePhase::FinalAnswer) => "final_answer",
                            None => "unknown",
                        };
                        let _ = tx_loop.send(ChunkType::Metadata(format!(
                            "assistant_message_phase={label}"
                        )));
                    }
                    Ok(ChunkType::ResponseCompleted { end_turn }) => {
                        saw_terminal_event = true;
                        response_end_turn = end_turn;
                        let _ = tx_loop.send(ChunkType::Metadata(format!(
                            "response.completed end_turn={end_turn:?}"
                        )));
                    }
                    Ok(ChunkType::Text(text)) => {
                        last_assistant_message_phase = current_assistant_message_phase;
                        accumulated_text.push_str(&text);
                        let _ = tx_loop.send(ChunkType::Text(text));
                    }
                    Ok(ChunkType::Reasoning(reasoning)) => {
                        let _ = tx_loop.send(ChunkType::Reasoning(reasoning));
                    }
                    Ok(ChunkType::ToolCall(json_str)) => {
                        has_tool_call = true;
                        let _ = tx_loop.send(ChunkType::ToolCall(json_str.clone()));
                        if let Err(err) = tool_call_accumulator.ingest(&json_str) {
                            let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                            *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                            return;
                        }
                    }
                    Ok(ChunkType::End(_content)) => {
                        // Processed internally — NOT forwarded to tx_loop.
                        // Forwarding End would cause relay_stream_to_sender
                        // to return Ended prematurely, dropping the channel
                        // before tool execution / subsequent steps.
                        saw_terminal_event = true;
                    }
                    Ok(ChunkType::Metadata(msg)) => {
                        let _ = tx_loop.send(ChunkType::Metadata(msg));
                    }
                    Ok(ChunkType::Incomplete(msg)) => {
                        let err = format!("Provider response incomplete: {}", msg);
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                    Ok(ChunkType::Failed(err)) => {
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                    Ok(ChunkType::Start) => {
                        let _ = tx_loop.send(ChunkType::Start);
                    }
                    Ok(ChunkType::NotSupported(msg)) => {
                        let _ = tx_loop.send(ChunkType::NotSupported(msg));
                    }
                    Err(e) => {
                        let err = e.to_string();
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                }
            }

            if !saw_terminal_event {
                let err = "Provider stream ended without a terminal completion event".to_string();
                let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                return;
            }

            // Build assistant message from accumulated text deltas
            let assistant_text = accumulated_text.trim().to_string();
            if !assistant_text.is_empty() {
                let assistant_msg = Message::assistant(&assistant_text);
                current_messages.push(assistant_msg.clone());
                messages_arc.lock().await.push(assistant_msg);
            }

            if !has_tool_call {
                let needs_follow_up = matches!(response_end_turn, Some(false))
                    || matches!(last_assistant_message_phase, Some(MessagePhase::Commentary));
                if needs_follow_up {
                    let reason = if matches!(response_end_turn, Some(false)) {
                        "end_turn=false"
                    } else {
                        "assistant_message_phase=commentary"
                    };
                    let _ = tx_loop.send(ChunkType::Metadata(format!(
                        "continuing model turn after non-final assistant output ({reason})"
                    )));
                    continue;
                }
                *stop_reason_arc.lock().await = Some(StopReason::Finish);
                break;
            }

            let tool_calls_to_execute = match tool_call_accumulator.finish() {
                Ok(tool_calls) if !tool_calls.is_empty() => tool_calls,
                Ok(_) => {
                    let err = "Tool call stream did not contain executable tool calls".to_string();
                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                    return;
                }
                Err(err) => {
                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                    return;
                }
            };

            let mut tool_results_to_observe = Vec::new();
            let mut tool_calls_to_run = Vec::new();

            for (call_id, tool_name, args) in tool_calls_to_execute {
                let cache_key = repeatable_tool_cache_key(&tool_name, &args);
                if let Some(cached_output) = cache_key
                    .as_ref()
                    .and_then(|key| cached_repeatable_tool_results.get(key))
                    .cloned()
                {
                    tool_results_to_observe.push(ToolExecutionResult {
                        call_id,
                        tool_name,
                        output: format!(
                            "Duplicate task call skipped; reusing the prior result from this response.\n\n{}",
                            cached_output
                        ),
                        cache_key: None,
                        is_error: false,
                    });
                } else {
                    tool_calls_to_run.push((call_id, tool_name, args, cache_key));
                }
            }

            let tool_results = join_all(tool_calls_to_run.into_iter().map(
                |(call_id, tool_name, args, cache_key)| {
                    let tool = tools.iter().find(|t| t.name == tool_name).cloned();

                    async move {
                        match tool {
                            Some(t) => match t.execute.call(args).await {
                                Ok(output) => ToolExecutionResult {
                                    call_id,
                                    tool_name: tool_name.clone(),
                                    output,
                                    cache_key,
                                    is_error: false,
                                },
                                Err(err) => ToolExecutionResult {
                                    call_id,
                                    tool_name: tool_name.clone(),
                                    output: format!("Tool '{}' error: {}", tool_name, err),
                                    cache_key: None,
                                    is_error: true,
                                },
                            },
                            None => ToolExecutionResult {
                                call_id,
                                tool_name: tool_name.clone(),
                                output: format!("Tool not found: {}", tool_name),
                                cache_key: None,
                                is_error: true,
                            },
                        }
                    }
                },
            ))
            .await;

            for result in tool_results {
                if result.is_error {
                    let _ = tx_loop.send(ChunkType::Metadata(format!(
                        "tool_result_error tool={} call_id={} output_chars={}",
                        result.tool_name,
                        result.call_id,
                        result.output.len()
                    )));
                } else if let Some(cache_key) = result.cache_key.as_ref() {
                    cached_repeatable_tool_results.insert(cache_key.clone(), result.output.clone());
                }
                tool_results_to_observe.push(result);
            }

            if !tool_results_to_observe.is_empty() {
                let observation = format_tool_observation(&tool_results_to_observe);
                let tool_names = tool_results_to_observe
                    .iter()
                    .map(|result| result.tool_name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let tool_result_summary = tool_results_log_summary(&tool_results_to_observe);
                let _ = tx_loop.send(ChunkType::Metadata(format!(
                    "tool_results_added count={} names={} {} observation_chars={} next_messages={}",
                    tool_results_to_observe.len(),
                    tool_names,
                    tool_result_summary,
                    observation.len(),
                    current_messages.len() + 1
                )));
                current_messages.push(Message::user(observation.clone()));
                messages_arc.lock().await.push(Message::user(observation));
            }
        }
    });

    response.add_handle(handle);
    Ok(response)
}

#[derive(Debug, Default)]
struct MessageLogSummary {
    system_messages: usize,
    user_messages: usize,
    assistant_messages: usize,
    text_bytes: usize,
    image_count: usize,
    max_message_role: &'static str,
    max_message_bytes: usize,
    last_message_role: &'static str,
    last_message_bytes: usize,
    last_message_images: usize,
}

fn provider_step_log_summary(messages: &[Message], tools: &[Tool]) -> String {
    let messages = message_log_summary(messages);
    let tools = tool_log_summary(tools);

    format!(
        "message_roles[system={},user={},assistant={}] message_text_bytes={} images={} max_message[role={},bytes={}] last_message[role={},bytes={},images={}] {}",
        messages.system_messages,
        messages.user_messages,
        messages.assistant_messages,
        messages.text_bytes,
        messages.image_count,
        messages.max_message_role,
        messages.max_message_bytes,
        messages.last_message_role,
        messages.last_message_bytes,
        messages.last_message_images,
        tools,
    )
}

fn message_log_summary(messages: &[Message]) -> MessageLogSummary {
    let mut summary = MessageLogSummary {
        max_message_role: "none",
        last_message_role: "none",
        ..MessageLogSummary::default()
    };

    for message in messages {
        let role = message_role(message);
        let (text_bytes, image_count) = message_size(message);

        match message {
            Message::System(_) => summary.system_messages += 1,
            Message::User(_) => summary.user_messages += 1,
            Message::Assistant(_) => summary.assistant_messages += 1,
        }

        summary.text_bytes += text_bytes;
        summary.image_count += image_count;
        summary.last_message_role = role;
        summary.last_message_bytes = text_bytes;
        summary.last_message_images = image_count;

        if text_bytes > summary.max_message_bytes {
            summary.max_message_role = role;
            summary.max_message_bytes = text_bytes;
        }
    }

    summary
}

fn message_role(message: &Message) -> &'static str {
    match message {
        Message::System(_) => "system",
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
    }
}

fn message_size(message: &Message) -> (usize, usize) {
    match message {
        Message::System(message) => (message.content.len(), 0),
        Message::User(message) => (message.content.len(), message.images.len()),
        Message::Assistant(message) => (message.content.len(), 0),
    }
}

fn tool_log_summary(tools: &[Tool]) -> String {
    let schema_bytes = tools
        .iter()
        .filter_map(|tool| serde_json::to_vec(&tool.input_schema).ok())
        .map(|schema| schema.len())
        .sum::<usize>();
    let description_bytes = tools
        .iter()
        .map(|tool| tool.description.len())
        .sum::<usize>();
    let tool_names = compact_tool_names(tools);

    format!(
        "tool_names=[{}] tool_schema_bytes={} tool_description_bytes={}",
        tool_names, schema_bytes, description_bytes,
    )
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

fn tool_results_log_summary(results: &[ToolExecutionResult]) -> String {
    let output_bytes = results
        .iter()
        .map(|result| result.output.len())
        .sum::<usize>();
    let error_results = results.iter().filter(|result| result.is_error).count();
    let max_output = results.iter().max_by_key(|result| result.output.len());
    let (max_tool, max_bytes) = max_output
        .map(|result| (result.tool_name.as_str(), result.output.len()))
        .unwrap_or(("none", 0));

    format!(
        "output_bytes={} error_results={} max_output[tool={},bytes={}]",
        output_bytes, error_results, max_tool, max_bytes,
    )
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    calls: Vec<PendingToolCall>,
}

#[derive(Debug)]
struct PendingToolCall {
    key: String,
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    final_arguments: Option<String>,
    saw_arguments: bool,
}

#[derive(Debug)]
struct ToolExecutionResult {
    call_id: String,
    tool_name: String,
    output: String,
    cache_key: Option<String>,
    is_error: bool,
}

fn repeatable_tool_cache_key(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    if tool_name != "task" {
        return None;
    }

    Some(format!("{}:{}", tool_name, canonical_json(args)))
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            value.to_string()
        }
        serde_json::Value::String(s) => {
            serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
        }
        serde_json::Value::Array(items) => {
            let parts = items.iter().map(canonical_json).collect::<Vec<_>>();
            format!("[{}]", parts.join(","))
        }
        serde_json::Value::Object(map) => {
            let sorted = map.iter().collect::<BTreeMap<_, _>>();
            let parts = sorted
                .into_iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                    format!("{}:{}", key, canonical_json(value))
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn format_tool_observation(results: &[ToolExecutionResult]) -> String {
    if let [result] = results {
        if result.is_error {
            return format!(
                "Tool `{}` failed:\n{}\n\nUse this tool error to adjust the next step. Do not repeat the same tool call unchanged unless the underlying file or input has changed.",
                result.tool_name, result.output
            );
        }

        return format!("Tool `{}` result:\n{}", result.tool_name, result.output);
    }

    let failed = results.iter().filter(|result| result.is_error).count();
    let mut observation = format!(
        "Tool batch results: {} tool calls returned, {} failed. Use these results to answer the user's request or adjust the next step. Do not repeat the same failing tool calls unchanged.",
        results.len(),
        failed
    );

    for (idx, result) in results.iter().enumerate() {
        observation.push_str(&format!(
            "\n\n<tool_result index=\"{}\" tool=\"{}\" call_id=\"{}\" status=\"{}\">\n{}\n</tool_result>",
            idx + 1,
            result.tool_name,
            result.call_id,
            if result.is_error { "error" } else { "ok" },
            result.output
        ));
    }

    observation
}

impl ToolCallAccumulator {
    fn ingest(&mut self, json_str: &str) -> std::result::Result<(), String> {
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| format!("Invalid tool call delta: {}", e))?;

        let items = parsed
            .as_array()
            .ok_or_else(|| "Unsupported tool call delta shape".to_string())?;

        for (array_index, item) in items.iter().enumerate() {
            self.ingest_openai_delta(item, array_index)?;
        }

        Ok(())
    }

    fn finish(self) -> std::result::Result<Vec<(String, String, serde_json::Value)>, String> {
        let mut results = Vec::new();

        for call in self.calls {
            let name = call
                .name
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("Tool call '{}' missing function name", call.key))?;

            let id = call.id.unwrap_or(call.key);
            let args = parse_tool_arguments(&id, &call.arguments, call.final_arguments.as_deref())?;

            results.push((id, name, args));
        }

        Ok(results)
    }

    fn ingest_openai_delta(
        &mut self,
        item: &serde_json::Value,
        array_index: usize,
    ) -> std::result::Result<(), String> {
        let key = tool_call_key(item, array_index);
        let pending = self.pending_for_key(key, item);

        if pending.id.is_none() {
            pending.id = item
                .get("id")
                .and_then(|value| value.as_str())
                .filter(|id| !id.is_empty())
                .map(ToString::to_string);
        }

        if let Some(function) = item.get("function") {
            if pending.name.is_none() {
                pending.name = function
                    .get("name")
                    .and_then(|value| value.as_str())
                    .filter(|name| !name.is_empty())
                    .map(ToString::to_string);
            }

            if let Some(arguments) = function.get("arguments") {
                pending.saw_arguments = true;
                match arguments {
                    serde_json::Value::String(delta) => pending.arguments.push_str(delta),
                    serde_json::Value::Null => {}
                    value => pending.arguments.push_str(&value.to_string()),
                }
            }

            if let Some(arguments) = function.get("arguments_done") {
                match arguments {
                    serde_json::Value::String(done) => {
                        pending.final_arguments = Some(done.clone());
                    }
                    serde_json::Value::Null => {}
                    value => pending.final_arguments = Some(value.to_string()),
                }
            }
        }

        Ok(())
    }

    fn pending_for_key(&mut self, key: String, item: &serde_json::Value) -> &mut PendingToolCall {
        if let Some(index) = self.calls.iter().position(|call| call.key == key) {
            return &mut self.calls[index];
        }

        if let Some(id) = item
            .get("id")
            .and_then(|value| value.as_str())
            .filter(|id| !id.is_empty())
        {
            if let Some(index) = self
                .calls
                .iter()
                .position(|call| call.id.as_deref() == Some(id))
            {
                return &mut self.calls[index];
            }
        }

        self.calls.push(PendingToolCall {
            key,
            id: None,
            name: None,
            arguments: String::new(),
            final_arguments: None,
            saw_arguments: false,
        });
        self.calls.last_mut().expect("pending tool call exists")
    }
}

fn parse_tool_arguments(
    id: &str,
    streamed_arguments: &str,
    final_arguments: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    let streamed = streamed_arguments.trim();

    if !streamed.is_empty() {
        match serde_json::from_str(streamed_arguments) {
            Ok(value) => return Ok(value),
            Err(streamed_err) => {
                if let Some(final_arguments) = final_arguments {
                    let final_trimmed = final_arguments.trim();
                    if !final_trimmed.is_empty() {
                        return serde_json::from_str(final_arguments).map_err(|final_err| {
                            format!(
                                "Tool call '{}' arguments are incomplete or invalid JSON: {}; final arguments were also invalid: {}",
                                id, streamed_err, final_err
                            )
                        });
                    }
                }

                return Err(format!(
                    "Tool call '{}' arguments are incomplete or invalid JSON: {}",
                    id, streamed_err
                ));
            }
        }
    }

    let Some(final_arguments) = final_arguments else {
        return Ok(serde_json::Value::Object(Default::default()));
    };

    let final_trimmed = final_arguments.trim();
    if final_trimmed.is_empty() {
        return Ok(serde_json::Value::Object(Default::default()));
    }

    serde_json::from_str(final_arguments).map_err(|e| {
        format!(
            "Tool call '{}' arguments are incomplete or invalid JSON: {}",
            id, e
        )
    })
}

fn tool_call_key(item: &serde_json::Value, array_index: usize) -> String {
    if let Some(index) = item.get("index").and_then(|value| value.as_u64()) {
        return format!("index:{}", index);
    }

    if let Some(id) = item
        .get("id")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
    {
        return format!("id:{}", id);
    }

    format!("position:{}", array_index)
}

#[cfg(test)]
mod tests {
    use super::{stream_with_tools, ToolCallAccumulator};
    use crate::chunk::{ChunkType, MessagePhase};
    use crate::message::Message;
    use crate::provider::{Provider, ProviderStream};
    use crate::stop::StopReason;
    use crate::tool::{Tool, ToolExecute};
    use async_trait::async_trait;
    use futures::StreamExt;
    use schemars::Schema;
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;
    use tokio::sync::Barrier;

    #[derive(Debug, Clone)]
    struct TwoToolCallProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct RepeatingTaskProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct UnterminatedProvider;

    #[derive(Debug, Clone)]
    struct FollowUpProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct RecoveringToolFailureProvider {
        requests: Arc<AtomicUsize>,
        observed_follow_up: Arc<Mutex<Option<String>>>,
    }

    #[async_trait]
    impl Provider for TwoToolCallProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        async fn stream_text(
            &self,
            _messages: &[Message],
            _tools: &[Tool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = if request == 0 {
                vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"wait","arguments":"{\"id\":1}"}},{"index":1,"id":"call_2","type":"function","function":{"name":"wait","arguments":"{\"id\":2}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End(String::new())),
                ]
            } else {
                vec![
                    Ok(ChunkType::Text("done".to_string())),
                    Ok(ChunkType::End(String::new())),
                ]
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[async_trait]
    impl Provider for RepeatingTaskProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        async fn stream_text(
            &self,
            _messages: &[Message],
            _tools: &[Tool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = match request {
                0 | 1 => vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_repeat","type":"function","function":{"name":"task","arguments":"{\"description\":\"Write haiku\",\"prompt\":\"Write a haiku\",\"subagent_type\":\"general\"}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End(String::new())),
                ],
                _ => vec![
                    Ok(ChunkType::Text("done".to_string())),
                    Ok(ChunkType::End(String::new())),
                ],
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[async_trait]
    impl Provider for UnterminatedProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        async fn stream_text(
            &self,
            _messages: &[Message],
            _tools: &[Tool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            Ok(Box::pin(futures::stream::iter(vec![Ok(ChunkType::Text(
                "still working".to_string(),
            ))])))
        }
    }

    #[async_trait]
    impl Provider for FollowUpProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        async fn stream_text(
            &self,
            _messages: &[Message],
            _tools: &[Tool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = if request == 0 {
                vec![
                    Ok(ChunkType::AssistantMessagePhase {
                        phase: Some(MessagePhase::Commentary),
                    }),
                    Ok(ChunkType::Text("I'll inspect that next.".to_string())),
                    Ok(ChunkType::ResponseCompleted {
                        end_turn: Some(false),
                    }),
                ]
            } else {
                vec![
                    Ok(ChunkType::AssistantMessagePhase {
                        phase: Some(MessagePhase::FinalAnswer),
                    }),
                    Ok(ChunkType::Text("Done.".to_string())),
                    Ok(ChunkType::ResponseCompleted {
                        end_turn: Some(true),
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[async_trait]
    impl Provider for RecoveringToolFailureProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        async fn stream_text(
            &self,
            messages: &[Message],
            _tools: &[Tool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = if request == 0 {
                vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_edit","type":"function","function":{"name":"edit","arguments":"{\"file_path\":\"src/lib.rs\",\"old_string\":\"missing\",\"new_string\":\"replacement\"}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End(String::new())),
                ]
            } else {
                let follow_up = messages
                    .last()
                    .and_then(|message| match message {
                        Message::User(user) => Some(user.content.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                *self.observed_follow_up.lock().unwrap() = Some(follow_up);

                vec![
                    Ok(ChunkType::Text("recovered".to_string())),
                    Ok(ChunkType::End(String::new())),
                ]
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[tokio::test]
    async fn executes_same_step_tool_calls_concurrently() {
        let provider = TwoToolCallProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let barrier = Arc::new(Barrier::new(2));
        let executions = Arc::new(AtomicUsize::new(0));

        let tool_barrier = barrier.clone();
        let tool_executions = executions.clone();
        let wait_tool = Tool::builder()
            .name("wait")
            .description("wait for a peer tool call")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(move |_input| {
                let barrier = tool_barrier.clone();
                let executions = tool_executions.clone();
                async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    barrier.wait().await;
                    Ok("ok".to_string())
                }
            }))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider,
            vec![Message::user("run both")],
            vec![wait_tool],
            None,
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let saw_done = tokio::time::timeout(Duration::from_secs(1), async {
            let mut saw_done = false;
            while let Some(chunk) = response.stream.next().await {
                if let ChunkType::Text(text) = chunk {
                    saw_done |= text == "done";
                }
            }
            saw_done
        })
        .await
        .expect("tool calls in the same step should not run serially");

        assert!(saw_done);
        assert_eq!(executions.load(Ordering::SeqCst), 2);

        let observations = response
            .messages()
            .await
            .into_iter()
            .filter_map(|message| match message {
                Message::User(user) if user.content.starts_with("Tool batch results:") => {
                    Some(user.content)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(observations.len(), 1);
        assert!(observations[0].contains("call_1"));
        assert!(observations[0].contains("call_2"));
    }

    #[tokio::test]
    async fn skips_exact_repeated_task_call_in_same_response() {
        let provider = RepeatingTaskProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let executions = Arc::new(AtomicUsize::new(0));

        let tool_executions = executions.clone();
        let task_tool = Tool::builder()
            .name("task")
            .description("launch subagent")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(move |_input| {
                let executions = tool_executions.clone();
                async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    Ok("subagent result".to_string())
                }
            }))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider,
            vec![Message::user("run task")],
            vec![task_tool],
            None,
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let mut saw_done = false;
        while let Some(chunk) = response.stream.next().await {
            if let ChunkType::Text(text) = chunk {
                saw_done |= text == "done";
            }
        }

        assert!(saw_done);
        assert_eq!(executions.load(Ordering::SeqCst), 1);

        let observations = response
            .messages()
            .await
            .into_iter()
            .filter_map(|message| match message {
                Message::User(user) if user.content.contains("Duplicate task call skipped") => {
                    Some(user.content)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(observations.len(), 1);
    }

    #[tokio::test]
    async fn stream_without_terminal_event_fails() {
        let mut response = stream_with_tools(
            UnterminatedProvider,
            vec![Message::user("work")],
            Vec::new(),
            None,
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let mut chunks = Vec::new();
        while let Some(chunk) = response.stream.next().await {
            chunks.push(chunk);
        }

        assert!(chunks.iter().any(|chunk| matches!(
            chunk,
            ChunkType::Failed(message)
                if message.contains("without a terminal completion event")
        )));
        assert!(matches!(
            response.stop_reason().await,
            Some(StopReason::Error(message))
                if message.contains("without a terminal completion event")
        ));
    }

    #[tokio::test]
    async fn continues_when_provider_marks_response_as_non_final() {
        let provider = FollowUpProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };

        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("finish the task")],
            Vec::new(),
            Some(3),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let mut text = String::new();
        while let Some(chunk) = response.stream.next().await {
            if let ChunkType::Text(delta) = chunk {
                text.push_str(&delta);
            }
        }

        assert_eq!(text, "I'll inspect that next.Done.");
        assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
        assert_eq!(response.stop_reason().await, Some(StopReason::Finish));
    }

    #[tokio::test]
    async fn tool_execution_error_is_returned_to_model_without_failing_stream() {
        let observed_follow_up = Arc::new(Mutex::new(None));
        let provider = RecoveringToolFailureProvider {
            requests: Arc::new(AtomicUsize::new(0)),
            observed_follow_up: observed_follow_up.clone(),
        };

        let edit_tool = Tool::builder()
            .name("edit")
            .description("edit files")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(move |_input| async move {
                Err("Execution error: Not found: Could not find text to replace".to_string())
            }))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("make the edit")],
            vec![edit_tool],
            Some(3),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let mut text = String::new();
        let mut failed_chunks = Vec::new();
        while let Some(chunk) = response.stream.next().await {
            match chunk {
                ChunkType::Text(delta) => text.push_str(&delta),
                ChunkType::Failed(err) => failed_chunks.push(err),
                _ => {}
            }
        }

        assert_eq!(text, "recovered");
        assert!(failed_chunks.is_empty());
        assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
        assert_eq!(response.stop_reason().await, Some(StopReason::Finish));

        let follow_up = observed_follow_up
            .lock()
            .unwrap()
            .clone()
            .expect("provider should receive failed tool observation");
        assert!(follow_up.contains("Tool `edit` failed"));
        assert!(follow_up.contains("Could not find text to replace"));
        assert!(follow_up.contains("Do not repeat the same tool call unchanged"));
    }

    #[test]
    fn accumulates_streamed_openai_tool_call_arguments() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]"#,
            )
            .unwrap();
        accumulator
            .ingest(r#"[{"index":0,"function":{"arguments":":\"ls -la\"}"}}]"#)
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "call_1");
        assert_eq!(calls[0].1, "bash");
        assert_eq!(calls[0].2["command"], "ls -la");
    }

    #[test]
    fn rejects_incomplete_tool_call_arguments() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]"#,
            )
            .unwrap();

        let error = accumulator.finish().unwrap_err();

        assert!(error.contains("arguments are incomplete or invalid JSON"));
    }

    #[test]
    fn supports_multiple_tool_calls_by_index() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"file_path\""}},{"index":1,"id":"call_2","type":"function","function":{"name":"bash","arguments":"{\"command\""}}]"#,
            )
            .unwrap();
        accumulator
            .ingest(
                r#"[{"index":0,"function":{"arguments":":\"Cargo.toml\"}"}},{"index":1,"function":{"arguments":":\"cargo test\"}"}}]"#,
            )
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, "read");
        assert_eq!(calls[0].2["file_path"], "Cargo.toml");
        assert_eq!(calls[1].1, "bash");
        assert_eq!(calls[1].2["command"], "cargo test");
    }

    #[test]
    fn empty_arguments_become_empty_object() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"list","arguments":""}}]"#,
            )
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls[0].2, serde_json::json!({}));
    }

    #[test]
    fn uses_final_arguments_when_delta_arguments_are_absent() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"read"}}]"#)
            .unwrap();
        accumulator
            .ingest(
                r#"[{"index":0,"function":{"arguments_done":"{\"file_path\":\"Cargo.toml\"}"}}]"#,
            )
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "read");
        assert_eq!(calls[0].2["file_path"], "Cargo.toml");
    }
}
