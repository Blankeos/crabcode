use super::context::estimate_acp_tokens;
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

fn live_assistant_context_tokens(assistant: &crate::session::types::Message) -> usize {
    let persisted = crate::session::compaction::message_context_tokens(assistant);
    let reasoning = assistant
        .reasoning
        .as_deref()
        .map(estimate_acp_tokens)
        .unwrap_or(0);
    persisted.saturating_add(reasoning)
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

#[cfg(test)]
mod tests {

    use super::super::test_support::session_with_config;
    use super::super::test_support::test_session;
    use super::super::transcript::record_tool_call;
    use super::super::transcript::record_tool_result;
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
