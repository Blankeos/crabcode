use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ratatui::layout::{Position, Rect};
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

    fn is_copyable(self) -> bool {
        matches!(self, ToastLevel::Error | ToastLevel::Warning)
    }
}

/// Ephemeral toast shown when a cached version check finds a newer release.
/// Clicking it explicitly starts `crabcode upgrade`. Keep exact.
pub const UPDATE_AVAILABLE_MESSAGE: &str = "New version available · Upgrade";

/// Toast shown after `crabcode upgrade` finishes. Keep exact.
pub const UPDATED_MESSAGE: &str = "Updated · Restart to apply";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastAction {
    /// Clicking the toast runs `crabcode upgrade` (no confirm, no auto-install).
    Upgrade,
}

#[derive(Debug, Clone)]
pub struct Toast {
    message: String,
    level: ToastLevel,
    expires_at: Instant,
    action: Option<ToastAction>,
}

impl Toast {
    pub fn new(message: impl Into<String>, level: ToastLevel, duration: Option<Duration>) -> Self {
        let duration = duration.unwrap_or(DEFAULT_TOAST_DURATION);
        Self {
            message: message.into(),
            level,
            expires_at: Instant::now() + duration,
            action: None,
        }
    }

    pub fn with_action(mut self, action: ToastAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Ephemeral update nudge. Info level so it never becomes copyable text.
    pub fn update_available() -> Self {
        Self::new(UPDATE_AVAILABLE_MESSAGE, ToastLevel::Info, None)
            .with_action(ToastAction::Upgrade)
    }

    /// Exact success toast after `crabcode upgrade` completes.
    pub fn updated() -> Self {
        Self::new(UPDATED_MESSAGE, ToastLevel::Success, None)
    }

    /// Upgrade failure toast (Error level, full message preserved for copy).
    pub fn upgrade_failed(message: impl Into<String>) -> Self {
        Self::new(message.into(), ToastLevel::Error, None)
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn level(&self) -> ToastLevel {
        self.level
    }

    pub fn action(&self) -> Option<ToastAction> {
        self.action
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

    /// Drop expired toasts. Returns true when anything was removed so the
    /// event loop can schedule a redraw — otherwise the last toast frame
    /// stays painted with a dead hitbox (layout already filters expired).
    pub fn remove_expired(&mut self) -> bool {
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|toast| !toast.is_expired(now));
        self.toasts.len() != before
    }

    /// How long until the next toast expires. `None` when no toasts are
    /// queued. The idle event loop caps its blocking `poll()` at this
    /// duration so expiry wakes exactly one redraw (no 60fps animation).
    pub fn time_until_next_expiry(&self) -> Option<Duration> {
        let now = Instant::now();
        self.toasts
            .iter()
            .filter_map(|toast| toast.expires_at.checked_duration_since(now))
            .min()
    }

    /// True when any toast is currently visible (used to bound idle wakeups).
    pub fn has_visible_toasts(&self) -> bool {
        let now = Instant::now();
        self.toasts.iter().any(|toast| !toast.is_expired(now))
    }

    pub fn copyable_message_at(
        &self,
        frame: Rect,
        position: Position,
    ) -> Option<(String, ToastLevel)> {
        layout_visible_toasts(frame, self, Instant::now())
            .into_iter()
            .find(|laid| laid.toast.level.is_copyable() && laid.area.contains(position))
            .map(|laid| (laid.toast.message.clone(), laid.toast.level))
    }

    /// Hit-test for actionable toasts (e.g. the Upgrade nudge). Matches the
    /// whole toast area: the entire toast is the Upgrade affordance.
    pub fn action_at(&self, frame: Rect, position: Position) -> Option<(ToastAction, String)> {
        layout_visible_toasts(frame, self, Instant::now())
            .into_iter()
            .find(|laid| laid.toast.action.is_some() && laid.area.contains(position))
            .and_then(|laid| {
                laid.toast
                    .action
                    .map(|action| (action, laid.toast.message.clone()))
            })
    }

    /// Dismiss toasts carrying the given action (used to retire the Upgrade
    /// nudge the moment its upgrade starts, preventing double-runs).
    pub fn remove_action(&mut self, action: ToastAction) {
        self.toasts.retain(|toast| toast.action != Some(action));
    }
}

struct LaidOutToast<'a> {
    toast: &'a Toast,
    area: Rect,
    wrapped_lines: Vec<String>,
    content_width: u16,
}

fn layout_visible_toasts<'a>(
    area: Rect,
    manager: &'a ToastManager,
    now: Instant,
) -> Vec<LaidOutToast<'a>> {
    let visible_toasts: Vec<&Toast> = manager
        .toasts
        .iter()
        .rev()
        .filter(|toast| !toast.is_expired(now))
        .take(MAX_VISIBLE_TOASTS)
        .collect();

    if visible_toasts.is_empty() {
        return Vec::new();
    }

    if area.width <= TOAST_HORIZONTAL_MARGIN * 2 + 8 || area.height <= TOAST_VERTICAL_MARGIN * 2 + 2
    {
        return Vec::new();
    }

    let available_width = area.width.saturating_sub(TOAST_HORIZONTAL_MARGIN * 2);
    let max_toast_width = available_width.min(TOAST_MAX_WIDTH);
    let max_content_width = max_toast_width.saturating_sub(ACCENT_WIDTH + HORIZONTAL_PADDING * 2);
    if max_content_width == 0 {
        return Vec::new();
    }

    let mut y = area.y.saturating_add(TOAST_VERTICAL_MARGIN);
    let mut laid_out = Vec::new();
    let bottom = area.y.saturating_add(area.height);

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
        if y.saturating_add(toast_height) > bottom {
            break;
        }

        let x = area.x.saturating_add(
            area.width
                .saturating_sub(toast_width)
                .saturating_sub(TOAST_HORIZONTAL_MARGIN),
        );

        laid_out.push(LaidOutToast {
            toast,
            area: Rect {
                x,
                y,
                width: toast_width,
                height: toast_height,
            },
            wrapped_lines,
            content_width,
        });

        y = y.saturating_add(toast_height + TOAST_VERTICAL_SPACING);
    }

    laid_out
}

pub fn render_toasts(frame: &mut Frame, manager: &ToastManager, colors: &ThemeColors) {
    let area = frame.area();
    for laid in layout_visible_toasts(area, manager, Instant::now()) {
        let accent = laid.toast.level.accent_color(colors);
        let background = tint_color(colors.dialog_background, accent, 0.14);

        frame.render_widget(Clear, laid.area);
        let body_area = Rect {
            x: laid.area.x.saturating_add(ACCENT_WIDTH),
            y: laid.area.y,
            width: laid.area.width.saturating_sub(ACCENT_WIDTH),
            height: laid.area.height,
        };
        if body_area.width > 0 {
            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(background)),
                body_area,
            );
        }

        let accent_area = Rect {
            x: laid.area.x,
            y: laid.area.y,
            width: ACCENT_WIDTH,
            height: laid.area.height,
        };
        if accent_area.width > 0 {
            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(accent)),
                accent_area,
            );
        }

        let text_height = laid.wrapped_lines.len().max(1) as u16;
        let text_area = Rect {
            x: laid.area.x + ACCENT_WIDTH + HORIZONTAL_PADDING,
            y: laid.area.y + VERTICAL_PADDING,
            width: laid.content_width,
            height: text_height,
        };

        let lines: Vec<Line> = laid
            .wrapped_lines
            .into_iter()
            .map(|line| Line::from(Span::styled(line, Style::default().fg(colors.text))))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(background)),
            text_area,
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    fn long_lived(message: &str, level: ToastLevel) -> Toast {
        Toast::new(message, level, Some(Duration::from_secs(60)))
    }

    fn copyable_areas(manager: &ToastManager) -> Vec<(Rect, String)> {
        layout_visible_toasts(frame(), manager, Instant::now())
            .into_iter()
            .filter(|laid| laid.toast.level.is_copyable())
            .map(|laid| (laid.area, laid.toast.message.clone()))
            .collect()
    }

    #[test]
    fn clicking_error_toast_returns_full_original_message() {
        let mut manager = ToastManager::new();
        let message = "LLM error: provider exploded\n".repeat(20);
        manager.add(long_lived(&message, ToastLevel::Error));

        let areas = copyable_areas(&manager);
        assert_eq!(areas.len(), 1);
        let (area, _) = &areas[0];
        assert!(
            area.height <= VERTICAL_PADDING * 2 + MAX_TEXT_LINES_PER_TOAST as u16,
            "displayed toast should stay truncated"
        );

        let copied = manager.copyable_message_at(frame(), Position::new(area.x, area.y));
        let copied = copied.map(|(message, _)| message);
        assert_eq!(copied.as_deref(), Some(message.as_str()));
    }

    #[test]
    fn clicking_warning_toast_returns_full_original_message() {
        let mut manager = ToastManager::new();
        let message = "warning: rate limit approaching\n".repeat(20);
        manager.add(long_lived(&message, ToastLevel::Warning));

        let areas = copyable_areas(&manager);
        assert_eq!(areas.len(), 1);
        let (area, _) = &areas[0];

        let copied = manager.copyable_message_at(frame(), Position::new(area.x, area.y));
        assert!(copied.is_some());
        let (copied_message, copied_level) = copied.unwrap();
        assert_eq!(copied_message, message);
        assert_eq!(copied_level, ToastLevel::Warning);
    }

    #[test]
    fn info_toasts_are_not_copyable() {
        let mut manager = ToastManager::new();
        manager.add(long_lived("Copied to clipboard", ToastLevel::Info));

        let laid = layout_visible_toasts(frame(), &manager, Instant::now());
        assert_eq!(laid.len(), 1);
        assert!(manager
            .copyable_message_at(frame(), Position::new(laid[0].area.x, laid[0].area.y))
            .is_none());
    }

    #[test]
    fn click_outside_toast_does_not_copy() {
        let mut manager = ToastManager::new();
        manager.add(long_lived("boom", ToastLevel::Error));

        assert!(manager
            .copyable_message_at(frame(), Position::new(0, 0))
            .is_none());
    }

    #[test]
    fn newest_error_toast_wins_when_stacked() {
        let mut manager = ToastManager::new();
        manager.add(long_lived("older error", ToastLevel::Error));
        manager.add(long_lived("newer error", ToastLevel::Error));

        let areas = copyable_areas(&manager);
        assert_eq!(areas.len(), 2);
        assert_eq!(areas[0].1, "newer error");
        assert_eq!(
            manager
                .copyable_message_at(frame(), Position::new(areas[0].0.x, areas[0].0.y))
                .map(|(message, _)| message)
                .as_deref(),
            Some("newer error")
        );
        assert_eq!(
            manager
                .copyable_message_at(frame(), Position::new(areas[1].0.x, areas[1].0.y))
                .map(|(message, _)| message)
                .as_deref(),
            Some("older error")
        );
    }

    #[test]
    fn update_available_toast_has_exact_message_and_upgrade_action() {
        let toast = Toast::update_available();
        assert_eq!(toast.message(), "New version available · Upgrade");
        assert_eq!(toast.message(), UPDATE_AVAILABLE_MESSAGE);
        assert_eq!(toast.action(), Some(ToastAction::Upgrade));
        assert_eq!(toast.level(), ToastLevel::Info);
    }

    #[test]
    fn updated_toast_has_exact_success_message() {
        let toast = Toast::updated();
        assert_eq!(toast.message(), "Updated · Restart to apply");
        assert_eq!(toast.message(), UPDATED_MESSAGE);
        assert_eq!(toast.level(), ToastLevel::Success);
        assert_eq!(toast.action(), None);
    }

    #[test]
    fn upgrade_failed_toast_is_error_level() {
        let toast = Toast::upgrade_failed("Update failed: boom");
        assert_eq!(toast.level(), ToastLevel::Error);
        assert_eq!(toast.action(), None);
        assert!(toast.message().starts_with("Update failed:"));
    }

    #[test]
    fn clicking_update_toast_returns_upgrade_action() {
        let mut manager = ToastManager::new();
        manager.add(Toast::update_available());

        let laid = layout_visible_toasts(frame(), &manager, Instant::now());
        assert_eq!(laid.len(), 1);
        let area = laid[0].area;

        let hit = manager.action_at(frame(), Position::new(area.x, area.y));
        assert!(hit.is_some());
        let (action, message) = hit.unwrap();
        assert_eq!(action, ToastAction::Upgrade);
        assert_eq!(message, UPDATE_AVAILABLE_MESSAGE);
    }

    #[test]
    fn update_toast_is_not_copyable() {
        // The Upgrade nudge is Info level, so the error-copy handler ignores it
        // and the update-click handler owns the hit area.
        let mut manager = ToastManager::new();
        manager.add(Toast::update_available());

        let laid = layout_visible_toasts(frame(), &manager, Instant::now());
        assert_eq!(laid.len(), 1);
        assert!(manager
            .copyable_message_at(frame(), Position::new(laid[0].area.x, laid[0].area.y))
            .is_none());
    }

    #[test]
    fn click_outside_update_toast_has_no_action() {
        let mut manager = ToastManager::new();
        manager.add(Toast::update_available());
        assert!(manager.action_at(frame(), Position::new(0, 0)).is_none());
    }

    #[test]
    fn remove_action_dismisses_only_upgrade_toasts() {
        let mut manager = ToastManager::new();
        manager.add(Toast::update_available());
        manager.add(long_lived("plain info", ToastLevel::Info));

        manager.remove_action(ToastAction::Upgrade);

        let laid = layout_visible_toasts(frame(), &manager, Instant::now());
        assert_eq!(laid.len(), 1);
        assert_eq!(laid[0].toast.message(), "plain info");
        assert!(manager
            .action_at(frame(), Position::new(laid[0].area.x, laid[0].area.y))
            .is_none());
    }

    #[test]
    fn remove_expired_reports_whether_redraw_is_needed() {
        let mut manager = ToastManager::new();
        assert!(!manager.remove_expired(), "empty queue needs no redraw");
        manager.add(long_lived("fresh", ToastLevel::Info));
        assert!(
            !manager.remove_expired(),
            "fresh toast must not trigger expiry redraw"
        );
        manager.add(Toast::new("gone", ToastLevel::Info, Some(Duration::ZERO)));
        // Zero-duration toast is already expired at `Instant::now()`.
        assert!(
            manager.remove_expired(),
            "expired toast must request exactly one redraw"
        );
        assert!(!manager.remove_expired(), "second sweep is a no-op");
    }

    #[test]
    fn time_until_next_expiry_bounds_idle_wakeup() {
        let mut manager = ToastManager::new();
        assert_eq!(manager.time_until_next_expiry(), None);
        manager.add(Toast::new(
            "short",
            ToastLevel::Info,
            Some(Duration::from_secs(4)),
        ));
        let until = manager.time_until_next_expiry().expect("toast pending");
        assert!(until <= Duration::from_secs(4) && !until.is_zero());
        assert!(manager.has_visible_toasts());
        manager.add(Toast::new("gone", ToastLevel::Info, Some(Duration::ZERO)));
        // Expired entries are ignored for wakeups (they need an immediate
        // redraw instead, via `remove_expired`).
        let until2 = manager
            .time_until_next_expiry()
            .expect("fresh still pending");
        assert!(until2 <= Duration::from_secs(4));
    }
}
