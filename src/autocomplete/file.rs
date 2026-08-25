use crate::autocomplete::Suggestion;
use ignore::WalkBuilder;
use notify::{event::ModifyKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::{Path, PathBuf};
use std::sync::{
    mpsc::{self, Receiver, SyncSender},
    Arc, Condvar, Mutex, Weak,
};
use std::thread;
use std::time::{Duration, Instant};

const MAX_SUGGESTIONS: usize = 80;
const EVENT_DEBOUNCE: Duration = Duration::from_millis(100);
// Wake rarely when idle; notify events still force an immediate refresh via refresh_tx.
const INDEXER_POLL_INTERVAL: Duration = Duration::from_secs(30);
const WATCHED_SAFETY_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const UNWATCHED_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct FileEntry {
    path: String,
    is_directory: bool,
}

struct FileAutoState {
    entries: Arc<Vec<FileEntry>>,
    generation: u64,
}

impl Default for FileAutoState {
    fn default() -> Self {
        Self {
            entries: Arc::new(Vec::new()),
            generation: 0,
        }
    }
}

struct FileAutoInner {
    state: Mutex<FileAutoState>,
    state_changed: Condvar,
    refresh_tx: SyncSender<()>,
}

#[derive(Clone)]
pub struct FileAuto {
    inner: Arc<FileAutoInner>,
}

impl FileAuto {
    pub fn new() -> Self {
        Self::new_at(".")
    }

    pub fn new_at(root: impl Into<PathBuf>) -> Self {
        Self::new_at_with_config(root, true, Vec::new())
    }

    pub fn new_at_with_config(
        root: impl Into<PathBuf>,
        watcher_enabled: bool,
        ignored_paths: Vec<String>,
    ) -> Self {
        let root = root.into();
        let (refresh_tx, refresh_rx) = mpsc::sync_channel(1);
        let inner = Arc::new(FileAutoInner {
            state: Mutex::new(FileAutoState::default()),
            state_changed: Condvar::new(),
            refresh_tx: refresh_tx.clone(),
        });
        let weak_inner = Arc::downgrade(&inner);
        let fallback_inner = Arc::clone(&inner);
        let fallback_root = root.clone();

        if thread::Builder::new()
            .name("crabcode-file-index".to_string())
            .spawn(move || {
                run_indexer(
                    root,
                    weak_inner,
                    refresh_tx,
                    refresh_rx,
                    watcher_enabled,
                    ignored_paths,
                )
            })
            .is_err()
        {
            publish_entries(&fallback_inner, collect_entries(&fallback_root, &[]));
        }

        Self { inner }
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

    fn entries(&self) -> Arc<Vec<FileEntry>> {
        Arc::clone(
            &self
                .inner
                .state
                .lock()
                .expect("file autocomplete index poisoned")
                .entries,
        )
    }

    #[cfg(test)]
    fn generation(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("file autocomplete index poisoned")
            .generation
    }

    #[cfg(test)]
    fn wait_for_generation_after(&self, generation: u64) -> bool {
        let state = self
            .inner
            .state
            .lock()
            .expect("file autocomplete index poisoned");
        self.inner
            .state_changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                state.generation <= generation
            })
            .expect("file autocomplete index poisoned")
            .0
            .generation
            > generation
    }

    #[cfg(test)]
    fn wait_until_ready(&self) -> bool {
        self.wait_for_generation_after(0)
    }
}

fn run_indexer(
    root: PathBuf,
    inner: Weak<FileAutoInner>,
    refresh_tx: SyncSender<()>,
    refresh_rx: Receiver<()>,
    watcher_enabled: bool,
    ignored_paths: Vec<String>,
) {
    let watcher = watcher_enabled
        .then(|| create_watcher(&root, refresh_tx))
        .flatten();
    let safety_refresh_interval = if watcher.is_some() {
        WATCHED_SAFETY_REFRESH_INTERVAL
    } else {
        UNWATCHED_REFRESH_INTERVAL
    };
    let mut last_refresh = Instant::now();

    if !refresh_index(&root, &inner, &ignored_paths) {
        return;
    }

    loop {
        let refresh_requested = match refresh_rx.recv_timeout(INDEXER_POLL_INTERVAL) {
            Ok(()) => {
                thread::sleep(EVENT_DEBOUNCE);
                while refresh_rx.try_recv().is_ok() {}
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        if inner.strong_count() == 0 {
            break;
        }

        if refresh_requested || last_refresh.elapsed() >= safety_refresh_interval {
            if !refresh_index(&root, &inner, &ignored_paths) {
                break;
            }
            last_refresh = Instant::now();
        }
    }
}

fn create_watcher(root: &Path, refresh_tx: SyncSender<()>) -> Option<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
        if event.as_ref().is_ok_and(event_requires_refresh) {
            let _ = refresh_tx.try_send(());
        }
    })
    .ok()?;

    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

fn event_requires_refresh(event: &Event) -> bool {
    let changes_paths = matches!(
        event.kind,
        EventKind::Any
            | EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Any | ModifyKind::Name(_))
            | EventKind::Other
    );

    event.paths.iter().any(|path| {
        let components = path
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect::<Vec<_>>();
        let git_position = components.iter().position(|component| *component == ".git");

        let is_ignore_file = path
            .file_name()
            .is_some_and(|name| name == ".gitignore" || name == ".ignore");
        let is_git_exclude = match git_position {
            None => false,
            Some(position) => {
                components.get(position + 1..)
                    == Some(
                        [
                            std::ffi::OsStr::new("info"),
                            std::ffi::OsStr::new("exclude"),
                        ]
                        .as_slice(),
                    )
            }
        };

        (git_position.is_none() && changes_paths) || is_ignore_file || is_git_exclude
    })
}

fn refresh_index(root: &Path, inner: &Weak<FileAutoInner>, ignored_paths: &[String]) -> bool {
    let entries = collect_entries(root, ignored_paths);
    let Some(inner) = inner.upgrade() else {
        return false;
    };
    publish_entries(&inner, entries);
    true
}

fn publish_entries(inner: &FileAutoInner, entries: Vec<FileEntry>) {
    let mut state = inner
        .state
        .lock()
        .expect("file autocomplete index poisoned");
    state.entries = Arc::new(entries);
    state.generation = state.generation.wrapping_add(1);
    inner.state_changed.notify_all();
}

fn collect_entries(root: &Path, ignored_paths: &[String]) -> Vec<FileEntry> {
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
            if ignored_paths.iter().any(|pattern| {
                let pattern = pattern.trim_end_matches('/');
                display == pattern || display.starts_with(&format!("{pattern}/"))
            }) {
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
        assert!(auto.wait_until_ready());
        assert!(auto.wait_until_ready());

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
        assert!(auto.wait_until_ready());

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
        assert!(auto.wait_until_ready());

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
        assert!(auto.wait_until_ready());

        let suggestions = auto.get_suggestions("tmp");

        assert!(suggestions.iter().any(|s| s.name == "important.tmp"));
        assert!(!suggestions.iter().any(|s| s.name == "hidden.tmp"));
    }

    #[test]
    fn test_index_refreshes_after_file_change() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("before.rs"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());
        assert!(auto.wait_until_ready());
        let generation = auto.generation();

        fs::write(temp.path().join("after.rs"), "").unwrap();

        assert!(auto.wait_for_generation_after(generation));
        assert!(auto
            .get_suggestions("after")
            .iter()
            .any(|suggestion| suggestion.name == "after.rs"));
    }

    #[test]
    fn test_index_refreshes_after_file_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let deleted = temp.path().join("deleted.rs");
        fs::write(&deleted, "").unwrap();
        let auto = FileAuto::new_at(temp.path());
        assert!(auto.wait_until_ready());
        let generation = auto.generation();

        fs::remove_file(deleted).unwrap();

        assert!(auto.wait_for_generation_after(generation));
        assert!(auto.get_suggestions("deleted").is_empty());
    }

    #[test]
    fn test_clones_share_the_same_index() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("shared.rs"), "").unwrap();
        let auto = FileAuto::new_at(temp.path());
        let clone = auto.clone();
        assert!(auto.wait_until_ready());

        assert_eq!(auto.generation(), clone.generation());
        assert!(clone
            .get_suggestions("shared")
            .iter()
            .any(|suggestion| suggestion.name == "shared.rs"));
    }

    #[test]
    fn test_watcher_ignores_content_changes_but_tracks_path_changes() {
        let file = PathBuf::from("src/main.rs");
        let content_change = Event::new(EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )))
        .add_path(file.clone());
        let create = Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(file);

        assert!(!event_requires_refresh(&content_change));
        assert!(event_requires_refresh(&create));
    }

    #[test]
    fn test_watcher_tracks_ignore_changes_but_ignores_git_metadata() {
        let gitignore = Event::new(EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )))
        .add_path(PathBuf::from(".gitignore"));
        let git_exclude = Event::new(EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )))
        .add_path(PathBuf::from(".git/info/exclude"));
        let git_index = Event::new(EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Content,
        )))
        .add_path(PathBuf::from(".git/index"));

        assert!(event_requires_refresh(&gitignore));
        assert!(event_requires_refresh(&git_exclude));
        assert!(!event_requires_refresh(&git_index));
    }
}
