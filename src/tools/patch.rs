use crate::tools::mutation::{FileMutation, LockedFile, STALE_FILE_MESSAGE};
use crate::tools::{
    get_string_param, validate_required, ParameterSchema, ParameterType, Tool, ToolContext,
    ToolError, ToolHandler, ToolResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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
            input_schema: None,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), ToolError> {
        validate_required(params, &["patch"])?;
        if !params.get("patch").is_some_and(Value::is_string) {
            return Err(ToolError::Validation("patch must be a string".to_string()));
        }
        Ok(())
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let patch = get_string_param(&params, "patch")
            .ok_or_else(|| ToolError::Validation("patch is required".to_string()))?;
        let patch = clean_patch_input(&patch);
        let paths = patch_paths_as_pathbufs(&params, ctx.workdir());
        let before = paths
            .iter()
            .map(|path| (path.clone(), std::fs::read_to_string(path).ok()))
            .collect::<Vec<_>>();
        let summary = if patch.trim_start().starts_with("*** Begin Patch") {
            apply_codex_patch(&patch)?
        } else {
            apply_unified_patch(&patch)?
        };
        let changes = before
            .into_iter()
            .filter_map(|(path, old_text)| {
                let new_text = std::fs::read_to_string(&path).ok();
                (old_text != new_text).then(|| {
                    serde_json::json!({
                        "path": path,
                        "old_text": old_text,
                        "new_text": new_text.unwrap_or_default(),
                    })
                })
            })
            .collect::<Vec<_>>();

        Ok(ToolResult::new(
            "Apply patch",
            format!("Applied patch: {}", summary.describe()),
        )
        .with_metadata("file_count", serde_json::json!(summary.touched()))
        .with_metadata("changes", serde_json::json!(changes)))
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchPreview {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Default)]
struct PatchPreviewState {
    original: HashMap<PathBuf, Option<String>>,
    current: HashMap<PathBuf, Option<String>>,
    order: Vec<PathBuf>,
}

impl PatchPreviewState {
    fn resolve(workdir: &Path, path: &str) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            workdir.join(path)
        }
    }

    fn load(&mut self, path: &Path) -> Result<Option<String>, ToolError> {
        if let Some(content) = self.current.get(path) {
            return Ok(content.clone());
        }
        let content = match std::fs::read(path) {
            Ok(bytes) => Some(decode_utf8(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ToolError::Execution(format!(
                    "Failed to read {}: {error}",
                    path.display()
                )));
            }
        };
        self.original.insert(path.to_path_buf(), content.clone());
        self.current.insert(path.to_path_buf(), content.clone());
        self.order.push(path.to_path_buf());
        Ok(content)
    }

    fn required(&mut self, path: &Path) -> Result<String, ToolError> {
        self.load(path)?.ok_or_else(|| {
            ToolError::NotFound(format!("Patch source file not found: {}", path.display()))
        })
    }

    fn create(&mut self, path: PathBuf, content: String) -> Result<(), ToolError> {
        if self.load(&path)?.is_some() {
            return Err(ToolError::Execution(format!(
                "Refusing to overwrite existing file: {}",
                path.display()
            )));
        }
        self.current.insert(path, Some(content));
        Ok(())
    }

    fn write(&mut self, path: PathBuf, content: String) -> Result<(), ToolError> {
        self.required(&path)?;
        self.current.insert(path, Some(content));
        Ok(())
    }

    fn delete(&mut self, path: PathBuf) -> Result<(), ToolError> {
        self.required(&path)?;
        self.current.insert(path, None);
        Ok(())
    }

    fn finish(self) -> Vec<PatchPreview> {
        self.order
            .into_iter()
            .filter_map(|path| {
                let old_text = self.original.get(&path).cloned().flatten();
                let new_text = self.current.get(&path).cloned().flatten();
                (old_text != new_text).then_some(PatchPreview {
                    path,
                    old_text,
                    new_text: new_text.unwrap_or_default(),
                })
            })
            .collect()
    }
}

pub(crate) fn preview_patch(
    params: &Value,
    workdir: &Path,
) -> Result<Vec<PatchPreview>, ToolError> {
    let patch = get_string_param(params, "patch")
        .ok_or_else(|| ToolError::Validation("patch is required".to_string()))?;
    let patch = clean_patch_input(&patch);
    let mut state = PatchPreviewState::default();
    if patch.trim_start().starts_with("*** Begin Patch") {
        preview_codex_patch(&patch, workdir, &mut state)?;
    } else {
        preview_unified_patch(&patch, workdir, &mut state)?;
    }
    let changes = state.finish();
    if changes.is_empty() {
        return Err(ToolError::Validation(
            "Patch did not contain any file changes".to_string(),
        ));
    }
    Ok(changes)
}

fn preview_unified_patch(
    patch: &str,
    workdir: &Path,
    state: &mut PatchPreviewState,
) -> Result<(), ToolError> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("--- ") {
            index += 1;
            continue;
        }
        let old_path = normalize_diff_path(lines[index].trim_start_matches("--- "));
        index += 1;
        if index >= lines.len() || !lines[index].starts_with("+++ ") {
            return Err(ToolError::Validation(
                "Unified diff file header must include a +++ path".to_string(),
            ));
        }
        let new_path = normalize_diff_path(lines[index].trim_start_matches("+++ "));
        index += 1;
        let source =
            (old_path != "/dev/null").then(|| PatchPreviewState::resolve(workdir, &old_path));
        let target =
            (new_path != "/dev/null").then(|| PatchPreviewState::resolve(workdir, &new_path));
        let mut content = match &source {
            Some(path) => state.required(path)?,
            None => String::new(),
        };
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
            index = next_index;
        }
        match (source, target) {
            (None, Some(target)) => state.create(target, content)?,
            (Some(source), None) => state.delete(source)?,
            (Some(source), Some(target)) if source == target => state.write(source, content)?,
            (Some(source), Some(target)) => {
                state.create(target, content)?;
                state.delete(source)?;
            }
            (None, None) => {
                return Err(ToolError::Validation(
                    "Patch cannot use /dev/null for both paths".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn preview_codex_patch(
    patch: &str,
    workdir: &Path,
    state: &mut PatchPreviewState,
) -> Result<(), ToolError> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut index = 0;
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
            state.create(
                PatchPreviewState::resolve(workdir, path),
                join_hunk_lines(&file_lines),
            )?;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            state.delete(PatchPreviewState::resolve(workdir, path))?;
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let source = PatchPreviewState::resolve(workdir, path);
            let mut content = state.required(&source)?;
            index += 1;
            let move_to = lines
                .get(index)
                .and_then(|line| line.trim().strip_prefix("*** Move to: "))
                .map(str::to_string);
            if move_to.is_some() {
                index += 1;
            }
            while index < lines.len() && !lines[index].starts_with("*** ") {
                if !lines[index].starts_with("@@") {
                    index += 1;
                    continue;
                }
                index += 1;
                let (old_text, new_text, next_index) = collect_hunk(&lines, index);
                content = replace_hunk(&content, &old_text, &new_text)?;
                index = next_index;
            }
            if let Some(target) = move_to {
                let target = PatchPreviewState::resolve(workdir, &target);
                if target == source {
                    state.write(source, content)?;
                } else {
                    state.create(target, content)?;
                    state.delete(source)?;
                }
            } else {
                state.write(source, content)?;
            }
            continue;
        }
        return Err(ToolError::Validation(format!(
            "Unsupported patch directive: {line}"
        )));
    }
    Ok(())
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

        FileMutation::with_lock_path(target_path, |locked| {
            let (expected, mut content) = if old_path == "/dev/null" {
                (None, String::new())
            } else {
                let bytes = locked.read()?;
                let content = decode_utf8(&bytes)?;
                (Some(bytes), content)
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
                let expected = expected.ok_or_else(|| {
                    ToolError::Validation("Delete hunk did not have source content".to_string())
                })?;
                locked.remove_if_unchanged(&expected)?;
                summary.deleted += 1;
            } else if old_path == "/dev/null" {
                locked.create_new(content.as_bytes())?;
                summary.added += 1;
            } else {
                let expected = expected.ok_or_else(|| {
                    ToolError::Validation("Update hunk did not have source content".to_string())
                })?;
                locked.write_if_unchanged(&expected, content.as_bytes())?;
                if applied_hunks > 0 {
                    summary.updated += 1;
                }
            }

            Ok(())
        })?;
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

    if let Some((start, end, old_replacement, new_replacement)) =
        find_controlled_hunk_match(content, old_text, new_text)
    {
        let mut out =
            String::with_capacity(content.len() - old_replacement.len() + new_replacement.len());
        out.push_str(&content[..start]);
        out.push_str(&new_replacement);
        out.push_str(&content[end..]);
        return Ok(out);
    }

    Err(ToolError::NotFound(
        "Could not apply patch hunk: context was not found".to_string(),
    ))
}

fn find_controlled_hunk_match(
    content: &str,
    old_text: &str,
    new_text: &str,
) -> Option<(usize, usize, String, String)> {
    let content_ending = detect_line_ending(content);
    let old_lf = normalize_line_endings(old_text);
    let new_lf = normalize_line_endings(new_text);
    let old_variants = hunk_text_variants(&old_lf);

    for old_variant_lf in old_variants {
        let new_variant_lf = align_trailing_newline(&new_lf, old_variant_lf.ends_with('\n'));
        for candidate_old in line_ending_variants(&old_variant_lf, content_ending) {
            if let Some(pos) = content.find(&candidate_old) {
                let candidate_new = convert_line_endings(&new_variant_lf, content_ending);
                return Some((pos, pos + candidate_old.len(), candidate_old, candidate_new));
            }
        }
    }

    find_line_sequence_match(content, &old_lf).map(|(start, end, matched)| {
        let new_replacement = convert_line_endings(
            &align_trailing_newline(&new_lf, matched.ends_with(content_ending)),
            content_ending,
        );
        (start, end, matched, new_replacement)
    })
}

fn hunk_text_variants(text: &str) -> Vec<String> {
    let mut variants = Vec::new();
    push_unique(&mut variants, text.to_string());
    if text.ends_with('\n') {
        push_unique(&mut variants, text.trim_end_matches('\n').to_string());
    }
    variants
}

fn line_ending_variants(text_lf: &str, preferred: &str) -> Vec<String> {
    let mut variants = Vec::new();
    push_unique(&mut variants, convert_line_endings(text_lf, preferred));
    push_unique(&mut variants, text_lf.to_string());
    push_unique(&mut variants, convert_line_endings(text_lf, "\r\n"));
    variants
}

fn find_line_sequence_match(content: &str, old_text_lf: &str) -> Option<(usize, usize, String)> {
    let pattern = split_hunk_pattern(old_text_lf);
    if pattern.is_empty() || pattern.len() > content.lines().count() {
        return None;
    }

    let content_lines = split_content_lines_with_offsets(content);
    if pattern.len() > content_lines.len() {
        return None;
    }

    for start in 0..=content_lines.len().saturating_sub(pattern.len()) {
        let window = &content_lines[start..start + pattern.len()];
        if lines_match(window, &pattern, MatchMode::TrimEnd)
            || lines_match(window, &pattern, MatchMode::Trim)
            || lines_match(window, &pattern, MatchMode::Normalized)
        {
            let start_byte = window.first()?.start;
            let end_byte = window.last()?.end;
            return Some((
                start_byte,
                end_byte,
                content[start_byte..end_byte].to_string(),
            ));
        }
    }

    None
}

#[derive(Clone, Copy)]
enum MatchMode {
    TrimEnd,
    Trim,
    Normalized,
}

struct ContentLine<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn split_content_lines_with_offsets(content: &str) -> Vec<ContentLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for piece in content.split_inclusive('\n') {
        let end = start + piece.len();
        let text = piece.trim_end_matches('\n').trim_end_matches('\r');
        lines.push(ContentLine { text, start, end });
        start = end;
    }
    if start < content.len() {
        lines.push(ContentLine {
            text: &content[start..],
            start,
            end: content.len(),
        });
    }
    lines
}

fn split_hunk_pattern(text_lf: &str) -> Vec<String> {
    let mut lines: Vec<String> = text_lf.split('\n').map(ToString::to_string).collect();
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn lines_match(window: &[ContentLine<'_>], pattern: &[String], mode: MatchMode) -> bool {
    window
        .iter()
        .zip(pattern)
        .all(|(line, expected)| match mode {
            MatchMode::TrimEnd => line.text.trim_end() == expected.trim_end(),
            MatchMode::Trim => line.text.trim() == expected.trim(),
            MatchMode::Normalized => {
                normalize_patch_context(line.text) == normalize_patch_context(expected)
            }
        })
}

fn normalize_patch_context(value: &str) -> String {
    let mut normalized = String::new();
    for ch in value.trim().chars() {
        match ch {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => normalized.push('-'),
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => normalized.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => normalized.push('"'),
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => normalized.push(' '),
            '\u{2026}' => normalized.push_str("..."),
            other => normalized.push(other),
        }
    }
    normalized
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn detect_line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn convert_line_endings(text_lf: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text_lf.replace('\n', "\r\n")
    } else {
        text_lf.to_string()
    }
}

fn align_trailing_newline(text: &str, should_end_with_newline: bool) -> String {
    if should_end_with_newline && !text.ends_with('\n') {
        format!("{}\n", text)
    } else if !should_end_with_newline && text.ends_with('\n') {
        text.trim_end_matches('\n').to_string()
    } else {
        text.to_string()
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
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
            FileMutation::create_new(path, join_hunk_lines(&file_lines).as_bytes())?;
            summary.added += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            FileMutation::with_lock_path(path, |locked| {
                let expected = locked.read()?;
                locked.remove_if_unchanged(&expected)?;
                Ok(())
            })?;
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

            let mut hunk_count = 0usize;
            let target = move_to.as_deref().unwrap_or(path);
            if target == path {
                FileMutation::with_lock_path(path, |locked| {
                    let expected = locked.read()?;
                    let mut content = decode_utf8(&expected)?;
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
                    locked.write_if_unchanged(&expected, content.as_bytes())?;
                    Ok(())
                })?;
            } else {
                let target_path = PathBuf::from(target);
                FileMutation::with_two_lock_paths(path, &target_path, |source, target_locked| {
                    let expected = source.read()?;
                    let mut content = decode_utf8(&expected)?;
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
                    write_move_if_unchanged(source, target_locked, &expected, content.as_bytes())?;
                    Ok(())
                })?;
            }
            if let Some(target) = move_to {
                if target != path {
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

fn decode_utf8(bytes: &[u8]) -> Result<String, ToolError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|e| ToolError::Execution(format!("Failed to decode file as UTF-8: {}", e)))
}

fn write_move_if_unchanged(
    source: &LockedFile<'_>,
    target: &LockedFile<'_>,
    expected: &[u8],
    content: &[u8],
) -> Result<(), ToolError> {
    if source.read()? != expected {
        return Err(ToolError::Execution(STALE_FILE_MESSAGE.to_string()));
    }
    target.create_new(content)?;
    if let Err(error) = source.remove_if_unchanged(expected) {
        let _ = target.remove_if_unchanged(content);
        return Err(error);
    }
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
        let changes = result.metadata["changes"]
            .as_array()
            .expect("patch changes");
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0]["old_text"], "one\ntwo\n");
        assert_eq!(changes[0]["new_text"], "one\nthree\n");
    }

    #[test]
    fn preview_patch_builds_full_file_changes_without_mutating_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let deleted = dir.path().join("deleted.txt");
        std::fs::write(&source, "one\ntwo\n").unwrap();
        std::fs::write(&deleted, "remove me\n").unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n*** Move to: moved.txt\n@@\n one\n-two\n+three\n*** Delete File: {}\n*** Add File: added.txt\n+new file\n*** End Patch\n",
            source.display(),
            deleted.display()
        );

        let changes = preview_patch(&serde_json::json!({ "patch": patch }), dir.path()).unwrap();

        assert_eq!(changes.len(), 4);
        assert!(changes.iter().any(|change| {
            change.path == source
                && change.old_text.as_deref() == Some("one\ntwo\n")
                && change.new_text.is_empty()
        }));
        assert!(changes.iter().any(|change| {
            change.path == dir.path().join("moved.txt")
                && change.old_text.is_none()
                && change.new_text == "one\nthree\n"
        }));
        assert!(changes.iter().any(|change| {
            change.path == deleted
                && change.old_text.as_deref() == Some("remove me\n")
                && change.new_text.is_empty()
        }));
        assert!(changes.iter().any(|change| {
            change.path == dir.path().join("added.txt")
                && change.old_text.is_none()
                && change.new_text == "new file\n"
        }));
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "one\ntwo\n");
        assert_eq!(std::fs::read_to_string(&deleted).unwrap(), "remove me\n");
        assert!(!dir.path().join("moved.txt").exists());
        assert!(!dir.path().join("added.txt").exists());
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

    #[tokio::test]
    async fn apply_patch_matches_hunk_with_trailing_whitespace_drift() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one   \ntwo\n").unwrap();
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

    #[tokio::test]
    async fn apply_patch_add_refuses_to_overwrite_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "existing\n").unwrap();
        let patch = format!(
            "*** Begin Patch\n*** Add File: {}\n+replacement\n*** End Patch\n",
            file.display()
        );

        let err = ApplyPatchTool::new()
            .execute(serde_json::json!({ "patch": patch }), &test_context())
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("Refusing to overwrite existing file"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "existing\n");
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
