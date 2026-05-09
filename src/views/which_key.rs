use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use std::time::{Duration, Instant};

use crate::theme::ThemeColors;

const TIMEOUT_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum WhichKeyAction {
    ShowModels,
    ShowThemes,
    ShowSessions,
    NewSession,
    Quit,
    ScrollUp,
    ScrollDown,
    None,
}

#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: String,
    pub description: String,
    pub action: WhichKeyAction,
}

#[derive(Debug)]
pub struct WhichKeyState {
    pub visible: bool,
    pub bindings: Vec<KeyBinding>,
    pub chat_bindings: Vec<KeyBinding>,
    pub last_key_time: Instant,
    pub is_chat_active: bool,
}

impl WhichKeyState {
    pub fn new() -> Self {
        let bindings = vec![
            KeyBinding {
                key: "m".to_string(),
                description: "Open Models dialog".to_string(),
                action: WhichKeyAction::ShowModels,
            },
            KeyBinding {
                key: "t".to_string(),
                description: "Open Themes dialog".to_string(),
                action: WhichKeyAction::ShowThemes,
            },
            KeyBinding {
                key: "l".to_string(),
                description: "Open Sessions dialog".to_string(),
                action: WhichKeyAction::ShowSessions,
            },
            KeyBinding {
                key: "n".to_string(),
                description: "Create new session".to_string(),
                action: WhichKeyAction::NewSession,
            },
            KeyBinding {
                key: "q".to_string(),
                description: "Quit application".to_string(),
                action: WhichKeyAction::Quit,
            },
        ];

        let chat_bindings = vec![
            KeyBinding {
                key: "k".to_string(),
                description: "Scroll up".to_string(),
                action: WhichKeyAction::ScrollUp,
            },
            KeyBinding {
                key: "j".to_string(),
                description: "Scroll down".to_string(),
                action: WhichKeyAction::ScrollDown,
            },
        ];

        Self {
            visible: false,
            bindings,
            chat_bindings,
            last_key_time: Instant::now(),
            is_chat_active: false,
        }
    }

    pub fn set_chat_active(&mut self, active: bool) {
        self.is_chat_active = active;
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.last_key_time = Instant::now();
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn is_timed_out(&self) -> bool {
        Instant::now().duration_since(self.last_key_time) > Duration::from_secs(TIMEOUT_SECONDS)
    }

    pub fn update_last_key_time(&mut self) {
        self.last_key_time = Instant::now();
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> WhichKeyAction {
        self.update_last_key_time();

        match event.code {
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.hide();
                WhichKeyAction::ShowModels
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.hide();
                WhichKeyAction::ShowThemes
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                self.hide();
                WhichKeyAction::ShowSessions
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.hide();
                WhichKeyAction::NewSession
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.hide();
                WhichKeyAction::Quit
            }
            KeyCode::Char('k') | KeyCode::Char('K') if self.is_chat_active => {
                self.hide();
                WhichKeyAction::ScrollUp
            }
            KeyCode::Char('j') | KeyCode::Char('J') if self.is_chat_active => {
                self.hide();
                WhichKeyAction::ScrollDown
            }
            KeyCode::Esc => {
                self.hide();
                WhichKeyAction::None
            }
            _ => WhichKeyAction::None,
        }
    }
}

impl Default for WhichKeyState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_which_key() -> WhichKeyState {
    WhichKeyState::new()
}

pub fn render_which_key(f: &mut Frame, state: &WhichKeyState, colors: &ThemeColors) {
    if !state.visible {
        return;
    }

    let area = f.area();
    let chat_bindings_count = if state.is_chat_active {
        state.chat_bindings.len()
    } else {
        0
    };
    let bindings_count = state.bindings.len() + chat_bindings_count;

    // Scale like the Dialog component (which is 70×25) — broad enough to visually
    // anchor the popup and cover behind-the-modal content (logo, scrollbar artefacts).
    const POPUP_WIDTH: u16 = 58;

    let popup_width = area.width.min(POPUP_WIDTH);
    let popup_height = area.height.min((bindings_count + 10) as u16);

    let popup_area = Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    // Clear and fill background (flat style like other dialogs)
    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        popup_area,
    );

    // Content area with padding (matching Dialog component)
    const PADDING: u16 = 3;
    let content_area = Rect {
        x: popup_area.x + PADDING,
        y: popup_area.y + PADDING,
        width: popup_area.width.saturating_sub(PADDING * 2),
        height: popup_area.height.saturating_sub(PADDING * 2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                      // top margin
            Constraint::Length(1),                      // title
            Constraint::Length(bindings_count as u16),  // bindings
            Constraint::Length(1),                      // spacer
            Constraint::Length(1),                      // footer
        ])
        .split(content_area);

    // Header: title (left) and esc hint (right) — same as Dialog
    let esc_text = "esc";
    let esc_width = esc_text.len() as u16;
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(esc_width)])
        .split(chunks[1]);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Shortcuts",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Left),
        header_chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            esc_text,
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    // Bindings
    let mut lines: Vec<Line> = vec![];

    for binding in &state.bindings {
        let key_span = Span::styled(
            format!("  {}  ", binding.key),
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        );
        let desc_span = Span::styled(&binding.description, Style::default().fg(colors.text));
        lines.push(Line::from(vec![key_span, Span::raw(" "), desc_span]));
    }

    if state.is_chat_active {
        for binding in &state.chat_bindings {
            let key_span = Span::styled(
                format!("  {}  ", binding.key),
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            );
            let desc_span = Span::styled(&binding.description, Style::default().fg(colors.text));
            lines.push(Line::from(vec![key_span, Span::raw(" "), desc_span]));
        }
    }

    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Left),
        chunks[2],
    );

    // Footer — dim hint matching Dialog footer style
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Press a key to execute, ESC to cancel",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]))
        .alignment(Alignment::Left),
        chunks[4],
    );
}
