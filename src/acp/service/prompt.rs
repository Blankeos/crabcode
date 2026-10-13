use super::attachment_storage::ManagedAttachmentGuard;
use super::attachments::model_supports_audio;
use super::attachments::prompt_content;
use super::cancellable;
use super::command_expansion::expand_slash_command;
use super::command_expansion::SlashExpansion;
use super::commands::command_text;
use super::commands::compact_command;
use super::context::acp_context_tokens;
use super::internal_error;
use super::models::find_selectable_model;
use super::models::model_context_window;
use super::models::resolved_reasoning;
use super::permissions::tool_permissions;
use super::replay::send_replay_text;
use super::usage::acp_session_cost;
use super::usage::send_usage;
use super::AcpService;
use agent_client_protocol::schema::v1::ContentBlock;
use agent_client_protocol::schema::v1::PromptResponse;
use agent_client_protocol::schema::v1::StopReason;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use tokio_util::sync::CancellationToken;

pub(super) fn acp_request_messages(
    history: &[crate::session::types::Message],
    system_message: crate::session::types::Message,
) -> Vec<crate::session::types::Message> {
    let mut messages = crate::session::compaction::filter_messages_for_context(history);
    messages.insert(0, system_message);
    messages
}

impl AcpService {
    pub async fn prompt(
        &self,
        session_id: String,
        prompt: Vec<ContentBlock>,
        connection: ConnectionTo<Client>,
    ) -> Result<PromptResponse, Error> {
        let cancellation = self.sessions.lock().await.begin_prompt(&session_id)?;
        // Only individual blocking operations are cancellable. Dropping prompt_inner
        // would skip transcript persistence and leave the session marked active.
        let mut result = self
            .prompt_inner(session_id.clone(), prompt, connection, cancellation.clone())
            .await;
        if cancellation.is_cancelled() {
            result = Ok(PromptResponse::new(StopReason::Cancelled));
        }
        // Cover setup errors and local commands as well as stream/compaction exits.
        if let Ok(mut manager) = self.session_manager.lock() {
            let status = if cancellation.is_cancelled()
                || result
                    .as_ref()
                    .is_ok_and(|response| response.stop_reason == StopReason::Cancelled)
            {
                crate::session::types::SessionStatus::Interrupted
            } else if result.is_err() {
                crate::session::types::SessionStatus::Failed
            } else {
                crate::session::types::SessionStatus::Idle
            };
            if manager.get_session_ref(&session_id).is_some() {
                if manager
                    .set_session_status(&session_id, status, None)
                    .is_err()
                {
                    result = Err(internal_error());
                }
            }
        } else {
            result = Err(internal_error());
        }
        // Also stop any detached producer after an error or early return.
        cancellation.cancel();
        self.sessions.lock().await.finish_prompt(&session_id);
        result
    }

    async fn prompt_inner(
        &self,
        session_id: String,
        prompt: Vec<ContentBlock>,
        connection: ConnectionTo<Client>,
        cancellation: CancellationToken,
    ) -> Result<PromptResponse, Error> {
        let mut session = self
            .sessions
            .lock()
            .await
            .attachments
            .get(&session_id)
            .cloned()
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let compact_text = command_text(&prompt);
        if compact_command(&compact_text)? {
            if prompt
                .iter()
                .any(|part| !matches!(part, ContentBlock::Text(_)))
            {
                return Err(Error::invalid_params().data("/compact does not accept attachments"));
            }
            return self
                .compact_session(&session_id, session, connection, cancellation)
                .await;
        }
        let expansion = if compact_text.trim_start().starts_with('/') {
            let Some(expansion) =
                cancellable(&cancellation, expand_slash_command(&session, &compact_text)).await
            else {
                return Ok(PromptResponse::new(StopReason::Cancelled));
            };
            Some(expansion?)
        } else {
            None
        };
        let expanded_prompt = match expansion {
            Some(SlashExpansion::LocalResult(text)) => {
                if prompt
                    .iter()
                    .any(|part| !matches!(part, ContentBlock::Text(_)))
                {
                    return Err(Error::invalid_params()
                        .data("local ACP commands do not accept attachments"));
                }
                let message_id = cuid2::create_id();
                send_replay_text(&connection, &session_id, &message_id, &text, false, false)?;
                return Ok(PromptResponse::new(StopReason::EndTurn));
            }
            Some(SlashExpansion::Prompt {
                prompt,
                agent,
                model,
            }) => {
                if let Some(agent) = agent {
                    if session
                        .config
                        .merged_config
                        .agent_registry
                        .get(&agent)
                        .is_none()
                    {
                        return Err(Error::invalid_params()
                            .data(format!("custom command references unknown agent: {agent}")));
                    }
                    session.agent = agent;
                }
                if let Some(model_ref) = model {
                    let (provider, model) = crate::app::parse_model_ref(&model_ref);
                    let canonical = format!("{provider}/{model}");
                    let model = find_selectable_model(&session.models, &canonical)?;
                    session.provider.clone_from(&model.provider_id);
                    session.model.clone_from(&model.id);
                    session.reasoning = resolved_reasoning(&session, session.reasoning_selection);
                    session.context_window = model_context_window(
                        &session.config,
                        &session.models,
                        &session.provider,
                        &session.model,
                    );
                }
                Some(prompt)
            }
            Some(SlashExpansion::Btw(question)) => {
                if prompt
                    .iter()
                    .any(|part| !matches!(part, ContentBlock::Text(_)))
                {
                    return Err(Error::invalid_params().data("/btw does not accept attachments"));
                }
                return self
                    .btw_session(&session_id, session, question, connection, cancellation)
                    .await;
            }
            None => None,
        };
        let supports_images = session
            .models
            .iter()
            .find(|model| model.provider_id == session.provider && model.id == session.model)
            .is_some_and(|model| model.attachment);
        let supports_audio =
            model_supports_audio(&session.config, &session.provider, &session.model);
        let (prompt, local_image_paths, local_audio_paths) = prompt_content(
            prompt,
            expanded_prompt.as_deref(),
            supports_images,
            supports_audio,
            &session_id,
            &session,
        )?;
        let mut managed_paths = local_image_paths.clone();
        managed_paths.extend(local_audio_paths.clone());
        let mut attachment_guard = ManagedAttachmentGuard::new(managed_paths);
        if prompt.trim().is_empty() {
            return Err(Error::invalid_params().data("prompt must include text content"));
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
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let process_registry = std::sync::Arc::new(crate::tools::ProcessRegistry::new());
        let prompt_registry = cancellable(
            &cancellation,
            crate::tools::initialize_tool_registry_with_dynamic_config(
                Some(sender.clone()),
                tool_permissions(&session),
                session.config.merged_config.agent_registry.clone(),
                cancellation.clone(),
                Some(&session.provider),
                &session.config.merged_config.websearch,
                &session.config.merged_config.mcp,
                &session.cwd,
                process_registry.clone(),
            ),
        )
        .await;
        let Some(prompt_registry) = prompt_registry else {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        };
        let is_git_repo =
            crate::utils::git::is_git_repo(&session.cwd.to_string_lossy()).unwrap_or(false);
        let system_prompt = cancellable(
            &cancellation,
            crate::prompt::SystemPromptComposer::new(
                &session.model,
                session.cwd.to_string_lossy(),
                is_git_repo,
                std::env::consts::OS,
            )
            .with_tool_registry(prompt_registry.clone())
            .with_agent_registry(session.config.merged_config.agent_registry.clone())
            .with_active_agent(session.agent.clone())
            .compose(),
        )
        .await;
        let Some(system_prompt) = system_prompt else {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        };
        let system_message = crate::session::types::Message::system(system_prompt);
        let prompt_tokens = crate::session::compaction::message_context_tokens(&system_message);
        let auto_compacted = self
            .maybe_auto_compact_session(
                &session_id,
                &session,
                &connection,
                cancellation.clone(),
                prompt_tokens,
                crate::session::compaction::message_context_tokens(&user_message),
            )
            .await?;
        if cancellation.is_cancelled() {
            return Ok(PromptResponse::new(StopReason::Cancelled));
        }
        if auto_compacted {
            messages = self
                .session_manager
                .lock()
                .map_err(|_| internal_error())?
                .get_session_ref(&session_id)
                .map(|stored| stored.messages.clone())
                .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        }
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
        let base_context_tokens = acp_context_tokens(&messages, prompt_tokens);
        let base_cost = acp_session_cost(&messages);
        messages = acp_request_messages(&messages, system_message);
        send_usage(
            &connection,
            &session_id,
            &session,
            base_context_tokens,
            (base_cost > 0.0).then_some(base_cost),
            None,
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
                stream_session.config.merged_config.compaction.clone(),
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

        let response = self
            .receive_prompt_stream(
                &session_id,
                &session,
                &connection,
                &cancellation,
                receiver,
                base_context_tokens,
                prompt_tokens,
                base_cost,
            )
            .await?;
        if !cancellation.is_cancelled() && response.stop_reason != StopReason::Cancelled {
            let _ = self
                .maybe_auto_compact_session(
                    &session_id,
                    &session,
                    &connection,
                    cancellation.clone(),
                    prompt_tokens,
                    0,
                )
                .await?;
        }
        Ok(response)
    }
}
