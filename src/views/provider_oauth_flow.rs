use crate::theme::ThemeColors;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};
use tui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

const MAX_CALLBACK_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOAuthFlowMode {
    BrowserWaiting,
    BrowserCallback,
    HeadlessPreparing,
    HeadlessCode,
}

#[derive(Debug)]
pub struct ProviderOAuthFlowState {
    pub visible: bool,
    pub mode: ProviderOAuthFlowMode,
    pub provider_label: String,
    pub url: Option<String>,
    pub code: Option<String>,
    callback_textarea: TextArea<'static>,
    pub callback_error: Option<String>,
    pub submitting: bool,
    open_link_area: Option<Rect>,
    submit_area: Option<Rect>,
    dialog_area: Rect,
    link_area: Option<Rect>,
}

impl ProviderOAuthFlowState {
    pub fn new() -> Self {
        Self {
            visible: false,
            mode: ProviderOAuthFlowMode::BrowserWaiting,
            provider_label: "OpenAI".to_string(),
            url: None,
            code: None,
            callback_textarea: TextArea::default(),
            callback_error: None,
            submitting: false,
            open_link_area: None,
            submit_area: None,
            dialog_area: Rect::default(),
            link_area: None,
        }
    }

    pub fn show_browser_waiting(&mut self) {
        self.show_browser_waiting_for("OpenAI");
    }

    pub fn show_browser_waiting_for(&mut self, provider_label: impl Into<String>) {
        self.hide();
        self.visible = true;
        self.mode = ProviderOAuthFlowMode::BrowserWaiting;
        self.provider_label = provider_label.into();
        self.url = None;
        self.code = None;
    }

    pub fn show_browser_callback_for(
        &mut self,
        provider_label: impl Into<String>,
        url: impl Into<String>,
    ) {
        self.hide();
        self.visible = true;
        self.mode = ProviderOAuthFlowMode::BrowserCallback;
        self.provider_label = provider_label.into();
        self.url = Some(url.into());
    }

    /// Insert a whole paste without submitting or retaining rejected content.
    pub fn insert_callback(&mut self, text: &str) {
        if !self.visible || self.mode != ProviderOAuthFlowMode::BrowserCallback || self.submitting {
            return;
        }
        if text.chars().any(char::is_control) {
            self.callback_error =
                Some("Paste one full callback URL without line breaks or tabs.".into());
            return;
        }
        if self.callback_textarea.lines()[0]
            .len()
            .saturating_add(text.len())
            > MAX_CALLBACK_BYTES
        {
            self.callback_error = Some(
                "Callback URL is too long (maximum 16 KiB). Paste only the localhost URL.".into(),
            );
            return;
        }
        self.callback_textarea.insert_str(text);
        self.callback_error = None;
    }

    pub fn set_callback_error(&mut self, error: String) {
        self.submitting = false;
        self.callback_error = Some(error);
    }

    pub fn mark_submitting(&mut self) {
        self.submitting = true;
        self.callback_error = None;
        // Replacing the editor also drops undo history and its internal yank buffer.
        self.callback_textarea = TextArea::default();
    }

    fn submit_callback(&mut self) -> ProviderOAuthFlowAction {
        if self.submitting {
            return ProviderOAuthFlowAction::Handled;
        }
        let callback = self.callback_textarea.lines()[0].trim().to_string();
        if callback.is_empty() {
            self.callback_error =
                Some("Paste the full localhost callback URL before submitting.".into());
            return ProviderOAuthFlowAction::Handled;
        }
        self.mark_submitting();
        ProviderOAuthFlowAction::SubmitCallback(callback)
    }

    pub fn show_headless_preparing(&mut self) {
        self.show_headless_preparing_for("OpenAI");
    }

    pub fn show_headless_preparing_for(&mut self, provider_label: impl Into<String>) {
        self.hide();
        self.visible = true;
        self.mode = ProviderOAuthFlowMode::HeadlessPreparing;
        self.provider_label = provider_label.into();
        self.url = None;
        self.code = None;
    }

    pub fn set_headless_code(&mut self, code: String, url: String) {
        self.visible = true;
        self.mode = ProviderOAuthFlowMode::HeadlessCode;
        self.url = Some(url);
        self.code = Some(code);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.url = None;
        self.code = None;
        self.link_area = None;
        self.open_link_area = None;
        self.submit_area = None;
        self.dialog_area = Rect::default();
        self.callback_textarea = TextArea::default();
        self.callback_error = None;
        self.submitting = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Default for ProviderOAuthFlowState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderOAuthFlowAction {
    Handled,
    NotHandled,
    Close,
    CopyLink(String),
    OpenLink(String),
    SubmitCallback(String),
    Cancel,
}

pub fn init_provider_oauth_flow() -> ProviderOAuthFlowState {
    ProviderOAuthFlowState::new()
}

pub fn handle_provider_oauth_flow_key_event(
    state: &mut ProviderOAuthFlowState,
    event: KeyEvent,
) -> ProviderOAuthFlowAction {
    if !state.visible {
        return ProviderOAuthFlowAction::NotHandled;
    }

    if event.kind == KeyEventKind::Release {
        return ProviderOAuthFlowAction::Handled;
    }

    if event.code == KeyCode::Esc {
        let callback = state.mode == ProviderOAuthFlowMode::BrowserCallback;
        state.hide();
        return if callback {
            ProviderOAuthFlowAction::Cancel
        } else {
            ProviderOAuthFlowAction::Close
        };
    }

    if event.code == KeyCode::Char('y') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(url) = &state.url {
            return ProviderOAuthFlowAction::CopyLink(url.clone());
        }
    }

    if state.mode == ProviderOAuthFlowMode::BrowserCallback {
        if event.code == KeyCode::Char('o') && event.modifiers == KeyModifiers::CONTROL {
            if let Some(url) = &state.url {
                return ProviderOAuthFlowAction::OpenLink(url.clone());
            }
        }
        if state.submitting {
            return ProviderOAuthFlowAction::Handled;
        }
        match event.code {
            KeyCode::Enter => return state.submit_callback(),
            KeyCode::Char(c)
                if !event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                state.insert_callback(&c.to_string());
            }
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End => {
                state.callback_textarea.input(event);
                state.callback_error = None;
            }
            KeyCode::Char(c)
                if event.modifiers == KeyModifiers::CONTROL
                    && matches!(
                        c,
                        'a' | 'e' | 'b' | 'f' | 'h' | 'd' | 'k' | 'u' | 'w' | 'z' | 'r'
                    ) =>
            {
                state.callback_textarea.input(event);
                state.callback_error = None;
            }
            _ => {}
        }
    }

    ProviderOAuthFlowAction::Handled
}

pub fn handle_provider_oauth_flow_mouse_event(
    state: &mut ProviderOAuthFlowState,
    event: MouseEvent,
) -> ProviderOAuthFlowAction {
    if !state.visible {
        return ProviderOAuthFlowAction::NotHandled;
    }

    if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        return ProviderOAuthFlowAction::Handled;
    }

    let point = Position::new(event.column, event.row);

    if !state.dialog_area.contains(point) {
        // Callback auth stays open until an explicit Esc cancellation.
        if state.mode == ProviderOAuthFlowMode::BrowserCallback {
            return ProviderOAuthFlowAction::Handled;
        }
        state.hide();
        return ProviderOAuthFlowAction::Close;
    }

    if state.mode == ProviderOAuthFlowMode::BrowserCallback {
        if state
            .open_link_area
            .is_some_and(|area| area.contains(point))
        {
            if let Some(url) = &state.url {
                return ProviderOAuthFlowAction::OpenLink(url.clone());
            }
        }
        if state.submit_area.is_some_and(|area| area.contains(point)) {
            return state.submit_callback();
        }
    }

    if let (Some(link_area), Some(url)) = (state.link_area, &state.url) {
        if link_area.contains(point) {
            return ProviderOAuthFlowAction::CopyLink(url.clone());
        }
    }

    ProviderOAuthFlowAction::Handled
}

pub fn render_provider_oauth_flow(
    frame: &mut Frame,
    state: &mut ProviderOAuthFlowState,
    area: Rect,
    colors: ThemeColors,
) {
    if !state.visible {
        return;
    }

    if state.mode == ProviderOAuthFlowMode::BrowserCallback {
        render_browser_callback(frame, state, area, colors);
        return;
    }

    const DIALOG_WIDTH: u16 = 82;
    const DIALOG_HEIGHT: u16 = 16;
    const PADDING: u16 = 3;

    let dialog_width = area.width.min(DIALOG_WIDTH);
    let dialog_height = area.height.min(DIALOG_HEIGHT);

    state.dialog_area = Rect {
        x: area.x.saturating_add((area.width - dialog_width) / 2),
        y: area.y.saturating_add((area.height - dialog_height) / 2),
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, state.dialog_area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        state.dialog_area,
    );

    let content_area = Rect {
        x: state
            .dialog_area
            .x
            .saturating_add(PADDING.min(state.dialog_area.width)),
        y: state
            .dialog_area
            .y
            .saturating_add(PADDING.min(state.dialog_area.height)),
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
        format!("{} OAuth", state.provider_label),
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
        ProviderOAuthFlowMode::BrowserCallback => unreachable!("rendered separately"),
        ProviderOAuthFlowMode::BrowserWaiting => {
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
        ProviderOAuthFlowMode::HeadlessPreparing => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::raw(format!(
                    "Requesting device code from {}...",
                    state.provider_label
                ))]))
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
        ProviderOAuthFlowMode::HeadlessCode => {
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

fn render_browser_callback(
    frame: &mut Frame,
    state: &mut ProviderOAuthFlowState,
    area: Rect,
    colors: ThemeColors,
) {
    let width = area.width.min(82);
    let height = area.height.min(20);
    state.dialog_area = Rect::new(
        area.x.saturating_add((area.width - width) / 2),
        area.y.saturating_add((area.height - height) / 2),
        width,
        height,
    );
    state.link_area = None;
    state.open_link_area = None;
    state.submit_area = None;
    frame.render_widget(Clear, state.dialog_area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        state.dialog_area,
    );
    let padding = if width >= 40 { 2 } else { 0 };
    let top: u16 = if height >= 20 { 1 } else { 0 };
    let content = Rect::new(
        state.dialog_area.x.saturating_add(padding),
        state.dialog_area.y.saturating_add(top),
        width.saturating_sub(padding * 2),
        height.saturating_sub(top * 2),
    );
    if content.is_empty() {
        return;
    }
    let row = |offset: u16, rows: u16| {
        Rect::new(
            content.x,
            content.y.saturating_add(offset.min(content.height)),
            content.width,
            rows.min(content.height.saturating_sub(offset)),
        )
    };
    let text = Style::default().fg(colors.text);
    let weak = Style::default().fg(colors.text_weak);
    let link = Style::default().fg(colors.primary);
    frame.render_widget(
        Paragraph::new(format!("{} OAuth", state.provider_label))
            .style(text.add_modifier(Modifier::BOLD)),
        row(0, 1),
    );
    frame.render_widget(
        Paragraph::new("Esc cancel")
            .style(link)
            .alignment(Alignment::Right),
        row(1, 1),
    );
    frame.render_widget(
        Paragraph::new("1. Authorize in your browser").style(text),
        row(2, 1),
    );
    let url_area = row(3, 1);
    if !url_area.is_empty() {
        state.link_area = Some(url_area);
    }
    frame.render_widget(
        Paragraph::new(state.url.as_deref().unwrap_or_default())
            .style(link.add_modifier(Modifier::UNDERLINED)),
        url_area,
    );
    let open_area = row(4, 1);
    if !open_area.is_empty() {
        state.open_link_area = Some(open_area);
    }
    frame.render_widget(
        Paragraph::new("[Open link] Ctrl+O   Ctrl+Y copy").style(link),
        open_area,
    );
    frame.render_widget(Paragraph::new("2. Callback URL").style(text), row(6, 1));
    state
        .callback_textarea
        .set_style(text.bg(colors.background_element));
    state
        .callback_textarea
        .set_cursor_line_style(Style::default());
    state
        .callback_textarea
        .set_cursor_style(if state.submitting {
            Style::default()
        } else {
            link.add_modifier(Modifier::REVERSED)
        });
    state
        .callback_textarea
        .set_placeholder_text("http://localhost:.../?code=...");
    frame.render_widget(&state.callback_textarea, row(7, 1));
    let submit_area = row(8, 1);
    if !submit_area.is_empty() {
        state.submit_area = Some(submit_area);
    }
    frame.render_widget(
        Paragraph::new(if state.submitting {
            "Submitting..."
        } else {
            "[Submit] Enter"
        })
        .style(if state.submitting { weak } else { link }),
        submit_area,
    );
    frame.render_widget(
        Paragraph::new(
            "Copy the full localhost URL from your browser, even if the page cannot load.",
        )
        .style(weak)
        .wrap(Wrap { trim: false }),
        row(9, 3),
    );
    if let Some(error) = &state.callback_error {
        frame.render_widget(
            Paragraph::new(error.as_str())
                .style(Style::default().fg(colors.error))
                .wrap(Wrap { trim: false }),
            row(12, 2),
        );
    }
    frame.render_widget(
        Paragraph::new(
            "Unofficial Antigravity access may be subject to Google account restrictions.",
        )
        .style(Style::default().fg(colors.warning))
        .wrap(Wrap { trim: false }),
        row(14, 3),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    const URL: &str = "https://example.com/authorize?state=full-state&challenge=full-challenge";
    const CALLBACK: &str = "http://localhost:1234/?code=secret&state=test";

    fn callback_state() -> ProviderOAuthFlowState {
        let mut state = ProviderOAuthFlowState::new();
        state.show_browser_callback_for("Antigravity", URL);
        state
    }

    fn key(
        state: &mut ProviderOAuthFlowState,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> ProviderOAuthFlowAction {
        handle_provider_oauth_flow_key_event(state, KeyEvent::new(code, modifiers))
    }

    fn render(state: &mut ProviderOAuthFlowState, width: u16, height: u16, area: Rect) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_provider_oauth_flow(
                    frame,
                    state,
                    area,
                    crate::theme::Theme::load_builtin_default().get_colors(true),
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn paste_is_bounded_single_line_and_never_submits() {
        let mut state = callback_state();
        for invalid in ["first\nsecond", "tab\tvalue", "escape\u{1b}", "\r", "\0"] {
            state.insert_callback(invalid);
            assert!(state.callback_error.is_some());
            assert_eq!(state.callback_textarea.lines(), &[String::new()]);
        }
        state.insert_callback(&"x".repeat(MAX_CALLBACK_BYTES + 1));
        assert!(state.callback_error.is_some());
        state.insert_callback(CALLBACK);
        assert_eq!(state.callback_textarea.lines(), &[CALLBACK.to_string()]);
        assert!(!state.submitting);
        assert!(state.callback_error.is_none());
    }

    #[test]
    fn keyboard_submit_clears_sensitive_history_and_blocks_duplicates() {
        let mut state = callback_state();
        assert_eq!(
            key(&mut state, KeyCode::Enter, KeyModifiers::NONE),
            ProviderOAuthFlowAction::Handled
        );
        assert!(state.callback_error.is_some());
        state.insert_callback(CALLBACK);
        key(&mut state, KeyCode::End, KeyModifiers::NONE);
        key(&mut state, KeyCode::Char('x'), KeyModifiers::NONE);
        key(&mut state, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(
            key(&mut state, KeyCode::Enter, KeyModifiers::NONE),
            ProviderOAuthFlowAction::SubmitCallback(CALLBACK.into())
        );
        assert!(state.submitting);
        key(&mut state, KeyCode::Char('z'), KeyModifiers::CONTROL);
        key(&mut state, KeyCode::Char('x'), KeyModifiers::NONE);
        state.insert_callback(CALLBACK);
        assert_eq!(state.callback_textarea.lines(), &[String::new()]);
        assert_eq!(
            key(&mut state, KeyCode::Enter, KeyModifiers::NONE),
            ProviderOAuthFlowAction::Handled
        );
        state.set_callback_error("Authorization failed; retry login.".into());
        assert!(!state.submitting);
        key(&mut state, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(state.callback_textarea.lines(), &[String::new()]);
    }

    #[test]
    fn release_events_do_not_submit_edit_or_cancel() {
        let mut state = callback_state();
        state.insert_callback(CALLBACK);
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('x')] {
            let mut event = KeyEvent::new(code, KeyModifiers::NONE);
            event.kind = KeyEventKind::Release;
            assert_eq!(
                handle_provider_oauth_flow_key_event(&mut state, event),
                ProviderOAuthFlowAction::Handled
            );
        }
        assert!(state.visible);
        assert!(!state.submitting);
        assert_eq!(state.callback_textarea.lines()[0], CALLBACK);
    }

    #[test]
    fn copy_open_cancel_and_hide_reset() {
        let mut state = callback_state();
        assert_eq!(
            key(&mut state, KeyCode::Char('y'), KeyModifiers::CONTROL),
            ProviderOAuthFlowAction::CopyLink(URL.into())
        );
        assert_eq!(
            key(&mut state, KeyCode::Char('o'), KeyModifiers::CONTROL),
            ProviderOAuthFlowAction::OpenLink(URL.into())
        );
        state.insert_callback(CALLBACK);
        state.callback_error = Some("retry".into());
        assert_eq!(
            key(&mut state, KeyCode::Esc, KeyModifiers::NONE),
            ProviderOAuthFlowAction::Cancel
        );
        assert!(state.url.is_none());
        assert!(state.callback_error.is_none());
        assert!(!state.submitting);
        assert!(
            state.link_area.is_none()
                && state.open_link_area.is_none()
                && state.submit_area.is_none()
        );
        assert_eq!(state.dialog_area, Rect::default());
        state.show_browser_callback_for("Antigravity", URL);
        key(&mut state, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(state.callback_textarea.lines(), &[String::new()]);
    }

    #[test]
    fn headless_keeps_copy_and_close_behavior() {
        let mut state = ProviderOAuthFlowState::new();
        state.show_headless_preparing();
        assert_eq!(state.mode, ProviderOAuthFlowMode::HeadlessPreparing);
        state.set_headless_code("device-code".into(), URL.into());
        assert_eq!(
            key(&mut state, KeyCode::Char('y'), KeyModifiers::CONTROL),
            ProviderOAuthFlowAction::CopyLink(URL.into())
        );
        assert_eq!(
            key(&mut state, KeyCode::Esc, KeyModifiers::NONE),
            ProviderOAuthFlowAction::Close
        );
        assert!(state.code.is_none());
    }

    #[test]
    fn responsive_render_and_clicks_preserve_full_url() {
        for (width, height) in [(100, 30), (40, 18), (20, 16), (1, 1)] {
            let mut state = callback_state();
            let output = render(&mut state, width, height, Rect::new(0, 0, width, height));
            if width >= 40 {
                assert!(output.contains("Esc cancel"));
                assert!(output.contains("2. Callback URL"));
                assert!(output.contains("[Submit] Enter"));
            }
            if let Some(area) = state.link_area {
                let event = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: area.x,
                    row: area.y,
                    modifiers: KeyModifiers::NONE,
                };
                assert_eq!(
                    handle_provider_oauth_flow_mouse_event(&mut state, event),
                    ProviderOAuthFlowAction::CopyLink(URL.into())
                );
            }
            if let Some(area) = state.open_link_area {
                let event = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: area.x,
                    row: area.y,
                    modifiers: KeyModifiers::NONE,
                };
                assert_eq!(
                    handle_provider_oauth_flow_mouse_event(&mut state, event),
                    ProviderOAuthFlowAction::OpenLink(URL.into())
                );
            }
            if let Some(area) = state.submit_area {
                state.insert_callback(CALLBACK);
                let event = MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: area.x,
                    row: area.y,
                    modifiers: KeyModifiers::NONE,
                };
                assert_eq!(
                    handle_provider_oauth_flow_mouse_event(&mut state, event),
                    ProviderOAuthFlowAction::SubmitCallback(CALLBACK.into())
                );
                assert_eq!(
                    handle_provider_oauth_flow_mouse_event(&mut state, event),
                    ProviderOAuthFlowAction::Handled
                );
            }
        }
        let mut state = callback_state();
        render(&mut state, 100, 30, Rect::new(10, 5, 40, 18));
        assert!(state.dialog_area.x >= 10 && state.dialog_area.y >= 5);
        for mode in [
            ProviderOAuthFlowMode::BrowserWaiting,
            ProviderOAuthFlowMode::HeadlessPreparing,
            ProviderOAuthFlowMode::HeadlessCode,
        ] {
            state.mode = mode;
            render(&mut state, 1, 1, Rect::new(0, 0, 1, 1));
        }
    }
}
