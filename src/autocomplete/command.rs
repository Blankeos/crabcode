use crate::command::registry::Registry;

#[derive(Clone)]
pub struct Suggestion {
    pub name: String,
    pub description: String,
}

#[derive(Default)]
pub struct CommandAuto {
    commands: Vec<Suggestion>,
    hidden_token_map: Vec<(String, String)>,
}

impl CommandAuto {
    pub fn new(registry: &Registry) -> Self {
        let commands: Vec<Suggestion> = registry
            .list_commands()
            .iter()
            .map(|cmd| Suggestion {
                name: cmd.name.clone(),
                description: cmd.description.clone(),
            })
            .collect();

        let hidden_token_map: Vec<(String, String)> = registry
            .list_commands()
            .iter()
            .flat_map(|cmd| {
                cmd.hidden_tokens
                    .iter()
                    .map(|t| (t.clone(), cmd.name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();

        Self {
            commands,
            hidden_token_map,
        }
    }

    pub fn get_suggestions(&self, input: &str) -> Vec<Suggestion> {
        let input_lower = input.to_lowercase();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results: Vec<Suggestion> = Vec::new();

        for cmd in &self.commands {
            if cmd.name.to_lowercase().starts_with(&input_lower) {
                if seen.insert(cmd.name.clone()) {
                    results.push(cmd.clone());
                }
            }
        }

        for (token, command_name) in &self.hidden_token_map {
            if token.to_lowercase().starts_with(&input_lower) {
                if seen.insert(command_name.clone()) {
                    if let Some(cmd) = self.commands.iter().find(|c| c.name == *command_name) {
                        results.push(cmd.clone());
                    }
                }
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::registry::{Command, Registry};
    use std::pin::Pin;

    fn dummy_handler(
        _parsed: &crate::command::parser::ParsedCommand,
        _sm: &mut crate::session::manager::SessionManager,
    ) -> Pin<Box<dyn std::future::Future<Output = crate::command::registry::CommandResult> + Send>>
    {
        Box::pin(async { crate::command::registry::CommandResult::Success("ok".to_string()) })
    }

    fn setup_registry() -> Registry {
        let mut registry = Registry::new();
        registry.register(Command {
            name: "help".to_string(),
            description: "Show help".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
        });
        registry.register(Command {
            name: "sessions".to_string(),
            description: "Manage sessions".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec!["resume".to_string()],
        });
        registry.register(Command {
            name: "exit".to_string(),
            description: "Exit the app".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
        });
        registry
    }

    #[test]
    fn test_command_auto_creation() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        assert_eq!(auto.commands.len(), 3);
    }

    #[test]
    fn test_command_auto_default() {
        let auto = CommandAuto::default();
        assert!(auto.commands.is_empty());
    }

    #[test]
    fn test_get_suggestions_empty() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("");
        assert_eq!(suggestions.len(), 3);
    }

    #[test]
    fn test_get_suggestions_partial() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("s");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "sessions");
    }

    #[test]
    fn test_get_suggestions_exact() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("help");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "help");
    }

    #[test]
    fn test_get_suggestions_hidden_token() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("res");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "sessions");
    }

    #[test]
    fn test_get_suggestions_no_match() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("xyz");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_get_suggestions_case_insensitive() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("HELP");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "help");
    }
}
