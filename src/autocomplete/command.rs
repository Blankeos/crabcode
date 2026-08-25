use crate::autocomplete::mru::SlashMru;
use crate::command::registry::Registry;
use std::cell::RefCell;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Command,
    Agent,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub description: String,
    pub replacement: String,
    pub kind: SuggestionKind,
    pub is_directory: bool,
}

impl Suggestion {
    pub fn command(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            replacement: name.clone(),
            name,
            description: description.into(),
            kind: SuggestionKind::Command,
            is_directory: false,
        }
    }

    pub fn file(path: impl Into<String>, is_directory: bool) -> Self {
        let path = path.into();
        Self {
            name: path.clone(),
            replacement: path,
            description: if is_directory {
                "directory".to_string()
            } else {
                String::new()
            },
            kind: SuggestionKind::File,
            is_directory,
        }
    }

    pub fn agent(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            replacement: name.clone(),
            name,
            description: description.into(),
            kind: SuggestionKind::Agent,
            is_directory: false,
        }
    }

    pub fn display_prefix(&self) -> &'static str {
        match self.kind {
            SuggestionKind::Command => "/",
            SuggestionKind::Agent => "@",
            SuggestionKind::File => "",
        }
    }
}

pub struct CommandAuto {
    commands: Vec<Suggestion>,
    hidden_token_map: Vec<(String, String)>,
    chat_only_commands: HashSet<String>,
    mru: RefCell<SlashMru>,
}

impl Default for CommandAuto {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            hidden_token_map: Vec::new(),
            chat_only_commands: HashSet::new(),
            mru: RefCell::new(SlashMru::new()),
        }
    }
}

impl CommandAuto {
    pub fn new(registry: &Registry) -> Self {
        let commands: Vec<Suggestion> = registry
            .list_commands()
            .iter()
            .filter(|cmd| !registry.is_hidden_from_autocomplete(&cmd.name))
            .map(|cmd| Suggestion::command(cmd.name.clone(), cmd.description.clone()))
            .collect();

        let hidden_token_map: Vec<(String, String)> = registry
            .list_commands()
            .iter()
            .filter(|cmd| !registry.is_hidden_from_autocomplete(&cmd.name))
            .flat_map(|cmd| {
                cmd.hidden_tokens
                    .iter()
                    .map(|t| (t.clone(), cmd.name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();

        let chat_only_commands: HashSet<String> = registry
            .list_commands()
            .iter()
            .filter(|cmd| cmd.chat_only)
            .map(|cmd| cmd.name.clone())
            .collect();

        Self {
            commands,
            hidden_token_map,
            chat_only_commands,
            mru: RefCell::new(SlashMru::new()),
        }
    }

    /// Tests / ephemeral: never touches disk.
    #[cfg(test)]
    fn with_in_memory_mru(mut self) -> Self {
        self.mru = RefCell::new(SlashMru::new_in_memory());
        self
    }

    /// Record that a slash command was executed (boosts future search ranking).
    pub fn touch_mru(&self, command_name: &str) {
        let mut mru = self.mru.borrow_mut();
        mru.touch(command_name);
        mru.persist_if_dirty();
    }

    pub fn get_suggestions(&self, input: &str, is_chat: bool) -> Vec<Suggestion> {
        let input_lower = input.to_lowercase();
        let trimmed = input.trim();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results: Vec<Suggestion> = Vec::new();

        for cmd in &self.commands {
            if !is_chat && self.chat_only_commands.contains(&cmd.name) {
                continue;
            }
            if cmd.name.to_lowercase().starts_with(&input_lower) {
                if seen.insert(cmd.name.clone()) {
                    results.push(cmd.clone());
                }
            }
        }

        for (token, command_name) in &self.hidden_token_map {
            if !is_chat && self.chat_only_commands.contains(command_name) {
                continue;
            }
            if token.to_lowercase().starts_with(&input_lower) {
                if seen.insert(command_name.clone()) {
                    if let Some(cmd) = self.commands.iter().find(|c| c.name == *command_name) {
                        results.push(cmd.clone());
                    }
                }
            }
        }

        // Empty `/` keeps registry order. Non-empty search: MRU recency boost.
        if !trimmed.is_empty() && results.len() > 1 {
            let mut mru = self.mru.borrow_mut();
            results.sort_by(|a, b| {
                let score_b = mru.rank_score(&b.name);
                let score_a = mru.rank_score(&a.name);
                score_b.cmp(&score_a).then_with(|| a.name.cmp(&b.name))
            });
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
            chat_only: false,
        });
        registry.register(Command {
            name: "sessions".to_string(),
            description: "Manage sessions".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec!["resume".to_string()],
            chat_only: false,
        });
        registry.register(Command {
            name: "exit".to_string(),
            description: "Exit the app".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: false,
        });
        registry.register(Command {
            name: "compact".to_string(),
            description: "Compact session".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: true,
        });
        registry
    }

    #[test]
    fn test_command_auto_creation() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        assert_eq!(auto.commands.len(), 4);
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
        let suggestions = auto.get_suggestions("", true);
        assert_eq!(suggestions.len(), 4);
    }

    #[test]
    fn test_chat_only_suggestions_hidden_outside_chat() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);

        let home_suggestions = auto.get_suggestions("c", false);
        assert!(home_suggestions.iter().all(|s| s.name != "compact"));

        let chat_suggestions = auto.get_suggestions("c", true);
        assert!(chat_suggestions.iter().any(|s| s.name == "compact"));
    }

    #[test]
    fn test_hidden_from_autocomplete_command_is_not_suggested() {
        let mut registry = setup_registry();
        registry.hide_from_autocomplete("sessions");
        let auto = CommandAuto::new(&registry);

        assert!(auto.get_suggestions("s", true).is_empty());
        assert!(auto.get_suggestions("res", true).is_empty());
    }

    #[test]
    fn test_get_suggestions_partial() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("s", true);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "sessions");
    }

    #[test]
    fn test_get_suggestions_exact() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("help", true);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "help");
    }

    #[test]
    fn test_get_suggestions_hidden_token() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("res", true);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "sessions");
    }

    #[test]
    fn test_hidden_token_uses_command_replacement() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("res", true);
        assert_eq!(suggestions[0].name, "sessions");
        assert_eq!(suggestions[0].replacement, "sessions");
    }

    #[test]
    fn test_get_suggestions_no_match() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("xyz", true);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_get_suggestions_case_insensitive() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry);
        let suggestions = auto.get_suggestions("HELP", true);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "help");
    }

    #[test]
    fn empty_query_keeps_registry_order_even_with_mru() {
        let registry = setup_registry();
        let auto = CommandAuto::new(&registry).with_in_memory_mru();
        let before: Vec<String> = auto
            .get_suggestions("", true)
            .iter()
            .map(|s| s.name.clone())
            .collect();
        auto.touch_mru("exit");
        auto.touch_mru("compact");
        let after: Vec<String> = auto
            .get_suggestions("", true)
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn search_ranks_recently_used_first() {
        let mut registry = setup_registry();
        registry.register(Command {
            name: "compact-mode".to_string(),
            description: "Toggle compact mode".to_string(),
            handler: dummy_handler,
            hidden_tokens: vec![],
            chat_only: true,
        });
        let auto = CommandAuto::new(&registry).with_in_memory_mru();

        // Without MRU, registry order: compact then compact-mode
        let before = auto.get_suggestions("comp", true);
        assert_eq!(before[0].name, "compact");
        assert_eq!(before[1].name, "compact-mode");

        auto.touch_mru("compact-mode");
        let after = auto.get_suggestions("comp", true);
        assert_eq!(after[0].name, "compact-mode");
        assert_eq!(after[1].name, "compact");
    }
}
