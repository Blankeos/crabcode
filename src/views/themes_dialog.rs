use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum ThemesDialogAction {
    PreviewTheme { theme_id: String },
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

    let before = dialog_state.dialog.get_selected().map(|it| it.id.clone());

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

    if dialog_state.dialog.is_visible() {
        let after = dialog_state.dialog.get_selected().map(|it| it.id.clone());

        if before != after {
            if let Some(theme_id) = after {
                return ThemesDialogAction::PreviewTheme { theme_id };
            }
        }
    }

    ThemesDialogAction::None
}

pub fn handle_themes_dialog_mouse_event(
    dialog_state: &mut ThemesDialogState,
    event: MouseEvent,
) -> ThemesDialogAction {
    if !dialog_state.dialog.is_visible() {
        return ThemesDialogAction::None;
    }

    let before = dialog_state.dialog.get_selected().map(|it| it.id.clone());
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
            let theme_id = selected.id.clone();
            dialog_state.dialog.hide();
            return ThemesDialogAction::SelectTheme { theme_id };
        }
    }

    if dialog_state.dialog.is_visible() {
        let after = dialog_state.dialog.get_selected().map(|it| it.id.clone());

        if before != after {
            if let Some(theme_id) = after {
                return ThemesDialogAction::PreviewTheme { theme_id };
            }
        }
    }

    ThemesDialogAction::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn theme_item(id: &str, name: &str) -> DialogItem {
        DialogItem {
            id: id.to_string(),
            name: name.to_string(),
            group: "Built in".to_string(),
            description: String::new(),
            tip: None,
            provider_id: String::new(),
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    const CENTER_DIALOG_LIST_Y: u16 = 6;

    #[test]
    fn mouse_click_on_item_selects_theme() {
        let mut state = init_themes_dialog(
            "Themes",
            vec![
                theme_item("ayu", "Ayu"),
                theme_item("tokyonight", "Tokyo Night"),
            ],
        );
        state.dialog.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_themes_dialog_mouse_event(
            &mut state,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                4,
                CENTER_DIALOG_LIST_Y + 2,
            ),
        );

        assert_eq!(
            action,
            ThemesDialogAction::SelectTheme {
                theme_id: "tokyonight".to_string(),
            }
        );
        assert!(!state.dialog.is_visible());
    }

    #[test]
    fn mouse_move_previews_theme() {
        let mut state = init_themes_dialog(
            "Themes",
            vec![
                theme_item("ayu", "Ayu"),
                theme_item("tokyonight", "Tokyo Night"),
            ],
        );
        state.dialog.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_themes_dialog_mouse_event(
            &mut state,
            mouse(MouseEventKind::Moved, 4, CENTER_DIALOG_LIST_Y + 2),
        );

        assert_eq!(
            action,
            ThemesDialogAction::PreviewTheme {
                theme_id: "tokyonight".to_string(),
            }
        );
        assert!(state.dialog.is_visible());
    }
}
