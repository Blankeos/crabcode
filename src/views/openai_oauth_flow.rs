use crate::theme::ThemeColors;
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
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAIOAuthFlowMode {
    BrowserWaiting,
    HeadlessPreparing,
    HeadlessCode,
}

#[derive(Debug)]
pub struct OpenAIOAuthFlowState {
    pub visible: bool,
    pub mode: OpenAIOAuthFlowMode,
    pub url: Option<String>,
    pub code: Option<String>,
    dialog_area: Rect,
    link_area: Option<Rect>,
}

impl OpenAIOAuthFlowState {
    pub fn new() -> Self {
        Self {
            visible: false,
            mode: OpenAIOAuthFlowMode::BrowserWaiting,
            url: None,
            code: None,
            dialog_area: Rect::default(),
            link_area: None,
        }
    }

    pub fn show_browser_waiting(&mut self) {
        self.visible = true;
        self.mode = OpenAIOAuthFlowMode::BrowserWaiting;
        self.url = None;
        self.code = None;
    }

    pub fn show_headless_preparing(&mut self) {
        self.visible = true;
        self.mode = OpenAIOAuthFlowMode::HeadlessPreparing;
        self.url = None;
        self.code = None;
    }

    pub fn set_headless_code(&mut self, code: String, url: String) {
        self.visible = true;
        self.mode = OpenAIOAuthFlowMode::HeadlessCode;
        self.url = Some(url);
        self.code = Some(code);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.url = None;
        self.code = None;
        self.link_area = None;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Default for OpenAIOAuthFlowState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAIOAuthFlowAction {
    Handled,
    NotHandled,
    Close,
    CopyLink(String),
}

pub fn init_openai_oauth_flow() -> OpenAIOAuthFlowState {
    OpenAIOAuthFlowState::new()
}

pub fn handle_openai_oauth_flow_key_event(
    state: &mut OpenAIOAuthFlowState,
    event: KeyEvent,
) -> OpenAIOAuthFlowAction {
    if !state.visible {
        return OpenAIOAuthFlowAction::NotHandled;
    }

    if event.code == KeyCode::Esc {
        state.hide();
        return OpenAIOAuthFlowAction::Close;
    }

    if event.code == KeyCode::Char('y') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(url) = &state.url {
            return OpenAIOAuthFlowAction::CopyLink(url.clone());
        }
    }

    OpenAIOAuthFlowAction::Handled
}

pub fn handle_openai_oauth_flow_mouse_event(
    state: &mut OpenAIOAuthFlowState,
    event: MouseEvent,
) -> OpenAIOAuthFlowAction {
    if !state.visible {
        return OpenAIOAuthFlowAction::NotHandled;
    }

    if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        return OpenAIOAuthFlowAction::Handled;
    }

    let point = Position::new(event.column, event.row);

    if !state.dialog_area.contains(point) {
        state.hide();
        return OpenAIOAuthFlowAction::Close;
    }

    if let (Some(link_area), Some(url)) = (state.link_area, &state.url) {
        if link_area.contains(point) {
            return OpenAIOAuthFlowAction::CopyLink(url.clone());
        }
    }

    OpenAIOAuthFlowAction::Handled
}

pub fn render_openai_oauth_flow(
    frame: &mut Frame,
    state: &mut OpenAIOAuthFlowState,
    area: Rect,
    colors: ThemeColors,
) {
    if !state.visible {
        return;
    }

    const DIALOG_WIDTH: u16 = 82;
    const DIALOG_HEIGHT: u16 = 16;
    const PADDING: u16 = 3;

    let dialog_width = area.width.min(DIALOG_WIDTH);
    let dialog_height = area.height.min(DIALOG_HEIGHT);

    state.dialog_area = Rect {
        x: (area.width - dialog_width) / 2,
        y: (area.height - dialog_height) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, state.dialog_area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        state.dialog_area,
    );

    let content_area = Rect {
        x: state.dialog_area.x + PADDING,
        y: state.dialog_area.y + PADDING,
        width: state.dialog_area.width.saturating_sub(PADDING * 2),
        height: state.dialog_area.height.saturating_sub(PADDING * 2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(content_area);

    state.link_area = None;

    let esc_text = "esc";
    let esc_area_width = (esc_text.width() as u16).saturating_add(1);
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(esc_area_width)])
        .split(chunks[0]);

    let title = Line::from(vec![Span::styled(
        "OpenAI OAuth",
        Style::default()
            .fg(colors.text)
            .add_modifier(Modifier::BOLD),
    )]);
    frame.render_widget(
        Paragraph::new(title).alignment(Alignment::Left),
        header_chunks[0],
    );

    let esc_hint = Line::from(vec![Span::styled(
        esc_text,
        Style::default()
            .fg(colors.primary)
            .add_modifier(Modifier::BOLD),
    )]);
    frame.render_widget(
        Paragraph::new(esc_hint).alignment(Alignment::Right),
        header_chunks[1],
    );

    match state.mode {
        OpenAIOAuthFlowMode::BrowserWaiting => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(
                    "Complete login in your browser. Waiting for callback...",
                )]))
                .style(Style::default().fg(colors.text)),
                chunks[2],
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    "If browser did not open, retry from /connect.",
                    Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM),
                )])),
                chunks[3],
            );
        }
        OpenAIOAuthFlowMode::HeadlessPreparing => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(
                    "Requesting device code from OpenAI...",
                )]))
                .style(Style::default().fg(colors.text)),
                chunks[2],
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    "This view will update with link + code automatically.",
                    Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM),
                )])),
                chunks[3],
            );
        }
        OpenAIOAuthFlowMode::HeadlessCode => {
            let url = state.url.clone().unwrap_or_default();
            let code = state.code.clone().unwrap_or_default();

            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(
                    "1. Open this login link (click it or press ctrl+y to copy):",
                )]))
                .style(Style::default().fg(colors.text)),
                chunks[2],
            );

            state.link_area = Some(chunks[3]);
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    url,
                    Style::default()
                        .fg(colors.primary)
                        .add_modifier(Modifier::UNDERLINED),
                )])),
                chunks[3],
            );

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw("2. Enter code: "),
                    Span::styled(
                        code,
                        Style::default()
                            .fg(colors.text)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])),
                chunks[4],
            );

            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    "3. Return here and wait for completion.",
                    Style::default().fg(colors.text_weak),
                )])),
                chunks[5],
            );
        }
    }

    let footer = if state.url.is_some() {
        Line::from(vec![
            Span::styled(
                "copy link",
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "ctrl+y",
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            "waiting...",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )])
    };

    frame.render_widget(Paragraph::new(footer).alignment(Alignment::Left), chunks[8]);
}
