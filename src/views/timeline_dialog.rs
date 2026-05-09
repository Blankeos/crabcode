use crate::session::types::{Message, MessageRole};
use crate::theme::ThemeColors;
use crate::ui::components::dialog::{Dialog, DialogAction as FooterAction, DialogItem, DialogPosition};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

#[derive(Debug)]
pub struct TimelineDialogState {
    pub dialog: Dialog,
}

impl TimelineDialogState {
    pub fn new() -> Self {
        let mut dialog = Dialog::new("Timeline").with_position(DialogPosition::Right);
        dialog = dialog.with_actions(vec![
            FooterAction {
                label: "Jump".to_string(),
                key: "enter".to_string(),
            },
        ]);
        Self { dialog }
    }

    pub fn build_from_messages(
        messages: &[Message],
        model: &str,
    ) -> Self {
        let mut state = Self::new();
        state.refresh_messages(messages, model);
        state
    }

    pub fn refresh_messages(&mut self, messages: &[Message], model: &str) {
        let mut items: Vec<DialogItem> = Vec::new();

        for (idx, message) in messages.iter().enumerate() {
            match message.role {
                MessageRole::User | MessageRole::Assistant => {}
                _ => continue,
            }

            let role_label = match message.role {
                MessageRole::User => "You",
                MessageRole::Assistant => "Agent",
                _ => unreachable!(),
            };

            let preview = message
                .content
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| {
                    let trimmed = line.trim();
                    if trimmed.len() > 60 {
                        format!("{}…", &trimmed[..60])
                    } else {
                        trimmed.to_string()
                    }
                })
                .unwrap_or_else(|| "(empty)".to_string());

            let name = format!("{}: {}", role_label, preview);

            let description = match message.role {
                MessageRole::Assistant => {
                    let m = message.model.as_deref().unwrap_or(model);
                    if message.is_complete {
                        format!("{}", m)
                    } else {
                        format!("{} · streaming", m)
                    }
                }
                MessageRole::User => message
                    .agent_mode
                    .as_deref()
                    .unwrap_or("")
                    .to_string(),
                _ => String::new(),
            };

            let tip = {
                let duration = message
                    .timestamp
                    .elapsed()
                    .unwrap_or_default();
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
            });
        }

        // Chronological order: oldest first, newest at bottom
        // Cursor starts at the most recent message (bottom)
        let last_index = items.len().saturating_sub(1);

        let was_visible = self.dialog.is_visible();
        let mut dialog = Dialog::with_items("Timeline", items)
            .with_position(DialogPosition::Right);
        dialog.selected_index = last_index;
        dialog = dialog.with_actions(vec![
            FooterAction {
                label: "Jump".to_string(),
                key: "enter".to_string(),
            },
        ]);

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
) -> Option<usize> {
    let prev_selected = state.dialog.selected_index;
    let handled = state.dialog.handle_mouse_event(event);

    if !state.dialog.is_visible() {
        return None;
    }

    // On click selection, return the selected message index
    if handled && state.dialog.selected_index != prev_selected {
        if let Some(selected) = state.dialog.get_selected() {
            if let Ok(idx) = selected.id.parse::<usize>() {
                return Some(idx);
            }
        }
    }

    None
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineDialogAction {
    Handled,
    NotHandled,
    Close,
    Select(usize),
    Navigate(usize),
}
