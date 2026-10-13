use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::persistence::PrefsDAO;

static SKILL_STORE: OnceLock<SkillStore> = OnceLock::new();

pub fn init_skill_store(xdg_config_home: &Path, project_root: &Path) {
    let store = SkillStore::load(xdg_config_home, project_root);
    let _ = SKILL_STORE.set(store);
}

pub fn get_skill_store() -> Option<&'static SkillStore> {
    SKILL_STORE.get()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: Option<String>,
    pub location: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct SkillStore {
    skills: HashMap<String, SkillInfo>,
    dirs: HashSet<PathBuf>,
    disabled: Arc<RwLock<BTreeSet<String>>>,
}

impl SkillStore {
    pub fn load(xdg_config_home: &Path, project_root: &Path) -> Self {
        let mut state = ScanState {
            matches: HashSet::new(),
            dirs: HashSet::new(),
        };

        let global_opencode = xdg_config_home.join("opencode");
        let global_crabcode = xdg_config_home.join("crabcode");
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        // Phase 1: External dirs (.claude/, .agents/) - Claude Code compat
        // Global
        for ext_dir in [".claude", ".agents"] {
            let root = home.join(ext_dir);
            scan(&mut state, &root, "skills/**/SKILL.md", true);
        }
        // Project (walk-up from project_root)
        let mut current = project_root.to_path_buf();
        loop {
            for ext_dir in [".claude", ".agents"] {
                let root = current.join(ext_dir);
                scan(&mut state, &root, "skills/**/SKILL.md", true);
            }
            if let Some(parent) = current.parent().map(|p| p.to_path_buf()) {
                if parent == current {
                    break;
                }
                current = parent;
            } else {
                break;
            }
        }

        // Phase 2: OpenCode native dirs (.opencode/skills/, .opencode/skill/)
        for dir in [&global_opencode, &global_crabcode] {
            scan(&mut state, dir, "{skill,skills}/**/SKILL.md", false);
        }

        // Phase 3: Project .opencode/ and .crabcode/
        for proj_dir in [
            project_root.join(".opencode"),
            project_root.join(".crabcode"),
        ] {
            scan(&mut state, &proj_dir, "{skill,skills}/**/SKILL.md", false);
        }

        // Phase 4: Config skills.paths (read from crabcode config later)
        // For now, discover from .opencode + .crabcode only

        // Parse all discovered SKILL.md files
        let mut skills: HashMap<String, SkillInfo> = HashMap::new();
        let mut matches: Vec<PathBuf> = state.matches.into_iter().collect();
        matches.sort();

        for match_path in &matches {
            if let Some(info) = parse_skill_file(match_path) {
                if let Some(existing) = skills.get(&info.name) {
                    crate::startup_diag!(
                        "Warning: duplicate skill name '{}' (existing: {}, duplicate: {})",
                        info.name,
                        existing.location.display(),
                        match_path.display()
                    );
                }
                skills.insert(info.name.clone(), info);
            }
        }

        if !skills.is_empty() {
            crate::startup_diag!("Loaded {} skills", skills.len());
        }

        let store = Self {
            skills,
            dirs: state.dirs,
            disabled: Arc::new(RwLock::new(BTreeSet::new())),
        };
        if let Err(err) = PrefsDAO::new().and_then(|prefs| store.load_preferences(&prefs)) {
            crate::startup_diag!("Warning: could not load skill preferences: {err}");
        }
        store
    }

    fn load_preferences(&self, prefs: &PrefsDAO) -> anyhow::Result<()> {
        *self.disabled.write().unwrap() = prefs.get_disabled_skills()?;
        Ok(())
    }

    /// Lookup for execution: disabled skills cannot be loaded.
    pub fn get(&self, name: &str) -> Option<&SkillInfo> {
        self.get_installed(name).filter(|_| self.is_enabled(name))
    }

    pub fn get_installed(&self, name: &str) -> Option<&SkillInfo> {
        self.skills.get(name)
    }

    /// The enabled catalog used by prompts, suggestions, and integrations.
    pub fn all(&self) -> Vec<&SkillInfo> {
        let disabled = self.disabled.read().unwrap();
        self.installed()
            .into_iter()
            .filter(|skill| !disabled.contains(&skill.name))
            .collect()
    }

    /// The full catalog for management, including disabled skills, sorted A–Z
    /// without treating uppercase names as a separate group.
    pub fn installed(&self) -> Vec<&SkillInfo> {
        let mut list: Vec<&SkillInfo> = self.skills.values().collect();
        list.sort_by_cached_key(|skill| (skill.name.to_lowercase(), skill.name.as_str()));
        list
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.skills.contains_key(name) && !self.disabled.read().unwrap().contains(name)
    }

    /// Persist before publishing the change, so a failed save leaves runtime
    /// availability unchanged. Clones share activation state with tool callers.
    pub fn set_enabled(&self, name: &str, enabled: bool, prefs: &PrefsDAO) -> anyhow::Result<()> {
        if !self.skills.contains_key(name) {
            anyhow::bail!("Skill \"{name}\" not found");
        }
        let mut disabled = self.disabled.write().unwrap();
        *disabled = prefs.set_skill_enabled(name, enabled)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn for_test(skills: impl IntoIterator<Item = SkillInfo>) -> Self {
        Self {
            skills: skills
                .into_iter()
                .map(|skill| (skill.name.clone(), skill))
                .collect(),
            dirs: HashSet::new(),
            disabled: Arc::new(RwLock::new(BTreeSet::new())),
        }
    }

    pub fn dirs(&self) -> &HashSet<PathBuf> {
        &self.dirs
    }
}

struct ScanState {
    matches: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

fn scan(state: &mut ScanState, root: &Path, pattern: &str, dot: bool) {
    if !root.is_dir() {
        return;
    }

    // Support both brace expansion patterns and simple globs
    let patterns: Vec<String> = if pattern.contains('{') {
        // Expand brace: "{skill,skills}/**/SKILL.md" -> ["skill/**/SKILL.md", "skills/**/SKILL.md"]
        expand_braces(pattern)
    } else {
        vec![pattern.to_string()]
    };

    for p in &patterns {
        let full_pattern = root.join(p).to_string_lossy().to_string();
        match glob::glob(&full_pattern) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.is_file() {
                        state.matches.insert(entry.clone());
                        if let Some(parent) = entry.parent() {
                            state.dirs.insert(parent.to_path_buf());
                        }
                    }
                }
            }
            Err(e) => {
                if !dot {
                    crate::startup_diag!("Warning: glob error scanning {}: {}", root.display(), e);
                }
            }
        }
    }
}

fn expand_braces(pattern: &str) -> Vec<String> {
    // Simple brace expansion for "{skill,skills}/**/SKILL.md" style patterns
    if let Some(brace_start) = pattern.find('{') {
        if let Some(brace_end) = pattern.find('}') {
            if brace_end > brace_start {
                let prefix = &pattern[..brace_start];
                let options = &pattern[brace_start + 1..brace_end];
                let suffix = &pattern[brace_end + 1..];
                return options
                    .split(',')
                    .map(|opt| format!("{}{}{}", prefix, opt.trim(), suffix))
                    .collect();
            }
        }
    }
    vec![pattern.to_string()]
}

fn parse_skill_file(path: &Path) -> Option<SkillInfo> {
    let content = fs::read_to_string(path).ok()?;

    // Parse YAML frontmatter between --- delimiters
    let (frontmatter, body) = if let Some(rest) = content.strip_prefix("---\n") {
        if let Some((fm, rest)) = rest.split_once("\n---") {
            (fm.to_string(), rest.trim_start().to_string())
        } else if let Some((fm, rest)) = rest.split_once("\r\n---") {
            (fm.to_string(), rest.trim_start().to_string())
        } else {
            // No closing ---, treat whole content as body
            (String::new(), content)
        }
    } else if let Some(rest) = content.strip_prefix("---\r\n") {
        if let Some((fm, rest)) = rest.split_once("\r\n---") {
            (fm.to_string(), rest.trim_start().to_string())
        } else {
            (String::new(), content)
        }
    } else {
        (String::new(), content)
    };

    if frontmatter.is_empty() {
        return None;
    }

    #[derive(Deserialize)]
    struct Frontmatter {
        name: String,
        description: Option<String>,
    }

    // Try serde_yaml first, then fallback sanitization
    let fm_data: Frontmatter = match serde_yaml::from_str(&frontmatter) {
        Ok(fm) => fm,
        Err(_) => {
            // Fallback: sanitize malformed YAML (Claude Code compat)
            let sanitized = fallback_sanitize_yaml(&frontmatter);
            serde_yaml::from_str(&sanitized).ok()?
        }
    };

    Some(SkillInfo {
        name: fm_data.name,
        description: fm_data.description,
        location: path.to_path_buf(),
        content: body,
    })
}

fn fallback_sanitize_yaml(frontmatter: &str) -> String {
    let mut result = String::new();

    for line in frontmatter.lines() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.starts_with('#') || trimmed.is_empty() {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Skip indented lines (continuations)
        if line.starts_with(' ') || line.starts_with('\t') {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Match key: value
        if let Some((key, value)) = trimmed.split_once(':') {
            let value = value.trim();

            // Skip empty, already quoted, or block scalar values
            if value.is_empty()
                || value == ">"
                || value == "|"
                || value.starts_with('"')
                || value.starts_with('\'')
            {
                result.push_str(line);
                result.push('\n');
                continue;
            }

            // If value contains a colon, convert to block scalar
            if value.contains(':') {
                result.push_str(&format!("{}: |-\n", key));
                result.push_str(&format!("  {}\n", value));
                continue;
            }
        }

        result.push_str(line);
        result.push('\n');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SkillStore {
        SkillStore::for_test(["beta", "alpha"].map(|name| SkillInfo {
            name: name.to_string(),
            description: Some(format!("Use {name}")),
            location: PathBuf::from(format!("/skills/{name}/SKILL.md")),
            content: format!("{name} instructions"),
        }))
    }

    #[test]
    fn disabled_skills_stay_installed_but_are_not_available() {
        let store = test_store();
        let clone = store.clone();
        let prefs = PrefsDAO::in_memory();
        assert_eq!(
            store
                .all()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert!(!store.is_enabled("missing"));

        store.set_enabled("alpha", false, &prefs).unwrap();
        assert!(store.get("alpha").is_none());
        assert!(clone.get("alpha").is_none());
        assert!(store.get_installed("alpha").is_some());
        assert_eq!(store.installed().len(), 2);
        assert_eq!(
            store
                .all()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["beta"]
        );

        let guidance = crate::prompt::render_skill_guidance(store.all());
        assert!(!guidance.contains("<name>alpha</name>"));
        assert!(guidance.contains("<name>beta</name>"));

        // Reconstructed activation state, as on restart, retains disabled names.
        let reloaded = test_store();
        reloaded.load_preferences(&prefs).unwrap();
        assert!(reloaded.get("alpha").is_none());

        store.set_enabled("alpha", true, &prefs).unwrap();
        assert!(clone.get("alpha").is_some());
        assert!(prefs.get_disabled_skills().unwrap().is_empty());
    }

    #[test]
    fn disabled_preferences_survive_removal_and_reinstallation_by_name() {
        let prefs = PrefsDAO::in_memory();
        let store = test_store();
        store.set_enabled("alpha", false, &prefs).unwrap();

        let removed = SkillStore::for_test([store.get_installed("beta").unwrap().clone()]);
        removed.load_preferences(&prefs).unwrap();
        assert!(removed.get_installed("alpha").is_none());
        removed.set_enabled("beta", false, &prefs).unwrap();
        assert_eq!(
            prefs.get_disabled_skills().unwrap(),
            ["alpha".to_string(), "beta".to_string()].into()
        );

        let reinstalled = test_store();
        reinstalled.load_preferences(&prefs).unwrap();
        assert!(reinstalled.get_installed("alpha").is_some());
        assert!(!reinstalled.is_enabled("alpha"));
        assert!(!reinstalled.is_enabled("beta"));

        // A new frontmatter name is a new skill, even at the same location.
        let mut renamed = store.get_installed("alpha").unwrap().clone();
        renamed.name = "renamed-alpha".to_string();
        let renamed = SkillStore::for_test([renamed]);
        renamed.load_preferences(&prefs).unwrap();
        assert!(renamed.is_enabled("renamed-alpha"));
    }

    #[test]
    fn disabling_all_skills_removes_prompt_guidance() {
        let store = test_store();
        let prefs = PrefsDAO::in_memory();
        for skill in store.installed() {
            store.set_enabled(&skill.name, false, &prefs).unwrap();
        }
        assert!(store.all().is_empty());
        assert!(crate::prompt::render_skill_guidance(store.all()).is_empty());
        assert_eq!(store.installed().len(), 2);
    }

    #[test]
    fn restored_skill_catalog_tracks_activation_without_rewriting_other_rules() {
        let store = test_store();
        let prefs = PrefsDAO::in_memory();
        let mut prompt = format!(
            "base rules\n\n{}\n\ncustom rules <available_skills>not our catalog</available_skills>",
            crate::prompt::render_skill_guidance(store.all())
        );
        let original = prompt.clone();
        assert!(!crate::prompt::refresh_skill_guidance(
            &mut prompt,
            store.all()
        ));
        assert_eq!(prompt, original);

        store.set_enabled("alpha", false, &prefs).unwrap();
        assert!(crate::prompt::refresh_skill_guidance(
            &mut prompt,
            store.all()
        ));
        assert!(!prompt.contains("<name>alpha</name>"));
        assert!(prompt.contains("<name>beta</name>"));
        assert!(prompt.starts_with("base rules"));
        assert!(
            prompt.ends_with("custom rules <available_skills>not our catalog</available_skills>")
        );

        store.set_enabled("beta", false, &prefs).unwrap();
        crate::prompt::refresh_skill_guidance(&mut prompt, store.all());
        assert!(!prompt.contains("Skills provide specialized instructions"));
        assert!(prompt.contains("not our catalog"));

        store.set_enabled("alpha", true, &prefs).unwrap();
        crate::prompt::refresh_skill_guidance(&mut prompt, store.all());
        assert!(prompt.contains("<name>alpha</name>"));
        assert!(!prompt.contains("<name>beta</name>"));
        let refreshed = prompt.clone();
        crate::prompt::refresh_skill_guidance(&mut prompt, store.all());
        assert_eq!(prompt, refreshed);
    }

    #[test]
    fn malformed_activation_preferences_do_not_change_availability() {
        let store = test_store();
        let prefs = PrefsDAO::in_memory();
        prefs
            .set_json_pref("disabled_skills", &serde_json::json!("invalid"))
            .unwrap();
        assert!(store.set_enabled("alpha", false, &prefs).is_err());
        assert!(store.is_enabled("alpha"));
        assert!(store.set_enabled("missing", false, &prefs).is_err());
    }

    #[test]
    fn test_fallback_sanitize_yaml() {
        let input = "name: test\ndescription: Use: build stuff with colons: here\nstatus: ok";
        let result = fallback_sanitize_yaml(input);
        assert!(result.contains("description: |-"));
        assert!(result.contains("  Use: build stuff with colons: here"));
        assert!(result.contains("status: ok"));
    }

    #[test]
    fn test_expand_braces() {
        let result = expand_braces("{skill,skills}/**/SKILL.md");
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"skill/**/SKILL.md".to_string()));
        assert!(result.contains(&"skills/**/SKILL.md".to_string()));
    }
}
