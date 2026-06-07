use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    Frame,
};

use unicode_width::UnicodeWidthStr;

use crate::theme::ThemeColors;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;

const LOGO: &str = include_str!("../../crabcode-logo.txt");
const MASCOT: &str = include_str!("../../mascot.txt");

#[derive(Debug, Clone)]
pub struct HomeState {
    phase: u8,
    tick_count: u32,
}

const PHASE_DURATIONS: [u32; 5] = [14, 7, 7, 7, 14];
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
    reasoning_effort: Option<String>,
    colors: &ThemeColors,
    usage_text: &str,
) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let input_height = input.get_height_for_width(size.width);
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
    let logo_area_height = if is_wide { 7 } else { 7 };

    let logo_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(logo_area_height),
            Constraint::Min(0),
        ])
        .split(home_chunks[0]);

    let mascot_frame = MASCOT
        .trim_end()
        .split("\n\n")
        .nth(home_state.frame())
        .or_else(|| MASCOT.trim_end().split("\n\n").next())
        .unwrap_or("");
    let mascot_raw: Vec<&str> = mascot_frame.lines().filter(|l| !l.is_empty()).collect();

    let logo_raw: Vec<&str> = LOGO.lines().filter(|l| !l.is_empty()).collect();
    let logo_width = logo_raw
        .iter()
        .map(|line| UnicodeWidthStr::width(*line))
        .max()
        .unwrap_or(0);
    let logo_lines: Vec<Line> = logo_raw
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let color = if i == 2 {
                crate::theme::darken_color(colors.primary, 0.7)
            } else {
                colors.primary
            };
            let padded = format!(
                "{}{}",
                line,
                " ".repeat(logo_width.saturating_sub(UnicodeWidthStr::width(*line)))
            );
            Line::styled(
                padded,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    if is_wide {
        let stack = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(logo_chunks[1]);

        let max_mascot_width = mascot_raw
            .iter()
            .map(|l| UnicodeWidthStr::width(*l))
            .max()
            .unwrap_or(0);
        let left_pad = ((stack[0].width as usize).saturating_sub(max_mascot_width)) / 2;
        let padding = " ".repeat(left_pad);

        let mascot_lines: Vec<Line> = mascot_raw
            .iter()
            .map(|line| {
                let padded = format!("{}{}", padding, line);
                Line::styled(
                    padded,
                    Style::default()
                        .fg(colors.primary)
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect();
        let mascot = Paragraph::new(Text::from(mascot_lines));
        let logo = Paragraph::new(Text::from(logo_lines)).alignment(Alignment::Center);

        f.render_widget(mascot, stack[0]);
        f.render_widget(logo, stack[2]);
    } else {
        let stack = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .split(logo_chunks[1]);

        let max_mascot_width = mascot_raw
            .iter()
            .map(|l| UnicodeWidthStr::width(*l))
            .max()
            .unwrap_or(0);
        let left_pad = ((stack[0].width as usize).saturating_sub(max_mascot_width)) / 2;
        let padding = " ".repeat(left_pad);

        let mascot_lines: Vec<Line> = mascot_raw
            .iter()
            .map(|line| {
                let padded = format!("{}{}", padding, line);
                Line::styled(
                    padded,
                    Style::default()
                        .fg(colors.primary)
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect();
        let mascot = Paragraph::new(Text::from(mascot_lines));
        let logo = Paragraph::new(Text::from(logo_lines)).alignment(Alignment::Center);

        f.render_widget(mascot, stack[0]);
        f.render_widget(logo, stack[2]);
    }
    input.render(
        f,
        home_chunks[1],
        &agent,
        &model,
        &provider_name,
        reasoning_effort.as_deref(),
        colors,
    );

    let help_text = vec![
        Span::styled("ctrl+p", Style::default().fg(colors.info)),
        Span::raw(" commands"),
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
