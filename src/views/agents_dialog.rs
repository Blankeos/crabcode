use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum AgentsDialogAction {
    SelectAgent { agent: String },
    None,
}

#[derive(Debug)]
pub struct AgentsDialogState {
    pub dialog: Dialog,
}

impl AgentsDialogState {
    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self {
            dialog: Dialog::with_items(title, items).with_actions(base_actions()),
        }
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
        let title = self.dialog.title.clone();
        let was_visible = self.dialog.is_visible();
        let selected_item = self
            .dialog
            .get_selected()
            .map(|item| (item.id.clone(), item.provider_id.clone()));
        let search_query = self.dialog.search_textarea.lines().join("");

        self.dialog = Dialog::with_items(title, items).with_actions(base_actions());

        if was_visible {
            self.dialog.show();
        }

        if !search_query.is_empty() {
            self.dialog.search_textarea.insert_str(&search_query);
            self.dialog.set_search_query(search_query);
        }

        if let Some((id, provider_id)) = selected_item {
            let _ = self.dialog.select_item_by_key(&id, &provider_id);
        }
    }
}

pub fn init_agents_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> AgentsDialogState {
    AgentsDialogState::with_items(title, items)
}

pub fn render_agents_dialog(
    f: &mut Frame,
    dialog_state: &mut AgentsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

fn base_actions() -> Vec<DialogAction> {
    vec![
        DialogAction {
            label: "Select".to_string(),
            key: "enter".to_string(),
        },
        DialogAction {
            label: "Close".to_string(),
            key: "esc".to_string(),
        },
    ]
}

pub fn handle_agents_dialog_key_event(
    dialog_state: &mut AgentsDialogState,
    event: KeyEvent,
) -> AgentsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return AgentsDialogAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            dialog_state.dialog.hide();
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return AgentsDialogAction::SelectAgent {
                    agent: selected.id.clone(),
                };
            }
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
        }
    }

    AgentsDialogAction::None
}

pub fn handle_agents_dialog_mouse_event(
    dialog_state: &mut AgentsDialogState,
    event: MouseEvent,
) -> AgentsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return AgentsDialogAction::None;
    }

    let clicked_item = if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        dialog_state
            .dialog
            .item_index_at_position(event.column, event.row)
    } else {
        None
    };

    dialog_state.dialog.handle_mouse_event(event);

    if clicked_item.is_some() && dialog_state.dialog.is_visible() {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            let agent = selected.id.clone();
            dialog_state.dialog.hide();
            return AgentsDialogAction::SelectAgent { agent };
        }
    }

    AgentsDialogAction::None
}
