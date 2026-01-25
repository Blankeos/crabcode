use crate::autocomplete::Suggestion;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    prelude::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem},
    Frame,
};

const MAX_VISIBLE_ITEMS: usize = 8;

pub enum PopupAction {
    Handled,
    Autocomplete,
    NotHandled,
}

pub struct Popup {
    pub suggestions: Vec<Suggestion>,
    pub selected_index: usize,
    pub visible: bool,
}

impl Popup {
    pub fn new() -> Self {
        Self {
            suggestions: Vec::new(),
            selected_index: 0,
            visible: false,
        }
    }

    pub fn set_suggestions(&mut self, suggestions: Vec<Suggestion>) {
        self.suggestions = suggestions;
        self.selected_index = 0;
        self.visible = !self.suggestions.is_empty();
    }

    pub fn clear(&mut self) {
        self.suggestions.clear();
        self.selected_index = 0;
        self.visible = false;
    }

    pub fn next(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.suggestions.len();
        }
    }

    pub fn previous(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.suggestions.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }

    pub fn get_selected(&self) -> Option<&Suggestion> {
        self.suggestions.get(self.selected_index)
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> PopupAction {
        if !self.visible {
            return PopupAction::NotHandled;
        }

        match event.code {
            KeyCode::Tab => PopupAction::Autocomplete,
            KeyCode::Up => {
                self.previous();
                PopupAction::Handled
            }
            KeyCode::Down => {
                self.next();
                PopupAction::Handled
            }
            KeyCode::Enter => {
                if !self.suggestions.is_empty() {
                    PopupAction::Autocomplete
                } else {
                    PopupAction::NotHandled
                }
            }
            KeyCode::Esc => {
                self.clear();
                PopupAction::Handled
            }
            _ => PopupAction::NotHandled,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, has_focus: bool) {
        if !self.visible || self.suggestions.is_empty() {
            return;
        }

        let popup_width = area.width;
        let popup_height = (self.suggestions.len() as u16).min(MAX_VISIBLE_ITEMS as u16) + 2;

        let popup_area = Rect {
            x: area.x,
            y: area.y.saturating_sub(popup_height).saturating_sub(2),
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(Clear, popup_area);

        let max_name_len = self
            .suggestions
            .iter()
            .map(|s| s.name.len())
            .max()
            .unwrap_or(0);

        use ratatui::text::Span;

        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, suggestion)| {
                let (bg_style, name_fg, desc_fg) = if i == self.selected_index {
                    (Color::Rgb(255, 200, 100), Color::Black, Color::Black)
                } else {
                    (Color::Reset, Color::White, Color::Rgb(150, 150, 150))
                };

                let name_style = Style::default()
                    .fg(name_fg)
                    .bg(bg_style)
                    .add_modifier(Modifier::BOLD);
                let desc_style = Style::default().fg(desc_fg).bg(bg_style);
                let padding_style = Style::default().bg(bg_style);

                let line = if !suggestion.description.is_empty() {
                    let mid_padding = " ".repeat(max_name_len + 3 - suggestion.name.len());
                    let content_len = suggestion.name.len()
                        + suggestion.description.len()
                        + mid_padding.len()
                        + 2;
                    let end_padding =
                        " ".repeat(popup_width.saturating_sub(content_len as u16).max(0) as usize);
                    Line::from(vec![
                        Span::styled(format!("/{}", suggestion.name), name_style),
                        Span::styled(mid_padding, padding_style),
                        Span::styled(suggestion.description.clone(), desc_style),
                        Span::styled(end_padding, padding_style),
                    ])
                } else {
                    let content_len = suggestion.name.len() + 1;
                    let end_padding =
                        " ".repeat(popup_width.saturating_sub(content_len as u16).max(0) as usize);
                    Line::from(vec![
                        Span::styled(format!("/{}", suggestion.name), name_style),
                        Span::styled(end_padding, padding_style),
                    ])
                };
                ListItem::new(line)
            })
            .collect();

        let border_style = if has_focus {
            Style::default().fg(Color::Rgb(255, 140, 0))
        } else {
            Style::default().fg(Color::Rgb(255, 200, 100))
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("Commands"),
        );

        frame.render_widget(list, popup_area);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn has_suggestions(&self) -> bool {
        !self.suggestions.is_empty()
    }
}

impl Default for Popup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_popup_creation() {
        let popup = Popup::new();
        assert!(!popup.is_visible());
        assert!(!popup.has_suggestions());
    }

    #[test]
    fn test_popup_default() {
        let popup = Popup::default();
        assert!(!popup.is_visible());
        assert!(!popup.has_suggestions());
    }

    #[test]
    fn test_set_suggestions() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion {
                name: "item1".to_string(),
                description: "desc1".to_string(),
            },
            Suggestion {
                name: "item2".to_string(),
                description: "desc2".to_string(),
            },
        ]);
        assert!(popup.is_visible());
        assert!(popup.has_suggestions());
        assert_eq!(popup.suggestions.len(), 2);
        assert_eq!(popup.selected_index, 0);
    }

    #[test]
    fn test_clear() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![Suggestion {
            name: "item1".to_string(),
            description: "desc1".to_string(),
        }]);
        popup.clear();
        assert!(!popup.is_visible());
        assert!(!popup.has_suggestions());
        assert_eq!(popup.suggestions.len(), 0);
    }

    #[test]
    fn test_next() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion {
                name: "item1".to_string(),
                description: "desc1".to_string(),
            },
            Suggestion {
                name: "item2".to_string(),
                description: "desc2".to_string(),
            },
            Suggestion {
                name: "item3".to_string(),
                description: "desc3".to_string(),
            },
        ]);
        popup.next();
        assert_eq!(popup.selected_index, 1);
        popup.next();
        assert_eq!(popup.selected_index, 2);
        popup.next();
        assert_eq!(popup.selected_index, 0);
    }

    #[test]
    fn test_previous() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion {
                name: "item1".to_string(),
                description: "desc1".to_string(),
            },
            Suggestion {
                name: "item2".to_string(),
                description: "desc2".to_string(),
            },
            Suggestion {
                name: "item3".to_string(),
                description: "desc3".to_string(),
            },
        ]);
        popup.previous();
        assert_eq!(popup.selected_index, 2);
        popup.previous();
        assert_eq!(popup.selected_index, 1);
    }

    #[test]
    fn test_get_selected() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion {
                name: "item1".to_string(),
                description: "desc1".to_string(),
            },
            Suggestion {
                name: "item2".to_string(),
                description: "desc2".to_string(),
            },
        ]);
        assert_eq!(popup.get_selected().map(|s| s.name.as_str()), Some("item1"));
        popup.next();
        assert_eq!(popup.get_selected().map(|s| s.name.as_str()), Some("item2"));
    }

    #[test]
    fn test_empty_suggestions() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![]);
        assert!(!popup.is_visible());
    }

    #[test]
    fn test_handle_key_event_not_visible() {
        let mut popup = Popup::new();
        let key = KeyEvent {
            code: KeyCode::Down,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::NotHandled));
    }

    #[test]
    fn test_handle_key_event_down() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion {
                name: "item1".to_string(),
                description: "desc1".to_string(),
            },
            Suggestion {
                name: "item2".to_string(),
                description: "desc2".to_string(),
            },
        ]);
        let key = KeyEvent {
            code: KeyCode::Down,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::Handled));
        assert_eq!(popup.selected_index, 1);
    }

    #[test]
    fn test_handle_key_event_up() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion {
                name: "item1".to_string(),
                description: "desc1".to_string(),
            },
            Suggestion {
                name: "item2".to_string(),
                description: "desc2".to_string(),
            },
        ]);
        let key = KeyEvent {
            code: KeyCode::Up,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::Handled));
        assert_eq!(popup.selected_index, 1);
    }

    #[test]
    fn test_handle_key_event_tab() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![Suggestion {
            name: "item1".to_string(),
            description: "desc1".to_string(),
        }]);
        let key = KeyEvent {
            code: KeyCode::Tab,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::Autocomplete));
    }

    #[test]
    fn test_handle_key_event_esc() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![Suggestion {
            name: "item1".to_string(),
            description: "desc1".to_string(),
        }]);
        let key = KeyEvent {
            code: KeyCode::Esc,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::Handled));
        assert!(!popup.is_visible());
    }

    #[test]
    fn test_handle_key_event_char() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![Suggestion {
            name: "item1".to_string(),
            description: "desc1".to_string(),
        }]);
        let key = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::NotHandled));
    }
}
