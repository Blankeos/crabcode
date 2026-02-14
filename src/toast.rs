use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::theme::ThemeColors;

const DEFAULT_TOAST_DURATION: Duration = Duration::from_secs(4);
const MAX_QUEUED_TOASTS: usize = 24;
const MAX_VISIBLE_TOASTS: usize = 3;
const MAX_TEXT_LINES_PER_TOAST: usize = 8;

const TOAST_MIN_CONTENT_WIDTH: u16 = 12;
const TOAST_MAX_WIDTH: u16 = 96;
const TOAST_HORIZONTAL_MARGIN: u16 = 2;
const TOAST_VERTICAL_MARGIN: u16 = 1;
const TOAST_VERTICAL_SPACING: u16 = 1;

const ACCENT_WIDTH: u16 = 1;
const HORIZONTAL_PADDING: u16 = 2;
const VERTICAL_PADDING: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Warning,
    Error,
    Success,
}

impl ToastLevel {
    fn accent_color(self, colors: &ThemeColors) -> Color {
        match self {
            ToastLevel::Info => colors.info,
            ToastLevel::Warning => colors.warning,
            ToastLevel::Error => colors.error,
            ToastLevel::Success => colors.success,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    message: String,
    level: ToastLevel,
    expires_at: Instant,
}

impl Toast {
    pub fn new(message: impl Into<String>, level: ToastLevel, duration: Option<Duration>) -> Self {
        let duration = duration.unwrap_or(DEFAULT_TOAST_DURATION);
        Self {
            message: message.into(),
            level,
            expires_at: Instant::now() + duration,
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at <= now
    }
}

#[derive(Debug)]
pub struct ToastManager {
    toasts: VecDeque<Toast>,
}

impl ToastManager {
    pub fn new() -> Self {
        Self {
            toasts: VecDeque::new(),
        }
    }

    pub fn add(&mut self, toast: Toast) {
        self.toasts.push_back(toast);
        while self.toasts.len() > MAX_QUEUED_TOASTS {
            let _ = self.toasts.pop_front();
        }
    }

    pub fn remove_expired(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|toast| !toast.is_expired(now));
    }
}

pub fn render_toasts(frame: &mut Frame, manager: &ToastManager, colors: &ThemeColors) {
    let now = Instant::now();
    let visible_toasts: Vec<&Toast> = manager
        .toasts
        .iter()
        .rev()
        .filter(|toast| !toast.is_expired(now))
        .take(MAX_VISIBLE_TOASTS)
        .collect();

    if visible_toasts.is_empty() {
        return;
    }

    let area = frame.area();
    if area.width <= TOAST_HORIZONTAL_MARGIN * 2 + 8 || area.height <= TOAST_VERTICAL_MARGIN * 2 + 2
    {
        return;
    }

    let available_width = area.width.saturating_sub(TOAST_HORIZONTAL_MARGIN * 2);
    let max_toast_width = available_width.min(TOAST_MAX_WIDTH);
    let max_content_width = max_toast_width.saturating_sub(ACCENT_WIDTH + HORIZONTAL_PADDING * 2);
    if max_content_width == 0 {
        return;
    }
    let mut y = area.y.saturating_add(TOAST_VERTICAL_MARGIN);

    for toast in visible_toasts {
        let preferred_content_width = preferred_content_width(&toast.message, max_content_width);
        let min_content_width = TOAST_MIN_CONTENT_WIDTH.min(max_content_width).max(1);
        let content_width = preferred_content_width.max(min_content_width);
        let toast_width = content_width.saturating_add(ACCENT_WIDTH + HORIZONTAL_PADDING * 2);

        let mut wrapped_lines = wrap_message(&toast.message, content_width as usize);
        if wrapped_lines.len() > MAX_TEXT_LINES_PER_TOAST {
            wrapped_lines.truncate(MAX_TEXT_LINES_PER_TOAST);
            if let Some(last_line) = wrapped_lines.last_mut() {
                truncate_with_ellipsis(last_line, content_width as usize);
            }
        }

        let text_height = wrapped_lines.len().max(1) as u16;
        let toast_height = text_height + VERTICAL_PADDING * 2;

        let x = area.x.saturating_add(
            area.width
                .saturating_sub(toast_width)
                .saturating_sub(TOAST_HORIZONTAL_MARGIN),
        );

        let bottom = area.y.saturating_add(area.height);
        if y.saturating_add(toast_height) > bottom {
            break;
        }

        let toast_area = Rect {
            x,
            y,
            width: toast_width,
            height: toast_height,
        };

        let accent = toast.level.accent_color(colors);
        let background = tint_color(colors.dialog_background, accent, 0.14);

        frame.render_widget(Clear, toast_area);
        let body_area = Rect {
            x: toast_area.x.saturating_add(ACCENT_WIDTH),
            y: toast_area.y,
            width: toast_area.width.saturating_sub(ACCENT_WIDTH),
            height: toast_area.height,
        };
        if body_area.width > 0 {
            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(background)),
                body_area,
            );
        }

        let accent_area = Rect {
            x: toast_area.x,
            y: toast_area.y,
            width: ACCENT_WIDTH,
            height: toast_area.height,
        };
        if accent_area.width > 0 {
            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(accent)),
                accent_area,
            );
        }

        let text_area = Rect {
            x: toast_area.x + ACCENT_WIDTH + HORIZONTAL_PADDING,
            y: toast_area.y + VERTICAL_PADDING,
            width: content_width,
            height: text_height,
        };

        let lines: Vec<Line> = wrapped_lines
            .into_iter()
            .map(|line| Line::from(Span::styled(line, Style::default().fg(colors.text))))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(background)),
            text_area,
        );

        y = y.saturating_add(toast_height + TOAST_VERTICAL_SPACING);
    }
}

fn preferred_content_width(message: &str, max_content_width: u16) -> u16 {
    let widest_line = message
        .lines()
        .map(|line| line.width() as u16)
        .max()
        .unwrap_or(0);

    widest_line.max(1).min(max_content_width)
}

fn wrap_message(message: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for raw_line in message.lines() {
        if raw_line.trim().is_empty() {
            lines.push(String::new());
            continue;
        }

        for wrapped in textwrap::wrap(raw_line, max_width) {
            lines.push(wrapped.into_owned());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn truncate_with_ellipsis(line: &mut String, max_width: usize) {
    if max_width == 0 {
        line.clear();
        return;
    }

    if line.width() <= max_width {
        return;
    }

    let suffix = "...";
    let suffix_width = suffix.width();
    if suffix_width >= max_width {
        *line = ".".repeat(max_width);
        return;
    }

    let target = max_width.saturating_sub(suffix_width);
    let mut trimmed = String::new();
    for ch in line.chars() {
        let mut candidate = trimmed.clone();
        candidate.push(ch);
        if candidate.width() > target {
            break;
        }
        trimmed.push(ch);
    }

    trimmed.push_str(suffix);
    *line = trimmed;
}

fn tint_color(base: Color, accent: Color, amount: f32) -> Color {
    match (base, accent) {
        (Color::Rgb(br, bg, bb), Color::Rgb(ar, ag, ab)) => {
            let mix = |base: u8, accent: u8| -> u8 {
                let base = base as f32;
                let accent = accent as f32;
                (base + (accent - base) * amount).clamp(0.0, 255.0) as u8
            };

            Color::Rgb(mix(br, ar), mix(bg, ag), mix(bb, ab))
        }
        _ => base,
    }
}
