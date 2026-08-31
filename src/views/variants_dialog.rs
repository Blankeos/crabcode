use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    model::reasoning::{ReasoningCapability, ReasoningEffort},
    theme::ThemeColors,
    ui::components::dialog::{Dialog, DialogItem},
};

const DEFAULT_VARIANT_ID: &str = "default";

pub struct VariantsDialogState {
    pub dialog: Dialog,
}

impl VariantsDialogState {
    pub fn new() -> Self {
        Self {
            dialog: Dialog::new("Select variant"),
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

    pub fn handle_key_event(&mut self, event: KeyEvent) -> bool {
        self.dialog.handle_key_event(event)
    }
}

impl Default for VariantsDialogState {
    fn default() -> Self {
        Self::new()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_include_default_and_select_override() {
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

        assert_eq!(state.selected_effort(), Some(Some(ReasoningEffort::Medium)));
        assert!(state.dialog.select_item_by_id(DEFAULT_VARIANT_ID));
        assert_eq!(state.selected_effort(), Some(None));
    }
}
