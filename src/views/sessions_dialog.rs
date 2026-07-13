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
            filter: SessionsDialogFilter::All,
            workspace_group_ids: HashMap::new(),
            current_workspace_id: None,
            last_list_signature: None,
        }
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
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
        self.dialog.update_items_in_place(items);
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
}

pub fn handle_sessions_dialog_key_event(
    dialog_state: &mut SessionsDialogState,
    event: KeyEvent,
) -> SessionsDialogAction {
    let was_visible = dialog_state.dialog.is_visible();

    if event.code == KeyCode::Char('n') && event.modifiers == KeyModifiers::CONTROL {
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

    if event.code == KeyCode::Char('p') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SessionsDialogAction::TogglePin(selected.id.clone());
        }
    }

    if event.code == KeyCode::Char('a') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SessionsDialogAction::Archive(selected.id.clone());
        }
    }

    if event.code == KeyCode::Char('d') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            if dialog_state.pending_delete.as_ref() == Some(&selected.id) {
                dialog_state.pending_delete = None;
                return SessionsDialogAction::Delete(selected.id.clone());
            }
            dialog_state.pending_delete = Some(selected.id.clone());
            return SessionsDialogAction::PendingDelete(selected.id.clone());
        }
    }

    if event.code == KeyCode::Esc && dialog_state.pending_delete.is_some() {
        dialog_state.pending_delete = None;
        return SessionsDialogAction::Handled;
    }

    if event.code == KeyCode::Char('r') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            let title = if selected.provider_id.is_empty() {
                selected.name.clone()
            } else {
                selected.provider_id.clone()
            };
            return SessionsDialogAction::Rename(selected.id.clone(), title);
        }
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
                key: "ctrl+d".to_string(),
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
            label: "Pin".to_string(),
            key: "ctrl+p".to_string(),
        },
        FooterAction {
            label: "Archive".to_string(),
            key: "ctrl+a".to_string(),
        },
        FooterAction {
            label: "Delete".to_string(),
            key: "ctrl+d".to_string(),
        },
        FooterAction {
            label: "Rename".to_string(),
            key: "ctrl+r".to_string(),
        },
    ])
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
    fn esc_cancels_pending_delete_without_closing_sessions_dialog() {
        let mut state =
            init_sessions_dialog("Sessions", vec![session_item("session-1", "First session")]);
        state.dialog.show();

        let action = handle_sessions_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
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
        assert!(state.dialog.is_visible());
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
        let items = (0..20)
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
