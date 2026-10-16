use super::AcpSession;
use agent_client_protocol::schema::v1::SessionMode;
use agent_client_protocol::schema::v1::SessionModeState;

pub(super) fn mode_description(agent: &crate::agent::definition::AgentDefinition) -> &str {
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

#[cfg(test)]
mod tests {
    use super::super::config::session_config_options;
    use super::super::test_support::test_session;
    use super::*;

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
