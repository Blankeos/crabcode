use crate::theme::{contrast_text, ThemeColors};
use crate::ui::scrollbar::{
    render_scrollbar, scrollbar_grab_offset, scrollbar_offset_from_row_with_grab, ScrollMetrics,
};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    prelude::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, ScrollbarState},
    Frame,
};
use std::collections::{HashMap, HashSet};
use tui_textarea::{Input as TuiInput, TextArea};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SEARCH_AREA_HEIGHT: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DialogPosition {
    Left,
    Center,
    Right,
}

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
    scrollbar_drag_offset: Option<u16>,
    pub visible_row_count: usize,
    pub actions: Vec<DialogAction>,
    pub position: DialogPosition,
    pub pending_delete_id: Option<String>,
    collapsible_groups: bool,
    collapsed_groups: HashSet<String>,
    focusable_group_headers: bool,
    focused_group_header: Option<String>,
    matcher: Matcher,
}

impl Dialog {
    fn group_has_header(group: &str) -> bool {
        !group.is_empty()
    }

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
            scrollbar_drag_offset: None,
            visible_row_count: 0,
            actions: Vec::new(),
            position: DialogPosition::Center,
            pending_delete_id: None,
            collapsible_groups: false,
            collapsed_groups: HashSet::new(),
            focusable_group_headers: false,
            focused_group_header: None,
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    pub fn with_position(mut self, position: DialogPosition) -> Self {
        self.position = position;
        self
    }

    pub fn with_collapsible_groups(mut self, enabled: bool) -> Self {
        self.collapsible_groups = enabled;
        if !enabled {
            self.collapsed_groups.clear();
        }
        self
    }

    pub fn with_focusable_group_headers(mut self, enabled: bool) -> Self {
        self.focusable_group_headers = enabled;
        if !enabled {
            self.focused_group_header = None;
        }
        self
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

        let valid_groups: HashSet<String> = self.groups.iter().cloned().collect();
        self.collapsed_groups
            .retain(|group| valid_groups.contains(group));
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
        self.is_dragging_scrollbar = false;
        self.scrollbar_drag_offset = None;
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
        self.search_textarea = TextArea::default();
        self.search_textarea.set_placeholder_text("Search");
        if !self.search_query.is_empty() {
            self.search_textarea.insert_str(&self.search_query);
        }
        self.apply_filter();
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.search_textarea = TextArea::default();
        self.search_textarea.set_placeholder_text("Search");
        self.apply_filter();
    }

    pub fn is_group_collapsed(&self, group: &str) -> bool {
        self.collapsible_groups && self.collapsed_groups.contains(group)
    }

    pub fn toggle_group_collapsed(&mut self, group: &str) {
        if !self.collapsible_groups {
            return;
        }

        if self.collapsed_groups.contains(group) {
            self.collapsed_groups.remove(group);
        } else {
            self.collapsed_groups.insert(group.to_string());
        }

        self.reconcile_selection_after_filter(None);
        self.update_scrollbar();
    }

    pub fn focus_group_header(&mut self, group: &str) -> bool {
        if !self.focusable_group_headers || !Self::group_has_header(group) {
            return false;
        }

        if !self
            .filtered_items
            .iter()
            .any(|(candidate, items)| candidate == group && !items.is_empty())
        {
            return false;
        }

        self.focused_group_header = Some(group.to_string());
        self.adjust_scroll();
        true
    }

    pub fn get_focused_group_header(&self) -> Option<&str> {
        self.focused_group_header.as_deref()
    }

    pub fn collapsed_groups(&self) -> HashSet<String> {
        self.collapsed_groups.clone()
    }

    pub fn set_collapsed_groups(&mut self, groups: HashSet<String>) {
        self.collapsed_groups = if self.collapsible_groups {
            groups
        } else {
            HashSet::new()
        };

        let valid_groups: HashSet<String> = self.groups.iter().cloned().collect();
        self.collapsed_groups
            .retain(|group| valid_groups.contains(group));
        self.reconcile_selection_after_filter(None);
        self.update_scrollbar();
    }

    pub fn preserve_scrollbar_drag_state_from(&mut self, previous: &Self) {
        self.is_dragging_scrollbar = previous.is_dragging_scrollbar;
        self.scrollbar_drag_offset = previous.scrollbar_drag_offset;
    }

    fn apply_filter(&mut self) {
        let preferred_selected = self
            .get_selected()
            .map(|item| (item.id.clone(), item.provider_id.clone()));

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
                    .map(|item| {
                        let base = format!("{} {}", group, item.name);
                        match &item.tip {
                            Some(tip) => format!("{} {}", base, tip),
                            None => base,
                        }
                    })
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
                                .find(|item| {
                                    let base = format!("{} {}", group, item.name);
                                    let s = match &item.tip {
                                        Some(tip) => format!("{} {}", base, tip),
                                        None => base,
                                    };
                                    s == *combined_str
                                })
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

        self.reconcile_selection_after_filter(preferred_selected);
    }

    fn reconcile_selection_after_filter(&mut self, preferred_selected: Option<(String, String)>) {
        let flat_len = self.get_flat_items().len();
        if flat_len == 0 {
            if let Some(group) = self.focused_group_header.clone() {
                if self.focus_group_header(&group) {
                    return;
                }
            }
            self.focused_group_header = None;
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.update_scrollbar();
            return;
        }

        if let Some(group) = self.focused_group_header.clone() {
            if self.focus_group_header(&group) {
                return;
            }
            self.focused_group_header = None;
        }

        if let Some((id, provider_id)) = preferred_selected {
            let selected_pos = {
                let flat_items = self.get_flat_items();
                flat_items
                    .iter()
                    .position(|item| item.id == id && item.provider_id == provider_id)
                    .or_else(|| flat_items.iter().position(|item| item.id == id))
            };

            if let Some(pos) = selected_pos {
                self.selected_index = pos;
                self.focused_group_header = None;
                self.adjust_scroll();
                return;
            }
        }

        if self.selected_index >= flat_len {
            self.selected_index = 0;
        }

        self.adjust_scroll();
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
        let flat_len = self.get_flat_items().len();
        if flat_len == 0 {
            return;
        }

        self.focused_group_header = None;

        if self.selected_index >= flat_len {
            self.selected_index = 0;
            self.adjust_scroll();
            return;
        }

        if self.selected_index < flat_len - 1 {
            self.selected_index += 1;
            self.adjust_scroll();
        }
    }

    pub fn previous(&mut self) {
        let flat_len = self.get_flat_items().len();
        if flat_len == 0 {
            return;
        }

        self.focused_group_header = None;

        if self.selected_index >= flat_len {
            self.selected_index = flat_len.saturating_sub(1);
            self.adjust_scroll();
            return;
        }

        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.adjust_scroll();
        }
    }

    pub fn next_wrapping(&mut self) {
        if self.focusable_group_headers {
            self.move_focus_wrapping(1);
            return;
        }

        let flat_len = self.get_flat_items().len();
        if flat_len == 0 {
            return;
        }

        self.focused_group_header = None;
        self.selected_index = if self.selected_index >= flat_len.saturating_sub(1) {
            0
        } else {
            self.selected_index + 1
        };
        self.adjust_scroll();
    }

    pub fn previous_wrapping(&mut self) {
        if self.focusable_group_headers {
            self.move_focus_wrapping(-1);
            return;
        }

        let flat_len = self.get_flat_items().len();
        if flat_len == 0 {
            return;
        }

        self.focused_group_header = None;
        self.selected_index = if self.selected_index == 0 || self.selected_index >= flat_len {
            flat_len.saturating_sub(1)
        } else {
            self.selected_index - 1
        };
        self.adjust_scroll();
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

    fn move_focus_wrapping(&mut self, delta: isize) {
        let rows = self.focus_rows();
        if rows.is_empty() {
            return;
        }

        let current = self
            .current_focus_row_index(&rows)
            .unwrap_or(if delta >= 0 { 0 } else { rows.len() - 1 });
        let next = if delta >= 0 {
            (current + 1) % rows.len()
        } else if current == 0 {
            rows.len() - 1
        } else {
            current - 1
        };

        self.apply_focus_row(&rows[next]);
        self.adjust_scroll();
    }

    fn focus_rows(&self) -> Vec<DialogFocusRow> {
        let mut rows = Vec::new();
        let mut item_index = 0;

        for (group, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            if self.focusable_group_headers && Self::group_has_header(group) {
                rows.push(DialogFocusRow::Group(group.clone()));
            }

            if self.is_group_collapsed(group) {
                continue;
            }

            for _ in items {
                rows.push(DialogFocusRow::Item(item_index));
                item_index += 1;
            }
        }

        rows
    }

    fn current_focus_row_index(&self, rows: &[DialogFocusRow]) -> Option<usize> {
        if let Some(group) = &self.focused_group_header {
            return rows.iter().position(
                |row| matches!(row, DialogFocusRow::Group(candidate) if candidate == group),
            );
        }

        rows.iter().position(
            |row| matches!(row, DialogFocusRow::Item(index) if *index == self.selected_index),
        )
    }

    fn apply_focus_row(&mut self, row: &DialogFocusRow) {
        match row {
            DialogFocusRow::Group(group) => {
                self.focused_group_header = Some(group.clone());
            }
            DialogFocusRow::Item(index) => {
                self.focused_group_header = None;
                self.selected_index = *index;
            }
        }
    }

    fn get_flat_items(&self) -> Vec<&DialogItem> {
        let mut items = Vec::new();
        for (group, group_items) in &self.filtered_items {
            if self.is_group_collapsed(group) {
                continue;
            }
            for item in group_items {
                items.push(item);
            }
        }
        items
    }

    fn get_content_line_count(&self) -> usize {
        if self.filtered_items.is_empty() {
            return 1;
        }
        let mut count = 0;
        for (group, items) in &self.filtered_items {
            let header = if Self::group_has_header(group) { 1 } else { 0 };
            let visible_items = if self.is_group_collapsed(group) {
                0
            } else {
                items.len()
            };
            count += visible_items + header;
        }
        count.max(1)
    }

    fn get_line_index_of_item(&self, item_index: usize) -> usize {
        let mut line_index = 0;
        let mut current_item_index = 0;

        for (group, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            if Self::group_has_header(group) {
                line_index += 1;
            }

            if self.is_group_collapsed(group) {
                continue;
            }

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

    fn get_line_index_of_group_header(&self, target_group: &str) -> usize {
        let mut line_index = 0;

        for (group, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            if Self::group_has_header(group) {
                if group == target_group {
                    return line_index;
                }
                line_index += 1;
            }

            if !self.is_group_collapsed(group) {
                line_index += items.len();
            }
        }

        line_index
    }

    pub fn adjust_scroll(&mut self) {
        let visible_rows = self.get_visible_row_count().max(1);
        let selected_line = self
            .focused_group_header
            .as_deref()
            .map(|group| self.get_line_index_of_group_header(group))
            .unwrap_or_else(|| self.get_line_index_of_item(self.selected_index));

        if selected_line < self.scroll_offset {
            self.scroll_offset = selected_line;
        } else if selected_line
            >= self
                .scroll_offset
                .saturating_add(visible_rows.saturating_sub(1))
        {
            self.scroll_offset = selected_line.saturating_sub(visible_rows.saturating_sub(1));
        }

        if self.focused_group_header.is_none() && self.selected_index == 0 {
            self.scroll_offset = 0;
        }

        self.update_scrollbar();
    }

    fn get_visible_row_count(&self) -> usize {
        if self.visible_row_count > 0 {
            self.visible_row_count
        } else {
            const DIALOG_HEIGHT_CENTER: u16 = 25;

            let footer_height = self.footer_height();
            let total_fixed_height = 1 + 1 + SEARCH_AREA_HEIGHT + 1 + footer_height;
            let padding = match self.position {
                DialogPosition::Center => 3u16,
                DialogPosition::Left | DialogPosition::Right => 1u16,
            };
            let padding_total = padding * 2;

            match self.position {
                DialogPosition::Center => {
                    let list_area_height =
                        DIALOG_HEIGHT_CENTER.saturating_sub(total_fixed_height + padding_total);
                    list_area_height as usize
                }
                DialogPosition::Left | DialogPosition::Right => {
                    // Side panels use full height, minus fixed chrome + padding
                    let list_area_height = 40u16.saturating_sub(total_fixed_height + padding_total);
                    list_area_height as usize
                }
            }
        }
    }

    pub fn get_selected(&self) -> Option<&DialogItem> {
        if self.focused_group_header.is_some() {
            return None;
        }

        let flat_items = self.get_flat_items();
        flat_items.get(self.selected_index).copied()
    }

    pub fn select_item_by_key(&mut self, id: &str, provider_id: &str) -> bool {
        let flat_items = self.get_flat_items();
        if let Some(pos) = flat_items
            .iter()
            .position(|item| item.id == id && item.provider_id == provider_id)
        {
            self.selected_index = pos;
            self.focused_group_header = None;
            self.adjust_scroll();
            return true;
        }
        false
    }

    pub fn select_item_by_id(&mut self, id: &str) -> bool {
        let flat_items = self.get_flat_items();
        if let Some(pos) = flat_items.iter().position(|item| item.id == id) {
            self.selected_index = pos;
            self.focused_group_header = None;
            self.adjust_scroll();
            return true;
        }
        false
    }

    pub fn select_index_clamped(&mut self, index: usize) -> bool {
        let item_count = self.get_flat_items().len();
        if item_count == 0 {
            self.focused_group_header = None;
            self.selected_index = 0;
            self.scroll_offset = 0;
            self.update_scrollbar();
            return false;
        }

        self.focused_group_header = None;
        self.selected_index = index.min(item_count.saturating_sub(1));
        self.adjust_scroll();
        true
    }

    fn footer_height(&self) -> u16 {
        if self.actions.len() > 4 {
            3
        } else if self.actions.len() > 2 {
            2
        } else {
            1
        }
    }

    fn layout_constraints(&self) -> [ratatui::layout::Constraint; 6] {
        [
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(SEARCH_AREA_HEIGHT),
            ratatui::layout::Constraint::Min(0),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(self.footer_height()),
        ]
    }

    fn truncate_to_width(text: &str, max_width: usize) -> String {
        if max_width == 0 {
            return String::new();
        }

        if text.width() <= max_width {
            return text.to_string();
        }

        const ELLIPSIS: &str = "...";
        let ellipsis_width = ELLIPSIS.width();
        if max_width <= ellipsis_width {
            return ".".repeat(max_width);
        }

        let content_width = max_width - ellipsis_width;
        let mut result = String::new();
        let mut width = 0usize;

        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > content_width {
                break;
            }
            result.push(ch);
            width += ch_width;
        }

        result.push_str(ELLIPSIS);
        result
    }

    fn left_item_spans_for_width(
        item: &DialogItem,
        width: usize,
        colors: ThemeColors,
    ) -> (Vec<Span<'static>>, usize) {
        if width == 0 {
            return (Vec::new(), 0);
        }

        let indent_width = width.min(2);
        let indent = " ".repeat(indent_width);
        let has_description = !item.description.is_empty();

        if !has_description {
            let name = Self::truncate_to_width(&item.name, width.saturating_sub(indent_width));
            let text = format!("{indent}{name}");
            let text_width = text.width();
            return (vec![Span::raw(text)], text_width);
        }

        let separator_width = 2usize;
        let full_name_width = item.name.width();
        if indent_width + full_name_width + separator_width >= width {
            let name = Self::truncate_to_width(&item.name, width.saturating_sub(indent_width));
            let text = format!("{indent}{name}");
            let text_width = text.width();
            return (vec![Span::raw(text)], text_width);
        }

        let desc_budget = width.saturating_sub(indent_width + full_name_width + separator_width);
        let description = Self::truncate_to_width(&item.description, desc_budget);
        let prefix = format!("{indent}{}  ", item.name);
        let text_width = prefix.width() + description.width();

        (
            vec![
                Span::raw(prefix),
                Span::styled(
                    description,
                    Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM),
                ),
            ],
            text_width,
        )
    }

    fn item_spans_for_width(
        item: &DialogItem,
        width: usize,
        colors: ThemeColors,
    ) -> Vec<Span<'static>> {
        if width == 0 {
            return Vec::new();
        }

        let has_description = !item.description.is_empty();
        let tip = item
            .tip
            .as_ref()
            .map(|tip| Self::truncate_to_width(tip, width));
        let tip_width = tip.as_ref().map(|tip| tip.width()).unwrap_or(0);
        let minimum_gap = if tip_width > 0 && width > tip_width {
            1
        } else {
            0
        };
        let left_budget = width.saturating_sub(tip_width + minimum_gap);
        let (mut spans, left_width) = Self::left_item_spans_for_width(item, left_budget, colors);

        if let Some(tip) = tip {
            let padding_len = width.saturating_sub(left_width + tip_width);
            spans.push(Span::raw(" ".repeat(padding_len)));

            let tip_style = if has_description {
                Style::default()
                    .fg(colors.text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(tip, tip_style));
        } else {
            spans.push(Span::raw(" ".repeat(width.saturating_sub(left_width))));
        }

        spans
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

        let padding = match self.position {
            DialogPosition::Center => 3u16,
            DialogPosition::Left | DialogPosition::Right => 1u16,
        };
        let content_area = Rect {
            x: self.dialog_area.x + padding,
            y: self.dialog_area.y + padding,
            width: self.dialog_area.width.saturating_sub(padding * 2),
            height: self.dialog_area.height.saturating_sub(padding * 2),
        };

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints(self.layout_constraints())
            .split(content_area);

        let list_area = chunks[3];
        let scrollbar_area = Rect {
            x: list_area.x + list_area.width.saturating_sub(1),
            y: list_area.y,
            width: 1,
            height: list_area.height,
        };

        if self.is_dragging_scrollbar {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.scroll_to_position(event.row, scrollbar_area);
                    return true;
                }
                MouseEventKind::Up(_) => {
                    self.is_dragging_scrollbar = false;
                    self.scrollbar_drag_offset = None;
                    return true;
                }
                _ => {}
            }
        }

        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && !self.dialog_area.contains(point)
        {
            self.hide();
            return true;
        }

        if matches!(
            event.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
        ) && self.dialog_area.contains(point)
        {
            match event.kind {
                MouseEventKind::ScrollDown => self.scroll_down(),
                MouseEventKind::ScrollUp => self.scroll_up(),
                _ => {}
            }
            return true;
        }

        if !content_area.contains(point) {
            self.is_dragging_scrollbar = false;
            self.scrollbar_drag_offset = None;
            return false;
        }

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
                    let total_lines = self.get_content_line_count();
                    let visible_rows = scrollbar_area.height as usize;
                    let metrics = ScrollMetrics::new(total_lines, visible_rows, self.scroll_offset);
                    if let Some(grab_offset) =
                        scrollbar_grab_offset(metrics, scrollbar_area, event.row)
                    {
                        self.is_dragging_scrollbar = true;
                        self.scrollbar_drag_offset = Some(grab_offset);
                        self.scroll_to_position(event.row, scrollbar_area);
                        true
                    } else {
                        false
                    }
                } else {
                    if let Some(item_index) = self.item_index_at_position(event.column, event.row) {
                        self.selected_index = item_index;
                        self.focused_group_header = None;
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
                    if let Some(item_index) = self.item_index_at_position(event.column, event.row) {
                        if item_index != self.selected_index || self.focused_group_header.is_some()
                        {
                            self.selected_index = item_index;
                            self.focused_group_header = None;
                        }
                    }
                }
                false
            }
            MouseEventKind::Up(_) => {
                if self.is_dragging_scrollbar {
                    self.is_dragging_scrollbar = false;
                    self.scrollbar_drag_offset = None;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub fn item_index_at_position(&self, column: u16, row: u16) -> Option<usize> {
        if !self.visible {
            return None;
        }

        use ratatui::layout::Position;
        let point = Position::new(column, row);

        if !self.dialog_area.contains(point) {
            return None;
        }

        let padding = match self.position {
            DialogPosition::Center => 3u16,
            DialogPosition::Left | DialogPosition::Right => 1u16,
        };
        let content_area = Rect {
            x: self.dialog_area.x + padding,
            y: self.dialog_area.y + padding,
            width: self.dialog_area.width.saturating_sub(padding * 2),
            height: self.dialog_area.height.saturating_sub(padding * 2),
        };

        if !content_area.contains(point) {
            return None;
        }

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints(self.layout_constraints())
            .split(content_area);

        let list_area = chunks[3];
        let list_content_area = Rect {
            x: list_area.x,
            y: list_area.y,
            width: list_area.width.saturating_sub(2),
            height: list_area.height,
        };

        if !list_content_area.contains(point) {
            return None;
        }

        self.get_item_index_from_y(row, list_area)
    }

    pub fn group_at_position(&self, column: u16, row: u16) -> Option<String> {
        if !self.visible || !self.collapsible_groups {
            return None;
        }

        use ratatui::layout::Position;
        let point = Position::new(column, row);

        if !self.dialog_area.contains(point) {
            return None;
        }

        let padding = match self.position {
            DialogPosition::Center => 3u16,
            DialogPosition::Left | DialogPosition::Right => 1u16,
        };
        let content_area = Rect {
            x: self.dialog_area.x + padding,
            y: self.dialog_area.y + padding,
            width: self.dialog_area.width.saturating_sub(padding * 2),
            height: self.dialog_area.height.saturating_sub(padding * 2),
        };

        if !content_area.contains(point) {
            return None;
        }

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints(self.layout_constraints())
            .split(content_area);

        let list_area = chunks[3];
        let list_content_area = Rect {
            x: list_area.x,
            y: list_area.y,
            width: list_area.width.saturating_sub(2),
            height: list_area.height,
        };

        if !list_content_area.contains(point) {
            return None;
        }

        let relative_y = row.saturating_sub(list_area.y) as usize;
        let content_line = self.scroll_offset + relative_y;
        self.get_group_from_line(content_line)
    }

    fn get_item_index_from_y(&self, row: u16, list_area: Rect) -> Option<usize> {
        let relative_y = row.saturating_sub(list_area.y) as usize;
        let content_line = self.scroll_offset + relative_y;
        self.get_item_index_from_line(content_line)
    }

    fn get_group_from_line(&self, line: usize) -> Option<String> {
        let mut current_line = 0;

        for (group, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            if Self::group_has_header(group) {
                if line == current_line {
                    return Some(group.clone());
                }
                current_line += 1;
            }

            let visible_items = if self.is_group_collapsed(group) {
                0
            } else {
                items.len()
            };

            if line < current_line + visible_items {
                return None;
            }

            current_line += visible_items;
        }

        None
    }

    fn get_item_index_from_line(&self, line: usize) -> Option<usize> {
        let mut current_line = 0;
        let mut item_index = 0;

        for (group, items) in &self.filtered_items {
            if items.is_empty() {
                continue;
            }

            let items_start_line = if Self::group_has_header(group) {
                current_line + 1
            } else {
                current_line
            };
            let visible_items = if self.is_group_collapsed(group) {
                0
            } else {
                items.len()
            };
            let items_end_line = items_start_line + visible_items;

            if line >= items_start_line && line < items_end_line {
                return Some(item_index + (line - items_start_line));
            }

            current_line = items_end_line;
            item_index += visible_items;
        }

        None
    }

    fn scroll_to_position(&mut self, row: u16, scrollbar_area: Rect) {
        let total_lines = self.get_content_line_count();
        if total_lines == 0 {
            return;
        }

        let visible_rows = scrollbar_area.height as usize;
        let max_offset = total_lines.saturating_sub(visible_rows);
        let metrics = ScrollMetrics::new(total_lines, visible_rows, self.scroll_offset);
        let grab_offset = self
            .scrollbar_drag_offset
            .or_else(|| scrollbar_grab_offset(metrics, scrollbar_area, row))
            .unwrap_or(0);
        let new_offset =
            scrollbar_offset_from_row_with_grab(metrics, scrollbar_area, row, grab_offset);
        self.scroll_offset = new_offset.min(max_offset);

        let flat_items = self.get_flat_items();
        if !flat_items.is_empty() && visible_rows > 0 {
            let item_at_offset = self.get_item_index_from_line(self.scroll_offset);
            if let Some(idx) = item_at_offset {
                self.selected_index = idx;
                self.focused_group_header = None;
            }
        }

        self.update_scrollbar();
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
        if !self.visible {
            return;
        }

        const DIALOG_WIDTH_CENTER: u16 = 70;
        const DIALOG_HEIGHT_CENTER: u16 = 25;
        const DIALOG_WIDTH_SIDE: u16 = 45;

        match self.position {
            DialogPosition::Center => {
                let dialog_width = area.width.min(DIALOG_WIDTH_CENTER);
                let dialog_height = area.height.min(DIALOG_HEIGHT_CENTER);

                self.dialog_area = Rect {
                    x: (area.width - dialog_width) / 2,
                    y: (area.height - dialog_height) / 2,
                    width: dialog_width,
                    height: dialog_height,
                };
            }
            DialogPosition::Right => {
                let dialog_width = area.width.min(DIALOG_WIDTH_SIDE);

                self.dialog_area = Rect {
                    x: area.width.saturating_sub(dialog_width),
                    y: area.y,
                    width: dialog_width,
                    height: area.height,
                };
            }
            DialogPosition::Left => {
                let dialog_width = area.width.min(DIALOG_WIDTH_SIDE);

                self.dialog_area = Rect {
                    x: area.x,
                    y: area.y,
                    width: dialog_width,
                    height: area.height,
                };
            }
        }

        frame.render_widget(Clear, self.dialog_area);

        let padding = match self.position {
            DialogPosition::Center => 3u16,
            DialogPosition::Left | DialogPosition::Right => 1u16,
        };
        self.content_area = Rect {
            x: self.dialog_area.x + padding,
            y: self.dialog_area.y + padding,
            width: self.dialog_area.width.saturating_sub(padding * 2),
            height: self.dialog_area.height.saturating_sub(padding * 2),
        };

        frame.render_widget(
            ratatui::widgets::Paragraph::new("")
                .style(ratatui::style::Style::default().bg(colors.dialog_background)),
            self.dialog_area,
        );

        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints(self.layout_constraints())
            .split(self.content_area);

        let esc_text = "esc";
        let esc_area_width = (esc_text.width() as u16).saturating_add(1);
        let header_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(esc_area_width),
            ])
            .split(chunks[0]);

        let title_paragraph = Paragraph::new(Line::from(vec![Span::styled(
            &self.title,
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(title_paragraph, header_chunks[0]);

        let esc_paragraph = Paragraph::new(Line::from(vec![Span::styled(
            esc_text,
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(ratatui::layout::Alignment::Right);
        frame.render_widget(esc_paragraph, header_chunks[1]);

        frame.render_widget(&self.search_textarea, chunks[2]);

        let mut content_lines = Vec::new();
        let list_area_width = chunks[3].width.saturating_sub(2); // Subtract scrollbar width
        let filtered_items = self.filtered_items.clone();

        if self.filtered_items.is_empty() {
            content_lines.push(Line::from(vec![Span::styled(
                "No results found",
                Style::default().fg(colors.text_weak),
            )]));
        } else {
            let mut item_index = 0;

            for (group, items) in &filtered_items {
                if items.is_empty() {
                    continue;
                }

                if Self::group_has_header(group) {
                    let header_spans = if self.collapsible_groups {
                        let chevron = if self.is_group_collapsed(group) {
                            "⏷"
                        } else {
                            "⏶"
                        };
                        let chevron_width = chevron.width();
                        let group = Self::truncate_to_width(
                            group,
                            (list_area_width as usize).saturating_sub(chevron_width),
                        );
                        let padding_len = (list_area_width as usize)
                            .saturating_sub(group.width() + chevron_width);
                        vec![
                            Span::styled(
                                group,
                                Style::default()
                                    .fg(colors.primary)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(" ".repeat(padding_len)),
                            Span::styled(
                                chevron,
                                Style::default()
                                    .fg(colors.primary)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]
                    } else {
                        vec![Span::styled(
                            Self::truncate_to_width(group, list_area_width as usize),
                            Style::default()
                                .fg(colors.primary)
                                .add_modifier(Modifier::BOLD),
                        )]
                    };

                    let mut header_spans = header_spans;
                    if self.focused_group_header.as_deref() == Some(group.as_str()) {
                        let fg = contrast_text(colors.primary);
                        for span in &mut header_spans {
                            let mut style = span.style.clone();
                            style = style.fg(fg).bg(colors.primary);
                            span.style = style;
                        }
                    }

                    content_lines.push(Line::from(header_spans));
                }

                if self.is_group_collapsed(group) {
                    continue;
                }

                for item in items {
                    let is_selected =
                        self.focused_group_header.is_none() && item_index == self.selected_index;
                    let is_pending_delete = self.pending_delete_id.as_ref() == Some(&item.id);
                    let mut spans =
                        Self::item_spans_for_width(item, list_area_width as usize, colors);

                    if is_selected {
                        let fg = contrast_text(colors.primary);
                        for span in &mut spans {
                            let mut style = span.style.clone();
                            style = style.fg(fg).bg(colors.primary);
                            span.style = style;
                        }
                    } else if is_pending_delete {
                        let fg = contrast_text(colors.error);
                        for span in &mut spans {
                            let mut style = span.style.clone();
                            style = style.fg(fg).bg(colors.error);
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

        let scrollbar_area = Rect {
            x: chunks[3].x + chunks[3].width.saturating_sub(1),
            y: chunks[3].y,
            width: 1,
            height: chunks[3].height,
        };
        render_scrollbar(
            frame,
            ScrollMetrics::new(
                self.get_content_line_count(),
                self.visible_row_count,
                self.scroll_offset,
            ),
            scrollbar_area,
            colors.background_element,
            colors.text_weak,
        );

        let footer_paragraph = Paragraph::new(self.footer_lines(chunks[5].width, colors))
            .alignment(ratatui::layout::Alignment::Left);
        frame.render_widget(footer_paragraph, chunks[5]);
    }

    fn footer_lines(&self, width: u16, colors: ThemeColors) -> Vec<Line<'static>> {
        if self.actions.is_empty() {
            return vec![Line::from(vec![])];
        }

        let max_lines = self.footer_height() as usize;
        let max_width = width.max(1) as usize;
        let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
        let mut current: Vec<Span<'static>> = Vec::new();
        let mut current_width = 0usize;

        for action in &self.actions {
            let action_width = action.label.width() + action.key.width() + 2;
            let spacer_width = if current.is_empty() { 0 } else { 2 };

            if !current.is_empty()
                && current_width + spacer_width + action_width > max_width
                && lines.len() + 1 < max_lines
            {
                lines.push(current);
                current = Vec::new();
                current_width = 0;
            }

            if !current.is_empty() {
                current.push(Span::raw("  "));
                current_width += 2;
            }

            current.push(Span::styled(
                action.label.clone(),
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            current.push(Span::raw("  "));
            current.push(Span::styled(
                action.key.clone(),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
            current_width += action_width;
        }

        lines.push(current);
        while lines.len() < max_lines {
            lines.push(Vec::new());
        }

        lines.into_iter().map(Line::from).collect()
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
            scrollbar_drag_offset: self.scrollbar_drag_offset,
            visible_row_count: self.visible_row_count,
            actions: self.actions.clone(),
            position: self.position,
            pending_delete_id: self.pending_delete_id.clone(),
            collapsible_groups: self.collapsible_groups,
            collapsed_groups: self.collapsed_groups.clone(),
            focusable_group_headers: self.focusable_group_headers,
            focused_group_header: self.focused_group_header.clone(),
            matcher: Matcher::new(Config::DEFAULT),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DialogFocusRow {
    Group(String),
    Item(usize),
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

    fn create_many_test_items(count: usize) -> Vec<DialogItem> {
        (0..count)
            .map(|idx| DialogItem {
                id: idx.to_string(),
                name: format!("Model {}", idx),
                group: "Group".to_string(),
                description: "".to_string(),
                tip: None,
                provider_id: "p".to_string(),
            })
            .collect()
    }

    fn create_fuzzy_test_items() -> Vec<DialogItem> {
        vec![
            DialogItem {
                id: "1".to_string(),
                name: "gitlab".to_string(),
                group: "Other".to_string(),
                description: "".to_string(),
                tip: None,
                provider_id: "p".to_string(),
            },
            DialogItem {
                id: "2".to_string(),
                name: "github".to_string(),
                group: "Other".to_string(),
                description: "".to_string(),
                tip: None,
                provider_id: "p".to_string(),
            },
            DialogItem {
                id: "3".to_string(),
                name: "gruvbox".to_string(),
                group: "Other".to_string(),
                description: "".to_string(),
                tip: None,
                provider_id: "p".to_string(),
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
    fn test_dialog_click_outside_closes_modal() {
        let mut dialog = Dialog::new("Test");
        dialog.show();
        dialog.dialog_area = Rect {
            x: 10,
            y: 10,
            width: 30,
            height: 10,
        };

        let handled = dialog.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });

        assert!(handled);
        assert!(!dialog.is_visible());
    }

    #[test]
    fn test_dialog_click_inside_keeps_modal_open() {
        let mut dialog = Dialog::new("Test");
        dialog.show();
        dialog.dialog_area = Rect {
            x: 10,
            y: 10,
            width: 30,
            height: 10,
        };

        let handled = dialog.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 11,
            row: 11,
            modifiers: KeyModifiers::NONE,
        });

        assert!(!handled);
        assert!(dialog.is_visible());
    }

    #[test]
    fn test_dialog_scrollbar_drag_continues_outside_content_area() {
        let mut dialog = Dialog::with_items("Models", create_many_test_items(40));
        dialog.show();
        dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        };
        dialog.visible_row_count = 8;

        let handled = dialog.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 36,
            row: 8,
            modifiers: KeyModifiers::NONE,
        });

        assert!(handled);
        assert!(dialog.is_dragging_scrollbar);

        let handled = dialog.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 80,
            row: 14,
            modifiers: KeyModifiers::NONE,
        });

        assert!(handled);
        assert!(dialog.is_dragging_scrollbar);
        assert_eq!(
            dialog.scroll_offset,
            dialog
                .get_content_line_count()
                .saturating_sub(dialog.get_visible_row_count())
        );

        let handled = dialog.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 80,
            row: 14,
            modifiers: KeyModifiers::NONE,
        });

        assert!(handled);
        assert!(!dialog.is_dragging_scrollbar);
        assert_eq!(dialog.scrollbar_drag_offset, None);
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
    fn test_dialog_next_from_invalid_selection_focuses_first() {
        let mut dialog = Dialog::with_items("Providers", create_fuzzy_test_items());
        dialog.set_search_query("gu");
        assert!(!dialog.get_flat_items().is_empty());

        dialog.selected_index = 999;
        dialog.next();
        assert_eq!(dialog.selected_index, 0);
        assert!(dialog.get_selected().is_some());
    }

    #[test]
    fn test_dialog_search_preserves_selected_item_if_still_present() {
        let mut dialog = Dialog::with_items("Providers", create_fuzzy_test_items());

        dialog.selected_index = 1;
        assert_eq!(dialog.get_selected().unwrap().name, "github");

        dialog.set_search_query("gu");
        assert_eq!(dialog.get_selected().unwrap().name, "github");
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
    fn test_dialog_select_index_clamped_uses_last_available_item() {
        let mut dialog = Dialog::with_items("Models", create_test_items());

        assert!(dialog.select_index_clamped(99));

        assert_eq!(dialog.selected_index, 2);
        assert_eq!(dialog.get_selected().unwrap().name, "Model C");
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
