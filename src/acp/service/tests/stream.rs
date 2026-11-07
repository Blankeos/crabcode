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
