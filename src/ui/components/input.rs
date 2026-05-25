use crate::autocomplete::{AutoComplete, Suggestion, SuggestionKind};
use crate::persistence::PromptHistoryCache;
use crate::push_toast;
use crate::theme::{agent_color, ThemeColors};
use crate::toast::{Toast, ToastLevel};
use crate::utils::image_attachment;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::prelude::{Rect, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::ops::Range;
use std::path::PathBuf;
use tui_textarea::{CursorMove, Input as TuiInput, TextArea};
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// Clamp a byte offset to the nearest valid UTF-8 character boundary in `s`.
fn char_boundary_before(s: &str, byte_idx: usize) -> usize {
    let idx = byte_idx.min(s.len());
    if s.is_char_boundary(idx) {
        idx
    } else {
        (0..idx).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
    }
}

/// Word category for word-delete logic (matching tui-textarea's CharKind).
fn char_kind(c: char) -> u8 {
    if c.is_whitespace() {
        0 // Space
    } else if c.is_ascii_punctuation() {
        1 // Punct
    } else {
        2 // Other (includes emoji, letters, etc.)
    }
}

const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;
const MAX_TEXTAREA_HEIGHT: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
struct VisualLine {
    source_row: usize,
    start_col: usize,
    end_col: usize,
}

pub struct Input {
    textarea: TextArea<'static>,
    pub autocomplete: Option<AutoComplete>,
    textarea_area: Option<Rect>,
    viewport_top: usize,
    preferred_visual_col: Option<usize>,
    prompt_history: Option<PromptHistoryCache>,
    draft_text: Option<String>,
    local_images: Vec<LocalImageAttachment>,
    pending_pastes: Vec<PendingPaste>,
    image_open_config: crate::config::ImagesConfig,
    hovered_image_placeholder: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingPaste {
    placeholder: String,
    content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalImageAttachment {
    pub placeholder: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionToken {
    query: String,
    range: Range<usize>,
}

impl Input {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        // Default selection style (will be updated per-theme in render)
        textarea.set_selection_style(
            Style::default()
                .bg(ratatui::style::Color::Rgb(255, 140, 0))
                .fg(ratatui::style::Color::Reset),
        );
        let prompt_history = PromptHistoryCache::new().ok();
        Self {
            textarea,
            autocomplete: None,
            textarea_area: None,
            viewport_top: 0,
            preferred_visual_col: None,
            prompt_history,
            draft_text: None,
            local_images: Vec::new(),
            pending_pastes: Vec::new(),
            image_open_config: crate::config::ImagesConfig::default(),
            hovered_image_placeholder: None,
        }
    }

    pub fn with_autocomplete(mut self, autocomplete: AutoComplete) -> Self {
        self.autocomplete = Some(autocomplete);
        self
    }

    pub fn set_image_open_config(&mut self, config: crate::config::ImagesConfig) {
        self.image_open_config = config;
    }

    pub fn contains_mouse(&self, mouse: MouseEvent) -> bool {
        let Some(area) = self.textarea_area else {
            return false;
        };
        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        area.contains(point)
    }

    pub fn clear_hover(&mut self) {
        self.hovered_image_placeholder = None;
    }

    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        area: Rect,
        agent: &str,
        model: &str,
        provider_name: &str,
        reasoning_effort: Option<&str>,
        colors: &ThemeColors,
    ) {
        let agent_color = agent_color(agent, colors);

        let border_set = border::Set {
            vertical_left: "┃",
            ..border::PLAIN
        };

        let border = Block::new()
            .borders(Borders::LEFT)
            .border_set(border_set)
            .border_style(Style::default().fg(agent_color));
        let inner_area = border.inner(area);

        let bg_area = Rect {
            x: inner_area.x,
            y: inner_area.y,
            width: inner_area.width,
            height: inner_area.height.saturating_sub(1),
        };
        let bg = Block::default().style(Style::default().bg(colors.background_element));
        frame.render_widget(bg, bg_area);

        let h_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Length(2),
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(2),
            ])
            .split(inner_area);

        let wrap_width = h_chunks[1].width as usize;
        let textarea_height = self.textarea_height(wrap_width) as u16;

        let v_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(textarea_height),
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(h_chunks[1]);

        self.textarea_area = Some(v_chunks[1]);

        self.textarea
            .set_selection_style(Style::default().bg(colors.accent).fg(colors.text));
        self.textarea.set_style(
            Style::default()
                .fg(colors.text)
                .bg(colors.background_element),
        );

        let visible_lines = v_chunks[1].height as usize;
        self.update_viewport(visible_lines, wrap_width);
        self.render_wrapped_textarea(frame, v_chunks[1], colors);

        let mut info_spans = vec![
            ratatui::text::Span::styled(agent.to_string(), Style::default().fg(agent_color)),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(model.to_string(), Style::default().fg(colors.text)),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(
                provider_name.to_string(),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(ratatui::style::Modifier::DIM),
            ),
        ];

        if let Some(reasoning_effort) = reasoning_effort {
            info_spans.push(ratatui::text::Span::raw("  "));
            info_spans.push(ratatui::text::Span::styled(
                reasoning_effort.to_string(),
                Style::default()
                    .fg(colors.warning)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ));
        }

        let info_text = ratatui::text::Line::from(info_spans);

        let info_paragraph = Paragraph::new(info_text);
        frame.render_widget(info_paragraph, v_chunks[3]);

        frame.render_widget(border, area);

        let cap_row = Paragraph::new(ratatui::text::Line::from(vec![
            ratatui::text::Span::styled("╹", Style::default().fg(agent_color)),
            ratatui::text::Span::styled(
                "▀".repeat(area.width as usize - 1),
                Style::default().fg(colors.background_element),
            ),
        ]));
        let cap_row_area = Rect::new(area.x, v_chunks[4].y, area.width, 1);
        frame.render_widget(cap_row, cap_row_area);
    }

    pub fn get_height(&self) -> u16 {
        // The exact wrap width is only known during render; keep the existing
        // compact default so layout can reserve space before the first draw.
        let line_count = self.textarea.lines().len().max(1);
        let textarea_height = line_count.min(MAX_TEXTAREA_HEIGHT) as u16;
        textarea_height + 4
    }

    pub fn get_height_for_width(&self, area_width: u16) -> u16 {
        let wrap_width = area_width.saturating_sub(5).max(1) as usize;
        self.textarea_height(wrap_width) as u16 + 4
    }

    pub fn handle_event(&mut self, event: KeyEvent) -> bool {
        let input = TuiInput::from(event);

        // push_toast(Toast::new(
        //     format!("Input event: {:?} | {:?}", input.key, input.shift),
        //     ToastLevel::Info,
        //     None,
        // ));

        // Check for Shift+Enter (works in most terminals)
        if event.code == KeyCode::Enter && event.modifiers.contains(KeyModifiers::SHIFT) {
            self.textarea.insert_newline();
            self.sync_image_placeholders();
            self.sync_pending_pastes();
            return true;
        }

        // Fallback: Alt+Enter for terminals where Shift+Enter doesn't work
        if event.code == KeyCode::Enter && event.modifiers.contains(KeyModifiers::ALT) {
            self.textarea.insert_newline();
            self.sync_image_placeholders();
            self.sync_pending_pastes();
            return true;
        }

        // Regular Enter submits
        if event.code == KeyCode::Enter && event.modifiers == KeyModifiers::NONE {
            self.save_current_to_history();
            return false;
        }

        // Handle Up arrow for prompt history navigation
        // Trigger when cursor is on first line
        if event.code == KeyCode::Up && event.modifiers == KeyModifiers::NONE {
            if self.move_cursor_visual(-1) {
                return true;
            }

            if self.is_cursor_on_first_visual_line() {
                let current_text = self.get_text();
                if let Some(ref mut history) = self.prompt_history {
                    if let Some(prompt) = history.navigate_up(&current_text) {
                        if self.draft_text.is_none() {
                            self.draft_text = Some(current_text);
                        }
                        self.set_text(&prompt);
                        self.textarea.move_cursor(CursorMove::Head);
                        return true;
                    }
                }
            }
        }

        // Handle Down arrow for prompt history navigation
        if event.code == KeyCode::Down && event.modifiers == KeyModifiers::NONE {
            if self.move_cursor_visual(1) {
                return true;
            }

            if self.is_cursor_on_last_visual_line() {
                let current_text = self.get_text();
                let should_reset = if let Some(ref mut history) = self.prompt_history {
                    if let Some(prompt) = history.navigate_down(&current_text) {
                        let is_empty = prompt.is_empty();
                        if is_empty {
                            // Restore draft text when reaching the end of history
                            if let Some(draft) = self.draft_text.take() {
                                self.set_text(&draft);
                            } else {
                                self.set_text("");
                            }
                        } else {
                            self.set_text(&prompt);
                        }
                        self.textarea.move_cursor(CursorMove::End);
                        is_empty
                    } else {
                        false
                    }
                } else {
                    false
                };
                if should_reset {
                    if let Some(ref mut history) = self.prompt_history {
                        history.reset_navigation();
                    }
                }
                if should_reset
                    || self
                        .prompt_history
                        .as_ref()
                        .map_or(false, |h| h.is_navigating())
                {
                    return true;
                }
            }
        }

        match event.code {
            KeyCode::Char('j') if event.modifiers == KeyModifiers::CONTROL => {
                self.preferred_visual_col = None;
                self.textarea.insert_newline();
                self.sync_image_placeholders();
                self.sync_pending_pastes();
                true
            }
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => false,
            KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => {
                self.preferred_visual_col = None;
                let (cursor_row, cursor_col) = self.textarea.cursor();
                if let Some(line) = self.textarea.lines().get(cursor_row) {
                    // Clamp to valid char boundary to avoid panics on multi-byte emoji
                    let safe_col = char_boundary_before(line, cursor_col);
                    let before_cursor = &line[..safe_col];
                    for _ in 0..before_cursor.chars().count() {
                        self.textarea.delete_char();
                    }
                }
                self.sync_image_placeholders();
                self.sync_pending_pastes();
                true
            }
            KeyCode::Tab => false,
            KeyCode::Esc => false,
            KeyCode::Backspace if self.remove_placeholder_at_cursor(false) => true,
            KeyCode::Delete if self.remove_placeholder_at_cursor(true) => true,
            KeyCode::Backspace if event.modifiers.contains(KeyModifiers::ALT) => {
                self.preferred_visual_col = None;
                // Handle Alt+Backspace (word-delete) ourselves to avoid
                // tui-textarea's buggy word boundary with multi-byte emoji
                self.delete_word_backward();
                self.sync_image_placeholders();
                self.sync_pending_pastes();
                true
            }
            _ => {
                self.preferred_visual_col = None;
                self.textarea.input(input);
                self.sync_image_placeholders();
                self.sync_pending_pastes();
                true
            }
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        let textarea_area = match self.textarea_area {
            Some(area) => area,
            None => return false,
        };

        let mouse_x = mouse.column;
        let mouse_y = mouse.row;

        // Check if mouse is within the textarea area
        let within_textarea = mouse_x >= textarea_area.x
            && mouse_x < textarea_area.x + textarea_area.width
            && mouse_y >= textarea_area.y
            && mouse_y < textarea_area.y + textarea_area.height;

        if !within_textarea {
            if matches!(mouse.kind, MouseEventKind::Moved) {
                self.hovered_image_placeholder = None;
            }
            return false;
        }

        match mouse.kind {
            MouseEventKind::Moved => {
                let previous_hover = self.hovered_image_placeholder.clone();
                self.hovered_image_placeholder = self
                    .image_at_mouse_position(textarea_area, mouse)
                    .map(|image| image.placeholder);
                previous_hover != self.hovered_image_placeholder
            }
            MouseEventKind::ScrollDown => {
                self.move_cursor_visual(1);
                true
            }
            MouseEventKind::ScrollUp => {
                self.move_cursor_visual(-1);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.preferred_visual_col = None;
                let relative_x = mouse_x.saturating_sub(textarea_area.x);
                let relative_y = mouse_y.saturating_sub(textarea_area.y);

                if let Some((target_row, target_col)) =
                    self.cursor_for_screen_position(textarea_area, relative_x, relative_y)
                {
                    let offset = self.flat_offset_for_position(target_row, target_col);
                    if let Some(image) = self.image_at_offset(offset) {
                        match image_attachment::open_path(&image.path, &self.image_open_config) {
                            Ok(()) => push_toast(Toast::new(
                                format!("Opened {}", image.placeholder),
                                ToastLevel::Info,
                                None,
                            )),
                            Err(err) => push_toast(Toast::new(
                                format!("Failed to open image: {}", err),
                                ToastLevel::Error,
                                None,
                            )),
                        }
                        return true;
                    }
                    // Position cursor and start selection for potential drag
                    self.textarea
                        .move_cursor(CursorMove::Jump(target_row as u16, target_col as u16));
                    self.textarea.start_selection();
                } else {
                    let lines = self.textarea.lines();
                    let last_row = lines.len().saturating_sub(1);
                    let last_col = lines[last_row].chars().count();
                    self.textarea
                        .move_cursor(CursorMove::Jump(last_row as u16, last_col as u16));
                    self.textarea.start_selection();
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.preferred_visual_col = None;
                // Extend the ongoing selection
                let relative_x = mouse_x.saturating_sub(textarea_area.x);
                let relative_y = mouse_y.saturating_sub(textarea_area.y);

                if let Some((target_row, target_col)) =
                    self.cursor_for_screen_position(textarea_area, relative_x, relative_y)
                {
                    // Since start_selection() was called and is_selecting() is true,
                    // move_cursor extends the selection
                    self.textarea
                        .move_cursor(CursorMove::Jump(target_row as u16, target_col as u16));
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Selection finalized (cursor was moved during drag)
                true
            }
            MouseEventKind::Up(MouseButton::Right) => {
                // Right-click clears selection
                self.textarea.cancel_selection();
                true
            }
            _ => false,
        }
    }

    pub fn has_selection(&self) -> bool {
        self.textarea.is_selecting()
    }

    pub fn get_selected_text(&self) -> String {
        let range = match self.textarea.selection_range() {
            Some(r) => r,
            None => return String::new(),
        };
        let ((start_row, start_col), (end_row, end_col)) = range;
        let lines = self.textarea.lines();

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i < start_row || i > end_row {
                continue;
            }
            let start = if i == start_row {
                start_col.min(line.len())
            } else {
                0
            };
            let end = if i == end_row {
                end_col.min(line.len())
            } else {
                line.len()
            };

            if start >= end {
                continue;
            }
            // Byte-based slicing (safe: start/end are guaranteed char boundaries)
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&line[start..end]);
        }
        result
    }

    pub fn clear_selection(&mut self) {
        self.textarea.cancel_selection();
    }

    /// Delete the word before the cursor. Handles multi-byte emoji correctly
    /// (works around a tui-textarea bug in find_word_start_backward).
    fn delete_word_backward(&mut self) {
        let (row, cursor_col) = self.textarea.cursor();
        let lines = self.textarea.lines();
        let line = match lines.get(row) {
            Some(l) => l,
            None => return,
        };

        // Find the word start by walking chars backwards from the cursor
        let safe_col = char_boundary_before(line, cursor_col);
        if safe_col == 0 {
            // At start of line: join with previous line if possible
            if row > 0 {
                self.textarea.move_cursor(CursorMove::Jump(row as u16, 0));
                self.textarea.delete_char(); // deletes newline, joining lines
            }
            return;
        }

        // Walk backwards from the cursor to find the word boundary
        let prefix = &line[..safe_col];
        let chars_rev: Vec<(usize, char)> = prefix.char_indices().rev().collect();

        if chars_rev.is_empty() {
            return;
        }

        // Determine the category of the character just before the cursor
        let (_, first_char) = chars_rev[0];
        let first_kind = char_kind(first_char);

        // Scan backward to find where the word starts
        let mut word_start = safe_col;
        for (byte_idx, c) in chars_rev.iter().skip(1) {
            let kind = char_kind(*c);
            if kind != first_kind {
                // Boundary found at the byte after this character
                word_start = byte_idx + c.len_utf8();
                break;
            }
            word_start = *byte_idx;
        }

        // Delete from word_start to safe_col
        if word_start < safe_col {
            let char_count = line[word_start..safe_col].chars().count();
            self.textarea
                .move_cursor(CursorMove::Jump(row as u16, safe_col as u16));
            for _ in 0..char_count {
                self.textarea.delete_char();
            }
        }
    }

    fn current_at_token(&self, allow_empty: bool) -> Option<CompletionToken> {
        let text = self.get_text();
        let cursor = self.flat_cursor_offset().min(text.len());
        if !text.is_char_boundary(cursor) {
            return None;
        }
        let before_cursor = &text[..cursor];
        let at_index = before_cursor.rfind('@')?;

        if at_index > 0 {
            let before_at = &text[..at_index];
            if !before_at
                .chars()
                .last()
                .map(char::is_whitespace)
                .unwrap_or(true)
            {
                return None;
            }
        }

        let query = &text[at_index + 1..cursor];
        if (!allow_empty && query.is_empty()) || query.chars().any(char::is_whitespace) {
            return None;
        }

        let end = cursor
            + text[cursor..]
                .find(char::is_whitespace)
                .unwrap_or_else(|| text.len().saturating_sub(cursor));

        Some(CompletionToken {
            query: query.to_string(),
            range: at_index..end,
        })
    }

    fn command_query(&self) -> Option<String> {
        let text = self.get_text();
        if !text.starts_with('/') || text.contains('\n') {
            return None;
        }
        Some(text.trim_start_matches('/').to_string())
    }

    fn char_col_to_byte_offset(line: &str, col: usize) -> usize {
        line.char_indices()
            .nth(col)
            .map(|(idx, _)| idx)
            .unwrap_or(line.len())
    }

    fn line_char_slice(line: &str, start_col: usize, end_col: usize) -> &str {
        let start = Self::char_col_to_byte_offset(line, start_col);
        let end = Self::char_col_to_byte_offset(line, end_col);
        &line[start..end]
    }

    fn display_col_to_char_col(
        line: &str,
        start_col: usize,
        end_col: usize,
        display_col: usize,
    ) -> usize {
        let mut current_display = 0;

        for (offset, ch) in Self::line_char_slice(line, start_col, end_col)
            .chars()
            .enumerate()
        {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(1);
            if display_col < current_display + char_width {
                return start_col + offset;
            }
            current_display += char_width;
        }

        end_col
    }

    fn flat_cursor_offset(&self) -> usize {
        let (row, col) = self.textarea.cursor();
        let lines = self.textarea.lines();
        let mut offset = 0;
        for line in lines.iter().take(row) {
            offset += line.len() + 1;
        }
        offset
            + lines
                .get(row)
                .map(|line| Self::char_col_to_byte_offset(line, col))
                .unwrap_or(0)
    }

    fn flat_offset_for_position(&self, row: usize, col: usize) -> usize {
        let lines = self.textarea.lines();
        let mut offset = 0;
        for line in lines.iter().take(row) {
            offset += line.len() + 1;
        }
        offset
            + lines
                .get(row)
                .map(|line| Self::char_col_to_byte_offset(line, col))
                .unwrap_or(0)
    }

    fn cursor_for_flat_offset(text: &str, mut offset: usize) -> (usize, usize) {
        offset = char_boundary_before(text, offset);
        let mut consumed = 0;
        for (row, line) in text.split('\n').enumerate() {
            let line_end = consumed + line.len();
            if offset <= line_end {
                return (row, line[..offset - consumed].chars().count());
            }
            consumed = line_end + 1;
        }
        let last_line = text.rsplit('\n').next().unwrap_or("");
        (
            text.lines().count().saturating_sub(1),
            last_line.chars().count(),
        )
    }

    fn reset_textarea(&mut self) {
        self.textarea = TextArea::default();
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_selection_style(
            Style::default()
                .bg(ratatui::style::Color::Rgb(255, 140, 0))
                .fg(ratatui::style::Color::Reset),
        );
    }

    fn set_text_preserving_images(&mut self, text: &str, cursor_offset: usize) {
        self.reset_textarea();
        self.textarea.insert_str(text);
        let cursor_offset = char_boundary_before(text, cursor_offset.min(text.len()));
        let (row, col) = Self::cursor_for_flat_offset(text, cursor_offset);
        self.textarea
            .move_cursor(CursorMove::Jump(row as u16, col as u16));
        self.viewport_top = 0;
        self.preferred_visual_col = None;
        self.hovered_image_placeholder = None;
    }

    fn image_placeholder(number: usize) -> String {
        format!("[Image #{}]", number)
    }

    fn next_scroll_offset(previous: usize, cursor: usize, visible_len: usize) -> usize {
        if visible_len == 0 {
            return 0;
        }
        if cursor < previous {
            cursor
        } else if previous + visible_len <= cursor {
            cursor + 1 - visible_len
        } else {
            previous
        }
    }

    fn textarea_height(&self, wrap_width: usize) -> usize {
        self.visual_lines(wrap_width)
            .len()
            .max(1)
            .min(MAX_TEXTAREA_HEIGHT)
    }

    fn visual_lines(&self, wrap_width: usize) -> Vec<VisualLine> {
        let wrap_width = wrap_width.max(1);
        let mut visual_lines = Vec::new();

        for (source_row, line) in self.textarea.lines().iter().enumerate() {
            let line_len = line.chars().count();
            if line_len == 0 {
                visual_lines.push(VisualLine {
                    source_row,
                    start_col: 0,
                    end_col: 0,
                });
                continue;
            }

            let mut start_col = 0;
            while start_col < line_len {
                let mut end_col = start_col;
                let mut width = 0;

                for ch in line.chars().skip(start_col) {
                    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if end_col > start_col && width + ch_width > wrap_width {
                        break;
                    }
                    width += ch_width;
                    end_col += 1;
                    if width >= wrap_width {
                        break;
                    }
                }

                if end_col == start_col {
                    end_col += 1;
                }

                visual_lines.push(VisualLine {
                    source_row,
                    start_col,
                    end_col,
                });
                start_col = end_col;
            }
        }

        visual_lines
    }

    fn cursor_visual_row(&self, visual_lines: &[VisualLine]) -> Option<usize> {
        let (cursor_row, cursor_col) = self.textarea.cursor();
        let line_len = self
            .textarea
            .lines()
            .get(cursor_row)
            .map(|line| line.chars().count())
            .unwrap_or(0);

        visual_lines
            .iter()
            .enumerate()
            .find_map(|(idx, visual_line)| {
                if visual_line.source_row != cursor_row {
                    return None;
                }

                let contains_cursor = cursor_col >= visual_line.start_col
                    && (cursor_col < visual_line.end_col
                        || (cursor_col == visual_line.end_col && cursor_col == line_len));

                contains_cursor.then_some(idx)
            })
    }

    fn cursor_display_col(&self, visual_line: &VisualLine) -> usize {
        let (_, cursor_col) = self.textarea.cursor();
        let Some(line) = self.textarea.lines().get(visual_line.source_row) else {
            return 0;
        };
        let cursor_col = cursor_col.clamp(visual_line.start_col, visual_line.end_col);
        UnicodeWidthStr::width(Self::line_char_slice(
            line,
            visual_line.start_col,
            cursor_col,
        ))
    }

    fn move_cursor_visual(&mut self, direction: isize) -> bool {
        let Some(area) = self.textarea_area else {
            return false;
        };
        if area.width == 0 {
            return false;
        }

        let visual_lines = self.visual_lines(area.width as usize);
        let Some(current_idx) = self.cursor_visual_row(&visual_lines) else {
            return false;
        };

        let target_idx = if direction < 0 {
            match current_idx.checked_sub(1) {
                Some(idx) => idx,
                None => return false,
            }
        } else {
            let idx = current_idx + 1;
            if idx >= visual_lines.len() {
                return false;
            }
            idx
        };

        let preferred_col = self
            .preferred_visual_col
            .unwrap_or_else(|| self.cursor_display_col(&visual_lines[current_idx]));
        let target = &visual_lines[target_idx];
        let Some(line) = self.textarea.lines().get(target.source_row) else {
            return false;
        };
        let target_col =
            Self::display_col_to_char_col(line, target.start_col, target.end_col, preferred_col);

        self.textarea.move_cursor(CursorMove::Jump(
            target.source_row as u16,
            target_col as u16,
        ));
        self.preferred_visual_col = Some(preferred_col);
        true
    }

    fn is_cursor_on_first_visual_line(&self) -> bool {
        let Some(area) = self.textarea_area else {
            return self.textarea.cursor().0 == 0;
        };
        let visual_lines = self.visual_lines(area.width as usize);
        self.cursor_visual_row(&visual_lines) == Some(0)
    }

    fn is_cursor_on_last_visual_line(&self) -> bool {
        let Some(area) = self.textarea_area else {
            return self.textarea.cursor().0 == self.textarea.lines().len().saturating_sub(1);
        };
        let visual_lines = self.visual_lines(area.width as usize);
        self.cursor_visual_row(&visual_lines) == visual_lines.len().checked_sub(1)
    }

    fn cursor_for_screen_position(
        &self,
        area: Rect,
        relative_x: u16,
        relative_y: u16,
    ) -> Option<(usize, usize)> {
        let visual_lines = self.visual_lines(area.width as usize);
        let visual_idx = self.viewport_top + relative_y as usize;
        let visual_line = visual_lines.get(visual_idx)?;
        let line = self.textarea.lines().get(visual_line.source_row)?;
        let target_col = Self::display_col_to_char_col(
            line,
            visual_line.start_col,
            visual_line.end_col,
            relative_x as usize,
        );

        Some((visual_line.source_row, target_col))
    }

    fn render_wrapped_textarea(
        &mut self,
        frame: &mut ratatui::Frame,
        area: Rect,
        colors: &ThemeColors,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let text_style = self.textarea.style();
        let cursor_style = self.textarea.cursor_style();
        let selection_style = self.textarea.selection_style();
        let selection_range = self.textarea.selection_range();
        let cursor = self.textarea.cursor();
        let visual_lines = self.visual_lines(area.width as usize);

        let text = if self.is_empty() && !self.textarea.placeholder_text().is_empty() {
            let placeholder_style = self
                .textarea
                .placeholder_style()
                .unwrap_or_else(|| Style::default().fg(colors.text_weak));
            Text::from(Line::from(vec![
                Span::styled(" ", cursor_style),
                Span::styled(
                    self.textarea.placeholder_text().to_string(),
                    placeholder_style,
                ),
            ]))
        } else {
            let lines = self.textarea.lines();
            let rendered = visual_lines
                .iter()
                .skip(self.viewport_top)
                .take(area.height as usize)
                .filter_map(|visual_line| {
                    let line = lines.get(visual_line.source_row)?;
                    Some(Self::render_visual_line(
                        line,
                        visual_line,
                        text_style,
                        cursor_style,
                        selection_style,
                        selection_range,
                        cursor,
                    ))
                })
                .collect::<Vec<_>>();
            Text::from(rendered)
        };

        frame.render_widget(Paragraph::new(text).style(text_style), area);
        self.style_placeholder_ranges(frame.buffer_mut(), area, colors, &visual_lines);
    }

    fn render_visual_line(
        line: &str,
        visual_line: &VisualLine,
        text_style: Style,
        cursor_style: Style,
        selection_style: Style,
        selection_range: Option<((usize, usize), (usize, usize))>,
        cursor: (usize, usize),
    ) -> Line<'static> {
        let line_len = line.chars().count();
        let mut spans = Vec::new();

        if visual_line.start_col == visual_line.end_col {
            if cursor == (visual_line.source_row, visual_line.start_col) {
                spans.push(Span::styled(" ", cursor_style));
            }
            return Line::from(spans);
        }

        for (idx, ch) in Self::line_char_slice(line, visual_line.start_col, visual_line.end_col)
            .chars()
            .enumerate()
        {
            let col = visual_line.start_col + idx;
            let mut style = text_style;

            if Self::position_in_selection(selection_range, visual_line.source_row, col) {
                style = selection_style;
            }
            if cursor == (visual_line.source_row, col) {
                style = cursor_style;
            }

            spans.push(Span::styled(ch.to_string(), style));
        }

        if cursor == (visual_line.source_row, visual_line.end_col)
            && visual_line.end_col == line_len
        {
            spans.push(Span::styled(" ", cursor_style));
        }

        Line::from(spans)
    }

    fn position_in_selection(
        selection_range: Option<((usize, usize), (usize, usize))>,
        row: usize,
        col: usize,
    ) -> bool {
        let Some((start, end)) = selection_range else {
            return false;
        };
        (row, col) >= start && (row, col) < end
    }

    fn update_viewport(&mut self, visible_lines: usize, wrap_width: usize) {
        let visual_lines = self.visual_lines(wrap_width);
        let cursor_visual_row = self.cursor_visual_row(&visual_lines).unwrap_or(0);
        let max_viewport_top = visual_lines.len().saturating_sub(visible_lines);

        self.viewport_top = self.viewport_top.min(max_viewport_top);
        self.viewport_top =
            Self::next_scroll_offset(self.viewport_top, cursor_visual_row, visible_lines)
                .min(max_viewport_top);
    }

    fn style_placeholder_ranges(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        colors: &ThemeColors,
        visual_lines: &[VisualLine],
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let lines = self.textarea.lines();

        for (screen_row, visual_line) in visual_lines
            .iter()
            .skip(self.viewport_top)
            .take(area.height as usize)
            .enumerate()
        {
            let Some(line) = lines.get(visual_line.source_row) else {
                continue;
            };
            let y = area.y + screen_row as u16;

            for image in &self.local_images {
                let placeholder_style = if self.hovered_image_placeholder.as_deref()
                    == Some(image.placeholder.as_str())
                {
                    Style::default().fg(colors.markdown_image_text)
                } else {
                    Style::default().fg(colors.markdown_image)
                };
                for (start, _) in line.match_indices(&image.placeholder) {
                    Self::style_line_byte_range(
                        buffer,
                        area,
                        y,
                        line,
                        start..start + image.placeholder.len(),
                        visual_line,
                        placeholder_style,
                    );
                }
            }

            for paste in &self.pending_pastes {
                let placeholder_style = Style::default().fg(colors.markdown_image);
                for (start, _) in line.match_indices(&paste.placeholder) {
                    Self::style_line_byte_range(
                        buffer,
                        area,
                        y,
                        line,
                        start..start + paste.placeholder.len(),
                        visual_line,
                        placeholder_style,
                    );
                }
            }
        }
    }

    fn style_line_byte_range(
        buffer: &mut Buffer,
        area: Rect,
        y: u16,
        line: &str,
        range: Range<usize>,
        visual_line: &VisualLine,
        style: Style,
    ) {
        if range.start > range.end
            || range.end > line.len()
            || !line.is_char_boundary(range.start)
            || !line.is_char_boundary(range.end)
        {
            return;
        }

        let range_start_col = line[..range.start].chars().count();
        let range_end_col = range_start_col + line[range].chars().count();
        let visible_start = range_start_col.max(visual_line.start_col);
        let visible_end = range_end_col.min(visual_line.end_col);

        if visible_start >= visible_end {
            return;
        }

        let prefix = Self::line_char_slice(line, visual_line.start_col, visible_start);
        let mut x_offset = UnicodeWidthStr::width(prefix);

        for ch in Self::line_char_slice(line, visible_start, visible_end).chars() {
            if x_offset >= area.width as usize {
                break;
            }
            let x = area.x + x_offset as u16;
            if let Some(cell) = buffer.cell_mut((x, y)) {
                if let Some(fg) = style.fg {
                    cell.set_fg(fg);
                } else {
                    cell.set_style(style);
                }
            }
            x_offset += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }

    fn next_large_paste_placeholder(&self, char_count: usize) -> String {
        let base = format!("[Pasted Content {char_count} chars]");
        let prefix = format!("{base} #");
        let mut max_suffix = 0usize;

        for paste in &self.pending_pastes {
            if paste.placeholder == base {
                max_suffix = max_suffix.max(1);
                continue;
            }
            if let Some(suffix) = paste.placeholder.strip_prefix(&prefix) {
                if let Ok(value) = suffix.parse::<usize>() {
                    max_suffix = max_suffix.max(value);
                }
            }
        }

        if max_suffix == 0 {
            base
        } else {
            format!("{base} #{}", max_suffix + 1)
        }
    }

    fn pending_paste_indices_by_placeholder_len(&self) -> Vec<usize> {
        let mut indices = (0..self.pending_pastes.len()).collect::<Vec<_>>();
        indices.sort_by(|&left, &right| {
            self.pending_pastes[right]
                .placeholder
                .len()
                .cmp(&self.pending_pastes[left].placeholder.len())
                .then_with(|| left.cmp(&right))
        });
        indices
    }

    fn pending_paste_match_at_offset(
        &self,
        text: &str,
        offset: usize,
        indices: &[usize],
        used_indices: &[usize],
    ) -> Option<usize> {
        indices.iter().copied().find(|idx| {
            !used_indices.contains(idx)
                && text[offset..].starts_with(&self.pending_pastes[*idx].placeholder)
        })
    }

    fn pending_paste_indices_in_text(&self, text: &str) -> Vec<usize> {
        let indices = self.pending_paste_indices_by_placeholder_len();
        let mut matched = Vec::new();
        let mut offset = 0;

        while offset < text.len() {
            if let Some(idx) = self.pending_paste_match_at_offset(text, offset, &indices, &matched)
            {
                matched.push(idx);
                offset += self.pending_pastes[idx].placeholder.len();
            } else if let Some(ch) = text[offset..].chars().next() {
                offset += ch.len_utf8();
            } else {
                break;
            }
        }

        matched
    }

    fn sync_pending_pastes(&mut self) {
        if self.pending_pastes.is_empty() {
            return;
        }

        let text = self.get_text();
        let matched = self.pending_paste_indices_in_text(&text);
        if matched.len() == self.pending_pastes.len()
            && matched.iter().copied().eq(0..self.pending_pastes.len())
        {
            return;
        }

        self.pending_pastes = matched
            .into_iter()
            .map(|idx| self.pending_pastes[idx].clone())
            .collect();
    }

    fn replace_pending_pastes(&self, text: &str) -> String {
        let indices = self.pending_paste_indices_by_placeholder_len();
        let mut expanded = String::with_capacity(text.len());
        let mut used_indices = Vec::new();
        let mut offset = 0;

        while offset < text.len() {
            if let Some(idx) =
                self.pending_paste_match_at_offset(text, offset, &indices, &used_indices)
            {
                let paste = &self.pending_pastes[idx];
                expanded.push_str(&paste.content);
                used_indices.push(idx);
                offset += paste.placeholder.len();
            } else if let Some(ch) = text[offset..].chars().next() {
                expanded.push(ch);
                offset += ch.len_utf8();
            } else {
                break;
            }
        }

        expanded
    }

    fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        let text = self.get_text();
        if range.start > range.end || range.end > text.len() {
            return;
        }
        let mut new_text = String::new();
        new_text.push_str(&text[..range.start]);
        new_text.push_str(replacement);
        new_text.push_str(&text[range.end..]);
        let cursor_offset = range.start + replacement.len();
        self.set_text_preserving_images(&new_text, cursor_offset);
        self.sync_image_placeholders();
        self.sync_pending_pastes();
    }

    fn quote_completion_path(path: &str) -> String {
        if path.chars().any(char::is_whitespace) {
            format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
        } else {
            path.to_string()
        }
    }

    fn remove_placeholder_at_cursor(&mut self, forward: bool) -> bool {
        let text = self.get_text();
        let cursor = self.flat_cursor_offset().min(text.len());
        let mut placeholders = self
            .local_images
            .iter()
            .map(|image| image.placeholder.as_str())
            .chain(
                self.pending_pastes
                    .iter()
                    .map(|paste| paste.placeholder.as_str()),
            )
            .collect::<Vec<_>>();
        placeholders.sort_by_key(|placeholder| std::cmp::Reverse(placeholder.len()));

        let target = placeholders.into_iter().find_map(|placeholder| {
            text.match_indices(placeholder).find_map(|(start, _)| {
                let end = start + placeholder.len();
                let should_remove = if forward {
                    cursor >= start && cursor < end
                } else {
                    cursor > start && cursor <= end
                };
                should_remove.then_some(start..end)
            })
        });

        if let Some(range) = target {
            self.replace_range(range, "");
            true
        } else {
            false
        }
    }

    fn image_at_offset(&self, offset: usize) -> Option<LocalImageAttachment> {
        let text = self.get_text();
        self.local_images.iter().find_map(|image| {
            text.match_indices(&image.placeholder)
                .any(|(start, _)| offset >= start && offset < start + image.placeholder.len())
                .then(|| image.clone())
        })
    }

    fn image_at_mouse_position(
        &self,
        textarea_area: Rect,
        mouse: MouseEvent,
    ) -> Option<LocalImageAttachment> {
        let relative_x = mouse.column.saturating_sub(textarea_area.x);
        let relative_y = mouse.row.saturating_sub(textarea_area.y);
        let (target_row, target_col) =
            self.cursor_for_screen_position(textarea_area, relative_x, relative_y)?;
        let offset = self.flat_offset_for_position(target_row, target_col);
        self.image_at_offset(offset)
    }

    pub fn attach_image(&mut self, path: PathBuf) {
        let placeholder = Self::image_placeholder(self.local_images.len() + 1);
        self.preferred_visual_col = None;
        self.textarea.insert_str(&placeholder);
        self.local_images
            .push(LocalImageAttachment { placeholder, path });
        self.sync_image_placeholders();
    }

    pub fn local_image_paths_for_submission(&mut self) -> Vec<PathBuf> {
        self.sync_image_placeholders();
        self.local_images
            .iter()
            .map(|image| image.path.clone())
            .collect()
    }

    fn sync_image_placeholders(&mut self) {
        if self.local_images.is_empty() {
            return;
        }

        let mut text = self.get_text();
        let mut kept = self
            .local_images
            .iter()
            .filter(|image| text.contains(&image.placeholder))
            .cloned()
            .collect::<Vec<_>>();

        if kept.len() == self.local_images.len()
            && kept
                .iter()
                .enumerate()
                .all(|(idx, image)| image.placeholder == Self::image_placeholder(idx + 1))
        {
            return;
        }

        let cursor = self.flat_cursor_offset().min(text.len());
        for (idx, image) in kept.iter_mut().enumerate() {
            let next_placeholder = Self::image_placeholder(idx + 1);
            if image.placeholder != next_placeholder {
                text = text.replacen(&image.placeholder, &next_placeholder, 1);
                image.placeholder = next_placeholder;
            }
        }

        self.local_images = kept;
        if let Some(hovered) = self.hovered_image_placeholder.as_deref() {
            if !self
                .local_images
                .iter()
                .any(|image| image.placeholder == hovered)
            {
                self.hovered_image_placeholder = None;
            }
        }
        self.set_text_preserving_images(&text, cursor);
    }

    pub fn apply_suggestion(&mut self, suggestion: &Suggestion) {
        match suggestion.kind {
            SuggestionKind::Command => {
                let replacement = format!("/{}", suggestion.replacement);
                let text = self.get_text();
                self.replace_range(0..text.len(), &replacement);
            }
            SuggestionKind::File => {
                let Some(token) = self.current_at_token(true) else {
                    return;
                };
                let path = PathBuf::from(&suggestion.replacement);
                if !suggestion.is_directory && image_attachment::is_supported_image_path(&path) {
                    let placeholder = Self::image_placeholder(self.local_images.len() + 1);
                    let replacement = format!("{placeholder} ");
                    self.replace_range(token.range, &replacement);
                    self.local_images
                        .push(LocalImageAttachment { placeholder, path });
                    self.sync_image_placeholders();
                } else {
                    let replacement =
                        format!("{} ", Self::quote_completion_path(&suggestion.replacement));
                    self.replace_range(token.range, &replacement);
                }
            }
        }
    }

    pub fn should_show_suggestions(&self) -> bool {
        self.command_query().is_some() || self.current_at_token(true).is_some()
    }

    pub fn is_slash_at_end(&self) -> bool {
        let text = self.get_text();
        text.trim_end() == "/"
    }

    pub fn complete_selection(&mut self, is_chat: bool) {
        if self.autocomplete.is_some() {
            if let Some(selected) = self.get_autocomplete_suggestions(is_chat).first().cloned() {
                self.apply_suggestion(&selected);
            }
        }
    }

    pub fn get_autocomplete_selection(&self, is_chat: bool) -> Option<String> {
        if let Some(autocomplete) = &self.autocomplete {
            let suggestions = if let Some(filter) = self.command_query() {
                autocomplete.command_auto.get_suggestions(&filter, is_chat)
            } else if let Some(token) = self.current_at_token(true) {
                autocomplete.file_auto.get_suggestions(&token.query)
            } else {
                Vec::new()
            };
            if !suggestions.is_empty() {
                return Some(suggestions[0].name.clone());
            }
        }
        None
    }

    pub fn get_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn submission_text(&self) -> String {
        self.replace_pending_pastes(&self.get_text())
    }

    pub fn is_empty(&self) -> bool {
        self.get_text().is_empty()
    }

    pub fn clear(&mut self) {
        self.reset_textarea();
        self.viewport_top = 0;
        self.preferred_visual_col = None;
        self.draft_text = None;
        self.local_images.clear();
        self.pending_pastes.clear();
        self.hovered_image_placeholder = None;
        if let Some(ref mut history) = self.prompt_history {
            history.reset_navigation();
        }
    }

    pub fn save_current_to_history(&mut self) {
        let text = self.submission_text();
        if !text.trim().is_empty() {
            if let Some(ref mut history) = self.prompt_history {
                let _ = history.add_prompt(&text);
            }
        }
        self.draft_text = None;
        if let Some(ref mut history) = self.prompt_history {
            history.reset_navigation();
        }
    }

    pub fn set_placeholder(&mut self, placeholder: &'static str) {
        self.textarea.set_placeholder_text(placeholder);
    }

    pub fn set_text(&mut self, text: &str) {
        self.reset_textarea();
        self.textarea.insert_str(text);
        self.viewport_top = 0;
        self.preferred_visual_col = None;
        self.local_images.clear();
        self.pending_pastes.clear();
        self.hovered_image_placeholder = None;
    }

    pub fn insert_char(&mut self, c: char) {
        self.preferred_visual_col = None;
        self.textarea.insert_str(c.to_string().as_str());
        self.sync_image_placeholders();
        self.sync_pending_pastes();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.preferred_visual_col = None;
        self.textarea.insert_str(text);
        self.sync_image_placeholders();
        self.sync_pending_pastes();
    }

    pub fn insert_paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let char_count = text.chars().count();

        if char_count > LARGE_PASTE_CHAR_THRESHOLD {
            self.sync_pending_pastes();
            let placeholder = self.next_large_paste_placeholder(char_count);
            self.preferred_visual_col = None;
            self.textarea.insert_str(&placeholder);
            self.pending_pastes.push(PendingPaste {
                placeholder,
                content: text,
            });
            self.sync_image_placeholders();
            return;
        }

        self.insert_str(&text);
    }

    pub fn get_autocomplete_suggestions(&self, is_chat: bool) -> Vec<Suggestion> {
        if let Some(autocomplete) = &self.autocomplete {
            if let Some(filter) = self.command_query() {
                return autocomplete.command_auto.get_suggestions(&filter, is_chat);
            }
            if let Some(token) = self.current_at_token(true) {
                return autocomplete.file_auto.get_suggestions(&token.query);
            }
        }
        Vec::new()
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use ratatui::style::Color;

    fn test_colors() -> ThemeColors {
        ThemeColors {
            primary: Color::Reset,
            secondary: Color::Reset,
            accent: Color::Yellow,
            interactive: Color::Reset,
            background: Color::Black,
            dialog_background: Color::Black,
            background_element: Color::Black,
            text: Color::White,
            text_weak: Color::Gray,
            text_strong: Color::White,
            border: Color::Gray,
            border_weak_focus: Color::Gray,
            border_focus: Color::Gray,
            border_strong_focus: Color::Gray,
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
            info: Color::Cyan,
            markdown_text: Color::White,
            markdown_heading: Color::Yellow,
            markdown_link: Color::Yellow,
            markdown_link_text: Color::Cyan,
            markdown_code: Color::Green,
            markdown_block_quote: Color::Gray,
            markdown_emph: Color::Yellow,
            markdown_strong: Color::Yellow,
            markdown_horizontal_rule: Color::Gray,
            markdown_list_item: Color::Yellow,
            markdown_list_enumeration: Color::Cyan,
            markdown_image: Color::Red,
            markdown_image_text: Color::Blue,
            markdown_code_block: Color::White,
            diff_add: Color::Green,
            diff_add_bg: Color::Green,
            diff_remove: Color::Red,
            diff_remove_bg: Color::Red,
            diff_gutter: Color::Gray,
        }
    }

    fn backspace_event() -> KeyEvent {
        KeyEvent {
            code: KeyCode::Backspace,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn modified_key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn mouse_event(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn mouse_event_at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn buffer_row_text(buffer: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
        (0..width)
            .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
            .collect()
    }

    fn find_buffer_text(
        buffer: &ratatui::buffer::Buffer,
        width: u16,
        height: u16,
        needle: &str,
    ) -> Option<(u16, u16)> {
        (0..height).find_map(|y| {
            let row = buffer_row_text(buffer, width, y);
            row.find(needle).map(|x| (x as u16, y))
        })
    }

    #[test]
    fn test_input_creation() {
        let input = Input::new();
        assert!(input.is_empty());
    }

    #[test]
    fn test_input_default() {
        let input = Input::default();
        assert!(input.is_empty());
    }

    #[test]
    fn test_input_get_text() {
        let input = Input::new();
        assert_eq!(input.get_text(), "");
    }

    #[test]
    fn test_input_clear() {
        let mut input = Input::new();
        input.set_placeholder("Test");
        input.clear();
        assert!(input.is_empty());
    }

    #[test]
    fn test_input_handle_event_return_true() {
        let mut input = Input::new();
        let event = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let handled = input.handle_event(event);
        assert!(handled);
    }

    #[test]
    fn test_input_handle_event_enter() {
        let mut input = Input::new();
        let event = KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let handled = input.handle_event(event);
        assert!(!handled);
    }

    #[test]
    fn test_input_handle_event_ctrl_c() {
        let mut input = Input::new();
        let event = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let handled = input.handle_event(event);
        assert!(!handled);
    }

    #[test]
    fn test_attach_image_inserts_placeholder() {
        let mut input = Input::new();
        let path = PathBuf::from("/tmp/example.png");

        input.attach_image(path.clone());

        assert_eq!(input.get_text(), "[Image #1]");
        assert_eq!(input.local_image_paths_for_submission(), vec![path]);
    }

    #[test]
    fn test_backspace_removes_image_placeholder() {
        let mut input = Input::new();
        input.attach_image(PathBuf::from("/tmp/example.png"));
        let event = backspace_event();

        let handled = input.handle_event(event);

        assert!(handled);
        assert_eq!(input.get_text(), "");
        assert!(input.local_image_paths_for_submission().is_empty());
    }

    #[test]
    fn test_large_paste_is_compacted_for_display() {
        let mut input = Input::new();
        let paste = "a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);

        input.insert_paste(&paste);

        assert_eq!(
            input.get_text(),
            format!("[Pasted Content {} chars]", LARGE_PASTE_CHAR_THRESHOLD + 1)
        );
        assert_eq!(input.submission_text(), paste);
    }

    #[test]
    fn test_threshold_sized_paste_stays_inline() {
        let mut input = Input::new();
        let paste = "a".repeat(LARGE_PASTE_CHAR_THRESHOLD);

        input.insert_paste(&paste);

        assert_eq!(input.get_text(), paste);
        assert_eq!(input.submission_text(), paste);
    }

    #[test]
    fn test_duplicate_large_paste_placeholders_are_unique() {
        let mut input = Input::new();
        let paste = "a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);

        input.insert_paste(&paste);
        input.insert_paste(&paste);

        assert_eq!(
            input.get_text(),
            format!(
                "[Pasted Content {} chars][Pasted Content {} chars] #2",
                LARGE_PASTE_CHAR_THRESHOLD + 1,
                LARGE_PASTE_CHAR_THRESHOLD + 1
            )
        );
        assert_eq!(input.submission_text(), format!("{paste}{paste}"));
    }

    #[test]
    fn test_large_paste_payload_is_pruned_after_placeholder_erasure() {
        let mut input = Input::new();
        let first = "a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);
        let second = "b".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);

        input.insert_paste(&first);
        assert!(input.handle_event(backspace_event()));
        input.insert_paste(&second);

        assert_eq!(
            input.get_text(),
            format!("[Pasted Content {} chars]", LARGE_PASTE_CHAR_THRESHOLD + 1)
        );
        assert_eq!(input.submission_text(), second);
    }

    #[test]
    fn test_large_paste_suffix_is_reused_after_latest_duplicate_erasure() {
        let mut input = Input::new();
        let paste = "a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);
        let base = format!("[Pasted Content {} chars]", LARGE_PASTE_CHAR_THRESHOLD + 1);
        let second = format!("{base} #2");

        input.insert_paste(&paste);
        input.insert_paste(&paste);
        assert_eq!(input.get_text(), format!("{base}{second}"));

        assert!(input.handle_event(backspace_event()));
        assert_eq!(input.get_text(), base);

        input.insert_paste(&paste);
        assert_eq!(input.get_text(), format!("{base}{second}"));
    }

    #[test]
    fn test_backspace_removes_large_paste_placeholder() {
        let mut input = Input::new();
        input.insert_paste(&"a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1));
        let event = backspace_event();

        let handled = input.handle_event(event);

        assert!(handled);
        assert_eq!(input.get_text(), "");
        assert_eq!(input.submission_text(), "");
    }

    #[test]
    fn test_long_unbroken_input_wraps_instead_of_scrolling() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut input = Input::new();
        input.insert_str("0123456789ABCDEF");

        let colors = test_colors();
        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                input.render(
                    frame,
                    Rect::new(0, 0, 20, 10),
                    "Plan",
                    "model",
                    "provider",
                    None,
                    &colors,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let first_input_row = buffer_row_text(buffer, 20, 1);
        let second_input_row = buffer_row_text(buffer, 20, 2);

        assert!(first_input_row.contains("0123456789ABCDE"));
        assert!(!first_input_row.contains('F'));
        assert!(second_input_row.contains('F'));
    }

    #[test]
    fn test_wrapped_input_and_paste_increase_height_like_newlines() {
        let mut newline_input = Input::new();
        newline_input.insert_str("a");
        assert!(newline_input.handle_event(modified_key_event(KeyCode::Enter, KeyModifiers::SHIFT)));
        newline_input.insert_str("b");

        let mut wrapped_input = Input::new();
        wrapped_input.insert_str("0123456789ABCDEF");

        let mut pasted_input = Input::new();
        pasted_input.insert_paste("0123456789ABCDEF");

        assert_eq!(newline_input.get_height_for_width(20), 6);
        assert_eq!(wrapped_input.get_height_for_width(20), 6);
        assert_eq!(pasted_input.get_height_for_width(20), 6);
    }

    #[test]
    fn test_up_down_move_across_wrapped_visual_lines() {
        let mut input = Input::new();
        input.insert_str("0123456789ABCDEF");
        input.textarea_area = Some(Rect::new(0, 0, 15, 6));

        input.textarea.move_cursor(CursorMove::Jump(0, 0));
        assert!(input.handle_event(key_event(KeyCode::Down)));
        assert_eq!(input.textarea.cursor(), (0, 15));
        assert_eq!(input.get_text(), "0123456789ABCDEF");

        assert!(input.handle_event(key_event(KeyCode::Up)));
        assert_eq!(input.textarea.cursor(), (0, 0));
        assert_eq!(input.get_text(), "0123456789ABCDEF");
    }

    #[test]
    fn test_mouse_scroll_moves_across_wrapped_visual_lines() {
        let mut input = Input::new();
        input.insert_str("0123456789ABCDEF");
        input.textarea_area = Some(Rect::new(0, 0, 15, 6));
        input.textarea.move_cursor(CursorMove::Jump(0, 0));

        assert!(input.handle_mouse_event(mouse_event(MouseEventKind::ScrollDown)));
        assert_eq!(input.textarea.cursor(), (0, 15));

        assert!(input.handle_mouse_event(mouse_event(MouseEventKind::ScrollUp)));
        assert_eq!(input.textarea.cursor(), (0, 0));
    }

    #[test]
    fn test_image_and_large_paste_placeholders_render_with_same_color() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut input = Input::new();
        input.attach_image(PathBuf::from("/tmp/example.png"));
        input.insert_str(" ");
        input.insert_paste(&"a".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1));

        let colors = test_colors();
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                input.render(
                    frame,
                    Rect::new(0, 0, 80, 6),
                    "Plan",
                    "model",
                    "provider",
                    None,
                    &colors,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let image_pos = find_buffer_text(buffer, 80, 6, "[Image #1]").expect("image placeholder");
        let paste_pos =
            find_buffer_text(buffer, 80, 6, "[Pasted Content").expect("paste placeholder");

        assert_eq!(
            buffer.cell(image_pos).expect("image cell").style().fg,
            Some(colors.markdown_image)
        );
        assert_eq!(
            buffer.cell(paste_pos).expect("paste cell").style().fg,
            Some(colors.markdown_image)
        );
    }

    #[test]
    fn test_hovered_image_placeholder_changes_foreground_only() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut input = Input::new();
        input.attach_image(PathBuf::from("/tmp/example.png"));

        let colors = test_colors();
        let backend = TestBackend::new(40, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                input.render(
                    frame,
                    Rect::new(0, 0, 40, 6),
                    "Plan",
                    "model",
                    "provider",
                    None,
                    &colors,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let image_pos = find_buffer_text(buffer, 40, 6, "[Image #1]").expect("image placeholder");
        let before_style = buffer.cell(image_pos).expect("image cell").style();
        assert_eq!(before_style.fg, Some(colors.markdown_image));

        assert!(input.handle_mouse_event(mouse_event_at(
            MouseEventKind::Moved,
            image_pos.0,
            image_pos.1
        )));

        terminal
            .draw(|frame| {
                input.render(
                    frame,
                    Rect::new(0, 0, 40, 6),
                    "Plan",
                    "model",
                    "provider",
                    None,
                    &colors,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let after_style = buffer.cell(image_pos).expect("image cell").style();
        assert_eq!(after_style.fg, Some(colors.markdown_image_text));
        assert_eq!(after_style.bg, before_style.bg);
    }
}
