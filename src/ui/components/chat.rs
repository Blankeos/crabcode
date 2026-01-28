use crate::session::types::{Message, MessageRole};
use crate::theme::ThemeColors;
use ratatui::{
    crossterm::event::{MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};

#[derive(Debug, Clone, Default)]
pub struct Chat {
    pub messages: Vec<Message>,
    pub scroll_offset: usize,
    pub scrollbar_state: ScrollbarState,
    pub is_dragging_scrollbar: bool,
    pub content_height: usize,
    pub viewport_height: usize,
    pub streaming_start_time: Option<std::time::Instant>,
    pub streaming_first_token_time: Option<std::time::Instant>,
    pub streaming_token_count: usize,
    /// Whether to autoscroll to bottom when new content arrives
    /// Only autoscrolls if user is already near the bottom
    pub autoscroll_enabled: bool,
    /// Track if user has manually scrolled up (away from bottom)
    user_scrolled_up: bool,
}

// Minimum elapsed time before showing tokens/s (250ms)
const MIN_TOKENS_PER_SECOND_ELAPSED_MS: u128 = 250;

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
            streaming_token_count: 0,
            autoscroll_enabled: true,
            user_scrolled_up: false,
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
            streaming_token_count: 0,
            autoscroll_enabled: true,
            user_scrolled_up: false,
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
        self.autoscroll_enabled
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

    pub fn append_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        let chunk_str = chunk.as_ref();
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append(chunk_str);
                // Track streaming metrics - record first token time
                if self.streaming_first_token_time.is_none() {
                    self.streaming_first_token_time = Some(std::time::Instant::now());
                }
                // Estimate tokens: ~4 characters per token on average
                self.streaming_token_count += chunk_str.chars().count().max(1) / 4;
                if self.should_autoscroll() {
                    // Reset scroll to show new content at bottom
                    self.scroll_offset = usize::MAX;
                    self.user_scrolled_up = false;
                }
            }
        } else {
            // Starting a new assistant message
            let now = std::time::Instant::now();
            self.streaming_start_time = Some(now);
            self.streaming_first_token_time = Some(now);
            // Estimate tokens: ~4 characters per token on average
            self.streaming_token_count = chunk_str.chars().count().max(1) / 4;
            self.add_assistant_message(chunk_str);
        }
    }

    pub fn append_reasoning_to_last_assistant(&mut self, chunk: impl AsRef<str>) {
        let chunk_str = chunk.as_ref();
        if self
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::Assistant)
        {
            if let Some(msg) = self.messages.last_mut() {
                msg.append_reasoning(chunk_str);
                // Track streaming metrics for reasoning tokens too
                if self.streaming_first_token_time.is_none() {
                    self.streaming_first_token_time = Some(std::time::Instant::now());
                }
                // Estimate tokens: ~4 characters per token on average
                self.streaming_token_count += chunk_str.chars().count().max(1) / 4;
                if self.should_autoscroll() {
                    // Reset scroll to show new content at bottom
                    self.scroll_offset = usize::MAX;
                    self.user_scrolled_up = false;
                }
            }
        } else {
            // Create a new assistant message with reasoning
            let mut msg = Message::incomplete("");
            msg.append_reasoning(chunk_str);
            // Track streaming metrics
            let now = std::time::Instant::now();
            self.streaming_start_time = Some(now);
            self.streaming_first_token_time = Some(now);
            self.streaming_token_count = chunk_str.chars().count().max(1) / 4;
            self.add_message(msg);
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.scrollbar_state = ScrollbarState::default();
        self.content_height = 0;
        self.streaming_start_time = None;
        self.streaming_first_token_time = None;
        self.streaming_token_count = 0;
    }

    pub fn get_streaming_tokens_per_sec(&self) -> Option<f64> {
        // Use first_token_time for more accurate measurement (like PR #5497)
        if let Some(first_token_time) = self.streaming_first_token_time {
            let elapsed_ms = first_token_time.elapsed().as_millis();
            // Only show after minimum elapsed time to avoid inaccurate early readings
            if elapsed_ms >= MIN_TOKENS_PER_SECOND_ELAPSED_MS && self.streaming_token_count > 0 {
                let tokens_per_sec =
                    (self.streaming_token_count as f64) / (elapsed_ms as f64 / 1000.0);
                if tokens_per_sec.is_finite() {
                    return Some(tokens_per_sec);
                }
            }
        }
        None
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming_first_token_time.is_some()
            && self
                .messages
                .last()
                .is_some_and(|m| m.role == MessageRole::Assistant && !m.is_complete)
    }

    pub fn finalize_streaming_metrics(&mut self) {
        if let Some(first_token_time) = self.streaming_first_token_time {
            let duration_ms = first_token_time.elapsed().as_millis() as u64;
            let token_count = self.streaming_token_count;

            if let Some(last_msg) = self.messages.last_mut() {
                if last_msg.role == MessageRole::Assistant {
                    last_msg.token_count = Some(token_count);
                    last_msg.duration_ms = Some(duration_ms);
                }
            }
        }

        // Reset streaming state
        self.streaming_start_time = None;
        self.streaming_first_token_time = None;
        self.streaming_token_count = 0;
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

        // Render content
        let paragraph = Paragraph::new(Text::from(content_lines))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset as u16, 0));

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

        for (idx, message) in self.messages.iter().enumerate() {
            let message_lines = self.format_message(message, max_width, idx, model, colors);
            total_height += message_lines.len();
        }

        total_height
    }

    fn render_visible_messages(
        &self,
        max_width: usize,
        model: &str,
        colors: &ThemeColors,
    ) -> Vec<Line> {
        let mut all_lines: Vec<Line> = Vec::new();

        for (idx, message) in self.messages.iter().enumerate() {
            let message_lines = self.format_message(message, max_width, idx, model, colors);
            all_lines.extend(message_lines);
        }

        all_lines
    }

    fn format_message(
        &self,
        message: &Message,
        max_width: usize,
        _idx: usize,
        model: &str,
        colors: &ThemeColors,
    ) -> Vec<Line> {
        let mut lines: Vec<Line> = Vec::new();

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

                // AI message: No box, display content directly
                let content = message.content.clone();
                let wrapped_lines = textwrap::wrap(&content, max_width);

                for line in wrapped_lines {
                    lines.push(Line::from(line.to_string()));
                }

                // Add empty line before metadata for spacing
                lines.push(Line::from(""));

                // Add metadata line (always show for assistant messages)
                let metadata = self.format_metadata(message, model, colors);
                lines.push(Line::from(metadata));

                // Add empty line after AI message
                lines.push(Line::from(""));
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
                // Tool messages: dimmed
                let prefix = "Tool: ";
                let content = format!("{}{}", prefix, message.content);
                let wrapped_lines = textwrap::wrap(&content, max_width);

                for line in wrapped_lines {
                    lines.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::Gray),
                    )));
                }
                lines.push(Line::from(""));
            }
        }

        lines
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

        // Duration and tokens/second if available (only show for completed messages)
        if message.is_complete {
            if let (Some(token_count), Some(duration_ms)) =
                (message.token_count, message.duration_ms)
            {
                // Duration
                let duration_sec = duration_ms as f64 / 1000.0;
                spans.push(Span::styled(
                    format!(" • {:.1}s", duration_sec),
                    Style::default().fg(colors.text_weak),
                ));

                // Tokens/second
                let tokens_per_sec = if duration_ms > 0 {
                    (token_count as f64) / (duration_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                spans.push(Span::styled(
                    format!(" • {:.0}t/s", tokens_per_sec),
                    Style::default().fg(colors.text_weak),
                ));
            } else {
                // Show 0s and 0t/s when metrics not yet available
                spans.push(Span::styled(" • 0s", Style::default().fg(colors.text_weak)));
                spans.push(Span::styled(
                    " • 0t/s",
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
