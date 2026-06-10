use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::ThemeColors;
use crate::ui::components::chat::Chat;
use crate::ui::components::find::FindBar;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;
use crate::ui::components::wave_spinner::WaveSpinner;

pub const SUBAGENT_FOOTER_HEIGHT: u16 = 3;
const QUEUED_MESSAGES_MAX_VISIBLE: usize = 3;
const QUEUED_MESSAGES_TOP_PADDING: u16 = 1;
const QUEUED_MESSAGES_BOTTOM_PADDING: u16 = 1;
const STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH: u16 = 48;

#[derive(Debug)]
pub struct ChatState {
    pub chat: Chat,
    pub wave_spinner: WaveSpinner,
}

#[derive(Debug, Clone)]
pub struct SubagentTab {
    pub label: String,
    pub agent: String,
    pub model: String,
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
    queued_messages: &[String],
    find_bar: &mut FindBar,
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
    let queue_height = if is_subagent_view {
        0
    } else {
        queued_messages_height(queued_messages)
    };
    let above_status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(0), // Reserved subagent header removed
                Constraint::Min(0),    // Chat content
                Constraint::Length(0), // Bottom padding
                Constraint::Length(queue_height),
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
                above_status_chunks[4],
                tabs,
                usage_text,
                colors,
                is_streaming,
                is_compacting,
                &mut chat_state.wave_spinner,
            );
        }
    } else {
        render_queued_messages(f, above_status_chunks[3], queued_messages, &agent, colors);

        input.render(
            f,
            above_status_chunks[4],
            &agent,
            &model,
            &provider_name,
            reasoning_effort.as_deref(),
            colors,
        );
    }

    if is_subagent_view {
        let blank = Block::default();
        f.render_widget(blank, above_status_chunks[6]);

        let status_bar = StatusBar::new(version, cwd, branch, agent, model);
        status_bar.render(f, main_chunks[1], colors);
        if find_bar.is_active() {
            find_bar.set_match_status(
                chat_state.chat.search_match_count(),
                chat_state.chat.search_active_match_index(),
            );
            find_bar.render(f, above_status_chunks[1], colors);
        }
        return;
    }

    let help_text = vec![
        Span::styled("ctrl+p", Style::default().fg(colors.info)),
        Span::raw(" commands"),
    ];
    let help_line = Line::from(help_text);
    let help_width = help_line.width() as u16;
    let available_width = above_status_chunks[5].width;

    let streaming_desired_width = if is_streaming {
        let agent_color = crate::theme::agent_color(&agent, colors);
        chat_state.wave_spinner.set_color(agent_color);
        streaming_status_desired_width(
            &chat_state.chat,
            &chat_state.wave_spinner,
            colors,
            is_compacting,
        )
    } else {
        0
    };
    let status_widths = chat_status_layout_widths(
        available_width,
        is_streaming,
        streaming_desired_width,
        usage_text,
        help_width,
    );

    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(status_widths.streaming),
            Constraint::Min(0),
            Constraint::Length(status_widths.usage),
            Constraint::Length(status_widths.help),
        ])
        .split(above_status_chunks[5]);

    if is_streaming && status_widths.streaming > 0 {
        let streaming_text = streaming_status_spans(
            &chat_state.chat,
            &chat_state.wave_spinner,
            colors,
            is_compacting,
            available_width,
        );
        let streaming_paragraph = Paragraph::new(Line::from(streaming_text));
        f.render_widget(streaming_paragraph, status_chunks[0]);
    }

    if !usage_text.is_empty() && status_widths.usage > 0 {
        let usage = Paragraph::new(Line::from(vec![Span::styled(
            usage_text,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]));
        f.render_widget(usage, status_chunks[2]);
    }

    let help = Paragraph::new(help_line).alignment(Alignment::Right);
    f.render_widget(help, status_chunks[3]);

    let blank = Block::default();
    f.render_widget(blank, above_status_chunks[6]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1], colors);

    if find_bar.is_active() {
        find_bar.set_match_status(
            chat_state.chat.search_match_count(),
            chat_state.chat.search_active_match_index(),
        );
        find_bar.render(f, above_status_chunks[1], colors);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChatStatusLayoutWidths {
    streaming: u16,
    usage: u16,
    help: u16,
}

fn chat_status_layout_widths(
    available_width: u16,
    is_streaming: bool,
    streaming_desired_width: u16,
    usage_text: &str,
    help_width: u16,
) -> ChatStatusLayoutWidths {
    let streaming = if is_streaming {
        streaming_desired_width.min(available_width)
    } else {
        0
    };
    let remaining = available_width.saturating_sub(streaming);
    let help = help_width.min(remaining);
    let usage = if !usage_text.is_empty() {
        (UnicodeWidthStr::width(usage_text) as u16 + 2).min(remaining.saturating_sub(help))
    } else {
        0
    };

    ChatStatusLayoutWidths {
        streaming,
        usage,
        help,
    }
}

fn streaming_status_desired_width(
    chat: &Chat,
    wave_spinner: &WaveSpinner,
    colors: &ThemeColors,
    is_compacting: bool,
) -> u16 {
    spans_width(&streaming_status_spans(
        chat,
        wave_spinner,
        colors,
        is_compacting,
        u16::MAX,
    ))
}

fn streaming_status_spans(
    chat: &Chat,
    wave_spinner: &WaveSpinner,
    colors: &ThemeColors,
    is_compacting: bool,
    available_width: u16,
) -> Vec<Span<'static>> {
    let spinner_width = if available_width < STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH {
        1
    } else {
        WaveSpinner::WIDTH
    };
    let mut streaming_text = wave_spinner.spans_for_width(spinner_width);
    if streaming_text.is_empty() {
        return streaming_text;
    }

    if is_compacting {
        streaming_text.push(Span::raw(" "));
        streaming_text.push(Span::styled(
            "compacting context",
            Style::default().fg(colors.info),
        ));
        return streaming_text;
    }

    let tps = chat.get_streaming_tokens_per_sec();
    if let Some(tps) = tps {
        streaming_text.push(Span::raw(" "));
        streaming_text.push(Span::styled(
            format!("{:.0}t/s", tps),
            Style::default().fg(colors.info),
        ));
    }

    if let Some(elapsed) = chat.get_streaming_elapsed_seconds() {
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

    streaming_text
}

fn subagent_streaming_status_spans(
    wave_spinner: &WaveSpinner,
    colors: &ThemeColors,
    is_compacting: bool,
    max_width: u16,
) -> Vec<Span<'static>> {
    let mut streaming_text = wave_spinner.spans_for_width(max_width);
    if streaming_text.is_empty() {
        return streaming_text;
    }

    streaming_text.push(Span::raw(" "));
    if is_compacting {
        streaming_text.push(Span::styled(
            "compacting context",
            Style::default().fg(colors.info),
        ));
    } else {
        streaming_text.push(Span::styled(
            "esc to stop",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
    }
    streaming_text
}

fn spans_width(spans: &[Span<'static>]) -> u16 {
    Line::from(spans.to_vec()).width().min(u16::MAX as usize) as u16
}

fn subagent_nav_width(content_width: u16, is_streaming: bool, nav_desired_width: u16) -> u16 {
    let streaming_priority_width = if is_streaming {
        WaveSpinner::WIDTH.min(content_width)
    } else {
        0
    };
    nav_desired_width.min(content_width.saturating_sub(streaming_priority_width))
}

pub fn queued_messages_height(messages: &[String]) -> u16 {
    if messages.is_empty() {
        return 0;
    }

    let visible_messages = messages.len().min(QUEUED_MESSAGES_MAX_VISIBLE);
    let overflow_line = usize::from(messages.len() > QUEUED_MESSAGES_MAX_VISIBLE);
    QUEUED_MESSAGES_TOP_PADDING
        + (1 + visible_messages + overflow_line) as u16
        + QUEUED_MESSAGES_BOTTOM_PADDING
}

fn render_queued_messages(
    f: &mut Frame,
    area: Rect,
    messages: &[String],
    agent: &str,
    colors: &ThemeColors,
) {
    if messages.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }

    let agent_color = crate::theme::agent_color(agent, colors);
    let border_set = border::Set {
        vertical_left: "┃",
        ..border::PLAIN
    };
    let border = Block::new()
        .borders(Borders::LEFT)
        .border_set(border_set)
        .border_style(Style::default().fg(agent_color));
    let inner_area = border.inner(area);
    let queue_bg = queued_messages_background(colors);
    let bg = Block::default().style(Style::default().bg(queue_bg));
    f.render_widget(bg, area);
    f.render_widget(border, area);

    let content_area = Rect {
        x: inner_area.x.saturating_add(2),
        y: inner_area.y.saturating_add(QUEUED_MESSAGES_TOP_PADDING),
        width: inner_area.width.saturating_sub(3),
        height: inner_area
            .height
            .saturating_sub(QUEUED_MESSAGES_TOP_PADDING + QUEUED_MESSAGES_BOTTOM_PADDING),
    };
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let mut lines = Vec::new();
    let hint = "esc to interrupt and send immediately";
    let title = "Messages to submit after next tool call";
    let title_width = 2 + UnicodeWidthStr::width(title);
    let hint_width = UnicodeWidthStr::width(hint);
    let show_hint = content_area.width as usize >= title_width + hint_width + 4;

    let mut header_spans = vec![
        Span::styled("•", Style::default().fg(agent_color)),
        Span::raw(" "),
        Span::styled(
            title,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if show_hint {
        let spacer_width = content_area
            .width
            .saturating_sub((title_width + hint_width) as u16);
        header_spans.push(Span::raw(" ".repeat(spacer_width as usize)));
        header_spans.push(Span::styled(
            hint,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
    }
    lines.push(Line::from(header_spans));

    let message_width = content_area.width.saturating_sub(4) as usize;
    for message in messages.iter().take(QUEUED_MESSAGES_MAX_VISIBLE) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("↳", Style::default().fg(colors.text_weak)),
            Span::raw(" "),
            Span::styled(
                truncate_to_width(message, message_width),
                Style::default().fg(colors.text_weak),
            ),
        ]));
    }

    if messages.len() > QUEUED_MESSAGES_MAX_VISIBLE {
        let more = messages.len() - QUEUED_MESSAGES_MAX_VISIBLE;
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("↳", Style::default().fg(colors.text_weak)),
            Span::raw(" "),
            Span::styled(
                format!("+{} more", more),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ),
        ]));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(queue_bg)),
        content_area,
    );
}

fn queued_messages_background(colors: &ThemeColors) -> Color {
    match colors.background_element {
        Color::Rgb(r, g, b) => {
            let luminance = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
            if luminance > 235.0 {
                Color::Rgb(
                    r.saturating_sub(14),
                    g.saturating_sub(14),
                    b.saturating_sub(14),
                )
            } else {
                Color::Rgb(
                    r.saturating_add(14),
                    g.saturating_add(14),
                    b.saturating_add(14),
                )
            }
        }
        _ if colors.dialog_background != colors.background_element => colors.dialog_background,
        _ => colors.background,
    }
}

fn truncate_to_width(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    if max_width <= ellipsis_width {
        return ".".repeat(max_width);
    }

    let mut rendered = String::new();
    let mut width = 0;
    let target_width = max_width - ellipsis_width;
    for ch in value.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + char_width > target_width {
            break;
        }
        width += char_width;
        rendered.push(ch);
    }
    rendered.push_str(ellipsis);
    rendered
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
    let active_agent = active_tab
        .map(|tab| tab.agent.as_str())
        .unwrap_or("Subagent");
    let active_model = active_tab.map(|tab| tab.model.as_str()).unwrap_or("");

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

    let nav_width = subagent_nav_width(content_area.width, is_streaming, nav_line.width() as u16);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(nav_width)])
        .split(content_area);

    let mut left_spans = Vec::new();
    if is_streaming {
        wave_spinner.set_color(active_color);
        left_spans.extend(subagent_streaming_status_spans(
            wave_spinner,
            colors,
            is_compacting,
            chunks[0].width,
        ));
        left_spans.push(Span::raw("  "));
    }

    left_spans.extend(agent_model_spans_with_color(
        active_agent,
        active_model,
        active_color,
        colors,
    ));
    left_spans.push(Span::raw("  "));
    left_spans.push(Span::styled(
        format!("{} ({} of {})", label, active_index + 1, total),
        Style::default()
            .fg(colors.text_weak)
            .add_modifier(Modifier::DIM),
    ));

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

    f.render_widget(Paragraph::new(Line::from(left_spans)), chunks[0]);
    f.render_widget(
        Paragraph::new(nav_line).alignment(Alignment::Right),
        chunks[1],
    );
}

fn agent_model_spans_with_color(
    agent: &str,
    model: &str,
    agent_color: Color,
    colors: &ThemeColors,
) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(
            "▣  ",
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            display_agent_name(agent),
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    if !model.trim().is_empty() {
        spans.push(Span::styled(" • ", Style::default().fg(colors.text_weak)));
        spans.push(Span::styled(
            model.trim().to_string(),
            Style::default().fg(colors.text),
        ));
    }

    spans
}

fn display_agent_name(agent: &str) -> String {
    let mut out = String::new();
    let mut word_start = true;
    for ch in agent.trim().chars() {
        if matches!(ch, '-' | '_' | ' ') {
            out.push(ch);
            word_start = true;
        } else if word_start {
            out.push(ch.to_ascii_uppercase());
            word_start = false;
        } else {
            out.push(ch);
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::{
        chat_status_layout_widths, display_agent_name, streaming_status_spans, subagent_nav_width,
        ChatStatusLayoutWidths, STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH,
    };
    use crate::theme::ThemeColors;
    use crate::ui::components::{chat::Chat, wave_spinner::WaveSpinner};
    use ratatui::style::Color;

    fn test_colors() -> ThemeColors {
        ThemeColors {
            primary: Color::Reset,
            secondary: Color::Reset,
            accent: Color::Reset,
            interactive: Color::Reset,
            background: Color::Reset,
            dialog_background: Color::Reset,
            background_element: Color::Reset,
            text: Color::Reset,
            text_weak: Color::Reset,
            text_strong: Color::Reset,
            border: Color::Reset,
            border_weak_focus: Color::Reset,
            border_focus: Color::Reset,
            border_strong_focus: Color::Reset,
            success: Color::Reset,
            warning: Color::Reset,
            error: Color::Reset,
            info: Color::Reset,
            markdown_text: Color::Reset,
            markdown_heading: Color::Reset,
            markdown_link: Color::Reset,
            markdown_link_text: Color::Reset,
            markdown_code: Color::Reset,
            markdown_block_quote: Color::Reset,
            markdown_emph: Color::Reset,
            markdown_strong: Color::Reset,
            markdown_horizontal_rule: Color::Reset,
            markdown_list_item: Color::Reset,
            markdown_list_enumeration: Color::Reset,
            markdown_image: Color::Reset,
            markdown_image_text: Color::Reset,
            markdown_code_block: Color::Reset,
            diff_add: Color::Reset,
            diff_add_bg: Color::Reset,
            diff_remove: Color::Reset,
            diff_remove_bg: Color::Reset,
            diff_gutter: Color::Reset,
        }
    }

    #[test]
    fn display_agent_name_title_cases_agent_words() {
        assert_eq!(display_agent_name("build"), "Build");
        assert_eq!(display_agent_name("vlm-agent"), "Vlm-Agent");
        assert_eq!(display_agent_name("general_reviewer"), "General_Reviewer");
    }

    #[test]
    fn status_row_reserves_streaming_before_help_or_usage() {
        assert_eq!(
            chat_status_layout_widths(4, true, 18, "100%", 13),
            ChatStatusLayoutWidths {
                streaming: 4,
                usage: 0,
                help: 0,
            }
        );
    }

    #[test]
    fn status_row_uses_remaining_width_for_help_and_usage() {
        assert_eq!(
            chat_status_layout_widths(40, true, 18, "100%", 13),
            ChatStatusLayoutWidths {
                streaming: 18,
                usage: 6,
                help: 13,
            }
        );
    }

    #[test]
    fn streaming_status_uses_long_spinner_before_first_token_at_normal_width() {
        let mut chat = Chat::new();
        chat.add_assistant_message("");
        if let Some(last) = chat.messages.last_mut() {
            last.is_complete = false;
        }
        chat.begin_streaming_turn();

        let colors = test_colors();
        let spinner = WaveSpinner::new(Color::Blue);
        let spans = streaming_status_spans(
            &chat,
            &spinner,
            &colors,
            false,
            STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH,
        );

        assert!(spans.len() > WaveSpinner::WIDTH as usize);
        assert_eq!(spans[0].content.as_ref(), "■");
    }

    #[test]
    fn streaming_status_compacts_only_below_terminal_breakpoint() {
        let chat = Chat::new();
        let colors = test_colors();
        let spinner = WaveSpinner::new(Color::Blue);
        let spans = streaming_status_spans(
            &chat,
            &spinner,
            &colors,
            false,
            STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH - 1,
        );

        assert_eq!(spans[0].content.as_ref(), "⠋");
    }

    #[test]
    fn subagent_footer_reserves_spinner_width_before_nav() {
        assert_eq!(subagent_nav_width(4, true, 24), 0);
        assert_eq!(subagent_nav_width(20, true, 24), 12);
        assert_eq!(subagent_nav_width(20, false, 24), 20);
    }
}
