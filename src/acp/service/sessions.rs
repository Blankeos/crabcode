use super::config::session_config_options;
use super::internal_error;
use super::mcp_config::merge_acp_mcp_servers;
use super::models::model_context_window;
use super::models::model_reasoning;
use super::models::resolve_model;
use super::modes::session_modes;
use super::permissions::configured_tool_permissions;
use super::replay::replay_messages;
use super::AcpService;
use super::AcpSession;
use agent_client_protocol::schema::v1::ListSessionsResponse;
use agent_client_protocol::schema::v1::LoadSessionResponse;
use agent_client_protocol::schema::v1::McpServer;
use agent_client_protocol::schema::v1::NewSessionResponse;
use agent_client_protocol::schema::v1::ResumeSessionResponse;
use agent_client_protocol::schema::v1::SessionInfo;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use std::path::Path;
use std::path::PathBuf;

fn acp_session_info(
    session: crate::session::manager::SessionInfo,
    root_id: Option<String>,
) -> SessionInfo {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "crabcode".to_string(),
        serde_json::json!({
            "parentSessionId": session.parent_id,
            "rootSessionId": root_id,
        }),
    );
    SessionInfo::new(session.id, session.workspace_path)
        .title(session.title)
        .updated_at(system_time_to_iso8601(session.updated_at))
        .meta(meta)
}

fn session_page(
    cursor: Option<&str>,
    total: usize,
) -> Result<(usize, usize, Option<String>), Error> {
    let offset = cursor
        .unwrap_or("0")
        .parse::<usize>()
        .map_err(|_| Error::invalid_params().data("invalid session list cursor"))?;
    let end = offset.saturating_add(100).min(total);
    let next_cursor = (end < total).then(|| end.to_string());
    Ok((offset.min(total), end, next_cursor))
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

fn system_time_to_iso8601(value: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(value).to_rfc3339()
}

impl AcpService {
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
        let context_window = model_context_window(&config, &models, &provider, &model);
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
        let tool_permissions = configured_tool_permissions(&cwd, &config);
        self.sessions.lock().await.attachments.insert(
            session_id.clone(),
            AcpSession {
                cwd,
                config,
                tool_permissions,
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
            .attachments
            .get(&session_id)
            .cloned()
            .ok_or_else(internal_error)?;
        Ok(NewSessionResponse::new(session_id)
            .modes(session_modes(&session))
            .config_options(session_config_options(&session)))
    }

    pub async fn list_sessions(
        &self,
        cwd: Option<PathBuf>,
        cursor: Option<String>,
    ) -> Result<ListSessionsResponse, Error> {
        let cwd = cwd.as_deref().map(workspace_path).transpose()?;
        let manager = self.session_manager.lock().map_err(|_| internal_error())?;
        let mut sessions = manager
            .list_sessions()
            .into_iter()
            .filter(|session| session.archived_at.is_none())
            .filter(|session| {
                cwd.as_ref()
                    .is_none_or(|cwd| session.workspace_path == cwd.to_string_lossy())
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));

        let (offset, end, next_cursor) = session_page(cursor.as_deref(), sessions.len())?;
        Ok(ListSessionsResponse::new(
            sessions
                .into_iter()
                .skip(offset)
                .take(end.saturating_sub(offset))
                .map(|session| {
                    let root_id = manager.root_session_id_for(&session.id);
                    acp_session_info(session, root_id)
                })
                .collect(),
        )
        .next_cursor(next_cursor))
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
            let source_title = manager
                .get_session(&session_id)
                .map(|session| session.title.clone())
                .unwrap_or_else(|| session_id.clone());
            manager
                .switch_current_workspace_path(&source.cwd.to_string_lossy())
                .map_err(|_| internal_error())?;
            let fork_id = manager.create_session(Some(format!("{source_title} (fork)")));
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
        self.sessions.lock().await.attachments.insert(
            fork_id.clone(),
            AcpSession {
                tool_permissions: configured_tool_permissions(&source.cwd, &source.config),
                ..source.clone()
            },
        );
        Ok(NewSessionResponse::new(fork_id)
            .modes(session_modes(&source))
            .config_options(session_config_options(&source)))
    }

    pub async fn close_session(&self, session_id: &str) {
        if let Some(session) = self.sessions.lock().await.attachments.remove(session_id) {
            if let Some(cancellation) = session.cancellation {
                cancellation.cancel();
            }
        }
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<(), Error> {
        self.close_session(session_id).await;
        let mut manager = self.session_manager.lock().map_err(|_| internal_error())?;
        match manager.try_delete_session(session_id) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::invalid_params().data("unknown session")),
            Err(_) => Err(internal_error()),
        }
    }

    pub async fn cancel_session(&self, session_id: &str) {
        if let Some(session) = self.sessions.lock().await.attachments.get(session_id) {
            if let Some(cancellation) = &session.cancellation {
                cancellation.cancel();
            }
        }
    }

    async fn attach_persisted_session(
        &self,
        session_id: &str,
        cwd: PathBuf,
    ) -> Result<(AcpSession, Vec<crate::session::types::Message>), Error> {
        // Fail promptly during close cleanup, before model/config discovery. Recheck
        // below after awaits and hold the lock through the actual attachment.
        self.sessions.lock().await.ensure_attachable(session_id)?;
        let cwd = workspace_path(&cwd)?;
        // Invalid load/resume requests must not wait on network model discovery.
        {
            let manager = self.session_manager.lock().map_err(|_| internal_error())?;
            let stored = manager
                .get_session_ref(session_id)
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
            if stored.workspace_path != cwd.to_string_lossy() {
                return Err(Error::invalid_params().data("session does not belong to cwd"));
            }
        }
        let config = crate::config::ConfigLoader::load_for(&cwd).map_err(|_| internal_error())?;
        crate::skill::init_skill_store(&config.xdg_config_home, &config.project_root);
        let models = crate::model::catalog::selectable_models(&config, None)
            .await
            .map_err(|_| internal_error())?;

        let mut sessions = self.sessions.lock().await;
        sessions.ensure_attachable(session_id)?;
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
        let context_window = model_context_window(&config, &models, &provider, &model);
        let skills = crate::skill::SkillStore::load(&config.xdg_config_home, &config.project_root);
        let tool_permissions = configured_tool_permissions(&cwd, &config);
        let session = AcpSession {
            cwd,
            config,
            tool_permissions,
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
        sessions
            .attachments
            .insert(session_id.to_string(), session.clone());
        Ok((session, messages))
    }
}

#[cfg(test)]
#[path = "tests/sessions.rs"]
mod tests;
