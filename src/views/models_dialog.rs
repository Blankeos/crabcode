use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

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
    CycleReasoning {
        provider_id: String,
        model_id: String,
        direction: i8,
    },
    None,
}

fn render_loading_message(f: &mut Frame, dialog: &Dialog, colors: ThemeColors, message: &str) {
    let area = Rect {
        x: dialog.content_area.x,
        y: dialog.content_area.y + dialog.content_area.height / 2,
        width: dialog.content_area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(colors.text_weak))
            .alignment(Alignment::Center),
        area,
    );
}

pub fn render_refresh_models_dialog(f: &mut Frame, area: Rect, colors: ThemeColors, frame: usize) {
    let width = area.width.min(42);
    let height = area.height.min(5);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        popup,
    );
    let content = Rect {
        x: popup.x + 2,
        y: popup.y + 1,
        width: popup.width.saturating_sub(4),
        height: popup.height.saturating_sub(2),
    };
    let glyph = crate::views::sessions_dialog::session_loading_glyph(frame);
    let lines = vec![
        Line::from(Span::styled(
            "Refreshing models",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("{} Updating model caches...", glyph),
            Style::default().fg(colors.text_weak),
        )),
        Line::from(Span::styled(
            "esc  close",
            Style::default().fg(colors.primary),
        )),
    ];
    f.render_widget(Paragraph::new(lines), content);
}

#[derive(Debug)]
pub struct ModelsDialogState {
    pub dialog: Dialog,
    loading: bool,
}

impl ModelsDialogState {
    pub fn new(dialog: Dialog) -> Self {
        Self {
            dialog,
            loading: false,
        }
    }

    pub fn with_items(title: impl Into<String>, items: Vec<DialogItem>) -> Self {
        Self {
            dialog: Dialog::with_items(title, items)
                .with_search_priority_groups(vec!["Favorite".to_string()])
                .with_actions(base_actions()),
            loading: false,
        }
    }

    pub fn start_loading(&mut self) {
        self.loading = true;
    }

    pub fn finish_loading(&mut self) {
        self.loading = false;
    }

    pub fn is_loading(&self) -> bool {
        self.loading
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

        self.dialog = Dialog::with_items(title, items)
            .with_search_priority_groups(vec!["Favorite".to_string()])
            .with_actions(actions);

        if was_visible {
            self.dialog.show();
        }

        self.dialog.restore_search_query(search_query);

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
    reasoning_effort: Option<&str>,
) {
    if dialog_state.loading {
        dialog_state.dialog.actions.clear();
        dialog_state.dialog.set_bottom_gap_height(1);
        dialog_state.dialog.render(f, area, colors);
        render_loading_message(f, &dialog_state.dialog, colors, "Loading models...");
        return;
    }

    dialog_state.dialog.actions = base_actions();
    dialog_state
        .dialog
        .set_bottom_gap_height(if reasoning_effort.is_some() { 3 } else { 1 });
    dialog_state.dialog.render(f, area, colors);

    if let Some(reasoning_effort) = reasoning_effort {
        render_reasoning_control(f, &dialog_state.dialog, colors, reasoning_effort);
    }
}

fn base_actions() -> Vec<DialogAction> {
    vec![
        DialogAction {
            label: "Connect provider".to_string(),
            key: "ctrl+a".to_string(),
        },
        DialogAction {
            label: "Favorite".to_string(),
            key: "ctrl+f".to_string(),
        },
    ]
}

fn render_reasoning_control(
    f: &mut Frame,
    dialog: &Dialog,
    colors: ThemeColors,
    reasoning_effort: &str,
) {
    let gap_height = 3;
    if dialog.content_area.height < gap_height + dialog.footer_height() {
        return;
    }

    let gap_area = Rect {
        x: dialog.content_area.x,
        y: dialog.content_area.y
            + dialog
                .content_area
                .height
                .saturating_sub(dialog.footer_height() + gap_height),
        width: dialog.content_area.width,
        height: gap_height,
    };
    let control_area = Rect {
        x: gap_area.x,
        y: gap_area.y + 1,
        width: gap_area.width,
        height: 1,
    };
    let line = reasoning_control_line(reasoning_effort, control_area.width, colors);

    f.render_widget(
        Paragraph::new(line).alignment(Alignment::Left),
        control_area,
    );
}

fn reasoning_control_line<'a>(
    reasoning_effort: &'a str,
    width: u16,
    colors: ThemeColors,
) -> Line<'a> {
    let width = width as usize;
    let effort_width = reasoning_effort.len();

    if width <= effort_width + 2 {
        return Line::from(vec![Span::styled(
            reasoning_effort.to_string(),
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )]);
    }

    let effort_start = width.saturating_sub(effort_width) / 2;
    let right_start = width.saturating_sub(1);
    let spaces_after_left = effort_start.saturating_sub(1);
    let used_through_effort = 1 + spaces_after_left + effort_width;
    let spaces_after_effort = right_start.saturating_sub(used_through_effort);

    Line::from(vec![
        Span::styled(
            "<",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(spaces_after_left)),
        Span::styled(
            reasoning_effort.to_string(),
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(spaces_after_effort)),
        Span::styled(
            ">",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

pub fn handle_models_dialog_key_event(
    dialog_state: &mut ModelsDialogState,
    event: KeyEvent,
) -> ModelsDialogAction {
    if !dialog_state.dialog.is_visible() {
        return ModelsDialogAction::None;
    }

    if dialog_state.loading {
        if event.code == KeyCode::Esc {
            dialog_state.dialog.hide();
        }
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
        KeyCode::Left | KeyCode::Right
            if event.modifiers == KeyModifiers::NONE
                || event.modifiers == KeyModifiers::CONTROL =>
        {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return ModelsDialogAction::CycleReasoning {
                    provider_id: selected.provider_id.clone(),
                    model_id: selected.id.clone(),
                    direction: if event.code == KeyCode::Left { -1 } else { 1 },
                };
            }
        }
        KeyCode::Char('t') if event.modifiers == KeyModifiers::CONTROL => {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return ModelsDialogAction::CycleReasoning {
                    provider_id: selected.provider_id.clone(),
                    model_id: selected.id.clone(),
                    direction: 1,
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
    if !dialog_state.dialog.is_visible() || dialog_state.loading {
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
            active: false,
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

    const CENTER_DIALOG_LIST_Y: u16 = 6;

    #[test]
    fn search_prioritizes_favorite_models() {
        let mut favorite = model_item("gpt-5", "GPT-5", "openai");
        favorite.group = "Favorite".to_string();

        let mut state = init_models_dialog(
            "Models",
            vec![
                model_item("gpt-4o", "GPT-4o", "openai"),
                favorite,
                model_item("claude-sonnet", "Claude Sonnet", "anthropic"),
            ],
        );

        state.dialog.set_search_query("gpt");

        assert_eq!(state.dialog.filtered_items.len(), 2);
        assert_eq!(state.dialog.filtered_items[0].0, "Favorite");
        assert_eq!(state.dialog.filtered_items[0].1[0].id, "gpt-5");
        assert_eq!(state.dialog.filtered_items[1].1[0].id, "gpt-4o");
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

        let action =
            handle_models_dialog_mouse_event(&mut state, left_click(4, CENTER_DIALOG_LIST_Y + 2));

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

        let action =
            handle_models_dialog_mouse_event(&mut state, left_click(4, CENTER_DIALOG_LIST_Y));

        assert_eq!(action, ModelsDialogAction::None);
        assert!(state.dialog.is_visible());
    }

    #[test]
    fn left_and_right_cycle_reasoning_for_selected_model() {
        let mut state = init_models_dialog(
            "Models",
            vec![
                model_item("gpt-5", "GPT-5", "openai"),
                model_item("claude-sonnet", "Claude Sonnet", "anthropic"),
            ],
        );
        state.dialog.show();
        state.dialog.next();

        let right = handle_models_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(
            right,
            ModelsDialogAction::CycleReasoning {
                provider_id: "anthropic".to_string(),
                model_id: "claude-sonnet".to_string(),
                direction: 1,
            }
        );

        let left = handle_models_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(
            left,
            ModelsDialogAction::CycleReasoning {
                provider_id: "anthropic".to_string(),
                model_id: "claude-sonnet".to_string(),
                direction: -1,
            }
        );
    }

    #[test]
    fn ctrl_t_cycles_reasoning_for_selected_model() {
        let mut state = init_models_dialog("Models", vec![model_item("gpt-5", "GPT-5", "openai")]);
        state.dialog.show();

        let action = handle_models_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        );

        assert_eq!(
            action,
            ModelsDialogAction::CycleReasoning {
                provider_id: "openai".to_string(),
                model_id: "gpt-5".to_string(),
                direction: 1,
            }
        );
    }

    #[test]
    fn footer_actions_do_not_include_reasoning_control() {
        let actions = base_actions();
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|action| action.label != "Reasoning"));
    }

    #[test]
    fn reasoning_control_line_spreads_arrows_and_value() {
        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let line = reasoning_control_line("xhigh", 21, colors);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(rendered.len(), 21);
        assert!(rendered.starts_with('<'));
        assert!(rendered.ends_with('>'));
        assert_eq!(rendered.find("xhigh"), Some((21 - "xhigh".len()) / 2));
    }

    #[test]
    fn selected_last_reasoning_model_stays_visible_above_control() {
        use ratatui::{backend::TestBackend, Terminal};

        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let mut state = init_models_dialog(
            "Models",
            (0..24)
                .map(|idx| model_item(&idx.to_string(), &format!("Model {idx}"), "openai"))
                .collect(),
        );
        state.dialog.show();
        state.dialog.select_index_clamped(23);

        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_models_dialog(
                    frame,
                    &mut state,
                    Rect::new(0, 0, 80, 30),
                    colors,
                    Some("high"),
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let selected_row = (0..buffer.area.height)
            .find(|&y| {
                let row_text = (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>();
                row_text.contains("Model 23")
            })
            .expect("last model row should be visible");

        assert!((0..buffer.area.width).any(|x| buffer
            .cell((x, selected_row))
            .is_some_and(|cell| cell.style().bg == Some(colors.primary))));
    }
    #[test]
    fn loading_dialog_renders_message_and_escape_closes_it() {
        use ratatui::{backend::TestBackend, Terminal};

        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let mut state = init_models_dialog("Available Models", vec![]);
        state.dialog.show();
        state.start_loading();
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                render_models_dialog(frame, &mut state, Rect::new(0, 0, 80, 30), colors, None);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Loading models..."));

        let action = handle_models_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(action, ModelsDialogAction::None);
        assert!(!state.dialog.is_visible());
    }

    #[test]
    fn compact_refresh_dialog_renders_progress() {
        use ratatui::{backend::TestBackend, Terminal};

        let colors = crate::theme::Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_refresh_models_dialog(frame, Rect::new(0, 0, 80, 24), colors, 0);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Refreshing models"));
        assert!(rendered.contains("Updating model caches..."));
    }
}
