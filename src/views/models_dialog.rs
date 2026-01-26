use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogItem};

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
            dialog: Dialog::with_items(title, items),
        }
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
        let title = self.dialog.title.clone();
        let was_visible = self.dialog.is_visible();
        let selected_index = self.dialog.selected_index;
        let items_clone = items.clone();

        self.dialog = Dialog::with_items(title, items);

        if was_visible {
            self.dialog.show();
        }

        if selected_index < items_clone.len() {
            self.dialog.selected_index = selected_index;
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
