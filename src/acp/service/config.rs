use super::AcpService;
use super::AcpSession;
use crate::config::configuration::LoadedConfig;
use agent_client_protocol::schema::v1::McpServer;
use agent_client_protocol::schema::v1::SessionConfigOption;
use agent_client_protocol::schema::v1::SessionConfigOptionCategory;
use agent_client_protocol::schema::v1::SessionConfigSelectGroup;
use agent_client_protocol::schema::v1::SessionConfigSelectOption;
use agent_client_protocol::schema::v1::SessionMode;
use agent_client_protocol::schema::v1::SessionModeState;
use agent_client_protocol::schema::v1::SetSessionConfigOptionResponse;
use agent_client_protocol::Error;

pub(super) fn model_output_limit(
    config: &LoadedConfig,
    provider_id: &str,
    model_id: &str,
) -> Option<u32> {
    if let Some(limit) = config
        .merged_config
        .custom_providers
        .get(&provider_id.trim().to_ascii_lowercase())
        .and_then(|provider| provider.models.get(model_id))
        .and_then(|model| model.max_tokens)
    {
        return Some(limit);
    }
    crate::model::discovery::Discovery::new_with_config(
        Some(config.merged_config.custom_providers.clone()),
        config.merged_config.disabled_providers.clone(),
        config.merged_config.enabled_providers.clone(),
    )
    .ok()
    .and_then(|discovery| discovery.get_model_output_limit(provider_id, model_id))
}

pub(super) fn resolved_reasoning(
    session: &AcpSession,
    requested: crate::model::reasoning::ReasoningEffort,
) -> Option<crate::model::reasoning::ReasoningEffort> {
    model_reasoning_capability(
        &session.config,
        &session.models,
        &session.provider,
        &session.model,
    )
    .and_then(|capability| capability.resolve(Some(requested)))
}

fn reasoning_effort_label(effort: crate::model::reasoning::ReasoningEffort) -> &'static str {
    match effort {
        crate::model::reasoning::ReasoningEffort::None => "None",
        crate::model::reasoning::ReasoningEffort::Minimal => "Minimal",
        crate::model::reasoning::ReasoningEffort::Low => "Low",
        crate::model::reasoning::ReasoningEffort::Medium => "Medium",
        crate::model::reasoning::ReasoningEffort::High => "High",
        crate::model::reasoning::ReasoningEffort::XHigh => "Xhigh",
        crate::model::reasoning::ReasoningEffort::Max => "Max",
    }
}

pub(super) fn merge_acp_mcp_servers(config: &mut LoadedConfig, servers: Vec<McpServer>) {
    for server in servers {
        let (name, server) = match server {
            McpServer::Stdio(server) => {
                let mut command = vec![server.command.to_string_lossy().into_owned()];
                command.extend(server.args);
                (
                    server.name,
                    crate::config::configuration::McpServerConfig::Local(
                        crate::config::configuration::McpLocalConfig {
                            command,
                            cwd: None,
                            environment: server
                                .env
                                .into_iter()
                                .map(|variable| (variable.name, variable.value))
                                .collect(),
                            enabled: true,
                            timeout_ms: None,
                        },
                    ),
                )
            }
            McpServer::Http(server) => (
                server.name,
                crate::config::configuration::McpServerConfig::Remote(
                    crate::config::configuration::McpRemoteConfig {
                        url: server.url,
                        headers: server
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        enabled: true,
                        timeout_ms: None,
                        oauth_enabled: false,
                        oauth_client_id: None,
                        oauth_client_secret: None,
                        oauth_scope: None,
                    },
                ),
            ),
            McpServer::Sse(server) => (
                server.name,
                crate::config::configuration::McpServerConfig::Remote(
                    crate::config::configuration::McpRemoteConfig {
                        url: server.url,
                        headers: server
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        enabled: true,
                        timeout_ms: None,
                        oauth_enabled: false,
                        oauth_client_id: None,
                        oauth_client_secret: None,
                        oauth_scope: None,
                    },
                ),
            ),
            #[allow(unreachable_patterns)]
            _ => continue,
        };
        config.merged_config.mcp.insert(name, server);
    }
}

pub(super) fn resolve_model(config: &LoadedConfig) -> (String, String) {
    if let Ok(Some((provider, model))) =
        crate::persistence::PrefsDAO::new().and_then(|prefs| prefs.get_active_model())
    {
        return (provider, model);
    }
    config
        .merged_config
        .model
        .as_deref()
        .map(crate::app::parse_model_ref)
        .unwrap_or_else(|| ("opencode".to_string(), "big-pickle".to_string()))
}

fn mode_description(agent: &crate::agent::definition::AgentDefinition) -> &str {
    if agent.name == "compaction" && agent.description.trim().is_empty() {
        "Compaction agent. Summarizes conversation context for continued work."
    } else {
        &agent.description
    }
}

pub(super) fn session_modes(session: &AcpSession) -> SessionModeState {
    let mut modes = session
        .config
        .merged_config
        .agent_registry
        .visible_primary_agents()
        .into_iter()
        .map(|agent| {
            SessionMode::new(agent.name.clone(), agent.name.clone())
                .description(mode_description(agent))
        })
        .collect::<Vec<_>>();
    modes.sort_by(|left, right| left.name.cmp(&right.name));
    SessionModeState::new(session.agent.clone(), modes)
}

pub(super) fn session_config_options(session: &AcpSession) -> Vec<SessionConfigOption> {
    let mut mode_options = session
        .config
        .merged_config
        .agent_registry
        .visible_primary_agents()
        .into_iter()
        .map(|agent| {
            SessionConfigSelectOption::new(agent.name.clone(), agent.name.clone())
                .description(mode_description(agent))
        })
        .collect::<Vec<_>>();
    mode_options.sort_by(|left, right| left.name.cmp(&right.name));

    let mut options = vec![
        SessionConfigOption::select("mode", "Mode", session.agent.clone(), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        model_config_option(&session.models, &session.provider, &session.model),
    ];
    options.push(reasoning_config_option(session));
    options
}

fn reasoning_config_option(session: &AcpSession) -> SessionConfigOption {
    let options = [
        crate::model::reasoning::ReasoningEffort::None,
        crate::model::reasoning::ReasoningEffort::Low,
        crate::model::reasoning::ReasoningEffort::Medium,
        crate::model::reasoning::ReasoningEffort::High,
        crate::model::reasoning::ReasoningEffort::XHigh,
        crate::model::reasoning::ReasoningEffort::Max,
    ]
    .iter()
    .map(|effort| SessionConfigSelectOption::new(effort.as_str(), reasoning_effort_label(*effort)))
    .collect::<Vec<_>>();
    let current = session.reasoning_selection.as_str();
    SessionConfigOption::select("effort", "Effort", current, options)
        .category(SessionConfigOptionCategory::ThoughtLevel)
}

fn model_config_option(
    models: &[crate::model::types::Model],
    provider: &str,
    model_id: &str,
) -> SessionConfigOption {
    let mut model_groups: Vec<SessionConfigSelectGroup> = Vec::new();
    for model in models {
        let option = SessionConfigSelectOption::new(model_value(model), model.name.clone())
            .description(model.id.clone());
        if model_groups
            .last()
            .is_some_and(|group| group.group.to_string() == model.provider_id)
        {
            model_groups
                .last_mut()
                .expect("model group exists")
                .options
                .push(option);
        } else {
            model_groups.push(SessionConfigSelectGroup::new(
                model.provider_id.clone(),
                model.provider_name.clone(),
                vec![option],
            ));
        }
    }

    SessionConfigOption::select(
        "model",
        "Model",
        format!("{provider}/{model_id}"),
        model_groups,
    )
    .category(SessionConfigOptionCategory::Model)
}

fn model_value(model: &crate::model::types::Model) -> String {
    crate::model::catalog::model_ref(model)
}

pub(super) fn find_selectable_model<'a>(
    models: &'a [crate::model::types::Model],
    model_ref: &str,
) -> Result<&'a crate::model::types::Model, Error> {
    models
        .iter()
        .find(|model| model_value(model) == model_ref)
        .ok_or_else(|| Error::invalid_params().data("unknown or unavailable ACP model"))
}

pub(super) fn model_reasoning(
    config: &LoadedConfig,
    models: &[crate::model::types::Model],
    provider: &str,
    model_id: &str,
) -> Option<crate::model::reasoning::ReasoningEffort> {
    model_reasoning_capability(config, models, provider, model_id)
        .and_then(|capability| capability.resolve(None))
}

pub(super) fn model_reasoning_capability(
    config: &LoadedConfig,
    models: &[crate::model::types::Model],
    provider: &str,
    model_id: &str,
) -> Option<crate::model::reasoning::ReasoningCapability> {
    if let Some(capability) = models
        .iter()
        .find(|model| model.provider_id == provider && model.id == model_id)
        .and_then(|model| {
            crate::model::reasoning::capability_from_options(&model.reasoning_options)
        })
    {
        return Some(capability);
    }
    crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok()
    .and_then(|discovery| discovery.get_model_reasoning_capability(provider, model_id))
    .filter(|capability| !capability.values().is_empty())
}

pub(super) fn model_context_window(
    config: &LoadedConfig,
    models: &[crate::model::types::Model],
    provider: &str,
    model: &str,
) -> Option<u32> {
    if let Some(context_window) = models
        .iter()
        .find(|candidate| candidate.provider_id == provider && candidate.id == model)
        .and_then(|model| model.context_window)
    {
        return Some(context_window);
    }

    let discovery = crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok();
    discovery
        .as_ref()
        .and_then(|discovery| discovery.get_model_limit(provider, model))
        .or_else(|| {
            config
                .merged_config
                .custom_providers
                .get(provider)
                .and_then(|provider| provider.models.get(model))
                .and_then(|model| model.context_window)
        })
}

impl AcpService {
    pub async fn set_mode(
        &self,
        session_id: &str,
        mode_id: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        if session
            .config
            .merged_config
            .agent_registry
            .primary_agent(mode_id)
            .is_none_or(|agent| agent.hidden)
        {
            return Err(Error::invalid_params().data("unknown ACP mode"));
        }
        session.agent = mode_id.to_string();
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    pub async fn set_model(
        &self,
        session_id: &str,
        model_ref: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let model = find_selectable_model(&session.models, model_ref)?;
        session.provider.clone_from(&model.provider_id);
        session.model.clone_from(&model.id);
        session.reasoning = resolved_reasoning(session, session.reasoning_selection);
        session.context_window = model_context_window(
            &session.config,
            &session.models,
            &session.provider,
            &session.model,
        );
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    pub async fn set_reasoning_effort(
        &self,
        session_id: &str,
        value: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let requested = value
            .parse::<crate::model::reasoning::ReasoningEffort>()
            .map_err(|_| Error::invalid_params().data("invalid reasoning effort"))?;
        session.reasoning_selection = requested;
        session.reasoning = resolved_reasoning(session, requested);
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::config_with_command;
    use super::super::test_support::empty_config;
    use super::super::test_support::model;
    use super::super::test_support::test_session;
    use super::*;
    use agent_client_protocol::schema::v1::SessionConfigOptionCategory;
    use std::path::PathBuf;

    #[test]
    fn merges_client_mcp_servers_into_session_config() {
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
        merge_acp_mcp_servers(
            &mut config,
            vec![serde_json::from_value(serde_json::json!({
                "name": "zed-fs",
                "command": "mcp-server",
                "args": ["--stdio"],
                "env": [{"name": "ROOT", "value": "/workspace"}]
            }))
            .expect("ACP stdio MCP server")],
        );

        let server = config
            .merged_config
            .mcp
            .get("zed-fs")
            .expect("merged ACP MCP server");
        assert_eq!(server.kind(), "local");
        assert!(server.enabled());
    }

    #[test]
    fn builds_grouped_model_config_option() {
        let option = model_config_option(
            &[
                model("anthropic", "Anthropic", "claude", "Claude"),
                model("openai", "OpenAI", "gpt-5", "GPT-5"),
                model("openai", "OpenAI", "gpt-5-mini", "GPT-5 Mini"),
            ],
            "openai",
            "gpt-5",
        );

        assert_eq!(option.id.to_string(), "model");
        assert_eq!(option.category, Some(SessionConfigOptionCategory::Model));
        let agent_client_protocol::schema::v1::SessionConfigKind::Select(select) = option.kind
        else {
            panic!("model option should be a select");
        };
        assert_eq!(select.current_value.to_string(), "openai/gpt-5");
        let agent_client_protocol::schema::v1::SessionConfigSelectOptions::Grouped(groups) =
            select.options
        else {
            panic!("model options should be grouped");
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].name, "OpenAI");
        assert_eq!(groups[1].options[1].value.to_string(), "openai/gpt-5-mini");
    }

    #[test]
    fn validates_model_refs_against_selectable_catalog() {
        let models = [model("openai", "OpenAI", "gpt-5", "GPT-5")];

        assert_eq!(
            find_selectable_model(&models, "openai/gpt-5")
                .expect("known model")
                .id,
            "gpt-5"
        );
        assert!(find_selectable_model(&models, "openai/missing").is_err());
        assert!(find_selectable_model(&models, "other/gpt-5").is_err());
    }

    #[test]
    fn resolves_context_window_from_selectable_models() {
        let mut model = model("example", "Example", "large-context", "Large Context");
        model.context_window = Some(1_090_000);

        assert_eq!(
            model_context_window(&empty_config(), &[model], "example", "large-context"),
            Some(1_090_000)
        );
    }

    #[test]
    fn preserves_selected_reasoning_effort_when_model_cannot_apply_it() {
        let session = test_session();
        let option = reasoning_config_option(&session);
        assert_eq!(option.id.to_string(), "effort");
        assert_eq!(option.name, "Effort");
        assert_eq!(
            option.category,
            Some(SessionConfigOptionCategory::ThoughtLevel)
        );
        let agent_client_protocol::schema::v1::SessionConfigKind::Select(select) = option.kind
        else {
            panic!("reasoning option should be a select");
        };
        assert_eq!(select.current_value.to_string(), "high");
        let agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(options) =
            select.options
        else {
            panic!("reasoning options should be ungrouped");
        };
        let values = options
            .iter()
            .map(|option| (option.value.to_string(), option.name.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                ("none".to_string(), "None".to_string()),
                ("low".to_string(), "Low".to_string()),
                ("medium".to_string(), "Medium".to_string()),
                ("high".to_string(), "High".to_string()),
                ("xhigh".to_string(), "Xhigh".to_string()),
                ("max".to_string(), "Max".to_string()),
            ]
        );
    }

    #[test]
    fn compaction_mode_descriptions_preserve_configuration_and_visibility() {
        let mut session = test_session();
        assert!(!session_modes(&session)
            .available_modes
            .iter()
            .any(|mode| mode.id.to_string() == "compaction"));

        for description in ["", " \n ", "Use this agent to preserve project decisions."] {
            let definitions = crate::agent::definition::parse_agent_definitions_from_config(
                Some(&serde_json::json!({"compaction": {
                    "mode": "primary", "description": description
                }})),
                &mut Vec::new(),
            );
            session.config.merged_config.agent_registry =
                crate::agent::definition::AgentRegistry::with_definitions(None, definitions);
            let expected = if description.trim().is_empty() {
                "Compaction agent. Summarizes conversation context for continued work."
            } else {
                description
            };
            let modes = session_modes(&session);
            let compaction = modes
                .available_modes
                .iter()
                .find(|mode| mode.id.to_string() == "compaction")
                .expect("configured compaction mode");
            assert_eq!(compaction.description.as_deref(), Some(expected));
            let config = serde_json::to_value(session_config_options(&session)).unwrap();
            let compaction = config[0]["options"]
                .as_array()
                .unwrap()
                .iter()
                .find(|option| option["value"] == "compaction")
                .expect("configured compaction option");
            assert_eq!(compaction["description"], expected);
            assert_eq!(
                modes
                    .available_modes
                    .iter()
                    .find(|mode| mode.id.to_string() == "build")
                    .unwrap()
                    .description
                    .as_deref(),
                Some("The default agent. Executes tools based on configured permissions.")
            );
        }

        for settings in [
            serde_json::json!({"mode": "subagent"}),
            serde_json::json!({"mode": "primary", "hidden": true}),
        ] {
            let definitions = crate::agent::definition::parse_agent_definitions_from_config(
                Some(&serde_json::json!({"compaction": settings})),
                &mut Vec::new(),
            );
            session.config.merged_config.agent_registry =
                crate::agent::definition::AgentRegistry::with_definitions(None, definitions);
            assert!(!session_modes(&session)
                .available_modes
                .iter()
                .any(|mode| mode.id.to_string() == "compaction"));
            let config = serde_json::to_value(session_config_options(&session)).unwrap();
            assert!(!config[0]["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|option| option["value"] == "compaction"));
        }
    }
}
