//! Background preparation and execution of a single model turn.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{client::stream_llm_with_cancellation, ChunkMessage, ChunkSender};
use crate::agent::definition::AgentRegistry;
use crate::config::{
    configuration::{CompactionConfig, McpConfig, WebsearchConfig},
    ProviderTimeout,
};
use crate::model::reasoning::ReasoningEffort;
use crate::session::{
    compaction::filter_messages_for_context,
    types::{Message, MessageRole},
};
use crate::tools::{ProcessRegistry, ToolPermissions};

pub(crate) struct TurnRequest {
    pub(crate) session_id: String,
    pub(crate) provider_name: String,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) agent_mode: String,
    pub(crate) agent_max_steps: Option<usize>,
    pub(crate) agent_registry: AgentRegistry,
    pub(crate) tool_permissions: ToolPermissions,
    pub(crate) websearch_config: WebsearchConfig,
    pub(crate) mcp_config: McpConfig,
    pub(crate) compaction_config: CompactionConfig,
    pub(crate) custom_instructions: String,
    pub(crate) process_registry: Arc<ProcessRegistry>,
    pub(crate) cwd: String,
    pub(crate) provider_timeout: Option<ProviderTimeout>,
    pub(crate) turn_guidance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(turn_guidance: Option<&str>) -> TurnRequest {
        TurnRequest {
            session_id: "turn-test".into(),
            provider_name: "unused-provider".into(),
            model: "unused-model".into(),
            reasoning_effort: None,
            agent_mode: "build".into(),
            agent_max_steps: None,
            agent_registry: AgentRegistry::default(),
            tool_permissions: ToolPermissions::new(".".to_string()),
            websearch_config: WebsearchConfig::default(),
            mcp_config: McpConfig::default(),
            compaction_config: CompactionConfig::default(),
            custom_instructions: String::new(),
            process_registry: Arc::new(ProcessRegistry::with_workdir(".".into())),
            cwd: ".".into(),
            provider_timeout: Some(ProviderTimeout::Millis(0)),
            turn_guidance: turn_guidance.map(str::to_owned),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn spawn_can_be_cancelled_before_any_preparation_runs() {
        // No system prompt: polling preparation would initialize tools and
        // compose a prompt. On a current-thread runtime spawn must return first.
        let history = vec![Message::user("new request")];
        let original = history.clone();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();
        request(Some("resume unfinished work")).spawn(&history, sender, cancel_token.clone());
        cancel_token.cancel();

        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(history, original);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn already_cancelled_spawn_emits_no_terminal_or_provider_chunks() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        request(None).spawn(&[], sender, cancel_token);

        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guidance_augments_prepared_request_without_changing_transcript() {
        let history = vec![Message::system("base prompt"), Message::user("new request")];
        let original = history.clone();
        let mut messages = filter_messages_for_context(&history);
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        request(Some("  resume unfinished work  "))
            .prepare_messages(&mut messages, &sender, &CancellationToken::new())
            .await;

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(messages[0].content.starts_with("base prompt"));
        assert!(messages[0].content.ends_with("\n\nresume unfinished work"));
        assert_eq!(messages[1], history[1]);
        assert_eq!(history, original);
    }

    #[test]
    fn no_guidance_leaves_messages_unchanged() {
        for guidance in [None, Some(""), Some(" \n\t ")] {
            let mut messages = vec![Message::system("base prompt")];
            let expected = messages.clone();
            apply_turn_guidance(&mut messages, guidance);
            assert_eq!(messages, expected);
        }
    }

    #[test]
    fn guidance_creates_missing_system_message_or_fills_empty_one() {
        let user = Message::user("new request");
        let mut messages = vec![user.clone()];
        apply_turn_guidance(&mut messages, Some(" guidance "));
        assert_eq!(messages[0].role, MessageRole::System);
        assert_eq!(messages[0].content, "guidance");
        assert_eq!(messages[1], user);

        let mut messages = vec![Message::system("")];
        apply_turn_guidance(&mut messages, Some("guidance"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "guidance");
    }

    #[test]
    fn compacted_snapshot_excludes_archived_system_prompt_and_markers() {
        use crate::session::compaction::{compaction_marker, SUMMARY_PREFIX};
        use crate::session::types::CompactionStats;

        let summary = Message::user(format!("{SUMMARY_PREFIX}\nsummary"));
        let tail = Message::user("recent request");
        let history = vec![
            Message::system("archived prompt"),
            Message::user("archived request"),
            summary.clone(),
            tail.clone(),
            compaction_marker(CompactionStats {
                before_tokens: 1000,
                after_tokens: 100,
                before_messages: 20,
                after_messages: 2,
            }),
        ];
        let original = history.clone();
        let mut messages = filter_messages_for_context(&history);

        assert_eq!(messages, vec![summary, tail]);
        // Neither the archived prompt nor the system-role marker should stop
        // preparation from composing a fresh system prompt.
        assert!(!messages.iter().any(|m| m.role == MessageRole::System));
        apply_turn_guidance(&mut messages, Some("resume unfinished work"));
        assert_eq!(messages[0].content, "resume unfinished work");
        assert_eq!(history, original);
    }
}

impl TurnRequest {
    /// Snapshot only active context on the caller; all prompt/tool/provider work
    /// runs in the spawned task so submitting a turn never awaits preparation.
    pub(crate) fn spawn(
        self,
        history: &[Message],
        sender: ChunkSender,
        cancel_token: CancellationToken,
    ) {
        let messages = filter_messages_for_context(history);
        tokio::spawn(self.run(messages, sender, cancel_token));
    }

    async fn run(
        self,
        mut messages: Vec<Message>,
        sender: ChunkSender,
        cancel_token: CancellationToken,
    ) {
        tokio::select! {
            biased;
            _ = cancel_token.cancelled() => return,
            _ = self.prepare_messages(&mut messages, &sender, &cancel_token) => {}
        }

        // Prompt setup is outside the provider timeout. The streaming path still
        // builds its own registry; the prompt-only registry is not reused.
        let stream = stream_llm_with_cancellation(
            cancel_token,
            self.session_id,
            self.provider_name,
            self.model,
            self.reasoning_effort,
            self.agent_mode,
            self.agent_max_steps,
            self.agent_registry,
            self.tool_permissions,
            self.websearch_config,
            self.mcp_config,
            self.compaction_config,
            self.cwd,
            None,
            messages,
            sender.clone(),
            self.process_registry,
        );

        let result: Result<Result<(), Box<dyn std::error::Error>>, u64> =
            match self.provider_timeout {
                Some(ProviderTimeout::Millis(ms)) => {
                    match tokio::time::timeout(std::time::Duration::from_millis(ms), stream).await {
                        Ok(inner) => Ok(inner),
                        Err(_) => Err(ms),
                    }
                }
                Some(ProviderTimeout::Disabled) | None => Ok(stream.await),
            };

        let _ = match result {
            Ok(Ok(())) => sender.send(ChunkMessage::End),
            Ok(Err(e)) => sender.send(ChunkMessage::Failed(e.to_string())),
            Err(ms) => sender.send(ChunkMessage::Failed(format!(
                "Timeout: No response within {} ms",
                ms
            ))),
        };
    }

    async fn prepare_messages(
        &self,
        messages: &mut Vec<Message>,
        sender: &ChunkSender,
        cancel_token: &CancellationToken,
    ) {
        let has_system = messages.iter().any(|m| m.role == MessageRole::System);
        if !has_system {
            let registry = crate::tools::initialize_tool_registry_with_dynamic_config(
                Some(sender.clone()),
                self.tool_permissions.clone(),
                self.agent_registry.clone(),
                cancel_token.clone(),
                Some(&self.provider_name),
                &self.websearch_config,
                &self.mcp_config,
                &self.cwd,
                self.process_registry.clone(),
            )
            .await;
            let prompt_registry = crate::tools::scope_tool_registry_for_agent(
                &registry,
                &self.tool_permissions,
                &self.agent_mode,
            )
            .await;
            let is_git_repo = crate::utils::git::is_git_repo(&self.cwd).unwrap_or(false);
            let system_prompt = crate::prompt::SystemPromptComposer::new(
                &self.model,
                &self.cwd,
                is_git_repo,
                std::env::consts::OS,
            )
            .with_tool_registry(prompt_registry)
            .with_agent_registry(self.agent_registry.clone())
            .with_active_agent(self.agent_mode.clone())
            .with_custom_instructions(self.custom_instructions.clone())
            .compose()
            .await;
            messages.insert(0, Message::system(system_prompt));
        } else if let Some(store) = crate::skill::get_skill_store() {
            if let Some(message) = messages.iter_mut().find(|m| m.role == MessageRole::System) {
                if crate::prompt::refresh_skill_guidance(&mut message.content, store.all()) {
                    message.token_count = None;
                }
            }
        }
        apply_turn_guidance(messages, self.turn_guidance.as_deref());
    }
}

fn apply_turn_guidance(messages: &mut Vec<Message>, turn_guidance: Option<&str>) {
    let Some(guidance) = turn_guidance
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    if let Some(system_message) = messages
        .iter_mut()
        .find(|message| message.role == MessageRole::System)
    {
        if !system_message.content.trim().is_empty() {
            system_message.content.push_str("\n\n");
        }
        system_message.content.push_str(guidance);
    } else {
        messages.insert(0, Message::system(guidance));
    }
}
