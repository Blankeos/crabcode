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

pub(super) fn record_tool_call(
    assistant: &mut crate::session::types::Message,
    tool_call: &crate::llm::ToolCall,
) {
    let args = serde_json::from_str(&tool_call.function.arguments)
        .unwrap_or_else(|_| serde_json::Value::String(tool_call.function.arguments.clone()));
    let provider_executed = matches!(
        tool_call.function.name.as_str(),
        "x_search" | "web_search" | "file_search"
    );

    if let Some(part) = assistant
        .parts
        .iter_mut()
        .find(|part| part.part_type == "tool_call" && part.tool_id() == Some(tool_call.id.as_str()))
    {
        if let Some(obj) = part.data.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(tool_call.function.name.clone()),
            );
            obj.insert("args".to_string(), args);
            if provider_executed {
                obj.insert(
                    "provider_executed".to_string(),
                    serde_json::Value::Bool(true),
                );
            }
        }
        return;
    }

    assistant.add_tool_call_part(tool_call.id.clone(), tool_call.function.name.clone(), args);
    if provider_executed {
        if let Some(obj) = assistant
            .parts
            .last_mut()
            .and_then(|part| part.data.as_object_mut())
        {
            obj.insert(
                "provider_executed".to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }
}

pub(super) fn record_tool_result(
    assistant: &mut crate::session::types::Message,
    result: &crate::llm::ToolCallResult,
) {
    let mut data = assistant
        .tool_call_part_data(&result.tool_call_id)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    data["id"] = serde_json::Value::String(result.tool_call_id.clone());
    data["name"] = serde_json::Value::String(result.name.clone());
    if matches!(
        result.name.as_str(),
        "x_search" | "web_search" | "file_search"
    ) {
        data["provider_executed"] = serde_json::Value::Bool(true);
    }

    match serde_json::from_str::<serde_json::Value>(&result.content) {
        Ok(serde_json::Value::Object(payload)) => {
            if let Some(data) = data.as_object_mut() {
                for (key, value) in payload {
                    data.insert(key, value);
                }
            }
        }
        _ => {
            data["status"] = serde_json::Value::String("ok".to_string());
            data["output_preview"] = serde_json::Value::String(result.content.clone());
        }
    }
    assistant.add_or_update_tool_result_part(data);
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
mod tests {
    use super::super::test_support::empty_config;
    use super::super::test_support::session_with_config;
    use super::super::AcpService;
    use super::super::AcpSessions;
    use super::*;
    use crate::session::manager::SessionManager;
    use agent_client_protocol::schema::v1::RequestPermissionRequest;
    use agent_client_protocol::schema::v1::SessionNotification;
    use agent_client_protocol::schema::v1::SessionUpdate;
    use agent_client_protocol::schema::v1::StopReason;
    use agent_client_protocol::Client;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::sync::Mutex as AsyncMutex;

    #[test]
    fn maps_typed_turn_stop_reasons_to_acp() {
        assert_eq!(
            acp_stop_reason(Some(crate::llm::TurnStopReason::MaxTokens)),
            StopReason::MaxTokens
        );
        assert_eq!(
            acp_stop_reason(Some(crate::llm::TurnStopReason::Refusal)),
            StopReason::Refusal
        );
        assert_eq!(
            acp_stop_reason(Some(crate::llm::TurnStopReason::MaxTurnRequests)),
            StopReason::MaxTurnRequests
        );
        assert_eq!(
            acp_stop_reason(Some(crate::llm::TurnStopReason::MaxTurnRequests)),
            StopReason::MaxTurnRequests
        );
        assert_eq!(acp_stop_reason(None), StopReason::EndTurn);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_stream_persists_partial_transcript_and_interrupted_status() {
        cancelled_stream_finalizes(false, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_permission_persists_partial_transcript_and_interrupted_status() {
        cancelled_stream_finalizes(true, false).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn close_reopen_waits_for_old_prompt_finalization() {
        cancelled_stream_finalizes(false, true).await;
    }

    async fn cancelled_stream_finalizes(pending_permission: bool, close_reopen: bool) {
        use agent_client_protocol::schema::v1::RequestPermissionResponse;
        use agent_client_protocol::{Agent, Responder};
        use std::time::Duration;

        tokio::task::LocalSet::new().run_until(async {
            let workspace = tempfile::tempdir().unwrap();
            let mut manager = SessionManager::new()
                .with_history_for_workspace(workspace.path()).unwrap();
            let session_id = manager.create_session(Some("ACP cancellation regression".into()));
            let db_id = manager.get_db_id(&session_id).unwrap();
            manager.set_session_status(
                &session_id, crate::session::types::SessionStatus::Streaming, None,
            ).unwrap();
            let service = AcpService {
                sessions: Arc::new(AsyncMutex::new(AcpSessions::default())),
                session_manager: Arc::new(Mutex::new(manager)),
                client_capabilities: Arc::new(Mutex::new(Default::default())),
            };
            let session = session_with_config(empty_config());
            service.sessions.lock().await.attachments.insert(session_id.clone(), session.clone());
            let cancellation = service.sessions.lock().await.begin_prompt(&session_id).unwrap();
            let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
            let (permission_tx, mut permission_rx) = tokio::sync::mpsc::unbounded_channel();
            let client = Client.builder().on_receive_request(
                async move |_request: RequestPermissionRequest,
                            responder: Responder<RequestPermissionResponse>, _connection| {
                    permission_tx.send(responder).unwrap();
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            ).on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    if matches!(notification.update, SessionUpdate::AgentMessageChunk(_)) {
                        let _ = observed_tx.send(());
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            );
            Agent.builder().connect_with(client, async move |connection| {
                let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                sender.send(crate::llm::ChunkMessage::Text("partial answer".into())).unwrap();
                sender.send(crate::llm::ChunkMessage::Reasoning("partial reasoning".into())).unwrap();
                let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                if pending_permission {
                    sender.send(crate::llm::ChunkMessage::PermissionRequest(crate::tools::PermissionPrompt {
                        tool_call_id: Some("pending-tool".into()),
                        tool_id: "bash".into(),
                        action: crate::tools::PermissionAction::Write,
                        permission: "bash".into(),
                        patterns: Vec::new(),
                        target: None,
                        command: Some("mock-command".into()),
                        workdir: None,
                        workspace: workspace.path().to_string_lossy().into_owned(),
                        reason: "regression test".into(),
                        raw_input: serde_json::json!({"command": "mock-command"}),
                        response_tx,
                    })).unwrap();
                }
                let stream = service.receive_prompt_stream(
                    &session_id, &session, &connection, &cancellation, receiver, 0, 0, 0.0,
                );
                tokio::pin!(stream);
                tokio::time::timeout(Duration::from_secs(2), async {
                    tokio::select! {
                        _ = observed_rx.recv() => {},
                        result = &mut stream => panic!("stream ended before cancellation: {result:?}"),
                    }
                }).await.unwrap();
                let _pending_responder = if pending_permission {
                    Some(tokio::time::timeout(Duration::from_secs(2), async {
                        tokio::select! {
                            responder = permission_rx.recv() => responder.unwrap(),
                            result = &mut stream => panic!("stream ended before permission: {result:?}"),
                        }
                    }).await.unwrap())
                } else {
                    None
                };
                if close_reopen {
                    service.close_session(&session_id).await;
                    assert!(cancellation.is_cancelled());
                    assert!(!service.sessions.lock().await.attachments.contains_key(&session_id));
                    assert!(service.available_commands(&session_id).await.is_err());
                    assert!(service.set_mode(&session_id, "Build").await.is_err());
                    assert!(service.set_model(&session_id, "unused").await.is_err());
                    assert!(service.set_reasoning_effort(&session_id, "none").await.is_err());
                    // The same guard serves load and resume. Resume must fail before
                    // any provider discovery, while the old stream still owns writes.
                    assert!(service.resume_session(session_id.clone(), workspace.path().into()).await.is_err());
                    assert!(service.load_session(session_id.clone(), workspace.path().into(), connection.clone()).await.is_err());
                    assert!(service.sessions.lock().await.begin_prompt(&session_id).is_err());
                } else {
                    cancellation.cancel();
                }
                let response = tokio::time::timeout(Duration::from_secs(1), &mut stream)
                    .await.unwrap().unwrap();
                assert_eq!(response.stop_reason, StopReason::Cancelled);
                if pending_permission {
                    assert!(matches!(response_rx.await.unwrap(), crate::tools::PermissionResponse::Deny));
                }
                let manager = service.session_manager.lock().unwrap();
                let stored = manager.get_session_ref(&session_id).unwrap();
                assert_eq!(stored.status, crate::session::types::SessionStatus::Interrupted);
                assert_eq!(stored.messages.len(), 1);
                let assistant = &stored.messages[0];
                assert_eq!(assistant.content, "partial answer");
                assert!(assistant.is_complete);
                assert!(assistant.was_interrupted);
                drop(manager);
                // A fresh DAO verifies the real database, not just the in-memory message.
                let dao = crate::persistence::HistoryDAO::new_for_workspace(workspace.path()).unwrap();
                assert_eq!(dao.get_session(db_id).unwrap().unwrap().status, "interrupted");
                let messages = dao.get_messages(db_id).unwrap();
                assert_eq!(messages.len(), 1);
                assert!(messages[0].parts.iter().any(|part|
                    part.part_type == "text" && part.data["text"] == "partial answer"
                ));
                drop(dao);
                if close_reopen {
                    // Stream persistence has ended, but wrapper finalization has not:
                    // reopen must stay blocked through that second status write too.
                    assert!(service.resume_session(session_id.clone(), workspace.path().into()).await.is_err());
                    service.session_manager.lock().unwrap().set_session_status(
                        &session_id, crate::session::types::SessionStatus::Interrupted, None,
                    ).unwrap();
                    service.sessions.lock().await.finish_prompt(&session_id);
                    let mut sessions = service.sessions.lock().await;
                    sessions.ensure_attachable(&session_id).unwrap();
                    sessions.attachments.insert(session_id.clone(), session.clone());
                    let next = sessions.begin_prompt(&session_id).unwrap();
                    drop(sessions);
                    service.session_manager.lock().unwrap().set_session_status(
                        &session_id, crate::session::types::SessionStatus::Streaming, None,
                    ).unwrap();
                    assert!(!next.is_cancelled());
                    assert!(service.sessions.lock().await.active_prompts.contains(&session_id));
                    let dao = crate::persistence::HistoryDAO::new_for_workspace(workspace.path()).unwrap();
                    assert_eq!(dao.get_session(db_id).unwrap().unwrap().status, "streaming");
                    service.cancel_session(&session_id).await;
                    assert!(next.is_cancelled(), "the new turn retains its cancellation token");
                    service.sessions.lock().await.finish_prompt(&session_id);
                } else {
                    service.sessions.lock().await.finish_prompt(&session_id);
                }
                service.session_manager.lock().unwrap().try_delete_session(&session_id).unwrap();
                Ok(())
            }).await.unwrap();
        }).await;
    }
}
