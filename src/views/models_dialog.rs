use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum ModelsDialogAction {
    SelectModel {
        provider_id: String,
        model_id: String,
    },
    ToggleFavorite {
        provider_id: String,
        model_id: String,
    },
    None,
}

#[derive(Debug)]
pub struct ModelsDialogState {
    pub dialog: Dialog,
}

impl ModelsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self { dialog }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self {
            dialog: Dialog::with_items(title, items).with_actions(vec![
                DialogAction {
                    label: "Connect provider".to_string(),
                    key: "ctrl+a".to_string(),
                },
                DialogAction {
                    label: "Favorite".to_string(),
                    key: "ctrl+f".to_string(),
                },
            ]),
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
        let actions = self.dialog.actions.clone();

        self.dialog = Dialog::with_items(title, items).with_actions(actions);

        if was_visible {
            self.dialog.show();
        }

        if !search_query.is_empty() {
            self.dialog.search_textarea.insert_str(&search_query);
            self.dialog.set_search_query(search_query);
        }

        if let Some((id, provider_id)) = selected_item {
            self.dialog.select_item_by_key(&id, &provider_id);
        }
    }
}

pub fn init_models_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> ModelsDialogState {
    ModelsDialogState::with_items(title, items)
}

pub fn render_models_dialog(
    f: &mut Frame,
    dialog_state: &mut ModelsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_models_dialog_key_event(
    dialog_state: &mut ModelsDialogState,
    event: KeyEvent,
) -> ModelsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return ModelsDialogAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            dialog_state.dialog.hide();
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return ModelsDialogAction::SelectModel {
                    provider_id: selected.provider_id.clone(),
                    model_id: selected.id.clone(),
                };
            }
        }
        KeyCode::Char('f') if event.modifiers == KeyModifiers::CONTROL => {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return ModelsDialogAction::ToggleFavorite {
                    provider_id: selected.provider_id.clone(),
                    model_id: selected.id.clone(),
                };
            }
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
        }
    }

    ModelsDialogAction::None
}

pub fn handle_models_dialog_mouse_event(
    dialog_state: &mut ModelsDialogState,
    event: MouseEvent,
) -> bool {
    dialog_state.dialog.handle_mouse_event(event)
}
