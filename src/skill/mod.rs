use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
                    eprintln!(
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
            eprintln!("Loaded {} skills", skills.len());
        }

        Self {
            skills,
            dirs: state.dirs,
        }
    }

    pub fn get(&self, name: &str) -> Option<&SkillInfo> {
        self.skills.get(name)
    }

    pub fn all(&self) -> Vec<&SkillInfo> {
        let mut list: Vec<&SkillInfo> = self.skills.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
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
                    eprintln!("Warning: glob error scanning {}: {}", root.display(), e);
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
