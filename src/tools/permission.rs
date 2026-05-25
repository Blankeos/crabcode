use crate::llm::{ChunkMessage, ChunkSender};
use crate::tools::ToolError;
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

impl PermissionAction {
    pub fn from_tool_id(tool_id: &str) -> Self {
        match tool_id {
            "read" | "view_image" => Self::Read,
            "write" => Self::Write,
            "edit" => Self::Edit,
            "list" => Self::List,
            "glob" => Self::Glob,
            "grep" => Self::Grep,
            "bash" => Self::Bash,
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

#[derive(Debug)]
pub struct PermissionPrompt {
    pub tool_id: String,
    pub action: PermissionAction,
    pub target: Option<String>,
    pub command: Option<String>,
    pub workdir: Option<String>,
    pub reason: String,
    pub response_tx: tokio::sync::oneshot::Sender<PermissionResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PermissionReasonKind {
    SensitivePath,
    ExternalPath,
    DoomLoop,
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
            // OpenCode plan mode denies file modifications, but keeps other
            // tools available under the normal permission policy.
            return !matches!(tool.as_str(), "write" | "edit");
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
    call_counts: Arc<RwLock<HashMap<ToolCallFingerprint, usize>>>,
    agent_policies: Arc<AgentToolPolicies>,
    dangerously_skip_permissions: bool,
}

impl ToolPermissions {
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: normalize_path(&workdir.into()),
            always_grants: Arc::new(RwLock::new(HashSet::new())),
            call_counts: Arc::new(RwLock::new(HashMap::new())),
            agent_policies: Arc::new(AgentToolPolicies::default()),
            dangerously_skip_permissions: false,
        }
    }

    pub fn with_agent_policies(mut self, policies: AgentToolPolicies) -> Self {
        self.agent_policies = Arc::new(policies);
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

        if self.dangerously_skip_permissions {
            return Ok(());
        }

        let action = PermissionAction::from_tool_id(tool_id);
        let path = extract_primary_path(action, params, &self.workdir);
        let command = if action == PermissionAction::Bash {
            get_string(params, "command").map(|s| s.trim().to_string())
        } else {
            None
        };

        let reason = self.evaluate_reason(action, path.as_deref());
        let reason = match reason {
            Some(reason) => Some(reason),
            None => self.evaluate_doom_loop(tool_id, params).await,
        };

        let Some(reason_kind) = reason else {
            return Ok(());
        };

        let target = path
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| command.clone());
        let prompt_target = if action == PermissionAction::Bash {
            command.clone().or_else(|| target.clone())
        } else {
            target.clone()
        };
        let workdir = if action == PermissionAction::Bash {
            path.as_ref().map(|p| p.display().to_string())
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

        if self.always_grants.read().await.contains(&fingerprint) {
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
                Ok(())
            }
        }
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
    }
}

fn get_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
        assert!(policies.is_allowed("plan", "bash"));
        assert!(!policies.is_allowed("plan", "write"));
        assert!(!policies.is_allowed("plan", "edit"));
    }

    #[test]
    fn sensitive_path_detection_matches_env_patterns() {
        assert!(is_sensitive_path(Path::new(".env")));
        assert!(is_sensitive_path(Path::new(".env.local")));
        assert!(is_sensitive_path(Path::new(".env.production")));
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
    async fn dangerous_skip_bypasses_permission_prompts() {
        let perms = ToolPermissions::new("/tmp/workspace").dangerously_skip_permissions(true);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let params = serde_json::json!({ "file_path": "/tmp/elsewhere/file.txt" });

        let result = perms.preflight("build", "read", &params, Some(&tx)).await;

        assert!(result.is_ok());
        assert!(rx.try_recv().is_err());
    }
}
