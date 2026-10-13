use super::internal_error;
use super::AcpSession;
use agent_client_protocol::schema::v1::Cost as AcpCost;
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::schema::v1::SessionUpdate;
use agent_client_protocol::schema::v1::UsageUpdate;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;

pub(super) fn acp_session_cost(messages: &[crate::session::types::Message]) -> f64 {
    messages
        .iter()
        .map(|message| message.cost.unwrap_or_else(|| message.usage_cost()))
        .sum()
}

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

pub(super) fn estimate_session_usage_cost(
    session: &AcpSession,
    usage: &crate::aisdk::chunk::TokenUsage,
) -> f64 {
    crate::model::discovery::Discovery::new_with_custom(Some(
        session.config.merged_config.custom_providers.clone(),
    ))
    .ok()
    .map(|discovery| {
        discovery.estimate_usage_cost(
            &session.provider,
            &session.model,
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
        )
    })
    .unwrap_or(0.0)
}

pub(super) fn send_usage(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    session: &AcpSession,
    used: usize,
    cost: Option<f64>,
    usage: Option<crate::aisdk::chunk::TokenUsage>,
) -> Result<(), Error> {
    let update = SessionUpdate::UsageUpdate(usage_update(session, used, cost, usage));
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

pub(super) fn send_live_usage(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    session: &AcpSession,
    base_context_tokens: usize,
    base_cost: f64,
    assistant: &crate::session::types::Message,
    live_usage: &AcpLiveUsage,
) -> Result<(), Error> {
    send_usage(
        connection,
        session_id,
        session,
        live_usage.context_tokens(base_context_tokens, assistant),
        cumulative_cost(base_cost, live_usage.cost),
        (!live_usage.cumulative.is_empty()).then_some(live_usage.cumulative),
    )
}

#[derive(Debug, Default)]
pub(super) struct AcpLiveUsage {
    pub(super) cumulative: crate::aisdk::chunk::TokenUsage,
    provider_context_floor: usize,
    pub(super) cost: f64,
}

impl AcpLiveUsage {
    pub(super) fn observe_provider_usage(
        &mut self,
        assistant: &crate::session::types::Message,
        usage: crate::aisdk::chunk::TokenUsage,
    ) {
        self.cumulative = self.cumulative.saturating_add(usage);
        let provider_input = usage
            .input
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write);
        let observed_assistant = live_assistant_context_tokens(assistant);
        self.provider_context_floor = self
            .provider_context_floor
            .max(usize::try_from(provider_input).unwrap_or(usize::MAX))
            .max(observed_assistant);
    }

    pub(super) fn context_tokens(
        &self,
        base_context_tokens: usize,
        assistant: &crate::session::types::Message,
    ) -> usize {
        base_context_tokens
            .saturating_add(live_assistant_context_tokens(assistant))
            .max(self.provider_context_floor)
    }
}

fn live_assistant_context_tokens(assistant: &crate::session::types::Message) -> usize {
    let persisted = crate::session::compaction::message_context_tokens(assistant);
    let reasoning = assistant
        .reasoning
        .as_deref()
        .map(estimate_acp_tokens)
        .unwrap_or(0);
    persisted.saturating_add(reasoning)
}

fn estimate_acp_tokens(content: &str) -> usize {
    content.chars().count().saturating_add(3) / 4
}

pub(super) fn cumulative_cost(base_cost: f64, turn_cost: f64) -> Option<f64> {
    let cost = base_cost + turn_cost;
    (cost > 0.0).then_some(cost)
}

fn usage_update(
    session: &AcpSession,
    used: usize,
    cost: Option<f64>,
    usage: Option<crate::aisdk::chunk::TokenUsage>,
) -> UsageUpdate {
    let size = session
        .context_window
        .map(u64::from)
        .unwrap_or_else(|| (used as u64).max(1));
    let mut meta = serde_json::Map::new();
    meta.insert(
        "crabcode".to_string(),
        serde_json::json!({
            "contextWindowKnown": session.context_window.is_some(),
            "usage": usage.map(|usage| serde_json::json!({
                "inputTokens": usage.input,
                "outputTokens": usage.output,
                "cacheReadTokens": usage.cache_read,
                "cacheWriteTokens": usage.cache_write,
            })),
        }),
    );
    UsageUpdate::new(used as u64, size)
        .cost(cost.map(|amount| AcpCost::new(amount, "USD")))
        .meta(meta)
}

#[cfg(test)]
mod tests {
    use super::super::stream::record_tool_call;
    use super::super::stream::record_tool_result;
    use super::super::test_support::session_with_config;
    use super::super::test_support::test_session;
    use super::*;
    use crate::config::configuration::LoadedConfig;
    use std::path::PathBuf;

    #[test]
    fn acp_usage_update_includes_cumulative_usd_cost() {
        let usage = crate::aisdk::chunk::TokenUsage {
            input: 800,
            output: 200,
            cache_read: 500,
            cache_write: 100,
        };
        let update = usage_update(&test_session(), 1_000, Some(0.125), Some(usage));
        assert_eq!(update.cost.as_ref().map(|cost| cost.amount), Some(0.125));
        assert_eq!(
            update.cost.as_ref().map(|cost| cost.currency.as_str()),
            Some("USD")
        );
        assert_eq!(update.size, 1_000);
        let meta = update.meta.expect("usage metadata");
        assert_eq!(meta["crabcode"]["contextWindowKnown"], false);
        assert_eq!(meta["crabcode"]["usage"]["inputTokens"], 800);
        assert_eq!(meta["crabcode"]["usage"]["cacheWriteTokens"], 100);
    }

    #[test]
    fn acp_live_usage_advances_with_streamed_tools_and_provider_context() {
        let mut assistant = crate::session::types::Message::incomplete("hello");
        record_tool_call(
            &mut assistant,
            &crate::llm::ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: crate::llm::FunctionCall {
                    name: "read".to_string(),
                    arguments: serde_json::json!({ "filePath": "src/main.rs" }).to_string(),
                },
            },
        );
        record_tool_result(
            &mut assistant,
            &crate::llm::ToolCallResult {
                tool_call_id: "call_1".to_string(),
                role: "tool".to_string(),
                name: "read".to_string(),
                content: serde_json::json!({
                    "status": "ok",
                    "output_preview": "x".repeat(4_000),
                })
                .to_string(),
            },
        );

        let mut live = AcpLiveUsage::default();
        let before_provider_usage = live.context_tokens(18_000, &assistant);
        assert!(before_provider_usage > 18_000);
        assert!(assistant
            .tool_call_part_data("call_1")
            .and_then(|part| part.get("args"))
            .is_some());

        live.observe_provider_usage(
            &assistant,
            crate::aisdk::chunk::TokenUsage {
                input: 70_000,
                output: 500,
                cache_read: 20_000,
                cache_write: 0,
            },
        );
        assert_eq!(live.context_tokens(18_000, &assistant), 90_000);
        assert_eq!(live.cumulative.input, 70_000);
        assert_eq!(live.cumulative.cache_read, 20_000);
    }

    #[test]
    fn acp_live_usage_keeps_largest_provider_context_across_tool_steps() {
        let assistant = crate::session::types::Message::incomplete("");
        let mut live = AcpLiveUsage::default();
        live.observe_provider_usage(
            &assistant,
            crate::aisdk::chunk::TokenUsage {
                input: 60_000,
                output: 200,
                cache_read: 20_000,
                cache_write: 0,
            },
        );
        live.observe_provider_usage(
            &assistant,
            crate::aisdk::chunk::TokenUsage {
                input: 10_000,
                output: 100,
                cache_read: 40_000,
                cache_write: 0,
            },
        );

        assert_eq!(live.context_tokens(18_000, &assistant), 80_000);
        assert_eq!(live.cumulative.input, 70_000);
        assert_eq!(live.cumulative.output, 300);
        assert_eq!(live.cumulative.cache_read, 60_000);
    }

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

    #[test]
    fn unknown_model_usage_cost_is_zero() {
        let mut session = session_with_config(LoadedConfig {
            merged_config: crate::config::configuration::MergedConfig::default(),
            raw_merged: serde_json::Value::Null,
            diagnostics: Default::default(),
            inventory: Default::default(),
            project_root: PathBuf::from("/tmp"),
            cwd: PathBuf::from("/tmp"),
            xdg_config_home: PathBuf::from("/tmp"),
        });
        session.provider = "no-such-provider".to_string();
        session.model = "no-such-model".to_string();
        let usage = crate::aisdk::chunk::TokenUsage {
            input: 1_000,
            output: 1_000,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(estimate_session_usage_cost(&session, &usage), 0.0);
    }
}
