use crate::autocomplete::{Suggestion, SuggestionKind};
use crate::theme::{contrast_text, ThemeColors};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    prelude::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem},
    Frame,
};
use std::ops::Range;

const MAX_VISIBLE_ITEMS: usize = 8;
const ITEM_HORIZONTAL_PADDING: usize = 1;

pub enum PopupAction {
    Handled,
    Autocomplete,
    NotHandled,
}

pub struct Popup {
    pub suggestions: Vec<Suggestion>,
    pub selected_index: usize,
    pub visible: bool,
    scroll_offset: usize,
}

impl Popup {
    pub fn new() -> Self {
        Self {
            suggestions: Vec::new(),
            selected_index: 0,
            visible: false,
            scroll_offset: 0,
        }
    }

    pub fn set_suggestions(&mut self, suggestions: Vec<Suggestion>) {
        self.suggestions = suggestions;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.visible = !self.suggestions.is_empty();
    }

    pub fn clear(&mut self) {
        self.suggestions.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.visible = false;
    }

    pub fn next(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.suggestions.len();
            self.keep_selected_visible();
        }
    }

    pub fn previous(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.suggestions.len() - 1
            } else {
                self.selected_index - 1
            };
            self.keep_selected_visible();
        }
    }

    pub fn get_selected(&self) -> Option<&Suggestion> {
        self.suggestions.get(self.selected_index)
    }

    fn popup_area(&self, area: Rect) -> Option<Rect> {
        if !self.visible || self.suggestions.is_empty() {
            return None;
        }

        let popup_height = (self.visible_range().len() as u16) + 2;

        Some(Rect {
            x: area.x,
            y: area.y.saturating_sub(popup_height).saturating_sub(3),
            width: area.width,
            height: popup_height,
        })
    }

    fn visible_range(&self) -> Range<usize> {
        let item_count = self.suggestions.len();
        if item_count == 0 {
            return 0..0;
        }

        let visible_count = item_count.min(MAX_VISIBLE_ITEMS);
        let max_start = item_count.saturating_sub(visible_count);
        let start = self.scroll_offset.min(max_start);

        start..start + visible_count
    }

    fn keep_selected_visible(&mut self) {
        if self.suggestions.is_empty() {
            self.scroll_offset = 0;
            return;
        }

        let visible_count = self.suggestions.len().min(MAX_VISIBLE_ITEMS);
        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset + visible_count {
            self.scroll_offset = self.selected_index + 1 - visible_count;
        }
    }

    fn scroll_down(&mut self) {
        let visible_count = self.suggestions.len().min(MAX_VISIBLE_ITEMS);
        let max_start = self.suggestions.len().saturating_sub(visible_count);
        self.scroll_offset = self.scroll_offset.saturating_add(1).min(max_start);
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn item_index_at(&self, area: Rect, position: Position) -> Option<usize> {
        let popup_area = self.popup_area(area)?;
        if !popup_area.contains(position)
            || position.x <= popup_area.x
            || position.x
                >= popup_area
                    .x
                    .saturating_add(popup_area.width)
                    .saturating_sub(1)
        {
            return None;
        }

        let relative_y = position.y.saturating_sub(popup_area.y);
        if relative_y == 0 || relative_y >= popup_area.height.saturating_sub(1) {
            return None;
        }

        let visible_range = self.visible_range();
        let item_offset = (relative_y - 1) as usize;
        if item_offset >= visible_range.len() {
            return None;
        }

        Some(visible_range.start + item_offset)
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

    pub fn handle_mouse_event(&mut self, event: MouseEvent, area: Rect) -> PopupAction {
        if !self.visible || self.suggestions.is_empty() {
            return PopupAction::NotHandled;
        }

        let position = Position::new(event.column, event.row);
        let Some(popup_area) = self.popup_area(area) else {
            return PopupAction::NotHandled;
        };

        match event.kind {
            MouseEventKind::ScrollDown if popup_area.contains(position) => {
                self.scroll_down();
                PopupAction::Handled
            }
            MouseEventKind::ScrollUp if popup_area.contains(position) => {
                self.scroll_up();
                PopupAction::Handled
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.item_index_at(area, position) {
                    self.selected_index = index;
                    PopupAction::Autocomplete
                } else if popup_area.contains(position) {
                    PopupAction::Handled
                } else {
                    PopupAction::NotHandled
                }
            }
            MouseEventKind::Moved => {
                if let Some(index) = self.item_index_at(area, position) {
                    self.selected_index = index;
                    PopupAction::Handled
                } else {
                    PopupAction::NotHandled
                }
            }
            _ => PopupAction::NotHandled,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, has_focus: bool, colors: ThemeColors) {
        if !self.visible || self.suggestions.is_empty() {
            return;
        }

        let popup_width = area.width;
        let item_width = popup_width.saturating_sub(2) as usize;
        let visible_range = self.visible_range();
        let Some(popup_area) = self.popup_area(area) else {
            return;
        };

        frame.render_widget(Clear, popup_area);

        let max_name_len = self
            .suggestions
            .iter()
            .map(|s| s.display_prefix().len() + s.name.len())
            .max()
            .unwrap_or(0);
        let title = if self
            .suggestions
            .first()
            .map(|s| s.kind == SuggestionKind::File)
            .unwrap_or(false)
        {
            "Files"
        } else {
            "Commands"
        };

        use ratatui::text::Span;

        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .enumerate()
            .skip(visible_range.start)
            .take(visible_range.len())
            .map(|(i, suggestion)| {
                let (bg_style, name_fg, desc_fg) = if i == self.selected_index {
                    let fg = contrast_text(colors.primary);
                    (colors.primary, fg, fg)
                } else {
                    (Color::Reset, Color::White, Color::Rgb(150, 150, 150))
                };

                let name_style = Style::default()
                    .fg(name_fg)
                    .bg(bg_style)
                    .add_modifier(Modifier::BOLD);
                let desc_style = Style::default().fg(desc_fg).bg(bg_style);
                let padding_style = Style::default().bg(bg_style);
                let left_padding = " ".repeat(ITEM_HORIZONTAL_PADDING);
                let right_padding = " ".repeat(ITEM_HORIZONTAL_PADDING);

                let display_name = format!("{}{}", suggestion.display_prefix(), suggestion.name);
                let display_name_len = display_name.len();

                let line = if !suggestion.description.is_empty() {
                    let mid_padding = " ".repeat(max_name_len + 3 - display_name_len);
                    let content_len = display_name_len
                        + suggestion.description.len()
                        + mid_padding.len()
                        + ITEM_HORIZONTAL_PADDING
                        + ITEM_HORIZONTAL_PADDING;
                    let end_padding = " ".repeat(item_width.saturating_sub(content_len));
                    Line::from(vec![
                        Span::styled(left_padding, padding_style),
                        Span::styled(display_name, name_style),
                        Span::styled(mid_padding, padding_style),
                        Span::styled(suggestion.description.clone(), desc_style),
                        Span::styled(end_padding, padding_style),
                        Span::styled(right_padding, padding_style),
                    ])
                } else {
                    let content_len =
                        display_name_len + ITEM_HORIZONTAL_PADDING + ITEM_HORIZONTAL_PADDING;
                    let end_padding = " ".repeat(item_width.saturating_sub(content_len));
                    Line::from(vec![
                        Span::styled(left_padding, padding_style),
                        Span::styled(display_name, name_style),
                        Span::styled(end_padding, padding_style),
                        Span::styled(right_padding, padding_style),
                    ])
                };
                ListItem::new(line)
            })
            .collect();

        let border_style = if has_focus {
            Style::default().fg(colors.border_focus)
        } else {
            Style::default().fg(colors.border_weak_focus)
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
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

    fn suggestion(name: &str, description: &str) -> Suggestion {
        Suggestion::command(name, description)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
        }
    }

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
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
        ]);
        assert!(popup.is_visible());
        assert!(popup.has_suggestions());
        assert_eq!(popup.suggestions.len(), 2);
        assert_eq!(popup.selected_index, 0);
        assert_eq!(popup.scroll_offset, 0);
    }

    #[test]
    fn test_clear() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![suggestion("item1", "desc1")]);
        popup.clear();
        assert!(!popup.is_visible());
        assert!(!popup.has_suggestions());
        assert_eq!(popup.suggestions.len(), 0);
        assert_eq!(popup.scroll_offset, 0);
    }

    #[test]
    fn test_next() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
            suggestion("item3", "desc3"),
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
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
            suggestion("item3", "desc3"),
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
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
        ]);
        assert_eq!(popup.get_selected().map(|s| s.name.as_str()), Some("item1"));
        popup.next();
        assert_eq!(popup.get_selected().map(|s| s.name.as_str()), Some("item2"));
    }

    #[test]
    fn test_visible_range_keeps_selected_item_in_view() {
        let mut popup = Popup::new();
        popup.set_suggestions(
            (0..10)
                .map(|i| Suggestion::command(format!("item{}", i), ""))
                .collect(),
        );

        assert_eq!(popup.visible_range(), 0..8);

        for _ in 0..8 {
            popup.next();
        }
        assert_eq!(popup.visible_range(), 1..9);

        popup.next();
        assert_eq!(popup.visible_range(), 2..10);
    }

    #[test]
    fn test_visible_range_empty() {
        let popup = Popup::new();
        assert_eq!(popup.visible_range(), 0..0);
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
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
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
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
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
        popup.set_suggestions(vec![suggestion("item1", "desc1")]);
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
        popup.set_suggestions(vec![suggestion("item1", "desc1")]);
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
        popup.set_suggestions(vec![suggestion("item1", "desc1")]);
        let key = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
            kind: ratatui::crossterm::event::KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        };
        let action = popup.handle_key_event(key);
        assert!(matches!(action, PopupAction::NotHandled));
    }

    #[test]
    fn test_handle_mouse_scroll_down_moves_visible_range_without_changing_selection() {
        let mut popup = Popup::new();
        popup.set_suggestions(
            (0..10)
                .map(|i| Suggestion::command(format!("item{}", i), ""))
                .collect(),
        );
        let anchor = Rect::new(0, 20, 40, 4);
        let popup_area = popup.popup_area(anchor).expect("popup area");
        popup.selected_index = 5;

        let action = popup.handle_mouse_event(
            mouse(
                MouseEventKind::ScrollDown,
                popup_area.x + 1,
                popup_area.y + 1,
            ),
            anchor,
        );

        assert!(matches!(action, PopupAction::Handled));
        assert_eq!(popup.selected_index, 5);
        assert_eq!(popup.visible_range(), 1..9);
    }

    #[test]
    fn test_handle_mouse_scroll_up_moves_visible_range_without_changing_selection() {
        let mut popup = Popup::new();
        popup.set_suggestions(
            (0..10)
                .map(|i| Suggestion::command(format!("item{}", i), ""))
                .collect(),
        );
        popup.scroll_offset = 2;
        popup.selected_index = 5;
        let anchor = Rect::new(0, 20, 40, 4);
        let popup_area = popup.popup_area(anchor).expect("popup area");

        let action = popup.handle_mouse_event(
            mouse(MouseEventKind::ScrollUp, popup_area.x + 1, popup_area.y + 1),
            anchor,
        );

        assert!(matches!(action, PopupAction::Handled));
        assert_eq!(popup.selected_index, 5);
        assert_eq!(popup.visible_range(), 1..9);
    }

    #[test]
    fn test_handle_mouse_click_autocompletes_clicked_item() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            suggestion("item1", "desc1"),
            suggestion("item2", "desc2"),
            suggestion("item3", "desc3"),
        ]);
        let anchor = Rect::new(0, 20, 40, 4);
        let popup_area = popup.popup_area(anchor).expect("popup area");

        let action = popup.handle_mouse_event(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                popup_area.x + 1,
                popup_area.y + 3,
            ),
            anchor,
        );

        assert!(matches!(action, PopupAction::Autocomplete));
        assert_eq!(popup.selected_index, 2);
    }

    #[test]
    fn test_handle_mouse_click_outside_popup_not_handled() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![suggestion("item1", "desc1")]);
        let anchor = Rect::new(0, 20, 40, 4);

        let action = popup.handle_mouse_event(
            mouse(MouseEventKind::Down(MouseButton::Left), 50, 20),
            anchor,
        );

        assert!(matches!(action, PopupAction::NotHandled));
        assert_eq!(popup.selected_index, 0);
    }
}
