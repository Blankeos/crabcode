use ratatui::crossterm::event::KeyEvent;
use ratatui::{layout::Rect, Frame};

use crate::autocomplete::Suggestion;
use crate::theme::ThemeColors;
use crate::ui::components::popup::{Popup, PopupAction};

pub struct SuggestionsPopupState {
    pub popup: Popup,
}

impl SuggestionsPopupState {
    pub fn new(popup: Popup) -> Self {
        Self { popup }
    }
}

pub fn init_suggestions_popup(popup: Popup) -> SuggestionsPopupState {
    SuggestionsPopupState::new(popup)
}

pub fn render_suggestions_popup(
    f: &mut Frame,
    popup_state: &SuggestionsPopupState,
    area: Rect,
    has_focus: bool,
    colors: ThemeColors,
) {
    popup_state.popup.render(f, area, has_focus, colors);
}

pub fn handle_suggestions_popup_key_event(
    popup_state: &mut SuggestionsPopupState,
    event: KeyEvent,
) -> PopupAction {
    popup_state.popup.handle_key_event(event)
}

pub fn set_suggestions(popup_state: &mut SuggestionsPopupState, suggestions: Vec<Suggestion>) {
    popup_state.popup.set_suggestions(suggestions);
}

pub fn clear_suggestions(popup_state: &mut SuggestionsPopupState) {
    popup_state.popup.clear();
}

pub fn get_selected_suggestion(popup_state: &SuggestionsPopupState) -> Option<&Suggestion> {
    popup_state.popup.get_selected()
}

pub fn is_suggestions_visible(popup_state: &SuggestionsPopupState) -> bool {
    popup_state.popup.is_visible()
}
