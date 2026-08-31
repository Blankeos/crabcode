use crate::mcp::McpServerView;
use crate::theme::ThemeColors;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct StatusDialogState {
    visible: bool,
    servers: Vec<McpServerView>,
}

impl StatusDialogState {
    pub fn show(&mut self, servers: Vec<McpServerView>) {
        self.servers = servers;
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

pub fn render_status_dialog(
    frame: &mut Frame<'_>,
    state: &StatusDialogState,
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

    frame.render_widget(Clear, dialog_area);

    let title = Line::from(vec![
        Span::styled(
            "Status",
            Style::default()
                .fg(colors.text_strong)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("esc", Style::default().fg(colors.text_weak)),
    ]);
    let block = Block::default()
        .borders(Borders::NONE)
        .title(title)
        .title_alignment(Alignment::Left)
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(colors.dialog_background));

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

    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), dialog_area);
}
