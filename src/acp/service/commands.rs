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

fn available_commands(session: &AcpSession) -> Vec<AvailableCommand> {
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

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SlashExpansion {
    Prompt {
        prompt: String,
        agent: Option<String>,
        model: Option<String>,
    },
    LocalResult(String),
    Btw(String),
}

pub(super) async fn expand_slash_command(
    session: &AcpSession,
    prompt: &str,
) -> Result<SlashExpansion, Error> {
    let Some(command_line) = prompt.strip_prefix('/') else {
        return Ok(SlashExpansion::Prompt {
            prompt: prompt.to_string(),
            agent: None,
            model: None,
        });
    };
    let (name, args) = command_line
        .split_once(char::is_whitespace)
        .map(|(name, args)| (name, args.trim_start()))
        .unwrap_or((command_line, ""));
    if name == "btw" {
        let question = args.trim();
        if question.is_empty() {
            return Err(Error::invalid_params().data("Usage: /btw <question>"));
        }
        return Ok(SlashExpansion::Btw(question.to_string()));
    }
    if name == "skills" {
        if !args.is_empty() {
            return Err(Error::invalid_params().data("Usage: /skills"));
        }
        let skills = session
            .skills
            .all()
            .into_iter()
            .map(|skill| {
                format!(
                    "- /{} — {}",
                    skill.name,
                    skill.description.as_deref().unwrap_or("No description")
                )
            })
            .collect::<Vec<_>>();
        return Ok(SlashExpansion::LocalResult(if skills.is_empty() {
            "No skills are available in this workspace.".to_string()
        } else {
            format!("Available skills:\n{}", skills.join("\n"))
        }));
    }
    if name == "mcp" {
        if !args.is_empty() {
            return Err(Error::invalid_params().data("Usage: /mcp"));
        }
        let manager = crate::mcp::McpManager::ensure(
            session.config.merged_config.mcp.clone(),
            session.cwd.clone(),
        );
        let servers = manager
            .lock()
            .await
            .views()
            .into_iter()
            .map(|server| {
                let detail = server
                    .detail
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default();
                format!(
                    "- {} ({}, {}){}",
                    server.name, server.kind, server.status, detail
                )
            })
            .collect::<Vec<_>>();
        return Ok(SlashExpansion::LocalResult(if servers.is_empty() {
            "No MCP servers are configured for this workspace.".to_string()
        } else {
            format!("MCP servers:\n{}", servers.join("\n"))
        }));
    }
    if let Some(command) = session
        .config
        .merged_config
        .commands
        .iter()
        .find(|command| command.name == name)
    {
        return command
            .render(args)
            .await
            .map_err(|_| internal_error())
            .map(|rendered| SlashExpansion::Prompt {
                prompt: rendered.prompt,
                agent: rendered.agent,
                model: rendered.model,
            });
    }
    if let Some(skill) = session.skills.get(name) {
        let mut expanded = skill.content.clone();
        if !args.is_empty() {
            expanded.push_str("\n\nUser task/context:\n");
            expanded.push_str(args);
        }
        return Ok(SlashExpansion::Prompt {
            prompt: expanded,
            agent: None,
            model: None,
        });
    }
    Err(Error::invalid_params().data(format!("Unknown ACP command: /{name}")))
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

    #[tokio::test]
    async fn expands_custom_slash_command_before_prompting() {
        let config = config_with_command(crate::command::custom::CustomCommand {
            name: "review".to_string(),
            description: None,
            template: "Review this carefully: $ARGUMENTS".to_string(),
            agent: Some("build".to_string()),
            model: Some("openai/gpt-5".to_string()),
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });

        let session = session_with_config(config);
        let prompt = expand_slash_command(&session, "/review src/acp/service.rs")
            .await
            .expect("expanded command");

        assert_eq!(
            prompt,
            SlashExpansion::Prompt {
                prompt: "Review this carefully: src/acp/service.rs".to_string(),
                agent: Some("build".to_string()),
                model: Some("openai/gpt-5".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn advertises_and_expands_workspace_skills() {
        let temp = tempfile::tempdir().expect("temp dir");
        let skill_dir = temp.path().join(".opencode/skill/reviewer");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: reviewer\ndescription: Review code carefully\n---\nInspect correctness and risks.",
        )
        .expect("skill file");
        let mut config = config_with_command(crate::command::custom::CustomCommand {
            name: "custom".to_string(),
            description: None,
            template: "$ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(
                temp.path().join("opencode.jsonc"),
            ),
            workdir: temp.path().to_path_buf(),
        });
        config.project_root = temp.path().to_path_buf();
        config.cwd = temp.path().to_path_buf();
        config.xdg_config_home = temp.path().join("config");
        let session = session_with_config(config);

        let commands = available_commands(&session);
        assert!(commands.iter().any(|command| command.name == "reviewer"));
        let prompt = expand_slash_command(&session, "/reviewer src/lib.rs")
            .await
            .expect("expanded skill");
        let SlashExpansion::Prompt { prompt, .. } = prompt else {
            panic!("expected skill prompt");
        };
        assert!(prompt.contains("Inspect correctness and risks."));
        assert!(prompt.contains("src/lib.rs"));
    }

    #[tokio::test]
    async fn reports_configured_mcp_servers() {
        let mut config = config_with_command(crate::command::custom::CustomCommand {
            name: "custom".to_string(),
            description: None,
            template: "$ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });
        config.merged_config.mcp.insert(
            "filesystem".to_string(),
            crate::config::configuration::McpServerConfig::Local(
                crate::config::configuration::McpLocalConfig {
                    command: vec!["mcp-server".to_string()],
                    cwd: None,
                    environment: Default::default(),
                    enabled: true,
                    timeout_ms: None,
                },
            ),
        );
        let session = session_with_config(config);

        let prompt = expand_slash_command(&session, "/mcp")
            .await
            .expect("mcp status");
        let SlashExpansion::LocalResult(prompt) = prompt else {
            panic!("expected local MCP result");
        };
        assert!(prompt.contains("filesystem (local, connecting)"));
    }
}
