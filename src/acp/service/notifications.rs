use super::internal_error;
use agent_client_protocol::schema::v1::ContentBlock;
use agent_client_protocol::schema::v1::ContentChunk;
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::schema::v1::SessionUpdate;
use agent_client_protocol::schema::v1::ToolCall;
use agent_client_protocol::schema::v1::ToolCallContent;
use agent_client_protocol::schema::v1::ToolCallLocation;
use agent_client_protocol::schema::v1::ToolCallStatus;
use agent_client_protocol::schema::v1::ToolCallUpdate;
use agent_client_protocol::schema::v1::ToolCallUpdateFields;
use agent_client_protocol::schema::v1::ToolKind;
use agent_client_protocol::Client;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::Error;
use std::path::Path;
use std::path::PathBuf;

fn mcp_native_tool_content(metadata: &serde_json::Value) -> Vec<ToolCallContent> {
    metadata
        .get("mcp_result")
        .and_then(|result| result.get("content"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| {
            !matches!(
                block.get("type").and_then(serde_json::Value::as_str),
                Some("text")
            )
        })
        .map(|block| {
            serde_json::from_value::<ContentBlock>(block.clone())
                .map(ToolCallContent::from)
                .unwrap_or_else(|_| {
                    let kind = block
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown");
                    ToolCallContent::from(format!(
                        "[MCP {kind} content]\n{}",
                        serde_json::to_string_pretty(block).unwrap_or_else(|_| block.to_string())
                    ))
                })
        })
        .collect()
}

fn metadata_has_mcp_images(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|metadata| metadata.get("mcp_result"))
        .and_then(|result| result.get("content"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|content| {
            content
                .iter()
                .any(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("image"))
        })
}

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

fn tool_result_text(payload: &serde_json::Value) -> String {
    payload
        .get("output")
        .or_else(|| payload.get("output_preview"))
        .or_else(|| payload.get("error"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn absolute_tool_path(path: &str, cwd: &Path) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

pub(super) fn tool_locations(
    tool_name: &str,
    input: &serde_json::Value,
    cwd: &Path,
) -> Vec<ToolCallLocation> {
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
        content.extend(mcp_native_tool_content(metadata));
    }

    if !metadata_has_mcp_images(payload.get("metadata")) {
        if let Some(images) = payload.get("images").and_then(serde_json::Value::as_array) {
            content.extend(images.iter().filter_map(tool_result_image));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ContentBlock;
    use agent_client_protocol::schema::v1::ToolCallContent;
    use agent_client_protocol::schema::v1::ToolKind;
    use std::path::Path;
    use std::path::PathBuf;

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
    fn acp_tool_result_restores_native_mcp_resources() {
        let metadata = serde_json::json!({
            "mcp_result": {
                "content": [
                    {
                        "type": "image",
                        "data": "aGk=",
                        "mimeType": "image/png",
                        "annotations": { "priority": 0.5 }
                    },
                    {
                        "type": "resource_link",
                        "uri": "file:///tmp/readme.md",
                        "name": "readme",
                        "mimeType": "text/markdown",
                        "annotations": { "priority": 0.8 }
                    }
                ]
            }
        });

        let content = mcp_native_tool_content(&metadata);
        assert_eq!(content.len(), 2);
        assert!(matches!(
            &content[0],
            ToolCallContent::Content(content)
                if matches!(&content.content, ContentBlock::Image(image)
                    if image.data == "aGk=" && image.annotations.is_some())
        ));
        assert!(matches!(
            &content[1],
            ToolCallContent::Content(content)
                if matches!(&content.content, ContentBlock::ResourceLink(link)
                    if link.uri == "file:///tmp/readme.md")
        ));
    }

    #[test]
    fn acp_tool_result_restores_mcp_audio_and_preserves_unknown_blocks() {
        let metadata = serde_json::json!({
            "mcp_result": {
                "content": [
                    { "type": "audio", "data": "YXVkaW8=", "mimeType": "audio/wav" },
                    { "type": "future_media", "payload": { "value": 1 } }
                ]
            }
        });
        let content = mcp_native_tool_content(&metadata);
        assert!(matches!(
            &content[0],
            ToolCallContent::Content(content)
                if matches!(&content.content, ContentBlock::Audio(audio)
                    if audio.data == "YXVkaW8=" && audio.mime_type == "audio/wav")
        ));
        assert!(matches!(
            &content[1],
            ToolCallContent::Content(content)
                if matches!(&content.content, ContentBlock::Text(text)
                    if text.text.contains("future_media") && text.text.contains("payload"))
        ));
    }
}
