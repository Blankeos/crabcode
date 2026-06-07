pub mod command;
pub mod file;

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
        Self {
            command_auto,
            file_auto: FileAuto::new(),
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
