use crate::theme::{contrast_text, ThemeColors};
use crate::tools::{PermissionPrompt, PermissionResponse};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
    Frame,
};
use std::collections::VecDeque;

#[derive(Default)]
pub struct PermissionDialogState {
    current: Option<PermissionPrompt>,
    queue: VecDeque<PermissionPrompt>,
    selected_action: usize,
}

impl PermissionDialogState {
    pub fn new() -> Self {
        Self {
            current: None,
            queue: VecDeque::new(),
            selected_action: 1,
        }
    }

    pub fn enqueue(&mut self, prompt: PermissionPrompt) {
        if self.current.is_none() {
            self.current = Some(prompt);
            self.selected_action = 1;
        } else {
            self.queue.push_back(prompt);
        }
    }

    pub fn has_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn next_action(&mut self) {
        self.selected_action = (self.selected_action + 1) % 3;
    }

    pub fn previous_action(&mut self) {
        self.selected_action = if self.selected_action == 0 {
            2
        } else {
            self.selected_action - 1
        };
    }

    pub fn selected_response(&self) -> PermissionResponse {
        match self.selected_action {
            0 => PermissionResponse::Deny,
            1 => PermissionResponse::AllowOnce,
            _ => PermissionResponse::AllowAlways,
        }
    }

    pub fn respond_current(&mut self, response: PermissionResponse) {
        if let Some(prompt) = self.current.take() {
            let _ = prompt.response_tx.send(response);
        }

        self.current = self.queue.pop_front();
        if self.current.is_some() {
            self.selected_action = 1;
        }
    }

    pub fn deny_current(&mut self) {
        self.respond_current(PermissionResponse::Deny);
    }

    pub fn clear_with_deny(&mut self) {
        if let Some(prompt) = self.current.take() {
            let _ = prompt.response_tx.send(PermissionResponse::Deny);
        }

        while let Some(prompt) = self.queue.pop_front() {
            let _ = prompt.response_tx.send(PermissionResponse::Deny);
        }

        self.selected_action = 1;
    }
}

pub enum PermissionDialogAction {
    Respond(PermissionResponse),
    Handled,
    NotHandled,
}

pub fn init_permission_dialog() -> PermissionDialogState {
    PermissionDialogState::new()
}

pub fn handle_permission_dialog_key_event(
    state: &mut PermissionDialogState,
    event: KeyEvent,
) -> PermissionDialogAction {
    if !state.has_active() {
        return PermissionDialogAction::NotHandled;
    }

    match event.code {
        KeyCode::Esc => PermissionDialogAction::Respond(PermissionResponse::Deny),
        KeyCode::Left => {
            state.previous_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Right | KeyCode::Tab => {
            state.next_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Char('h') => {
            state.previous_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Char('l') => {
            state.next_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Char('1') => PermissionDialogAction::Respond(PermissionResponse::Deny),
        KeyCode::Char('2') => PermissionDialogAction::Respond(PermissionResponse::AllowOnce),
        KeyCode::Char('3') => PermissionDialogAction::Respond(PermissionResponse::AllowAlways),
        KeyCode::Enter => PermissionDialogAction::Respond(state.selected_response()),
        _ => PermissionDialogAction::NotHandled,
    }
}

pub fn handle_permission_dialog_mouse_event(
    _state: &mut PermissionDialogState,
    _event: MouseEvent,
) -> bool {
    false
}

pub fn render_permission_dialog(
    f: &mut Frame,
    state: &mut PermissionDialogState,
    area: Rect,
    colors: ThemeColors,
) {
    let Some(prompt) = state.current.as_ref() else {
        return;
    };

    let width = area.width.min(78).max(54).min(area.width);
    let height = area.height.min(17).max(12).min(area.height);
    let dialog_area = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };

    f.render_widget(Clear, dialog_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        dialog_area,
    );

    const PADDING: u16 = 3;
    let content_area = Rect {
        x: dialog_area.x + PADDING,
        y: dialog_area.y + PADDING,
        width: dialog_area.width.saturating_sub(PADDING * 2),
        height: dialog_area.height.saturating_sub(PADDING * 2),
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(content_area);

    let esc_text = "esc";
    let esc_area_width = (esc_text.len() as u16).saturating_add(1);
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(esc_area_width)])
        .split(chunks[0]);

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Permission required",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )])),
        header_chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            esc_text,
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    let target = prompt
        .target
        .as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "(none)".to_string());
    let summary = Line::from(vec![
        Span::styled(
            "Tool",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::raw(" "),
        Span::styled(
            prompt.tool_id.clone(),
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  •  ",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(
            "Target",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::raw(" "),
        Span::styled(target, Style::default().fg(colors.text)),
    ]);
    f.render_widget(Paragraph::new(summary), chunks[2]);

    let reason = Paragraph::new(prompt.reason.clone())
        .style(Style::default().fg(colors.text))
        .wrap(Wrap { trim: true });
    f.render_widget(reason, chunks[3]);

    let actions = [("Deny", "1"), ("Allow Once", "2"), ("Allow Always", "3")];
    let mut action_spans = Vec::new();
    for (idx, (label, key)) in actions.iter().enumerate() {
        if idx > 0 {
            action_spans.push(Span::raw("   "));
        }

        let is_selected = idx == state.selected_action;
        if is_selected {
            let selected = Style::default()
                .bg(colors.primary)
                .fg(contrast_text(colors.primary))
                .add_modifier(Modifier::BOLD);
            action_spans.push(Span::styled(format!(" {} ({}) ", label, key), selected));
        } else {
            action_spans.push(Span::styled(
                format!("{} ", label),
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            action_spans.push(Span::styled(
                format!("({})", key),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
        }
    }

    let actions_line = Paragraph::new(Line::from(action_spans)).alignment(Alignment::Left);
    f.render_widget(actions_line, chunks[5]);

    let help = Line::from(vec![
        Span::styled(
            "Confirm",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  enter",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::raw("   "),
        Span::styled(
            "Switch",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  ⇄",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
        Span::raw("   "),
        Span::styled(
            "Deny",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  esc",
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        ),
    ]);
    let help = Paragraph::new(help).alignment(Alignment::Left);
    f.render_widget(help, chunks[7]);
}
