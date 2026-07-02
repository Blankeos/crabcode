use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::theme::{contrast_text, ThemeColors};
use crate::ui::textarea_keys::{has_command_modifier, input_textarea};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindBarAction {
    None,
    CommitSearch,
    Next,
    Previous,
    Close,
}

#[derive(Debug)]
pub struct FindBar {
    textarea: TextArea<'static>,
    pub active: bool,
    editing: bool,
    match_count: usize,
    active_index: Option<usize>,
    last_committed_query: String,
}

impl FindBar {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
        Self {
            textarea,
            active: false,
            editing: true,
            match_count: 0,
            active_index: None,
            last_committed_query: String::new(),
        }
    }

    pub fn show(&mut self) {
        self.active = true;
        self.editing = true;
        self.match_count = 0;
        self.active_index = None;
        self.last_committed_query.clear();
    }

    pub fn close(&mut self) {
        self.active = false;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn query(&self) -> String {
        self.textarea.lines().join("")
    }

    pub fn committed_query(&self) -> &str {
        &self.last_committed_query
    }

    pub fn clear_matches(&mut self) {
        self.match_count = 0;
        self.active_index = None;
    }

    pub fn set_match_status(&mut self, match_count: usize, active_index: Option<usize>) {
        self.match_count = match_count;
        self.active_index = active_index.filter(|idx| *idx < match_count);
    }

    pub fn commit_current_query(&mut self) {
        self.last_committed_query = self.query();
        self.editing = false;
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> FindBarAction {
        if !self.active {
            return FindBarAction::None;
        }

        match event.code {
            KeyCode::Esc => return FindBarAction::Close,
            KeyCode::Enter if event.modifiers == KeyModifiers::NONE => {
                if self.editing {
                    self.commit_current_query();
                    return FindBarAction::CommitSearch;
                }
                return FindBarAction::Next;
            }
            KeyCode::Enter if !self.editing && event.modifiers == KeyModifiers::SHIFT => {
                return FindBarAction::Previous;
            }
            KeyCode::Char('n') if !self.editing && event.modifiers == KeyModifiers::NONE => {
                return FindBarAction::Next;
            }
            KeyCode::Char('N') if !self.editing => {
                return FindBarAction::Previous;
            }
            KeyCode::Char('n')
                if !self.editing && event.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                return FindBarAction::Previous;
            }
            KeyCode::Left if event.modifiers.contains(KeyModifiers::ALT) => {
                self.editing = true;
                self.textarea
                    .move_cursor(tui_textarea::CursorMove::WordBack);
                return FindBarAction::None;
            }
            KeyCode::Right if event.modifiers.contains(KeyModifiers::ALT) => {
                self.editing = true;
                self.textarea
                    .move_cursor(tui_textarea::CursorMove::WordForward);
                return FindBarAction::None;
            }
            KeyCode::Backspace if event.modifiers.contains(KeyModifiers::ALT) => {
                self.editing = true;
                self.textarea.delete_word();
                return FindBarAction::None;
            }
            KeyCode::Left if has_command_modifier(event.modifiers) => {
                self.editing = true;
                self.textarea.move_cursor(tui_textarea::CursorMove::Head);
                return FindBarAction::None;
            }
            KeyCode::Right if has_command_modifier(event.modifiers) => {
                self.editing = true;
                self.textarea.move_cursor(tui_textarea::CursorMove::End);
                return FindBarAction::None;
            }
            KeyCode::Backspace if has_command_modifier(event.modifiers) => {
                self.editing = true;
                self.textarea.delete_line_by_head();
                return FindBarAction::None;
            }
            _ => {}
        }

        self.editing = true;
        let _ = input_textarea(&mut self.textarea, event);
        FindBarAction::None
    }

    pub fn insert_text(&mut self, text: &str) {
        self.editing = true;
        let single_line = text.replace(['\r', '\n'], " ");
        self.textarea.insert_str(&single_line);
    }

    pub fn render(&mut self, f: &mut Frame, frame_area: Rect, colors: &ThemeColors) {
        if !self.active || frame_area.width == 0 || frame_area.height == 0 {
            return;
        }

        let query = self.query();
        let query_width = UnicodeWidthStr::width(query.as_str()) as u16;
        let status = self.status_text();
        let status_width = UnicodeWidthStr::width(status.as_str()) as u16;
        let desired_width = 18u16
            .saturating_add(query_width)
            .saturating_add(status_width)
            .min(54);
        let min_width = 26u16.min(frame_area.width);
        let width = desired_width.max(min_width).min(frame_area.width);
        let height = 3u16.min(frame_area.height);
        if width == 0 || height == 0 {
            return;
        }

        let x = frame_area
            .x
            .saturating_add(frame_area.width.saturating_sub(width));
        let y = frame_area.y;
        let area = Rect::new(x, y, width, height);
        f.render_widget(Clear, area);

        let bg = colors.dialog_background;
        let border = if self.match_count == 0 && !query.is_empty() {
            colors.warning
        } else {
            colors.border_focus
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border).bg(bg))
            .style(Style::default().bg(bg));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let label = "find ";
        let status_reserved = status_width.saturating_add(1).min(inner.width);
        let input_width = inner
            .width
            .saturating_sub(label.len() as u16)
            .saturating_sub(status_reserved);
        let display_query = truncate_start_to_width(&query, input_width as usize);

        let mut spans = vec![
            Span::styled(
                label,
                Style::default()
                    .fg(colors.info)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                display_query.clone(),
                Style::default().fg(colors.text_strong).bg(bg),
            ),
        ];

        let current_width = spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum::<usize>() as u16;
        let gap_width = inner
            .width
            .saturating_sub(current_width)
            .saturating_sub(status_width);
        if gap_width > 0 {
            spans.push(Span::styled(
                " ".repeat(gap_width as usize),
                Style::default().bg(bg),
            ));
        }
        spans.push(Span::styled(
            status,
            Style::default().fg(colors.text_weak).bg(bg),
        ));

        let paragraph = Paragraph::new(Line::from(spans)).alignment(Alignment::Left);
        f.render_widget(paragraph, inner);

        let cursor_col = label.len() as u16 + UnicodeWidthStr::width(display_query.as_str()) as u16;
        if cursor_col < inner.width.saturating_sub(status_width) {
            f.set_cursor_position((inner.x.saturating_add(cursor_col), inner.y));
        }
    }

    fn status_text(&self) -> String {
        if self.editing || self.last_committed_query.is_empty() {
            return String::new();
        }
        match (self.match_count, self.active_index) {
            (0, _) => "0/0".to_string(),
            (count, Some(idx)) => format!("{}/{}", idx + 1, count),
            (count, None) => format!("0/{count}"),
        }
    }
}

impl Default for FindBar {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate_start_to_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }

    if max_width == 0 {
        return String::new();
    }

    let ellipsis = "…";
    if max_width == 1 {
        return ellipsis.to_string();
    }

    let keep_width = max_width.saturating_sub(1);
    let mut kept = Vec::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > keep_width {
            break;
        }
        width += ch_width;
        kept.push(ch);
    }
    kept.reverse();
    format!("{ellipsis}{}", kept.into_iter().collect::<String>())
}

pub fn find_match_fg(colors: &ThemeColors) -> ratatui::style::Color {
    contrast_text(colors.warning)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn typing_n_edits_query_until_search_is_committed() {
        let mut find = FindBar::new();
        find.show();

        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            FindBarAction::None
        );
        assert_eq!(find.query(), "n");

        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)),
            FindBarAction::None
        );
        for ch in ['n', 'e'] {
            assert_eq!(
                find.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                FindBarAction::None
            );
        }
        assert_eq!(find.query(), "none");

        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FindBarAction::CommitSearch
        );
        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            FindBarAction::Next
        );

        find.show();
        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            FindBarAction::None
        );
        assert_eq!(find.query(), "nonen");
    }

    #[test]
    fn enter_repeats_next_match_after_search_is_committed() {
        let mut find = FindBar::new();
        find.show();

        for ch in ['f', 'i', 'n', 'd'] {
            assert_eq!(
                find.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                FindBarAction::None
            );
        }

        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FindBarAction::CommitSearch
        );
        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            FindBarAction::Next
        );
        assert_eq!(
            find.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            FindBarAction::Previous
        );
    }
}
