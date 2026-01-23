pub mod command;
pub mod file;

pub use command::{CommandAuto, Suggestion};
pub use file::FileAuto;

pub enum AutoCompleteMode {
    Command,
    File,
}

pub struct AutoComplete {
    pub command_auto: CommandAuto,
    pub file_auto: FileAuto,
    pub mode: AutoCompleteMode,
}

impl AutoComplete {
    pub fn new(command_auto: CommandAuto) -> Self {
        Self {
            command_auto,
            file_auto: FileAuto::new(),
            mode: AutoCompleteMode::Command,
        }
    }

    pub fn get_suggestions(&self, input: &str) -> Vec<Suggestion> {
        match &self.mode {
            AutoCompleteMode::Command => self.command_auto.get_suggestions(input),
            AutoCompleteMode::File => self
                .file_auto
                .get_suggestions(input)
                .into_iter()
                .map(|name| Suggestion {
                    name,
                    description: String::new(),
                })
                .collect(),
        }
    }
}
