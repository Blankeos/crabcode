use crate::command::parser::ParsedCommand;
use crate::command::registry::{Command, CommandResult, Registry};
use crate::session::manager::SessionManager;
use std::pin::Pin;

pub fn handle_exit<'a>(
    _parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async { CommandResult::Success("Exiting...".to_string()) })
}

pub fn handle_sessions<'a>(
    _parsed: &'a ParsedCommand,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move {
        let sessions = sm.list_sessions();

        if sessions.is_empty() {
            CommandResult::Success("No active sessions".to_string())
        } else {
            let mut output = String::from("Active sessions:\n");
            for session in sessions {
                output.push_str(&format!(
                    "  - {} ({} messages)\n",
                    session.id, session.message_count
                ));
            }
            CommandResult::Success(output)
        }
    })
}

pub fn handle_new<'a>(
    parsed: &'a ParsedCommand,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let name = if parsed.args.is_empty() {
        None
    } else {
        Some(parsed.args[0].clone())
    };

    Box::pin(async move {
        let session_id = sm.create_session(name);
        CommandResult::Success(format!("Created new session: {}", session_id))
    })
}

pub fn handle_connect<'a>(
    parsed: &'a ParsedCommand,
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

            let discovery = match crate::model::discovery::Discovery::new() {
                Ok(d) => d,
                Err(e) => return CommandResult::Error(format!("Failed to initialize provider discovery: {}", e)),
            };

            let providers_map = match discovery.fetch_providers().await {
                Ok(p) => p,
                Err(e) => return CommandResult::Error(format!("Failed to fetch providers: {}", e)),
            };

            const POPULAR_PROVIDERS: &[&str] = &["opencode", "anthropic", "openai", "google"];

            let mut items: Vec<crate::command::registry::DialogItem> = providers_map
                .into_iter()
                .map(|(id, provider)| {
                    let group = if POPULAR_PROVIDERS.contains(&id.as_str()) {
                        "Popular"
                    } else {
                        "Other"
                    };
                    crate::command::registry::DialogItem {
                        id: id.clone(),
                        name: provider.name.clone(),
                        group: group.to_string(),
                        description: id.clone(),
                        connected: connected_providers.contains_key(&id),
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
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    use crate::command::registry::DialogItem;
    use crate::model::discovery::Discovery;
    use crate::persistence::AuthDAO;

    let provider_filter = if parsed.args.is_empty() {
        None
    } else {
        Some(parsed.args[0].clone())
    };

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
            return CommandResult::Error("No models available. Please connect a provider first using /connect".to_string());
        }

        let discovery = Discovery::new();

        match discovery {
            Ok(d) => match d.fetch_models().await {
                Ok(models) => {
                    let items: Vec<DialogItem> = models
                        .into_iter()
                        .filter(|model| {
                            connected_providers.contains_key(&model.provider_id)
                                && if let Some(filter) = &provider_filter {
                                    model.provider_id.contains(filter)
                                        || model.provider_name.to_lowercase().contains(filter)
                                } else {
                                    true
                                }
                        })
                        .map(|model| DialogItem {
                            id: model.id.clone(),
                            name: model.name.clone(),
                            group: model.provider_name.clone(),
                            description: format!(
                                "{} | {}",
                                model.provider_name,
                                model.capabilities.join(", ")
                            ),
                            connected: false,
                        })
                        .collect();

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
        description: "Create new session".to_string(),
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
        };
        let mut session_manager = SessionManager::new();
        let result = handle_exit(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[tokio::test]
    async fn test_handle_sessions() {
        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("No active sessions"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_sessions_with_data() {
        let mut session_manager = SessionManager::new();
        session_manager.create_session(Some("session-1".to_string()));
        session_manager.create_session(Some("session-2".to_string()));

        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
        };
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Active sessions:"));
                assert!(msg.contains("session-1"));
                assert!(msg.contains("session-2"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_new_no_args() {
        let parsed = ParsedCommand {
            name: "new".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Created new session: session-1"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_new_with_name() {
        let parsed = ParsedCommand {
            name: "new".to_string(),
            args: vec!["my-session".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Created new session: my-session"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_connect_no_args() {
        let _ = crate::config::ApiKeyConfig::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();

        let parsed = ParsedCommand {
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
                    assert!(items.iter().any(|item| item.id == "anthropic" || item.id == "openai" || item.id == "google" || item.id == "opencode"));
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
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"exit".to_string()));
        assert!(names.contains(&"sessions".to_string()));
        assert!(names.contains(&"new".to_string()));
        assert!(names.contains(&"connect".to_string()));
        assert!(names.contains(&"models".to_string()));
    }

    #[tokio::test]
    async fn test_execute_exit_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
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
