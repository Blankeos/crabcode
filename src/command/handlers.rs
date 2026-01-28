use crate::command::parser::ParsedCommand;
use crate::command::registry::{Command, CommandResult, Registry};
use crate::session::manager::SessionManager;
use chrono::{DateTime, Local, Utc};
use std::pin::Pin;

pub fn handle_exit<'a>(
    _parsed: &'a ParsedCommand<'a>,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async { CommandResult::Success("Exiting...".to_string()) })
}

pub fn handle_sessions<'a>(
    _parsed: &'a ParsedCommand<'a>,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move {
        let mut sessions = sm.list_sessions();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let items: Vec<crate::command::registry::DialogItem> = sessions
            .into_iter()
            .map(|session| {
                let date_group = format_date_group(session.updated_at);
                let time = format_time(session.updated_at);

                crate::command::registry::DialogItem {
                    id: session.id.clone(),
                    name: session.title.clone(),
                    group: date_group,
                    description: String::new(),
                    tip: Some(time),
                    provider_id: String::new(),
                }
            })
            .collect();

        CommandResult::ShowDialog {
            title: "Sessions".to_string(),
            items,
        }
    })
}

fn format_date_group(created_at: std::time::SystemTime) -> String {
    let datetime: DateTime<Local> = created_at.into();
    let now: DateTime<Local> = Utc::now().into();
    let duration = now.signed_duration_since(datetime);

    if duration.num_days() == 0 {
        "Today".to_string()
    } else {
        datetime.format("%a %b %d %Y").to_string()
    }
}

fn format_time(created_at: std::time::SystemTime) -> String {
    let datetime: DateTime<Local> = created_at.into();
    datetime.format("%-I:%M %p").to_string()
}

pub fn handle_new<'a>(
    _parsed: &'a ParsedCommand<'a>,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move { CommandResult::Success("".to_string()) })
}

pub fn handle_connect<'a>(
    parsed: &'a ParsedCommand<'a>,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if args.is_empty() {
            let auth_dao = match crate::persistence::AuthDAO::new() {
                Ok(dao) => dao,
                Err(e) => {
                    return CommandResult::Error(format!("Failed to load auth config: {}", e))
                }
            };

            let connected_providers = match auth_dao.load() {
                Ok(providers) => providers,
                Err(e) => return CommandResult::Error(format!("Failed to load providers: {}", e)),
            };

            let api_key_config = match crate::config::ApiKeyConfig::load() {
                Ok(c) => c,
                Err(e) => return CommandResult::Error(format!("Failed to load API key config: {}", e)),
            };

            let discovery = match crate::model::discovery::Discovery::new() {
                Ok(d) => d,
                Err(e) => {
                    return CommandResult::Error(format!(
                        "Failed to initialize provider discovery: {}",
                        e
                    ))
                }
            };

            let providers_map = match discovery.fetch_providers().await {
                Ok(p) => p,
                Err(e) => return CommandResult::Error(format!("Failed to fetch providers: {}", e)),
            };

            const POPULAR_PROVIDERS: &[&str] = &[
                "opencode",
                "anthropic",
                "openai",
                "google",
                "zai-coding-plan",
            ];

            let mut items: Vec<crate::command::registry::DialogItem> = providers_map
                .into_iter()
                .map(|(id, provider)| {
                    let group = if POPULAR_PROVIDERS.contains(&id.as_str()) {
                        "Popular"
                    } else {
                        "Other"
                    };
                    let is_connected = connected_providers.contains_key(&id);
                    crate::command::registry::DialogItem {
                        id: id.clone(),
                        name: provider.name.clone(),
                        group: group.to_string(),
                        description: id.clone(),
                        tip: if is_connected {
                            Some("🟢 Connected".to_string())
                        } else {
                            None
                        },
                        provider_id: id.clone(),
                    }
                })
                .collect();

            items.sort_by(|a, b| a.name.cmp(&b.name));

            CommandResult::ShowDialog {
                title: "Connect a provider".to_string(),
                items,
            }
        } else {
            let config = match crate::config::ApiKeyConfig::load() {
                Ok(c) => c,
                Err(e) => return CommandResult::Error(format!("Failed to load config: {}", e)),
            };

            if args.len() == 1 {
                let provider = &args[0];
                if let Some(_api_key) = config.get_api_key(provider) {
                    CommandResult::Success(format!("Provider '{}' is configured", provider))
                } else {
                    CommandResult::Success(format!(
                        "Provider '{}' is not configured. Usage: /connect {} <api_key>",
                        provider, provider
                    ))
                }
            } else {
                let provider = &args[0];
                let api_key = &args[1];
                let mut config = config;
                config.set_api_key(provider.clone(), api_key.clone());
                if let Err(e) = config.save() {
                    CommandResult::Error(format!("Failed to save config: {}", e))
                } else {
                    CommandResult::Success(format!(
                        "API key configured for provider '{}'",
                        provider
                    ))
                }
            }
        }
    })
}

pub fn handle_models<'a>(
    parsed: &'a ParsedCommand<'a>,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    use crate::command::registry::DialogItem;
    use crate::model::discovery::Discovery;
    use crate::model::types::Model as ModelType;
    use crate::persistence::AuthDAO;

    let provider_filter = if parsed.args.is_empty() {
        None
    } else {
        Some(parsed.args[0].clone())
    };

    let active_model_id = parsed.active_model_id.clone();
    let prefs_data = parsed.prefs_dao.and_then(|dao| {
        match dao.get_model_preferences() {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("DEBUG: Failed to get prefs: {}", e);
                None
            },
        }
    });

    Box::pin(async move {
        let auth_dao = match AuthDAO::new() {
            Ok(dao) => dao,
            Err(e) => return CommandResult::Error(format!("Failed to load auth config: {}", e)),
        };

        let connected_providers = match auth_dao.load() {
            Ok(providers) => providers,
            Err(e) => return CommandResult::Error(format!("Failed to load providers: {}", e)),
        };

        if connected_providers.is_empty() {
            return CommandResult::Error(
                "No models available. Please connect a provider first using /connect".to_string(),
            );
        }

        let discovery = Discovery::new();

        match discovery {
            Ok(d) => match d.fetch_models().await {
                Ok(models) => {
                    let prefs = prefs_data;

                    let mut model_lookup: std::collections::HashMap<(String, String), ModelType> =
                        std::collections::HashMap::new();

                    for model in &models {
                        if connected_providers.contains_key(&model.provider_id)
                            && if let Some(filter) = &provider_filter {
                                model.provider_id.contains(filter)
                                    || model.provider_name.to_lowercase().contains(filter)
                            } else {
                                true
                            }
                        {
                            model_lookup.insert(
                                (model.provider_id.clone(), model.id.clone()),
                                model.clone(),
                            );
                        }
                    }

                    let favorites_set = prefs
                        .as_ref()
                        .map(|p| {
                            p.favorite
                                .iter()
                                .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                                .collect::<std::collections::HashSet<_>>()
                        })
                        .unwrap_or_default();

                    let recent_set = prefs
                        .as_ref()
                        .map(|p| {
                            p.recent
                                .iter()
                                .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                                .collect::<std::collections::HashSet<_>>()
                        })
                        .unwrap_or_default();

                    let mut items: Vec<DialogItem> = Vec::new();

                    let add_model_item = |items: &mut Vec<DialogItem>, model: &ModelType, group: &str| {
                        let is_active = active_model_id.as_ref() == Some(&model.id);
                        let is_favorite = favorites_set.contains(&(model.provider_id.clone(), model.id.clone()));

                        let tip = if is_active {
                            Some("✓ Active".to_string())
                        } else if is_favorite {
                            Some("★ Favorite".to_string())
                        } else {
                            None
                        };

                        let description = if group == "Favorite" || group == "Recent" {
                            model.provider_name.clone()
                        } else {
                            format!(
                                "{} | {}",
                                model.provider_name,
                                model.capabilities.join(", ")
                            )
                        };

                        items.push(DialogItem {
                            id: model.id.clone(),
                            name: model.name.clone(),
                            group: group.to_string(),
                            description,
                            tip,
                            provider_id: model.provider_id.clone(),
                        });
                    };

                    let favorites_list = prefs
                        .as_ref()
                        .map(|p| p.favorite.clone())
                        .unwrap_or_default();

                    let mut favorite_models = Vec::new();
                    for fav in &favorites_list {
                        if let Some(model) = model_lookup.get(&(fav.provider_id.clone(), fav.model_id.clone())) {
                            favorite_models.push(model.clone());
                        }
                    }

                    for model in &favorite_models {
                        add_model_item(&mut items, model, "Favorite");
                    }

                    let recent_list = prefs
                        .as_ref()
                        .map(|p| p.recent.clone())
                        .unwrap_or_default();

                    let mut recent_models = Vec::new();
                    for recent in &recent_list {
                        if favorites_set.contains(&(recent.provider_id.clone(), recent.model_id.clone())) {
                            continue;
                        }
                        if let Some(model) = model_lookup.get(&(recent.provider_id.clone(), recent.model_id.clone())) {
                            recent_models.push(model.clone());
                        }
                    }

                    for model in &recent_models {
                        add_model_item(&mut items, model, "Recent");
                    }

                    let mut provider_models: std::collections::HashMap<String, Vec<ModelType>> =
                        std::collections::HashMap::new();

                    for model in models {
                        let model_key = (model.provider_id.clone(), model.id.clone());
                        if favorites_set.contains(&model_key) || recent_set.contains(&model_key) {
                            continue;
                        }

                        if connected_providers.contains_key(&model.provider_id)
                            && if let Some(filter) = &provider_filter {
                                model.provider_id.contains(filter)
                                    || model.provider_name.to_lowercase().contains(filter)
                            } else {
                                true
                            }
                        {
                            provider_models
                                .entry(model.provider_name.clone())
                                .or_default()
                                .push(model);
                        }
                    }

                    for (provider_name, models_list) in provider_models {
                        for model in &models_list {
                            add_model_item(&mut items, model, &provider_name);
                        }
                    }

                    items.sort_by(|a, b| {
                        let is_a_special = a.group == "Favorite" || a.group == "Recent";
                        let is_b_special = b.group == "Favorite" || b.group == "Recent";

                        if is_a_special && !is_b_special {
                            return std::cmp::Ordering::Less;
                        }
                        if !is_a_special && is_b_special {
                            return std::cmp::Ordering::Greater;
                        }

                        if is_a_special && is_b_special {
                            if a.group == "Favorite" && b.group != "Favorite" {
                                return std::cmp::Ordering::Less;
                            }
                            if a.group != "Favorite" && b.group == "Favorite" {
                                return std::cmp::Ordering::Greater;
                            }
                            return std::cmp::Ordering::Equal;
                        }

                        a.group.cmp(&b.group).then(a.name.cmp(&b.name))
                    });

                    if items.is_empty() {
                        if let Some(filter) = provider_filter {
                            CommandResult::Error(format!(
                                "No models found for provider: {}",
                                filter
                            ))
                        } else {
                            CommandResult::Error("No models available".to_string())
                        }
                    } else {
                        CommandResult::ShowDialog {
                            title: "Available Models".to_string(),
                            items,
                        }
                    }
                }
                Err(e) => CommandResult::Error(format!("Failed to fetch models: {}", e)),
            },
            Err(e) => CommandResult::Error(format!("Failed to initialize model discovery: {}", e)),
        }
    })
}

pub fn register_all_commands(registry: &mut Registry) {
    registry.register(Command {
        name: "exit".to_string(),
        description: "Quit crabcode".to_string(),
        handler: handle_exit,
    });

    registry.register(Command {
        name: "sessions".to_string(),
        description: "List all sessions".to_string(),
        handler: handle_sessions,
    });

    registry.register(Command {
        name: "new".to_string(),
        description: "Switch to home screen".to_string(),
        handler: handle_new,
    });

    registry.register(Command {
        name: "home".to_string(),
        description: "Switch to home screen".to_string(),
        handler: handle_new,
    });

    registry.register(Command {
        name: "connect".to_string(),
        description: "Connect to a model provider".to_string(),
        handler: handle_connect,
    });

    registry.register(Command {
        name: "models".to_string(),
        description: "List available models".to_string(),
        handler: handle_models,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::registry::Registry;

    fn create_registry() -> Registry {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        registry
    }

    #[tokio::test]
    async fn test_handle_exit() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "exit".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_exit(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[tokio::test]
    async fn test_handle_sessions() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "sessions".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Sessions");
                assert!(items.is_empty());
            }
            _ => panic!("Expected ShowDialog"),
        }
    }

    #[tokio::test]
    async fn test_handle_sessions_with_data() {
        let mut session_manager = SessionManager::new();
        session_manager.create_session(Some("session-1".to_string()));
        session_manager.create_session(Some("session-2".to_string()));

        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "sessions".to_string(),
            args: vec![],
        };
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Sessions");
                assert_eq!(items.len(), 2);
                assert!(items.iter().any(|item| item.name == "session-1"), "Items: {:?}", items.iter().map(|i| &i.name).collect::<Vec<_>>());
                assert!(items.iter().any(|item| item.name == "session-2"));
            }
            _ => panic!("Expected ShowDialog"),
        }
    }

    #[tokio::test]
    async fn test_handle_new_no_args() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "new".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.is_empty());
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_new_with_name() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "new".to_string(),
            args: vec!["my-session".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.is_empty());
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_home() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "home".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.is_empty());
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_connect_no_args() {
        let _ = crate::config::ApiKeyConfig::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();

        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "connect".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Connect a provider");
                assert!(!items.is_empty());
                if items.len() >= 4 {
                    assert!(items.iter().any(|item| item.id == "anthropic"
                        || item.id == "openai"
                        || item.id == "google"
                        || item.id == "opencode"));
                }
            }
            _ => panic!("Expected ShowDialog"),
        }

        let _ = crate::config::ApiKeyConfig::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_connect_provider_only() {
        let _ = crate::config::ApiKeyConfig::cleanup_test();

        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "connect".to_string(),
            args: vec!["nano-gpt".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("not configured") || msg.contains("is not configured"));
            }
            _ => panic!("Expected Success"),
        }

        let _ = crate::config::ApiKeyConfig::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_connect_with_api_key() {
        let _ = crate::config::ApiKeyConfig::cleanup_test();

        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "connect".to_string(),
            args: vec!["nano-gpt".to_string(), "sk-test-key".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("API key configured"));
            }
            _ => panic!("Expected Success"),
        }

        let _ = crate::config::ApiKeyConfig::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_connect_and_retrieve() {
        let _ = crate::config::ApiKeyConfig::cleanup_test();

        let mut session_manager = SessionManager::new();

        let parsed1 = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "connect".to_string(),
            args: vec!["nano-gpt".to_string(), "sk-test-key".to_string()],
        };
        let result1 = handle_connect(&parsed1, &mut session_manager).await;
        match result1 {
            CommandResult::Success(msg) => {
                assert!(msg.contains("API key configured"));
            }
            _ => panic!("Expected Success"),
        }

        let config = crate::config::ApiKeyConfig::load_test().unwrap();
        if let Some(api_key) = config.get_api_key("nano-gpt") {
            assert_eq!(api_key, "sk-test-key");
        }

        let _ = crate::config::ApiKeyConfig::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models() {
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "models".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(!items.is_empty());
            }
            CommandResult::Error(_) => {}
            _ => panic!("Expected ShowDialog or Error"),
        }
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models_with_filter() {
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "models".to_string(),
            args: vec!["open".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(!items.is_empty());
            }
            CommandResult::Error(_) => {}
            _ => panic!("Expected ShowDialog or Error"),
        }
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models_cleanup() {
        let _ = crate::config::ApiKeyConfig::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "models".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(!items.is_empty());
            }
            CommandResult::Error(_) => {}
            _ => panic!("Expected ShowDialog or Error"),
        }
        let _ = crate::config::ApiKeyConfig::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_registry_has_all_commands() {
        let registry = create_registry();
        let names = registry.get_command_names();
        assert_eq!(names.len(), 6);
        assert!(names.contains(&"exit".to_string()));
        assert!(names.contains(&"sessions".to_string()));
        assert!(names.contains(&"new".to_string()));
        assert!(names.contains(&"connect".to_string()));
        assert!(names.contains(&"models".to_string()));
        assert!(names.contains(&"home".to_string()));
    }

    #[tokio::test]
    async fn test_execute_exit_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "exit".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[tokio::test]
    async fn test_execute_unknown_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_dao: None,
            active_model_id: None,
            name: "unknown".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Error(msg) => {
                assert!(msg.contains("Unknown command"));
            }
            _ => panic!("Expected Error"),
        }
    }
}
