use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

use crate::skill::SkillStore;
use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction, DialogItem};

#[derive(Debug, Clone, PartialEq)]
pub enum SkillsDialogAction {
    Toggle { skill_id: String },
    None,
}

#[derive(Debug)]
pub struct SkillsDialogState {
    pub dialog: Dialog,
}

impl SkillsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog: dialog.with_actions(vec![
                DialogAction {
                    label: "toggle".to_string(),
                    key: "space".to_string(),
                },
                DialogAction {
                    label: "close".to_string(),
                    key: "esc".to_string(),
                },
            ]),
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self::new(Dialog::with_items(title, items))
    }

    /// Update activation labels without losing search, selection, or scroll.
    pub fn refresh(&mut self, store: &SkillStore) {
        self.dialog.set_items_preserve_ui(skill_dialog_items(store));
    }
}

fn skill_dialog_items(store: &SkillStore) -> Vec<DialogItem> {
    store
        .installed()
        .into_iter()
        .map(|skill| {
            let enabled = store.is_enabled(&skill.name);
            DialogItem {
                id: skill.name.clone(),
                name: skill.name.clone(),
                group: "Skills".to_string(),
                description: skill
                    .description
                    .clone()
                    .unwrap_or_else(|| "No description".to_string()),
                tip: Some(if enabled { "enabled" } else { "disabled" }.to_string()),
                provider_id: String::new(),
                active: enabled,
            }
        })
        .collect()
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
        KeyCode::Char(' ') => {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return SkillsDialogAction::Toggle {
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
) -> SkillsDialogAction {
    // Scrolling, hovering, and scrollbar dragging must never toggle a skill.
    let clicked_item = matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
        && dialog_state
            .dialog
            .item_index_at_position(event.column, event.row)
            .is_some();
    dialog_state.dialog.handle_mouse_event(event);
    if clicked_item && dialog_state.dialog.is_visible() {
        if let Some(selected) = dialog_state.dialog.get_selected() {
            return SkillsDialogAction::Toggle {
                skill_id: selected.id.clone(),
            };
        }
    }
    SkillsDialogAction::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::PrefsDAO;
    use crate::skill::SkillInfo;
    use ratatui::crossterm::event::KeyModifiers;

    fn store() -> SkillStore {
        SkillStore::for_test(["alpha", "beta"].map(|name| SkillInfo {
            name: name.to_string(),
            description: None,
            location: format!("/skills/{name}/SKILL.md").into(),
            content: String::new(),
        }))
    }

    fn state_for(store: &SkillStore) -> SkillsDialogState {
        let mut state = init_skills_dialog("Skills", vec![]);
        state.refresh(store);
        state.dialog.show();
        state
    }

    fn state() -> SkillsDialogState {
        let mut state = state_for(&store());
        state.dialog.dialog_area = Rect::new(0, 0, 80, 30);
        state
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn skills_are_alphabetical_regardless_of_case_or_activation() {
        let store =
            SkillStore::for_test(["zebra", "Beta", "alpha", "Alpha", "delta"].map(|name| {
                SkillInfo {
                    name: name.to_string(),
                    description: None,
                    location: format!("/skills/{name}/SKILL.md").into(),
                    content: String::new(),
                }
            }));
        store
            .set_enabled("Beta", false, &PrefsDAO::in_memory())
            .unwrap();
        let state = state_for(&store);
        let names = state
            .dialog
            .filtered_items
            .iter()
            .flat_map(|(_, items)| items.iter().map(|item| item.name.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(names, ["Alpha", "alpha", "Beta", "delta", "zebra"]);
        assert_eq!(
            store
                .all()
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "alpha", "delta", "zebra"]
        );
    }

    #[test]
    fn space_toggles_without_closing() {
        let mut state = state();
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            SkillsDialogAction::Toggle {
                skill_id: "alpha".to_string()
            }
        );
        assert!(state.dialog.is_visible());
        handle_skills_dialog_key_event(&mut state, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!state.dialog.is_visible());
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            SkillsDialogAction::None
        );
    }

    #[test]
    fn enter_does_not_toggle_or_close() {
        let mut state = state();
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            SkillsDialogAction::None
        );
        assert!(state.dialog.search_query.is_empty());
        assert!(state.dialog.is_visible());
    }

    #[test]
    fn empty_or_filtered_out_list_does_not_toggle() {
        let mut state = state();
        state.dialog.set_search_query("no-such-skill");
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            SkillsDialogAction::None
        );
        assert!(state.dialog.is_visible());
        state.dialog.set_items(vec![]);
        assert_eq!(
            handle_skills_dialog_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
            ),
            SkillsDialogAction::None
        );
    }

    #[test]
    fn disabled_skills_remain_visible_and_refresh_keeps_search_and_selection() {
        let store = store();
        let prefs = PrefsDAO::in_memory();
        let mut state = state_for(&store);
        state.dialog.set_search_query("beta");
        assert!(state.dialog.get_selected().unwrap().active);

        store.set_enabled("beta", false, &prefs).unwrap();
        state.refresh(&store);
        assert_eq!(state.dialog.items.len(), 2);
        assert_eq!(state.dialog.search_query, "beta");
        let selected = state.dialog.get_selected().unwrap();
        assert_eq!(selected.id, "beta");
        assert!(!selected.active);
        assert_eq!(selected.tip.as_deref(), Some("disabled"));
        assert!(state.dialog.is_visible());

        store.set_enabled("beta", true, &prefs).unwrap();
        state.refresh(&store);
        assert_eq!(
            state.dialog.get_selected().unwrap().tip.as_deref(),
            Some("enabled")
        );
    }

    #[test]
    fn only_a_left_click_on_a_skill_toggles() {
        let mut state = state();
        // Centered dialog: list starts at row 6, followed by group header.
        let row = 8;
        assert_eq!(
            handle_skills_dialog_mouse_event(
                &mut state,
                mouse(MouseEventKind::Down(MouseButton::Left), 4, row)
            ),
            SkillsDialogAction::Toggle {
                skill_id: "beta".to_string()
            }
        );
        assert!(state.dialog.is_visible());
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollUp,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
        ] {
            assert_eq!(
                handle_skills_dialog_mouse_event(&mut state, mouse(kind, 4, row)),
                SkillsDialogAction::None
            );
        }
        assert_eq!(
            handle_skills_dialog_mouse_event(
                &mut state,
                mouse(MouseEventKind::Down(MouseButton::Left), 75, row)
            ),
            SkillsDialogAction::None
        );
    }

    #[test]
    fn rendered_skill_dialog_shows_activation_and_controls() {
        let store = store();
        store
            .set_enabled("beta", false, &PrefsDAO::in_memory())
            .unwrap();
        let mut state = state_for(&store);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|f| {
                render_skills_dialog(
                    f,
                    &mut state,
                    f.area(),
                    crate::theme::Theme::load_builtin_default().get_colors(true),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for label in [
            "alpha", "beta", "enabled", "disabled", "toggle", "space", "close",
        ] {
            assert!(text.contains(label), "missing label: {label}");
        }
        assert_eq!(text.matches("toggle").count(), 1);
        assert!(!text.contains("enter"));
    }
}
