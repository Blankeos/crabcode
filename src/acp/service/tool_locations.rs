use agent_client_protocol::schema::v1::ToolCallLocation;
use std::path::Path;
use std::path::PathBuf;

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

pub(super) fn tool_result_locations(
    payload: &serde_json::Value,
    cwd: &Path,
) -> Vec<ToolCallLocation> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;

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
}
