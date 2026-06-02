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
    pub hidden_tokens: Vec<String>,
    pub chat_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandResult {
    Success(String),
    Error(String),
    RunPrompt {
        prompt: String,
        agent: Option<String>,
        model: Option<String>,
        subtask: Option<bool>,
    },
    ShowDialog {
        title: String,
        items: Vec<DialogItem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct DialogItem {
    pub id: String,
    pub name: String,
    pub group: String,
    pub description: String,
    pub tip: Option<String>,
    pub provider_id: String,
}

pub struct Registry {
    commands: HashMap<String, Command>,
    custom_commands: HashMap<String, crate::command::custom::CustomCommand>,
    hidden_from_autocomplete: std::collections::HashSet<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
            custom_commands: HashMap::new(),
            hidden_from_autocomplete: std::collections::HashSet::new(),
        }
    }

    pub fn register(&mut self, command: Command) {
        self.commands.insert(command.name.clone(), command);
    }

    pub fn register_custom(&mut self, command: crate::command::custom::CustomCommand) {
        self.commands.insert(
            command.name.clone(),
            Command {
                name: command.name.clone(),
                description: command.description.clone().unwrap_or_default(),
                handler: handle_custom_command,
                hidden_tokens: vec![],
                chat_only: false,
            },
        );
        self.custom_commands.insert(command.name.clone(), command);
    }

    pub fn has_public_command(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    pub fn is_custom_command(&self, name: &str) -> bool {
        self.custom_commands.contains_key(name)
    }

    pub fn custom_command(&self, name: &str) -> Option<&crate::command::custom::CustomCommand> {
        self.custom_commands.get(name)
    }

    pub fn hide_from_autocomplete(&mut self, name: impl Into<String>) {
        self.hidden_from_autocomplete.insert(name.into());
    }

    pub fn is_hidden_from_autocomplete(&self, name: &str) -> bool {
        self.hidden_from_autocomplete.contains(name)
    }

    pub fn get(&self, name: &str) -> Option<&Command> {
        if let Some(cmd) = self.commands.get(name) {
            return Some(cmd);
        }
        // Check hidden_tokens
        for cmd in self.commands.values() {
            if cmd.hidden_tokens.iter().any(|t| t == name) {
                return Some(cmd);
            }
        }
        None
    }

    pub fn is_chat_only(&self, name: &str) -> bool {
        self.get(name).is_some_and(|cmd| cmd.chat_only)
    }

    pub async fn execute<'a>(
        &self,
        parsed: &'a ParsedCommand,
        session_manager: &'a mut SessionManager,
    ) -> CommandResult {
        if let Some(command) = self.custom_commands.get(&parsed.name) {
            return match command.render(parsed.raw_args()).await {
                Ok(rendered) => CommandResult::RunPrompt {
                    prompt: rendered.prompt,
                    agent: rendered.agent,
                    model: rendered.model,
                    subtask: rendered.subtask,
                },
                Err(err) => CommandResult::Error(format!(
                    "Failed to render command {}: {}",
                    parsed.name, err
                )),
            };
        }

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

fn handle_custom_command<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let name = parsed.name.clone();
    Box::pin(async move { CommandResult::Error(format!("Unknown command: {}", name)) })
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

    fn create_test_dialog_item(id: &str) -> DialogItem {
        DialogItem {
            id: id.to_string(),
            name: "Test Item".to_string(),
            group: "Test Group".to_string(),
            description: "Test description".to_string(),
            tip: None,
            provider_id: String::new(),
        }
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
            hidden_tokens: vec![],
            chat_only: false,
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
            hidden_tokens: vec![],
            chat_only: false,
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

    #[test]
    fn test_get_by_hidden_token() {
        let mut registry = Registry::new();
        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec!["alias".to_string()],
            chat_only: false,
        };
        registry.register(command);
        assert!(registry.get("alias").is_some());
        assert_eq!(registry.get("alias").unwrap().name, "test");
    }

    #[test]
    fn test_is_chat_only_checks_hidden_token() {
        let mut registry = Registry::new();
        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec!["alias".to_string()],
            chat_only: true,
        };
        registry.register(command);

        assert!(registry.is_chat_only("test"));
        assert!(registry.is_chat_only("alias"));
        assert!(!registry.is_chat_only("missing"));
    }

    #[tokio::test]
    async fn test_execute_command() {
        let mut registry = Registry::new();
        let command = Command {
            name: "test".to_string(),
            description: "Test command".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: false,
        };
        registry.register(command);
        let parsed = ParsedCommand {
            name: "test".to_string(),
            args: vec![],
            raw: "/test".to_string(),
            prefs_data: None,
            active_model_id: None,
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
            raw: "/unknown".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(
            result,
            CommandResult::Error("Unknown command: unknown".to_string())
        );
    }

    #[tokio::test]
    async fn test_custom_command_overrides_registered_command() {
        let mut registry = Registry::new();
        registry.register(Command {
            name: "test".to_string(),
            description: "Built in test".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: false,
        });
        registry.register_custom(crate::command::custom::CustomCommand {
            name: "test".to_string(),
            description: Some("Custom test".to_string()),
            agent: Some("build".to_string()),
            model: Some("openai/gpt-5".to_string()),
            subtask: Some(false),
            template: "Run $ARGUMENTS".to_string(),
            source: crate::command::custom::CustomCommandSource::Config(std::path::PathBuf::from(
                "/tmp/opencode.json",
            )),
            workdir: std::path::PathBuf::from("."),
        });

        let parsed = ParsedCommand {
            name: "test".to_string(),
            args: vec!["unit".to_string()],
            raw: "/test unit".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;

        assert_eq!(
            result,
            CommandResult::RunPrompt {
                prompt: "Run unit".to_string(),
                agent: Some("build".to_string()),
                model: Some("openai/gpt-5".to_string()),
                subtask: Some(false),
            }
        );
        assert_eq!(registry.get("test").unwrap().description, "Custom test");
    }

    #[test]
    fn test_list_commands() {
        let mut registry = Registry::new();

        let command1 = Command {
            name: "test1".to_string(),
            description: "Test command 1".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: false,
        };
        let command2 = Command {
            name: "test2".to_string(),
            description: "Test command 2".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: false,
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
            hidden_tokens: vec![],
            chat_only: false,
        };
        let command2 = Command {
            name: "apple".to_string(),
            description: "Test command 2".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: false,
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
            hidden_tokens: vec![],
            chat_only: false,
        };
        registry.register(command);

        let parsed = ParsedCommand {
            name: "test".to_string(),
            args: vec!["arg1".to_string(), "arg2".to_string()],
            raw: "/test arg1 arg2".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(
            result,
            CommandResult::Success("Args: [\"arg1\", \"arg2\"]".to_string())
        );
    }
}
