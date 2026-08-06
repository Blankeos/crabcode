use crate::tools::{
    expand_permission_pattern, PermissionPolicyAction, PermissionRule, PermissionRules,
};
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// Re-export json5 for use in load_config_value
use json5;

pub fn discover_themes(
    xdg_config_home: &Path,
    project_root: &Path,
    cwd: &Path,
    selected_theme_id: Option<&str>,
) -> (Vec<crate::theme::Theme>, usize) {
    let mut theme_by_id: HashMap<String, usize> = HashMap::new();
    let mut themes: Vec<crate::theme::Theme> = Vec::new();

    for theme in crate::theme::Theme::bundled_themes() {
        upsert_theme(&mut themes, &mut theme_by_id, theme);
    }

    let mut layers: Vec<Vec<PathBuf>> = Vec::new();

    let mut built_in = Vec::new();
    if PathBuf::from("src/theme.json").is_file() {
        built_in.push(PathBuf::from("src/theme.json"));
    }
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
            upsert_theme(&mut themes, &mut theme_by_id, theme);
        }
    }

    if themes.is_empty() {
        themes.push(crate::theme::Theme::load_builtin_default());
    }

    let mut selected_idx = 0usize;
    if let Some(id) = selected_theme_id {
        if let Some((idx, _)) = themes.iter().enumerate().find(|(_, t)| t.id == id) {
            selected_idx = idx;
        }
    }

    (themes, selected_idx)
}

fn upsert_theme(
    themes: &mut Vec<crate::theme::Theme>,
    theme_by_id: &mut HashMap<String, usize>,
    theme: crate::theme::Theme,
) {
    if let Some(idx) = theme_by_id.get(&theme.id).copied() {
        themes[idx] = theme;
    } else {
        let idx = themes.len();
        theme_by_id.insert(theme.id.clone(), idx);
        themes.push(theme);
    }
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

fn parse_provider_id_set(
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
    key: &str,
) -> HashSet<String> {
    let Some(value) = value else {
        return HashSet::new();
    };
    let Some(entries) = value.as_array() else {
        diagnostics
            .warnings
            .push(format!("{key} must be an array of provider IDs"));
        return HashSet::new();
    };

    entries
        .iter()
        .filter_map(|entry| {
            let Some(provider_id) = entry.as_str() else {
                diagnostics
                    .warnings
                    .push(format!("{key} entries must be provider IDs"));
                return None;
            };
            let provider_id = provider_id.trim();
            (!provider_id.is_empty()).then(|| provider_id.to_string())
        })
        .collect()
}

fn parse_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
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
    pub command_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalNotificationMode {
    Auto,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalNotificationCondition {
    Unfocused,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosNotificationBackend {
    CrabcodeNotifier,
    Osascript,
}

impl Default for MacosNotificationBackend {
    fn default() -> Self {
        Self::CrabcodeNotifier
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEventConfig {
    pub terminal: TerminalNotificationMode,
    pub sound_enabled: bool,
    pub sound_file: Option<PathBuf>,
    pub desktop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsConfig {
    pub error: NotificationEventConfig,
    pub complete: NotificationEventConfig,
    pub subagent_complete: NotificationEventConfig,
    pub permission: NotificationEventConfig,
    pub question: NotificationEventConfig,
    pub terminal_condition: TerminalNotificationCondition,
    pub macos_backend: MacosNotificationBackend,
}

impl NotificationsConfig {
    pub fn desktop_for_event(&self, event: crate::sound::SoundEvent) -> bool {
        match event {
            crate::sound::SoundEvent::Error => self.error.desktop,
            crate::sound::SoundEvent::Complete => self.complete.desktop,
            crate::sound::SoundEvent::SubagentComplete => self.subagent_complete.desktop,
            crate::sound::SoundEvent::Permission => self.permission.desktop,
            crate::sound::SoundEvent::Question => self.question.desktop,
        }
    }

    pub fn any_desktop_enabled(&self) -> bool {
        self.error.desktop
            || self.complete.desktop
            || self.subagent_complete.desktop
            || self.permission.desktop
            || self.question.desktop
    }
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            error: NotificationEventConfig {
                terminal: TerminalNotificationMode::Disabled,
                sound_enabled: true,
                sound_file: None,
                desktop: false,
            },
            complete: NotificationEventConfig {
                terminal: TerminalNotificationMode::Auto,
                sound_enabled: true,
                sound_file: None,
                desktop: false,
            },
            subagent_complete: NotificationEventConfig {
                terminal: TerminalNotificationMode::Auto,
                sound_enabled: true,
                sound_file: None,
                desktop: false,
            },
            permission: NotificationEventConfig {
                terminal: TerminalNotificationMode::Auto,
                sound_enabled: false,
                sound_file: None,
                desktop: false,
            },
            question: NotificationEventConfig {
                terminal: TerminalNotificationMode::Auto,
                sound_enabled: false,
                sound_file: None,
                desktop: false,
            },
            terminal_condition: TerminalNotificationCondition::Unfocused,
            macos_backend: MacosNotificationBackend::CrabcodeNotifier,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageOpenCommandConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageOpenWith {
    Auto,
    System,
    Editor,
    Command(ImageOpenCommandConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagesConfig {
    pub open_with: ImageOpenWith,
}

impl Default for ImagesConfig {
    fn default() -> Self {
        Self {
            open_with: ImageOpenWith::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebsearchProvider {
    ExaHostedMcp,
    Exa,
    Tavily,
    Perplexity,
    Brave,
    OllamaCloud,
    SerpApi,
    Keiro,
}

impl WebsearchProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExaHostedMcp => "exa-hosted-mcp",
            Self::Exa => "exa",
            Self::Tavily => "tavily",
            Self::Perplexity => "perplexity",
            Self::Brave => "brave",
            Self::OllamaCloud => "ollama-cloud",
            Self::SerpApi => "serpapi",
            Self::Keiro => "keiro",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebsearchConfig {
    pub enabled: Option<bool>,
    pub provider: WebsearchProvider,
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
}

impl Default for WebsearchConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            provider: WebsearchProvider::ExaHostedMcp,
            endpoint: None,
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpLocalConfig {
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub environment: HashMap<String, String>,
    pub enabled: bool,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRemoteConfig {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub enabled: bool,
    pub timeout_ms: Option<u64>,
    pub oauth_enabled: bool,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub oauth_scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    Local(McpLocalConfig),
    Remote(McpRemoteConfig),
}

impl McpServerConfig {
    pub fn enabled(&self) -> bool {
        match self {
            Self::Local(config) => config.enabled,
            Self::Remote(config) => config.enabled,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        match self {
            Self::Local(config) => config.enabled = enabled,
            Self::Remote(config) => config.enabled = enabled,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Remote(_) => "remote",
        }
    }
}

pub type McpConfig = BTreeMap<String, McpServerConfig>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CompactionConfig {
    #[default]
    Enabled,
    Disabled,
    Settings {
        auto: bool,
        prune: bool,
    },
}

impl CompactionConfig {
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WatcherConfig {
    #[default]
    Enabled,
    Disabled,
    Settings {
        ignore: Vec<String>,
    },
}

impl WatcherConfig {
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub fn ignored_paths(&self) -> &[String] {
        match self {
            Self::Settings { ignore } => ignore,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FormatterConfig {
    #[default]
    Disabled,
    Command(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTimeout {
    Millis(u64),
    Disabled,
}

#[derive(Debug, Clone, Default)]
pub struct MergedConfig {
    pub theme: Option<String>,
    pub model: Option<String>,
    pub small_model: Option<String>,
    pub default_agent: Option<String>,
    pub agent_registry: crate::agent::definition::AgentRegistry,
    pub commands: Vec<crate::command::custom::CustomCommand>,
    pub agent_tool_policies: HashMap<String, Vec<String>>,
    pub permission_rules: PermissionRules,
    pub agent_permission_rules: HashMap<String, PermissionRules>,
    pub agent_steps: HashMap<String, usize>,
    pub provider_timeouts: HashMap<String, ProviderTimeout>,
    pub disabled_providers: HashSet<String>,
    pub enabled_providers: Option<HashSet<String>>,
    pub custom_providers: HashMap<String, CustomProviderConfig>,
    pub notifications: NotificationsConfig,
    pub images: ImagesConfig,
    pub websearch: WebsearchConfig,
    pub mcp: McpConfig,
    pub instructions: Vec<String>,
    pub tools: HashMap<String, bool>,
    pub compaction: CompactionConfig,
    pub watcher: WatcherConfig,
    pub formatter: HashMap<String, FormatterConfig>,
    pub disabled_providers: HashSet<String>,
    pub enabled_providers: Option<HashSet<String>>,
}

impl MergedConfig {
    pub fn provider_is_enabled(&self, provider_id: &str) -> bool {
        !self.disabled_providers.contains(provider_id)
            && self
                .enabled_providers
                .as_ref()
                .is_none_or(|enabled| enabled.contains(provider_id))
    }
}

#[derive(Debug, Clone)]
pub struct CustomModelConfig {
    pub name: Option<String>,
    pub context_window: Option<u32>,
    pub max_tokens: Option<u32>,
    pub attachment: Option<bool>,
    pub reasoning: Option<bool>,
    pub temperature: Option<bool>,
    pub tool_call: Option<bool>,
    pub modalities: Option<CustomModelModalities>,
    pub launch: bool,
}

#[derive(Debug, Clone)]
pub struct CustomModelModalities {
    pub input: Vec<String>,
    pub output: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CustomProviderConfig {
    pub name: Option<String>,
    pub npm: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub models: HashMap<String, CustomModelConfig>,
}

impl CustomProviderConfig {
    pub fn resolved_api_key(&self) -> Option<String> {
        resolve_api_key_value(self.api_key.as_deref(), |variable| {
            std::env::var(variable).ok()
        })
    }
}

fn resolve_api_key_value(
    value: Option<&str>,
    get_env: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(variable) = value
        .strip_prefix("{env:")
        .and_then(|value| value.strip_suffix('}'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return get_env(variable)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    Some(value.to_string())
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
        let cwd = crate::utils::cwd::current_dir()?;
        Self::load_for(&cwd)
    }

    pub fn load_for(cwd: &Path) -> Result<LoadedConfig> {
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
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

        for source in &sources {
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

        let commands = load_custom_commands(
            &sources,
            &xdg_config_home,
            &project_root,
            &mut inventory,
            &mut diagnostics,
        );
        let mut merged_config = parse_merged_config(&merged, &mut diagnostics);
        merged_config.instructions =
            load_instruction_files(&merged_config.instructions, &project_root, &mut diagnostics);
        let mut agent_definitions = crate::agent::definition::load_markdown_agent_definitions(
            &inventory.opencode_agents,
            &mut diagnostics.warnings,
        );
        let mut ignored_agent_warnings = Vec::new();
        agent_definitions.extend(
            crate::agent::definition::parse_agent_definitions_from_config(
                merged.get("agent"),
                &mut ignored_agent_warnings,
            ),
        );
        merged_config.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(
            merged_config.default_agent.as_deref(),
            agent_definitions,
        );
        merged_config.sync_agent_derived_fields();
        merged_config.commands = commands;
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
    let global_crabcode = xdg_config_home.join("crabcode");
    let local_opencode = project_root.join(".opencode");
    let local_crabcode = project_root.join(".crabcode");

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
        global_crabcode.join("skills"),
        global_crabcode.join("skill"),
        local_opencode.join("skills"),
        local_opencode.join("skill"),
        local_crabcode.join("skills"),
        local_crabcode.join("skill"),
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

fn load_custom_commands(
    sources: &[SourceFile],
    xdg_config_home: &Path,
    project_root: &Path,
    inventory: &mut ConfigInventory,
    diagnostics: &mut ConfigDiagnostics,
) -> Vec<crate::command::custom::CustomCommand> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut commands = Vec::new();
    let mut command_by_name: HashMap<String, usize> = HashMap::new();

    for layer in command_layers(xdg_config_home, project_root, &home) {
        if let Some(source) = sources.iter().find(|source| source.label == layer.label) {
            merge_config_commands(
                source,
                project_root,
                &mut commands,
                &mut command_by_name,
                diagnostics,
            );
        }

        for dir in layer.dirs {
            let discovered = crate::command::custom::commands_from_directory(
                &dir,
                project_root,
                &mut diagnostics.warnings,
            );
            for command in discovered {
                if let crate::command::custom::CustomCommandSource::File(path) = &command.source {
                    inventory.command_files.push(path.clone());
                }
                upsert_custom_command(&mut commands, &mut command_by_name, command);
            }
        }
    }

    inventory.command_files.sort();
    inventory.command_files.dedup();

    if !commands.is_empty() {
        diagnostics
            .info
            .push(format!("Discovered {} custom commands", commands.len()));
    }

    commands
}

struct CommandLayer {
    label: &'static str,
    dirs: Vec<PathBuf>,
}

fn command_layers(xdg_config_home: &Path, project_root: &Path, home: &Path) -> Vec<CommandLayer> {
    vec![
        CommandLayer {
            label: "OpenCode global",
            dirs: vec![xdg_config_home.join("opencode"), home.join(".opencode")],
        },
        CommandLayer {
            label: "Crabcode global",
            dirs: vec![xdg_config_home.join("crabcode"), home.join(".crabcode")],
        },
        CommandLayer {
            label: "OpenCode local",
            dirs: vec![project_root.join(".opencode")],
        },
        CommandLayer {
            label: "Crabcode local",
            dirs: vec![project_root.join(".crabcode")],
        },
    ]
}

fn merge_config_commands(
    source: &SourceFile,
    project_root: &Path,
    commands: &mut Vec<crate::command::custom::CustomCommand>,
    command_by_name: &mut HashMap<String, usize>,
    diagnostics: &mut ConfigDiagnostics,
) {
    let parsed = match load_config_value(&source.path) {
        Ok(v) => v,
        Err(_) => return,
    };
    let filtered = filter_top_level(parsed, source.kind);
    let Some(mut command_value) = filtered.get("command").cloned() else {
        return;
    };

    let base_dir = source
        .path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| project_root.to_path_buf());
    let mut provenance = HashMap::new();
    provenance.insert("".to_string(), base_dir);
    substitute_placeholders(&mut command_value, &provenance, diagnostics);

    let parsed_commands = crate::command::custom::commands_from_config_value(
        &command_value,
        &source.path,
        project_root,
        &mut diagnostics.warnings,
    );
    for command in parsed_commands {
        upsert_custom_command(commands, command_by_name, command);
    }
}

fn upsert_custom_command(
    commands: &mut Vec<crate::command::custom::CustomCommand>,
    command_by_name: &mut HashMap<String, usize>,
    command: crate::command::custom::CustomCommand,
) {
    if let Some(idx) = command_by_name.get(&command.name).copied() {
        commands[idx] = command;
    } else {
        let idx = commands.len();
        command_by_name.insert(command.name.clone(), idx);
        commands.push(command);
    }
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

    // Use json5 for lenient parsing (handles trailing commas, comments, etc.)
    let v: Value = json5::from_str(&content)
        .with_context(|| format!("Invalid JSON/JSONC in {}", path.display()))?;
    Ok(v)
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
        "small_model",
        "smallModel",
        "provider",
        "command",
        "permission",
        "compaction",
        "watcher",
        "default_agent",
        "formatter",
        "disabled_providers",
        "disabledProviders",
        "enabled_providers",
        "enabledProviders",
    ]
    .into_iter()
    .collect()
}

fn crabcode_allowed_keys() -> BTreeSet<&'static str> {
    let mut out = opencode_allowed_keys();
    out.insert("theme");
    out.insert("notifications");
    out.insert("images");
    out.insert("websearch");
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

fn load_instruction_files(
    paths: &[String],
    project_root: &Path,
    diagnostics: &mut ConfigDiagnostics,
) -> Vec<String> {
    paths
        .iter()
        .filter_map(|configured_path| {
            let path = expand_path(configured_path, project_root);
            match fs::read_to_string(&path) {
                Ok(contents) => Some(contents),
                Err(error) => {
                    diagnostics.warnings.push(format!(
                        "Failed to read instruction file {}: {error}",
                        path.display()
                    ));
                    None
                }
            }
        })
        .collect()
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

    if let Some(Value::String(model)) = obj.get("small_model").or_else(|| obj.get("smallModel")) {
        if !model.trim().is_empty() {
            out.small_model = Some(model.trim().to_string());
        }
    }

    if let Some(Value::String(default_agent)) = obj.get("default_agent") {
        if !default_agent.trim().is_empty() {
            out.default_agent = Some(default_agent.trim().to_string());
        }
    }

    out.permission_rules = parse_permission_rules(obj.get("permission"), diagnostics, "permission");
    let json_agents = crate::agent::definition::parse_agent_definitions_from_config(
        obj.get("agent"),
        &mut diagnostics.warnings,
    );
    out.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(
        out.default_agent.as_deref(),
        json_agents,
    );
    out.sync_agent_derived_fields();
    out.provider_timeouts = parse_provider_timeouts(obj.get("provider"), diagnostics);
    let enabled_providers = obj
        .get("enabled_providers")
        .or_else(|| obj.get("enabledProviders"));
    out.enabled_providers = enabled_providers
        .map(|value| parse_provider_id_set(Some(value), diagnostics, "enabled_providers"));
    out.disabled_providers = parse_provider_id_set(
        obj.get("disabled_providers")
            .or_else(|| obj.get("disabledProviders")),
        diagnostics,
        "disabled_providers",
    );
    out.custom_providers = parse_custom_providers(obj.get("provider"), diagnostics);

    let mut notifications = NotificationsConfig::default();
    apply_notifications(obj.get("notifications"), &mut notifications, diagnostics);
    out.notifications = notifications;
    out.images = parse_images(obj.get("images"), diagnostics);
    out.websearch = parse_websearch(obj.get("websearch"), diagnostics);
    out.mcp = parse_mcp(obj.get("mcp"), diagnostics);
    out.instructions = obj
        .get("instructions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|instruction| !instruction.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    out.tools = obj
        .get("tools")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(tool, enabled)| enabled.as_bool().map(|enabled| (tool.clone(), enabled)))
        .collect();
    out.compaction = match obj.get("compaction") {
        Some(Value::Bool(false)) => CompactionConfig::Disabled,
        Some(Value::Object(settings)) => CompactionConfig::Settings {
            auto: settings
                .get("auto")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            prune: settings
                .get("prune")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        _ => CompactionConfig::Enabled,
    };
    out.watcher = match obj.get("watcher") {
        Some(Value::Bool(false)) => WatcherConfig::Disabled,
        Some(Value::Object(settings)) => WatcherConfig::Settings {
            ignore: settings
                .get("ignore")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        },
        _ => WatcherConfig::Enabled,
    };
    out.formatter = obj
        .get("formatter")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(extension, formatter)| match formatter {
            Value::String(command) if !command.trim().is_empty() => Some((
                extension.trim_start_matches('.').to_string(),
                FormatterConfig::Command(command.trim().to_string()),
            )),
            Value::Bool(false) => Some((
                extension.trim_start_matches('.').to_string(),
                FormatterConfig::Disabled,
            )),
            _ => None,
        })
        .collect();
    out.disabled_providers = parse_string_array(obj.get("disabled_providers"))
        .into_iter()
        .collect();
    out.enabled_providers = obj
        .get("enabled_providers")
        .map(|value| parse_string_array(Some(value)).into_iter().collect());

    out
}

fn parse_mcp(value: Option<&Value>, diagnostics: &mut ConfigDiagnostics) -> McpConfig {
    let mut out = McpConfig::new();
    let Some(value) = value else {
        return out;
    };
    let Some(map) = value.as_object() else {
        diagnostics
            .warnings
            .push("mcp must be an object".to_string());
        return out;
    };

    for (name, entry) in map {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            diagnostics
                .warnings
                .push("mcp server names must not be empty".to_string());
            continue;
        }
        match parse_mcp_server(trimmed_name, entry, diagnostics) {
            Some(server) => {
                out.insert(trimmed_name.to_string(), server);
            }
            None => continue,
        }
    }
    out
}

fn parse_mcp_server(
    name: &str,
    entry: &Value,
    diagnostics: &mut ConfigDiagnostics,
) -> Option<McpServerConfig> {
    let Some(map) = entry.as_object() else {
        diagnostics
            .warnings
            .push(format!("mcp.{name} must be an object"));
        return None;
    };
    let enabled = map.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let timeout_ms = parse_optional_u64(
        map.get("timeout"),
        &format!("mcp.{name}.timeout"),
        diagnostics,
    );
    let kind = map.get("type").and_then(Value::as_str).unwrap_or_else(|| {
        if map.contains_key("url") {
            "remote"
        } else {
            "local"
        }
    });

    match kind {
        "local" => {
            let command = match map.get("command") {
                Some(Value::Array(values)) => values
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>(),
                Some(Value::String(raw)) => shlex::split(raw).unwrap_or_else(|| vec![raw.clone()]),
                _ => Vec::new(),
            };
            if command.is_empty() {
                diagnostics.warnings.push(format!(
                    "mcp.{name}.command must be a non-empty string array"
                ));
                return None;
            }
            Some(McpServerConfig::Local(McpLocalConfig {
                command,
                cwd: map
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned),
                environment: parse_string_map(
                    map.get("environment").or_else(|| map.get("env")),
                    &format!("mcp.{name}.environment"),
                    diagnostics,
                ),
                enabled,
                timeout_ms,
            }))
        }
        "remote" => {
            let Some(url) = map
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                diagnostics
                    .warnings
                    .push(format!("mcp.{name}.url must be a non-empty string"));
                return None;
            };
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                diagnostics
                    .warnings
                    .push(format!("mcp.{name}.url must be an http(s) URL"));
                return None;
            }
            let (oauth_enabled, oauth_client_id, oauth_client_secret, oauth_scope) =
                parse_mcp_oauth(name, map.get("oauth"), diagnostics);
            Some(McpServerConfig::Remote(McpRemoteConfig {
                url: url.to_string(),
                headers: parse_string_map(
                    map.get("headers"),
                    &format!("mcp.{name}.headers"),
                    diagnostics,
                ),
                enabled,
                timeout_ms,
                oauth_enabled,
                oauth_client_id,
                oauth_client_secret,
                oauth_scope,
            }))
        }
        other => {
            diagnostics.warnings.push(format!(
                "mcp.{name}.type must be local or remote; got {other}"
            ));
            None
        }
    }
}

fn parse_mcp_oauth(
    name: &str,
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
) -> (bool, Option<String>, Option<String>, Option<String>) {
    let Some(value) = value else {
        return (true, None, None, None);
    };
    if value.as_bool() == Some(false) {
        return (false, None, None, None);
    }
    let Some(map) = value.as_object() else {
        diagnostics
            .warnings
            .push(format!("mcp.{name}.oauth must be an object or false"));
        return (true, None, None, None);
    };
    (
        true,
        optional_string(map.get("clientId")),
        optional_string(map.get("clientSecret")),
        optional_string(map.get("scope")),
    )
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_string_map(
    value: Option<&Value>,
    label: &str,
    diagnostics: &mut ConfigDiagnostics,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(value) = value else {
        return out;
    };
    let Some(map) = value.as_object() else {
        diagnostics
            .warnings
            .push(format!("{label} must be an object"));
        return out;
    };
    for (key, value) in map {
        if let Some(raw) = value.as_str() {
            out.insert(key.clone(), raw.to_string());
        } else {
            diagnostics
                .warnings
                .push(format!("{label}.{key} must be a string"));
        }
    }
    out
}

fn parse_optional_u64(
    value: Option<&Value>,
    label: &str,
    diagnostics: &mut ConfigDiagnostics,
) -> Option<u64> {
    match value {
        None => None,
        Some(Value::Number(n)) => n.as_u64(),
        Some(_) => {
            diagnostics
                .warnings
                .push(format!("{label} must be a number"));
            None
        }
    }
}

fn parse_websearch(value: Option<&Value>, diagnostics: &mut ConfigDiagnostics) -> WebsearchConfig {
    let mut websearch = WebsearchConfig::default();
    let Some(value) = value else {
        return websearch;
    };

    match value {
        Value::Bool(enabled) => {
            websearch.enabled = Some(*enabled);
            return websearch;
        }
        Value::Object(map) => {
            if let Some(enabled) = map.get("enabled") {
                if let Some(v) = enabled.as_bool() {
                    websearch.enabled = Some(v);
                } else {
                    diagnostics
                        .warnings
                        .push("websearch.enabled must be a boolean".to_string());
                }
            }

            if let Some(provider) = map.get("provider") {
                if let Some(raw) = provider.as_str() {
                    match parse_websearch_provider(raw) {
                        Some(provider) => websearch.provider = provider,
                        _ => diagnostics.warnings.push(format!(
                            "websearch.provider must be one of: exa-hosted-mcp, exa, tavily, perplexity, brave, ollama-cloud, serpapi, keiro; got {}",
                            raw
                        )),
                    }
                } else {
                    diagnostics
                        .warnings
                        .push("websearch.provider must be a string".to_string());
                }
            }

            if let Some(endpoint) = map.get("endpoint") {
                if let Some(raw) = endpoint.as_str() {
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        websearch.endpoint = None;
                    } else if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
                        websearch.endpoint = Some(trimmed.to_string());
                    } else {
                        diagnostics
                            .warnings
                            .push("websearch.endpoint must be an http(s) URL".to_string());
                    }
                } else {
                    diagnostics
                        .warnings
                        .push("websearch.endpoint must be a string".to_string());
                }
            }

            if let Some(api_key) = map.get("apiKey") {
                if let Some(raw) = api_key.as_str() {
                    let trimmed = raw.trim();
                    websearch.api_key = if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    };
                } else if api_key.is_null() || api_key.as_bool() == Some(false) {
                    websearch.api_key = None;
                } else {
                    diagnostics
                        .warnings
                        .push("websearch.apiKey must be a string, false, or null".to_string());
                }
            }
        }
        _ => diagnostics
            .warnings
            .push("websearch must be a boolean or object".to_string()),
    }

    websearch
}

fn parse_websearch_provider(raw: &str) -> Option<WebsearchProvider> {
    let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "exa-hosted-mcp" => Some(WebsearchProvider::ExaHostedMcp),
        "exa" => Some(WebsearchProvider::Exa),
        "tavily" => Some(WebsearchProvider::Tavily),
        "perplexity" => Some(WebsearchProvider::Perplexity),
        "brave" => Some(WebsearchProvider::Brave),
        "ollama-cloud" => Some(WebsearchProvider::OllamaCloud),
        "serpapi" => Some(WebsearchProvider::SerpApi),
        "keiro" => Some(WebsearchProvider::Keiro),
        _ => None,
    }
}

impl MergedConfig {
    fn sync_agent_derived_fields(&mut self) {
        self.agent_tool_policies = self.agent_registry.tool_policy_map();
        self.agent_permission_rules = self.agent_registry.permission_rules_map();
        self.agent_steps = self.agent_registry.max_steps_map();
    }
}

fn parse_agent_tool_policies(
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let Some(Value::Object(agents)) = value else {
        return out;
    };

    for (name, val) in agents {
        let Some(agent_obj) = val.as_object() else {
            continue;
        };

        let Some(tools_val) = agent_obj.get("tools") else {
            continue;
        };

        let mut tools = Vec::new();
        match tools_val {
            Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            tools.push(trimmed.to_ascii_lowercase());
                        }
                    }
                }
            }
            Value::String(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    tools.push(trimmed.to_ascii_lowercase());
                }
            }
            _ => {
                diagnostics.warnings.push(format!(
                    "agent.{}.tools must be a string or array of strings",
                    name
                ));
            }
        }

        if !tools.is_empty() {
            out.insert(name.trim().to_ascii_lowercase(), tools);
        }
    }

    out
}

fn parse_agent_permission_rules(
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
) -> HashMap<String, PermissionRules> {
    let mut out = HashMap::new();
    let Some(Value::Object(agents)) = value else {
        return out;
    };

    for (name, val) in agents {
        let Some(agent_obj) = val.as_object() else {
            continue;
        };

        let Some(permission) = agent_obj.get("permission") else {
            continue;
        };

        let key = name.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }

        let rules = parse_permission_rules(
            Some(permission),
            diagnostics,
            &format!("agent.{}.permission", name),
        );
        if !rules.is_empty() {
            out.insert(key, rules);
        }
    }

    out
}

fn parse_permission_rules(
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
    context: &str,
) -> PermissionRules {
    let mut out = Vec::new();
    let Some(value) = value else {
        return out;
    };

    if value.is_null() {
        return out;
    }

    if let Some(action) = value.as_str() {
        match PermissionPolicyAction::parse(action) {
            Some(action) => out.push(PermissionRule {
                permission: "*".to_string(),
                pattern: "*".to_string(),
                action,
            }),
            None => diagnostics.warnings.push(format!(
                "{} must be one of allow, ask, or deny; got '{}'",
                context, action
            )),
        }
        return out;
    }

    let Some(map) = value.as_object() else {
        diagnostics
            .warnings
            .push(format!("{} must be a string or object", context));
        return out;
    };

    for (permission, value) in map {
        let permission = permission.trim().to_ascii_lowercase();
        if permission.is_empty() {
            diagnostics
                .warnings
                .push(format!("{} contains an empty permission key", context));
            continue;
        }

        if let Some(action) = value.as_str() {
            match PermissionPolicyAction::parse(action) {
                Some(action) => out.push(PermissionRule {
                    permission,
                    pattern: "*".to_string(),
                    action,
                }),
                None => diagnostics.warnings.push(format!(
                    "{}.{} must be one of allow, ask, or deny; got '{}'",
                    context, permission, action
                )),
            }
            continue;
        }

        let Some(patterns) = value.as_object() else {
            diagnostics.warnings.push(format!(
                "{}.{} must be one of allow, ask, deny, or an object of pattern rules",
                context, permission
            ));
            continue;
        };

        for (pattern, action_value) in patterns {
            let Some(action_text) = action_value.as_str() else {
                diagnostics.warnings.push(format!(
                    "{}.{}.{} must be one of allow, ask, or deny",
                    context, permission, pattern
                ));
                continue;
            };

            let Some(action) = PermissionPolicyAction::parse(action_text) else {
                diagnostics.warnings.push(format!(
                    "{}.{}.{} must be one of allow, ask, or deny; got '{}'",
                    context, permission, pattern, action_text
                ));
                continue;
            };

            let pattern = expand_permission_pattern(pattern);
            if pattern.trim().is_empty() {
                diagnostics.warnings.push(format!(
                    "{}.{} contains an empty permission pattern",
                    context, permission
                ));
                continue;
            }

            out.push(PermissionRule {
                permission: permission.clone(),
                pattern,
                action,
            });
        }
    }

    out
}

fn parse_agent_steps(
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    let Some(Value::Object(agents)) = value else {
        return out;
    };

    for (name, val) in agents {
        let Some(agent_obj) = val.as_object() else {
            continue;
        };

        let Some(raw) = agent_obj.get("steps").or_else(|| agent_obj.get("maxSteps")) else {
            continue;
        };

        let Some(num) = raw.as_u64() else {
            diagnostics
                .warnings
                .push(format!("agent.{}.steps must be a positive integer", name));
            continue;
        };

        if num == 0 {
            diagnostics
                .warnings
                .push(format!("agent.{}.steps must be greater than 0", name));
            continue;
        }

        if num > usize::MAX as u64 {
            diagnostics.warnings.push(format!(
                "agent.{}.steps is too large for this platform; ignoring value {}",
                name, num
            ));
            continue;
        }

        out.insert(name.trim().to_ascii_lowercase(), num as usize);
    }

    out
}

fn parse_provider_timeouts(
    value: Option<&Value>,
    diagnostics: &mut ConfigDiagnostics,
) -> HashMap<String, ProviderTimeout> {
    let mut out = HashMap::new();
    let Some(Value::Object(providers)) = value else {
        return out;
    };

    for (provider_id, provider_val) in providers {
        let Some(provider_obj) = provider_val.as_object() else {
            continue;
        };

        let Some(options_val) = provider_obj.get("options") else {
            continue;
        };

        let Some(options_obj) = options_val.as_object() else {
            diagnostics.warnings.push(format!(
                "provider.{}.options must be an object",
                provider_id
            ));
            continue;
        };

        let Some(timeout_val) = options_obj.get("timeout") else {
            continue;
        };

        let timeout = match timeout_val {
            Value::Bool(false) => ProviderTimeout::Disabled,
            Value::Number(n) => {
                let Some(ms) = n.as_u64() else {
                    diagnostics.warnings.push(format!(
                        "provider.{}.options.timeout must be a positive integer in milliseconds or false",
                        provider_id
                    ));
                    continue;
                };

                if ms == 0 {
                    diagnostics.warnings.push(format!(
                        "provider.{}.options.timeout must be greater than 0 when set",
                        provider_id
                    ));
                    continue;
                }

                ProviderTimeout::Millis(ms)
            }
            _ => {
                diagnostics.warnings.push(format!(
                    "provider.{}.options.timeout must be a positive integer in milliseconds or false",
                    provider_id
                ));
                continue;
            }
        };

        out.insert(provider_id.trim().to_ascii_lowercase(), timeout);
    }

    out
}
fn parse_custom_providers(
    value: Option<&Value>,
    _diagnostics: &mut ConfigDiagnostics,
) -> HashMap<String, CustomProviderConfig> {
    let mut out = HashMap::new();
    let Some(Value::Object(providers)) = value else {
        return out;
    };

    for (provider_id, provider_val) in providers {
        let Some(provider_obj) = provider_val.as_object() else {
            continue;
        };

        let name = provider_obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);

        let npm = provider_obj
            .get("npm")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let options = provider_obj.get("options").and_then(Value::as_object);
        let base_url = options
            .and_then(|options| options.get("baseURL"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let api_key = options
            .and_then(|options| options.get("apiKey"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut models = HashMap::new();
        if let Some(models_val) = provider_obj.get("models") {
            if let Value::Object(model_map) = models_val {
                for (model_id, model_val) in model_map {
                    let Some(model_obj) = model_val.as_object() else {
                        continue;
                    };

                    let model_name = model_obj
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);

                    let limit = model_obj.get("limit").and_then(Value::as_object);

                    let context_window = model_obj
                        .get("contextWindow")
                        .and_then(Value::as_u64)
                        .or_else(|| {
                            limit
                                .and_then(|limit| limit.get("context"))
                                .and_then(Value::as_u64)
                        })
                        .and_then(|value| u32::try_from(value).ok());

                    let max_tokens = model_obj
                        .get("maxTokens")
                        .and_then(Value::as_u64)
                        .or_else(|| {
                            limit
                                .and_then(|limit| limit.get("output"))
                                .and_then(Value::as_u64)
                        })
                        .and_then(|value| u32::try_from(value).ok());

                    let modalities =
                        model_obj
                            .get("modalities")
                            .and_then(Value::as_object)
                            .map(|modalities| CustomModelModalities {
                                input: parse_string_array(modalities.get("input")),
                                output: parse_string_array(modalities.get("output")),
                            });

                    let launch = model_obj
                        .get("_launch")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);

                    models.insert(
                        model_id.clone(),
                        CustomModelConfig {
                            name: model_name,
                            context_window,
                            max_tokens,
                            attachment: model_obj.get("attachment").and_then(Value::as_bool),
                            reasoning: model_obj.get("reasoning").and_then(Value::as_bool),
                            temperature: model_obj.get("temperature").and_then(Value::as_bool),
                            tool_call: model_obj.get("tool_call").and_then(Value::as_bool),
                            modalities,
                            launch,
                        },
                    );
                }
            }
        }

        if name.is_none()
            && npm.is_none()
            && base_url.is_none()
            && api_key.is_none()
            && models.is_empty()
        {
            continue;
        }

        out.insert(
            provider_id.trim().to_ascii_lowercase(),
            CustomProviderConfig {
                name,
                npm,
                base_url,
                api_key,
                models,
            },
        );
    }

    out
}

fn parse_images(value: Option<&Value>, diagnostics: &mut ConfigDiagnostics) -> ImagesConfig {
    let mut images = ImagesConfig::default();
    let Some(value) = value else {
        return images;
    };
    if value.is_null() {
        return images;
    }
    let Value::Object(map) = value else {
        diagnostics
            .warnings
            .push("images must be an object".to_string());
        return images;
    };

    let Some(open_with) = map.get("openWith").or_else(|| map.get("open_with")) else {
        return images;
    };

    images.open_with = parse_image_open_with(open_with, "images.openWith", diagnostics);
    images
}

fn parse_image_open_with(
    value: &Value,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) -> ImageOpenWith {
    match value {
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "auto" => ImageOpenWith::Auto,
            "system" => ImageOpenWith::System,
            "editor" => ImageOpenWith::Editor,
            _ => {
                diagnostics.warnings.push(format!(
                    "{}: expected auto, system, editor, or a command object",
                    key
                ));
                ImageOpenWith::Auto
            }
        },
        Value::Object(map) => {
            let command = match map.get("command").and_then(Value::as_str) {
                Some(command) if !command.trim().is_empty() => command.trim().to_string(),
                _ => {
                    diagnostics
                        .warnings
                        .push(format!("{}.command must be a non-empty string", key));
                    return ImageOpenWith::Auto;
                }
            };

            let args = match map.get("args") {
                Some(Value::Array(raw_args)) => {
                    let mut args = Vec::new();
                    for arg in raw_args {
                        if let Some(arg) = arg.as_str() {
                            args.push(arg.to_string());
                        } else {
                            diagnostics
                                .warnings
                                .push(format!("{}.args must contain only strings", key));
                            return ImageOpenWith::Auto;
                        }
                    }
                    args
                }
                Some(_) => {
                    diagnostics
                        .warnings
                        .push(format!("{}.args must be an array of strings", key));
                    return ImageOpenWith::Auto;
                }
                None => vec!["{path}".to_string()],
            };

            ImageOpenWith::Command(ImageOpenCommandConfig { command, args })
        }
        _ => {
            diagnostics.warnings.push(format!(
                "{}: expected auto, system, editor, or a command object",
                key
            ));
            ImageOpenWith::Auto
        }
    }
}

fn apply_notifications(
    value: Option<&Value>,
    notifications: &mut NotificationsConfig,
    diagnostics: &mut ConfigDiagnostics,
) {
    let Some(value) = value else {
        return;
    };

    if value.is_null() {
        return;
    }

    let Value::Object(map) = value else {
        diagnostics
            .warnings
            .push("notifications must be an object".to_string());
        return;
    };

    apply_legacy_terminal_notifications(map.get("terminal"), notifications, diagnostics);

    if let Some(condition) = map
        .get("terminalCondition")
        .or_else(|| map.get("terminal_condition"))
    {
        notifications.terminal_condition = parse_terminal_notification_condition(
            condition,
            "notifications.terminalCondition",
            diagnostics,
        );
    }

    if let Some(backend) = map.get("macosBackend").or_else(|| map.get("macos_backend")) {
        notifications.macos_backend =
            parse_macos_notification_backend(backend, "notifications.macosBackend", diagnostics);
    }

    apply_notification_event(
        &mut notifications.error,
        map.get("error"),
        "notifications.error",
        diagnostics,
    );
    apply_notification_event(
        &mut notifications.complete,
        map.get("complete"),
        "notifications.complete",
        diagnostics,
    );
    notifications.subagent_complete = notifications.complete.clone();
    notifications.subagent_complete.sound_file = None;
    apply_notification_event(
        &mut notifications.subagent_complete,
        map.get("subagentComplete")
            .or_else(|| map.get("subagent_complete")),
        "notifications.subagentComplete",
        diagnostics,
    );
    apply_notification_event(
        &mut notifications.permission,
        map.get("permission"),
        "notifications.permission",
        diagnostics,
    );
    apply_notification_event(
        &mut notifications.question,
        map.get("question"),
        "notifications.question",
        diagnostics,
    );
}

fn apply_legacy_terminal_notifications(
    value: Option<&Value>,
    notifications: &mut NotificationsConfig,
    diagnostics: &mut ConfigDiagnostics,
) {
    let Some(value) = value else {
        return;
    };

    let Value::Object(terminal_map) = value else {
        diagnostics
            .warnings
            .push("notifications.terminal must be an object".to_string());
        return;
    };

    diagnostics.warnings.push(
        "notifications.terminal is deprecated; use notifications.<event>.terminal and notifications.terminalCondition instead"
            .to_string(),
    );

    if let Some(complete) = terminal_map.get("complete") {
        notifications.complete.terminal = parse_terminal_notification_mode(
            complete,
            "notifications.terminal.complete",
            diagnostics,
        );
    }

    if let Some(permission) = terminal_map.get("permission") {
        notifications.permission.terminal = parse_terminal_notification_mode(
            permission,
            "notifications.terminal.permission",
            diagnostics,
        );
    }

    if let Some(question) = terminal_map.get("question") {
        notifications.question.terminal = parse_terminal_notification_mode(
            question,
            "notifications.terminal.question",
            diagnostics,
        );
    }

    if let Some(condition) = terminal_map.get("condition") {
        notifications.terminal_condition = parse_terminal_notification_condition(
            condition,
            "notifications.terminal.condition",
            diagnostics,
        );
    }
}

fn apply_notification_event(
    target: &mut NotificationEventConfig,
    value: Option<&Value>,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) {
    let Some(value) = value else {
        return;
    };

    if value.is_null() {
        return;
    }

    let Value::Object(map) = value else {
        diagnostics
            .warnings
            .push(format!("{} must be an object", key));
        return;
    };

    if let Some(terminal) = map.get("terminal") {
        target.terminal =
            parse_terminal_notification_mode(terminal, &format!("{}.terminal", key), diagnostics);
    }

    if let Some(desktop) = map.get("desktop") {
        if let Some(desktop) = desktop.as_bool() {
            target.desktop = desktop;
        } else if !desktop.is_null() {
            diagnostics
                .warnings
                .push(format!("{}.desktop must be a boolean", key));
        }
    }

    if let Some(sound_enabled) = map.get("soundEnabled").or_else(|| map.get("sound_enabled")) {
        if let Some(sound_enabled) = sound_enabled.as_bool() {
            target.sound_enabled = sound_enabled;
        } else if !sound_enabled.is_null() {
            diagnostics
                .warnings
                .push(format!("{}.soundEnabled must be a boolean", key));
        }
    }

    if let Some(sound_file) = map.get("soundFile").or_else(|| map.get("sound_file")) {
        match sound_file {
            Value::String(file) => {
                apply_sound_file(target, file, &format!("{}.soundFile", key), diagnostics);
            }
            Value::Null => {
                target.sound_file = None;
            }
            _ => {
                diagnostics
                    .warnings
                    .push(format!("{}.soundFile must be a string or null", key));
            }
        }
    }
}

fn apply_sound_file(
    target: &mut NotificationEventConfig,
    file: &str,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) {
    if file.trim().is_empty() {
        target.sound_file = None;
        return;
    }

    let p = PathBuf::from(file);
    if p.is_absolute() {
        target.sound_file = Some(p);
    } else {
        diagnostics.warnings.push(format!(
            "{}: sound file must be an absolute path; treating as disabled",
            key
        ));
        target.sound_file = None;
        target.sound_enabled = false;
    }
}

fn parse_terminal_notification_mode(
    value: &Value,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) -> TerminalNotificationMode {
    match value {
        Value::Bool(true) => TerminalNotificationMode::Enabled,
        Value::Bool(false) => TerminalNotificationMode::Disabled,
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "auto" => TerminalNotificationMode::Auto,
            "enabled" | "on" | "true" => TerminalNotificationMode::Enabled,
            "disabled" | "off" | "false" => TerminalNotificationMode::Disabled,
            _ => {
                diagnostics.warnings.push(format!(
                    "{}: expected auto, enabled, disabled, true, or false",
                    key
                ));
                TerminalNotificationMode::Auto
            }
        },
        _ => {
            diagnostics
                .warnings
                .push(format!("{}: expected string or boolean", key));
            TerminalNotificationMode::Auto
        }
    }
}

fn parse_macos_notification_backend(
    value: &Value,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) -> MacosNotificationBackend {
    let Some(value) = value.as_str() else {
        if !value.is_null() {
            diagnostics
                .warnings
                .push(format!("{} must be \"crabcode\" or \"osascript\"", key));
        }
        return MacosNotificationBackend::CrabcodeNotifier;
    };

    match value.trim().to_ascii_lowercase().as_str() {
        "crabcode" | "crabcode-notifier" | "crabcode_notifier" | "notifier" | "native" => {
            MacosNotificationBackend::CrabcodeNotifier
        }
        "osascript" | "script" => MacosNotificationBackend::Osascript,
        _ => {
            diagnostics
                .warnings
                .push(format!("{} must be \"crabcode\" or \"osascript\"", key));
            MacosNotificationBackend::CrabcodeNotifier
        }
    }
}

fn parse_terminal_notification_condition(
    value: &Value,
    key: &str,
    diagnostics: &mut ConfigDiagnostics,
) -> TerminalNotificationCondition {
    let Some(s) = value.as_str() else {
        diagnostics
            .warnings
            .push(format!("{}: expected unfocused or always", key));
        return TerminalNotificationCondition::Unfocused;
    };

    match s.trim().to_ascii_lowercase().as_str() {
        "unfocused" => TerminalNotificationCondition::Unfocused,
        "always" => TerminalNotificationCondition::Always,
        _ => {
            diagnostics
                .warnings
                .push(format!("{}: expected unfocused or always", key));
            TerminalNotificationCondition::Unfocused
        }
    }
}

fn collect_unimplemented_keys(merged: &Value) -> Vec<String> {
    let Some(obj) = merged.as_object() else {
        return Vec::new();
    };

    let supported: BTreeSet<&'static str> = crabcode_allowed_keys();
    let implemented: BTreeSet<&'static str> = [
        "theme",
        "model",
        "small_model",
        "smallModel",
        "default_agent",
        "command",
        "agent",
        "provider",
        "disabled_providers",
        "disabledProviders",
        "enabled_providers",
        "enabledProviders",
        "notifications",
        "images",
        "websearch",
        "instructions",
        "tools",
        "watcher",
        "disabled_providers",
        "enabled_providers",
        "permission",
        "mcp",
    ]
    .into_iter()
    .collect();

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_applies_top_level_runtime_configuration() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "instructions": ["AGENTS.md"],
                "tools": { "bash": false, "read": true },
                "compaction": false,
                "watcher": { "ignore": ["generated", "tmp/cache"] },
                "formatter": { "rs": "rustfmt", ".md": false },
                "disabled_providers": ["openai"],
                "enabled_providers": ["anthropic", "google"]
            }),
            &mut diagnostics,
        );

        assert_eq!(config.instructions, vec!["AGENTS.md"]);
        assert_eq!(config.tools.get("bash"), Some(&false));
        assert!(!config.compaction.is_enabled());
        assert_eq!(config.watcher.ignored_paths(), ["generated", "tmp/cache"]);
        assert_eq!(
            config.formatter.get("rs"),
            Some(&FormatterConfig::Command("rustfmt".into()))
        );
        assert_eq!(config.formatter.get("md"), Some(&FormatterConfig::Disabled));
        assert!(!config.provider_is_enabled("openai"));
        assert!(config.provider_is_enabled("anthropic"));
        assert!(!config.provider_is_enabled("mistral"));
    }

    #[test]
    fn parses_small_model_aliases() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({ "small_model": "openai/gpt-4.1-mini" }),
            &mut diagnostics,
        );

        assert_eq!(config.small_model.as_deref(), Some("openai/gpt-4.1-mini"));
        assert!(diagnostics.warnings.is_empty());

        let config = parse_merged_config(
            &json!({ "smallModel": "anthropic/claude-haiku" }),
            &mut diagnostics,
        );

        assert_eq!(
            config.small_model.as_deref(),
            Some("anthropic/claude-haiku")
        );
    }

    #[test]
    fn parses_enabled_and_disabled_providers() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "enabledProviders": [" anthropic ", "openai", ""],
                "disabled_providers": ["openai", 42]
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.enabled_providers,
            BTreeSet::from(["anthropic".to_string(), "openai".to_string()])
        );
        assert_eq!(
            config.disabled_providers,
            BTreeSet::from(["openai".to_string()])
        );
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("disabled_providers entries")));
    }

    #[test]
    fn opencode_filter_keeps_small_model() {
        let filtered = filter_top_level(
            json!({
                "small_model": "openai/gpt-4.1-mini",
                "smallModel": "anthropic/claude-haiku",
                "theme": "ignored"
            }),
            SourceKind::OpenCode,
        );

        assert_eq!(
            filtered.get("small_model").and_then(Value::as_str),
            Some("openai/gpt-4.1-mini")
        );
        assert_eq!(
            filtered.get("smallModel").and_then(Value::as_str),
            Some("anthropic/claude-haiku")
        );
        assert!(filtered.get("theme").is_none());
    }

    #[test]
    fn parses_event_notifications() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "notifications": {
                    "terminalCondition": "always",
                    "macosBackend": "osascript",
                    "complete": {
                        "terminal": "enabled",
                        "desktop": true,
                        "soundEnabled": true,
                        "soundFile": "/tmp/complete.wav"
                    },
                    "subagentComplete": {
                        "terminal": "disabled",
                        "soundFile": "/tmp/subagent-complete.wav"
                    },
                    "permission": {
                        "terminal": "enabled"
                    },
                    "question": {
                        "terminal": "disabled"
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.notifications.complete.terminal,
            TerminalNotificationMode::Enabled
        );
        assert!(config.notifications.complete.desktop);
        assert!(config.notifications.complete.sound_enabled);
        assert_eq!(
            config.notifications.complete.sound_file,
            Some(PathBuf::from("/tmp/complete.wav"))
        );
        assert_eq!(
            config.notifications.subagent_complete.terminal,
            TerminalNotificationMode::Disabled
        );
        assert!(config.notifications.subagent_complete.desktop);
        assert_eq!(
            config.notifications.subagent_complete.sound_file,
            Some(PathBuf::from("/tmp/subagent-complete.wav"))
        );
        assert_eq!(
            config.notifications.permission.terminal,
            TerminalNotificationMode::Enabled
        );
        assert_eq!(
            config.notifications.question.terminal,
            TerminalNotificationMode::Disabled
        );
        assert_eq!(
            config.notifications.terminal_condition,
            TerminalNotificationCondition::Always
        );
        assert_eq!(
            config.notifications.macos_backend,
            MacosNotificationBackend::Osascript
        );
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn subagent_complete_notification_inherits_complete_settings_but_not_sound_file() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "notifications": {
                    "complete": {
                        "terminal": "disabled",
                        "desktop": true,
                        "soundEnabled": false,
                        "soundFile": "/tmp/complete.wav"
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.notifications.subagent_complete.terminal,
            config.notifications.complete.terminal
        );
        assert_eq!(
            config.notifications.subagent_complete.sound_enabled,
            config.notifications.complete.sound_enabled
        );
        assert_eq!(
            config.notifications.subagent_complete.desktop,
            config.notifications.complete.desktop
        );
        assert_eq!(config.notifications.subagent_complete.sound_file, None);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_websearch_config() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "websearch": {
                    "enabled": true,
                    "provider": "exa",
                    "endpoint": "https://mcp.exa.ai/mcp",
                    "apiKey": "secret"
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(config.websearch.enabled, Some(true));
        assert_eq!(config.websearch.provider, WebsearchProvider::Exa);
        assert_eq!(config.websearch.provider.as_str(), "exa");
        assert_eq!(
            config.websearch.endpoint.as_deref(),
            Some("https://mcp.exa.ai/mcp")
        );
        assert_eq!(config.websearch.api_key.as_deref(), Some("secret"));
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_websearch_boolean_shorthand() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(&json!({ "websearch": false }), &mut diagnostics);

        assert_eq!(config.websearch.enabled, Some(false));
        assert_eq!(config.websearch.provider, WebsearchProvider::ExaHostedMcp);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_keiro_websearch_config() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "websearch": {
                    "provider": "keiro",
                    "apiKey": "secret"
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(config.websearch.provider, WebsearchProvider::Keiro);
        assert_eq!(config.websearch.api_key.as_deref(), Some("secret"));
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_only_canonical_websearch_providers() {
        assert_eq!(
            parse_websearch_provider("exa-hosted-mcp"),
            Some(WebsearchProvider::ExaHostedMcp)
        );
        assert_eq!(
            parse_websearch_provider("exa"),
            Some(WebsearchProvider::Exa)
        );
        assert_eq!(
            parse_websearch_provider("tavily"),
            Some(WebsearchProvider::Tavily)
        );
        assert_eq!(
            parse_websearch_provider("perplexity"),
            Some(WebsearchProvider::Perplexity)
        );
        assert_eq!(
            parse_websearch_provider("brave"),
            Some(WebsearchProvider::Brave)
        );
        assert_eq!(
            parse_websearch_provider("ollama-cloud"),
            Some(WebsearchProvider::OllamaCloud)
        );
        assert_eq!(
            parse_websearch_provider("serpapi"),
            Some(WebsearchProvider::SerpApi)
        );
        assert_eq!(
            parse_websearch_provider("keiro"),
            Some(WebsearchProvider::Keiro)
        );
        assert_eq!(parse_websearch_provider("ollama"), None);
        assert_eq!(parse_websearch_provider("keiro-labs"), None);
        assert_eq!(parse_websearch_provider("keirolabs"), None);
        assert_eq!(parse_websearch_provider("brave-search"), None);
    }

    #[test]
    fn parses_macos_notification_backend() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "notifications": {
                    "macosBackend": "osascript"
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.notifications.macos_backend,
            MacosNotificationBackend::Osascript
        );
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn top_level_sounds_config_is_ignored() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "sounds": {
                    "complete": {
                        "enabled": false,
                        "notify": true,
                        "file": "/tmp/legacy.wav"
                    }
                }
            }),
            &mut diagnostics,
        );

        assert!(config.notifications.complete.sound_enabled);
        assert!(!config.notifications.complete.desktop);
        assert_eq!(config.notifications.complete.sound_file, None);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn legacy_terminal_notifications_are_migrated() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "notifications": {
                    "terminal": {
                        "complete": "enabled",
                        "permission": "enabled",
                        "question": "disabled",
                        "condition": "always"
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.notifications.complete.terminal,
            TerminalNotificationMode::Enabled
        );
        assert_eq!(
            config.notifications.permission.terminal,
            TerminalNotificationMode::Enabled
        );
        assert_eq!(
            config.notifications.question.terminal,
            TerminalNotificationMode::Disabled
        );
        assert_eq!(
            config.notifications.terminal_condition,
            TerminalNotificationCondition::Always
        );
        assert!(diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("notifications.terminal is deprecated")));
    }

    #[test]
    fn terminal_notifications_default_to_auto_unfocused() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(&json!({}), &mut diagnostics);

        assert_eq!(
            config.notifications.complete.terminal,
            TerminalNotificationMode::Auto
        );
        assert_eq!(
            config.notifications.permission.terminal,
            TerminalNotificationMode::Auto
        );
        assert_eq!(
            config.notifications.question.terminal,
            TerminalNotificationMode::Auto
        );
        assert_eq!(
            config.notifications.terminal_condition,
            TerminalNotificationCondition::Unfocused
        );
    }

    #[test]
    fn terminal_notification_boolean_complete_is_supported() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "notifications": {
                    "complete": {
                        "terminal": false
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.notifications.complete.terminal,
            TerminalNotificationMode::Disabled
        );
    }

    #[test]
    fn images_open_with_defaults_to_auto() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(&json!({}), &mut diagnostics);

        assert_eq!(config.images.open_with, ImageOpenWith::Auto);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_images_open_with_string() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "images": {
                    "openWith": "system"
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(config.images.open_with, ImageOpenWith::System);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_images_open_with_command() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "images": {
                    "openWith": {
                        "command": "zed",
                        "args": ["{path}"]
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(
            config.images.open_with,
            ImageOpenWith::Command(ImageOpenCommandConfig {
                command: "zed".to_string(),
                args: vec!["{path}".to_string()],
            })
        );
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_global_permission_rules_in_order() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "permission": {
                    "bash": {
                        "*": "ask",
                        "git *": "allow",
                        "git push *": "deny"
                    },
                    "mcp_*": "deny"
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(config.permission_rules.len(), 4);
        assert_eq!(config.permission_rules[0].permission, "bash");
        assert_eq!(config.permission_rules[0].pattern, "*");
        assert_eq!(
            config.permission_rules[0].action,
            PermissionPolicyAction::Ask
        );
        assert_eq!(config.permission_rules[3].permission, "mcp_*");
        assert_eq!(
            config.permission_rules[3].action,
            PermissionPolicyAction::Deny
        );
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_agent_permission_overrides() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "permission": "ask",
                "agent": {
                    "build": {
                        "permission": {
                            "bash": {
                                "*": "ask",
                                "git status *": "allow"
                            },
                            "edit": "deny"
                        }
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(config.permission_rules.len(), 1);
        assert_eq!(config.permission_rules[0].permission, "*");
        assert_eq!(
            config.permission_rules[0].action,
            PermissionPolicyAction::Ask
        );

        let build_rules = config
            .agent_permission_rules
            .get("build")
            .expect("build agent permission rules");
        assert_eq!(build_rules.len(), 3);
        assert_eq!(build_rules[2].permission, "edit");
        assert_eq!(build_rules[2].action, PermissionPolicyAction::Deny);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn agent_max_steps_alias_is_supported() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "agent": {
                    "build": {
                        "maxSteps": 42
                    }
                }
            }),
            &mut diagnostics,
        );

        assert_eq!(config.agent_steps.get("build"), Some(&42));
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn parses_standard_custom_provider_options_and_model_metadata() {
        let mut diagnostics = ConfigDiagnostics::default();
        let config = parse_merged_config(
            &json!({
                "provider": {
                    "custom": {
                        "name": "Custom Provider",
                        "npm": "@ai-sdk/openai-compatible",
                        "options": {
                            "baseURL": "https://example.com/v1",
                            "apiKey": "{env:CUSTOM_PROVIDER_KEY}"
                        },
                        "models": {
                            "vision-model": {
                                "name": "Vision Model",
                                "attachment": true,
                                "reasoning": true,
                                "temperature": true,
                                "tool_call": true,
                                "limit": {
                                    "context": 128000,
                                    "output": 8192
                                },
                                "modalities": {
                                    "input": ["text", "image"],
                                    "output": ["text"]
                                }
                            }
                        }
                    },
                    "anthropic": {
                        "options": {
                            "timeout": 300000
                        }
                    }
                }
            }),
            &mut diagnostics,
        );

        let provider = config
            .custom_providers
            .get("custom")
            .expect("custom provider");
        assert_eq!(provider.name.as_deref(), Some("Custom Provider"));
        assert_eq!(provider.npm.as_deref(), Some("@ai-sdk/openai-compatible"));
        assert_eq!(provider.base_url.as_deref(), Some("https://example.com/v1"));
        assert_eq!(
            provider.api_key.as_deref(),
            Some("{env:CUSTOM_PROVIDER_KEY}")
        );

        let model = provider.models.get("vision-model").expect("custom model");
        assert_eq!(model.name.as_deref(), Some("Vision Model"));
        assert_eq!(model.context_window, Some(128000));
        assert_eq!(model.max_tokens, Some(8192));
        assert_eq!(model.attachment, Some(true));
        assert_eq!(model.reasoning, Some(true));
        assert_eq!(model.temperature, Some(true));
        assert_eq!(model.tool_call, Some(true));
        let modalities = model.modalities.as_ref().expect("model modalities");
        assert_eq!(modalities.input, ["text", "image"]);
        assert_eq!(modalities.output, ["text"]);

        assert!(!config.custom_providers.contains_key("anthropic"));
        assert_eq!(
            config.provider_timeouts.get("anthropic"),
            Some(&ProviderTimeout::Millis(300000))
        );
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn resolves_literal_custom_provider_api_key() {
        let provider = CustomProviderConfig {
            name: None,
            npm: None,
            base_url: None,
            api_key: Some(" secret-key ".to_string()),
            models: HashMap::new(),
        };

        assert_eq!(provider.resolved_api_key().as_deref(), Some("secret-key"));
    }

    #[test]
    fn resolves_custom_provider_api_key_from_environment_reference() {
        assert_eq!(
            resolve_api_key_value(Some("{env:CUSTOM_PROVIDER_KEY}"), |variable| {
                assert_eq!(variable, "CUSTOM_PROVIDER_KEY");
                Some(" env-secret ".to_string())
            })
            .as_deref(),
            Some("env-secret")
        );
        assert_eq!(
            resolve_api_key_value(Some("{env:MISSING_KEY}"), |_| None),
            None
        );
    }
}
