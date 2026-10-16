use super::tool_locations::absolute_tool_path;
use agent_client_protocol::schema::v1::ContentBlock;
use agent_client_protocol::schema::v1::ToolCallContent;
use std::path::Path;

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

fn tool_result_text(payload: &serde_json::Value) -> String {
    payload
        .get("output")
        .or_else(|| payload.get("output_preview"))
        .or_else(|| payload.get("error"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn tool_result_content(payload: &serde_json::Value, cwd: &Path) -> Vec<ToolCallContent> {
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

#[cfg(test)]
mod tests {
    use super::super::tool_locations::tool_result_locations;
    use super::*;
    use agent_client_protocol::schema::v1::ContentBlock;
    use agent_client_protocol::schema::v1::ToolCallContent;

    use std::path::Path;
    use std::path::PathBuf;

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
