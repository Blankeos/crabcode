use super::permissions::configured_tool_permissions;
use super::AcpSession;
use crate::config::configuration::LoadedConfig;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn model(
    provider_id: &str,
    provider_name: &str,
    id: &str,
    name: &str,
) -> crate::model::types::Model {
    crate::model::types::Model {
        id: id.to_string(),
        name: name.to_string(),
        family: String::new(),
        provider_id: provider_id.to_string(),
        provider_name: provider_name.to_string(),
        attachment: false,
        structured_output: false,
        free: false,
        local: false,
        reasoning_options: Vec::new(),
        context_window: None,
    }
}

pub(super) fn reasoning_model() -> crate::model::types::Model {
    let mut model = model("openai", "OpenAI", "o3", "o3");
    model.reasoning_options = vec![crate::model::reasoning::ReasoningOption {
        kind: "effort".to_string(),
        values: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
    }];
    model
}

pub(super) fn test_session() -> AcpSession {
    AcpSession {
        cwd: PathBuf::from("/tmp"),
        config: crate::config::configuration::LoadedConfig {
            merged_config: crate::config::configuration::MergedConfig::default(),
            raw_merged: serde_json::Value::Null,
            diagnostics: Default::default(),
            inventory: Default::default(),
            project_root: PathBuf::from("/tmp"),
            cwd: PathBuf::from("/tmp"),
            xdg_config_home: PathBuf::from("/tmp"),
        },
        tool_permissions: crate::tools::ToolPermissions::new("/tmp"),
        skills: crate::skill::SkillStore::load(Path::new("/tmp"), Path::new("/tmp")),
        models: vec![model("example", "Example", "chat", "Chat")],
        provider: "example".to_string(),
        model: "chat".to_string(),
        agent: "Build".to_string(),
        reasoning_selection: crate::model::reasoning::ReasoningEffort::High,
        reasoning: None,
        context_window: None,
        cancellation: None,
    }
}

pub(super) fn config_with_command(command: crate::command::custom::CustomCommand) -> LoadedConfig {
    let mut merged_config = crate::config::configuration::MergedConfig::default();
    merged_config.commands.push(command);
    config_with_merged(merged_config)
}

pub(super) fn config_with_merged(
    merged_config: crate::config::configuration::MergedConfig,
) -> LoadedConfig {
    LoadedConfig {
        merged_config,
        raw_merged: serde_json::Value::Null,
        diagnostics: Default::default(),
        inventory: Default::default(),
        project_root: PathBuf::from("/tmp"),
        cwd: PathBuf::from("/tmp"),
        xdg_config_home: PathBuf::from("/tmp"),
    }
}

pub(super) fn empty_config() -> LoadedConfig {
    config_with_merged(crate::config::configuration::MergedConfig::default())
}

pub(super) fn session_with_config(config: LoadedConfig) -> AcpSession {
    let skills = crate::skill::SkillStore::load(&config.xdg_config_home, &config.project_root);
    AcpSession {
        cwd: config.cwd.clone(),
        tool_permissions: configured_tool_permissions(&config.cwd, &config),
        config,
        skills,
        models: Vec::new(),
        provider: String::new(),
        model: String::new(),
        agent: "Build".to_string(),
        reasoning_selection: crate::model::reasoning::ReasoningEffort::None,
        reasoning: None,
        context_window: None,
        cancellation: None,
    }
}
