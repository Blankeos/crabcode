use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum SkillsDialogAction {
    SelectSkill { skill_id: String },
    None,
}

#[derive(Debug)]
pub struct SkillsDialogState {
    pub dialog: Dialog,
}

impl SkillsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self { dialog }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self {
            dialog: Dialog::with_items(title, items),
        }
    }
}

pub fn init_skills_dialog(title: impl Into<String>, items: Vec<DialogItem>) -> SkillsDialogState {
    SkillsDialogState::with_items(title, items)
}

pub fn render_skills_dialog(
    f: &mut Frame,
    dialog_state: &mut SkillsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    dialog_state.dialog.render(f, area, colors);
}

pub fn handle_skills_dialog_key_event(
    dialog_state: &mut SkillsDialogState,
    event: KeyEvent,
) -> SkillsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return SkillsDialogAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            dialog_state.dialog.hide();
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return SkillsDialogAction::SelectSkill {
                    skill_id: selected.id.clone(),
                };
            }
        }
        _ => {
            dialog_state.dialog.handle_key_event(event);
        }
    }

    SkillsDialogAction::None
}

pub fn handle_skills_dialog_mouse_event(
    dialog_state: &mut SkillsDialogState,
    event: MouseEvent,
) -> bool {
    dialog_state.dialog.handle_mouse_event(event)
}
