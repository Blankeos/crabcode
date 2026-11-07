use super::models::model_reasoning_capability;
use super::usage::estimate_session_usage_cost;
use super::AcpSession;
use agent_client_protocol::Error;

pub(super) fn compaction_reasoning(
    session: &AcpSession,
) -> Option<crate::model::reasoning::ReasoningEffort> {
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

pub(super) fn compacted_messages(
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

#[cfg(test)]
mod tests {
    use super::super::test_support::test_session;
    use super::*;

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
