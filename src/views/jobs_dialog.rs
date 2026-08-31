use crate::theme::ThemeColors;
use crate::tools::process_registry::{JobKind, JobStatus, ProcessJobSnapshot, ProcessRegistry};
use crate::ui::components::dialog::{
    Dialog, DialogAction as FooterAction, DialogItem, DialogPosition,
};
use crate::ui::selection::{extract_selected_text, Selection};
use crate::views::sessions_dialog::session_loading_glyph;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{layout::Rect, Frame};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

const DETAIL_OUTPUT_CAP: usize = 32_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobsDialogAction {
    Close,
    Handled,
    NotHandled,
    /// Restore interactive PTY overlay for this job id
    FocusInteractive(String),
    /// Kill selected job
    Kill(String),
    /// Restart selected job (same id / command / cwd)
    Restart(String),
}

#[derive(Debug)]
pub struct JobsDialogState {
    pub dialog: Dialog,
    /// When Some, showing log detail for this task_id instead of list
    detail_task_id: Option<String>,
    pub detail_scroll: u16,
    detail_text: String,
    detail_lines: Vec<String>,
    detail_title: String,
    detail_command: String,
    detail_content_area: Rect,
    detail_stick_to_bottom: bool,
    pub selection: Selection,
}

impl JobsDialogState {
    pub fn new() -> Self {
        let dialog = Dialog::new("Jobs")
            .with_position(DialogPosition::Center)
            .with_actions(list_actions(None));
        Self {
            dialog,
            detail_task_id: None,
            detail_scroll: 0,
            detail_text: String::new(),
            detail_lines: Vec::new(),
            detail_title: String::new(),
            detail_command: String::new(),
            detail_content_area: Rect::default(),
            detail_stick_to_bottom: true,
            selection: Selection::new(),
        }
    }

    pub fn show(&mut self) {
        self.detail_task_id = None;
        self.detail_scroll = 0;
        self.detail_text.clear();
        self.detail_lines.clear();
        self.detail_command.clear();
        self.detail_stick_to_bottom = true;
        self.selection.clear();
        self.dialog.show();
    }

    pub fn hide(&mut self) {
        self.detail_task_id = None;
        self.detail_scroll = 0;
        self.detail_text.clear();
        self.detail_lines.clear();
        self.detail_command.clear();
        self.selection.clear();
        self.dialog.hide();
    }

    pub fn is_visible(&self) -> bool {
        self.dialog.is_visible()
    }

    pub fn is_detail_open(&self) -> bool {
        self.detail_task_id.is_some()
    }

    pub fn detail_task_id(&self) -> Option<&str> {
        self.detail_task_id.as_deref()
    }

    pub fn detail_content_area(&self) -> Rect {
        self.detail_content_area
    }

    pub fn selected_text(&self) -> Option<String> {
        if !self.selection.active {
            return None;
        }
        let lines: Vec<Line<'_>> = self
            .detail_lines
            .iter()
            .map(|s| Line::from(s.as_str()))
            .collect();
        extract_selected_text(&lines, &self.selection).filter(|t| !t.is_empty())
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn refresh_from_registry(&mut self, registry: &ProcessRegistry, spinner_frame: usize) {
        let snaps = registry.list_blocking();
        self.refresh_from_snapshots(&snaps, spinner_frame);
    }

    pub fn refresh_from_snapshots(&mut self, snaps: &[ProcessJobSnapshot], spinner_frame: usize) {
        let selected_id = self
            .dialog
            .get_selected()
            .map(|item| item.id.clone())
            .or_else(|| self.detail_task_id.clone());

        let mut items = Vec::with_capacity(snaps.len());
        for snap in snaps {
            let name = derive_job_name(&snap.description, &snap.command);
            let icon = job_status_icon(snap, spinner_frame);
            let elapsed = job_elapsed(snap);
            items.push(DialogItem {
                id: snap.id.clone(),
                name: format!("{icon}  {name}"),
                description: String::new(),
                group: String::new(),
                tip: Some(format_job_duration(elapsed)),
                provider_id: name,
                active: false,
            });
        }

        let selected = match selected_id.as_deref() {
            Some(id) => snaps.iter().find(|s| s.id == id),
            None => None,
        };
        self.dialog.set_items(items);
        self.dialog.actions = list_actions(selected);

        if let Some(id) = selected_id {
            let _ = self.dialog.select_item_by_id(&id);
        }
    }

    /// Re-fetch running job output while detail is open. Preserves scroll unless
    /// the view was stuck to the bottom.
    pub fn refresh_detail_output(&mut self, registry: &ProcessRegistry) -> bool {
        let Some(task_id) = self.detail_task_id.clone() else {
            return false;
        };
        let Some(snap) = registry.get_blocking(&task_id) else {
            return false;
        };
        if !matches!(snap.status, JobStatus::Running) {
            // Still refresh once so tip/status icons stay accurate if job just ended.
            self.open_detail_from_snapshot(&snap, registry);
            return false;
        }
        self.open_detail_from_snapshot(&snap, registry);
        true
    }

    fn open_detail(&mut self, registry: &ProcessRegistry) {
        let Some(item) = self.dialog.get_selected() else {
            return;
        };
        let id = item.id.clone();
        let Some(snap) = registry.get_blocking(&id) else {
            return;
        };
        self.open_detail_from_snapshot(&snap, registry);
    }

    fn open_detail_from_snapshot(&mut self, snap: &ProcessJobSnapshot, registry: &ProcessRegistry) {
        let was_stuck =
            self.detail_stick_to_bottom || self.detail_task_id.as_deref() != Some(snap.id.as_str());
        let prev_scroll = self.detail_scroll;

        // Never wait on the UI thread — a wait here freezes the whole app
        // (including Esc-to-close) until the timeout fires.
        let output = registry
            .output_blocking(&snap.id, None, Some(0))
            .map(|o| o.text)
            .unwrap_or_default();
        let text = truncate_detail(&output);
        let lines: Vec<String> = text.lines().map(str::to_string).collect();

        self.detail_task_id = Some(snap.id.clone());
        self.detail_title = derive_job_name(&snap.description, &snap.command);
        self.detail_command = snap.command.clone();
        self.detail_text = text;
        self.detail_lines = lines;
        if was_stuck {
            self.detail_scroll = u16::MAX;
            self.detail_stick_to_bottom = true;
        } else {
            self.detail_scroll = prev_scroll;
        }
        // Selection stays unless content shrank past it — keep simple and clear.
        if self.selection.active {
            let max_line = self.detail_lines.len().saturating_sub(1);
            if self.selection.start_line > max_line || self.selection.end_line > max_line {
                self.selection.clear();
            }
        }
        self.dialog.actions = detail_actions(snap);
    }

    fn close_detail(&mut self, registry: &ProcessRegistry, spinner_frame: usize) {
        self.detail_task_id = None;
        self.detail_scroll = 0;
        self.detail_text.clear();
        self.detail_lines.clear();
        self.detail_command.clear();
        self.detail_stick_to_bottom = true;
        self.selection.clear();
        self.refresh_from_registry(registry, spinner_frame);
    }

    fn selected_snapshot<'a>(
        &self,
        snaps: &'a [ProcessJobSnapshot],
    ) -> Option<&'a ProcessJobSnapshot> {
        let id = self.dialog.get_selected()?.id.clone();
        snaps.iter().find(|s| s.id == id)
    }
}

impl Default for JobsDialogState {
    fn default() -> Self {
        Self::new()
    }
}

/// Grok-style duration buckets: <10s → "1.2s"; <60 → "12s"; <60m → "1m5s"; else "1h2m".
pub fn format_job_duration(d: Duration) -> String {
    let total_secs = d.as_secs_f64();
    if total_secs < 10.0 {
        format!("{:.1}s", total_secs)
    } else if total_secs < 60.0 {
        format!("{}s", total_secs as u64)
    } else if total_secs < 3600.0 {
        let mins = (total_secs / 60.0) as u64;
        let secs = (total_secs % 60.0) as u64;
        format!("{mins}m{secs}s")
    } else {
        let hours = (total_secs / 3600.0) as u64;
        let mins = ((total_secs % 3600.0) / 60.0) as u64;
        format!("{hours}h{mins}m")
    }
}

/// Prefer agent description (short), else derive a title-cased name from the command.
pub fn derive_job_name(description: &str, command: &str) -> String {
    let d = description.trim();
    if !d.is_empty() {
        return truncate_words(d, 6);
    }
    derive_name_from_command(command)
}

fn derive_name_from_command(command: &str) -> String {
    let stripped = strip_env_assignments(command.trim());
    if stripped.is_empty() {
        return "Job".to_string();
    }

    let tokens: Vec<&str> = stripped.split_whitespace().collect();
    if tokens.is_empty() {
        return "Job".to_string();
    }

    let mut words: Vec<String> = Vec::new();
    let bin = file_stem(tokens[0]);
    words.push(titlecase_ascii(&bin));

    let mut i = 1usize;
    // Skip common package-manager glue: run / exec / x / npx wrappers already handled by bin.
    while i < tokens.len() && words.len() < 4 {
        let t = tokens[i];
        i += 1;
        if t.starts_with('-') {
            // Keep meaningful short flags only when alone would look empty.
            continue;
        }
        if matches!(
            t,
            "run" | "exec" | "cmd" | "command" | "--" | "yarn" | "pnpm" | "npx" | "bunx"
        ) {
            continue;
        }
        // Skip path-like / file args after we already have a couple words.
        if words.len() >= 2 && (t.contains('/') || t.contains('.')) {
            continue;
        }
        words.push(titlecase_ascii(t));
    }

    if words.is_empty() {
        "Job".to_string()
    } else {
        words.join(" ")
    }
}

fn strip_env_assignments(command: &str) -> String {
    let mut out = Vec::new();
    let mut skipping_env = true;
    for token in command.split_whitespace() {
        if skipping_env {
            if let Some((k, _)) = token.split_once('=') {
                if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    continue;
                }
            }
            skipping_env = false;
        }
        out.push(token);
    }
    out.join(" ")
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.rsplit('\\').next().unwrap_or(name);
    name.to_string()
}

fn titlecase_ascii(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_ascii_uppercase().to_string() + chars.as_str()
}

fn truncate_words(s: &str, max_words: usize) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() <= max_words {
        words.join(" ")
    } else {
        format!("{}…", words[..max_words].join(" "))
    }
}

fn job_elapsed(snap: &ProcessJobSnapshot) -> Duration {
    match snap.ended_at {
        Some(ended) => ended.saturating_duration_since(snap.started_at),
        None => snap.started_at.elapsed(),
    }
}

fn job_status_icon(snap: &ProcessJobSnapshot, spinner_frame: usize) -> String {
    match snap.status {
        JobStatus::Running => session_loading_glyph(spinner_frame).to_string(),
        JobStatus::Exited if snap.exit_code.unwrap_or(0) == 0 => "✓".to_string(),
        JobStatus::Exited | JobStatus::Failed | JobStatus::Killed => "✗".to_string(),
    }
}

/// Stable footer — always the same actions so height doesn't jump on selection.
fn list_actions(_selected: Option<&ProcessJobSnapshot>) -> Vec<FooterAction> {
    vec![
        FooterAction {
            key: "esc".into(),
            label: "close".into(),
        },
        FooterAction {
            key: "enter".into(),
            label: "view".into(),
        },
        FooterAction {
            key: "x".into(),
            label: "kill".into(),
        },
        FooterAction {
            key: "r".into(),
            label: "restart".into(),
        },
    ]
}

/// Stable footer for detail view (same count every time).
fn detail_actions(_snap: &ProcessJobSnapshot) -> Vec<FooterAction> {
    vec![
        FooterAction {
            key: "esc".into(),
            label: "back".into(),
        },
        FooterAction {
            key: "↑↓".into(),
            label: "scroll".into(),
        },
        FooterAction {
            key: "x".into(),
            label: "kill".into(),
        },
        FooterAction {
            key: "r".into(),
            label: "restart".into(),
        },
        FooterAction {
            key: "y".into(),
            label: "copy sel".into(),
        },
    ]
}

fn truncate_detail(text: &str) -> String {
    if text.len() <= DETAIL_OUTPUT_CAP {
        text.to_string()
    } else {
        let start = text.len() - DETAIL_OUTPUT_CAP;
        let start = text
            .char_indices()
            .find(|(i, _)| *i >= start)
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("…[truncated]\n{}", &text[start..])
    }
}

pub fn init_jobs_dialog() -> JobsDialogState {
    JobsDialogState::new()
}

pub fn render_jobs_dialog(
    f: &mut Frame,
    state: &mut JobsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    if state.detail_task_id.is_some() {
        render_detail(f, state, area, colors);
    } else {
        state.dialog.render(f, area, colors);
    }
}

fn render_detail(f: &mut Frame, state: &mut JobsDialogState, area: Rect, colors: ThemeColors) {
    let width = ((area.width as f32) * 0.9).round() as u16;
    let width = width.max(40).min(area.width.saturating_sub(2));
    let height = ((area.height as f32) * 0.85).round() as u16;
    let height = height.max(12).min(area.height.saturating_sub(1));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let panel = Rect {
        x,
        y,
        width,
        height,
    };
    // Keep for mouse hit-testing / selection bar placement.
    state.dialog.dialog_area = panel;

    f.render_widget(Clear, panel);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", state.detail_title))
        .border_style(Style::default().fg(colors.border))
        .style(Style::default().bg(colors.background));
    let inner = block.inner(panel);
    f.render_widget(block, panel);

    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(1), // $ command
            ratatui::layout::Constraint::Length(1), // blank
            ratatui::layout::Constraint::Min(3),    // output
            ratatui::layout::Constraint::Length(1), // footer
        ])
        .split(inner);

    let cmd_line = Line::from(Span::styled(
        format!("$ {}", state.detail_command),
        Style::default().fg(colors.text_weak),
    ));
    f.render_widget(Paragraph::new(cmd_line), chunks[0]);

    let content_area = chunks[2];
    state.detail_content_area = content_area;

    let max_scroll = state
        .detail_lines
        .len()
        .saturating_sub(content_area.height as usize);
    if state.detail_stick_to_bottom || state.detail_scroll == u16::MAX {
        state.detail_scroll = max_scroll as u16;
        state.detail_stick_to_bottom = true;
    } else if state.detail_scroll as usize > max_scroll {
        state.detail_scroll = max_scroll as u16;
    }

    let start = state.detail_scroll as usize;
    let end = (start + content_area.height as usize).min(state.detail_lines.len());
    let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
    for line_text in &state.detail_lines[start..end] {
        lines.push(Line::from(Span::styled(
            line_text.clone(),
            Style::default().fg(colors.text),
        )));
    }
    let lines = crate::ui::selection::apply_selection_to_lines_with_offset(
        lines,
        &state.selection,
        colors.accent,
        start,
    );

    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(colors.background))
            .wrap(Wrap { trim: false }),
        content_area,
    );

    // Match Dialog/models footer: label primary+bold, key text_weak+dim
    let footer_lines = state.dialog.footer_lines(chunks[3].width, colors);
    f.render_widget(Paragraph::new(footer_lines), chunks[3]);
}

fn detail_mouse_to_pos(
    mouse: MouseEvent,
    content_area: Rect,
    scroll: u16,
    lines: &[String],
) -> Option<(usize, usize)> {
    if mouse.column < content_area.x
        || mouse.row < content_area.y
        || mouse.column >= content_area.x.saturating_add(content_area.width)
        || mouse.row >= content_area.y.saturating_add(content_area.height)
    {
        return None;
    }
    let rel_row = (mouse.row - content_area.y) as usize;
    let rel_col = (mouse.column - content_area.x) as usize;
    let line_idx = scroll as usize + rel_row;
    if line_idx >= lines.len() {
        // Allow selecting past last line as end-of-last-line.
        let last = lines.len().saturating_sub(1);
        let col = lines.get(last).map(|l| l.width()).unwrap_or(0);
        return Some((last, col));
    }
    let line_width = lines[line_idx].width();
    Some((line_idx, rel_col.min(line_width)))
}

pub fn handle_jobs_dialog_key_event(
    state: &mut JobsDialogState,
    key: KeyEvent,
    registry: &ProcessRegistry,
    spinner_frame: usize,
) -> JobsDialogAction {
    if !state.is_visible() {
        return JobsDialogAction::NotHandled;
    }

    if state.detail_task_id.is_some() {
        return handle_detail_key(state, key, registry, spinner_frame);
    }

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => JobsDialogAction::Close,
        (KeyCode::Enter, _) => {
            state.open_detail(registry);
            JobsDialogAction::Handled
        }
        (KeyCode::Char('f'), _) => {
            if let Some(item) = state.dialog.get_selected() {
                let id = item.id.clone();
                if let Some(snap) = registry.get_blocking(&id) {
                    if matches!(snap.kind, JobKind::Interactive) {
                        return JobsDialogAction::FocusInteractive(id);
                    }
                }
            }
            JobsDialogAction::Handled
        }
        (KeyCode::Char('x'), _) => {
            if let Some(item) = state.dialog.get_selected() {
                let id = item.id.clone();
                if let Some(snap) = registry.get_blocking(&id) {
                    if matches!(snap.status, JobStatus::Running) {
                        return JobsDialogAction::Kill(id);
                    }
                }
            }
            JobsDialogAction::Handled
        }
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
            state.refresh_from_registry(registry, spinner_frame);
            JobsDialogAction::Handled
        }
        (KeyCode::Char('r'), _) => {
            if let Some(item) = state.dialog.get_selected() {
                return JobsDialogAction::Restart(item.id.clone());
            }
            JobsDialogAction::Handled
        }
        _ => {
            // Let Dialog handle navigation / search.
            if state.dialog.handle_key_event(key) {
                // Refresh footer actions for newly selected row.
                let snaps = registry.list_blocking();
                let selected = state.selected_snapshot(&snaps);
                state.dialog.actions = list_actions(selected);
                JobsDialogAction::Handled
            } else {
                JobsDialogAction::NotHandled
            }
        }
    }
}

fn handle_detail_key(
    state: &mut JobsDialogState,
    key: KeyEvent,
    registry: &ProcessRegistry,
    spinner_frame: usize,
) -> JobsDialogAction {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            state.close_detail(registry, spinner_frame);
            JobsDialogAction::Handled
        }
        (KeyCode::Char('f'), _) => {
            if let Some(id) = state.detail_task_id.clone() {
                if let Some(snap) = registry.get_blocking(&id) {
                    if matches!(snap.kind, JobKind::Interactive) {
                        return JobsDialogAction::FocusInteractive(id);
                    }
                }
            }
            JobsDialogAction::Handled
        }
        (KeyCode::Char('x'), _) => {
            if let Some(id) = state.detail_task_id.clone() {
                if let Some(snap) = registry.get_blocking(&id) {
                    if matches!(snap.status, JobStatus::Running) {
                        return JobsDialogAction::Kill(id);
                    }
                }
            }
            JobsDialogAction::Handled
        }
        (KeyCode::Char('r'), _) => {
            if let Some(id) = state.detail_task_id.clone() {
                return JobsDialogAction::Restart(id);
            }
            JobsDialogAction::Handled
        }
        (KeyCode::Char('y'), _) => {
            // App layer also handles yank via SelectionActionTarget; treat as handled
            // so Dialog doesn't eat it. Actual copy is done by App when selection exists.
            JobsDialogAction::Handled
        }
        (KeyCode::Up | KeyCode::Char('k'), _) => {
            state.detail_scroll = state.detail_scroll.saturating_sub(1);
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        (KeyCode::Down | KeyCode::Char('j'), _) => {
            state.detail_scroll = state.detail_scroll.saturating_add(1);
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        (KeyCode::PageUp, _) => {
            let page = state.detail_content_area.height.max(1);
            state.detail_scroll = state.detail_scroll.saturating_sub(page);
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        (KeyCode::PageDown, _) => {
            let page = state.detail_content_area.height.max(1);
            state.detail_scroll = state.detail_scroll.saturating_add(page);
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        (KeyCode::Home, _) => {
            state.detail_scroll = 0;
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        (KeyCode::End, _) => {
            state.detail_scroll = u16::MAX;
            state.detail_stick_to_bottom = true;
            JobsDialogAction::Handled
        }
        _ => JobsDialogAction::NotHandled,
    }
}

pub fn handle_jobs_dialog_mouse_event(
    state: &mut JobsDialogState,
    mouse: MouseEvent,
    registry: &ProcessRegistry,
    spinner_frame: usize,
) -> JobsDialogAction {
    if !state.is_visible() {
        return JobsDialogAction::NotHandled;
    }

    if state.detail_task_id.is_some() {
        return handle_detail_mouse(state, mouse, registry, spinner_frame);
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left)
        | MouseEventKind::ScrollDown
        | MouseEventKind::ScrollUp => {
            if state.dialog.handle_mouse_event(mouse) {
                let snaps = registry.list_blocking();
                let selected = state.selected_snapshot(&snaps);
                state.dialog.actions = list_actions(selected);
                // Double-click / activate via dialog enter path is key-only;
                // single click just selects.
                JobsDialogAction::Handled
            } else {
                JobsDialogAction::NotHandled
            }
        }
        _ => JobsDialogAction::NotHandled,
    }
}

fn handle_detail_mouse(
    state: &mut JobsDialogState,
    mouse: MouseEvent,
    registry: &ProcessRegistry,
    spinner_frame: usize,
) -> JobsDialogAction {
    let content = state.detail_content_area;
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            state.detail_scroll = state.detail_scroll.saturating_sub(3);
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        MouseEventKind::ScrollDown => {
            state.detail_scroll = state.detail_scroll.saturating_add(3);
            state.detail_stick_to_bottom = false;
            JobsDialogAction::Handled
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some((line, col)) =
                detail_mouse_to_pos(mouse, content, state.detail_scroll, &state.detail_lines)
            {
                state.selection.start(line, col);
                JobsDialogAction::Handled
            } else {
                // Click outside content clears selection / ignores.
                if state.selection.active {
                    state.selection.clear();
                    JobsDialogAction::Handled
                } else {
                    JobsDialogAction::NotHandled
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if state.selection.is_dragging {
                if let Some((line, col)) =
                    detail_mouse_to_pos(mouse, content, state.detail_scroll, &state.detail_lines)
                {
                    state.selection.extend(line, col);
                }
                JobsDialogAction::Handled
            } else {
                JobsDialogAction::NotHandled
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if state.selection.is_dragging {
                state.selection.finish();
                // App shows the floating action bar when selection is non-empty.
                JobsDialogAction::Handled
            } else {
                JobsDialogAction::NotHandled
            }
        }
        _ => {
            let _ = (registry, spinner_frame);
            JobsDialogAction::NotHandled
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn format_job_duration_buckets() {
        assert_eq!(format_job_duration(Duration::from_millis(1200)), "1.2s");
        assert_eq!(format_job_duration(Duration::from_secs(12)), "12s");
        assert_eq!(format_job_duration(Duration::from_secs(65)), "1m5s");
        assert_eq!(format_job_duration(Duration::from_secs(3725)), "1h2m");
    }

    #[test]
    fn derive_job_name_prefers_description() {
        assert_eq!(
            derive_job_name("Dev Server Hot Reload", "bun run dev --port 3000"),
            "Dev Server Hot Reload"
        );
        assert_eq!(
            derive_job_name("one two three four five six seven", "ignored"),
            "one two three four five six…"
        );
    }

    #[test]
    fn derive_job_name_from_command() {
        assert_eq!(derive_job_name("", "bun run dev"), "Bun Dev");
        assert_eq!(derive_job_name("", "npm run build --watch"), "Npm Build");
        assert_eq!(derive_job_name("", "cargo test"), "Cargo Test");
        assert_eq!(
            derive_job_name("", "FOO=1 BAR=2 bun run start"),
            "Bun Start"
        );
    }

    #[test]
    fn init_uses_center_position() {
        let state = init_jobs_dialog();
        assert!(matches!(state.dialog.position, DialogPosition::Center));
        assert_eq!(state.dialog.title, "Jobs");
    }

    #[test]
    fn footer_actions_include_restart() {
        assert!(list_actions(None)
            .iter()
            .any(|a| a.key == "r" && a.label == "restart"));
        let snap = ProcessJobSnapshot {
            id: "job_test".into(),
            kind: JobKind::Background,
            command: "sleep 1".into(),
            description: "n".into(),
            workdir: std::path::PathBuf::from("/tmp"),
            status: JobStatus::Running,
            exit_code: None,
            started_at: std::time::Instant::now(),
            ended_at: None,
            bytes_total: 0,
            truncated: false,
        };
        assert!(detail_actions(&snap)
            .iter()
            .any(|a| a.key == "r" && a.label == "restart"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refresh_builds_flat_items_with_duration_tip() {
        let _state = crate::jobs::test_env::TempState::new();
        let workdir = tempfile::tempdir().expect("workdir");
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());
        let spawned = registry
            .spawn_background(
                "echo jobs_dialog_test",
                "echo test",
                workdir.path(),
                None,
                CancellationToken::new(),
            )
            .await
            .expect("spawn");

        for _ in 0..40 {
            let out = registry
                .output(&spawned.task_id, Some(50), Some(0))
                .await
                .expect("output");
            if out.status.is_terminal() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let mut state = init_jobs_dialog();
        let snaps = registry.list().await;
        state.refresh_from_snapshots(&snaps, 0);
        let item = state
            .dialog
            .items
            .iter()
            .find(|item| item.id == spawned.task_id)
            .expect("expected spawned job in dialog items");
        assert!(
            item.group.is_empty(),
            "jobs list must be flat (empty group)"
        );
        assert!(
            item.active == false,
            "active must be false to avoid ● prefix"
        );
        let tip = item.tip.as_deref().unwrap_or("");
        assert!(
            tip.ends_with('s') || tip.contains('m') || tip.contains('h'),
            "tip should be duration-only, got {tip:?}"
        );
        assert!(
            !tip.to_lowercase().contains("running") && !tip.to_lowercase().contains("exited"),
            "tip must omit status words: {tip:?}"
        );
        assert!(
            item.name.contains('✓')
                || item.name.contains('✗')
                || item.name.contains('⠋')
                || item.name.contains('·')
                || item
                    .name
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_ascii_alphanumeric()),
            "name should start with status icon, got {:?}",
            item.name
        );
    }
}
