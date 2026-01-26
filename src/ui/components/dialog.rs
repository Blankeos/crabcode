use crate::theme::ThemeColors;
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
    pub tip: Option<String>,
    pub provider_id: String,
}

impl Clone for DialogItem {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            group: self.group.clone(),
            description: self.description.clone(),
            tip: self.tip.clone(),
            provider_id: self.provider_id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DialogAction {
    pub label: String,
    pub key: String,
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
    pub visible_row_count: usize,
    pub actions: Vec<DialogAction>,
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
            visible_row_count: 0,
            actions: Vec::new(),
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        let mut dialog = Self::new(title);
        dialog.set_items(items);
        dialog
    }

    pub fn with_actions(mut self, actions: Vec<DialogAction>) -> Self {
        self.actions = actions;
        self
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

        let mut seen_groups = std::collections::HashSet::new();
        let mut groups_in_order: Vec<String> = Vec::new();

        for item in &self.items {
            let group = item.group.clone();
            if seen_groups.insert(group.clone()) {
                groups_in_order.push(group.clone());
            }
            self.grouped_items
                .entry(group)
                .or_default()
                .push(item.clone());
        }

        const SPECIAL_GROUPS: &[&str] = &["Favorite", "Recent", "Popular", "Other"];
        let mut special: Vec<String> = Vec::new();
        let mut regular: Vec<String> = Vec::new();

        for group in groups_in_order {
            if SPECIAL_GROUPS.contains(&group.as_str()) {
                special.push(group);
            } else {
                regular.push(group);
            }
        }

        special.sort_by(|a, b| {
            let ai = SPECIAL_GROUPS.iter().position(|&g| g == a).unwrap();
            let bi = SPECIAL_GROUPS.iter().position(|&g| g == b).unwrap();
            ai.cmp(&bi)
        });

        self.groups = special.into_iter().chain(regular.into_iter()).collect();
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

                let combined_strings: Vec<String> = items
                    .iter()
                    .map(|item| format!("{} {}", group, item.name))
                    .collect();

                let matched: Vec<(&str, u32)> = pattern.match_list(
                    combined_strings.iter().map(|s| s.as_str()),
                    &mut self.matcher,
                );

                if !matched.is_empty() {
                    let mut scored_items: Vec<(DialogItem, u32)> = matched
                        .into_iter()
                        .filter_map(|(combined_str, score)| {
                            items
                                .iter()
                                .find(|item| format!("{} {}", group, item.name) == *combined_str)
                                .map(|item| (item.clone(), score))
                        })
                        .collect();

                    scored_items.sort_by(|a, b| b.1.cmp(&a.1));

                    let sorted_items: Vec<DialogItem> =
                        scored_items.into_iter().map(|(item, _)| item).collect();

                    filtered.push((group.clone(), sorted_items));
                }
            }
            self.filtered_items = filtered;
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&mut self) {
        let total_lines = self.get_content_line_count();
        let visible_rows = self.get_visible_row_count().max(1);
        let max_offset = total_lines.saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(max_offset);

        let scrollbar_content_length = max_offset.saturating_add(1).max(1);
        let scrollbar_position = self
            .scroll_offset
            .min(scrollbar_content_length.saturating_sub(1));
        self.scrollbar_state = self
            .scrollbar_state
            .content_length(scrollbar_content_length);
        self.scrollbar_state = self.scrollbar_state.position(scrollbar_position);
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

    pub fn scroll_down(&mut self) {
        let total_lines = self.get_content_line_count();
        if total_lines == 0 {
            return;
        }
        let visible_rows = self.get_visible_row_count().max(1);
        let max_offset = total_lines.saturating_sub(visible_rows);
        self.scroll_offset = (self.scroll_offset + 1).min(max_offset);
        self.update_scrollbar();
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.update_scrollbar();
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
        let flat_items = self.get_flat_items();
        if flat_items.is_empty() {
            return 1;
        }
        let mut count = 0;
        for (_, items) in &self.filtered_items {
            count += items.len() + 1;
        }
        count
    }

    fn get_line_index_of_item(&self, item_index: usize) -> usize {
        let mut line_index = 0;
        let mut current_item_index = 0;

        for (_, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            line_index += 1;

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
        let visible_rows = self.get_visible_row_count().max(1);
        let selected_line = self.get_line_index_of_item(self.selected_index);

        if selected_line < self.scroll_offset {
            self.scroll_offset = selected_line;
        } else if selected_line
            >= self
                .scroll_offset
                .saturating_add(visible_rows.saturating_sub(1))
        {
            self.scroll_offset = selected_line.saturating_sub(visible_rows.saturating_sub(1));
        }

        if self.selected_index == 0 {
            self.scroll_offset = 0;
        }

        self.update_scrollbar();
    }

    fn get_visible_row_count(&self) -> usize {
        if self.visible_row_count > 0 {
            self.visible_row_count
        } else {
            const DIALOG_WIDTH: u16 = 70;
            const DIALOG_HEIGHT: u16 = 25;
            const PADDING: u16 = 3;

            let total_fixed_height = 1 + 1 + 3 + 1 + 1;
            let padding_total = PADDING * 2;
            let list_area_height = DIALOG_HEIGHT.saturating_sub(total_fixed_height + padding_total);
            list_area_height as usize
        }
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
                self.scroll_down();
                true
            }
            MouseEventKind::ScrollUp => {
                self.scroll_up();
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
        self.get_item_index_from_line(content_line)
    }

    fn get_item_index_from_line(&self, line: usize) -> Option<usize> {
        let mut current_line = 0;
        let mut item_index = 0;

        for (_, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            let group_header_line = current_line;
            let items_start_line = group_header_line + 1;
            let items_end_line = items_start_line + items.len();

            if line >= items_start_line && line < items_end_line {
                return Some(item_index + (line - items_start_line));
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
        let max_offset = total_lines.saturating_sub(visible_rows);

        let new_offset = if max_offset > 0 {
            (relative_y * max_offset) / visible_rows
        } else {
            0
        };
        self.scroll_offset = new_offset.min(max_offset);

        let flat_items = self.get_flat_items();
        if !flat_items.is_empty() && visible_rows > 0 {
            let item_at_offset = self.get_item_index_from_line(self.scroll_offset);
            if let Some(idx) = item_at_offset {
                self.selected_index = idx;
            }
        }

        self.update_scrollbar();
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
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
                    .fg(colors.primary)
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
        let filtered_items = self.filtered_items.clone();

        if flat_items.is_empty() {
            content_lines.push(Line::from(vec![Span::styled(
                "No results found",
                Style::default().fg(Color::Gray),
            )]));
        } else {
            let mut item_index = 0;

            for (group, items) in &filtered_items {
                if items.is_empty() {
                    continue;
                }
                content_lines.push(Line::from(vec![Span::styled(
                    group.clone(),
                    Style::default()
                        .fg(colors.primary)
                        .add_modifier(Modifier::BOLD),
                )]));

                for item in items {
                    let is_selected = item_index == self.selected_index;
                    let is_special_group = group == "Favorite" || group == "Recent";
                    let has_description = is_special_group && !item.description.is_empty();

                    let mut spans: Vec<Span> = if let Some(tip) = &item.tip {
                        let base_len = if has_description {
                            item.name.len() + item.description.len() + 4
                        } else {
                            item.name.len() + 2
                        };
                        let padding_len =
                            (list_area_width as usize).saturating_sub(base_len + tip.len() + 2);
                        let padding_after_tip = (list_area_width as usize)
                            .saturating_sub(base_len + tip.len() + 2 + padding_len);

                        if has_description {
                            vec![
                                Span::raw(format!("  {}  ", item.name)),
                                Span::styled(
                                    item.description.clone(),
                                    Style::default()
                                        .fg(Color::Rgb(150, 150, 150))
                                        .add_modifier(Modifier::DIM),
                                ),
                                Span::raw(" ".repeat(padding_len)),
                                Span::styled(
                                    tip,
                                    Style::default()
                                        .fg(Color::Rgb(100, 200, 100))
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::raw(" ".repeat(padding_after_tip)),
                            ]
                        } else {
                            vec![
                                Span::raw(format!("  {}", item.name)),
                                Span::raw(" ".repeat(padding_len)),
                                Span::styled(
                                    tip,
                                    Style::default()
                                        .fg(Color::Rgb(150, 120, 100))
                                        .add_modifier(Modifier::DIM),
                                ),
                                Span::raw(" ".repeat(padding_after_tip)),
                            ]
                        }
                    } else if has_description {
                        let text_len = item.name.len() + item.description.len() + 4;
                        let padding_len = (list_area_width as usize).saturating_sub(text_len);
                        vec![
                            Span::raw(format!("  {}  ", item.name)),
                            Span::styled(
                                item.description.clone(),
                                Style::default()
                                    .fg(Color::Rgb(150, 150, 150))
                                    .add_modifier(Modifier::DIM),
                            ),
                            Span::raw(" ".repeat(padding_len)),
                        ]
                    } else {
                        let text_len = item.name.len() + 2;
                        let padding_len = (list_area_width as usize).saturating_sub(text_len);
                        vec![
                            Span::raw(format!("  {}", item.name)),
                            Span::raw(" ".repeat(padding_len)),
                        ]
                    };

                    if is_selected {
                        for span in &mut spans {
                            let mut style = span.style.clone();
                            style = style.fg(Color::Black).bg(colors.primary);
                            span.style = style;
                        }
                    }

                    content_lines.push(Line::from(spans));
                    item_index += 1;
                }
            }
        }

        self.visible_row_count = chunks[3].height as usize;
        self.update_scrollbar();

        let list_content_area = Rect {
            x: chunks[3].x,
            y: chunks[3].y,
            width: chunks[3].width.saturating_sub(2),
            height: chunks[3].height,
        };

        let content_paragraph =
            Paragraph::new(content_lines).scroll((self.scroll_offset as u16, 0));
        frame.render_widget(content_paragraph, list_content_area);

        let scrollbar_area = chunks[3];
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"))
                .track_symbol(Some(" ")),
            scrollbar_area,
            &mut self.scrollbar_state,
        );

        let mut footer_spans = vec![];
        for (i, action) in self.actions.iter().enumerate() {
            if i > 0 {
                footer_spans.push(Span::raw("  "));
            }
            footer_spans.push(Span::styled(
                &action.label,
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            footer_spans.push(Span::raw("  "));
            footer_spans.push(Span::styled(
                &action.key,
                Style::default()
                    .fg(Color::Rgb(150, 120, 100))
                    .add_modifier(Modifier::DIM),
            ));
        }

        let footer_line = if footer_spans.is_empty() {
            Line::from(vec![
                Span::styled(
                    "Connect provider",
                    Style::default()
                        .fg(colors.primary)
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
                        .fg(colors.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    "ctrl+f",
                    Style::default()
                        .fg(Color::Rgb(150, 120, 100))
                        .add_modifier(Modifier::DIM),
                ),
            ])
        } else {
            Line::from(footer_spans)
        };

        let footer_paragraph =
            Paragraph::new(footer_line).alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(footer_paragraph, chunks[5]);
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
            visible_row_count: self.visible_row_count,
            actions: self.actions.clone(),
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
                tip: None,
                provider_id: "provider1".to_string(),
            },
            DialogItem {
                id: "2".to_string(),
                name: "Model B".to_string(),
                group: "Provider1".to_string(),
                description: "Description for Model B".to_string(),
                tip: None,
                provider_id: "provider1".to_string(),
            },
            DialogItem {
                id: "3".to_string(),
                name: "Model C".to_string(),
                group: "Provider2".to_string(),
                description: "Description for Model C".to_string(),
                tip: None,
                provider_id: "provider2".to_string(),
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
