use crate::autocomplete::{AutoComplete, Suggestion};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::{Rect, Style};
use ratatui::widgets::{Block, Paragraph};
use tui_textarea::{Input as TuiInput, TextArea};

pub struct Input {
    textarea: TextArea<'static>,
    pub autocomplete: Option<AutoComplete>,
}

impl Input {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        Self {
            textarea,
            autocomplete: None,
        }
    }

    pub fn with_autocomplete(mut self, autocomplete: AutoComplete) -> Self {
        self.autocomplete = Some(autocomplete);
        self
    }

    pub fn render(&self, frame: &mut ratatui::Frame, area: Rect, agent: &str, model: &str) {
        let agent_color = if agent == "Plan" {
            ratatui::style::Color::Rgb(255, 165, 0)
        } else {
            ratatui::style::Color::Rgb(147, 112, 219)
        };

        let border = Block::bordered()
            .borders(ratatui::widgets::Borders::LEFT)
            .border_style(ratatui::style::Style::default().fg(agent_color))
            .border_type(ratatui::widgets::BorderType::Thick)
            .padding(ratatui::widgets::Padding::horizontal(1));
        let inner_area = border.inner(area);

        let line_count = self.textarea.lines().len().max(1);
        let textarea_height = line_count.min(6) as u16;

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(textarea_height),
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(inner_area);

        frame.render_widget(&self.textarea, chunks[1]);

        let info_text = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                agent.to_string(),
                ratatui::style::Style::default().fg(agent_color),
            ),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(
                model.to_string(),
                ratatui::style::Style::default().fg(ratatui::style::Color::Rgb(255, 200, 100)),
            ),
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(
                "OpenAI",
                ratatui::style::Style::default().fg(ratatui::style::Color::Yellow),
            ),
        ]);

        let info_paragraph = Paragraph::new(info_text);
        frame.render_widget(info_paragraph, chunks[3]);
        frame.render_widget(border, area);
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
            return false;
        }

        match event.code {
            KeyCode::Char('j') if event.modifiers == KeyModifiers::CONTROL => {
                self.textarea.insert_newline();
                true
            }
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => false,
            KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => {
                let (cursor_row, cursor_col) = self.textarea.cursor();
                if let Some(lines) = self.textarea.lines().get(cursor_row) {
                    let before_cursor = &lines[..cursor_col.min(lines.len())];
                    for _ in 0..before_cursor.chars().count() {
                        self.textarea.delete_char();
                    }
                }
                true
            }
            KeyCode::Tab => false,
            KeyCode::Esc => false,
            _ => {
                self.textarea.input(input);
                true
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

    pub fn complete_selection(&mut self) {
        if let Some(selected) = self.get_autocomplete_selection() {
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

    pub fn get_autocomplete_selection(&self) -> Option<String> {
        if let Some(autocomplete) = &self.autocomplete {
            let text = self.get_text();
            let suggestions = if text.starts_with('/') {
                let filter = text.trim_start_matches('/');
                autocomplete.get_suggestions(filter)
            } else {
                autocomplete.get_suggestions(&text)
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
    }

    pub fn set_placeholder(&mut self, placeholder: &'static str) {
        self.textarea.set_placeholder_text(placeholder);
    }

    pub fn set_text(&mut self, text: &str) {
        self.textarea = TextArea::default();
        self.textarea.insert_str(text);
    }

    pub fn insert_char(&mut self, c: char) {
        self.textarea.insert_str(c.to_string().as_str());
    }

    pub fn get_autocomplete_suggestions(&self) -> Vec<Suggestion> {
        if let Some(autocomplete) = &self.autocomplete {
            let text = self.get_text();
            if text.starts_with('/') {
                let filter = text.trim_start_matches('/');
                return autocomplete.get_suggestions(filter);
            } else {
                return autocomplete.get_suggestions(&text);
            }
        }
        Vec::new()
    }

    pub fn get_height(&self) -> u16 {
        let line_count = self.textarea.lines().len().max(1);
        let textarea_height = line_count.min(6) as u16;
        textarea_height + 3
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
