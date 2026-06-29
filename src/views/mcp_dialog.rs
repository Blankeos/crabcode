use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum McpDialogAction {
    Toggle { server_id: String },
    None,
}

#[derive(Debug)]
pub struct McpDialogState {
    pub dialog: Dialog,
}

impl McpDialogState {
    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self {
            dialog: Dialog::with_items(title, items).with_actions(vec![DialogAction {
                label: "toggle".to_string(),
                key: "space".to_string(),
            }]),
        }
    }
}

pub fn init_mcp_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> McpDialogState {
    McpDialogState::with_items(title, items)
}

pub fn render_mcp_dialog(
    f: &mut Frame,
    dialog_state: &mut McpDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_mcp_dialog_key_event(
    dialog_state: &mut McpDialogState,
    event: KeyEvent,
) -> McpDialogAction {
    if !dialog_state.dialog.is_visible() {
        return McpDialogAction::None;
    }

    match event.code {
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return McpDialogAction::Toggle {
                    server_id: selected.id.clone(),
                };
            }
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
        }
    }

    McpDialogAction::None
}

pub fn handle_mcp_dialog_mouse_event(
    dialog_state: &mut McpDialogState,
    event: MouseEvent,
) -> McpDialogAction {
    if !dialog_state.dialog.handle_mouse_event(event) {
        return McpDialogAction::None;
    }
    if !dialog_state.dialog.is_visible() {
        return McpDialogAction::None;
    }
    dialog_state
        .dialog
        .get_selected()
        .map(|selected| McpDialogAction::Toggle {
            server_id: selected.id.clone(),
        })
        .unwrap_or(McpDialogAction::None)
}
