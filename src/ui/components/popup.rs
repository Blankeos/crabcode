use crate::autocomplete::Suggestion;
use ratatui::{
    prelude::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem},
    Frame,
};

const MAX_VISIBLE_ITEMS: usize = 8;

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

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if !self.visible || self.suggestions.is_empty() {
            return;
        }

        let popup_width = area.width.min(60);
        let popup_height = (self.suggestions.len() as u16).min(MAX_VISIBLE_ITEMS as u16) + 2;

        let popup_area = Rect {
            x: area.x,
            y: area.y.saturating_sub(popup_height),
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, suggestion)| {
                let style = if i == self.selected_index {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let text = if !suggestion.description.is_empty() {
                    format!("{} - {}", suggestion.name, suggestion.description)
                } else {
                    suggestion.name.clone()
                };
                ListItem::new(Line::styled(text, style))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightGreen))
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
}
