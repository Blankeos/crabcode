pub(super) fn acp_context_tokens(
    messages: &[crate::session::types::Message],
    prompt_tokens: usize,
) -> usize {
    let estimated =
        crate::session::compaction::total_context_tokens(messages).saturating_add(prompt_tokens);
    // Retained tail usage still describes the old context, as does the summary's billed usage.
    let start = messages
        .iter()
        .rposition(crate::session::compaction::is_compaction_marker)
        .map(|index| index + 1)
        .unwrap_or(0);
    for (index, message) in messages.iter().enumerate().skip(start).rev() {
        if message.role != crate::session::types::MessageRole::Assistant {
            continue;
        }
        // Each tool step bills its prompt again; cumulative usage is not a context size.
        let provider_context = message
            .parts
            .iter()
            .enumerate()
            .filter_map(|(part_index, part)| {
                let usage = part.recorded_usage()?;
                let input = usage
                    .input
                    .saturating_add(usage.cache_read)
                    .saturating_add(usage.cache_write);
                if input == 0 {
                    return None;
                }
                let mut tokens =
                    usize::try_from(input.saturating_add(usage.output)).unwrap_or(usize::MAX);
                if let Some(previous_prompt) = part
                    .data
                    .get("acp_prompt_tokens")
                    .and_then(serde_json::Value::as_u64)
                {
                    tokens = tokens
                        .saturating_sub(usize::try_from(previous_prompt).unwrap_or(usize::MAX))
                        .saturating_add(prompt_tokens);
                }
                Some(tokens.saturating_add(
                    crate::session::compaction::assistant_parts_context_tokens(
                        &message.parts[part_index + 1..],
                    ),
                ))
            })
            .max()
            .filter(|tokens| *tokens > 0);
        if let Some(tokens) = provider_context {
            let following =
                crate::session::compaction::total_context_tokens(&messages[index + 1..]);
            return estimated.max(tokens.saturating_add(following));
        }
    }
    estimated
}

pub(super) fn estimate_acp_tokens(content: &str) -> usize {
    content.chars().count().saturating_add(3) / 4
}

#[cfg(test)]
mod tests {
    use super::super::transcript::record_tool_result;
    use super::*;

    #[test]
    fn acp_compaction_counts_provider_context_and_cached_input() {
        use crate::session::types::{Message, MessagePart};
        let mut assistant = Message::assistant("answer");
        assistant
            .parts
            .push(MessagePart::usage(60_000, 200, 20_000, 5_000, 0.0));
        let messages = vec![Message::user("question"), assistant];

        assert_eq!(acp_context_tokens(&messages, 1_000), 85_200);
    }

    #[test]
    fn acp_compaction_does_not_sum_context_across_tool_requests() {
        use crate::session::types::{Message, MessagePart};
        let mut assistant = Message::assistant("answer");
        assistant
            .parts
            .push(MessagePart::usage(60_000, 200, 20_000, 0, 0.0));
        assistant
            .parts
            .push(MessagePart::usage(10_000, 100, 40_000, 0, 0.0));

        assert_eq!(
            acp_context_tokens(&[Message::user("question"), assistant], 0),
            80_200
        );
    }

    #[test]
    fn acp_compaction_uses_latest_turn_usage_and_counts_pending_history() {
        use crate::session::types::{Message, MessagePart};
        let mut old = Message::assistant("old answer");
        old.parts.push(MessagePart::usage(90_000, 100, 0, 0, 0.0));
        let mut latest = Message::assistant("new answer");
        latest
            .parts
            .push(MessagePart::usage(50_000, 200, 0, 0, 0.0));
        let pending = Message::user("continue");
        let pending_tokens = crate::session::compaction::message_context_tokens(&pending);
        let messages = vec![Message::user("question"), old, latest, pending];

        assert_eq!(acp_context_tokens(&messages, 0), 50_200 + pending_tokens);
    }

    #[test]
    fn acp_compaction_counts_prompt_and_pending_tokens_without_provider_usage() {
        use crate::config::configuration::CompactionConfig;
        use crate::session::types::Message;
        let messages = vec![Message::user("history ".repeat(2_000))];
        let history_tokens = crate::session::compaction::total_context_tokens(&messages);
        let config = CompactionConfig::Settings {
            auto: true,
            prune: false,
            reserved: Some(1_000),
        };
        let context_window = Some(history_tokens as u32 + 3_000);
        assert!(!crate::session::compaction::should_auto_compact(
            &config,
            acp_context_tokens(&messages, 1_500),
            context_window,
            None,
        ));
        assert!(crate::session::compaction::should_auto_compact(
            &config,
            acp_context_tokens(&messages, 1_500) + 500,
            context_window,
            None,
        ));
    }

    #[test]
    fn acp_compaction_replaces_previous_prompt_estimate_in_provider_context() {
        use crate::session::types::{Message, MessagePart};
        let mut assistant = Message::assistant("answer");
        let mut usage = MessagePart::usage(80_000, 200, 0, 0, 0.0);
        usage.data["acp_prompt_tokens"] = serde_json::json!(5_000);
        assistant.parts.push(usage);
        let messages = vec![Message::user("question"), assistant];
        assert_eq!(acp_context_tokens(&messages, 25_000), 100_200);
        assert_eq!(acp_context_tokens(&messages, 1_000), 76_200);
    }

    #[test]
    fn acp_compaction_keeps_provider_context_when_new_input_usage_is_missing() {
        use crate::session::types::{Message, MessagePart};
        for output in [0, 100] {
            let mut previous = Message::assistant("previous answer");
            previous
                .parts
                .push(MessagePart::usage(85_000, 200, 0, 0, 0.0));
            let mut latest = Message::assistant("new answer");
            let mut usage = MessagePart::usage(0, output, 0, 0, 0.0);
            usage.data["acp_prompt_tokens"] = serde_json::json!(1_000);
            latest.parts.push(usage);
            let following_tokens = crate::session::compaction::message_context_tokens(&latest);
            assert_eq!(
                acp_context_tokens(&[Message::user("question"), previous, latest], 2_000),
                85_200 + following_tokens
            );
        }
    }

    #[test]
    fn acp_compaction_counts_tool_output_after_provider_usage() {
        use crate::session::types::{Message, MessagePart};
        let mut assistant = Message::assistant("answer");
        assistant
            .parts
            .push(MessagePart::usage(80_000, 200, 0, 0, 0.0));
        record_tool_result(
            &mut assistant,
            &crate::llm::ToolCallResult {
                tool_call_id: "call_1".into(),
                role: "tool".into(),
                name: "read".into(),
                content: serde_json::json!({"output_preview": "late tool output ".repeat(1_000)})
                    .to_string(),
            },
        );
        let mut following = Message::assistant("");
        following.parts = assistant.parts[2..].to_vec();
        let extra_tokens = crate::session::compaction::message_context_tokens(&following);
        assert!(extra_tokens > 1_000);
        assert_eq!(
            acp_context_tokens(&[Message::user("question"), assistant], 0),
            80_200 + extra_tokens
        );
    }
}
