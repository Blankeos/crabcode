use crate::chunk::{ChunkType, FinishReason, MessagePhase};
use crate::error::{Error, Result};
use crate::message::Message;
use crate::provider::{Provider, ProviderStream};
use crate::retry::RetryError;
use crate::stop::{StopReason, StopWhenFn};
use crate::tool::{HostedTool, Tool, ToolOutput};
use futures::{future::join_all, StreamExt};
use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

const PHASELESS_AMBIGUOUS_FOLLOW_UP_LIMIT: usize = 1;
const PROVIDER_STEP_MAX_RETRIES: usize = 10;

/// Keep the newest N tool outputs intact for the model; older ones are pruned.
/// Matches Grok Build / OpenCode mid-session tool-result retention behavior.
const KEEP_RECENT_TOOL_OUTPUTS: usize = 6;
/// Soft-trim threshold for older-but-still-retained tool outputs (chars).
const TOOL_OUTPUT_SOFT_TRIM_CHARS: usize = 4_000;
const TOOL_OUTPUT_SOFT_TRIM_HEAD: usize = 1_500;
const TOOL_OUTPUT_SOFT_TRIM_TAIL: usize = 1_500;
const PRUNED_TOOL_OUTPUT_PLACEHOLDER: &str = "[Old tool result content cleared]";

pub struct StreamTextResponse {
    pub stream: LanguageModelStream,
    stop_reason: Arc<tokio::sync::Mutex<Option<StopReason>>>,
    messages: Arc<tokio::sync::Mutex<Vec<Message>>>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for StreamTextResponse {
    /// Safety net: abort the background provider/tool loop if the consumer drops
    /// the response without draining the stream (e.g. cancellation or early exit).
    fn drop(&mut self) {
        for handle in &self._handles {
            handle.abort();
        }
    }
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
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<StreamTextResponse> {
    stream_with_hosted_tools(
        provider,
        messages,
        tools,
        Vec::new(),
        max_steps,
        stop_when,
        headers,
        cancel_token,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_with_hosted_tools<P: Provider>(
    provider: P,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    hosted_tools: Vec<HostedTool>,
    max_steps: Option<usize>,
    stop_when: Option<StopWhenFn>,
    headers: HashMap<String, String>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
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
        let mut cached_repeatable_tool_results: HashMap<String, ToolOutput> = HashMap::new();
        let mut phase_less_ambiguous_follow_ups = 0usize;

        loop {
            step_idx += 1;

            if step_idx > max_steps {
                let _ = tx_loop.send(ChunkType::Incomplete("Max steps reached".to_string()));
                *stop_reason_arc.lock().await = Some(StopReason::Hook);
                break;
            }

            if cancel_token
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                let err = "Streaming cancelled by user".to_string();
                let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                break;
            }

            if let Some(ref hook) = stop_when {
                if hook(step_idx) {
                    *stop_reason_arc.lock().await = Some(StopReason::Hook);
                    break;
                }
            }

            let step_summary = provider_step_log_summary(&current_messages, &tools);
            let pruned = prune_stale_tool_outputs_in_place(&mut current_messages);
            if pruned > 0 {
                let _ = tx_loop.send(ChunkType::Metadata(format!(
                    "tool_outputs_pruned count={} keep_recent={}",
                    pruned, KEEP_RECENT_TOOL_OUTPUTS
                )));
            }
            let _ = tx_loop.send(ChunkType::Metadata(format!(
                "provider_step_start step={} messages={} tools={} {}",
                step_idx,
                current_messages.len(),
                tools.len(),
                step_summary
            )));

            let mut attempt = 1usize;
            let mut stream = match open_provider_stream_with_retries(
                &provider_clone,
                &current_messages,
                &tools,
                &hosted_tools,
                &headers,
                &tx_loop,
                step_idx,
                &mut attempt,
                cancel_token.as_ref(),
            )
            .await
            {
                Ok(stream) => stream,
                Err(error) => {
                    let err = match error {
                        Error::Provider(message) if message == "Streaming cancelled by user" => {
                            message
                        }
                        error => provider_step_error_message(
                            step_idx,
                            current_messages.len(),
                            tools.len(),
                            &step_summary,
                            error,
                        ),
                    };
                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                    return;
                }
            };

            let mut has_tool_call = false;
            let mut tool_call_accumulator = ToolCallAccumulator::default();
            let mut accumulated_text = String::new();
            let mut accumulated_reasoning = String::new();
            let mut saw_terminal_event = false;
            let mut response_end_turn = None;
            let mut provider_finish_reason = None;
            let mut last_assistant_message_phase = None;
            let mut current_assistant_message_phase = None;
            let mut emitted_non_replayable_output = false;

            loop {
                let next_chunk = if let Some(token) = cancel_token.as_ref() {
                    tokio::select! {
                        _ = token.cancelled() => {
                            let err = "Streaming cancelled by user".to_string();
                            let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                            *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                            return;
                        }
                        chunk = stream.next() => chunk,
                    }
                } else {
                    stream.next().await
                };
                let Some(chunk) = next_chunk else { break };
                match chunk {
                    Ok(ChunkType::AssistantMessagePhase { phase }) => {
                        current_assistant_message_phase = phase;
                        last_assistant_message_phase = phase;
                        let label = message_phase_label(phase);
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
                        emitted_non_replayable_output = true;
                        last_assistant_message_phase = current_assistant_message_phase;
                        accumulated_text.push_str(&text);
                        let _ = tx_loop.send(ChunkType::Text(text));
                    }
                    Ok(ChunkType::Reasoning(reasoning)) => {
                        emitted_non_replayable_output = true;
                        accumulated_reasoning.push_str(&reasoning);
                        let _ = tx_loop.send(ChunkType::Reasoning(reasoning));
                    }
                    Ok(ChunkType::ToolCall(json_str)) => {
                        emitted_non_replayable_output = true;
                        has_tool_call = true;
                        let _ = tx_loop.send(ChunkType::ToolCall(json_str.clone()));
                        if let Err(err) = tool_call_accumulator.ingest(&json_str) {
                            let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                            *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                            return;
                        }
                    }
                    Ok(ChunkType::End { reason }) => {
                        // Processed internally — NOT forwarded to tx_loop.
                        // Forwarding End would cause relay_stream_to_sender
                        // to return Ended prematurely, dropping the channel
                        // before tool execution / subsequent steps.
                        saw_terminal_event = true;
                        if let Some(reason) = reason {
                            let label = reason.label().to_string();
                            provider_finish_reason = Some(reason);
                            let _ = tx_loop.send(ChunkType::Metadata(format!(
                                "provider_finish_reason={label}"
                            )));
                        }
                    }
                    Ok(ChunkType::Metadata(msg)) => {
                        let _ = tx_loop.send(ChunkType::Metadata(msg));
                    }
                    Ok(ChunkType::Warning(msg)) => {
                        let _ = tx_loop.send(ChunkType::Warning(msg));
                    }
                    Ok(ChunkType::Retry(status)) => {
                        let _ = tx_loop.send(ChunkType::Retry(status));
                    }
                    Ok(ChunkType::StreamRollback { .. }) => {}
                    Ok(ChunkType::RetryableFailure(retry_error)) => {
                        if attempt <= PROVIDER_STEP_MAX_RETRIES {
                            rollback_provider_attempt(
                                &tx_loop,
                                &mut accumulated_text,
                                &mut accumulated_reasoning,
                                &mut has_tool_call,
                                &mut tool_call_accumulator,
                                &mut saw_terminal_event,
                                &mut response_end_turn,
                                &mut provider_finish_reason,
                                &mut last_assistant_message_phase,
                                &mut current_assistant_message_phase,
                                &mut emitted_non_replayable_output,
                            );
                            if !emit_retry_and_sleep(
                                &tx_loop,
                                step_idx,
                                attempt,
                                &retry_error,
                                cancel_token.as_ref(),
                            )
                            .await
                            {
                                let err = "Streaming cancelled by user".to_string();
                                let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                                *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                                return;
                            }
                            attempt += 1;
                            stream = match open_provider_stream_with_retries(
                                &provider_clone,
                                &current_messages,
                                &tools,
                                &hosted_tools,
                                &headers,
                                &tx_loop,
                                step_idx,
                                &mut attempt,
                                cancel_token.as_ref(),
                            )
                            .await
                            {
                                Ok(stream) => stream,
                                Err(error) => {
                                    let err = provider_step_error_message(
                                        step_idx,
                                        current_messages.len(),
                                        tools.len(),
                                        &step_summary,
                                        error,
                                    );
                                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                                    return;
                                }
                            };
                            continue;
                        }

                        let err = format!(
                            "Provider stream failed after {} retries: {}",
                            PROVIDER_STEP_MAX_RETRIES, retry_error.message
                        );
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                    Ok(ChunkType::Incomplete(msg)) => {
                        let err = format!("Provider response incomplete: {}", msg);
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                    Ok(ChunkType::Failed(err)) => {
                        let retry_error = RetryError::from_message(err.clone());
                        if crate::retry::retryable(&retry_error)
                            && attempt <= PROVIDER_STEP_MAX_RETRIES
                        {
                            rollback_provider_attempt(
                                &tx_loop,
                                &mut accumulated_text,
                                &mut accumulated_reasoning,
                                &mut has_tool_call,
                                &mut tool_call_accumulator,
                                &mut saw_terminal_event,
                                &mut response_end_turn,
                                &mut provider_finish_reason,
                                &mut last_assistant_message_phase,
                                &mut current_assistant_message_phase,
                                &mut emitted_non_replayable_output,
                            );
                            if !emit_retry_and_sleep(
                                &tx_loop,
                                step_idx,
                                attempt,
                                &retry_error,
                                cancel_token.as_ref(),
                            )
                            .await
                            {
                                let err = "Streaming cancelled by user".to_string();
                                let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                                *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                                return;
                            }
                            attempt += 1;
                            stream = match open_provider_stream_with_retries(
                                &provider_clone,
                                &current_messages,
                                &tools,
                                &hosted_tools,
                                &headers,
                                &tx_loop,
                                step_idx,
                                &mut attempt,
                                cancel_token.as_ref(),
                            )
                            .await
                            {
                                Ok(stream) => stream,
                                Err(error) => {
                                    let err = provider_step_error_message(
                                        step_idx,
                                        current_messages.len(),
                                        tools.len(),
                                        &step_summary,
                                        error,
                                    );
                                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                                    return;
                                }
                            };
                            continue;
                        }

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
                    Err(e) => match retry_error_from_provider_error(e) {
                        Ok(retry_error) if attempt <= PROVIDER_STEP_MAX_RETRIES => {
                            rollback_provider_attempt(
                                &tx_loop,
                                &mut accumulated_text,
                                &mut accumulated_reasoning,
                                &mut has_tool_call,
                                &mut tool_call_accumulator,
                                &mut saw_terminal_event,
                                &mut response_end_turn,
                                &mut provider_finish_reason,
                                &mut last_assistant_message_phase,
                                &mut current_assistant_message_phase,
                                &mut emitted_non_replayable_output,
                            );
                            if !emit_retry_and_sleep(
                                &tx_loop,
                                step_idx,
                                attempt,
                                &retry_error,
                                cancel_token.as_ref(),
                            )
                            .await
                            {
                                let err = "Streaming cancelled by user".to_string();
                                let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                                *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                                return;
                            }
                            attempt += 1;
                            stream = match open_provider_stream_with_retries(
                                &provider_clone,
                                &current_messages,
                                &tools,
                                &hosted_tools,
                                &headers,
                                &tx_loop,
                                step_idx,
                                &mut attempt,
                                cancel_token.as_ref(),
                            )
                            .await
                            {
                                Ok(stream) => stream,
                                Err(error) => {
                                    let err = provider_step_error_message(
                                        step_idx,
                                        current_messages.len(),
                                        tools.len(),
                                        &step_summary,
                                        error,
                                    );
                                    let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                                    *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                                    return;
                                }
                            };
                            continue;
                        }
                        Ok(retry_error) => {
                            let err = format!(
                                "Provider stream failed after {} retries: {}",
                                PROVIDER_STEP_MAX_RETRIES, retry_error.message
                            );
                            let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                            *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                            return;
                        }
                        Err(error) => {
                            let err = error.to_string();
                            let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                            *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                            return;
                        }
                    },
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
                let end_turn_requires_follow_up = matches!(response_end_turn, Some(false));
                let commentary_requires_follow_up =
                    matches!(last_assistant_message_phase, Some(MessagePhase::Commentary));
                let phase_less_ambiguous_requires_follow_up = !tools.is_empty()
                    && response_end_turn.is_none()
                    && last_assistant_message_phase.is_none()
                    && phase_less_ambiguous_follow_ups < PHASELESS_AMBIGUOUS_FOLLOW_UP_LIMIT
                    && provider_finish_reason
                        .as_ref()
                        .is_none_or(|reason| !reason.is_final_assistant_stop());
                let needs_follow_up = end_turn_requires_follow_up
                    || commentary_requires_follow_up
                    || phase_less_ambiguous_requires_follow_up;
                let action = if needs_follow_up {
                    "continue"
                } else {
                    "finish"
                };
                let _ = tx_loop.send(ChunkType::Metadata(format!(
                    "provider_step_finish step={} has_tool_call=false end_turn={:?} provider_finish_reason={} last_phase={} assistant_text_chars={} action={} preview={:?}",
                    step_idx,
                    response_end_turn,
                    provider_finish_reason
                        .as_ref()
                        .map(FinishReason::label)
                        .unwrap_or("unknown"),
                    message_phase_label(last_assistant_message_phase),
                    assistant_text.len(),
                    action,
                    log_preview(&assistant_text, 160)
                )));

                if needs_follow_up {
                    let reason = if end_turn_requires_follow_up {
                        "end_turn=false"
                    } else if phase_less_ambiguous_requires_follow_up {
                        phase_less_ambiguous_follow_ups += 1;
                        "phase_less_terminal_without_final_signal"
                    } else {
                        "assistant_message_phase=commentary"
                    };
                    let _ = tx_loop.send(ChunkType::Metadata(format!(
                        "continuing model turn after non-final assistant output step={} reason={}",
                        step_idx, reason
                    )));
                    continue;
                }
                *stop_reason_arc.lock().await = Some(StopReason::Finish);
                break;
            }

            phase_less_ambiguous_follow_ups = 0;

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
            let mut tool_call_messages = Vec::new();

            for tool_call in tool_calls_to_execute {
                let call_id = tool_call.call_id;
                let tool_name = tool_call.name;
                let args = tool_call.arguments;
                let arguments = canonical_json(&args);
                let tool_call_message = if accumulated_reasoning.is_empty() {
                    if let Some(item_id) = tool_call.item_id {
                        Message::tool_call_with_item_id(
                            item_id,
                            call_id.clone(),
                            tool_name.clone(),
                            arguments,
                        )
                    } else {
                        Message::tool_call(call_id.clone(), tool_name.clone(), arguments)
                    }
                } else if let Some(item_id) = tool_call.item_id {
                    Message::tool_call_with_item_id_and_reasoning(
                        item_id,
                        call_id.clone(),
                        tool_name.clone(),
                        arguments,
                        accumulated_reasoning.clone(),
                    )
                } else {
                    Message::tool_call_with_reasoning(
                        call_id.clone(),
                        tool_name.clone(),
                        arguments,
                        accumulated_reasoning.clone(),
                    )
                };
                current_messages.push(tool_call_message.clone());
                tool_call_messages.push(tool_call_message);

                let cache_key = repeatable_tool_cache_key(&tool_name, &args);
                if let Some(cached_output) = cache_key
                    .as_ref()
                    .and_then(|key| cached_repeatable_tool_results.get(key))
                    .cloned()
                {
                    tool_results_to_observe.push(ToolExecutionResult {
                        call_id,
                        tool_name,
                        output: ToolOutput::new(format!(
                            "Duplicate task call skipped; reusing the prior result from this response.\n\n{}",
                            cached_output.text
                        )),
                        cache_key: None,
                        is_error: false,
                    });
                } else {
                    tool_calls_to_run.push((call_id, tool_name, args, cache_key));
                }
            }

            if !tool_call_messages.is_empty() {
                messages_arc.lock().await.extend(tool_call_messages);
            }

            let tool_work = join_all(tool_calls_to_run.into_iter().map(
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
                                    output: ToolOutput::new(format!(
                                        "Tool '{}' error: {}",
                                        tool_name, err
                                    )),
                                    cache_key: None,
                                    is_error: true,
                                },
                            },
                            None => ToolExecutionResult {
                                call_id,
                                tool_name: tool_name.clone(),
                                output: ToolOutput::new(format!("Tool not found: {}", tool_name)),
                                cache_key: None,
                                is_error: true,
                            },
                        }
                    }
                },
            ));
            let tool_results = if let Some(token) = cancel_token.as_ref() {
                tokio::select! {
                    _ = token.cancelled() => {
                        let err = "Tool execution cancelled by user".to_string();
                        let _ = tx_loop.send(ChunkType::Failed(err.clone()));
                        *stop_reason_arc.lock().await = Some(StopReason::Error(err));
                        return;
                    }
                    results = tool_work => results,
                }
            } else {
                tool_work.await
            };

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
                let tool_names = tool_results_to_observe
                    .iter()
                    .map(|result| result.tool_name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                let tool_result_summary = tool_results_log_summary(&tool_results_to_observe);
                let _ = tx_loop.send(ChunkType::Metadata(format!(
                    "tool_results_added count={} names={} {} next_messages={}",
                    tool_results_to_observe.len(),
                    tool_names,
                    tool_result_summary,
                    current_messages.len() + tool_results_to_observe.len()
                )));
                let tool_output_messages = tool_results_to_observe
                    .into_iter()
                    .map(|result| {
                        Message::tool_output_with_images(
                            result.call_id,
                            result.tool_name,
                            result.output.text,
                            result.output.images,
                            result.is_error,
                        )
                    })
                    .collect::<Vec<_>>();
                current_messages.extend(tool_output_messages.clone());
                messages_arc.lock().await.extend(tool_output_messages);
            }
        }
    });

    response.add_handle(handle);
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
fn rollback_provider_attempt(
    tx: &mpsc::UnboundedSender<ChunkType>,
    accumulated_text: &mut String,
    accumulated_reasoning: &mut String,
    has_tool_call: &mut bool,
    tool_call_accumulator: &mut ToolCallAccumulator,
    saw_terminal_event: &mut bool,
    response_end_turn: &mut Option<bool>,
    provider_finish_reason: &mut Option<FinishReason>,
    last_assistant_message_phase: &mut Option<MessagePhase>,
    current_assistant_message_phase: &mut Option<MessagePhase>,
    emitted_non_replayable_output: &mut bool,
) {
    if !accumulated_text.is_empty() || !accumulated_reasoning.is_empty() {
        let _ = tx.send(ChunkType::StreamRollback {
            text: std::mem::take(accumulated_text),
            reasoning: std::mem::take(accumulated_reasoning),
        });
    } else {
        accumulated_text.clear();
        accumulated_reasoning.clear();
    }
    *has_tool_call = false;
    *tool_call_accumulator = ToolCallAccumulator::default();
    *saw_terminal_event = false;
    *response_end_turn = None;
    *provider_finish_reason = None;
    *last_assistant_message_phase = None;
    *current_assistant_message_phase = None;
    *emitted_non_replayable_output = false;
}

/// Prune older tool outputs in the live multi-step transcript before each
/// provider request. Durable UI/history copies are left intact — this only
/// mutates the request-facing message list.
///
/// Strategy (Grok Build / OpenCode inspired):
/// - Keep the newest [`KEEP_RECENT_TOOL_OUTPUTS`] results full-size
/// - Soft-trim large older results to head+tail
/// - Hard-clear anything older than 2× the keep window to a placeholder
/// - Drop attached images on pruned outputs (base64 is extremely expensive)
fn prune_stale_tool_outputs_in_place(messages: &mut [Message]) -> usize {
    let tool_output_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(idx, message)| matches!(message, Message::ToolOutput(_)).then_some(idx))
        .collect();

    if tool_output_indices.is_empty() {
        return 0;
    }

    let keep_from = tool_output_indices
        .len()
        .saturating_sub(KEEP_RECENT_TOOL_OUTPUTS);
    let hard_clear_before = tool_output_indices
        .len()
        .saturating_sub(KEEP_RECENT_TOOL_OUTPUTS.saturating_mul(2));
    let mut pruned = 0usize;

    for (rank, &idx) in tool_output_indices.iter().enumerate() {
        if rank >= keep_from {
            continue;
        }

        let Message::ToolOutput(output) = &mut messages[idx] else {
            continue;
        };

        let had_images = !output.images.is_empty();
        let original_len = output.output.len();
        if rank < hard_clear_before {
            if original_len > PRUNED_TOOL_OUTPUT_PLACEHOLDER.len() || had_images {
                output.output = PRUNED_TOOL_OUTPUT_PLACEHOLDER.to_string();
                output.images.clear();
                pruned += 1;
            }
            continue;
        }

        let trimmed = soft_trim_tool_output(&output.output);
        if trimmed.len() < original_len || had_images {
            output.output = trimmed;
            output.images.clear();
            pruned += 1;
        }
    }

    pruned
}

fn soft_trim_tool_output(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= TOOL_OUTPUT_SOFT_TRIM_CHARS {
        return text.to_string();
    }

    let head: String = text.chars().take(TOOL_OUTPUT_SOFT_TRIM_HEAD).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(TOOL_OUTPUT_SOFT_TRIM_TAIL)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let omitted = char_count
        .saturating_sub(TOOL_OUTPUT_SOFT_TRIM_HEAD)
        .saturating_sub(TOOL_OUTPUT_SOFT_TRIM_TAIL);
    format!("{head}\n\n...[{omitted} chars truncated]...\n\n{tail}")
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
            Message::ToolCall(_) | Message::ToolOutput(_) => {}
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
        Message::ToolCall(_) => "tool_call",
        Message::ToolOutput(_) => "tool_output",
    }
}

fn message_size(message: &Message) -> (usize, usize) {
    match message {
        Message::System(message) => (message.content.len(), 0),
        Message::User(message) => (message.content.len(), message.images.len()),
        Message::Assistant(message) => (message.content.len(), 0),
        Message::ToolCall(message) => (message.arguments.len(), 0),
        Message::ToolOutput(message) => (message.output.len(), message.images.len()),
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

fn message_phase_label(phase: Option<MessagePhase>) -> &'static str {
    match phase {
        Some(MessagePhase::Commentary) => "commentary",
        Some(MessagePhase::FinalAnswer) => "final_answer",
        None => "unknown",
    }
}

fn log_preview(text: &str, max_chars: usize) -> String {
    let mut preview = String::new();
    let mut chars = 0usize;
    let mut previous_was_whitespace = false;

    for ch in text.trim().chars() {
        if chars >= max_chars {
            preview.push_str("...");
            break;
        }

        if ch.is_whitespace() {
            if !previous_was_whitespace && !preview.is_empty() {
                preview.push(' ');
                chars += 1;
            }
            previous_was_whitespace = true;
        } else {
            preview.push(ch);
            chars += 1;
            previous_was_whitespace = false;
        }
    }

    preview
}

fn retry_error_from_provider_error(error: Error) -> std::result::Result<RetryError, Error> {
    match error {
        Error::RetryableProvider(retry_error) if crate::retry::retryable(&retry_error) => {
            Ok(retry_error)
        }
        Error::Provider(message) => {
            let retry_error = RetryError::from_message(message);
            if crate::retry::retryable(&retry_error) {
                Ok(retry_error)
            } else {
                Err(Error::Provider(retry_error.message))
            }
        }
        Error::Http(error) => Ok(RetryError::from_message(error.to_string())),
        other => Err(other),
    }
}

async fn open_provider_stream_with_retries<P: Provider>(
    provider: &P,
    messages: &[Message],
    tools: &[Tool],
    hosted_tools: &[HostedTool],
    headers: &HashMap<String, String>,
    tx: &mpsc::UnboundedSender<ChunkType>,
    step_idx: usize,
    attempt: &mut usize,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> std::result::Result<ProviderStream, Error> {
    loop {
        let stream_result = if let Some(token) = cancel_token {
            tokio::select! {
                _ = token.cancelled() => {
                    return Err(Error::Provider("Streaming cancelled by user".to_string()));
                }
                result = provider.stream_text(messages, tools, hosted_tools, headers) => result,
            }
        } else {
            provider
                .stream_text(messages, tools, hosted_tools, headers)
                .await
        };
        match stream_result {
            Ok(stream) => return Ok(stream),
            Err(error) => match retry_error_from_provider_error(error) {
                Ok(retry_error) if *attempt <= PROVIDER_STEP_MAX_RETRIES => {
                    if !emit_retry_and_sleep(tx, step_idx, *attempt, &retry_error, cancel_token)
                        .await
                    {
                        return Err(Error::Provider("Streaming cancelled by user".to_string()));
                    }
                    *attempt += 1;
                }
                Ok(retry_error) => {
                    return Err(Error::Provider(format!(
                        "{} retries_exhausted={}",
                        retry_error.message, PROVIDER_STEP_MAX_RETRIES
                    )));
                }
                Err(error) => return Err(error),
            },
        }
    }
}

fn provider_step_error_message(
    step_idx: usize,
    message_count: usize,
    tool_count: usize,
    step_summary: &str,
    error: Error,
) -> String {
    format!(
        "provider_step_error step={} messages={} tools={} {} error={}",
        step_idx, message_count, tool_count, step_summary, error
    )
}

async fn emit_retry_and_sleep(
    tx: &mpsc::UnboundedSender<ChunkType>,
    step_idx: usize,
    attempt: usize,
    retry_error: &RetryError,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
) -> bool {
    let status = crate::retry::status_for_attempt(retry_error, attempt);
    let delay = std::time::Duration::from_millis(status.delay_ms);
    let _ = tx.send(ChunkType::Metadata(format!(
        "provider_step_retry step={} attempt={} delay_ms={} next_epoch_ms={} error={}",
        step_idx, status.attempt, status.delay_ms, status.next_epoch_ms, status.message,
    )));
    let _ = tx.send(ChunkType::Retry(status));
    if let Some(cancel_token) = cancel_token {
        tokio::select! {
            _ = cancel_token.cancelled() => false,
            _ = tokio::time::sleep(delay) => true,
        }
    } else {
        tokio::time::sleep(delay).await;
        true
    }
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    calls: Vec<PendingToolCall>,
}

#[derive(Debug)]
struct PendingToolCall {
    key: String,
    id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
    final_arguments: Option<String>,
    saw_arguments: bool,
}

#[derive(Debug)]
struct CompletedToolCall {
    item_id: Option<String>,
    call_id: String,
    name: String,
    arguments: serde_json::Value,
}

#[derive(Debug)]
struct ToolExecutionResult {
    call_id: String,
    tool_name: String,
    output: ToolOutput,
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

    fn finish(self) -> std::result::Result<Vec<CompletedToolCall>, String> {
        let mut results = Vec::new();

        for call in self.calls {
            let name = call
                .name
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("Tool call '{}' missing function name", call.key))?;

            let item_id = call.id.or_else(|| Some(call.key.clone()));
            let call_id = call
                .call_id
                .clone()
                .or_else(|| item_id.clone())
                .unwrap_or_else(|| call.key.clone());
            let args =
                parse_tool_arguments(&call_id, &call.arguments, call.final_arguments.as_deref())?;

            results.push(CompletedToolCall {
                item_id,
                call_id,
                name,
                arguments: args,
            });
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
        if pending.call_id.is_none() {
            pending.call_id = item
                .get("call_id")
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
            call_id: None,
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
    use super::{
        prune_stale_tool_outputs_in_place, soft_trim_tool_output, stream_with_tools,
        ToolCallAccumulator, KEEP_RECENT_TOOL_OUTPUTS, PRUNED_TOOL_OUTPUT_PLACEHOLDER,
        TOOL_OUTPUT_SOFT_TRIM_CHARS,
    };
    use crate::chunk::{ChunkType, FinishReason, MessagePhase};
    use crate::message::Message;
    use crate::provider::{Provider, ProviderStream};
    use crate::stop::StopReason;
    use crate::tool::{HostedTool, Tool, ToolExecute};
    use async_trait::async_trait;
    use futures::StreamExt;
    use schemars::Schema;
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;
    use tokio::sync::Barrier;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn soft_trim_tool_output_keeps_short_text() {
        assert_eq!(soft_trim_tool_output("short"), "short");
    }

    #[test]
    fn soft_trim_tool_output_keeps_head_and_tail() {
        let text = "a".repeat(TOOL_OUTPUT_SOFT_TRIM_CHARS + 500);
        let trimmed = soft_trim_tool_output(&text);
        assert!(trimmed.len() < text.len());
        assert!(trimmed.starts_with('a'));
        assert!(trimmed.ends_with('a'));
        assert!(trimmed.contains("chars truncated"));
    }

    #[test]
    fn prune_stale_tool_outputs_clears_oldest_and_keeps_recent() {
        let mut messages = Vec::new();
        let total = KEEP_RECENT_TOOL_OUTPUTS * 2 + 2;
        for i in 0..total {
            messages.push(Message::tool_output(
                format!("call_{i}"),
                "bash",
                "x".repeat(TOOL_OUTPUT_SOFT_TRIM_CHARS + 200),
                false,
            ));
        }

        let pruned = prune_stale_tool_outputs_in_place(&mut messages);
        assert!(pruned > 0);

        let outputs: Vec<_> = messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolOutput(output) => Some(output.output.as_str()),
                _ => None,
            })
            .collect();

        assert_eq!(outputs[0], PRUNED_TOOL_OUTPUT_PLACEHOLDER);
        let last = outputs.last().expect("output");
        assert_eq!(last.len(), TOOL_OUTPUT_SOFT_TRIM_CHARS + 200);
    }

    #[derive(Debug, Clone)]
    struct BlockingTextProvider;

    #[derive(Debug, Clone)]
    struct NeverEndingProvider {
        polled: Arc<AtomicUsize>,
        stream_dropped: Arc<AtomicBool>,
    }

    struct NeverEndingStream {
        polled: Arc<AtomicUsize>,
        stream_dropped: Arc<AtomicBool>,
    }

    impl Drop for NeverEndingStream {
        fn drop(&mut self) {
            self.stream_dropped.store(true, Ordering::SeqCst);
        }
    }

    impl futures::Stream for NeverEndingStream {
        type Item = crate::error::Result<ChunkType>;

        fn poll_next(
            self: Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            self.polled.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Pending
        }
    }

    #[async_trait]
    impl Provider for NeverEndingProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            Ok(Box::pin(NeverEndingStream {
                polled: self.polled.clone(),
                stream_dropped: self.stream_dropped.clone(),
            }))
        }
    }

    #[derive(Debug, Clone)]
    struct BlockingStartupProvider;

    #[async_trait]
    impl Provider for BlockingStartupProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            std::future::pending::<()>().await;
            unreachable!("stream_text should not return while blocked")
        }
    }

    #[async_trait]
    impl Provider for BlockingTextProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    #[derive(Debug, Clone)]
    struct TwoToolCallProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct ReasoningToolCallProvider {
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
    struct PhaselessAmbiguousProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct PhaselessFinalProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct RecoveringToolFailureProvider {
        requests: Arc<AtomicUsize>,
        observed_follow_up: Arc<Mutex<Option<String>>>,
    }

    #[derive(Debug, Clone)]
    struct RecoveringRateLimitProvider {
        requests: Arc<AtomicUsize>,
    }

    #[derive(Debug, Clone)]
    struct RecoveringPartialStreamProvider {
        requests: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for RecoveringRateLimitProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            if request == 0 {
                let retry_error = crate::retry::RetryError {
                    message: "Too Many Requests".to_string(),
                    status: Some(429),
                    headers: HashMap::from([("retry-after-ms".to_string(), "0".to_string())]),
                    replay_safe: true,
                };
                return Err(crate::error::Error::RetryableProvider(retry_error));
            }

            Ok(Box::pin(futures::stream::iter(vec![
                Ok(ChunkType::Text("recovered".to_string())),
                Ok(ChunkType::End {
                    reason: Some(FinishReason::Stop),
                }),
            ])))
        }
    }

    #[async_trait]
    impl Provider for RecoveringPartialStreamProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            if request == 0 {
                return Ok(Box::pin(futures::stream::iter(vec![
                    Ok(ChunkType::Text("partial".to_string())),
                    Ok(ChunkType::Reasoning("thinking".to_string())),
                    Ok(ChunkType::RetryableFailure(crate::retry::RetryError {
                        message: "websocket closed before response.completed".to_string(),
                        status: None,
                        headers: HashMap::from([("retry-after-ms".to_string(), "0".to_string())]),
                        replay_safe: true,
                    })),
                ])));
            }

            Ok(Box::pin(futures::stream::iter(vec![
                Ok(ChunkType::Text("recovered".to_string())),
                Ok(ChunkType::End {
                    reason: Some(FinishReason::Stop),
                }),
            ])))
        }
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = if request == 0 {
                vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_1","type":"function","function":{"name":"wait","arguments":"{\"id\":1}"}},{"index":1,"id":"call_2","type":"function","function":{"name":"wait","arguments":"{\"id\":2}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::ToolCalls),
                    }),
                ]
            } else {
                vec![
                    Ok(ChunkType::Text("done".to_string())),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::Stop),
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[async_trait]
    impl Provider for ReasoningToolCallProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = if request == 0 {
                vec![
                    Ok(ChunkType::Reasoning("inspect the file".to_string())),
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_read","type":"function","function":{"name":"read","arguments":"{\"file_path\":\"src/lib.rs\"}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::ToolCalls),
                    }),
                ]
            } else {
                vec![
                    Ok(ChunkType::Text("done".to_string())),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::Stop),
                    }),
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = match request {
                0 | 1 => vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_repeat","type":"function","function":{"name":"task","arguments":"{\"description\":\"Write haiku\",\"prompt\":\"Write a haiku\",\"subagent_type\":\"general\"}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::ToolCalls),
                    }),
                ],
                _ => vec![
                    Ok(ChunkType::Text("done".to_string())),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::Stop),
                    }),
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
            _hosted_tools: &[HostedTool],
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
            _hosted_tools: &[HostedTool],
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
    impl Provider for PhaselessAmbiguousProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = match request {
                0 => vec![
                    Ok(ChunkType::Text("Dependency conflict found.".to_string())),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::EndTurn),
                    }),
                ],
                1 => vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_list","type":"function","function":{"name":"list","arguments":"{\"path\":\".\"}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::ToolCalls),
                    }),
                ],
                _ => vec![
                    Ok(ChunkType::Text("Done.".to_string())),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::Stop),
                    }),
                ],
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[async_trait]
    impl Provider for PhaselessFinalProvider {
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(ChunkType::Text("Done. Build now passes.".to_string())),
                Ok(ChunkType::End {
                    reason: Some(FinishReason::Stop),
                }),
            ])))
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
            _hosted_tools: &[HostedTool],
            _headers: &HashMap<String, String>,
        ) -> crate::error::Result<ProviderStream> {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            let chunks = if request == 0 {
                vec![
                    Ok(ChunkType::ToolCall(
                        r#"[{"index":0,"id":"call_edit","type":"function","function":{"name":"edit","arguments":"{\"file_path\":\"src/lib.rs\",\"old_string\":\"missing\",\"new_string\":\"replacement\"}"}}]"#
                            .to_string(),
                    )),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::ToolCalls),
                    }),
                ]
            } else {
                let follow_up = messages
                    .last()
                    .and_then(|message| match message {
                        Message::ToolOutput(output) => Some(output.output.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                *self.observed_follow_up.lock().unwrap() = Some(follow_up);

                vec![
                    Ok(ChunkType::Text("recovered".to_string())),
                    Ok(ChunkType::End {
                        reason: Some(FinishReason::Stop),
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(chunks)))
        }
    }

    #[tokio::test]
    async fn retries_retryable_provider_errors_before_output() {
        let provider = RecoveringRateLimitProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };

        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("hello")],
            Vec::new(),
            None,
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let mut saw_retry = false;
        let mut text = String::new();
        while let Some(chunk) = response.stream.next().await {
            match chunk {
                ChunkType::Retry(status) => {
                    saw_retry = true;
                    assert_eq!(status.attempt, 1);
                    assert_eq!(status.message, "Too Many Requests");
                }
                ChunkType::Text(delta) => text.push_str(&delta),
                _ => {}
            }
        }

        assert!(saw_retry);
        assert_eq!(text, "recovered");
        assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rolls_back_partial_output_before_retrying_stream() {
        let provider = RecoveringPartialStreamProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("hello")],
            Vec::new(),
            None,
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let mut visible_text = String::new();
        let mut visible_reasoning = String::new();
        let mut saw_rollback = false;
        let mut saw_retry = false;
        while let Some(chunk) = response.stream.next().await {
            match chunk {
                ChunkType::Text(text) => visible_text.push_str(&text),
                ChunkType::Reasoning(reasoning) => visible_reasoning.push_str(&reasoning),
                ChunkType::StreamRollback { text, reasoning } => {
                    saw_rollback = true;
                    assert!(visible_text.ends_with(&text));
                    assert!(visible_reasoning.ends_with(&reasoning));
                    visible_text.truncate(visible_text.len() - text.len());
                    visible_reasoning.truncate(visible_reasoning.len() - reasoning.len());
                }
                ChunkType::Retry(_) => saw_retry = true,
                _ => {}
            }
        }

        assert!(saw_rollback);
        assert!(saw_retry);
        assert_eq!(visible_text, "recovered");
        assert!(visible_reasoning.is_empty());
        assert_eq!(provider.requests.load(Ordering::SeqCst), 2);

        let messages = response.messages().await;
        assert!(matches!(
            messages.last(),
            Some(Message::Assistant(message)) if message.content == "recovered"
        ));
    }

    #[tokio::test]
    async fn cancellation_during_tool_execution_emits_failed() {
        let cancel_token = CancellationToken::new();
        let provider = TwoToolCallProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let started = Arc::new(AtomicUsize::new(0));

        let tool_started = started.clone();
        let wait_tool = Tool::builder()
            .name("wait")
            .description("block until cancelled")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(move |_input| {
                let tool_started = tool_started.clone();
                async move {
                    tool_started.fetch_add(1, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                    Ok("ok".to_string())
                }
            }))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider,
            vec![Message::user("run tools")],
            vec![wait_tool],
            None,
            None,
            HashMap::new(),
            Some(cancel_token.clone()),
        )
        .await
        .unwrap();

        let drain = tokio::spawn(async move {
            let mut failed = None;
            while let Some(chunk) = response.stream.next().await {
                if let ChunkType::Failed(msg) = chunk {
                    failed = Some(msg);
                }
            }
            failed
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            while started.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("tool execution should start before cancellation");

        cancel_token.cancel();

        let failed = tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("stream should finish promptly after tool cancellation")
            .expect("drain task should complete");
        assert_eq!(failed.as_deref(), Some("Tool execution cancelled by user"));
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
            None,
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
                Message::ToolOutput(output) if output.name == "wait" => Some(output),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(observations.len(), 2);
        assert!(observations.iter().any(|output| output.call_id == "call_1"));
        assert!(observations.iter().any(|output| output.call_id == "call_2"));
    }

    #[tokio::test]
    async fn dropping_stream_text_response_drops_provider_stream_promptly() {
        let polled = Arc::new(AtomicUsize::new(0));
        let stream_dropped = Arc::new(AtomicBool::new(false));
        let provider = NeverEndingProvider {
            polled: polled.clone(),
            stream_dropped: stream_dropped.clone(),
        };

        let response = stream_with_tools(
            provider,
            vec![Message::user("hello")],
            Vec::new(),
            None,
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while polled.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background task should start polling the provider stream");

        drop(response);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !stream_dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropping StreamTextResponse should drop the provider stream promptly");
    }

    #[tokio::test]
    async fn cancellation_while_blocked_on_provider_stream_next_emits_failed_and_closes() {
        let cancel_token = CancellationToken::new();
        let provider = BlockingTextProvider;

        let mut response = stream_with_tools(
            provider,
            vec![Message::user("hello")],
            Vec::new(),
            None,
            None,
            HashMap::new(),
            Some(cancel_token.clone()),
        )
        .await
        .unwrap();

        let drain = tokio::spawn(async move {
            let mut failed = None;
            while let Some(chunk) = response.stream.next().await {
                if let ChunkType::Failed(msg) = chunk {
                    failed = Some(msg);
                }
            }
            failed
        });

        tokio::time::sleep(Duration::from_millis(25)).await;
        cancel_token.cancel();

        let failed = tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("stream should close promptly after cancel during stream.next")
            .expect("drain task should complete");
        assert_eq!(failed.as_deref(), Some("Streaming cancelled by user"));
    }

    #[tokio::test]
    async fn cancellation_while_blocked_on_provider_stream_text_startup_emits_failed_and_closes() {
        let cancel_token = CancellationToken::new();
        let provider = BlockingStartupProvider;

        let mut response = stream_with_tools(
            provider,
            vec![Message::user("hello")],
            Vec::new(),
            None,
            None,
            HashMap::new(),
            Some(cancel_token.clone()),
        )
        .await
        .unwrap();

        let drain = tokio::spawn(async move {
            let mut failed = None;
            while let Some(chunk) = response.stream.next().await {
                if let ChunkType::Failed(msg) = chunk {
                    failed = Some(msg);
                }
            }
            failed
        });

        tokio::time::sleep(Duration::from_millis(25)).await;
        cancel_token.cancel();

        let failed = tokio::time::timeout(Duration::from_secs(1), drain)
            .await
            .expect("stream should close promptly after cancel during provider startup")
            .expect("drain task should complete");
        assert_eq!(failed.as_deref(), Some("Streaming cancelled by user"));
    }

    #[tokio::test]
    async fn preserves_reasoning_content_on_tool_call_history() {
        let provider = ReasoningToolCallProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let read_tool = Tool::builder()
            .name("read")
            .description("read a file")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(|_input| async move {
                Ok("file contents".to_string())
            }))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider,
            vec![Message::user("inspect")],
            vec![read_tool],
            Some(3),
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        while response.stream.next().await.is_some() {}

        let tool_call_reasoning = response.messages().await.into_iter().find_map(|message| {
            if let Message::ToolCall(tool_call) = message {
                tool_call.reasoning_content
            } else {
                None
            }
        });

        assert_eq!(tool_call_reasoning.as_deref(), Some("inspect the file"));
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
            None,
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
                Message::ToolOutput(output)
                    if output.output.contains("Duplicate task call skipped") =>
                {
                    Some(output.output)
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
            None,
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
            None,
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
    async fn continues_once_after_phase_less_end_turn_without_final_phase() {
        let provider = PhaselessAmbiguousProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };
        let executions = Arc::new(AtomicUsize::new(0));

        let list_executions = executions.clone();
        let list_tool = Tool::builder()
            .name("list")
            .description("list files")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(move |_input| {
                let list_executions = list_executions.clone();
                async move {
                    list_executions.fetch_add(1, Ordering::SeqCst);
                    Ok("package.json\nbun.lock".to_string())
                }
            }))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("fix the build")],
            vec![list_tool],
            Some(5),
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let mut text = String::new();
        let mut continuation_logged = false;
        let mut finish_reason_logged = false;
        while let Some(chunk) = response.stream.next().await {
            match chunk {
                ChunkType::Text(delta) => text.push_str(&delta),
                ChunkType::Metadata(message)
                    if message.contains("phase_less_terminal_without_final_signal") =>
                {
                    continuation_logged = true;
                }
                ChunkType::Metadata(message)
                    if message.contains("provider_finish_reason=end_turn") =>
                {
                    finish_reason_logged = true;
                }
                _ => {}
            }
        }

        assert_eq!(text, "Dependency conflict found.Done.");
        assert!(continuation_logged);
        assert!(finish_reason_logged);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 3);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(response.stop_reason().await, Some(StopReason::Finish));
    }

    #[tokio::test]
    async fn phase_less_final_text_still_finishes() {
        let provider = PhaselessFinalProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };

        let noop_tool = Tool::builder()
            .name("noop")
            .description("noop")
            .input_schema(Schema::from(true))
            .execute(ToolExecute::new(
                |_input| async move { Ok("ok".to_string()) },
            ))
            .build()
            .unwrap();

        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("fix the build")],
            vec![noop_tool],
            Some(5),
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let mut text = String::new();
        while let Some(chunk) = response.stream.next().await {
            if let ChunkType::Text(delta) = chunk {
                text.push_str(&delta);
            }
        }

        assert_eq!(text, "Done. Build now passes.");
        assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
        assert_eq!(response.stop_reason().await, Some(StopReason::Finish));
    }

    #[tokio::test]
    async fn max_steps_allows_exact_configured_step_count() {
        let provider = FollowUpProvider {
            requests: Arc::new(AtomicUsize::new(0)),
        };

        let mut response = stream_with_tools(
            provider.clone(),
            vec![Message::user("finish the task")],
            Vec::new(),
            Some(1),
            None,
            HashMap::new(),
            None,
        )
        .await
        .unwrap();

        let mut text = String::new();
        let mut incomplete = Vec::new();
        while let Some(chunk) = response.stream.next().await {
            match chunk {
                ChunkType::Text(delta) => text.push_str(&delta),
                ChunkType::Incomplete(message) => incomplete.push(message),
                _ => {}
            }
        }

        assert_eq!(text, "I'll inspect that next.");
        assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
        assert_eq!(incomplete, vec!["Max steps reached".to_string()]);
        assert_eq!(response.stop_reason().await, Some(StopReason::Hook));
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
                Err::<String, String>(
                    "Execution error: Not found: Could not find text to replace".to_string(),
                )
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
            None,
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
        assert!(follow_up.contains("Tool 'edit' error"));
        assert!(follow_up.contains("Could not find text to replace"));
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
        assert_eq!(calls[0].call_id, "call_1");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[0].arguments["command"], "ls -la");
    }

    #[test]
    fn uses_responses_call_id_for_tool_output_correlation() {
        let mut accumulator = ToolCallAccumulator::default();

        accumulator
            .ingest(
                r#"[{"index":0,"id":"fc_1","call_id":"call_1","type":"function","function":{"name":"read","arguments":""}}]"#,
            )
            .unwrap();
        accumulator
            .ingest(r#"[{"index":0,"id":"fc_1","function":{"arguments_done":"{\"file_path\":\"Cargo.toml\"}"}}]"#)
            .unwrap();

        let calls = accumulator.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call_1");
        assert_eq!(calls[0].item_id.as_deref(), Some("fc_1"));
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments["file_path"], "Cargo.toml");
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
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments["file_path"], "Cargo.toml");
        assert_eq!(calls[1].name, "bash");
        assert_eq!(calls[1].arguments["command"], "cargo test");
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

        assert_eq!(calls[0].arguments, serde_json::json!({}));
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
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments["file_path"], "Cargo.toml");
    }
}
