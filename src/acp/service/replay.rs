use super::internal_error;
use super::notifications::send_tool_result;
use super::notifications::tool_kind;
use super::notifications::tool_title;
use super::tool_locations::tool_locations;
use agent_client_protocol::schema::v1::ContentChunk;
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::schema::v1::SessionUpdate;
use agent_client_protocol::schema::v1::ToolCall;
use agent_client_protocol::schema::v1::ToolCallStatus;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use std::path::Path;

pub(super) fn replay_messages(
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

pub(super) fn send_replay_text(
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
