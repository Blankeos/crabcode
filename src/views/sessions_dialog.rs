use crate::theme::ThemeColors;
use crate::ui::components::dialog::{
    Dialog, DialogAction as FooterAction, DialogItem, DialogPosition,
};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionsDialogFilter {
    Active,
    All,
    Archived,
}

impl SessionsDialogFilter {
    pub fn next(self) -> Self {
        match self {
            Self::Active => Self::All,
            Self::All => Self::Archived,
            Self::Archived => Self::Active,
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

#[derive(Debug)]
pub struct SessionsDialogState {
    pub dialog: Dialog,
    pub pending_delete: Option<String>,
    pub filter: SessionsDialogFilter,
}

impl SessionsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            pending_delete: None,
            filter: SessionsDialogFilter::Active,
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        let dialog = Dialog::with_items(title, items)
            .with_position(DialogPosition::Left)
            .with_collapsible_groups(true);
        Self {
            dialog: with_sessions_actions(dialog, SessionsDialogFilter::Active, false),
            pending_delete: None,
            filter: SessionsDialogFilter::Active,
        }
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
        let previous_dialog = self.dialog.clone();
        let title = self.dialog.title.clone();
        let was_visible = self.dialog.is_visible();
        let selected_index = self.dialog.selected_index;
        let scroll_offset = self.dialog.scroll_offset;
        let items_clone = items.clone();
        let search_query = self.dialog.search_query.clone();
        let collapsed_groups = self.dialog.collapsed_groups();
        let filter = self.filter;

        self.dialog = Dialog::with_items(title, items)
            .with_position(DialogPosition::Left)
            .with_collapsible_groups(true);
        self.dialog.set_collapsed_groups(collapsed_groups);
        self.dialog = with_sessions_actions(self.dialog.clone(), filter, false);
        self.dialog.set_search_query(search_query);

        if was_visible {
            self.dialog.show();
        }

        if selected_index < items_clone.len() {
            self.dialog.selected_index = selected_index;
        }
        self.dialog.scroll_offset = scroll_offset;
        self.dialog
            .preserve_scrollbar_drag_state_from(&previous_dialog);
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

    let handled = dialog_state.dialog.handle_key_event(event);

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
}

fn with_sessions_actions(
    dialog: Dialog,
    filter: SessionsDialogFilter,
    confirm_delete: bool,
) -> Dialog {
    if confirm_delete {
        return dialog.with_actions(vec![FooterAction {
            label: "confirm".to_string(),
            key: "ctrl+d".to_string(),
        }]);
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

        let action = handle_sessions_dialog_mouse_event(&mut state, left_click(4, 8));

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

        let action = handle_sessions_dialog_mouse_event(&mut state, left_click(4, 6));

        assert_eq!(action, SessionsDialogAction::Handled);
        assert!(state.dialog.is_group_collapsed("Today"));
        assert_eq!(state.dialog.selected_index, 0);

        let action = handle_sessions_dialog_mouse_event(&mut state, left_click(4, 6));

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
