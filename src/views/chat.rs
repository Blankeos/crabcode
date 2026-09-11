use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::session::types::MessageRole;
use crate::theme::ThemeColors;
use crate::ui::components::chat::Chat;
use crate::ui::components::find::FindBar;
use crate::ui::components::input::Input;
use crate::ui::components::status_bar::StatusBar;
use crate::ui::components::wave_spinner::WaveSpinner;
use crate::ui::markdown::streaming::render_markdown;
use crate::ui::selection::non_selectable_style;

pub const SUBAGENT_FOOTER_HEIGHT: u16 = 3;
const QUEUED_MESSAGES_MAX_VISIBLE: usize = 3;
const QUEUED_MESSAGES_TOP_PADDING: u16 = 1;
const QUEUED_MESSAGES_BOTTOM_PADDING: u16 = 1;
const STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH: u16 = 64;
const SUBAGENT_FOOTER_NAV_GAP: &str = "   ";
/// Hover target for the pending (queued) header actions.
/// Tracks mouse hover for a foreground-only highlight with no background.
/// Follows the `Input`/`Chat` hover convention: `Option` state set on
/// `MouseEventKind::Moved`, cleared off-target/hidden, rendered fg-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingHover {
    Edit,
    Send,
}
/// Minimum transcript rows to keep visible when clamping the input height
/// on short terminals. The input shrinks instead of forcing its full
/// 9-line height and starving the chat view.
const MIN_CHAT_VIEWPORT_HEIGHT: u16 = 5;

/// Desired input height clamped to what fits in `total_height` alongside
/// the queue / btw panels, help line and status rows.
pub fn chat_input_height(
    input: &Input,
    width: u16,
    total_height: u16,
    queue_height: u16,
    btw_height: u16,
    help_height: u16,
) -> u16 {
    // Outer status bar (1) + inner status row (1).
    let reserved = queue_height + btw_height + help_height + 1 + 1 + MIN_CHAT_VIEWPORT_HEIGHT;
    let max_allowed = total_height.saturating_sub(reserved);
    input.get_height_for_layout(width, max_allowed)
}

/// Paint only the animated loading cells into an already rendered frame.
/// Callers must start from the last complete buffer; this deliberately skips
/// transcript layout and every other chat widget.
pub fn render_subagent_spinner_only(
    buffer: &mut Buffer,
    wave_spinner: &mut WaveSpinner,
    agent_color: Color,
) -> bool {
    let size = buffer.area;
    if size.width == 0 || size.height < SUBAGENT_FOOTER_HEIGHT + 2 {
        return false;
    }

    let main_height = size.height.saturating_sub(1);
    let footer = Rect::new(
        size.x,
        size.y
            + main_height
                .saturating_sub(SUBAGENT_FOOTER_HEIGHT)
                .saturating_sub(1),
        size.width,
        SUBAGENT_FOOTER_HEIGHT,
    );
    let inner = Rect::new(
        footer.x.saturating_add(1),
        footer.y,
        footer.width.saturating_sub(1),
        footer.height,
    );
    let content = centered_subagent_footer_content(inner);
    if content.width == 0 || content.height == 0 {
        return false;
    }

    let spinner_width = if footer.width < STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH {
        1
    } else {
        WaveSpinner::WIDTH.min(content.width)
    };
    wave_spinner.set_color(agent_color);
    let spinner = Line::from(wave_spinner.spans_for_width(spinner_width));
    Paragraph::new(spinner).render(Rect::new(content.x, content.y, spinner_width, 1), buffer);
    true
}

#[derive(Debug)]
pub struct ChatState {
    pub chat: Chat,
    pub wave_spinner: WaveSpinner,
    pub compact_mode: bool,
    /// Index of the most recent user message that has scrolled past the top
    /// of the viewport, shown as a sticky message in compact mode.
    pub sticky_message_index: Option<usize>,
    /// Last-rendered chat content rect (excludes compact chrome). Used for mouse hit-testing.
    pub last_chat_area: Option<Rect>,
    /// Clickable sticky user-message bar from the last render: (rect, message_index).
    pub sticky_click_target: Option<(Rect, usize)>,
}

#[derive(Debug, Clone)]
pub struct SubagentTab {
    pub session_id: String,
    pub label: String,
    pub agent: String,
    pub model: String,
    pub active: bool,
    pub running: bool,
    pub color: ratatui::style::Color,
}

#[derive(Debug, Clone)]
pub struct SubagentTabs {
    pub root_session_id: String,
    pub is_child_session: bool,
    pub tabs: Vec<SubagentTab>,
}

impl ChatState {
    pub fn new(chat: Chat, agent_color: ratatui::style::Color, compact_mode: bool) -> Self {
        Self {
            chat,
            wave_spinner: WaveSpinner::with_speed(agent_color, 40),
            compact_mode,
            sticky_message_index: None,
            last_chat_area: None,
            sticky_click_target: None,
        }
    }
}

pub fn init_chat(chat: Chat, agent: &str, colors: &ThemeColors, compact_mode: bool) -> ChatState {
    let agent_color = crate::theme::agent_color(agent, colors);
    ChatState::new(chat, agent_color, compact_mode)
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
    model_display: String,
    provider_name: String,
    reasoning_effort: Option<String>,
    reasoning_effort_explicit: bool,
    colors: &ThemeColors,
    is_streaming: bool,
    is_compacting: bool,
    esc_cancel_primed: bool,
    retry_status: Option<&crate::app::StreamingRetryStatus>,
    usage_text: &str,
    subagent_tabs: Option<SubagentTabs>,
    queued_messages: &[String],
    queued_has_user_messages: bool,
    btw_entry: Option<&crate::app::BtwEntry>,
    btw_scroll: usize,
    btw_panel_area: &mut Option<Rect>,
    find_bar: &mut FindBar,
    show_terminal_cursor: bool,
    session_title: Option<&str>,
    running_jobs: usize,
    jobs_chip_area: &mut Option<Rect>,
    queued_edit_area: &mut Option<Rect>,
    queued_send_area: &mut Option<Rect>,
    queued_hover: Option<PendingHover>,
) {
    *jobs_chip_area = None;
    *queued_edit_area = None;
    *queued_send_area = None;
    let size = f.area();
    let is_subagent_view = subagent_tabs
        .as_ref()
        .is_some_and(|tabs| tabs.is_child_session);

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let help_height = if is_subagent_view { 0 } else { 1 };
    let queue_height = if is_subagent_view {
        0
    } else {
        queued_messages_height(queued_messages)
    };
    let btw_height = if is_subagent_view {
        0
    } else {
        btw_panel_height(btw_entry, size.width, colors)
    };
    let input_height = if is_subagent_view {
        SUBAGENT_FOOTER_HEIGHT
    } else {
        chat_input_height(
            input,
            size.width,
            size.height,
            queue_height,
            btw_height,
            help_height,
        )
    };
    let above_status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(0), // Reserved subagent header removed
                Constraint::Min(0),    // Chat content
                Constraint::Length(0), // Bottom padding
                Constraint::Length(queue_height + btw_height),
                Constraint::Length(input_height),
                Constraint::Length(help_height),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    // Compact mode: sticky header (session title) + sticky scrolled-past user message.
    //
    // Layout: the sticky bar is an *overlay* painted on top of the transcript,
    // not a layout row. Showing/hiding sticky must not change transcript
    // viewport height or scroll extent (header is always 3 rows; chat fills the
    // rest of the content area).
    //
    // Sticky visibility is driven by transcript line offsets vs scroll_offset.
    // Eligibility: a prior user once its body has left the top (`body_end <= S`).
    // Hide: when the next user message enters the sticky overlay's covered top
    // region (`S + sticky_height > next_user_start`). Overlay height defines that
    // visual coverage only — it never shrinks chat_area / scroll extent.
    // Assistant/tool blocks between users do not suppress sticky.
    let (chat_area, sticky_overlay) = if chat_state.compact_mode {
        // Fixed layout first so chat_area is independent of sticky overlay height.
        let (header_area, chat_area) = compact_transcript_layout(above_status_chunks[1]);

        // Match Chat::render content width (area.width - 2 scrollbar gutter).
        // Refresh layout cache before sticky math so show/hide uses this frame's
        // positions, not the previous frame's (stale after resize / new messages).
        let content_max_width = (chat_area.width.saturating_sub(2) as usize).max(1);
        chat_state
            .chat
            .ensure_render_cache(content_max_width, &model, colors);

        // Resolve stick-to-bottom MAX so sticky math uses a real line offset.
        // Raw `scroll_offset == usize::MAX` would make every body_end <= S and
        // incorrectly pin the latest still-visible user message.
        let scroll_offset = chat_state.chat.resolved_scroll_offset();
        // One start line per transcript message / rendered block (groups share a start).
        let rendered_message_starts = &chat_state.chat.message_line_positions;
        let content_height = chat_state.chat.content_height;

        let msg_end_line = |idx: usize| -> usize {
            (idx + 1..rendered_message_starts.len())
                .find_map(|i| rendered_message_starts.get(i).copied())
                .unwrap_or(content_height)
        };

        // (message_index, start_line, body_end) for every non-compaction user message.
        // body_end excludes the trailing inter-message blank so sticky appears as
        // soon as the real message body has fully left the viewport top.
        let user_messages: Vec<(usize, usize, usize)> = chat_state
            .chat
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.role == MessageRole::User
                    && !crate::session::compaction::is_compaction_display_item(m)
            })
            .filter_map(|(i, _)| {
                rendered_message_starts.get(i).map(|&start| {
                    let end = msg_end_line(i);
                    (i, start, user_message_body_end(end))
                })
            })
            .collect();

        let display_sticky = resolve_sticky_display(
            &user_messages,
            scroll_offset,
            chat_state.sticky_message_index,
        );

        // Update memory: remember last displayed sticky; clear only when scrolled
        // above the first user message (nothing left to be sticky about).
        if let Some(idx) = display_sticky {
            chat_state.sticky_message_index = Some(idx);
        } else {
            let first_start = user_messages.first().map(|(_, s, _)| *s).unwrap_or(0);
            if scroll_offset <= first_start {
                chat_state.sticky_message_index = None;
            }
            // else keep memory for hysteresis while in dead/transition zones
        }

        let sticky_height: u16 = if let Some(idx) = display_sticky {
            let msg_start = rendered_message_starts.get(idx).copied().unwrap_or(0);
            let msg_end = msg_end_line(idx);
            sticky_overlay_height_for_span(msg_start, user_message_body_end(msg_end)) as u16
        } else {
            0
        };

        // Compact header: title on middle row (accent + bold). Skip empty titles
        // but keep the fixed 3-row slot so layout does not jump.
        if let Some(title) = session_title.filter(|t| !t.is_empty()) {
            let header_inner = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ]
                    .as_ref(),
                )
                .split(header_area);
            f.render_widget(
                Paragraph::new(title).style(
                    Style::default()
                        .fg(colors.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                header_inner[1],
            );
        }

        chat_state.last_chat_area = Some(chat_area);
        // Clear previous sticky target; set only when an overlay bar is drawn.
        chat_state.sticky_click_target = None;

        let sticky_overlay = display_sticky
            .and_then(|idx| sticky_overlay_rect(chat_area, sticky_height).map(|rect| (rect, idx)));

        (chat_area, sticky_overlay)
    } else {
        // Leaving compact mode: clear sticky state so re-enabling starts clean.
        chat_state.sticky_message_index = None;
        chat_state.sticky_click_target = None;
        chat_state.last_chat_area = Some(above_status_chunks[1]);
        (above_status_chunks[1], None)
    };

    // Transcript first so the sticky overlay (if any) paints on top of it.
    chat_state.chat.render(f, chat_area, &agent, &model, colors);

    // Paint sticky as an overlay over the top of the transcript. This keeps
    // transcript viewport height / scroll extent independent of sticky state.
    if let Some((sticky_rect, idx)) = sticky_overlay {
        chat_state.sticky_click_target = Some((sticky_rect, idx));

        // Content panel matches Chat content area (full width minus scrollbar gutter).
        let content_width = sticky_rect.width.saturating_sub(2);
        let content_rect = Rect {
            x: sticky_rect.x,
            y: sticky_rect.y,
            width: content_width,
            height: sticky_rect.height,
        };
        let max_width = content_width as usize;
        let sticky_height = sticky_rect.height;
        let sticky_msg = chat_state.chat.messages.get(idx);

        let border_color = crate::theme::agent_mode_color(
            sticky_msg.and_then(|m| m.agent_mode.as_deref()),
            colors,
        );
        let bg = colors.background_element;
        let border_style = non_selectable_style(Style::default().fg(border_color));
        let pad_style = non_selectable_style(Style::default().bg(bg));
        // ▴ affordance: weak text so it reads as a clickable cue, not content.
        let arrow_style = non_selectable_style(Style::default().fg(colors.text_weak).bg(bg));

        let horizontal_padding = 2usize;

        let padding_line = || {
            let mut line = Line::from(vec![
                Span::styled("▌", border_style),
                Span::styled(" ".repeat(max_width.saturating_sub(1)), pad_style),
            ]);
            line.style = Style::default().bg(bg);
            line
        };

        // Bottom padding with a horizontally-centered ▴ click affordance.
        let bottom_padding_line = || {
            // Layout: "▌" + spaces + "▴" + spaces, total width = max_width.
            let body_width = max_width.saturating_sub(1); // after border
            let arrow = "▴";
            let arrow_w = 1usize;
            let left = body_width.saturating_sub(arrow_w) / 2;
            let right = body_width.saturating_sub(left + arrow_w);
            let mut line = Line::from(vec![
                Span::styled("▌", border_style),
                Span::styled(" ".repeat(left), pad_style),
                Span::styled(arrow, arrow_style),
                Span::styled(" ".repeat(right), pad_style),
            ]);
            line.style = Style::default().bg(bg);
            line
        };

        // Number of content rows = sticky height minus top/bottom padding.
        let content_rows = sticky_height.saturating_sub(2) as usize;
        let mut sticky_lines: Vec<Line> = Vec::with_capacity(sticky_height as usize);
        sticky_lines.push(padding_line());

        // Content rows: same wrap width as the live user bubble so sticky text
        // matches the faded-out original (image placeholders, agent mentions).
        let content_lines = chat_state
            .chat
            .format_user_message_content_lines(idx, max_width, colors);
        let mut content_iter = content_lines.into_iter();
        for _ in 0..content_rows {
            if let Some(content_line) = content_iter.next() {
                let line_width = content_line.width();
                let trailing_padding =
                    " ".repeat(max_width.saturating_sub(1 + horizontal_padding + line_width));
                let mut spans = Vec::with_capacity(content_line.spans.len() + 3);
                spans.push(Span::styled("▌", border_style));
                spans.push(Span::styled(" ".repeat(horizontal_padding), pad_style));
                spans.extend(content_line.spans);
                spans.push(Span::styled(trailing_padding, pad_style));
                let mut panel_line = Line::from(spans);
                panel_line.style = Style::default().bg(bg);
                sticky_lines.push(panel_line);
            } else {
                // Message has fewer lines than the sticky can show.
                sticky_lines.push(padding_line());
            }
        }

        sticky_lines.push(bottom_padding_line());

        // Paragraph patches styles onto existing cells and only rewrites
        // grapheme-covered cells. Clear first so bold/fg/bg from the
        // underlying transcript cannot leak into the sticky rectangle.
        // Paint only the content strip so the scrollbar gutter stays free.
        paint_sticky_overlay(
            f.buffer_mut(),
            content_rect,
            sticky_lines,
            colors.background_element,
        );
        // Chat paints its scrollbar before this overlay. Re-paint so the thumb
        // stays above the sticky bar. Overlay geometry / click target are
        // unchanged — only paint order is adjusted.
        chat_state.chat.render_scrollbar_over(
            f,
            chat_area,
            colors.background_element,
            colors.text_weak,
        );
    }

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
                esc_cancel_primed,
                retry_status,
                &mut chat_state.wave_spinner,
            );
        }
    } else {
        let above_input = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(btw_height),
                    Constraint::Length(queue_height),
                ]
                .as_ref(),
            )
            .split(above_status_chunks[3]);
        render_btw_panel(
            f,
            above_input[0],
            btw_entry,
            btw_scroll,
            btw_panel_area,
            colors,
        );
        render_queued_messages(
            f,
            above_input[1],
            queued_messages,
            queued_has_user_messages,
            &agent,
            colors,
            esc_cancel_primed,
            queued_edit_area,
            queued_send_area,
            queued_hover,
        );

        input.render(
            f,
            above_status_chunks[4],
            &agent,
            &model_display,
            &provider_name,
            reasoning_effort.as_deref(),
            reasoning_effort_explicit,
            colors,
            show_terminal_cursor,
        );
    }

    if is_subagent_view {
        f.render_widget(
            Block::default().style(Style::default().bg(colors.background)),
            above_status_chunks[6],
        );

        let status_bar = StatusBar::new(version, cwd, branch, agent, model_display);
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

    let mut help_text = Vec::new();
    if running_jobs > 0 {
        let chip_label = if running_jobs == 1 {
            "● 1 job".to_string()
        } else {
            format!("● {running_jobs} jobs")
        };
        help_text.push(Span::styled(
            chip_label,
            Style::default()
                .fg(colors.warning)
                .add_modifier(Modifier::BOLD),
        ));
        help_text.push(Span::raw("  "));
    }
    help_text.push(Span::styled("ctrl+p", Style::default().fg(colors.info)));
    help_text.push(Span::raw(" commands"));
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
            esc_cancel_primed,
            retry_status,
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
            esc_cancel_primed,
            retry_status,
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

    let help = Paragraph::new(help_line.clone()).alignment(Alignment::Right);
    f.render_widget(help, status_chunks[3]);
    if running_jobs > 0 {
        // Right-aligned chip sits at the start of the help line content.
        let chip_label_width = if running_jobs == 1 {
            "● 1 job".chars().count() as u16
        } else {
            format!("● {running_jobs} jobs").chars().count() as u16
        };
        let area = status_chunks[3];
        let chip_x = area.x.saturating_add(area.width.saturating_sub(help_width));
        *jobs_chip_area = Some(Rect {
            x: chip_x,
            y: area.y,
            width: chip_label_width.min(area.width),
            height: 1,
        });
    }

    f.render_widget(
        Block::default().style(Style::default().bg(colors.background)),
        above_status_chunks[6],
    );

    let status_bar = StatusBar::new(version, cwd, branch, agent, model_display);
    status_bar.render(f, main_chunks[1], colors);

    if find_bar.is_active() {
        find_bar.set_match_status(
            chat_state.chat.search_match_count(),
            chat_state.chat.search_active_match_index(),
        );
        find_bar.render(f, above_status_chunks[1], colors);
    }
}

/// Fixed compact-mode layout: 3-row header + full remaining height for the
/// transcript. Sticky is an overlay and does not participate in this split.
fn compact_transcript_layout(area: Rect) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    (chunks[0], chunks[1])
}

/// Sticky overlay rect at the top of the transcript area, clamped so it never
/// exceeds the transcript height.
fn sticky_overlay_rect(chat_area: Rect, sticky_height: u16) -> Option<Rect> {
    if sticky_height == 0 || chat_area.height == 0 || chat_area.width == 0 {
        return None;
    }
    let height = sticky_height.min(chat_area.height);
    Some(Rect {
        x: chat_area.x,
        y: chat_area.y,
        width: chat_area.width,
        height,
    })
}

/// Extra clearance (in rows) required when scrolling up before the previous
/// sticky is allowed to replace the remembered one.
const STICKY_UP_HYSTERESIS: usize = 5;

/// User messages are laid out as: top pad + content + bottom pad + trailing blank.
/// The trailing blank is inter-message spacing, not part of the message body.
/// Sticky must appear as soon as the body has fully left the viewport top —
/// one scroll row earlier than treating `msg_end` (which includes the blank).
fn user_message_body_end(msg_end_including_trailing_blank: usize) -> usize {
    msg_end_including_trailing_blank.saturating_sub(1)
}

/// Sticky overlay row count for a user message whose body occupies
/// `[msg_start, body_end)` in the transcript.
///
/// User messages render as top pad + content + bottom pad (+ trailing blank
/// excluded from body_end). Overlay height clamps to 3..=5 rows and is used
/// only for the visual covered region / hide boundary — never for scroll extent.
fn sticky_overlay_height_for_span(msg_start: usize, body_end: usize) -> usize {
    let msg_body_lines = body_end.saturating_sub(msg_start);
    msg_body_lines.min(5).max(3)
}

/// Start line of the next user message after `message_index`, if any.
///
/// The sticky overlay must not cover the next *user* message in the viewport.
/// Intermediate assistant/tool blocks do not suppress sticky — otherwise a
/// normal user→assistant transcript would hide sticky as soon as the prior
/// user's body leaves the top.
fn next_user_start_after(
    user_messages: &[(usize, usize, usize)],
    message_index: usize,
) -> Option<usize> {
    user_messages
        .iter()
        .find(|(idx, _, _)| *idx > message_index)
        .map(|(_, start, _)| *start)
}

/// Natural sticky candidate while scrolling down.
///
/// `user_messages` entries are `(message_index, start_line, body_end)` sorted in
/// transcript order.
///
/// Show: last user message whose body is fully above the viewport (`body_end <= S`).
/// Hide: when the next user message's first row enters the sticky overlay's
/// half-open top coverage `[S, S + sticky_height)` — i.e.
/// `S + sticky_height > next_user_start` (still visible when equal).
/// Sticky height defines that covered region only; it does not change scroll
/// extent. Intermediate assistant/tool rows do not hide sticky.
fn natural_sticky_index(
    user_messages: &[(usize, usize, usize)],
    scroll_offset: usize,
) -> Option<usize> {
    let prev = user_messages
        .iter()
        .rev()
        .find(|(_, _, body_end)| *body_end <= scroll_offset)
        .copied();
    match prev {
        Some((idx, start, body_end)) => {
            let sticky_height = sticky_overlay_height_for_span(start, body_end);
            let next_start = next_user_start_after(user_messages, idx);
            match next_start {
                // Next user message's first row is inside the sticky-covered top
                // region; hide immediately. Equal bottom edge keeps sticky visible.
                Some(ns) if scroll_offset.saturating_add(sticky_height) > ns => None,
                _ => Some(idx),
            }
        }
        None => None,
    }
}

/// Resolve which sticky (if any) to display, applying scroll-up hysteresis via
/// the remembered sticky index.
///
/// `user_messages` entries are `(message_index, start_line, body_end)`.
/// When natural selection is `None`, memory is never resurrected.
fn resolve_sticky_display(
    user_messages: &[(usize, usize, usize)],
    scroll_offset: usize,
    memory: Option<usize>,
) -> Option<usize> {
    let natural = natural_sticky_index(user_messages, scroll_offset);

    match (memory, natural) {
        // No memory yet — follow natural.
        (None, nat) => nat,

        // Natural is None — dead zone, next user under sticky, or body still visible.
        // Never re-show / resurrect memory once natural has cleared.
        (Some(_memory), None) => None,

        // Natural caught up to or passed memory (scroll down / same) — follow natural.
        (Some(mem), Some(nat)) if nat >= mem => Some(nat),

        // Natural wants an older message (scroll up) — require clearance above `memory`.
        (Some(mem), Some(nat)) => {
            let memory_entry = user_messages.iter().find(|(i, _, _)| *i == mem);
            let (memory_start, memory_body_end) = match memory_entry {
                Some((_, start, body_end)) => (*start, *body_end),
                None => return Some(nat),
            };
            // Clearance uses the same one-row hide offset as the natural hide
            // boundary (+1) so directional hysteresis stays consistent.
            if scroll_offset
                .saturating_add(1)
                .saturating_add(STICKY_UP_HYSTERESIS)
                <= memory_start
            {
                // Enough space above the remembered message → show older sticky.
                Some(nat)
            } else if memory_body_end <= scroll_offset {
                // Memory is still fully above viewport → keep it sticky.
                Some(mem)
            } else {
                // Memory has re-entered the viewport — no sticky.
                None
            }
        }
    }
}

/// Clear the sticky rectangle, then paint the sticky Paragraph so styles from
/// the underlying transcript cannot leak into unwritten sticky cells.
fn paint_sticky_overlay(
    buf: &mut Buffer,
    sticky_area: Rect,
    sticky_lines: Vec<Line<'static>>,
    bg: Color,
) {
    Clear.render(sticky_area, buf);
    Paragraph::new(sticky_lines)
        .style(Style::default().bg(bg))
        .render(sticky_area, buf);
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

/// OpenCode-style interrupt hint: `esc interrupt` → `esc again to interrupt`.
fn cancel_hint(esc_cancel_primed: bool) -> &'static str {
    if esc_cancel_primed {
        "esc again to interrupt"
    } else {
        "esc interrupt"
    }
}

/// Armed interrupt uses warning so it stands out from the loading spinner (agent/primary)
/// and streaming metrics (`info`).
fn cancel_hint_style(colors: &ThemeColors, esc_cancel_primed: bool) -> Style {
    if esc_cancel_primed {
        Style::default().fg(colors.warning)
    } else {
        Style::default()
            .fg(colors.text_weak)
            .add_modifier(Modifier::DIM)
    }
}

fn streaming_status_desired_width(
    chat: &Chat,
    wave_spinner: &WaveSpinner,
    colors: &ThemeColors,
    is_compacting: bool,
    esc_cancel_primed: bool,
    retry_status: Option<&crate::app::StreamingRetryStatus>,
) -> u16 {
    spans_width(&streaming_status_spans(
        chat,
        wave_spinner,
        colors,
        is_compacting,
        esc_cancel_primed,
        retry_status,
        u16::MAX,
    ))
}

fn streaming_status_spans(
    chat: &Chat,
    wave_spinner: &WaveSpinner,
    colors: &ThemeColors,
    is_compacting: bool,
    esc_cancel_primed: bool,
    retry_status: Option<&crate::app::StreamingRetryStatus>,
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

    if let Some(retry) = retry_status {
        let seconds = retry_seconds_remaining(retry.next_epoch_ms);
        let retrying = if seconds > 0 {
            format!("retrying in {}s", seconds)
        } else {
            "retrying now".to_string()
        };
        let attempt = format!("attempt #{}", retry.attempt);
        let controls = cancel_hint(esc_cancel_primed);
        let fixed_width = spans_width(&streaming_text)
            .saturating_add(1)
            .saturating_add(3)
            .saturating_add(UnicodeWidthStr::width(retrying.as_str()) as u16)
            .saturating_add(3)
            .saturating_add(UnicodeWidthStr::width(attempt.as_str()) as u16)
            .saturating_add(2)
            .saturating_add(UnicodeWidthStr::width(controls) as u16);
        let message = if available_width == u16::MAX {
            retry.message.clone()
        } else {
            truncate_to_width(
                &retry.message,
                available_width.saturating_sub(fixed_width) as usize,
            )
        };
        streaming_text.push(Span::raw(" "));
        if !message.is_empty() {
            streaming_text.push(Span::styled(message, Style::default().fg(colors.warning)));
            streaming_text.push(Span::raw(" · "));
        }
        streaming_text.push(Span::styled(retrying, Style::default().fg(colors.info)));
        streaming_text.push(Span::raw(" · "));
        streaming_text.push(Span::styled(
            attempt,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
        streaming_text.push(Span::raw("  "));
        streaming_text.push(Span::styled(
            controls,
            cancel_hint_style(colors, esc_cancel_primed),
        ));
        return streaming_text;
    }

    if is_compacting {
        streaming_text.push(Span::raw(" "));
        streaming_text.push(Span::styled(
            "compacting context",
            Style::default().fg(colors.info),
        ));
        streaming_text.push(Span::raw("  "));
        streaming_text.push(Span::styled(
            cancel_hint(esc_cancel_primed),
            cancel_hint_style(colors, esc_cancel_primed),
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
        cancel_hint(esc_cancel_primed),
        cancel_hint_style(colors, esc_cancel_primed),
    ));

    streaming_text
}

fn subagent_streaming_status_spans(
    wave_spinner: &WaveSpinner,
    colors: &ThemeColors,
    is_compacting: bool,
    esc_cancel_primed: bool,
    retry_status: Option<&crate::app::StreamingRetryStatus>,
    available_width: u16,
    max_width: u16,
) -> Vec<Span<'static>> {
    let spinner_width = if available_width < STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH {
        1
    } else {
        WaveSpinner::WIDTH
    };
    let mut streaming_text = wave_spinner.spans_for_width(spinner_width.min(max_width));
    if streaming_text.is_empty() {
        return streaming_text;
    }

    streaming_text.push(Span::raw(" "));
    if is_compacting {
        streaming_text.push(Span::styled(
            "compacting context",
            Style::default().fg(colors.info),
        ));
        streaming_text.push(Span::raw("  "));
        streaming_text.push(Span::styled(
            cancel_hint(esc_cancel_primed),
            cancel_hint_style(colors, esc_cancel_primed),
        ));
    } else if let Some(retry) = retry_status {
        let seconds = retry_seconds_remaining(retry.next_epoch_ms);
        let retrying = if seconds > 0 {
            format!("retrying in {}s", seconds)
        } else {
            "retrying now".to_string()
        };
        let attempt = format!("attempt #{}", retry.attempt);
        let fixed_width = spans_width(&streaming_text)
            .saturating_add(1)
            .saturating_add(UnicodeWidthStr::width(retrying.as_str()) as u16)
            .saturating_add(3)
            .saturating_add(UnicodeWidthStr::width(attempt.as_str()) as u16);
        let message = truncate_to_width(
            &retry.message,
            max_width.saturating_sub(fixed_width).min(48) as usize,
        );
        if !message.is_empty() {
            streaming_text.push(Span::styled(message, Style::default().fg(colors.warning)));
            streaming_text.push(Span::raw(" · "));
        }
        streaming_text.push(Span::styled(
            format!("{} · {}", retrying, attempt),
            Style::default().fg(colors.warning),
        ));
    } else {
        streaming_text.push(Span::styled(
            cancel_hint(esc_cancel_primed),
            cancel_hint_style(colors, esc_cancel_primed),
        ));
    }
    streaming_text
}

fn spans_width(spans: &[Span<'static>]) -> u16 {
    Line::from(spans.to_vec()).width().min(u16::MAX as usize) as u16
}

fn retry_seconds_remaining(next_epoch_ms: u64) -> u64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    next_epoch_ms.saturating_sub(now_ms).div_ceil(1000)
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
    has_pending_user_messages: bool,
    agent: &str,
    colors: &ThemeColors,
    esc_cancel_primed: bool,
    edit_area: &mut Option<Rect>,
    send_area: &mut Option<Rect>,
    queued_hover: Option<PendingHover>,
) {
    *edit_area = None;
    *send_area = None;
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
    let hint_prefix = if esc_cancel_primed {
        "esc again to "
    } else {
        "esc "
    };
    let send_label = "send now";
    let send_width = UnicodeWidthStr::width(hint_prefix) + UnicodeWidthStr::width(send_label);
    let title = "Queued message";
    let title_width = 2 + UnicodeWidthStr::width(title);
    let content_width = content_area.width as usize;

    // Header actions (top-right, right-aligned): quiet clickable `edit` plus
    // the accurate Esc send-now hint. Desired order:
    // `edit   esc send now` (armed: `esc again to send now`).
    // Keyboard binding (Ctrl+X r) stays active but has no visible hint.
    // No extra bottom action row is reserved. Narrow-safe: edit + send-now,
    // then send-only, then title-only. Hitboxes require an
    // actual pending user message — a compact-only queue (e.g. ["/compact"])
    // keeps the queued line (and the send-now hint text when it fits) but offers
    // no edit/send targets. Widths use Unicode display width so hitboxes
    // align with the right-aligned header labels on narrow terminals.
    const HEADER_GAP: usize = 3;
    const HEADER_MIN_SPACER: usize = 2;
    let edit_label = "edit";
    let edit_width = UnicodeWidthStr::width(edit_label);
    let right_edit = edit_width + HEADER_GAP + send_width;

    #[derive(Debug, PartialEq, Eq)]
    enum PendingHeaderMode {
        Edit,
        SendOnly,
        TitleOnly,
    }
    let mode = if has_pending_user_messages {
        if content_width >= title_width + right_edit + HEADER_MIN_SPACER {
            PendingHeaderMode::Edit
        } else if content_width >= title_width + send_width + HEADER_MIN_SPACER {
            PendingHeaderMode::SendOnly
        } else {
            PendingHeaderMode::TitleOnly
        }
    } else if content_width >= title_width + send_width + HEADER_MIN_SPACER {
        PendingHeaderMode::SendOnly
    } else {
        PendingHeaderMode::TitleOnly
    };
    let right_width = match mode {
        PendingHeaderMode::Edit => right_edit,
        PendingHeaderMode::SendOnly => send_width,
        PendingHeaderMode::TitleOnly => 0,
    };
    let show_edit = matches!(mode, PendingHeaderMode::Edit);

    // Foreground-only hover: idle stays muted (text_weak), hovered turns
    // bright (text) with no background change. Mirrors the Input/Chat image
    // hover convention (fg change, bg untouched). Effective hover requires a
    // clickable target so stale hover never highlights hidden text.
    let edit_hovered =
        matches!(queued_hover, Some(PendingHover::Edit)) && has_pending_user_messages && show_edit;
    let send_hovered = matches!(queued_hover, Some(PendingHover::Send))
        && has_pending_user_messages
        && right_width > 0
        && (show_edit || matches!(mode, PendingHeaderMode::SendOnly));
    let edit_fg = if edit_hovered {
        colors.text
    } else {
        colors.text_weak
    };
    let send_prefix_fg = if send_hovered {
        colors.text
    } else {
        colors.text_weak
    };
    let send_label_fg = if send_hovered {
        colors.text
    } else {
        colors.text_weak
    };

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
    if right_width > 0 {
        let spacer_width = content_width.saturating_sub(title_width + right_width);
        header_spans.push(Span::raw(" ".repeat(spacer_width)));
        if show_edit {
            header_spans.push(Span::styled(edit_label, Style::default().fg(edit_fg)));
            header_spans.push(Span::raw(" ".repeat(HEADER_GAP)));
        }
        header_spans.push(Span::styled(
            hint_prefix,
            Style::default()
                .fg(send_prefix_fg)
                .add_modifier(Modifier::DIM),
        ));
        header_spans.push(Span::styled(
            send_label,
            Style::default()
                .fg(send_label_fg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(header_spans));

    // Header hitboxes clamped to the content area so narrow terminals never
    // produce out-of-bounds rects. `edit` covers the quiet label; send covers
    // prefix + label and invokes the same handler as the
    // former Send action (steering path). Compact-only and too-narrow headers
    // leave both as None (reset at entry).
    if has_pending_user_messages && right_width > 0 {
        let header_y = content_area.y;
        let content_right = content_area.x.saturating_add(content_area.width);
        if show_edit {
            let edit_w = edit_width as u16;
            let edit_x = content_right.saturating_sub(right_width as u16);
            let edit_w = edit_w.min(content_right.saturating_sub(edit_x));
            if edit_w > 0 {
                *edit_area = Some(Rect::new(edit_x, header_y, edit_w, 1));
            }
            let send_x = edit_x.saturating_add(edit_w.saturating_add(HEADER_GAP as u16));
            if send_x < content_right {
                let send_w = (send_width as u16).min(content_right.saturating_sub(send_x));
                if send_w > 0 {
                    *send_area = Some(Rect::new(send_x, header_y, send_w, 1));
                }
            }
        } else {
            let send_x = content_right.saturating_sub(send_width as u16);
            if send_x >= content_area.x && send_x < content_right {
                let send_w = (send_width as u16).min(content_right.saturating_sub(send_x));
                if send_w > 0 {
                    *send_area = Some(Rect::new(send_x, header_y, send_w, 1));
                }
            }
        }
    }

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

pub(crate) const BTW_MAX_BODY_LINES: usize = 10;
const BTW_TOP_PADDING: u16 = 1;
const BTW_BOTTOM_PADDING: u16 = 1;
/// Lines scrolled per mouse-wheel notch inside the `/btw` panel.
const BTW_SCROLL_STEP: usize = 3;

/// Text width available for the `/btw` answer body: panel width minus the
/// left border (1), content inset (3) and the 2-space body indent.
///
/// Keep in sync with the layout in [`render_btw_panel`]: the content area
/// there is `inner(2 + 1 wide)`, and the body is laid out at
/// `content_area.width - 2`.
pub(crate) fn btw_body_width(panel_width: u16) -> usize {
    content_width_for_area_width(panel_width)
}

fn content_width_for_area_width(panel_width: u16) -> usize {
    (panel_width as usize).saturating_sub(1 + 3 + 2).max(10)
}

/// Body viewport (visible answer lines) for a panel of the given height.
/// Shared by the mouse-scroll clamp ([`App::btw_scroll_max`]) and render so
/// the scroll window always matches the space that was actually allocated —
/// critical when the terminal is too short for the full 10-line cap.
pub(crate) fn btw_body_viewport(panel_height: u16) -> usize {
    (panel_height as usize)
        .saturating_sub((BTW_TOP_PADDING + BTW_BOTTOM_PADDING + 1) as usize)
        .max(1)
}

/// Rendered `/btw` answer body: full markdown like the main transcript.
/// Pending and error states stay plain styled text.
pub(crate) fn btw_body_lines(
    entry: &crate::app::BtwEntry,
    content_width: usize,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    if let Some(answer) = entry.answer.as_deref() {
        let mut lines = render_markdown(answer, content_width.max(10), colors);
        if lines.is_empty() {
            lines.push(Line::from(""));
        }
        return lines;
    }
    if let Some(error) = entry.error.as_deref() {
        let style = Style::default().fg(colors.error);
        let mut lines = Vec::new();
        for source_line in format!("error: {error}").lines() {
            for wrapped in wrap_plain_text_line(source_line, content_width.max(10)) {
                lines.push(Line::styled(wrapped, style));
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(""));
        }
        return lines;
    }
    vec![Line::styled(
        "thinking…",
        Style::default()
            .fg(colors.text_weak)
            .add_modifier(Modifier::DIM),
    )]
}

/// Height of the `/btw` side-answer panel above the input (0 when absent).
/// Capped at header + [`BTW_MAX_BODY_LINES`] + padding; longer answers
/// scroll inside the panel instead of growing it.
pub(crate) fn btw_panel_height(
    entry: Option<&crate::app::BtwEntry>,
    width: u16,
    colors: &ThemeColors,
) -> u16 {
    let Some(entry) = entry else {
        return 0;
    };
    if width == 0 {
        return 0;
    }
    let total = btw_body_lines(entry, btw_body_width(width), colors)
        .len()
        .max(1);
    let visible = total.min(BTW_MAX_BODY_LINES);
    BTW_TOP_PADDING + (1 + visible) as u16 + BTW_BOTTOM_PADDING
}

fn wrap_plain_text_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in line.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        let separator_width = usize::from(!current.is_empty());
        if !current.is_empty() && current_width + separator_width + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += separator_width + word_width;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if word_width <= width {
            current.push_str(word);
            current_width = word_width;
        } else {
            // Split overlong words on char boundaries.
            let mut chunk = String::new();
            let mut chunk_width = 0usize;
            for ch in word.chars() {
                let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                if chunk_width + char_width > width && !chunk.is_empty() {
                    lines.push(std::mem::take(&mut chunk));
                    chunk_width = 0;
                }
                chunk.push(ch);
                chunk_width += char_width;
            }
            current = chunk;
            current_width = chunk_width;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(crate) fn render_btw_panel(
    f: &mut Frame,
    area: Rect,
    entry: Option<&crate::app::BtwEntry>,
    scroll_offset: usize,
    panel_area_out: &mut Option<Rect>,
    colors: &ThemeColors,
) {
    let Some(entry) = entry else {
        return;
    };
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Hit-test target for mouse-wheel scrolling over the panel.
    *panel_area_out = Some(area);

    let border_set = border::Set {
        vertical_left: "┃",
        ..border::PLAIN
    };
    let border = Block::new()
        .borders(Borders::LEFT)
        .border_set(border_set)
        .border_style(Style::default().fg(colors.info));
    let inner_area = border.inner(area);
    let panel_bg = queued_messages_background(colors);
    let bg = Block::default().style(Style::default().bg(panel_bg));
    f.render_widget(bg, area);
    f.render_widget(border, area);

    let content_area = Rect {
        x: inner_area.x.saturating_add(2),
        y: inner_area.y.saturating_add(BTW_TOP_PADDING),
        width: inner_area.width.saturating_sub(3),
        height: inner_area
            .height
            .saturating_sub(BTW_TOP_PADDING + BTW_BOTTOM_PADDING),
    };
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let mut lines = Vec::new();
    // The panel is always laid out at the width `btw_panel_height` computed
    // for, and its viewport is the actual allocated body height — never the
    // 10-line cap. Both must match render or the wrap (total) and the max
    // offset drift apart on small terminals.
    let viewport = btw_body_viewport(area.height);
    let content_width = content_width_for_area_width(area.width);
    let body_all = btw_body_lines(entry, content_width, colors);
    let total = body_all.len().max(1);
    let max_offset = total.saturating_sub(viewport);
    let start = scroll_offset.min(max_offset);
    let end = (start + viewport).min(total);

    let mut hint_text = String::from("esc dismiss");
    if max_offset > 0 {
        hint_text.push_str(&format!(" · ↑↓ {end}/{total}"));
    }
    let hint_width = UnicodeWidthStr::width(hint_text.as_str());
    let title = format!("◐ btw — {}", entry.question);
    let title = truncate_to_width(&title, content_area.width as usize);
    let title_width = 2 + UnicodeWidthStr::width(title.as_str());
    let show_hint = content_area.width as usize >= title_width + hint_width + 4;

    let mut header_spans = vec![
        Span::styled("•", Style::default().fg(colors.info)),
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
            "esc ",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ));
        header_spans.push(Span::styled(
            "dismiss",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::BOLD),
        ));
        if max_offset > 0 {
            header_spans.push(Span::styled(
                format!(" · ↑↓ {end}/{total}"),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
        }
    }
    lines.push(Line::from(header_spans));

    for mut body_line in body_all.into_iter().skip(start).take(viewport) {
        // Keep the 2-space body indent; preserve each line's own style.
        let mut spans = Vec::with_capacity(body_line.spans.len() + 1);
        spans.push(Span::raw("  "));
        spans.append(&mut body_line.spans);
        let mut line = Line::from(spans);
        line.style = body_line.style;
        lines.push(line);
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(panel_bg)),
        content_area,
    );
}

fn render_subagent_footer(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    tabs: &SubagentTabs,
    usage_text: &str,
    colors: &ThemeColors,
    is_streaming: bool,
    is_compacting: bool,
    esc_cancel_primed: bool,
    retry_status: Option<&crate::app::StreamingRetryStatus>,
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
        Span::raw(SUBAGENT_FOOTER_NAV_GAP),
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
            esc_cancel_primed,
            retry_status,
            area.width,
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
        btw_body_lines, btw_body_width, btw_panel_height, chat_status_layout_widths,
        compact_transcript_layout, display_agent_name, natural_sticky_index, paint_sticky_overlay,
        queued_messages_height, render_chat, render_subagent_spinner_only, resolve_sticky_display,
        sticky_overlay_height_for_span, sticky_overlay_rect, streaming_status_spans,
        subagent_nav_width, subagent_streaming_status_spans, user_message_body_end,
        wrap_plain_text_line, ChatState, ChatStatusLayoutWidths, PendingHover, BTW_MAX_BODY_LINES,
        STICKY_UP_HYSTERESIS, STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH,
    };
    use crate::theme::ThemeColors;
    use crate::ui::components::{
        chat::Chat, find::FindBar, input::Input, wave_spinner::WaveSpinner,
    };
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span},
    };

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
            false,
            None,
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
            false,
            None,
            STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH - 1,
        );

        assert_eq!(spans[0].content.as_ref(), "⠋");
    }

    #[test]
    fn subagent_streaming_status_uses_parent_compact_breakpoint() {
        let colors = test_colors();
        let spinner = WaveSpinner::new(Color::Blue);

        let compact = subagent_streaming_status_spans(
            &spinner,
            &colors,
            false,
            false,
            None,
            STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH - 1,
            80,
        );
        let full = subagent_streaming_status_spans(
            &spinner,
            &colors,
            false,
            false,
            None,
            STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH,
            80,
        );

        assert_eq!(compact[0].content.as_ref(), "⠋");
        assert_eq!(full[0].content.as_ref(), "■");
    }

    #[test]
    fn isolated_subagent_spinner_preserves_every_cell_outside_spinner() {
        let area = Rect::new(0, 0, 100, 30);
        let mut buffer = Buffer::filled(area, ratatui::buffer::Cell::new("x"));
        let original = buffer.clone();
        let mut spinner = WaveSpinner::new(Color::Blue);

        assert!(render_subagent_spinner_only(
            &mut buffer,
            &mut spinner,
            Color::Blue
        ));

        let spinner_y = 26;
        for y in 0..area.height {
            for x in 0..area.width {
                if y != spinner_y || !(3..11).contains(&x) {
                    assert_eq!(buffer[(x, y)], original[(x, y)], "changed ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn isolated_compact_spinner_changes_exactly_one_cell() {
        let area = Rect::new(0, 0, STREAMING_STATUS_COMPACT_BREAKPOINT_WIDTH - 1, 20);
        let mut buffer = Buffer::filled(area, ratatui::buffer::Cell::new("x"));
        let original = buffer.clone();
        let mut spinner = WaveSpinner::new(Color::Blue);

        assert!(render_subagent_spinner_only(
            &mut buffer,
            &mut spinner,
            Color::Blue
        ));
        assert_eq!(
            buffer
                .content
                .iter()
                .zip(&original.content)
                .filter(|(current, previous)| current != previous)
                .count(),
            1
        );
    }

    #[test]
    fn streaming_status_shows_retry_countdown() {
        let chat = Chat::new();
        let colors = test_colors();
        let spinner = WaveSpinner::new(Color::Blue);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let retry = crate::app::StreamingRetryStatus {
            attempt: 2,
            message: "Too Many Requests".to_string(),
            next_epoch_ms: now_ms + 2_000,
        };

        let line = streaming_status_spans(&chat, &spinner, &colors, false, false, Some(&retry), 96)
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>();

        assert!(line.contains("Too Many Requests"));
        assert!(line.contains("retrying in"));
        assert!(line.contains("attempt #2"));
        assert!(line.contains("esc interrupt"));
    }

    #[test]
    fn streaming_status_shows_esc_again_when_cancel_primed() {
        let chat = Chat::new();
        let mut colors = test_colors();
        colors.warning = Color::Yellow;
        colors.info = Color::Cyan;
        colors.text_weak = Color::DarkGray;
        let spinner = WaveSpinner::new(Color::Blue);

        let primed = streaming_status_spans(&chat, &spinner, &colors, false, true, None, 96);
        let idle = streaming_status_spans(&chat, &spinner, &colors, false, false, None, 96);

        let primed_line = primed
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(primed_line.contains("esc again to interrupt"));

        let primed_hint = primed
            .iter()
            .find(|span| span.content.as_ref() == "esc again to interrupt")
            .expect("armed interrupt hint span");
        assert_eq!(primed_hint.style.fg, Some(Color::Yellow));
        assert_ne!(primed_hint.style.fg, Some(colors.info));

        let idle_hint = idle
            .iter()
            .find(|span| span.content.as_ref() == "esc interrupt")
            .expect("idle interrupt hint span");
        assert_eq!(idle_hint.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn subagent_footer_reserves_spinner_width_before_nav() {
        assert_eq!(subagent_nav_width(4, true, 24), 0);
        assert_eq!(subagent_nav_width(20, true, 24), 12);
        assert_eq!(subagent_nav_width(20, false, 24), 20);
    }

    #[test]
    fn compact_transcript_layout_keeps_header_and_full_chat_height() {
        let area = Rect::new(0, 0, 80, 30);
        let (header, chat) = compact_transcript_layout(area);
        assert_eq!(header, Rect::new(0, 0, 80, 3));
        assert_eq!(chat, Rect::new(0, 3, 80, 27));
        // Sticky is not a layout row: chat fills everything below the header.
        assert_eq!(header.height + chat.height, area.height);
    }

    #[test]
    fn sticky_overlay_rect_sits_on_top_of_transcript_without_shrinking_it() {
        let chat_area = Rect::new(0, 3, 80, 27);
        let sticky = sticky_overlay_rect(chat_area, 5).expect("sticky overlay");
        assert_eq!(sticky, Rect::new(0, 3, 80, 5));
        // Overlay occupies the top of the transcript; chat area itself is unchanged.
        assert_eq!(sticky.x, chat_area.x);
        assert_eq!(sticky.y, chat_area.y);
        assert_eq!(sticky.width, chat_area.width);
        assert!(sticky.height < chat_area.height);
    }

    #[test]
    fn sticky_overlay_rect_is_none_when_height_or_area_is_zero() {
        let chat_area = Rect::new(0, 3, 80, 27);
        assert!(sticky_overlay_rect(chat_area, 0).is_none());
        assert!(sticky_overlay_rect(Rect::new(0, 0, 0, 10), 3).is_none());
        assert!(sticky_overlay_rect(Rect::new(0, 0, 10, 0), 3).is_none());
    }

    #[test]
    fn sticky_overlay_rect_clamps_to_transcript_height() {
        let chat_area = Rect::new(0, 3, 80, 2);
        let sticky = sticky_overlay_rect(chat_area, 5).expect("clamped sticky");
        assert_eq!(sticky.height, 2);
        assert_eq!(sticky.y, chat_area.y);
    }

    #[test]
    fn sticky_overlay_does_not_leak_underlying_cell_styles() {
        // Paragraph patches styles and only rewrites grapheme-covered cells.
        // Pre-fill the sticky rect with conspicuous formatting, then ensure
        // paint_sticky_overlay clears before drawing so bold/fg/bg cannot leak
        // into sticky cells (including trailing/unwritten ones).
        let sticky = Rect::new(0, 0, 20, 3);
        let sticky_bg = Color::Rgb(30, 30, 40);
        let leak_style = Style::default()
            .fg(Color::Rgb(255, 0, 0))
            .bg(Color::Rgb(0, 255, 0))
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        for y in sticky.y..sticky.bottom() {
            for x in sticky.x..sticky.right() {
                let cell = buf.cell_mut((x, y)).expect("pre-fill cell");
                cell.set_symbol("X");
                cell.set_style(leak_style);
            }
        }

        // Short sticky content leaves many trailing cells on each row.
        let sticky_lines = vec![
            Line::from(Span::styled(
                "▌ ",
                Style::default().fg(Color::Gray).bg(sticky_bg),
            )),
            Line::from(vec![
                Span::styled("▌ ", Style::default().fg(Color::Gray).bg(sticky_bg)),
                Span::styled("hi", Style::default().fg(Color::White).bg(sticky_bg)),
            ]),
            Line::from(Span::styled(
                "▌  ▴  ",
                Style::default().fg(Color::Gray).bg(sticky_bg),
            )),
        ];
        paint_sticky_overlay(&mut buf, sticky, sticky_lines, sticky_bg);

        for y in sticky.y..sticky.bottom() {
            for x in sticky.x..sticky.right() {
                let cell = buf.cell((x, y)).expect("sticky cell");
                assert_ne!(
                    cell.symbol(),
                    "X",
                    "sticky cell ({x},{y}) retained pre-fill symbol"
                );
                assert_eq!(
                    cell.bg, sticky_bg,
                    "sticky cell ({x},{y}) missing sticky background"
                );
                assert_ne!(
                    cell.fg,
                    Color::Rgb(255, 0, 0),
                    "sticky cell ({x},{y}) leaked underlying foreground"
                );
                assert!(
                    !cell.modifier.contains(Modifier::BOLD),
                    "sticky cell ({x},{y}) leaked bold"
                );
                assert!(
                    !cell.modifier.contains(Modifier::UNDERLINED),
                    "sticky cell ({x},{y}) leaked underline"
                );
            }
        }
    }

    #[test]
    fn sticky_visibility_does_not_change_transcript_viewport_height() {
        // Simulates the compact layout: chat area is always full-height below
        // the header, whether or not a sticky overlay would be painted.
        let content_area = Rect::new(0, 0, 100, 40);
        let (header, chat_without_sticky) = compact_transcript_layout(content_area);
        let (_, chat_with_sticky) = compact_transcript_layout(content_area);
        let sticky = sticky_overlay_rect(chat_with_sticky, 5).expect("sticky");

        assert_eq!(header.height, 3);
        assert_eq!(chat_without_sticky.height, chat_with_sticky.height);
        assert_eq!(chat_with_sticky.height, content_area.height - header.height);
        // Overlay lives inside the chat rect; it does not reduce chat height.
        assert!(sticky.y >= chat_with_sticky.y);
        assert!(sticky.bottom() <= chat_with_sticky.bottom());
        assert_eq!(chat_with_sticky.height, 37);
    }

    /// Build synthetic `(index, start, body_end)` user-message rows.
    ///
    /// `body_lines` is the full layout height of the user message including the
    /// trailing inter-message blank (top pad + content + bottom pad + blank).
    /// Body end used for sticky is therefore `start + body_lines - 1`.
    fn synthetic_user_messages(specs: &[(usize, usize, usize)]) -> Vec<(usize, usize, usize)> {
        specs
            .iter()
            .map(|&(idx, start, body_lines)| {
                let end_including_blank = start + body_lines;
                (idx, start, user_message_body_end(end_including_blank))
            })
            .collect()
    }

    #[test]
    fn sticky_appears_for_normal_user_assistant_transcript() {
        // Normal conversation: user → assistant → (later) user.
        // Sticky must appear once the prior user's body has left the top, even
        // though the assistant occupies the next rendered block and is fully
        // inside the viewport. Intermediate assistant/tool rows do not hide
        // sticky — only the next *user* message does, via sticky coverage.
        //
        // U0: start=0, body_lines=4 → body_end=3, sticky_height=3.
        // Assistant at 4 (ignored for hide). Next user U1 at 40.
        let msgs = synthetic_user_messages(&[
            (0, 0, 4),  // U0 body_end = 3
            (2, 40, 4), // U1 body_end = 43
        ]);

        assert_eq!(msgs[0].2, 3, "1-line body_end excludes trailing blank");
        assert_eq!(
            sticky_overlay_height_for_span(msgs[0].1, msgs[0].2),
            3,
            "1-line content → 3-row sticky overlay"
        );

        assert_eq!(natural_sticky_index(&msgs, 2), None, "body still in view");
        assert_eq!(
            natural_sticky_index(&msgs, 3),
            Some(0),
            "sticky appears for normal user→assistant once body fully leaves"
        );
        assert_eq!(
            natural_sticky_index(&msgs, 4),
            Some(0),
            "assistant immediately below user does not suppress sticky"
        );
        assert_eq!(
            natural_sticky_index(&msgs, 20),
            Some(0),
            "sticky stays while scrolling through assistant content"
        );
        // Still well above U1's sticky-coverage boundary (40 - 3 = 37).
        assert_eq!(natural_sticky_index(&msgs, 30), Some(0));
    }

    #[test]
    fn sticky_appears_immediately_when_message_body_fully_leaves_viewport() {
        // 1-line user content → layout is 4 rows: pad + content + pad + blank.
        // body_end = start + 3. Sticky must appear at S == body_end, not one row
        // later (which would wait for the trailing blank).
        let msgs = synthetic_user_messages(&[
            (0, 0, 4),  // U0: lines 0..4, body_end = 3
            (2, 40, 4), // U1 later
        ]);

        assert_eq!(msgs[0].2, 3, "1-line body_end excludes trailing blank");
        assert_eq!(natural_sticky_index(&msgs, 2), None, "body still in view");
        assert_eq!(
            natural_sticky_index(&msgs, 3),
            Some(0),
            "sticky appears the row the body fully leaves"
        );
        assert_eq!(natural_sticky_index(&msgs, 4), Some(0));

        // 3-line user content → layout is 6 rows: pad + 3 content + pad + blank.
        // body_end = start + 5. Same "appear immediately" rule.
        let tall = synthetic_user_messages(&[
            (0, 0, 6), // body_end = 5
            (2, 50, 6),
        ]);
        assert_eq!(tall[0].2, 5, "3-line body_end excludes trailing blank");
        assert_eq!(natural_sticky_index(&tall, 4), None);
        assert_eq!(
            natural_sticky_index(&tall, 5),
            Some(0),
            "tall sticky appears as soon as body leaves, not after blank"
        );
    }

    #[test]
    fn sticky_hides_when_next_user_enters_sticky_covered_region() {
        // Adjacent examples from the product requirement:
        // sticky rendered rows 1..3 and next viewport message rows 4..6 → hide on
        // the first scroll increment where the next message enters the sticky-
        // covered top region. Same for sticky rows 1..5.
        //
        // Hide formula: S + sticky_height > next_user_start (half-open coverage).
        // Equal bottom edge keeps sticky visible.

        // 1-line / 3-row sticky. Place next user so body_end + sticky_height
        // lands exactly on next_user_start: body_end=3, sticky_height=3 →
        // next_user_start=6. Visible at S=3 (3+3==6), hidden at S=4 (4+3>6).
        let short = synthetic_user_messages(&[
            (0, 0, 4), // body_end = 3, sticky_height = 3
            (2, 6, 4), // next user starts at row 6
        ]);
        assert_eq!(sticky_overlay_height_for_span(0, 3), 3);
        assert_eq!(
            natural_sticky_index(&short, 3),
            Some(0),
            "adjacent 3-row sticky: equal edge (S+H == next_start) stays visible"
        );
        assert_eq!(
            natural_sticky_index(&short, 4),
            None,
            "adjacent 3-row sticky: first increment past equal edge hides"
        );

        // 3-line / 5-row sticky. body_end=5, sticky_height=5 → next_user_start=10.
        // Visible at S=5 (5+5==10), hidden at S=6 (6+5>10) — one increment later.
        let tall = synthetic_user_messages(&[
            (0, 0, 6),  // body_end = 5, sticky_height = 5
            (2, 10, 6), // next user starts at row 10
        ]);
        assert_eq!(sticky_overlay_height_for_span(0, 5), 5);
        assert_eq!(
            natural_sticky_index(&tall, 5),
            Some(0),
            "adjacent 5-row sticky: equal edge keeps sticky visible"
        );
        assert_eq!(
            natural_sticky_index(&tall, 6),
            None,
            "adjacent 5-row sticky: first increment past equal edge hides"
        );

        // Non-adjacent: next user far below. Sticky remains while scrolling
        // through intermediate content until the covered region reaches it.
        let gap = synthetic_user_messages(&[
            (0, 0, 4),  // body_end = 3, sticky_height = 3
            (2, 40, 4), // next user at 40
        ]);
        // Hide when S + 3 > 40 → S >= 38.
        assert_eq!(natural_sticky_index(&gap, 37), Some(0));
        assert_eq!(
            natural_sticky_index(&gap, 38),
            None,
            "next user first row enters sticky-covered top region"
        );
    }

    #[test]
    fn sticky_hide_uses_sticky_coverage_not_viewport_height() {
        // Hide is driven by sticky overlay coverage vs next *user* start, not
        // full viewport height and not intermediate assistant/tool blocks.
        // Short (3-row) and tall (5-row) stickies therefore hide at different
        // offsets for the same next_user_start — sticky_height matters for the
        // visual covered region, but never for scroll extent / layout.
        let short = synthetic_user_messages(&[
            (0, 0, 4), // body_end = 3; sticky_height = 3
            (2, 20, 4),
        ]);
        let tall = synthetic_user_messages(&[
            (0, 0, 6), // body_end = 5; sticky_height = 5
            (2, 20, 6),
        ]);

        // Short: hide when S + 3 > 20 → S >= 18.
        assert_eq!(natural_sticky_index(&short, 3), Some(0));
        assert_eq!(natural_sticky_index(&short, 17), Some(0));
        assert_eq!(
            natural_sticky_index(&short, 18),
            None,
            "short sticky hides at S + sticky_height > next_user"
        );

        // Tall: hide when S + 5 > 20 → S >= 16 — earlier than short because the
        // taller overlay covers more of the top region.
        assert_eq!(natural_sticky_index(&tall, 5), Some(0));
        assert_eq!(natural_sticky_index(&tall, 15), Some(0));
        assert_eq!(
            natural_sticky_index(&tall, 16),
            None,
            "tall sticky hides earlier by sticky_height, not by viewport height"
        );
    }

    #[test]
    fn sticky_ignores_assistant_and_tool_blocks_between_users() {
        // Transcript: U0 (idx 0) → assistant (1) → tool (2) → U1 (3).
        // Sticky for U0 must remain while scrolling through assistant/tool and
        // only hide when U1 enters the sticky-covered top region.
        let msgs = synthetic_user_messages(&[
            (0, 0, 4),  // U0 body_end = 3, sticky_height = 3
            (3, 50, 4), // U1 body_end = 53
        ]);

        assert_eq!(
            natural_sticky_index(&msgs, 3),
            Some(0),
            "U0 sticky while assistant immediately follows"
        );
        assert_eq!(
            natural_sticky_index(&msgs, 25),
            Some(0),
            "assistant/tool content does not suppress sticky"
        );
        // Hide when S + 3 > 50 → S >= 48.
        assert_eq!(natural_sticky_index(&msgs, 47), Some(0));
        assert_eq!(
            natural_sticky_index(&msgs, 48),
            None,
            "hide only when next *user* enters sticky coverage"
        );
        // U1 sticky once its body is fully above.
        assert_eq!(
            natural_sticky_index(&msgs, 53),
            Some(3),
            "selected sticky remains the previous user message (U1)"
        );
    }

    #[test]
    fn sticky_display_uses_up_hysteresis_without_overlay_geometry() {
        // U0 body_end=3 sticky_height=3, U1 start=40 body_end=43 sticky_height=3.
        // Hide U0 sticky when S + 3 > 40, i.e. S >= 38.
        let msgs = synthetic_user_messages(&[(0, 0, 4), (2, 40, 4)]);

        // Scroll down: memory tracks natural.
        assert_eq!(resolve_sticky_display(&msgs, 3, None), Some(0));
        assert_eq!(resolve_sticky_display(&msgs, 20, Some(0)), Some(0));

        // Past hide boundary for U0, before U1 body is fully above → no sticky.
        // natural == None must not resurrect remembered state.
        assert_eq!(
            resolve_sticky_display(&msgs, 38, Some(0)),
            None,
            "natural None cannot resurrect memory"
        );
        assert_eq!(resolve_sticky_display(&msgs, 40, Some(0)), None);
        assert_eq!(resolve_sticky_display(&msgs, 43, Some(0)), Some(2));

        // Scroll up from U1 sticky: hand-off to U0 requires natural to want U0
        // (S + sticky_height <= U1_start) and clearance
        // S + 1 + UP_HYSTERESIS <= memory_start.
        assert_eq!(STICKY_UP_HYSTERESIS, 5);
        assert_eq!(
            resolve_sticky_display(&msgs, 36, Some(2)),
            None,
            "within sticky-coverage of U1 → natural None"
        );
        assert_eq!(
            resolve_sticky_display(&msgs, 37, Some(2)),
            None,
            "U1 body re-entered viewport → no sticky"
        );
        // S=30: natural wants U0 (30+3 <= 40) and clearance 30+1+5=36 <= 40.
        assert_eq!(
            resolve_sticky_display(&msgs, 30, Some(2)),
            Some(0),
            "clearance met and next user not under sticky coverage → hand off"
        );
        // Body of the remembered sticky re-entered → clear.
        assert_eq!(resolve_sticky_display(&msgs, 2, Some(0)), None);
    }

    #[test]
    fn sticky_at_bottom_keeps_previous_user_while_latest_still_visible() {
        // U0 body_end=3, U1 start=100 body_end=103.
        // Near the bottom (S=90) U1 is still on screen below the top → sticky stays U0.
        // Callers must pass resolved_scroll_offset(); raw stick-to-bottom MAX would
        // make every body_end <= S and incorrectly pin the latest user.
        let msgs = synthetic_user_messages(&[(0, 0, 4), (5, 100, 4)]);
        assert_eq!(
            natural_sticky_index(&msgs, 90),
            Some(0),
            "previous user sticky while latest remains visible"
        );
        assert_eq!(
            natural_sticky_index(&msgs, 103),
            Some(5),
            "latest becomes sticky only after its body fully leaves the top"
        );
        assert_eq!(
            natural_sticky_index(&msgs, usize::MAX),
            Some(5),
            "MAX sentinel traps sticky on the latest user — never pass unresolved offset"
        );
    }

    #[test]
    fn compact_render_keeps_chat_area_stable_when_sticky_appears() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut chat_state = ChatState {
            chat: Chat::new(),
            wave_spinner: WaveSpinner::new(Color::Blue),
            compact_mode: true,
            sticky_message_index: None,
            sticky_click_target: None,
            last_chat_area: None,
        };
        // Sticky appears for a prior user once its body leaves the top, even with
        // a following assistant. Tall assistant content gives room to scroll the
        // first user fully above the viewport while remaining well clear of the
        // next user sticky-coverage boundary.
        chat_state.chat.add_user_message("sticky candidate");
        // Tall enough that first_user body_end is reachable within max_scroll
        // after chrome (header/input) shrinks the chat viewport on an 80x40 term.
        chat_state
            .chat
            .add_assistant_message("assistant reply\n".repeat(120));
        chat_state.chat.add_user_message("later user");
        // Pin to top after adds (add_* sets scroll_offset = MAX while autoscroll is on).
        chat_state.chat.autoscroll_enabled = false;
        chat_state.chat.scroll_offset = 0;
        chat_state.chat.scroll_up(0); // marks user_scrolled_up so pin-to-bottom stays off

        let mut input = Input::new();
        let mut find_bar = FindBar::new();
        let colors = test_colors();
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");

        // First paint: near top, no sticky expected. Record chat area height.
        terminal
            .draw(|f| {
                render_chat(
                    f,
                    &mut chat_state,
                    &mut input,
                    "0.0.0".into(),
                    "/tmp".into(),
                    None,
                    "build".into(),
                    "model".into(),
                    "model".into(),
                    "provider".into(),
                    None,
                    false,
                    &colors,
                    false,
                    false,
                    false,
                    None,
                    "",
                    None,
                    &[],
                    false,
                    None,
                    0,
                    &mut None,
                    &mut find_bar,
                    true,
                    Some("Session"),
                    0,
                    &mut None,
                    &mut None,
                    &mut None,
                    None,
                );
            })
            .expect("draw without sticky");

        let area_without = chat_state.last_chat_area.expect("chat area without sticky");
        let viewport_without = chat_state.chat.viewport_height;
        assert!(
            chat_state.sticky_click_target.is_none(),
            "sticky should be hidden near the top of the transcript"
        );

        // Scroll so the first user message is fully above the viewport, but the
        // later user has not entered the sticky-covered top region yet.
        // Use the first user's body_end as the scroll target. Must be within
        // max_scroll_offset — sticky math uses resolved_scroll_offset().
        let first_user_body_end = {
            let starts = &chat_state.chat.message_line_positions;
            let end = starts
                .get(1)
                .copied()
                .unwrap_or(chat_state.chat.content_height);
            user_message_body_end(end)
        };
        let max_scroll = chat_state.chat.max_scroll_offset();
        assert!(
            first_user_body_end <= max_scroll,
            "fixture must allow scrolling past first user body (body_end={first_user_body_end}, max_scroll={max_scroll})"
        );
        chat_state.chat.scroll_offset = first_user_body_end;
        chat_state.chat.scroll_up(0);

        terminal
            .draw(|f| {
                render_chat(
                    f,
                    &mut chat_state,
                    &mut input,
                    "0.0.0".into(),
                    "/tmp".into(),
                    None,
                    "build".into(),
                    "model".into(),
                    "model".into(),
                    "provider".into(),
                    None,
                    false,
                    &colors,
                    false,
                    false,
                    false,
                    None,
                    "",
                    None,
                    &[],
                    false,
                    None,
                    0,
                    &mut None,
                    &mut find_bar,
                    true,
                    Some("Session"),
                    0,
                    &mut None,
                    &mut None,
                    &mut None,
                    None,
                );
            })
            .expect("draw with sticky");

        let area_with = chat_state.last_chat_area.expect("chat area with sticky");
        let viewport_with = chat_state.chat.viewport_height;

        assert_eq!(
            area_without, area_with,
            "sticky overlay must not change the transcript layout rect"
        );
        assert_eq!(
            viewport_without, viewport_with,
            "sticky overlay must not change Chat::viewport_height / scroll extent"
        );
        let (sticky_rect, sticky_idx) = chat_state
            .sticky_click_target
            .expect("sticky click target for normal user→assistant after body leaves");
        // First user message is index 0 (user, assistant, later user).
        assert_eq!(sticky_idx, 0);
        assert_eq!(sticky_rect.x, area_with.x);
        assert_eq!(sticky_rect.y, area_with.y);
        assert_eq!(sticky_rect.width, area_with.width);
        assert!(sticky_rect.height >= 3 && sticky_rect.height <= 5);
    }

    #[test]
    fn compact_render_without_sticky_leaves_full_transcript_area() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut chat_state = ChatState {
            chat: Chat::new(),
            wave_spinner: WaveSpinner::new(Color::Blue),
            compact_mode: true,
            sticky_message_index: None,
            sticky_click_target: None,
            last_chat_area: None,
        };
        chat_state.chat.autoscroll_enabled = false;
        chat_state
            .chat
            .add_user_message("only message still in view");
        chat_state.chat.scroll_offset = 0;
        chat_state.chat.scroll_up(0);

        let mut input = Input::new();
        let mut find_bar = FindBar::new();
        let colors = test_colors();
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|f| {
                render_chat(
                    f,
                    &mut chat_state,
                    &mut input,
                    "0.0.0".into(),
                    "/tmp".into(),
                    None,
                    "build".into(),
                    "model".into(),
                    "model".into(),
                    "provider".into(),
                    None,
                    false,
                    &colors,
                    false,
                    false,
                    false,
                    None,
                    "",
                    None,
                    &[],
                    false,
                    None,
                    0,
                    &mut None,
                    &mut find_bar,
                    true,
                    Some("Session"),
                    0,
                    &mut None,
                    &mut None,
                    &mut None,
                    None,
                );
            })
            .expect("draw");

        let chat_area = chat_state.last_chat_area.expect("chat area");
        // Transcript is everything below the fixed 3-row compact header.
        // Input/help/status rows reduce available height, but sticky is not a
        // layout row so the chat area is still "full" relative to that chrome.
        assert_eq!(chat_area.y, 3, "chat starts immediately under the header");
        assert!(chat_area.height > 0);
        assert!(chat_state.sticky_click_target.is_none());
        // Overlay helpers agree: no sticky height → no overlay rect.
        assert!(sticky_overlay_rect(chat_area, 0).is_none());
    }

    #[test]
    fn btw_panel_height_is_zero_without_entry() {
        assert_eq!(btw_panel_height(None, 80, &test_colors()), 0);
    }

    #[test]
    fn btw_panel_height_grows_with_wrapped_answer() {
        let colors = test_colors();
        let pending =
            crate::app::BtwEntry::pending(Some("s1".to_string()), "what broke?".to_string());
        assert!(pending.is_pending());
        let pending_height = btw_panel_height(Some(&pending), 80, &colors);
        // Header + 1 body line + padding.
        assert_eq!(pending_height, 1 + (1 + 1) + 1);

        let mut answered =
            crate::app::BtwEntry::pending(Some("s1".to_string()), "what broke?".to_string());
        answered.answer = Some("one two three four five".to_string());
        // Narrow width forces the answer onto 3 lines.
        assert_eq!(
            btw_panel_height(Some(&answered), 18, &colors),
            1 + (1 + 3) + 1
        );
    }

    #[test]
    fn btw_panel_height_caps_long_answers() {
        let colors = test_colors();
        let mut answered =
            crate::app::BtwEntry::pending(Some("s1".to_string()), "long answer".to_string());
        answered.answer = Some(
            (1..=30)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
        // Header + capped viewport + padding, regardless of total lines.
        assert_eq!(
            btw_panel_height(Some(&answered), 80, &colors),
            1 + (1 + BTW_MAX_BODY_LINES) as u16 + 1
        );
        let lines = btw_body_lines(&answered, btw_body_width(80), &colors);
        assert!(lines.len() > BTW_MAX_BODY_LINES);
    }

    #[test]
    fn btw_body_renders_markdown_formatting() {
        let colors = test_colors();
        let mut answered =
            crate::app::BtwEntry::pending(Some("s1".to_string()), "format me".to_string());
        answered.answer = Some("Hello **bold** and `code`".to_string());
        let lines = btw_body_lines(&answered, btw_body_width(80), &colors);
        assert_eq!(lines.len(), 1);
        // Bold + code produce multiple styled spans, not one plain span.
        assert!(lines[0].spans.len() > 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("bold") && text.contains("code"));
    }

    #[test]
    fn wrap_plain_text_line_splits_long_words() {
        let lines = wrap_plain_text_line("abcdefghij", 4);
        assert_eq!(lines, vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap_plain_text_line("", 10), vec![String::new()]);
    }

    #[test]
    fn queued_messages_height_has_no_action_row() {
        assert_eq!(queued_messages_height(&[]), 0);
        // Top(1) + header(1) + 1 message + bottom(1); header hosts the actions.
        assert_eq!(queued_messages_height(&["one".to_string()]), 4);
        // Overflow adds exactly one "+N more" line; no extra action row.
        assert_eq!(
            queued_messages_height(&[
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]),
            1 + (1 + 3 + 1) + 1
        );
    }

    fn buffer_row_text(buffer: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
        (0..width)
            .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
            .collect()
    }

    fn render_pending_buffer(
        width: u16,
        height: u16,
        messages: &[String],
        has_user: bool,
        esc_primed: bool,
        colors: &ThemeColors,
        hover: Option<PendingHover>,
    ) -> (Option<Rect>, Option<Rect>, Buffer) {
        use ratatui::{backend::TestBackend, Terminal};

        let mut chat_state = ChatState {
            chat: Chat::new(),
            wave_spinner: WaveSpinner::new(Color::Blue),
            compact_mode: false,
            sticky_message_index: None,
            sticky_click_target: None,
            last_chat_area: None,
        };
        let mut input = Input::new();
        let mut find_bar = FindBar::new();
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut edit: Option<Rect> = None;
        let mut send: Option<Rect> = None;
        terminal
            .draw(|f| {
                render_chat(
                    f,
                    &mut chat_state,
                    &mut input,
                    "0.0.0".into(),
                    "/tmp".into(),
                    None,
                    "build".into(),
                    "model".into(),
                    "model".into(),
                    "provider".into(),
                    None,
                    false,
                    colors,
                    false,
                    false,
                    esc_primed,
                    None,
                    "",
                    None,
                    messages,
                    has_user,
                    None,
                    0,
                    &mut None,
                    &mut find_bar,
                    true,
                    Some("Session"),
                    0,
                    &mut None,
                    &mut edit,
                    &mut send,
                    hover,
                );
            })
            .expect("draw pending");
        let buffer = terminal.backend().buffer().clone();
        (edit, send, buffer)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pending_snapshot(
        width: u16,
        height: u16,
        messages: &[String],
        has_user: bool,
        esc_primed: bool,
    ) -> (Option<Rect>, Option<Rect>, Vec<String>) {
        render_pending_snapshot_with_hover(width, height, messages, has_user, esc_primed, None)
    }

    fn render_pending_snapshot_with_hover(
        width: u16,
        height: u16,
        messages: &[String],
        has_user: bool,
        esc_primed: bool,
        hover: Option<PendingHover>,
    ) -> (Option<Rect>, Option<Rect>, Vec<String>) {
        let colors = test_colors();
        let (edit, send, buffer) = render_pending_buffer(
            width, height, messages, has_user, esc_primed, &colors, hover,
        );
        let rows = (0..height)
            .map(|y| buffer_row_text(&buffer, width, y))
            .collect();
        (edit, send, rows)
    }

    fn render_pending_hitboxes(
        width: u16,
        height: u16,
        messages: &[String],
        has_user: bool,
    ) -> (Option<Rect>, Option<Rect>) {
        let (edit, send, _) = render_pending_snapshot(width, height, messages, has_user, false);
        (edit, send)
    }

    #[test]
    fn pending_hitboxes_appear_in_header_at_wide_width() {
        let messages = vec!["hello pending".to_string()];
        let (edit, send, rows) = render_pending_snapshot(80, 30, &messages, true, false);
        let edit = edit.expect("edit hitbox at 80 cols");
        let send = send.expect("send hitbox at 80 cols");
        // Shared header row, edit before send, both single-row and in bounds.
        assert_eq!(edit.height, 1);
        assert_eq!(send.height, 1);
        assert_eq!(edit.y, send.y);
        assert!(edit.x + edit.width <= 80);
        assert!(send.x + send.width <= 80);
        assert!(edit.x < send.x);
        // Quiet `edit` has no visible shortcut: single "edit" (4) mode at any width.
        assert_eq!(edit.width, 4);
        // Unprimed send-now hint is "esc " (4) + "send now" (8) via display width.
        assert_eq!(send.width, 12, "wide shows full 'esc send now' hint");
        // Header coordinates: same row hosts the title and both actions.
        let header = rows.get(edit.y as usize).expect("header row");
        assert!(
            header.contains("Queued message"),
            "header row must keep the title, got: {header:?}"
        );
        assert!(
            header.contains("edit"),
            "header must show edit, got: {header:?}"
        );
        assert!(
            !header.contains("(^X r)"),
            "header must not show the ^X hint, got: {header:?}"
        );
        assert!(
            !header.contains("^X"),
            "header must not show any ^X shortcut, got: {header:?}"
        );
        assert!(
            header.contains("esc"),
            "header must show esc hint, got: {header:?}"
        );
        assert!(
            header.contains("send now"),
            "header must show send now, got: {header:?}"
        );
        assert!(
            !header.contains("steer"),
            "header must not show stale steer wording, got: {header:?}"
        );
        // Hitboxes align with the right-aligned header labels (byte offsets
        // differ from columns for wide border/bullet glyphs, so compare via
        // display width).
        let edit_byte = header.find("edit").expect("edit offset");
        let edit_col = unicode_width::UnicodeWidthStr::width(&header[..edit_byte]) as u16;
        assert_eq!(edit.x, edit_col, "edit hitbox must sit on the header label");
        let send_byte = header.find("esc").expect("esc offset");
        let send_col = unicode_width::UnicodeWidthStr::width(&header[..send_byte]) as u16;
        assert_eq!(send.x, send_col, "send hitbox must sit on the esc hint");
        // Muted style: edit uses text_weak with no bold, distinct from primary.
        let mut styled_colors = test_colors();
        styled_colors.primary = Color::Red;
        styled_colors.text_weak = Color::Gray;
        let (styled_edit, _, styled_buffer) =
            render_pending_buffer(80, 30, &messages, true, false, &styled_colors, None);
        let styled_edit = styled_edit.expect("styled edit hitbox");
        assert_eq!(styled_edit.width, 4);
        for dx in 0..styled_edit.width {
            let cell = styled_buffer
                .cell((styled_edit.x + dx, styled_edit.y))
                .expect("edit cell");
            assert_eq!(
                cell.fg,
                Color::Gray,
                "edit cell {dx} must use text_weak, got {:?}",
                cell.fg
            );
            assert!(
                !cell.modifier.contains(Modifier::BOLD),
                "edit cell {dx} must not be bold"
            );
        }
    }

    #[test]
    fn pending_edit_quiet_at_medium_width() {
        let messages = vec!["hello pending".to_string()];
        // 44 cols -> content width 40: quiet edit (no ^X hint) + unprimed
        // send-now — same single mode as wide (needs 16+19+2=37).
        let (edit, send, rows) = render_pending_snapshot(44, 30, &messages, true, false);
        let edit = edit.expect("edit hitbox at 44 cols");
        let send = send.expect("send hitbox at 44 cols");
        assert_eq!(edit.width, 4, "medium keeps quiet edit without hint");
        assert_eq!(send.width, 12);
        assert_eq!(edit.y, send.y);
        assert!(edit.x + edit.width <= 44);
        assert!(send.x + send.width <= 44);
        let header = rows.get(edit.y as usize).expect("header row");
        assert!(header.contains("edit"), "got: {header:?}");
        assert!(
            !header.contains("(^X r)"),
            "medium header must not show the ^X hint, got: {header:?}"
        );
        assert!(header.contains("send now"), "got: {header:?}");
        assert!(!header.contains("steer"), "got: {header:?}");
    }

    #[test]
    fn pending_send_hint_preserves_armed_wording() {
        let messages = vec!["hello pending".to_string()];
        let (edit, send, rows) = render_pending_snapshot(80, 30, &messages, true, true);
        let edit = edit.expect("edit hitbox when primed");
        let send = send.expect("send hitbox when primed");
        assert_eq!(edit.width, 4, "primed keeps quiet edit without hint");
        // Armed hint is "esc again to " (13) + "send now" (8) via display width.
        assert_eq!(send.width, 21, "primed shows 'esc again to send now'");
        assert_eq!(edit.y, send.y);
        let header = rows.get(edit.y as usize).expect("header row");
        assert!(
            header.contains("esc again to"),
            "armed header must keep double-Esc wording, got: {header:?}"
        );
        assert!(header.contains("send now"), "got: {header:?}");
        assert!(!header.contains("steer"), "got: {header:?}");
        assert!(
            !header.contains("(^X r)"),
            "armed header must not show the ^X hint, got: {header:?}"
        );
    }

    #[test]
    fn pending_send_only_fallback_when_edit_does_not_fit() {
        let messages = vec!["hello pending".to_string()];
        // 36 cols -> content width 32: title + send-only fits (16+12+2=30),
        // but edit + send (16+19+2=37) does not.
        let (edit, send, rows) = render_pending_snapshot(36, 30, &messages, true, false);
        assert!(edit.is_none(), "too narrow for edit must hide only edit");
        let send = send.expect("send-only fallback must stay clickable");
        assert_eq!(send.width, 12);
        let header = rows.get(send.y as usize).expect("header row");
        assert!(
            header.contains("Queued message"),
            "fallback stays on the header, got: {header:?}"
        );
        assert!(
            !header.contains("edit"),
            "fallback header must drop edit, got: {header:?}"
        );
        assert!(header.contains("send now"), "got: {header:?}");
    }

    #[test]
    fn pending_hitboxes_hide_when_too_narrow() {
        let messages = vec!["hello pending".to_string()];
        // 20 cols -> content width 16: title fills the row, no room for
        // header actions alongside it.
        let (edit, send) = render_pending_hitboxes(20, 30, &messages, true);
        assert!(edit.is_none(), "too narrow must hide Edit target");
        assert!(send.is_none(), "too narrow must hide send target");
        // 14 cols -> content width 10: still hidden and never out of bounds.
        let (edit, send) = render_pending_hitboxes(14, 30, &messages, true);
        assert!(edit.is_none(), "too narrow must hide Edit target");
        assert!(send.is_none(), "too narrow must hide send target");
    }

    #[test]
    fn pending_hitboxes_reset_when_hidden() {
        use ratatui::{backend::TestBackend, Terminal};

        let mut chat_state = ChatState {
            chat: Chat::new(),
            wave_spinner: WaveSpinner::new(Color::Blue),
            compact_mode: false,
            sticky_message_index: None,
            sticky_click_target: None,
            last_chat_area: None,
        };
        let mut input = Input::new();
        let mut find_bar = FindBar::new();
        let colors = test_colors();
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let messages = vec!["hello pending".to_string()];

        let mut edit: Option<Rect> = None;
        let mut send: Option<Rect> = None;
        terminal
            .draw(|f| {
                render_chat(
                    f,
                    &mut chat_state,
                    &mut input,
                    "0.0.0".into(),
                    "/tmp".into(),
                    None,
                    "build".into(),
                    "model".into(),
                    "model".into(),
                    "provider".into(),
                    None,
                    false,
                    &colors,
                    false,
                    false,
                    false,
                    None,
                    "",
                    None,
                    &messages,
                    true,
                    None,
                    0,
                    &mut None,
                    &mut find_bar,
                    true,
                    Some("Session"),
                    0,
                    &mut None,
                    &mut edit,
                    &mut send,
                    None,
                );
            })
            .expect("draw with pending");
        assert!(edit.is_some() && send.is_some());

        // Second paint with no queued messages must clear the previous boxes.
        let empty: Vec<String> = Vec::new();
        terminal
            .draw(|f| {
                render_chat(
                    f,
                    &mut chat_state,
                    &mut input,
                    "0.0.0".into(),
                    "/tmp".into(),
                    None,
                    "build".into(),
                    "model".into(),
                    "model".into(),
                    "provider".into(),
                    None,
                    false,
                    &colors,
                    false,
                    false,
                    false,
                    None,
                    "",
                    None,
                    &empty,
                    false,
                    None,
                    0,
                    &mut None,
                    &mut find_bar,
                    true,
                    Some("Session"),
                    0,
                    &mut None,
                    &mut edit,
                    &mut send,
                    None,
                );
            })
            .expect("draw without pending");
        assert!(edit.is_none(), "empty queue must reset Edit hitbox");
        assert!(send.is_none(), "empty queue must reset send hitbox");
    }

    #[test]
    fn pending_hitboxes_hide_for_compact_only_queue() {
        // A compact-only queue still renders the "/compact" line, but Edit/send
        // require an actual pending user message.
        let compact_only = vec!["/compact".to_string()];
        let (edit, send) = render_pending_hitboxes(80, 30, &compact_only, false);
        assert!(edit.is_none(), "compact-only must not offer Edit");
        assert!(send.is_none(), "compact-only must not offer send");

        // Mixed queue (user message + /compact) keeps both targets.
        let mixed = vec!["hello pending".to_string(), "/compact".to_string()];
        let (edit, send) = render_pending_hitboxes(80, 30, &mixed, true);
        assert!(edit.is_some(), "mixed queue must offer Edit");
        assert!(send.is_some(), "mixed queue must offer send");
    }

    fn hover_test_colors() -> ThemeColors {
        let mut colors = test_colors();
        // Distinct idle (muted) vs hover (bright) foregrounds; backgrounds stay
        // Reset so fg-only can be asserted via bg equality.
        colors.text_weak = Color::Gray;
        colors.text = Color::White;
        colors
    }

    #[test]
    fn pending_edit_hover_changes_foreground_only() {
        let messages = vec!["hello pending".to_string()];
        let colors = hover_test_colors();
        let (edit, _, idle_buffer) =
            render_pending_buffer(80, 30, &messages, true, false, &colors, None);
        let edit = edit.expect("edit hitbox");
        // Idle Edit cells are muted.
        for dx in 0..edit.width {
            let cell = idle_buffer
                .cell((edit.x + dx, edit.y))
                .expect("idle Edit cell");
            assert_eq!(cell.fg, Color::Gray, "idle Edit must be muted");
            assert_eq!(cell.bg, Color::Reset, "idle Edit must have no bg");
        }
        let (hover_edit, _, hover_buffer) = render_pending_buffer(
            80,
            30,
            &messages,
            true,
            false,
            &colors,
            Some(PendingHover::Edit),
        );
        let hover_edit = hover_edit.expect("hovered edit hitbox");
        assert_eq!(hover_edit, edit, "hover must not move the hitbox");
        for dx in 0..hover_edit.width {
            let idle_cell = idle_buffer.cell((edit.x + dx, edit.y)).expect("idle cell");
            let cell = hover_buffer
                .cell((hover_edit.x + dx, hover_edit.y))
                .expect("hovered Edit cell");
            assert_eq!(
                cell.fg,
                Color::White,
                "hovered Edit cell {dx} must use text, got {:?}",
                cell.fg
            );
            assert_eq!(
                cell.bg, idle_cell.bg,
                "hovered Edit cell {dx} must not change background"
            );
            assert!(
                !cell.modifier.contains(Modifier::BOLD),
                "hovered Edit cell {dx} must stay non-bold (fg-only)"
            );
        }
    }

    #[test]
    fn pending_send_hover_highlights_full_action_foreground_only() {
        let messages = vec!["hello pending".to_string()];
        let colors = hover_test_colors();
        let (_, send, idle_buffer) =
            render_pending_buffer(80, 30, &messages, true, false, &colors, None);
        let send = send.expect("send hitbox");
        // Unprimed full action is "esc send now" (12 cols).
        assert_eq!(send.width, 12);
        for dx in 0..send.width {
            let cell = idle_buffer
                .cell((send.x + dx, send.y))
                .expect("idle send cell");
            assert_eq!(cell.fg, Color::Gray, "idle send must be muted");
            assert_eq!(cell.bg, Color::Reset, "idle send must have no bg");
        }
        let (_, hover_send, hover_buffer) = render_pending_buffer(
            80,
            30,
            &messages,
            true,
            false,
            &colors,
            Some(PendingHover::Send),
        );
        let hover_send = hover_send.expect("hovered send hitbox");
        assert_eq!(hover_send, send, "hover must not move the hitbox");
        for dx in 0..hover_send.width {
            let idle_cell = idle_buffer.cell((send.x + dx, send.y)).expect("idle cell");
            let cell = hover_buffer
                .cell((hover_send.x + dx, hover_send.y))
                .expect("hovered send cell");
            assert_eq!(
                cell.fg,
                Color::White,
                "hovered send cell {dx} must use text, got {:?}",
                cell.fg
            );
            assert_eq!(
                cell.bg, idle_cell.bg,
                "hovered send cell {dx} must not change background"
            );
        }
        // Armed full action is "esc again to send now" (21 cols); hover still
        // covers the whole action with fg-only change.
        let (_, armed_send, armed_idle) =
            render_pending_buffer(80, 30, &messages, true, true, &colors, None);
        let armed_send = armed_send.expect("armed send hitbox");
        assert_eq!(armed_send.width, 21);
        let (_, armed_hover, armed_hover_buffer) = render_pending_buffer(
            80,
            30,
            &messages,
            true,
            true,
            &colors,
            Some(PendingHover::Send),
        );
        let armed_hover = armed_hover.expect("armed hovered hitbox");
        assert_eq!(armed_hover, armed_send);
        for dx in 0..armed_hover.width {
            let idle_cell = armed_idle
                .cell((armed_send.x + dx, armed_send.y))
                .expect("armed idle cell");
            let cell = armed_hover_buffer
                .cell((armed_hover.x + dx, armed_hover.y))
                .expect("armed hovered cell");
            assert_eq!(cell.fg, Color::White, "armed hover cell {dx} must use text");
            assert_eq!(cell.bg, idle_cell.bg, "armed hover must not change bg");
        }
    }

    #[test]
    fn pending_hover_ignored_when_no_clickable_target() {
        // Compact-only queue shows the send-now text but offers no hitbox;
        // stale Send hover must not brighten it (stays muted).
        let colors = hover_test_colors();
        let compact_only = vec!["/compact".to_string()];
        let (edit, send, buffer) = render_pending_buffer(
            80,
            30,
            &compact_only,
            false,
            false,
            &colors,
            Some(PendingHover::Send),
        );
        assert!(edit.is_none());
        assert!(send.is_none());
        // Find the header row via the title and assert every send-now cell
        // remains muted even with hover set.
        let mut found_title = false;
        for y in 0..30u16 {
            let row = buffer_row_text(&buffer, 80, y);
            if row.contains("Queued message") {
                found_title = true;
                if let Some(byte) = row.find("esc") {
                    let col = unicode_width::UnicodeWidthStr::width(&row[..byte]) as u16;
                    for dx in 0..12u16 {
                        if let Some(cell) = buffer.cell((col + dx, y)) {
                            // Only check cells that are part of the hint; the
                            // row may be shorter on narrow renders, so skip OOB.
                            assert_eq!(
                                cell.fg,
                                Color::Gray,
                                "compact-only hover must stay muted at ({}, {y})",
                                col + dx
                            );
                            assert_eq!(cell.bg, Color::Reset);
                        }
                    }
                }
                break;
            }
        }
        assert!(found_title, "compact header must render");
    }
}
