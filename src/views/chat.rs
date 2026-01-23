use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

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
) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let chat_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(3),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    chat_state.chat.render(f, chat_chunks[0]);
    input.render(f, chat_chunks[1]);

    let help_text = vec![
        Span::styled("/", Style::default().fg(Color::Rgb(255, 140, 0))),
        Span::raw(" commands  "),
        Span::styled("tab", Style::default().fg(Color::Rgb(255, 140, 0))),
        Span::raw(" agents  "),
        Span::styled("ctrl+cc", Style::default().fg(Color::Rgb(255, 140, 0))),
        Span::raw(" quit"),
    ];
    let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
    f.render_widget(help, chat_chunks[2]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1]);
}
