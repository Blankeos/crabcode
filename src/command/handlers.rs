use crate::command::parser::ParsedCommand;
use crate::command::registry::{Command, CommandResult, Registry};
use crate::session::manager::SessionManager;

pub fn handle_exit(_parsed: &ParsedCommand, _sm: &mut SessionManager) -> CommandResult {
    CommandResult::Success("Exiting...".to_string())
}

pub fn handle_sessions(_parsed: &ParsedCommand, sm: &mut SessionManager) -> CommandResult {
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
}

pub fn handle_new(parsed: &ParsedCommand, sm: &mut SessionManager) -> CommandResult {
    let name = if parsed.args.is_empty() {
        None
    } else {
        Some(parsed.args[0].clone())
    };

    let session_id = sm.create_session(name);
    CommandResult::Success(format!("Created new session: {}", session_id))
}

pub fn handle_connect(parsed: &ParsedCommand, _sm: &mut SessionManager) -> CommandResult {
    if parsed.args.is_empty() {
        CommandResult::Error("Usage: /connect <provider> [model]".to_string())
    } else {
        let provider = &parsed.args[0];
        let model = if parsed.args.len() > 1 {
            &parsed.args[1]
        } else {
            "default"
        };
        CommandResult::Success(format!("Connected to {} using model {}", provider, model))
    }
}

pub fn handle_models(_parsed: &ParsedCommand, _sm: &mut SessionManager) -> CommandResult {
    CommandResult::Success(
        "Available models:\n  nano-gpt: gpt-4, gpt-3.5-turbo\n  z-ai: coding-plan".to_string(),
    )
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
        description: "Connect/configure model".to_string(),
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

    #[test]
    fn test_handle_exit() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_exit(&parsed, &mut session_manager);
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[test]
    fn test_handle_sessions() {
        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_sessions(&parsed, &mut session_manager);
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("No active sessions"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[test]
    fn test_handle_sessions_with_data() {
        let mut session_manager = SessionManager::new();
        session_manager.create_session(Some("session-1".to_string()));
        session_manager.create_session(Some("session-2".to_string()));

        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
        };
        let result = handle_sessions(&parsed, &mut session_manager);
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Active sessions:"));
                assert!(msg.contains("session-1"));
                assert!(msg.contains("session-2"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[test]
    fn test_handle_new_no_args() {
        let parsed = ParsedCommand {
            name: "new".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager);
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Created new session: session-1"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[test]
    fn test_handle_new_with_name() {
        let parsed = ParsedCommand {
            name: "new".to_string(),
            args: vec!["my-session".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager);
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Created new session: my-session"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[test]
    fn test_handle_connect_no_args() {
        let parsed = ParsedCommand {
            name: "connect".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager);
        assert_eq!(
            result,
            CommandResult::Error("Usage: /connect <provider> [model]".to_string())
        );
    }

    #[test]
    fn test_handle_connect_provider_only() {
        let parsed = ParsedCommand {
            name: "connect".to_string(),
            args: vec!["nano-gpt".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager);
        assert_eq!(
            result,
            CommandResult::Success("Connected to nano-gpt using model default".to_string())
        );
    }

    #[test]
    fn test_handle_connect_with_model() {
        let parsed = ParsedCommand {
            name: "connect".to_string(),
            args: vec!["nano-gpt".to_string(), "gpt-4".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager);
        assert_eq!(
            result,
            CommandResult::Success("Connected to nano-gpt using model gpt-4".to_string())
        );
    }

    #[test]
    fn test_handle_models() {
        let parsed = ParsedCommand {
            name: "models".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager);
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.contains("Available models:"));
                assert!(msg.contains("nano-gpt"));
                assert!(msg.contains("z-ai"));
            }
            _ => panic!("Expected Success"),
        }
    }

    #[test]
    fn test_registry_has_all_commands() {
        let registry = create_registry();
        let names = registry.get_command_names();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"exit".to_string()));
        assert!(names.contains(&"sessions".to_string()));
        assert!(names.contains(&"new".to_string()));
        assert!(names.contains(&"connect".to_string()));
        assert!(names.contains(&"models".to_string()));
    }

    #[test]
    fn test_execute_exit_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager);
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[test]
    fn test_execute_unknown_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "unknown".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager);
        match result {
            CommandResult::Error(msg) => {
                assert!(msg.contains("Unknown command"));
            }
            _ => panic!("Expected Error"),
        }
    }
}
