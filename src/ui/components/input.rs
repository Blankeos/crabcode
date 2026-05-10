use crate::autocomplete::{AutoComplete, Suggestion};
use crate::persistence::PromptHistoryCache;
use crate::theme::{agent_color, ThemeColors};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::prelude::{Rect, Style};
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_textarea::{CursorMove, Input as TuiInput, TextArea};
use unicode_width::UnicodeWidthChar;

/// Convert a display-column position to a byte offset within a string.
/// Handles multi-byte and wide characters (emoji, CJK, etc.)
fn display_col_to_byte_offset(line: &str, display_col: usize) -> usize {
    let mut current_display = 0;

    for (byte_idx, c) in line.char_indices() {
        let char_width = UnicodeWidthChar::width(c).unwrap_or(1);
        if display_col < current_display + char_width {
            return byte_idx;
        }
        current_display += char_width;
    }

    line.len()
}

/// Clamp a byte offset to the nearest valid UTF-8 character boundary in `s`.
fn char_boundary_before(s: &str, byte_idx: usize) -> usize {
    let idx = byte_idx.min(s.len());
    if s.is_char_boundary(idx) {
        idx
    } else {
        (0..idx)
            .rev()
            .find(|&i| s.is_char_boundary(i))
            .unwrap_or(0)
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

pub struct Input {
    textarea: TextArea<'static>,
    pub autocomplete: Option<AutoComplete>,
    textarea_area: Option<Rect>,
    viewport_top: usize,
    prompt_history: Option<PromptHistoryCache>,
    draft_text: Option<String>,
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
            prompt_history,
            draft_text: None,
        }
    }

    pub fn with_autocomplete(mut self, autocomplete: AutoComplete) -> Self {
        self.autocomplete = Some(autocomplete);
        self
    }

pub fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        area: Rect,
        agent: &str,
        model: &str,
        provider_name: &str,
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

        let line_count = self.textarea.lines().len().max(1);
        let textarea_height = line_count.min(6) as u16;

        let h_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Length(2),
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(2),
            ])
            .split(inner_area);

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

        self.textarea.set_selection_style(
            Style::default()
                .bg(colors.accent)
                .fg(colors.text),
        );
        self.textarea
            .set_style(Style::default().fg(colors.text).bg(colors.background_element));

        let line_count = self.textarea.lines().len();
        let visible_lines = v_chunks[1].height as usize;
        let max_viewport_top = line_count.saturating_sub(visible_lines);
        self.viewport_top = self.viewport_top.min(max_viewport_top);

        frame.render_widget(&self.textarea, v_chunks[1]);

        let info_text = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                agent.to_string(),
                Style::default().fg(agent_color),
            ),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(
                model.to_string(),
                Style::default().fg(colors.text),
            ),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(
                provider_name.to_string(),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(ratatui::style::Modifier::DIM),
            ),
        ]);

        let info_paragraph = Paragraph::new(info_text);
        frame.render_widget(info_paragraph, v_chunks[3]);

        frame.render_widget(border, area);

        let cap_row = Paragraph::new(ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                "╹",
                Style::default().fg(agent_color),
            ),
            ratatui::text::Span::styled(
                "▀".repeat(area.width as usize - 1),
                Style::default().fg(colors.background_element),
            ),
        ]));
        let cap_row_area = Rect::new(area.x, v_chunks[4].y, area.width, 1);
        frame.render_widget(cap_row, cap_row_area);
    }

    pub fn get_height(&self) -> u16 {
        let line_count = self.textarea.lines().len().max(1);
        let textarea_height = line_count.min(6) as u16;
        textarea_height + 4
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
            return true;
        }

        // Fallback: Alt+Enter for terminals where Shift+Enter doesn't work
        if event.code == KeyCode::Enter && event.modifiers.contains(KeyModifiers::ALT) {
            self.textarea.insert_newline();
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
            let (cursor_row, _) = self.textarea.cursor();
            if cursor_row == 0 {
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
            let line_count = self.textarea.lines().len();
            let (cursor_row, _) = self.textarea.cursor();
            if cursor_row == line_count.saturating_sub(1) {
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
                self.textarea.insert_newline();
                true
            }
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => false,
            KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => {
                let (cursor_row, cursor_col) = self.textarea.cursor();
                if let Some(line) = self.textarea.lines().get(cursor_row) {
                    // Clamp to valid char boundary to avoid panics on multi-byte emoji
                    let safe_col = char_boundary_before(line, cursor_col);
                    let before_cursor = &line[..safe_col];
                    for _ in 0..before_cursor.chars().count() {
                        self.textarea.delete_char();
                    }
                }
                true
            }
            KeyCode::Tab => false,
            KeyCode::Esc => false,
            KeyCode::Backspace if event.modifiers.contains(KeyModifiers::ALT) => {
                // Handle Alt+Backspace (word-delete) ourselves to avoid
                // tui-textarea's buggy word boundary with multi-byte emoji
                self.delete_word_backward();
                true
            }
            _ => {
                self.textarea.input(input);
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
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                let line_count = self.textarea.lines().len();
                let visible_lines = textarea_area.height as usize;

                if line_count > visible_lines {
                    let max_viewport_top = line_count.saturating_sub(visible_lines);
                    if self.viewport_top < max_viewport_top {
                        self.viewport_top += 1;
                        let target_row = self.viewport_top + visible_lines - 1;
                        let (_, cursor_col) = self.textarea.cursor();
                        self.textarea
                            .move_cursor(CursorMove::Jump(target_row as u16, cursor_col as u16));
                    }
                }
                true
            }
            MouseEventKind::ScrollUp => {
                let line_count = self.textarea.lines().len();
                let visible_lines = textarea_area.height as usize;

                if line_count > visible_lines {
                    if self.viewport_top > 0 {
                        self.viewport_top -= 1;
                        let target_row = self.viewport_top;
                        let (_, cursor_col) = self.textarea.cursor();
                        self.textarea
                            .move_cursor(CursorMove::Jump(target_row as u16, cursor_col as u16));
                    }
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let relative_x = mouse_x.saturating_sub(textarea_area.x);
                let relative_y = mouse_y.saturating_sub(textarea_area.y);

                let lines = self.textarea.lines();
                let target_row = self.viewport_top + relative_y as usize;

                if target_row < lines.len() {
                    let line = &lines[target_row];
                    let target_col = display_col_to_byte_offset(line, relative_x as usize);
                    // Position cursor and start selection for potential drag
                    self.textarea
                        .move_cursor(CursorMove::Jump(target_row as u16, target_col as u16));
                    self.textarea.start_selection();
                } else {
                    let last_row = lines.len().saturating_sub(1);
                    let last_col = lines[last_row].len();
                    self.textarea
                        .move_cursor(CursorMove::Jump(last_row as u16, last_col as u16));
                    self.textarea.start_selection();
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Extend the ongoing selection
                let relative_x = mouse_x.saturating_sub(textarea_area.x);
                let relative_y = mouse_y.saturating_sub(textarea_area.y);

                let lines = self.textarea.lines();
                let target_row = self.viewport_top + relative_y as usize;

                if target_row < lines.len() {
                    let line = &lines[target_row];
                    let target_col = display_col_to_byte_offset(line, relative_x as usize);
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
            let start = if i == start_row { start_col.min(line.len()) } else { 0 };
            let end = if i == end_row { end_col.min(line.len()) } else { line.len() };

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

    pub fn should_show_suggestions(&self) -> bool {
        let text = self.get_text();
        !text.is_empty() && text.starts_with('/')
    }

    pub fn is_slash_at_end(&self) -> bool {
        let text = self.get_text();
        text.trim_end() == "/"
    }

    pub fn complete_selection(&mut self, is_chat: bool) {
        if let Some(selected) = self.get_autocomplete_selection(is_chat) {
            let current_text = self.get_text();
            let start_index = current_text.rfind('/').map_or(0, |i| i + 1);

            let new_text = if start_index == 0 {
                selected.clone()
            } else {
                format!("{}{}", &current_text[..start_index], selected)
            };

            self.set_text(&new_text);
        }
    }

    pub fn get_autocomplete_selection(&self, is_chat: bool) -> Option<String> {
        if let Some(autocomplete) = &self.autocomplete {
            let text = self.get_text();
            let suggestions = if text.starts_with('/') {
                let filter = text.trim_start_matches('/');
                autocomplete.get_suggestions(filter, is_chat)
            } else {
                autocomplete.get_suggestions(&text, is_chat)
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

    pub fn is_empty(&self) -> bool {
        self.get_text().is_empty()
    }

    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_selection_style(
            Style::default()
                .bg(ratatui::style::Color::Rgb(255, 140, 0))
                .fg(ratatui::style::Color::Reset),
        );
        self.viewport_top = 0;
        self.draft_text = None;
        if let Some(ref mut history) = self.prompt_history {
            history.reset_navigation();
        }
    }

    pub fn save_current_to_history(&mut self) {
        let text = self.get_text();
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
        self.textarea = TextArea::default();
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_selection_style(
            Style::default()
                .bg(ratatui::style::Color::Rgb(255, 140, 0))
                .fg(ratatui::style::Color::Reset),
        );
        self.textarea.insert_str(text);
        self.viewport_top = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        self.textarea.insert_str(c.to_string().as_str());
    }

    pub fn insert_str(&mut self, text: &str) {
        self.textarea.insert_str(text);
    }

    pub fn get_autocomplete_suggestions(&self, is_chat: bool) -> Vec<Suggestion> {
        if let Some(autocomplete) = &self.autocomplete {
            let text = self.get_text();
            if text.starts_with('/') {
                let filter = text.trim_start_matches('/');
                return autocomplete.get_suggestions(filter, is_chat);
            } else {
                return autocomplete.get_suggestions(&text, is_chat);
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
}
