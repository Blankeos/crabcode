use crate::mcp::McpServerView;
use crate::theme::ThemeColors;
use ratatui::layout::{Alignment, Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct StatusDialogState {
    visible: bool,
    servers: Vec<McpServerView>,
    area: Option<Rect>,
}

impl StatusDialogState {
    pub fn show(&mut self, servers: Vec<McpServerView>) {
        self.servers = servers;
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.area = None;
    }

    pub fn contains(&self, position: ratatui::layout::Position) -> bool {
        self.area.is_some_and(|area| area.contains(position))
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

pub fn render_status_dialog(
    frame: &mut Frame<'_>,
    state: &mut StatusDialogState,
    area: Rect,
    colors: &ThemeColors,
) {
    if !state.visible {
        return;
    }

    let desired_height = (state.servers.len() as u16 + 5).min(area.height.saturating_sub(2));
    let desired_width = area.width.saturating_sub(4).min(90);
    let [dialog_area] = Layout::horizontal([Constraint::Length(desired_width)])
        .flex(Flex::Center)
        .areas(area);
    let [dialog_area] = Layout::vertical([Constraint::Length(desired_height.max(7))])
        .flex(Flex::Center)
        .areas(dialog_area);
    state.area = Some(dialog_area);

    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(colors.dialog_background));
    let content_area = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(content_area);
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(4)])
        .split(chunks[0]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Status",
            Style::default()
                .fg(colors.text)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Left),
        header_chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "esc",
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Right),
        header_chunks[1],
    );

    let mut lines = vec![Line::from(Span::styled(
        format!("{} MCP Servers", state.servers.len()),
        Style::default().fg(colors.text),
    ))];

    if state.servers.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No MCP servers configured",
            Style::default().fg(colors.text_weak),
        )));
    } else {
        for server in &state.servers {
            let (bullet_color, status) = match server.status.as_str() {
                "connected" => (colors.success, "Connected".to_string()),
                "failed" => (
                    colors.error,
                    server
                        .detail
                        .clone()
                        .unwrap_or_else(|| "Connection failed".to_string()),
                ),
                "needs_auth" => (
                    colors.warning,
                    server
                        .detail
                        .clone()
                        .unwrap_or_else(|| "Authentication required".to_string()),
                ),
                "connecting" => (colors.warning, "Connecting".to_string()),
                "disabled" => (colors.text_weak, "Disabled".to_string()),
                status => (colors.text_weak, status.to_string()),
            };
            lines.push(Line::from(vec![
                Span::styled("• ", Style::default().fg(bullet_color)),
                Span::styled(
                    server.name.clone(),
                    Style::default()
                        .fg(colors.text_strong)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(status, Style::default().fg(colors.text_weak)),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn rendered_dialog_tracks_bounds_for_outside_click_dismissal() {
        let mut state = StatusDialogState::default();
        state.show(Vec::new());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let colors = Theme::load_builtin_default().get_colors(true);

        terminal
            .draw(|frame| {
                let area = frame.area();
                render_status_dialog(frame, &mut state, area, &colors);
            })
            .expect("render status dialog");

        assert!(state.contains(ratatui::layout::Position::new(40, 12)));
        assert!(!state.contains(ratatui::layout::Position::new(0, 0)));
        state.hide();
        assert!(!state.contains(ratatui::layout::Position::new(40, 12)));
    }
}
