use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::theme::ThemeColors;
use crate::ui::components::chat::Chat;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;

#[derive(Debug, Clone)]
pub struct ChatState {
    pub chat: Chat,
}

impl ChatState {
    pub fn new(chat: Chat) -> Self {
        Self { chat }
    }
}

pub fn init_chat(chat: Chat) -> ChatState {
    ChatState::new(chat)
}

pub fn render_chat(
    f: &mut Frame,
    chat_state: &ChatState,
    input: &Input,
    version: String,
    cwd: String,
    branch: Option<String>,
    agent: String,
    model: String,
    colors: &ThemeColors,
) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let above_status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    chat_state.chat.render(f, above_status_chunks[0]);
    input.render(f, above_status_chunks[1], &agent, &model);

    let help_text = vec![
        Span::styled("/", Style::default().fg(colors.info)),
        Span::raw(" commands  "),
        Span::styled("tab", Style::default().fg(colors.info)),
        Span::raw(" agents  "),
        Span::styled("ctrl+cc", Style::default().fg(colors.info)),
        Span::raw(" quit"),
    ];
    let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
    f.render_widget(help, above_status_chunks[2]);

    let blank = Block::default();
    f.render_widget(blank, above_status_chunks[3]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1]);
}
