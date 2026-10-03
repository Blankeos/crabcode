use crate::autocomplete::{Suggestion, SuggestionKind};
use crate::theme::{contrast_text, ThemeColors};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    prelude::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem},
    Frame,
};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

const MAX_VISIBLE_ITEMS: usize = 8;
const ITEM_HORIZONTAL_PADDING: usize = 1;

// Keep text on one terminal row, including descriptions supplied by plugins.
fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_columns(text: &str, width: usize) -> String {
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let next = index + character.len_utf8();
        if text[..next].width() > width {
            break;
        }

        end = next;
    }
    text[..end].to_owned()
}

fn suggestion_row(
    suggestion: &Suggestion,
    width: usize,
    max_name_width: usize,
    selected: bool,
    colors: ThemeColors,
) -> Line<'static> {
    let (background, name_fg, description_fg) = if selected {
        let foreground = contrast_text(colors.primary);
        (colors.primary, foreground, foreground)
    } else {
        (Color::Reset, colors.text, colors.text_weak)
    };
    let name_style = Style::default()
        .fg(name_fg)
        .bg(background)
        .add_modifier(Modifier::BOLD);
    let description_style = Style::default().fg(description_fg).bg(background);
    let padding_style = Style::default().bg(background);
    let label = match suggestion.kind {
        SuggestionKind::Skill => "skill",
        SuggestionKind::Agent => "agent",
        _ => "",
    };
    // Labels take precedence over padding and content on very narrow terminals.
    let label = truncate_columns(label, width);
    let remaining = width.saturating_sub(label.width());
    let right_padding = ITEM_HORIZONTAL_PADDING.min(remaining);
    let left_padding = ITEM_HORIZONTAL_PADDING.min(remaining - right_padding);
    let content_width = remaining - left_padding - right_padding;
    let label_gap = usize::from(!label.is_empty() && content_width > 0);
    let text_width = content_width - label_gap;
    let display_name = format!(
        "{}{}",
        suggestion.display_prefix(),
        normalize_whitespace(&suggestion.name)
    );
    let description = normalize_whitespace(&suggestion.description);
    // Cap the aligned name column so a long name cannot consume the description.
    let name_budget = if description.is_empty() {
        text_width
    } else {
        max_name_width.min(text_width / 2)
    };
    let name = truncate_columns(&display_name, name_budget);
    let gap = if description.is_empty() {
        0
    } else {
        (name_budget.saturating_sub(name.width()) + 3).min(text_width.saturating_sub(name.width()))
    };
    let description = truncate_columns(&description, text_width.saturating_sub(name.width() + gap));
    let end_padding = content_width.saturating_sub(name.width() + gap + description.width());
    Line::from(vec![
        Span::styled(" ".repeat(left_padding), padding_style),
        Span::styled(name, name_style),
        Span::styled(" ".repeat(gap), padding_style),
        Span::styled(description, description_style),
        Span::styled(" ".repeat(end_padding), padding_style),
        Span::styled(label, description_style),
        Span::styled(" ".repeat(right_padding), padding_style),
    ])
}

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
    selection_explicit: bool,
}

impl Popup {
    pub fn new() -> Self {
        Self {
            suggestions: Vec::new(),
            selected_index: 0,
            visible: false,
            scroll_offset: 0,
            selection_explicit: false,
        }
    }

    pub fn set_suggestions(&mut self, suggestions: Vec<Suggestion>) {
        self.suggestions = suggestions;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.visible = !self.suggestions.is_empty();
        self.selection_explicit = false;
    }

    pub fn clear(&mut self) {
        self.suggestions.clear();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.visible = false;
        self.selection_explicit = false;
    }

    pub fn next(&mut self) {
        if !self.suggestions.is_empty() {
            self.selection_explicit = true;
            self.selected_index = (self.selected_index + 1) % self.suggestions.len();
            self.keep_selected_visible();
        }
    }

    pub fn previous(&mut self) {
        if !self.suggestions.is_empty() {
            self.selection_explicit = true;
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

    pub fn selection_is_explicit(&self) -> bool {
        self.selection_explicit
    }

    fn popup_area(&self, area: Rect) -> Option<Rect> {
        if !self.visible || self.suggestions.is_empty() {
            return None;
        }

        let popup_height = (self.visible_range().len() as u16) + 2;
        // The popup is displayed above the input, with a three-row gap. On a
        // short terminal, avoid rendering past the top edge of the frame.
        let available_height = area.y.saturating_sub(3);
        let popup_height = popup_height.min(available_height);

        // A bordered list needs a top border, one item row, and a bottom
        // border. Hide it until there is enough vertical space to render one.
        if popup_height < 3 {
            return None;
        }

        Some(Rect {
            x: area.x,
            y: available_height.saturating_sub(popup_height),
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
                    self.selection_explicit = true;
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
                    self.selection_explicit = true;
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

        let max_name_width = self
            .suggestions
            .iter()
            .map(|s| s.display_prefix().width() + normalize_whitespace(&s.name).width())
            .max()
            .unwrap_or(0);
        let title = if self
            .suggestions
            .iter()
            .all(|s| s.kind == SuggestionKind::File)
        {
            "Files"
        } else if self
            .suggestions
            .iter()
            .all(|s| s.kind == SuggestionKind::Command)
        {
            "Commands"
        } else {
            "Mentions"
        };

        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .enumerate()
            .skip(visible_range.start)
            .take(visible_range.len())
            .map(|(i, suggestion)| {
                ListItem::new(suggestion_row(
                    suggestion,
                    item_width,
                    max_name_width,
                    i == self.selected_index,
                    colors,
                ))
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

    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    fn test_colors() -> ThemeColors {
        Theme::load_builtin_default().get_colors(true)
    }

    fn render_popup(popup: &Popup, width: u16, focused: bool) -> (Buffer, Rect) {
        let mut terminal = Terminal::new(TestBackend::new(width + 4, 24)).unwrap();
        let anchor = Rect::new(2, 20, width, 2);
        terminal
            .draw(|frame| popup.render(frame, anchor, focused, test_colors()))
            .unwrap();
        (
            terminal.backend().buffer().clone(),
            popup.popup_area(anchor).unwrap(),
        )
    }

    fn row_text(buffer: &Buffer, area: Rect, row: u16) -> String {
        (area.x..area.right())
            .map(|x| buffer[(x, area.y + row)].symbol())
            .collect()
    }

    fn skill(name: &str, description: &str) -> Suggestion {
        let mut suggestion = Suggestion::agent(name, description);
        suggestion.kind = SuggestionKind::Skill;
        suggestion
    }

    #[test]
    fn test_render_type_labels_titles_and_theme_colors() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            skill("review", "Review changes"),
            Suggestion::agent("executor", "Run tasks"),
            suggestion("help", "Show help"),
            Suggestion::file("src/main.rs", false),
        ]);
        let (buffer, area) = render_popup(&popup, 52, true);
        assert!(row_text(&buffer, area, 0).contains("Mentions"));
        assert!(row_text(&buffer, area, 1).ends_with("skill │"));
        assert!(row_text(&buffer, area, 2).ends_with("agent │"));
        let command_row = row_text(&buffer, area, 3);
        assert!(command_row.contains("/help"));
        assert!(command_row.contains("Show help"));
        assert!(!command_row.contains("command"));
        assert!(!row_text(&buffer, area, 4).contains("file"));
        let colors = test_colors();
        for x in area.x + 1..area.right() - 1 {
            assert_eq!(buffer[(x, area.y + 1)].bg, colors.primary);
        }
        assert_eq!(
            buffer[(area.x + 2, area.y + 1)].fg,
            contrast_text(colors.primary)
        );
        assert_eq!(
            buffer[(area.right() - 3, area.y + 1)].fg,
            contrast_text(colors.primary)
        );
        assert_eq!(buffer[(area.x + 2, area.y + 2)].fg, colors.text);
        assert_eq!(buffer[(area.right() - 3, area.y + 2)].fg, colors.text_weak);
        assert_eq!(buffer[(area.x, area.y + 1)].fg, colors.border_focus);
        let (buffer, area) = render_popup(&popup, 52, false);
        assert_eq!(buffer[(area.x, area.y + 1)].fg, colors.border_weak_focus);

        for (suggestions, title) in [
            (vec![suggestion("help", "")], "Commands"),
            (vec![Suggestion::file("src", true)], "Files"),
            (
                vec![Suggestion::file("src", true), skill("review", "")],
                "Mentions",
            ),
        ] {
            popup.set_suggestions(suggestions);
            let (buffer, area) = render_popup(&popup, 30, true);
            assert!(row_text(&buffer, area, 0).contains(title));
        }
    }

    #[test]
    fn test_render_unicode_and_narrow_rows_without_overflow() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            skill(
                &"界e\u{301}🦀".repeat(30),
                &"説明\n\t more   words ".repeat(30),
            ),
            Suggestion::agent(&"🦀界".repeat(30), "line one\nline two\tline three"),
            suggestion(&"界".repeat(30), &"説明 ".repeat(30)),
        ]);
        for width in 0..=48 {
            let (buffer, area) = render_popup(&popup, width, true);
            for y in area.y..area.bottom() {
                assert_eq!(buffer[(0, y)].symbol(), " ");
                assert_eq!(buffer[(1, y)].symbol(), " ");
                assert_eq!(buffer[(area.right(), y)].symbol(), " ");
                if width >= 2 && y > area.y && y < area.bottom() - 1 {
                    assert_eq!(buffer[(area.x, y)].symbol(), "│");
                    assert_eq!(buffer[(area.right() - 1, y)].symbol(), "│");
                }
            }
            if width >= 9 {
                assert!(row_text(&buffer, area, 1).ends_with("skill │"));
                assert!(row_text(&buffer, area, 2).ends_with("agent │"));
            }
            for suggestion in &popup.suggestions {
                let row = suggestion_row(suggestion, width as usize, 200, false, test_colors());
                assert_eq!(row.width(), width as usize);
                assert!(row
                    .spans
                    .iter()
                    .all(|span| !span.content.contains(['\n', '\t'])));
            }
        }
    }

    #[test]
    fn test_description_whitespace_and_display_column_alignment() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![
            Suggestion::agent("界", "  first\n second\tthird  "),
            Suggestion::agent("ab", "other"),
        ]);
        let (buffer, area) = render_popup(&popup, 50, true);
        let row = row_text(&buffer, area, 1);
        assert!(row.contains("first second third"));
        // Both names occupy three display columns including the prefix.
        let description_x = area.x + 2 + 3 + 3;
        assert_eq!(buffer[(description_x, area.y + 1)].symbol(), "f");
        assert_eq!(buffer[(description_x, area.y + 2)].symbol(), "o");
    }

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
    fn test_popup_area_clamps_to_space_above_anchor() {
        let mut popup = Popup::new();
        popup.set_suggestions(
            (0..MAX_VISIBLE_ITEMS)
                .map(|index| suggestion(&format!("item{index}"), "desc"))
                .collect(),
        );

        let area = popup
            .popup_area(Rect::new(0, 7, 40, 2))
            .expect("popup area");

        assert_eq!(area, Rect::new(0, 0, 40, 4));
    }

    #[test]
    fn test_popup_area_hides_when_too_short_for_border_and_item() {
        let mut popup = Popup::new();
        popup.set_suggestions(vec![suggestion("item1", "desc1")]);

        assert_eq!(popup.popup_area(Rect::new(0, 5, 40, 2)), None);
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
