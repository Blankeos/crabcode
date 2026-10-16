use super::cancellable;
use super::compaction_summary::compacted_messages;
use super::compaction_summary::compaction_reasoning;
use super::context::acp_context_tokens;
use super::internal_error;
use super::internal_error_with;
use super::models::model_output_limit;
use super::notifications::send_text;
use super::usage::acp_session_cost;
use super::usage::send_usage;
use super::AcpService;
use super::AcpSession;
use agent_client_protocol::schema::v1::PromptResponse;
use agent_client_protocol::schema::v1::StopReason;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use tokio_util::sync::CancellationToken;

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
#[path = "tests/compaction.rs"]
mod tests;
