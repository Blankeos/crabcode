use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;

const LOGO: &str = r#"
🦀▄▄▄▄ ▄▄▄▄   ▄▄▄  ▄▄▄▄   ▄▄▄▄  ▄▄▄  ▄▄▄▄  ▄▄▄▄▄
██▀▀▀ ██▄█▄ ██▀██ ██▄██ ██▀▀▀ ██▀██ ██▀██ ██▄▄
▀████ ██ ██ ██▀██ ██▄█▀ ▀████ ▀███▀ ████▀ ██▄▄▄
"#;

#[derive(Debug, Clone)]
pub struct HomeState;

impl HomeState {
    pub fn new() -> Self {
        Self
    }
}

pub fn init_home() -> HomeState {
    HomeState::new()
}

pub fn render_home(
    f: &mut Frame,
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

    let input_height = input.get_height();
    let home_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(input_height),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    let logo_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(home_chunks[0]);

    let logo = Paragraph::new(LOGO.trim())
        .style(
            Style::default()
                .fg(Color::Rgb(255, 140, 0))
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);

    f.render_widget(logo, logo_chunks[1]);
    input.render(f, home_chunks[1]);

    let help_text = vec![
        Span::styled("/", Style::default().fg(Color::Cyan)),
        Span::raw(" commands  "),
        Span::styled("tab", Style::default().fg(Color::Cyan)),
        Span::raw(" agents  "),
        Span::styled("ctrl+cc", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ];
    let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
    f.render_widget(help, home_chunks[2]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1]);
}
