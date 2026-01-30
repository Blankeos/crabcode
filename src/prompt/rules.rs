use std::path::{Path, PathBuf};

const DEFAULT_MAX_RULE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct RuleFile {
    path: PathBuf,
    contents: String,
    truncated: bool,
}

#[derive(Debug, Clone, Default)]
struct ResolvedRules {
    local: Option<RuleFile>,
    global: Option<RuleFile>,
}

#[derive(Debug, Clone)]
struct ResolveOptions {
    config_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    disable_claude_code: bool,
    disable_claude_code_prompt: bool,
    max_bytes: usize,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            config_dir: dirs::config_dir(),
            home_dir: dirs::home_dir(),
            disable_claude_code: env_truthy("CRABCODE_DISABLE_CLAUDE_CODE"),
            disable_claude_code_prompt: env_truthy("CRABCODE_DISABLE_CLAUDE_CODE_PROMPT"),
            max_bytes: DEFAULT_MAX_RULE_BYTES,
        }
    }
}

pub async fn get_custom_instructions(working_directory: &str) -> String {
    let rules = resolve_rules(Path::new(working_directory), ResolveOptions::default()).await;
    format_rules_for_prompt(&rules)
}

async fn resolve_rules(start_dir: &Path, opts: ResolveOptions) -> ResolvedRules {
    let local = resolve_local_rules(start_dir, &opts).await;
    let global = resolve_global_rules(&opts).await;
    ResolvedRules { local, global }
}

async fn resolve_local_rules(start_dir: &Path, opts: &ResolveOptions) -> Option<RuleFile> {
    let allow_claude_local = !opts.disable_claude_code;

    let mut dir = start_dir.to_path_buf();
    if !dir.is_dir() {
        dir.pop();
    }

    loop {
        let agents = dir.join("AGENTS.md");
        if file_exists(&agents).await {
            if let Some(rule) = read_rule_file(&agents, opts.max_bytes).await {
                return Some(rule);
            }
        }

        if allow_claude_local {
            let claudemd = dir.join("CLAUDE.md");
            if file_exists(&claudemd).await {
                if let Some(rule) = read_rule_file(&claudemd, opts.max_bytes).await {
                    return Some(rule);
                }
            }
        }

        if !dir.pop() {
            break;
        }
    }

    None
}

async fn resolve_global_rules(opts: &ResolveOptions) -> Option<RuleFile> {
    if let Some(config_dir) = &opts.config_dir {
        let global_agents = config_dir.join("crabcode").join("AGENTS.md");
        if file_exists(&global_agents).await {
            if let Some(rule) = read_rule_file(&global_agents, opts.max_bytes).await {
                return Some(rule);
            }
        }
    }

    let allow_claude_global = !opts.disable_claude_code && !opts.disable_claude_code_prompt;
    if allow_claude_global {
        if let Some(home_dir) = &opts.home_dir {
            let claude_global = home_dir.join(".claude").join("CLAUDE.md");
            if file_exists(&claude_global).await {
                if let Some(rule) = read_rule_file(&claude_global, opts.max_bytes).await {
                    return Some(rule);
                }
            }
        }
    }

    None
}

fn format_rules_for_prompt(rules: &ResolvedRules) -> String {
    let mut out = String::new();

    if let Some(local) = &rules.local {
        push_rule_section(&mut out, local);
    }

    if let Some(global) = &rules.global {
        if !out.is_empty() {
            out.push_str("\n\n---\n\n");
        }
        push_rule_section(&mut out, global);
    }

    out
}

fn push_rule_section(out: &mut String, rule: &RuleFile) {
    let path_str = display_path_best_effort(&rule.path);
    out.push_str("Instructions from: ");
    out.push_str(&path_str);
    out.push('\n');
    out.push_str(&rule.contents);

    if rule.truncated {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\n[crabcode] Note: instructions truncated due to size limit.\n");
    } else if !out.ends_with('\n') {
        out.push('\n');
    }
}

fn display_path_best_effort(path: &Path) -> String {
    // Best-effort canonicalization; never fail prompt creation.
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

async fn read_rule_file(path: &Path, max_bytes: usize) -> Option<RuleFile> {
    let bytes = tokio::fs::read(path).await.ok()?;

    let (slice, truncated) = if bytes.len() > max_bytes {
        (&bytes[..max_bytes], true)
    } else {
        (&bytes[..], false)
    };

    let contents = String::from_utf8_lossy(slice).to_string();
    Some(RuleFile {
        path: path.to_path_buf(),
        contents,
        truncated,
    })
}

async fn file_exists(path: &Path) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(m) => m.is_file(),
        Err(_) => false,
    }
}

fn env_truthy(key: &str) -> bool {
    let v = std::env::var(key).unwrap_or_default();
    if v.is_empty() {
        return false;
    }
    matches!(
        v.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("crabcode_{prefix}_{nanos}"))
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[tokio::test]
    async fn local_prefers_agents_over_claude_same_dir() {
        let root = unique_temp_dir("rules1");
        fs::create_dir_all(&root).unwrap();
        write_file(&root.join("AGENTS.md"), "agents");
        write_file(&root.join("CLAUDE.md"), "claude");

        let opts = ResolveOptions {
            config_dir: None,
            home_dir: None,
            disable_claude_code: false,
            disable_claude_code_prompt: false,
            max_bytes: 1024,
        };
        let rules = resolve_rules(&root, opts).await;
        assert!(rules.local.is_some());
        assert_eq!(rules.local.unwrap().contents, "agents");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn upward_traversal_finds_parent_rules() {
        let root = unique_temp_dir("rules2");
        let child = root.join("a").join("b");
        fs::create_dir_all(&child).unwrap();
        write_file(&root.join("AGENTS.md"), "root agents");

        let opts = ResolveOptions {
            config_dir: None,
            home_dir: None,
            disable_claude_code: false,
            disable_claude_code_prompt: false,
            max_bytes: 1024,
        };
        let rules = resolve_rules(&child, opts).await;
        assert!(rules.local.is_some());
        assert_eq!(rules.local.unwrap().contents, "root agents");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn first_match_wins_child_claude_beats_parent_agents() {
        let root = unique_temp_dir("rules3");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        write_file(&root.join("AGENTS.md"), "parent agents");
        write_file(&child.join("CLAUDE.md"), "child claude");

        let opts = ResolveOptions {
            config_dir: None,
            home_dir: None,
            disable_claude_code: false,
            disable_claude_code_prompt: false,
            max_bytes: 1024,
        };
        let rules = resolve_rules(&child, opts).await;
        assert!(rules.local.is_some());
        assert_eq!(rules.local.unwrap().contents, "child claude");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn global_prefers_config_agents_over_claude() {
        let root = unique_temp_dir("rules4");
        let config_dir = root.join("config");
        let home_dir = root.join("home");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();

        write_file(
            &config_dir.join("crabcode").join("AGENTS.md"),
            "global agents",
        );
        write_file(&home_dir.join(".claude").join("CLAUDE.md"), "global claude");

        let opts = ResolveOptions {
            config_dir: Some(config_dir),
            home_dir: Some(home_dir),
            disable_claude_code: false,
            disable_claude_code_prompt: false,
            max_bytes: 1024,
        };
        let rules = resolve_rules(&root, opts).await;
        assert!(rules.global.is_some());
        assert_eq!(rules.global.unwrap().contents, "global agents");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn global_claude_disabled_by_prompt_flag() {
        let root = unique_temp_dir("rules5");
        let home_dir = root.join("home");
        fs::create_dir_all(&home_dir).unwrap();
        write_file(&home_dir.join(".claude").join("CLAUDE.md"), "global claude");

        let opts = ResolveOptions {
            config_dir: None,
            home_dir: Some(home_dir),
            disable_claude_code: false,
            disable_claude_code_prompt: true,
            max_bytes: 1024,
        };
        let rules = resolve_rules(&root, opts).await;
        assert!(rules.global.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn local_claude_disabled_by_global_flag() {
        let root = unique_temp_dir("rules6");
        fs::create_dir_all(&root).unwrap();
        write_file(&root.join("CLAUDE.md"), "claude");

        let opts = ResolveOptions {
            config_dir: None,
            home_dir: None,
            disable_claude_code: true,
            disable_claude_code_prompt: false,
            max_bytes: 1024,
        };
        let rules = resolve_rules(&root, opts).await;
        assert!(rules.local.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn truncates_large_files() {
        let root = unique_temp_dir("rules7");
        fs::create_dir_all(&root).unwrap();
        let big = "a".repeat(2048);
        write_file(&root.join("AGENTS.md"), &big);

        let opts = ResolveOptions {
            config_dir: None,
            home_dir: None,
            disable_claude_code: false,
            disable_claude_code_prompt: false,
            max_bytes: 64,
        };
        let rules = resolve_rules(&root, opts).await;
        let rf = rules.local.unwrap();
        assert!(rf.truncated);
        assert_eq!(rf.contents.len(), 64);

        let _ = fs::remove_dir_all(&root);
    }
}
