use crate::session::types::{Message, MessageRole};
use crate::theme::{contrast_text, ThemeColors};
use crate::ui::markdown::streaming::{render_markdown, SimpleStreamingRenderer};
use crate::ui::scrollbar::{
    render_scrollbar, scrollbar_grab_offset, scrollbar_offset_from_row_with_grab, ScrollMetrics,
};
use crate::ui::selection::Selection;
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
    /// Starting line positions for each message in the rendered content
    pub message_line_positions: Vec<usize>,
    /// Text selection state for copy-on-select
    pub selection: Selection,
    /// Anchor that existed before the current mouse click started.
    pending_click_anchor: Option<(usize, usize)>,
    /// Index of the message highlighted by timeline navigation (None = no highlight)
    pub highlighted_message_index: Option<usize>,
    /// Render cache — fingerprints content to skip expensive re-formatting
    cached_lines: Vec<Line<'static>>,
    cached_positions: Vec<usize>,
    cached_fingerprint: u64,
}

// Minimum elapsed time before showing tokens/s (250ms)
const MIN_TOKENS_PER_SECOND_ELAPSED_MS: u128 = 250;
const TOOL_RESULT_MAX_SCREEN_LINES: usize = 8;

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
            message_line_positions: Vec::new(),
            selection: Selection::new(),
            pending_click_anchor: None,
            highlighted_message_index: None,
            cached_lines: Vec::new(),
            cached_positions: Vec::new(),
            cached_fingerprint: 0,
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
            message_line_positions: Vec::new(),
            selection: Selection::new(),
            pending_click_anchor: None,
            highlighted_message_index: None,
            cached_lines: Vec::new(),
            cached_positions: Vec::new(),
            cached_fingerprint: 0,
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
        self.cached_lines.clear();
        self.cached_fingerprint = 0;
    }

    fn invalidate_cache(&mut self) {
        self.cached_fingerprint = 0;
    }

    fn compute_fingerprint(&self, max_width: usize, colors: &ThemeColors) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // Bump this whenever rendering logic changes (tables, markdown, etc.)
        const RENDER_VERSION: u64 = 4;
        RENDER_VERSION.hash(&mut h);
        colors.hash(&mut h);
        self.messages.len().hash(&mut h);
        for msg in &self.messages {
            std::mem::discriminant(&msg.role).hash(&mut h);
            msg.content.hash(&mut h);
            msg.reasoning.hash(&mut h);
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

    pub fn clear_highlighted_message(&mut self) {
        self.highlighted_message_index = None;
    }

    fn update_scrollbar(&mut self) {
        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        let content_length = max_offset.saturating_add(1).max(1);
        let position = self.scroll_offset.min(content_length.saturating_sub(1));
        self.scrollbar_state = self.scrollbar_state.content_length(content_length);
        self.scrollbar_state = self.scrollbar_state.position(position);
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
                self.selection.finish();
                self.pending_click_anchor = None;
                // Copy will be handled by app.rs on mouse up
            }
            return false;
        }

        // Calculate the content area (exclude scrollbar column)
        let content_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height,
        };
        let rendered_content_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: content_area.width,
            height: content_area.height,
        };

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
                        true
                    } else {
                        // Start text selection and record this normal click as the anchor.
                        self.selection.start(content_line, content_col);
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
                    let content_line = (event.row.saturating_sub(rendered_content_area.y) as usize)
                        .saturating_add(self.scroll_offset);
                    let content_col = event.column.saturating_sub(rendered_content_area.x) as usize;
                    self.selection.extend(content_line, content_col);
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

        let fingerprint = self.compute_fingerprint(max_width, colors);
        let cache_valid = !self.cached_lines.is_empty() && fingerprint == self.cached_fingerprint;

        let positions: Vec<usize>;
        let mut all_lines: Vec<Line<'static>>;

        if cache_valid {
            positions = self.cached_positions.clone();
            all_lines = self.cached_lines.clone();
        } else {
            let (message_lines, message_positions) =
                self.build_all_lines_with_positions(max_width, model, colors);
            positions = message_positions;
            all_lines = message_lines.into_iter().map(line_to_static).collect();

            self.cached_lines = all_lines.clone();
            self.cached_positions = positions.clone();
            self.cached_fingerprint = fingerprint;
        }

        let content_height = all_lines.len();

        let viewport = self.viewport_height;
        let max_offset = content_height.saturating_sub(viewport);
        let clamped_scroll = self.scroll_offset.min(max_offset);
        let render_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: content_area.width,
            height: content_area.height,
        };

        // Render timeline highlight as a full-width background overlay
        if let Some(hl) = self.highlighted_message_index {
            if hl < positions.len() {
                let start = positions[hl];
                let end = if hl + 1 < positions.len() {
                    positions[hl + 1]
                } else {
                    content_height
                };

                if end > start {
                    let hl_color = colors.interactive;
                    let hl_fg = contrast_text(hl_color);

                    for line in all_lines.iter_mut().take(end).skip(start) {
                        for span in line.spans.iter_mut() {
                            span.style = span.style.fg(hl_fg);
                        }
                    }

                    let vis_start = start.max(clamped_scroll);
                    let vis_end = end.min(clamped_scroll.saturating_add(viewport));

                    if vis_end > vis_start {
                        let y = content_area
                            .y
                            .saturating_add((vis_start - clamped_scroll) as u16);
                        let height = (vis_end - vis_start).saturating_sub(1) as u16;
                        if height > 0 {
                            let hl_area = Rect {
                                x: content_area.x,
                                y,
                                width: content_area.width,
                                height,
                            };
                            let hl_block = Block::new().style(Style::default().bg(hl_color));
                            f.render_widget(hl_block, hl_area);
                        }
                    }
                }
            }
        }

        render_line_backgrounds(
            f,
            render_area,
            &all_lines,
            clamped_scroll,
            render_area.height as usize,
            colors.background_element,
        );

        let content_lines = crate::ui::selection::apply_selection_to_lines(
            all_lines,
            &self.selection,
            colors.accent,
        );

        let paragraph =
            Paragraph::new(Text::from(content_lines)).scroll((clamped_scroll as u16, 0));

        f.render_widget(paragraph, render_area);

        self.content_height = content_height;
        self.message_line_positions = positions;
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
        let marker = if active { "~" } else { "•" };
        let heading = if active { "Exploring" } else { "Explored" };

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
            Span::styled(marker, gutter_style),
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
                // User message: Box with left border colored by agent mode
                let border_color =
                    crate::theme::agent_mode_color(message.agent_mode.as_deref(), colors);
                let bg = colors.background_element;
                let border_style = Style::default().fg(border_color);
                let pad_style = Style::default().bg(bg);
                let text_style = Style::default().fg(colors.text).bg(bg);
                let content = message.content.clone();
                let horizontal_padding = 2usize;
                let right_padding = 2usize;
                let wrap_width = max_width
                    .saturating_sub(1 + horizontal_padding + right_padding)
                    .max(1);

                let padding_line = || {
                    Line::from(vec![
                        Span::styled("▌", border_style),
                        Span::styled(" ".repeat(max_width.saturating_sub(1)), pad_style),
                    ])
                };

                // Wrap content to fit within max_width - padding
                let wrapped_lines = textwrap::wrap(&content, wrap_width);

                lines.push(padding_line());

                for line in wrapped_lines.iter() {
                    let line_width = UnicodeWidthStr::width(line.as_ref());
                    let trailing_padding =
                        " ".repeat(max_width.saturating_sub(1 + horizontal_padding + line_width));

                    lines.push(Line::from(vec![
                        Span::styled("▌", border_style),
                        Span::styled(" ".repeat(horizontal_padding), pad_style),
                        Span::styled(line.to_string(), text_style),
                        Span::styled(trailing_padding, pad_style),
                    ]));
                }

                lines.push(padding_line());

                // Add empty line after user message
                lines.push(Line::from(""));
            }
            MessageRole::Assistant => {
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
                        let reasoning_prefix = "💭 Thinking...";
                        lines.push(Line::from(vec![Span::styled(
                            reasoning_prefix,
                            Style::default()
                                .fg(colors.text_weak)
                                .add_modifier(Modifier::ITALIC),
                        )]));

                        let reasoning_style = Style::default()
                            .fg(colors.text_weak)
                            .add_modifier(Modifier::ITALIC);
                        let reasoning_line = Line::from(Span::styled(
                            reasoning_trimmed.to_string(),
                            reasoning_style,
                        ));
                        lines.extend(wrap_styled_line(
                            &reasoning_line,
                            WrapOptions::new(max_width.max(1)),
                        ));

                        // Add separator between reasoning and content (only if there's content)
                        if has_visible_content {
                            lines.push(Line::from(""));
                        }
                    }
                }

                let is_streaming = streaming_idx == Some(idx) && !message.is_complete;

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
                    return lines;
                }

                // Add empty line before metadata for spacing
                let next_role = self.messages.get(idx + 1).map(|m| m.role.clone());
                let show_metadata = message.is_complete
                    && !matches!(
                        next_role,
                        Some(MessageRole::Tool) | Some(MessageRole::Assistant)
                    );

                if show_metadata {
                    lines.push(Line::from(""));
                    let metadata = self.format_metadata(message, model, colors);
                    lines.push(Line::from(metadata));
                    lines.push(Line::from(""));
                } else {
                    // Keep spacing consistent between segments, but skip the
                    // blank line when the next message is a compact tool panel.
                    let next_is_compact_tool_panel = self
                        .messages
                        .get(idx + 1)
                        .map(|m| m.role == MessageRole::Tool && is_compact_tool_panel(&m.content))
                        .unwrap_or(false);
                    if !next_is_compact_tool_panel {
                        lines.push(Line::from(""));
                    }
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
                // Panel-style tools already own their vertical spacing.
                if !is_compact_tool_panel(&message.content) {
                    lines.push(Line::from(""));
                }
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

        fn titlecase_ascii(value: &str) -> String {
            let mut chars = value.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            first.to_ascii_uppercase().to_string() + chars.as_str()
        }

        fn format_duration_ms(ms: u64) -> String {
            if ms >= 1000 {
                format!("{:.1}s", ms as f64 / 1000.0)
            } else {
                format!("{}ms", ms)
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

        fn push_limited_wrapped<'a>(
            out: &mut Vec<Line<'a>>,
            line: Line<'static>,
            max_width: usize,
            subsequent_indent: Line<'static>,
            max_lines: usize,
            style: Style,
        ) {
            let wrapped = wrap_styled_line(
                &line,
                WrapOptions::new(max_width.max(1)).subsequent_indent(subsequent_indent),
            );
            if wrapped.len() <= max_lines {
                out.extend(wrapped);
                return;
            }

            let omitted = wrapped.len().saturating_sub(max_lines.saturating_sub(1));
            out.extend(wrapped.into_iter().take(max_lines.saturating_sub(1)));
            out.push(Line::from(Span::styled(
                format!("  … +{} lines", omitted),
                style,
            )));
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

        let icon = match status.as_str() {
            "running" => "~",
            "ok" => "✓",
            "error" => "✗",
            _ => "•",
        };

        let tool_label = match name.as_str() {
            "glob" => "Glob",
            "read" => "Read",
            "write" => "Write",
            "edit" => "Edit",
            "bash" => "Bash",
            "list" => "List",
            "grep" => "Grep",
            "todowrite" => "Todos",
            "question" => "Questions",
            "task" => "Task",
            other => other,
        };

        let args_obj = args.as_ref().and_then(|v| v.as_object());
        if let Some(item) = parsed.as_ref().and_then(exploration_tool_item) {
            return self.format_exploration_group(&[item], max_width, colors);
        }

        let args_str = if name == "glob" {
            let pat = args_obj
                .and_then(|o| o.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let base = args_obj
                .and_then(|o| o.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut s = String::new();
            if !pat.is_empty() {
                s.push_str(&format!("\"{}\"", pat));
            }
            if !base.is_empty() && base != "." {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(&format!("in \"{}\"", base));
            }
            s
        } else if name == "edit" {
            // For edits, show only the file path in the header; the diff is rendered below.
            args_obj
                .and_then(|o| o.get("file_path"))
                .and_then(|v| v.as_str())
                .map(|p| format!("\"{}\"", p))
                .unwrap_or_default()
        } else if name == "todowrite" {
            String::new()
        } else {
            args.as_ref().map(args_preview).unwrap_or_default()
        };

        let mut header = format!("{}{} {}", indent, icon, tool_label);
        if !args_str.is_empty() {
            header.push(' ');
            header.push_str(&args_str);
        }

        if name == "glob" {
            if let Some(mc) = metadata
                .as_ref()
                .and_then(|m| m.get("match_count"))
                .and_then(|v| v.as_i64())
            {
                header.push_str(&format!(" ({} matches)", mc));
            }
        }

        // Panel-style tools render header and body inside one solid background
        // and skip the normal dim header path.
        if name == "question" && status != "error" {
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
            let questions = question_values(&args, &metadata);
            let answers = answer_values(&metadata, &output_preview);
            let mut panel_lines: Vec<Line<'_>> = Vec::new();

            panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));
            panel_lines.push(Line::from(vec![Span::styled("# Questions", header_style)]));

            if status == "running" {
                let count = questions.len();
                let text = if count == 1 {
                    "Asking 1 question...".to_string()
                } else if count > 1 {
                    format!("Asking {} questions...", count)
                } else {
                    "Asking questions...".to_string()
                };
                panel_lines.push(Line::from(vec![Span::styled(text, question_style)]));
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
            }

            out.extend(panel_lines);
        } else if name == "task" {
            let subagent_type = args_obj
                .and_then(|o| o.get("subagent_type"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    metadata
                        .as_ref()
                        .and_then(|m| m.get("subagent_type"))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("general");
            let description = args_obj
                .and_then(|o| o.get("description"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Task");
            let header_text = format!(
                "{} Task — {}",
                titlecase_ascii(subagent_type),
                description.trim()
            );

            let count = metadata
                .as_ref()
                .and_then(|m| m.get("child_tool_call_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let plural = if count == 1 { "toolcall" } else { "toolcalls" };
            let duration = metadata
                .as_ref()
                .and_then(|m| m.get("duration_ms"))
                .and_then(|v| v.as_u64())
                .map(format_duration_ms);
            let stats = match status.as_str() {
                "running" => "running".to_string(),
                "error" => "failed".to_string(),
                _ => {
                    let base = format!("{} {}", count, plural);
                    duration
                        .map(|d| format!("{} · {}", base, d))
                        .unwrap_or(base)
                }
            };

            let connector_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
            let header_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
            let stats_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
            let hint_key_style = Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD);
            let hint_style = Style::default().fg(colors.text_weak);

            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled("  ┌ ", connector_style),
                    Span::styled(header_text, header_style),
                ]),
                max_width,
                Line::from(Span::styled("    ", header_style)),
            );
            push_wrapped(
                &mut out,
                Line::from(vec![
                    Span::styled("  │ ", connector_style),
                    Span::styled(stats, stats_style),
                ]),
                max_width,
                Line::from(Span::styled("    ", stats_style)),
            );

            out.push(Line::from(""));
            out.push(Line::from(vec![
                Span::styled("ctrl+x", hint_key_style),
                Span::raw(" "),
                Span::styled("down", hint_key_style),
                Span::raw(" "),
                Span::styled("view subagents", hint_style),
            ]));
            out.push(Line::from(""));
        } else if name == "todowrite" && status == "ok" {
            if let Some(ref preview) = output_preview {
                let bg = colors.background_element;
                let pad_style = Style::default().bg(bg);
                let header_style = Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM)
                    .bg(bg);
                let item_style = Style::default().fg(colors.text).bg(bg);

                let panel_width = max_width.saturating_sub(2).max(10);

                // Panel header: # + label (opencode style)
                let header_text = format!("# {}", tool_label);
                let mut panel_lines: Vec<Line<'_>> = Vec::new();

                // Padding top
                panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));

                // Panel header
                panel_lines.push(Line::from(vec![Span::styled(header_text, header_style)]));

                // Body: each todo item as plain text (no markdown — avoids
                // brackets being interpreted as links).
                let preview_trimmed = preview.trim_end();
                for raw_line in preview_trimmed.lines() {
                    let trimmed = raw_line.trim_end();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let line = Line::from(vec![Span::styled(trimmed.to_string(), item_style)]);
                    panel_lines.extend(wrap_styled_line(
                        &line,
                        WrapOptions::new(panel_width)
                            .subsequent_indent(Line::from(Span::styled("  ", item_style))),
                    ));
                }

                // Padding bottom
                panel_lines.push(Line::from(vec![Span::styled("", pad_style)]));

                // Indent text one cell; the panel background is painted in a
                // separate pass so padding rows do not wrap.
                for line in &mut panel_lines {
                    line.spans.insert(0, Span::styled(" ", pad_style));
                }

                out.extend(panel_lines);
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

            let marker = if active { "~" } else { "•" };
            let marker_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
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
                    Span::styled(file_path, target_style),
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
                let diff_lines = crate::ui::diff::format_edit_diff_with_start(
                    old_str, new_str, start_line, max_width, colors, "    ",
                );
                out.extend(diff_lines);
            }
        } else {
            // Default header for all other tools.
            let header_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM);
            push_wrapped(
                &mut out,
                Line::from(Span::styled(header, header_style)),
                max_width,
                Line::from(Span::styled("  ", header_style)),
            );

            // Render a subtle result line for completed tools.
            if status == "ok" {
                if let Some(ref preview) = output_preview {
                    let mut result_text = preview.clone();
                    // For edits, prepend the title (e.g. "Edit: file.rs") if available.
                    if name == "edit" {
                        if let Some(ref t) = title {
                            result_text = format!("{} — {}", t, preview);
                        }
                    }
                    let result_style = Style::default().fg(colors.text_weak);
                    let mut emitted = 0usize;
                    for (line_idx, raw_line) in result_text.lines().enumerate() {
                        if emitted >= TOOL_RESULT_MAX_SCREEN_LINES {
                            out.push(Line::from(Span::styled("  …", result_style)));
                            break;
                        }
                        let prefix = if line_idx == 0 { "  → " } else { "    " };
                        let line = Line::from(Span::styled(
                            format!("{}{}", prefix, raw_line),
                            result_style,
                        ));
                        let before = out.len();
                        push_limited_wrapped(
                            &mut out,
                            line,
                            max_width,
                            Line::from(Span::styled("    ", result_style)),
                            TOOL_RESULT_MAX_SCREEN_LINES.saturating_sub(emitted),
                            result_style,
                        );
                        emitted += out.len().saturating_sub(before);
                    }
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

    fn format_metadata(&self, message: &Message, _model: &str, colors: &ThemeColors) -> Vec<Span> {
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
            agent_mode,
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ));

        // Separator (bullet)
        spans.push(Span::styled(" • ", Style::default().fg(colors.text_weak)));

        // Model ID - use persisted model from message, fallback to current model
        let model_display = message.model.as_deref().unwrap_or(_model);
        spans.push(Span::styled(
            model_display.to_string(),
            Style::default().fg(colors.text),
        ));

        // Timing + throughput metrics (only show for completed messages)
        if message.is_complete {
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

fn is_compact_tool_panel(content: &str) -> bool {
    serde_json::from_str::<JsonValue>(content)
        .ok()
        .and_then(|v| {
            let name = v.get("name").and_then(|n| n.as_str())?;
            let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("ok");
            Some(match name {
                "question" => status != "error",
                "todowrite" => status == "ok",
                "task" => true,
                _ => false,
            })
        })
        .unwrap_or(false)
}

fn is_synthetic_tool_result_text(content: &str) -> bool {
    content.trim_start().starts_with("[tool result:")
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
    line.spans.iter().any(|span| span.style.bg == Some(bg))
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
        style: Style::default(),
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

        assert_eq!(rendered, vec!["• Explored", "  └ Read AGENTS.md"]);
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

        assert_eq!(rendered, vec!["• Explored", "  └ List src/ui"]);
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
                "• Explored",
                "  └ List .",
                "    Read README.md",
                "    Search opencode|codex in references",
                ""
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
            vec!["• Explored", "  └ Read README.md, AGENTS.md", ""]
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
                "• Edited README.md (+1 -1)",
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
            vec!["• Added src/new.rs (+1 -0)", "    1 +fn main() {}"]
        );
    }

    #[test]
    fn test_question_panel_keeps_padding_without_extra_gap() {
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

        assert_eq!(rendered.len(), 5);
        assert!(rendered[0].trim().is_empty());
        assert_eq!(rendered[1].trim(), "# Questions");
        assert!(rendered[3].contains("Provide columns and rows"));
        assert!(rendered[4].trim().is_empty());
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
    fn test_task_tool_renders_opencode_style_summary() {
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
            .any(|line| line.contains("General Task") && line.contains("Say hi")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("0 toolcalls") && line.contains("4.1s")));
        assert!(rendered
            .iter()
            .any(|line| line.contains("ctrl+x down view subagents")));
        assert!(!rendered
            .iter()
            .any(|line| line.contains("prompt=\"Say hi\"")));
        assert!(!rendered.iter().any(|line| line.contains("Hi there!")));
    }

    #[test]
    fn test_todowrite_panel_keeps_padding_without_extra_gap() {
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

        assert_eq!(rendered.len(), 6);
        assert!(rendered[0].trim().is_empty());
        assert_eq!(rendered[1].trim(), "# Todos");
        assert!(rendered[4].contains("Implement rendering"));
        assert!(rendered[5].trim().is_empty());
    }

    #[test]
    fn test_short_tool_panel_renders_without_trailing_blank_row() {
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

        assert!(rows[0].trim().is_empty());
        assert_eq!(rows[1].trim(), "# Todos");
        assert!(rows[4].contains("Implement rendering"));
        assert!(rows[5].trim().is_empty());
        assert_eq!(buffer[(0, 0)].bg, colors.background_element);
        assert_eq!(buffer[(0, 5)].bg, colors.background_element);
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
