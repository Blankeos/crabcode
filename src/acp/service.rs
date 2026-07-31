use crate::config::configuration::LoadedConfig;
use crate::session::manager::SessionManager;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, EmbeddedResourceResource, ListSessionsResponse,
    LoadSessionResponse, NewSessionResponse, PermissionOption, PermissionOptionKind,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, ResumeSessionResponse,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectGroup,
    SessionConfigSelectOption, SessionInfo, SessionMode, SessionModeState, SessionNotification,
    SessionUpdate, SetSessionConfigOptionResponse, StopReason, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{Client, ConnectionTo, Error};
use base64::Engine as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AcpService {
    sessions: Arc<AsyncMutex<HashMap<String, AcpSession>>>,
    session_manager: Arc<Mutex<SessionManager>>,
}

#[derive(Clone)]
struct AcpSession {
    cwd: PathBuf,
    config: LoadedConfig,
    models: Vec<crate::model::types::Model>,
    provider: String,
    model: String,
    agent: String,
    reasoning: Option<crate::model::reasoning::ReasoningEffort>,
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
        })
    }

    pub async fn new_session(&self, cwd: PathBuf) -> Result<NewSessionResponse, Error> {
        let cwd = workspace_path(&cwd)?;
        let config = crate::config::ConfigLoader::load_for(&cwd).map_err(|_| internal_error())?;
        crate::skill::init_skill_store(&config.xdg_config_home, &config.project_root);
        let (provider, model) = resolve_model(&config);
        let models = crate::model::catalog::selectable_models(&config, None)
            .await
            .map_err(|_| internal_error())?;
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

        self.sessions.lock().await.insert(
            session_id.clone(),
            AcpSession {
                cwd,
                config,
                models,
                provider,
                model,
                agent,
                reasoning: None,
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
        let (_session, messages) = self.attach_persisted_session(&session_id, cwd).await?;
        replay_messages(&connection, &session_id, &messages)?;
        Ok(LoadSessionResponse::new()
            .modes(session_modes(&_session))
            .config_options(session_config_options(&_session)))
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
            manager
                .replace_session_messages(&fork_id, messages)
                .map_err(|_| internal_error())?;
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
        session.reasoning = None;
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
        let session = AcpSession {
            cwd,
            config,
            models,
            provider,
            model,
            agent,
            reasoning: None,
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
        let (prompt, local_image_paths) = prompt_content(prompt, supports_images, &session)?;
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
        user_message.provider = Some(session.provider.clone());
        user_message.model = Some(session.model.clone());
        user_message.agent_mode = Some(session.agent.clone());
        {
            let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
            manager
                .add_message_to_session(&session_id, &user_message)
                .map_err(|_| internal_error())?;
            manager
                .set_session_status(
                    &session_id,
                    crate::session::types::SessionStatus::Streaming,
                    None,
                )
                .map_err(|_| internal_error())?;
        }
        messages.push(user_message);

        let prompt_registry = crate::tools::initialize_tool_registry_with_dynamic_config(
            None,
            tool_permissions(&session),
            session.config.merged_config.agent_registry.clone(),
            cancellation.clone(),
            Some(&session.provider),
            &session.config.merged_config.websearch,
            &session.config.merged_config.mcp,
            &session.cwd,
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
        .with_tool_registry(prompt_registry)
        .with_agent_registry(session.config.merged_config.agent_registry.clone())
        .with_active_agent(session.agent.clone())
        .compose()
        .await;
        messages.insert(0, crate::session::types::Message::system(system_prompt));

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let stream_session_id = session_id.clone();
        let stream_cancellation = cancellation.clone();
        let stream_sender = sender.clone();
        let stream_session = session.clone();
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
                messages,
                sender,
            )
            .await;
            if let Err(error) = result {
                let _ = stream_sender.send(crate::llm::ChunkMessage::Failed(error.to_string()));
            }
            let _ = stream_sender.send(crate::llm::ChunkMessage::End);
        });

        let message_id = cuid2::create_id();
        let mut assistant = crate::session::types::Message::incomplete("");
        assistant.provider = Some(session.provider.clone());
        assistant.model = Some(session.model.clone());
        assistant.agent_mode = Some(session.agent.clone());
        let mut failed = None;
        let mut cancelled = false;

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
                        send_tool_call(&connection, &session_id, tool_call)?;
                    }
                }
                crate::llm::ChunkMessage::ToolResult(result) => {
                    assistant.add_or_update_tool_result_part(serde_json::json!({
                        "id": result.tool_call_id,
                        "name": result.name,
                        "content": result.content,
                    }));
                    send_tool_result(&connection, &session_id, result)?;
                }
                crate::llm::ChunkMessage::Cancelled => cancelled = true,
                crate::llm::ChunkMessage::Failed(error) => failed = Some(error),
                crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                    let response = request_permission(&connection, &session_id, &prompt).await;
                    let _ = prompt.response_tx.send(response);
                }
                crate::llm::ChunkMessage::QuestionRequest { response_tx, .. } => {
                    let _ = response_tx.send(serde_json::json!({"skipped": true}));
                }
                crate::llm::ChunkMessage::TerminalSessionRequest(request) => {
                    let _ = request
                        .control_tx
                        .send(crate::tools::TerminalSessionControl::Stop);
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
        if failed.is_some() {
            return Err(internal_error());
        }
        Ok(PromptResponse::new(StopReason::EndTurn))
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

    vec![
        SessionConfigOption::select("mode", "Mode", session.agent.clone(), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        model_config_option(&session.models, &session.provider, &session.model),
    ]
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

static ACP_IMAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn prompt_content(
    parts: Vec<ContentBlock>,
    supports_images: bool,
    session: &AcpSession,
) -> Result<(String, Vec<String>), Error> {
    let mut text = String::new();
    let mut local_image_paths = Vec::new();
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
                local_image_paths.push(write_prompt_image(&image)?);
            }
            ContentBlock::Audio(_) => {
                return Err(Error::invalid_params().data("audio ACP prompts are not supported yet"));
            }
            _ => {}
        }
    }
    if text.is_empty() && !local_image_paths.is_empty() {
        text.push_str("[Image attached]");
    }
    Ok((text, local_image_paths))
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
    image: &agent_client_protocol::schema::v1::ImageContent,
) -> Result<String, Error> {
    const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

    let extension = match image.mime_type.as_str() {
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
        .decode(&image.data)
        .map_err(|error| Error::invalid_params().data(format!("invalid image data: {error}")))?;
    if data.len() > MAX_IMAGE_BYTES {
        return Err(Error::invalid_params().data("image exceeds the 20 MiB size limit"));
    }

    let directory = std::env::temp_dir().join("crabcode").join("acp-images");
    std::fs::create_dir_all(&directory).map_err(|_| internal_error())?;
    let sequence = ACP_IMAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("{}-{sequence}.{extension}", std::process::id()));
    std::fs::write(&path, data).map_err(|_| internal_error())?;
    Ok(path.to_string_lossy().into_owned())
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

fn send_tool_call(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    tool_call: crate::llm::ToolCall,
) -> Result<(), Error> {
    let raw_input = serde_json::from_str(&tool_call.function.arguments)
        .unwrap_or_else(|_| serde_json::json!({ "arguments": tool_call.function.arguments }));
    let title = tool_title(&tool_call.function.name, &raw_input);
    let update = SessionUpdate::ToolCall(
        ToolCall::new(tool_call.id, title)
            .kind(tool_kind(&tool_call.function.name))
            .status(ToolCallStatus::Pending)
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
    let tool_call_id = format!("permission:{}", cuid2::create_id());
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
) -> Result<(), Error> {
    let payload = serde_json::from_str::<serde_json::Value>(&result.content).unwrap_or_else(
        |_| serde_json::json!({ "status": "error", "output_preview": result.content }),
    );
    let status = match payload.get("status").and_then(serde_json::Value::as_str) {
        Some("ok") => ToolCallStatus::Completed,
        _ => ToolCallStatus::Failed,
    };
    let text = payload
        .get("output_preview")
        .or_else(|| payload.get("error"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let fields = ToolCallUpdateFields::new()
        .status(status)
        .content((!text.is_empty()).then(|| vec![ToolCallContent::from(text)]))
        .raw_output(payload);
    let update = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(result.tool_call_id, fields));
    connection
        .send_notification(SessionNotification::new(session_id.to_string(), update))
        .map_err(|_| internal_error())
}

fn replay_messages(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    messages: &[crate::session::types::Message],
) -> Result<(), Error> {
    for (message_index, message) in messages.iter().enumerate() {
        let message_id = format!("{session_id}:message:{message_index}");
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
                        "tool_call" => replay_tool_call(connection, session_id, part)?,
                        "tool_result" => replay_tool_result(connection, session_id, part)?,
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
    )
}

fn tool_kind(tool_name: &str) -> ToolKind {
    match tool_name {
        "bash" | "terminal_session" => ToolKind::Execute,
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

    #[test]
    fn maps_crabcode_tools_to_acp_kinds() {
        assert_eq!(tool_kind("bash"), ToolKind::Execute);
        assert_eq!(tool_kind("read"), ToolKind::Read);
        assert_eq!(tool_kind("apply_patch"), ToolKind::Edit);
        assert_eq!(tool_kind("unknown"), ToolKind::Other);
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
    fn writes_supported_acp_image_to_temp_file() {
        let image = agent_client_protocol::schema::v1::ImageContent::new("aGk=", "image/png");
        let path = write_prompt_image(&image).expect("image file");

        assert_eq!(std::fs::read(&path).expect("image bytes"), b"hi");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_unsupported_acp_image_mime_type() {
        let image = agent_client_protocol::schema::v1::ImageContent::new("aGk=", "image/tiff");

        assert!(write_prompt_image(&image).is_err());
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
}
