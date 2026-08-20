pub mod command;
pub mod file;
pub mod mru;

pub use command::{CommandAuto, Suggestion, SuggestionKind};
pub use file::FileAuto;

pub enum AutoCompleteMode {
    Command,
    File,
}

pub struct AutoComplete {
    pub command_auto: CommandAuto,
    pub file_auto: FileAuto,
    pub agents: Vec<Suggestion>,
    pub mode: AutoCompleteMode,
}

impl AutoComplete {
    pub fn new(command_auto: CommandAuto) -> Self {
        Self::new_at(command_auto, ".")
    }

    pub fn new_at(command_auto: CommandAuto, root: impl Into<std::path::PathBuf>) -> Self {
        Self::new_at_with_file_config(command_auto, root, true, Vec::new())
    }

    pub fn new_at_with_file_config(
        command_auto: CommandAuto,
        root: impl Into<std::path::PathBuf>,
        watcher_enabled: bool,
        ignored_paths: Vec<String>,
    ) -> Self {
        Self {
            command_auto,
            file_auto: FileAuto::new_at_with_config(root, watcher_enabled, ignored_paths),
            agents: Vec::new(),
            mode: AutoCompleteMode::Command,
        }
    }

    pub fn with_agents(mut self, agents: Vec<Suggestion>) -> Self {
        self.agents = agents;
        self
    }

    pub fn get_suggestions(&self, input: &str, is_chat: bool) -> Vec<Suggestion> {
        match &self.mode {
            AutoCompleteMode::Command => self.command_auto.get_suggestions(input, is_chat),
            AutoCompleteMode::File => self.file_auto.get_suggestions(input),
        }
    }
}
