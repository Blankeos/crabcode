use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
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
) -> ModelsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return ModelsDialogAction::None;
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
            let provider_id = selected.provider_id.clone();
            let model_id = selected.id.clone();
            dialog_state.dialog.hide();
            return ModelsDialogAction::SelectModel {
                provider_id,
                model_id,
            };
        }
    }

    ModelsDialogAction::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_item(id: &str, name: &str, provider_id: &str) -> DialogItem {
        DialogItem {
            id: id.to_string(),
            name: name.to_string(),
            group: "OpenAI".to_string(),
            description: String::new(),
            tip: None,
            provider_id: provider_id.to_string(),
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

    #[test]
    fn mouse_click_on_item_selects_model() {
        let mut state = init_models_dialog(
            "Models",
            vec![
                model_item("gpt-5", "GPT-5", "openai"),
                model_item("claude-sonnet", "Claude Sonnet", "anthropic"),
            ],
        );
        state.dialog.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_models_dialog_mouse_event(&mut state, left_click(4, 10));

        assert_eq!(
            action,
            ModelsDialogAction::SelectModel {
                provider_id: "anthropic".to_string(),
                model_id: "claude-sonnet".to_string(),
            }
        );
        assert!(!state.dialog.is_visible());
    }

    #[test]
    fn mouse_click_on_group_header_does_not_select_model() {
        let mut state = init_models_dialog("Models", vec![model_item("gpt-5", "GPT-5", "openai")]);
        state.dialog.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };

        let action = handle_models_dialog_mouse_event(&mut state, left_click(4, 8));

        assert_eq!(action, ModelsDialogAction::None);
        assert!(state.dialog.is_visible());
    }
}
