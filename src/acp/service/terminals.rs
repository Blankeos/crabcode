use super::cancellable;
use super::tool_locations::absolute_tool_path;
use agent_client_protocol::schema::v1::CreateTerminalRequest;
use agent_client_protocol::schema::v1::KillTerminalRequest;
use agent_client_protocol::schema::v1::ReleaseTerminalRequest;
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::schema::v1::SessionUpdate;
use agent_client_protocol::schema::v1::Terminal;
use agent_client_protocol::schema::v1::TerminalOutputRequest;
use agent_client_protocol::schema::v1::ToolCallContent;
use agent_client_protocol::schema::v1::ToolCallUpdate;
use agent_client_protocol::schema::v1::ToolCallUpdateFields;
use agent_client_protocol::schema::v1::WaitForTerminalExitRequest;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use std::path::Path;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(super) async fn bridge_terminal_session(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    session_cwd: &Path,
    request: crate::tools::TerminalSessionRequest,
    cancellation: &CancellationToken,
) {
    // Connection-owned task retains the create response even when the prompt stops.
    // Do not tokio::spawn SDK block_task futures: they need the connection task context.
    let worker_connection = connection.clone();
    let worker_session_id = session_id.to_string();
    let worker_cwd = session_cwd.to_path_buf();
    let worker_cancellation = cancellation.clone();
    let control_tx = request.control_tx.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    if connection
        .spawn(async move {
            terminal_session_worker(
                &worker_connection,
                &worker_session_id,
                &worker_cwd,
                request,
                &worker_cancellation,
            )
            .await;
            let _ = done_tx.send(());
            Ok(())
        })
        .is_err()
    {
        let _ = control_tx.send(crate::tools::TerminalSessionControl::Stop);
        return;
    }
    if cancellable(cancellation, done_rx).await.is_none() {
        let _ = control_tx.send(crate::tools::TerminalSessionControl::Stop);
    }
}

async fn cleanup_terminal(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    terminal_id: agent_client_protocol::schema::v1::TerminalId,
) {
    // Issue both requests even if a client never acknowledges kill.
    let kill = connection
        .send_request(KillTerminalRequest::new(
            session_id.to_string(),
            terminal_id.clone(),
        ))
        .block_task();
    let release = connection
        .send_request(ReleaseTerminalRequest::new(
            session_id.to_string(),
            terminal_id,
        ))
        .block_task();
    let _ = tokio::join!(kill, release);
}

async fn terminal_session_worker(
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

    if cancellation.is_cancelled() {
        cleanup_terminal(connection, session_id, terminal_id).await;
        return;
    }

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
        cleanup_terminal(connection, session_id, terminal_id).await;
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
            cleanup_terminal(connection, session_id, terminal_id).await;
            return;
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

    let output = cancellable(
        cancellation,
        connection
            .send_request(TerminalOutputRequest::new(
                session_id.to_string(),
                terminal_id.clone(),
            ))
            .block_task(),
    )
    .await;
    let Some(output) = output else {
        cleanup_terminal(connection, session_id, terminal_id).await;
        return;
    };
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

#[cfg(test)]
mod tests {
    use super::super::AcpService;
    use super::*;
    use agent_client_protocol::schema::v1::CreateTerminalRequest;
    use agent_client_protocol::schema::v1::KillTerminalRequest;
    use agent_client_protocol::schema::v1::ReleaseTerminalRequest;

    use agent_client_protocol::Client;
    use agent_client_protocol::ConnectionTo;

    use std::path::Path;

    use tokio_util::sync::CancellationToken;

    #[test]
    fn terminal_support_tracks_client_capability() {
        let service = AcpService::new(Path::new("/tmp")).unwrap();
        assert!(!service.supports_terminals());

        service.set_client_capabilities(
            agent_client_protocol::schema::v1::ClientCapabilities::new().terminal(true),
        );
        assert!(service.supports_terminals());
    }

    async fn delayed_terminal_creation_cleanup(acknowledge_kill: bool) {
        use agent_client_protocol::schema::v1::{
            CreateTerminalResponse, KillTerminalResponse, ReleaseTerminalResponse,
        };
        use agent_client_protocol::{Agent, Responder};
        use std::time::Duration;

        tokio::task::LocalSet::new()
            .run_until(async {
                let cancellation = CancellationToken::new();
                let (create_tx, mut create_rx) = tokio::sync::mpsc::unbounded_channel();
                let (cleanup_tx, mut cleanup_rx) = tokio::sync::mpsc::unbounded_channel();
                let kill_tx = cleanup_tx.clone();
                let (pending_kill_tx, mut pending_kill_rx) =
                    tokio::sync::mpsc::unbounded_channel();
                let client = Client
                    .builder()
                    .on_receive_request(
                        async move |request: CreateTerminalRequest,
                                    responder: Responder<CreateTerminalResponse>,
                                    _cx: ConnectionTo<Agent>| {
                            assert_eq!(request.session_id.to_string(), "cancelled-session");
                            assert_eq!(request.command, "bash");
                            // Retain the response until the prompt-side bridge has returned.
                            create_tx.send(responder).unwrap();
                            Ok(())
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |request: KillTerminalRequest,
                                    responder: Responder<KillTerminalResponse>,
                                    _cx: ConnectionTo<Agent>| {
                            kill_tx
                                .send(("kill", request.session_id, request.terminal_id))
                                .unwrap();
                            if acknowledge_kill {
                                responder.respond(KillTerminalResponse::new())
                            } else {
                                // Handlers run in the client's dispatch loop: waiting here
                                // would prevent it from dispatching terminal/release at all.
                                // Retain the responder without replying, but let dispatch
                                // continue so release must arrive with kill still outstanding.
                                pending_kill_tx.send(responder).unwrap();
                                Ok(())
                            }
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |request: ReleaseTerminalRequest,
                                    responder: Responder<ReleaseTerminalResponse>,
                                    _cx: ConnectionTo<Agent>| {
                            cleanup_tx
                                .send(("release", request.session_id, request.terminal_id))
                                .unwrap();
                            responder.respond(ReleaseTerminalResponse::new())
                        },
                        agent_client_protocol::on_receive_request!(),
                    );

                let result = Agent
                    .builder()
                    .connect_with(client, async move |connection| {
                        let (control_tx, mut control_rx) =
                            tokio::sync::mpsc::unbounded_channel();
                        let request = crate::tools::TerminalSessionRequest {
                            start: crate::tools::TerminalSessionStart {
                                session_id: "cancelled-session".to_string(),
                                tool_call_id: "terminal-call".to_string(),
                                command: "mock-command-never-executed".to_string(),
                                description: "Delayed mock terminal".to_string(),
                                workdir: None,
                                cols: 80,
                                rows: 24,
                                job_id: None,
                            },
                            control_tx,
                        };
                        let bridge = bridge_terminal_session(
                            &connection,
                            "cancelled-session",
                            Path::new("/tmp"),
                            request,
                            &cancellation,
                        );
                        tokio::pin!(bridge);
                        let responder = tokio::time::timeout(Duration::from_secs(2), async {
                            tokio::select! {
                                responder = create_rx.recv() => responder.unwrap(),
                                _ = &mut bridge => panic!("bridge returned before creation or cancellation"),
                            }
                        })
                        .await
                        .expect("mock client did not receive terminal/create");

                        cancellation.cancel();
                        tokio::time::timeout(Duration::from_secs(1), &mut bridge)
                            .await
                            .expect("prompt-side bridge waited for delayed terminal creation");
                        assert!(matches!(
                            control_rx.try_recv(),
                            Ok(crate::tools::TerminalSessionControl::Stop)
                        ));
                        assert!(cleanup_rx.try_recv().is_err());

                        // The terminal is only allocated after cancellation has returned.
                        responder.respond(CreateTerminalResponse::new("late-terminal"))?;
                        let mut cleanup = Vec::new();
                        for _ in 0..2 {
                            let (method, session_id, terminal_id) =
                                tokio::time::timeout(Duration::from_secs(2), cleanup_rx.recv())
                                    .await
                                    .expect("late terminal did not receive both cleanup requests")
                                    .expect("mock cleanup channel closed");
                            assert_eq!(session_id.to_string(), "cancelled-session");
                            assert_eq!(terminal_id.to_string(), "late-terminal");
                            cleanup.push(method);
                        }
                        cleanup.sort_unstable();
                        assert_eq!(cleanup, ["kill", "release"]);
                        if !acknowledge_kill {
                            // Only drop the unanswered kill responder after observing release.
                            // Dropping it earlier would send an error response and accidentally
                            // allow cleanup that waits for kill acknowledgement to pass.
                            let pending_kill = pending_kill_rx
                                .try_recv()
                                .expect("kill must remain unanswered until release arrives");
                            drop(pending_kill);
                        }
                        Ok(())
                    })
                    .await;
                result.expect("mock ACP connection failed");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_terminal_creation_cleans_up_delayed_response() {
        delayed_terminal_creation_cleanup(true).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_terminal_creation_releases_without_kill_acknowledgement() {
        delayed_terminal_creation_cleanup(false).await;
    }
}
