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
    ShowTimeline,
    /// Ctrl+X opens WhichKey; bind `j` here for jobs (don't steal Ctrl+X).
    ShowJobs,
    ToggleThinking,
    GoChild,
    GoParent,
    PreviousChild,
    NextChild,
    NewSession,
    Quit,
    ScrollUp,
    ScrollDown,
    ScrollToTop,
    ScrollToBottom,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhichKeySubmenu {
    Goto,
}

#[derive(Debug, Clone)]
enum BindingTarget {
    Action(WhichKeyAction),
    Submenu(WhichKeySubmenu),
}

#[derive(Debug, Clone)]
struct KeyBinding {
    key: String,
    description: String,
    target: BindingTarget,
}

#[derive(Debug)]
pub struct WhichKeyState {
    pub visible: bool,
    root_bindings: Vec<KeyBinding>,
    chat_bindings: Vec<KeyBinding>,
    goto_bindings: Vec<KeyBinding>,
    active_submenu: Option<WhichKeySubmenu>,
    pub last_key_time: Instant,
    pub is_chat_active: bool,
}

impl WhichKeyState {
    pub fn new() -> Self {
        let root_bindings = vec![
            KeyBinding {
                key: "m".to_string(),
                description: "Open Models dialog".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ShowModels),
            },
            KeyBinding {
                key: "t".to_string(),
                description: "Open Themes dialog".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ShowThemes),
            },
            KeyBinding {
                key: "l".to_string(),
                description: "Open Sessions dialog".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ShowSessions),
            },
            KeyBinding {
                key: "n".to_string(),
                description: "Create new session".to_string(),
                target: BindingTarget::Action(WhichKeyAction::NewSession),
            },
            // Ctrl+X opens WhichKey; jobs are bound here (don't steal Ctrl+X).
            KeyBinding {
                key: "j".to_string(),
                description: "Jobs (background/interactive)".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ShowJobs),
            },
            KeyBinding {
                key: "q".to_string(),
                description: "Quit application".to_string(),
                target: BindingTarget::Action(WhichKeyAction::Quit),
            },
        ];

        let chat_bindings = vec![
            KeyBinding {
                key: "↓".to_string(),
                description: "Go to first subagent session".to_string(),
                target: BindingTarget::Action(WhichKeyAction::GoChild),
            },
            KeyBinding {
                key: "↑".to_string(),
                description: "Go to parent session".to_string(),
                target: BindingTarget::Action(WhichKeyAction::GoParent),
            },
            KeyBinding {
                key: "←".to_string(),
                description: "Previous subagent session".to_string(),
                target: BindingTarget::Action(WhichKeyAction::PreviousChild),
            },
            KeyBinding {
                key: "→".to_string(),
                description: "Next subagent session".to_string(),
                target: BindingTarget::Action(WhichKeyAction::NextChild),
            },
            KeyBinding {
                key: "g".to_string(),
                description: "Goto…".to_string(),
                target: BindingTarget::Submenu(WhichKeySubmenu::Goto),
            },
            KeyBinding {
                key: "e".to_string(),
                description: "Expand/collapse thinking".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ToggleThinking),
            },
            KeyBinding {
                key: "k".to_string(),
                description: "Scroll up".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ScrollUp),
            },
            KeyBinding {
                key: "j".to_string(),
                description: "Scroll down".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ScrollDown),
            },
        ];

        let goto_bindings = vec![
            KeyBinding {
                key: "g".to_string(),
                description: "Scroll to top".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ScrollToTop),
            },
            KeyBinding {
                key: "e".to_string(),
                description: "Scroll to bottom".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ScrollToBottom),
            },
            KeyBinding {
                key: "t".to_string(),
                description: "Open Messages Timeline dialog".to_string(),
                target: BindingTarget::Action(WhichKeyAction::ShowTimeline),
            },
        ];

        Self {
            visible: false,
            root_bindings,
            chat_bindings,
            goto_bindings,
            active_submenu: None,
            last_key_time: Instant::now(),
            is_chat_active: false,
        }
    }

    pub fn set_chat_active(&mut self, active: bool) {
        self.is_chat_active = active;
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.active_submenu = None;
        self.update_last_key_time();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.active_submenu = None;
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

    fn current_bindings(&self) -> Vec<&KeyBinding> {
        match self.active_submenu {
            Some(WhichKeySubmenu::Goto) => self.goto_bindings.iter().collect(),
            None => {
                let mut bindings: Vec<&KeyBinding> = self.root_bindings.iter().collect();
                if self.is_chat_active {
                    bindings.extend(self.chat_bindings.iter());
                }
                bindings
            }
        }
    }

    fn title(&self) -> &'static str {
        match self.active_submenu {
            Some(WhichKeySubmenu::Goto) => "Shortcuts · g",
            None => "Shortcuts",
        }
    }

    fn match_key(&self, code: KeyCode) -> Option<&KeyBinding> {
        let key = match code {
            KeyCode::Char(c) => c.to_ascii_lowercase().to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            _ => return None,
        };

        self.current_bindings()
            .into_iter()
            .find(|binding| binding.key == key)
    }

    pub fn handle_key_event(&mut self, event: KeyEvent) -> WhichKeyAction {
        self.update_last_key_time();

        match event.code {
            KeyCode::Esc => {
                self.hide();
                WhichKeyAction::None
            }
            code => {
                let target = self.match_key(code).map(|b| b.target.clone());
                match target {
                    Some(BindingTarget::Submenu(submenu)) => {
                        self.active_submenu = Some(submenu);
                        WhichKeyAction::None
                    }
                    Some(BindingTarget::Action(action)) => {
                        self.hide();
                        action
                    }
                    None => WhichKeyAction::None,
                }
            }
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
    let bindings = state.current_bindings();
    let bindings_count = bindings.len();

    // Scale like the Dialog component (which is 70×25) — broad enough to visually
    // anchor the popup and cover behind-the-modal content (logo, scrollbar artefacts).
    const POPUP_WIDTH: u16 = 58;

    let popup_width = area.width.min(POPUP_WIDTH);
    let popup_height = area.height.min((bindings_count + 7) as u16);

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
    const PADDING_X: u16 = 3;
    const PADDING_Y: u16 = 1;
    let content_area = Rect {
        x: popup_area.x + PADDING_X,
        y: popup_area.y + PADDING_Y,
        width: popup_area.width.saturating_sub(PADDING_X * 2),
        height: popup_area.height.saturating_sub(PADDING_Y * 2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // spacer
            Constraint::Min(1),    // bindings
            Constraint::Length(1), // spacer
            Constraint::Length(1), // footer
        ])
        .split(content_area);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            state.title(),
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Left),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::new();
    for binding in bindings {
        let key_span = Span::styled(
            format!("  {}  ", binding.key),
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        );
        let desc_span = Span::styled(&binding.description, Style::default().fg(colors.text));
        lines.push(Line::from(vec![key_span, Span::raw(" "), desc_span]));
    }

    f.render_widget(Paragraph::new(lines).alignment(Alignment::Left), chunks[2]);

    let footer = "Press a key to execute, ESC to cancel";

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            footer,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]))
        .alignment(Alignment::Left),
        chunks[4],
    );
}
