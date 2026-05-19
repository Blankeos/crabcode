use crate::autocomplete::Suggestion;
use ignore::WalkBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_SUGGESTIONS: usize = 80;
const CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct FileEntry {
    path: String,
    is_directory: bool,
}

#[derive(Default)]
struct FileAutoCache {
    entries: Vec<FileEntry>,
    refreshed_at: Option<Instant>,
}

pub struct FileAuto {
    root: PathBuf,
    cache: Mutex<FileAutoCache>,
}

impl FileAuto {
    pub fn new() -> Self {
        Self::new_at(".")
    }

    pub fn new_at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache: Mutex::new(FileAutoCache::default()),
        }
    }

    pub fn get_suggestions(&self, input: &str) -> Vec<Suggestion> {
        let entries = self.entries();
        let query = input.trim();

        if query.is_empty() {
            return entries
                .iter()
                .take(MAX_SUGGESTIONS)
                .map(|entry| Suggestion::file(entry.path.clone(), entry.is_directory))
                .collect();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut scored = entries
            .iter()
            .filter_map(|entry| {
                let mut buf = Vec::new();
                pattern
                    .score(Utf32Str::new(&entry.path, &mut buf), &mut matcher)
                    .map(|score| (entry, score))
            })
            .collect::<Vec<_>>();

        scored.sort_by(|(a, a_score), (b, b_score)| {
            b_score
                .cmp(a_score)
                .then_with(|| a.path.len().cmp(&b.path.len()))
                .then_with(|| a.path.cmp(&b.path))
        });

        scored
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .map(|(entry, _)| Suggestion::file(entry.path.clone(), entry.is_directory))
            .collect()
    }

    pub fn expand_path(&self, input: &str) -> Option<String> {
        let suggestions = self.get_suggestions(input);
        (suggestions.len() == 1).then(|| suggestions[0].replacement.clone())
    }

    fn entries(&self) -> Vec<FileEntry> {
        let mut cache = self.cache.lock().expect("file autocomplete cache poisoned");
        let should_refresh = cache
            .refreshed_at
            .map(|refreshed_at| refreshed_at.elapsed() > CACHE_TTL)
            .unwrap_or(true);

        if should_refresh {
            cache.entries = collect_entries(&self.root);
            cache.refreshed_at = Some(Instant::now());
        }

        cache.entries.clone()
    }
}

fn collect_entries(root: &Path) -> Vec<FileEntry> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(true)
        .require_git(true)
        .filter_entry(|entry| entry.file_name() != ".git");

    let root_input = root.to_path_buf();
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut entries = builder
        .build()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path == root_input || path == root_abs || path == Path::new(".") {
                return None;
            }
            let file_type = entry.file_type()?;
            let is_directory = file_type.is_dir();
            let rel = path
                .strip_prefix(&root_input)
                .or_else(|_| path.strip_prefix(&root_abs))
                .unwrap_or(path);
            let mut display = rel.to_string_lossy().replace('\\', "/");
            if display.is_empty() {
                return None;
            }
            if is_directory && !display.ends_with('/') {
                display.push('/');
            }
            Some(FileEntry {
                path: display,
                is_directory,
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.path.cmp(&b.path))
    });
    entries
}

impl Default for FileAuto {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_file_auto_creation() {
        let _auto = FileAuto::new();
    }

    #[test]
    fn test_file_auto_default() {
        let _auto = FileAuto::default();
    }

    #[test]
    fn test_get_suggestions_empty_query_lists_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("alpha.rs"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());

        let suggestions = auto.get_suggestions("");

        assert!(suggestions.iter().any(|s| s.name == "alpha.rs"));
    }

    #[test]
    fn test_get_suggestions_no_match() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("alpha.rs"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());

        let suggestions = auto.get_suggestions("xyz123abc");

        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_hidden_files_are_suggested() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".env"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());

        let suggestions = auto.get_suggestions("env");

        assert!(suggestions.iter().any(|s| s.name == ".env"));
    }

    #[test]
    fn test_gitignore_is_respected_inside_git_repo() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join(".git")).unwrap();
        fs::write(temp.path().join(".gitignore"), "target/\n").unwrap();
        fs::create_dir(temp.path().join("target")).unwrap();
        fs::write(temp.path().join("target/ignored.txt"), "").unwrap();
        fs::write(temp.path().join("kept.txt"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());

        let suggestions = auto.get_suggestions("txt");

        assert!(suggestions.iter().any(|s| s.name == "kept.txt"));
        assert!(!suggestions.iter().any(|s| s.name == "target/ignored.txt"));
    }

    #[test]
    fn test_ignore_negation_can_make_file_visible() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".ignore"), "*.tmp\n!important.tmp\n").unwrap();
        fs::write(temp.path().join("hidden.tmp"), "").unwrap();
        fs::write(temp.path().join("important.tmp"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());

        let suggestions = auto.get_suggestions("tmp");

        assert!(suggestions.iter().any(|s| s.name == "important.tmp"));
        assert!(!suggestions.iter().any(|s| s.name == "hidden.tmp"));
    }
}
