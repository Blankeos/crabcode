use crate::session::types::{Message, MessageRole};
use crate::theme::ThemeColors;
use crate::ui::markdown::streaming::{render_markdown, SimpleStreamingRenderer};
use ratatui::{
    crossterm::event::{MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub messages: Vec<Message>,
    pub scroll_offset: usize,
    pub scrollbar_state: ScrollbarState,
    pub is_dragging_scrollbar: bool,
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
}

// Minimum elapsed time before showing tokens/s (250ms)
const MIN_TOKENS_PER_SECOND_ELAPSED_MS: u128 = 250;

fn now_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Chat {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            is_dragging_scrollbar: false,
            content_height: 0,
            viewport_height: 0,
            streaming_start_time: None,
            streaming_first_token_time: None,
            streaming_end_time: None,
            streaming_t0_ms: None,
            streaming_t1_ms: None,
            streaming_tn_ms: None,
            streaming_token_count: 0,
            autoscroll_enabled: true,
            user_scrolled_up: false,
            cached_tokens_per_sec: None,
            last_tps_calculated: None,
            streaming_renderer: None,
            streaming_message_idx: None,
        }
    }

    pub fn with_messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            is_dragging_scrollbar: false,
            content_height: 0,
            viewport_height: 0,
            streaming_start_time: None,
            streaming_first_token_time: None,
            streaming_end_time: None,
            streaming_t0_ms: None,
            streaming_t1_ms: None,
            streaming_tn_ms: None,
            streaming_token_count: 0,
            autoscroll_enabled: true,
            user_scrolled_up: false,
            cached_tokens_per_sec: None,
            last_tps_calculated: None,
            streaming_renderer: None,
            streaming_message_idx: None,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
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

        // Estimate tokens: ~4 characters per token on average
        self.streaming_token_count += chunk_str.chars().count().max(1) / 4;
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

        let now = std::time::Instant::now();
        if self.streaming_start_time.is_none() {
            self.streaming_start_time = Some(now);
            self.streaming_t0_ms = Some(now_epoch_ms());
        }
        if self.streaming_first_token_time.is_none() {
            self.streaming_first_token_time = Some(now);
            self.streaming_t1_ms = Some(now_epoch_ms());
        }
        self.streaming_token_count += chunk_str.chars().count().max(1) / 4;
        if self.should_autoscroll() {
            self.scroll_offset = usize::MAX;
            self.user_scrolled_up = false;
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.scrollbar_state = ScrollbarState::default();
        self.content_height = 0;
        self.streaming_start_time = None;
        self.streaming_first_token_time = None;
        self.streaming_end_time = None;
        self.streaming_t0_ms = None;
        self.streaming_t1_ms = None;
        self.streaming_tn_ms = None;
        self.streaming_token_count = 0;
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
        self.cached_tokens_per_sec = None;
        self.last_tps_calculated = None;

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

    pub fn get_streaming_tokens_per_sec(&mut self) -> Option<f64> {
        // Throttle token calculation to prevent excessive updates during high-frequency renders
        // caused by mouse movement. Only recalculate every 100ms.
        const TPS_THROTTLE_MS: u128 = 100;

        let now = std::time::Instant::now();
        if let Some(last_calc) = self.last_tps_calculated {
            if now.duration_since(last_calc).as_millis() < TPS_THROTTLE_MS {
                // Still within throttle window, return cached value
                return self.cached_tokens_per_sec;
            }
        }
        // Update timestamp for next throttle check
        self.last_tps_calculated = Some(now);

        // Use first_token_time for more accurate measurement (like PR #5497)
        let result = if let Some(first_token_time) = self.streaming_first_token_time {
            let elapsed_ms = first_token_time.elapsed().as_millis();
            // Only show after minimum elapsed time to avoid inaccurate early readings
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

        // Cache the result for throttled returns
        self.cached_tokens_per_sec = result;
        result
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

        let decode_duration_ms = if let (Some(t1), Some(tn)) =
            (self.streaming_first_token_time, self.streaming_end_time)
        {
            tn.duration_since(t1).as_millis() as u64
        } else if let Some(t1) = self.streaming_first_token_time {
            t1.elapsed().as_millis() as u64
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
        self.streaming_renderer = None;
        self.streaming_message_idx = None;
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

    fn update_scrollbar(&mut self) {
        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        let content_length = max_offset.saturating_add(1).max(1);
        let position = self.scroll_offset.min(content_length.saturating_sub(1));
        self.scrollbar_state = self.scrollbar_state.content_length(content_length);
        self.scrollbar_state = self.scrollbar_state.position(position);
    }

    pub fn handle_mouse_event(&mut self, event: MouseEvent, area: Rect) -> bool {
        use ratatui::layout::Position;
        let point = Position::new(event.column, event.row);

        if !area.contains(point) {
            self.is_dragging_scrollbar = false;
            return false;
        }

        // Calculate scrollbar area (rightmost column)
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };

        let is_on_scrollbar = scrollbar_area.contains(point);

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_down(3);
                true
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if is_on_scrollbar {
                    self.is_dragging_scrollbar = true;
                    self.scroll_to_position(event.row, scrollbar_area);
                    true
                } else {
                    false
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.is_dragging_scrollbar {
                    self.scroll_to_position(event.row, scrollbar_area);
                    true
                } else {
                    false
                }
            }
            MouseEventKind::Up(_) => {
                if self.is_dragging_scrollbar {
                    self.is_dragging_scrollbar = false;
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

        let relative_y = row.saturating_sub(scrollbar_area.y) as usize;
        let max_offset = self.content_height.saturating_sub(self.viewport_height);

        let new_offset = if max_offset > 0 && scrollbar_area.height > 0 {
            (relative_y * max_offset) / scrollbar_area.height as usize
        } else {
            0
        };
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

        // Update streaming renderer before calculating heights
        self.update_streaming_renderer();

        // Calculate content area (leave space for scrollbar)
        let content_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(1),
            height: area.height,
        };

        // Calculate total content height first
        let total_height =
            self.calculate_content_height(content_area.width as usize, model, colors);
        self.content_height = total_height;

        // Clamp scroll offset
        let max_offset = self.content_height.saturating_sub(self.viewport_height);
        self.scroll_offset = self.scroll_offset.min(max_offset);
        self.update_scrollbar();

        // Now render the visible content
        let content_lines =
            self.render_visible_messages(content_area.width as usize, model, colors);

        // Store scroll_offset before creating paragraph
        let scroll_offset = self.scroll_offset;

        // Render content
        let paragraph = Paragraph::new(Text::from(content_lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset as u16, 0));

        f.render_widget(paragraph, content_area);

        // Render scrollbar
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };

        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_symbol(Some(" "))
                .begin_symbol(Some(" "))
                .end_symbol(Some(" "))
                .thumb_symbol("█"),
            scrollbar_area,
            &mut self.scrollbar_state,
        );
    }

    fn calculate_content_height(
        &self,
        max_width: usize,
        model: &str,
        colors: &ThemeColors,
    ) -> usize {
        let mut total_height = 0;
        let message_count = self.messages.len();
        let streaming_idx = self.streaming_assistant_idx();
        let streaming_content = self.streaming_renderer.as_ref().map(|r| r.get_content());

        for (idx, message) in self.messages.iter().enumerate() {
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
            total_height += message_lines.len();
        }

        total_height
    }

    fn render_visible_messages<'a>(
        &'a self,
        max_width: usize,
        model: &'a str,
        colors: &'a ThemeColors,
    ) -> Vec<Line<'a>> {
        let mut all_lines: Vec<Line<'a>> = Vec::new();
        let message_count = self.messages.len();
        let streaming_idx = self.streaming_assistant_idx();
        let streaming_content = self.streaming_renderer.as_ref().map(|r| r.get_content());

        for (idx, message) in self.messages.iter().enumerate() {
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
        }

        all_lines
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

        let _ = message_count;

        match message.role {
            MessageRole::User => {
                // User message: Box with left border colored by agent mode
                let border_color = self.get_agent_color(message.agent_mode.as_deref());
                let content = message.content.clone();

                // Wrap content to fit within max_width - padding
                let wrapped_lines = textwrap::wrap(&content, max_width.saturating_sub(4));

                for (i, line) in wrapped_lines.iter().enumerate() {
                    let is_first = i == 0;
                    let _is_last = i == wrapped_lines.len() - 1;

                    let left_border = if is_first { "▌ " } else { "│ " };

                    let right_padding = " ".repeat(max_width.saturating_sub(line.len() + 3));

                    lines.push(Line::from(vec![
                        Span::styled(left_border, Style::default().fg(border_color)),
                        Span::raw(line.to_string()),
                        Span::raw(right_padding),
                    ]));
                }

                // Add empty line after user message
                lines.push(Line::from(""));
            }
            MessageRole::Assistant => {
                // Display reasoning/thinking tokens if present
                if let Some(ref reasoning) = message.reasoning {
                    if !reasoning.is_empty() {
                        let reasoning_prefix = "💭 Thinking...";
                        lines.push(Line::from(vec![Span::styled(
                            reasoning_prefix,
                            Style::default()
                                .fg(colors.text_weak)
                                .add_modifier(Modifier::ITALIC),
                        )]));

                        let wrapped_reasoning = textwrap::wrap(reasoning, max_width);
                        for line in wrapped_reasoning {
                            lines.push(Line::from(Span::styled(
                                line.to_string(),
                                Style::default()
                                    .fg(colors.text_weak)
                                    .add_modifier(Modifier::ITALIC),
                            )));
                        }

                        // Add separator between reasoning and content
                        lines.push(Line::from(""));
                    }
                }

                let is_streaming = streaming_idx == Some(idx) && !message.is_complete;

                if is_streaming {
                    // Use the streaming renderer content for markdown
                    if let Some(content) = streaming_content {
                        let markdown_lines = render_markdown(content, max_width);
                        lines.extend(markdown_lines);
                    } else {
                        // Fallback to plain text if renderer not available
                        let content = message.content.clone();
                        let wrapped_lines = textwrap::wrap(&content, max_width);
                        for line in wrapped_lines {
                            lines.push(Line::from(line.to_string()));
                        }
                    }
                } else {
                    // For complete messages, use tui-markdown directly
                    let markdown_lines = render_markdown(&message.content, max_width);
                    lines.extend(markdown_lines);
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
                    // Keep spacing consistent between segments.
                    lines.push(Line::from(""));
                }
            }
            MessageRole::System => {
                // System messages: simple display
                let prefix = "System: ";
                let content = format!("{}{}", prefix, message.content);
                let wrapped_lines = textwrap::wrap(&content, max_width);

                for line in wrapped_lines {
                    lines.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::Yellow),
                    )));
                }
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
        fn preview_value(v: &JsonValue, max_len: usize) -> String {
            let mut s = match v {
                JsonValue::String(s) => s.clone(),
                JsonValue::Number(n) => n.to_string(),
                JsonValue::Bool(b) => b.to_string(),
                JsonValue::Null => "null".to_string(),
                other => other.to_string(),
            };
            if s.len() > max_len {
                s.truncate(max_len);
                s.push_str("…");
            }
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

        let _ = attached;
        let indent = "";
        let mut out: Vec<Line<'a>> = Vec::new();

        let parsed: Option<JsonValue> = serde_json::from_str(&message.content).ok();
        let (name, status, args, metadata, output_preview) =
            if let Some(JsonValue::Object(obj)) = parsed {
                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let status = obj
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ok")
                    .to_string();
                let args = obj.get("args").cloned();
                let metadata = obj.get("metadata").cloned();
                let output_preview = obj
                    .get("output_preview")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (name, status, args, metadata, output_preview)
            } else {
                (
                    "tool".to_string(),
                    "ok".to_string(),
                    None,
                    None,
                    Some(message.content.clone()),
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
            other => other,
        };

        let args_obj = args.as_ref().and_then(|v| v.as_object());
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

        let wrapped = textwrap::wrap(&header, max_width);
        for line in wrapped {
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            )));
        }

        if status == "error" {
            if let Some(preview) = output_preview {
                let first = preview.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    let mut line = first.to_string();
                    if line.len() > max_width.saturating_sub(6) {
                        line.truncate(max_width.saturating_sub(6));
                        line.push_str("…");
                    }
                    out.push(Line::from(Span::styled(
                        format!("{}    {}", indent, line),
                        Style::default().fg(colors.error),
                    )));
                }
            }
        }

        out
    }

    fn get_agent_color(&self, agent_mode: Option<&str>) -> Color {
        match agent_mode {
            Some("Plan") => Color::Rgb(255, 165, 0),    // Orange
            Some("Build") => Color::Rgb(147, 112, 219), // Purple
            _ => Color::Gray,
        }
    }

    fn format_metadata(&self, message: &Message, _model: &str, colors: &ThemeColors) -> Vec<Span> {
        let mut spans = Vec::new();

        // Get agent mode from previous user message or default to "Plan"
        let agent_mode = self.get_agent_mode_for_message(message);
        let agent_color = self.get_agent_color(Some(&agent_mode));

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
            Style::default().fg(colors.text_weak),
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

use ratatui::text::Text;

#[cfg(test)]
mod tests {
    use super::*;

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
