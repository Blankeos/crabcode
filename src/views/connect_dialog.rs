use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogItem};
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

#[derive(Debug)]
pub struct ConnectDialogState {
    pub dialog: Dialog,
    pub pending_selection: Option<DialogItem>,
}

impl ConnectDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            pending_selection: None,
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self {
            dialog: Dialog::with_items(title, items),
            pending_selection: None,
        }
    }
}

pub fn init_connect_dialog() -> ConnectDialogState {
    ConnectDialogState::new(Dialog::with_items("Connect a provider", vec![]))
}

pub fn render_connect_dialog(
    f: &mut Frame,
    dialog_state: &mut ConnectDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_connect_dialog_key_event(
    dialog_state: &mut ConnectDialogState,
    event: KeyEvent,
) -> bool {
    let was_visible = dialog_state.dialog.is_visible();

    if event.code == ratatui::crossterm::event::KeyCode::Enter && was_visible {
        if let Some(item) = dialog_state.dialog.get_selected() {
            dialog_state.pending_selection = Some(item.clone());
            dialog_state.dialog.hide();
            return false;
        }
    }

    let handled = dialog_state.dialog.handle_key_event(event);

    if was_visible && !dialog_state.dialog.is_visible() {
        return false;
    }

    handled
}

pub fn get_pending_selection(dialog_state: &mut ConnectDialogState) -> Option<DialogItem> {
    dialog_state.pending_selection.take()
}

pub fn handle_connect_dialog_mouse_event(
    dialog_state: &mut ConnectDialogState,
    event: MouseEvent,
) -> bool {
    dialog_state.dialog.handle_mouse_event(event)
}
