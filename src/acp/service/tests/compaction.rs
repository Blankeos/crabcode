use super::super::compaction_summary::compacted_messages;
use super::super::context::acp_context_tokens;
use super::super::prompt::acp_request_messages;
use super::super::test_support::test_session;
use super::super::usage::acp_session_cost;
use super::super::AcpService;
use super::super::AcpSessions;
use crate::session::manager::SessionManager;
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::schema::v1::SessionUpdate;
use agent_client_protocol::Client;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::Mutex as AsyncMutex;

#[test]
fn acp_compaction_discards_provider_usage_before_compaction_boundary() {
    use crate::session::types::{CompactionStats, Message, MessagePart};
    let mut assistant = Message::assistant("old answer");
    assistant
        .parts
        .push(MessagePart::usage(90_000, 100, 0, 0, 0.0));
    let mut summary = Message::user(format!(
        "{}\nsummary",
        crate::session::compaction::SUMMARY_PREFIX
    ));
    summary
        .parts
        .push(MessagePart::usage(95_000, 500, 0, 0, 0.0));
    let marker = crate::session::compaction::compaction_marker(CompactionStats {
        before_tokens: 90_100,
        after_tokens: 100,
        before_messages: 2,
        after_messages: 1,
    });
    let messages = vec![Message::user("question"), assistant, summary, marker];

    assert_eq!(
        acp_context_tokens(&messages, 1_000),
        crate::session::compaction::total_context_tokens(&messages) + 1_000
    );
}

#[test]
fn acp_compaction_honors_reserved_tokens_and_auto_setting() {
    use crate::config::configuration::CompactionConfig;
    use crate::session::types::{Message, MessagePart};
    let mut assistant = Message::assistant("answer");
    assistant
        .parts
        .push(MessagePart::usage(70_000, 0, 10_000, 0, 0.0));
    let messages = vec![Message::user("history ".repeat(2_000)), assistant];
    assert!(crate::session::compaction::select_messages_for_compaction(
        &messages,
        crate::session::compaction::DEFAULT_TAIL_TURNS,
    )
    .is_some());
    let config = CompactionConfig::Settings {
        auto: true,
        prune: false,
        reserved: Some(20_000),
    };
    let used = acp_context_tokens(&messages, 1_000);
    assert!(crate::session::compaction::total_context_tokens(&messages) < 80_000);
    assert!(crate::session::compaction::should_auto_compact(
        &config,
        used,
        Some(100_000),
        None
    ));
    assert!(!crate::session::compaction::should_auto_compact(
        &config,
        used,
        Some(100_001),
        None
    ));
    assert!(!crate::session::compaction::should_auto_compact(
        &CompactionConfig::Disabled,
        used,
        Some(100_000),
        None
    ));
    assert!(!crate::session::compaction::should_auto_compact(
        &config, used, None, None
    ));
    let no_auto = CompactionConfig::Settings {
        auto: false,
        prune: false,
        reserved: Some(20_000),
    };
    assert!(!crate::session::compaction::should_auto_compact(
        &no_auto,
        used,
        Some(100_000),
        None
    ));
}

#[test]
fn acp_compaction_keeps_fresh_system_prompt_in_request() {
    use crate::session::types::{CompactionStats, Message};
    let summary = Message::user(format!(
        "{}\nsummary",
        crate::session::compaction::SUMMARY_PREFIX
    ));
    let tail = Message::assistant("retained answer");
    let marker = crate::session::compaction::compaction_marker(CompactionStats {
        before_tokens: 10_000,
        after_tokens: 100,
        before_messages: 3,
        after_messages: 2,
    });
    let pending = Message::user("continue");
    let history = vec![
        Message::user("archived history"),
        summary.clone(),
        tail.clone(),
        marker,
        pending.clone(),
    ];
    let system = Message::system("fresh system instructions");
    let request = acp_request_messages(&history, system.clone());
    assert_eq!(request, vec![system, summary, tail, pending]);
    assert_eq!(
        crate::session::compaction::filter_messages_for_context(&request),
        request
    );
}

#[test]
fn acp_compaction_cost_keeps_summary_billing_without_double_counting() {
    use crate::session::types::{Message, MessagePart};
    let mut assistant = Message::assistant("answer");
    assistant.cost = Some(0.25);
    assistant
        .parts
        .push(MessagePart::usage(100, 20, 0, 0, 0.25));
    let mut summary = Message::user(format!(
        "{}\nsummary",
        crate::session::compaction::SUMMARY_PREFIX
    ));
    summary.parts.push(MessagePart::usage(100, 20, 0, 0, 0.05));
    assert!((acp_session_cost(&[assistant, summary]) - 0.30).abs() < f64::EPSILON);
}

#[tokio::test(flavor = "current_thread")]
async fn acp_compaction_notifies_client_of_reduced_context_and_summary_cost() {
    use crate::session::types::{Message, MessagePart};
    use agent_client_protocol::Agent;
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = tempfile::tempdir().unwrap();
            let mut manager = SessionManager::new()
                .with_history_for_workspace(workspace.path())
                .unwrap();
            let session_id = manager.create_session(Some("ACP compaction regression".into()));
            let mut assistant = Message::assistant("old answer");
            assistant
                .parts
                .push(MessagePart::usage(80_000, 200, 0, 0, 0.25));
            let messages = vec![Message::user("history ".repeat(2_000)), assistant];
            let selection = crate::session::compaction::select_messages_for_compaction(
                &messages,
                crate::session::compaction::DEFAULT_TAIL_TURNS,
            )
            .unwrap();
            let mut session = test_session();
            session.context_window = Some(100_000);
            let summary = crate::llm::client::CompactionSummary {
                text: "Retained task details".into(),
                usage: crate::aisdk::chunk::TokenUsage {
                    input: 3_000,
                    output: 20,
                    cache_read: 0,
                    cache_write: 0,
                },
            };
            let (mut compacted, stats) = compacted_messages(
                &messages,
                &selection,
                &summary,
                &session,
                crate::session::compaction::total_context_tokens(&messages),
                messages.len(),
            )
            .unwrap();
            let summary_message = compacted
                .iter_mut()
                .find(|message| crate::session::compaction::is_compaction_summary(message))
                .unwrap();
            summary_message
                .parts
                .push(MessagePart::usage(0, 1, 0, 0, 0.05));
            let expected_used =
                crate::session::compaction::total_context_tokens(&compacted) + 1_000;
            manager
                .replace_session_messages(&session_id, compacted)
                .unwrap();
            let service = AcpService {
                sessions: Arc::new(AsyncMutex::new(AcpSessions::default())),
                session_manager: Arc::new(Mutex::new(manager)),
                client_capabilities: Arc::new(Mutex::new(Default::default())),
            };
            let (observed_tx, mut observed_rx) = tokio::sync::mpsc::unbounded_channel();
            let client = Client.builder().on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    let _ = observed_tx.send(notification.update);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            );
            Agent
                .builder()
                .connect_with(client, async move |connection| {
                    service
                        .notify_compaction(&session_id, &session, &connection, stats, 1_000)
                        .unwrap();
                    let feedback =
                        tokio::time::timeout(std::time::Duration::from_secs(2), observed_rx.recv())
                            .await
                            .unwrap()
                            .unwrap();
                    assert!(matches!(feedback, SessionUpdate::AgentMessageChunk(_)));
                    let update =
                        tokio::time::timeout(std::time::Duration::from_secs(2), observed_rx.recv())
                            .await
                            .unwrap()
                            .unwrap();
                    let SessionUpdate::UsageUpdate(usage) = update else {
                        panic!("missing usage update")
                    };
                    assert_eq!(usage.used, expected_used as u64);
                    assert_eq!(usage.size, 100_000);
                    assert!((usage.cost.unwrap().amount - 0.30).abs() < f64::EPSILON);
                    service
                        .session_manager
                        .lock()
                        .unwrap()
                        .try_delete_session(&session_id)
                        .unwrap();
                    Ok(())
                })
                .await
                .unwrap();
        })
        .await;
}
