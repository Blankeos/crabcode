use std::collections::HashMap;
use std::path::PathBuf;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::skill::SkillStore;
use crate::theme::{contrast_text, ThemeColors};
use crate::ui::components::dialog::{dim_backdrop, Dialog, DialogAction, DialogItem};
use crate::ui::components::file_actions::{reveal_label, FileAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsDialogAction {
    Toggle {
        skill_id: String,
    },
    /// Insert `@name` at the chat cursor. The dialog closes; the input is kept.
    InsertMention {
        name: String,
    },
    /// Open, reveal, or copy a skill file. Uses the same file-action config as chat links.
    File {
        action: FileAction,
        path: PathBuf,
    },
    None,
}

#[derive(Debug)]
pub struct SkillsDialogState {
    pub dialog: Dialog,
    locations: HashMap<String, PathBuf>,
    source: Option<SkillSource>,
}

#[derive(Debug)]
struct SkillSource {
    path: PathBuf,
    selected: usize,
    area: Rect,
    esc_area: Rect,
    action_rows: [Rect; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceEvent {
    None,
    Close,
    Choose(FileAction),
}

const SOURCE_ACTIONS: [FileAction; 3] = [FileAction::Open, FileAction::Reveal, FileAction::Copy];

impl SkillsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog: dialog.with_actions(list_actions()),
            locations: HashMap::new(),
            source: None,
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self::new(Dialog::with_items(title, items))
    }

    /// Update activation labels without losing search, selection, or scroll.
    pub fn refresh(&mut self, store: &SkillStore) {
        let installed = store.installed();
        self.locations = installed
            .iter()
            .map(|skill| (skill.name.clone(), skill.location.clone()))
            .collect();
        let enabled = store.all();
        let tokens = crate::prompt::estimate_skill_catalog_tokens(enabled.iter().copied());
        self.dialog.title = "Skills".to_string();
        self.dialog.title_note = skill_title_note(enabled.len(), installed.len(), tokens);
        self.dialog.set_items_preserve_ui(skill_dialog_items(store));
    }

    pub fn info_is_open(&self) -> bool {
        self.source.is_some()
    }
}

impl SkillSource {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            selected: 0,
            area: Rect::default(),
            esc_area: Rect::default(),
            action_rows: [Rect::default(); 3],
        }
    }

    fn handle_key(&mut self, event: KeyEvent) -> SourceEvent {
        match event.code {
            KeyCode::Esc => SourceEvent::Close,
            KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                self.selected = (self.selected + 2) % 3;
                SourceEvent::None
            }
            KeyCode::Tab if event.modifiers.contains(KeyModifiers::SHIFT) => {
                self.selected = (self.selected + 2) % 3;
                SourceEvent::None
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                self.selected = (self.selected + 1) % 3;
                SourceEvent::None
            }
            KeyCode::Home => {
                self.selected = 0;
                SourceEvent::None
            }
            KeyCode::End => {
                self.selected = 2;
                SourceEvent::None
            }
            KeyCode::Enter | KeyCode::Char(' ') if event.modifiers == KeyModifiers::NONE => {
                SourceEvent::Choose(SOURCE_ACTIONS[self.selected])
            }
            KeyCode::Char(ch) if event.modifiers == KeyModifiers::NONE => source_action_for_key(ch)
                .map(SourceEvent::Choose)
                .unwrap_or(SourceEvent::None),
            _ => SourceEvent::None,
        }
    }

    fn action_at(&self, position: Position) -> Option<FileAction> {
        self.action_rows
            .iter()
            .enumerate()
            .find_map(|(index, row)| row.contains(position).then_some(SOURCE_ACTIONS[index]))
    }

    fn render(&mut self, frame: &mut Frame, screen: Rect, name: &str, colors: ThemeColors) {
        let width = screen.width.min(70).max(1);
        let pad_x = 3.min(width / 4);
        let inner_width = width.saturating_sub(pad_x.saturating_mul(2)).max(1);
        let path_lines = wrap_path(&self.path.display().to_string(), inner_width as usize);
        let path_height = path_lines.len().max(1) as u16;
        // pad, title, gap, path, gap, three actions, pad
        let height = (8 + path_height).min(screen.height).max(1);
        let area = Rect {
            x: screen.x + screen.width.saturating_sub(width) / 2,
            y: screen.y + screen.height.saturating_sub(height) / 2,
            width,
            height,
        };
        self.area = area;
        dim_backdrop(frame, screen, area);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
            area,
        );

        let content = Rect {
            x: area.x + pad_x.min(area.width.saturating_sub(1)),
            y: area.y + 1.min(area.height.saturating_sub(1)),
            width: inner_width.min(area.width),
            height: area.height.saturating_sub(2).max(1),
        };
        let name = truncate_to_width(name, content.width.saturating_sub(4) as usize);
        let gap = (content.width as usize)
            .saturating_sub(name.width())
            .saturating_sub("esc".width());
        let title = Line::from(vec![
            Span::styled(
                name,
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(
                "esc",
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(title),
            Rect::new(content.x, content.y, content.width, 1),
        );
        self.esc_area = Rect::new(
            content.right().saturating_sub(3),
            content.y,
            3.min(content.width),
            1,
        );

        let path_area = Rect::new(
            content.x,
            content.y.saturating_add(2),
            content.width,
            path_height.min(content.height.saturating_sub(2)),
        );
        let path_style = Style::default()
            .fg(colors.text_weak)
            .bg(colors.dialog_background);
        frame.render_widget(
            Paragraph::new(
                path_lines
                    .into_iter()
                    .map(|line| Line::from(Span::styled(line, path_style)))
                    .collect::<Vec<_>>(),
            ),
            path_area,
        );

        let actions_y = path_area.bottom().saturating_add(1);
        let base = Style::default()
            .fg(colors.text)
            .bg(colors.dialog_background);
        let selected_fg = contrast_text(colors.primary);
        for (index, action) in SOURCE_ACTIONS.iter().enumerate() {
            let row = Rect::new(
                content.x,
                actions_y.saturating_add(index as u16),
                content.width,
                1,
            );
            self.action_rows[index] = row;
            if row.y >= area.bottom() {
                continue;
            }
            let style = if index == self.selected {
                Style::default()
                    .fg(selected_fg)
                    .bg(colors.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                base
            };
            let line = Line::from(vec![
                Span::styled(
                    source_key(*action).to_string(),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {} ", source_label(*action)), style),
            ]);
            frame.render_widget(Paragraph::new(line).style(style), row);
        }
    }
}

/// `25` when every installed skill is enabled, `23/25` otherwise.
/// The token figure is the enabled catalog only, so a fully disabled list
/// shows no estimate.
fn skill_title_note(enabled: usize, installed: usize, tokens: usize) -> String {
    let count = if enabled == installed {
        installed.to_string()
    } else {
        format!("{enabled}/{installed}")
    };
    if tokens == 0 {
        count
    } else {
        format!("{count}  {}", format_tokens(tokens))
    }
}

fn format_tokens(tokens: usize) -> String {
    if tokens < 1_000 {
        return format!("~{tokens}");
    }
    let tenths = tokens.saturating_add(50) / 100;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        format!("~{whole}k")
    } else {
        format!("~{whole}.{frac}k")
    }
}

fn list_actions() -> Vec<DialogAction> {
    vec![
        DialogAction {
            label: "toggle".to_string(),
            key: "space".to_string(),
        },
        DialogAction {
            label: "insert".to_string(),
            key: "enter".to_string(),
        },
        DialogAction {
            label: "info".to_string(),
            key: "ctrl+i".to_string(),
        },
        DialogAction {
            label: "close".to_string(),
            key: "esc".to_string(),
        },
    ]
}

fn skill_dialog_items(store: &SkillStore) -> Vec<DialogItem> {
    store
        .installed()
        .into_iter()
        .map(|skill| {
            let enabled = store.is_enabled(&skill.name);
            DialogItem {
                id: skill.name.clone(),
                name: skill.name.clone(),
                group: "Skills".to_string(),
                description: skill
                    .description
                    .clone()
                    .unwrap_or_else(|| "No description".to_string()),
                tip: Some(if enabled { "enabled" } else { "disabled" }.to_string()),
                provider_id: String::new(),
                active: enabled,
            }
        })
        .collect()
}

fn is_info_key(event: KeyEvent) -> bool {
    event.modifiers.contains(KeyModifiers::CONTROL)
        && !event.modifiers.contains(KeyModifiers::ALT)
        && matches!(event.code, KeyCode::Char('i') | KeyCode::Char('I'))
}

fn source_action_for_key(ch: char) -> Option<FileAction> {
    match ch {
        'o' | 'O' => Some(FileAction::Open),
        'r' | 'R' => Some(FileAction::Reveal),
        'y' | 'Y' => Some(FileAction::Copy),
        _ => None,
    }
}

fn source_key(action: FileAction) -> &'static str {
    match action {
        FileAction::Open => "o",
        FileAction::Reveal => "r",
        FileAction::Copy => "y",
    }
}

fn source_label(action: FileAction) -> String {
    match action {
        FileAction::Open => "Open".to_string(),
        FileAction::Reveal => reveal_label().to_string(),
        FileAction::Copy => "Copy absolute path".to_string(),
    }
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width + 3 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

/// Wrap a path on `/` when it fits, so the filename stays on one line.
fn wrap_path(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if current_width + ch_width > width && !current.is_empty() {
            if let Some(slash) = current.rfind('/') {
                let break_at = slash + '/'.len_utf8();
                if break_at < current.len() {
                    let rest = current[break_at..].to_string();
                    current.truncate(break_at);
                    lines.push(std::mem::take(&mut current));
                    current = rest;
                    current_width = current.width();
                    if current_width + ch_width <= width {
                        current.push(ch);
                        current_width += ch_width;
                        continue;
                    }
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
            }
            if current_width + ch_width > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
        }
        current.push(ch);
        current_width += ch_width;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub fn init_skills_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> SkillsDialogState {
    SkillsDialogState::with_items(title, items)
}

pub fn render_skills_dialog(
    frame: &mut Frame,
    dialog_state: &mut SkillsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(frame, area, colors);
    if let Some(source) = dialog_state.source.as_mut() {
        let name = dialog_state
            .dialog
            .get_selected()
            .map(|item| item.name.clone())
            .unwrap_or_default();
        source.render(frame, area, &name, colors);
    }
}

pub fn handle_skills_dialog_key_event(
    dialog_state: &mut SkillsDialogState,
    event: KeyEvent,
) -> SkillsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return SkillsDialogAction::None;
    }
    if dialog_state.source.is_some() {
        return handle_source_key(dialog_state, event);
    }

    if is_info_key(event) {
        return open_source(dialog_state);
    }

    match event.code {
        KeyCode::Char(' ') => {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return SkillsDialogAction::Toggle {
                    skill_id: selected.id.clone(),
                };
            }
        }
        KeyCode::Enter if event.modifiers == KeyModifiers::NONE => {
            if let Some(name) = dialog_state
                .dialog
                .get_selected()
                .map(|item| item.id.clone())
            {
                dialog_state.dialog.hide();
                dialog_state.source = None;
                return SkillsDialogAction::InsertMention { name };
            }
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
            if !dialog_state.dialog.is_visible() {
                dialog_state.source = None;
            }
        }
    }

    SkillsDialogAction::None
}

fn open_source(dialog_state: &mut SkillsDialogState) -> SkillsDialogAction {
    let Some(selected) = dialog_state.dialog.get_selected() else {
        return SkillsDialogAction::None;
    };
    let Some(path) = dialog_state.locations.get(&selected.id).cloned() else {
        return SkillsDialogAction::None;
    };
    dialog_state.source = Some(SkillSource::new(path));
    SkillsDialogAction::None
}

fn handle_source_key(dialog_state: &mut SkillsDialogState, event: KeyEvent) -> SkillsDialogAction {
    let outcome = {
        let Some(source) = dialog_state.source.as_mut() else {
            return SkillsDialogAction::None;
        };
        let path = source.path.clone();
        match source.handle_key(event) {
            SourceEvent::None => return SkillsDialogAction::None,
            SourceEvent::Close => None,
            SourceEvent::Choose(action) => Some(SkillsDialogAction::File { action, path }),
        }
    };
    match outcome {
        None => {
            dialog_state.source = None;
            SkillsDialogAction::None
        }
        Some(action) => action,
    }
}

pub fn handle_skills_dialog_mouse_event(
    dialog_state: &mut SkillsDialogState,
    event: MouseEvent,
) -> SkillsDialogAction {
    if dialog_state.source.is_some() {
        return handle_source_mouse(dialog_state, event);
    }

    // Scrolling, hovering, and scrollbar dragging must never toggle a skill.
    let clicked_item = matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
        && dialog_state
            .dialog
            .item_index_at_position(event.column, event.row)
            .is_some();
    dialog_state.dialog.handle_mouse_event(event);
    if !dialog_state.dialog.is_visible() {
        dialog_state.source = None;
        return SkillsDialogAction::None;
    }
    if clicked_item && dialog_state.dialog.is_visible() {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SkillsDialogAction::Toggle {
                skill_id: selected.id.clone(),
            };
        }
    }
    SkillsDialogAction::None
}

fn handle_source_mouse(
    dialog_state: &mut SkillsDialogState,
    event: MouseEvent,
) -> SkillsDialogAction {
    let outcome = {
        let Some(source) = dialog_state.source.as_mut() else {
            return SkillsDialogAction::None;
        };
        let position = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Moved => {
                if let Some(index) = source
                    .action_rows
                    .iter()
                    .position(|row| row.contains(position))
                {
                    source.selected = index;
                }
                return SkillsDialogAction::None;
            }
            MouseEventKind::Down(MouseButton::Left)
                if source.esc_area.contains(position) || !source.area.contains(position) =>
            {
                None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                return source
                    .action_at(position)
                    .map(|action| SkillsDialogAction::File {
                        action,
                        path: source.path.clone(),
                    })
                    .unwrap_or(SkillsDialogAction::None);
            }
            _ => return SkillsDialogAction::None,
        }
    };
    match outcome {
        None => {
            dialog_state.source = None;
            SkillsDialogAction::None
        }
        Some(action) => action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::PrefsDAO;
    use crate::skill::SkillInfo;
    use ratatui::crossterm::event::KeyModifiers;

    fn store() -> SkillStore {
        SkillStore::for_test(["alpha", "beta"].map(|name| SkillInfo {
            name: name.to_string(),
            description: None,
            location: format!("/skills/{name}/SKILL.md").into(),
            content: String::new(),
        }))
    }

    fn state_for(store: &SkillStore) -> SkillsDialogState {
        let mut state = init_skills_dialog("Skills", vec![]);
        state.refresh(store);
        state.dialog.show();
        state
    }

    fn state() -> SkillsDialogState {
        let mut state = state_for(&store());
        state.dialog.dialog_area = Rect::new(0, 0, 80, 30);
        state
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn rendered(state: &mut SkillsDialogState) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                render_skills_dialog(
                    frame,
                    state,
                    frame.area(),
                    crate::theme::Theme::load_builtin_default().get_colors(true),
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn title_note_drops_the_fraction_when_nothing_is_disabled() {
        assert_eq!(skill_title_note(25, 25, 2_400), "25  ~2.4k");
        assert_eq!(skill_title_note(23, 25, 980), "23/25  ~980");
        assert_eq!(skill_title_note(0, 25, 0), "0/25");
        assert_eq!(skill_title_note(0, 0, 0), "0");
    }

    #[test]
    fn skills_are_alphabetical_regardless_of_case_or_activation() {
        let store =
            SkillStore::for_test(["zebra", "Beta", "alpha", "Alpha", "delta"].map(|name| {
                SkillInfo {
                    name: name.to_string(),
                    description: None,
                    location: format!("/skills/{name}/SKILL.md").into(),
                    content: String::new(),
                }
            }));
        store
            .set_enabled("Beta", false, &PrefsDAO::in_memory())
            .unwrap();
        let state = state_for(&store);
        let names = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter().map(|item| item.name.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(names, ["Alpha", "alpha", "Beta", "delta", "zebra"]);
        assert_eq!(state.dialog.title, "Skills");
        assert!(
            state.dialog.title_note.starts_with("4/5  ~"),
            "{}",
            state.dialog.title_note
        );
    }

    #[test]
    fn space_toggles_without_closing() {
        let mut state = state();
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char(' '))),
            SkillsDialogAction::Toggle {
                skill_id: "alpha".to_string()
            }
        );
        assert!(state.dialog.is_visible());
        handle_skills_dialog_key_event(&mut state, key(KeyCode::Esc));
        assert!(!state.dialog.is_visible());
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char(' '))),
            SkillsDialogAction::None
        );
    }

    #[test]
    fn enter_inserts_mention_and_closes_without_toggling() {
        let mut state = state();
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Enter)),
            SkillsDialogAction::InsertMention {
                name: "alpha".to_string()
            }
        );
        assert!(!state.dialog.is_visible());
        assert!(!state.info_is_open());
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Enter)),
            SkillsDialogAction::None
        );
    }

    #[test]
    fn empty_or_filtered_out_list_does_not_toggle_or_insert() {
        let mut state = state();
        state.dialog.set_search_query("no-such-skill");
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char(' '))),
            SkillsDialogAction::None
        );
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Enter)),
            SkillsDialogAction::None
        );
        assert!(state.dialog.is_visible());
        state.dialog.set_items(vec![]);
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char(' '))),
            SkillsDialogAction::None
        );
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Enter)),
            SkillsDialogAction::None
        );
        assert!(state.dialog.is_visible());
    }

    #[test]
    fn disabled_skills_remain_visible_and_refresh_keeps_search_and_selection() {
        let store = store();
        let prefs = PrefsDAO::in_memory();
        let mut state = state_for(&store);
        state.dialog.set_search_query("beta");
        assert!(state.dialog.get_selected().unwrap().active);

        store.set_enabled("beta", false, &prefs).unwrap();
        state.refresh(&store);
        assert_eq!(state.dialog.items.len(), 2);
        assert_eq!(state.dialog.title, "Skills");
        assert!(
            state.dialog.title_note.starts_with("1/2  ~"),
            "{}",
            state.dialog.title_note
        );
        assert_eq!(state.dialog.search_query, "beta");
        let selected = state.dialog.get_selected().unwrap();
        assert_eq!(selected.id, "beta");
        assert!(!selected.active);
        assert_eq!(selected.tip.as_deref(), Some("disabled"));
        assert!(state.dialog.is_visible());

        store.set_enabled("beta", true, &prefs).unwrap();
        state.refresh(&store);
        assert!(
            state.dialog.title_note.starts_with("2  ~"),
            "{}",
            state.dialog.title_note
        );
        assert!(!state.dialog.title_note.contains('/'));
        assert_eq!(
            state.dialog.get_selected().unwrap().tip.as_deref(),
            Some("enabled")
        );
    }

    #[test]
    fn only_a_left_click_on_a_skill_toggles() {
        let mut state = state();
        // Centered dialog: list starts at row 6, followed by group header.
        let row = 8;
        assert_eq!(
            handle_skills_dialog_mouse_event(
                &mut state,
                mouse(MouseEventKind::Down(MouseButton::Left), 4, row)
            ),
            SkillsDialogAction::Toggle {
                skill_id: "beta".to_string()
            }
        );
        assert!(state.dialog.is_visible());
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollUp,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
        ] {
            assert_eq!(
                handle_skills_dialog_mouse_event(&mut state, mouse(kind, 4, row)),
                SkillsDialogAction::None
            );
        }
        assert_eq!(
            handle_skills_dialog_mouse_event(
                &mut state,
                mouse(MouseEventKind::Down(MouseButton::Left), 75, row)
            ),
            SkillsDialogAction::None
        );
    }

    #[test]
    fn rendered_skill_dialog_shows_activation_controls_and_count() {
        let store = store();
        store
            .set_enabled("beta", false, &PrefsDAO::in_memory())
            .unwrap();
        let mut state = state_for(&store);
        let text = rendered(&mut state);
        for label in [
            "Skills", "1/2", "~", "alpha", "beta", "enabled", "disabled", "toggle", "space",
            "insert", "enter", "info", "ctrl+i", "close",
        ] {
            assert!(text.contains(label), "missing label: {label}");
        }
        assert_eq!(text.matches("toggle").count(), 1);

        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| render_skills_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let skills = find_cell(&buffer, "Skills").expect("title");
        let count = find_cell(&buffer, "1/2").expect("count");
        assert_eq!(buffer[skills].style().fg, Some(colors.text));
        assert_eq!(buffer[count].style().fg, Some(colors.text_weak));
    }

    fn find_cell(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, u16)> {
        let width = buffer.area.width;
        (0..buffer.area.height).find_map(|y| {
            let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
            row.find(needle).map(|x| (x as u16, y))
        })
    }

    #[test]
    fn ctrl_i_opens_source_panel_and_letters_do_not_search() {
        let mut state = state();
        state.dialog.set_search_query("alp");
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL)
            ),
            SkillsDialogAction::None
        );
        assert!(state.info_is_open());
        assert!(state.dialog.is_visible());
        assert_eq!(state.dialog.search_query, "alp");

        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char('z'))),
            SkillsDialogAction::None
        );
        assert_eq!(state.dialog.search_query, "alp");
        assert!(state.info_is_open());

        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| render_skills_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("/skills/alpha/SKILL.md"));
        assert!(text.contains("Open"));
        assert!(text.contains(reveal_label()));
        assert!(text.contains("Copy absolute path"));
        let path = find_cell(&buffer, "/skills/alpha").expect("path");
        let action = find_cell(&buffer, "Reveal").expect("unselected action");
        let title = state.source.as_ref().unwrap();
        let title = buffer[(title.area.x + 3, title.area.y + 1)].style();
        assert_eq!(buffer[path].style().fg, Some(colors.text_weak));
        assert_eq!(buffer[action].style().fg, Some(colors.text));
        assert_eq!(title.fg, Some(colors.primary));
        assert!(title.add_modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn source_panel_navigates_and_chooses_file_actions() {
        let mut state = state();
        handle_skills_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
        );
        let path = PathBuf::from("/skills/alpha/SKILL.md");
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Down)),
            SkillsDialogAction::None
        );
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Enter)),
            SkillsDialogAction::File {
                action: FileAction::Reveal,
                path: path.clone(),
            }
        );
        assert!(state.info_is_open());
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char('y'))),
            SkillsDialogAction::File {
                action: FileAction::Copy,
                path: path.clone(),
            }
        );
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char('o'))),
            SkillsDialogAction::File {
                action: FileAction::Open,
                path: path.clone(),
            }
        );
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::End)),
            SkillsDialogAction::None
        );
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Char(' '))),
            SkillsDialogAction::File {
                action: FileAction::Copy,
                path,
            }
        );
    }

    #[test]
    fn source_panel_escape_returns_to_the_skill_list() {
        let mut state = state();
        handle_skills_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('I'), KeyModifiers::CONTROL),
        );
        assert!(state.info_is_open());
        assert_eq!(
            handle_skills_dialog_key_event(&mut state, key(KeyCode::Esc)),
            SkillsDialogAction::None
        );
        assert!(!state.info_is_open());
        assert!(state.dialog.is_visible());
        assert_eq!(state.dialog.search_query, "");
        handle_skills_dialog_key_event(&mut state, key(KeyCode::Esc));
        assert!(!state.dialog.is_visible());
    }

    #[test]
    fn source_panel_click_outside_closes_and_click_on_action_chooses() {
        let mut state = state();
        handle_skills_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
        );
        let text_before = rendered(&mut state);
        assert!(text_before.contains("/skills/alpha/SKILL.md"));
        let open_row = state.source.as_ref().unwrap().action_rows[0];
        assert_eq!(
            handle_skills_dialog_mouse_event(
                &mut state,
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    open_row.x,
                    open_row.y
                )
            ),
            SkillsDialogAction::File {
                action: FileAction::Open,
                path: PathBuf::from("/skills/alpha/SKILL.md"),
            }
        );
        assert!(state.info_is_open());
        assert_eq!(
            handle_skills_dialog_mouse_event(
                &mut state,
                mouse(MouseEventKind::Down(MouseButton::Left), 0, 0)
            ),
            SkillsDialogAction::None
        );
        assert!(!state.info_is_open());
        assert!(state.dialog.is_visible());
    }

    #[test]
    fn wrapped_source_path_keeps_the_filename() {
        let store = SkillStore::for_test([SkillInfo {
            name: "alpha".to_string(),
            description: None,
            location: format!("/{}/SKILL.md", "directory/".repeat(12)).into(),
            content: String::new(),
        }]);
        let mut state = state_for(&store);
        handle_skills_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL),
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                render_skills_dialog(
                    frame,
                    &mut state,
                    frame.area(),
                    crate::theme::Theme::load_builtin_default().get_colors(true),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let panel = state.source.as_ref().unwrap().area;
        let rows = (panel.y..panel.bottom())
            .map(|y| {
                (panel.x..panel.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|row| row.contains("directory") || row.contains("SKILL"))
            .collect::<Vec<_>>();
        assert!(
            rows.iter().any(|row| row == "SKILL.md"),
            "filename should stay intact on its own line: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("directory/") && row.ends_with('/')),
            "long paths should break after a slash: {rows:?}"
        );
        assert!(
            rows.iter().all(|row| {
                row == "SKILL.md"
                    || row
                        .split('/')
                        .filter(|segment| !segment.is_empty())
                        .all(|segment| segment == "directory")
            }),
            "wrap should not split a path segment: {rows:?}"
        );
    }

    #[test]
    fn ctrl_i_without_a_known_location_does_nothing() {
        let mut state = init_skills_dialog(
            "Skills",
            vec![DialogItem {
                id: "alpha".to_string(),
                name: "alpha".to_string(),
                group: "Skills".to_string(),
                description: String::new(),
                tip: None,
                provider_id: String::new(),
                active: true,
            }],
        );
        state.dialog.show();
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL)
            ),
            SkillsDialogAction::None
        );
        assert!(!state.info_is_open());
    }
}
