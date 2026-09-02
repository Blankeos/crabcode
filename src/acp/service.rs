use crate::config::configuration::LoadedConfig;
use crate::session::manager::SessionManager;
use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, ContentBlock, ContentChunk,
    Cost as AcpCost, CreateElicitationRequest, CreateTerminalRequest, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EmbeddedResourceResource, EnumOption, KillTerminalRequest, ListSessionsResponse,
    LoadSessionResponse, McpServer, MultiSelectPropertySchema, NewSessionResponse,
    PermissionOption, PermissionOptionKind, PromptResponse, ReleaseTerminalRequest,
    RequestPermissionOutcome, RequestPermissionRequest, ResumeSessionResponse, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectGroup, SessionConfigSelectOption, SessionInfo,
    SessionMode, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionResponse, StopReason, StringPropertySchema, Terminal,
    TerminalOutputRequest, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, UnstructuredCommandInput, UsageUpdate,
    WaitForTerminalExitRequest,
};
use agent_client_protocol::{Client, ConnectionTo, Error};
use base64::Engine as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AcpService {
    sessions: Arc<AsyncMutex<HashMap<String, AcpSession>>>,
    session_manager: Arc<Mutex<SessionManager>>,
    client_capabilities: Arc<Mutex<agent_client_protocol::schema::v1::ClientCapabilities>>,
}

fn write_prompt_audio(
    session_id: &str,
    audio: &agent_client_protocol::schema::v1::AudioContent,
) -> Result<String, Error> {
    const MAX_AUDIO_BYTES: usize = 20 * 1024 * 1024;
    let media_type = audio.mime_type.trim().to_ascii_lowercase();
    let extension = match media_type.as_str() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        _ => {
            return Err(Error::invalid_params()
                .data(format!("unsupported audio MIME type: {}", audio.mime_type)));
        }
    };
    let data = base64::engine::general_purpose::STANDARD
        .decode(audio.data.trim())
        .map_err(|error| Error::invalid_params().data(format!("invalid audio data: {error}")))?;
    if data.len() > MAX_AUDIO_BYTES {
        return Err(Error::invalid_params().data("audio exceeds the 20 MiB size limit"));
    }
    let path = crate::persistence::attachments::write(session_id, extension, &data)
        .map_err(|_| internal_error())?;
    Ok(path.to_string_lossy().into_owned())
}

fn model_supports_audio(config: &LoadedConfig, provider: &str, model: &str) -> bool {
    crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok()
    .is_some_and(|discovery| discovery.model_supports_input_modality(provider, model, "audio"))
}

struct ManagedAttachmentGuard {
    paths: Vec<String>,
    committed: bool,
}

impl ManagedAttachmentGuard {
    fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ManagedAttachmentGuard {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.paths {
                crate::persistence::attachments::remove_file(Path::new(path));
            }
        }
    }
}

struct AcpQuestionField {
    selection: String,
    custom: String,
    labels: HashMap<String, String>,
    multiple: bool,
}

struct AcpQuestionForm {
    request: CreateElicitationRequest,
    fields: Vec<AcpQuestionField>,
}

fn skipped_question_answers(questions: &serde_json::Value) -> serde_json::Value {
    let count = questions.as_array().map_or(1, Vec::len);
    serde_json::Value::Array(
        (0..count)
            .map(|_| serde_json::Value::Array(Vec::new()))
            .collect(),
    )
}

fn question_text(question: &serde_json::Value, key: &str, fallback: &str) -> String {
    question
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn acp_question_form(
    session_id: &str,
    tool_call_id: Option<&str>,
    questions: &serde_json::Value,
) -> AcpQuestionForm {
    let question_items = questions
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![questions.clone()]);
    let mut schema = ElicitationSchema::new()
        .title("Agent questions")
        .description("Answer any fields you want; blank fields are treated as skipped.");
    let mut fields = Vec::with_capacity(question_items.len());

    for (question_index, question) in question_items.iter().enumerate() {
        let selection = format!("question_{question_index}");
        let custom = format!("question_{question_index}_custom");
        let prompt = question_text(question, "question", "Question");
        let header = question_text(
            question,
            "header",
            &format!("Question {}", question_index + 1),
        );
        let mut labels = HashMap::new();
        let options = question
            .get("options")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(option_index, option)| {
                let label = option
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| option.as_str())?
                    .trim();
                if label.is_empty() {
                    return None;
                }

                let value = format!("q{question_index}_option_{option_index}");
                labels.insert(value.clone(), label.to_string());
                let description = option
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let mut option = EnumOption::new(value, label);
                if let Some(description) = description {
                    option = option.description(description);
                }
                Some(option)
            })
            .collect::<Vec<_>>();
        let multiple = question
            .get("multiple")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if multiple {
            schema = schema.property(
                selection.clone(),
                MultiSelectPropertySchema::titled(options)
                    .title(header.clone())
                    .description(prompt.clone()),
                false,
            );
        } else {
            schema = schema.property(
                selection.clone(),
                StringPropertySchema::new()
                    .title(header.clone())
                    .description(prompt.clone())
                    .one_of(options),
                false,
            );
        }
        schema = schema.property(
            custom.clone(),
            StringPropertySchema::new()
                .title(format!("{header}: custom answer"))
                .description("Optional free-form answer."),
            false,
        );
        fields.push(AcpQuestionField {
            selection,
            custom,
            labels,
            multiple,
        });
    }

    let scope = ElicitationSessionScope::new(session_id.to_string())
        .tool_call_id(tool_call_id.map(agent_client_protocol::schema::v1::ToolCallId::new));
    let request = CreateElicitationRequest::new(
        ElicitationFormMode::new(scope, schema),
        "The agent needs additional input to continue.",
    );
    AcpQuestionForm { request, fields }
}

fn acp_question_answers(
    fields: &[AcpQuestionField],
    action: ElicitationAction,
) -> serde_json::Value {
    let ElicitationAction::Accept(accepted) = action else {
        return serde_json::Value::Array(
            fields
                .iter()
                .map(|_| serde_json::Value::Array(Vec::new()))
                .collect(),
        );
    };
    let content = accepted.content.unwrap_or_default();
    serde_json::Value::Array(
        fields
            .iter()
            .map(|field| {
                let mut answers = Vec::new();
                match content.get(&field.selection) {
                    Some(ElicitationContentValue::String(value)) => {
                        if let Some(label) = field.labels.get(value) {
                            answers.push(serde_json::Value::String(label.clone()));
                        }
                    }
                    Some(ElicitationContentValue::StringArray(values)) => {
                        answers.extend(values.iter().filter_map(|value| {
                            field
                                .labels
                                .get(value)
                                .cloned()
                                .map(serde_json::Value::String)
                        }));
                    }
                    _ => {}
                }
                if let Some(ElicitationContentValue::String(custom)) = content.get(&field.custom) {
                    let custom = custom.trim();
                    if !custom.is_empty() {
                        if !field.multiple {
                            answers.clear();
                        }
                        answers.push(serde_json::Value::String(custom.to_string()));
                    }
                }
                serde_json::Value::Array(answers)
            })
            .collect(),
    )
}

fn compact_command(prompt: &str) -> Result<bool, Error> {
    let trimmed = prompt.trim();
    let Some(command_line) = trimmed.strip_prefix('/') else {
        return Ok(false);
    };
    let (name, args) = command_line
        .split_once(char::is_whitespace)
        .map(|(name, args)| (name, args.trim()))
        .unwrap_or((command_line, ""));
    if name != "compact" {
        return Ok(false);
    }
    if !args.is_empty() {
        return Err(Error::invalid_params().data("Usage: /compact"));
    }
    Ok(true)
}

fn permission_tool_call_id(tool_call_id: Option<&str>) -> String {
    tool_call_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("permission:{}", cuid2::create_id()))
}

fn resolved_reasoning(
    session: &AcpSession,
    requested: crate::model::reasoning::ReasoningEffort,
) -> Option<crate::model::reasoning::ReasoningEffort> {
    model_reasoning_capability(
        &session.config,
        &session.models,
        &session.provider,
        &session.model,
    )
    .and_then(|capability| capability.resolve(Some(requested)))
}

fn reasoning_effort_label(effort: crate::model::reasoning::ReasoningEffort) -> &'static str {
    match effort {
        crate::model::reasoning::ReasoningEffort::None => "None",
        crate::model::reasoning::ReasoningEffort::Minimal => "Minimal",
        crate::model::reasoning::ReasoningEffort::Low => "Low",
        crate::model::reasoning::ReasoningEffort::Medium => "Medium",
        crate::model::reasoning::ReasoningEffort::High => "High",
        crate::model::reasoning::ReasoningEffort::XHigh => "Xhigh",
        crate::model::reasoning::ReasoningEffort::Max => "Max",
    }
}

fn available_commands(session: &AcpSession) -> Vec<AvailableCommand> {
    let mut commands: Vec<_> = session
        .config
        .merged_config
        .commands
        .iter()
        .filter(|command| command.name != "compact")
        .map(|command| {
            let description = command
                .description
                .clone()
                .unwrap_or_else(|| format!("Run /{}", command.name));
            let mut available = AvailableCommand::new(command.name.clone(), description);
            if command.template.contains("$ARGUMENTS") {
                available = available.input(AvailableCommandInput::Unstructured(
                    UnstructuredCommandInput::new("Arguments"),
                ));
            }
            available
        })
        .collect();
    commands.extend(
        session
            .skills
            .all()
            .into_iter()
            .filter(|skill| skill.name != "compact")
            .map(|skill| {
                AvailableCommand::new(
                    skill.name.clone(),
                    skill
                        .description
                        .clone()
                        .unwrap_or_else(|| format!("Use the {} skill", skill.name)),
                )
                .input(AvailableCommandInput::Unstructured(
                    UnstructuredCommandInput::new("Task or context for this skill"),
                ))
            }),
    );
    commands.push(AvailableCommand::new(
        "skills",
        "List skills available in this workspace",
    ));
    commands.push(AvailableCommand::new(
        "mcp",
        "List configured MCP servers and their status",
    ));
    commands.push(AvailableCommand::new(
        "compact",
        "Summarize this session to reduce context",
    ));
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    commands.dedup_by(|left, right| left.name == right.name);
    commands
}

async fn expand_slash_command(session: &AcpSession, prompt: &str) -> Result<String, Error> {
    let Some(command_line) = prompt.strip_prefix('/') else {
        return Ok(prompt.to_string());
    };
    let (name, args) = command_line
        .split_once(char::is_whitespace)
        .map(|(name, args)| (name, args.trim_start()))
        .unwrap_or((command_line, ""));
    if let Some(command) = session
        .config
        .merged_config
        .commands
        .iter()
        .find(|command| command.name == name)
    {
        return command
            .render(args)
            .await
            .map(|rendered| rendered.prompt)
            .map_err(|_| internal_error());
    }
    if let Some(skill) = session.skills.get(name) {
        let mut expanded = skill.content.clone();
        if !args.is_empty() {
            expanded.push_str("\n\nUser task/context:\n");
            expanded.push_str(args);
        }
        return Ok(expanded);
    }
    if name == "skills" {
        let skills = session
            .skills
            .all()
            .into_iter()
            .map(|skill| {
                format!(
                    "- /{} — {}",
                    skill.name,
                    skill.description.as_deref().unwrap_or("No description")
                )
            })
            .collect::<Vec<_>>();
        return Ok(if skills.is_empty() {
            "No skills are available in this workspace.".to_string()
        } else {
            format!("Available workspace skills:\n{}", skills.join("\n"))
        });
    }
    if name == "mcp" {
        let servers = session
            .config
            .merged_config
            .mcp
            .iter()
            .map(|(name, server)| {
                format!(
                    "- {} ({}, {})",
                    name,
                    server.kind(),
                    if server.enabled() {
                        "enabled"
                    } else {
                        "disabled"
                    }
                )
            })
            .collect::<Vec<_>>();
        return Ok(if servers.is_empty() {
            "No MCP servers are configured for this workspace.".to_string()
        } else {
            format!("Configured MCP servers:\n{}", servers.join("\n"))
        });
    }
    Ok(prompt.to_string())
}

fn merge_acp_mcp_servers(config: &mut LoadedConfig, servers: Vec<McpServer>) {
    for server in servers {
        let (name, server) = match server {
            McpServer::Stdio(server) => {
                let mut command = vec![server.command.to_string_lossy().into_owned()];
                command.extend(server.args);
                (
                    server.name,
                    crate::config::configuration::McpServerConfig::Local(
                        crate::config::configuration::McpLocalConfig {
                            command,
                            cwd: None,
                            environment: server
                                .env
                                .into_iter()
                                .map(|variable| (variable.name, variable.value))
                                .collect(),
                            enabled: true,
                            timeout_ms: None,
                        },
                    ),
                )
            }
            McpServer::Http(server) => (
                server.name,
                crate::config::configuration::McpServerConfig::Remote(
                    crate::config::configuration::McpRemoteConfig {
                        url: server.url,
                        headers: server
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        enabled: true,
                        timeout_ms: None,
                        oauth_enabled: false,
                        oauth_client_id: None,
                        oauth_client_secret: None,
                        oauth_scope: None,
                    },
                ),
            ),
            McpServer::Sse(server) => (
                server.name,
                crate::config::configuration::McpServerConfig::Remote(
                    crate::config::configuration::McpRemoteConfig {
                        url: server.url,
                        headers: server
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        enabled: true,
                        timeout_ms: None,
                        oauth_enabled: false,
                        oauth_client_id: None,
                        oauth_client_secret: None,
                        oauth_scope: None,
                    },
                ),
            ),
            #[allow(unreachable_patterns)]
            _ => continue,
        };
        config.merged_config.mcp.insert(name, server);
    }
}

#[derive(Clone)]
struct AcpSession {
    cwd: PathBuf,
    config: LoadedConfig,
    skills: crate::skill::SkillStore,
    models: Vec<crate::model::types::Model>,
    provider: String,
    model: String,
    agent: String,
    reasoning_selection: crate::model::reasoning::ReasoningEffort,
    reasoning: Option<crate::model::reasoning::ReasoningEffort>,
    context_window: Option<u32>,
    cancellation: Option<CancellationToken>,
}

impl AcpService {
    pub fn new(initial_workspace: &Path) -> Result<Self, Error> {
        let session_manager = SessionManager::new()
            .with_history_for_workspace(initial_workspace)
            .map_err(|_| internal_error())?;
        Ok(Self {
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
            session_manager: Arc::new(Mutex::new(session_manager)),
            client_capabilities: Arc::new(Mutex::new(Default::default())),
        })
    }

    pub fn set_client_capabilities(
        &self,
        capabilities: agent_client_protocol::schema::v1::ClientCapabilities,
    ) {
        if let Ok(mut current) = self.client_capabilities.lock() {
            *current = capabilities;
        }
    }

    fn supports_form_elicitation(&self) -> bool {
        self.client_capabilities
            .lock()
            .ok()
            .and_then(|capabilities| capabilities.elicitation.clone())
            .and_then(|elicitation| elicitation.form)
            .is_some()
    }

    fn supports_terminals(&self) -> bool {
        self.client_capabilities
            .lock()
            .ok()
            .is_some_and(|capabilities| capabilities.terminal)
    }

    pub async fn available_commands(
        &self,
        session_id: &str,
    ) -> Result<AvailableCommandsUpdate, Error> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        Ok(AvailableCommandsUpdate::new(available_commands(session)))
    }

    pub async fn new_session(
        &self,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
    ) -> Result<NewSessionResponse, Error> {
        let cwd = workspace_path(&cwd)?;
        let mut config =
            crate::config::ConfigLoader::load_for(&cwd).map_err(|_| internal_error())?;
        merge_acp_mcp_servers(&mut config, mcp_servers);
        crate::skill::init_skill_store(&config.xdg_config_home, &config.project_root);
        let (provider, model) = resolve_model(&config);
        let models = crate::model::catalog::selectable_models(&config, None)
            .await
            .map_err(|_| internal_error())?;
        let reasoning = model_reasoning(&config, &models, &provider, &model);
        let reasoning_selection =
            reasoning.unwrap_or(crate::model::reasoning::ReasoningEffort::None);
        let context_window = model_context_window(&config, &provider, &model);
        let agent = config
            .merged_config
            .default_agent
            .clone()
            .unwrap_or_else(|| {
                config
                    .merged_config
                    .agent_registry
                    .default_agent()
                    .to_string()
            });

        let session_id = {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .switch_current_workspace_path(&cwd.to_string_lossy())
                .map_err(|_| internal_error())?;
            manager.create_session(Some("New session".to_string()))
        };

        let skills = crate::skill::SkillStore::load(&config.xdg_config_home, &config.project_root);
        self.sessions.lock().await.insert(
            session_id.clone(),
            AcpSession {
                cwd,
                config,
                skills,
                models,
                provider,
                model,
                agent,
                reasoning_selection,
                reasoning,
                context_window,
                cancellation: None,
            },
        );

        let session = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(internal_error)?;
        Ok(NewSessionResponse::new(session_id)
            .modes(session_modes(&session))
            .config_options(session_config_options(&session)))
    }

    pub async fn list_sessions(&self, cwd: Option<PathBuf>) -> Result<ListSessionsResponse, Error> {
        let cwd = cwd.as_deref().map(workspace_path).transpose()?;
        let manager = self.session_manager.lock().map_err(|_| internal_error())?;
        let mut sessions = manager
            .list_sessions()
            .into_iter()
            .filter(|session| session.parent_id.is_none() && session.archived_at.is_none())
            .filter(|session| {
                cwd.as_ref()
                    .is_none_or(|cwd| session.workspace_path == cwd.to_string_lossy())
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));

        Ok(ListSessionsResponse::new(
            sessions
                .into_iter()
                .take(100)
                .map(|session| {
                    SessionInfo::new(session.id, session.workspace_path)
                        .title(session.title)
                        .updated_at(system_time_to_iso8601(session.updated_at))
                })
                .collect(),
        ))
    }

    pub async fn load_session(
        &self,
        session_id: String,
        cwd: PathBuf,
        connection: ConnectionTo<Client>,
    ) -> Result<LoadSessionResponse, Error> {
        let (session, messages) = self.attach_persisted_session(&session_id, cwd).await?;
        replay_messages(&connection, &session_id, &messages, &session.cwd)?;
        Ok(LoadSessionResponse::new()
            .modes(session_modes(&session))
            .config_options(session_config_options(&session)))
    }

    pub async fn resume_session(
        &self,
        session_id: String,
        cwd: PathBuf,
    ) -> Result<ResumeSessionResponse, Error> {
        let (session, _) = self.attach_persisted_session(&session_id, cwd).await?;
        Ok(ResumeSessionResponse::new()
            .modes(session_modes(&session))
            .config_options(session_config_options(&session)))
    }

    pub async fn fork_session(
        &self,
        session_id: String,
        cwd: PathBuf,
    ) -> Result<NewSessionResponse, Error> {
        let (source, messages) = self.attach_persisted_session(&session_id, cwd).await?;
        let fork_id = {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .switch_current_workspace_path(&source.cwd.to_string_lossy())
                .map_err(|_| internal_error())?;
            let fork_id = manager.create_session(Some(format!("{} (fork)", session_id)));
            let messages =
                match crate::persistence::attachments::clone_messages(&messages, &fork_id) {
                    Ok(messages) => messages,
                    Err(_) => {
                        manager.delete_session(&fork_id);
                        return Err(internal_error());
                    }
                };
            if manager
                .replace_session_messages(&fork_id, messages)
                .is_err()
            {
                manager.delete_session(&fork_id);
                return Err(internal_error());
            }
            fork_id
        };
        self.sessions
            .lock()
            .await
            .insert(fork_id.clone(), source.clone());
        Ok(NewSessionResponse::new(fork_id)
            .modes(session_modes(&source))
            .config_options(session_config_options(&source)))
    }

    pub async fn close_session(&self, session_id: &str) {
        if let Some(session) = self.sessions.lock().await.remove(session_id) {
            if let Some(cancellation) = session.cancellation {
                cancellation.cancel();
            }
        }
    }

    pub async fn cancel_session(&self, session_id: &str) {
        if let Some(session) = self.sessions.lock().await.get(session_id) {
            if let Some(cancellation) = &session.cancellation {
                cancellation.cancel();
            }
        }
    }

    pub async fn set_mode(
        &self,
        session_id: &str,
        mode_id: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        if session
            .config
            .merged_config
            .agent_registry
            .primary_agent(mode_id)
            .is_none_or(|agent| agent.hidden)
        {
            return Err(Error::invalid_params().data("unknown ACP mode"));
        }
        session.agent = mode_id.to_string();
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    pub async fn set_model(
        &self,
        session_id: &str,
        model_ref: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let model = find_selectable_model(&session.models, model_ref)?;
        session.provider.clone_from(&model.provider_id);
        session.model.clone_from(&model.id);
        session.reasoning = resolved_reasoning(session, session.reasoning_selection);
        session.context_window =
            model_context_window(&session.config, &session.provider, &session.model);
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    pub async fn set_reasoning_effort(
        &self,
        session_id: &str,
        value: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let requested = value
            .parse::<crate::model::reasoning::ReasoningEffort>()
            .map_err(|_| Error::invalid_params().data("invalid reasoning effort"))?;
        session.reasoning_selection = requested;
        session.reasoning = resolved_reasoning(session, requested);
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    async fn attach_persisted_session(
        &self,
        session_id: &str,
        cwd: PathBuf,
    ) -> Result<(AcpSession, Vec<crate::session::types::Message>), Error> {
        let cwd = workspace_path(&cwd)?;
        let config = crate::config::ConfigLoader::load_for(&cwd).map_err(|_| internal_error())?;
        crate::skill::init_skill_store(&config.xdg_config_home, &config.project_root);
        let models = crate::model::catalog::selectable_models(&config, None)
            .await
            .map_err(|_| internal_error())?;

        let messages = {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            let stored = manager
                .get_session(session_id)
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
            if stored.workspace_path != cwd.to_string_lossy() {
                return Err(Error::invalid_params().data("session does not belong to cwd"));
            }
            stored.messages.clone()
        };

        let (provider, model) = messages
            .iter()
            .rev()
            .find_map(|message| match (&message.provider, &message.model) {
                (Some(provider), Some(model)) => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .unwrap_or_else(|| resolve_model(&config));
        let agent = messages
            .iter()
            .rev()
            .find_map(|message| message.agent_mode.clone())
            .unwrap_or_else(|| {
                config
                    .merged_config
                    .default_agent
                    .clone()
                    .unwrap_or_else(|| {
                        config
                            .merged_config
                            .agent_registry
                            .default_agent()
                            .to_string()
                    })
            });
        let reasoning = model_reasoning(&config, &models, &provider, &model);
        let reasoning_selection =
            reasoning.unwrap_or(crate::model::reasoning::ReasoningEffort::None);
        let context_window = model_context_window(&config, &provider, &model);
        let skills = crate::skill::SkillStore::load(&config.xdg_config_home, &config.project_root);
        let session = AcpSession {
            cwd,
            config,
            skills,
            models,
            provider,
            model,
            agent,
            reasoning_selection,
            reasoning,
            context_window,
            cancellation: None,
        };
        self.sessions
            .lock()
            .await
            .insert(session_id.to_string(), session.clone());
        Ok((session, messages))
    }

    pub async fn prompt(
        &self,
        session_id: String,
        prompt: Vec<ContentBlock>,
        connection: ConnectionTo<Client>,
    ) -> Result<PromptResponse, Error> {
        let session = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let supports_images = session
            .models
            .iter()
            .find(|model| model.provider_id == session.provider && model.id == session.model)
            .is_some_and(|model| model.attachment);
        let compact_text = prompt
            .iter()
            .filter_map(|part| match part {
                ContentBlock::Text(content) => Some(content.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        if compact_command(&compact_text)? {
            if prompt
                .iter()
                .any(|part| !matches!(part, ContentBlock::Text(_)))
            {
                return Err(Error::invalid_params().data("/compact does not accept attachments"));
            }
            return self.compact_session(&session_id, session, connection).await;
        }
        let supports_audio =
            model_supports_audio(&session.config, &session.provider, &session.model);
        let (prompt, local_image_paths, local_audio_paths) = prompt_content(
            prompt,
            supports_images,
            supports_audio,
            &session_id,
            &session,
        )?;
        let mut managed_paths = local_image_paths.clone();
        managed_paths.extend(local_audio_paths.clone());
        let mut attachment_guard = ManagedAttachmentGuard::new(managed_paths);
        let prompt = expand_slash_command(&session, &prompt).await?;
        if prompt.trim().is_empty() {
            return Err(Error::invalid_params().data("prompt must include text content"));
        }
        let cancellation = CancellationToken::new();
        {
            let mut sessions = self.sessions.lock().await;
            let Some(current) = sessions.get_mut(&session_id) else {
                return Err(Error::invalid_params().data("unknown session"));
            };
            if current.cancellation.is_some() {
                return Err(Error::invalid_params().data("session already has an active prompt"));
            }
            current.cancellation = Some(cancellation.clone());
        }

        let mut messages = {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            let stored = manager
                .get_session(&session_id)
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
            stored.messages.clone()
        };
        let mut user_message = crate::session::types::Message::user(&prompt);
        user_message.local_image_paths = local_image_paths;
        user_message.local_audio_paths = local_audio_paths;
        user_message.provider = Some(session.provider.clone());
        user_message.model = Some(session.model.clone());
        user_message.agent_mode = Some(session.agent.clone());
        {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .add_message_to_session(&session_id, &user_message)
                .map_err(|_| internal_error())?;
            attachment_guard.commit();
            manager
                .set_session_status(
                    &session_id,
                    crate::session::types::SessionStatus::Streaming,
                    None,
                )
                .map_err(|_| internal_error())?;
        }
        messages.push(user_message);

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let process_registry = std::sync::Arc::new(crate::tools::ProcessRegistry::new());
        let prompt_registry = crate::tools::initialize_tool_registry_with_dynamic_config(
            Some(sender.clone()),
            tool_permissions(&session),
            session.config.merged_config.agent_registry.clone(),
            cancellation.clone(),
            Some(&session.provider),
            &session.config.merged_config.websearch,
            &session.config.merged_config.mcp,
            &session.cwd,
            process_registry.clone(),
        )
        .await;
        let is_git_repo =
            crate::utils::git::is_git_repo(&session.cwd.to_string_lossy()).unwrap_or(false);
        let system_prompt = crate::prompt::SystemPromptComposer::new(
            &session.model,
            session.cwd.to_string_lossy(),
            is_git_repo,
            std::env::consts::OS,
        )
        .with_tool_registry(prompt_registry.clone())
        .with_agent_registry(session.config.merged_config.agent_registry.clone())
        .with_active_agent(session.agent.clone())
        .compose()
        .await;
        messages.insert(0, crate::session::types::Message::system(system_prompt));
        let base_context_tokens = crate::session::compaction::total_context_tokens(&messages);
        let base_cost = messages
            .iter()
            .filter_map(|message| message.cost)
            .sum::<f64>();
        send_usage(
            &connection,
            &session_id,
            &session,
            base_context_tokens,
            (base_cost > 0.0).then_some(base_cost),
        )?;

        let stream_session_id = session_id.clone();
        let stream_cancellation = cancellation.clone();
        let stream_sender = sender.clone();
        let stream_session = session.clone();
        let stream_tool_registry = prompt_registry;
        let stream_process_registry = process_registry;
        tokio::spawn(async move {
            let result = crate::llm::client::stream_llm_with_cancellation(
                stream_cancellation,
                stream_session_id,
                stream_session.provider.clone(),
                stream_session.model.clone(),
                stream_session.reasoning,
                stream_session.agent.clone(),
                stream_session
                    .config
                    .merged_config
                    .agent_registry
                    .get(&stream_session.agent)
                    .and_then(|agent| agent.max_steps),
                stream_session.config.merged_config.agent_registry.clone(),
                tool_permissions(&stream_session),
                stream_session.config.merged_config.websearch.clone(),
                stream_session.config.merged_config.mcp.clone(),
                stream_session.cwd.to_string_lossy().to_string(),
                Some(stream_tool_registry),
                messages,
                sender,
                stream_process_registry,
            )
            .await;
            if let Err(error) = result {
                let _ = stream_sender.send(crate::llm::ChunkMessage::Failed(error.to_string()));
            }
            let _ = stream_sender.send(crate::llm::ChunkMessage::End);
        });

        let mut assistant = crate::session::types::Message::incomplete("");
        let message_id = assistant.id.clone();
        assistant.provider = Some(session.provider.clone());
        assistant.model = Some(session.model.clone());
        assistant.agent_mode = Some(session.agent.clone());
        let mut failed = None;
        let mut cancelled = false;
        let mut turn_stop_reason = None;

        while let Some(chunk) = receiver.recv().await {
            match chunk {
                crate::llm::ChunkMessage::Text(text) => {
                    assistant.append(&text);
                    send_text(&connection, &session_id, &message_id, text, false)?;
                }
                crate::llm::ChunkMessage::Reasoning(text) => {
                    assistant.append_reasoning(&text);
                    send_text(&connection, &session_id, &message_id, text, true)?;
                }
                crate::llm::ChunkMessage::ToolCalls(tool_calls) => {
                    for tool_call in tool_calls {
                        send_tool_call(&connection, &session_id, tool_call, &session.cwd)?;
                    }
                }
                crate::llm::ChunkMessage::ToolResult(result) => {
                    assistant.add_or_update_tool_result_part(serde_json::json!({
                        "id": result.tool_call_id,
                        "name": result.name,
                        "content": result.content,
                    }));
                    send_tool_result(&connection, &session_id, result, &session.cwd)?;
                }
                crate::llm::ChunkMessage::Metrics {
                    token_count,
                    duration_ms,
                    usage,
                    cost,
                } => {
                    assistant.token_count = Some(token_count);
                    assistant.duration_ms = Some(duration_ms);
                    if let Some(usage) = usage {
                        assistant.apply_usage(usage, cost);
                    }
                    send_usage(
                        &connection,
                        &session_id,
                        &session,
                        base_context_tokens.saturating_add(token_count),
                        cost.map(|turn_cost| base_cost + turn_cost),
                    )?;
                }
                crate::llm::ChunkMessage::Cancelled => cancelled = true,
                crate::llm::ChunkMessage::Failed(error) => failed = Some(error),
                crate::llm::ChunkMessage::TurnStopReason(reason) => turn_stop_reason = Some(reason),
                crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                    let response = request_permission(&connection, &session_id, &prompt).await;
                    let _ = prompt.response_tx.send(response);
                }
                crate::llm::ChunkMessage::QuestionRequest {
                    tool_call_id,
                    questions,
                    response_tx,
                } => {
                    let response = if self.supports_form_elicitation() {
                        request_questions(
                            &connection,
                            &session_id,
                            tool_call_id.as_deref(),
                            &questions,
                            &cancellation,
                        )
                        .await
                    } else {
                        skipped_question_answers(&questions)
                    };
                    let _ = response_tx.send(response);
                }
                crate::llm::ChunkMessage::TerminalSessionRequest(request) => {
                    if self.supports_terminals() {
                        bridge_terminal_session(
                            &connection,
                            &session_id,
                            &session.cwd,
                            request,
                            &cancellation,
                        )
                        .await;
                    } else {
                        let _ = request
                            .control_tx
                            .send(crate::tools::TerminalSessionControl::Stop);
                    }
                }
                crate::llm::ChunkMessage::End => break,
                _ => {}
            }
        }

        assistant.is_complete = true;
        assistant.was_interrupted = cancelled || cancellation.is_cancelled();
        {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .add_message_to_session(&session_id, &assistant)
                .map_err(|_| internal_error())?;
            let status = if assistant.was_interrupted {
                crate::session::types::SessionStatus::Interrupted
            } else if failed.is_some() {
                crate::session::types::SessionStatus::Failed
            } else {
                crate::session::types::SessionStatus::Idle
            };
            manager
                .set_session_status(&session_id, status, failed.as_deref())
                .map_err(|_| internal_error())?;
        }
        if let Some(current) = self.sessions.lock().await.get_mut(&session_id) {
            current.cancellation = None;
        }

        if assistant.was_interrupted {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }
        if let Some(error) = failed {
            return Err(internal_error_with(&error));
        }
        Ok(PromptResponse::new(acp_stop_reason(turn_stop_reason)))
    }

    async fn compact_session(
        &self,
        session_id: &str,
        session: AcpSession,
        connection: ConnectionTo<Client>,
    ) -> Result<PromptResponse, Error> {
        let cancellation = CancellationToken::new();
        {
            let mut sessions = self.sessions.lock().await;
            let current = sessions
                .get_mut(session_id)
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
            if current.cancellation.is_some() {
                return Err(Error::invalid_params().data("session already has an active prompt"));
            }
            current.cancellation = Some(cancellation.clone());
        }
        let status_result = self
            .session_manager
            .lock()
            .map_err(|_| internal_error())?
            .set_session_status(
                session_id,
                crate::session::types::SessionStatus::Streaming,
                None,
            );
        if status_result.is_err() {
            if let Some(current) = self.sessions.lock().await.get_mut(session_id) {
                current.cancellation = None;
            }
            return Err(internal_error());
        }

        let result = self
            .run_compaction(session_id, &session, cancellation.clone())
            .await;
        if let Some(current) = self.sessions.lock().await.get_mut(session_id) {
            current.cancellation = None;
        }
        self.session_manager
            .lock()
            .map_err(|_| internal_error())?
            .set_session_status(session_id, crate::session::types::SessionStatus::Idle, None)
            .map_err(|_| internal_error())?;

        match result {
            Ok(stats) => {
                let feedback = format!(
                    "Context compacted ({})",
                    crate::session::compaction::format_compaction_stats(stats)
                );
                send_text(
                    &connection,
                    session_id,
                    &cuid2::create_id(),
                    feedback,
                    false,
                )?;
                Ok(PromptResponse::new(StopReason::EndTurn))
            }
            Err(_error) if cancellation.is_cancelled() => {
                Ok(PromptResponse::new(StopReason::Cancelled))
            }
            Err(error) => Err(error),
        }
    }

    async fn run_compaction(
        &self,
        session_id: &str,
        session: &AcpSession,
        cancellation: CancellationToken,
    ) -> Result<crate::session::types::CompactionStats, Error> {
        let messages = {
            let manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .get_session_ref(session_id)
                .map(|stored| stored.messages.clone())
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?
        };
        let selection = crate::session::compaction::select_messages_for_compaction_with_min(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
            0,
        )
        .ok_or_else(|| Error::invalid_params().data("Nothing to compact"))?;
        let before_tokens = crate::session::compaction::total_context_tokens(&messages);
        let before_messages =
            crate::session::compaction::filter_messages_for_context(&messages).len();
        let prompt = crate::session::compaction::build_prompt(&selection.messages_to_summarize);
        let summary = crate::llm::client::summarize_for_compaction(
            session.provider.clone(),
            session.model.clone(),
            compaction_reasoning(session),
            prompt,
            cancellation.clone(),
        )
        .await
        .map_err(|error| internal_error_with(&error.to_string()))?;
        if cancellation.is_cancelled() {
            return Err(internal_error_with("Compaction cancelled by user"));
        }
        let (compacted, stats) = compacted_messages(
            &messages,
            &selection,
            &summary,
            session,
            before_tokens,
            before_messages,
        )?;
        let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
        manager
            .replace_session_messages(session_id, compacted)
            .map_err(|_| internal_error())?;
        Ok(stats)
    }
}

fn compaction_reasoning(session: &AcpSession) -> Option<crate::model::reasoning::ReasoningEffort> {
    use crate::model::reasoning::ReasoningEffort;
    let capability = model_reasoning_capability(
        &session.config,
        &session.models,
        &session.provider,
        &session.model,
    )?;
    [
        ReasoningEffort::None,
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
    ]
    .into_iter()
    .find(|effort| capability.values().contains(effort))
    .or(session.reasoning)
    .filter(|effort| *effort != ReasoningEffort::None)
}

fn compacted_messages(
    messages: &[crate::session::types::Message],
    selection: &crate::session::compaction::CompactionSelection,
    summary: &str,
    session: &AcpSession,
    before_tokens: usize,
    before_messages: usize,
) -> Result<
    (
        Vec<crate::session::types::Message>,
        crate::session::types::CompactionStats,
    ),
    Error,
> {
    let mut compacted = crate::session::compaction::apply_soft_compaction(
        messages,
        selection,
        summary,
        Some(session.model.clone()),
        Some(session.provider.clone()),
        Some(session.agent.clone()),
        crate::session::types::CompactionStats {
            before_tokens,
            after_tokens: 0,
            before_messages,
            after_messages: 0,
        },
    );
    let after_tokens = crate::session::compaction::total_context_tokens(&compacted);
    let after_messages = crate::session::compaction::filter_messages_for_context(&compacted).len();
    let stats = crate::session::types::CompactionStats {
        before_tokens,
        after_tokens,
        before_messages,
        after_messages,
    };
    if after_tokens >= before_tokens {
        return Err(Error::invalid_params().data(format!(
            "Compaction did not reduce context ({})",
            crate::session::compaction::format_compaction_stats(stats)
        )));
    }
    if let Some(marker) = compacted
        .iter_mut()
        .rev()
        .find(|message| crate::session::compaction::is_compaction_marker(message))
    {
        marker.compaction_stats = Some(stats);
    }
    Ok((compacted, stats))
}

fn acp_stop_reason(reason: Option<crate::llm::TurnStopReason>) -> StopReason {
    match reason {
        Some(crate::llm::TurnStopReason::MaxTokens) => StopReason::MaxTokens,
        Some(crate::llm::TurnStopReason::Refusal) => StopReason::Refusal,
        None => StopReason::EndTurn,
    }
}

fn resolve_model(config: &LoadedConfig) -> (String, String) {
    if let Ok(Some((provider, model))) =
        crate::persistence::PrefsDAO::new().and_then(|prefs| prefs.get_active_model())
    {
        return (provider, model);
    }
    config
        .merged_config
        .model
        .as_deref()
        .map(crate::app::parse_model_ref)
        .unwrap_or_else(|| ("opencode".to_string(), "big-pickle".to_string()))
}

fn tool_permissions(session: &AcpSession) -> crate::tools::ToolPermissions {
    let mut policies = crate::tools::AgentToolPolicies::default();
    for (mode, tools) in session
        .config
        .merged_config
        .agent_registry
        .tool_policy_map()
    {
        policies = policies.with_custom_tools(mode, tools);
    }
    crate::tools::ToolPermissions::new(&session.cwd)
        .with_agent_policies(policies)
        .with_permission_rules(session.config.merged_config.permission_rules.clone())
        .with_agent_permission_rules(
            session
                .config
                .merged_config
                .agent_registry
                .permission_rules_map(),
        )
}

fn session_modes(session: &AcpSession) -> SessionModeState {
    let mut modes = session
        .config
        .merged_config
        .agent_registry
        .visible_primary_agents()
        .into_iter()
        .map(|agent| {
            SessionMode::new(agent.name.clone(), agent.name.clone())
                .description(agent.description.clone())
        })
        .collect::<Vec<_>>();
    modes.sort_by(|left, right| left.name.cmp(&right.name));
    SessionModeState::new(session.agent.clone(), modes)
}

fn session_config_options(session: &AcpSession) -> Vec<SessionConfigOption> {
    let mut mode_options = session
        .config
        .merged_config
        .agent_registry
        .visible_primary_agents()
        .into_iter()
        .map(|agent| {
            SessionConfigSelectOption::new(agent.name.clone(), agent.name.clone())
                .description(agent.description.clone())
        })
        .collect::<Vec<_>>();
    mode_options.sort_by(|left, right| left.name.cmp(&right.name));

    let mut options = vec![
        SessionConfigOption::select("mode", "Mode", session.agent.clone(), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        model_config_option(&session.models, &session.provider, &session.model),
    ];
    options.push(reasoning_config_option(session));
    options
}

fn reasoning_config_option(session: &AcpSession) -> SessionConfigOption {
    let options = [
        crate::model::reasoning::ReasoningEffort::None,
        crate::model::reasoning::ReasoningEffort::Low,
        crate::model::reasoning::ReasoningEffort::Medium,
        crate::model::reasoning::ReasoningEffort::High,
        crate::model::reasoning::ReasoningEffort::XHigh,
        crate::model::reasoning::ReasoningEffort::Max,
    ]
    .iter()
    .map(|effort| SessionConfigSelectOption::new(effort.as_str(), reasoning_effort_label(*effort)))
    .collect::<Vec<_>>();
    let current = session.reasoning_selection.as_str();
    SessionConfigOption::select("effort", "Effort", current, options)
        .category(SessionConfigOptionCategory::ThoughtLevel)
}

fn model_config_option(
    models: &[crate::model::types::Model],
    provider: &str,
    model_id: &str,
) -> SessionConfigOption {
    let mut model_groups: Vec<SessionConfigSelectGroup> = Vec::new();
    for model in models {
        let option = SessionConfigSelectOption::new(model_value(model), model.name.clone())
            .description(model.id.clone());
        if model_groups
            .last()
            .is_some_and(|group| group.group.to_string() == model.provider_id)
        {
            model_groups
                .last_mut()
                .expect("model group exists")
                .options
                .push(option);
        } else {
            model_groups.push(SessionConfigSelectGroup::new(
                model.provider_id.clone(),
                model.provider_name.clone(),
                vec![option],
            ));
        }
    }

    SessionConfigOption::select(
        "model",
        "Model",
        format!("{provider}/{model_id}"),
        model_groups,
    )
    .category(SessionConfigOptionCategory::Model)
}

fn model_value(model: &crate::model::types::Model) -> String {
    crate::model::catalog::model_ref(model)
}

fn find_selectable_model<'a>(
    models: &'a [crate::model::types::Model],
    model_ref: &str,
) -> Result<&'a crate::model::types::Model, Error> {
    models
        .iter()
        .find(|model| model_value(model) == model_ref)
        .ok_or_else(|| Error::invalid_params().data("unknown or unavailable ACP model"))
}

fn model_reasoning(
    config: &LoadedConfig,
    models: &[crate::model::types::Model],
    provider: &str,
    model_id: &str,
) -> Option<crate::model::reasoning::ReasoningEffort> {
    model_reasoning_capability(config, models, provider, model_id)
        .and_then(|capability| capability.resolve(None))
}

fn model_reasoning_capability(
    config: &LoadedConfig,
    models: &[crate::model::types::Model],
    provider: &str,
    model_id: &str,
) -> Option<crate::model::reasoning::ReasoningCapability> {
    if let Some(capability) = models
        .iter()
        .find(|model| model.provider_id == provider && model.id == model_id)
        .and_then(|model| {
            crate::model::reasoning::capability_from_options(&model.reasoning_options)
        })
    {
        return Some(capability);
    }
    crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok()
    .and_then(|discovery| discovery.get_model_reasoning_capability(provider, model_id))
    .filter(|capability| !capability.values().is_empty())
}

fn model_context_window(config: &LoadedConfig, provider: &str, model: &str) -> Option<u32> {
    let discovery = crate::model::discovery::Discovery::new_with_custom(Some(
        config.merged_config.custom_providers.clone(),
    ))
    .ok();
    discovery
        .as_ref()
        .and_then(|discovery| discovery.get_model_limit(provider, model))
        .or_else(|| {
            config
                .merged_config
                .custom_providers
                .get(provider)
                .and_then(|provider| provider.models.get(model))
                .and_then(|model| model.context_window)
        })
}

fn workspace_path(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() {
        return Err(Error::invalid_params().data("cwd must be an absolute path"));
    }
    let path = path
        .canonicalize()
        .map_err(|_| Error::resource_not_found(Some(path.to_string_lossy().to_string())))?;
    if !path.is_dir() {
        return Err(Error::invalid_params().data("cwd must be a directory"));
    }
    Ok(path)
}

fn prompt_content(
    parts: Vec<ContentBlock>,
    supports_images: bool,
    supports_audio: bool,
    session_id: &str,
    session: &AcpSession,
) -> Result<(String, Vec<String>, Vec<String>), Error> {
    let mut text = String::new();
    let mut local_image_paths = Vec::new();
    let mut local_audio_paths = Vec::new();
    for part in parts {
        match part {
            ContentBlock::Text(content) => text.push_str(&content.text),
            ContentBlock::ResourceLink(link) => {
                text.push_str(&format!("[{}]", link.uri));
            }
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(resource) => {
                    text.push_str(&format!("[{}]\n{}", resource.uri, resource.text));
                }
                EmbeddedResourceResource::BlobResourceContents(resource) => {
                    text.push_str(&format!("[{}]", resource.uri));
                }
                _ => {}
            },
            ContentBlock::Image(image) => {
                if !supports_images {
                    return Err(Error::invalid_params().data(format!(
                        "model {}/{} does not support image input",
                        session.provider, session.model
                    )));
                }
                match write_prompt_image(session_id, &image) {
                    Ok(path) => local_image_paths.push(path),
                    Err(error) => {
                        for path in &local_image_paths {
                            crate::persistence::attachments::remove_file(Path::new(path));
                        }
                        return Err(error);
                    }
                }
            }
            ContentBlock::Audio(audio) => {
                if !supports_audio {
                    for path in local_image_paths.iter().chain(local_audio_paths.iter()) {
                        crate::persistence::attachments::remove_file(Path::new(path));
                    }
                    return Err(Error::invalid_params().data(format!(
                        "model {}/{} does not support audio input",
                        session.provider, session.model
                    )));
                }
                match write_prompt_audio(session_id, &audio) {
                    Ok(path) => local_audio_paths.push(path),
                    Err(error) => {
                        for path in local_image_paths.iter().chain(local_audio_paths.iter()) {
                            crate::persistence::attachments::remove_file(Path::new(path));
                        }
                        return Err(error);
                    }
                }
            }
            _ => {}
        }
    }
    if text.is_empty() {
        if !local_image_paths.is_empty() {
            text.push_str("[Image attached]");
        } else if !local_audio_paths.is_empty() {
            text.push_str("[Audio attached]");
        }
    }
    Ok((text, local_image_paths, local_audio_paths))
}

fn prompt_text(parts: Vec<ContentBlock>) -> Result<String, Error> {
    let mut text = String::new();
    for part in parts {
        match part {
            ContentBlock::Text(content) => text.push_str(&content.text),
            ContentBlock::ResourceLink(link) => text.push_str(&format!("[{}]", link.uri)),
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(resource) => {
                    text.push_str(&format!("[{}]\n{}", resource.uri, resource.text));
                }
                EmbeddedResourceResource::BlobResourceContents(resource) => {
                    text.push_str(&format!("[{}]", resource.uri));
                }
                _ => {}
            },
            ContentBlock::Image(_) | ContentBlock::Audio(_) => {
                return Err(
                    Error::invalid_params().data("binary ACP prompt content is not supported here")
                );
            }
            _ => {}
        }
    }
    Ok(text)
}

fn write_prompt_image(
    session_id: &str,
    image: &agent_client_protocol::schema::v1::ImageContent,
) -> Result<String, Error> {
    const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

    let (mime_type, encoded_data) = prompt_image_payload(image)?;
    let extension = match mime_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        mime_type => {
            return Err(
                Error::invalid_params().data(format!("unsupported image MIME type: {mime_type}"))
            );
        }
    };
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded_data)
        .map_err(|error| Error::invalid_params().data(format!("invalid image data: {error}")))?;
    if data.len() > MAX_IMAGE_BYTES {
        return Err(Error::invalid_params().data("image exceeds the 20 MiB size limit"));
    }

    let path = crate::persistence::attachments::write(session_id, extension, &data)
        .map_err(|_| internal_error())?;
    Ok(path.to_string_lossy().into_owned())
}

fn prompt_image_payload(
    image: &agent_client_protocol::schema::v1::ImageContent,
) -> Result<(&str, &str), Error> {
    let data = image.data.trim();
    if data.is_empty() {
        return Err(Error::invalid_params().data("image data is empty"));
    }

    let Some(data_uri) = data.strip_prefix("data:") else {
        return Ok((image.mime_type.as_str(), data));
    };
    let (metadata, payload) = data_uri
        .split_once(',')
        .ok_or_else(|| Error::invalid_params().data("invalid image data URI"))?;
    let mut metadata = metadata.split(';');
    let mime_type = metadata
        .next()
        .filter(|mime_type| !mime_type.is_empty())
        .ok_or_else(|| Error::invalid_params().data("image data URI is missing a MIME type"))?;
    if !metadata.any(|value| value.eq_ignore_ascii_case("base64")) {
        return Err(Error::invalid_params().data("image data URI must be base64 encoded"));
    }
    let payload = payload.trim();
    if payload.is_empty() {
        return Err(Error::invalid_params().data("image data is empty"));
    }

    Ok((mime_type, payload))
}

fn send_text(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    message_id: &str,
    text: String,
    thought: bool,
) -> Result<(), Error> {
    let update = if thought {
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(text.into()).message_id(message_id))
    } else {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into()).message_id(message_id))
    };
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

fn tool_result_text(payload: &serde_json::Value) -> String {
    payload
        .get("output")
        .or_else(|| payload.get("output_preview"))
        .or_else(|| payload.get("error"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn absolute_tool_path(path: &str, cwd: &Path) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn tool_locations(tool_name: &str, input: &serde_json::Value, cwd: &Path) -> Vec<ToolCallLocation> {
    let paths = if tool_name == "write_files" {
        input
            .get("files")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|file| file.get("file_path").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect()
    } else if tool_name == "apply_patch" {
        crate::tools::patch::patch_paths_from_params(input)
    } else {
        input
            .get("file_path")
            .or_else(|| input.get("filePath"))
            .or_else(|| input.get("filepath"))
            .or_else(|| input.get("path"))
            .and_then(serde_json::Value::as_str)
            .map(|path| vec![path.to_string()])
            .unwrap_or_default()
    };

    paths
        .into_iter()
        .map(|path| ToolCallLocation::new(absolute_tool_path(&path, cwd)))
        .collect()
}

fn tool_result_locations(payload: &serde_json::Value, cwd: &Path) -> Vec<ToolCallLocation> {
    let Some(metadata) = payload.get("metadata") else {
        return Vec::new();
    };
    let line = metadata
        .get("line_number")
        .and_then(serde_json::Value::as_u64)
        .and_then(|line| u32::try_from(line).ok());

    if let Some(changes) = metadata
        .get("changes")
        .and_then(serde_json::Value::as_array)
    {
        return changes
            .iter()
            .filter_map(|change| change.get("path").and_then(serde_json::Value::as_str))
            .map(|path| ToolCallLocation::new(absolute_tool_path(path, cwd)))
            .collect();
    }

    metadata
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(|path| ToolCallLocation::new(absolute_tool_path(path, cwd)).line(line))
        .into_iter()
        .collect()
}

fn tool_result_content(payload: &serde_json::Value, cwd: &Path) -> Vec<ToolCallContent> {
    let mut content = Vec::new();
    let text = tool_result_text(payload);
    if !text.is_empty() {
        content.push(ToolCallContent::from(text));
    }

    if let Some(metadata) = payload.get("metadata") {
        if let Some(changes) = metadata
            .get("changes")
            .and_then(serde_json::Value::as_array)
        {
            content.extend(changes.iter().filter_map(|change| tool_diff(change, cwd)));
        } else if let Some(diff) = tool_diff(metadata, cwd) {
            content.push(diff);
        }
    }

    if let Some(images) = payload.get("images").and_then(serde_json::Value::as_array) {
        content.extend(images.iter().filter_map(tool_result_image));
    }

    content
}

fn tool_diff(change: &serde_json::Value, cwd: &Path) -> Option<ToolCallContent> {
    let path = change.get("path")?.as_str()?;
    let new_text = change.get("new_text")?.as_str()?;
    let old_text = change.get("old_text").and_then(serde_json::Value::as_str);
    Some(
        agent_client_protocol::schema::v1::Diff::new(
            absolute_tool_path(path, cwd),
            new_text.to_string(),
        )
        .old_text(old_text.map(str::to_string))
        .into(),
    )
}

fn tool_result_image(image: &serde_json::Value) -> Option<ToolCallContent> {
    let data = image.get("data_url")?.as_str()?;
    let media_type = image.get("media_type")?.as_str()?;
    let encoded = data
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
        .map(|(_, encoded)| encoded)
        .unwrap_or(data);
    Some(ToolCallContent::from(ContentBlock::Image(
        agent_client_protocol::schema::v1::ImageContent::new(encoded, media_type),
    )))
}

fn send_tool_call(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    tool_call: crate::llm::ToolCall,
    cwd: &Path,
) -> Result<(), Error> {
    let raw_input = serde_json::from_str(&tool_call.function.arguments)
        .unwrap_or_else(|_| serde_json::json!({ "arguments": tool_call.function.arguments }));
    let title = tool_title(&tool_call.function.name, &raw_input);
    let update = SessionUpdate::ToolCall(
        ToolCall::new(tool_call.id, title)
            .kind(tool_kind(&tool_call.function.name))
            .status(ToolCallStatus::Pending)
            .locations(tool_locations(&tool_call.function.name, &raw_input, cwd))
            .raw_input(raw_input),
    );
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

async fn request_permission(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    prompt: &crate::tools::PermissionPrompt,
) -> crate::tools::PermissionResponse {
    let tool_call_id = permission_tool_call_id(prompt.tool_call_id.as_deref());
    let input = serde_json::json!({
        "tool": prompt.tool_id,
        "permission": prompt.permission,
        "patterns": prompt.patterns,
        "target": prompt.target,
        "command": prompt.command,
        "workdir": prompt.workdir,
        "reason": prompt.reason,
    });
    let tool_call = ToolCallUpdate::new(
        tool_call_id,
        ToolCallUpdateFields::new()
            .title(permission_title(prompt))
            .kind(tool_kind(&prompt.tool_id))
            .status(ToolCallStatus::Pending)
            .raw_input(input),
    );
    let request = RequestPermissionRequest::new(
        session_id.to_string(),
        tool_call,
        vec![
            PermissionOption::new("once", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("always", "Always allow", PermissionOptionKind::AllowAlways),
            PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
        ],
    );
    let Ok(response) = connection.send_request(request).block_task().await else {
        return crate::tools::PermissionResponse::Deny;
    };
    match response.outcome {
        RequestPermissionOutcome::Selected(selection)
            if selection.option_id.to_string() == "once" =>
        {
            crate::tools::PermissionResponse::AllowOnce
        }
        RequestPermissionOutcome::Selected(selection)
            if selection.option_id.to_string() == "always" =>
        {
            crate::tools::PermissionResponse::AllowAlways
        }
        RequestPermissionOutcome::Selected(_) | RequestPermissionOutcome::Cancelled => {
            crate::tools::PermissionResponse::Deny
        }
        _ => crate::tools::PermissionResponse::Deny,
    }
}

async fn request_questions(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    tool_call_id: Option<&str>,
    questions: &serde_json::Value,
    cancellation: &CancellationToken,
) -> serde_json::Value {
    let form = acp_question_form(session_id, tool_call_id, questions);
    let request = connection.send_request(form.request).block_task();
    tokio::pin!(request);
    let response = tokio::select! {
        _ = cancellation.cancelled() => return skipped_question_answers(questions),
        response = &mut request => response,
    };
    let Ok(response) = response else {
        return skipped_question_answers(questions);
    };
    acp_question_answers(&form.fields, response.action)
}

async fn bridge_terminal_session(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    session_cwd: &Path,
    request: crate::tools::TerminalSessionRequest,
    cancellation: &CancellationToken,
) {
    let start = request.start;
    let control_tx = request.control_tx;
    let cwd = start
        .workdir
        .as_deref()
        .map(PathBuf::from)
        .map(|path| absolute_tool_path(&path.to_string_lossy(), session_cwd))
        .unwrap_or_else(|| session_cwd.to_path_buf());
    let create = CreateTerminalRequest::new(session_id.to_string(), "bash")
        .args(vec!["-c".to_string(), start.command.clone()])
        .cwd(cwd)
        .output_byte_limit(crate::tools::terminal_session::MAX_TRANSCRIPT_BYTES as u64);
    let terminal_id = match connection.send_request(create).block_task().await {
        Ok(response) => response.terminal_id,
        Err(error) => {
            let _ = control_tx.send(crate::tools::TerminalSessionControl::ExternalError(
                format!("ACP client could not create terminal: {error}"),
            ));
            return;
        }
    };

    let terminal_update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
        start.tool_call_id.clone(),
        ToolCallUpdateFields::new().content(vec![ToolCallContent::Terminal(Terminal::new(
            terminal_id.clone(),
        ))]),
    ));
    if connection
        .send_notification(SessionNotification::new(
            session_id.to_string(),
            terminal_update,
        ))
        .is_err()
    {
        let _ = connection
            .send_request(ReleaseTerminalRequest::new(
                session_id.to_string(),
                terminal_id,
            ))
            .block_task()
            .await;
        let _ = control_tx.send(crate::tools::TerminalSessionControl::ExternalError(
            "ACP client could not embed terminal".to_string(),
        ));
        return;
    }

    let wait = connection
        .send_request(WaitForTerminalExitRequest::new(
            session_id.to_string(),
            terminal_id.clone(),
        ))
        .block_task();
    tokio::pin!(wait);
    let (stopped_by_user, exit_code, wait_error) = tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = connection
                .send_request(KillTerminalRequest::new(session_id.to_string(), terminal_id.clone()))
                .block_task()
                .await;
            (true, None, None)
        }
        response = &mut wait => match response {
            Ok(response) => (
                false,
                response.exit_status.exit_code.and_then(|code| i32::try_from(code).ok()),
                None,
            ),
            Err(error) => (false, None, Some(error.to_string())),
        }
    };

    let output = connection
        .send_request(TerminalOutputRequest::new(
            session_id.to_string(),
            terminal_id.clone(),
        ))
        .block_task()
        .await;
    let _ = connection
        .send_request(ReleaseTerminalRequest::new(
            session_id.to_string(),
            terminal_id,
        ))
        .block_task()
        .await;

    match (wait_error, output) {
        (None, Ok(output)) => {
            let result = crate::tools::terminal_session::external_terminal_result(
                &start,
                &output.output,
                output.truncated,
                exit_code,
                stopped_by_user,
            );
            let _ = control_tx.send(crate::tools::TerminalSessionControl::ExternalResult(result));
        }
        (Some(error), _) => {
            let _ = control_tx.send(crate::tools::TerminalSessionControl::ExternalError(
                format!("ACP terminal wait failed: {error}"),
            ));
        }
        (None, Err(error)) => {
            let _ = control_tx.send(crate::tools::TerminalSessionControl::ExternalError(
                format!("ACP terminal output failed: {error}"),
            ));
        }
    }
}

fn permission_title(prompt: &crate::tools::PermissionPrompt) -> String {
    prompt
        .command
        .as_deref()
        .or(prompt.target.as_deref())
        .unwrap_or(&prompt.reason)
        .to_string()
}

fn send_tool_result(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    result: crate::llm::ToolCallResult,
    cwd: &Path,
) -> Result<(), Error> {
    let payload = serde_json::from_str::<serde_json::Value>(&result.content).unwrap_or_else(
        |_| serde_json::json!({ "status": "error", "output_preview": result.content }),
    );
    let status = match payload.get("status").and_then(serde_json::Value::as_str) {
        Some("ok") => ToolCallStatus::Completed,
        _ => ToolCallStatus::Failed,
    };
    let content = tool_result_content(&payload, cwd);
    let locations = tool_result_locations(&payload, cwd);
    let mut fields = ToolCallUpdateFields::new()
        .status(status)
        .content((!content.is_empty()).then_some(content))
        .raw_output(payload);
    if !locations.is_empty() {
        fields = fields.locations(locations);
    }
    let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(result.tool_call_id, fields));
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

fn send_usage(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    session: &AcpSession,
    used: usize,
    cost: Option<f64>,
) -> Result<(), Error> {
    let Some(size) = session.context_window else {
        return Ok(());
    };
    let update = SessionUpdate::UsageUpdate(
        UsageUpdate::new(used as u64, size as u64)
            .cost(cost.map(|amount| AcpCost::new(amount, "USD"))),
    );
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

fn replay_messages(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    messages: &[crate::session::types::Message],
    cwd: &Path,
) -> Result<(), Error> {
    for message in messages {
        if crate::session::compaction::is_compaction_display_item(message) {
            continue;
        }
        let message_id = message.id.clone();
        match message.role {
            crate::session::types::MessageRole::User => {
                if !message.content.is_empty() {
                    send_replay_text(
                        connection,
                        session_id,
                        &message_id,
                        &message.content,
                        true,
                        false,
                    )?;
                }
            }
            crate::session::types::MessageRole::Assistant => {
                for part in &message.parts {
                    match part.part_type.as_str() {
                        "text" => {
                            if let Some(text) = part.text_value() {
                                send_replay_text(
                                    connection,
                                    session_id,
                                    &message_id,
                                    text,
                                    false,
                                    false,
                                )?;
                            }
                        }
                        "reasoning" => {
                            if let Some(text) = part.text_value() {
                                send_replay_text(
                                    connection,
                                    session_id,
                                    &message_id,
                                    text,
                                    false,
                                    true,
                                )?;
                            }
                        }
                        "tool_call" => replay_tool_call(connection, session_id, part, cwd)?,
                        "tool_result" => replay_tool_result(connection, session_id, part, cwd)?,
                        _ => {}
                    }
                }
            }
            crate::session::types::MessageRole::System
            | crate::session::types::MessageRole::Tool => {}
        }
    }
    Ok(())
}

fn send_replay_text(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    message_id: &str,
    text: &str,
    user: bool,
    thought: bool,
) -> Result<(), Error> {
    let chunk = ContentChunk::new(text.to_string().into()).message_id(message_id);
    let update = if user {
        SessionUpdate::UserMessageChunk(chunk)
    } else if thought {
        SessionUpdate::AgentThoughtChunk(chunk)
    } else {
        SessionUpdate::AgentMessageChunk(chunk)
    };
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

fn replay_tool_call(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    part: &crate::session::types::MessagePart,
    cwd: &Path,
) -> Result<(), Error> {
    let Some(tool_call_id) = part.tool_id() else {
        return Ok(());
    };
    let name = part.tool_name().unwrap_or("tool");
    let input = part
        .data
        .get("args")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let status = match part.tool_status() {
        Some("completed") | Some("ok") => ToolCallStatus::Completed,
        Some("error") | Some("failed") => ToolCallStatus::Failed,
        Some("running") => ToolCallStatus::InProgress,
        _ => ToolCallStatus::Pending,
    };
    let update = SessionUpdate::ToolCall(
        ToolCall::new(tool_call_id.to_string(), tool_title(name, &input))
            .kind(tool_kind(name))
            .status(status)
            .locations(tool_locations(name, &input, cwd))
            .raw_input(input),
    );
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

fn replay_tool_result(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    part: &crate::session::types::MessagePart,
    cwd: &Path,
) -> Result<(), Error> {
    let Some(tool_call_id) = part.tool_id() else {
        return Ok(());
    };
    let content = part
        .data
        .get("content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    send_tool_result(
        connection,
        session_id,
        crate::llm::ToolCallResult {
            tool_call_id: tool_call_id.to_string(),
            role: "tool".to_string(),
            name: part.tool_name().unwrap_or("tool").to_string(),
            content,
        },
        cwd,
    )
}

fn tool_kind(tool_name: &str) -> ToolKind {
    match tool_name {
        "bash" | "bash_output" | "bash_kill" | "bash_restart" | "terminal_session" => {
            ToolKind::Execute
        }
        "webfetch" => ToolKind::Fetch,
        "grep" | "glob" | "context" => ToolKind::Search,
        "read" | "view_image" => ToolKind::Read,
        "edit" | "write" | "write_files" | "apply_patch" => ToolKind::Edit,
        "task" => ToolKind::Think,
        _ => ToolKind::Other,
    }
}

fn tool_title(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "bash" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(tool_name)
            .to_string(),
        "read" | "edit" | "write" | "grep" | "glob" => input
            .get("filePath")
            .or_else(|| input.get("filepath"))
            .or_else(|| input.get("path"))
            .or_else(|| input.get("pattern"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(tool_name)
            .to_string(),
        _ => tool_name.to_string(),
    }
}

fn system_time_to_iso8601(value: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(value).to_rfc3339()
}

fn internal_error() -> Error {
    Error::internal_error().data("Crabcode ACP operation failed")
}

fn internal_error_with(error: &str) -> Error {
    Error::internal_error().data(format!("Crabcode ACP operation failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(
        provider_id: &str,
        provider_name: &str,
        id: &str,
        name: &str,
    ) -> crate::model::types::Model {
        crate::model::types::Model {
            id: id.to_string(),
            name: name.to_string(),
            family: String::new(),
            provider_id: provider_id.to_string(),
            provider_name: provider_name.to_string(),
            attachment: false,
            structured_output: false,
            free: false,
            local: false,
            reasoning_options: Vec::new(),
        }
    }

    fn reasoning_model() -> crate::model::types::Model {
        let mut model = model("openai", "OpenAI", "o3", "o3");
        model.reasoning_options = vec![crate::model::reasoning::ReasoningOption {
            kind: "effort".to_string(),
            values: vec!["low".to_string(), "medium".to_string(), "high".to_string()],
        }];
        model
    }

    fn test_session() -> AcpSession {
        AcpSession {
            cwd: PathBuf::from("/tmp"),
            config: crate::config::configuration::LoadedConfig {
                merged_config: crate::config::configuration::MergedConfig::default(),
                raw_merged: serde_json::Value::Null,
                diagnostics: Default::default(),
                inventory: Default::default(),
                project_root: PathBuf::from("/tmp"),
                cwd: PathBuf::from("/tmp"),
                xdg_config_home: PathBuf::from("/tmp"),
            },
            skills: crate::skill::SkillStore::load(Path::new("/tmp"), Path::new("/tmp")),
            models: vec![model("example", "Example", "chat", "Chat")],
            provider: "example".to_string(),
            model: "chat".to_string(),
            agent: "Build".to_string(),
            reasoning_selection: crate::model::reasoning::ReasoningEffort::High,
            reasoning: None,
            context_window: None,
            cancellation: None,
        }
    }

    #[test]
    fn maps_crabcode_tools_to_acp_kinds() {
        assert_eq!(tool_kind("bash"), ToolKind::Execute);
        assert_eq!(tool_kind("read"), ToolKind::Read);
        assert_eq!(tool_kind("apply_patch"), ToolKind::Edit);
        assert_eq!(tool_kind("unknown"), ToolKind::Other);
    }

    #[test]
    fn acp_usage_update_includes_cumulative_usd_cost() {
        let update = UsageUpdate::new(1_000, 200_000).cost(AcpCost::new(0.125, "USD"));
        assert_eq!(update.cost.as_ref().map(|cost| cost.amount), Some(0.125));
        assert_eq!(
            update.cost.as_ref().map(|cost| cost.currency.as_str()),
            Some("USD")
        );
    }

    #[test]
    fn terminal_support_tracks_client_capability() {
        let service = AcpService::new(Path::new("/tmp")).unwrap();
        assert!(!service.supports_terminals());

        service.set_client_capabilities(
            agent_client_protocol::schema::v1::ClientCapabilities::new().terminal(true),
        );
        assert!(service.supports_terminals());
    }

    #[test]
    fn builds_titles_from_tool_input() {
        assert_eq!(
            tool_title("bash", &serde_json::json!({"command": "cargo test"})),
            "cargo test"
        );
        assert_eq!(
            tool_title("read", &serde_json::json!({"filePath": "src/main.rs"})),
            "src/main.rs"
        );
    }

    #[test]
    fn acp_tool_result_prefers_full_output() {
        let payload = serde_json::json!({
            "output": "complete tool output",
            "output_preview": "short preview",
        });

        assert_eq!(tool_result_text(&payload), "complete tool output");
    }

    #[test]
    fn acp_tool_result_supports_legacy_preview_payloads() {
        let payload = serde_json::json!({"output_preview": "legacy output"});

        assert_eq!(tool_result_text(&payload), "legacy output");
    }

    #[test]
    fn acp_tool_locations_normalize_multi_file_and_patch_paths() {
        let cwd = Path::new("/tmp/workspace");
        let write_locations = tool_locations(
            "write_files",
            &serde_json::json!({
                "files": [
                    {"file_path": "src/a.rs", "content": "a"},
                    {"file_path": "/tmp/b.rs", "content": "b"}
                ]
            }),
            cwd,
        );
        assert_eq!(
            write_locations[0].path,
            PathBuf::from("/tmp/workspace/src/a.rs")
        );
        assert_eq!(write_locations[1].path, PathBuf::from("/tmp/b.rs"));

        let patch_locations = tool_locations(
            "apply_patch",
            &serde_json::json!({
                "patch": "*** Begin Patch\n*** Update File: src/a.rs\n*** Add File: src/b.rs\n*** End Patch"
            }),
            cwd,
        );
        assert_eq!(patch_locations.len(), 2);
        assert_eq!(
            patch_locations[1].path,
            PathBuf::from("/tmp/workspace/src/b.rs")
        );
    }

    #[test]
    fn acp_tool_result_emits_diff_location_and_image_content() {
        let payload = serde_json::json!({
            "output": "updated",
            "metadata": {
                "path": "src/main.rs",
                "line_number": 4,
                "old_text": "fn old() {}",
                "new_text": "fn new() {}"
            },
            "images": [{
                "data_url": "data:image/png;base64,aGk=",
                "media_type": "image/png"
            }]
        });
        let cwd = Path::new("/tmp/workspace");

        let locations = tool_result_locations(&payload, cwd);
        assert_eq!(
            locations[0].path,
            PathBuf::from("/tmp/workspace/src/main.rs")
        );
        assert_eq!(locations[0].line, Some(4));

        let content = tool_result_content(&payload, cwd);
        let diff = content.iter().find_map(|item| match item {
            ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        });
        let diff = diff.expect("diff content");
        assert_eq!(diff.path, PathBuf::from("/tmp/workspace/src/main.rs"));
        assert_eq!(diff.old_text.as_deref(), Some("fn old() {}"));
        assert_eq!(diff.new_text, "fn new() {}");
        assert!(content.iter().any(|item| matches!(
            item,
            ToolCallContent::Content(content)
                if matches!(&content.content, ContentBlock::Image(image) if image.data == "aGk=" && image.mime_type == "image/png")
        )));
    }

    #[test]
    fn acp_permission_prefers_originating_tool_call_id() {
        assert_eq!(permission_tool_call_id(Some("call_123")), "call_123");
    }

    #[test]
    fn acp_permission_generates_fallback_id_without_origin() {
        assert!(permission_tool_call_id(None).starts_with("permission:"));
    }

    #[test]
    fn acp_question_form_preserves_single_multi_custom_and_scope() {
        let form = acp_question_form(
            "session_1",
            Some("question_call_1"),
            &serde_json::json!([
                {
                    "question": "Pick one",
                    "header": "Single",
                    "options": [
                        {"label": "A", "description": "First"},
                        {"label": "B", "description": "Second"}
                    ]
                },
                {
                    "question": "Pick several",
                    "header": "Multiple",
                    "multiple": true,
                    "options": [
                        {"label": "X", "description": "First"},
                        {"label": "Y", "description": "Second"}
                    ]
                }
            ]),
        );
        let wire = serde_json::to_value(&form.request).expect("elicitation request");

        assert_eq!(wire["mode"], "form");
        assert_eq!(wire["sessionId"], "session_1");
        assert_eq!(wire["toolCallId"], "question_call_1");
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_0"]["type"],
            "string"
        );
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_0"]["oneOf"][0]["title"],
            "A"
        );
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_1"]["type"],
            "array"
        );
        assert_eq!(
            wire["requestedSchema"]["properties"]["question_1_custom"]["type"],
            "string"
        );
    }

    #[test]
    fn acp_question_answers_restore_labels_and_custom_text() {
        let form = acp_question_form(
            "session_1",
            None,
            &serde_json::json!([
                {
                    "question": "Pick one",
                    "options": [{"label": "A"}, {"label": "B"}]
                },
                {
                    "question": "Pick several",
                    "multiple": true,
                    "options": [{"label": "X"}, {"label": "Y"}]
                }
            ]),
        );
        let mut content = std::collections::BTreeMap::new();
        content.insert(
            "question_0".to_string(),
            ElicitationContentValue::String("q0_option_1".to_string()),
        );
        content.insert(
            "question_1".to_string(),
            ElicitationContentValue::StringArray(vec![
                "q1_option_0".to_string(),
                "q1_option_1".to_string(),
            ]),
        );
        content.insert(
            "question_1_custom".to_string(),
            ElicitationContentValue::String("Other choice".to_string()),
        );
        let action = ElicitationAction::Accept(
            agent_client_protocol::schema::v1::ElicitationAcceptAction::new().content(content),
        );

        assert_eq!(
            acp_question_answers(&form.fields, action),
            serde_json::json!([["B"], ["X", "Y", "Other choice"]])
        );
        assert_eq!(
            acp_question_answers(&form.fields, ElicitationAction::Cancel),
            serde_json::json!([[], []])
        );

        let mut custom_content = std::collections::BTreeMap::new();
        custom_content.insert(
            "question_0".to_string(),
            ElicitationContentValue::String("q0_option_0".to_string()),
        );
        custom_content.insert(
            "question_0_custom".to_string(),
            ElicitationContentValue::String("Custom only".to_string()),
        );
        let custom_action = ElicitationAction::Accept(
            agent_client_protocol::schema::v1::ElicitationAcceptAction::new()
                .content(custom_content),
        );
        assert_eq!(
            acp_question_answers(&form.fields, custom_action),
            serde_json::json!([["Custom only"], []])
        );
    }

    #[test]
    fn maps_typed_turn_stop_reasons_to_acp() {
        assert_eq!(
            acp_stop_reason(Some(crate::llm::TurnStopReason::MaxTokens)),
            StopReason::MaxTokens
        );
        assert_eq!(
            acp_stop_reason(Some(crate::llm::TurnStopReason::Refusal)),
            StopReason::Refusal
        );
        assert_eq!(acp_stop_reason(None), StopReason::EndTurn);
    }

    #[test]
    fn recognizes_only_exact_compact_control_command() {
        assert_eq!(compact_command("/compact").unwrap(), true);
        assert_eq!(compact_command("  /compact  ").unwrap(), true);
        assert_eq!(compact_command("/compactness").unwrap(), false);
        assert_eq!(compact_command("hello").unwrap(), false);
        assert!(compact_command("/compact extra").is_err());
    }

    #[test]
    fn advertises_compact_as_no_input_command() {
        let session = test_session();
        let command = available_commands(&session)
            .into_iter()
            .find(|command| command.name == "compact")
            .expect("compact command");
        assert_eq!(
            command.description,
            "Summarize this session to reduce context"
        );
        assert!(command.input.is_none());
    }

    #[test]
    fn builds_smaller_soft_compaction_for_acp() {
        let session = test_session();
        let messages = vec![
            crate::session::types::Message::user("u".repeat(8_000)),
            crate::session::types::Message::assistant("a".repeat(8_000)),
            crate::session::types::Message::user("recent"),
        ];
        let selection = crate::session::compaction::select_messages_for_compaction_with_min(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
            0,
        )
        .expect("compaction selection");
        let before_tokens = crate::session::compaction::total_context_tokens(&messages);
        let before_messages =
            crate::session::compaction::filter_messages_for_context(&messages).len();

        let (compacted, stats) = compacted_messages(
            &messages,
            &selection,
            "short handoff",
            &session,
            before_tokens,
            before_messages,
        )
        .expect("smaller compaction");

        assert!(stats.after_tokens < stats.before_tokens);
        assert!(crate::session::compaction::latest_compaction_stats(&compacted).is_some());
        assert_eq!(compacted[0].id, messages[0].id);
    }

    #[test]
    fn flattens_text_and_embedded_context() {
        let text = prompt_text(vec![
            ContentBlock::from("Inspect this."),
            ContentBlock::Resource(agent_client_protocol::schema::v1::EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(
                    agent_client_protocol::schema::v1::TextResourceContents::new(
                        "fn main() {}",
                        "file:///tmp/main.rs",
                    ),
                ),
            )),
        ])
        .expect("prompt text");

        assert_eq!(text, "Inspect this.[file:///tmp/main.rs]\nfn main() {}");
    }

    #[test]
    fn writes_supported_acp_image_to_managed_session_storage() {
        let session_id = format!("acp-image-{}", cuid2::create_id());
        let image = agent_client_protocol::schema::v1::ImageContent::new("aGk=", "image/png");
        let path = write_prompt_image(&session_id, &image).expect("image file");

        assert!(Path::new(&path).starts_with(crate::persistence::attachments::root_dir()));
        assert_eq!(std::fs::read(&path).expect("image bytes"), b"hi");
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn writes_supported_acp_audio_to_managed_session_storage() {
        let session_id = format!("acp-audio-{}", cuid2::create_id());
        let audio = agent_client_protocol::schema::v1::AudioContent::new("YXVkaW8=", "audio/wav");
        let path = write_prompt_audio(&session_id, &audio).expect("audio file");

        assert!(path.ends_with(".wav"));
        assert_eq!(std::fs::read(&path).unwrap(), b"audio");
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn rejects_audio_for_models_without_audio_modality() {
        let session_id = format!("acp-audio-{}", cuid2::create_id());
        let result = prompt_content(
            vec![ContentBlock::Audio(
                agent_client_protocol::schema::v1::AudioContent::new("YXVkaW8=", "audio/wav"),
            )],
            false,
            false,
            &session_id,
            &test_session(),
        );

        assert!(result.is_err());
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn writes_acp_clipboard_image_data_uri_to_managed_storage() {
        let session_id = format!("acp-image-{}", cuid2::create_id());
        let image = agent_client_protocol::schema::v1::ImageContent::new(
            "data:image/png;base64,aGk=",
            "application/octet-stream",
        );
        let path = write_prompt_image(&session_id, &image).expect("image file");

        assert!(path.ends_with(".png"));
        assert_eq!(std::fs::read(&path).expect("image bytes"), b"hi");
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn prompt_image_failure_rolls_back_prior_managed_files() {
        let session_id = format!("acp-image-{}", cuid2::create_id());
        let result = prompt_content(
            vec![
                ContentBlock::Image(agent_client_protocol::schema::v1::ImageContent::new(
                    "aGk=",
                    "image/png",
                )),
                ContentBlock::Image(agent_client_protocol::schema::v1::ImageContent::new(
                    "not-base64",
                    "image/png",
                )),
            ],
            true,
            false,
            &session_id,
            &test_session(),
        );

        assert!(result.is_err());
        let directory = crate::persistence::attachments::session_dir(&session_id).unwrap();
        assert!(
            !directory.exists()
                || std::fs::read_dir(&directory)
                    .unwrap()
                    .all(|entry| entry.is_err())
        );
        crate::persistence::attachments::cleanup_session(&session_id).unwrap();
    }

    #[test]
    fn rejects_non_base64_acp_image_data_uri() {
        let image = agent_client_protocol::schema::v1::ImageContent::new(
            "data:image/png,not-base64",
            "image/png",
        );

        assert!(write_prompt_image("test", &image).is_err());
    }

    #[test]
    fn rejects_unsupported_acp_image_mime_type() {
        let image = agent_client_protocol::schema::v1::ImageContent::new("aGk=", "image/tiff");

        assert!(write_prompt_image("test", &image).is_err());
    }

    fn config_with_command(command: crate::command::custom::CustomCommand) -> LoadedConfig {
        let mut merged_config = crate::config::configuration::MergedConfig::default();
        merged_config.commands.push(command);
        LoadedConfig {
            merged_config,
            raw_merged: serde_json::Value::Null,
            diagnostics: Default::default(),
            inventory: Default::default(),
            project_root: PathBuf::from("/tmp"),
            cwd: PathBuf::from("/tmp"),
            xdg_config_home: PathBuf::from("/tmp"),
        }
    }

    fn session_with_config(config: LoadedConfig) -> AcpSession {
        let skills = crate::skill::SkillStore::load(&config.xdg_config_home, &config.project_root);
        AcpSession {
            cwd: config.cwd.clone(),
            config,
            skills,
            models: Vec::new(),
            provider: String::new(),
            model: String::new(),
            agent: "Build".to_string(),
            reasoning_selection: crate::model::reasoning::ReasoningEffort::None,
            reasoning: None,
            context_window: None,
            cancellation: None,
        }
    }

    #[test]
    fn advertises_custom_commands_with_argument_input() {
        let config = config_with_command(crate::command::custom::CustomCommand {
            name: "review".to_string(),
            description: Some("Review selected code".to_string()),
            template: "Review: $ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });

        let session = session_with_config(config);
        let commands = available_commands(&session);

        let command = commands
            .iter()
            .find(|command| command.name == "review")
            .expect("review command");
        assert!(commands.iter().any(|command| command.name == "skills"));
        assert!(commands.iter().any(|command| command.name == "mcp"));
        assert_eq!(command.description, "Review selected code");
        assert!(matches!(
            command.input,
            Some(AvailableCommandInput::Unstructured(_))
        ));
    }

    #[tokio::test]
    async fn expands_custom_slash_command_before_prompting() {
        let config = config_with_command(crate::command::custom::CustomCommand {
            name: "review".to_string(),
            description: None,
            template: "Review this carefully: $ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });

        let session = session_with_config(config);
        let prompt = expand_slash_command(&session, "/review src/acp/service.rs")
            .await
            .expect("expanded command");

        assert_eq!(prompt, "Review this carefully: src/acp/service.rs");
    }

    #[tokio::test]
    async fn advertises_and_expands_workspace_skills() {
        let temp = tempfile::tempdir().expect("temp dir");
        let skill_dir = temp.path().join(".opencode/skill/reviewer");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: reviewer\ndescription: Review code carefully\n---\nInspect correctness and risks.",
        )
        .expect("skill file");
        let mut config = config_with_command(crate::command::custom::CustomCommand {
            name: "custom".to_string(),
            description: None,
            template: "$ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(
                temp.path().join("opencode.jsonc"),
            ),
            workdir: temp.path().to_path_buf(),
        });
        config.project_root = temp.path().to_path_buf();
        config.cwd = temp.path().to_path_buf();
        config.xdg_config_home = temp.path().join("config");
        let session = session_with_config(config);

        let commands = available_commands(&session);
        assert!(commands.iter().any(|command| command.name == "reviewer"));
        let prompt = expand_slash_command(&session, "/reviewer src/lib.rs")
            .await
            .expect("expanded skill");
        assert!(prompt.contains("Inspect correctness and risks."));
        assert!(prompt.contains("src/lib.rs"));
    }

    #[tokio::test]
    async fn reports_configured_mcp_servers() {
        let mut config = config_with_command(crate::command::custom::CustomCommand {
            name: "custom".to_string(),
            description: None,
            template: "$ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });
        config.merged_config.mcp.insert(
            "filesystem".to_string(),
            crate::config::configuration::McpServerConfig::Local(
                crate::config::configuration::McpLocalConfig {
                    command: vec!["mcp-server".to_string()],
                    cwd: None,
                    environment: Default::default(),
                    enabled: true,
                    timeout_ms: None,
                },
            ),
        );
        let session = session_with_config(config);

        let prompt = expand_slash_command(&session, "/mcp")
            .await
            .expect("mcp status");
        assert!(prompt.contains("filesystem (local, enabled)"));
    }

    #[test]
    fn merges_client_mcp_servers_into_session_config() {
        let mut config = config_with_command(crate::command::custom::CustomCommand {
            name: "custom".to_string(),
            description: None,
            template: "$ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });
        merge_acp_mcp_servers(
            &mut config,
            vec![serde_json::from_value(serde_json::json!({
                "name": "zed-fs",
                "command": "mcp-server",
                "args": ["--stdio"],
                "env": [{"name": "ROOT", "value": "/workspace"}]
            }))
            .expect("ACP stdio MCP server")],
        );

        let server = config
            .merged_config
            .mcp
            .get("zed-fs")
            .expect("merged ACP MCP server");
        assert_eq!(server.kind(), "local");
        assert!(server.enabled());
    }

    #[test]
    fn rejects_non_absolute_workspaces() {
        assert!(workspace_path(Path::new("relative")).is_err());
    }

    #[test]
    fn builds_grouped_model_config_option() {
        let option = model_config_option(
            &[
                model("anthropic", "Anthropic", "claude", "Claude"),
                model("openai", "OpenAI", "gpt-5", "GPT-5"),
                model("openai", "OpenAI", "gpt-5-mini", "GPT-5 Mini"),
            ],
            "openai",
            "gpt-5",
        );

        assert_eq!(option.id.to_string(), "model");
        assert_eq!(option.category, Some(SessionConfigOptionCategory::Model));
        let agent_client_protocol::schema::v1::SessionConfigKind::Select(select) = option.kind
        else {
            panic!("model option should be a select");
        };
        assert_eq!(select.current_value.to_string(), "openai/gpt-5");
        let agent_client_protocol::schema::v1::SessionConfigSelectOptions::Grouped(groups) =
            select.options
        else {
            panic!("model options should be grouped");
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].name, "OpenAI");
        assert_eq!(groups[1].options[1].value.to_string(), "openai/gpt-5-mini");
    }

    #[test]
    fn validates_model_refs_against_selectable_catalog() {
        let models = [model("openai", "OpenAI", "gpt-5", "GPT-5")];

        assert_eq!(
            find_selectable_model(&models, "openai/gpt-5")
                .expect("known model")
                .id,
            "gpt-5"
        );
        assert!(find_selectable_model(&models, "openai/missing").is_err());
        assert!(find_selectable_model(&models, "other/gpt-5").is_err());
    }

    #[test]
    fn preserves_selected_reasoning_effort_when_model_cannot_apply_it() {
        let session = test_session();
        let option = reasoning_config_option(&session);
        assert_eq!(option.id.to_string(), "effort");
        assert_eq!(option.name, "Effort");
        assert_eq!(
            option.category,
            Some(SessionConfigOptionCategory::ThoughtLevel)
        );
        let agent_client_protocol::schema::v1::SessionConfigKind::Select(select) = option.kind
        else {
            panic!("reasoning option should be a select");
        };
        assert_eq!(select.current_value.to_string(), "high");
        let agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(options) =
            select.options
        else {
            panic!("reasoning options should be ungrouped");
        };
        let values = options
            .iter()
            .map(|option| (option.value.to_string(), option.name.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                ("none".to_string(), "None".to_string()),
                ("low".to_string(), "Low".to_string()),
                ("medium".to_string(), "Medium".to_string()),
                ("high".to_string(), "High".to_string()),
                ("xhigh".to_string(), "Xhigh".to_string()),
                ("max".to_string(), "Max".to_string()),
            ]
        );
    }
}
