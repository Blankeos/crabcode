use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::theme::ThemeColors;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;

const LOGO: &str = r#"
 ▄▄▄▄ ▄▄▄▄   ▄▄▄  ▄▄▄▄   ▄▄▄▄  ▄▄▄  ▄▄▄▄  ▄▄▄▄▄
██▀▀▀ ██▄█▄ ██▀██ ██▄██ ██▀▀▀ ██▀██ ██▀██ ██▄▄
▀████ ██ ██ ██▀██ ██▄█▀ ▀████ ▀███▀ ████▀ ██▄▄▄
"#;

const MASCO: [&str; 2] = [
    r#"
    ▃▃▛████▜▃▃
 █▟▟▜████████▛▙▙█
    ▞ ▘    ▝ ▚
"#,
    r#"
    ▃▃▛████▜▃▃
 █▙▟▜████████▛▙▟█
    ▞ ▘    ▝ ▚
"#,
];

#[derive(Debug, Clone)]
pub struct HomeState {
    phase: u8,
    tick_count: u32,
}

const PHASE_DURATIONS: [u32; 5] = [20, 10, 10, 10, 20];
const PHASE_FRAMES: [usize; 5] = [0, 1, 0, 1, 0];

impl HomeState {
    pub fn new() -> Self {
        Self {
            phase: 0,
            tick_count: 0,
        }
    }

    pub fn tick(&mut self) {
        self.tick_count += 1;
        if self.tick_count >= PHASE_DURATIONS[self.phase as usize] {
            self.tick_count = 0;
            self.phase = (self.phase + 1) % PHASE_DURATIONS.len() as u8;
        }
    }

    pub fn frame(&self) -> usize {
        PHASE_FRAMES[self.phase as usize]
    }
}

pub fn init_home() -> HomeState {
    HomeState::new()
}

pub fn render_home(
    f: &mut Frame,
    input: &mut Input,
    home_state: &HomeState,
    version: String,
    cwd: String,
    branch: Option<String>,
    agent: String,
    model: String,
    provider_name: String,
    colors: &ThemeColors,
    usage_text: &str,
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
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    let is_wide = size.width >= 80;
    let logo_area_height = if is_wide { 5 } else { 7 };

    let logo_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(logo_area_height),
            Constraint::Min(0),
        ])
        .split(home_chunks[0]);

    let mascot_lines: Vec<Line> = MASCO[home_state.frame()]
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            Line::styled(
                line,
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    let logo_lines: Vec<Line> = LOGO
        .lines()
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, line)| {
            let color = if i == 2 {
                crate::theme::darken_color(colors.primary, 0.7)
            } else {
                colors.primary
            };
            Line::styled(
                line,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    if is_wide {
        let logo_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(22),
                Constraint::Min(55),
                Constraint::Fill(1),
            ])
            .split(logo_chunks[1]);

        let mascot = Paragraph::new(Text::from(mascot_lines));
        let logo = Paragraph::new(Text::from(logo_lines)).alignment(Alignment::Center);

        f.render_widget(mascot, logo_row[1]);
        f.render_widget(logo, logo_row[2]);
    } else {
        let stack = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(logo_chunks[1]);

        let mascot = Paragraph::new(Text::from(mascot_lines)).alignment(Alignment::Center);
        let logo = Paragraph::new(Text::from(logo_lines)).alignment(Alignment::Center);

        f.render_widget(mascot, stack[0]);
        f.render_widget(logo, stack[2]);
    }
    input.render(f, home_chunks[1], &agent, &model, &provider_name, colors);

    let help_text = vec![
        Span::styled("/", Style::default().fg(colors.info)),
        Span::raw(" commands  "),
        Span::styled("ctrl+x", Style::default().fg(colors.info)),
        Span::raw(" shortcuts  "),
        Span::styled("tab", Style::default().fg(colors.info)),
        Span::raw(" agents  "),
        Span::styled("ctrl+cc", Style::default().fg(colors.info)),
        Span::raw(" quit "),
    ];
    let help_line = Line::from(help_text);
    let help_width = help_line.width() as u16;
    let available_width = home_chunks[2].width;
    let help_width = help_width.min(available_width);

    let usage_width = if !usage_text.is_empty() {
        (usage_text.len() as u16 + 2).min(available_width.saturating_sub(help_width))
    } else {
        0
    };

    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(usage_width),
            Constraint::Min(0),
            Constraint::Length(help_width),
        ])
        .split(home_chunks[2]);

    if !usage_text.is_empty() {
        let usage = Paragraph::new(Line::from(vec![Span::styled(
            usage_text,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]));
        f.render_widget(usage, status_chunks[0]);
    }

    let help = Paragraph::new(help_line).alignment(Alignment::Right);
    f.render_widget(help, status_chunks[2]);

    let blank = Block::default();
    f.render_widget(blank, home_chunks[3]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1], colors);
}
