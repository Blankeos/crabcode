use crate::config::{McpConfig, McpServerConfig};
use crate::tools::{
    ParameterSchema, ParameterType, Tool, ToolContext, ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, ContentBlock, JsonObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct McpServerView {
    pub name: String,
    pub enabled: bool,
    pub status: String,
    pub kind: String,
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
    Connected,
    Failed(String),
    NeedsAuth,
}

impl McpStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Connected => "connected",
            Self::Failed(_) => "failed",
            Self::NeedsAuth => "needs_auth",
        }
    }
}

impl McpManager {
    pub async fn connect(config: McpConfig, workspace: impl Into<PathBuf>) -> Arc<Mutex<Self>> {
        let workspace = workspace.into();
        let mut manager = Self {
            workspace,
            servers: BTreeMap::new(),
        };

        for (name, server_config) in config {
            let mut state = McpServerState {
                status: if server_config.enabled() {
                    McpStatus::Failed("not connected".to_string())
                } else {
                    McpStatus::Disabled
                },
                config: server_config,
                client: None,
                tools: Vec::new(),
            };
            if state.config.enabled() {
                manager.connect_one(&name, &mut state).await;
            }
            manager.servers.insert(name, state);
        }

        Arc::new(Mutex::new(manager))
    }

    pub fn views(&self) -> Vec<McpServerView> {
        self.servers
            .iter()
            .map(|(name, state)| McpServerView {
                name: name.clone(),
                enabled: state.config.enabled(),
                status: state.status.as_str().to_string(),
                kind: state.config.kind().to_string(),
            })
            .collect()
    }

    pub fn tools(&self) -> Vec<McpToolSpec> {
        self.servers
            .values()
            .flat_map(|state| state.tools.clone())
            .collect()
    }

    pub async fn set_enabled(&mut self, name: &str, enabled: bool) -> anyhow::Result<()> {
        let Some(mut state) = self.servers.remove(name) else {
            anyhow::bail!("mcp server not found");
        };
        state.config.set_enabled(enabled);
        if enabled {
            self.connect_one(name, &mut state).await;
        } else {
            state.tools.clear();
            state.status = McpStatus::Disabled;
            if let Some(mut client) = state.client.take() {
                let _ = client.close_with_timeout(Duration::from_millis(500)).await;
            }
        }
        self.servers.insert(name.to_string(), state);
        Ok(())
    }

    async fn connect_one(&self, name: &str, state: &mut McpServerState) {
        let timeout = state_timeout(&state.config);
        match self.open_client(&state.config).await {
            Ok(client) => match tokio::time::timeout(timeout, client.list_all_tools()).await {
                Ok(Ok(tools)) => {
                    state.tools = tools
                        .into_iter()
                        .map(|tool| McpToolSpec {
                            server: name.to_string(),
                            tool_id: tool_name(name, &tool.name),
                            name: tool.name.to_string(),
                            description: tool
                                .description
                                .map(|d| d.to_string())
                                .unwrap_or_default(),
                            input_schema: Value::Object(tool.input_schema.as_ref().clone()),
                        })
                        .collect();
                    state.client = Some(client);
                    state.status = McpStatus::Connected;
                }
                Ok(Err(err)) => {
                    state.status = McpStatus::Failed(err.to_string());
                }
                Err(_) => {
                    state.status = McpStatus::Failed(format!(
                        "timed out after {} ms while listing tools",
                        timeout.as_millis()
                    ));
                }
            },
            Err(err) => {
                let msg = err.to_string();
                state.status = if msg.to_ascii_lowercase().contains("auth")
                    || msg.to_ascii_lowercase().contains("401")
                    || msg.to_ascii_lowercase().contains("unauthorized")
                {
                    McpStatus::NeedsAuth
                } else {
                    McpStatus::Failed(msg)
                };
            }
        }
    }

    async fn open_client(
        &self,
        config: &McpServerConfig,
    ) -> anyhow::Result<RunningService<RoleClient, ()>> {
        match config {
            McpServerConfig::Local(local) => {
                let command = local.command.first().cloned().unwrap_or_default();
                let args = local.command.iter().skip(1).cloned().collect::<Vec<_>>();
                let cwd = local
                    .cwd
                    .as_deref()
                    .map(|cwd| resolve_path(&self.workspace, cwd))
                    .unwrap_or_else(|| self.workspace.clone());
                let env = local.environment.clone();
                let transport = TokioChildProcess::new(
                    tokio::process::Command::new(command).configure(move |cmd| {
                        cmd.args(args);
                        cmd.current_dir(cwd);
                        cmd.envs(env);
                    }),
                )?;
                Ok(().serve(transport).await?)
            }
            McpServerConfig::Remote(remote) => {
                let mut headers = HashMap::new();
                for (key, value) in &remote.headers {
                    let name = HeaderName::from_bytes(key.as_bytes())?;
                    let value = HeaderValue::from_str(value)?;
                    headers.insert(name, value);
                }
                let transport = StreamableHttpClientTransport::from_config(
                    rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                        remote.url.clone(),
                    )
                    .custom_headers(headers),
                );
                Ok(().serve(transport).await?)
            }
        }
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
        let output = if let Some(structured) = result.structured_content {
            serde_json::to_string_pretty(&structured).unwrap_or_else(|_| structured.to_string())
        } else {
            call_tool_result_text(&result)
        };
        Ok(ToolResult::new(
            format!("MCP: {server_name}.{tool_name}"),
            output,
        ))
    }
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
        map.entry("type".to_string())
            .or_insert_with(|| Value::String("object".to_string()));
        map.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Default::default()));
        map.entry("additionalProperties".to_string())
            .or_insert_with(|| Value::Bool(false));
    }
    schema
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
