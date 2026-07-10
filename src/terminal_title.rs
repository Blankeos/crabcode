use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalTitleItem {
    Activity,
    ProjectName,
    RunState,
    ThreadTitle,
    ThreadTitleTruncated,
    GitBranch,
}

impl TerminalTitleItem {
    pub const ALL: [Self; 6] = [
        Self::Activity,
        Self::ProjectName,
        Self::RunState,
        Self::ThreadTitle,
        Self::ThreadTitleTruncated,
        Self::GitBranch,
    ];

    pub const DEFAULT: [Self; 2] = [Self::Activity, Self::ProjectName];

    pub fn label(self) -> &'static str {
        match self {
            Self::Activity => "activity",
            Self::ProjectName => "projectname",
            Self::RunState => "run state",
            Self::ThreadTitle => "thread title",
            Self::ThreadTitleTruncated => "thread title truncated",
            Self::GitBranch => "git branch",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Activity => "Spinner while active; action marker when blocked",
            Self::ProjectName => "Current project directory name",
            Self::RunState => "Ready, Working, or Thinking",
            Self::ThreadTitle => "Full current thread title",
            Self::ThreadTitleTruncated => "Current thread title truncated to 48 characters",
            Self::GitBranch => "Current Git branch when available",
        }
    }

    pub fn separator_from_previous(self, previous: Option<Self>) -> &'static str {
        match previous {
            None => "",
            Some(previous) if previous == Self::Activity || self == Self::Activity => " ",
            Some(_) => " | ",
        }
    }
}

pub fn normalized_items(
    items: impl IntoIterator<Item = TerminalTitleItem>,
) -> Vec<TerminalTitleItem> {
    let mut normalized = Vec::new();
    for item in items {
        if !normalized.contains(&item) {
            normalized.push(item);
        }
    }
    normalized
}

pub fn default_items() -> Vec<TerminalTitleItem> {
    TerminalTitleItem::DEFAULT.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preserves_first_occurrence_order() {
        assert_eq!(
            normalized_items([
                TerminalTitleItem::GitBranch,
                TerminalTitleItem::Activity,
                TerminalTitleItem::GitBranch,
            ]),
            vec![TerminalTitleItem::GitBranch, TerminalTitleItem::Activity]
        );
    }
}
