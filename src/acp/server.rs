use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthMethodTerminal, CancelNotification,
    ClientCapabilities, CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest,
    DeleteSessionResponse, ForkSessionRequest, ForkSessionResponse, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, LoadSessionRequest,
    McpCapabilities, NewSessionRequest, PromptCapabilities, PromptRequest, ResumeSessionRequest,
    SessionCapabilities, SessionCloseCapabilities, SessionDeleteCapabilities,
    SessionForkCapabilities, SessionListCapabilities, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest,
    SetSessionModeResponse,
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
    let initialize_service = service.clone();

    Agent
        .builder()
        .name("crabcode-acp")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                initialize_service.set_client_capabilities(request.client_capabilities.clone());
                let response = InitializeResponse::new(request.protocol_version)
                    .agent_capabilities(capabilities())
                    .agent_info(Implementation::new("crabcode", env!("CARGO_PKG_VERSION")))
                    .auth_methods(auth_methods(&request.client_capabilities));
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: DeleteSessionRequest, responder, _connection| {
                    responder.respond_with_result(
                        service
                            .delete_session(&request.session_id.to_string())
                            .await
                            .map(|_| DeleteSessionResponse::new()),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: ForkSessionRequest, responder, connection| {
                    let service = service.clone();
                    let task_connection = connection.clone();
                    connection.spawn(async move {
                        let result: Result<_, agent_client_protocol::Error> = async {
                            let response = service
                                .fork_session(request.session_id.to_string(), request.cwd)
                                .await?;
                            let session_id = response.session_id.clone();
                            let commands =
                                service.available_commands(&session_id.to_string()).await?;
                            Ok((response, session_id, commands))
                        }
                        .await;
                        match result {
                            Ok((response, session_id, commands)) => {
                                responder.respond(
                                    ForkSessionResponse::new(session_id.clone())
                                        .modes(response.modes)
                                        .config_options(response.config_options),
                                )?;
                                task_connection.send_notification(SessionNotification::new(
                                    session_id,
                                    SessionUpdate::AvailableCommandsUpdate(commands),
                                ))
                            }
                            Err(error) => responder.respond_with_result(Err(error)),
                        }
                    })
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
                        let result: Result<_, agent_client_protocol::Error> = async {
                            let response = service
                                .load_session(
                                    session_id.to_string(),
                                    request.cwd,
                                    task_connection.clone(),
                                )
                                .await?;
                            let commands =
                                service.available_commands(&session_id.to_string()).await?;
                            Ok((response, commands))
                        }
                        .await;
                        match result {
                            Ok((response, commands)) => {
                                responder.respond(response)?;
                                task_connection.send_notification(SessionNotification::new(
                                    session_id,
                                    SessionUpdate::AvailableCommandsUpdate(commands),
                                ))
                            }
                            Err(error) => responder.respond_with_result(Err(error)),
                        }
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
                        let result: Result<_, agent_client_protocol::Error> = async {
                            let response = service
                                .resume_session(session_id.to_string(), request.cwd)
                                .await?;
                            let commands =
                                service.available_commands(&session_id.to_string()).await?;
                            Ok((response, commands))
                        }
                        .await;
                        match result {
                            Ok((response, commands)) => {
                                responder.respond(response)?;
                                task_connection.send_notification(SessionNotification::new(
                                    session_id,
                                    SessionUpdate::AvailableCommandsUpdate(commands),
                                ))
                            }
                            Err(error) => responder.respond_with_result(Err(error)),
                        }
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
                        let result: Result<_, agent_client_protocol::Error> = async {
                            let response = service
                                .new_session(request.cwd, request.mcp_servers)
                                .await?;
                            let session_id = response.session_id.clone();
                            let commands =
                                service.available_commands(&session_id.to_string()).await?;
                            Ok((response, session_id, commands))
                        }
                        .await;
                        match result {
                            Ok((response, session_id, commands)) => {
                                responder.respond(response)?;
                                task_connection.send_notification(SessionNotification::new(
                                    session_id,
                                    SessionUpdate::AvailableCommandsUpdate(commands),
                                ))
                            }
                            Err(error) => responder.respond_with_result(Err(error)),
                        }
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let service = service.clone();
                async move |request: ListSessionsRequest, responder, _connection| {
                    responder.respond_with_result(
                        service.list_sessions(request.cwd, request.cursor).await,
                    )
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
        .prompt_capabilities(
            PromptCapabilities::new()
                .embedded_context(true)
                .image(true)
                .audio(true),
        )
        .mcp_capabilities(McpCapabilities::new().http(true).sse(true))
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .resume(SessionResumeCapabilities::new())
                .fork(SessionForkCapabilities::new())
                .close(SessionCloseCapabilities::new())
                .delete(SessionDeleteCapabilities::new()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_audio_prompt_support() {
        let capabilities = capabilities();
        let prompt = capabilities.prompt_capabilities;
        assert!(prompt.audio);
        assert!(prompt.image);
        assert!(capabilities.session_capabilities.delete.is_some());
    }
}

// Terminal auth is independent of terminal/* tool support. Older registry
// validators opt in via _meta instead of auth.terminal; do not label their
// legacy descriptor as a modern terminal method without the new capability.
fn auth_methods(client: &ClientCapabilities) -> Vec<AuthMethod> {
    let args = vec!["acp".to_owned(), "--login".to_owned()];
    if client.auth.terminal {
        return vec![AuthMethod::Terminal(
            AuthMethodTerminal::new("crabcode-login", "Connect a provider")
                .description("Connect with OAuth or an API key, then reconnect Crabcode")
                .args(vec!["--login".to_owned()]),
        )];
    }
    if client
        .meta
        .as_ref()
        .and_then(|meta| meta.get("terminal-auth"))
        == Some(&serde_json::Value::Bool(true))
    {
        let mut meta = serde_json::Map::new();
        meta.insert("terminal-auth".into(), serde_json::json!({"args": args}));
        return vec![AuthMethod::Agent(
            AuthMethodAgent::new("crabcode-login", "Connect a provider")
                .description("Run crabcode acp --login in a terminal, then reconnect")
                .meta(meta),
        )];
    }
    // No protocol-driven credential flow is implemented. Do not pretend that
    // authenticate can collect credentials on clients without terminal auth.
    Vec::new()
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    fn methods(caps: serde_json::Value) -> serde_json::Value {
        let client = serde_json::from_value(caps).unwrap();
        serde_json::to_value(auth_methods(&client)).unwrap()
    }

    #[test]
    fn modern_terminal_auth_uses_login_only_args() {
        let value = methods(serde_json::json!({"auth": {"terminal": true}}));
        assert_eq!(value[0]["type"], "terminal");
        assert_eq!(value[0]["args"], serde_json::json!(["--login"]));
        assert_eq!(value[0]["id"], "crabcode-login");
        assert!(value[0].get("command").is_none());
        assert!(value[0].get("env").is_none());
    }

    #[test]
    fn registry_legacy_opt_in_is_supported() {
        let value = methods(serde_json::json!({
            "terminal": true,
            "fs": {"readTextFile": true, "writeTextFile": true},
            "_meta": {"terminal_output": true, "terminal-auth": true}
        }));
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert!(value[0].get("type").is_none());
        assert_eq!(
            value[0]["_meta"]["terminal-auth"]["args"],
            serde_json::json!(["acp", "--login"])
        );
    }

    #[test]
    fn terminal_tools_do_not_imply_terminal_auth() {
        for caps in [
            serde_json::json!({}),
            serde_json::json!({"terminal": true}),
            serde_json::json!({"auth": {"terminal": false}}),
            serde_json::json!({"_meta": {"terminal-auth": false}}),
        ] {
            assert_eq!(methods(caps), serde_json::json!([]));
        }
    }
}
