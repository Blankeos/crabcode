use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

pub fn discover_themes(
    xdg_config_home: &Path,
    project_root: &Path,
    cwd: &Path,
    selected_theme_id: Option<&str>,
) -> (Vec<crate::theme::Theme>, usize) {
    let mut theme_by_id: HashMap<String, usize> = HashMap::new();
    let mut themes: Vec<crate::theme::Theme> = Vec::new();

    let mut layers: Vec<Vec<PathBuf>> = Vec::new();

    let mut built_in = Vec::new();
    if PathBuf::from("src/theme.json").is_file() {
        built_in.push(PathBuf::from("src/theme.json"));
    }
    built_in.extend(list_json_files(Path::new("src/themes")));
    built_in.extend(list_json_files(Path::new("src/generated_themes")));
    layers.push(built_in);

    layers.push(list_json_files(
        &xdg_config_home.join("opencode").join("themes"),
    ));
    layers.push(list_json_files(
        &xdg_config_home.join("crabcode").join("themes"),
    ));
    layers.push(list_json_files(
        &project_root.join(".opencode").join("themes"),
    ));
    layers.push(list_json_files(
        &project_root.join(".crabcode").join("themes"),
    ));
    if cwd != project_root {
        layers.push(list_json_files(&cwd.join(".opencode").join("themes")));
    }

    for files in layers {
        for path in files {
            let Ok(theme) = crate::theme::Theme::load_from_file(&path) else {
                continue;
            };
            if let Some(idx) = theme_by_id.get(&theme.id).copied() {
                themes[idx] = theme;
            } else {
                let idx = themes.len();
                theme_by_id.insert(theme.id.clone(), idx);
                themes.push(theme);
            }
        }
    }

    if themes.is_empty() {
        let fallback = crate::theme::Theme::load_from_file("src/theme.json").unwrap_or_else(|_| {
            crate::theme::Theme::load_from_file("src/generated_themes/ayu.json").unwrap()
        });
        themes.push(fallback);
    }

    let mut selected_idx = 0usize;
    if let Some(id) = selected_theme_id {
        if let Some((idx, _)) = themes.iter().enumerate().find(|(_, t)| t.id == id) {
            selected_idx = idx;
        }
    }

    (themes, selected_idx)
}

fn list_json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case("json") {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    OpenCode,
    Crabcode,
}

#[derive(Debug, Clone)]
struct SourceFile {
    label: &'static str,
    kind: SourceKind,
    path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigDiagnostics {
    pub warnings: Vec<String>,
    pub info: Vec<String>,
    pub unimplemented_keys: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ConfigInventory {
    pub opencode_agents: Vec<PathBuf>,
    pub opencode_skills_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct SoundEffectConfig {
    pub file: Option<PathBuf>,
    pub enabled: bool,
}

impl SoundEffectConfig {
    pub fn is_effectively_enabled(&self) -> bool {
        self.enabled && self.file.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct SoundsConfig {
    pub error: SoundEffectConfig,
    pub complete: SoundEffectConfig,
    pub permission: SoundEffectConfig,
    pub question: SoundEffectConfig,
}

impl Default for SoundsConfig {
    fn default() -> Self {
        Self {
            error: SoundEffectConfig {
                file: None,
                enabled: false,
            },
            complete: SoundEffectConfig {
                file: None,
                enabled: true,
            },
            permission: SoundEffectConfig {
                file: None,
                enabled: false,
            },
            question: SoundEffectConfig {
                file: None,
                enabled: false,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MergedConfig {
    pub theme: Option<String>,
    pub model: Option<String>,
    pub sounds: SoundsConfig,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub merged_config: MergedConfig,
    pub raw_merged: Value,
    pub diagnostics: ConfigDiagnostics,
    pub inventory: ConfigInventory,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub xdg_config_home: PathBuf,
}

pub struct ConfigLoader;

impl ConfigLoader {
    pub fn load() -> Result<LoadedConfig> {
        let cwd = std::env::current_dir().context("Failed to determine current directory")?;
        let xdg_config_home = xdg_config_home();
        let project_root = discover_project_root(&cwd);

        let mut diagnostics = ConfigDiagnostics::default();
        let mut inventory = ConfigInventory::default();

        discover_opencode_inventory(
            &xdg_config_home,
            &project_root,
            &mut inventory,
            &mut diagnostics,
        );

        let sources = resolve_sources(&xdg_config_home, &project_root)?;

        let mut merged: Value = Value::Object(serde_json::Map::new());
        let mut provenance: HashMap<String, PathBuf> = HashMap::new();
        provenance.insert("".to_string(), cwd.clone());

        for source in sources {
            let parsed = match load_config_value(&source.path) {
                Ok(v) => v,
                Err(e) => {
                    diagnostics.warnings.push(format!(
                        "Failed to parse {} config at {}: {}",
                        source.label,
                        source.path.display(),
                        e
                    ));
                    continue;
                }
            };

            let filtered = filter_top_level(parsed, source.kind);
            let base_dir = source
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| cwd.clone());
            deep_merge_with_provenance(
                &mut merged,
                &filtered,
                "".to_string(),
                &base_dir,
                &mut provenance,
            );
        }

        substitute_placeholders(&mut merged, &provenance, &mut diagnostics);

        let merged_config = parse_merged_config(&merged, &mut diagnostics);
        diagnostics.unimplemented_keys = collect_unimplemented_keys(&merged);

        Ok(LoadedConfig {
            merged_config,
            raw_merged: merged,
            diagnostics,
            inventory,
            project_root,
            cwd,
            xdg_config_home,
        })
    }
}

fn xdg_config_home() -> PathBuf {
    if let Ok(val) = std::env::var("XDG_CONFIG_HOME") {
        if !val.trim().is_empty() {
            return PathBuf::from(val);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
}

fn discover_project_root(cwd: &Path) -> PathBuf {
    let mut current = cwd.to_path_buf();
    let mut saw_git = false;
    loop {
        if current.join(".git").is_dir() {
            saw_git = true;
            break;
        }
        let parent = match current.parent() {
            Some(p) => p.to_path_buf(),
            None => break,
        };
        if parent == current {
            break;
        }
        current = parent;
    }

    if saw_git {
        current
    } else {
        cwd.to_path_buf()
    }
}

fn discover_opencode_inventory(
    xdg_config_home: &Path,
    project_root: &Path,
    inventory: &mut ConfigInventory,
    diagnostics: &mut ConfigDiagnostics,
) {
    let global_opencode = xdg_config_home.join("opencode");
    let local_opencode = project_root.join(".opencode");

    let mut agents = Vec::new();
    agents.extend(list_md_files(&global_opencode.join("agents")));
    agents.extend(list_md_files(&global_opencode.join("agent")));
    agents.extend(list_md_files(&local_opencode.join("agents")));
    agents.extend(list_md_files(&local_opencode.join("agent")));
    agents.sort();
    agents.dedup();

    if !agents.is_empty() {
        diagnostics
            .info
            .push(format!("Discovered {} OpenCode agent files", agents.len()));
    }
    inventory.opencode_agents = agents;

    let mut skills_dirs = Vec::new();
    for dir in [
        global_opencode.join("skills"),
        global_opencode.join("skill"),
        local_opencode.join("skills"),
        local_opencode.join("skill"),
    ] {
        if dir.is_dir() {
            skills_dirs.push(dir);
        }
    }
    skills_dirs.sort();
    skills_dirs.dedup();

    if !skills_dirs.is_empty() {
        diagnostics.info.push(format!(
            "Discovered {} OpenCode skills dirs",
            skills_dirs.len()
        ));
    }
    inventory.opencode_skills_dirs = skills_dirs;
}

fn list_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case("md") {
                    out.push(path);
                }
            }
        }
    }
    out
}

fn resolve_sources(xdg_config_home: &Path, project_root: &Path) -> Result<Vec<SourceFile>> {
    let mut out = Vec::new();

    let opencode_global = resolve_single_layer(
        "OpenCode global",
        SourceKind::OpenCode,
        &[
            xdg_config_home.join("opencode").join("opencode.jsonc"),
            xdg_config_home.join("opencode").join("opencode.json"),
            xdg_config_home.join("opencode.jsonc"),
            xdg_config_home.join("opencode.json"),
        ],
    )?;
    if let Some(path) = opencode_global {
        out.push(SourceFile {
            label: "OpenCode global",
            kind: SourceKind::OpenCode,
            path,
        });
    }

    let crabcode_global = resolve_single_layer(
        "Crabcode global",
        SourceKind::Crabcode,
        &[
            xdg_config_home.join("crabcode").join("crabcode.jsonc"),
            xdg_config_home.join("crabcode").join("crabcode.json"),
            xdg_config_home.join("crabcode.jsonc"),
            xdg_config_home.join("crabcode.json"),
        ],
    )?;
    if let Some(path) = crabcode_global {
        out.push(SourceFile {
            label: "Crabcode global",
            kind: SourceKind::Crabcode,
            path,
        });
    }

    let opencode_local = resolve_single_layer(
        "OpenCode local",
        SourceKind::OpenCode,
        &[
            project_root.join(".opencode").join("opencode.jsonc"),
            project_root.join(".opencode").join("opencode.json"),
            project_root.join("opencode.jsonc"),
            project_root.join("opencode.json"),
        ],
    )?;
    if let Some(path) = opencode_local {
        out.push(SourceFile {
            label: "OpenCode local",
            kind: SourceKind::OpenCode,
            path,
        });
    }

    let crabcode_local = resolve_single_layer(
        "Crabcode local",
        SourceKind::Crabcode,
        &[
            project_root.join(".crabcode").join("crabcode.jsonc"),
            project_root.join(".crabcode").join("crabcode.json"),
            project_root.join(".opencode").join("crabcode.jsonc"),
            project_root.join(".opencode").join("crabcode.json"),
            project_root.join("crabcode.jsonc"),
            project_root.join("crabcode.json"),
        ],
    )?;
    if let Some(path) = crabcode_local {
        out.push(SourceFile {
            label: "Crabcode local",
            kind: SourceKind::Crabcode,
            path,
        });
    }

    Ok(out)
}

fn resolve_single_layer(
    label: &'static str,
    _kind: SourceKind,
    candidates: &[PathBuf],
) -> Result<Option<PathBuf>> {
    let existing: Vec<PathBuf> = candidates.iter().filter(|p| p.is_file()).cloned().collect();
    if existing.len() > 1 {
        let mut msg = format!(
            "Multiple config files found for {}. Keep only one:\n",
            label
        );
        for p in existing {
            msg.push_str(&format!("- {}\n", p.display()));
        }
        return Err(anyhow!(msg));
    }
    Ok(existing.into_iter().next())
}

fn load_config_value(path: &Path) -> Result<Value> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file {}", path.display()))?;

    match path.extension().and_then(|s| s.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("jsonc") => {
            let v: Value = json5::from_str(&content)
                .with_context(|| format!("Invalid JSONC in {}", path.display()))?;
            Ok(v)
        }
        _ => {
            let v: Value = serde_json::from_str(&content)
                .with_context(|| format!("Invalid JSON in {}", path.display()))?;
            Ok(v)
        }
    }
}

fn filter_top_level(value: Value, kind: SourceKind) -> Value {
    let mut map = match value {
        Value::Object(m) => m,
        _ => return Value::Object(serde_json::Map::new()),
    };

    let allow: BTreeSet<&'static str> = match kind {
        SourceKind::OpenCode => opencode_allowed_keys(),
        SourceKind::Crabcode => crabcode_allowed_keys(),
    };

    let ignore: BTreeSet<&'static str> = match kind {
        SourceKind::OpenCode => opencode_ignored_keys(),
        SourceKind::Crabcode => BTreeSet::new(),
    };

    map.retain(|k, _| {
        let k = k.as_str();
        if ignore.contains(k) {
            return false;
        }
        allow.contains(k)
    });

    Value::Object(map)
}

fn opencode_allowed_keys() -> BTreeSet<&'static str> {
    [
        "$schema",
        "agent",
        "instructions",
        "tools",
        "mcp",
        "model",
        "provider",
        "command",
        "permission",
        "compaction",
        "watcher",
        "default_agent",
        "formatter",
        "disabled_providers",
        "enabled_providers",
    ]
    .into_iter()
    .collect()
}

fn crabcode_allowed_keys() -> BTreeSet<&'static str> {
    let mut out = opencode_allowed_keys();
    out.insert("theme");
    out.insert("sounds");
    out
}

fn opencode_ignored_keys() -> BTreeSet<&'static str> {
    [
        "keybinds",
        "theme",
        "share",
        "tui",
        "server",
        "plugin",
        "tool",
        "custom tools",
        "custom_tools",
        "customTools",
        "sounds",
    ]
    .into_iter()
    .collect()
}

fn deep_merge_with_provenance(
    base: &mut Value,
    overlay: &Value,
    pointer: String,
    overlay_base_dir: &Path,
    provenance: &mut HashMap<String, PathBuf>,
) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (k, overlay_v) in overlay_map {
                let child_ptr = format!("{}/{}", pointer, escape_json_pointer(k));
                if overlay_v.is_null() {
                    base_map.remove(k);
                    remove_provenance_subtree(provenance, &child_ptr);
                    continue;
                }
                match base_map.get_mut(k) {
                    Some(base_v) => {
                        if base_v.is_object() && overlay_v.is_object() {
                            deep_merge_with_provenance(
                                base_v,
                                overlay_v,
                                child_ptr,
                                overlay_base_dir,
                                provenance,
                            );
                        } else {
                            *base_v = overlay_v.clone();
                            set_provenance_for_subtree(
                                base_v,
                                &child_ptr,
                                overlay_base_dir,
                                provenance,
                            );
                        }
                    }
                    None => {
                        base_map.insert(k.clone(), overlay_v.clone());
                        if let Some(v) = base_map.get(k) {
                            set_provenance_for_subtree(v, &child_ptr, overlay_base_dir, provenance);
                        }
                    }
                }
            }
        }
        (base_slot, overlay_v) => {
            if overlay_v.is_null() {
                *base_slot = Value::Object(serde_json::Map::new());
                remove_provenance_subtree(provenance, &pointer);
                return;
            }
            *base_slot = overlay_v.clone();
            set_provenance_for_subtree(base_slot, &pointer, overlay_base_dir, provenance);
        }
    }
}

fn remove_provenance_subtree(provenance: &mut HashMap<String, PathBuf>, pointer: &str) {
    let keys: Vec<String> = provenance
        .keys()
        .filter(|k| k == &pointer || k.starts_with(&(pointer.to_string() + "/")))
        .cloned()
        .collect();
    for k in keys {
        provenance.remove(&k);
    }
}

fn set_provenance_for_subtree(
    value: &Value,
    pointer: &str,
    overlay_base_dir: &Path,
    provenance: &mut HashMap<String, PathBuf>,
) {
    remove_provenance_subtree(provenance, pointer);
    provenance.insert(pointer.to_string(), overlay_base_dir.to_path_buf());

    if matches!(value, Value::Object(_) | Value::Array(_)) {
        // Child pointers are resolved by nearest ancestor, so we don't need to enumerate.
    }
}

fn escape_json_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn substitute_placeholders(
    value: &mut Value,
    provenance: &HashMap<String, PathBuf>,
    diagnostics: &mut ConfigDiagnostics,
) {
    let re = Regex::new(r"\{(env|file):([^}]+)\}").unwrap();
    substitute_placeholders_inner(value, "".to_string(), provenance, diagnostics, &re);
}

fn substitute_placeholders_inner(
    value: &mut Value,
    pointer: String,
    provenance: &HashMap<String, PathBuf>,
    diagnostics: &mut ConfigDiagnostics,
    re: &Regex,
) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let child_ptr = format!("{}/{}", pointer, escape_json_pointer(k));
                substitute_placeholders_inner(v, child_ptr, provenance, diagnostics, re);
            }
        }
        Value::Array(arr) => {
            for (idx, v) in arr.iter_mut().enumerate() {
                let child_ptr = format!("{}/{}", pointer, idx);
                substitute_placeholders_inner(v, child_ptr, provenance, diagnostics, re);
            }
        }
        Value::String(s) => {
            let base_dir = find_base_dir_for_pointer(provenance, &pointer);
            let replaced = replace_in_string(s, &base_dir, diagnostics, re);
            *s = replaced;
        }
        _ => {}
    }
}

fn find_base_dir_for_pointer(provenance: &HashMap<String, PathBuf>, pointer: &str) -> PathBuf {
    let mut cur = pointer.to_string();
    loop {
        if let Some(p) = provenance.get(&cur) {
            return p.clone();
        }
        if cur.is_empty() {
            return PathBuf::from(".");
        }
        if let Some((parent, _)) = cur.rsplit_once('/') {
            cur = parent.to_string();
        } else {
            cur.clear();
        }
    }
}

fn replace_in_string(
    s: &str,
    base_dir: &Path,
    diagnostics: &mut ConfigDiagnostics,
    re: &Regex,
) -> String {
    re.replace_all(s, |caps: &regex::Captures<'_>| {
        let kind = &caps[1];
        let arg = caps[2].trim();
        match kind {
            "env" => std::env::var(arg).unwrap_or_default(),
            "file" => {
                let path = expand_path(arg, base_dir);
                match fs::read_to_string(&path) {
                    Ok(content) => trim_trailing_newlines(&content),
                    Err(e) => {
                        diagnostics.warnings.push(format!(
                            "Failed to read file for placeholder {{file:{}}} at {}: {}",
                            arg,
                            path.display(),
                            e
                        ));
                        String::new()
                    }
                }
            }
            _ => String::new(),
        }
    })
    .to_string()
}

fn trim_trailing_newlines(s: &str) -> String {
    s.trim_end_matches(['\n', '\r']).to_string()
}

fn expand_path(arg: &str, base_dir: &Path) -> PathBuf {
    let arg = arg.trim();
    if let Some(rest) = arg.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if arg == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    let p = PathBuf::from(arg);
    if p.is_absolute() {
        p
    } else {
        base_dir.join(p)
    }
}

fn parse_merged_config(merged: &Value, diagnostics: &mut ConfigDiagnostics) -> MergedConfig {
    let mut out = MergedConfig::default();
    let obj = match merged.as_object() {
        Some(o) => o,
        None => return out,
    };

    if let Some(Value::String(theme)) = obj.get("theme") {
        if !theme.trim().is_empty() {
            out.theme = Some(theme.trim().to_string());
        }
    }

    if let Some(Value::String(model)) = obj.get("model") {
        if !model.trim().is_empty() {
            out.model = Some(model.trim().to_string());
        }
    }

    out.sounds = parse_sounds(obj.get("sounds"), diagnostics);

    out
}

fn parse_sounds(value: Option<&Value>, diagnostics: &mut ConfigDiagnostics) -> SoundsConfig {
    let mut sounds = SoundsConfig::default();
    let Some(Value::Object(map)) = value else {
        return sounds;
    };

    apply_sound_event(
        &mut sounds.error,
        map.get("error"),
        "sounds.error",
        diagnostics,
    );
    apply_sound_event(
        &mut sounds.complete,
        map.get("complete"),
        "sounds.complete",
        diagnostics,
    );
    apply_sound_event(
        &mut sounds.permission,
        map.get("permission"),
        "sounds.permission",
        diagnostics,
    );
    apply_sound_event(
        &mut sounds.question,
        map.get("question"),
        "sounds.question",
        diagnostics,
    );

    sounds
}

fn apply_sound_event(
    target: &mut SoundEffectConfig,
    value: Option<&Value>,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) {
    let Some(Value::Object(map)) = value else {
        return;
    };

    if let Some(Value::Bool(enabled)) = map.get("enabled") {
        target.enabled = *enabled;
    }

    if let Some(Value::String(file)) = map.get("file") {
        let p = PathBuf::from(file);
        if p.is_absolute() {
            target.file = Some(p);
        } else {
            diagnostics.warnings.push(format!(
                "{}: sound file must be an absolute path; treating as disabled",
                key
            ));
            target.file = None;
            target.enabled = false;
        }
    }

    if target.file.is_none() {
        target.enabled = false;
    }
}

fn collect_unimplemented_keys(merged: &Value) -> Vec<String> {
    let Some(obj) = merged.as_object() else {
        return Vec::new();
    };

    let supported: BTreeSet<&'static str> = crabcode_allowed_keys();
    let implemented: BTreeSet<&'static str> = ["theme", "model", "sounds"].into_iter().collect();

    let mut keys = Vec::new();
    for k in obj.keys() {
        let ks = k.as_str();
        if ks == "$schema" {
            continue;
        }
        if supported.contains(ks) && !implemented.contains(ks) {
            keys.push(ks.to_string());
        }
    }
    keys.sort();
    keys
}
