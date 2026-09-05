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
use crate::views::chat::{btw_panel_height, chat_input_height, render_btw_panel};

const LOGO: &str = include_str!("../../crabcode-logo.txt");
const MASCOT: &str = include_str!("../../mascot.txt");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpSummary {
    pub connected: usize,
    pub enabled: usize,
    pub has_error: bool,
}

fn mcp_status(summary: McpSummary) -> Option<(&'static str, String)> {
    if summary.enabled == 0 {
        return None;
    }
    let indicator = if summary.has_error {
        "●"
    } else if summary.connected == summary.enabled {
        "●"
    } else {
        "◐"
    };
    let count = if summary.connected == summary.enabled {
        summary.connected.to_string()
    } else {
        format!("{}/{}", summary.connected, summary.enabled)
    };
    Some((indicator, format!(" {count} MCP")))
}

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
    reasoning_effort_explicit: bool,
    mcp_summary: McpSummary,
    colors: &ThemeColors,
    usage_text: &str,
    btw_entry: Option<&crate::app::BtwEntry>,
    btw_scroll: usize,
    btw_panel_area: &mut Option<ratatui::layout::Rect>,
) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let btw_height = btw_panel_height(btw_entry, size.width, colors);
    let input_height = chat_input_height(input, size.width, size.height, 0, btw_height, 1);
    let home_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(btw_height),
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
        home_chunks[2],
        &agent,
        &model,
        &provider_name,
        reasoning_effort.as_deref(),
        reasoning_effort_explicit,
        colors,
        true,
    );
    render_btw_panel(
        f,
        home_chunks[1],
        btw_entry,
        btw_scroll,
        btw_panel_area,
        colors,
    );

    let help_text = vec![
        Span::styled("tab", Style::default().fg(colors.info)),
        Span::raw(" agents  "),
        Span::styled("ctrl+p", Style::default().fg(colors.info)),
        Span::raw(" commands"),
    ];
    let help_line = Line::from(help_text);
    let help_width = help_line.width() as u16;
    let available_width = home_chunks[3].width;
    let help_width = help_width.min(available_width);

    let mut status_spans = Vec::new();
    if !usage_text.is_empty() {
        status_spans.push(Span::styled(
            usage_text.to_string(),
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
    }
    if let Some((indicator, label)) = mcp_status(mcp_summary) {
        if !status_spans.is_empty() {
            status_spans.push(Span::raw("  "));
        }
        let color = if mcp_summary.has_error {
            colors.error
        } else if mcp_summary.connected == mcp_summary.enabled {
            colors.success
        } else {
            colors.warning
        };
        status_spans.push(Span::styled(indicator, Style::default().fg(color)));
        status_spans.push(Span::styled(label, Style::default().fg(colors.text)));
    }
    let status_line = Line::from(status_spans);
    let status_width = (status_line.width() as u16).min(available_width.saturating_sub(help_width));

    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(status_width),
            Constraint::Min(0),
            Constraint::Length(help_width),
        ])
        .split(home_chunks[3]);

    if status_width > 0 {
        f.render_widget(Paragraph::new(status_line), status_chunks[0]);
    }

    let help = Paragraph::new(help_line).alignment(Alignment::Right);
    f.render_widget(help, status_chunks[2]);

    // Keep spacer on theme canvas (don't Reset over solid bg).
    f.render_widget(
        Block::default().style(Style::default().bg(colors.background)),
        home_chunks[4],
    );

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1], colors);
}

#[cfg(test)]
mod tests {
    use super::{mcp_status, McpSummary};

    #[test]
    fn mcp_status_hides_when_no_servers_are_enabled() {
        assert_eq!(mcp_status(McpSummary::default()), None);
    }

    #[test]
    fn mcp_status_shows_connected_and_total_counts() {
        assert_eq!(
            mcp_status(McpSummary {
                connected: 1,
                enabled: 2,
                has_error: false,
            }),
            Some(("◐", " 1/2 MCP".to_string()))
        );
        assert_eq!(
            mcp_status(McpSummary {
                connected: 2,
                enabled: 2,
                has_error: false,
            }),
            Some(("●", " 2 MCP".to_string()))
        );
    }
}
