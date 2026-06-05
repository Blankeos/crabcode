use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};
use tui_textarea::{Input as TuiInput, TextArea};

use crate::theme::ThemeColors;

pub const DEFAULT_REMOTE_BIND: &str = "0.0.0.0:8421";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDialogSubmission {
    pub bind: String,
    pub pair_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteDialogFocus {
    Bind,
    Pin,
}

#[derive(Debug)]
pub struct RemoteDialogState {
    visible: bool,
    bind_textarea: TextArea<'static>,
    pin_textarea: TextArea<'static>,
    focus: RemoteDialogFocus,
    dialog_area: Rect,
    bind_area: Rect,
    pin_area: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDialogAction {
    Submit(RemoteDialogSubmission),
    BlockedStreaming,
    Cancel,
    Handled,
    NotHandled,
}

impl RemoteDialogState {
    pub fn new() -> Self {
        Self {
            visible: false,
            bind_textarea: bind_textarea(),
            pin_textarea: pin_textarea(),
            focus: RemoteDialogFocus::Bind,
            dialog_area: Rect::default(),
            bind_area: Rect::default(),
            pin_area: Rect::default(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.focus = RemoteDialogFocus::Bind;
        self.bind_textarea = bind_textarea();
        self.pin_textarea = pin_textarea();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.bind_textarea = bind_textarea();
        self.pin_textarea = pin_textarea();
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn insert_text(&mut self, text: &str) {
        match self.focus {
            RemoteDialogFocus::Bind => self.bind_textarea.insert_str(text),
            RemoteDialogFocus::Pin => self.pin_textarea.insert_str(text),
        };
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            RemoteDialogFocus::Bind => RemoteDialogFocus::Pin,
            RemoteDialogFocus::Pin => RemoteDialogFocus::Bind,
        };
    }

    fn raw_bind(&self) -> String {
        self.bind_textarea.lines().join("")
    }

    fn raw_pin(&self) -> String {
        self.pin_textarea.lines().join("")
    }

    fn submission(&self) -> RemoteDialogSubmission {
        let bind = normalize_bind_input(&self.raw_bind());
        let pair_code = self.raw_pin().trim().to_string();
        let pair_code = (!pair_code.is_empty()).then_some(pair_code);

        RemoteDialogSubmission { bind, pair_code }
    }
}

impl Default for RemoteDialogState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_remote_dialog() -> RemoteDialogState {
    RemoteDialogState::new()
}

pub fn render_remote_dialog(
    f: &mut Frame,
    state: &mut RemoteDialogState,
    area: Rect,
    colors: ThemeColors,
    submit_enabled: bool,
) {
    if !state.visible {
        return;
    }

    const DIALOG_WIDTH: u16 = 72;
    const DIALOG_HEIGHT: u16 = 18;

    let dialog_width = area.width.min(DIALOG_WIDTH);
    let dialog_height = area.height.min(DIALOG_HEIGHT);
    state.dialog_area = Rect {
        x: area.x + area.width.saturating_sub(dialog_width) / 2,
        y: area.y + area.height.saturating_sub(dialog_height) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, state.dialog_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        state.dialog_area,
    );

    const PADDING_X: u16 = 3;
    const PADDING_Y: u16 = 2;
    let content_area = Rect {
        x: state.dialog_area.x + PADDING_X,
        y: state.dialog_area.y + PADDING_Y,
        width: state.dialog_area.width.saturating_sub(PADDING_X * 2),
        height: state.dialog_area.height.saturating_sub(PADDING_Y * 2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(content_area);

    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(4)])
        .split(chunks[0]);
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Start remote host",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )])),
        header_chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "esc",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    let warning = Line::from(vec![
        Span::styled(
            "Warning: ",
            Style::default()
                .fg(colors.warning)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "This will close the current session and start ",
            Style::default().fg(colors.text_weak),
        ),
        Span::styled(
            "`crabcode serve`",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(".", Style::default().fg(colors.text_weak)),
    ]);
    f.render_widget(Paragraph::new(warning).wrap(Wrap { trim: true }), chunks[2]);

    render_label(
        f,
        chunks[4],
        "Bind address",
        state.focus == RemoteDialogFocus::Bind,
        colors,
    );
    state.bind_area = chunks[5];
    style_textarea(
        &mut state.bind_textarea,
        state.focus == RemoteDialogFocus::Bind,
        colors,
    );
    f.render_widget(&state.bind_textarea, state.bind_area);

    render_label(
        f,
        chunks[6],
        "Pin (optional)",
        state.focus == RemoteDialogFocus::Pin,
        colors,
    );
    state.pin_area = chunks[7];
    style_textarea(
        &mut state.pin_textarea,
        state.focus == RemoteDialogFocus::Pin,
        colors,
    );
    f.render_widget(&state.pin_textarea, state.pin_area);

    let footer = if submit_enabled {
        Line::from(vec![
            Span::styled(
                "enter",
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" start  ", Style::default().fg(colors.text_weak)),
            Span::styled(
                "tab",
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" switch field", Style::default().fg(colors.text_weak)),
        ])
    } else {
        Line::from(vec![Span::styled(
            "Wait for the current response to finish before starting remote mode",
            Style::default()
                .fg(colors.warning)
                .add_modifier(Modifier::BOLD),
        )])
    };
    f.render_widget(Paragraph::new(footer), chunks[8]);
}

pub fn handle_remote_dialog_key_event(
    state: &mut RemoteDialogState,
    event: KeyEvent,
    submit_enabled: bool,
) -> RemoteDialogAction {
    if !state.visible {
        return RemoteDialogAction::NotHandled;
    }

    match event.code {
        KeyCode::Esc => {
            state.hide();
            RemoteDialogAction::Cancel
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.toggle_focus();
            RemoteDialogAction::Handled
        }
        KeyCode::Enter => {
            if !submit_enabled {
                return RemoteDialogAction::BlockedStreaming;
            }
            let submission = state.submission();
            state.hide();
            RemoteDialogAction::Submit(submission)
        }
        _ => {
            let input = TuiInput::from(event);
            match state.focus {
                RemoteDialogFocus::Bind => {
                    state.bind_textarea.input(input);
                }
                RemoteDialogFocus::Pin => {
                    state.pin_textarea.input(input);
                }
            }
            RemoteDialogAction::Handled
        }
    }
}

pub fn handle_remote_dialog_mouse_event(
    state: &mut RemoteDialogState,
    event: MouseEvent,
) -> RemoteDialogAction {
    if !state.visible {
        return RemoteDialogAction::NotHandled;
    }

    if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        let point = Position::new(event.column, event.row);
        if state.bind_area.contains(point) {
            state.focus = RemoteDialogFocus::Bind;
            return RemoteDialogAction::Handled;
        }
        if state.pin_area.contains(point) {
            state.focus = RemoteDialogFocus::Pin;
            return RemoteDialogAction::Handled;
        }
    }

    RemoteDialogAction::Handled
}

pub fn normalize_bind_input(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return DEFAULT_REMOTE_BIND.to_string();
    }

    if let Ok(url) = url::Url::parse(trimmed) {
        if let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) {
            if host.contains(':') && !host.starts_with('[') {
                return format!("[{host}]:{port}");
            }
            return format!("{host}:{port}");
        }
    }

    if let Some(port) = trimmed.strip_prefix(':').filter(|port| !port.is_empty()) {
        return format!("0.0.0.0:{port}");
    }

    trimmed.to_string()
}

fn bind_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text(DEFAULT_REMOTE_BIND);
    textarea
}

fn pin_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("No pin");
    textarea
}

fn style_textarea(textarea: &mut TextArea<'static>, focused: bool, colors: ThemeColors) {
    let fg = if focused {
        colors.text
    } else {
        colors.text_weak
    };
    let cursor = if focused {
        colors.primary
    } else {
        colors.text_weak
    };
    textarea.set_style(Style::default().fg(fg));
    textarea.set_cursor_line_style(Style::default().fg(fg));
    textarea.set_cursor_style(Style::default().fg(cursor).add_modifier(Modifier::REVERSED));
}

fn render_label(f: &mut Frame, area: Rect, label: &str, focused: bool, colors: ThemeColors) {
    let marker = if focused { "● " } else { "  " };
    let marker_style = Style::default().fg(if focused {
        colors.primary
    } else {
        colors.text_weak
    });
    let label_style = Style::default().fg(if focused {
        colors.text
    } else {
        colors.text_weak
    });
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(label, label_style),
        ])),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_empty_bind_uses_remote_default() {
        assert_eq!(normalize_bind_input("   "), DEFAULT_REMOTE_BIND);
    }

    #[test]
    fn normalize_bind_accepts_http_url() {
        assert_eq!(
            normalize_bind_input("http://0.0.0.0:8421"),
            DEFAULT_REMOTE_BIND
        );
    }

    #[test]
    fn normalize_bind_accepts_port_shorthand() {
        assert_eq!(normalize_bind_input(":9000"), "0.0.0.0:9000");
    }
}
