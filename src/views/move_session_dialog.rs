use crate::session::manager::WorkspaceInfo;
use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

#[derive(Debug)]
pub struct MoveSessionDialogState {
    pub dialog: Dialog,
}

impl MoveSessionDialogState {
    pub fn new() -> Self {
        Self {
            dialog: dialog_with_actions(Dialog::with_items("Move session", Vec::new())),
        }
    }

    pub fn refresh_workspaces(
        &mut self,
        workspaces: Vec<WorkspaceInfo>,
        current_workspace_id: i64,
    ) {
        let was_visible = self.dialog.is_visible();
        let search_query = self.dialog.search_query.clone();
        let selected = self
            .dialog
            .get_selected()
            .map(|item| (item.id.clone(), item.provider_id.clone()));

        let items = workspaces
            .into_iter()
            .map(|workspace| DialogItem {
                id: workspace.id.to_string(),
                name: workspace.name,
                group: if workspace.id == current_workspace_id {
                    "Current".to_string()
                } else {
                    "Other".to_string()
                },
                description: workspace.path,
                tip: Some(crate::utils::time::relative_readable_time_from_now(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(workspace.last_opened_at.max(0) as u64),
                )),
                provider_id: workspace.id.to_string(),
                active: workspace.id == current_workspace_id,
            })
            .collect();

        self.dialog = dialog_with_actions(Dialog::with_items("Move session", items));
        self.dialog.set_search_query(search_query);
        if was_visible {
            self.dialog.show();
        }

        if let Some((id, provider_id)) = selected {
            let _ = self.dialog.select_item_by_key(&id, &provider_id);
        } else {
            let _ = self
                .dialog
                .select_item_by_id(&current_workspace_id.to_string());
        }
    }

    pub fn show(&mut self) {
        self.dialog.show();
    }
}

impl Default for MoveSessionDialogState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn init_move_session_dialog() -> MoveSessionDialogState {
    MoveSessionDialogState::new()
}

pub fn render_move_session_dialog(
    f: &mut Frame,
    state: &mut MoveSessionDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    state.dialog.render(f, area, colors);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveSessionDialogAction {
    None,
    Close,
    MoveToWorkspace(i64),
}

pub fn handle_move_session_dialog_key_event(
    state: &mut MoveSessionDialogState,
    event: KeyEvent,
) -> MoveSessionDialogAction {
    if !state.dialog.is_visible() {
        return MoveSessionDialogAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            state.dialog.hide();
            selected_workspace_id(state).map_or(MoveSessionDialogAction::None, |workspace_id| {
                MoveSessionDialogAction::MoveToWorkspace(workspace_id)
            })
        }
        _ => {
            let was_visible = state.dialog.is_visible();
            state.dialog.handle_key_event(event);
            if was_visible && !state.dialog.is_visible() {
                MoveSessionDialogAction::Close
            } else {
                MoveSessionDialogAction::None
            }
        }
    }
}

pub fn handle_move_session_dialog_mouse_event(
    state: &mut MoveSessionDialogState,
    event: MouseEvent,
) -> MoveSessionDialogAction {
    if !state.dialog.is_visible() {
        return MoveSessionDialogAction::None;
    }

    let clicked_item = if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        state.dialog.item_index_at_position(event.column, event.row)
    } else {
        None
    };

    let was_visible = state.dialog.is_visible();
    state.dialog.handle_mouse_event(event);

    if clicked_item.is_some() && state.dialog.is_visible() {
        state.dialog.hide();
        return selected_workspace_id(state)
            .map_or(MoveSessionDialogAction::None, |workspace_id| {
                MoveSessionDialogAction::MoveToWorkspace(workspace_id)
            });
    }

    if was_visible && !state.dialog.is_visible() {
        MoveSessionDialogAction::Close
    } else {
        MoveSessionDialogAction::None
    }
}

fn selected_workspace_id(state: &MoveSessionDialogState) -> Option<i64> {
    state.dialog.get_selected()?.id.parse::<i64>().ok()
}

fn dialog_with_actions(dialog: Dialog) -> Dialog {
    dialog.with_actions(vec![
        DialogAction {
            label: "Move".to_string(),
            key: "enter".to_string(),
        },
        DialogAction {
            label: "Close".to_string(),
            key: "esc".to_string(),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_marks_current_workspace_active() {
        let mut state = init_move_session_dialog();
        state.refresh_workspaces(
            vec![
                WorkspaceInfo {
                    id: 1,
                    path: "/tmp/a".to_string(),
                    name: "a".to_string(),
                    sort_order: 0,
                    last_opened_at: 1,
                },
                WorkspaceInfo {
                    id: 2,
                    path: "/tmp/b".to_string(),
                    name: "b".to_string(),
                    sort_order: 1,
                    last_opened_at: 2,
                },
            ],
            2,
        );

        let selected = state.dialog.get_selected().unwrap();
        assert_eq!(selected.id, "2");
        assert!(selected.active);
    }
}
