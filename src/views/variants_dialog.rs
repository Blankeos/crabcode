use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

use crate::{
    model::reasoning::{ReasoningCapability, ReasoningEffort},
    theme::ThemeColors,
    ui::components::dialog::{Dialog, DialogAction, DialogItem},
};

const DEFAULT_VARIANT_ID: &str = "default";
const VARIANTS_DIALOG_HEIGHT: u16 = 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantsDialogAction {
    Select,
    None,
}

pub struct VariantsDialogState {
    pub dialog: Dialog,
}

impl VariantsDialogState {
    pub fn new() -> Self {
        Self {
            dialog: Dialog::new("Select variant")
                .with_max_height(VARIANTS_DIALOG_HEIGHT)
                .with_actions(base_actions()),
        }
    }

    pub fn show(&mut self, capability: &ReasoningCapability, selected: Option<ReasoningEffort>) {
        let mut items = vec![variant_item(
            DEFAULT_VARIANT_ID,
            "Default",
            selected.is_none(),
        )];
        items.extend(capability.values().iter().copied().map(|effort| {
            variant_item(effort.as_str(), effort.as_str(), selected == Some(effort))
        }));
        self.dialog.set_items(items);
        self.dialog.show();

        let selected_id = selected
            .map(ReasoningEffort::as_str)
            .unwrap_or(DEFAULT_VARIANT_ID);
        self.dialog.select_item_by_id(selected_id);
    }

    pub fn selected_effort(&self) -> Option<Option<ReasoningEffort>> {
        let selected = self.dialog.get_selected()?;
        if selected.id == DEFAULT_VARIANT_ID {
            Some(None)
        } else {
            selected.id.parse().ok().map(Some)
        }
    }
}

impl Default for VariantsDialogState {
    fn default() -> Self {
        Self::new()
    }
}

fn base_actions() -> Vec<DialogAction> {
    vec![
        DialogAction {
            label: "Select".to_string(),
            key: "enter".to_string(),
        },
        DialogAction {
            label: "Close".to_string(),
            key: "esc".to_string(),
        },
    ]
}

fn variant_item(id: &str, name: &str, active: bool) -> DialogItem {
    DialogItem {
        id: id.to_string(),
        name: name.to_string(),
        group: String::new(),
        description: String::new(),
        tip: None,
        provider_id: String::new(),
        active,
    }
}

pub fn render_variants_dialog(
    frame: &mut Frame,
    state: &mut VariantsDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    state.dialog.render(frame, area, colors);
}

pub fn handle_variants_dialog_key_event(
    state: &mut VariantsDialogState,
    event: KeyEvent,
) -> VariantsDialogAction {
    if !state.dialog.is_visible() {
        return VariantsDialogAction::None;
    }

    match event.code {
        KeyCode::Enter => {
            state.dialog.hide();
            VariantsDialogAction::Select
        }
        _ => {
            state.dialog.handle_key_event(event);
            VariantsDialogAction::None
        }
    }
}

pub fn handle_variants_dialog_mouse_event(
    state: &mut VariantsDialogState,
    event: MouseEvent,
) -> VariantsDialogAction {
    if !state.dialog.is_visible() {
        return VariantsDialogAction::None;
    }

    let clicked_item = if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        state.dialog.item_index_at_position(event.column, event.row)
    } else {
        None
    };

    state.dialog.handle_mouse_event(event);

    if clicked_item.is_some() && state.dialog.is_visible() {
        state.dialog.hide();
        return VariantsDialogAction::Select;
    }

    VariantsDialogAction::None
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn shown_state() -> VariantsDialogState {
        let capability = ReasoningCapability::effort(
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            ReasoningEffort::Medium,
        );
        let mut state = VariantsDialogState::new();
        state.show(&capability, Some(ReasoningEffort::Medium));
        state
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn variants_include_default_and_select_override() {
        let mut state = shown_state();

        assert_eq!(state.selected_effort(), Some(Some(ReasoningEffort::Medium)));
        assert!(state.dialog.select_item_by_id(DEFAULT_VARIANT_ID));
        assert_eq!(state.selected_effort(), Some(None));
    }

    #[test]
    fn arrow_keys_navigate_without_confirming() {
        let mut state = shown_state();

        assert_eq!(
            handle_variants_dialog_key_event(&mut state, key(KeyCode::Down)),
            VariantsDialogAction::None
        );
        assert!(state.dialog.is_visible());
        assert_eq!(state.selected_effort(), Some(Some(ReasoningEffort::High)));

        assert_eq!(
            handle_variants_dialog_key_event(&mut state, key(KeyCode::Up)),
            VariantsDialogAction::None
        );
        assert_eq!(
            handle_variants_dialog_key_event(&mut state, key(KeyCode::Up)),
            VariantsDialogAction::None
        );
        assert!(state.dialog.is_visible());
        assert_eq!(state.selected_effort(), Some(Some(ReasoningEffort::Low)));
    }

    #[test]
    fn enter_confirms_highlighted_variant() {
        let mut state = shown_state();

        handle_variants_dialog_key_event(&mut state, key(KeyCode::Down));
        assert_eq!(
            handle_variants_dialog_key_event(&mut state, key(KeyCode::Enter)),
            VariantsDialogAction::Select
        );
        assert!(!state.dialog.is_visible());
        assert_eq!(state.selected_effort(), Some(Some(ReasoningEffort::High)));
    }

    #[test]
    fn escape_closes_without_confirming() {
        let mut state = shown_state();

        assert_eq!(
            handle_variants_dialog_key_event(&mut state, key(KeyCode::Esc)),
            VariantsDialogAction::None
        );
        assert!(!state.dialog.is_visible());
        assert_eq!(state.selected_effort(), Some(Some(ReasoningEffort::Medium)));
    }

    #[test]
    fn variants_dialog_is_shorter_than_default_picker() {
        use crate::theme::Theme;
        use ratatui::{backend::TestBackend, Terminal};

        let mut state = shown_state();
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let colors = Theme::load_builtin_default().get_colors(true);

        terminal
            .draw(|frame| {
                render_variants_dialog(frame, &mut state, frame.area(), colors);
            })
            .expect("render variants dialog");

        assert_eq!(state.dialog.dialog_area.height, VARIANTS_DIALOG_HEIGHT);
        assert!(state.dialog.dialog_area.height < 25);
        assert!(state.dialog.visible_row_count >= 5);
    }
}
