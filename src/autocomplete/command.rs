use crate::command::registry::Registry;

#[derive(Default)]
pub struct CommandAuto {
    commands: Vec<String>,
}

impl CommandAuto {
    pub fn new(registry: &Registry) -> Self {
        let commands = registry.get_command_names();
        Self { commands }
    }

    pub fn get_suggestions(&self, input: &str) -> Vec<String> {
        let input_lower = input.to_lowercase();
        self.commands
            .iter()
            .filter(|cmd| cmd.to_lowercase().starts_with(&input_lower))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::registry::{Command, Registry};

    fn dummy_handler(
        _parsed: &crate::command::parser::ParsedCommand,
    ) -> crate::command::registry::CommandResult {
        crate::command::registry::CommandResult::Success("ok".to_string())
    }

    fn setup_registry() -> Registry {
        let mut registry = Registry::new();
        registry.register(Command {
            name: "help".to_string(),
            description: "Show help".to_string(),
            handler: dummy_handler,
        });
        registry.register(Command {
            name: "sessions".to_string(),
            description: "Manage sessions".to_string(),
            handler: dummy_handler,
        });
        registry.register(Command {
            name: "exit".to_string(),
            description: "Exit the app".to_string(),
            handler: dummy_handler,
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
        assert_eq!(suggestions[0], "sessions");
    }

    #[test]
    fn test_get_suggestions_exact() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("help");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0], "help");
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
        assert_eq!(suggestions[0], "help");
    }
}
