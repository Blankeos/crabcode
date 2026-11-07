use super::tool_locations::absolute_tool_path;
use agent_client_protocol::schema::v1::ToolCallContent;
use std::path::Path;

pub(super) fn permission_tool_content(
    prompt: &crate::tools::PermissionPrompt,
    cwd: &Path,
) -> Vec<ToolCallContent> {
    match prompt.tool_id.as_str() {
        "write" => prompt
            .raw_input
            .get("file_path")
            .or_else(|| prompt.raw_input.get("filePath"))
            .and_then(serde_json::Value::as_str)
            .zip(
                prompt
                    .raw_input
                    .get("content")
                    .and_then(serde_json::Value::as_str),
            )
            .map(|(path, new_text)| vec![preflight_diff(path, new_text, cwd)])
            .unwrap_or_default(),
        "write_files" => prompt
            .raw_input
            .get("files")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|file| {
                let path = file.get("file_path")?.as_str()?;
                let content = file.get("content")?.as_str()?;
                Some(preflight_diff(path, content, cwd))
            })
            .collect(),
        "edit" => permission_edit_diff(&prompt.raw_input, cwd)
            .into_iter()
            .collect(),
        "apply_patch"
            if crate::tools::patch::patch_paths_from_params(&prompt.raw_input)
                .iter()
                .all(|path| preview_read_allowed(&absolute_tool_path(path, cwd), cwd)) =>
        {
            crate::tools::patch::preview_patch(&prompt.raw_input, cwd)
                .unwrap_or_default()
                .into_iter()
                .map(|change| {
                    agent_client_protocol::schema::v1::Diff::new(change.path, change.new_text)
                        .old_text(change.old_text)
                        .into()
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

// A write/edit approval is not a read approval. Keep ordinary workspace previews,
// but never acquire protected file contents just to display a permission request.
fn preview_read_allowed(path: &Path, cwd: &Path) -> bool {
    !crate::tools::permission::is_outside_workdir(path, cwd)
        && !crate::tools::permission::is_sensitive_path(path)
        && !std::fs::canonicalize(path)
            .ok()
            .is_some_and(|resolved| crate::tools::permission::is_sensitive_path(&resolved))
}

fn preflight_diff(path: &str, new_text: &str, cwd: &Path) -> ToolCallContent {
    let path = absolute_tool_path(path, cwd);
    let old_text = preview_read_allowed(&path, cwd)
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten();
    agent_client_protocol::schema::v1::Diff::new(path, new_text.to_string())
        .old_text(old_text)
        .into()
}

fn permission_edit_diff(input: &serde_json::Value, cwd: &Path) -> Option<ToolCallContent> {
    let path = input
        .get("file_path")
        .or_else(|| input.get("filePath"))?
        .as_str()?;
    let old_string = input.get("old_string")?.as_str()?;
    let new_string = input.get("new_string")?.as_str()?;
    let absolute = absolute_tool_path(path, cwd);
    if !preview_read_allowed(&absolute, cwd) {
        return None;
    }
    let old_text = std::fs::read_to_string(&absolute).ok()?;
    let new_text = if input
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        old_text.replace(old_string, new_string)
    } else {
        old_text.replacen(old_string, new_string, 1)
    };
    Some(
        agent_client_protocol::schema::v1::Diff::new(absolute, new_text)
            .old_text(old_text)
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ToolCallContent;

    #[test]
    fn acp_permission_edit_includes_preflight_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "fn old() {}\n").unwrap();
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let prompt = crate::tools::PermissionPrompt {
            tool_call_id: Some("call_edit".to_string()),
            tool_id: "edit".to_string(),
            action: crate::tools::PermissionAction::Write,
            permission: "edit".to_string(),
            patterns: vec![path.to_string_lossy().into_owned()],
            target: Some(path.to_string_lossy().into_owned()),
            command: None,
            workdir: None,
            workspace: dir.path().to_string_lossy().into_owned(),
            reason: "approval".to_string(),
            raw_input: serde_json::json!({
                "file_path": path,
                "old_string": "old",
                "new_string": "new",
                "replace_all": false
            }),
            response_tx,
        };

        let content = permission_tool_content(&prompt, dir.path());
        let ToolCallContent::Diff(diff) = &content[0] else {
            panic!("expected preflight diff");
        };
        assert_eq!(diff.old_text.as_deref(), Some("fn old() {}\n"));
        assert_eq!(diff.new_text, "fn new() {}\n");
    }

    #[test]
    fn acp_permission_apply_patch_includes_preflight_diff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "before\n").unwrap();
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let prompt = crate::tools::PermissionPrompt {
            tool_call_id: Some("call_patch".to_string()),
            tool_id: "apply_patch".to_string(),
            action: crate::tools::PermissionAction::Write,
            permission: "edit".to_string(),
            patterns: vec![path.to_string_lossy().into_owned()],
            target: Some(path.to_string_lossy().into_owned()),
            command: None,
            workdir: None,
            workspace: dir.path().to_string_lossy().into_owned(),
            reason: "approval".to_string(),
            raw_input: serde_json::json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n@@\n-before\n+after\n*** End Patch\n",
                    path.display()
                )
            }),
            response_tx,
        };

        let content = permission_tool_content(&prompt, dir.path());
        let ToolCallContent::Diff(diff) = &content[0] else {
            panic!("expected apply_patch preflight diff");
        };
        assert_eq!(diff.path, path);
        assert_eq!(diff.old_text.as_deref(), Some("before\n"));
        assert_eq!(diff.new_text, "after\n");
    }

    #[test]
    fn previews_keep_workspace_diffs_but_do_not_read_protected_files() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let ordinary = workspace.path().join("hello.txt");
        std::fs::write(&ordinary, "before").unwrap();
        let preview =
            serde_json::to_string(&preflight_diff("hello.txt", "after", workspace.path())).unwrap();
        assert!(preview.contains("before"));
        for path in [
            workspace.path().join(".env"),
            external.path().join("secret.txt"),
        ] {
            std::fs::write(&path, "secret-never-preview").unwrap();
            let preview = serde_json::to_string(&preflight_diff(
                path.to_str().unwrap(),
                "after",
                workspace.path(),
            ))
            .unwrap();
            assert!(!preview.contains("secret-never-preview"));
            assert!(permission_edit_diff(
                &serde_json::json!({
                    "file_path": path, "old_string": "secret", "new_string": "public"
                }),
                workspace.path()
            )
            .is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn previews_reject_symlink_escape_and_sensitive_alias() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let secret = external.path().join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        let link = workspace.path().join("alias.txt");
        std::os::unix::fs::symlink(secret, &link).unwrap();
        assert!(!preview_read_allowed(&link, workspace.path()));
        let env = workspace.path().join(".env");
        std::fs::write(&env, "secret").unwrap();
        let alias = workspace.path().join("config.txt");
        std::os::unix::fs::symlink(env, &alias).unwrap();
        assert!(!preview_read_allowed(&alias, workspace.path()));
    }
}
