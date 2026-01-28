use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::time::{Duration, Instant};

use crate::theme::ThemeColors;

const TIMEOUT_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum WhichKeyAction {
    ShowModels,
    ShowSessions,
    NewSession,
    Quit,
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
    pub last_key_time: Instant,
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

        Self {
            visible: false,
            bindings,
            last_key_time: Instant::now(),
        }
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
    let popup_width = 40u16;
    let popup_height = 12u16;

    let popup_area = Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Shortcuts ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors.border_focus))
        .title_style(
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        );

    let mut lines: Vec<Line> = vec![];

    lines.push(Line::from(""));

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

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  ESC ",
            Style::default()
                .fg(colors.info)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("cancel", Style::default().fg(colors.text_weak)),
    ]));

    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, popup_area);
}
