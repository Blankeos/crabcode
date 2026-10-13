use super::cancellable;
use super::internal_error;
use super::internal_error_with;
use super::notifications::send_text;
use super::AcpService;
use super::AcpSession;
use agent_client_protocol::schema::v1::AvailableCommand;
use agent_client_protocol::schema::v1::AvailableCommandInput;
use agent_client_protocol::schema::v1::AvailableCommandsUpdate;
use agent_client_protocol::schema::v1::ContentBlock;
use agent_client_protocol::schema::v1::PromptResponse;
use agent_client_protocol::schema::v1::StopReason;
use agent_client_protocol::schema::v1::UnstructuredCommandInput;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use tokio_util::sync::CancellationToken;

pub(super) fn command_text(parts: &[ContentBlock]) -> String {
    parts
        .iter()
        .find_map(|part| match part {
            ContentBlock::Text(content) => Some(content.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn compact_command(prompt: &str) -> Result<bool, Error> {
    let trimmed = prompt.trim();
    let Some(command_line) = trimmed.strip_prefix('/') else {
        return Ok(false);
    };
    let (name, args) = command_line
        .split_once(char::is_whitespace)
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((command_line, ""));
    if name != "compact" {
        return Ok(false);
    }
    if !args.is_empty() {
        return Err(Error::invalid_params().data("Usage: /compact"));
    }
    Ok(true)
}

pub(super) fn available_commands(session: &AcpSession) -> Vec<AvailableCommand> {
    let mut commands: Vec<_> = session
        .config
        .merged_config
        .commands
        .iter()
        .filter(|command| !matches!(command.name.as_str(), "btw" | "compact" | "skills" | "mcp"))
        .map(|command| {
            let description = command
                .description
                .clone()
                .unwrap_or_else(|| format!("Run /{}", command.name));
            AvailableCommand::new(command.name.clone(), description).input(
                AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("Arguments")),
            )
        })
        .collect();
    commands.extend(
        session
            .skills
            .all()
            .into_iter()
            .filter(|skill| !matches!(skill.name.as_str(), "btw" | "compact" | "skills" | "mcp"))
            .map(|skill| {
                AvailableCommand::new(
                    skill.name.clone(),
                    skill
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("Use the {} skill", skill.name)),
                )
                .input(AvailableCommandInput::Unstructured(
                    UnstructuredCommandInput::new("Task or context for this skill"),
                ))
            }),
    );
    commands.push(AvailableCommand::new(
        "skills",
        "List skills available in this workspace",
    ));
    commands.push(AvailableCommand::new(
        "mcp",
        "List configured MCP servers and their status",
    ));
    commands.push(AvailableCommand::new(
        "compact",
        "Summarize this session to reduce context",
    ));
    commands.push(
        AvailableCommand::new(
            "btw",
            "Ask a quick side question without changing session history",
        )
        .input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new("Question"),
        )),
    );
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    commands.dedup_by(|left, right| left.name == right.name);
    commands
}

impl AcpService {
    pub async fn available_commands(
        &self,
        session_id: &str,
    ) -> Result<AvailableCommandsUpdate, Error> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        Ok(AvailableCommandsUpdate::new(available_commands(session)))
    }

    pub(super) async fn btw_session(
        &self,
        session_id: &str,
        session: AcpSession,
        question: String,
        connection: ConnectionTo<Client>,
        cancellation: CancellationToken,
    ) -> Result<PromptResponse, Error> {
        let history = self
            .session_manager
            .lock()
            .map_err(|_| internal_error())?
            .get_session_ref(session_id)
            .map(|stored| stored.messages.clone())
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let answer = {
            let result = cancellable(
                &cancellation,
                crate::llm::client::generate_btw_answer(
                    session.provider,
                    session.model,
                    question,
                    history,
                ),
            )
            .await;
            let Some(result) = result else {
                return Ok(PromptResponse::new(StopReason::Cancelled));
            };
            result.map_err(|error| internal_error_with(&error.to_string()))?
        };
        // Serialize notification with close/delete, which cancel under this same lock.
        let sessions = self.sessions.lock().await;
        if cancellation.is_cancelled() || !sessions.attachments.contains_key(session_id) {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }
        send_text(&connection, session_id, &cuid2::create_id(), answer, false)?;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }
}

#[cfg(test)]
mod tests {
    use super::super::command_expansion::expand_slash_command;
    use super::super::command_expansion::SlashExpansion;
    use super::super::test_support::config_with_command;

    use super::super::test_support::session_with_config;
    use super::super::test_support::test_session;
    use super::*;
    use agent_client_protocol::schema::v1::AvailableCommandInput;

    use std::path::PathBuf;

    #[test]
    fn recognizes_only_exact_compact_control_command() {
        assert_eq!(compact_command("/compact").unwrap(), true);
        assert_eq!(compact_command("  /compact  ").unwrap(), true);
        assert_eq!(compact_command("/compactness").unwrap(), false);
        assert_eq!(compact_command("hello").unwrap(), false);
        assert!(compact_command("/compact extra").is_err());
    }

    #[test]
    fn advertises_compact_as_no_input_command() {
        let session = test_session();
        let command = available_commands(&session)
            .into_iter()
            .find(|command| command.name == "compact")
            .expect("compact command");
        assert_eq!(
            command.description,
            "Summarize this session to reduce context"
        );
        assert!(command.input.is_none());
    }

    #[tokio::test]
    async fn advertises_and_parses_btw_side_command() {
        let session = test_session();
        let command = available_commands(&session)
            .into_iter()
            .find(|command| command.name == "btw")
            .expect("btw command");
        assert!(command.input.is_some());
        assert_eq!(
            expand_slash_command(&session, "/btw what changed?")
                .await
                .expect("btw expansion"),
            SlashExpansion::Btw("what changed?".to_string())
        );
        assert!(expand_slash_command(&session, "/btw").await.is_err());
    }

    #[test]
    fn advertises_custom_commands_with_argument_input() {
        let config = config_with_command(crate::command::custom::CustomCommand {
            name: "review".to_string(),
            description: Some("Review selected code".to_string()),
            template: "Review: $ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });

        let session = session_with_config(config);
        let commands = available_commands(&session);

        let command = commands
            .iter()
            .find(|command| command.name == "review")
            .expect("review command");
        assert!(commands.iter().any(|command| command.name == "skills"));
        assert!(commands.iter().any(|command| command.name == "mcp"));
        assert_eq!(command.description, "Review selected code");
        assert!(matches!(
            command.input,
            Some(AvailableCommandInput::Unstructured(_))
        ));
    }
}
