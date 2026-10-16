use super::cancellable;
use super::internal_error;
use super::internal_error_with;
use super::notifications::send_text;
use super::notifications::send_tool_call;
use super::notifications::send_tool_result;
use super::permissions::request_permission;
use super::questions::request_questions;
use super::questions::skipped_question_answers;
use super::terminals::bridge_terminal_session;
use super::transcript::record_tool_call;
use super::transcript::record_tool_result;
use super::usage::cumulative_cost;
use super::usage::estimate_session_usage_cost;
use super::usage::send_live_usage;
use super::usage::send_usage;
use super::usage::AcpLiveUsage;
use super::AcpService;
use super::AcpSession;
use agent_client_protocol::schema::v1::PromptResponse;
use agent_client_protocol::schema::v1::StopReason;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use tokio_util::sync::CancellationToken;

fn acp_stop_reason(reason: Option<crate::llm::TurnStopReason>) -> StopReason {
    match reason {
        Some(crate::llm::TurnStopReason::MaxTokens) => StopReason::MaxTokens,
        Some(crate::llm::TurnStopReason::MaxTurnRequests) => StopReason::MaxTurnRequests,
        Some(crate::llm::TurnStopReason::Refusal) => StopReason::Refusal,
        None => StopReason::EndTurn,
    }
}

impl AcpService {
    // Keep finalization outside every cancellable await, including client requests.
    pub(super) async fn receive_prompt_stream(
        &self,
        session_id: &str,
        session: &AcpSession,
        connection: &ConnectionTo<Client>,
        cancellation: &CancellationToken,
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<crate::llm::ChunkMessage>,
        base_context_tokens: usize,
        base_prompt_tokens: usize,
        base_cost: f64,
    ) -> Result<PromptResponse, Error> {
        let mut assistant = crate::session::types::Message::incomplete("");
        let message_id = assistant.id.clone();
        assistant.provider = Some(session.provider.clone());
        assistant.model = Some(session.model.clone());
        assistant.agent_mode = Some(session.agent.clone());
        let mut failed = None;
        let mut cancelled = false;
        let mut turn_stop_reason = None;
        let mut live_usage = AcpLiveUsage::default();

        // Notification failures must take the same persistence path as cancellation.
        let stream_result: Result<(), Error> = async {
            while let Some(chunk) = cancellable(cancellation, receiver.recv()).await.flatten() {
                match chunk {
                    crate::llm::ChunkMessage::Text(text) => {
                        assistant.append(&text);
                        send_text(connection, session_id, &message_id, text, false)?;
                        send_live_usage(
                            connection,
                            session_id,
                            session,
                            base_context_tokens,
                            base_cost,
                            &assistant,
                            &live_usage,
                        )?;
                    }
                    crate::llm::ChunkMessage::Reasoning(text) => {
                        assistant.append_reasoning(&text);
                        send_text(connection, session_id, &message_id, text, true)?;
                        send_live_usage(
                            connection,
                            session_id,
                            session,
                            base_context_tokens,
                            base_cost,
                            &assistant,
                            &live_usage,
                        )?;
                    }
                    crate::llm::ChunkMessage::ToolCalls(tool_calls) => {
                        for tool_call in &tool_calls {
                            record_tool_call(&mut assistant, tool_call);
                        }
                        for tool_call in tool_calls {
                            send_tool_call(connection, session_id, tool_call, &session.cwd)?;
                        }
                        send_live_usage(
                            connection,
                            session_id,
                            session,
                            base_context_tokens,
                            base_cost,
                            &assistant,
                            &live_usage,
                        )?;
                    }
                    crate::llm::ChunkMessage::ToolResult(result) => {
                        record_tool_result(&mut assistant, &result);
                        send_tool_result(connection, session_id, result, &session.cwd)?;
                        send_live_usage(
                            connection,
                            session_id,
                            session,
                            base_context_tokens,
                            base_cost,
                            &assistant,
                            &live_usage,
                        )?;
                    }
                    crate::llm::ChunkMessage::Metrics {
                        token_count,
                        duration_ms,
                        usage,
                        cost,
                    } => {
                        assistant.token_count = Some(token_count);
                        assistant.duration_ms = Some(duration_ms);
                        if let Some(usage) = usage {
                            assistant.apply_usage(usage, cost);
                            live_usage.cumulative = usage;
                        }
                        live_usage.cost = cost.unwrap_or(live_usage.cost);
                        send_usage(
                            connection,
                            session_id,
                            session,
                            live_usage
                                .context_tokens(base_context_tokens, &assistant)
                                .max(base_context_tokens.saturating_add(token_count)),
                            cumulative_cost(base_cost, live_usage.cost),
                            usage,
                        )?;
                    }
                    crate::llm::ChunkMessage::Usage(usage) => {
                        live_usage.observe_provider_usage(&assistant, usage);
                        live_usage.cost += estimate_session_usage_cost(session, &usage);
                        assistant
                            .parts
                            .push(crate::session::types::MessagePart::usage(
                                usage.input,
                                usage.output,
                                usage.cache_read,
                                usage.cache_write,
                                estimate_session_usage_cost(session, &usage),
                            ));
                        if let Some(part) = assistant.parts.last_mut() {
                            part.data["acp_prompt_tokens"] = serde_json::json!(base_prompt_tokens);
                        }
                        if usage.output > 0 {
                            assistant.output_tokens = Some(
                                assistant
                                    .output_tokens
                                    .unwrap_or(0)
                                    .saturating_add(usage.output as usize),
                            );
                        }
                        send_live_usage(
                            connection,
                            session_id,
                            session,
                            base_context_tokens,
                            base_cost,
                            &assistant,
                            &live_usage,
                        )?;
                    }
                    crate::llm::ChunkMessage::Cancelled => {
                        cancelled = true;
                        break;
                    }
                    crate::llm::ChunkMessage::Failed(error) => failed = Some(error),
                    crate::llm::ChunkMessage::TurnStopReason(reason) => {
                        turn_stop_reason = Some(reason)
                    }
                    crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                        let response = cancellable(
                            cancellation,
                            request_permission(connection, session_id, &prompt),
                        )
                        .await
                        .unwrap_or(crate::tools::PermissionResponse::Deny);
                        let _ = prompt.response_tx.send(response);
                    }
                    crate::llm::ChunkMessage::QuestionRequest {
                        tool_call_id,
                        questions,
                        response_tx,
                    } => {
                        let response = if self.supports_form_elicitation() {
                            request_questions(
                                connection,
                                session_id,
                                tool_call_id.as_deref(),
                                &questions,
                                cancellation,
                            )
                            .await
                        } else {
                            skipped_question_answers(&questions)
                        };
                        let _ = response_tx.send(response);
                    }
                    crate::llm::ChunkMessage::TerminalSessionRequest(request) => {
                        if self.supports_terminals() {
                            bridge_terminal_session(
                                connection,
                                session_id,
                                &session.cwd,
                                request,
                                cancellation,
                            )
                            .await;
                        } else {
                            let _ = request
                                .control_tx
                                .send(crate::tools::TerminalSessionControl::Stop);
                        }
                    }
                    crate::llm::ChunkMessage::End => break,
                    _ => {}
                }
            }

            Ok(())
        }
        .await;
        if stream_result.is_err() && failed.is_none() {
            failed = Some("ACP stream notification failed".to_string());
        }
        assistant.is_complete = true;
        assistant.was_interrupted = cancelled || cancellation.is_cancelled();
        {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            let persist_result = manager.add_message_to_session(session_id, &assistant);
            let status = if assistant.was_interrupted {
                crate::session::types::SessionStatus::Interrupted
            } else if failed.is_some() {
                crate::session::types::SessionStatus::Failed
            } else {
                crate::session::types::SessionStatus::Idle
            };
            manager
                .set_session_status(session_id, status, failed.as_deref())
                .map_err(|_| internal_error())?;
            persist_result.map_err(|_| internal_error())?;
        }
        if assistant.was_interrupted {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }
        stream_result?;
        if let Some(error) = failed {
            return Err(internal_error_with(&error));
        }
        Ok(PromptResponse::new(acp_stop_reason(turn_stop_reason)))
    }
}

#[cfg(test)]
#[path = "tests/stream.rs"]
mod tests;
