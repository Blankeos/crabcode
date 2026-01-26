use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction as FooterAction, DialogItem};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{layout::Rect, Frame};

#[derive(Debug)]
pub struct SessionsDialogState {
    pub dialog: Dialog,
    pub pending_delete: Option<String>,
}

impl SessionsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            pending_delete: None,
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        let mut dialog = Dialog::with_items(title, items);
        dialog = dialog.with_actions(vec![
            FooterAction {
                label: "Delete".to_string(),
                key: "ctrl+d".to_string(),
            },
            FooterAction {
                label: "Rename".to_string(),
                key: "ctrl+r".to_string(),
            },
        ]);
        Self {
            dialog,
            pending_delete: None,
        }
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
        let title = self.dialog.title.clone();
        let was_visible = self.dialog.is_visible();
        let selected_index = self.dialog.selected_index;
        let items_clone = items.clone();

        self.dialog = Dialog::with_items(title, items);
        self.dialog = self.dialog.clone().with_actions(vec![
            FooterAction {
                label: "Delete".to_string(),
                key: "ctrl+d".to_string(),
            },
            FooterAction {
                label: "Rename".to_string(),
                key: "ctrl+r".to_string(),
            },
        ]);

        if was_visible {
            self.dialog.show();
        }

        if selected_index < items_clone.len() {
            self.dialog.selected_index = selected_index;
        }
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
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_sessions_dialog_key_event(
    dialog_state: &mut SessionsDialogState,
    event: KeyEvent,
) -> SessionsDialogAction {
    let was_visible = dialog_state.dialog.is_visible();

    if event.code == KeyCode::Char('d') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            dialog_state.pending_delete = Some(selected.id.clone());
            return SessionsDialogAction::Delete(selected.id.clone());
        }
    }

    if event.code == KeyCode::Char('r') && event.modifiers == KeyModifiers::CONTROL {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SessionsDialogAction::Rename(selected.id.clone(), selected.name.clone());
        }
    }

    let handled = dialog_state.dialog.handle_key_event(event);

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
) -> bool {
    dialog_state.dialog.handle_mouse_event(event)
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
    Delete(String),
    Rename(String, String),
}
