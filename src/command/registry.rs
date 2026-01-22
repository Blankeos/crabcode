use crate::command::parser::ParsedCommand;
use crate::session::manager::SessionManager;
use std::collections::HashMap;
use std::pin::Pin;

pub type CommandHandler =
    for<'a> fn(
        &'a ParsedCommand,
        &'a mut SessionManager,
    ) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>>;

#[derive(Clone)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub handler: CommandHandler,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandResult {
    Success(String),
    Error(String),
}

pub struct Registry {
    commands: HashMap<String, Command>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub fn register(&mut self, command: Command) {
        self.commands.insert(command.name.clone(), command);
    }

    pub fn get(&self, name: &str) -> Option<&Command> {
        self.commands.get(name)
    }

    pub async fn execute<'a>(
        &self,
        parsed: &'a ParsedCommand,
        session_manager: &'a mut SessionManager,
    ) -> CommandResult {
        if let Some(command) = self.get(&parsed.name) {
            (command.handler)(parsed, session_manager).await
        } else {
            CommandResult::Error(format!("Unknown command: {}", parsed.name))
        }
    }

    pub fn list_commands(&self) -> Vec<&Command> {
        self.commands.values().collect()
    }

    pub fn get_command_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_handler<'a>(
        _parsed: &'a ParsedCommand,
        _sm: &'a mut SessionManager,
    ) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
        Box::pin(async { CommandResult::Success("ok".to_string()) })
    }

    fn dummy_error_handler<'a>(
        _parsed: &'a ParsedCommand,
        _sm: &'a mut SessionManager,
    ) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
        Box::pin(async { CommandResult::Error("error".to_string()) })
    }

    #[test]
    fn test_registry_creation() {
        let registry = Registry::new();
        assert_eq!(registry.commands.len(), 0);
    }

    #[test]
    fn test_registry_default() {
        let registry = Registry::default();
        assert_eq!(registry.commands.len(), 0);
    }

    #[test]
    fn test_register_command() {
        let mut registry = Registry::new();
        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: dummy_handler,
        };
        registry.register(command);
        assert_eq!(registry.commands.len(), 1);
    }

    #[test]
    fn test_get_command() {
        let mut registry = Registry::new();
        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: dummy_handler,
        };
        registry.register(command.clone());

        let retrieved = registry.get("test");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "test");
    }

    #[test]
    fn test_get_nonexistent_command() {
        let registry = Registry::new();
        let retrieved = registry.get("nonexistent");
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_execute_command() {
        let mut registry = Registry::new();
        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: dummy_handler,
        };
        registry.register(command);

        let parsed = ParsedCommand {
            name: "test".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success("ok".to_string()));
    }

    #[tokio::test]
    async fn test_execute_unknown_command() {
        let registry = Registry::new();

        let parsed = ParsedCommand {
            name: "unknown".to_string(),
            args: vec![],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(
            result,
            CommandResult::Error("Unknown command: unknown".to_string())
        );
    }

    #[test]
    fn test_list_commands() {
        let mut registry = Registry::new();

        let command1 = Command {
            name: "test1".to_string(),
            description: "Test command 1".to_string(),
            handler: dummy_handler,
        };
        let command2 = Command {
            name: "test2".to_string(),
            description: "Test command 2".to_string(),
            handler: dummy_handler,
        };

        registry.register(command1);
        registry.register(command2);

        let commands = registry.list_commands();
        assert_eq!(commands.len(), 2);
    }

    #[test]
    fn test_get_command_names() {
        let mut registry = Registry::new();

        let command1 = Command {
            name: "zebra".to_string(),
            description: "Test command 1".to_string(),
            handler: dummy_handler,
        };
        let command2 = Command {
            name: "apple".to_string(),
            description: "Test command 2".to_string(),
            handler: dummy_handler,
        };

        registry.register(command1);
        registry.register(command2);

        let names = registry.get_command_names();
        assert_eq!(names, vec!["apple".to_string(), "zebra".to_string()]);
    }

    #[tokio::test]
    async fn test_execute_with_args() {
        let mut registry = Registry::new();

        let handler_with_args =
            |parsed: &ParsedCommand,
             _sm: &mut SessionManager|
             -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + '_>> {
                let args = parsed.args.clone();
                Box::pin(async move {
                    if !args.is_empty() {
                        CommandResult::Success(format!("Args: {:?}", args))
                    } else {
                        CommandResult::Error("No args".to_string())
                    }
                })
            };

        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: handler_with_args,
        };
        registry.register(command);

        let parsed = ParsedCommand {
            name: "test".to_string(),
            args: vec!["arg1".to_string(), "arg2".to_string()],
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(
            result,
            CommandResult::Success("Args: [\"arg1\", \"arg2\"]".to_string())
        );
    }
}
