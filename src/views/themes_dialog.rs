use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum ThemesDialogAction {
    SelectTheme { theme_id: String },
    None,
}

#[derive(Debug)]
pub struct ThemesDialogState {
    pub dialog: Dialog,
}

impl ThemesDialogState {
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

pub fn init_themes_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> ThemesDialogState {
    ThemesDialogState::with_items(title, items)
}

pub fn render_themes_dialog(
    f: &mut Frame,
    dialog_state: &mut ThemesDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_themes_dialog_key_event(
    dialog_state: &mut ThemesDialogState,
    event: KeyEvent,
) -> ThemesDialogAction {
    if !dialog_state.dialog.is_visible() {
        return ThemesDialogAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            dialog_state.dialog.hide();
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return ThemesDialogAction::SelectTheme {
                    theme_id: selected.id.clone(),
                };
            }
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
        }
    }

    ThemesDialogAction::None
}

pub fn handle_themes_dialog_mouse_event(
    dialog_state: &mut ThemesDialogState,
    event: MouseEvent,
) -> bool {
    dialog_state.dialog.handle_mouse_event(event)
}
