use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, LoadSessionRequest, McpCapabilities, NewSessionRequest,
    PromptCapabilities, PromptRequest, ResumeSessionRequest, SessionCapabilities,
    SessionCloseCapabilities, SessionForkCapabilities, SessionListCapabilities,
    SessionNotification, SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionModeRequest, SetSessionModeResponse,
};
use agent_client_protocol::{Agent, Stdio};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub async fn run(cwd: Option<PathBuf>) -> Result<()> {
    let workspace = match cwd {
        Some(path) => path,
        None => std::env::current_dir().context("failed to determine current directory")?,
    };
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("invalid ACP workspace: {}", workspace.display()))?;

    // Validate workspace configuration before accepting editor requests. ACP
    // protocol bytes are written by the SDK; diagnostics must never use stdout.
    crate::config::ConfigLoader::load_for(&workspace).with_context(|| {
        format!(
            "failed to load Crabcode configuration for ACP workspace {}",
            workspace.display()
        )
    })?;
    let service = crate::acp::service::AcpService::new(&workspace)
        .map_err(|_| anyhow::anyhow!("failed to initialize ACP session storage"))?;

    Agent
        .builder()
        .name("crabcode-acp")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                let response = InitializeResponse::new(request.protocol_version)
                    .agent_capabilities(capabilities())
                    .agent_info(Implementation::new("crabcode", env!("CARGO_PKG_VERSION")));
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: ForkSessionRequest, responder, _connection| {
                    let result = service
                        .fork_session(request.session_id.to_string(), request.cwd)
                        .await
                        .map(|response| {
                            ForkSessionResponse::new(response.session_id)
                                .modes(response.modes)
                                .config_options(response.config_options)
                        });
                    responder.respond_with_result(result)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: SetSessionModeRequest, responder, _connection| {
                    let result = service
                        .set_mode(
                            &request.session_id.to_string(),
                            &request.mode_id.to_string(),
                        )
                        .await
                        .map(|_| SetSessionModeResponse::new());
                    responder.respond_with_result(result)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                    let result = match (
                        request.config_id.to_string().as_str(),
                        request.value.as_value_id(),
                    ) {
                        ("mode", Some(mode)) => {
                            service
                                .set_mode(&request.session_id.to_string(), &mode.to_string())
                                .await
                        }
                        ("model", Some(model)) => {
                            service
                                .set_model(&request.session_id.to_string(), &model.to_string())
                                .await
                        }
                        ("effort" | "reasoning_effort", Some(effort)) => {
                            service
                                .set_reasoning_effort(
                                    &request.session_id.to_string(),
                                    &effort.to_string(),
                                )
                                .await
                        }
                        ("mode" | "model" | "effort" | "reasoning_effort", None) => {
                            Err(agent_client_protocol::Error::invalid_params()
                                .data("config option value must be a string"))
                        }
                        _ => Err(agent_client_protocol::Error::invalid_params()
                            .data("unknown config option")),
                    };
                    responder.respond_with_result(result)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: LoadSessionRequest, responder, connection| {
                    let session_id = request.session_id;
                    let service = service.clone();
                    let task_connection = connection.clone();
                    connection.spawn(async move {
                        let response = service
                            .load_session(
                                session_id.to_string(),
                                request.cwd,
                                task_connection.clone(),
                            )
                            .await?;
                        let commands = service.available_commands(&session_id.to_string()).await?;
                        responder.respond(response)?;
                        task_connection.send_notification(SessionNotification::new(
                            session_id,
                            SessionUpdate::AvailableCommandsUpdate(commands),
                        ))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: ResumeSessionRequest, responder, connection| {
                    let session_id = request.session_id;
                    let service = service.clone();
                    let task_connection = connection.clone();
                    connection.spawn(async move {
                        let response = service
                            .resume_session(session_id.to_string(), request.cwd)
                            .await?;
                        let commands = service.available_commands(&session_id.to_string()).await?;
                        responder.respond(response)?;
                        task_connection.send_notification(SessionNotification::new(
                            session_id,
                            SessionUpdate::AvailableCommandsUpdate(commands),
                        ))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: NewSessionRequest, responder, connection| {
                    let service = service.clone();
                    let task_connection = connection.clone();
                    connection.spawn(async move {
                        let response = service
                            .new_session(request.cwd, request.mcp_servers)
                            .await?;
                        let session_id = response.session_id.clone();
                        let commands = service.available_commands(&session_id.to_string()).await?;
                        responder.respond(response)?;
                        task_connection.send_notification(SessionNotification::new(
                            session_id,
                            SessionUpdate::AvailableCommandsUpdate(commands),
                        ))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: ListSessionsRequest, responder, _connection| {
                    responder.respond_with_result(service.list_sessions(request.cwd).await)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: CloseSessionRequest, responder, _connection| {
                    service.close_session(&request.session_id.to_string()).await;
                    responder.respond(CloseSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: PromptRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    let service = service.clone();
                    let prompt_connection = connection.clone();
                    connection.spawn(async move {
                        let result = service
                            .prompt(session_id, request.prompt, prompt_connection)
                            .await;
                        responder.respond_with_result(result)
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let service = service.clone();
                async move |notification: CancelNotification, _connection| {
                    service
                        .cancel_session(&notification.session_id.to_string())
                        .await;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
        .map_err(|error| anyhow::anyhow!("ACP stdio server failed: {error}"))
}

fn capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new().embedded_context(true).image(true))
        .mcp_capabilities(McpCapabilities::new().http(true).sse(true))
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .resume(SessionResumeCapabilities::new())
                .fork(SessionForkCapabilities::new())
                .close(SessionCloseCapabilities::new()),
        )
}
