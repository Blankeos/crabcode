use crate::theme::{contrast_text, ThemeColors};
use crate::tools::{PermissionAction, PermissionPrompt, PermissionResponse};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
    Frame,
};
use std::collections::VecDeque;

#[derive(Default)]
pub struct PermissionDialogState {
    current: Option<PermissionPrompt>,
    queue: VecDeque<PermissionPrompt>,
    selected_action: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionPromptSnapshot {
    pub tool_id: String,
    pub action: String,
    pub target: Option<String>,
    pub command: Option<String>,
    pub workdir: Option<String>,
    pub reason: String,
    pub queued_count: usize,
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

    pub fn current_snapshot(&self) -> Option<PermissionPromptSnapshot> {
        let prompt = self.current.as_ref()?;
        Some(PermissionPromptSnapshot {
            tool_id: prompt.tool_id.clone(),
            action: permission_action_label(prompt.action).to_string(),
            target: prompt.target.clone(),
            command: prompt.command.clone(),
            workdir: prompt.workdir.clone(),
            reason: prompt.reason.clone(),
            queued_count: self.queue.len(),
        })
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

fn permission_action_label(action: PermissionAction) -> &'static str {
    match action {
        PermissionAction::Read => "read",
        PermissionAction::Write => "write",
        PermissionAction::Edit => "edit",
        PermissionAction::List => "list",
        PermissionAction::Glob => "glob",
        PermissionAction::Grep => "grep",
        PermissionAction::Bash => "bash",
        PermissionAction::Unknown => "unknown",
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
        KeyCode::Char('1') => PermissionDialogAction::Respond(PermissionResponse::AllowOnce),
        KeyCode::Char('2') => PermissionDialogAction::Respond(PermissionResponse::AllowAlways),
        KeyCode::Char('3') => PermissionDialogAction::Respond(PermissionResponse::Deny),
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

fn permission_detail_lines(prompt: &PermissionPrompt, colors: ThemeColors) -> Vec<Line<'static>> {
    let is_bash = prompt.action == PermissionAction::Bash || prompt.tool_id == "bash";
    let label_style = Style::default()
        .fg(colors.text_weak)
        .add_modifier(Modifier::DIM);
    let value_style = Style::default().fg(colors.text);
    let mut details = vec![Line::from(vec![
        Span::styled("Tool ", label_style),
        Span::styled(
            prompt.tool_id.clone(),
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" • ", label_style),
        Span::styled(prompt.reason.clone(), label_style),
    ])];

    if is_bash {
        let command = prompt
            .command
            .as_deref()
            .or(prompt.target.as_deref())
            .unwrap_or("(none)");
        details.push(Line::from(vec![
            Span::styled("Command ", label_style),
            Span::styled(command.to_string(), value_style),
        ]));

        if let Some(workdir) = prompt.workdir.as_deref() {
            details.push(Line::from(vec![
                Span::styled("Workdir ", label_style),
                Span::styled(workdir.to_string(), value_style),
            ]));
        }
    } else {
        let target = prompt
            .target
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(none)".to_string());
        details.push(Line::from(vec![
            Span::styled("Target ", label_style),
            Span::styled(target, value_style),
        ]));
    }

    details
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

    let details = permission_detail_lines(prompt, colors);
    let detail_line_count = details.len() as u16;
    let desired_height = (detail_line_count + 5).clamp(8, 10);
    let panel_height = area.height.min(desired_height);
    let dialog_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(panel_height),
        width: area.width,
        height: panel_height,
    };

    f.render_widget(Clear, dialog_area);
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(colors.dialog_background)),
        dialog_area,
    );

    let border = Block::default()
        .style(Style::default().bg(colors.dialog_background))
        .borders(Borders::LEFT)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(colors.warning))
        .padding(Padding::new(1, 1, 1, 1));
    let content_area = border.inner(dialog_area);
    f.render_widget(border, dialog_area);

    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content_area);

    let esc_text = "esc reject";
    let esc_area_width = (esc_text.len() as u16).min(chunks[0].width);
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(esc_area_width)])
        .split(chunks[0]);

    let title = if state.queue.is_empty() {
        "Permission required".to_string()
    } else {
        format!("Permission required (+{} queued)", state.queue.len())
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            title,
            Style::default()
                .fg(colors.warning)
                .add_modifier(Modifier::BOLD),
        )])),
        header_chunks[0],
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            esc_text,
            Style::default()
                .fg(colors.text_weak)
                .add_modifier(Modifier::DIM),
        )]))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    let detail_block = Paragraph::new(details)
        .style(Style::default().bg(colors.dialog_background))
        .wrap(Wrap { trim: true });
    f.render_widget(detail_block, chunks[1]);

    let actions = [
        (1usize, "Allow once", "1"),
        (2usize, "Allow always", "2"),
        (0usize, "Reject", "3"),
    ];
    let mut action_spans = Vec::new();
    for (idx, (action_index, label, key)) in actions.iter().enumerate() {
        if idx > 0 {
            action_spans.push(Span::raw("  "));
        }

        let option_text = format!(" {} ({}) ", label, key);
        let is_selected = *action_index == state.selected_action;
        if is_selected {
            let selected = Style::default()
                .bg(colors.warning)
                .fg(contrast_text(colors.warning))
                .add_modifier(Modifier::BOLD);
            action_spans.push(Span::styled(option_text, selected));
        } else {
            action_spans.push(Span::raw(" "));
            action_spans.push(Span::styled(
                *label,
                Style::default()
                    .fg(colors.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            action_spans.push(Span::raw(" "));
            action_spans.push(Span::styled(
                format!("({})", key),
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM),
            ));
            action_spans.push(Span::raw(" "));
        }
    }

    let help = Line::from(vec![
        Span::styled("⇆", Style::default().fg(colors.info)),
        Span::raw(" select  "),
        Span::styled("enter", Style::default().fg(colors.info)),
        Span::raw(" confirm"),
    ]);

    let actions_line = Paragraph::new(Line::from(action_spans)).alignment(Alignment::Left);
    let help_width = help.width() as u16;
    let can_render_help = chunks[2].width > 42;
    if can_render_help {
        let footer_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(help_width.min(chunks[2].width.saturating_sub(20))),
            ])
            .split(chunks[2]);
        f.render_widget(actions_line, footer_chunks[0]);
        f.render_widget(
            Paragraph::new(help).alignment(Alignment::Right),
            footer_chunks[1],
        );
    } else {
        f.render_widget(actions_line, chunks[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn bash_detail_lines_show_command_and_workdir() {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let prompt = PermissionPrompt {
            tool_id: "bash".to_string(),
            action: PermissionAction::Bash,
            target: Some("cargo test".to_string()),
            command: Some("cargo test".to_string()),
            workdir: Some("/tmp/workspace".to_string()),
            reason: "Bash command execution requires permission".to_string(),
            response_tx,
        };
        let colors = Theme::load_builtin_default().get_colors(true);

        let rendered = permission_detail_lines(&prompt, colors)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "Tool bash • Bash command execution requires permission",
                "Command cargo test",
                "Workdir /tmp/workspace"
            ]
        );
        assert!(!rendered.iter().any(|line| line.contains("Target")));
    }

    #[test]
    fn current_snapshot_exposes_remote_safe_prompt_details() {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let mut state = PermissionDialogState::new();
        state.enqueue(PermissionPrompt {
            tool_id: "bash".to_string(),
            action: PermissionAction::Bash,
            target: Some("cargo test".to_string()),
            command: Some("cargo test".to_string()),
            workdir: Some("/tmp/workspace".to_string()),
            reason: "Bash command execution requires permission".to_string(),
            response_tx,
        });

        let snapshot = state.current_snapshot().unwrap();
        assert_eq!(snapshot.tool_id, "bash");
        assert_eq!(snapshot.action, "bash");
        assert_eq!(snapshot.command.as_deref(), Some("cargo test"));
        assert_eq!(snapshot.queued_count, 0);
    }
}
