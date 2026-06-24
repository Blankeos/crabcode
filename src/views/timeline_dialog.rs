use crate::session::types::{Message, MessageRole};
use crate::theme::ThemeColors;
use crate::ui::components::dialog::{
    Dialog, DialogAction as FooterAction, DialogItem, DialogPosition,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimelineRole {
    User,
    Assistant,
}

#[derive(Debug)]
pub struct TimelineDialogState {
    pub dialog: Dialog,
}

impl TimelineDialogState {
    pub fn new() -> Self {
        let mut dialog = Dialog::new("Timeline").with_position(DialogPosition::Right);
        dialog = dialog.with_actions(vec![FooterAction {
            label: "Jump actions".to_string(),
            key: "enter".to_string(),
        }]);
        Self { dialog }
    }

    pub fn build_from_messages(messages: &[Message]) -> Self {
        let mut state = Self::new();
        state.refresh_messages(messages);
        state
    }

    pub fn refresh_messages(&mut self, messages: &[Message]) {
        let mut items: Vec<DialogItem> = Vec::new();
        let mut last_timeline_role: Option<TimelineRole> = None;

        for (idx, message) in messages.iter().enumerate() {
            let timeline_role = match message.role {
                MessageRole::User => TimelineRole::User,
                MessageRole::Assistant => TimelineRole::Assistant,
                _ => continue,
            };

            let preview = message_preview(message);

            if timeline_role == TimelineRole::Assistant
                && (!message.is_complete || preview == "(empty)")
            {
                continue;
            }

            if timeline_role == TimelineRole::Assistant
                && last_timeline_role == Some(TimelineRole::Assistant)
            {
                continue;
            }

            let role_label = match timeline_role {
                TimelineRole::User => "You",
                TimelineRole::Assistant => "Agent",
            };

            let name = format!("{}: {}", role_label, preview);
            let description = String::new();

            let tip = {
                let duration = message.timestamp.elapsed().unwrap_or_default();
                let secs = duration.as_secs();
                if secs < 60 {
                    format!("{}s ago", secs)
                } else if secs < 3600 {
                    format!("{}m ago", secs / 60)
                } else {
                    format!("{}h ago", secs / 3600)
                }
            };

            items.push(DialogItem {
                id: idx.to_string(),
                name,
                group: String::new(),
                description,
                tip: Some(tip),
                provider_id: String::new(),
                active: false,
            });
            last_timeline_role = Some(timeline_role);
        }

        // Chronological order: oldest first, newest at bottom
        // Cursor starts at the most recent message (bottom)
        let last_index = items.len().saturating_sub(1);

        let was_visible = self.dialog.is_visible();
        let mut dialog = Dialog::with_items("Timeline", items).with_position(DialogPosition::Right);
        dialog.selected_index = last_index;
        dialog.adjust_scroll();
        dialog = dialog.with_actions(vec![FooterAction {
            label: "Jump actions".to_string(),
            key: "enter".to_string(),
        }]);

        if was_visible {
            dialog.show();
        }

        self.dialog = dialog;
    }

    pub fn show(&mut self) {
        self.dialog.show();
    }

    pub fn hide(&mut self) {
        self.dialog.hide();
    }
}

fn message_preview(message: &Message) -> String {
    message
        .content
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            let trimmed = line.trim();
            let truncated: String = trimmed.chars().take(20).collect();
            if truncated.len() < trimmed.len() {
                format!("{}...", truncated)
            } else {
                truncated
            }
        })
        .unwrap_or_else(|| "(empty)".to_string())
}

pub fn init_timeline_dialog() -> TimelineDialogState {
    TimelineDialogState::new()
}

pub fn render_timeline_dialog(
    f: &mut Frame,
    state: &mut TimelineDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    state.dialog.render(f, area, colors);
}

pub fn handle_timeline_dialog_key_event(
    state: &mut TimelineDialogState,
    event: KeyEvent,
) -> TimelineDialogAction {
    let was_visible = state.dialog.is_visible();
    let prev_selected = state.dialog.selected_index;

    let handled = state.dialog.handle_key_event(event);

    if was_visible && !state.dialog.is_visible() {
        return TimelineDialogAction::Close;
    }

    if event.code == KeyCode::Enter && was_visible {
        if let Some(selected) = state.dialog.get_selected() {
            if let Ok(idx) = selected.id.parse::<usize>() {
                return TimelineDialogAction::Select(idx);
            }
        }
    }

    // Detect navigation (up/down changed selection)
    if handled && state.dialog.selected_index != prev_selected {
        if let Some(selected) = state.dialog.get_selected() {
            if let Ok(idx) = selected.id.parse::<usize>() {
                return TimelineDialogAction::Navigate(idx);
            }
        }
    }

    if handled {
        TimelineDialogAction::Handled
    } else {
        TimelineDialogAction::NotHandled
    }
}

pub fn handle_timeline_dialog_mouse_event(
    state: &mut TimelineDialogState,
    event: MouseEvent,
) -> TimelineDialogAction {
    let was_visible = state.dialog.is_visible();
    let prev_selected = state.dialog.selected_index;
    let clicked_item = if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        state.dialog.item_index_at_position(event.column, event.row)
    } else {
        None
    };

    let handled = state.dialog.handle_mouse_event(event);

    if was_visible && !state.dialog.is_visible() {
        return TimelineDialogAction::Close;
    }

    if clicked_item.is_some() {
        if let Some(selected) = state.dialog.get_selected() {
            if let Ok(idx) = selected.id.parse::<usize>() {
                return TimelineDialogAction::Select(idx);
            }
        }
    }

    if handled && state.dialog.selected_index != prev_selected {
        if let Some(selected) = state.dialog.get_selected() {
            if let Ok(idx) = selected.id.parse::<usize>() {
                return TimelineDialogAction::Navigate(idx);
            }
        }
    }

    if handled {
        TimelineDialogAction::Handled
    } else {
        TimelineDialogAction::NotHandled
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineDialogAction {
    Handled,
    NotHandled,
    Close,
    Select(usize),
    Navigate(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn item_names(state: &TimelineDialogState) -> Vec<String> {
        state
            .dialog
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect()
    }

    fn item_ids(state: &TimelineDialogState) -> Vec<String> {
        state
            .dialog
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect()
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
    fn assistant_segments_between_user_messages_collapse_into_one_timeline_item() {
        let messages = vec![
            Message::user("Ask me 4 questions"),
            Message::assistant(""),
            Message::tool("question tool panel"),
            Message::assistant(""),
            Message::tool("another tool panel"),
            Message::assistant("Final answer after tools"),
            Message::user("Next prompt"),
            Message::assistant("Next response"),
        ];

        let state = TimelineDialogState::build_from_messages(&messages);

        assert_eq!(
            item_names(&state),
            vec![
                "You: Ask me 4 questions",
                "Agent: Final answer after t...",
                "You: Next prompt",
                "Agent: Next response",
            ]
        );
        assert_eq!(item_ids(&state), vec!["0", "5", "6", "7"]);
    }

    #[test]
    fn assistant_segments_without_visible_text_are_hidden() {
        let messages = vec![
            Message::user("Run tools"),
            Message::assistant(""),
            Message::tool("tool call"),
            Message::assistant(""),
        ];

        let state = TimelineDialogState::build_from_messages(&messages);

        assert_eq!(item_names(&state), vec!["You: Run tools"]);
        assert_eq!(item_ids(&state), vec!["0"]);
    }

    #[test]
    fn timeline_does_not_show_agent_when_latest_message_is_user() {
        let messages = vec![
            Message::user("Earlier prompt"),
            Message::assistant("Earlier answer"),
            Message::user("Fresh prompt"),
        ];

        let state = TimelineDialogState::build_from_messages(&messages);

        assert_eq!(
            item_names(&state),
            vec![
                "You: Earlier prompt",
                "Agent: Earlier answer",
                "You: Fresh prompt",
            ]
        );
        assert_eq!(item_ids(&state), vec!["0", "1", "2"]);
    }

    #[test]
    fn timeline_hides_trailing_empty_incomplete_assistant_placeholder() {
        let messages = vec![
            Message::user("Earlier prompt"),
            Message::assistant("Earlier answer"),
            Message::user("Fresh prompt"),
            Message::incomplete(""),
        ];

        let state = TimelineDialogState::build_from_messages(&messages);

        assert_eq!(
            item_names(&state),
            vec![
                "You: Earlier prompt",
                "Agent: Earlier answer",
                "You: Fresh prompt",
            ]
        );
        assert_eq!(item_ids(&state), vec!["0", "1", "2"]);
    }

    #[test]
    fn timeline_hides_incomplete_assistant_while_streaming() {
        let messages = vec![
            Message::user("Fresh prompt"),
            Message::incomplete("partial"),
        ];

        let state = TimelineDialogState::build_from_messages(&messages);

        assert_eq!(item_names(&state), vec!["You: Fresh prompt"]);
        assert_eq!(item_ids(&state), vec!["0"]);
    }

    #[test]
    fn mouse_click_on_item_selects_message() {
        let messages = vec![
            Message::user("First prompt"),
            Message::assistant("First answer"),
        ];
        let mut state = TimelineDialogState::build_from_messages(&messages);
        state.show();
        state.dialog.dialog_area = Rect {
            x: 0,
            y: 0,
            width: 45,
            height: 30,
        };

        let action = handle_timeline_dialog_mouse_event(&mut state, left_click(2, 5));

        assert_eq!(action, TimelineDialogAction::Select(0));
    }

    #[test]
    fn mouse_click_outside_closes_timeline() {
        let messages = vec![Message::user("First prompt")];
        let mut state = TimelineDialogState::build_from_messages(&messages);
        state.show();
        state.dialog.dialog_area = Rect {
            x: 10,
            y: 0,
            width: 45,
            height: 30,
        };

        let action = handle_timeline_dialog_mouse_event(&mut state, left_click(2, 6));

        assert_eq!(action, TimelineDialogAction::Close);
        assert!(!state.dialog.is_visible());
    }
}
