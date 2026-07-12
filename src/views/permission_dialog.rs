use crate::theme::{contrast_text, ThemeColors};
use crate::tools::{PermissionAction, PermissionGrant, PermissionPrompt, PermissionResponse};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap},
    Frame,
};
use std::collections::VecDeque;

const PERMISSION_DIALOG_MIN_HEIGHT: u16 = 11;
const PERMISSION_DIALOG_MAX_HEIGHT: u16 = 18;
const PERMISSION_DIALOG_CHROME_HEIGHT: u16 = 8;

#[derive(Default)]
pub struct PermissionDialogState {
    current: Option<PermissionPrompt>,
    queue: VecDeque<PermissionPrompt>,
    selected_action: usize,
    action_hitboxes: Vec<(Rect, PermissionResponse)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionPromptSnapshot {
    pub tool_id: String,
    pub action: String,
    pub permission: String,
    pub patterns: Vec<String>,
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
            action_hitboxes: Vec::new(),
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
            permission: prompt.permission.clone(),
            patterns: prompt.patterns.clone(),
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
            if response == PermissionResponse::AllowAlways {
                let grant = PermissionGrant {
                    permission: prompt.permission.clone(),
                    patterns: prompt.patterns.clone(),
                };
                let mut remaining = VecDeque::new();
                while let Some(queued) = self.queue.pop_front() {
                    let queued_grant = PermissionGrant {
                        permission: queued.permission.clone(),
                        patterns: queued.patterns.clone(),
                    };
                    if grant.matches(&queued_grant) {
                        let _ = queued.response_tx.send(PermissionResponse::AllowAlways);
                    } else {
                        remaining.push_back(queued);
                    }
                }
                self.queue = remaining;
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        KeyCode::Left | KeyCode::Up => {
            state.previous_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
            state.next_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Char('h') | KeyCode::Char('k') => {
            state.previous_action();
            PermissionDialogAction::Handled
        }
        KeyCode::Char('l') | KeyCode::Char('j') => {
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
    state: &mut PermissionDialogState,
    event: MouseEvent,
) -> PermissionDialogAction {
    if !state.has_active() {
        return PermissionDialogAction::NotHandled;
    }

    let point = Position::new(event.column, event.row);
    let response = state
        .action_hitboxes
        .iter()
        .find(|(area, _)| area.contains(point))
        .map(|(_, response)| *response);
    let Some(response) = response else {
        return PermissionDialogAction::NotHandled;
    };

    if matches!(event.kind, MouseEventKind::Moved) {
        state.selected_action = match response {
            PermissionResponse::Deny => 0,
            PermissionResponse::AllowOnce => 1,
            PermissionResponse::AllowAlways => 2,
        };
        return PermissionDialogAction::Handled;
    }

    if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
        return PermissionDialogAction::Respond(response);
    }

    PermissionDialogAction::NotHandled
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
        let target = permission_display_target(prompt).unwrap_or_else(|| "(none)".to_string());
        details.push(Line::from(vec![
            Span::styled("Target ", label_style),
            Span::styled(target, value_style),
        ]));
    }

    if !prompt.patterns.is_empty() {
        details.push(Line::from(vec![
            Span::styled("Patterns ", label_style),
            Span::styled(prompt.patterns.join(", "), value_style),
        ]));
    }

    details
}

fn permission_display_target(prompt: &PermissionPrompt) -> Option<String> {
    if prompt.permission == "external_directory" {
        prompt
            .patterns
            .first()
            .cloned()
            .or_else(|| prompt.target.clone())
    } else {
        prompt.target.clone()
    }
}

fn permission_action_lines(colors: ThemeColors, selected_action: usize) -> Vec<Line<'static>> {
    let actions = [
        (1usize, "Allow once", "Approve this single request", "1"),
        (2usize, "Allow always", "Remember matching requests", "2"),
        (0usize, "Reject", "Deny and return to the agent", "3"),
    ];
    let selected_style = Style::default()
        .bg(colors.info)
        .fg(contrast_text(colors.info))
        .add_modifier(Modifier::BOLD);

    actions
        .into_iter()
        .map(|(action_index, label, description, key)| {
            let is_selected = action_index == selected_action;
            let label_style = if is_selected {
                selected_style
            } else {
                Style::default().fg(colors.text)
            };
            let weak_style = if is_selected {
                selected_style
            } else {
                Style::default()
                    .fg(colors.text_weak)
                    .add_modifier(Modifier::DIM)
            };

            Line::from(vec![
                Span::styled("( ) ", weak_style),
                Span::styled(label.to_string(), label_style),
                Span::styled(format!(" ({})", key), weak_style),
                Span::styled(" - ", weak_style),
                Span::styled(description.to_string(), weak_style),
            ])
        })
        .collect()
}

fn permission_dialog_body_width(area_width: u16) -> u16 {
    // The dialog has a left border plus left/right padding before the body.
    area_width.saturating_sub(3).max(1)
}

fn wrapped_lines_height(lines: &[Line<'_>], width: u16) -> u16 {
    let width = usize::from(width.max(1));

    lines
        .iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            if text.is_empty() {
                1
            } else {
                text.lines()
                    .map(|part| textwrap::wrap(part, width).len().max(1) as u16)
                    .sum()
            }
        })
        .sum()
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
    let detail_line_count =
        wrapped_lines_height(&details, permission_dialog_body_width(area.width));
    let desired_height = detail_line_count
        .saturating_add(PERMISSION_DIALOG_CHROME_HEIGHT)
        .clamp(PERMISSION_DIALOG_MIN_HEIGHT, PERMISSION_DIALOG_MAX_HEIGHT);
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

    let fixed_content_height = 1 + 1 + 1 + 3 + 1;
    let detail_height = detail_line_count
        .min(content_area.height.saturating_sub(fixed_content_height))
        .max(1);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(detail_height),
            Constraint::Length(1),
            Constraint::Length(3),
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
    f.render_widget(detail_block, chunks[2]);

    let action_lines = permission_action_lines(colors, state.selected_action);

    let help = Line::from(vec![
        Span::styled("↑↓", Style::default().fg(colors.info)),
        Span::raw(" move  "),
        Span::styled("enter", Style::default().fg(colors.info)),
        Span::raw(" confirm"),
    ]);

    let actions_block = Paragraph::new(action_lines)
        .style(Style::default().bg(colors.dialog_background))
        .alignment(Alignment::Left);
    let help_width = help.width() as u16;
    f.render_widget(actions_block, chunks[4]);
    state.action_hitboxes = [
        PermissionResponse::AllowOnce,
        PermissionResponse::AllowAlways,
        PermissionResponse::Deny,
    ]
    .into_iter()
    .enumerate()
    .filter_map(|(row, response)| {
        (row < chunks[4].height as usize).then_some((
            Rect {
                x: chunks[4].x,
                y: chunks[4].y.saturating_add(row as u16),
                width: chunks[4].width,
                height: 1,
            },
            response,
        ))
    })
    .collect();

    let can_render_help = chunks[5].width > 42;
    if can_render_help {
        let footer_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(help_width.min(chunks[5].width.saturating_sub(20))),
            ])
            .split(chunks[5]);
        f.render_widget(
            Paragraph::new(help).alignment(Alignment::Right),
            footer_chunks[1],
        );
    } else {
        f.render_widget(Paragraph::new(help).alignment(Alignment::Left), chunks[5]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::crossterm::event::KeyModifiers;

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
            permission: "bash".to_string(),
            patterns: vec!["cargo test".to_string()],
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
                "Workdir /tmp/workspace",
                "Patterns cargo test"
            ]
        );
        assert!(!rendered.iter().any(|line| line.contains("Target")));
    }

    #[test]
    fn external_directory_detail_target_shows_wildcard_scope() {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let prompt = PermissionPrompt {
            tool_id: "read".to_string(),
            action: PermissionAction::Read,
            permission: "external_directory".to_string(),
            patterns: vec!["/Users/carlo/Desktop/Projects/sheetpilot/*".to_string()],
            target: Some("/Users/carlo/Desktop/Projects/sheetpilot".to_string()),
            command: None,
            workdir: None,
            reason: "Tool 'read' wants to access path outside working directory".to_string(),
            response_tx,
        };
        let colors = Theme::load_builtin_default().get_colors(true);

        let rendered = permission_detail_lines(&prompt, colors)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(rendered.contains(&"Target /Users/carlo/Desktop/Projects/sheetpilot/*".to_string()));
    }

    #[test]
    fn current_snapshot_exposes_remote_safe_prompt_details() {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let mut state = PermissionDialogState::new();
        state.enqueue(PermissionPrompt {
            tool_id: "bash".to_string(),
            action: PermissionAction::Bash,
            permission: "bash".to_string(),
            patterns: vec!["cargo test".to_string()],
            target: Some("cargo test".to_string()),
            command: Some("cargo test".to_string()),
            workdir: Some("/tmp/workspace".to_string()),
            reason: "Bash command execution requires permission".to_string(),
            response_tx,
        });

        let snapshot = state.current_snapshot().unwrap();
        assert_eq!(snapshot.tool_id, "bash");
        assert_eq!(snapshot.action, "bash");
        assert_eq!(snapshot.permission, "bash");
        assert_eq!(snapshot.patterns, vec!["cargo test"]);
        assert_eq!(snapshot.command.as_deref(), Some("cargo test"));
        assert_eq!(snapshot.queued_count, 0);
    }

    #[test]
    fn action_lines_render_as_vertical_radio_options() {
        let colors = Theme::load_builtin_default().get_colors(true);
        let rendered = permission_action_lines(colors, 1)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert_eq!(rendered.len(), 3);
        assert_eq!(
            rendered,
            vec![
                "( ) Allow once (1) - Approve this single request",
                "( ) Allow always (2) - Remember matching requests",
                "( ) Reject (3) - Deny and return to the agent"
            ]
        );
    }

    #[test]
    fn vertical_keys_move_selected_action() {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let mut state = PermissionDialogState::new();
        state.enqueue(PermissionPrompt {
            tool_id: "read".to_string(),
            action: PermissionAction::Read,
            permission: "read".to_string(),
            patterns: vec!["/tmp/file".to_string()],
            target: Some("/tmp/file".to_string()),
            command: None,
            workdir: None,
            reason: "explicit approval required".to_string(),
            response_tx,
        });

        handle_permission_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(state.selected_action, 2);

        handle_permission_dialog_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        );
        assert_eq!(state.selected_action, 1);
    }

    #[test]
    fn render_expands_for_wrapped_details_so_reject_stays_visible() {
        use ratatui::{backend::TestBackend, Terminal};

        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let mut state = PermissionDialogState::new();
        state.enqueue(PermissionPrompt {
            tool_id: "read".to_string(),
            action: PermissionAction::Read,
            permission: "external_directory".to_string(),
            patterns: vec!["/Users/carlo/Desktop/Projects/sheetpilot/*".to_string()],
            target: Some("/Users/carlo/Desktop/Projects/sheetpilot/README.md".to_string()),
            command: None,
            workdir: None,
            reason: "Tool 'read' wants to access path outside working directory: /Users/carlo/Desktop/Projects/sheetpilot/README.md".to_string(),
            response_tx,
        });
        let colors = Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(64, 16);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| render_permission_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Allow once"));
        assert!(rendered.contains("Allow always"));
        assert!(rendered.contains("Reject"));
    }

    #[test]
    fn clicking_permission_action_returns_response() {
        use ratatui::{backend::TestBackend, Terminal};

        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let mut state = PermissionDialogState::new();
        state.enqueue(PermissionPrompt {
            tool_id: "read".to_string(),
            action: PermissionAction::Read,
            permission: "read".to_string(),
            patterns: vec!["/tmp/file".to_string()],
            target: Some("/tmp/file".to_string()),
            command: None,
            workdir: None,
            reason: "explicit approval required".to_string(),
            response_tx,
        });
        let colors = Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(64, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_permission_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let reject_area = state.action_hitboxes[2].0;
        let action = handle_permission_dialog_mouse_event(
            &mut state,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: reject_area.x,
                row: reject_area.y,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(
            action,
            PermissionDialogAction::Respond(PermissionResponse::Deny)
        );
    }

    #[test]
    fn hovering_permission_action_updates_selection_without_responding() {
        use ratatui::{backend::TestBackend, Terminal};

        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        let mut state = PermissionDialogState::new();
        state.enqueue(PermissionPrompt {
            tool_id: "read".to_string(),
            action: PermissionAction::Read,
            permission: "read".to_string(),
            patterns: vec!["/tmp/file".to_string()],
            target: Some("/tmp/file".to_string()),
            command: None,
            workdir: None,
            reason: "explicit approval required".to_string(),
            response_tx,
        });
        let colors = Theme::load_builtin_default().get_colors(true);
        let backend = TestBackend::new(64, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_permission_dialog(frame, &mut state, frame.area(), colors))
            .unwrap();

        let always_area = state.action_hitboxes[1].0;
        let action = handle_permission_dialog_mouse_event(
            &mut state,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: always_area.x,
                row: always_area.y,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(action, PermissionDialogAction::Handled);
        assert_eq!(state.selected_action, 2);
        assert!(state.has_active());
    }
}
