use crate::autocomplete::{AutoComplete, Suggestion};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{prelude::Rect, widgets::Block};
use tui_textarea::{Input as TuiInput, TextArea};

pub struct Input {
    textarea: TextArea<'static>,
    pub autocomplete: Option<AutoComplete>,
}

impl Input {
    pub fn new() -> Self {
        let textarea = TextArea::default();
        Self {
            textarea,
            autocomplete: None,
        }
    }

    pub fn with_autocomplete(mut self, autocomplete: AutoComplete) -> Self {
        self.autocomplete = Some(autocomplete);
        self
    }

    pub fn render(&self, frame: &mut ratatui::Frame, area: Rect) {
        let block = Block::bordered();
        let inner_area = block.inner(area);
        frame.render_widget(&self.textarea, inner_area);
        frame.render_widget(block, area);
    }

    pub fn handle_event(&mut self, event: KeyEvent) -> bool {
        let input = TuiInput::from(event);
        match event.code {
            KeyCode::Enter if event.modifiers == KeyModifiers::NONE => false,
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => false,
            KeyCode::Tab => true,
            KeyCode::Up | KeyCode::Down => false,
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
    }

    pub fn set_placeholder(&mut self, placeholder: &'static str) {
        self.textarea.set_placeholder_text(placeholder);
    }

    pub fn set_text(&mut self, text: &str) {
        self.textarea = TextArea::default();
        self.textarea.insert_str(text);
    }

    pub fn insert_char(&mut self, c: char) {
        self.textarea.insert_str(&c.to_string());
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
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
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
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
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
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        let handled = input.handle_event(event);
        assert!(!handled);
    }
}
