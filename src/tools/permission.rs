use crate::llm::{ChunkMessage, ChunkSender};
use crate::tools::ToolError;
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;

const DOOM_LOOP_THRESHOLD: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionAction {
    Read,
    Write,
    Edit,
    List,
    Glob,
    Grep,
    Bash,
    Unknown,
}

fn external_directory_pattern_matches(candidate: &str, grant: &str) -> bool {
    if grant == "*" {
        return true;
    }

    let Some(grant_root) = grant.strip_suffix("/*") else {
        return wildcard_match(candidate, grant);
    };
    let candidate_root = candidate.strip_suffix("/*").unwrap_or(candidate);

    Path::new(candidate_root).starts_with(Path::new(grant_root))
}

fn is_safe_workspace_read_like_action(action: PermissionAction) -> bool {
    matches!(
        action,
        PermissionAction::Read
            | PermissionAction::List
            | PermissionAction::Glob
            | PermissionAction::Grep
    )
}

impl PermissionAction {
    pub fn from_tool_id(tool_id: &str) -> Self {
        match tool_id {
            "read" | "view_image" => Self::Read,
            "write" | "write_files" => Self::Write,
            "edit" | "apply_patch" => Self::Edit,
            "list" => Self::List,
            "glob" => Self::Glob,
            "grep" => Self::Grep,
            "bash" | "terminal_session" => Self::Bash,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResponse {
    Deny,
    AllowOnce,
    AllowAlways,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionPolicyAction {
    Allow,
    Deny,
    Ask,
}

impl PermissionPolicyAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionRule {
    pub permission: String,
    pub pattern: String,
    pub action: PermissionPolicyAction,
}

pub type PermissionRules = Vec<PermissionRule>;

#[derive(Debug)]
pub struct PermissionPrompt {
    pub tool_id: String,
    pub action: PermissionAction,
    pub permission: String,
    pub patterns: Vec<String>,
    pub target: Option<String>,
    pub command: Option<String>,
    pub workdir: Option<String>,
    pub reason: String,
    pub response_tx: tokio::sync::oneshot::Sender<PermissionResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionGrant {
    pub permission: String,
    pub patterns: Vec<String>,
}

impl PermissionGrant {
    pub fn matches(&self, other: &Self) -> bool {
        if !wildcard_match(&other.permission, &self.permission)
            && !wildcard_match(&self.permission, &other.permission)
        {
            return false;
        }

        other.patterns.iter().any(|candidate| {
            self.patterns
                .iter()
                .any(|grant| match self.permission.as_str() {
                    "external_directory" => external_directory_pattern_matches(candidate, grant),
                    _ => wildcard_match(candidate, grant),
                })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PermissionReasonKind {
    SensitivePath,
    ExternalPath,
    DoomLoop,
    ConfiguredAsk,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PermissionFingerprint {
    tool_id: String,
    action: PermissionAction,
    target: Option<String>,
    command: Option<String>,
    reason: PermissionReasonKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ToolCallFingerprint {
    tool_id: String,
    params: String,
}

#[derive(Debug, Clone)]
pub struct AgentToolPolicies {
    custom: HashMap<String, HashSet<String>>,
}

impl AgentToolPolicies {
    pub fn new() -> Self {
        Self {
            custom: HashMap::new(),
        }
    }

    pub fn with_custom_tools(
        mut self,
        mode_name: impl Into<String>,
        tools: impl IntoIterator<Item = String>,
    ) -> Self {
        let mode = mode_name.into().trim().to_ascii_lowercase();
        if mode.is_empty() {
            return self;
        }

        let set: HashSet<String> = tools
            .into_iter()
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        self.custom.insert(mode, set);
        self
    }

    pub fn is_allowed(&self, mode_name: &str, tool_id: &str) -> bool {
        let mode = mode_name.trim().to_ascii_lowercase();
        let tool = tool_id.trim().to_ascii_lowercase();

        if let Some(custom) = self.custom.get(&mode) {
            return custom.contains("*") || custom.contains(&tool);
        }

        if mode == "plan" {
            // OpenCode plan mode is read-only by default. Custom agent tool
            // policies above can still opt specific tools back in.
            return !matches!(
                tool.as_str(),
                "bash" | "terminal_session" | "write" | "write_files" | "edit" | "apply_patch"
            );
        }

        if mode == "build" {
            return true;
        }

        // Unknown/custom modes default to build behavior unless explicitly configured.
        true
    }
}

impl Default for AgentToolPolicies {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct ToolPermissions {
    workdir: PathBuf,
    always_grants: Arc<RwLock<HashSet<PermissionFingerprint>>>,
    runtime_grants: Arc<RwLock<HashSet<PermissionGrant>>>,
    call_counts: Arc<RwLock<HashMap<ToolCallFingerprint, usize>>>,
    agent_policies: Arc<AgentToolPolicies>,
    permission_rules: Arc<PermissionRules>,
    agent_permission_rules: Arc<HashMap<String, PermissionRules>>,
    global_tool_config: Arc<HashMap<String, bool>>,
    dangerously_skip_permissions: bool,
}

impl ToolPermissions {
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: normalize_path(&workdir.into()),
            always_grants: Arc::new(RwLock::new(HashSet::new())),
            runtime_grants: Arc::new(RwLock::new(HashSet::new())),
            call_counts: Arc::new(RwLock::new(HashMap::new())),
            agent_policies: Arc::new(AgentToolPolicies::default()),
            permission_rules: Arc::new(Vec::new()),
            agent_permission_rules: Arc::new(HashMap::new()),
            global_tool_config: Arc::new(HashMap::new()),
            dangerously_skip_permissions: false,
        }
    }

    pub fn with_agent_policies(mut self, policies: AgentToolPolicies) -> Self {
        self.agent_policies = Arc::new(policies);
        self
    }

    pub fn with_global_tool_config(mut self, tools: HashMap<String, bool>) -> Self {
        self.global_tool_config = Arc::new(tools);
        self
    }

    pub fn with_permission_rules(mut self, rules: PermissionRules) -> Self {
        self.permission_rules = Arc::new(rules);
        self
    }

    pub fn with_agent_permission_rules(mut self, rules: HashMap<String, PermissionRules>) -> Self {
        let normalized = rules
            .into_iter()
            .map(|(agent, rules)| (agent.trim().to_ascii_lowercase(), rules))
            .filter(|(agent, _)| !agent.is_empty())
            .collect();
        self.agent_permission_rules = Arc::new(normalized);
        self
    }

    pub fn with_workdir(mut self, workdir: impl Into<PathBuf>) -> Self {
        self.workdir = normalize_path(&workdir.into());
        self
    }

    pub fn dangerously_skip_permissions(mut self, enabled: bool) -> Self {
        self.dangerously_skip_permissions = enabled;
        self
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn is_tool_allowed_for_agent(&self, agent_mode: &str, tool_id: &str) -> bool {
        self.agent_policies.is_allowed(agent_mode, tool_id)
            && self
                .global_tool_config
                .get(tool_id)
                .copied()
                .unwrap_or(true)
    }

    pub fn is_tool_visible_for_agent(&self, agent_mode: &str, tool_id: &str) -> bool {
        if !self.is_tool_allowed_for_agent(agent_mode, tool_id) {
            return false;
        }

        let permission_key = permission_key_for_tool_id(tool_id);
        let patterns = vec!["*".to_string()];
        !matches!(
            self.evaluate_config_decision(agent_mode, &permission_key, tool_id, &patterns),
            Some(PermissionPolicyAction::Deny)
        )
    }

    pub async fn preflight(
        &self,
        agent_mode: &str,
        tool_id: &str,
        params: &Value,
        sender: Option<&ChunkSender>,
    ) -> Result<(), ToolError> {
        if !self.is_tool_allowed_for_agent(agent_mode, tool_id) {
            return Err(ToolError::Permission(format!(
                "Tool '{}' is not available in {} mode",
                tool_id, agent_mode
            )));
        }

        let action = PermissionAction::from_tool_id(tool_id);
        let paths = extract_primary_paths(tool_id, action, params, &self.workdir);
        let path = paths.first().cloned();
        let command = if action == PermissionAction::Bash {
            get_string(params, "command").map(|s| s.trim().to_string())
        } else {
            None
        };
        let permission_key = permission_key_for_tool_id(tool_id);
        let patterns = permission_patterns_for_tool(
            tool_id,
            action,
            params,
            path.as_deref(),
            command.as_deref(),
            &self.workdir,
        );

        match self.evaluate_config_decision(agent_mode, &permission_key, tool_id, &patterns) {
            Some(PermissionPolicyAction::Deny) => {
                return Err(ToolError::Permission(configured_deny_text(
                    tool_id, &patterns,
                )));
            }
            Some(PermissionPolicyAction::Ask) if !self.dangerously_skip_permissions => {
                return self
                    .ask_permission(
                        tool_id,
                        action,
                        PermissionReasonKind::ConfiguredAsk,
                        path.as_deref(),
                        command.clone(),
                        sender,
                    )
                    .await;
            }
            _ => {}
        }

        let (mut reason, mut reason_path) = self.evaluate_reasons(action, &paths);
        let prompt_path = reason_path.as_deref().or(path.as_deref());
        if let Some(reason_kind) = reason {
            match self.evaluate_guard_decision(
                agent_mode,
                tool_id,
                reason_kind,
                prompt_path,
                &patterns,
            ) {
                Some(PermissionPolicyAction::Deny) => {
                    return Err(ToolError::Permission(guard_deny_text(
                        reason_kind,
                        tool_id,
                        prompt_path,
                    )));
                }
                Some(PermissionPolicyAction::Allow) => {
                    reason = None;
                    reason_path = None;
                }
                _ => {}
            }
        }

        if self.dangerously_skip_permissions {
            return Ok(());
        }

        if let Some(reason_kind) = reason {
            return self
                .ask_permission(
                    tool_id,
                    action,
                    reason_kind,
                    reason_path.as_deref().or(path.as_deref()),
                    command.clone(),
                    sender,
                )
                .await;
        }

        let doom_loop_reason = if is_safe_workspace_read_like_action(action) {
            None
        } else {
            self.evaluate_doom_loop(tool_id, params).await
        };

        if let Some(reason_kind) = doom_loop_reason {
            match self.evaluate_guard_decision(
                agent_mode,
                tool_id,
                reason_kind,
                path.as_deref(),
                &patterns,
            ) {
                Some(PermissionPolicyAction::Deny) => {
                    return Err(ToolError::Permission(guard_deny_text(
                        reason_kind,
                        tool_id,
                        path.as_deref(),
                    )));
                }
                Some(PermissionPolicyAction::Allow) => return Ok(()),
                _ => {
                    return self
                        .ask_permission(
                            tool_id,
                            action,
                            reason_kind,
                            path.as_deref(),
                            command,
                            sender,
                        )
                        .await;
                }
            }
        }

        Ok(())
    }

    async fn ask_permission(
        &self,
        tool_id: &str,
        action: PermissionAction,
        reason_kind: PermissionReasonKind,
        path: Option<&Path>,
        command: Option<String>,
        sender: Option<&ChunkSender>,
    ) -> Result<(), ToolError> {
        let target = path
            .map(|p| p.display().to_string())
            .or_else(|| command.clone());
        let workdir = if action == PermissionAction::Bash {
            path.map(|p| p.display().to_string())
        } else {
            None
        };

        let fingerprint = PermissionFingerprint {
            tool_id: tool_id.to_string(),
            action,
            target: target.clone(),
            command: command.clone(),
            reason: reason_kind,
        };

        let grant = permission_grant_for_prompt(
            reason_kind,
            tool_id,
            action,
            path,
            command.as_deref(),
            &self.workdir,
        );

        let prompt_target = if action == PermissionAction::Bash {
            command.clone().or_else(|| target.clone())
        } else {
            target.clone()
        };

        if self.always_grants.read().await.contains(&fingerprint) {
            return Ok(());
        }

        if self
            .runtime_grants
            .read()
            .await
            .iter()
            .any(|remembered| remembered.matches(&grant))
        {
            return Ok(());
        }

        let reason_text = reason_text(reason_kind, tool_id, target.as_deref());

        let Some(sender) = sender else {
            return Err(ToolError::Permission(reason_text));
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let prompt = PermissionPrompt {
            tool_id: tool_id.to_string(),
            action,
            permission: grant.permission.clone(),
            patterns: grant.patterns.clone(),
            target: prompt_target,
            command,
            workdir,
            reason: reason_text,
            response_tx,
        };

        sender
            .send(ChunkMessage::PermissionRequest(prompt))
            .map_err(|_| {
                ToolError::Execution("Failed to deliver permission request to UI".to_string())
            })?;

        let response = response_rx.await.unwrap_or(PermissionResponse::Deny);
        match response {
            PermissionResponse::Deny => Err(ToolError::Permission(
                "Permission denied by user".to_string(),
            )),
            PermissionResponse::AllowOnce => Ok(()),
            PermissionResponse::AllowAlways => {
                self.always_grants.write().await.insert(fingerprint);
                if reason_kind != PermissionReasonKind::DoomLoop {
                    self.runtime_grants.write().await.insert(grant);
                }
                Ok(())
            }
        }
    }

    fn evaluate_config_decision(
        &self,
        agent_mode: &str,
        permission_key: &str,
        tool_id: &str,
        patterns: &[String],
    ) -> Option<PermissionPolicyAction> {
        let agent_key = agent_mode.trim().to_ascii_lowercase();
        let empty: &[PermissionRule] = &[];
        let agent_rules = self
            .agent_permission_rules
            .get(&agent_key)
            .map(Vec::as_slice)
            .unwrap_or(empty);
        evaluate_permission_rules(
            permission_key,
            tool_id,
            patterns,
            &[self.permission_rules.as_slice(), agent_rules],
        )
    }

    fn evaluate_guard_decision(
        &self,
        agent_mode: &str,
        tool_id: &str,
        reason: PermissionReasonKind,
        path: Option<&Path>,
        fallback_patterns: &[String],
    ) -> Option<PermissionPolicyAction> {
        let (permission_key, patterns) = match reason {
            PermissionReasonKind::ExternalPath => (
                "external_directory".to_string(),
                path.map(external_directory_patterns)
                    .filter(|patterns| !patterns.is_empty())
                    .unwrap_or_else(|| fallback_patterns.to_vec()),
            ),
            PermissionReasonKind::DoomLoop => (
                "doom_loop".to_string(),
                vec![tool_id.to_string(), "*".to_string()],
            ),
            PermissionReasonKind::SensitivePath | PermissionReasonKind::ConfiguredAsk => (
                permission_key_for_tool_id(tool_id),
                fallback_patterns.to_vec(),
            ),
        };

        self.evaluate_config_decision(agent_mode, &permission_key, tool_id, &patterns)
    }

    fn evaluate_reason(
        &self,
        action: PermissionAction,
        path: Option<&Path>,
    ) -> Option<PermissionReasonKind> {
        if action == PermissionAction::Read {
            if let Some(path) = path {
                if is_sensitive_path(path) {
                    return Some(PermissionReasonKind::SensitivePath);
                }
            }
        }

        if matches!(
            action,
            PermissionAction::Read
                | PermissionAction::Write
                | PermissionAction::Edit
                | PermissionAction::List
                | PermissionAction::Glob
                | PermissionAction::Grep
                | PermissionAction::Bash
        ) {
            if let Some(path) = path {
                if is_outside_workdir(path, &self.workdir) {
                    return Some(PermissionReasonKind::ExternalPath);
                }
            }
        }

        None
    }

    fn evaluate_reasons(
        &self,
        action: PermissionAction,
        paths: &[PathBuf],
    ) -> (Option<PermissionReasonKind>, Option<PathBuf>) {
        for path in paths {
            if let Some(reason) = self.evaluate_reason(action, Some(path.as_path())) {
                return (Some(reason), Some(path.clone()));
            }
        }
        (None, None)
    }

    async fn evaluate_doom_loop(
        &self,
        tool_id: &str,
        params: &Value,
    ) -> Option<PermissionReasonKind> {
        let key = ToolCallFingerprint {
            tool_id: tool_id.to_string(),
            params: serde_json::to_string(params).unwrap_or_else(|_| params.to_string()),
        };

        let mut call_counts = self.call_counts.write().await;
        let count = call_counts.entry(key).or_insert(0);
        *count += 1;

        if *count >= DOOM_LOOP_THRESHOLD {
            Some(PermissionReasonKind::DoomLoop)
        } else {
            None
        }
    }
}

fn reason_text(reason: PermissionReasonKind, tool_id: &str, target: Option<&str>) -> String {
    match reason {
        PermissionReasonKind::SensitivePath => match target {
            Some(target) => format!(
                "Tool '{}' wants to access sensitive file '{}'; explicit approval required",
                tool_id, target
            ),
            None => format!(
                "Tool '{}' wants to access a sensitive file; explicit approval required",
                tool_id
            ),
        },
        PermissionReasonKind::ExternalPath => match target {
            Some(target) => format!(
                "Tool '{}' wants to access path outside working directory: {}",
                tool_id, target
            ),
            None => format!(
                "Tool '{}' wants to access path outside working directory",
                tool_id
            ),
        },
        PermissionReasonKind::DoomLoop => match target {
            Some(target) => format!(
                "Tool '{}' repeated the same request for {}; explicit approval required",
                tool_id, target
            ),
            None => format!(
                "Tool '{}' repeated the same request; explicit approval required",
                tool_id
            ),
        },
        PermissionReasonKind::ConfiguredAsk => match target {
            Some(target) => format!(
                "Permission config requires approval before tool '{}' can access '{}'",
                tool_id, target
            ),
            None => format!(
                "Permission config requires approval before running tool '{}'",
                tool_id
            ),
        },
    }
}

fn configured_deny_text(tool_id: &str, patterns: &[String]) -> String {
    let target = patterns
        .iter()
        .find(|pattern| pattern.as_str() != "*")
        .map(String::as_str)
        .unwrap_or("*");
    format!(
        "Permission config denies tool '{}' for pattern '{}'",
        tool_id, target
    )
}

fn guard_deny_text(reason: PermissionReasonKind, tool_id: &str, path: Option<&Path>) -> String {
    let target = path.map(|p| p.display().to_string());
    match reason {
        PermissionReasonKind::SensitivePath => match target {
            Some(target) => format!(
                "Permission config denies tool '{}' access to sensitive file '{}'",
                tool_id, target
            ),
            None => format!(
                "Permission config denies tool '{}' access to sensitive files",
                tool_id
            ),
        },
        PermissionReasonKind::ExternalPath => match target {
            Some(target) => format!(
                "Permission config denies tool '{}' access outside the working directory: {}",
                tool_id, target
            ),
            None => format!(
                "Permission config denies tool '{}' access outside the working directory",
                tool_id
            ),
        },
        PermissionReasonKind::DoomLoop => {
            format!(
                "Permission config denies repeated identical tool calls for '{}'",
                tool_id
            )
        }
        PermissionReasonKind::ConfiguredAsk => configured_deny_text(tool_id, &[]),
    }
}

fn get_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn permission_key_for_tool_id(tool_id: &str) -> String {
    match tool_id.trim().to_ascii_lowercase().as_str() {
        "write" | "write_files" | "edit" | "apply_patch" => "edit".to_string(),
        "read" | "view_image" => "read".to_string(),
        "terminal_session" => "bash".to_string(),
        other => other.to_string(),
    }
}

fn permission_patterns_for_tool(
    tool_id: &str,
    action: PermissionAction,
    params: &Value,
    path: Option<&Path>,
    command: Option<&str>,
    workdir: &Path,
) -> Vec<String> {
    let mut patterns = Vec::new();

    match tool_id {
        "bash" | "terminal_session" => {
            if let Some(command) = command {
                push_nonempty(&mut patterns, command);
            }
        }
        "glob" => {
            if let Some(pattern) = get_string(params, "pattern") {
                push_nonempty(&mut patterns, &pattern);
            }
        }
        "grep" => {
            if let Some(pattern) = get_string(params, "pattern") {
                push_nonempty(&mut patterns, &pattern);
            }
        }
        "skill" => {
            if let Some(name) = get_string(params, "name") {
                push_nonempty(&mut patterns, &name);
            }
        }
        "task" => {
            if let Some(subagent) = get_string(params, "subagent_type") {
                push_nonempty(&mut patterns, &subagent);
            }
        }
        "webfetch" => {
            if let Some(url) = get_string(params, "url") {
                push_nonempty(&mut patterns, &url);
            }
        }
        "websearch" => {
            if let Some(query) = get_string(params, "query") {
                push_nonempty(&mut patterns, &query);
            }
        }
        "question" | "update_plan" => patterns.push("*".to_string()),
        "write_files" => {
            if let Some(files) = params.get("files").and_then(Value::as_array) {
                for file in files {
                    if let Some(path) = get_string(file, "file_path")
                        .or_else(|| get_string(file, "filePath"))
                        .or_else(|| get_string(file, "path"))
                    {
                        push_nonempty(&mut patterns, &path);
                    }
                }
            }
        }
        "apply_patch" => {
            for path in crate::tools::patch::patch_paths_from_params(params) {
                push_nonempty(&mut patterns, &path);
            }
        }
        _ => {}
    }

    if patterns.is_empty() {
        if let Some(path) = path {
            patterns.extend(path_patterns(path, workdir));
        }
    }

    if patterns.is_empty() {
        if matches!(
            action,
            PermissionAction::Unknown
                | PermissionAction::Bash
                | PermissionAction::Glob
                | PermissionAction::Grep
        ) {
            patterns.push("*".to_string());
        }
    }

    patterns
}

fn push_nonempty(patterns: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if !trimmed.is_empty() {
        patterns.push(trimmed.to_string());
    }
}

fn path_patterns(path: &Path, workdir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let absolute = normalize_path(path);

    if let Ok(relative) = absolute.strip_prefix(workdir) {
        let relative = normalize_pattern_path(relative);
        if !relative.is_empty() {
            out.push(relative);
        }
    }

    let absolute = normalize_pattern_path(&absolute);
    if !absolute.is_empty() && !out.iter().any(|existing| existing == &absolute) {
        out.push(absolute);
    }

    if out.is_empty() {
        out.push("*".to_string());
    }

    out
}

fn external_directory_patterns(path: &Path) -> Vec<String> {
    let absolute = normalize_path(path);
    let directory = if std::fs::metadata(&absolute)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        absolute
    } else {
        absolute
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| absolute.clone())
    };
    let directory = normalize_pattern_path(&directory);
    if directory.is_empty() {
        vec!["*".to_string()]
    } else {
        vec![format!("{}/*", directory.trim_end_matches('/'))]
    }
}

fn permission_grant_for_prompt(
    reason: PermissionReasonKind,
    tool_id: &str,
    action: PermissionAction,
    path: Option<&Path>,
    command: Option<&str>,
    workdir: &Path,
) -> PermissionGrant {
    let permission = match reason {
        PermissionReasonKind::ExternalPath => "external_directory".to_string(),
        PermissionReasonKind::DoomLoop => "doom_loop".to_string(),
        PermissionReasonKind::SensitivePath | PermissionReasonKind::ConfiguredAsk => {
            permission_key_for_tool_id(tool_id)
        }
    };

    let mut patterns = match reason {
        PermissionReasonKind::ExternalPath => {
            path.map(external_directory_patterns).unwrap_or_default()
        }
        PermissionReasonKind::DoomLoop => vec![tool_id.to_string()],
        PermissionReasonKind::SensitivePath | PermissionReasonKind::ConfiguredAsk => {
            if action == PermissionAction::Bash {
                command
                    .map(|command| vec![command.to_string()])
                    .unwrap_or_default()
            } else {
                path.map(|path| path_patterns(path, workdir))
                    .unwrap_or_default()
            }
        }
    };

    if patterns.is_empty() {
        patterns.push("*".to_string());
    }

    PermissionGrant {
        permission,
        patterns,
    }
}

fn normalize_pattern_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn evaluate_permission_rules(
    permission_key: &str,
    tool_id: &str,
    patterns: &[String],
    rulesets: &[&[PermissionRule]],
) -> Option<PermissionPolicyAction> {
    let permission_key = permission_key.trim().to_ascii_lowercase();
    let tool_id = tool_id.trim().to_ascii_lowercase();
    let patterns = if patterns.is_empty() {
        vec!["*".to_string()]
    } else {
        patterns.to_vec()
    };

    let mut decision = None;
    for ruleset in rulesets {
        for rule in *ruleset {
            if !wildcard_match(&permission_key, &rule.permission)
                && !wildcard_match(&tool_id, &rule.permission)
            {
                continue;
            }

            if patterns
                .iter()
                .any(|pattern| wildcard_match(pattern, &rule.pattern))
            {
                decision = Some(rule.action);
            }
        }
    }

    decision
}

pub fn expand_permission_pattern(pattern: &str) -> String {
    let trimmed = pattern.trim();
    let Some(home) = dirs::home_dir() else {
        return trimmed.to_string();
    };
    let home = home.to_string_lossy();

    if trimmed == "~" {
        return home.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return format!("{}/{}", home, rest);
    }
    if trimmed == "$HOME" {
        return home.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("$HOME/") {
        return format!("{}/{}", home, rest);
    }
    trimmed.to_string()
}

pub(crate) fn wildcard_match(input: &str, pattern: &str) -> bool {
    let input = input.replace('\\', "/");
    let pattern = pattern.replace('\\', "/");
    let mut escaped = String::new();

    for ch in pattern.chars() {
        match ch {
            '*' => escaped.push_str(".*"),
            '?' => escaped.push('.'),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }

    if escaped.ends_with(" .*") {
        escaped.truncate(escaped.len() - 3);
        escaped.push_str("( .*)?");
    }

    let pattern = format!("(?s)^{}$", escaped);
    Regex::new(&pattern)
        .map(|regex| regex.is_match(&input))
        .unwrap_or(false)
}

fn extract_primary_path(
    action: PermissionAction,
    params: &Value,
    workdir: &Path,
) -> Option<PathBuf> {
    let raw = match action {
        PermissionAction::Read | PermissionAction::Write | PermissionAction::Edit => {
            get_string(params, "file_path")
                .or_else(|| get_string(params, "filePath"))
                .or_else(|| get_string(params, "path"))
                .or_else(|| first_write_files_path(params))
        }
        PermissionAction::List | PermissionAction::Glob | PermissionAction::Grep => {
            get_string(params, "path").or_else(|| Some(".".to_string()))
        }
        PermissionAction::Bash => {
            get_string(params, "workdir").or_else(|| get_string(params, "path"))
        }
        PermissionAction::Unknown => None,
    }?;

    Some(resolve_path(&raw, workdir))
}

fn extract_primary_paths(
    tool_id: &str,
    action: PermissionAction,
    params: &Value,
    workdir: &Path,
) -> Vec<PathBuf> {
    if tool_id == "write_files" {
        return write_files_paths(params)
            .into_iter()
            .map(|path| resolve_path(&path, workdir))
            .collect();
    }

    if tool_id == "apply_patch" {
        return crate::tools::patch::patch_paths_as_pathbufs(params, workdir)
            .into_iter()
            .map(|path| normalize_path(&path))
            .collect();
    }

    extract_primary_path(action, params, workdir)
        .into_iter()
        .collect()
}

fn write_files_paths(params: &Value) -> Vec<String> {
    params
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| {
                    get_string(file, "file_path")
                        .or_else(|| get_string(file, "filePath"))
                        .or_else(|| get_string(file, "path"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn first_write_files_path(params: &Value) -> Option<String> {
    params
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| {
            get_string(file, "file_path")
                .or_else(|| get_string(file, "filePath"))
                .or_else(|| get_string(file, "path"))
        })
}

pub fn resolve_path(raw: &str, workdir: &Path) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        normalize_path(&p)
    } else {
        normalize_path(&workdir.join(p))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }

    out
}

fn canonical_or_normalized(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_path(path))
}

pub fn is_outside_workdir(path: &Path, workdir: &Path) -> bool {
    let target = canonical_or_normalized(path);
    let base = canonical_or_normalized(workdir);
    !target.starts_with(base)
}

pub fn is_sensitive_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };

    let lower = name.to_ascii_lowercase();
    if lower == ".env.example" {
        return false;
    }

    lower == ".env"
        || lower == ".envrc"
        || lower.starts_with(".env.")
        || lower == "auth.json"
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
}

pub fn is_gitignored(path: &Path, workdir: &Path) -> bool {
    let relative = path.strip_prefix(workdir).ok();
    let candidate = relative.unwrap_or(path);

    let status = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .arg("check-ignore")
        .arg("-q")
        .arg("--")
        .arg(candidate)
        .status();

    matches!(status, Ok(s) if s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_mode_blocks_mutating_tools() {
        let policies = AgentToolPolicies::default();
        assert!(policies.is_allowed("plan", "read"));
        assert!(policies.is_allowed("plan", "glob"));
        assert!(!policies.is_allowed("plan", "bash"));
        assert!(!policies.is_allowed("plan", "terminal_session"));
        assert!(!policies.is_allowed("plan", "write"));
        assert!(!policies.is_allowed("plan", "write_files"));
        assert!(!policies.is_allowed("plan", "edit"));
        assert!(!policies.is_allowed("plan", "apply_patch"));
    }

    #[test]
    fn custom_plan_policy_can_explicitly_allow_bash() {
        let policies = AgentToolPolicies::default()
            .with_custom_tools("plan", vec!["read".to_string(), "bash".to_string()]);

        assert!(policies.is_allowed("plan", "bash"));
        assert!(!policies.is_allowed("plan", "write"));
    }

    #[test]
    fn sensitive_path_detection_matches_env_patterns() {
        assert!(is_sensitive_path(Path::new(".env")));
        assert!(is_sensitive_path(Path::new(".env.local")));
        assert!(is_sensitive_path(Path::new(".env.production")));
        assert!(!is_sensitive_path(Path::new(".env.example")));
        assert!(!is_sensitive_path(Path::new("README.md")));
    }

    #[test]
    fn external_path_detection_works() {
        let wd = PathBuf::from("/tmp/workspace");
        assert!(!is_outside_workdir(
            Path::new("/tmp/workspace/src/main.rs"),
            &wd
        ));
        assert!(is_outside_workdir(
            Path::new("/tmp/elsewhere/file.txt"),
            &wd
        ));
    }

    #[test]
    fn extract_primary_path_accepts_camel_case_file_path() {
        let wd = PathBuf::from("/tmp/workspace");
        let params = serde_json::json!({ "filePath": ".env" });

        let extracted = extract_primary_path(PermissionAction::Read, &params, &wd)
            .expect("expected path to be extracted");

        assert_eq!(extracted, PathBuf::from("/tmp/workspace/.env"));
    }

    #[test]
    fn extract_primary_paths_collects_all_write_files_paths() {
        let wd = PathBuf::from("/tmp/workspace");
        let params = serde_json::json!({
            "files": [
                { "file_path": "src/a.ts", "content": "a" },
                { "file_path": "/tmp/elsewhere/b.ts", "content": "b" }
            ]
        });

        let extracted = extract_primary_paths("write_files", PermissionAction::Write, &params, &wd);

        assert_eq!(
            extracted,
            vec![
                PathBuf::from("/tmp/workspace/src/a.ts"),
                PathBuf::from("/tmp/elsewhere/b.ts")
            ]
        );
    }

    #[test]
    fn extract_primary_paths_collects_all_apply_patch_paths() {
        let wd = PathBuf::from("/tmp/workspace");
        let params = serde_json::json!({
            "patch": "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-old\n+new\n--- /dev/null\n+++ b/src/b.ts\n@@ -0,0 +1 @@\n+new\n"
        });

        let extracted = extract_primary_paths("apply_patch", PermissionAction::Edit, &params, &wd);

        assert_eq!(
            extracted,
            vec![
                PathBuf::from("/tmp/workspace/src/a.ts"),
                PathBuf::from("/tmp/workspace/src/b.ts")
            ]
        );
    }

    #[tokio::test]
    async fn allow_always_persists_for_same_request_fingerprint() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/elsewhere/file.txt" });

        let perms_for_task = perms.clone();
        let params_for_task = params.clone();
        let tx_for_task = tx.clone();
        let first = tokio::spawn(async move {
            perms_for_task
                .preflight("build", "read", &params_for_task, Some(&tx_for_task))
                .await
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };
        let _ = prompt.response_tx.send(PermissionResponse::AllowAlways);

        let first_result = first.await.expect("task should complete");
        assert!(first_result.is_ok());

        let second = perms.preflight("build", "read", &params, Some(&tx)).await;
        assert!(second.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn allow_always_external_directory_covers_nonexistent_descendants() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workdir = temp.path().join("workspace");
        let external = temp.path().join("elsewhere");
        std::fs::create_dir_all(&workdir).expect("workspace");
        std::fs::create_dir_all(&external).expect("external");

        let perms = ToolPermissions::new(&workdir);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let dir_params = serde_json::json!({ "file_path": external.to_string_lossy() });

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = dir_params.clone();
            let tx = tx.clone();
            async move { perms.preflight("build", "read", &params, Some(&tx)).await }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };
        let _ = prompt.response_tx.send(PermissionResponse::AllowAlways);
        assert!(pending
            .await
            .expect("preflight task should complete")
            .is_ok());

        let nested_missing = external.join("somewhat-else").join("business.md");
        let result = perms
            .preflight(
                "build",
                "read",
                &serde_json::json!({ "file_path": nested_missing }),
                Some(&tx),
            )
            .await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn allow_always_external_directory_covers_nested_reads() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workdir = temp.path().join("workspace");
        let external = temp.path().join("elsewhere");
        std::fs::create_dir_all(&workdir).expect("workspace");
        std::fs::create_dir_all(&external).expect("external");
        std::fs::write(external.join("README.md"), "hello").expect("readme");

        let perms = ToolPermissions::new(&workdir);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let dir_params = serde_json::json!({ "file_path": external.to_string_lossy() });

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = dir_params.clone();
            let tx = tx.clone();
            async move { perms.preflight("build", "read", &params, Some(&tx)).await }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        let expected_scope = format!("{}/*", external.to_string_lossy());
        assert_eq!(prompt.permission, "external_directory");
        assert_eq!(prompt.patterns, vec![expected_scope.clone()]);
        assert_eq!(
            prompt.target.as_deref(),
            Some(external.to_string_lossy().as_ref())
        );
        let _ = prompt.response_tx.send(PermissionResponse::AllowAlways);
        assert!(pending
            .await
            .expect("preflight task should complete")
            .is_ok());

        let file_params =
            serde_json::json!({ "file_path": external.join("README.md").to_string_lossy() });
        let result = perms
            .preflight("build", "read", &file_params, Some(&tx))
            .await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn sensitive_writes_are_allowed_by_default() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/workspace/.env" });

        let write_result = perms.preflight("build", "write", &params, Some(&tx)).await;
        let edit_result = perms.preflight("build", "edit", &params, Some(&tx)).await;

        assert!(write_result.is_ok());
        assert!(edit_result.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn read_tool_prompts_for_sensitive_path() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/workspace/.env" });

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move { perms.preflight("build", "read", &params, Some(&tx)).await }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        assert_eq!(prompt.tool_id, "read");
        assert_eq!(prompt.action, PermissionAction::Read);
        assert_eq!(prompt.target.as_deref(), Some("/tmp/workspace/.env"));
        assert!(prompt.reason.contains("sensitive file"));

        let _ = prompt.response_tx.send(PermissionResponse::Deny);
        let result = pending.await.expect("preflight task should complete");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_tool_allows_env_example_by_default() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/workspace/.env.example" });

        let result = perms.preflight("build", "read", &params, Some(&tx)).await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn read_tool_prompts_for_external_path() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/elsewhere/file.txt" });

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move { perms.preflight("build", "read", &params, Some(&tx)).await }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        assert_eq!(prompt.tool_id, "read");
        assert_eq!(prompt.action, PermissionAction::Read);
        assert_eq!(prompt.target.as_deref(), Some("/tmp/elsewhere/file.txt"));
        assert!(prompt.reason.contains("outside working directory"));

        let _ = prompt.response_tx.send(PermissionResponse::Deny);
        let result = pending.await.expect("preflight task should complete");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_files_prompts_for_external_secondary_path() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({
            "files": [
                { "file_path": "/tmp/workspace/src/a.ts", "content": "a" },
                { "file_path": "/tmp/elsewhere/b.ts", "content": "b" }
            ]
        });

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move {
                perms
                    .preflight("build", "write_files", &params, Some(&tx))
                    .await
            }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        assert_eq!(prompt.tool_id, "write_files");
        assert_eq!(prompt.action, PermissionAction::Write);
        assert_eq!(prompt.target.as_deref(), Some("/tmp/elsewhere/b.ts"));
        assert!(prompt.reason.contains("outside working directory"));

        let _ = prompt.response_tx.send(PermissionResponse::Deny);
        let result = pending.await.expect("preflight task should complete");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn search_tools_prompt_for_external_paths() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let external = serde_json::json!({ "path": "/tmp/elsewhere" });

        let list_result = perms.preflight("build", "list", &external, None).await;
        let glob_result = perms.preflight("build", "glob", &external, None).await;
        let grep_result = perms.preflight("build", "grep", &external, None).await;

        assert!(list_result.is_err());
        assert!(glob_result.is_err());
        assert!(grep_result.is_err());
    }

    #[tokio::test]
    async fn bash_is_allowed_by_default_inside_workspace() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({
            "command": "pwd",
            "workdir": "/tmp/workspace",
        });

        let result = perms.preflight("build", "bash", &params, Some(&tx)).await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn configured_bash_patterns_use_last_matching_rule() {
        let perms = ToolPermissions::new("/tmp/workspace").with_permission_rules(vec![
            PermissionRule {
                permission: "bash".to_string(),
                pattern: "*".to_string(),
                action: PermissionPolicyAction::Ask,
            },
            PermissionRule {
                permission: "bash".to_string(),
                pattern: "git *".to_string(),
                action: PermissionPolicyAction::Allow,
            },
            PermissionRule {
                permission: "bash".to_string(),
                pattern: "git push *".to_string(),
                action: PermissionPolicyAction::Deny,
            },
        ]);

        let allowed = serde_json::json!({
            "command": "git status --short",
            "workdir": "/tmp/workspace",
        });
        let denied = serde_json::json!({
            "command": "git push origin main",
            "workdir": "/tmp/workspace",
        });

        assert!(perms
            .preflight("build", "bash", &allowed, None)
            .await
            .is_ok());
        assert!(perms
            .preflight("build", "bash", &denied, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn configured_ask_prompts_for_matching_tool_pattern() {
        let perms =
            ToolPermissions::new("/tmp/workspace").with_permission_rules(vec![PermissionRule {
                permission: "mcp_*".to_string(),
                pattern: "*".to_string(),
                action: PermissionPolicyAction::Ask,
            }]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({});

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move {
                perms
                    .preflight("build", "mcp_lookup", &params, Some(&tx))
                    .await
            }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        assert_eq!(prompt.tool_id, "mcp_lookup");
        assert!(prompt
            .reason
            .contains("Permission config requires approval"));

        let _ = prompt.response_tx.send(PermissionResponse::Deny);
        let result = pending.await.expect("preflight task should complete");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn agent_permission_rules_override_global_rules() {
        let mut agent_rules = HashMap::new();
        agent_rules.insert(
            "build".to_string(),
            vec![PermissionRule {
                permission: "bash".to_string(),
                pattern: "git *".to_string(),
                action: PermissionPolicyAction::Allow,
            }],
        );

        let perms = ToolPermissions::new("/tmp/workspace")
            .with_permission_rules(vec![PermissionRule {
                permission: "bash".to_string(),
                pattern: "*".to_string(),
                action: PermissionPolicyAction::Deny,
            }])
            .with_agent_permission_rules(agent_rules);
        let params = serde_json::json!({
            "command": "git status",
            "workdir": "/tmp/workspace",
        });

        assert!(perms
            .preflight("build", "bash", &params, None)
            .await
            .is_ok());
        assert!(perms
            .preflight("plan", "bash", &params, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn external_directory_allow_bypasses_default_prompt() {
        let perms =
            ToolPermissions::new("/tmp/workspace").with_permission_rules(vec![PermissionRule {
                permission: "external_directory".to_string(),
                pattern: "/tmp/elsewhere/*".to_string(),
                action: PermissionPolicyAction::Allow,
            }]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/elsewhere/file.txt" });

        let result = perms.preflight("build", "read", &params, Some(&tx)).await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn bash_external_workdir_prompt_separates_command_from_workdir() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({
            "command": "pwd",
            "workdir": "/tmp/elsewhere",
        });

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move { perms.preflight("build", "bash", &params, Some(&tx)).await }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        assert_eq!(prompt.target.as_deref(), Some("pwd"));
        assert_eq!(prompt.command.as_deref(), Some("pwd"));
        assert_eq!(prompt.workdir.as_deref(), Some("/tmp/elsewhere"));
        assert!(prompt.reason.contains("outside working directory"));

        let _ = prompt.response_tx.send(PermissionResponse::Deny);
        let _ = pending.await.expect("preflight task should complete");
    }

    #[tokio::test]
    async fn repeated_allowed_call_prompts_for_doom_loop() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({
            "command": "pwd",
            "workdir": "/tmp/workspace",
        });

        let first = perms.preflight("build", "bash", &params, Some(&tx)).await;
        let second = perms.preflight("build", "bash", &params, Some(&tx)).await;
        assert!(first.is_ok());
        assert!(second.is_ok());
        assert!(rx.try_recv().is_err());

        let pending = tokio::spawn({
            let perms = perms.clone();
            let params = params.clone();
            let tx = tx.clone();
            async move { perms.preflight("build", "bash", &params, Some(&tx)).await }
        });

        let prompt = match rx.recv().await {
            Some(ChunkMessage::PermissionRequest(prompt)) => prompt,
            _ => panic!("Expected permission prompt"),
        };

        assert_eq!(prompt.tool_id, "bash");
        assert_eq!(prompt.action, PermissionAction::Bash);
        assert_eq!(prompt.target.as_deref(), Some("pwd"));
        assert!(prompt.reason.contains("repeated the same request"));

        let _ = prompt.response_tx.send(PermissionResponse::Deny);
        let result = pending.await.expect("preflight task should complete");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn repeated_workspace_reads_do_not_prompt_for_doom_loop() {
        let perms = ToolPermissions::new("/tmp/workspace");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/workspace/src/main.rs" });

        for _ in 0..4 {
            assert!(perms
                .preflight("build", "read", &params, Some(&tx))
                .await
                .is_ok());
        }

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dangerous_skip_bypasses_permission_prompts() {
        let perms = ToolPermissions::new("/tmp/workspace").dangerously_skip_permissions(true);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/elsewhere/file.txt" });

        let result = perms.preflight("build", "read", &params, Some(&tx)).await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }
}
