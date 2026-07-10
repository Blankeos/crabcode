use crate::terminal_title::{normalized_items, TerminalTitleItem};
use crate::theme::{contrast_text, ThemeColors};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleDialogAction {
    None,
    Changed,
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TitleDialogItem {
    kind: TerminalTitleItem,
    enabled: bool,
}

#[derive(Debug)]
pub struct TitleDialogState {
    items: Vec<TitleDialogItem>,
    original_enabled: Vec<TerminalTitleItem>,
    selected_index: usize,
    visible: bool,
    dialog_area: Rect,
    rows_area: Rect,
}

impl TitleDialogState {
    pub fn new() -> Self {
        Self {
            items: Self::ordered_items(&[]),
            original_enabled: Vec::new(),
            selected_index: 0,
            visible: false,
            dialog_area: Rect::default(),
            rows_area: Rect::default(),
        }
    }

    pub fn show(&mut self, enabled: &[TerminalTitleItem]) {
        let enabled = normalized_items(enabled.iter().copied());
        self.items = Self::ordered_items(&enabled);
        self.original_enabled = enabled;
        self.selected_index = self.selected_index.min(self.items.len().saturating_sub(1));
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn enabled_items(&self) -> Vec<TerminalTitleItem> {
        self.items
            .iter()
            .filter(|item| item.enabled)
            .map(|item| item.kind)
            .collect()
    }

    pub fn original_enabled_items(&self) -> Vec<TerminalTitleItem> {
        self.original_enabled.clone()
    }

    fn ordered_items(enabled: &[TerminalTitleItem]) -> Vec<TitleDialogItem> {
        enabled
            .iter()
            .copied()
            .map(|kind| TitleDialogItem {
                kind,
                enabled: true,
            })
            .chain(
                TerminalTitleItem::ALL
                    .into_iter()
                    .filter(|kind| !enabled.contains(kind))
                    .map(|kind| TitleDialogItem {
                        kind,
                        enabled: false,
                    }),
            )
            .collect()
    }

    fn previous(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    fn next(&mut self) {
        self.selected_index = (self.selected_index + 1).min(self.items.len().saturating_sub(1));
    }

    fn toggle_selected(&mut self) {
        if let Some(item) = self.items.get_mut(self.selected_index) {
            item.enabled = !item.enabled;
        }
    }

    fn move_selected(&mut self, offset: isize) {
        let new_index = self.selected_index as isize + offset;
        if new_index < 0 || new_index >= self.items.len() as isize {
            return;
        }
        let new_index = new_index as usize;
        self.items.swap(self.selected_index, new_index);
        self.selected_index = new_index;
    }

    fn select_row(&mut self, column: u16, row: u16) -> bool {
        if !self.rows_area.contains(Position::new(column, row)) {
            return false;
        }
        let index = row.saturating_sub(self.rows_area.y) as usize / 2;
        if index >= self.items.len() {
            return false;
        }
        self.selected_index = index;
        true
    }
}

impl Default for TitleDialogState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_title_dialog() -> TitleDialogState {
    TitleDialogState::new()
}

pub fn handle_title_dialog_key_event(
    state: &mut TitleDialogState,
    event: KeyEvent,
) -> TitleDialogAction {
    if !state.is_visible() {
        return TitleDialogAction::None;
    }

    match event.code {
        KeyCode::Esc => {
            state.hide();
            TitleDialogAction::Cancel
        }
        KeyCode::Enter => {
            state.hide();
            TitleDialogAction::Confirm
        }
        KeyCode::Char(' ') => {
            state.toggle_selected();
            TitleDialogAction::Changed
        }
        KeyCode::Up => {
            state.previous();
            TitleDialogAction::None
        }
        KeyCode::Down => {
            state.next();
            TitleDialogAction::None
        }
        KeyCode::Left => {
            state.move_selected(-1);
            TitleDialogAction::Changed
        }
        KeyCode::Right => {
            state.move_selected(1);
            TitleDialogAction::Changed
        }
        _ => TitleDialogAction::None,
    }
}

pub fn handle_title_dialog_mouse_event(
    state: &mut TitleDialogState,
    event: MouseEvent,
) -> TitleDialogAction {
    if !state.is_visible() {
        return TitleDialogAction::None;
    }

    if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
        && !state
            .dialog_area
            .contains(Position::new(event.column, event.row))
    {
        state.hide();
        return TitleDialogAction::Cancel;
    }

    match event.kind {
        MouseEventKind::ScrollUp => {
            state.previous();
            TitleDialogAction::None
        }
        MouseEventKind::ScrollDown => {
            state.next();
            TitleDialogAction::None
        }
        MouseEventKind::Down(MouseButton::Left) if state.select_row(event.column, event.row) => {
            state.toggle_selected();
            TitleDialogAction::Changed
        }
        _ => TitleDialogAction::None,
    }
}

pub fn render_title_dialog(
    frame: &mut Frame,
    state: &mut TitleDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    if !state.is_visible() {
        return;
    }

    let dialog_width = area.width.min(78);
    let dialog_height = area.height.min(20);
    state.dialog_area = Rect {
        x: area.x + area.width.saturating_sub(dialog_width) / 2,
        y: area.y + area.height.saturating_sub(dialog_height) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, state.dialog_area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        state.dialog_area,
    );

    let content = Rect {
        x: state.dialog_area.x.saturating_add(3),
        y: state.dialog_area.y.saturating_add(1),
        width: state.dialog_area.width.saturating_sub(6),
        height: state.dialog_area.height.saturating_sub(2),
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(12),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(content);

    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(4)])
        .split(chunks[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Configure Terminal Title",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        ))),
        header[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "esc",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Right),
        header[1],
    );
    frame.render_widget(
        Paragraph::new("Select which items appear in the terminal title.")
            .style(Style::default().fg(colors.text_weak)),
        chunks[1],
    );

    state.rows_area = chunks[2];
    let mut lines = Vec::with_capacity(state.items.len() * 2);
    for (index, item) in state.items.iter().enumerate() {
        let selected = index == state.selected_index;
        let marker = if item.enabled { "[x]" } else { "[ ]" };
        let row_style = if selected {
            Style::default()
                .fg(contrast_text(colors.primary))
                .bg(colors.primary)
        } else {
            Style::default().fg(colors.text)
        };
        let description_style = if selected {
            row_style.add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(colors.text_weak)
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "› " } else { "  " }, row_style),
            Span::styled(format!("{marker} {}", item.kind.label()), row_style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("      {}", item.kind.description()),
            description_style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), state.rows_area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("space", Style::default().fg(colors.primary)),
            Span::styled(" toggle  ", Style::default().fg(colors.text_weak)),
            Span::styled("↑/↓", Style::default().fg(colors.primary)),
            Span::styled(" select  ", Style::default().fg(colors.text_weak)),
            Span::styled("←/→", Style::default().fg(colors.primary)),
            Span::styled(" reorder  ", Style::default().fg(colors.text_weak)),
            Span::styled("enter", Style::default().fg(colors.primary)),
            Span::styled(" save", Style::default().fg(colors.text_weak)),
        ])),
        chunks[4],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn selected_items_are_first_and_keep_configured_order() {
        let mut state = TitleDialogState::new();
        state.show(&[TerminalTitleItem::GitBranch, TerminalTitleItem::Activity]);

        assert_eq!(
            state.enabled_items(),
            vec![TerminalTitleItem::GitBranch, TerminalTitleItem::Activity]
        );
    }

    #[test]
    fn space_toggles_and_horizontal_arrows_reorder() {
        let mut state = TitleDialogState::new();
        state.show(&TerminalTitleItem::DEFAULT);

        handle_title_dialog_key_event(&mut state, key(KeyCode::Char(' ')));
        assert_eq!(state.enabled_items(), vec![TerminalTitleItem::ProjectName]);

        handle_title_dialog_key_event(&mut state, key(KeyCode::Right));
        handle_title_dialog_key_event(&mut state, key(KeyCode::Char(' ')));
        assert_eq!(
            state.enabled_items(),
            vec![TerminalTitleItem::ProjectName, TerminalTitleItem::Activity]
        );
    }
}
