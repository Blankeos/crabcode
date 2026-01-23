use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};

pub const LOGO: &str = r#"
🦀▄▄▄▄ ▄▄▄▄   ▄▄▄  ▄▄▄▄   ▄▄▄▄  ▄▄▄  ▄▄▄▄  ▄▄▄▄▄
██▀▀▀ ██▄█▄ ██▀██ ██▄██ ██▀▀▀ ██▀██ ██▀██ ██▄▄
▀████ ██ ██ ██▀██ ██▄█▀ ▀████ ▀███▀ ████▀ ██▄▄▄
"#;

pub struct Landing;

impl Landing {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self, f: &mut Frame) {
        let size = f.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
            .split(size);

        let top_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Length(2)].as_ref())
            .split(chunks[0]);

        let logo_text = Text::from(LOGO.trim());

        let logo = Paragraph::new(logo_text)
            .style(
                Style::default()
                    .fg(Color::Rgb(255, 140, 0))
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center);

        let welcome_text = Text::from(vec![Line::from(vec![
            Span::styled(
                "Crabcode",
                Style::default()
                    .fg(Color::Rgb(255, 165, 0))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" - "),
            Span::styled(
                "Rust AI CLI Coding Agent",
                Style::default().fg(Color::White),
            ),
        ])]);

        let welcome = Paragraph::new(welcome_text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });

        f.render_widget(logo, top_chunks[0]);
        f.render_widget(welcome, top_chunks[1]);
    }
}

impl Default for Landing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn test_landing_creation() {
        let _landing = Landing::new();
        let _landing_default = Landing::default();
    }

    #[test]
    fn test_logo_content() {
        assert!(LOGO.contains("▄▄▄▄"));
        assert!(LOGO.contains("██"));
        assert!(LOGO.contains("▀████"));
    }

    #[test]
    fn test_logo_is_not_empty() {
        let trimmed = LOGO.trim();
        assert!(!trimmed.is_empty());
        assert!(trimmed.len() > 0);
    }

    #[test]
    fn test_render_landing() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                Landing::new().render(f);
            })
            .unwrap();
    }
}
