use super::internal_error;
use super::AcpSession;
use agent_client_protocol::Error;

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

#[cfg(test)]
mod tests {
    use super::super::commands::available_commands;
    use super::super::test_support::config_with_command;

    use super::super::test_support::session_with_config;
    use super::*;
    use std::path::PathBuf;

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
