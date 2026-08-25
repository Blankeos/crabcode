use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum ThemesDialogAction {
    PreviewTheme { theme_id: String },
    SelectTheme { theme_id: String },
    ToggleTransparent,
    None,
}

#[derive(Debug)]
pub struct ThemesDialogState {
    pub dialog: Dialog,
    /// Whether the main UI background is transparent (terminal shows through).
    pub transparent: bool,
}

impl ThemesDialogState {
    pub fn new(dialog: Dialog, transparent: bool) -> Self {
        let mut state = Self {
            dialog,
            transparent,
        };
        state.sync_actions();
        state
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>, transparent: bool) -> Self {
        Self::new(Dialog::with_items(title, items), transparent)
    }

    pub fn set_transparent(&mut self, transparent: bool) {
        self.transparent = transparent;
        self.sync_actions();
    }

    pub fn toggle_transparent(&mut self) -> bool {
        self.transparent = !self.transparent;
        self.sync_actions();
        self.transparent
    }

    fn sync_actions(&mut self) {
        self.dialog.actions = transparent_actions(self.transparent);
    }

    pub fn refresh_items(&mut self, items: Vec<DialogItem>) {
        let title = self.dialog.title.clone();
        let was_visible = self.dialog.is_visible();
        let selected_index = self.dialog.selected_index;
        let items_clone = items.clone();
        let transparent = self.transparent;

        self.dialog = Dialog::with_items(title, items);
        self.sync_actions();
        // Keep transparent flag in sync after rebuild.
        let _ = transparent;

        if was_visible {
            self.dialog.show();
        }

        if selected_index < items_clone.len() {
            self.dialog.selected_index = selected_index;
        }
    }
}

fn transparent_actions(transparent: bool) -> Vec<DialogAction> {
    let label = if transparent {
        "transparent: on"
    } else {
        "transparent: off"
    };
    vec![
        DialogAction {
            key: "ctrl+t".to_string(),
            label: label.to_string(),
        },
        DialogAction {
            key: "esc".to_string(),
            label: "close".to_string(),
        },
        DialogAction {
            key: "enter".to_string(),
            label: "select".to_string(),
        },
    ]
}

pub fn init_themes_dialog(
    title: impl Into<String>,
    items: Vec<DialogItem>,
    transparent: bool,
) -> ThemesDialogState {
    ThemesDialogState::with_items(title, items, transparent)
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

    // Toggle transparency without leaving the dialog.
    if event.code == KeyCode::Char('t') && event.modifiers == KeyModifiers::CONTROL {
        dialog_state.toggle_transparent();
        return ThemesDialogAction::ToggleTransparent;
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

    fn theme_item(id: &str, name: &str, appearance: &str) -> DialogItem {
        DialogItem {
            id: id.to_string(),
            name: name.to_string(),
            group: "Built in".to_string(),
            description: appearance.to_string(),
            tip: None,
            provider_id: String::new(),
            active: false,
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
                theme_item("ayu", "Ayu", "dark"),
                theme_item("tokyonight", "Tokyo Night", "dark"),
            ],
            false,
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
                theme_item("ayu", "Ayu", "dark"),
                theme_item("tokyonight", "Tokyo Night", "dark"),
            ],
            false,
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

    #[test]
    fn ctrl_t_toggles_transparent() {
        let mut state = init_themes_dialog("Themes", vec![theme_item("ayu", "Ayu", "dark")], false);
        state.dialog.show();

        let action = handle_themes_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, ThemesDialogAction::ToggleTransparent);
        assert!(state.transparent);
        assert!(state
            .dialog
            .actions
            .iter()
            .any(|a| a.label.contains("transparent: on")));

        let action = handle_themes_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, ThemesDialogAction::ToggleTransparent);
        assert!(!state.transparent);
    }

    #[test]
    fn search_matches_appearance_description() {
        let mut state = init_themes_dialog(
            "Themes",
            vec![
                theme_item("grokday", "Grok Day", "light"),
                theme_item("groknight", "Grok Night", "dark"),
            ],
            false,
        );
        state.dialog.show();
        state.dialog.set_search_query("light");
        let visible: Vec<_> = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter().map(|i| i.id.as_str()))
            .collect();
        assert!(visible.contains(&"grokday"));
        assert!(!visible.contains(&"groknight"));
    }
}
