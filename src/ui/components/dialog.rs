use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    prelude::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Frame,
};
use std::collections::HashMap;
use tui_textarea::{Input as TuiInput, TextArea};

#[derive(Debug)]
pub struct DialogItem {
    pub id: String,
    pub name: String,
    pub group: String,
    pub description: String,
}

impl Clone for DialogItem {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            group: self.group.clone(),
            description: self.description.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Dialog {
    pub title: String,
    pub items: Vec<DialogItem>,
    pub grouped_items: HashMap<String, Vec<DialogItem>>,
    pub filtered_items: Vec<(String, Vec<DialogItem>)>,
    pub groups: Vec<String>,
    pub selected_index: usize,
    pub visible: bool,
    pub search_query: String,
    pub scroll_offset: usize,
    pub dialog_area: Rect,
    pub content_area: Rect,
    pub search_textarea: TextArea<'static>,
    pub scrollbar_state: ScrollbarState,
    pub is_dragging_scrollbar: bool,
    matcher: Matcher,
}

impl Dialog {
    pub fn new(title: impl Into<String>) -> Self {
        let title = title.into();
        let mut search_textarea = TextArea::default();
        search_textarea.set_placeholder_text("Search");
        Self {
            title,
            items: Vec::new(),
            grouped_items: HashMap::new(),
            filtered_items: Vec::new(),
            groups: Vec::new(),
            selected_index: 0,
            visible: false,
            search_query: String::new(),
            scroll_offset: 0,
            dialog_area: Rect::default(),
            content_area: Rect::default(),
            search_textarea,
            scrollbar_state: ScrollbarState::default(),
            is_dragging_scrollbar: false,
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        let mut dialog = Self::new(title);
        dialog.set_items(items);
        dialog
    }

    pub fn set_items(&mut self, items: Vec<DialogItem>) {
        self.items = items;
        self.group_items();
        self.apply_filter();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.update_scrollbar();
    }

    fn group_items(&mut self) {
        self.grouped_items.clear();
        self.groups.clear();

        for item in &self.items {
            self.grouped_items
                .entry(item.group.clone())
                .or_default()
                .push(item.clone());
        }

        self.groups = {
            let mut groups: Vec<_> = self.grouped_items.keys().cloned().collect();
            groups.sort();
            groups
        };
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.apply_filter();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.search_query.clear();
        self.search_textarea = TextArea::default();
        self.search_textarea.set_placeholder_text("Search");
    }

    pub fn toggle(&mut self) {
        if self.visible {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
        self.apply_filter();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.update_scrollbar();
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.apply_filter();
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.update_scrollbar();
    }

    fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_items = self
                .groups
                .iter()
                .map(|group| {
                    (
                        group.clone(),
                        self.grouped_items.get(group).unwrap().clone(),
                    )
                })
                .collect();
        } else {
            let pattern = Pattern::parse(
                &self.search_query,
                CaseMatching::Ignore,
                Normalization::Smart,
            );
            let mut filtered: Vec<(String, Vec<DialogItem>)> = Vec::new();

            for group in &self.groups {
                let items = self.grouped_items.get(group).unwrap();
                let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
                let descriptions: Vec<&str> =
                    items.iter().map(|item| item.description.as_str()).collect();

                let matched_names: Vec<(&str, u32)> = pattern.match_list(names, &mut self.matcher);
                let matched_descriptions: Vec<(&str, u32)> =
                    pattern.match_list(descriptions, &mut self.matcher);

                let matching_items: Vec<DialogItem> = items
                    .iter()
                    .filter(|item| {
                        matched_names
                            .iter()
                            .any(|(name, _)| *name == item.name.as_str())
                            || matched_descriptions
                                .iter()
                                .any(|(desc, _)| *desc == item.description.as_str())
                    })
                    .cloned()
                    .collect();

                if !matching_items.is_empty() {
                    filtered.push((group.clone(), matching_items));
                }
            }
            self.filtered_items = filtered;
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&mut self) {
        let mut total_lines = 0;
        for (_, items) in &self.filtered_items {
            if !items.is_empty() {
                total_lines += items.len() + 1;
            }
        }
        self.scrollbar_state = self.scrollbar_state.content_length(total_lines);
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }

    pub fn next(&mut self) {
        let flat_items = self.get_flat_items();
        if !flat_items.is_empty() && self.selected_index < flat_items.len() - 1 {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    pub fn previous(&mut self) {
        let flat_items = self.get_flat_items();
        if !flat_items.is_empty() && self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    fn get_flat_items(&self) -> Vec<&DialogItem> {
        let mut items = Vec::new();
        for (_, group_items) in &self.filtered_items {
            for item in group_items {
                items.push(item);
            }
        }
        items
    }

    fn get_content_line_count(&self) -> usize {
        let mut count = 0;
        for (_, items) in &self.filtered_items {
            if !items.is_empty() {
                count += items.len() + 1;
            }
        }
        count
    }

    fn get_line_index_of_item(&self, item_index: usize) -> usize {
        let mut line_index = 0;
        let mut current_item_index = 0;

        for (_, items) in &self.filtered_items {
            for _item in items {
                if current_item_index == item_index {
                    return line_index;
                }
                line_index += 1;
                current_item_index += 1;
            }
        }
        line_index
    }

    fn adjust_scroll(&mut self) {
        let visible_rows = self.get_visible_row_count();
        let selected_line = self.get_line_index_of_item(self.selected_index);

        if selected_line < self.scroll_offset {
            self.scroll_offset = selected_line;
        } else if selected_line >= self.scroll_offset + visible_rows {
            self.scroll_offset = selected_line - visible_rows + 1;
        }
        self.update_scrollbar();
    }

    fn get_visible_row_count(&self) -> usize {
        let dialog_height = self.dialog_area.height.saturating_sub(10);
        dialog_height as usize
    }

    pub fn get_selected(&self) -> Option<&DialogItem> {
        let flat_items = self.get_flat_items();
        flat_items.get(self.selected_index).copied()
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> bool {
        if !self.visible {
            return false;
        }

        match event.code {
            KeyCode::Esc => {
                self.hide();
                true
            }
            KeyCode::Enter => true,
            KeyCode::Up => {
                self.previous();
                true
            }
            KeyCode::Down => {
                self.next();
                true
            }
            KeyCode::Char('j') if event.modifiers == KeyModifiers::CONTROL => true,
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => false,
            _ => {
                let input = TuiInput::from(event);
                self.search_textarea.input(input);
                self.search_query = self.search_textarea.lines().join("");
                self.apply_filter();
                true
            }
        }
    }

    pub fn handle_mouse_event(&mut self, event: MouseEvent) -> bool {
        if !self.visible {
            return false;
        }

        use ratatui::layout::Position;
        let point = Position::new(event.column, event.row);

        const PADDING: u16 = 3;
        let content_area = Rect {
            x: self.dialog_area.x + PADDING,
            y: self.dialog_area.y + PADDING,
            width: self.dialog_area.width.saturating_sub(PADDING * 2),
            height: self.dialog_area.height.saturating_sub(PADDING * 2),
        };

        if !content_area.contains(point) {
            self.is_dragging_scrollbar = false;
            return false;
        }

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(3),
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(content_area);

        let list_area = chunks[3];
        let scrollbar_area = Rect {
            x: list_area.x + list_area.width - 1,
            y: list_area.y,
            width: 1,
            height: list_area.height,
        };

        let is_on_scrollbar = scrollbar_area.contains(point);

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.next();
                true
            }
            MouseEventKind::ScrollUp => {
                self.previous();
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if is_on_scrollbar {
                    self.is_dragging_scrollbar = true;
                    self.scroll_to_position(event.row, scrollbar_area);
                    true
                } else {
                    if let Some(item_index) = self.get_item_index_from_y(event.row, list_area) {
                        self.selected_index = item_index;
                        return true;
                    }
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
            MouseEventKind::Moved => {
                if !is_on_scrollbar {
                    if let Some(item_index) = self.get_item_index_from_y(event.row, list_area) {
                        if item_index != self.selected_index {
                            self.selected_index = item_index;
                        }
                    }
                }
                false
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

    fn get_item_index_from_y(&self, row: u16, list_area: Rect) -> Option<usize> {
        let relative_y = row.saturating_sub(list_area.y) as usize;
        let content_line = self.scroll_offset + relative_y;

        let mut current_line = 0;
        let mut item_index = 0;

        for (_, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            let group_header_line = current_line;
            let items_start_line = group_header_line + 1;
            let items_end_line = items_start_line + items.len();

            if content_line >= items_start_line && content_line < items_end_line {
                return Some(item_index + (content_line - items_start_line));
            }

            current_line = items_end_line;
            item_index += items.len();
        }

        None
    }

    fn scroll_to_position(&mut self, row: u16, scrollbar_area: Rect) {
        let total_lines = self.get_content_line_count();
        if total_lines == 0 {
            return;
        }

        let visible_rows = scrollbar_area.height as usize;
        let relative_y = row.saturating_sub(scrollbar_area.y) as usize;

        let new_offset = (relative_y * total_lines) / visible_rows.max(1);
        self.scroll_offset = new_offset.saturating_sub(visible_rows / 3);
        let max_offset = total_lines.saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(max_offset);

        let flat_items = self.get_flat_items();
        if !flat_items.is_empty() {
            self.selected_index = flat_items
                .len()
                .saturating_sub(1)
                .min(self.scroll_offset + visible_rows / 2);
        }

        self.update_scrollbar();
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        const DIALOG_WIDTH: u16 = 70;
        const DIALOG_HEIGHT: u16 = 25;

        let dialog_width = area.width.min(DIALOG_WIDTH);
        let dialog_height = area.height.min(DIALOG_HEIGHT);

        self.dialog_area = Rect {
            x: (area.width - dialog_width) / 2,
            y: (area.height - dialog_height) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        frame.render_widget(Clear, self.dialog_area);

        const PADDING: u16 = 3;
        self.content_area = Rect {
            x: self.dialog_area.x + PADDING,
            y: self.dialog_area.y + PADDING,
            width: self.dialog_area.width.saturating_sub(PADDING * 2),
            height: self.dialog_area.height.saturating_sub(PADDING * 2),
        };

        frame.render_widget(
            ratatui::widgets::Paragraph::new("")
                .style(ratatui::style::Style::default().bg(Color::Rgb(20, 20, 30))),
            self.dialog_area,
        );

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(1),
                ratatui::layout::Constraint::Length(3),
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(self.content_area);

        let title_line = Line::from(vec![
            Span::styled(
                &self.title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                "esc",
                Style::default()
                    .fg(Color::Rgb(255, 140, 0))
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        let title_paragraph =
            Paragraph::new(title_line).alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(title_paragraph, chunks[0]);

        frame.render_widget(&self.search_textarea, chunks[2]);

        let mut content_lines = Vec::new();
        let flat_items = self.get_flat_items();
        let list_area_width = chunks[3].width;

        if flat_items.is_empty() {
            content_lines.push(Line::from(vec![Span::styled(
                "No results found",
                Style::default().fg(Color::Gray),
            )]));
        } else {
            let mut item_index = 0;

            for (group, items) in &self.filtered_items {
                if items.is_empty() {
                    continue;
                }
                content_lines.push(Line::from(vec![Span::styled(
                    group.clone(),
                    Style::default()
                        .fg(Color::Rgb(255, 140, 0))
                        .add_modifier(Modifier::BOLD),
                )]));

                for item in items {
                    let style = if item_index == self.selected_index {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Rgb(255, 200, 100))
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let item_text = format!("  {}", item.name);
                    let padding =
                        " ".repeat((list_area_width as usize).saturating_sub(item_text.len()));
                    let full_text = format!("{}{}", item_text, padding);
                    content_lines.push(Line::from(full_text).style(style));
                    item_index += 1;
                }
            }
        }

        let content_paragraph = Paragraph::new(content_lines)
            .wrap(Wrap { trim: false })
            .scroll(((self.scroll_offset as u16).saturating_sub(1), 0));
        frame.render_widget(content_paragraph, chunks[3]);

        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            chunks[3],
            &mut self.scrollbar_state.clone(),
        );

        let footer_line = Line::from(vec![
            Span::styled(
                "Connect provider",
                Style::default()
                    .fg(Color::Rgb(255, 180, 120))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "ctrl+a",
                Style::default()
                    .fg(Color::Rgb(150, 120, 100))
                    .add_modifier(Modifier::DIM),
            ),
            Span::raw("  "),
            Span::styled(
                "Favorite",
                Style::default()
                    .fg(Color::Rgb(255, 180, 120))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "ctrl+f",
                Style::default()
                    .fg(Color::Rgb(150, 120, 100))
                    .add_modifier(Modifier::DIM),
            ),
        ]);

        let footer_paragraph =
            Paragraph::new(footer_line).alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(footer_paragraph, chunks[4]);
    }
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new("Dialog")
    }
}

impl Clone for Dialog {
    fn clone(&self) -> Self {
        Self {
            title: self.title.clone(),
            items: self.items.clone(),
            grouped_items: self.grouped_items.clone(),
            filtered_items: self.filtered_items.clone(),
            groups: self.groups.clone(),
            selected_index: self.selected_index,
            visible: self.visible,
            search_query: self.search_query.clone(),
            scroll_offset: self.scroll_offset,
            dialog_area: self.dialog_area,
            content_area: self.content_area,
            search_textarea: self.search_textarea.clone(),
            scrollbar_state: self.scrollbar_state,
            is_dragging_scrollbar: self.is_dragging_scrollbar,
            matcher: Matcher::new(Config::DEFAULT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_items() -> Vec<DialogItem> {
        vec![
            DialogItem {
                id: "1".to_string(),
                name: "Model A".to_string(),
                group: "Provider1".to_string(),
                description: "Description for Model A".to_string(),
            },
            DialogItem {
                id: "2".to_string(),
                name: "Model B".to_string(),
                group: "Provider1".to_string(),
                description: "Description for Model B".to_string(),
            },
            DialogItem {
                id: "3".to_string(),
                name: "Model C".to_string(),
                group: "Provider2".to_string(),
                description: "Description for Model C".to_string(),
            },
        ]
    }

    #[test]
    fn test_dialog_creation() {
        let dialog = Dialog::new("Test Dialog");
        assert_eq!(dialog.title, "Test Dialog");
        assert!(!dialog.is_visible());
        assert!(dialog.items.is_empty());
    }

    #[test]
    fn test_dialog_default() {
        let dialog = Dialog::default();
        assert_eq!(dialog.title, "Dialog");
        assert!(!dialog.is_visible());
    }

    #[test]
    fn test_dialog_with_items() {
        let items = create_test_items();
        let dialog = Dialog::with_items("Models", items);
        assert_eq!(dialog.items.len(), 3);
        assert_eq!(dialog.groups.len(), 2);
    }

    #[test]
    fn test_dialog_set_items() {
        let mut dialog = Dialog::new("Models");
        let items = create_test_items();
        dialog.set_items(items);
        assert_eq!(dialog.items.len(), 3);
        assert_eq!(dialog.groups.len(), 2);
        assert_eq!(dialog.selected_index, 0);
    }

    #[test]
    fn test_dialog_show_hide() {
        let mut dialog = Dialog::new("Test");
        assert!(!dialog.is_visible());

        dialog.show();
        assert!(dialog.is_visible());

        dialog.hide();
        assert!(!dialog.is_visible());
    }

    #[test]
    fn test_dialog_toggle() {
        let mut dialog = Dialog::new("Test");
        assert!(!dialog.is_visible());

        dialog.toggle();
        assert!(dialog.is_visible());

        dialog.toggle();
        assert!(!dialog.is_visible());
    }

    #[test]
    fn test_dialog_search() {
        let mut dialog = Dialog::with_items("Models", create_test_items());
        dialog.set_search_query("Model A");
        assert_eq!(dialog.filtered_items.len(), 1);
        assert_eq!(dialog.filtered_items[0].1[0].name, "Model A");
    }

    #[test]
    fn test_dialog_search_case_insensitive() {
        let mut dialog = Dialog::with_items("Models", create_test_items());
        dialog.set_search_query("model a");
        assert_eq!(dialog.filtered_items.len(), 1);
        assert_eq!(dialog.filtered_items[0].1[0].name, "Model A");
    }

    #[test]
    fn test_dialog_clear_search() {
        let mut dialog = Dialog::with_items("Models", create_test_items());
        dialog.set_search_query("Model");
        assert_eq!(dialog.filtered_items.len(), 2);

        dialog.clear_search();
        assert!(dialog.search_query.is_empty());
        assert_eq!(dialog.filtered_items.len(), 2);
    }

    #[test]
    fn test_dialog_next() {
        let mut dialog = Dialog::with_items("Models", create_test_items());
        assert_eq!(dialog.selected_index, 0);

        dialog.next();
        assert_eq!(dialog.selected_index, 1);

        dialog.next();
        assert_eq!(dialog.selected_index, 2);

        dialog.next();
        assert_eq!(dialog.selected_index, 2);
    }

    #[test]
    fn test_dialog_previous() {
        let mut dialog = Dialog::with_items("Models", create_test_items());
        assert_eq!(dialog.selected_index, 0);

        dialog.previous();
        assert_eq!(dialog.selected_index, 0);

        dialog.selected_index = 2;
        dialog.previous();
        assert_eq!(dialog.selected_index, 1);

        dialog.previous();
        assert_eq!(dialog.selected_index, 0);

        dialog.previous();
        assert_eq!(dialog.selected_index, 0);
    }

    #[test]
    fn test_dialog_get_selected() {
        let dialog = Dialog::with_items("Models", create_test_items());
        let selected = dialog.get_selected();
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "Model A");
    }

    #[test]
    fn test_dialog_empty_items() {
        let mut dialog = Dialog::new("Models");
        dialog.set_search_query("test");
        assert!(dialog.get_flat_items().is_empty());
    }

    #[test]
    fn test_dialog_clone() {
        let dialog = Dialog::with_items("Models", create_test_items());
        let dialog2 = dialog.clone();
        assert_eq!(dialog.title, dialog2.title);
        assert_eq!(dialog.items.len(), dialog2.items.len());
    }
}
