use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::ui::components::dialog::{Dialog, DialogItem};

#[derive(Debug, Clone)]
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
}

pub fn init_models_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> ModelsDialogState {
    ModelsDialogState::with_items(title, items)
}

pub fn render_models_dialog(f: &mut Frame, dialog_state: &ModelsDialogState, area: Rect) {
    dialog_state.dialog.render(f, area);
}

pub fn handle_models_dialog_key_event(
    dialog_state: &mut ModelsDialogState,
    event: KeyEvent,
) -> bool {
    let handled = dialog_state.dialog.handle_key_event(event);
    if !dialog_state.dialog.is_visible() {
        dialog_state.dialog.hide();
    }
    handled
}

pub fn handle_models_dialog_mouse_event(
    dialog_state: &mut ModelsDialogState,
    event: MouseEvent,
) -> bool {
    dialog_state.dialog.handle_mouse_event(event)
}
