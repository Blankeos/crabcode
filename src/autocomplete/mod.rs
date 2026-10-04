pub mod command;
pub mod file;
pub mod mru;

pub use command::{CommandAuto, Suggestion, SuggestionKind};
pub use file::FileAuto;

pub enum AutoCompleteMode {
    Command,
    File,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_agent_name_collisions_only_suggest_the_agent() {
        let root = tempfile::tempdir().unwrap();
        let autocomplete = AutoComplete::new_at_with_file_config(
            CommandAuto::default(),
            root.path(),
            false,
            Vec::new(),
        )
        .with_skills(vec![Suggestion::skill("Explore", "Explore skill")])
        .with_agents(vec![Suggestion::agent("explore", "Explore agent")]);
        assert_eq!(
            autocomplete.mention_suggestions("EXP"),
            vec![Suggestion::agent("explore", "Explore agent")]
        );
    }
}

pub struct AutoComplete {
    pub command_auto: CommandAuto,
    pub file_auto: FileAuto,
    pub agents: Vec<Suggestion>,
    pub skills: Vec<Suggestion>,
    pub mode: AutoCompleteMode,
}

impl AutoComplete {
    pub fn new(command_auto: CommandAuto) -> Self {
        Self::new_at(command_auto, ".")
    }

    pub fn with_skills(mut self, mut skills: Vec<Suggestion>) -> Self {
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        self.skills = skills;
        self
    }

    /// Skills and agents precede fuzzy file matches in the shared `@` menu.
    pub fn mention_suggestions(&self, query: &str) -> Vec<Suggestion> {
        let query_lower = query.to_ascii_lowercase();
        let mut suggestions = self
            .skills
            .iter()
            // Both kinds insert @name. Preserve agent dispatch on collisions.
            .filter(|skill| {
                !self
                    .agents
                    .iter()
                    .any(|agent| agent.name.eq_ignore_ascii_case(&skill.name))
            })
            .chain(&self.agents)
            .filter(|suggestion| {
                suggestion
                    .name
                    .to_ascii_lowercase()
                    .starts_with(&query_lower)
            })
            .cloned()
            .collect::<Vec<_>>();
        suggestions.extend(self.file_auto.get_suggestions(query));
        suggestions
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
            skills: Vec::new(),
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
