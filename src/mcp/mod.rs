pub mod cli;
mod credentials;
pub mod oauth;

use crate::config::configuration::{McpConfig, McpRemoteConfig, McpServerConfig};
use crate::tools::{
    ParameterSchema, ParameterType, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, ContentBlock, JsonObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::auth::AuthorizationManager;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Process-wide MCP managers keyed by workspace. Connections warm in the
/// background so chat never blocks on process spawn / tool listing.
static MCP_POOL: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<Mutex<McpManager>>>>> =
    std::sync::OnceLock::new();

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct McpServerView {
    pub name: String,
    pub enabled: bool,
    pub status: String,
    pub kind: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpToolSpec {
    pub server: String,
    pub name: String,
    pub tool_id: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct McpManager {
    workspace: PathBuf,
    servers: BTreeMap<String, McpServerState>,
}

struct McpServerState {
    config: McpServerConfig,
    status: McpStatus,
    client: Option<RunningService<RoleClient, ()>>,
    tools: Vec<McpToolSpec>,
}

#[derive(Debug, Clone)]
enum McpStatus {
    Disabled,
    Connecting,
    Connected,
    Failed(String),
    NeedsAuth,
}

impl McpStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Failed(_) => "failed",
            Self::NeedsAuth => "needs_auth",
        }
    }

    fn detail(&self) -> Option<String> {
        match self {
            Self::Failed(msg) => Some(msg.clone()),
            Self::NeedsAuth => Some("authentication required".to_string()),
            _ => None,
        }
    }
}

impl McpManager {
    /// Get or create the shared manager for `workspace` and kick off background
    /// connects. Returns immediately — never waits on MCP servers.
    pub fn ensure(config: McpConfig, workspace: impl Into<PathBuf>) -> Arc<Mutex<Self>> {
        let workspace = workspace.into();
        let key = workspace.to_string_lossy().to_string();
        let pool = MCP_POOL.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        let mut guard = pool.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(existing) = guard.get(&key).cloned() {
            drop(guard);
            let bg = existing.clone();
            let _ = tokio::spawn(async move {
                // Mutate config under a short lock, then warm without holding it.
                {
                    let mut m = bg.lock().await;
                    m.apply_config(config);
                }
                warm_connections(bg).await;
            });
            return existing;
        }

        let mut manager = Self {
            workspace,
            servers: BTreeMap::new(),
        };
        for (name, server_config) in config {
            manager.servers.insert(
                name,
                McpServerState {
                    status: if server_config.enabled() {
                        McpStatus::Connecting
                    } else {
                        McpStatus::Disabled
                    },
                    config: server_config,
                    client: None,
                    tools: Vec::new(),
                },
            );
        }
        let manager = Arc::new(Mutex::new(manager));
        guard.insert(key, manager.clone());
        drop(guard);

        let bg = manager.clone();
        let _ = tokio::spawn(async move {
            warm_connections(bg).await;
        });
        manager
    }

    /// Fully connect (tests / callers that need tools ready). Parallel.
    pub async fn connect(config: McpConfig, workspace: impl Into<PathBuf>) -> Arc<Mutex<Self>> {
        let manager = Self::ensure(config, workspace);
        warm_connections(manager.clone()).await;
        manager
    }

    pub fn views(&self) -> Vec<McpServerView> {
        self.servers
            .iter()
            .map(|(name, state)| McpServerView {
                name: name.clone(),
                enabled: state.config.enabled(),
                status: state.status.as_str().to_string(),
                kind: state.config.kind().to_string(),
                detail: state.status.detail(),
            })
            .collect()
    }

    pub fn tools(&self) -> Vec<McpToolSpec> {
        self.servers
            .values()
            .filter(|state| state.config.enabled())
            .flat_map(|state| state.tools.clone())
            .collect()
    }

    /// Drop a server from the live tool list immediately (TUI `/mcp` toggle off).
    ///
    /// Does not wait on the background `ensure` apply_config task, which otherwise
    /// can still expose disabled schemas on the next model request.
    pub fn disable_sync(&mut self, name: &str) {
        let Some(state) = self.servers.get_mut(name) else {
            return;
        };
        state.config.set_enabled(false);
        state.client = None;
        state.tools.clear();
        state.status = McpStatus::Disabled;
    }

    /// Apply config changes under a short lock. Does not open connections —
    /// call `warm_connections` afterwards without holding the mutex.
    pub(crate) fn apply_config(&mut self, config: McpConfig) {
        let incoming: HashSet<String> = config.keys().cloned().collect();
        let stale: Vec<String> = self
            .servers
            .keys()
            .filter(|name| !incoming.contains(*name))
            .cloned()
            .collect();
        for name in stale {
            if let Some(state) = self.servers.get_mut(&name) {
                state.client = None;
                state.tools.clear();
                state.status = McpStatus::Disabled;
            }
            self.servers.remove(&name);
        }

        for (name, server_config) in config {
            match self.servers.get_mut(&name) {
                Some(state) => {
                    let was_enabled = state.config.enabled();
                    let now_enabled = server_config.enabled();
                    state.config = server_config;
                    if was_enabled && !now_enabled {
                        state.client = None;
                        state.tools.clear();
                        state.status = McpStatus::Disabled;
                    } else if !was_enabled && now_enabled {
                        state.status = McpStatus::Connecting;
                        state.client = None;
                        state.tools.clear();
                    } else if now_enabled && state.client.is_none() {
                        if !matches!(state.status, McpStatus::Connecting) {
                            state.status = McpStatus::Connecting;
                        }
                    }
                }
                None => {
                    let enabled = server_config.enabled();
                    self.servers.insert(
                        name,
                        McpServerState {
                            status: if enabled {
                                McpStatus::Connecting
                            } else {
                                McpStatus::Disabled
                            },
                            config: server_config,
                            client: None,
                            tools: Vec::new(),
                        },
                    );
                }
            }
        }
    }

    pub async fn set_enabled(&mut self, name: &str, enabled: bool) -> anyhow::Result<()> {
        let Some(mut state) = self.servers.remove(name) else {
            anyhow::bail!("mcp server not found");
        };
        state.config.set_enabled(enabled);
        if !enabled {
            state.client = None;
            state.tools.clear();
            state.status = McpStatus::Disabled;
        } else {
            state.status = McpStatus::Connecting;
            let result = open_and_list_tools(&self.workspace, name, &state.config).await;
            apply_connect_result(&mut state, result);
        }
        self.servers.insert(name.to_string(), state);
        Ok(())
    }

    pub async fn reconnect(&mut self, name: &str) -> anyhow::Result<()> {
        let Some(mut state) = self.servers.remove(name) else {
            anyhow::bail!("mcp server not found");
        };
        if !state.config.enabled() {
            self.servers.insert(name.to_string(), state);
            anyhow::bail!("mcp server is disabled");
        }
        state.client = None;
        state.tools.clear();
        state.status = McpStatus::Connecting;
        let result = open_and_list_tools(&self.workspace, name, &state.config).await;
        apply_connect_result(&mut state, result);
        self.servers.insert(name.to_string(), state);
        Ok(())
    }

    pub fn remote_config(&self, name: &str) -> Option<McpRemoteConfig> {
        match self.servers.get(name).map(|state| &state.config) {
            Some(McpServerConfig::Remote(remote)) => Some(remote.clone()),
            _ => None,
        }
    }

    pub fn status_of(&self, name: &str) -> Option<&'static str> {
        self.servers.get(name).map(|state| state.status.as_str())
    }

    async fn call_tool(
        &mut self,
        server_name: &str,
        tool_name: &str,
        params: Value,
    ) -> Result<ToolResult, ToolError> {
        let state = self
            .servers
            .get_mut(server_name)
            .ok_or_else(|| ToolError::NotFound(format!("MCP server '{server_name}' not found")))?;
        if !state.config.enabled() || !matches!(state.status, McpStatus::Connected) {
            return Err(ToolError::Execution(format!(
                "MCP server '{server_name}' is {}",
                state.status.as_str()
            )));
        }
        let client = state.client.as_ref().ok_or_else(|| {
            ToolError::Execution(format!("MCP server '{server_name}' is not connected"))
        })?;
        let mut request = CallToolRequestParams::new(tool_name.to_string());
        if let Value::Object(map) = params {
            request = request.with_arguments(JsonObject::from_iter(map));
        }
        let timeout = state_timeout(&state.config);
        let result = tokio::time::timeout(timeout, client.call_tool(request))
            .await
            .map_err(|_| {
                ToolError::Execution(format!(
                    "MCP tool timed out after {} ms",
                    timeout.as_millis()
                ))
            })?
            .map_err(|err| ToolError::Execution(err.to_string()))?;
        if result.is_error == Some(true) {
            return Err(ToolError::Execution(call_tool_result_text(&result)));
        }
        Ok(mcp_tool_result(server_name, tool_name, &result))
    }
}

fn mcp_tool_result(
    server_name: &str,
    tool_name: &str,
    result: &rmcp::model::CallToolResult,
) -> ToolResult {
    let text = call_tool_result_text(result);
    let output = if let Some(structured) = result.structured_content.as_ref() {
        let structured =
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string());
        if text.trim().is_empty() || text == structured {
            structured
        } else {
            format!("{structured}\n\n{text}")
        }
    } else {
        text
    };
    let mut tool_result = ToolResult::new(format!("MCP: {server_name}.{tool_name}"), output)
        .with_metadata(
            "mcp_result",
            serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
        );
    for content in &result.content {
        if let ContentBlock::Image(image) = content {
            tool_result = tool_result.with_image(
                format!("data:{};base64,{}", image.mime_type, image.data),
                image.mime_type.clone(),
            );
        }
    }
    tool_result
}

type ConnectOutcome = Result<(RunningService<RoleClient, ()>, Vec<McpToolSpec>), McpStatus>;

/// Warm connections without holding the manager lock during I/O.
async fn warm_connections(manager: Arc<Mutex<McpManager>>) {
    let (workspace, jobs) = {
        let mut m = manager.lock().await;
        let jobs: Vec<(String, McpServerConfig)> = m
            .servers
            .iter_mut()
            .filter(|(_, state)| {
                state.config.enabled()
                    && state.client.is_none()
                    && !matches!(state.status, McpStatus::Disabled)
            })
            .map(|(name, state)| {
                state.status = McpStatus::Connecting;
                (name.clone(), state.config.clone())
            })
            .collect();
        (m.workspace.clone(), jobs)
    };

    if jobs.is_empty() {
        return;
    }

    let results = futures::future::join_all(jobs.into_iter().map(|(name, config)| {
        let workspace = workspace.clone();
        async move {
            let result = open_and_list_tools(&workspace, &name, &config).await;
            (name, result)
        }
    }))
    .await;

    let mut m = manager.lock().await;
    for (name, result) in results {
        if let Some(state) = m.servers.get_mut(&name) {
            if !state.config.enabled() {
                state.client = None;
                state.tools.clear();
                state.status = McpStatus::Disabled;
                continue;
            }
            apply_connect_result(state, result);
            if matches!(state.status, McpStatus::NeedsAuth) {
                toast_needs_auth(&name);
            }
        }
    }
}

fn apply_connect_result(state: &mut McpServerState, result: ConnectOutcome) {
    match result {
        Ok((client, tools)) => {
            state.tools = tools;
            state.client = Some(client);
            state.status = McpStatus::Connected;
        }
        Err(status) => {
            state.client = None;
            state.tools.clear();
            state.status = status;
        }
    }
}

async fn open_and_list_tools(
    workspace: &Path,
    name: &str,
    config: &McpServerConfig,
) -> ConnectOutcome {
    let timeout = state_timeout(config);
    let client = match open_client(workspace, name, config).await {
        Ok(client) => client,
        Err(err) => {
            let msg = err.to_string();
            let oauth_enabled = matches!(
                config,
                McpServerConfig::Remote(remote) if oauth::should_use_oauth(remote)
            );
            return Err(if oauth_enabled && oauth::is_auth_error_message(&msg) {
                McpStatus::NeedsAuth
            } else {
                McpStatus::Failed(msg)
            });
        }
    };

    match tokio::time::timeout(timeout, client.list_all_tools()).await {
        Ok(Ok(tools)) => {
            let tools = tools
                .into_iter()
                .map(|tool| McpToolSpec {
                    server: name.to_string(),
                    tool_id: tool_name(name, &tool.name),
                    name: tool.name.to_string(),
                    description: tool.description.map(|d| d.to_string()).unwrap_or_default(),
                    input_schema: Value::Object(tool.input_schema.as_ref().clone()),
                })
                .collect();
            Ok((client, tools))
        }
        Ok(Err(err)) => Err(McpStatus::Failed(err.to_string())),
        Err(_) => Err(McpStatus::Failed(format!(
            "timed out after {} ms while listing tools",
            timeout.as_millis()
        ))),
    }
}

async fn open_client(
    workspace: &Path,
    name: &str,
    config: &McpServerConfig,
) -> anyhow::Result<RunningService<RoleClient, ()>> {
    match config {
        McpServerConfig::Local(local) => {
            let command = local.command.first().cloned().unwrap_or_default();
            let args = local.command.iter().skip(1).cloned().collect::<Vec<_>>();
            let cwd = local
                .cwd
                .as_deref()
                .map(|cwd| resolve_path(workspace, cwd))
                .unwrap_or_else(|| workspace.to_path_buf());
            let env = local.environment.clone();
            let (transport, _) = TokioChildProcess::builder(
                tokio::process::Command::new(command).configure(move |cmd| {
                    cmd.args(args);
                    cmd.current_dir(cwd);
                    cmd.envs(env);
                }),
            )
            .stderr(Stdio::null())
            .spawn()?;
            Ok(().serve(transport).await?)
        }
        McpServerConfig::Remote(remote) => open_remote_client(name, remote).await,
    }
}

async fn open_remote_client(
    name: &str,
    remote: &McpRemoteConfig,
) -> anyhow::Result<RunningService<RoleClient, ()>> {
    let headers = remote_headers(remote)?;
    let mut transport_config =
        rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
            remote.url.clone(),
        )
        .custom_headers(headers);

    if oauth::should_use_oauth(remote) && credentials::has_credentials(name, &remote.url) {
        let mut manager = AuthorizationManager::new(remote.url.as_str())
            .await
            .map_err(|e| anyhow::anyhow!("failed to start OAuth client: {e}"))?;
        manager.set_credential_store(credentials::FileCredentialStore::new(
            name.to_string(),
            remote.url.clone(),
        ));
        let hydrated = manager
            .initialize_from_store()
            .await
            .map_err(|e| anyhow::anyhow!("failed to load MCP OAuth credentials: {e}"))?;
        if hydrated {
            let token = manager
                .get_access_token()
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            transport_config = transport_config.auth_header(token);
        }
    }

    let transport = StreamableHttpClientTransport::from_config(transport_config);
    Ok(().serve(transport).await?)
}

fn remote_headers(remote: &McpRemoteConfig) -> anyhow::Result<HashMap<HeaderName, HeaderValue>> {
    let mut headers = HashMap::new();
    for (key, value) in &remote.headers {
        if oauth::should_use_oauth(remote) && key.eq_ignore_ascii_case("authorization") {
            continue;
        }
        let name = HeaderName::from_bytes(key.as_bytes())?;
        let value = HeaderValue::from_str(value)?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn toast_needs_auth(name: &str) {
    static TOASTED: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    let toasted = TOASTED.get_or_init(|| std::sync::Mutex::new(HashSet::new()));
    let mut guard = toasted.lock().unwrap_or_else(|e| e.into_inner());
    if !guard.insert(name.to_string()) {
        return;
    }
    crate::push_toast(crate::toast::Toast::new(
        format!("MCP \"{name}\" needs authentication. Run: crabcode mcp auth {name}"),
        crate::toast::ToastLevel::Warning,
        Some(Duration::from_secs(8)),
    ));
}

fn state_timeout(config: &McpServerConfig) -> Duration {
    let ms = match config {
        McpServerConfig::Local(local) => local.timeout_ms,
        McpServerConfig::Remote(remote) => remote.timeout_ms,
    }
    .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(ms)
}

fn resolve_path(base: &Path, path: &str) -> PathBuf {
    let path = shellexpand_home(path);
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn shellexpand_home(path: &str) -> String {
    if path == "~" {
        return dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn tool_name(server: &str, name: &str) -> String {
    format!("{}_{}", sanitize(server), sanitize(name))
}

fn call_tool_result_text(result: &rmcp::model::CallToolResult) -> String {
    let text = result
        .content
        .iter()
        .filter_map(content_text)
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        serde_json::to_string(result)
            .unwrap_or_else(|_| "MCP tool returned no text content".to_string())
    } else {
        text
    }
}

fn content_text(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        ContentBlock::Image(image) => Some(format!(
            "[image: {} bytes, {}]",
            image.data.len(),
            image.mime_type
        )),
        ContentBlock::Audio(audio) => Some(format!(
            "[audio: {} bytes, {}]",
            audio.data.len(),
            audio.mime_type
        )),
        ContentBlock::Resource(resource) => {
            let uri = match &resource.resource {
                rmcp::model::ResourceContents::TextResourceContents { uri, .. }
                | rmcp::model::ResourceContents::BlobResourceContents { uri, .. } => uri,
                _ => "unknown",
            };
            Some(format!("[resource: {uri}]"))
        }
        ContentBlock::ResourceLink(resource) => Some(format!("[resource: {}]", resource.uri)),
        _ => Some("[unsupported MCP content]".to_string()),
    }
}

#[derive(Clone)]
pub struct McpToolHandler {
    manager: Arc<Mutex<McpManager>>,
    spec: McpToolSpec,
}

impl McpToolHandler {
    pub fn new(manager: Arc<Mutex<McpManager>>, spec: McpToolSpec) -> Self {
        Self { manager, spec }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn definition(&self) -> Tool {
        Tool {
            id: self.spec.tool_id.clone(),
            description: self.spec.description.clone(),
            parameters: parameters_from_schema(&self.spec.input_schema),
            input_schema: Some(normalize_input_schema(&self.spec.input_schema)),
        }
    }

    fn mcp_server(&self) -> Option<&str> {
        Some(self.spec.server.as_str())
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        if params.is_object() {
            Ok(())
        } else {
            Err(ToolError::Validation(
                "MCP tool input must be an object".to_string(),
            ))
        }
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        self.manager
            .lock()
            .await
            .call_tool(&self.spec.server, &self.spec.name, params)
            .await
    }
}

fn normalize_input_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Value::Object(map) = &mut schema {
        // OpenAI/xAI Responses require tool `parameters` to be a plain object schema.
        // MCP servers (e.g. cua-driver `browser_prepare`) sometimes emit root anyOf/oneOf
        // with non-object branches; strip those before we send the schema upstream.
        flatten_root_non_object_unions(map);

        map.entry("type".to_string())
            .or_insert_with(|| Value::String("object".to_string()));
        map.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Default::default()));
        map.entry("additionalProperties".to_string())
            .or_insert_with(|| Value::Bool(false));
    }
    schema
}

fn branch_is_typed_object(branch: &Value) -> bool {
    let Some(obj) = branch.as_object() else {
        return false;
    };
    match obj.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
        _ => false,
    }
}

fn flatten_root_non_object_unions(map: &mut serde_json::Map<String, Value>) {
    for key in ["anyOf", "oneOf"] {
        let Some(Value::Array(branches)) = map.get(key).cloned() else {
            continue;
        };
        if !branches
            .iter()
            .any(|branch| !branch_is_typed_object(branch))
        {
            continue;
        }

        let root_is_object = map
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t == "object")
            || map.contains_key("properties");
        if !root_is_object {
            if let Some(Value::Object(branch)) = branches
                .into_iter()
                .find(|branch| branch_is_typed_object(branch))
            {
                for (k, v) in branch {
                    map.entry(k).or_insert(v);
                }
            }
        }

        map.remove(key);
    }
}

fn parameters_from_schema(schema: &Value) -> Vec<ParameterSchema> {
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, schema)| ParameterSchema {
            name: name.clone(),
            description: schema
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            required: required.iter().any(|item| item == name),
            param_type: parameter_type_from_schema(schema),
        })
        .collect()
}

fn parameter_type_from_schema(schema: &Value) -> ParameterType {
    match schema.get("type").and_then(Value::as_str) {
        Some("integer") => ParameterType::Integer,
        Some("number") => ParameterType::String,
        Some("boolean") => ParameterType::Boolean,
        Some("array") => ParameterType::Array(Box::new(
            schema
                .get("items")
                .map(parameter_type_from_schema)
                .unwrap_or(ParameterType::String),
        )),
        Some("object") => ParameterType::Object(HashMap::new()),
        _ => ParameterType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_result_preserves_images_resources_annotations_and_structured_content() {
        let resource = rmcp::model::Resource::new("file:///tmp/readme.md", "readme")
            .with_mime_type("text/markdown");
        let mut result = rmcp::model::CallToolResult::success(vec![
            ContentBlock::Image(
                rmcp::model::ImageContent::new("aGk=", "image/png")
                    .with_annotations(rmcp::model::Annotations::default().with_priority(0.8)),
            ),
            ContentBlock::ResourceLink(resource),
        ]);
        result.structured_content = Some(json!({"answer": 42}));

        let converted = mcp_tool_result("docs", "lookup", &result);
        assert_eq!(converted.images.len(), 1);
        assert_eq!(converted.images[0].media_type, "image/png");
        assert_eq!(
            converted.metadata["mcp_result"]["structuredContent"]["answer"],
            42
        );
        let priority = converted.metadata["mcp_result"]["content"][0]["annotations"]["priority"]
            .as_f64()
            .unwrap();
        assert!((priority - 0.8).abs() < 0.000_001);
        assert_eq!(
            converted.metadata["mcp_result"]["content"][1]["uri"],
            "file:///tmp/readme.md"
        );
    }

    #[test]
    fn normalize_strips_root_anyof_with_non_object_branches() {
        // Mirrors cua-driver `browser_prepare`: object root + anyOf of required-only
        // branches that omit `type: object`. xAI Responses rejects that shape.
        let input = json!({
            "type": "object",
            "properties": {
                "pid": { "type": "integer" },
                "allow_launch": { "type": "boolean" }
            },
            "required": [],
            "additionalProperties": true,
            "anyOf": [
                { "required": ["pid"] },
                {
                    "properties": {
                        "allow_launch": { "const": true }
                    },
                    "required": ["allow_launch"]
                }
            ]
        });

        let normalized = normalize_input_schema(&input);
        assert_eq!(
            normalized.get("type").and_then(Value::as_str),
            Some("object")
        );
        assert!(normalized.get("anyOf").is_none());
        assert!(normalized.get("oneOf").is_none());
        assert!(normalized
            .get("properties")
            .and_then(Value::as_object)
            .is_some());
        assert_eq!(normalized.get("additionalProperties"), Some(&json!(true)));
    }

    #[test]
    fn normalize_keeps_object_only_anyof_branches() {
        let input = json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "a": { "type": "string" } },
                    "required": ["a"]
                },
                {
                    "type": "object",
                    "properties": { "b": { "type": "integer" } },
                    "required": ["b"]
                }
            ]
        });

        let normalized = normalize_input_schema(&input);
        assert!(normalized.get("anyOf").is_some());
        assert_eq!(
            normalized.get("type").and_then(Value::as_str),
            Some("object")
        );
    }

    #[test]
    fn normalize_promotes_sole_object_branch_when_root_lacks_object() {
        let input = json!({
            "anyOf": [
                { "type": "null" },
                {
                    "type": "object",
                    "properties": { "x": { "type": "string" } },
                    "required": ["x"]
                }
            ]
        });

        let normalized = normalize_input_schema(&input);
        assert!(normalized.get("anyOf").is_none());
        assert_eq!(
            normalized.get("type").and_then(Value::as_str),
            Some("object")
        );
        assert!(normalized
            .pointer("/properties/x")
            .is_some_and(|v| v.get("type").and_then(Value::as_str) == Some("string")));
    }

    fn remote_server(enabled: bool) -> McpServerConfig {
        McpServerConfig::Remote(McpRemoteConfig {
            url: "https://example.test/mcp".to_string(),
            headers: HashMap::new(),
            enabled,
            timeout_ms: None,
            oauth_enabled: false,
            oauth_client_id: None,
            oauth_client_secret: None,
            oauth_scope: None,
        })
    }

    fn manager_with_tool(server: &str, enabled: bool, tool_id: &str) -> McpManager {
        let mut servers = BTreeMap::new();
        servers.insert(
            server.to_string(),
            McpServerState {
                config: remote_server(enabled),
                status: if enabled {
                    McpStatus::Connected
                } else {
                    McpStatus::Disabled
                },
                client: None,
                tools: vec![McpToolSpec {
                    server: server.to_string(),
                    name: "browser".to_string(),
                    tool_id: tool_id.to_string(),
                    description: "drive a browser".to_string(),
                    input_schema: json!({"type": "object", "properties": {}}),
                }],
            },
        );
        McpManager {
            workspace: PathBuf::from("."),
            servers,
        }
    }

    #[test]
    fn tools_omits_disabled_servers_even_if_specs_linger() {
        let manager = manager_with_tool("cua-driver", false, "cua-driver_browser");
        assert!(manager.tools().is_empty());
    }

    #[test]
    fn apply_config_clears_disabled_server_tools() {
        let mut manager = manager_with_tool("cua-driver", true, "cua-driver_browser");
        assert_eq!(manager.tools().len(), 1);

        let mut config = McpConfig::new();
        config.insert("cua-driver".to_string(), remote_server(false));
        manager.apply_config(config);

        assert!(manager.tools().is_empty());
        assert_eq!(manager.status_of("cua-driver"), Some("disabled"));
    }

    #[test]
    fn disable_sync_drops_tools_immediately() {
        let mut manager = manager_with_tool("doop", true, "doop_generate");
        assert_eq!(manager.tools().len(), 1);
        manager.disable_sync("doop");
        assert!(manager.tools().is_empty());
        assert_eq!(manager.status_of("doop"), Some("disabled"));
    }
}
