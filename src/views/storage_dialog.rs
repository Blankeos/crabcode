use crate::theme::{contrast_text, ThemeColors};
use crate::utils::storage::{format_bytes, StorageCategory, StorageReport, StorageRow};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

const STORAGE_CATEGORIES: [StorageCategory; 3] = [
    StorageCategory::PastedImages,
    StorageCategory::DataDb,
    StorageCategory::ModelsDevCache,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageDialogAction {
    None,
    Close,
    Refresh,
    Open(StorageCategory),
}

#[derive(Debug)]
pub struct StorageDialogState {
    visible: bool,
    selected_index: usize,
    checking: bool,
    report: Option<StorageReport>,
    error: Option<String>,
    dialog_area: Rect,
    rows_area: Rect,
}

impl StorageDialogState {
    pub fn new() -> Self {
        Self {
            visible: false,
            selected_index: 0,
            checking: false,
            report: None,
            error: None,
            dialog_area: Rect::default(),
            rows_area: Rect::default(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.selected_index = self
            .selected_index
            .min(STORAGE_CATEGORIES.len().saturating_sub(1));
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn has_report(&self) -> bool {
        self.report.is_some()
    }

    pub fn is_checking(&self) -> bool {
        self.checking
    }

    pub fn start_checking(&mut self) {
        self.checking = true;
        self.error = None;
    }

    pub fn set_report(&mut self, report: StorageReport) {
        self.report = Some(report);
        self.checking = false;
        self.error = None;
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.checking = false;
        self.error = Some(error.into());
    }

    pub fn open_path_for(&self, category: StorageCategory) -> Option<std::path::PathBuf> {
        self.report
            .as_ref()?
            .rows
            .iter()
            .find(|row| row.category == category)
            .and_then(|row| row.open_path.clone())
    }

    fn selected_category(&self) -> StorageCategory {
        STORAGE_CATEGORIES[self
            .selected_index
            .min(STORAGE_CATEGORIES.len().saturating_sub(1))]
    }

    fn next(&mut self) {
        if self.selected_index + 1 < STORAGE_CATEGORIES.len() {
            self.selected_index += 1;
        }
    }

    fn previous(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    fn select_row_at(&mut self, col: u16, row: u16) -> Option<StorageCategory> {
        if !self.rows_area.contains(Position::new(col, row)) {
            return None;
        }

        let index = row.saturating_sub(self.rows_area.y) as usize / 2;
        if index >= STORAGE_CATEGORIES.len() {
            return None;
        }

        self.selected_index = index;
        Some(STORAGE_CATEGORIES[index])
    }
}

impl Default for StorageDialogState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_storage_dialog() -> StorageDialogState {
    StorageDialogState::new()
}

pub fn handle_storage_dialog_key_event(
    state: &mut StorageDialogState,
    event: KeyEvent,
) -> StorageDialogAction {
    if !state.is_visible() {
        return StorageDialogAction::None;
    }

    match event.code {
        KeyCode::Esc => {
            state.hide();
            StorageDialogAction::Close
        }
        KeyCode::Enter => StorageDialogAction::Open(state.selected_category()),
        KeyCode::Up => {
            state.previous();
            StorageDialogAction::None
        }
        KeyCode::Down => {
            state.next();
            StorageDialogAction::None
        }
        KeyCode::Char('r') | KeyCode::Char('R') => StorageDialogAction::Refresh,
        _ => StorageDialogAction::None,
    }
}

pub fn handle_storage_dialog_mouse_event(
    state: &mut StorageDialogState,
    event: MouseEvent,
) -> StorageDialogAction {
    if !state.is_visible() {
        return StorageDialogAction::None;
    }

    match event.kind {
        MouseEventKind::ScrollUp => {
            state.previous();
            StorageDialogAction::None
        }
        MouseEventKind::ScrollDown => {
            state.next();
            StorageDialogAction::None
        }
        MouseEventKind::Down(MouseButton::Left) => state
            .select_row_at(event.column, event.row)
            .map(StorageDialogAction::Open)
            .unwrap_or(StorageDialogAction::None),
        _ => StorageDialogAction::None,
    }
}

pub fn render_storage_dialog(
    f: &mut Frame,
    state: &mut StorageDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    if !state.is_visible() {
        return;
    }

    let dialog_width = area.width.min(80);
    let dialog_height = area.height.min(16);
    state.dialog_area = Rect {
        x: area.x + area.width.saturating_sub(dialog_width) / 2,
        y: area.y + area.height.saturating_sub(dialog_height) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, state.dialog_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        state.dialog_area,
    );

    let content = Rect {
        x: state.dialog_area.x + 3,
        y: state.dialog_area.y + 1,
        width: state.dialog_area.width.saturating_sub(6),
        height: state.dialog_area.height.saturating_sub(2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(content);

    render_header(f, chunks[0], colors);
    render_summary(f, state, chunks[2], colors);

    state.rows_area = chunks[3];
    render_rows(f, state, chunks[3], colors);
    render_footer(f, chunks[5], colors);
}

fn render_header(f: &mut Frame, area: Rect, colors: ThemeColors) {
    let esc_width = 4;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(esc_width)])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Storage",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )])),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "esc",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(ratatui::layout::Alignment::Right),
        chunks[1],
    );
}

fn render_summary(f: &mut Frame, state: &StorageDialogState, area: Rect, colors: ThemeColors) {
    let text = if let Some(report) = &state.report {
        let mut text = format!("Total {}", format_bytes(report.total_bytes));
        if state.checking {
            text.push_str("  refreshing...");
        } else {
            text.push_str(&format!("  {}", checked_age(report.checked_at)));
        }
        text
    } else if state.checking {
        "Total checking...".to_string()
    } else if let Some(error) = &state.error {
        format!("Total unavailable: {}", error)
    } else {
        "Total not checked".to_string()
    };

    let style = if state.error.is_some() && state.report.is_none() {
        Style::default().fg(colors.error)
    } else {
        Style::default().fg(colors.text_weak)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(text, style)])),
        area,
    );
}

fn render_rows(f: &mut Frame, state: &StorageDialogState, area: Rect, colors: ThemeColors) {
    let mut lines = Vec::new();

    for (index, category) in STORAGE_CATEGORIES.iter().enumerate() {
        let row = state
            .report
            .as_ref()
            .and_then(|report| report.rows.iter().find(|row| row.category == *category));
        lines.extend(storage_row_lines(
            index,
            label_for_category(*category),
            row,
            state
                .report
                .as_ref()
                .map(|report| report.total_bytes)
                .unwrap_or(0),
            state.checking && state.report.is_none(),
            index == state.selected_index,
            area.width as usize,
            colors,
        ));
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn storage_row_lines(
    index: usize,
    fallback_label: &str,
    row: Option<&StorageRow>,
    total_bytes: u64,
    checking: bool,
    selected: bool,
    width: usize,
    colors: ThemeColors,
) -> Vec<Line<'static>> {
    let label = row
        .map(|row| row.label.clone())
        .unwrap_or_else(|| fallback_label.to_string());
    let detail = row
        .map(|row| row.detail.clone())
        .unwrap_or_else(|| "waiting for storage check".to_string());
    let size = row
        .map(|row| format_bytes(row.bytes))
        .unwrap_or_else(|| "-".to_string());
    let percent = row
        .map(|row| percent_of(row.bytes, total_bytes))
        .map(|percent| format!("{percent:>3}%"))
        .unwrap_or_else(|| {
            if checking {
                "...".to_string()
            } else {
                "--%".to_string()
            }
        });
    let meter = if let Some(row) = row {
        meter_text(row.bytes, total_bytes, 22)
    } else if checking {
        placeholder_meter_text("checking", 22)
    } else {
        placeholder_meter_text("not checked", 22)
    };

    let marker = if selected { ">" } else { " " };
    let right = format!("{}  {}", percent, pad_left(&size, 10));
    let left_budget = width.saturating_sub(right.width() + 2);
    let left = truncate(&format!("{marker} {label}"), left_budget);
    let first_gap = width.saturating_sub(left.width() + right.width());

    let detail_prefix = "  ";
    let detail_budget = width.saturating_sub(meter.width() + detail_prefix.width() + 2);
    let detail = truncate(&detail, detail_budget);
    let second_left = format!("{detail_prefix}{detail}");
    let second_gap = width.saturating_sub(second_left.width() + meter.width());

    vec![
        styled_storage_line(
            vec![
                Span::styled(left, Style::default().fg(colors.text)),
                Span::raw(" ".repeat(first_gap)),
                Span::styled(
                    right,
                    Style::default()
                        .fg(colors.text)
                        .add_modifier(Modifier::BOLD),
                ),
            ],
            selected,
            index,
            colors,
        ),
        styled_storage_line(
            vec![
                Span::styled(
                    second_left,
                    Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM),
                ),
                Span::raw(" ".repeat(second_gap)),
                Span::styled(
                    meter,
                    Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM),
                ),
            ],
            selected,
            index,
            colors,
        ),
    ]
}

fn styled_storage_line(
    mut spans: Vec<Span<'static>>,
    selected: bool,
    index: usize,
    colors: ThemeColors,
) -> Line<'static> {
    if selected {
        let fg = contrast_text(colors.primary);
        for span in &mut spans {
            span.style = span.style.fg(fg).bg(colors.primary);
        }
    } else if index % 2 == 1 {
        for span in &mut spans {
            span.style = span.style.bg(colors.background_element);
        }
    }

    Line::from(spans)
}

fn percent_of(bytes: u64, total_bytes: u64) -> u16 {
    if total_bytes == 0 {
        0
    } else {
        ((bytes as f64 / total_bytes as f64) * 100.0).round() as u16
    }
}

fn meter_text(bytes: u64, total_bytes: u64, width: usize) -> String {
    let percent = percent_of(bytes, total_bytes);
    let bar_width = width.saturating_sub(2).max(1);
    let filled = ((percent as usize * bar_width) + 50) / 100;
    let empty = bar_width.saturating_sub(filled.min(bar_width));
    format!(
        "[{}{}]",
        "#".repeat(filled.min(bar_width)),
        "-".repeat(empty)
    )
}

fn placeholder_meter_text(label: &str, width: usize) -> String {
    let inner_width = width.saturating_sub(2).max(1);
    let label = truncate(label, inner_width);
    let padding = inner_width.saturating_sub(label.width());
    format!("[{}{}]", label, "-".repeat(padding))
}

fn checked_age(checked_at: std::time::SystemTime) -> String {
    let elapsed = checked_at.elapsed().unwrap_or_default();
    if elapsed.as_secs() < 60 {
        "cached now".to_string()
    } else if elapsed.as_secs() < 3600 {
        format!("cached {}m ago", elapsed.as_secs() / 60)
    } else {
        format!("cached {}h ago", elapsed.as_secs() / 3600)
    }
}

fn render_footer(f: &mut Frame, area: Rect, colors: ThemeColors) {
    let spans = vec![
        Span::styled("Open", Style::default().fg(colors.text_weak)),
        Span::raw(" "),
        Span::styled(
            "enter",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Refresh", Style::default().fg(colors.text_weak)),
        Span::raw(" "),
        Span::styled(
            "r",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn label_for_category(category: StorageCategory) -> &'static str {
    match category {
        StorageCategory::PastedImages => "Pasted Images",
        StorageCategory::DataDb => "Data.db",
        StorageCategory::ModelsDevCache => "Models.dev Cache",
    }
}

fn truncate(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + char_width > max_width - 3 {
            break;
        }
        out.push(ch);
        width += char_width;
    }
    out.push_str("...");
    out
}

fn pad_left(text: &str, width: usize) -> String {
    let text_width = text.width();
    if text_width >= width {
        text.to_string()
    } else {
        format!("{}{}", " ".repeat(width - text_width), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_is_relative_to_total_tracked_bytes() {
        assert_eq!(meter_text(50, 200, 14), "[###---------]");
    }

    #[test]
    fn storage_dialog_keeps_cached_report_while_refreshing() {
        let mut state = StorageDialogState::new();
        state.set_report(StorageReport {
            rows: Vec::new(),
            total_bytes: 12,
            checked_at: std::time::SystemTime::now(),
        });
        state.start_checking();

        assert!(state.has_report());
        assert!(state.is_checking());
    }
}
