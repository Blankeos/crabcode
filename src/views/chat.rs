use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::theme::ThemeColors;
use crate::ui::components::chat::Chat;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;
use crate::ui::components::wave_spinner::WaveSpinner;

#[derive(Debug)]
pub struct ChatState {
    pub chat: Chat,
    pub wave_spinner: WaveSpinner,
}

impl ChatState {
    pub fn new(chat: Chat, agent_color: ratatui::style::Color) -> Self {
        Self {
            chat,
            wave_spinner: WaveSpinner::with_speed(agent_color, 40),
        }
    }
}

pub fn init_chat(chat: Chat, agent: &str) -> ChatState {
    let agent_color = get_agent_color(agent);
    ChatState::new(chat, agent_color)
}

fn get_agent_color(agent: &str) -> ratatui::style::Color {
    match agent {
        "Plan" => ratatui::style::Color::Rgb(255, 165, 0), // Orange
        "Build" => ratatui::style::Color::Rgb(147, 112, 219), // Purple
        _ => ratatui::style::Color::Gray,
    }
}

pub fn render_chat(
    f: &mut Frame,
    chat_state: &mut ChatState,
    input: &mut Input,
    version: String,
    cwd: String,
    branch: Option<String>,
    agent: String,
    model: String,
    provider_name: String,
    colors: &ThemeColors,
    is_streaming: bool,
) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let input_height = input.get_height();
    let above_status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(1), // Top padding
                Constraint::Min(0),    // Chat content
                Constraint::Length(1), // Bottom padding
                Constraint::Length(input_height),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    chat_state
        .chat
        .render(f, above_status_chunks[1], &agent, &model, colors);
    input.render(f, above_status_chunks[3], &agent, &model, &provider_name);

    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(35)])
        .split(above_status_chunks[4]);

    if is_streaming {
        // Update spinner color based on current agent (only if changed)
        let agent_color = get_agent_color(&agent);
        chat_state.wave_spinner.set_color(agent_color);

        // Animation update is now handled in the main event loop at a fixed rate
        // to prevent speed issues when mouse movement causes frequent redraws
        let mut streaming_text = chat_state.wave_spinner.spans();

        // Add tokens/second if available
        if let Some(tps) = chat_state.chat.get_streaming_tokens_per_sec() {
            streaming_text.push(Span::raw(" "));
            streaming_text.push(Span::styled(
                format!("{:.0}t/s", tps),
                Style::default().fg(colors.info),
            ));
        }

        streaming_text.push(Span::raw("  "));
        streaming_text.push(Span::styled(
            "esc to stop",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));

        let streaming_paragraph = Paragraph::new(Line::from(streaming_text));
        f.render_widget(streaming_paragraph, status_chunks[0]);
    }

    let help_text = vec![
        Span::styled("/", Style::default().fg(colors.info)),
        Span::raw(" commands  "),
        Span::styled("tab", Style::default().fg(colors.info)),
        Span::raw(" agents  "),
        Span::styled("ctrl+cc", Style::default().fg(colors.info)),
        Span::raw(" quit"),
    ];
    let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
    f.render_widget(help, status_chunks[1]);

    let blank = Block::default();
    f.render_widget(blank, above_status_chunks[5]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1]);
}
