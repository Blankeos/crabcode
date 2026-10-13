use super::cancellable;
use super::config::model_output_limit;
use super::config::model_reasoning_capability;
use super::internal_error;
use super::internal_error_with;
use super::notifications::send_text;
use super::usage::acp_context_tokens;
use super::usage::acp_session_cost;
use super::usage::estimate_session_usage_cost;
use super::usage::send_usage;
use super::AcpService;
use super::AcpSession;
use agent_client_protocol::schema::v1::PromptResponse;
use agent_client_protocol::schema::v1::StopReason;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use tokio_util::sync::CancellationToken;

fn compaction_reasoning(session: &AcpSession) -> Option<crate::model::reasoning::ReasoningEffort> {
    use crate::model::reasoning::ReasoningEffort;
    let capability = model_reasoning_capability(
        &session.config,
        &session.models,
        &session.provider,
        &session.model,
    )?;
    [
        ReasoningEffort::None,
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
    ]
    .into_iter()
    .find(|effort| capability.values().contains(effort))
    .or(session.reasoning)
    .filter(|effort| *effort != ReasoningEffort::None)
}

fn compacted_messages(
    messages: &[crate::session::types::Message],
    selection: &crate::session::compaction::CompactionSelection,
    summary: &crate::llm::client::CompactionSummary,
    session: &AcpSession,
    before_tokens: usize,
    before_messages: usize,
) -> Result<
    (
        Vec<crate::session::types::Message>,
        crate::session::types::CompactionStats,
    ),
    Error,
> {
    let mut compacted = crate::session::compaction::apply_soft_compaction(
        messages,
        selection,
        &summary.text,
        Some(session.model.clone()),
        Some(session.provider.clone()),
        Some(session.agent.clone()),
        crate::session::types::CompactionStats {
            before_tokens,
            after_tokens: 0,
            before_messages,
            after_messages: 0,
        },
    );
    crate::session::compaction::attach_summary_usage(
        &mut compacted,
        crate::session::types::RecordedUsage {
            input: summary.usage.input,
            output: summary.usage.output,
            cache_read: summary.usage.cache_read,
            cache_write: summary.usage.cache_write,
            cost: estimate_session_usage_cost(session, &summary.usage),
        },
    );
    let after_tokens = crate::session::compaction::total_context_tokens(&compacted);
    let after_messages = crate::session::compaction::filter_messages_for_context(&compacted).len();
    let stats = crate::session::types::CompactionStats {
        before_tokens,
        after_tokens,
        before_messages,
        after_messages,
    };
    if after_tokens >= before_tokens {
        return Err(Error::invalid_params().data(format!(
            "Compaction did not reduce context ({})",
            crate::session::compaction::format_compaction_stats(stats)
        )));
    }
    if let Some(marker) = compacted
        .iter_mut()
        .rev()
        .find(|message| crate::session::compaction::is_compaction_marker(message))
    {
        marker.compaction_stats = Some(stats);
    }
    Ok((compacted, stats))
}

impl AcpService {
    pub(super) async fn compact_session(
        &self,
        session_id: &str,
        session: AcpSession,
        connection: ConnectionTo<Client>,
        cancellation: CancellationToken,
    ) -> Result<PromptResponse, Error> {
        let status_result = self
            .session_manager
            .lock()
            .map_err(|_| internal_error())?
            .set_session_status(
                session_id,
                crate::session::types::SessionStatus::Streaming,
                None,
            );
        if status_result.is_err() {
            return Err(internal_error());
        }

        let result = self
            .run_compaction(session_id, &session, cancellation.clone(), 0)
            .await;
        self.session_manager
            .lock()
            .map_err(|_| internal_error())?
            .set_session_status(
                session_id,
                if cancellation.is_cancelled() {
                    crate::session::types::SessionStatus::Interrupted
                } else {
                    crate::session::types::SessionStatus::Idle
                },
                None,
            )
            .map_err(|_| internal_error())?;

        match result {
            Ok(stats) => {
                self.notify_compaction(session_id, &session, &connection, stats, 0)?;
                Ok(PromptResponse::new(StopReason::EndTurn))
            }
            Err(_error) if cancellation.is_cancelled() => {
                Ok(PromptResponse::new(StopReason::Cancelled))
            }
            Err(error) => Err(error),
        }
    }

    async fn run_compaction(
        &self,
        session_id: &str,
        session: &AcpSession,
        cancellation: CancellationToken,
        minimum_tokens: usize,
    ) -> Result<crate::session::types::CompactionStats, Error> {
        let messages = {
            let manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .get_session_ref(session_id)
                .map(|stored| stored.messages.clone())
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?
        };
        let selection = crate::session::compaction::select_messages_for_compaction_with_min(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
            minimum_tokens,
        )
        .ok_or_else(|| Error::invalid_params().data("Nothing to compact"))?;
        let before_tokens = crate::session::compaction::total_context_tokens(&messages);
        let before_messages =
            crate::session::compaction::filter_messages_for_context(&messages).len();
        let prompt = crate::session::compaction::build_prompt(&selection.messages_to_summarize);
        let summary = cancellable(
            &cancellation,
            crate::llm::client::summarize_for_compaction(
                session.provider.clone(),
                session.model.clone(),
                compaction_reasoning(session),
                prompt,
                cancellation.clone(),
            ),
        )
        .await
        .ok_or_else(|| internal_error_with("Compaction cancelled by user"))?
        .map_err(|error| internal_error_with(&error.to_string()))?;
        if cancellation.is_cancelled() {
            return Err(internal_error_with("Compaction cancelled by user"));
        }
        let (compacted, stats) = compacted_messages(
            &messages,
            &selection,
            &summary,
            session,
            before_tokens,
            before_messages,
        )?;
        let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
        manager
            .replace_session_messages(session_id, compacted)
            .map_err(|_| internal_error())?;
        Ok(stats)
    }

    pub(super) async fn maybe_auto_compact_session(
        &self,
        session_id: &str,
        session: &AcpSession,
        connection: &ConnectionTo<Client>,
        cancellation: CancellationToken,
        prompt_tokens: usize,
        additional_tokens: usize,
    ) -> Result<bool, Error> {
        let messages = self
            .session_manager
            .lock()
            .map_err(|_| internal_error())?
            .get_session_ref(session_id)
            .map(|stored| stored.messages.clone())
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let used_tokens =
            acp_context_tokens(&messages, prompt_tokens).saturating_add(additional_tokens);
        if !crate::session::compaction::should_auto_compact(
            &session.config.merged_config.compaction,
            used_tokens,
            session.context_window,
            model_output_limit(&session.config, &session.provider, &session.model),
        ) {
            return Ok(false);
        }
        if crate::session::compaction::select_messages_for_compaction(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
        )
        .is_none()
        {
            return Ok(false);
        }
        self.session_manager
            .lock()
            .map_err(|_| internal_error())?
            .set_session_status(
                session_id,
                crate::session::types::SessionStatus::Streaming,
                None,
            )
            .map_err(|_| internal_error())?;
        let result = self
            .run_compaction(
                session_id,
                session,
                cancellation.clone(),
                crate::session::compaction::MIN_COMPACTABLE_TOKENS,
            )
            .await;
        self.session_manager
            .lock()
            .map_err(|_| internal_error())?
            .set_session_status(
                session_id,
                if cancellation.is_cancelled() {
                    crate::session::types::SessionStatus::Interrupted
                } else {
                    crate::session::types::SessionStatus::Idle
                },
                None,
            )
            .map_err(|_| internal_error())?;
        match result {
            Ok(stats) => {
                self.notify_compaction(session_id, session, connection, stats, prompt_tokens)?;
                Ok(true)
            }
            Err(error) => {
                crate::emit_log!("ACP auto-compaction skipped after failure: {:?}", error);
                Ok(false)
            }
        }
    }

    fn notify_compaction(
        &self,
        session_id: &str,
        session: &AcpSession,
        connection: &ConnectionTo<Client>,
        stats: crate::session::types::CompactionStats,
        prompt_tokens: usize,
    ) -> Result<(), Error> {
        let (used_tokens, cost) = {
            let manager = self.session_manager.lock().map_err(|_| internal_error())?;
            let stored = manager
                .get_session_ref(session_id)
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
            (
                acp_context_tokens(&stored.messages, prompt_tokens),
                acp_session_cost(&stored.messages),
            )
        };
        send_text(
            connection,
            session_id,
            &cuid2::create_id(),
            format!(
                "Context compacted ({})",
                crate::session::compaction::format_compaction_stats(stats)
            ),
            false,
        )?;
        send_usage(
            connection,
            session_id,
            session,
            used_tokens,
            (cost > 0.0).then_some(cost),
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::prompt::acp_request_messages;
    use super::super::test_support::test_session;
    use super::super::usage::acp_context_tokens;
    use super::super::usage::acp_session_cost;
    use super::super::AcpService;
    use super::super::AcpSessions;
    use super::*;
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
                        let feedback = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            observed_rx.recv(),
                        )
                        .await
                        .unwrap()
                        .unwrap();
                        assert!(matches!(feedback, SessionUpdate::AgentMessageChunk(_)));
                        let update = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            observed_rx.recv(),
                        )
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

    #[test]
    fn builds_smaller_soft_compaction_for_acp() {
        let session = test_session();
        let messages = vec![
            crate::session::types::Message::user("u".repeat(8_000)),
            crate::session::types::Message::assistant("a".repeat(8_000)),
            crate::session::types::Message::user("recent"),
        ];
        let selection = crate::session::compaction::select_messages_for_compaction_with_min(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
            0,
        )
        .expect("compaction selection");
        let before_tokens = crate::session::compaction::total_context_tokens(&messages);
        let before_messages =
            crate::session::compaction::filter_messages_for_context(&messages).len();

        let (compacted, stats) = compacted_messages(
            &messages,
            &selection,
            &crate::llm::client::CompactionSummary {
                text: "short handoff".to_string(),
                usage: Default::default(),
            },
            &session,
            before_tokens,
            before_messages,
        )
        .expect("smaller compaction");

        assert!(stats.after_tokens < stats.before_tokens);
        assert!(crate::session::compaction::latest_compaction_stats(&compacted).is_some());
        assert_eq!(compacted[0].id, messages[0].id);
    }
}
