use crate::theme::{contrast_text, ThemeColors};
use crate::ui::scrollbar::{
    render_scrollbar, scrollbar_grab_offset, scrollbar_offset_from_row_with_grab, ScrollMetrics,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionDialogItem {
    pub id: String,
    pub key: char,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionDialogEvent {
    None,
    Close,
    Select,
    Shortcut(char),
}

#[derive(Debug, Clone)]
pub struct ActionDialog {
    title: String,
    pub items: Vec<ActionDialogItem>,
    selected_index: usize,
    visible: bool,
    dialog_area: Rect,
    content_area: Rect,
    shortcut_hitboxes: Vec<(Rect, KeyCode)>,
    scroll_offset: usize,
    visible_row_count: usize,
    is_dragging_scrollbar: bool,
    scrollbar_drag_offset: Option<u16>,
}

impl ActionDialog {
    pub fn with_items(title: impl Into<String>, items: Vec<ActionDialogItem>) -> Self {
        Self {
            title: title.into(),
            items,
            selected_index: 0,
            visible: false,
            dialog_area: Rect::default(),
            content_area: Rect::default(),
            shortcut_hitboxes: Vec::new(),
            scroll_offset: 0,
            visible_row_count: 0,
            is_dragging_scrollbar: false,
            scrollbar_drag_offset: None,
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.selected_index = self.selected_index.min(self.items.len().saturating_sub(1));
        self.adjust_scroll();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.shortcut_hitboxes.clear();
        self.is_dragging_scrollbar = false;
        self.scrollbar_drag_offset = None;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn get_selected(&self) -> Option<&ActionDialogItem> {
        self.items.get(self.selected_index)
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn select_item_by_id(&mut self, id: &str) -> bool {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            self.selected_index = index;
            self.adjust_scroll();
            true
        } else {
            false
        }
    }

    pub fn item_id_for_shortcut(&self, key: char) -> Option<String> {
        let key = key.to_ascii_lowercase();
        self.items
            .iter()
            .find(|item| item.key.to_ascii_lowercase() == key)
            .map(|item| item.id.clone())
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> ActionDialogEvent {
        if !self.visible {
            return ActionDialogEvent::None;
        }

        match event.code {
            KeyCode::Esc => {
                self.hide();
                ActionDialogEvent::Close
            }
            KeyCode::Enter => ActionDialogEvent::Select,
            KeyCode::Up | KeyCode::Char('k') if event.modifiers == KeyModifiers::NONE => {
                self.previous();
                ActionDialogEvent::None
            }
            KeyCode::Down | KeyCode::Char('j') if event.modifiers == KeyModifiers::NONE => {
                self.next();
                ActionDialogEvent::None
            }
            KeyCode::Char(ch) if event.modifiers == KeyModifiers::NONE => {
                let shortcut = ch.to_ascii_lowercase();
                if self.item_id_for_shortcut(shortcut).is_some() {
                    ActionDialogEvent::Shortcut(shortcut)
                } else {
                    ActionDialogEvent::None
                }
            }
            KeyCode::Char('c') if event.modifiers == KeyModifiers::CONTROL => {
                ActionDialogEvent::None
            }
            _ => ActionDialogEvent::None,
        }
    }

    pub fn handle_mouse_event(&mut self, event: MouseEvent) -> ActionDialogEvent {
        if !self.visible {
            return ActionDialogEvent::None;
        }

        let point = Position::new(event.column, event.row);
        let list_area = self.list_area();
        let scrollbar_area = self.scrollbar_area(list_area);

        if self.is_dragging_scrollbar {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.scroll_to_position(event.row, scrollbar_area);
                    return ActionDialogEvent::None;
                }
                MouseEventKind::Up(_) => {
                    self.is_dragging_scrollbar = false;
                    self.scrollbar_drag_offset = None;
                    return ActionDialogEvent::None;
                }
                _ => {}
            }
        }

        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && !self.dialog_area.contains(point)
        {
            self.hide();
            return ActionDialogEvent::Close;
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
            return ActionDialogEvent::None;
        }

        if !self.content_area.contains(point) {
            self.is_dragging_scrollbar = false;
            self.scrollbar_drag_offset = None;
            return ActionDialogEvent::None;
        }

        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some((_, code)) = self
                .shortcut_hitboxes
                .iter()
                .find(|(area, _)| area.contains(point))
            {
                return self.handle_key_event(KeyEvent::new(*code, KeyModifiers::NONE));
            }
        }

        let is_on_scrollbar = scrollbar_area.contains(point);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if is_on_scrollbar {
                    let metrics = ScrollMetrics::new(
                        self.items.len(),
                        scrollbar_area.height as usize,
                        self.scroll_offset,
                    );
                    if let Some(grab_offset) =
                        scrollbar_grab_offset(metrics, scrollbar_area, event.row)
                    {
                        self.is_dragging_scrollbar = true;
                        self.scrollbar_drag_offset = Some(grab_offset);
                        self.scroll_to_position(event.row, scrollbar_area);
                    }
                    return ActionDialogEvent::None;
                }

                if let Some(index) = self.item_index_at_position(event.column, event.row) {
                    self.selected_index = index;
                    self.adjust_scroll();
                }
                ActionDialogEvent::None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(index) = self.item_index_at_position(event.column, event.row) {
                    if index == self.selected_index {
                        return ActionDialogEvent::Select;
                    }
                }
                ActionDialogEvent::None
            }
            MouseEventKind::Moved => {
                if !is_on_scrollbar {
                    if let Some(index) = self.item_index_at_position(event.column, event.row) {
                        self.selected_index = index;
                        self.adjust_scroll();
                    }
                }
                ActionDialogEvent::None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.is_dragging_scrollbar {
                    self.scroll_to_position(event.row, scrollbar_area);
                }
                ActionDialogEvent::None
            }
            _ => ActionDialogEvent::None,
        }
    }

    pub fn contains_position(&self, column: u16, row: u16) -> bool {
        self.visible && self.dialog_area.contains(Position::new(column, row))
    }

    pub fn item_index_at_position(&self, column: u16, row: u16) -> Option<usize> {
        if !self.visible {
            return None;
        }

        let point = Position::new(column, row);
        let list_area = self.list_area();
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
        let index = self.scroll_offset.saturating_add(relative_y);
        (index < self.items.len()).then_some(index)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
        if !self.visible {
            return;
        }

        self.shortcut_hitboxes.clear();

        const DIALOG_WIDTH: u16 = 70;
        const MIN_DIALOG_HEIGHT: u16 = 7;
        const HEADER_HEIGHT: u16 = 1;
        const HEADER_GAP_HEIGHT: u16 = 1;
        const FOOTER_GAP_HEIGHT: u16 = 1;
        const FOOTER_HEIGHT: u16 = 1;
        const PADDING_X: u16 = 3;
        const PADDING_Y: u16 = 2;

        let dialog_width = area.width.min(DIALOG_WIDTH);
        let desired_height = self.items.len() as u16
            + HEADER_HEIGHT
            + HEADER_GAP_HEIGHT
            + FOOTER_GAP_HEIGHT
            + FOOTER_HEIGHT
            + (PADDING_Y * 2);
        let dialog_height = area.height.min(desired_height.max(MIN_DIALOG_HEIGHT));

        self.dialog_area = Rect {
            x: area.x + area.width.saturating_sub(dialog_width) / 2,
            y: area.y + area.height.saturating_sub(dialog_height) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        frame.render_widget(Clear, self.dialog_area);
        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
            self.dialog_area,
        );

        self.content_area = Rect {
            x: self.dialog_area.x.saturating_add(PADDING_X),
            y: self.dialog_area.y.saturating_add(PADDING_Y),
            width: self.dialog_area.width.saturating_sub(PADDING_X * 2),
            height: self.dialog_area.height.saturating_sub(PADDING_Y * 2),
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(HEADER_HEIGHT),
                Constraint::Length(HEADER_GAP_HEIGHT),
                Constraint::Min(0),
                Constraint::Length(FOOTER_GAP_HEIGHT),
                Constraint::Length(FOOTER_HEIGHT),
            ])
            .split(self.content_area);

        self.render_header(frame, chunks[0], colors);
        self.render_items(frame, chunks[2], colors);
        self.render_footer(frame, chunks[4], colors);
    }

    fn render_header(&mut self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
        let esc_text = "esc";
        let esc_area_width = (esc_text.width() as u16).saturating_add(1);
        let header_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(esc_area_width)])
            .split(area);

        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                self.title.clone(),
                Style::default()
                    .fg(colors.text)
                    .add_modifier(Modifier::BOLD),
            )]))
            .alignment(Alignment::Left),
            header_chunks[0],
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                esc_text,
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            )]))
            .alignment(Alignment::Right),
            header_chunks[1],
        );

        let esc_width = (esc_text.width() as u16).min(header_chunks[1].width);
        self.register_shortcut_hitbox(
            Rect::new(
                header_chunks[1].right().saturating_sub(esc_width),
                header_chunks[1].y,
                esc_width,
                header_chunks[1].height.min(1),
            )
            .intersection(frame.area()),
            KeyCode::Esc,
        );
    }

    fn render_items(&mut self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
        let previous_visible_row_count = self.visible_row_count;
        self.visible_row_count = area.height as usize;
        if previous_visible_row_count != self.visible_row_count {
            self.adjust_scroll();
        }

        let list_content_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height,
        };
        let list_width = list_content_area.width as usize;
        let lines = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut spans = Self::item_spans_for_width(item, list_width, colors);
                if index == self.selected_index {
                    let fg = contrast_text(colors.primary);
                    for span in &mut spans {
                        let mut style = span.style;
                        style = style.fg(fg).bg(colors.primary);
                        span.style = style;
                    }
                }
                Line::from(spans)
            })
            .collect::<Vec<_>>();

        frame.render_widget(
            Paragraph::new(lines).scroll((self.scroll_offset as u16, 0)),
            list_content_area,
        );

        let scrollbar_area = self.scrollbar_area(area);
        render_scrollbar(
            frame,
            ScrollMetrics::new(self.items.len(), self.visible_row_count, self.scroll_offset),
            scrollbar_area,
            colors.background_element,
            colors.text_weak,
        );
    }

    fn render_footer(&mut self, frame: &mut Frame, area: Rect, colors: ThemeColors) {
        let primary = Style::default().fg(colors.primary);
        let weak = Style::default().fg(colors.text_weak);
        let hints = [
            (
                Span::styled("enter", primary.add_modifier(Modifier::BOLD)),
                Some(KeyCode::Enter),
            ),
            (Span::styled(" select", weak), Some(KeyCode::Enter)),
            (Span::styled("  ", primary), None),
            (Span::styled("↑", primary), Some(KeyCode::Up)),
            (Span::styled("/", primary), None),
            (Span::styled("↓", primary), Some(KeyCode::Down)),
            (Span::styled(" move", weak), None),
        ];
        let mut x = area.x;
        let mut spans = Vec::with_capacity(hints.len());
        for (span, code) in hints {
            let width = span.width() as u16;
            if let Some(code) = code {
                self.register_shortcut_hitbox(
                    Rect::new(x, area.y, width, area.height.min(1))
                        .intersection(area)
                        .intersection(frame.area()),
                    code,
                );
            }
            x = x.saturating_add(width);
            spans.push(span);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn register_shortcut_hitbox(&mut self, area: Rect, code: KeyCode) {
        if area.width > 0 && area.height > 0 {
            self.shortcut_hitboxes.push((area, code));
        }
    }

    fn item_spans_for_width(
        item: &ActionDialogItem,
        width: usize,
        colors: ThemeColors,
    ) -> Vec<Span<'static>> {
        if width == 0 {
            return Vec::new();
        }

        let key_text = format!("{} ", item.key);
        let key_width = key_text.width();
        let label_width = item.label.width();
        let separator_width = if item.description.is_empty() { 0 } else { 2 };
        let desc_budget = width.saturating_sub(key_width + label_width + separator_width);
        let description = Self::truncate_to_width(&item.description, desc_budget);
        let total_width = key_width + label_width + separator_width + description.width();

        let mut spans = vec![
            Span::styled(
                key_text,
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                item.label.clone(),
                Style::default()
                    .fg(colors.text)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if !description.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                description,
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
        }

        spans.push(Span::raw(" ".repeat(width.saturating_sub(total_width))));
        spans
    }

    fn next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1).min(self.items.len().saturating_sub(1));
        self.adjust_scroll();
    }

    fn previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected_index = self.selected_index.saturating_sub(1);
        self.adjust_scroll();
    }

    fn scroll_down(&mut self) {
        let visible_rows = self.visible_row_count.max(1);
        let max_offset = self.items.len().saturating_sub(visible_rows);
        self.scroll_offset = (self.scroll_offset + 1).min(max_offset);
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn adjust_scroll(&mut self) {
        let visible_rows = self.visible_row_count.max(1);
        let max_offset = self.items.len().saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(max_offset);

        if self.selected_index < self.scroll_offset {
            self.scroll_offset = self.selected_index;
        } else if self.selected_index >= self.scroll_offset.saturating_add(visible_rows) {
            self.scroll_offset = self
                .selected_index
                .saturating_sub(visible_rows.saturating_sub(1));
        }
    }

    fn scroll_to_position(&mut self, row: u16, scrollbar_area: Rect) {
        let visible_rows = scrollbar_area.height as usize;
        let max_offset = self.items.len().saturating_sub(visible_rows);
        let metrics = ScrollMetrics::new(self.items.len(), visible_rows, self.scroll_offset);
        let grab_offset = self
            .scrollbar_drag_offset
            .or_else(|| scrollbar_grab_offset(metrics, scrollbar_area, row))
            .unwrap_or(0);
        self.scroll_offset =
            scrollbar_offset_from_row_with_grab(metrics, scrollbar_area, row, grab_offset)
                .min(max_offset);
    }

    fn list_area(&self) -> Rect {
        const HEADER_HEIGHT: u16 = 1;
        const HEADER_GAP_HEIGHT: u16 = 1;
        const FOOTER_GAP_HEIGHT: u16 = 1;
        const FOOTER_HEIGHT: u16 = 1;
        if self.content_area.height
            <= HEADER_HEIGHT + HEADER_GAP_HEIGHT + FOOTER_GAP_HEIGHT + FOOTER_HEIGHT
        {
            return Rect::default();
        }
        Rect {
            x: self.content_area.x,
            y: self
                .content_area
                .y
                .saturating_add(HEADER_HEIGHT + HEADER_GAP_HEIGHT),
            width: self.content_area.width,
            height: self.content_area.height.saturating_sub(
                HEADER_HEIGHT + HEADER_GAP_HEIGHT + FOOTER_GAP_HEIGHT + FOOTER_HEIGHT,
            ),
        }
    }

    fn scrollbar_area(&self, list_area: Rect) -> Rect {
        Rect {
            x: list_area
                .x
                .saturating_add(list_area.width.saturating_sub(1)),
            y: list_area.y,
            width: 1,
            height: list_area.height,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    fn render_dialog(dialog: &mut ActionDialog, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                dialog.render(
                    frame,
                    frame.area(),
                    Theme::load_builtin_default().get_colors(true),
                );
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn rendered_text_position(buffer: &Buffer, text: &str) -> Position {
        for y in buffer.area.y..buffer.area.bottom() {
            for x in buffer.area.x..buffer.area.right() {
                let mut column = x;
                if text.chars().all(|ch| {
                    let matches = column < buffer.area.right()
                        && buffer[(column, y)].symbol() == ch.to_string();
                    column += UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
                    matches
                }) {
                    return Position::new(x, y);
                }
            }
        }
        panic!("{text:?} not found in rendered dialog");
    }

    fn mouse(kind: MouseEventKind, point: Position) -> MouseEvent {
        MouseEvent {
            kind,
            column: point.x,
            row: point.y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(dialog: &mut ActionDialog, point: Position) -> ActionDialogEvent {
        dialog.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), point))
    }

    fn items(count: usize) -> Vec<ActionDialogItem> {
        (0..count)
            .map(|index| ActionDialogItem {
                id: index.to_string(),
                key: 'a',
                label: format!("Action {index}"),
                description: String::new(),
            })
            .collect()
    }

    #[test]
    fn rendered_esc_hint_closes_dialog_and_hidden_dialog_ignores_clicks() {
        let mut dialog = ActionDialog::with_items("Actions", items(3));
        dialog.show();
        let buffer = render_dialog(&mut dialog, 90, 24);
        let esc = rendered_text_position(&buffer, "esc");

        for x in esc.x..esc.x + 3 {
            let point = Position::new(x, esc.y);
            dialog.show();
            render_dialog(&mut dialog, 90, 24);
            assert_eq!(click(&mut dialog, point), ActionDialogEvent::Close);
            assert!(!dialog.is_visible());
            assert_eq!(click(&mut dialog, point), ActionDialogEvent::None);
        }
        for text in ["enter select", "↑", "↓"] {
            let point = rendered_text_position(&buffer, text);
            assert_eq!(click(&mut dialog, point), ActionDialogEvent::None);
        }
        assert_eq!(dialog.selected_index(), 0);
    }

    #[test]
    fn rendered_enter_select_hint_selects_only_on_left_press() {
        let mut dialog = ActionDialog::with_items("Actions", items(3));
        dialog.show();
        assert!(dialog.select_item_by_id("1"));
        let buffer = render_dialog(&mut dialog, 90, 24);
        let enter = rendered_text_position(&buffer, "enter select");

        for x in enter.x..enter.x + "enter select".width() as u16 {
            let point = Position::new(x, enter.y);
            for kind in [
                MouseEventKind::Moved,
                MouseEventKind::Drag(MouseButton::Left),
                MouseEventKind::Down(MouseButton::Right),
                MouseEventKind::Down(MouseButton::Middle),
                MouseEventKind::Up(MouseButton::Left),
            ] {
                assert_eq!(
                    dialog.handle_mouse_event(mouse(kind, point)),
                    ActionDialogEvent::None
                );
            }
            assert_eq!(click(&mut dialog, point), ActionDialogEvent::Select);
            assert_eq!(
                dialog.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), point)),
                ActionDialogEvent::None
            );
            assert!(dialog.is_visible());
            assert_eq!(dialog.get_selected().unwrap().id, "1");
        }
    }

    #[test]
    fn rendered_arrow_hints_navigate_and_keep_selection_visible() {
        let mut dialog = ActionDialog::with_items("Actions", items(40));
        dialog.show();
        let buffer = render_dialog(&mut dialog, 90, 12);
        let up = rendered_text_position(&buffer, "↑");
        let down = rendered_text_position(&buffer, "↓");
        let mut keyboard_dialog = dialog.clone();

        for (point, code) in [(up, KeyCode::Up), (down, KeyCode::Down), (up, KeyCode::Up)] {
            for _ in 0..45 {
                assert_eq!(
                    click(&mut dialog, point),
                    keyboard_dialog.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE))
                );
                assert_eq!(dialog.selected_index(), keyboard_dialog.selected_index());
                assert_eq!(dialog.scroll_offset, keyboard_dialog.scroll_offset);
                assert_eq!(
                    dialog.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), point)),
                    ActionDialogEvent::None
                );
                assert_eq!(dialog.selected_index(), keyboard_dialog.selected_index());
                assert!(dialog.selected_index >= dialog.scroll_offset);
                assert!(dialog.selected_index < dialog.scroll_offset + dialog.visible_row_count);
            }
        }
    }

    #[test]
    fn unrelated_clicks_preserve_existing_dialog_mouse_behavior() {
        let mut dialog = ActionDialog::with_items("Actions", items(3));
        dialog.show();
        let buffer = render_dialog(&mut dialog, 90, 24);
        let esc = rendered_text_position(&buffer, "esc");
        let enter = rendered_text_position(&buffer, "enter select");
        let up = rendered_text_position(&buffer, "↑");
        let down = rendered_text_position(&buffer, "↓");

        for point in [
            rendered_text_position(&buffer, "Actions"),
            Position::new(esc.x - 1, esc.y),
            Position::new(esc.x, esc.y - 1),
            Position::new(enter.x, enter.y + 1),
            Position::new(up.x - 1, up.y),
            Position::new(up.x + 1, up.y),
            Position::new(down.x + 1, down.y),
            Position::new(dialog.dialog_area.x, dialog.dialog_area.y),
        ] {
            assert_eq!(click(&mut dialog, point), ActionDialogEvent::None);
            assert!(dialog.is_visible());
            assert_eq!(dialog.selected_index(), 0);
        }

        for point in [esc, up, down] {
            for kind in [
                MouseEventKind::Moved,
                MouseEventKind::Down(MouseButton::Right),
                MouseEventKind::Up(MouseButton::Left),
                MouseEventKind::Drag(MouseButton::Left),
            ] {
                assert_eq!(
                    dialog.handle_mouse_event(mouse(kind, point)),
                    ActionDialogEvent::None
                );
                assert!(dialog.is_visible());
                assert_eq!(dialog.selected_index(), 0);
            }
        }

        let item = rendered_text_position(&buffer, "Action 1");
        assert_eq!(click(&mut dialog, item), ActionDialogEvent::None);
        assert_eq!(dialog.selected_index(), 1);
        assert_eq!(
            dialog.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), item)),
            ActionDialogEvent::Select
        );
        assert_eq!(
            click(&mut dialog, Position::new(0, 0)),
            ActionDialogEvent::Close
        );
        assert!(!dialog.is_visible());
    }

    #[test]
    fn rendered_shortcut_hitboxes_follow_clipping_and_resize() {
        let mut dialog = ActionDialog::with_items("Actions", items(3));
        dialog.show();
        let wide_buffer = render_dialog(&mut dialog, 90, 24);
        let old_down = rendered_text_position(&wide_buffer, "↓");

        // Resizing only the height leaves the old footer inside the dialog, but
        // it must no longer respond as a shortcut.
        let buffer = render_dialog(&mut dialog, 90, 20);
        assert_eq!(buffer[(old_down.x, old_down.y)].symbol(), " ");
        assert!(dialog.select_item_by_id("1"));
        assert_eq!(click(&mut dialog, old_down), ActionDialogEvent::None);
        assert_eq!(dialog.selected_index(), 1);
        assert!(dialog.is_visible());

        // Six columns of dialog padding leave only "enter select  ↑/" visible.
        let buffer = render_dialog(&mut dialog, 22, 24);
        let enter = rendered_text_position(&buffer, "enter select");
        let up = rendered_text_position(&buffer, "↑");
        assert_eq!(buffer[(up.x + 1, up.y)].symbol(), "/");
        assert_eq!(buffer[(up.x + 2, up.y)].symbol(), " ");
        assert!(dialog.select_item_by_id("1"));
        assert_eq!(
            click(&mut dialog, Position::new(up.x + 2, up.y)),
            ActionDialogEvent::None
        );
        assert_eq!(dialog.selected_index(), 1);
        assert_eq!(click(&mut dialog, up), ActionDialogEvent::None);
        assert_eq!(dialog.selected_index(), 0);
        assert_eq!(click(&mut dialog, enter), ActionDialogEvent::Select);

        // Right-aligned text wider than the header is clipped to "e". Both
        // the header and footer remain clickable even with one visible cell.
        let buffer = render_dialog(&mut dialog, 7, 24);
        let esc = rendered_text_position(&buffer, "e");
        let enter = Position::new(esc.x, dialog.content_area.bottom() - 1);
        assert_eq!(buffer[(enter.x, enter.y)].symbol(), "e");
        assert_eq!(click(&mut dialog, enter), ActionDialogEvent::Select);
        assert_eq!(click(&mut dialog, esc), ActionDialogEvent::Close);

        dialog.show();
        render_dialog(&mut dialog, 6, 3);
        assert!(dialog.shortcut_hitboxes.is_empty());
        assert_eq!(
            click(&mut dialog, Position::new(3, 1)),
            ActionDialogEvent::None
        );
        assert!(dialog.is_visible());
    }

    #[test]
    fn wheel_scroll_does_not_change_selection() {
        let mut dialog = ActionDialog::with_items("Actions", items(40));
        dialog.visible_row_count = 5;
        dialog.selected_index = 10;

        for _ in 0..20 {
            dialog.scroll_down();
        }
        assert_eq!(dialog.scroll_offset, 20);
        assert_eq!(dialog.selected_index, 10);

        for _ in 0..5 {
            dialog.scroll_up();
        }
        assert_eq!(dialog.scroll_offset, 15);
        assert_eq!(dialog.selected_index, 10);
    }

    #[test]
    fn scrollbar_scroll_does_not_change_selection() {
        let mut dialog = ActionDialog::with_items("Actions", items(40));
        dialog.visible_row_count = 5;
        dialog.selected_index = 10;
        let scrollbar_area = Rect::new(0, 0, 1, 5);

        dialog.scroll_to_position(4, scrollbar_area);

        assert!(dialog.scroll_offset > 0);
        assert_eq!(dialog.selected_index, 10);
    }
}
