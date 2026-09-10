use crate::theme::ThemeColors;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tui_textarea::{CursorMove, TextArea};

use crate::ui::textarea_keys::input_textarea;

#[derive(Debug)]
pub struct SessionRenameDialogState {
    pub visible: bool,
    pub dialog_area: Rect,
    pub content_area: Rect,
    pub input_textarea: TextArea<'static>,
    pub session_id: Option<String>,
    pub original_title: String,
    is_input_focused: Arc<AtomicBool>,
    colors: ThemeColors,
}

impl SessionRenameDialogState {
    pub fn new(colors: ThemeColors) -> Self {
        let mut input_textarea = TextArea::default();
        input_textarea.set_placeholder_text("Session title");
        input_textarea.set_cursor_line_style(Style::default().fg(colors.primary));

        Self {
            visible: false,
            dialog_area: Rect::default(),
            content_area: Rect::default(),
            input_textarea,
            session_id: None,
            original_title: String::new(),
            is_input_focused: Arc::new(AtomicBool::new(false)),
            colors,
        }
    }

    pub fn set_colors(&mut self, colors: ThemeColors) {
        self.colors = colors;
    }

    pub fn show(&mut self, session_id: String, current_title: String) {
        self.session_id = Some(session_id);
        self.original_title = current_title.clone();
        self.input_textarea = TextArea::from(vec![current_title]);
        self.input_textarea.set_placeholder_text("Session title");
        self.input_textarea
            .set_cursor_line_style(Style::default().fg(self.colors.primary));
        self.input_textarea.move_cursor(CursorMove::End);
        self.visible = true;
        self.is_input_focused.store(true, Ordering::SeqCst);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.session_id = None;
        self.original_title.clear();
        self.is_input_focused.store(false, Ordering::SeqCst);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn get_input_text(&self) -> String {
        self.input_textarea.lines().join("\n")
    }

    pub fn get_rename_info(&self) -> Option<(String, String)> {
        if let (Some(id), _) = (&self.session_id, !self.original_title.is_empty()) {
            Some((id.clone(), self.original_title.clone()))
        } else {
            None
        }
    }
}

impl Default for SessionRenameDialogState {
    fn default() -> Self {
        Self::new(ThemeColors {
            primary: Color::Rgb(255, 140, 0),
            secondary: Color::Rgb(255, 140, 0),
            accent: Color::Rgb(255, 140, 0),
            interactive: Color::Rgb(255, 140, 0),
            background: Color::Reset,
            dialog_background: Color::Reset,
            background_element: Color::Reset,
            text: Color::Reset,
            text_weak: Color::Reset,
            text_strong: Color::Reset,
            border: Color::Reset,
            border_weak_focus: Color::Rgb(255, 200, 100),
            border_focus: Color::Rgb(255, 140, 0),
            border_strong_focus: Color::Rgb(255, 100, 0),
            success: Color::Rgb(0, 255, 0),
            warning: Color::Rgb(255, 255, 0),
            error: Color::Rgb(255, 0, 0),
            info: Color::Rgb(0, 255, 255),
            markdown_text: Color::Reset,
            markdown_heading: Color::Rgb(255, 140, 0),
            markdown_link: Color::Rgb(0, 255, 255),
            markdown_link_text: Color::Rgb(0, 255, 255),
            markdown_code: Color::Rgb(0, 255, 0),
            markdown_block_quote: Color::Rgb(255, 255, 0),
            markdown_emph: Color::Rgb(255, 255, 0),
            markdown_strong: Color::Rgb(255, 140, 0),
            markdown_horizontal_rule: Color::Reset,
            markdown_list_item: Color::Rgb(255, 140, 0),
            markdown_list_enumeration: Color::Rgb(0, 255, 255),
            markdown_image: Color::Rgb(255, 140, 0),
            markdown_image_text: Color::Rgb(0, 255, 255),
            markdown_code_block: Color::Reset,
            diff_add: Color::Rgb(0, 255, 0),
            diff_add_bg: Color::Rgb(0, 60, 0),
            diff_remove: Color::Rgb(255, 0, 0),
            diff_remove_bg: Color::Rgb(60, 0, 0),
            diff_gutter: Color::Rgb(140, 140, 140),
        })
    }
}

pub fn init_session_rename_dialog(colors: ThemeColors) -> SessionRenameDialogState {
    SessionRenameDialogState::new(colors)
}

pub fn render_session_rename_dialog(
    f: &mut Frame,
    dialog_state: &mut SessionRenameDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    if !dialog_state.visible {
        return;
    }

    const DIALOG_WIDTH: u16 = 60;
    const DIALOG_HEIGHT: u16 = 10;

    let dialog_width = area.width.min(DIALOG_WIDTH);
    let dialog_height = area.height.min(DIALOG_HEIGHT);

    dialog_state.dialog_area = Rect {
        x: (area.width - dialog_width) / 2,
        y: (area.height - dialog_height) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_state.dialog_area);

    const PADDING: u16 = 3;
    dialog_state.content_area = Rect {
        x: dialog_state.dialog_area.x + PADDING,
        y: dialog_state.dialog_area.y + PADDING,
        width: dialog_state.dialog_area.width.saturating_sub(PADDING * 2),
        height: dialog_state.dialog_area.height.saturating_sub(PADDING * 2),
    };

    f.render_widget(
        ratatui::widgets::Paragraph::new("")
            .style(ratatui::style::Style::default().bg(colors.dialog_background)),
        dialog_state.dialog_area,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(dialog_state.content_area);

    let title_line = Line::from(vec![
        Span::styled(
            "Rename session",
            Style::default()
                .fg(colors.text)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "esc",
            Style::default()
                .fg(colors.primary)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
    ]);

    let title_paragraph = Paragraph::new(title_line).alignment(Alignment::Left);
    f.render_widget(title_paragraph, chunks[0]);

    // Single caret: hardware cursor below is source of truth; suppress
    // fake block so emoji can't show two split carets.
    dialog_state
        .input_textarea
        .set_cursor_style(Style::default());
    f.render_widget(&dialog_state.input_textarea, chunks[3]);

    // Hardware cursor follows the text field so terminal cursor effects
    // (e.g. ghostty shaders) and IME popups track typing.
    if chunks[3].width > 0 && chunks[3].height > 0 {
        use unicode_width::UnicodeWidthStr;
        let (row, col) = dialog_state.input_textarea.cursor();
        let line = dialog_state
            .input_textarea
            .lines()
            .get(row)
            .cloned()
            .unwrap_or_default();
        let prefix: String = line.chars().take(col).collect();
        let x = chunks[3]
            .x
            .saturating_add((prefix.width() as u16).min(chunks[3].width.saturating_sub(1)));
        let y = chunks[3]
            .y
            .saturating_add((row as u16).min(chunks[3].height.saturating_sub(1)));
        f.set_cursor_position((x, y));
    }

    let footer_line = Line::from(vec![Span::styled(
        "enter submit",
        Style::default()
            .fg(colors.text_weak)
            .add_modifier(ratatui::style::Modifier::DIM),
    )]);

    let footer_paragraph = Paragraph::new(footer_line).alignment(Alignment::Left);
    f.render_widget(footer_paragraph, chunks[4]);
}

pub fn handle_session_rename_dialog_key_event(
    dialog_state: &mut SessionRenameDialogState,
    event: KeyEvent,
) -> RenameAction {
    if !dialog_state.visible {
        return RenameAction::NotHandled;
    }

    match event.code {
        KeyCode::Esc => {
            dialog_state.hide();
            RenameAction::Cancel
        }
        KeyCode::Enter => {
            if let Some((session_id, _)) = dialog_state.get_rename_info() {
                let new_title = dialog_state.get_input_text();
                dialog_state.hide();
                RenameAction::Submit(session_id, new_title)
            } else {
                RenameAction::Handled
            }
        }
        _ => {
            input_textarea(&mut dialog_state.input_textarea, event);
            RenameAction::Handled
        }
    }
}

pub fn handle_session_rename_dialog_mouse_event(
    _dialog_state: &mut SessionRenameDialogState,
    _event: MouseEvent,
) -> bool {
    false
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenameAction {
    Handled,
    NotHandled,
    Cancel,
    Submit(String, String),
}
