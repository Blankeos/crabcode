use crate::theme::ThemeColors;
use crate::ui::components::dialog::{
    Dialog, DialogAction as FooterAction, DialogItem, DialogPosition,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, Frame};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionsDialogFilter {
    Active,
    All,
    Archived,
}

impl SessionsDialogFilter {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Active,
            Self::Active => Self::Archived,
            Self::Archived => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::All => "All",
            Self::Archived => "Archive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsDialogListSignature {
    pub rows: Vec<SessionsDialogRowSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionsDialogRowSignature {
    pub id: String,
    pub title: String,
    pub pinned: bool,
    pub tip: Option<String>,
    pub group: String,
    pub is_streaming: bool,
    pub unread_completed: bool,
}

pub fn session_loading_glyph(frame: usize) -> &'static str {
    const SPINNER_CHARS: &[&str] = &["·", "✻", "✽", "✶", "✳", "✢"];
    SPINNER_CHARS[frame % SPINNER_CHARS.len()]
}

#[derive(Debug)]
pub struct SessionsDialogState {
    pub dialog: Dialog,
    pub pending_delete: Option<String>,
    pub item_menu: Option<SessionsItemMenu>,
    pub filter: SessionsDialogFilter,
    workspace_group_ids: HashMap<String, i64>,
    current_workspace_id: Option<i64>,
    pub(crate) last_list_signature: Option<SessionsDialogListSignature>,
}

impl SessionsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            pending_delete: None,
            item_menu: None,
            filter: SessionsDialogFilter::All,
            workspace_group_ids: HashMap::new(),
            current_workspace_id: None,
            last_list_signature: None,
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        let dialog = Dialog::with_items(title, items)
            .with_position(DialogPosition::Left)
            .with_collapsible_groups(true)
            .with_focusable_group_headers(true);
        Self {
            dialog: with_sessions_actions(dialog, SessionsDialogFilter::All, false),
            pending_delete: None,
            item_menu: None,
            filter: SessionsDialogFilter::All,
            workspace_group_ids: HashMap::new(),
            current_workspace_id: None,
            last_list_signature: None,
        }
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
        // The item menu acts on a snapshot of the selected row; rebuilds can
        // move or remove that row, so never keep a stale menu (or an armed
        // delete nothing can confirm) across a refresh.
        self.item_menu = None;
        self.pending_delete = None;
        let previous_dialog = self.dialog.clone();
        let title = self.dialog.title.clone();
        let was_visible = self.dialog.is_visible();
        let selected_item = self
            .dialog
            .get_selected()
            .map(|item| (item.id.clone(), item.provider_id.clone()));
        let focused_group = self.dialog.get_focused_group_header().map(str::to_string);
        let scroll_offset = self.dialog.scroll_offset;
        let visible_row_count = self.dialog.visible_row_count;
        let search_query = self.dialog.search_query.clone();
        let collapsed_groups = self.dialog.collapsed_groups();
        let filter = self.filter;
        let priority_groups = self.current_workspace_priority_groups();

        self.dialog = Dialog::with_items(title, items)
            .with_position(DialogPosition::Left)
            .with_collapsible_groups(true)
            .with_focusable_group_headers(true)
            .with_search_priority_groups(priority_groups);
        self.dialog.set_collapsed_groups(collapsed_groups);
        self.dialog = with_sessions_actions(self.dialog.clone(), filter, false);
        self.dialog.restore_search_query(search_query);

        if was_visible {
            self.dialog.show();
        }

        if let Some(group) = focused_group {
            let _ = self.dialog.focus_group_header(&group);
        } else if let Some((id, provider_id)) = selected_item {
            self.dialog.select_item_by_key(&id, &provider_id);
        }
        self.dialog.visible_row_count = visible_row_count;
        self.dialog.scroll_offset = scroll_offset;
        self.dialog
            .preserve_scrollbar_drag_state_from(&previous_dialog);
    }

    pub fn refresh_items_if_changed(
        &mut self,
        items: Vec<DialogItem>,
        signature: SessionsDialogListSignature,
    ) {
        if self.last_list_signature.as_ref() == Some(&signature) {
            return;
        }
        self.last_list_signature = Some(signature);
        self.refresh_items(items);
    }

    pub fn apply_streaming_row_markers(&mut self, streaming_session_ids: &[String], frame: usize) {
        let glyph = format!("{} ", session_loading_glyph(frame));
        let streaming: std::collections::HashSet<&str> =
            streaming_session_ids.iter().map(String::as_str).collect();

        for item in &mut self.dialog.items {
            let pin = if item.name.contains('★') {
                "★ "
            } else {
                ""
            };
            let title = item.provider_id.clone();
            if streaming.contains(item.id.as_str()) {
                item.name = format!("{glyph}{pin}{title}");
            } else {
                let unread = item.name.starts_with('●');
                item.name = if unread {
                    format!("● {pin}{title}")
                } else {
                    format!("{pin}{title}")
                };
            }
        }
        let items = std::mem::take(&mut self.dialog.items);
        let scroll_offset = self.dialog.scroll_offset;
        self.dialog.update_items_in_place(items);
        self.dialog.scroll_offset = scroll_offset;
    }

    pub fn set_workspace_group_ids(&mut self, group_ids: HashMap<String, i64>) {
        self.workspace_group_ids = group_ids;
        self.apply_current_workspace_search_priority();
    }

    pub fn set_current_workspace_id(&mut self, workspace_id: i64) {
        self.current_workspace_id = Some(workspace_id);
        self.apply_current_workspace_search_priority();
    }

    pub fn focus_workspace(&mut self, workspace_id: i64) -> bool {
        let Some(group) = self
            .workspace_group_ids
            .iter()
            .find_map(|(group, id)| (*id == workspace_id).then(|| group.clone()))
        else {
            return false;
        };

        self.dialog.focus_group_header(&group)
    }

    pub fn select_first_item_in_workspace(&mut self, workspace_id: i64) -> bool {
        let Some(group) = self
            .workspace_group_ids
            .iter()
            .find_map(|(group, id)| (*id == workspace_id).then(|| group.clone()))
        else {
            return false;
        };

        self.dialog.select_first_item_in_group(&group)
    }

    fn focused_workspace_group(&self) -> Option<(String, i64)> {
        let group = self.dialog.get_focused_group_header()?.to_string();
        let workspace_id = self.workspace_group_ids.get(&group).copied()?;
        Some((group, workspace_id))
    }

    fn current_workspace_priority_groups(&self) -> Vec<String> {
        let Some(current_workspace_id) = self.current_workspace_id else {
            return Vec::new();
        };

        self.workspace_group_ids
            .iter()
            .filter_map(|(group, id)| (*id == current_workspace_id).then(|| group.clone()))
            .collect()
    }

    fn apply_current_workspace_search_priority(&mut self) {
        let priority_groups = self.current_workspace_priority_groups();
        self.dialog.set_search_priority_groups(priority_groups);
    }
}

pub fn init_sessions_dialog(
    title: impl Into<String>,
    items: Vec<DialogItem>,
) -> SessionsDialogState {
    SessionsDialogState::with_items(title, items)
}

pub fn render_sessions_dialog(
    f: &mut Frame,
    dialog_state: &mut SessionsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.pending_delete_id = dialog_state.pending_delete.clone();
    if dialog_state.pending_delete.is_some() {
        dialog_state.dialog =
            with_sessions_actions(dialog_state.dialog.clone(), dialog_state.filter, true);
    } else {
        dialog_state.dialog =
            with_sessions_actions(dialog_state.dialog.clone(), dialog_state.filter, false);
    }
    dialog_state.dialog.render(f, area, colors);
    if let Some(menu) = dialog_state.item_menu.clone() {
        let delete_armed = dialog_state.pending_delete.as_ref() == Some(&menu.session_id);
        render_item_menu(f, &menu, delete_armed, area, colors);
    }
}

pub fn handle_sessions_dialog_key_event(
    dialog_state: &mut SessionsDialogState,
    event: KeyEvent,
) -> SessionsDialogAction {
    let was_visible = dialog_state.dialog.is_visible();

    // Modal item menu (ctrl+o) takes all keys while open, lazygit-style.
    if dialog_state.item_menu.is_some() {
        return handle_item_menu_key_event(dialog_state, event);
    }

    if event.code == KeyCode::Char('o') && event.modifiers == KeyModifiers::CONTROL {
        if open_item_menu(dialog_state) {
            return SessionsDialogAction::Handled;
        }
        return SessionsDialogAction::NotHandled;
    }

    // While the user is typing in search, ctrl+n must move the cursor down
    // instead of opening the new-session flow.
    let search_typing = !dialog_state.dialog.search_query.is_empty();

    if !search_typing
        && event.code == KeyCode::Char('n')
        && event.modifiers == KeyModifiers::CONTROL
    {
        return SessionsDialogAction::NewSession;
    }

    if event.code == KeyCode::Tab {
        dialog_state.filter = dialog_state.filter.next();
        dialog_state.pending_delete = None;
        return SessionsDialogAction::ChangeFilter(dialog_state.filter);
    }

    if event.modifiers.contains(KeyModifiers::ALT) {
        if let Some((group, workspace_id)) = dialog_state.focused_workspace_group() {
            match event.code {
                KeyCode::Up => {
                    dialog_state.pending_delete = None;
                    return SessionsDialogAction::MoveWorkspaceGroup {
                        workspace_id,
                        group,
                        direction: WorkspaceGroupMoveDirection::Up,
                    };
                }
                KeyCode::Down => {
                    dialog_state.pending_delete = None;
                    return SessionsDialogAction::MoveWorkspaceGroup {
                        workspace_id,
                        group,
                        direction: WorkspaceGroupMoveDirection::Down,
                    };
                }
                _ => {}
            }
        }
    }

    if event.code == KeyCode::Right {
        if let Some(group) = dialog_state
            .dialog
            .get_focused_group_header()
            .map(str::to_string)
        {
            dialog_state.dialog.toggle_group_collapsed(&group);
            let _ = dialog_state.dialog.focus_group_header(&group);
            dialog_state.pending_delete = None;
            return SessionsDialogAction::Handled;
        }
    }

    if event.code == KeyCode::Esc && dialog_state.pending_delete.is_some() {
        dialog_state.pending_delete = None;
        return SessionsDialogAction::Handled;
    }

    let handled = match event.code {
        KeyCode::Up if was_visible => {
            dialog_state.dialog.previous_wrapping();
            true
        }
        KeyCode::Down if was_visible => {
            dialog_state.dialog.next_wrapping();
            true
        }
        _ => dialog_state.dialog.handle_key_event(event),
    };

    // Clear pending delete when user navigates away
    if matches!(event.code, KeyCode::Up | KeyCode::Down | KeyCode::Esc) {
        dialog_state.pending_delete = None;
    }

    if was_visible && !dialog_state.dialog.is_visible() {
        return SessionsDialogAction::Close;
    }

    if event.code == KeyCode::Enter && was_visible {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SessionsDialogAction::Select(selected.id.clone());
        }
    }

    if handled {
        SessionsDialogAction::Handled
    } else {
        SessionsDialogAction::NotHandled
    }
}

pub fn handle_sessions_dialog_mouse_event(
    dialog_state: &mut SessionsDialogState,
    event: MouseEvent,
) -> SessionsDialogAction {
    // Modal item menu: clicks neither select nor dismiss behind it.
    if dialog_state.item_menu.is_some() {
        return SessionsDialogAction::Handled;
    }
    let was_visible = dialog_state.dialog.is_visible();
    let previous_index = dialog_state.dialog.selected_index;

    if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        if let Some(group) = dialog_state
            .dialog
            .group_at_position(event.column, event.row)
        {
            let _ = dialog_state.dialog.focus_group_header(&group);
            dialog_state.dialog.toggle_group_collapsed(&group);
            dialog_state.pending_delete = None;
            return SessionsDialogAction::Handled;
        }
    }

    let clicked_item = if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        dialog_state
            .dialog
            .item_index_at_position(event.column, event.row)
    } else {
        None
    };

    let handled = dialog_state.dialog.handle_mouse_event(event);

    if dialog_state.dialog.selected_index != previous_index {
        dialog_state.pending_delete = None;
    }

    if was_visible && !dialog_state.dialog.is_visible() {
        dialog_state.pending_delete = None;
        return SessionsDialogAction::Close;
    }

    if clicked_item.is_some() {
        dialog_state.pending_delete = None;
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SessionsDialogAction::Select(selected.id.clone());
        }
    }

    if handled {
        SessionsDialogAction::Handled
    } else {
        SessionsDialogAction::NotHandled
    }
}

pub fn get_pending_delete(dialog_state: &mut SessionsDialogState) -> Option<String> {
    dialog_state.pending_delete.take()
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionsDialogAction {
    Handled,
    NotHandled,
    Close,
    Select(String),
    NewSession,
    ChangeFilter(SessionsDialogFilter),
    TogglePin(String),
    Archive(String),
    Delete(String),
    PendingDelete(String),
    Rename(String, String),
    MoveWorkspaceGroup {
        workspace_id: i64,
        group: String,
        direction: WorkspaceGroupMoveDirection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceGroupMoveDirection {
    Up,
    Down,
}

impl WorkspaceGroupMoveDirection {
    pub fn offset(self) -> isize {
        match self {
            Self::Up => -1,
            Self::Down => 1,
        }
    }
}

fn with_sessions_actions(
    dialog: Dialog,
    filter: SessionsDialogFilter,
    confirm_delete: bool,
) -> Dialog {
    if confirm_delete {
        return dialog.with_actions(vec![
            FooterAction {
                label: "Confirm".to_string(),
                key: "enter".to_string(),
            },
            FooterAction {
                label: "Cancel".to_string(),
                key: "esc".to_string(),
            },
        ]);
    }

    dialog.with_actions(vec![
        FooterAction {
            label: filter.label().to_string(),
            key: "tab".to_string(),
        },
        FooterAction {
            label: "New".to_string(),
            key: "ctrl+n".to_string(),
        },
        FooterAction {
            label: "Options".to_string(),
            key: "ctrl+o".to_string(),
        },
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemMenuEntry {
    Pin,
    Archive,
    Unarchive,
    Rename,
    Delete,
}

impl ItemMenuEntry {
    const ALL: [Self; 5] = [
        Self::Pin,
        Self::Archive,
        Self::Unarchive,
        Self::Rename,
        Self::Delete,
    ];

    fn shortcut(self) -> char {
        match self {
            Self::Pin => 'p',
            Self::Archive => 'a',
            Self::Unarchive => 'u',
            Self::Rename => 'r',
            Self::Delete => 'd',
        }
    }
}

/// Lazygit-style per-item options menu opened with ctrl+o. Inapplicable rows
/// stay visible but disabled (dimmed + crossed out, skipped in navigation),
/// so it's always clear why an option can't run here.
#[derive(Debug, Clone)]
pub struct SessionsItemMenu {
    session_id: String,
    session_title: String,
    pinned: bool,
    archived: bool,
    selected: usize,
}

impl SessionsItemMenu {
    fn is_enabled(&self, entry: ItemMenuEntry) -> bool {
        match entry {
            ItemMenuEntry::Pin | ItemMenuEntry::Rename | ItemMenuEntry::Delete => true,
            ItemMenuEntry::Archive => !self.archived,
            ItemMenuEntry::Unarchive => self.archived,
        }
    }

    fn disabled_reason(&self, entry: ItemMenuEntry) -> Option<&'static str> {
        match entry {
            ItemMenuEntry::Archive if self.archived => Some("already archived"),
            ItemMenuEntry::Unarchive if !self.archived => Some("not archived"),
            _ => None,
        }
    }

    fn label(&self, entry: ItemMenuEntry, delete_armed: bool) -> String {
        match entry {
            ItemMenuEntry::Pin => {
                if self.pinned {
                    "Unpin".to_string()
                } else {
                    "Pin".to_string()
                }
            }
            ItemMenuEntry::Archive => "Archive".to_string(),
            ItemMenuEntry::Unarchive => "Unarchive".to_string(),
            ItemMenuEntry::Rename => "Rename".to_string(),
            ItemMenuEntry::Delete => {
                if delete_armed {
                    "Confirm delete".to_string()
                } else {
                    "Delete".to_string()
                }
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let mut index = self.selected as isize;
        loop {
            index += delta;
            if index < 0 || index >= ItemMenuEntry::ALL.len() as isize {
                break;
            }
            if self.is_enabled(ItemMenuEntry::ALL[index as usize]) {
                self.selected = index as usize;
                break;
            }
        }
    }
}

fn open_item_menu(dialog_state: &mut SessionsDialogState) -> bool {
    let Some(selected) = dialog_state.dialog.get_selected() else {
        return false;
    };
    let id = selected.id.clone();
    let title = if selected.provider_id.is_empty() {
        selected.name.clone()
    } else {
        selected.provider_id.clone()
    };
    let pinned = dialog_state
        .last_list_signature
        .as_ref()
        .and_then(|signature| {
            signature
                .rows
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.pinned)
        })
        .unwrap_or_else(|| selected.name.contains('★'));
    dialog_state.item_menu = Some(SessionsItemMenu {
        session_id: id,
        session_title: title,
        pinned,
        archived: dialog_state.filter == SessionsDialogFilter::Archived,
        selected: 0,
    });
    true
}

fn close_item_menu(dialog_state: &mut SessionsDialogState) {
    dialog_state.item_menu = None;
    dialog_state.pending_delete = None;
}

fn run_item_menu_entry(
    dialog_state: &mut SessionsDialogState,
    entry: ItemMenuEntry,
) -> SessionsDialogAction {
    let Some(menu) = dialog_state.item_menu.clone() else {
        return SessionsDialogAction::Handled;
    };
    if !menu.is_enabled(entry) {
        return SessionsDialogAction::Handled;
    }
    let id = menu.session_id.clone();
    match entry {
        ItemMenuEntry::Pin => {
            close_item_menu(dialog_state);
            SessionsDialogAction::TogglePin(id)
        }
        ItemMenuEntry::Archive | ItemMenuEntry::Unarchive => {
            close_item_menu(dialog_state);
            SessionsDialogAction::Archive(id)
        }
        ItemMenuEntry::Rename => {
            close_item_menu(dialog_state);
            SessionsDialogAction::Rename(id, menu.session_title.clone())
        }
        ItemMenuEntry::Delete => {
            if dialog_state.pending_delete.as_ref() == Some(&id) {
                close_item_menu(dialog_state);
                SessionsDialogAction::Delete(id)
            } else {
                dialog_state.pending_delete = Some(id.clone());
                if let Some(menu) = dialog_state.item_menu.as_mut() {
                    // Park selection on Delete so Enter confirms the armed delete.
                    menu.selected = ItemMenuEntry::ALL
                        .iter()
                        .position(|entry| *entry == ItemMenuEntry::Delete)
                        .unwrap_or(menu.selected);
                }
                SessionsDialogAction::PendingDelete(id)
            }
        }
    }
}

fn handle_item_menu_key_event(
    dialog_state: &mut SessionsDialogState,
    event: KeyEvent,
) -> SessionsDialogAction {
    if event.code == KeyCode::Esc
        || (event.code == KeyCode::Char('o') && event.modifiers == KeyModifiers::CONTROL)
    {
        close_item_menu(dialog_state);
        return SessionsDialogAction::Handled;
    }

    // Modal: anything with ctrl/alt/super (except the ctrl+o toggle above)
    // is swallowed so search emacs keys can't leak into item actions.
    if event.modifiers != KeyModifiers::NONE && event.modifiers != KeyModifiers::SHIFT {
        return SessionsDialogAction::Handled;
    }

    match event.code {
        KeyCode::Up => {
            if let Some(menu) = dialog_state.item_menu.as_mut() {
                menu.move_selection(-1);
            }
            SessionsDialogAction::Handled
        }
        KeyCode::Down => {
            if let Some(menu) = dialog_state.item_menu.as_mut() {
                menu.move_selection(1);
            }
            SessionsDialogAction::Handled
        }
        KeyCode::Enter => {
            let entry = dialog_state
                .item_menu
                .as_ref()
                .map(|menu| ItemMenuEntry::ALL[menu.selected]);
            match entry {
                Some(entry) => run_item_menu_entry(dialog_state, entry),
                None => SessionsDialogAction::Handled,
            }
        }
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'j' => {
                if let Some(menu) = dialog_state.item_menu.as_mut() {
                    menu.move_selection(1);
                }
                SessionsDialogAction::Handled
            }
            'k' => {
                if let Some(menu) = dialog_state.item_menu.as_mut() {
                    menu.move_selection(-1);
                }
                SessionsDialogAction::Handled
            }
            shortcut => match ItemMenuEntry::ALL
                .iter()
                .find(|entry| entry.shortcut() == shortcut)
            {
                Some(entry) => run_item_menu_entry(dialog_state, *entry),
                None => SessionsDialogAction::Handled,
            },
        },
        _ => SessionsDialogAction::Handled,
    }
}

fn render_item_menu(
    f: &mut Frame,
    menu: &SessionsItemMenu,
    delete_armed: bool,
    area: Rect,
    colors: ThemeColors,
) {
    use ratatui::{
        layout::{Alignment, Constraint, Direction, Layout},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::{Clear, Paragraph},
    };
    use unicode_width::UnicodeWidthStr;

    let rows: Vec<(ItemMenuEntry, String, bool, Option<&str>)> = ItemMenuEntry::ALL
        .iter()
        .map(|entry| {
            (
                *entry,
                menu.label(*entry, delete_armed),
                menu.is_enabled(*entry),
                menu.disabled_reason(*entry),
            )
        })
        .collect();

    let label_width = rows
        .iter()
        .map(|(_, label, _, _)| label.width())
        .max()
        .unwrap_or(0);
    let reason_width = rows
        .iter()
        .filter_map(|(_, _, enabled, reason)| {
            if *enabled {
                None
            } else {
                reason.map(|reason| reason.width() + 3)
            }
        })
        .max()
        .unwrap_or(0);
    // Marker + key chip + label + optional " (reason)".
    let content_width = 2 + 5 + 1 + label_width + reason_width;
    let title = format!("Options · {}", menu.session_title);
    let footer = "↑↓ navigate · enter run · esc close";

    let popup_width = (content_width + 6).min(area.width as usize).max(20) as u16;
    let popup_height = (rows.len() + 6).min(area.height as usize).max(7) as u16;
    let popup_area = Rect {
        x: area.x + area.width.saturating_sub(popup_width) / 2,
        y: area.y + area.height.saturating_sub(popup_height) / 2,
        width: popup_width,
        height: popup_height,
    };

    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        popup_area,
    );

    let content_area = Rect {
        x: popup_area.x + 3,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(6),
        height: popup_area.height.saturating_sub(2),
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(content_area);

    let max_title_width = content_area.width as usize;
    let title_text = if title.width() > max_title_width {
        let mut truncated = String::new();
        let mut width = 0;
        for char in title.chars() {
            let char_width = unicode_width::UnicodeWidthChar::width(char).unwrap_or(0);
            if width + char_width > max_title_width.saturating_sub(1) {
                break;
            }
            truncated.push(char);
            width += char_width;
        }
        format!("{truncated}…")
    } else {
        title
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            title_text,
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Left),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::new();
    for (index, (entry, label, enabled, reason)) in rows.iter().enumerate() {
        let is_selected = index == menu.selected;
        let marker = if is_selected { "▸ " } else { "  " };
        let key_text = format!(" {} ", entry.shortcut());

        if *enabled {
            let marker_style = if is_selected {
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(colors.text_weak)
            };
            let label_style = if is_selected {
                Style::default()
                    .fg(colors.text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(colors.text)
            };
            let padding = " ".repeat(label_width.saturating_sub(label.width()));
            lines.push(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(
                    key_text,
                    Style::default()
                        .fg(colors.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(label.clone(), label_style),
                Span::raw(padding),
            ]));
        } else {
            let disabled_style = Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT);
            let padding = " ".repeat(label_width.saturating_sub(label.width()));
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(colors.text_weak)),
                Span::styled(key_text, Style::default().fg(colors.text_weak)),
                Span::raw(" "),
                Span::styled(label.clone(), disabled_style),
                Span::raw(padding),
            ];
            if let Some(reason) = reason {
                spans.push(Span::styled(
                    format!(" ({reason})"),
                    Style::default()
                        .fg(colors.text_weak)
                        .add_modifier(Modifier::DIM),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Left), chunks[2]);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            footer,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]))
        .alignment(Alignment::Left),
        chunks[4],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_item(id: &str, name: &str) -> DialogItem {
        session_item_in_group(id, name, "Today")
    }

    fn session_item_in_group(id: &str, name: &str, group: &str) -> DialogItem {
        DialogItem {
            id: id.to_string(),
            name: name.to_string(),
            group: group.to_string(),
            description: String::new(),
            tip: None,
            provider_id: String::new(),
            active: false,
        }
    }

    fn left_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn scroll_down(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn refresh_items_if_changed_skips_identical_signature() {
        let items = vec![session_item("session-1", "First session")];
        let mut state = init_sessions_dialog("Sessions", items.clone());
        state.dialog.show();
        state.dialog.selected_index = 0;

        let signature = SessionsDialogListSignature {
            rows: vec![SessionsDialogRowSignature {
                id: "session-1".to_string(),
                title: String::new(),
                pinned: false,
                tip: None,
                group: "Today".to_string(),
                is_streaming: false,
                unread_completed: false,
            }],
        };

        state.refresh_items_if_changed(items.clone(), signature.clone());
        let selected_after_first = state.dialog.selected_index;

        state.refresh_items_if_changed(items, signature);
        assert_eq!(state.dialog.selected_index, selected_after_first);
    }

    #[test]
    fn apply_streaming_row_markers_clears_spinner_when_not_streaming() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![DialogItem {
                id: "s1".to_string(),
                name: "✻ ★ Title".to_string(),
                group: "Today".to_string(),
                description: String::new(),
                tip: None,
                provider_id: "Title".to_string(),
                active: false,
            }],
        );
        state.apply_streaming_row_markers(&[], 2);
        assert_eq!(state.dialog.items[0].name, "★ Title");
    }

    #[test]
    fn filter_cycle_starts_from_all() {
        assert_eq!(
            SessionsDialogFilter::All.next(),
            SessionsDialogFilter::Active
        );
        assert_eq!(
            SessionsDialogFilter::Active.next(),
            SessionsDialogFilter::Archived
        );
        assert_eq!(
            SessionsDialogFilter::Archived.next(),
            SessionsDialogFilter::All
        );
    }

    #[test]
    fn ctrl_n_requests_new_session_when_sessions_dialog_is_focused() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
        );

        assert_eq!(action, SessionsDialogAction::NewSession);
    }

    #[test]
    fn ctrl_a_no_longer_archives_cmd_left_goes_to_search() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );

        assert_ne!(
            action,
            SessionsDialogAction::Archive("session-1".to_string())
        );
        assert_eq!(state.pending_delete, None);
    }

    #[test]
    fn ctrl_a_edits_search_instead_of_archiving_while_searching() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();
        state.dialog.set_search_query("First");

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
    }

    #[test]
    fn ctrl_d_does_not_arm_delete_while_searching() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();
        state.dialog.set_search_query("First");

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert_eq!(state.pending_delete, None);
    }

    #[test]
    fn esc_closes_item_menu_and_cancels_armed_delete() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(state.item_menu.is_some());

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            SessionsDialogAction::PendingDelete("session-1".to_string())
        );
        assert_eq!(state.pending_delete.as_deref(), Some("session-1"));
        assert!(state.dialog.is_visible());

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert_eq!(state.pending_delete, None);
        assert!(state.item_menu.is_none());
        assert!(state.dialog.is_visible());
    }

    #[test]
    fn item_menu_archive_shortcut_archives_unarchived_session() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            SessionsDialogAction::Archive("session-1".to_string())
        );
        assert!(state.item_menu.is_none());
    }

    #[test]
    fn item_menu_shows_archive_disabled_when_already_archived() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();
        state.filter = SessionsDialogFilter::Archived;

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);
        let menu = state.item_menu.as_ref().expect("menu open");
        assert!(menu.archived);
        assert!(!menu.is_enabled(ItemMenuEntry::Archive));
        assert!(menu.is_enabled(ItemMenuEntry::Unarchive));

        // Disabled shortcut does nothing (stays on menu).
        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        );
        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(state.item_menu.is_some());

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            SessionsDialogAction::Archive("session-1".to_string())
        );
    }

    #[test]
    fn item_menu_navigation_skips_disabled_entries() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();
        state.filter = SessionsDialogFilter::Archived;

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);
        // Pin(0), Archive(1, disabled), Unarchive(2): j must skip Archive.
        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(action, SessionsDialogAction::Handled);
        assert_eq!(state.item_menu.as_ref().expect("menu open").selected, 2);
    }

    #[test]
    fn item_menu_delete_needs_two_steps() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            SessionsDialogAction::PendingDelete("session-1".to_string())
        );
        assert!(state.item_menu.is_some());

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        // Arming parks selection on Delete, so Enter confirms it.
        assert_eq!(
            action,
            SessionsDialogAction::Delete("session-1".to_string())
        );
        assert!(state.item_menu.is_none());
    }

    #[test]
    fn item_menu_pin_and_rename_shortcuts() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            SessionsDialogAction::TogglePin("session-1".to_string())
        );

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, SessionsDialogAction::Handled);

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            SessionsDialogAction::Rename("session-1".to_string(), "First session".to_string())
        );
    }

    #[test]
    fn esc_closes_sessions_dialog_without_pending_delete() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Close);
        assert!(!state.dialog.is_visible());
    }

    #[test]
    fn down_moves_from_last_session_in_workspace_to_next_workspace_header() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item_in_group("session-1", "First session", "Workspace A"),
                session_item_in_group("session-2", "Second session", "Workspace A"),
                session_item_in_group("session-3", "Third session", "Workspace B"),
            ],
        );
        state.dialog.show();
        state.dialog.selected_index = 1;

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert_eq!(state.dialog.get_focused_group_header(), Some("Workspace B"));
        assert!(state.dialog.get_selected().is_none());
    }

    #[test]
    fn arrow_navigation_cycles_across_workspace_groups() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item_in_group("session-1", "First session", "Workspace A"),
                session_item_in_group("session-2", "Second session", "Workspace A"),
                session_item_in_group("session-3", "Third session", "Workspace B"),
            ],
        );
        state.dialog.show();
        state.dialog.selected_index = 2;

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert_eq!(state.dialog.get_focused_group_header(), Some("Workspace A"));

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert_eq!(state.dialog.get_selected().unwrap().id, "session-3");
    }

    #[test]
    fn right_toggles_focused_workspace_header_collapse() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![session_item_in_group(
                "session-1",
                "First session",
                "Workspace A",
            )],
        );
        state.dialog.show();
        assert!(state.dialog.focus_group_header("Workspace A"));

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(state.dialog.is_group_collapsed("Workspace A"));
        assert_eq!(state.dialog.get_focused_group_header(), Some("Workspace A"));

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );

        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(!state.dialog.is_group_collapsed("Workspace A"));
    }

    #[test]
    fn option_arrows_request_workspace_group_move_when_header_focused() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![session_item_in_group(
                "session-1",
                "First session",
                "Workspace A",
            )],
        );
        state.dialog.show();
        state.set_workspace_group_ids(HashMap::from([("Workspace A".to_string(), 42)]));
        assert!(state.dialog.focus_group_header("Workspace A"));

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Up, KeyModifiers::ALT),
        );

        assert_eq!(
            action,
            SessionsDialogAction::MoveWorkspaceGroup {
                workspace_id: 42,
                group: "Workspace A".to_string(),
                direction: WorkspaceGroupMoveDirection::Up
            }
        );
    }

    #[test]
    fn mouse_click_on_item_selects_session() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item("session-1", "First session"),
                session_item("session-2", "Second session"),
            ],
        );
        state.dialog.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_sessions_dialog_mouse_event(&mut state, left_click(4, 7));

        assert_eq!(
            action,
            SessionsDialogAction::Select("session-2".to_string())
        );
        assert_eq!(state.dialog.selected_index, 1);
    }

    #[test]
    fn mouse_click_on_group_header_toggles_workspace_collapse() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_sessions_dialog_mouse_event(&mut state, left_click(4, 5));

        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(state.dialog.is_group_collapsed("Today"));
        assert_eq!(state.dialog.selected_index, 0);

        let action = handle_sessions_dialog_mouse_event(&mut state, left_click(4, 5));

        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(!state.dialog.is_group_collapsed("Today"));
    }

    #[test]
    fn mouse_wheel_scrolls_session_list() {
        let items = (0..40)
            .map(|idx| session_item(&format!("session-{idx}"), &format!("Session {idx}")))
            .collect();
        let mut state = init_sessions_dialog("Sessions", items);
        state.dialog.show();
        state.dialog.visible_row_count = 5;
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_sessions_dialog_mouse_event(&mut state, scroll_down(4, 8));

        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(state.dialog.scroll_offset > 0);
    }

    #[test]
    fn mouse_move_after_wheel_scroll_updates_selection_without_resetting_viewport() {
        let items = (0..40)
            .map(|idx| session_item(&format!("session-{idx}"), &format!("Session {idx}")))
            .collect();
        let mut state = init_sessions_dialog("Sessions", items);
        state.dialog.show();
        state.dialog.visible_row_count = 5;
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        handle_sessions_dialog_mouse_event(&mut state, scroll_down(4, 8));
        let scroll_offset = state.dialog.scroll_offset;
        let action = handle_sessions_dialog_mouse_event(
            &mut state,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 4,
                row: 8,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(action, SessionsDialogAction::NotHandled);
        assert_ne!(state.dialog.selected_index, 0);
        assert_eq!(state.dialog.scroll_offset, scroll_offset);
    }

    #[test]
    fn streaming_marker_update_preserves_viewport_after_hover() {
        let items = (0..20)
            .map(|idx| DialogItem {
                id: format!("session-{idx}"),
                name: format!("Session {idx}"),
                group: "Today".to_string(),
                description: String::new(),
                tip: None,
                provider_id: format!("Session {idx}"),
                active: false,
            })
            .collect();
        let mut state = init_sessions_dialog("Sessions", items);
        state.dialog.show();
        state.dialog.visible_row_count = 5;
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        handle_sessions_dialog_mouse_event(&mut state, scroll_down(4, 8));
        handle_sessions_dialog_mouse_event(
            &mut state,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 4,
                row: 8,
                modifiers: KeyModifiers::NONE,
            },
        );
        let scroll_offset = state.dialog.scroll_offset;

        state.apply_streaming_row_markers(&["session-0".to_string()], 1);

        assert_eq!(state.dialog.scroll_offset, scroll_offset);
    }

    #[test]
    fn mouse_wheel_scroll_through_grouped_sessions_survives_refresh() {
        let items: Vec<_> = (0..30)
            .map(|idx| {
                session_item_in_group(
                    &format!("session-{idx}"),
                    &format!("Session {idx}"),
                    if idx < 15 {
                        "Workspace A"
                    } else {
                        "Workspace B"
                    },
                )
            })
            .collect();
        let mut state = init_sessions_dialog("Sessions", items.clone());
        state.dialog.show();
        state.dialog.visible_row_count = 5;

        for _ in 0..18 {
            state.dialog.scroll_down();
        }
        let scroll_offset = state.dialog.scroll_offset;

        state.refresh_items(items);

        assert!(scroll_offset > 15);
        assert_eq!(state.dialog.scroll_offset, scroll_offset);
        assert_eq!(state.dialog.visible_row_count, 5);
    }

    #[test]
    fn refresh_preserves_scroll_and_visible_search_query() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item("session-1", "First session"),
                session_item("session-2", "Second session"),
            ],
        );
        state.dialog.show();
        state.dialog.set_search_query("Second");
        state.dialog.scroll_offset = 3;

        state.refresh_items(vec![
            session_item("session-1", "First session"),
            session_item("session-2", "Second session"),
            session_item("session-3", "Third session"),
        ]);

        assert_eq!(state.dialog.search_query, "Second");
        assert_eq!(state.dialog.search_textarea.lines().join(""), "Second");
        assert_eq!(state.dialog.scroll_offset, 3);
    }

    #[test]
    fn search_prioritizes_current_workspace_sessions() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item_in_group("other", "Fix parser bug", "other-workspace"),
                session_item_in_group("current", "Parser cleanup", "current-workspace"),
            ],
        );
        state.set_current_workspace_id(2);
        state.set_workspace_group_ids(HashMap::from([
            ("other-workspace".to_string(), 1),
            ("current-workspace".to_string(), 2),
        ]));

        state.dialog.set_search_query("parser");

        assert_eq!(state.dialog.get_selected().unwrap().id, "current");
    }

    #[test]
    fn refresh_preserves_selected_session_by_id_after_reorder() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item("session-1", "First session"),
                session_item("session-2", "Second session"),
            ],
        );
        state.dialog.show();
        state.dialog.selected_index = 1;

        state.refresh_items(vec![
            session_item("session-2", "Second session"),
            session_item("session-1", "First session"),
        ]);

        assert_eq!(state.dialog.get_selected().unwrap().id, "session-2");
        assert_eq!(state.dialog.selected_index, 0);
    }

    #[test]
    fn refresh_preserves_collapsed_workspaces() {
        let mut state = init_sessions_dialog(
            "Sessions",
            vec![
                session_item_in_group("session-1", "First session", "Workspace A"),
                session_item_in_group("session-2", "Second session", "Workspace B"),
            ],
        );
        state.dialog.show();
        state.dialog.toggle_group_collapsed("Workspace A");

        state.refresh_items(vec![
            session_item_in_group("session-1", "First session", "Workspace A"),
            session_item_in_group("session-2", "Second session", "Workspace B"),
            session_item_in_group("session-3", "Third session", "Workspace A"),
        ]);

        assert!(state.dialog.is_group_collapsed("Workspace A"));
        assert!(!state.dialog.is_group_collapsed("Workspace B"));
    }
}
