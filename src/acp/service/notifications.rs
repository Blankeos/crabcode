use super::internal_error;
use super::tool_content::tool_result_content;
use super::tool_locations::tool_locations;
use super::tool_locations::tool_result_locations;
use agent_client_protocol::schema::v1::ContentChunk;
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::schema::v1::SessionUpdate;
use agent_client_protocol::schema::v1::ToolCall;
use agent_client_protocol::schema::v1::ToolCallStatus;
use agent_client_protocol::schema::v1::ToolCallUpdate;
use agent_client_protocol::schema::v1::ToolCallUpdateFields;
use agent_client_protocol::schema::v1::ToolKind;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use std::path::Path;

pub(super) fn send_text(
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

pub(super) fn send_tool_call(
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

pub(super) fn send_tool_result(
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

pub(super) fn tool_kind(tool_name: &str) -> ToolKind {
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

pub(super) fn tool_title(tool_name: &str, input: &serde_json::Value) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ToolKind;

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
}
