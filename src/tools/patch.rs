use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct ApplyPatchTool;

#[derive(Default)]
struct PatchSummary {
    added: usize,
    updated: usize,
    deleted: usize,
    moved: usize,
}

impl PatchSummary {
    fn touched(&self) -> usize {
        self.added + self.updated + self.deleted + self.moved
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("added {}", self.added));
        }
        if self.updated > 0 {
            parts.push(format!("updated {}", self.updated));
        }
        if self.deleted > 0 {
            parts.push(format!("deleted {}", self.deleted));
        }
        if self.moved > 0 {
            parts.push(format!("moved {}", self.moved));
        }
        if parts.is_empty() {
            "no files changed".to_string()
        } else {
            parts.join(", ")
        }
    }
}

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for ApplyPatchTool {
    fn definition(&self) -> Tool {
        Tool {
            id: "apply_patch".to_string(),
            description: "Apply a compact multi-file patch. Prefer this for edits to existing files, especially multi-file changes, because a unified diff is much shorter than rewriting whole files. Accepts standard unified diffs and Codex-style patches beginning with *** Begin Patch.".to_string(),
            parameters: vec![ParameterSchema {
                name: "patch".to_string(),
                description: "Patch text to apply. Use standard unified diff format with ---/+++/@@ hunks, or Codex-style *** Begin Patch format.".to_string(),
                required: true,
                param_type: ParameterType::String,
            }],
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["patch"])?;
        if !params.get("patch").is_some_and(Value::is_string) {
            return Err(ToolError::Validation("patch must be a string".to_string()));
        }
        Ok(())
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let patch = get_string_param(&params, "patch")
            .ok_or_else(|| ToolError::Validation("patch is required".to_string()))?;
        let patch = clean_patch_input(&patch);
        let summary = if patch.trim_start().starts_with("*** Begin Patch") {
            apply_codex_patch(&patch)?
        } else {
            apply_unified_patch(&patch)?
        };

        Ok(ToolResult::new(
            "Apply patch",
            format!("Applied patch: {}", summary.describe()),
        )
        .with_metadata("file_count", serde_json::json!(summary.touched())))
    }
}

pub(crate) fn patch_paths_from_params(params: &Value) -> Vec<String> {
    params
        .get("patch")
        .and_then(Value::as_str)
        .map(extract_patch_paths)
        .unwrap_or_default()
}

pub(crate) fn extract_patch_paths(patch: &str) -> Vec<String> {
    let patch = clean_patch_input(patch);
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for line in patch.lines() {
        let candidates: Vec<String> = if let Some(path) = line.strip_prefix("*** Update File: ") {
            vec![path.to_string()]
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            vec![path.to_string()]
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            vec![path.to_string()]
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            vec![path.to_string()]
        } else if let Some(path) = line.strip_prefix("--- ") {
            vec![normalize_diff_path(path)]
        } else if let Some(path) = line.strip_prefix("+++ ") {
            vec![normalize_diff_path(path)]
        } else if let Some(rest) = line.strip_prefix("diff --git ") {
            rest.split_whitespace().map(normalize_diff_path).collect()
        } else {
            Vec::new()
        };

        for path in candidates {
            let path = path.trim();
            if path.is_empty() || path == "/dev/null" {
                continue;
            }
            if seen.insert(path.to_string()) {
                paths.push(path.to_string());
            }
        }
    }

    paths
}

pub(crate) fn patch_paths_as_pathbufs(params: &Value, workdir: &Path) -> Vec<PathBuf> {
    patch_paths_from_params(params)
        .into_iter()
        .map(|path| {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                workdir.join(path)
            }
        })
        .collect()
}

fn clean_patch_input(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut lines: Vec<&str> = trimmed.lines().collect();
    if lines
        .first()
        .is_some_and(|line| line.trim_start().starts_with("```"))
    {
        lines.remove(0);
        if lines
            .last()
            .is_some_and(|line| line.trim_start().starts_with("```"))
        {
            lines.pop();
        }
    }
    lines.join("\n")
}

fn normalize_diff_path(raw: &str) -> String {
    let path = raw
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('"');

    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path)
        .to_string()
}

fn apply_unified_patch(patch: &str) -> Result<PatchSummary, ToolError> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut index = 0;
    let mut summary = PatchSummary::default();

    while index < lines.len() {
        if !lines[index].starts_with("--- ") {
            index += 1;
            continue;
        }

        let old_path = normalize_diff_path(
            lines[index]
                .strip_prefix("--- ")
                .expect("line prefix already checked"),
        );
        index += 1;

        if index >= lines.len() || !lines[index].starts_with("+++ ") {
            return Err(ToolError::Validation(
                "Unified diff file header must include a +++ path".to_string(),
            ));
        }
        let new_path = normalize_diff_path(
            lines[index]
                .strip_prefix("+++ ")
                .expect("line prefix already checked"),
        );
        index += 1;

        let target_path = if new_path == "/dev/null" {
            old_path.as_str()
        } else {
            new_path.as_str()
        };
        let mut content = if old_path == "/dev/null" {
            String::new()
        } else {
            read_file(target_path)?
        };
        let mut applied_hunks = 0usize;

        while index < lines.len()
            && !lines[index].starts_with("--- ")
            && !lines[index].starts_with("diff --git ")
        {
            if !lines[index].starts_with("@@") {
                index += 1;
                continue;
            }
            index += 1;
            let (old_text, new_text, next_index) = collect_hunk(&lines, index);
            content = replace_hunk(&content, &old_text, &new_text)?;
            applied_hunks += 1;
            index = next_index;
        }

        if new_path == "/dev/null" {
            std::fs::remove_file(target_path)
                .map_err(|e| ToolError::Execution(format!("Failed to delete file: {}", e)))?;
            summary.deleted += 1;
        } else {
            write_atomic(target_path, &content)?;
            if old_path == "/dev/null" {
                summary.added += 1;
            } else if applied_hunks > 0 {
                summary.updated += 1;
            }
        }
    }

    if summary.touched() == 0 {
        return Err(ToolError::Validation(
            "Patch did not contain any file changes".to_string(),
        ));
    }

    Ok(summary)
}

fn collect_hunk(lines: &[&str], mut index: usize) -> (String, String, usize) {
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();

    while index < lines.len()
        && !lines[index].starts_with("@@")
        && !lines[index].starts_with("--- ")
        && !lines[index].starts_with("diff --git ")
        && !lines[index].starts_with("*** ")
    {
        let line = lines[index];
        if line == r"\ No newline at end of file" {
            index += 1;
            continue;
        }
        let (prefix, rest) = line.split_at(line.len().min(1));
        match prefix {
            " " => {
                old_lines.push(rest.to_string());
                new_lines.push(rest.to_string());
            }
            "-" => old_lines.push(rest.to_string()),
            "+" => new_lines.push(rest.to_string()),
            _ => {}
        }
        index += 1;
    }

    (
        join_hunk_lines(&old_lines),
        join_hunk_lines(&new_lines),
        index,
    )
}

fn join_hunk_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

fn replace_hunk(content: &str, old_text: &str, new_text: &str) -> Result<String, ToolError> {
    if old_text.is_empty() {
        let mut out = content.to_string();
        out.push_str(new_text);
        return Ok(out);
    }

    if let Some(pos) = content.find(old_text) {
        let mut out = String::with_capacity(content.len() - old_text.len() + new_text.len());
        out.push_str(&content[..pos]);
        out.push_str(new_text);
        out.push_str(&content[pos + old_text.len()..]);
        return Ok(out);
    }

    if old_text.ends_with('\n') {
        let old_trimmed = old_text.trim_end_matches('\n');
        if let Some(pos) = content.find(old_trimmed) {
            let new_trimmed = new_text.trim_end_matches('\n');
            let mut out =
                String::with_capacity(content.len() - old_trimmed.len() + new_trimmed.len());
            out.push_str(&content[..pos]);
            out.push_str(new_trimmed);
            out.push_str(&content[pos + old_trimmed.len()..]);
            return Ok(out);
        }
    }

    Err(ToolError::NotFound(
        "Could not apply patch hunk: context was not found".to_string(),
    ))
}

fn apply_codex_patch(patch: &str) -> Result<PatchSummary, ToolError> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut index = 0;
    let mut summary = PatchSummary::default();

    if lines.get(index).map(|line| line.trim()) != Some("*** Begin Patch") {
        return Err(ToolError::Validation(
            "Codex patch must start with *** Begin Patch".to_string(),
        ));
    }
    index += 1;

    while index < lines.len() {
        let line = lines[index].trim();
        if line == "*** End Patch" {
            break;
        }

        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut file_lines = Vec::new();
            while index < lines.len() && !lines[index].starts_with("*** ") {
                let Some(content) = lines[index].strip_prefix('+') else {
                    return Err(ToolError::Validation(
                        "Add File lines must start with +".to_string(),
                    ));
                };
                file_lines.push(content.to_string());
                index += 1;
            }
            write_atomic(path, &join_hunk_lines(&file_lines))?;
            summary.added += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            std::fs::remove_file(path)
                .map_err(|e| ToolError::Execution(format!("Failed to delete file: {}", e)))?;
            summary.deleted += 1;
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if let Some(target) = lines
                .get(index)
                .and_then(|line| line.trim().strip_prefix("*** Move to: "))
            {
                move_to = Some(target.to_string());
                index += 1;
            }

            let mut content = read_file(path)?;
            let mut hunk_count = 0usize;
            while index < lines.len() && !lines[index].starts_with("*** ") {
                if !lines[index].starts_with("@@") {
                    index += 1;
                    continue;
                }
                index += 1;
                let (old_text, new_text, next_index) = collect_hunk(&lines, index);
                content = replace_hunk(&content, &old_text, &new_text)?;
                hunk_count += 1;
                index = next_index;
            }

            let target = move_to.as_deref().unwrap_or(path);
            write_atomic(target, &content)?;
            if let Some(target) = move_to {
                if target != path {
                    let _ = std::fs::remove_file(path);
                    summary.moved += 1;
                } else if hunk_count > 0 {
                    summary.updated += 1;
                }
            } else if hunk_count > 0 {
                summary.updated += 1;
            }
            continue;
        }

        return Err(ToolError::Validation(format!(
            "Unsupported patch directive: {}",
            line
        )));
    }

    if summary.touched() == 0 {
        return Err(ToolError::Validation(
            "Patch did not contain any file changes".to_string(),
        ));
    }

    Ok(summary)
}

fn read_file(path: &str) -> Result<String, ToolError> {
    std::fs::read_to_string(path)
        .map_err(|e| ToolError::Execution(format!("Failed to read file '{}': {}", path, e)))
}

fn write_atomic(path: &str, content: &str) -> Result<(), ToolError> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::Execution(format!("Failed to create directories: {}", e))
            })?;
        }
    }

    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, content)
        .map_err(|e| ToolError::Execution(format!("Failed to write temp file: {}", e)))?;
    std::fs::rename(&temp_path, path)
        .map_err(|e| ToolError::Execution(format!("Failed to rename file: {}", e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolHandler;

    fn test_context() -> ToolContext {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        ToolContext::new("session", "message", "build", rx)
    }

    #[tokio::test]
    async fn apply_patch_updates_multiple_files_from_unified_diff() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.txt");
        let second = dir.path().join("b.txt");
        std::fs::write(&first, "one\ntwo\n").unwrap();
        std::fs::write(&second, "alpha\nbeta\n").unwrap();

        let patch = format!(
            "--- {}\n+++ {}\n@@ -1,2 +1,2 @@\n one\n-two\n+three\n--- {}\n+++ {}\n@@ -1,2 +1,2 @@\n alpha\n-beta\n+gamma\n",
            first.display(),
            first.display(),
            second.display(),
            second.display()
        );

        let result = ApplyPatchTool::new()
            .execute(serde_json::json!({ "patch": patch }), &test_context())
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(first).unwrap(), "one\nthree\n");
        assert_eq!(std::fs::read_to_string(second).unwrap(), "alpha\ngamma\n");
        assert!(result.output.contains("updated 2"));
        assert_eq!(result.metadata["file_count"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn apply_patch_supports_codex_patch_format() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n one\n-two\n+three\n*** End Patch\n",
            file.display()
        );

        ApplyPatchTool::new()
            .execute(serde_json::json!({ "patch": patch }), &test_context())
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(file).unwrap(), "one\nthree\n");
    }

    #[test]
    fn extract_patch_paths_finds_unified_and_codex_paths() {
        let patch = "*** Begin Patch\n*** Update File: src/a.ts\n*** Move to: src/b.ts\n*** End Patch\n--- a/src/c.ts\n+++ b/src/c.ts\n";
        assert_eq!(
            extract_patch_paths(patch),
            vec!["src/a.ts", "src/b.ts", "src/c.ts"]
        );
    }
}
