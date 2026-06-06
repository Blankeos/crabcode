use crate::session::types::{Message, MessageRole};
use crate::theme::ThemeColors;
use crate::ui::markdown::streaming::{render_markdown, SimpleStreamingRenderer};
use crate::ui::scrollbar::{
    render_scrollbar, scrollbar_grab_offset, scrollbar_offset_from_row_with_grab, ScrollMetrics,
};
use crate::ui::selection::{non_selectable_style, EdgeScrollDirection, Selection};
use crate::ui::wrapping::{wrap_styled_line, WrapOptions};
use crate::utils::token_counter::StreamingTokenCounter;
use ratatui::{
    crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, ScrollbarState},
    Frame,
};
use serde_json::Value as JsonValue;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub messages: Vec<Message>,
    pub scroll_offset: usize,
    pub scrollbar_state: ScrollbarState,
    pub is_dragging_scrollbar: bool,
    scrollbar_drag_offset: Option<u16>,
    pub content_height: usize,
    pub viewport_height: usize,
    // Streaming metrics tracking (per streaming turn)
    pub streaming_start_time: Option<std::time::Instant>,
    pub streaming_first_token_time: Option<std::time::Instant>,
    pub streaming_end_time: Option<std::time::Instant>,
    pub streaming_t0_ms: Option<u64>,
    pub streaming_t1_ms: Option<u64>,
    pub streaming_tn_ms: Option<u64>,
    pub streaming_token_count: usize,
    streaming_pause_started_at: Option<std::time::Instant>,
    streaming_paused_duration: std::time::Duration,
    streaming_token_counter: Option<StreamingTokenCounter>,
    /// Whether to autoscroll to bottom when new content arrives
    /// Only autoscrolls if user is already near the bottom
    pub autoscroll_enabled: bool,
    /// Track if user has manually scrolled up (away from bottom)
    user_scrolled_up: bool,
    /// Last calculated tokens per second value (for throttling display updates)
    cached_tokens_per_sec: Option<f64>,
    /// Last time tokens per second was calculated (for throttling updates)
    last_tps_calculated: Option<std::time::Instant>,
    /// Markdown renderer for the last (streaming) message
    streaming_renderer: Option<SimpleStreamingRenderer>,
    /// Index of the message currently being rendered by streaming_renderer
    streaming_message_idx: Option<usize>,
    /// Whether assistant reasoning/thinking text is expanded in chat.
    thinking_visible: bool,
    /// Starting line positions for each message in the rendered content
    pub message_line_positions: Vec<usize>,
    /// Text selection state for copy-on-select
    pub selection: Selection,
    selection_edge_scroll: Option<SelectionEdgeScroll>,
    /// Anchor that existed before the current mouse click started.
    pending_click_anchor: Option<(usize, usize)>,
    /// Index of the message highlighted by timeline navigation (None = no highlight)
    pub highlighted_message_index: Option<usize>,
    /// Monotonic marker for render-affecting message changes.
    render_revision: u64,
    /// Render cache keyed by revision, width, and theme to skip expensive re-formatting.
    cached_lines: Vec<Line<'static>>,
    cached_positions: Vec<usize>,
    cached_revision: u64,
    cached_width: usize,
    cached_colors_hash: u64,
    cached_fingerprint: u64,
    cached_active_tools_revision: std::cell::Cell<u64>,
    cached_has_active_tools: std::cell::Cell<bool>,
    tool_marker_animation_phase: bool,
    hovered_image: Option<ChatImageTarget>,
    hovered_hyperlink: Option<ChatHyperlinkHover>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionEdgeScroll {
    direction: EdgeScrollDirection,
    column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatImageTarget {
    pub message_index: usize,
    pub image_index: usize,
    pub placeholder: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatHyperlinkHover {
    content_line: usize,
    range: crate::ui::hyperlink::HyperlinkRange,
}

// Minimum elapsed time before showing tokens/s (250ms)
const MIN_TOKENS_PER_SECOND_ELAPSED_MS: u128 = 250;
const TOOL_RESULT_MAX_SCREEN_LINES: usize = 8;
const PATCH_DIFF_PREVIEW_MAX_LINES: usize = 40;
const TOOL_MARKER_ACTIVE: &str = "⬡";
const TOOL_MARKER_DONE: &str = "⬢";

#[derive(Debug, Clone)]
struct ParsedToolMessage {
    name: String,
    status: String,
    args: Option<JsonValue>,
    metadata: Option<JsonValue>,
    output_preview: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExplorationToolItem {
    label: &'static str,
    target: String,
    active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskToolItem {
    subagent_type: String,
    description: String,
    active: bool,
    failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanStep {
    step: String,
    status: PlanStepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanUpdateDisplay {
    explanation: Option<String>,
    plan: Vec<PlanStep>,
}

#[derive(Default)]
struct PatchPreview {
    paths: Vec<String>,
    added: usize,
    removed: usize,
    files: Vec<PatchFilePreview>,
    truncated: bool,
}

#[derive(Default)]
struct PatchFilePreview {
    path: String,
    diff_lines: Vec<crate::ui::diff::DiffLine>,
}

enum PatchPreviewMode {
    None,
    AddFile {
        new_line: usize,
    },
    Hunk {
        old_line: Option<usize>,
        new_line: Option<usize>,
        pending: Vec<(char, String)>,
    },
}

fn patch_preview_from_text(patch: &str) -> PatchPreview {
    let mut preview = PatchPreview {
        paths: crate::tools::patch::extract_patch_paths(patch)
            .into_iter()
            .map(|path| display_path(&path, false))
            .collect(),
        ..PatchPreview::default()
    };
    let lines = patch_lines_without_fences(patch);
    let mut mode = PatchPreviewMode::None;
    let mut current_file = None::<usize>;
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        let next = lines.get(index + 1).copied();

        if trimmed == r"\ No newline at end of file" || trimmed.starts_with("```") {
            index += 1;
            continue;
        }

        if trimmed.starts_with("*** Add File: ") {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            let path = trimmed
                .strip_prefix("*** Add File: ")
                .expect("prefix already checked");
            current_file = Some(push_patch_file_preview(&mut preview, path));
            mode = PatchPreviewMode::AddFile { new_line: 1 };
            index += 1;
            continue;
        }

        if let Some(path) = trimmed
            .strip_prefix("*** Update File: ")
            .or_else(|| trimmed.strip_prefix("*** Delete File: "))
            .or_else(|| trimmed.strip_prefix("*** Move to: "))
        {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            current_file = Some(push_patch_file_preview(&mut preview, path));
            mode = PatchPreviewMode::None;
            index += 1;
            continue;
        }

        if trimmed == "*** Begin Patch" || trimmed == "*** End Patch" {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            index += 1;
            continue;
        }

        if line.starts_with("diff --git ")
            || line.starts_with("index ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
        {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            mode = PatchPreviewMode::None;
            index += 1;
            continue;
        }

        if line.starts_with("--- ") && next.is_some_and(|next| next.starts_with("+++ ")) {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            current_file = next
                .and_then(unified_diff_path_from_plus_header)
                .map(|path| {
                    if path == "/dev/null" {
                        let old_path = line
                            .strip_prefix("--- ")
                            .map(normalize_diff_preview_path)
                            .unwrap_or_default();
                        push_patch_file_preview(&mut preview, &old_path)
                    } else {
                        push_patch_file_preview(&mut preview, &path)
                    }
                });
            mode = PatchPreviewMode::None;
            index += 1;
            continue;
        }
        if line.starts_with("+++ ") {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            mode = PatchPreviewMode::None;
            index += 1;
            continue;
        }

        if line.starts_with("@@") {
            flush_patch_hunk(&mut preview, current_file, &mut mode);
            let (old_line, new_line) = parse_patch_hunk_start(line);
            mode = PatchPreviewMode::Hunk {
                old_line,
                new_line,
                pending: Vec::new(),
            };
            index += 1;
            continue;
        }

        match &mut mode {
            PatchPreviewMode::AddFile { new_line } => {
                if let Some(text) = line.strip_prefix('+') {
                    let line_number = Some(*new_line);
                    *new_line += 1;
                    push_patch_diff_line(
                        &mut preview,
                        current_file,
                        crate::ui::diff::DiffLineType::Add,
                        line_number,
                        text,
                    );
                }
            }
            PatchPreviewMode::Hunk { pending, .. } => {
                let Some((prefix, text)) = split_patch_line(line) else {
                    flush_patch_hunk(&mut preview, current_file, &mut mode);
                    index += 1;
                    continue;
                };
                pending.push((prefix, text.to_string()));
            }
            PatchPreviewMode::None => {}
        }

        index += 1;
    }

    flush_patch_hunk(&mut preview, current_file, &mut mode);

    if preview.truncated {
        let file_index = current_file.unwrap_or_else(|| ensure_patch_file_preview(&mut preview));
        if let Some(file) = preview.files.get_mut(file_index) {
            file.diff_lines.push(crate::ui::diff::DiffLine {
                line_type: crate::ui::diff::DiffLineType::Context,
                line_number: None,
                text: "⋯".to_string(),
            });
        }
    }

    preview
}

fn push_patch_file_preview(preview: &mut PatchPreview, path: &str) -> usize {
    let path = display_path(&normalize_diff_preview_path(path), false);
    if let Some(index) = preview.files.iter().position(|file| file.path == path) {
        return index;
    }
    preview.files.push(PatchFilePreview {
        path,
        diff_lines: Vec::new(),
    });
    preview.files.len() - 1
}

fn ensure_patch_file_preview(preview: &mut PatchPreview) -> usize {
    if preview.files.is_empty() {
        let path = preview
            .paths
            .first()
            .cloned()
            .unwrap_or_else(|| "Patch".to_string());
        preview.files.push(PatchFilePreview {
            path,
            diff_lines: Vec::new(),
        });
    }
    preview.files.len() - 1
}

fn unified_diff_path_from_plus_header(line: &str) -> Option<String> {
    line.strip_prefix("+++ ").map(normalize_diff_preview_path)
}

fn flush_patch_hunk(
    preview: &mut PatchPreview,
    file_index: Option<usize>,
    mode: &mut PatchPreviewMode,
) {
    let PatchPreviewMode::Hunk {
        old_line,
        new_line,
        pending,
    } = mode
    else {
        return;
    };

    if pending.is_empty() {
        return;
    }

    let (mut old_cursor, mut new_cursor) = (*old_line, *new_line);
    if (old_cursor.is_none() || new_cursor.is_none()) && file_index.is_some() {
        if let Some(inferred) = infer_patch_hunk_start(preview, file_index, pending) {
            old_cursor.get_or_insert(inferred);
            new_cursor.get_or_insert(inferred);
        }
    }

    let pending_lines = std::mem::take(pending);
    for (prefix, text) in pending_lines {
        match prefix {
            ' ' => {
                let line_number = new_cursor;
                increment_optional_line(&mut old_cursor);
                increment_optional_line(&mut new_cursor);
                push_patch_diff_line(
                    preview,
                    file_index,
                    crate::ui::diff::DiffLineType::Context,
                    line_number,
                    &text,
                );
            }
            '-' => {
                let line_number = old_cursor;
                increment_optional_line(&mut old_cursor);
                push_patch_diff_line(
                    preview,
                    file_index,
                    crate::ui::diff::DiffLineType::Remove,
                    line_number,
                    &text,
                );
            }
            '+' => {
                let line_number = new_cursor;
                increment_optional_line(&mut new_cursor);
                push_patch_diff_line(
                    preview,
                    file_index,
                    crate::ui::diff::DiffLineType::Add,
                    line_number,
                    &text,
                );
            }
            _ => {}
        }
    }
}

fn infer_patch_hunk_start(
    preview: &PatchPreview,
    file_index: Option<usize>,
    pending: &[(char, String)],
) -> Option<usize> {
    let path = file_index
        .and_then(|index| preview.files.get(index))
        .map(|file| file.path.as_str())
        .or_else(|| preview.paths.first().map(String::as_str))?;
    let content = std::fs::read_to_string(path).ok()?;
    let old_text = patch_hunk_side_text(pending, '+');
    let new_text = patch_hunk_side_text(pending, '-');
    if old_text.is_empty() && new_text.is_empty() {
        return Some(1);
    }

    let byte_offset = find_hunk_text_offset(&content, &old_text)
        .or_else(|| find_hunk_text_offset(&content, &new_text))?;
    Some(content[..byte_offset].lines().count() + 1)
}

fn patch_hunk_side_text(pending: &[(char, String)], excluded_prefix: char) -> String {
    pending
        .iter()
        .filter(|(prefix, _)| *prefix != excluded_prefix)
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_hunk_text_offset(content: &str, text: &str) -> Option<usize> {
    if text.is_empty() {
        return Some(0);
    }
    content.find(text).or_else(|| {
        let with_newline = format!("{}\n", text);
        content.find(&with_newline)
    })
}

fn normalize_diff_preview_path(raw: &str) -> String {
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

fn patch_lines_without_fences(patch: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = patch.trim().lines().collect();
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
    lines
}

fn parse_patch_hunk_start(line: &str) -> (Option<usize>, Option<usize>) {
    let mut old_line = None;
    let mut new_line = None;
    for part in line.split_whitespace() {
        if old_line.is_none() && part.starts_with('-') {
            old_line = parse_patch_range_start(part);
        } else if new_line.is_none() && part.starts_with('+') {
            new_line = parse_patch_range_start(part);
        }
    }
    (old_line, new_line)
}

fn parse_patch_range_start(part: &str) -> Option<usize> {
    part.get(1..)?
        .split(',')
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|line| line.max(1))
}

fn split_patch_line(line: &str) -> Option<(char, &str)> {
    let prefix = line.chars().next()?;
    if matches!(prefix, ' ' | '-' | '+') {
        Some((prefix, &line[prefix.len_utf8()..]))
    } else {
        None
    }
}

fn increment_optional_line(line: &mut Option<usize>) {
    if let Some(value) = line.as_mut() {
        *value += 1;
    }
}

fn push_patch_diff_line(
    preview: &mut PatchPreview,
    file_index: Option<usize>,
    line_type: crate::ui::diff::DiffLineType,
    line_number: Option<usize>,
    text: &str,
) {
    match line_type {
        crate::ui::diff::DiffLineType::Add => preview.added += 1,
        crate::ui::diff::DiffLineType::Remove => preview.removed += 1,
        crate::ui::diff::DiffLineType::Context => {}
    }

    if patch_preview_line_count(preview) < PATCH_DIFF_PREVIEW_MAX_LINES {
        let file_index = file_index.unwrap_or_else(|| ensure_patch_file_preview(preview));
        if let Some(file) = preview.files.get_mut(file_index) {
            file.diff_lines.push(crate::ui::diff::DiffLine {
                line_type,
                line_number,
                text: text.to_string(),
            });
        }
    } else {
        preview.truncated = true;
    }
}

fn patch_preview_line_count(preview: &PatchPreview) -> usize {
    preview.files.iter().map(|file| file.diff_lines.len()).sum()
}

fn now_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    (chars.saturating_add(3)) / 4
}

fn parse_tool_message(content: &str) -> Option<ParsedToolMessage> {
    let JsonValue::Object(obj) = serde_json::from_str::<JsonValue>(content).ok()? else {
        return None;
    };

    Some(ParsedToolMessage {
        name: obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("tool")
            .to_string(),
        status: obj
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("ok")
            .to_string(),
        args: obj.get("args").cloned(),
        metadata: obj.get("metadata").cloned(),
        output_preview: obj
            .get("output_preview")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        title: obj
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

fn assistant_tool_result_ids(message: &Message) -> std::collections::HashSet<String> {
    message
        .parts
        .iter()
        .filter(|part| part.part_type == "tool_result")
        .filter_map(|part| part.tool_id().map(|id| id.to_string()))
        .collect()
}

fn assistant_tool_part_content(
    message: &Message,
    part: &crate::session::types::MessagePart,
    result_ids: &std::collections::HashSet<String>,
) -> Option<String> {
    match part.part_type.as_str() {
        "tool_call" => {
            let id = part.tool_id()?;
            if result_ids.contains(id) {
                return None;
            }

            let mut payload = part.data.clone();
            if payload.get("status").is_none() {
                payload["status"] = JsonValue::String("running".to_string());
            }
            serde_json::to_string(&payload).ok()
        }
        "tool_result" => {
            let mut payload = part.data.clone();
            if payload.get("args").is_none() {
                if let Some(id) = part.tool_id() {
                    if let Some(args) = message
                        .tool_call_part_data(id)
                        .and_then(|call| call.get("args"))
                        .cloned()
                    {
                        payload["args"] = args;
                    }
                }
            }
            serde_json::to_string(&payload).ok()
        }
        _ => None,
    }
}

fn arg_string<'a>(
    obj: Option<&'a serde_json::Map<String, JsonValue>>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| obj.and_then(|o| o.get(*key)).and_then(|v| v.as_str()))
        .filter(|value| !value.trim().is_empty())
}

fn strip_tool_title<'a>(title: Option<&'a str>, label: &str) -> Option<&'a str> {
    let prefix = format!("{}:", label);
    title
        .and_then(|value| value.strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn display_path(raw: &str, basename_only: bool) -> String {
    let trimmed = raw.trim();
    let path = std::path::Path::new(trimmed);

    if basename_only {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(trimmed)
            .to_string();
    }

    if path.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(rel) = path.strip_prefix(cwd) {
                let rendered = rel.to_string_lossy();
                return if rendered.is_empty() {
                    ".".to_string()
                } else {
                    rendered.into_owned()
                };
            }
        }
    }

    trimmed.to_string()
}

fn tool_path_candidates(message: &Message) -> Vec<std::path::PathBuf> {
    if message.role != MessageRole::Tool {
        return Vec::new();
    }

    let Some(info) = parse_tool_message(&message.content) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    let mut push_candidate = |value: Option<&str>| {
        if let Some(path) = value.and_then(path_candidate_from_value) {
            if !candidates.iter().any(|candidate| candidate == &path) {
                candidates.push(path);
            }
        }
    };

    let args_obj = info.args.as_ref().and_then(|value| value.as_object());
    let metadata_obj = info.metadata.as_ref().and_then(|value| value.as_object());
    for key in ["path", "file_path", "filePath"] {
        push_candidate(arg_string(args_obj, &[key]));
        push_candidate(arg_string(metadata_obj, &[key]));
    }

    if let Some(title) = info.title.as_deref() {
        push_candidate(title.split_once(':').map(|(_, path)| path.trim()));
    }

    candidates
}

fn matching_tool_path(message: &Message, display: &str) -> Option<std::path::PathBuf> {
    tool_path_candidates(message)
        .into_iter()
        .find(|path| path_matches_display(path, display))
}

fn path_candidate_from_value(value: &str) -> Option<std::path::PathBuf> {
    let path_text = value.trim();
    if path_text.is_empty() {
        return None;
    }

    if path_text.starts_with("file://") {
        return url::Url::parse(path_text).ok()?.to_file_path().ok();
    }

    if let Some(rest) = path_text.strip_prefix("~/") {
        return dirs::home_dir().map(|home| home.join(rest));
    }

    let path = std::path::PathBuf::from(path_text);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(path))
    }
}

fn path_matches_display(path: &std::path::Path, display: &str) -> bool {
    if display.is_empty() {
        return false;
    }

    let path_text = path.to_string_lossy();
    let candidates = [
        path_text.into_owned(),
        display_path(&path.to_string_lossy(), false),
        display_path(&path.to_string_lossy(), true),
    ];

    candidates
        .iter()
        .any(|candidate| display_matches_candidate(display, candidate))
}

fn display_matches_candidate(display: &str, candidate: &str) -> bool {
    display == candidate
        || display
            .strip_prefix(candidate)
            .is_some_and(is_display_location_suffix)
}

fn is_display_location_suffix(suffix: &str) -> bool {
    let Some(rest) = suffix.strip_prefix(':') else {
        return false;
    };

    !rest.is_empty()
        && rest
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, ':' | '-'))
}

fn search_target(
    args_obj: Option<&serde_json::Map<String, JsonValue>>,
    title: Option<&str>,
    title_label: &str,
) -> Option<String> {
    let query = arg_string(args_obj, &["pattern", "query"])
        .or_else(|| strip_tool_title(title, title_label))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let path = arg_string(args_obj, &["path"]);
    let include = arg_string(args_obj, &["include"]);

    let mut target = query.to_string();
    if let Some(path) = path.filter(|path| *path != ".") {
        target.push_str(" in ");
        target.push_str(&display_path(path, false));
    }
    if let Some(include) = include {
        target.push_str(" include=");
        target.push_str(include);
    }

    Some(target)
}

fn exploration_tool_item(info: &ParsedToolMessage) -> Option<ExplorationToolItem> {
    if info.status == "error" {
        return None;
    }

    let args_obj = info.args.as_ref().and_then(|v| v.as_object());
    let title = info.title.as_deref();
    let active = matches!(info.status.as_str(), "running" | "pending");

    let (label, target) = match info.name.as_str() {
        "read" => {
            let target = arg_string(args_obj, &["file_path", "filePath", "path"])
                .or_else(|| strip_tool_title(title, "Read"))
                .map(|path| display_path(path, true))?;
            ("Read", target)
        }
        "list" => {
            let target = arg_string(args_obj, &["path"])
                .or_else(|| strip_tool_title(title, "List"))
                .map(|path| display_path(path, false))?;
            ("List", target)
        }
        "glob" => ("Search", search_target(args_obj, title, "Glob")?),
        "grep" => ("Search", search_target(args_obj, title, "Grep")?),
        _ => return None,
    };

    Some(ExplorationToolItem {
        label,
        target,
        active,
    })
}

fn exploration_tool_item_for_message(message: &Message) -> Option<ExplorationToolItem> {
    if message.role != MessageRole::Tool {
        return None;
    }

    parse_tool_message(&message.content)
        .as_ref()
        .and_then(exploration_tool_item)
}

fn task_tool_item(info: &ParsedToolMessage) -> Option<TaskToolItem> {
    if info.name != "task" {
        return None;
    }

    let args_obj = info.args.as_ref().and_then(|v| v.as_object());
    let subagent_type = args_obj
        .and_then(|o| o.get("subagent_type"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            info.metadata
                .as_ref()
                .and_then(|m| m.get("subagent_type"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("general");
    let description = args_obj
        .and_then(|o| o.get("description"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            info.metadata
                .as_ref()
                .and_then(|m| m.get("child_session_title"))
                .and_then(|v| v.as_str())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Task");

    Some(TaskToolItem {
        subagent_type: titlecase_ascii(subagent_type),
        description: description.to_string(),
        active: matches!(info.status.as_str(), "running" | "pending"),
        failed: info.status == "error",
    })
}

fn task_tool_item_for_message(message: &Message) -> Option<TaskToolItem> {
    if message.role != MessageRole::Tool {
        return None;
    }

    parse_tool_message(&message.content)
        .as_ref()
        .and_then(task_tool_item)
}

fn metadata_usize(metadata: Option<&JsonValue>, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|key| {
            metadata
                .and_then(|m| m.get(*key))
                .and_then(|value| value.as_u64())
        })
        .map(|value| value as usize)
}

fn parse_line_number(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("line ")? + "line ".len();
    let digits: String = lower[start..]
        .chars()
        .skip_while(|ch| ch.is_ascii_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn titlecase_ascii(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_ascii_uppercase().to_string() + chars.as_str()
}

fn normalize_plan_status(status: Option<&str>) -> PlanStepStatus {
    match status
        .unwrap_or("pending")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "completed" | "complete" | "done" | "x" | "✓" | "✔" => PlanStepStatus::Completed,
        "in_progress" | "in-progress" | "in progress" | "doing" | "active" | "current" => {
            PlanStepStatus::InProgress
        }
        _ => PlanStepStatus::Pending,
    }
}

fn strip_plain_list_marker(line: &str) -> &str {
    let trimmed = line.trim();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return rest.trim_start();
    }

    if let Some((prefix, rest)) = trimmed.split_once(". ") {
        if !prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit()) {
            return rest.trim_start();
        }
    }

    trimmed
}

fn parse_plan_checkbox_line(line: &str) -> Option<PlanStep> {
    let line = strip_plain_list_marker(line);
    let (status, rest) = if let Some(rest) = line.strip_prefix("[ ]") {
        (PlanStepStatus::Pending, rest)
    } else if let Some(rest) = line.strip_prefix("[x]") {
        (PlanStepStatus::Completed, rest)
    } else if let Some(rest) = line.strip_prefix("[X]") {
        (PlanStepStatus::Completed, rest)
    } else if let Some(rest) = line.strip_prefix("[✓]") {
        (PlanStepStatus::Completed, rest)
    } else if let Some(rest) = line.strip_prefix("[✔]") {
        (PlanStepStatus::Completed, rest)
    } else if let Some(rest) = line.strip_prefix("✔") {
        (PlanStepStatus::Completed, rest)
    } else if let Some(rest) = line.strip_prefix("[•]") {
        (PlanStepStatus::InProgress, rest)
    } else if let Some(rest) = line.strip_prefix("•") {
        (PlanStepStatus::InProgress, rest)
    } else if let Some(rest) = line.strip_prefix("□") {
        (PlanStepStatus::Pending, rest)
    } else {
        return None;
    };

    let step = rest.trim();
    if step.is_empty() {
        None
    } else {
        Some(PlanStep {
            step: step.to_string(),
            status,
        })
    }
}

fn plan_steps_from_text(raw: &str) -> Vec<PlanStep> {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            parse_plan_checkbox_line(trimmed).or_else(|| {
                let step = strip_plain_list_marker(trimmed);
                if step.is_empty() {
                    None
                } else {
                    Some(PlanStep {
                        step: step.to_string(),
                        status: PlanStepStatus::Pending,
                    })
                }
            })
        })
        .collect()
}

fn plan_step_from_json(value: &JsonValue) -> Option<PlanStep> {
    match value {
        JsonValue::Object(obj) => {
            let step = ["step", "content", "todo", "task", "title", "description"]
                .iter()
                .find_map(|key| obj.get(*key).and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(PlanStep {
                step: step.to_string(),
                status: normalize_plan_status(obj.get("status").and_then(|v| v.as_str())),
            })
        }
        JsonValue::String(step) => {
            let trimmed = step.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.lines().count() > 1
                || trimmed
                    .lines()
                    .any(|line| parse_plan_checkbox_line(line).is_some())
            {
                let steps = plan_steps_from_text(trimmed);
                if steps.len() == 1 {
                    steps.into_iter().next()
                } else {
                    None
                }
            } else {
                Some(PlanStep {
                    step: trimmed.to_string(),
                    status: PlanStepStatus::Pending,
                })
            }
        }
        _ => None,
    }
}

fn plan_steps_from_json(value: &JsonValue) -> Vec<PlanStep> {
    match value {
        JsonValue::Array(items) => items.iter().filter_map(plan_step_from_json).collect(),
        JsonValue::Object(_) => plan_step_from_json(value).into_iter().collect(),
        JsonValue::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.starts_with('[') || trimmed.starts_with('{') {
                if let Ok(parsed) = serde_json::from_str::<JsonValue>(trimmed) {
                    let parsed_steps = plan_steps_from_json(&parsed);
                    if !parsed_steps.is_empty() {
                        return parsed_steps;
                    }
                }
            }
            plan_steps_from_text(trimmed)
        }
        _ => Vec::new(),
    }
}

fn plan_update_display(
    name: &str,
    args: &Option<JsonValue>,
    metadata: &Option<JsonValue>,
    output_preview: &Option<String>,
) -> Option<PlanUpdateDisplay> {
    if !matches!(name, "update_plan" | "todowrite") {
        return None;
    }

    let explanation = metadata
        .as_ref()
        .and_then(|m| m.get("explanation"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            args.as_ref()
                .and_then(|a| a.get("explanation"))
                .and_then(|v| v.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    let plan_value = metadata
        .as_ref()
        .and_then(|m| m.get("plan").or_else(|| m.get("todo_items")))
        .or_else(|| {
            args.as_ref()
                .and_then(|a| a.get("plan").or_else(|| a.get("todos")))
        });

    let mut plan = plan_value.map(plan_steps_from_json).unwrap_or_default();
    if plan.is_empty() {
        if let Some(preview) = output_preview.as_deref() {
            plan = plan_steps_from_text(preview);
        }
    }

    if plan.is_empty() {
        None
    } else {
        Some(PlanUpdateDisplay { explanation, plan })
    }
}

impl Chat {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            is_dragging_scrollbar: false,
            scrollbar_drag_offset: None,
            content_height: 0,
            viewport_height: 0,
            streaming_start_time: None,
            streaming_first_token_time: None,
            streaming_end_time: None,
            streaming_t0_ms: None,
            streaming_t1_ms: None,
            streaming_tn_ms: None,
            streaming_token_count: 0,
            streaming_pause_started_at: None,
            streaming_paused_duration: std::time::Duration::default(),
            streaming_token_counter: None,
            autoscroll_enabled: true,
            user_scrolled_up: false,
            cached_tokens_per_sec: None,
            last_tps_calculated: None,
            streaming_renderer: None,
            streaming_message_idx: None,
            thinking_visible: true,
            message_line_positions: Vec::new(),
            selection: Selection::new(),
            selection_edge_scroll: None,
            pending_click_anchor: None,
            highlighted_message_index: None,
            render_revision: 1,
            cached_lines: Vec::new(),
            cached_positions: Vec::new(),
            cached_revision: 0,
            cached_width: 0,
            cached_colors_hash: 0,
            cached_fingerprint: 0,
            cached_active_tools_revision: std::cell::Cell::new(0),
            cached_has_active_tools: std::cell::Cell::new(false),
            tool_marker_animation_phase: false,
            hovered_image: None,
            hovered_hyperlink: None,
        }
    }

    pub fn with_messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            is_dragging_scrollbar: false,
            scrollbar_drag_offset: None,
            content_height: 0,
            viewport_height: 0,
            streaming_start_time: None,
            streaming_first_token_time: None,
            streaming_end_time: None,
            streaming_t0_ms: None,
            streaming_t1_ms: None,
            streaming_tn_ms: None,
            streaming_token_count: 0,
            streaming_pause_started_at: None,
            streaming_paused_duration: std::time::Duration::default(),
            streaming_token_counter: None,
            autoscroll_enabled: true,
            user_scrolled_up: false,
            cached_tokens_per_sec: None,
            last_tps_calculated: None,
            streaming_renderer: None,
            streaming_message_idx: None,
            thinking_visible: true,
            message_line_positions: Vec::new(),
            selection: Selection::new(),
            selection_edge_scroll: None,
            pending_click_anchor: None,
            highlighted_message_index: None,
            render_revision: 1,
            cached_lines: Vec::new(),
            cached_positions: Vec::new(),
            cached_revision: 0,
            cached_width: 0,
            cached_colors_hash: 0,
            cached_fingerprint: 0,
            cached_active_tools_revision: std::cell::Cell::new(0),
            cached_has_active_tools: std::cell::Cell::new(false),
            tool_marker_animation_phase: false,
            hovered_image: None,
            hovered_hyperlink: None,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.invalidate_cache();
        if self.should_autoscroll() {
            // Reset scroll to show new content at bottom
            // Content height will be recalculated on next render
            self.scroll_offset = usize::MAX;
            self.user_scrolled_up = false;
        }
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.invalidate_cache();
    }

    pub fn truncate_messages(&mut self, len: usize) {
        self.messages.truncate(len);
        self.invalidate_cache();
    }

    pub fn mark_render_dirty(&mut self) {
        self.invalidate_cache();
    }

    pub fn render_revision(&self) -> u64 {
        self.render_revision
    }

    pub fn thinking_visible(&self) -> bool {
        self.thinking_visible
    }

    pub fn set_thinking_visible(&mut self, visible: bool) {
        if self.thinking_visible == visible {
            return;
        }

        self.thinking_visible = visible;
        self.invalidate_cache();
    }

    fn should_autoscroll(&self) -> bool {
        self.autoscroll_enabled && !self.user_scrolled_up
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::user(content));
    }

    pub fn add_user_message_with_agent_mode(
        &mut self,
        content: impl Into<String>,
        agent_mode: String,
    ) {
        let mut msg = Message::user(content);
        msg.agent_mode = Some(agent_mode);
        self.add_message(msg);
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::assistant(content));
    }

    fn streaming_assistant_idx(&self) -> Option<usize> {
        self.messages
            .iter()
            .rposition(|m| m.role == MessageRole::Assistant && !m.is_complete)
    }

    pub fn append_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        let chunk_str = chunk.as_ref();

        // Append only if the last message is the current streaming assistant segment.
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant && !m.is_complete)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append(chunk_str);
            }
        } else {
            // Start a new assistant segment (e.g. after tool rows).
            self.add_message(Message::incomplete(chunk_str));
        }

        self.invalidate_cache();

        let now = std::time::Instant::now();
        if self.streaming_start_time.is_none() {
            // Fallback: streaming should normally be initialized by begin_streaming_turn().
            self.streaming_start_time = Some(now);
            self.streaming_t0_ms = Some(now_epoch_ms());
        }
        if self.streaming_first_token_time.is_none() {
            self.streaming_first_token_time = Some(now);
            self.streaming_t1_ms = Some(now_epoch_ms());
        }

        self.update_streaming_token_count(chunk_str);
        if self.should_autoscroll() {
            self.scroll_offset = usize::MAX;
            self.user_scrolled_up = false;
        }
    }

    pub fn append_reasoning_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        let chunk_str = chunk.as_ref();

        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant && !m.is_complete)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append_reasoning(chunk_str);
            }
        } else {
            let mut msg = Message::incomplete("");
            msg.append_reasoning(chunk_str);
            self.add_message(msg);
        }

        self.invalidate_cache();

        let now = std::time::Instant::now();
        if self.streaming_start_time.is_none() {
            self.streaming_start_time = Some(now);
            self.streaming_t0_ms = Some(now_epoch_ms());
        }
        if self.streaming_first_token_time.is_none() {
            self.streaming_first_token_time = Some(now);
            self.streaming_t1_ms = Some(now_epoch_ms());
        }
        self.update_streaming_token_count(chunk_str);
        if self.should_autoscroll() {
            self.scroll_offset = usize::MAX;
            self.user_scrolled_up = false;
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.scrollbar_state = ScrollbarState::default();
        self.is_dragging_scrollbar = false;
        self.scrollbar_drag_offset = None;
        self.content_height = 0;
        self.streaming_start_time = None;
        self.streaming_first_token_time = None;
        self.streaming_end_time = None;
        self.streaming_t0_ms = None;
        self.streaming_t1_ms = None;
        self.streaming_tn_ms = None;
        self.streaming_token_count = 0;
        self.streaming_pause_started_at = None;
        self.streaming_paused_duration = std::time::Duration::default();
        self.streaming_token_counter = None;
        self.selection.reset();
        self.pending_click_anchor = None;
        self.hovered_image = None;
        self.hovered_hyperlink = None;
        self.cached_lines.clear();
        self.cached_positions.clear();
        self.cached_revision = 0;
        self.cached_width = 0;
        self.cached_colors_hash = 0;
        self.cached_fingerprint = 0;
        self.cached_active_tools_revision.set(0);
        self.cached_has_active_tools.set(false);
        self.tool_marker_animation_phase = false;
        self.invalidate_cache();
    }

    fn invalidate_cache(&mut self) {
        self.render_revision = self.render_revision.wrapping_add(1).max(1);
        self.cached_fingerprint = 0;
        self.cached_active_tools_revision.set(0);
    }

    fn cache_colors_hash(colors: &ThemeColors) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        colors.hash(&mut h);
        h.finish()
    }

    fn compute_fingerprint(&self, max_width: usize, colors: &ThemeColors) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // Bump this whenever rendering logic changes (tables, markdown, etc.)
        const RENDER_VERSION: u64 = 9;
        RENDER_VERSION.hash(&mut h);
        colors.hash(&mut h);
        self.thinking_visible.hash(&mut h);
        self.messages.len().hash(&mut h);
        for msg in &self.messages {
            std::mem::discriminant(&msg.role).hash(&mut h);
            msg.content.hash(&mut h);
            msg.reasoning.hash(&mut h);
            for part in &msg.parts {
                part.part_type.hash(&mut h);
                part.data.to_string().hash(&mut h);
            }
            msg.is_complete.hash(&mut h);
            msg.agent_mode.hash(&mut h);
            msg.token_count.hash(&mut h);
            msg.duration_ms.hash(&mut h);
            msg.t0_ms.hash(&mut h);
            msg.t1_ms.hash(&mut h);
            msg.tn_ms.hash(&mut h);
            msg.output_tokens.hash(&mut h);
            msg.model.hash(&mut h);
            msg.provider.hash(&mut h);
            msg.compaction_stats.hash(&mut h);
            msg.was_interrupted.hash(&mut h);
        }
        max_width.hash(&mut h);
        h.finish()
    }

    pub fn begin_streaming_turn(&mut self) {
        let now = std::time::Instant::now();
        let t0_ms = now_epoch_ms();

        self.streaming_start_time = Some(now);
        self.streaming_first_token_time = None;
        self.streaming_end_time = None;
        self.streaming_t0_ms = Some(t0_ms);
        self.streaming_t1_ms = None;
        self.streaming_tn_ms = None;
        self.streaming_token_count = 0;
        self.streaming_pause_started_at = None;
        self.streaming_paused_duration = std::time::Duration::default();
        self.cached_tokens_per_sec = None;
        self.last_tps_calculated = None;

        if let Some(counter) = self.streaming_token_counter.as_mut() {
            counter.reset();
        }

        if let Some(msg) = self
            .messages
            .last_mut()
            .filter(|m| m.role == MessageRole::Assistant && !m.is_complete)
        {
            msg.t0_ms = Some(t0_ms);
        }
    }

    pub fn mark_streaming_end(&mut self) {
        let now = std::time::Instant::now();
        self.streaming_end_time = Some(now);
        self.streaming_tn_ms = Some(now_epoch_ms());
    }

    pub fn get_streaming_tokens_per_sec(&self) -> Option<f64> {
        self.cached_tokens_per_sec
    }

    pub fn pause_streaming_tps_timer(&mut self) {
        if self.streaming_start_time.is_none() {
            return;
        }

        if self.streaming_pause_started_at.is_none() {
            self.streaming_pause_started_at = Some(std::time::Instant::now());
        }
    }

    pub fn resume_streaming_tps_timer(&mut self) {
        if let Some(started) = self.streaming_pause_started_at.take() {
            self.streaming_paused_duration += started.elapsed();
            self.last_tps_calculated = None;
        }
    }

    fn total_paused_duration(&self) -> std::time::Duration {
        let mut paused = self.streaming_paused_duration;
        if let Some(started) = self.streaming_pause_started_at {
            paused += started.elapsed();
        }
        paused
    }

    pub fn get_streaming_elapsed_seconds(&self) -> Option<f64> {
        self.streaming_start_time.map(|start| {
            let elapsed = start.elapsed();
            let paused = self.total_paused_duration();
            elapsed.saturating_sub(paused).as_secs_f64()
        })
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming_first_token_time.is_some() && self.streaming_assistant_idx().is_some()
    }

    pub fn finalize_streaming_metrics(&mut self) {
        let token_count = self.streaming_token_count;

        let t0_ms = self.streaming_t0_ms;
        let t1_ms = self.streaming_t1_ms;
        let tn_ms = self.streaming_tn_ms.or_else(|| {
            // Fallback: if caller didn't mark end, compute an end timestamp now.
            Some(now_epoch_ms())
        });

        let paused_ms = self.total_paused_duration().as_millis();

        let decode_duration_ms = if let (Some(t1), Some(tn)) =
            (self.streaming_first_token_time, self.streaming_end_time)
        {
            tn.duration_since(t1).as_millis().saturating_sub(paused_ms) as u64
        } else if let Some(t1) = self.streaming_first_token_time {
            t1.elapsed().as_millis().saturating_sub(paused_ms) as u64
        } else {
            0
        };

        if let Some(idx) = self
            .messages
            .iter()
            .rposition(|m| m.role == MessageRole::Assistant)
        {
            if let Some(msg) = self.messages.get_mut(idx) {
                msg.output_tokens = Some(token_count);
                msg.token_count = Some(token_count);
                msg.duration_ms = Some(decode_duration_ms);
                msg.t0_ms = t0_ms;
                msg.t1_ms = t1_ms;
                msg.tn_ms = tn_ms;
            }
        }

        // Reset streaming state
        self.streaming_start_time = None;
        self.streaming_first_token_time = None;
        self.streaming_end_time = None;
        self.streaming_t0_ms = None;
        self.streaming_t1_ms = None;
        self.streaming_tn_ms = None;
        self.streaming_token_count = 0;
        self.streaming_pause_started_at = None;
        self.streaming_paused_duration = std::time::Duration::default();
        self.streaming_renderer = None;
        self.streaming_message_idx = None;
        self.streaming_token_counter = None;
        self.invalidate_cache();
    }

    fn current_tool_marker_animation_phase() -> bool {
        (now_epoch_ms() / 500) % 2 == 1
    }

    fn active_tool_marker(&self) -> &'static str {
        if self.tool_marker_animation_phase {
            TOOL_MARKER_DONE
        } else {
            TOOL_MARKER_ACTIVE
        }
    }

    fn tool_marker(&self, active: bool) -> &'static str {
        if active {
            self.active_tool_marker()
        } else {
            TOOL_MARKER_DONE
        }
    }

    pub(crate) fn has_active_tool_messages(&self) -> bool {
        if self.cached_active_tools_revision.get() == self.render_revision {
            return self.cached_has_active_tools.get();
        }

        let has_active_tools = self.messages.iter().rev().any(|message| {
            message.has_running_tool_parts()
                || (message.role == MessageRole::Tool
                    && parse_tool_message(&message.content)
                        .map(|info| matches!(info.status.as_str(), "running" | "pending"))
                        .unwrap_or(false))
        });

        self.cached_has_active_tools.set(has_active_tools);
        self.cached_active_tools_revision.set(self.render_revision);
        has_active_tools
    }

    pub fn prepare_streaming_token_counter(&mut self, model: &str) {
        self.streaming_token_counter = Some(StreamingTokenCounter::new(model));
    }

    fn update_streaming_token_count(&mut self, chunk: &str) {
        if let Some(counter) = self.streaming_token_counter.as_mut() {
            self.streaming_token_count = counter.add_text(chunk);
        } else {
            self.streaming_token_count = self
                .streaming_token_count
                .saturating_add(estimate_tokens(chunk));
        }

        self.update_streaming_tokens_per_sec();
    }

    fn update_streaming_tokens_per_sec(&mut self) {
        const TPS_THROTTLE_MS: u128 = 100;

        let now = std::time::Instant::now();
        if let Some(last_calc) = self.last_tps_calculated {
            if now.duration_since(last_calc).as_millis() < TPS_THROTTLE_MS {
                return;
            }
        }
        self.last_tps_calculated = Some(now);

        let result = if let Some(first_token_time) = self.streaming_first_token_time {
            let paused_ms = self.total_paused_duration().as_millis();
            let elapsed_ms = first_token_time
                .elapsed()
                .as_millis()
                .saturating_sub(paused_ms);
            if elapsed_ms >= MIN_TOKENS_PER_SECOND_ELAPSED_MS && self.streaming_token_count > 0 {
                let tokens_per_sec =
                    (self.streaming_token_count as f64) / (elapsed_ms as f64 / 1000.0);
                if tokens_per_sec.is_finite() {
                    Some(tokens_per_sec)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        self.cached_tokens_per_sec = result;
    }

    /// Update the streaming markdown renderer for the current streaming message
    /// This should be called before render() to ensure the renderer is up to date
    fn update_streaming_renderer(&mut self) {
        // Check if we're streaming and have messages
        if !self.is_streaming() || self.messages.is_empty() {
            // Not streaming, clear renderer if it exists
            if self.streaming_renderer.is_some() {
                self.streaming_renderer = None;
                self.streaming_message_idx = None;
            }
            return;
        }

        let Some(last_idx) = self.streaming_assistant_idx() else {
            if self.streaming_renderer.is_some() {
                self.streaming_renderer = None;
                self.streaming_message_idx = None;
            }
            return;
        };

        // Check if we're still rendering the same message
        if let Some(renderer_idx) = self.streaming_message_idx {
            if renderer_idx != last_idx {
                // Different message, reset renderer
                self.streaming_renderer = Some(SimpleStreamingRenderer::new());
                self.streaming_message_idx = Some(last_idx);
            }
        } else {
            // No renderer yet, create one
            self.streaming_renderer = Some(SimpleStreamingRenderer::new());
            self.streaming_message_idx = Some(last_idx);
        }

        // Update the renderer content if needed
        if let Some(ref mut renderer) = self.streaming_renderer {
            if let Some(msg) = self.messages.get(last_idx) {
                if renderer.content() != msg.content {
                    renderer.reset();
                    renderer.append(&msg.content);
                }
            }
        }
    }

    pub fn scroll_down(&mut self, amount: usize) {
        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_offset);
        // Check if we're now at the bottom
        self.user_scrolled_up = self.scroll_offset < max_offset;
        self.update_scrollbar();
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        self.user_scrolled_up = true;
        self.update_scrollbar();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.content_height.saturating_sub(self.viewport_height);
        self.user_scrolled_up = false;
        self.update_scrollbar();
    }

    pub fn scroll_to_bottom_on_next_render(&mut self) {
        self.scroll_offset = usize::MAX;
        self.user_scrolled_up = false;
        self.update_scrollbar();
    }

    pub fn get_message_line_positions(
        &self,
        max_width: usize,
        model: &str,
        colors: &ThemeColors,
    ) -> Vec<usize> {
        self.build_all_lines_with_positions(max_width, model, colors)
            .1
    }

    pub fn scroll_to_message_index(&mut self, idx: usize) {
        if idx >= self.messages.len() {
            return;
        }

        let line_pos = self.message_line_positions.get(idx).copied().unwrap_or(0);

        // Scroll so the message is visible (near top of viewport, with a small margin)
        let target_offset = line_pos.saturating_sub(2);
        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = target_offset.min(max_offset);
        self.user_scrolled_up = true;
        self.update_scrollbar();
    }

    pub fn set_highlighted_message(&mut self, idx: Option<usize>) {
        self.highlighted_message_index = idx;
    }

    pub fn set_hovered_image(&mut self, target: Option<ChatImageTarget>) -> bool {
        if self.hovered_image == target {
            return false;
        }
        self.hovered_image = target;
        self.cached_revision = 0;
        true
    }

    pub fn clear_hovered_image(&mut self) -> bool {
        self.set_hovered_image(None)
    }

    pub fn set_hovered_hyperlink(&mut self, target: Option<ChatHyperlinkHover>) -> bool {
        if self.hovered_hyperlink == target {
            return false;
        }
        self.hovered_hyperlink = target;
        true
    }

    pub fn clear_hovered_hyperlink(&mut self) -> bool {
        self.set_hovered_hyperlink(None)
    }

    pub fn image_at_position(&self, event: MouseEvent, area: Rect) -> Option<ChatImageTarget> {
        use ratatui::layout::Position;

        let point = Position::new(event.column, event.row);
        let content_area = Self::content_area_for(area);

        if !content_area.contains(point) || self.cached_lines.is_empty() {
            return None;
        }

        let content_line =
            (event.row.saturating_sub(content_area.y) as usize).saturating_add(self.scroll_offset);
        let content_col = event.column.saturating_sub(content_area.x) as usize;
        let message_index =
            self.message_index_at_content_line(content_line, self.content_height)?;
        let line = self.cached_lines.get(content_line)?;
        let placeholder = placeholder_at_line_col(line, content_col)?;
        let image_index = image_index_from_placeholder(&placeholder)?;
        let path = self
            .messages
            .get(message_index)?
            .local_image_paths
            .get(image_index)?
            .clone();

        Some(ChatImageTarget {
            message_index,
            image_index,
            placeholder,
            path,
        })
    }

    pub fn hyperlink_at_position(
        &self,
        event: MouseEvent,
        area: Rect,
    ) -> Option<crate::ui::hyperlink::HyperlinkTarget> {
        use ratatui::layout::Position;

        let point = Position::new(event.column, event.row);
        let content_area = Self::content_area_for(area);

        if !content_area.contains(point) || self.cached_lines.is_empty() {
            return None;
        }

        let content_line =
            (event.row.saturating_sub(content_area.y) as usize).saturating_add(self.scroll_offset);
        let content_col = event.column.saturating_sub(content_area.x) as usize;
        let line = self.cached_lines.get(content_line)?;
        let range = crate::ui::hyperlink::hyperlink_range_at_line_col(line, content_col)?;

        self.resolve_hyperlink_target(content_line, &range)
            .or_else(|| Some(range.target))
    }

    pub fn hyperlink_hover_at_position(
        &self,
        event: MouseEvent,
        area: Rect,
    ) -> Option<ChatHyperlinkHover> {
        use ratatui::layout::Position;

        let point = Position::new(event.column, event.row);
        let content_area = Self::content_area_for(area);

        if !content_area.contains(point) || self.cached_lines.is_empty() {
            return None;
        }

        let content_line =
            (event.row.saturating_sub(content_area.y) as usize).saturating_add(self.scroll_offset);
        let content_col = event.column.saturating_sub(content_area.x) as usize;
        let line = self.cached_lines.get(content_line)?;
        let range = crate::ui::hyperlink::hyperlink_range_at_line_col(line, content_col)?;

        let clickable = self
            .resolve_hyperlink_target(content_line, &range)
            .or_else(|| Some(range.target.clone()))
            .is_some();

        clickable.then_some(ChatHyperlinkHover {
            content_line,
            range,
        })
    }

    fn resolve_hyperlink_target(
        &self,
        content_line: usize,
        range: &crate::ui::hyperlink::HyperlinkRange,
    ) -> Option<crate::ui::hyperlink::HyperlinkTarget> {
        if !matches!(range.target, crate::ui::hyperlink::HyperlinkTarget::File(_)) {
            return None;
        }

        let display = range.text.trim();
        let message_index = self
            .message_index_at_content_line(content_line, self.content_height)
            .or_else(|| self.raw_message_index_at_content_line(content_line, self.content_height));

        if let Some(target) = message_index
            .and_then(|idx| self.messages.get(idx))
            .and_then(|message| matching_tool_path(message, display))
        {
            return Some(crate::ui::hyperlink::HyperlinkTarget::File(target));
        }

        self.messages
            .iter()
            .find_map(|message| matching_tool_path(message, display))
            .map(crate::ui::hyperlink::HyperlinkTarget::File)
    }

    fn raw_message_index_at_content_line(
        &self,
        content_line: usize,
        content_height: usize,
    ) -> Option<usize> {
        if content_line >= content_height {
            return None;
        }

        self.message_line_positions
            .iter()
            .copied()
            .enumerate()
            .find_map(|(idx, start)| {
                let end = self
                    .message_line_positions
                    .iter()
                    .copied()
                    .skip(idx + 1)
                    .find(|&next_start| next_start > start)
                    .unwrap_or(content_height);
                (content_line >= start && content_line < end).then_some(idx)
            })
    }

    pub fn clear_highlighted_message(&mut self) {
        self.highlighted_message_index = None;
    }

    fn content_area_for(area: Rect) -> Rect {
        Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height,
        }
    }

    pub fn message_index_at_position(&self, event: MouseEvent, area: Rect) -> Option<usize> {
        use ratatui::layout::Position;

        let point = Position::new(event.column, event.row);
        let content_area = Self::content_area_for(area);

        if !content_area.contains(point) || self.message_line_positions.is_empty() {
            return None;
        }

        let content_line =
            (event.row.saturating_sub(content_area.y) as usize).saturating_add(self.scroll_offset);
        let content_height = self.content_height.max(
            self.message_line_positions
                .iter()
                .copied()
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        self.message_index_at_content_line(content_line, content_height)
    }

    fn message_index_at_content_line(
        &self,
        content_line: usize,
        content_height: usize,
    ) -> Option<usize> {
        if content_line >= content_height {
            return None;
        }

        let mut idx = 0usize;
        while idx < self.messages.len() {
            let Some(message) = self.messages.get(idx) else {
                break;
            };

            if crate::session::compaction::is_compaction_display_item(message) {
                idx = idx.saturating_add(1);
                continue;
            }

            let Some(block) =
                crate::session::types::logical_message_block_range(&self.messages, idx)
            else {
                idx = idx.saturating_add(1);
                continue;
            };

            if block.start != idx {
                idx = idx.saturating_add(1);
                continue;
            }

            let Some((start, mut end)) =
                self.message_block_line_range(idx, &self.message_line_positions, content_height)
            else {
                idx = block.end.max(idx.saturating_add(1));
                continue;
            };

            while end > start
                && self
                    .cached_lines
                    .get(end - 1)
                    .map(line_is_blank)
                    .unwrap_or(false)
            {
                end -= 1;
            }

            if content_line >= start && content_line < end {
                return Some(idx);
            }

            idx = block.end.max(idx.saturating_add(1));
        }

        None
    }

    fn message_block_line_range(
        &self,
        idx: usize,
        positions: &[usize],
        content_height: usize,
    ) -> Option<(usize, usize)> {
        let message = self.messages.get(idx)?;
        if crate::session::compaction::is_compaction_display_item(message) {
            return None;
        }

        let block = crate::session::types::logical_message_block_range(&self.messages, idx)?;
        let start = positions.get(block.start).copied()?;
        let end = positions
            .iter()
            .copied()
            .skip(block.end)
            .find(|&next_start| next_start > start)
            .unwrap_or(content_height);

        (end > start).then_some((start, end))
    }

    fn update_scrollbar(&mut self) {
        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        let content_length = max_offset.saturating_add(1).max(1);
        let position = self.scroll_offset.min(content_length.saturating_sub(1));
        self.scrollbar_state = self.scrollbar_state.content_length(content_length);
        self.scrollbar_state = self.scrollbar_state.position(position);
    }

    pub fn has_active_selection_edge_scroll(&self) -> bool {
        self.selection_edge_scroll.is_some()
    }

    pub fn tick_selection_edge_scroll(&mut self) -> bool {
        let Some(edge_scroll) = self.selection_edge_scroll else {
            return false;
        };
        if !self.selection.is_dragging {
            self.selection_edge_scroll = None;
            return false;
        }

        let before = self.scroll_offset;
        match edge_scroll.direction {
            EdgeScrollDirection::Up => self.scroll_up(1),
            EdgeScrollDirection::Down => self.scroll_down(1),
        }

        if self.scroll_offset == before {
            self.selection_edge_scroll = None;
            return false;
        }

        let line = match edge_scroll.direction {
            EdgeScrollDirection::Up => self.scroll_offset,
            EdgeScrollDirection::Down => self
                .scroll_offset
                .saturating_add(self.viewport_height.saturating_sub(1))
                .min(self.content_height.saturating_sub(1)),
        };
        self.selection.extend(line, edge_scroll.column);
        true
    }

    fn clear_selection_edge_scroll(&mut self) {
        self.selection_edge_scroll = None;
    }

    fn edge_scroll_direction(area: Rect, row: u16) -> Option<EdgeScrollDirection> {
        if area.height == 0 {
            return None;
        }
        let bottom = area.y.saturating_add(area.height.saturating_sub(1));
        if row <= area.y {
            Some(EdgeScrollDirection::Up)
        } else if row >= bottom {
            Some(EdgeScrollDirection::Down)
        } else {
            None
        }
    }

    fn clamped_content_column(content_area: Rect, column: u16) -> usize {
        if content_area.width == 0 {
            return 0;
        }
        column
            .saturating_sub(content_area.x)
            .min(content_area.width.saturating_sub(1)) as usize
    }

    fn clamped_content_row(content_area: Rect, row: u16) -> u16 {
        if content_area.height == 0 {
            return 0;
        }
        row.saturating_sub(content_area.y)
            .min(content_area.height.saturating_sub(1))
    }

    fn update_selection_edge_scroll(&mut self, content_area: Rect, event: MouseEvent) {
        if !self.selection.is_dragging || content_area.width == 0 || content_area.height == 0 {
            self.clear_selection_edge_scroll();
            return;
        }

        self.selection_edge_scroll =
            Self::edge_scroll_direction(content_area, event.row).map(|direction| {
                SelectionEdgeScroll {
                    direction,
                    column: Self::clamped_content_column(content_area, event.column),
                }
            });
    }

    fn drag_selection_to_position(&mut self, content_area: Rect, event: MouseEvent) {
        let content_line = (Self::clamped_content_row(content_area, event.row) as usize
            + self.scroll_offset)
            .min(self.content_height.saturating_sub(1));
        let content_col = Self::clamped_content_column(content_area, event.column);
        self.selection.extend(content_line, content_col);
    }

    pub fn has_selection(&self) -> bool {
        self.selection.active
    }

    pub fn get_selected_text<'a>(
        &'a self,
        max_width: usize,
        model: &'a str,
        colors: &'a ThemeColors,
    ) -> Option<String> {
        if !self.selection.active {
            return None;
        }

        let ((s_line, _), (e_line, _)) = self.selection.range();
        if s_line < self.cached_lines.len() && e_line < self.cached_lines.len() {
            return crate::ui::selection::extract_selected_text(
                &self.cached_lines,
                &self.selection,
            );
        }

        let lines =
            self.render_visible_messages_without_selection_styling(max_width, model, colors);
        crate::ui::selection::extract_selected_text(&lines, &self.selection)
    }

    /// Like render_visible_messages but without applying selection styling
    /// (used internally by get_selected_text to get clean text)
    fn render_visible_messages_without_selection_styling<'a>(
        &'a self,
        max_width: usize,
        model: &'a str,
        colors: &'a ThemeColors,
    ) -> Vec<Line<'a>> {
        self.build_all_lines(max_width, model, colors)
    }

    pub fn handle_mouse_event(&mut self, event: MouseEvent, area: Rect) -> bool {
        use ratatui::layout::Position;
        let point = Position::new(event.column, event.row);

        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };
        let content_area = Self::content_area_for(area);
        let rendered_content_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: content_area.width,
            height: content_area.height,
        };

        if self.is_dragging_scrollbar {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.scroll_to_position(event.row, scrollbar_area);
                    return true;
                }
                MouseEventKind::Up(_) => {
                    self.is_dragging_scrollbar = false;
                    self.scrollbar_drag_offset = None;
                    return true;
                }
                _ => {}
            }
        }

        if !area.contains(point) {
            self.is_dragging_scrollbar = false;
            self.scrollbar_drag_offset = None;
            // If dragging selection outside area, finalize it
            if self.selection.is_dragging {
                match event.kind {
                    MouseEventKind::Drag(MouseButton::Left) => {
                        self.drag_selection_to_position(rendered_content_area, event);
                        self.update_selection_edge_scroll(rendered_content_area, event);
                        let _ = self.tick_selection_edge_scroll();
                        return true;
                    }
                    MouseEventKind::Up(_) => {
                        self.selection.finish();
                        self.clear_selection_edge_scroll();
                        self.pending_click_anchor = None;
                        // Copy will be handled by app.rs on mouse up
                        return true;
                    }
                    _ => {}
                }
            }
            return false;
        }

        let is_on_scrollbar = scrollbar_area.contains(point);
        let is_in_content = rendered_content_area.contains(point);

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_down(1);
                true
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up(1);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if is_on_scrollbar {
                    let metrics = ScrollMetrics::new(
                        self.content_height,
                        self.viewport_height,
                        self.scroll_offset,
                    );
                    if let Some(grab_offset) =
                        scrollbar_grab_offset(metrics, scrollbar_area, event.row)
                    {
                        self.is_dragging_scrollbar = true;
                        self.scrollbar_drag_offset = Some(grab_offset);
                        self.scroll_to_position(event.row, scrollbar_area);
                        true
                    } else {
                        false
                    }
                } else if is_in_content {
                    let content_line = (event.row.saturating_sub(rendered_content_area.y) as usize)
                        .saturating_add(self.scroll_offset);
                    let content_col = event.column.saturating_sub(rendered_content_area.x) as usize;
                    self.pending_click_anchor = self.selection.anchor;

                    if event.modifiers.contains(KeyModifiers::SHIFT)
                        && self
                            .selection
                            .start_from_anchor_to(content_line, content_col)
                    {
                        self.clear_selection_edge_scroll();
                        true
                    } else {
                        // Start text selection and record this normal click as the anchor.
                        self.selection.start(content_line, content_col);
                        self.clear_selection_edge_scroll();
                        true
                    }
                } else {
                    false
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.is_dragging_scrollbar {
                    self.scroll_to_position(event.row, scrollbar_area);
                    true
                } else if is_in_content && self.selection.is_dragging {
                    // Extend text selection
                    self.drag_selection_to_position(rendered_content_area, event);
                    self.update_selection_edge_scroll(rendered_content_area, event);
                    let _ = self.tick_selection_edge_scroll();
                    true
                } else {
                    false
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.is_dragging_scrollbar {
                    self.is_dragging_scrollbar = false;
                    self.scrollbar_drag_offset = None;
                    true
                } else if self.selection.is_dragging {
                    let ((s_line, s_col), (e_line, e_col)) = self.selection.range();
                    let is_zero_width_click = s_line == e_line && s_col == e_col;

                    if event.modifiers.contains(KeyModifiers::SHIFT)
                        && self.pending_click_anchor.is_some()
                        && is_zero_width_click
                    {
                        let content_line = (event.row.saturating_sub(rendered_content_area.y)
                            as usize)
                            .saturating_add(self.scroll_offset);
                        let content_col =
                            event.column.saturating_sub(rendered_content_area.x) as usize;
                        if let Some(anchor) = self.pending_click_anchor {
                            self.selection.anchor = Some(anchor);
                            self.selection
                                .start_from_anchor_to(content_line, content_col);
                        }
                    }

                    // Finalize text selection
                    self.selection.finish();
                    self.clear_selection_edge_scroll();
                    self.pending_click_anchor = None;
                    // If selection is zero-width (click without drag), clear it
                    let ((s_line, s_col), (e_line, e_col)) = self.selection.range();
                    if s_line == e_line && s_col == e_col {
                        self.selection.clear();
                    }
                    true
                } else {
                    false
                }
            }
            MouseEventKind::Up(MouseButton::Right) => {
                // Right-click clears selection
                if self.selection.active {
                    self.selection.clear();
                    self.clear_selection_edge_scroll();
                    self.pending_click_anchor = None;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn scroll_to_position(&mut self, row: u16, scrollbar_area: Rect) {
        if self.content_height == 0 || self.viewport_height == 0 {
            return;
        }

        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        let metrics = ScrollMetrics::new(
            self.content_height,
            self.viewport_height,
            self.scroll_offset,
        );
        let grab_offset = self
            .scrollbar_drag_offset
            .or_else(|| scrollbar_grab_offset(metrics, scrollbar_area, row))
            .unwrap_or(0);
        let new_offset =
            scrollbar_offset_from_row_with_grab(metrics, scrollbar_area, row, grab_offset);
        self.scroll_offset = new_offset.min(max_offset);
        // Track if user scrolled away from bottom
        self.user_scrolled_up = self.scroll_offset < max_offset;
        self.update_scrollbar();
    }

    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        _agent: &str,
        model: &str,
        colors: &ThemeColors,
    ) {
        self.viewport_height = area.height as usize;

        self.update_streaming_renderer();

        let content_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height,
        };

        let max_width = content_area.width as usize;

        let colors_hash = Self::cache_colors_hash(colors);
        let has_active_tools = self.has_active_tool_messages();
        let animation_phase = if has_active_tools {
            Self::current_tool_marker_animation_phase()
        } else {
            false
        };
        if self.tool_marker_animation_phase != animation_phase {
            self.tool_marker_animation_phase = animation_phase;
            if has_active_tools {
                self.cached_revision = 0;
            }
        }

        let cache_valid = self.cached_revision == self.render_revision
            && self.cached_width == max_width
            && self.cached_colors_hash == colors_hash;

        if !cache_valid {
            let (message_lines, message_positions) =
                self.build_all_lines_with_positions(max_width, model, colors);
            self.cached_lines = message_lines.into_iter().map(line_to_static).collect();
            self.message_line_positions = message_positions.clone();
            self.cached_positions = message_positions;
            self.cached_revision = self.render_revision;
            self.cached_width = max_width;
            self.cached_colors_hash = colors_hash;
        }

        let all_lines = &self.cached_lines;
        let positions = &self.cached_positions;

        let content_height = all_lines.len();
        let viewport = self.viewport_height;
        let max_offset = content_height.saturating_sub(viewport);
        let was_pinned_to_bottom = self.scroll_offset == usize::MAX
            || (self.scroll_offset >= self.content_height.saturating_sub(self.viewport_height)
                && !self.user_scrolled_up);
        let clamped_scroll = if was_pinned_to_bottom {
            max_offset
        } else {
            self.scroll_offset.min(max_offset)
        };
        let visible_start = clamped_scroll.min(content_height);
        let visible_end = content_height.min(clamped_scroll.saturating_add(viewport));

        let highlight_range = self
            .highlighted_message_index
            .and_then(|hl| self.message_block_line_range(hl, positions, content_height));
        let visible_highlight_range =
            trim_trailing_blank_highlight_lines(highlight_range, all_lines);
        let highlight_bg = self
            .highlighted_message_index
            .and_then(|idx| {
                crate::session::types::logical_message_block_start(&self.messages, idx)
                    .and_then(|start| self.messages.get(start))
            })
            .map(|message| timeline_highlight_bg(message, colors))
            .unwrap_or(colors.interactive);

        let mut content_lines: Vec<Line<'static>> = all_lines[visible_start..visible_end].to_vec();
        apply_timeline_highlight_to_lines(
            &mut content_lines,
            visible_highlight_range,
            visible_start,
            highlight_bg,
        );

        let render_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: content_area.width,
            height: content_area.height,
        };

        render_line_backgrounds(
            f,
            render_area,
            all_lines,
            clamped_scroll,
            render_area.height as usize,
            colors.background_element,
        );

        // Render timeline highlight after panel backgrounds so every selected
        // message has a visible full-width band.
        if let Some((start, end)) = visible_highlight_range {
            let vis_start = start.max(clamped_scroll);
            let vis_end = end.min(clamped_scroll.saturating_add(viewport));

            if vis_end > vis_start {
                let y = content_area
                    .y
                    .saturating_add((vis_start - clamped_scroll) as u16);
                let height = (vis_end - vis_start) as u16;
                if height > 0 {
                    let hl_area = Rect {
                        x: content_area.x,
                        y,
                        width: content_area.width,
                        height,
                    };
                    let hl_block = Block::new().style(Style::default().bg(highlight_bg));
                    f.render_widget(hl_block, hl_area);
                }
            }
        }

        let content_lines = crate::ui::selection::apply_selection_to_lines_with_offset(
            content_lines,
            &self.selection,
            colors.accent,
            visible_start,
        );

        let paragraph = Paragraph::new(Text::from(content_lines));

        f.render_widget(paragraph, render_area);
        if let Some(hovered) = &self.hovered_hyperlink {
            if hovered.content_line >= visible_start && hovered.content_line < visible_end {
                crate::ui::hyperlink::mark_hyperlink_range(
                    f.buffer_mut(),
                    render_area,
                    hovered.content_line - visible_start,
                    &hovered.range,
                );
            }
        }

        self.content_height = content_height;
        self.scroll_offset = clamped_scroll;
        self.update_scrollbar();

        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };

        render_scrollbar(
            f,
            ScrollMetrics::new(content_height, viewport, clamped_scroll),
            scrollbar_area,
            colors.background_element,
            colors.text_weak,
        );
    }

    fn build_all_lines<'a>(
        &'a self,
        max_width: usize,
        model: &'a str,
        colors: &'a ThemeColors,
    ) -> Vec<Line<'a>> {
        self.build_all_lines_with_positions(max_width, model, colors)
            .0
    }

    fn build_all_lines_with_positions<'a>(
        &'a self,
        max_width: usize,
        model: &'a str,
        colors: &'a ThemeColors,
    ) -> (Vec<Line<'a>>, Vec<usize>) {
        let mut all_lines: Vec<Line<'a>> = Vec::new();
        let message_count = self.messages.len();
        let streaming_idx = self.streaming_assistant_idx();
        let streaming_content = self.streaming_renderer.as_ref().map(|r| r.get_content());
        let mut positions = Vec::with_capacity(message_count);
        let mut idx = 0usize;

        while idx < self.messages.len() {
            positions.push(all_lines.len());
            if let Some(items) = self.task_group_at(idx) {
                let group_start = all_lines.len();
                let group_len = items.len();
                all_lines.extend(self.format_task_group(&items, max_width, colors));
                all_lines.push(Line::from(""));
                positions.extend(std::iter::repeat(group_start).take(group_len.saturating_sub(1)));
                idx += group_len;
                continue;
            }

            if let Some(items) = self.exploration_group_at(idx) {
                let group_start = all_lines.len();
                let group_len = items.len();
                all_lines.extend(self.format_exploration_group(&items, max_width, colors));
                all_lines.push(Line::from(""));
                positions.extend(std::iter::repeat(group_start).take(group_len.saturating_sub(1)));
                idx += group_len;
                continue;
            }

            let message = &self.messages[idx];
            if crate::session::compaction::is_compaction_marker(message)
                || (crate::session::compaction::is_compaction_summary(message)
                    && message.compaction_stats.is_some())
            {
                all_lines.extend(format_compaction_marker(
                    message.compaction_stats,
                    max_width,
                    colors,
                ));
                all_lines.push(Line::from(""));
                idx += 1;
                continue;
            }
            if crate::session::compaction::is_compaction_summary(message) {
                idx += 1;
                continue;
            }

            let attached_to_assistant =
                idx > 0 && self.messages[idx - 1].role == MessageRole::Assistant;
            let message_lines = self.format_message(
                message,
                max_width,
                idx,
                message_count,
                streaming_content,
                streaming_idx,
                model,
                colors,
                attached_to_assistant,
            );
            all_lines.extend(message_lines);
            idx += 1;
        }

        (all_lines, positions)
    }

    fn exploration_group_at(&self, start: usize) -> Option<Vec<ExplorationToolItem>> {
        let first = exploration_tool_item_for_message(self.messages.get(start)?)?;
        let mut items = vec![first];

        for message in self.messages.iter().skip(start + 1) {
            let Some(item) = exploration_tool_item_for_message(message) else {
                break;
            };
            items.push(item);
        }

        Some(items)
    }

    fn task_group_at(&self, start: usize) -> Option<Vec<TaskToolItem>> {
        let first = task_tool_item_for_message(self.messages.get(start)?)?;
        let mut items = vec![first];

        for message in self.messages.iter().skip(start + 1) {
            let Some(item) = task_tool_item_for_message(message) else {
                break;
            };
            items.push(item);
        }

        Some(items)
    }

    fn format_task_group<'a>(
        &'a self,
        items: &[TaskToolItem],
        max_width: usize,
        colors: &'a ThemeColors,
    ) -> Vec<Line<'a>> {
        fn push_wrapped<'a>(
            out: &mut Vec<Line<'a>>,
            line: Line<'static>,
            max_width: usize,
            subsequent_indent: Line<'static>,
        ) {
            out.extend(wrap_styled_line(
                &line,
                WrapOptions::new(max_width.max(1)).subsequent_indent(subsequent_indent),
            ));
        }

        let mut out = Vec::new();
        if items.is_empty() {
            return out;
        }

        let active = items.iter().any(|item| item.active);
        let failed = items.iter().any(|item| item.failed);
        let marker = self.tool_marker(active);
        let marker_color = if failed {
            colors.error
        } else if active {
            colors.accent
        } else {
            colors.success
        };
        let marker_style = Style::default()
            .fg(marker_color)
            .add_modifier(Modifier::BOLD);
        let title_style = Style::default()
            .fg(if failed { colors.error } else { colors.text })
            .add_modifier(Modifier::BOLD);
        let hint_key_style = Style::default()
            .fg(colors.text)
            .add_modifier(Modifier::BOLD);
        let hint_style = Style::default().fg(colors.text_weak);

        let noun = if items.len() == 1 {
            "subagent"
        } else {
            "subagents"
        };
        push_wrapped(
            &mut out,
            Line::from(vec![
                Span::styled(marker.to_string(), marker_style),
                Span::raw(" "),
                Span::styled(format!("Started {} {}", items.len(), noun), title_style),
                Span::styled(" - ", hint_style),
                Span::styled("ctrl+x", hint_key_style),
                Span::raw(" "),
                Span::styled("down", hint_key_style),
                Span::raw(" "),
                Span::styled("to view subagents", hint_style),
            ]),
            max_width,
            Line::from(Span::styled("  ", hint_style)),
        );

        let gutter_style = Style::default()
            .fg(colors.text_weak)
            .add_modifier(Modifier::DIM);
        let type_style = Style::default()
            .fg(colors.text)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(colors.text_weak);
        for (idx, item) in items.iter().enumerate() {
            let item_marker = self.tool_marker(item.active);
            let item_marker_style = Style::default()
                .fg(if item.failed {
                    colors.error
                } else if item.active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled("  ".to_string(), gutter_style),
                    Span::styled(item_marker.to_string(), item_marker_style),
                    Span::raw(" "),
                    Span::styled(item.subagent_type.clone(), type_style),
                    Span::styled(" - ".to_string(), desc_style),
                    Span::styled(item.description.clone(), desc_style),
                    Span::styled(format!(" #{}", idx + 1), desc_style),
                ]),
                max_width,
                Line::from(Span::styled("    ", gutter_style)),
            );
        }

        out
    }

    fn format_exploration_group<'a>(
        &'a self,
        items: &[ExplorationToolItem],
        max_width: usize,
        colors: &'a ThemeColors,
    ) -> Vec<Line<'a>> {
        fn push_wrapped<'a>(
            out: &mut Vec<Line<'a>>,
            line: Line<'static>,
            max_width: usize,
            subsequent_indent: Line<'static>,
        ) {
            out.extend(wrap_styled_line(
                &line,
                WrapOptions::new(max_width.max(1)).subsequent_indent(subsequent_indent),
            ));
        }

        let mut out = Vec::new();
        if items.is_empty() {
            return out;
        }

        let active = items.iter().any(|item| item.active);
        let display_items = if items.iter().all(|item| item.label == "Read") {
            let mut targets: Vec<String> = Vec::new();
            for item in items {
                if !targets.iter().any(|target| target == &item.target) {
                    targets.push(item.target.clone());
                }
            }
            vec![ExplorationToolItem {
                label: "Read",
                target: targets.join(", "),
                active,
            }]
        } else {
            items.to_vec()
        };
        let marker = self.tool_marker(active);
        let heading = if active { "Exploring" } else { "Explored" };

        let marker_style = Style::default()
            .fg(if active {
                colors.accent
            } else {
                colors.success
            })
            .add_modifier(Modifier::BOLD);
        let gutter_style = Style::default()
            .fg(colors.text_weak)
            .add_modifier(Modifier::DIM);
        let title_style = Style::default()
            .fg(colors.text)
            .add_modifier(Modifier::BOLD);
        let action_style = Style::default()
            .fg(colors.accent)
            .add_modifier(Modifier::BOLD);
        let target_style = Style::default().fg(colors.text);

        out.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled(heading, title_style),
        ]));

        for (idx, item) in display_items.iter().enumerate() {
            let branch = if idx == 0 { "  └ " } else { "    " };
            let indent_width =
                UnicodeWidthStr::width(branch) + UnicodeWidthStr::width(item.label) + 1;
            let mut spans = vec![
                Span::styled(branch.to_string(), gutter_style),
                Span::styled(item.label.to_string(), action_style),
            ];
            if !item.target.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(item.target.clone(), target_style));
            }

            push_wrapped(
                &mut out,
                Line::from(spans),
                max_width,
                Line::from(Span::styled(" ".repeat(indent_width), gutter_style)),
            );
        }

        out
    }

    fn format_message<'a>(
        &'a self,
        message: &'a Message,
        max_width: usize,
        idx: usize,
        message_count: usize,
        streaming_content: Option<&'a str>,
        streaming_idx: Option<usize>,
        model: &'a str,
        colors: &'a ThemeColors,
        attached_to_assistant: bool,
    ) -> Vec<Line<'a>> {
        let mut lines: Vec<Line<'a>> = Vec::new();
        let max_width = max_width.max(1);

        let _ = message_count;

        match message.role {
            MessageRole::User => {
                if crate::session::compaction::is_compaction_display_item(message) {
                    return lines;
                }

                // User message: Box with left border colored by agent mode
                let border_color =
                    crate::theme::agent_mode_color(message.agent_mode.as_deref(), colors);
                let bg = colors.background_element;
                let border_style = non_selectable_style(Style::default().fg(border_color));
                let pad_style = non_selectable_style(Style::default().bg(bg));
                let text_style = Style::default().fg(colors.text).bg(bg);
                let image_style = |placeholder: &str| {
                    let is_hovered = self.hovered_image.as_ref().is_some_and(|target| {
                        target.message_index == idx && target.placeholder == placeholder
                    });
                    if is_hovered {
                        Style::default().fg(colors.markdown_image_text).bg(bg)
                    } else {
                        Style::default().fg(colors.markdown_image).bg(bg)
                    }
                };
                let content = message.content.clone();
                let horizontal_padding = 2usize;
                let right_padding = 2usize;
                let wrap_width = max_width
                    .saturating_sub(1 + horizontal_padding + right_padding)
                    .max(1);

                let padding_line = || {
                    let mut line = Line::from(vec![
                        Span::styled("▌", border_style),
                        Span::styled(" ".repeat(max_width.saturating_sub(1)), pad_style),
                    ]);
                    line.style = Style::default().bg(bg);
                    line
                };

                let wrapped_lines = content
                    .split('\n')
                    .flat_map(|content_line| {
                        let content_line = content_line.strip_suffix('\r').unwrap_or(content_line);
                        let styled_content = Line::from(spans_with_image_placeholders(
                            content_line,
                            text_style,
                            &image_style,
                        ));
                        wrap_styled_line(&styled_content, WrapOptions::new(wrap_width))
                    })
                    .collect::<Vec<_>>();

                lines.push(padding_line());

                for line in wrapped_lines {
                    let line_width = line.width();
                    let trailing_padding =
                        " ".repeat(max_width.saturating_sub(1 + horizontal_padding + line_width));
                    let mut spans = Vec::with_capacity(line.spans.len() + 3);
                    spans.push(Span::styled("▌", border_style));
                    spans.push(Span::styled(" ".repeat(horizontal_padding), pad_style));
                    spans.extend(line.spans);
                    spans.push(Span::styled(trailing_padding, pad_style));

                    let mut panel_line = Line::from(spans);
                    panel_line.style = Style::default().bg(bg);
                    lines.push(panel_line);
                }

                lines.push(padding_line());

                // Add empty line after user message
                lines.push(Line::from(""));
            }
            MessageRole::Assistant => {
                let has_ordered_parts = message
                    .parts
                    .iter()
                    .any(|part| matches!(part.part_type.as_str(), "tool_call" | "tool_result"));
                let is_streaming = streaming_idx == Some(idx) && !message.is_complete;

                if has_ordered_parts {
                    let result_ids = assistant_tool_result_ids(message);
                    let mut pending_exploration: Vec<ExplorationToolItem> = Vec::new();
                    let mut pending_tasks: Vec<TaskToolItem> = Vec::new();
                    let mut emitted_anything = false;

                    fn flush_pending_exploration<'a>(
                        chat: &Chat,
                        pending: &mut Vec<ExplorationToolItem>,
                        lines: &mut Vec<Line<'a>>,
                        max_width: usize,
                        colors: &ThemeColors,
                        emitted_anything: &mut bool,
                    ) {
                        if pending.is_empty() {
                            return;
                        }

                        for line in chat
                            .format_exploration_group(pending, max_width, colors)
                            .into_iter()
                            .map(line_to_static)
                        {
                            lines.push(line);
                        }
                        lines.push(Line::from(""));
                        pending.clear();
                        *emitted_anything = true;
                    }

                    fn flush_pending_tasks<'a>(
                        chat: &Chat,
                        pending: &mut Vec<TaskToolItem>,
                        lines: &mut Vec<Line<'a>>,
                        max_width: usize,
                        colors: &ThemeColors,
                        emitted_anything: &mut bool,
                    ) {
                        if pending.is_empty() {
                            return;
                        }

                        for line in chat
                            .format_task_group(pending, max_width, colors)
                            .into_iter()
                            .map(line_to_static)
                        {
                            lines.push(line);
                        }
                        lines.push(Line::from(""));
                        pending.clear();
                        *emitted_anything = true;
                    }

                    fn flush_pending_tool_groups<'a>(
                        chat: &Chat,
                        pending_exploration: &mut Vec<ExplorationToolItem>,
                        pending_tasks: &mut Vec<TaskToolItem>,
                        lines: &mut Vec<Line<'a>>,
                        max_width: usize,
                        colors: &ThemeColors,
                        emitted_anything: &mut bool,
                    ) {
                        flush_pending_exploration(
                            chat,
                            pending_exploration,
                            lines,
                            max_width,
                            colors,
                            emitted_anything,
                        );
                        flush_pending_tasks(
                            chat,
                            pending_tasks,
                            lines,
                            max_width,
                            colors,
                            emitted_anything,
                        );
                    }

                    for part in &message.parts {
                        match part.part_type.as_str() {
                            "reasoning" => {
                                let Some(reasoning) = part
                                    .text_value()
                                    .map(str::trim)
                                    .filter(|reasoning| !reasoning.is_empty())
                                else {
                                    continue;
                                };

                                flush_pending_tool_groups(
                                    self,
                                    &mut pending_exploration,
                                    &mut pending_tasks,
                                    &mut lines,
                                    max_width,
                                    colors,
                                    &mut emitted_anything,
                                );
                                emitted_anything = true;
                                let reasoning_style = Style::default()
                                    .fg(colors.text_weak)
                                    .add_modifier(Modifier::ITALIC);
                                let reasoning_prefix = if self.thinking_visible {
                                    "💭 Thinking..."
                                } else {
                                    "💭 Thinking collapsed"
                                };
                                lines.push(Line::from(vec![Span::styled(
                                    reasoning_prefix,
                                    reasoning_style,
                                )]));

                                if self.thinking_visible {
                                    let reasoning_line = Line::from(Span::styled(
                                        reasoning.to_string(),
                                        reasoning_style,
                                    ));
                                    lines.extend(wrap_styled_line(
                                        &reasoning_line,
                                        WrapOptions::new(max_width.max(1)),
                                    ));
                                }
                                lines.push(Line::from(""));
                            }
                            "text" => {
                                let Some(text) = part.text_value() else {
                                    continue;
                                };
                                let visible_text = if is_synthetic_tool_result_text(text) {
                                    ""
                                } else {
                                    text
                                };
                                if visible_text.trim().is_empty() {
                                    continue;
                                }

                                flush_pending_tool_groups(
                                    self,
                                    &mut pending_exploration,
                                    &mut pending_tasks,
                                    &mut lines,
                                    max_width,
                                    colors,
                                    &mut emitted_anything,
                                );
                                emitted_anything = true;
                                lines.extend(render_markdown(visible_text, max_width, colors));
                                lines.push(Line::from(""));
                            }
                            "tool_call" | "tool_result" => {
                                let Some(content) =
                                    assistant_tool_part_content(message, part, &result_ids)
                                else {
                                    continue;
                                };

                                let parsed = parse_tool_message(&content);
                                if let Some(item) = parsed.as_ref().and_then(exploration_tool_item)
                                {
                                    flush_pending_tasks(
                                        self,
                                        &mut pending_tasks,
                                        &mut lines,
                                        max_width,
                                        colors,
                                        &mut emitted_anything,
                                    );
                                    pending_exploration.push(item);
                                    continue;
                                }

                                if let Some(item) = parsed.as_ref().and_then(task_tool_item) {
                                    flush_pending_exploration(
                                        self,
                                        &mut pending_exploration,
                                        &mut lines,
                                        max_width,
                                        colors,
                                        &mut emitted_anything,
                                    );
                                    pending_tasks.push(item);
                                    continue;
                                }

                                flush_pending_tool_groups(
                                    self,
                                    &mut pending_exploration,
                                    &mut pending_tasks,
                                    &mut lines,
                                    max_width,
                                    colors,
                                    &mut emitted_anything,
                                );
                                emitted_anything = true;
                                let tool_message = Message::tool(content);
                                let tool_lines =
                                    self.format_tool_row(&tool_message, max_width, colors, true);
                                for line in tool_lines.into_iter().map(line_to_static) {
                                    lines.push(line);
                                }
                                lines.push(Line::from(""));
                            }
                            _ => {}
                        }
                    }
                    flush_pending_tool_groups(
                        self,
                        &mut pending_exploration,
                        &mut pending_tasks,
                        &mut lines,
                        max_width,
                        colors,
                        &mut emitted_anything,
                    );

                    if !emitted_anything {
                        if is_streaming || (message.is_complete && message.was_interrupted) {
                            let metadata =
                                self.format_metadata(message, model, colors, !is_streaming);
                            lines.push(Line::from(metadata));
                            lines.push(Line::from(""));
                        }
                        return lines;
                    }

                    let next_role = self.messages.get(idx + 1).map(|m| m.role.clone());
                    let show_metadata = is_streaming
                        || (message.is_complete
                            && (message.was_interrupted
                                || !matches!(
                                    next_role,
                                    Some(MessageRole::Tool) | Some(MessageRole::Assistant)
                                )));

                    if show_metadata {
                        let metadata = self.format_metadata(message, model, colors, !is_streaming);
                        lines.push(Line::from(metadata));
                        lines.push(Line::from(""));
                    }
                    return lines;
                }

                let visible_content = if is_synthetic_tool_result_text(&message.content) {
                    ""
                } else {
                    message.content.as_str()
                };
                let has_visible_content = !visible_content.trim().is_empty();
                let mut emitted_anything = false;

                // Display reasoning/thinking tokens if present
                if let Some(ref reasoning) = message.reasoning {
                    let reasoning_trimmed = reasoning.trim();
                    if !reasoning_trimmed.is_empty() {
                        emitted_anything = true;
                        let reasoning_style = Style::default()
                            .fg(colors.text_weak)
                            .add_modifier(Modifier::ITALIC);
                        let reasoning_prefix = if self.thinking_visible {
                            "💭 Thinking..."
                        } else {
                            "💭 Thinking collapsed"
                        };
                        lines.push(Line::from(vec![Span::styled(
                            reasoning_prefix,
                            reasoning_style,
                        )]));

                        if self.thinking_visible {
                            let reasoning_line = Line::from(Span::styled(
                                reasoning_trimmed.to_string(),
                                reasoning_style,
                            ));
                            lines.extend(wrap_styled_line(
                                &reasoning_line,
                                WrapOptions::new(max_width.max(1)),
                            ));
                        }

                        // Add separator between reasoning and content (only if there's content)
                        if has_visible_content {
                            lines.push(Line::from(""));
                        }
                    }
                }

                if has_visible_content && is_streaming {
                    // Use the streaming renderer content for markdown
                    if let Some(content) = streaming_content {
                        let markdown_lines = render_markdown(content, max_width, colors);
                        lines.extend(markdown_lines);
                    } else {
                        // Fallback to plain text if renderer not available
                        let content = message.content.clone();
                        let line = Line::from(Span::styled(
                            content,
                            Style::default().fg(colors.markdown_text),
                        ));
                        lines.extend(wrap_styled_line(&line, WrapOptions::new(max_width.max(1))));
                    }
                    emitted_anything = true;
                } else if has_visible_content {
                    // For complete messages, use tui-markdown directly
                    let markdown_lines = render_markdown(visible_content, max_width, colors);
                    lines.extend(markdown_lines);
                    emitted_anything = true;
                }

                if !emitted_anything {
                    if is_streaming || (message.is_complete && message.was_interrupted) {
                        let metadata = self.format_metadata(message, model, colors, !is_streaming);
                        lines.push(Line::from(metadata));
                        lines.push(Line::from(""));
                    }
                    return lines;
                }

                // Add empty line before metadata for spacing
                let next_role = self.messages.get(idx + 1).map(|m| m.role.clone());
                let show_metadata = is_streaming
                    || (message.is_complete
                        && (message.was_interrupted
                            || !matches!(
                                next_role,
                                Some(MessageRole::Tool) | Some(MessageRole::Assistant)
                            )));

                if show_metadata {
                    lines.push(Line::from(""));
                    let metadata = self.format_metadata(message, model, colors, !is_streaming);
                    lines.push(Line::from(metadata));
                    lines.push(Line::from(""));
                } else {
                    lines.push(Line::from(""));
                }
            }
            MessageRole::System => {
                // System messages: simple display
                let prefix = "System: ";
                let content = format!("{}{}", prefix, message.content);
                let line = Line::from(Span::styled(content, Style::default().fg(Color::Yellow)));
                lines.extend(wrap_styled_line(&line, WrapOptions::new(max_width.max(1))));
                lines.push(Line::from(""));
            }
            MessageRole::Tool => {
                lines.extend(self.format_tool_row(
                    message,
                    max_width,
                    colors,
                    attached_to_assistant,
                ));
                lines.push(Line::from(""));
            }
        }

        lines
    }

    fn format_tool_row<'a>(
        &'a self,
        message: &'a Message,
        max_width: usize,
        colors: &'a ThemeColors,
        attached: bool,
    ) -> Vec<Line<'a>> {
        let max_width = max_width.max(1);

        fn truncate_chars(mut s: String, max_len: usize) -> String {
            if s.chars().count() <= max_len {
                return s;
            }

            s = s.chars().take(max_len).collect();
            s.push('…');
            s
        }

        fn preview_value(v: &JsonValue, max_len: usize) -> String {
            let mut s = match v {
                JsonValue::String(s) => s.clone(),
                JsonValue::Number(n) => n.to_string(),
                JsonValue::Bool(b) => b.to_string(),
                JsonValue::Null => "null".to_string(),
                other => other.to_string(),
            };
            s = truncate_chars(s, max_len);
            if matches!(v, JsonValue::String(_)) {
                format!("\"{}\"", s)
            } else {
                s
            }
        }

        fn args_preview(args: &JsonValue) -> String {
            if let Some(obj) = args.as_object() {
                let mut keys: Vec<&String> = obj.keys().collect();
                keys.sort();
                let mut parts = Vec::new();
                for key in keys.into_iter().take(3) {
                    if let Some(val) = obj.get(key) {
                        parts.push(format!("{}={}", key, preview_value(val, 24)));
                    }
                }
                parts.join(" ")
            } else {
                preview_value(args, 64)
            }
        }

        fn question_values(
            args: &Option<JsonValue>,
            metadata: &Option<JsonValue>,
        ) -> Vec<JsonValue> {
            let from_metadata = metadata.as_ref().and_then(|m| m.get("questions")).cloned();
            let from_args = args.as_ref().and_then(|a| a.get("questions")).cloned();

            match from_metadata.or(from_args) {
                Some(JsonValue::Array(items)) => items,
                Some(JsonValue::Object(obj)) => vec![JsonValue::Object(obj)],
                Some(JsonValue::String(s)) => {
                    let trimmed = s.trim();
                    if trimmed.starts_with('[') || trimmed.starts_with('{') {
                        match serde_json::from_str::<JsonValue>(trimmed) {
                            Ok(JsonValue::Array(items)) => items,
                            Ok(JsonValue::Object(obj)) => vec![JsonValue::Object(obj)],
                            _ => vec![JsonValue::String(s)],
                        }
                    } else {
                        vec![JsonValue::String(s)]
                    }
                }
                _ => Vec::new(),
            }
        }

        fn answer_values(
            metadata: &Option<JsonValue>,
            output_preview: &Option<String>,
        ) -> Vec<JsonValue> {
            if let Some(JsonValue::Array(items)) = metadata.as_ref().and_then(|m| m.get("answers"))
            {
                return items.clone();
            }

            output_preview
                .as_ref()
                .and_then(|preview| serde_json::from_str::<JsonValue>(preview).ok())
                .and_then(|value| match value {
                    JsonValue::Array(items) => Some(items),
                    _ => None,
                })
                .unwrap_or_default()
        }

        fn is_generic_question_label(text: &str) -> bool {
            let text = text.trim();
            text.is_empty() || text.eq_ignore_ascii_case("question")
        }

        fn question_text(value: &JsonValue, idx: usize) -> String {
            if let Some(text) = value.as_str() {
                return text.to_string();
            }

            let Some(obj) = value.as_object() else {
                return format!("Question {}", idx + 1);
            };

            let primary = ["question", "text", "prompt"]
                .iter()
                .find_map(|key| obj.get(*key).and_then(|v| v.as_str()));
            if let Some(text) = primary.filter(|text| !is_generic_question_label(text)) {
                return text.trim().to_string();
            }

            let fallback = ["header", "title", "name"]
                .iter()
                .find_map(|key| obj.get(*key).and_then(|v| v.as_str()));
            if let Some(text) = fallback.filter(|text| !is_generic_question_label(text)) {
                return text.trim().to_string();
            }

            format!("Question {}", idx + 1)
        }

        fn format_answer(value: Option<&JsonValue>) -> String {
            match value {
                Some(JsonValue::Array(items)) => {
                    let labels: Vec<String> = items
                        .iter()
                        .filter_map(|item| {
                            item.as_str()
                                .map(|s| s.to_string())
                                .or_else(|| Some(item.to_string()))
                        })
                        .collect();
                    if labels.is_empty() {
                        "Skipped".to_string()
                    } else {
                        labels.join(", ")
                    }
                }
                Some(JsonValue::String(s)) if !s.trim().is_empty() => s.clone(),
                Some(value) if !value.is_null() => value.to_string(),
                _ => "Skipped".to_string(),
            }
        }

        fn push_wrapped<'a>(
            out: &mut Vec<Line<'a>>,
            line: Line<'static>,
            max_width: usize,
            subsequent_indent: Line<'static>,
        ) {
            out.extend(wrap_styled_line(
                &line,
                WrapOptions::new(max_width.max(1)).subsequent_indent(subsequent_indent),
            ));
        }

        fn push_preview_lines<'a>(
            out: &mut Vec<Line<'a>>,
            preview: &str,
            max_width: usize,
            style: Style,
        ) {
            let trimmed = preview.trim_matches('\n');
            if trimmed.trim().is_empty() {
                return;
            }

            let raw_lines: Vec<&str> = trimmed.lines().collect();
            let max_lines = TOOL_RESULT_MAX_SCREEN_LINES.max(1);
            let mut display_lines: Vec<String> = Vec::new();
            if raw_lines.len() <= max_lines {
                display_lines.extend(raw_lines.iter().map(|line| line.to_string()));
            } else {
                let tail_count = if max_lines >= 3 { 1 } else { 0 };
                let head_count = max_lines.saturating_sub(tail_count + 1).max(1);
                for line in raw_lines.iter().take(head_count) {
                    display_lines.push((*line).to_string());
                }
                let omitted = raw_lines.len().saturating_sub(head_count + tail_count);
                display_lines.push(format!("… +{} lines", omitted));
                if tail_count > 0 {
                    for line in raw_lines
                        .iter()
                        .skip(raw_lines.len().saturating_sub(tail_count))
                    {
                        display_lines.push((*line).to_string());
                    }
                }
            }

            for (idx, raw_line) in display_lines.into_iter().enumerate() {
                let prefix = if idx == 0 { "  └ " } else { "    " };
                let line = Line::from(Span::styled(format!("{}{}", prefix, raw_line), style));
                out.extend(wrap_styled_line(
                    &line,
                    WrapOptions::new(max_width.max(1))
                        .subsequent_indent(Line::from(Span::styled("    ", style))),
                ));
            }
        }

        fn push_prefixed_inner_lines<'a>(
            out: &mut Vec<Line<'a>>,
            mut inner: Vec<Line<'static>>,
            colors: &'a ThemeColors,
        ) {
            let gutter_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
            for (idx, line) in inner.iter_mut().enumerate() {
                let prefix = if idx == 0 { "  └ " } else { "    " };
                line.spans
                    .insert(0, Span::styled(prefix.to_string(), gutter_style));
            }
            out.extend(inner);
        }

        let _ = attached;
        let indent = "";
        let mut out: Vec<Line<'a>> = Vec::new();

        let parsed = parse_tool_message(&message.content);
        let (name, status, args, metadata, output_preview, title) =
            if let Some(info) = parsed.as_ref() {
                (
                    info.name.clone(),
                    info.status.clone(),
                    info.args.clone(),
                    info.metadata.clone(),
                    info.output_preview.clone(),
                    info.title.clone(),
                )
            } else {
                (
                    "tool".to_string(),
                    "ok".to_string(),
                    None,
                    None,
                    Some(message.content.clone()),
                    None,
                )
            };

        let tool_label = match name.as_str() {
            "glob" => "Glob",
            "read" => "Read",
            "write" => "Write",
            "edit" => "Edit",
            "bash" => "Bash",
            "list" => "List",
            "grep" => "Grep",
            "update_plan" | "todowrite" => "Updated Plan",
            "question" => "Question",
            "task" => "Task",
            "webfetch" => "Webfetch",
            "view_image" => "Viewed Image",
            "skill" => "Skill",
            other => other,
        };

        let args_obj = args.as_ref().and_then(|v| v.as_object());
        if let Some(item) = parsed.as_ref().and_then(task_tool_item) {
            return self.format_task_group(&[item], max_width, colors);
        }

        if let Some(item) = parsed.as_ref().and_then(exploration_tool_item) {
            return self.format_exploration_group(&[item], max_width, colors);
        }

        if let Some(plan_update) = plan_update_display(&name, &args, &metadata, &output_preview) {
            let active = matches!(status.as_str(), "running" | "pending");
            let marker_style = Style::default()
                .fg(if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD);
            let note_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::ITALIC);

            out.push(Line::from(vec![
                Span::styled(self.tool_marker(active), marker_style),
                Span::raw(" "),
                Span::styled("Updated Plan", title_style),
            ]));

            let inner_width = max_width.saturating_sub(4).max(1);
            let mut inner: Vec<Line<'static>> = Vec::new();
            if let Some(explanation) = plan_update.explanation {
                push_wrapped(
                    &mut inner,
                    Line::from(Span::styled(explanation, note_style)),
                    inner_width,
                    Line::from(Span::styled("", note_style)),
                );
            }

            for item in plan_update.plan {
                let (marker, item_style) = match item.status {
                    PlanStepStatus::Completed => (
                        "✔ ",
                        Style::default()
                            .fg(colors.text_weak)
                            .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
                    ),
                    PlanStepStatus::InProgress => (
                        "• ",
                        Style::default()
                            .fg(colors.accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                    PlanStepStatus::Pending => (
                        "□ ",
                        Style::default()
                            .fg(colors.text_weak)
                            .add_modifier(Modifier::DIM),
                    ),
                };
                push_wrapped(
                    &mut inner,
                    Line::from(vec![
                        Span::styled(marker.to_string(), item_style),
                        Span::styled(item.step, item_style),
                    ]),
                    inner_width,
                    Line::from(Span::styled("  ", item_style)),
                );
            }

            push_prefixed_inner_lines(&mut out, inner, colors);
        } else if name == "question" && status != "error" {
            let active = matches!(status.as_str(), "running" | "pending");
            let questions = question_values(&args, &metadata);
            let count = questions.len();
            let header_text = if matches!(status.as_str(), "running" | "pending") {
                if count == 1 {
                    "Asking 1 question...".to_string()
                } else if count > 1 {
                    format!("Asking {} questions...", count)
                } else {
                    "Asking questions...".to_string()
                }
            } else {
                "Questions".to_string()
            };
            let marker_style = Style::default()
                .fg(if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD);
            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled(self.tool_marker(active), marker_style),
                    Span::raw(" "),
                    Span::styled(header_text, title_style),
                ]),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );

            let bg = colors.background_element;
            let pad_style = Style::default().bg(bg);
            let header_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM)
                .bg(bg);
            let question_style = Style::default().fg(colors.text_weak).bg(bg);
            let answer_style = Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD)
                .bg(bg);

            let panel_width = max_width.saturating_sub(2).max(10);
            let answers = answer_values(&metadata, &output_preview);
            let mut panel_lines: Vec<Line<'_>> = Vec::new();

            panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));
            panel_lines.push(Line::from(vec![Span::styled("# Questions", header_style)]));

            if status == "running" {
                if questions.is_empty() {
                    panel_lines.push(Line::from(vec![Span::styled(
                        "Waiting for question details...",
                        question_style,
                    )]));
                } else {
                    for (idx, question) in questions.iter().enumerate() {
                        if idx > 0 {
                            panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));
                        }
                        let q_line = Line::from(vec![Span::styled(
                            question_text(question, idx),
                            question_style,
                        )]);
                        panel_lines.extend(wrap_styled_line(
                            &q_line,
                            WrapOptions::new(panel_width)
                                .subsequent_indent(Line::from(Span::styled("  ", question_style))),
                        ));
                    }
                }
            } else {
                for (idx, question) in questions.iter().enumerate() {
                    if idx > 0 {
                        panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));
                    }
                    let q_line = Line::from(vec![Span::styled(
                        question_text(question, idx),
                        question_style,
                    )]);
                    panel_lines.extend(wrap_styled_line(
                        &q_line,
                        WrapOptions::new(panel_width)
                            .subsequent_indent(Line::from(Span::styled("  ", question_style))),
                    ));

                    let answer = format_answer(answers.get(idx));
                    let a_line = Line::from(vec![
                        Span::styled("  -> ", header_style),
                        Span::styled(answer, answer_style),
                    ]);
                    panel_lines.extend(wrap_styled_line(
                        &a_line,
                        WrapOptions::new(panel_width)
                            .subsequent_indent(Line::from(Span::styled("     ", answer_style))),
                    ));
                }
            }

            panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));
            for line in &mut panel_lines {
                line.spans.insert(0, Span::styled(" ", pad_style));
                line.style = Style::default().bg(bg);
            }

            out.extend(panel_lines);
        } else if name == "view_image" {
            let active = matches!(status.as_str(), "running" | "pending");
            let path = metadata
                .as_ref()
                .and_then(|m| m.get("path"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    args_obj
                        .and_then(|o| o.get("path"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| strip_tool_title(title.as_deref(), "Viewed Image"))
                .unwrap_or("image");
            let marker_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else {
                    colors.text
                })
                .add_modifier(Modifier::BOLD);
            let gutter_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
            let path_style = Style::default().fg(colors.text_weak);
            let heading = if active {
                "Viewing Image"
            } else {
                "Viewed Image"
            };

            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled(self.tool_marker(active), marker_style),
                    Span::raw(" "),
                    Span::styled(heading.to_string(), title_style),
                ]),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );
            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled("  └ ".to_string(), gutter_style),
                    Span::styled(display_path(path, true), path_style),
                ]),
                max_width,
                Line::from(Span::styled("    ", gutter_style)),
            );
        } else if name == "webfetch" {
            let active = matches!(status.as_str(), "running" | "pending");
            let url = metadata
                .as_ref()
                .and_then(|m| m.get("url"))
                .and_then(|v| v.as_str())
                .or_else(|| args_obj.and_then(|o| o.get("url")).and_then(|v| v.as_str()))
                .or_else(|| strip_tool_title(title.as_deref(), "Fetched"))
                .unwrap_or("url");
            let marker_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else {
                    colors.text
                })
                .add_modifier(Modifier::BOLD);
            let target_style = Style::default().fg(colors.text);
            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled(self.tool_marker(active), marker_style),
                    Span::raw(" "),
                    Span::styled("Webfetch", title_style),
                    Span::raw(" "),
                    Span::styled(url.to_string(), target_style),
                ]),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );
            if status == "ok" {
                if let Some(ref preview) = output_preview {
                    let result_style = Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM);
                    push_preview_lines(&mut out, preview, max_width, result_style);
                }
            }
        } else if name == "bash" {
            let command = metadata
                .as_ref()
                .and_then(|m| m.get("command"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    args_obj
                        .and_then(|o| o.get("command"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| strip_tool_title(title.as_deref(), "Bash"))
                .unwrap_or("command");
            let active = matches!(status.as_str(), "running" | "pending");
            let verb = if active { "Running" } else { "Ran" };
            let marker_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else {
                    colors.text
                })
                .add_modifier(Modifier::BOLD);
            let command_style = Style::default().fg(colors.text);
            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled(self.tool_marker(active), marker_style),
                    Span::raw(" "),
                    Span::styled(verb.to_string(), title_style),
                    Span::raw(" "),
                    Span::styled(command.to_string(), command_style),
                ]),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );
            if status == "ok" {
                if let Some(ref preview) = output_preview {
                    let result_style = Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM);
                    push_preview_lines(&mut out, preview, max_width, result_style);
                }
            }
        } else if name == "apply_patch" && status != "error" {
            let patch = args_obj
                .and_then(|o| o.get("patch"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let preview = patch_preview_from_text(patch);
            let active = matches!(status.as_str(), "running" | "pending");
            let file_count = metadata_usize(metadata.as_ref(), &["file_count"])
                .unwrap_or_else(|| preview.paths.len());
            let description = if preview.paths.is_empty() {
                if file_count == 1 {
                    "1 file".to_string()
                } else if file_count > 1 {
                    format!("{} files", file_count)
                } else {
                    "workspace".to_string()
                }
            } else if preview.paths.len() == 1 {
                preview.paths[0].clone()
            } else {
                preview
                    .paths
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let description = if preview.paths.len() > 3 {
                format!(
                    "{} +{} more",
                    description,
                    preview.paths.len().saturating_sub(3)
                )
            } else {
                description
            };

            let marker = self.tool_marker(active);
            let marker_style = Style::default()
                .fg(if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD);
            let target_style = Style::default().fg(colors.text);
            let add_style = Style::default()
                .fg(colors.diff_add)
                .add_modifier(Modifier::BOLD);
            let remove_style = Style::default()
                .fg(colors.diff_remove)
                .add_modifier(Modifier::BOLD);
            let verb = if active {
                "Applying patch"
            } else {
                "Applied patch"
            };

            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled(marker.to_string(), marker_style),
                    Span::raw(" "),
                    Span::styled(verb.to_string(), title_style),
                    Span::raw(" "),
                    Span::styled(description, target_style),
                    Span::raw(" ("),
                    Span::styled(format!("+{}", preview.added), add_style),
                    Span::raw(" "),
                    Span::styled(format!("-{}", preview.removed), remove_style),
                    Span::raw(")"),
                ]),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );

            if preview.files.iter().any(|file| !file.diff_lines.is_empty()) {
                for (index, file) in preview.files.iter().enumerate() {
                    if file.diff_lines.is_empty() {
                        continue;
                    }
                    if preview.files.len() > 1 || index > 0 {
                        let header_style = Style::default()
                            .fg(colors.warning)
                            .add_modifier(Modifier::BOLD);
                        let rule_width = max_width.saturating_sub(file.path.chars().count() + 8);
                        out.push(Line::from(vec![
                            Span::styled("    ── ", header_style),
                            Span::styled(file.path.clone(), header_style),
                            Span::raw(" "),
                            Span::styled("─".repeat(rule_width), header_style),
                        ]));
                    }
                    out.extend(crate::ui::diff::render_unified_diff_for_path_with_indent(
                        &file.diff_lines,
                        max_width,
                        colors,
                        "    ",
                        &file.path,
                    ));
                }
            } else if let Some(ref preview_text) = output_preview {
                let result_style = Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM);
                push_preview_lines(&mut out, preview_text, max_width, result_style);
            }
        } else if matches!(name.as_str(), "edit" | "write") && status != "error" {
            let file_path = args_obj
                .and_then(|o| o.get("file_path").or_else(|| o.get("filePath")))
                .and_then(|v| v.as_str())
                .or_else(|| strip_tool_title(title.as_deref(), tool_label))
                .map(|path| display_path(path, false))
                .unwrap_or_else(|| "file".to_string());

            let (old_str, new_str) = if name == "edit" {
                args_obj
                    .map(|obj| {
                        (
                            obj.get("old_string").and_then(|v| v.as_str()).unwrap_or(""),
                            obj.get("new_string").and_then(|v| v.as_str()).unwrap_or(""),
                        )
                    })
                    .unwrap_or(("", ""))
            } else {
                (
                    "",
                    args_obj
                        .and_then(|obj| obj.get("content"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                )
            };

            let stats = crate::ui::diff::compute_diff_stats(old_str, new_str);
            let active = matches!(status.as_str(), "running" | "pending");
            let verb = if name == "edit" {
                if active {
                    "Editing"
                } else {
                    "Edited"
                }
            } else if active {
                "Writing"
            } else if output_preview
                .as_deref()
                .map(|preview| preview.starts_with("Created file"))
                .unwrap_or(false)
            {
                "Added"
            } else if output_preview
                .as_deref()
                .map(|preview| preview.starts_with("Updated file"))
                .unwrap_or(false)
            {
                "Edited"
            } else {
                "Wrote"
            };

            let marker = self.tool_marker(active);
            let marker_style = Style::default()
                .fg(if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD);
            let target_style = Style::default().fg(colors.text);
            let add_style = Style::default()
                .fg(colors.diff_add)
                .add_modifier(Modifier::BOLD);
            let remove_style = Style::default()
                .fg(colors.diff_remove)
                .add_modifier(Modifier::BOLD);

            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled(marker.to_string(), marker_style),
                    Span::raw(" "),
                    Span::styled(verb.to_string(), title_style),
                    Span::raw(" "),
                    Span::styled(file_path.clone(), target_style),
                    Span::raw(" ("),
                    Span::styled(format!("+{}", stats.added), add_style),
                    Span::raw(" "),
                    Span::styled(format!("-{}", stats.removed), remove_style),
                    Span::raw(")"),
                ]),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );

            let start_line =
                metadata_usize(metadata.as_ref(), &["line_number", "line", "start_line"])
                    .or_else(|| output_preview.as_deref().and_then(parse_line_number))
                    .unwrap_or(1);

            if !old_str.is_empty() || !new_str.is_empty() {
                let diff_lines = crate::ui::diff::format_edit_diff_for_path_with_start(
                    old_str, new_str, start_line, max_width, colors, "    ", &file_path,
                );
                out.extend(diff_lines);
            }
        } else {
            let active = matches!(status.as_str(), "running" | "pending");
            let marker_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else if active {
                    colors.accent
                } else {
                    colors.success
                })
                .add_modifier(Modifier::BOLD);
            let title_style = Style::default()
                .fg(if status == "error" {
                    colors.error
                } else {
                    colors.text
                })
                .add_modifier(Modifier::BOLD);
            let args_str = if name == "skill" {
                args_obj
                    .and_then(|o| o.get("name"))
                    .and_then(|v| v.as_str())
                    .or_else(|| strip_tool_title(title.as_deref(), "Loaded skill"))
                    .map(ToString::to_string)
                    .unwrap_or_default()
            } else {
                args.as_ref().map(args_preview).unwrap_or_default()
            };
            let mut spans = vec![
                Span::styled(self.tool_marker(active), marker_style),
                Span::raw(" "),
                Span::styled(tool_label.to_string(), title_style),
            ];
            if !args_str.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(args_str, Style::default().fg(colors.text)));
            }
            push_wrapped(
                &mut out,
                Line::from(spans),
                max_width,
                Line::from(Span::styled("  ", marker_style)),
            );

            if status == "ok" {
                if let Some(ref preview) = output_preview {
                    let result_style = Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM);
                    push_preview_lines(&mut out, preview, max_width, result_style);
                }
            }
        }

        if status == "error" {
            if let Some(preview) = output_preview {
                let first = preview.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    let line = truncate_chars(first.to_string(), max_width.saturating_sub(6));
                    out.push(Line::from(Span::styled(
                        format!("{}    {}", indent, line),
                        Style::default().fg(colors.error),
                    )));
                }
            }
        }

        out
    }

    fn format_metadata(
        &self,
        message: &Message,
        model: &str,
        colors: &ThemeColors,
        include_metrics: bool,
    ) -> Vec<Span<'_>> {
        let mut spans = Vec::new();

        // Get agent mode from previous user message or default to "Plan"
        let agent_mode = self.get_agent_mode_for_message(message);
        let agent_color = crate::theme::agent_color(&agent_mode, colors);

        // Agent icon (▣) with extra space
        spans.push(Span::styled(
            "▣  ",
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ));

        // Agent type
        spans.push(Span::styled(
            display_agent_name(&agent_mode),
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ));

        // Separator (bullet)
        spans.push(Span::styled(" • ", Style::default().fg(colors.text_weak)));

        // Model ID - use persisted model from message, fallback to current model
        let model_display = message.model.as_deref().unwrap_or(model);
        spans.push(Span::styled(
            model_display.to_string(),
            Style::default().fg(colors.text),
        ));

        // Timing + throughput metrics are shown only once the stream is done.
        if include_metrics {
            if let (Some(t0), Some(t1), Some(tn)) = (message.t0_ms, message.t1_ms, message.tn_ms) {
                let output_tokens = message.output_tokens.or(message.token_count).unwrap_or(0);

                let total_ms = tn.saturating_sub(t0);
                let ttft_ms = t1.saturating_sub(t0);
                let decode_ms = tn.saturating_sub(t1);

                let total_sec = total_ms as f64 / 1000.0;
                let ttft_sec = ttft_ms as f64 / 1000.0;

                spans.push(Span::styled(
                    format!(" • {:.1}s", total_sec),
                    Style::default().fg(colors.text_weak),
                ));
                spans.push(Span::styled(
                    format!(" • ttft {:.1}s", ttft_sec),
                    Style::default().fg(colors.text_weak),
                ));

                let tokens_per_sec = if decode_ms > 0 && output_tokens > 0 {
                    (output_tokens as f64) / (decode_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                spans.push(Span::styled(
                    format!(" • {:.0}t/s", tokens_per_sec),
                    Style::default().fg(colors.text_weak),
                ));
            } else if let (Some(token_count), Some(duration_ms)) =
                (message.token_count, message.duration_ms)
            {
                // Backward-compatible fallback: duration_ms reflects decode time.
                let duration_sec = duration_ms as f64 / 1000.0;
                spans.push(Span::styled(
                    format!(" • {:.1}s", duration_sec),
                    Style::default().fg(colors.text_weak),
                ));
                let tokens_per_sec = if duration_ms > 0 {
                    (token_count as f64) / (duration_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                spans.push(Span::styled(
                    format!(" • {:.0}t/s", tokens_per_sec),
                    Style::default().fg(colors.text_weak),
                ));
            }
        }

        if message.was_interrupted {
            spans.push(Span::styled(
                " • interrupted",
                Style::default()
                    .fg(colors.warning)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        spans
    }

    fn get_agent_mode_for_message(&self, message: &Message) -> String {
        // Find the index of the current message by comparing content and timestamp
        if let Some(current_idx) = self
            .messages
            .iter()
            .position(|m| m.content == message.content && m.timestamp == message.timestamp)
        {
            // Look backwards for the preceding user message
            for i in (0..current_idx).rev() {
                if self.messages[i].role == MessageRole::User {
                    if let Some(ref agent_mode) = self.messages[i].agent_mode {
                        return agent_mode.clone();
                    }
                }
            }
        }
        // Default to Plan if no preceding user message with agent_mode found
        "Plan".to_string()
    }
}

fn format_compaction_marker<'a>(
    stats: Option<crate::session::types::CompactionStats>,
    max_width: usize,
    colors: &'a ThemeColors,
) -> Vec<Line<'a>> {
    let detail = stats
        .map(crate::session::compaction::format_compaction_stats)
        .unwrap_or_else(|| "summary retained".to_string());

    let line = Line::from(vec![
        Span::styled("• ", Style::default().fg(colors.info)),
        Span::styled(
            "Context compacted",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({})", detail),
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
    ]);

    wrap_styled_line(&line, WrapOptions::new(max_width.max(1)))
}

fn is_synthetic_tool_result_text(content: &str) -> bool {
    content.trim_start().starts_with("[tool result:")
}

fn display_agent_name(agent: &str) -> String {
    let mut out = String::new();
    let mut word_start = true;
    for ch in agent.trim().chars() {
        if matches!(ch, '-' | '_' | ' ') {
            out.push(ch);
            word_start = true;
        } else if word_start {
            out.push(ch.to_ascii_uppercase());
            word_start = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn render_line_backgrounds(
    f: &mut Frame,
    area: Rect,
    lines: &[Line<'_>],
    scroll_offset: usize,
    viewport_height: usize,
    bg: Color,
) {
    if area.width == 0 || area.height == 0 || viewport_height == 0 {
        return;
    }

    let visible_start = scroll_offset.min(lines.len());
    let visible_end = lines
        .len()
        .min(scroll_offset.saturating_add(viewport_height));
    let mut run_start: Option<usize> = None;

    for idx in visible_start..visible_end {
        let is_panel_line = line_uses_background(&lines[idx], bg);
        match (run_start, is_panel_line) {
            (None, true) => run_start = Some(idx),
            (Some(start), false) => {
                render_background_run(f, area, scroll_offset, start, idx, bg);
                run_start = None;
            }
            _ => {}
        }
    }

    if let Some(start) = run_start {
        render_background_run(f, area, scroll_offset, start, visible_end, bg);
    }
}

fn apply_timeline_highlight_to_lines(
    lines: &mut [Line<'static>],
    highlight_range: Option<(usize, usize)>,
    visible_start: usize,
    bg: Color,
) {
    let Some((start, end)) = highlight_range else {
        return;
    };

    let highlight_style = Style::default().bg(bg);

    for (line_idx, line) in lines.iter_mut().enumerate() {
        let global_idx = visible_start + line_idx;
        if global_idx < start || global_idx >= end {
            continue;
        }

        line.style = line.style.patch(highlight_style);
        for span in line.spans.iter_mut() {
            span.style = span.style.bg(bg);
        }
    }
}

fn timeline_highlight_bg(message: &Message, colors: &ThemeColors) -> Color {
    if matches!(message.role, MessageRole::Assistant) {
        return blend_colors(colors.interactive, colors.background, 0.22)
            .unwrap_or(colors.background_element);
    }

    colors.interactive
}

fn blend_colors(foreground: Color, background: Color, alpha: f32) -> Option<Color> {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(br, bg, bb)) = (foreground, background) else {
        return None;
    };

    let alpha = alpha.clamp(0.0, 1.0);
    let mix = |front: u8, back: u8| {
        ((front as f32 * alpha) + (back as f32 * (1.0 - alpha))).round() as u8
    };

    Some(Color::Rgb(mix(fr, br), mix(fg, bg), mix(fb, bb)))
}

fn trim_trailing_blank_highlight_lines(
    highlight_range: Option<(usize, usize)>,
    lines: &[Line<'_>],
) -> Option<(usize, usize)> {
    let (start, mut end) = highlight_range?;
    while end > start && line_is_blank(&lines[end - 1]) {
        end -= 1;
    }

    (end > start).then_some((start, end))
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

fn render_background_run(
    f: &mut Frame,
    area: Rect,
    scroll_offset: usize,
    start: usize,
    end: usize,
    bg: Color,
) {
    let y_offset = start.saturating_sub(scroll_offset) as u16;
    let height = end.saturating_sub(start) as u16;
    if height == 0 {
        return;
    }

    let bg_area = Rect {
        x: area.x,
        y: area.y.saturating_add(y_offset),
        width: area.width,
        height,
    };
    f.render_widget(Block::default().style(Style::default().bg(bg)), bg_area);
}

fn line_uses_background(line: &Line<'_>, bg: Color) -> bool {
    line.style.bg == Some(bg)
}

fn spans_with_image_placeholders<F>(
    text: &str,
    text_style: Style,
    image_style: &F,
) -> Vec<Span<'static>>
where
    F: Fn(&str) -> Style,
{
    let mut spans = Vec::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[Image #") {
        if start > 0 {
            spans.push(Span::styled(remaining[..start].to_string(), text_style));
        }

        let placeholder_start = &remaining[start..];
        let Some(end_offset) = placeholder_start.find(']') else {
            spans.push(Span::styled(placeholder_start.to_string(), text_style));
            return spans;
        };
        let end = start + end_offset + 1;
        let placeholder = &remaining[start..end];

        if placeholder["[Image #".len()..placeholder.len() - 1]
            .chars()
            .all(|ch| ch.is_ascii_digit())
        {
            spans.push(Span::styled(
                placeholder.to_string(),
                image_style(placeholder),
            ));
        } else {
            spans.push(Span::styled(placeholder.to_string(), text_style));
        }

        remaining = &remaining[end..];
    }

    if !remaining.is_empty() || spans.is_empty() {
        spans.push(Span::styled(remaining.to_string(), text_style));
    }

    spans
}

fn placeholder_at_line_col(line: &Line<'_>, target_col: usize) -> Option<String> {
    let mut col = 0usize;
    for span in &line.spans {
        let text = span.content.as_ref();
        let width = UnicodeWidthStr::width(text);
        if target_col >= col && target_col < col.saturating_add(width) {
            return image_placeholder_in_text_at_display_col(text, target_col - col);
        }
        col = col.saturating_add(width);
    }
    None
}

fn image_placeholder_in_text_at_display_col(text: &str, target_col: usize) -> Option<String> {
    let mut search_from = 0usize;
    while let Some(relative_start) = text[search_from..].find("[Image #") {
        let start = search_from + relative_start;
        let placeholder_start = &text[start..];
        let Some(end_offset) = placeholder_start.find(']') else {
            return None;
        };
        let end = start + end_offset + 1;
        let placeholder = &text[start..end];
        if image_index_from_placeholder(placeholder).is_some() {
            let start_col = UnicodeWidthStr::width(&text[..start]);
            let end_col = start_col + UnicodeWidthStr::width(placeholder);
            if target_col >= start_col && target_col < end_col {
                return Some(placeholder.to_string());
            }
        }
        search_from = end;
    }
    None
}

fn image_index_from_placeholder(placeholder: &str) -> Option<usize> {
    let raw_number = placeholder.strip_prefix("[Image #")?.strip_suffix(']')?;
    let one_based = raw_number.parse::<usize>().ok()?;
    one_based.checked_sub(1)
}

fn line_to_static(line: Line<'_>) -> Line<'static> {
    Line {
        spans: line
            .spans
            .into_iter()
            .map(|span| Span {
                content: std::borrow::Cow::Owned(span.content.into_owned()),
                style: span.style,
            })
            .collect(),
        style: line.style,
        alignment: line.alignment,
    }
}

use ratatui::text::Text;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    #[test]
    fn display_agent_name_title_cases_agent_words() {
        assert_eq!(display_agent_name("build"), "Build");
        assert_eq!(display_agent_name("vlm-agent"), "Vlm-Agent");
        assert_eq!(display_agent_name("general_reviewer"), "General_Reviewer");
    }

    fn test_colors() -> ThemeColors {
        ThemeColors {
            primary: Color::Reset,
            secondary: Color::Reset,
            accent: Color::Reset,
            interactive: Color::Reset,
            background: Color::Reset,
            dialog_background: Color::Reset,
            background_element: Color::Reset,
            text: Color::Reset,
            text_weak: Color::Reset,
            text_strong: Color::Reset,
            border: Color::Reset,
            border_weak_focus: Color::Reset,
            border_focus: Color::Reset,
            border_strong_focus: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            error: Color::Reset,
            info: Color::Reset,
            markdown_text: Color::Reset,
            markdown_heading: Color::Reset,
            markdown_link: Color::Reset,
            markdown_link_text: Color::Reset,
            markdown_code: Color::Reset,
            markdown_block_quote: Color::Reset,
            markdown_emph: Color::Reset,
            markdown_strong: Color::Reset,
            markdown_horizontal_rule: Color::Reset,
            markdown_list_item: Color::Reset,
            markdown_list_enumeration: Color::Reset,
            markdown_image: Color::Reset,
            markdown_image_text: Color::Reset,
            markdown_code_block: Color::Reset,
            diff_add: Color::Reset,
            diff_add_bg: Color::Reset,
            diff_remove: Color::Reset,
            diff_remove_bg: Color::Reset,
            diff_gutter: Color::Reset,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn trimmed_line_text(line: &Line<'_>) -> String {
        line_text(line).trim_end().to_string()
    }

    fn buffer_row_text(buffer: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
        (0..width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers,
        }
    }

    fn chat_with_content_height(content_height: usize) -> Chat {
        let mut chat = Chat::new();
        chat.content_height = content_height;
        chat.viewport_height = 10;
        chat
    }

    #[test]
    fn test_chat_new() {
        let chat = Chat::new();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_default() {
        let chat = Chat::default();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_chat_with_messages() {
        let messages = vec![Message::user("hello"), Message::assistant("hi there")];
        let chat = Chat::with_messages(messages.clone());
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].content, "hello");
        assert_eq!(chat.messages[1].content, "hi there");
        assert!(chat.thinking_visible());
    }

    #[test]
    fn assistant_reasoning_can_be_collapsed() {
        let mut assistant = Message::assistant("Final answer");
        assistant.reasoning = Some("Private reasoning".to_string());
        let mut chat = Chat::with_messages(vec![assistant]);
        let colors = test_colors();

        let expanded = chat
            .build_all_lines(100, "model", &colors)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();
        assert!(expanded
            .iter()
            .any(|line| line.contains("Private reasoning")));

        chat.set_thinking_visible(false);
        let collapsed = chat
            .build_all_lines(100, "model", &colors)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(collapsed
            .iter()
            .any(|line| line.contains("Thinking collapsed")));
        assert!(!collapsed
            .iter()
            .any(|line| line.contains("Private reasoning")));
    }

    #[test]
    fn test_chat_add_message() {
        let mut chat = Chat::new();
        chat.add_message(Message::user("test"));
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "test");
    }

    #[test]
    fn test_chat_add_user_message() {
        let mut chat = Chat::new();
        chat.add_user_message("hello");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, MessageRole::User);
        assert_eq!(chat.messages[0].content, "hello");
    }

    #[test]
    fn test_chat_add_assistant_message() {
        let mut chat = Chat::new();
        chat.add_assistant_message("response");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, MessageRole::Assistant);
        assert_eq!(chat.messages[0].content, "response");
    }

    #[test]
    fn test_chat_append_to_last_assistant() {
        let mut chat = Chat::new();

        chat.append_to_last_assistant("hello");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "hello");

        chat.append_to_last_assistant(" world");
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].content, "hello world");

        chat.add_user_message("user");
        chat.append_to_last_assistant(" assistant");
        assert_eq!(chat.messages.len(), 3);
        assert_eq!(chat.messages[2].content, " assistant");
    }

    #[test]
    fn click_hit_test_maps_visible_row_to_message_index() {
        let mut chat = Chat::with_messages(vec![Message::user("hello"), Message::assistant("hi")]);
        let colors = test_colors();
        let positions = chat.get_message_line_positions(40, "model", &colors);
        chat.message_line_positions = positions;
        chat.content_height = chat.build_all_lines(40, "model", &colors).len();
        chat.viewport_height = 8;
        chat.scroll_offset = 0;

        assert_eq!(
            chat.message_index_at_position(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    1,
                    1,
                    KeyModifiers::empty()
                ),
                Rect::new(0, 0, 40, 8),
            ),
            Some(0)
        );
        assert_eq!(
            chat.message_index_at_position(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    1,
                    4,
                    KeyModifiers::empty()
                ),
                Rect::new(0, 0, 40, 8),
            ),
            Some(1)
        );
    }

    #[test]
    fn click_hit_test_maps_assistant_turn_rows_to_block_start() {
        let mut chat = Chat::with_messages(vec![
            Message::user("Prompt"),
            Message::assistant("I will check."),
            Message::tool(
                serde_json::json!({
                    "name": "bash",
                    "status": "ok",
                    "output_preview": "tests passed",
                })
                .to_string(),
            ),
            Message::assistant("Done."),
            Message::user("Next prompt"),
        ]);
        let colors = test_colors();
        let (lines, positions) = chat.build_all_lines_with_positions(80, "model", &colors);
        let content_height = lines.len();
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.message_line_positions = positions.clone();
        chat.content_height = content_height;
        chat.viewport_height = 20;
        chat.scroll_offset = 0;

        let assistant_range = chat
            .message_block_line_range(1, &positions, content_height)
            .expect("assistant block range");

        assert!(assistant_range.0 <= positions[2]);
        assert!(positions[3] < assistant_range.1);
        assert_eq!(
            chat.message_index_at_content_line(positions[2], content_height),
            Some(1)
        );
        assert_eq!(
            chat.message_index_at_content_line(positions[3], content_height),
            Some(1)
        );
        assert_eq!(
            chat.message_index_at_content_line(positions[4], content_height),
            Some(4)
        );
    }

    #[test]
    fn assistant_timeline_highlight_uses_muted_interactive_color() {
        let mut colors = test_colors();
        colors.interactive = Color::Rgb(100, 50, 200);
        colors.background = Color::Rgb(10, 10, 10);

        assert_eq!(
            timeline_highlight_bg(&Message::assistant("Answer"), &colors),
            Color::Rgb(30, 19, 52)
        );
        assert_eq!(
            timeline_highlight_bg(&Message::user("Prompt"), &colors),
            colors.interactive
        );
    }

    #[test]
    fn test_render_fingerprint_changes_for_same_length_content_mutation() {
        let mut chat = Chat::new();
        chat.add_assistant_message("abcd");
        let colors = test_colors();

        let before = chat.compute_fingerprint(80, &colors);
        chat.messages[0].content = "wxyz".to_string();
        let after = chat.compute_fingerprint(80, &colors);

        assert_ne!(before, after);
    }

    #[test]
    fn test_render_fingerprint_changes_when_theme_changes() {
        let mut chat = Chat::new();
        chat.add_assistant_message("plain markdown text");
        let mut first = test_colors();
        first.markdown_text = Color::Rgb(10, 20, 30);
        let mut second = first;
        second.markdown_text = Color::Rgb(200, 210, 220);

        let before = chat.compute_fingerprint(80, &first);
        let after = chat.compute_fingerprint(80, &second);

        assert_ne!(before, after);
    }

    #[test]
    fn test_tool_result_preview_is_bounded() {
        let chat = Chat::new();
        let output_preview = (0..40)
            .map(|idx| format!("line {}", idx))
            .collect::<Vec<_>>()
            .join("\n");
        let content = serde_json::json!({
            "name": "bash",
            "status": "ok",
            "args": { "command": "printf lots" },
            "output_preview": output_preview,
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 40, &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains('…')));
        assert!(rendered.len() <= TOOL_RESULT_MAX_SCREEN_LINES + 2);
    }

    #[test]
    fn test_webfetch_tool_renders_semantic_preview() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "webfetch",
            "status": "ok",
            "args": { "url": "https://gittydocs.carlo.tl/llms.txt" },
            "metadata": { "url": "https://gittydocs.carlo.tl/llms.txt" },
            "output_preview": "# gittydocs\n\nSimple, fast docs from your Markdown.",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 80, &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered[0],
            "⬢ Webfetch https://gittydocs.carlo.tl/llms.txt"
        );
        assert_eq!(rendered[1], "  └ # gittydocs");
        assert!(rendered
            .iter()
            .any(|line| line.contains("Simple, fast docs")));
        assert!(!rendered.iter().any(|line| line.contains("curl")));
    }

    #[test]
    fn test_active_tool_marker_uses_animation_phase() {
        let mut chat = Chat::new();
        let content = serde_json::json!({
            "name": "webfetch",
            "status": "running",
            "args": { "url": "https://example.com" },
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let first_frame = chat
            .format_tool_row(&msg, 80, &colors, false)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();
        chat.tool_marker_animation_phase = true;
        let second_frame = chat
            .format_tool_row(&msg, 80, &colors, false)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert_eq!(first_frame[0], "⬡ Webfetch https://example.com");
        assert_eq!(second_frame[0], "⬢ Webfetch https://example.com");
    }

    #[test]
    fn test_active_tool_scan_cache_recomputes_after_render_dirty() {
        let mut chat = Chat::new();
        let content = serde_json::json!({
            "name": "bash",
            "status": "running",
            "args": { "command": "printf hello" },
        })
        .to_string();

        chat.add_message(Message::tool(content));
        assert!(chat.has_active_tool_messages());

        chat.messages[0].content = serde_json::json!({
            "name": "bash",
            "status": "ok",
            "args": { "command": "printf hello" },
            "output_preview": "hello",
        })
        .to_string();
        chat.mark_render_dirty();

        assert!(!chat.has_active_tool_messages());
    }

    #[test]
    fn test_bash_tool_renders_ran_command_preview() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "bash",
            "status": "ok",
            "args": { "command": "printf hello" },
            "metadata": { "command": "printf hello", "exit_code": 0 },
            "output_preview": "hello",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 80, &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["⬢ Ran printf hello", "  └ hello"]);
    }

    #[test]
    fn test_read_tool_renders_codex_style_explored_summary() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "read",
            "status": "ok",
            "args": { "file_path": "/Users/carlo/Desktop/Projects/crabcode/AGENTS.md" },
            "output_preview": "00001| # Agent Context\n00002| More content",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 80, &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["⬢ Explored", "  └ Read AGENTS.md"]);
    }

    #[test]
    fn test_list_tool_renders_codex_style_explored_summary() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "list",
            "status": "ok",
            "args": { "path": "src/ui" },
            "output_preview": "src/ui\ncomponents\nmarkdown",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 80, &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["⬢ Explored", "  └ List src/ui"]);
    }

    #[test]
    fn test_adjacent_context_tools_render_as_one_explored_group() {
        let mut chat = Chat::new();
        chat.add_message(Message::tool(
            serde_json::json!({
                "name": "list",
                "status": "ok",
                "args": { "path": ". " },
                "output_preview": "README.md\nsrc/",
            })
            .to_string(),
        ));
        chat.add_message(Message::tool(
            serde_json::json!({
                "name": "read",
                "status": "ok",
                "args": { "file_path": "/Users/carlo/Desktop/Projects/crabcode/README.md" },
                "output_preview": "00001| # CrabCode",
            })
            .to_string(),
        ));
        chat.add_message(Message::tool(
            serde_json::json!({
                "name": "grep",
                "status": "ok",
                "args": { "pattern": "opencode|codex", "path": "references" },
                "output_preview": "references/codex",
            })
            .to_string(),
        ));
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "model", &colors);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Explored",
                "  └ List .",
                "    Read README.md",
                "    Search opencode|codex in references",
                ""
            ]
        );
    }

    #[test]
    fn test_structured_assistant_context_tools_render_as_one_explored_group() {
        let chat = Chat::new();
        let mut msg = Message::incomplete("");
        msg.add_tool_call_part(
            "call_1",
            "grep",
            serde_json::json!({ "pattern": "Explored", "path": "src" }),
        );
        msg.add_tool_call_part("call_2", "list", serde_json::json!({ "path": "." }));
        msg.add_tool_call_part(
            "call_3",
            "read",
            serde_json::json!({ "file_path": "/repo/justfile" }),
        );
        msg.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_1",
            "name": "grep",
            "status": "ok",
            "output_preview": "src/ui/components/chat.rs: Explored",
        }));
        msg.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_2",
            "name": "list",
            "status": "ok",
            "output_preview": "src/\njustfile",
        }));
        msg.add_or_update_tool_result_part(serde_json::json!({
            "id": "call_3",
            "name": "read",
            "status": "ok",
            "output_preview": "default:\n    just --list",
        }));
        let colors = test_colors();

        let lines = chat.format_message(&msg, 100, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Explored",
                "  └ Search Explored in src",
                "    List .",
                "    Read justfile",
                "",
            ]
        );
    }

    #[test]
    fn test_read_only_context_group_collapses_targets() {
        let mut chat = Chat::new();
        for file in ["README.md", "AGENTS.md"] {
            chat.add_message(Message::tool(
                serde_json::json!({
                    "name": "read",
                    "status": "ok",
                    "args": { "file_path": format!("/repo/{file}") },
                    "output_preview": "content",
                })
                .to_string(),
            ));
        }
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "model", &colors);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["⬢ Explored", "  └ Read README.md, AGENTS.md", ""]
        );
    }

    #[test]
    fn test_edit_tool_renders_codex_style_diff_summary() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "edit",
            "status": "ok",
            "args": {
                "file_path": "/Users/carlo/Desktop/Projects/crabcode/README.md",
                "old_string": "alpha\nbeta\nomega",
                "new_string": "alpha\nbravo\nomega",
            },
            "metadata": { "line_number": 3 },
            "output_preview": "Replaced at line 3",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 80, &colors, false);
        let rendered = lines.iter().map(trimmed_line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Edited README.md (+1 -1)",
                "    3  alpha",
                "    4 -beta",
                "    4 +bravo",
                "    5  omega",
            ]
        );
    }

    #[test]
    fn test_write_tool_renders_added_diff_summary() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "write",
            "status": "ok",
            "args": {
                "file_path": "src/new.rs",
                "content": "fn main() {}\n",
            },
            "output_preview": "Created file with 13 bytes",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 80, &colors, false);
        let rendered = lines.iter().map(trimmed_line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["⬢ Added src/new.rs (+1 -0)", "    1 +fn main() {}"]
        );
    }

    #[test]
    fn test_apply_patch_tool_renders_diff_summary() {
        let chat = Chat::new();
        let patch = "*** Begin Patch\n*** Update File: src/ui/components/chat.rs\n@@ -7,3 +7,3 @@\n alpha\n-beta\n+bravo\n*** End Patch\n";
        let content = serde_json::json!({
            "name": "apply_patch",
            "status": "ok",
            "args": { "patch": patch },
            "metadata": { "file_count": 1 },
            "output_preview": "Applied patch: updated 1",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 100, &colors, false);
        let rendered = lines.iter().map(trimmed_line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Applied patch src/ui/components/chat.rs (+1 -1)",
                "    7  alpha",
                "    8 -beta",
                "    8 +bravo",
            ]
        );
    }

    #[test]
    fn test_apply_patch_tool_infers_line_numbers_for_rangeless_hunk() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("hello.txt");
        std::fs::write(&file_path, "alpha\nbravo\ngamma\n").unwrap();
        let file_path = file_path.to_string_lossy().to_string();
        let chat = Chat::new();
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-beta\n+bravo\n*** End Patch\n",
            file_path
        );
        let content = serde_json::json!({
            "name": "apply_patch",
            "status": "ok",
            "args": { "patch": patch },
            "metadata": { "file_count": 1 },
            "output_preview": "Applied patch: updated 1",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 120, &colors, false);
        let rendered = lines.iter().map(trimmed_line_text).collect::<Vec<_>>();

        assert!(rendered[0].contains("hello.txt (+1 -1)"));
        assert!(rendered.iter().any(|line| line == "    2 -beta"));
        assert!(rendered.iter().any(|line| line == "    2 +bravo"));
    }

    #[test]
    fn test_apply_patch_tool_groups_multifile_diff_with_headers() {
        let chat = Chat::new();
        let patch = "*** Begin Patch\n*** Add File: tmp/apply-patch-smoke/a.txt\n+one\n+two\n*** Add File: tmp/apply-patch-smoke/b.txt\n+red\n+blue\n*** End Patch\n";
        let content = serde_json::json!({
            "name": "apply_patch",
            "status": "ok",
            "args": { "patch": patch },
            "metadata": { "file_count": 2 },
            "output_preview": "Applied patch: added 2",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_tool_row(&msg, 120, &colors, false);
        let rendered = lines.iter().map(trimmed_line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered[0],
            "⬢ Applied patch tmp/apply-patch-smoke/a.txt, tmp/apply-patch-smoke/b.txt (+4 -0)"
        );
        assert!(rendered
            .iter()
            .any(|line| line.contains("── tmp/apply-patch-smoke/a.txt")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("── tmp/apply-patch-smoke/b.txt")));
        assert!(rendered.iter().any(|line| line == "    1 +one"));
        assert!(rendered.iter().any(|line| line == "    1 +red"));
    }

    #[test]
    fn test_user_message_preserves_explicit_linebreaks() {
        let chat = Chat::new();
        let msg = Message::user("I want\n- [ ] To do this\n\nBut I dont want to do this.");
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("I want")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("- [ ] To do this")));
        assert!(rendered.iter().any(|line| line.trim().is_empty()));
        assert!(rendered
            .iter()
            .any(|line| line.contains("But I dont want to do this.")));
    }

    #[test]
    fn test_user_message_image_placeholders_use_markdown_image_color() {
        let chat = Chat::new();
        let msg = Message::user("see [Image #1] and [Image #2]");
        let mut colors = test_colors();
        colors.text = Color::White;
        colors.background_element = Color::Rgb(10, 10, 10);
        colors.markdown_image = Color::Rgb(0, 200, 255);

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let content_line = lines
            .iter()
            .find(|line| line_text(line).contains("[Image #1]"))
            .expect("rendered image placeholders");

        let image_spans = content_line
            .spans
            .iter()
            .filter(|span| span.content.starts_with("[Image #"))
            .collect::<Vec<_>>();
        assert_eq!(image_spans.len(), 2);
        assert!(image_spans
            .iter()
            .all(|span| span.style.fg == Some(colors.markdown_image)));
        assert!(image_spans
            .iter()
            .all(|span| span.style.bg == Some(colors.background_element)));
    }

    #[test]
    fn test_user_message_image_hit_test_finds_placeholder() {
        let mut msg = Message::user("see [Image #1] please");
        msg.local_image_paths = vec!["/tmp/example.png".to_string()];
        let mut chat = Chat::with_messages(vec![msg]);
        let colors = test_colors();
        let area = Rect::new(0, 0, 80, 10);
        let content_width = area.width.saturating_sub(2) as usize;
        let (lines, positions) =
            chat.build_all_lines_with_positions(content_width, "model", &colors);
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.cached_positions = positions.clone();
        chat.message_line_positions = positions;
        chat.content_height = chat.cached_lines.len();
        chat.viewport_height = area.height as usize;
        chat.scroll_offset = 0;

        let (line_idx, col) = chat
            .cached_lines
            .iter()
            .enumerate()
            .find_map(|(line_idx, line)| {
                let text = line_text(line);
                text.find("[Image #1]").map(|col| (line_idx, col as u16))
            })
            .expect("image placeholder position");

        let target = chat
            .image_at_position(
                mouse(
                    MouseEventKind::Moved,
                    col,
                    line_idx as u16,
                    KeyModifiers::empty(),
                ),
                area,
            )
            .expect("image target");

        assert_eq!(target.message_index, 0);
        assert_eq!(target.image_index, 0);
        assert_eq!(target.placeholder, "[Image #1]");
        assert_eq!(target.path, "/tmp/example.png");
    }

    #[test]
    fn test_hyperlink_hit_test_finds_file_path() {
        let mut chat = Chat::with_messages(vec![Message::assistant("open src/ui/hyperlink.rs:12")]);
        let colors = test_colors();
        let area = Rect::new(0, 0, 80, 10);
        let content_width = area.width.saturating_sub(2) as usize;
        let (lines, positions) =
            chat.build_all_lines_with_positions(content_width, "model", &colors);
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.cached_positions = positions.clone();
        chat.message_line_positions = positions;
        chat.content_height = chat.cached_lines.len();
        chat.viewport_height = area.height as usize;
        chat.scroll_offset = 0;

        let (line_idx, col) = chat
            .cached_lines
            .iter()
            .enumerate()
            .find_map(|(line_idx, line)| {
                let text = line_text(line);
                text.find("src/ui/hyperlink.rs")
                    .map(|col| (line_idx, col as u16))
            })
            .expect("path position");

        let target = chat
            .hyperlink_at_position(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    col,
                    line_idx as u16,
                    KeyModifiers::empty(),
                ),
                area,
            )
            .expect("hyperlink target");

        match target {
            crate::ui::hyperlink::HyperlinkTarget::File(path) => {
                assert!(path.ends_with("src/ui/hyperlink.rs"));
            }
            crate::ui::hyperlink::HyperlinkTarget::Url(url) => {
                panic!("expected file target, got {url}");
            }
        }
    }

    #[test]
    fn test_hyperlink_hit_test_uses_tool_metadata_for_short_path() {
        let full_path = std::env::current_dir()
            .unwrap()
            .join("fixtures/not-real/screenshot_1.png");
        let message = Message::tool(
            serde_json::json!({
                "name": "view_image",
                "status": "ok",
                "metadata": { "path": full_path.to_string_lossy().to_string() },
                "title": format!("Viewed Image: {}", full_path.display()),
            })
            .to_string(),
        );
        let mut chat = Chat::with_messages(vec![message]);
        let colors = test_colors();
        let area = Rect::new(0, 0, 80, 10);
        assert_eq!(
            tool_path_candidates(&chat.messages[0]),
            vec![full_path.clone()]
        );
        let content_width = area.width.saturating_sub(2) as usize;
        let (lines, positions) =
            chat.build_all_lines_with_positions(content_width, "model", &colors);
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.cached_positions = positions.clone();
        chat.message_line_positions = positions;
        chat.content_height = chat.cached_lines.len();
        chat.viewport_height = area.height as usize;
        chat.scroll_offset = 0;

        let (line_idx, col) = chat
            .cached_lines
            .iter()
            .enumerate()
            .find_map(|(line_idx, line)| {
                let text = line_text(line);
                text.find("screenshot_1.png")
                    .map(|col| (line_idx, col as u16))
            })
            .expect("short path position");
        assert_eq!(
            chat.raw_message_index_at_content_line(line_idx, chat.content_height),
            Some(0)
        );
        assert!(path_matches_display(&full_path, "screenshot_1.png"));

        let target = chat
            .hyperlink_at_position(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    col,
                    line_idx as u16,
                    KeyModifiers::empty(),
                ),
                area,
            )
            .expect("hyperlink target");

        match target {
            crate::ui::hyperlink::HyperlinkTarget::File(path) => assert_eq!(path, full_path),
            crate::ui::hyperlink::HyperlinkTarget::Url(url) => {
                panic!("expected file target, got {url}");
            }
        }
    }

    #[test]
    fn test_hyperlink_underline_only_renders_on_hover() {
        use ratatui::{backend::TestBackend, Terminal};

        let colors = test_colors();
        let mut chat = Chat::with_messages(vec![Message::assistant("open src/ui/hyperlink.rs")]);
        let area = Rect::new(0, 0, 80, 10);
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| chat.render(f, area, "Plan", "model", &colors))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(!(0..area.height).any(|y| {
            (0..area.width).any(|x| buffer[(x, y)].modifier.contains(Modifier::UNDERLINED))
        }));

        let (line_idx, col) = chat
            .cached_lines
            .iter()
            .enumerate()
            .find_map(|(line_idx, line)| {
                let text = line_text(line);
                text.find("src/ui/hyperlink.rs")
                    .map(|col| (line_idx, col as u16))
            })
            .expect("path position");
        let hover = chat
            .hyperlink_hover_at_position(
                mouse(
                    MouseEventKind::Moved,
                    col,
                    line_idx as u16,
                    KeyModifiers::empty(),
                ),
                area,
            )
            .expect("hyperlink hover");
        chat.set_hovered_hyperlink(Some(hover));

        terminal
            .draw(|f| chat.render(f, area, "Plan", "model", &colors))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let underlined = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].modifier.contains(Modifier::UNDERLINED))
            .count();

        assert_eq!(underlined, "src/ui/hyperlink.rs".len());
    }

    #[test]
    fn selected_text_uses_render_cached_lines_when_copy_width_differs() {
        let colors = test_colors();
        let content = "Intro line that wraps differently when copy uses the wrong width.\n\nSo the flow would be:\n```sh\ncode\n```";
        let mut chat = Chat::with_messages(vec![Message::assistant(content)]);
        let rendered_width = 42;
        let (lines, positions) =
            chat.build_all_lines_with_positions(rendered_width, "model", &colors);
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.cached_positions = positions.clone();
        chat.message_line_positions = positions;
        chat.content_height = chat.cached_lines.len();
        chat.viewport_height = 20;
        chat.scroll_offset = 0;

        let (line_idx, start_col) = chat
            .cached_lines
            .iter()
            .enumerate()
            .find_map(|(line_idx, line)| {
                let text = line_text(line);
                text.find("So the flow").map(|start| (line_idx, start))
            })
            .expect("rendered target line");

        chat.selection.active = true;
        chat.selection.start_line = line_idx;
        chat.selection.end_line = line_idx;
        chat.selection.start_col = start_col;
        chat.selection.end_col = start_col + "So the flow".len();

        assert_eq!(
            chat.get_selected_text(120, "model", &colors).as_deref(),
            Some("So the flow")
        );
    }

    #[test]
    fn selected_text_inside_fenced_code_uses_render_cached_lines_when_copy_width_differs() {
        let colors = test_colors();
        let content = r#"Before text that is intentionally long enough to wrap at the rendered width.

```sh
codex exec --skip-git-repo-check \
    "Use the imagegen skill to generate: ... Save the final image to ./assets/foo.png."
```"#;
        let mut chat = Chat::with_messages(vec![Message::assistant(content)]);
        let rendered_width = 64;
        let (lines, positions) =
            chat.build_all_lines_with_positions(rendered_width, "model", &colors);
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.cached_positions = positions.clone();
        chat.message_line_positions = positions;
        chat.content_height = chat.cached_lines.len();
        chat.viewport_height = 20;
        chat.scroll_offset = 0;

        let (line_idx, start_col) = chat
            .cached_lines
            .iter()
            .enumerate()
            .find_map(|(line_idx, line)| {
                let text = line_text(line);
                text.find("imagegen skill").map(|start| (line_idx, start))
            })
            .expect("rendered fenced-code target");

        chat.selection.active = true;
        chat.selection.start_line = line_idx;
        chat.selection.end_line = line_idx;
        chat.selection.start_col = start_col;
        chat.selection.end_col = start_col + "imagegen skill".len();

        assert_eq!(
            chat.get_selected_text(120, "model", &colors).as_deref(),
            Some("imagegen skill")
        );
    }

    #[test]
    fn selected_user_message_text_excludes_panel_gutter_and_padding() {
        let colors = test_colors();
        let mut chat =
            Chat::with_messages(vec![Message::user("control if\njust quickly bloats it.")]);
        let rendered_width = 40;
        let (lines, positions) =
            chat.build_all_lines_with_positions(rendered_width, "model", &colors);
        chat.cached_lines = lines.into_iter().map(line_to_static).collect();
        chat.cached_positions = positions.clone();
        chat.message_line_positions = positions;
        chat.content_height = chat.cached_lines.len();
        chat.viewport_height = 20;
        chat.scroll_offset = 0;

        let first_line = chat
            .cached_lines
            .iter()
            .position(|line| line_text(line).contains("control if"))
            .expect("first user text line");
        let second_line = chat
            .cached_lines
            .iter()
            .position(|line| line_text(line).contains("just quickly bloats it."))
            .expect("second user text line");
        let second_line_width =
            UnicodeWidthStr::width(line_text(&chat.cached_lines[second_line]).as_str());

        chat.selection.active = true;
        chat.selection.start_line = first_line;
        chat.selection.start_col = 0;
        chat.selection.end_line = second_line;
        chat.selection.end_col = second_line_width;

        let selected = chat
            .get_selected_text(rendered_width, "model", &colors)
            .expect("selected text");

        assert_eq!(selected, "control if\njust quickly bloats it.");
        assert!(!selected.contains('▌'));
    }

    #[test]
    fn test_compaction_marker_renders_at_compaction_point() {
        let summary = Message::user(format!(
            "{}\nsummary content that should stay hidden",
            crate::session::compaction::SUMMARY_PREFIX
        ));
        let stats = crate::session::types::CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 8,
            after_messages: 2,
        };
        let marker = crate::session::compaction::compaction_marker(stats);
        let chat = Chat::with_messages(vec![
            summary,
            Message::user("tail"),
            marker,
            Message::user("after compact"),
        ]);
        let colors = test_colors();

        let lines = chat.build_all_lines(80, "model", &colors);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(!rendered.iter().any(|line| line.contains("summary content")));
        let marker_idx = rendered
            .iter()
            .position(|line| line.contains("Context compacted"))
            .expect("rendered compaction marker");
        let tail_idx = rendered
            .iter()
            .position(|line| line.contains("tail"))
            .expect("rendered retained tail");
        let after_idx = rendered
            .iter()
            .position(|line| line.contains("after compact"))
            .expect("rendered later user message");

        assert_eq!(
            rendered.get(marker_idx),
            Some(&"• Context compacted (12.0K -> 360, saved 97%)".to_string())
        );
        assert!(tail_idx < marker_idx);
        assert!(marker_idx < after_idx);
    }

    #[test]
    fn test_question_panel_uses_bottom_margin_and_inner_padding() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "question",
            "status": "ok",
            "args": {
                "questions": [{ "question": "Question" }]
            },
            "metadata": {
                "questions": [{ "question": "Question" }],
                "answers": ["Provide columns and rows"]
            }
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered.len(), 7);
        assert_eq!(rendered[0].trim(), "⬢ Questions");
        assert!(rendered[1].trim().is_empty());
        assert_eq!(rendered[2].trim(), "# Questions");
        assert!(rendered[4].contains("Provide columns and rows"));
        assert!(rendered[5].trim().is_empty());
        assert!(rendered[6].trim().is_empty());
    }

    #[test]
    fn test_question_panel_uses_header_when_question_is_generic() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "question",
            "status": "ok",
            "args": {
                "questions": [{ "question": "Question", "header": "Location" }]
            },
            "metadata": {
                "questions": [{ "question": "Question", "header": "Location" }],
                "answers": ["Indoor"]
            }
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.trim() == "Location"));
        assert!(!rendered.iter().any(|line| line.trim() == "Question"));
    }

    #[test]
    fn test_task_tool_renders_cursor_style_subagent_summary() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "task",
            "status": "ok",
            "args": {
                "subagent_type": "general",
                "description": "Say hi",
                "prompt": "Say hi"
            },
            "metadata": {
                "subagent_type": "general",
                "child_tool_call_count": 0,
                "duration_ms": 4100
            },
            "output_preview": "Hi there!"
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(rendered
            .iter()
            .any(|line| line.contains("Started 1 subagent")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("ctrl+x down to view subagents")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("⬢ General - Say hi #1")));
        assert!(!rendered
            .iter()
            .any(|line| line.contains("prompt=\"Say hi\"")));
        assert!(!rendered.iter().any(|line| line.contains("Hi there!")));
    }

    #[test]
    fn test_adjacent_task_tools_render_as_one_subagent_group() {
        let mut chat = Chat::new();
        for (description, status) in [
            ("read", "running"),
            ("write a haiku", "ok"),
            ("write a haiku", "ok"),
        ] {
            chat.add_message(Message::tool(
                serde_json::json!({
                    "name": "task",
                    "status": status,
                    "args": {
                        "subagent_type": "explore",
                        "description": description,
                        "prompt": description
                    },
                    "metadata": {
                        "subagent_type": "explore"
                    }
                })
                .to_string(),
            ));
        }
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "model", &colors);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬡ Started 3 subagents - ctrl+x down to view subagents",
                "  ⬡ Explore - read #1",
                "  ⬢ Explore - write a haiku #2",
                "  ⬢ Explore - write a haiku #3",
                "",
            ]
        );
    }

    #[test]
    fn test_legacy_todowrite_history_renders_as_updated_plan() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "todowrite",
            "status": "ok",
            "output_preview": "[ ] Define table data\n[ ] Choose rendering file\n[ ] Implement rendering\n",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Updated Plan",
                "  └ □ Define table data",
                "    □ Choose rendering file",
                "    □ Implement rendering",
                "",
            ]
        );
    }

    #[test]
    fn test_updated_plan_renders_in_progress_distinctly() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "update_plan",
            "status": "ok",
            "output_preview": "[ ] Locate renderer\n[•] Implement highlighting\n[x] Validate\n",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Updated Plan",
                "  └ □ Locate renderer",
                "    • Implement highlighting",
                "    ✔ Validate",
                "",
            ]
        );
    }

    #[test]
    fn test_updated_plan_renders_explanation_before_steps() {
        let chat = Chat::new();
        let content = serde_json::json!({
            "name": "update_plan",
            "status": "ok",
            "metadata": {
                "explanation": "Need a short plan before editing.",
                "plan": [
                    {"step": "Locate renderer", "status": "completed"},
                    {"step": "Implement checklist", "status": "in_progress"},
                    {"step": "Validate output", "status": "pending"}
                ]
            },
            "output_preview": "Plan updated",
        })
        .to_string();
        let msg = Message::tool(content);
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "⬢ Updated Plan",
                "  └ Need a short plan before editing.",
                "    ✔ Locate renderer",
                "    • Implement checklist",
                "    □ Validate output",
                "",
            ]
        );
    }

    #[test]
    fn test_short_updated_plan_content_renders_at_top() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut colors = test_colors();
        colors.background_element = Color::Indexed(236);

        let content = serde_json::json!({
            "name": "todowrite",
            "status": "ok",
            "output_preview": "[ ] Define table data\n[ ] Choose rendering file\n[ ] Implement rendering\n",
        })
        .to_string();
        let mut chat = Chat::new();
        chat.add_message(Message::tool(content));

        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| chat.render(f, Rect::new(0, 0, 40, 8), "Plan", "model", &colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rows = (0..8)
            .map(|y| buffer_row_text(buffer, 38, y))
            .collect::<Vec<_>>();

        assert!(rows[0].contains("⬢ Updated Plan"));
        assert!(rows[1].contains("Define table data"));
        assert!(rows[3].contains("Implement rendering"));
        assert!(rows[4].trim().is_empty());
        assert!(rows[5].trim().is_empty());
        assert!(rows[6].trim().is_empty());
        assert!(rows[7].trim().is_empty());
    }

    #[test]
    fn test_short_chat_content_renders_at_top() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut colors = test_colors();
        colors.background_element = Color::Indexed(236);
        let mut chat = Chat::new();
        chat.add_user_message("hello");

        let backend = TestBackend::new(40, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| chat.render(f, Rect::new(0, 0, 40, 8), "Plan", "model", &colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rows = (0..8)
            .map(|y| buffer_row_text(buffer, 38, y))
            .collect::<Vec<_>>();

        assert!(rows[0].starts_with("▌"));
        assert!(!rows[0].contains("hello"));
        assert!(rows[1].starts_with("▌"));
        assert!(rows[1].contains("hello"));
        assert!(rows[2].starts_with("▌"));
        assert!(!rows[2].contains("hello"));
        assert!(rows[3].trim().is_empty());

        assert_eq!(buffer[(1, 0)].bg, colors.background_element);
        assert_eq!(buffer[(1, 1)].bg, colors.background_element);
        assert_eq!(buffer[(1, 2)].bg, colors.background_element);
        assert_ne!(buffer[(1, 3)].bg, colors.background_element);
    }

    #[test]
    fn test_inline_code_background_does_not_fill_full_row() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut colors = test_colors();
        colors.background_element = Color::Indexed(236);
        colors.markdown_text = Color::White;
        colors.markdown_code = Color::Green;

        let mut chat = Chat::new();
        chat.add_assistant_message("before `ThemeColors` after");

        let backend = TestBackend::new(50, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| chat.render(f, Rect::new(0, 0, 50, 8), "Plan", "model", &colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let (y, row) = (0..8)
            .map(|y| (y, buffer_row_text(buffer, 48, y)))
            .find(|(_, row)| row.contains("ThemeColors"))
            .expect("rendered inline code row");
        let before_start = row.find("before").expect("rendered leading text") as u16;
        let code_start = row.find("ThemeColors").expect("rendered inline code") as u16;
        let code_end = code_start + "ThemeColors".len() as u16;
        let after_start = row.find("after").expect("rendered trailing text") as u16;

        assert_ne!(buffer[(before_start, y)].bg, colors.background_element);
        assert_eq!(buffer[(code_start, y)].bg, colors.background_element);
        assert_eq!(buffer[(code_end - 1, y)].bg, colors.background_element);
        assert_ne!(buffer[(after_start, y)].bg, colors.background_element);
        assert_ne!(buffer[(47, y)].bg, colors.background_element);
    }

    #[test]
    fn test_synthetic_tool_result_assistant_text_is_hidden() {
        let chat = Chat::new();
        let msg = Message::assistant(
            "[tool result: todowrite] [ ] Add unit tests [tool result: todowrite] [ ] Refactor",
        );
        let colors = test_colors();

        let lines = chat.format_message(&msg, 80, 0, 1, None, None, "model", &colors, false);

        assert!(lines.is_empty());
    }

    #[test]
    fn streaming_assistant_metadata_shows_agent_model_without_metrics() {
        let mut chat = Chat::new();
        let mut user = Message::user("Prompt");
        user.agent_mode = Some("build".to_string());
        chat.add_message(user);

        let mut msg = Message::incomplete("Streaming answer.");
        msg.model = Some("glm-4.7".to_string());
        msg.t0_ms = Some(1_000);
        msg.t1_ms = Some(1_200);
        msg.tn_ms = Some(2_000);
        msg.output_tokens = Some(40);
        chat.add_message(msg);
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "fallback-model", &colors);
        let metadata = lines
            .iter()
            .map(line_text)
            .find(|line| line.contains("Build • glm-4.7"))
            .expect("streaming metadata line");

        assert!(!metadata.contains("ttft"));
        assert!(!metadata.contains("t/s"));
        assert!(!metadata.contains("1.0s"));
    }

    #[test]
    fn completed_assistant_metadata_includes_latency_metrics() {
        let mut chat = Chat::new();
        let mut user = Message::user("Prompt");
        user.agent_mode = Some("build".to_string());
        chat.add_message(user);

        let mut msg = Message::assistant("Done.");
        msg.model = Some("glm-4.7".to_string());
        msg.t0_ms = Some(1_000);
        msg.t1_ms = Some(1_200);
        msg.tn_ms = Some(2_000);
        msg.output_tokens = Some(40);
        chat.add_message(msg);
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "fallback-model", &colors);
        let metadata = lines
            .iter()
            .map(line_text)
            .find(|line| line.contains("Build • glm-4.7"))
            .expect("completed metadata line");

        assert!(metadata.contains("1.0s"));
        assert!(metadata.contains("ttft 0.2s"));
        assert!(metadata.contains("50t/s"));
    }

    #[test]
    fn interrupted_assistant_metadata_shows_status_label() {
        let mut chat = Chat::new();
        let mut msg = Message::assistant("Partial answer.");
        msg.t0_ms = Some(1_000);
        msg.t1_ms = Some(1_200);
        msg.tn_ms = Some(2_000);
        msg.output_tokens = Some(40);
        msg.mark_interrupted();
        chat.add_message(msg);
        chat.add_message(Message::tool(
            serde_json::json!({
                "id": "call_1",
                "name": "read",
                "status": "error",
                "output_preview": "Streaming cancelled by user",
            })
            .to_string(),
        ));
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "model", &colors);

        assert!(lines
            .iter()
            .map(line_text)
            .any(|line| line.contains("interrupted")));
    }

    #[test]
    fn interrupted_empty_assistant_metadata_still_shows_status_label() {
        let mut chat = Chat::new();
        let mut msg = Message::assistant("");
        msg.mark_interrupted();
        chat.add_message(msg);
        chat.add_message(Message::tool(
            serde_json::json!({
                "id": "call_1",
                "name": "read",
                "status": "error",
                "output_preview": "Streaming cancelled by user",
            })
            .to_string(),
        ));
        let colors = test_colors();

        let lines = chat.build_all_lines(100, "model", &colors);

        assert!(lines
            .iter()
            .map(line_text)
            .any(|line| line.contains("interrupted")));
    }

    #[test]
    fn test_streaming_pause_excluded_from_decode_duration() {
        use std::time::Duration;

        let mut chat = Chat::new();
        chat.add_assistant_message("");
        if let Some(last) = chat.messages.last_mut() {
            last.is_complete = false;
        }

        chat.begin_streaming_turn();
        chat.append_to_last_assistant("hello");

        std::thread::sleep(Duration::from_millis(40));
        chat.pause_streaming_tps_timer();
        std::thread::sleep(Duration::from_millis(320));
        chat.resume_streaming_tps_timer();
        std::thread::sleep(Duration::from_millis(40));

        chat.mark_streaming_end();
        chat.finalize_streaming_metrics();

        let duration_ms = chat
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
            .and_then(|m| m.duration_ms)
            .unwrap_or(0);

        assert!(duration_ms < 250, "duration was {}ms", duration_ms);
    }

    #[test]
    fn test_streaming_elapsed_timer_freezes_while_paused() {
        use std::time::Duration;

        let mut chat = Chat::new();
        chat.add_assistant_message("");
        if let Some(last) = chat.messages.last_mut() {
            last.is_complete = false;
        }

        chat.begin_streaming_turn();
        chat.append_to_last_assistant("hello");
        std::thread::sleep(Duration::from_millis(60));

        let before_pause = chat.get_streaming_elapsed_seconds().unwrap_or(0.0);
        chat.pause_streaming_tps_timer();
        std::thread::sleep(Duration::from_millis(220));
        let during_pause = chat.get_streaming_elapsed_seconds().unwrap_or(0.0);

        assert!(
            (during_pause - before_pause).abs() < 0.06,
            "timer moved during pause (before={:.3}s, during={:.3}s)",
            before_pause,
            during_pause
        );

        chat.resume_streaming_tps_timer();
        std::thread::sleep(Duration::from_millis(70));
        let after_resume = chat.get_streaming_elapsed_seconds().unwrap_or(0.0);
        assert!(
            after_resume > during_pause + 0.03,
            "timer did not resume (during={:.3}s, after={:.3}s)",
            during_pause,
            after_resume
        );
    }

    #[test]
    fn test_chat_clear() {
        let mut chat = Chat::new();
        chat.add_user_message("hello");
        chat.add_assistant_message("hi");
        assert_eq!(chat.messages.len(), 2);

        chat.clear();
        assert!(chat.messages.is_empty());
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_plain_click_records_shift_selection_anchor() {
        let mut chat = chat_with_content_height(100);
        let area = Rect::new(0, 0, 40, 10);

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                3,
                2,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                3,
                2,
                KeyModifiers::NONE,
            ),
            area,
        ));

        assert!(!chat.selection.active);
        assert!(!chat.selection.is_dragging);
        assert_eq!(chat.selection.anchor, Some((2, 3)));
    }

    #[test]
    fn test_shift_click_selects_from_last_plain_click_anchor() {
        let mut chat = chat_with_content_height(100);
        let area = Rect::new(0, 0, 40, 10);

        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                3,
                2,
                KeyModifiers::NONE,
            ),
            area,
        );
        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                3,
                2,
                KeyModifiers::NONE,
            ),
            area,
        );

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                8,
                5,
                KeyModifiers::SHIFT,
            ),
            area,
        ));
        assert!(chat.selection.active);
        assert!(chat.selection.is_dragging);
        assert_eq!(chat.selection.anchor, Some((2, 3)));
        assert_eq!(chat.selection.range(), ((2, 3), (5, 8)));

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                8,
                5,
                KeyModifiers::SHIFT,
            ),
            area,
        ));
        assert!(chat.selection.active);
        assert!(!chat.selection.is_dragging);
        assert_eq!(chat.selection.anchor, Some((2, 3)));
        assert_eq!(chat.selection.range(), ((2, 3), (5, 8)));
    }

    #[test]
    fn test_shift_click_selects_when_shift_is_only_reported_on_mouse_up() {
        let mut chat = chat_with_content_height(100);
        let area = Rect::new(0, 0, 40, 10);

        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                3,
                2,
                KeyModifiers::NONE,
            ),
            area,
        );
        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                3,
                2,
                KeyModifiers::NONE,
            ),
            area,
        );

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                8,
                5,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert_eq!(chat.pending_click_anchor, Some((2, 3)));
        assert_eq!(chat.selection.anchor, Some((5, 8)));

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                8,
                5,
                KeyModifiers::SHIFT,
            ),
            area,
        ));
        assert!(chat.selection.active);
        assert!(!chat.selection.is_dragging);
        assert_eq!(chat.selection.anchor, Some((2, 3)));
        assert_eq!(chat.selection.range(), ((2, 3), (5, 8)));
    }

    #[test]
    fn test_shift_click_keeps_original_anchor_for_repeated_ranges() {
        let mut chat = chat_with_content_height(100);
        let area = Rect::new(0, 0, 40, 10);

        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                10,
                6,
                KeyModifiers::NONE,
            ),
            area,
        );
        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                10,
                6,
                KeyModifiers::NONE,
            ),
            area,
        );

        chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                2,
                4,
                KeyModifiers::SHIFT,
            ),
            area,
        );

        assert_eq!(chat.selection.anchor, Some((6, 10)));
        assert_eq!(chat.selection.range(), ((4, 2), (6, 10)));
    }

    #[test]
    fn test_chat_scroll_down() {
        let mut chat = Chat::new();
        chat.content_height = 100;
        chat.viewport_height = 20;
        chat.scroll_down(5);
        assert_eq!(chat.scroll_offset, 5);

        chat.scroll_down(3);
        assert_eq!(chat.scroll_offset, 8);
    }

    #[test]
    fn test_chat_scroll_up() {
        let mut chat = Chat::new();
        chat.scroll_offset = 10;
        chat.scroll_up(3);
        assert_eq!(chat.scroll_offset, 7);

        chat.scroll_up(10);
        assert_eq!(chat.scroll_offset, 0);
    }

    #[test]
    fn test_mouse_drag_at_bottom_edge_scrolls_chat_selection() {
        let mut chat = chat_with_content_height(20);
        chat.viewport_height = 5;
        let area = Rect::new(0, 0, 40, 5);

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                2,
                2,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                2,
                4,
                KeyModifiers::NONE,
            ),
            area,
        ));

        assert_eq!(chat.scroll_offset, 1);
        assert!(chat.has_active_selection_edge_scroll());
        assert_eq!(chat.selection.range(), ((2, 2), (5, 2)));

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                2,
                4,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert!(!chat.has_active_selection_edge_scroll());
    }

    #[test]
    fn test_chat_scroll_to_bottom() {
        let mut chat = Chat::new();
        chat.content_height = 100;
        chat.viewport_height = 20;
        chat.scroll_offset = 10;
        chat.scroll_to_bottom();
        assert_eq!(chat.scroll_offset, 80);
    }

    #[test]
    fn test_chat_scrollbar_drag_continues_outside_area() {
        let mut chat = chat_with_content_height(100);
        let area = Rect::new(0, 0, 40, 10);

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                39,
                0,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert!(chat.is_dragging_scrollbar);

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                80,
                9,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert_eq!(chat.scroll_offset, 90);
        assert!(chat.is_dragging_scrollbar);

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                80,
                9,
                KeyModifiers::NONE,
            ),
            area,
        ));
        assert!(!chat.is_dragging_scrollbar);
        assert_eq!(chat.scrollbar_drag_offset, None);
    }

    #[test]
    fn test_chat_scrollbar_thumb_click_preserves_grab_point() {
        let mut chat = chat_with_content_height(30);
        chat.scroll_offset = 6;
        let area = Rect::new(0, 0, 40, 10);

        assert!(chat.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                39,
                4,
                KeyModifiers::NONE,
            ),
            area,
        ));

        assert_eq!(chat.scroll_offset, 6);
        assert_eq!(chat.scrollbar_drag_offset, Some(2));
    }

    #[test]
    fn test_chat_scroll_to_bottom_after_add() {
        let mut chat = Chat::new();
        chat.viewport_height = 20;
        chat.content_height = 100;
        // When already at bottom, adding a message should autoscroll
        chat.scroll_to_bottom();
        chat.add_user_message("test");
        // scroll_offset should be MAX (will be clamped to actual bottom on render)
        assert_eq!(chat.scroll_offset, usize::MAX);
        assert!(!chat.user_scrolled_up);
    }

    #[test]
    fn test_chat_no_autoscroll_when_scrolled_up() {
        let mut chat = Chat::new();
        chat.viewport_height = 20;
        chat.content_height = 100;
        // Scroll up (not at bottom) - this sets user_scrolled_up = true
        chat.scroll_up(10);
        let offset_before = chat.scroll_offset;
        chat.add_user_message("test");
        // Should NOT scroll to bottom - should stay at offset
        assert_eq!(chat.scroll_offset, offset_before);
        assert!(chat.user_scrolled_up);
    }

    #[test]
    fn test_chat_autoscroll_when_not_scrolled_up() {
        let mut chat = Chat::new();
        chat.viewport_height = 20;
        chat.content_height = 100;
        // At bottom, user_scrolled_up should be false
        chat.scroll_to_bottom();
        assert!(!chat.user_scrolled_up);
        chat.add_user_message("test");
        // Should autoscroll (scroll_offset set to MAX)
        assert_eq!(chat.scroll_offset, usize::MAX);
        assert!(!chat.user_scrolled_up);
    }

    #[test]
    fn test_chat_multiple_messages() {
        let mut chat = Chat::new();
        chat.add_user_message("hello");
        chat.add_assistant_message("hi");
        chat.add_user_message("how are you?");

        assert_eq!(chat.messages.len(), 3);
        assert_eq!(chat.messages[0].content, "hello");
        assert_eq!(chat.messages[1].content, "hi");
        assert_eq!(chat.messages[2].content, "how are you?");
    }

    #[test]
    fn test_chat_clone() {
        let mut chat1 = Chat::new();
        chat1.add_user_message("test");

        let chat2 = chat1.clone();
        assert_eq!(chat1.messages.len(), chat2.messages.len());
        assert_eq!(chat1.messages[0].content, chat2.messages[0].content);
    }
}
