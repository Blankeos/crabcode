use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::theme::ThemeColors;
use crate::ui::components::chat::Chat;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;
use crate::ui::components::wave_spinner::WaveSpinner;

pub const SUBAGENT_FOOTER_HEIGHT: u16 = 3;

#[derive(Debug)]
pub struct ChatState {
    pub chat: Chat,
    pub wave_spinner: WaveSpinner,
}

#[derive(Debug, Clone)]
pub struct SubagentTab {
    pub label: String,
    pub active: bool,
    pub running: bool,
    pub color: ratatui::style::Color,
}

#[derive(Debug, Clone)]
pub struct SubagentTabs {
    pub is_child_session: bool,
    pub tabs: Vec<SubagentTab>,
}

impl ChatState {
    pub fn new(chat: Chat, agent_color: ratatui::style::Color) -> Self {
        Self {
            chat,
            wave_spinner: WaveSpinner::with_speed(agent_color, 40),
        }
    }
}

pub fn init_chat(chat: Chat, agent: &str, colors: &ThemeColors) -> ChatState {
    let agent_color = crate::theme::agent_color(agent, colors);
    ChatState::new(chat, agent_color)
}

pub fn agent_color_for_tab(agent_index: usize, colors: &ThemeColors) -> ratatui::style::Color {
    // Matches OpenCode's visible agent rotation:
    // secondary/accent/success/warning/primary/error/info.
    match agent_index % 7 {
        0 => colors.secondary,
        1 => colors.accent,
        2 => colors.success,
        3 => colors.warning,
        4 => colors.primary,
        5 => colors.error,
        _ => colors.info,
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
    reasoning_effort: Option<String>,
    colors: &ThemeColors,
    is_streaming: bool,
    is_compacting: bool,
    usage_text: &str,
    subagent_tabs: Option<SubagentTabs>,
) {
    let size = f.area();
    let is_subagent_view = subagent_tabs
        .as_ref()
        .is_some_and(|tabs| tabs.is_child_session);

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let input_height = if is_subagent_view {
        SUBAGENT_FOOTER_HEIGHT
    } else {
        input.get_height_for_width(size.width)
    };
    let help_height = if is_subagent_view { 0 } else { 1 };
    let above_status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(0), // Reserved subagent header removed
                Constraint::Min(0),    // Chat content
                Constraint::Length(0), // Bottom padding
                Constraint::Length(input_height),
                Constraint::Length(help_height),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    chat_state
        .chat
        .render(f, above_status_chunks[1], &agent, &model, colors);

    if is_subagent_view {
        if let Some(tabs) = subagent_tabs.as_ref() {
            render_subagent_footer(
                f,
                above_status_chunks[3],
                tabs,
                usage_text,
                colors,
                is_streaming,
                is_compacting,
                &mut chat_state.wave_spinner,
            );
        }
    } else {
        input.render(
            f,
            above_status_chunks[3],
            &agent,
            &model,
            &provider_name,
            reasoning_effort.as_deref(),
            colors,
        );
    }

    if is_subagent_view {
        let blank = Block::default();
        f.render_widget(blank, above_status_chunks[5]);

        let status_bar = StatusBar::new(version, cwd, branch, agent, model);
        status_bar.render(f, main_chunks[1], colors);
        return;
    }

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
    let available_width = above_status_chunks[4].width;
    let help_width = help_width.min(available_width);

    let usage_width = if !usage_text.is_empty() {
        (usage_text.len() as u16 + 2).min(available_width.saturating_sub(help_width))
    } else {
        0
    };
    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(usage_width),
            Constraint::Length(help_width),
        ])
        .split(above_status_chunks[4]);

    if is_streaming {
        let agent_color = crate::theme::agent_color(&agent, colors);
        chat_state.wave_spinner.set_color(agent_color);

        let mut streaming_text = chat_state.wave_spinner.spans();

        if is_compacting {
            streaming_text.push(Span::raw(" "));
            streaming_text.push(Span::styled(
                "compacting context",
                Style::default().fg(colors.info),
            ));

            let streaming_paragraph = Paragraph::new(Line::from(streaming_text));
            f.render_widget(streaming_paragraph, status_chunks[0]);
        } else {
            let tps = chat_state.chat.get_streaming_tokens_per_sec();

            if let Some(tps) = tps {
                streaming_text.push(Span::raw(" "));
                streaming_text.push(Span::styled(
                    format!("{:.0}t/s", tps),
                    Style::default().fg(colors.info),
                ));
            }

            if let Some(elapsed) = chat_state.chat.get_streaming_elapsed_seconds() {
                streaming_text.push(Span::raw(if tps.is_some() { " · " } else { " " }));
                streaming_text.push(Span::styled(
                    format!("{:.1}s", elapsed),
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
    }

    if !usage_text.is_empty() {
        let usage = Paragraph::new(Line::from(vec![Span::styled(
            usage_text,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]));
        f.render_widget(usage, status_chunks[1]);
    }

    let help = Paragraph::new(help_line).alignment(Alignment::Right);
    f.render_widget(help, status_chunks[2]);

    let blank = Block::default();
    f.render_widget(blank, above_status_chunks[5]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1], colors);
}

fn render_subagent_footer(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    tabs: &SubagentTabs,
    usage_text: &str,
    colors: &ThemeColors,
    is_streaming: bool,
    is_compacting: bool,
    wave_spinner: &mut WaveSpinner,
) {
    if tabs.tabs.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }

    let child_tabs = tabs.tabs.iter().skip(1).collect::<Vec<_>>();
    let total = child_tabs.len().max(1);
    let active_index = child_tabs.iter().position(|tab| tab.active).unwrap_or(0);
    let active_tab = child_tabs
        .get(active_index)
        .copied()
        .or_else(|| child_tabs.first().copied());
    let label = active_tab
        .map(|tab| tab.label.as_str())
        .unwrap_or("Subagent");
    let running = active_tab.is_some_and(|tab| tab.running);
    let active_color = active_tab.map(|tab| tab.color).unwrap_or(colors.primary);

    let border_set = border::Set {
        vertical_left: "┃",
        ..border::PLAIN
    };
    let border = Block::new()
        .borders(Borders::LEFT)
        .border_set(border_set)
        .border_style(Style::default().fg(active_color));
    let inner_area = border.inner(area);

    let bg = Block::default().style(Style::default().bg(colors.background_element));
    f.render_widget(bg, area);
    f.render_widget(border, area);

    let content_area = centered_subagent_footer_content(inner_area);
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let mut left_spans = vec![
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" ({} of {})", active_index + 1, total)),
    ];

    if running {
        left_spans.push(Span::raw(" "));
        left_spans.push(Span::styled("~", Style::default().fg(active_color)));
    }

    if !usage_text.is_empty() {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            usage_text.to_string(),
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
    }

    if is_streaming {
        wave_spinner.set_color(active_color);
        left_spans.push(Span::raw("  "));
        if is_compacting {
            left_spans.push(Span::styled(
                "compacting context",
                Style::default().fg(colors.info),
            ));
        } else {
            left_spans.extend(wave_spinner.spans());
            left_spans.push(Span::raw(" "));
            left_spans.push(Span::styled(
                "esc to stop",
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
        }
    }

    let nav_line = Line::from(vec![
        Span::styled(
            "Parent ",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("up", Style::default().fg(colors.text)),
        Span::raw("  "),
        Span::styled(
            "Prev ",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("left", Style::default().fg(colors.text)),
        Span::raw("  "),
        Span::styled(
            "Next ",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled("right", Style::default().fg(colors.text)),
    ]);

    let nav_width = nav_line.width() as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(nav_width.min(content_area.width)),
        ])
        .split(content_area);

    f.render_widget(Paragraph::new(Line::from(left_spans)), chunks[0]);
    f.render_widget(
        Paragraph::new(nav_line).alignment(Alignment::Right),
        chunks[1],
    );
}

fn centered_subagent_footer_content(area: Rect) -> Rect {
    if area.width <= 3 || area.height == 0 {
        return Rect::new(area.x, area.y, area.width, area.height.min(1));
    }

    Rect {
        x: area.x + 2,
        y: area.y + area.height / 2,
        width: area.width.saturating_sub(3),
        height: 1,
    }
}
