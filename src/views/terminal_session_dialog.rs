use crate::theme::ThemeColors;
use crate::tools::{
    TerminalSessionControl, TerminalSessionEvent, TerminalSessionRequest, TerminalSessionStart,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use std::collections::VecDeque;
use tokio::sync::mpsc;
use vt100::{Color as VtColor, Parser as VtParser, Screen as VtScreen};

const DIALOG_MIN_WIDTH: u16 = 48;
const DIALOG_MIN_HEIGHT: u16 = 16;
const HEADER_HEIGHT: u16 = 1;
const FOOTER_HEIGHT: u16 = 1;
const CHROME_GAP_HEIGHT: u16 = 1;
const PADDING_X: u16 = 1;
const PADDING_Y: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSessionResponse {
    Close,
    Handled,
    NotHandled,
}

struct ActiveTerminalSession {
    start: TerminalSessionStart,
    control_tx: mpsc::UnboundedSender<TerminalSessionControl>,
    parser: VtParser,
    started: bool,
    status: String,
    exit_code: Option<i32>,
    stopped_by_user: bool,
}

pub struct TerminalSessionDialogState {
    current: Option<ActiveTerminalSession>,
    queue: VecDeque<TerminalSessionRequest>,
    last_terminal_size: (u16, u16),
    user_controlled: bool,
}

impl TerminalSessionDialogState {
    pub fn new() -> Self {
        Self {
            current: None,
            queue: VecDeque::new(),
            last_terminal_size: (0, 0),
            user_controlled: true,
        }
    }

    pub fn enqueue(&mut self, request: TerminalSessionRequest) {
        if self.current.is_none() {
            self.activate(request);
        } else {
            self.queue.push_back(request);
        }
    }

    pub fn has_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn is_user_controlled(&self) -> bool {
        self.user_controlled
    }

    pub fn set_user_controlled(&mut self, value: bool) {
        self.user_controlled = value;
    }

    fn activate(&mut self, request: TerminalSessionRequest) {
        let rows = request.start.rows.max(1);
        let cols = request.start.cols.max(1);
        self.current = Some(ActiveTerminalSession {
            start: request.start,
            control_tx: request.control_tx,
            parser: VtParser::new(rows, cols, 0),
            started: false,
            status: "starting".to_string(),
            exit_code: None,
            stopped_by_user: false,
        });
    }

    /// Called from the app when `ChunkMessage::TerminalSessionEvent` arrives.
    pub fn apply_event(&mut self, tool_call_id: &str, event: TerminalSessionEvent) -> bool {
        let Some(active) = self.current.as_mut() else {
            self.remove_queued(tool_call_id);
            return false;
        };
        if active.start.tool_call_id != tool_call_id {
            if matches!(
                event,
                TerminalSessionEvent::Exited { .. } | TerminalSessionEvent::Stopped
            ) {
                self.remove_queued(tool_call_id);
            }
            return false;
        }

        match event {
            TerminalSessionEvent::Started => active.status = "running".to_string(),
            TerminalSessionEvent::Output(bytes) => active.parser.process(&bytes),
            TerminalSessionEvent::Resized { rows, cols } => {
                active.parser.set_size(rows.max(1), cols.max(1));
            }
            TerminalSessionEvent::Exited { exit_code } => {
                active.exit_code = exit_code;
                active.status = exit_code
                    .map(|c| format!("exited ({c})"))
                    .unwrap_or_else(|| "exited".to_string());
            }
            TerminalSessionEvent::Stopped => {
                active.stopped_by_user = true;
                active.status = "stopped".to_string();
            }
        }
        true
    }

    fn remove_queued(&mut self, tool_call_id: &str) {
        self.queue
            .retain(|request| request.start.tool_call_id != tool_call_id);
    }

    fn send_control(&self, control: TerminalSessionControl) {
        if let Some(active) = self.current.as_ref() {
            let _ = active.control_tx.send(control);
        }
    }

    pub fn send_input(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.send_control(TerminalSessionControl::Input(bytes));
        }
    }

    pub fn send_resize(&self, cols: u16, rows: u16) {
        self.send_control(TerminalSessionControl::Resize { rows, cols });
    }

    pub fn send_stop(&self) {
        self.send_control(TerminalSessionControl::Stop);
    }

    pub fn close_current(&mut self) {
        if let Some(mut active) = self.current.take() {
            active.stopped_by_user = true;
            let _ = active.control_tx.send(TerminalSessionControl::Stop);
        }
        self.last_terminal_size = (0, 0);
        if let Some(next) = self.queue.pop_front() {
            self.activate(next);
        }
    }

    pub fn clear_all_with_stop(&mut self) {
        if self.current.is_some() {
            self.send_stop();
            self.current = None;
        }
        self.queue.clear();
        self.last_terminal_size = (0, 0);
    }

    pub fn insert_paste(&mut self, text: &str) {
        self.send_input(text.as_bytes().to_vec());
    }

    fn title(&self) -> String {
        self.current
            .as_ref()
            .map(|a| a.start.description.clone())
            .unwrap_or_default()
    }

    fn status_line(&self) -> String {
        let Some(active) = self.current.as_ref() else {
            return String::new();
        };
        let queued = self.queue.len();
        if queued > 0 {
            format!("{} · {} queued", active.status, queued)
        } else {
            active.status.clone()
        }
    }
}

impl Default for TerminalSessionDialogState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_terminal_session_dialog() -> TerminalSessionDialogState {
    TerminalSessionDialogState::new()
}

/// Some terminals report Ctrl+] as Ctrl+5 because both map to the ASCII group separator.
fn is_terminal_stop_chord(event: KeyEvent) -> bool {
    event.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(event.code, KeyCode::Char(']' | '5'))
}

/// Encode a key event as PTY input bytes. The stop chord is reserved and not sent.
pub fn encode_terminal_key(event: KeyEvent) -> Option<Vec<u8>> {
    if is_terminal_stop_chord(event) {
        return None;
    }

    if event.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = event.code {
            let lower = c.to_ascii_lowercase();
            if ('a'..='z').contains(&lower) {
                return Some(vec![lower as u8 & 0x1f]);
            }
        }
    }

    match event.code {
        KeyCode::Char(c) => {
            if event.modifiers.intersects(KeyModifiers::ALT) {
                let mut v = vec![0x1b];
                v.extend(c.encode_utf8(&mut [0; 4]).bytes());
                Some(v)
            } else {
                Some(c.encode_utf8(&mut [0; 4]).as_bytes().to_vec())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::F(n) => {
            let seq = match n {
                1 => "\x1bOP",
                2 => "\x1bOQ",
                3 => "\x1bOR",
                4 => "\x1bOS",
                5 => "\x1b[15~",
                6 => "\x1b[17~",
                7 => "\x1b[18~",
                8 => "\x1b[19~",
                9 => "\x1b[20~",
                10 => "\x1b[21~",
                11 => "\x1b[23~",
                12 => "\x1b[24~",
                _ => return None,
            };
            Some(seq.as_bytes().to_vec())
        }
        _ => None,
    }
}

pub fn handle_terminal_session_dialog_key_event(
    state: &mut TerminalSessionDialogState,
    event: KeyEvent,
) -> TerminalSessionResponse {
    if !state.has_active() {
        return TerminalSessionResponse::NotHandled;
    }

    if is_terminal_stop_chord(event) {
        state.close_current();
        return if state.has_active() {
            TerminalSessionResponse::Handled
        } else {
            TerminalSessionResponse::Close
        };
    }

    if let Some(bytes) = encode_terminal_key(event) {
        state.send_input(bytes);
        return TerminalSessionResponse::Handled;
    }

    TerminalSessionResponse::NotHandled
}

pub fn render_terminal_session_dialog(
    f: &mut Frame,
    state: &mut TerminalSessionDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    if !state.has_active() {
        return;
    }

    let dialog_width = area.width.max(DIALOG_MIN_WIDTH).min(area.width);
    let dialog_height = area.height.max(DIALOG_MIN_HEIGHT).min(area.height);
    let dialog_area = Rect {
        x: area.x + area.width.saturating_sub(dialog_width) / 2,
        y: area.y + area.height.saturating_sub(dialog_height) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        dialog_area,
    );

    let inner = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors.border_focus))
        .style(Style::default().bg(colors.dialog_background));

    let inner_area = inner.inner(dialog_area);
    f.render_widget(inner, dialog_area);

    let padded = Rect {
        x: inner_area.x + PADDING_X,
        y: inner_area.y + PADDING_Y,
        width: inner_area.width.saturating_sub(PADDING_X * 2),
        height: inner_area.height.saturating_sub(PADDING_Y * 2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Length(CHROME_GAP_HEIGHT),
            Constraint::Min(0),
            Constraint::Length(CHROME_GAP_HEIGHT),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .split(padded);

    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Percentage(35)])
        .split(chunks[0]);
    f.render_widget(
        Paragraph::new(Span::styled(
            state.title(),
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )),
        header_chunks[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            state.status_line(),
            Style::default().fg(colors.info),
        ))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    let terminal_area = chunks[2];
    let term_cols = terminal_area.width.max(1);
    let term_rows = terminal_area.height.max(1);
    let needs_start = state.current.as_ref().is_some_and(|active| !active.started);
    if needs_start {
        state.last_terminal_size = (term_cols, term_rows);
        if let Some(active) = state.current.as_mut() {
            active.parser.set_size(term_rows, term_cols);
            active.started = true;
            let _ = active.control_tx.send(TerminalSessionControl::Start {
                rows: term_rows,
                cols: term_cols,
            });
        }
    } else if (term_cols, term_rows) != state.last_terminal_size {
        state.last_terminal_size = (term_cols, term_rows);
        if let Some(active) = state.current.as_mut() {
            active.parser.set_size(term_rows, term_cols);
        }
        state.send_resize(term_cols, term_rows);
    }

    if let Some(active) = state.current.as_ref() {
        render_vt_screen(f, active.parser.screen(), terminal_area, colors);
    }

    let footer = Line::from(vec![
        Span::styled(
            "ctrl+]",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" close/stop", Style::default().fg(colors.text_weak)),
    ]);
    f.render_widget(Paragraph::new(footer), chunks[4]);
}

fn render_vt_screen(f: &mut Frame, screen: &VtScreen, area: Rect, colors: ThemeColors) {
    let (rows, cols) = screen.size();
    let (cur_row, cur_col) = screen.cursor_position();
    let show_cursor = !screen.hide_cursor();

    let mut lines: Vec<Line> = Vec::new();
    for row in 0..rows.min(area.height) {
        let mut spans = Vec::new();
        for col in 0..cols.min(area.width) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let text = if cell.has_contents() {
                cell.contents()
            } else {
                " ".to_string()
            };
            let mut style = cell_style(cell, colors);
            if show_cursor && row == cur_row && col == cur_col {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(text, style));
        }
        if spans.is_empty() {
            spans.push(Span::styled(" ", Style::default().fg(colors.text)));
        }
        lines.push(Line::from(spans));
    }

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(colors.background_element)),
        area,
    );
}

fn cell_style(cell: &vt100::Cell, colors: ThemeColors) -> Style {
    let mut style = Style::default();
    let fg = map_vt_color(&cell.fgcolor(), colors, true);
    let bg = map_vt_color(&cell.bgcolor(), colors, false);
    if cell.inverse() {
        style = style.fg(bg).bg(fg);
    } else {
        style = style.fg(fg).bg(bg);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn map_vt_color(color: &VtColor, colors: ThemeColors, foreground: bool) -> Color {
    match color {
        VtColor::Default => {
            if foreground {
                colors.text
            } else {
                colors.background_element
            }
        }
        VtColor::Idx(i) => ansi_index_to_color(*i),
        VtColor::Rgb(r, g, b) => Color::Rgb(*r, *g, *b),
    }
}

fn ansi_index_to_color(idx: u8) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        16..=231 => {
            let idx = idx - 16;
            let r = idx / 36;
            let g = (idx / 6) % 6;
            let b = idx % 6;
            let cube = [0u8, 95, 135, 175, 215, 255];
            Color::Rgb(cube[r as usize], cube[g as usize], cube[b as usize])
        }
        232..=255 => {
            let level = 8 + (idx - 232) * 10;
            Color::Rgb(level, level, level)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn sample_start(id: &str, title: &str) -> TerminalSessionStart {
        TerminalSessionStart {
            session_id: id.into(),
            tool_call_id: format!("call-{id}"),
            command: "echo".into(),
            description: title.into(),
            workdir: None,
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn encode_enter_is_carriage_return() {
        let bytes = encode_terminal_key(KeyEvent::from(KeyCode::Enter)).unwrap();
        assert_eq!(bytes, vec![b'\r']);
    }

    #[test]
    fn encode_arrow_keys_use_csi() {
        let up = encode_terminal_key(KeyEvent::from(KeyCode::Up)).unwrap();
        assert_eq!(up, b"\x1b[A");
    }

    #[test]
    fn ctrl_bracket_is_not_encoded() {
        let ev = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert!(encode_terminal_key(ev).is_none());
    }

    #[test]
    fn ctrl_five_is_treated_as_ctrl_bracket() {
        let ev = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL);
        assert!(encode_terminal_key(ev).is_none());
        assert!(is_terminal_stop_chord(ev));
    }

    #[test]
    fn plain_five_is_still_terminal_input() {
        let ev = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE);
        assert_eq!(encode_terminal_key(ev), Some(vec![b'5']));
    }

    #[test]
    fn vt100_parser_applies_sgr_red() {
        let mut parser = VtParser::new(2, 16, 0);
        parser.process(b"\x1b[31mhi\x1b[m");
        let cell = parser.screen().cell(0, 0).unwrap();
        assert_eq!(cell.fgcolor(), VtColor::Idx(1));
        assert_eq!(cell.contents(), "h");
    }

    #[test]
    fn apply_event_routes_by_tool_call_id() {
        let (control_tx, _control_rx) = mpsc::unbounded_channel();
        let mut state = TerminalSessionDialogState::new();
        state.enqueue(TerminalSessionRequest {
            start: sample_start("1", "first"),
            control_tx,
        });
        assert!(!state.apply_event("other", TerminalSessionEvent::Output(b"skip".to_vec())));
        assert!(state.apply_event("call-1", TerminalSessionEvent::Output(b"x".to_vec())));
        let cell = state
            .current
            .as_ref()
            .unwrap()
            .parser
            .screen()
            .cell(0, 0)
            .unwrap();
        assert_eq!(cell.contents(), "x");
    }

    #[test]
    fn dialog_queue_activates_next_after_close() {
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        let mut state = TerminalSessionDialogState::new();
        state.enqueue(TerminalSessionRequest {
            start: sample_start("1", "first"),
            control_tx: tx1,
        });
        state.enqueue(TerminalSessionRequest {
            start: sample_start("2", "second"),
            control_tx: tx2,
        });
        assert_eq!(state.title(), "first");
        state.close_current();
        assert!(state.has_active());
        assert_eq!(state.title(), "second");
    }

    #[test]
    fn ctrl_bracket_closes_dialog() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = TerminalSessionDialogState::new();
        state.enqueue(TerminalSessionRequest {
            start: sample_start("1", "t"),
            control_tx: tx,
        });
        let resp = handle_terminal_session_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL),
        );
        assert_eq!(resp, TerminalSessionResponse::Close);
        assert!(!state.has_active());
    }

    #[test]
    fn ctrl_five_closes_dialog_for_legacy_terminal_encoding() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = TerminalSessionDialogState::new();
        state.enqueue(TerminalSessionRequest {
            start: sample_start("1", "t"),
            control_tx: tx,
        });

        let response = handle_terminal_session_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL),
        );

        assert_eq!(response, TerminalSessionResponse::Close);
        assert!(!state.has_active());
        assert!(matches!(
            rx.try_recv().unwrap(),
            TerminalSessionControl::Stop
        ));
    }
}
