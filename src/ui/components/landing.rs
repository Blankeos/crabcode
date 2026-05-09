use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
    Frame,
};

fn darken_color(color: Color, factor: f32) -> Color {
    match color {
        Color::Rgb(r, g, b) => {
            let r = (r as f32 * factor).max(0.0).min(255.0) as u8;
            let g = (g as f32 * factor).max(0.0).min(255.0) as u8;
            let b = (b as f32 * factor).max(0.0).min(255.0) as u8;
            Color::Rgb(r, g, b)
        }
        _ => color,
    }
}

pub const LOGO: &str = r#"
 ▄▄▄▄ ▄▄▄▄   ▄▄▄  ▄▄▄▄   ▄▄▄▄  ▄▄▄  ▄▄▄▄  ▄▄▄▄▄
██▀▀▀ ██▄█▄ ██▀██ ██▄██ ██▀▀▀ ██▀██ ██▀██ ██▄▄
▀████ ██ ██ ██▀██ ██▄█▀ ▀████ ▀███▀ ████▀ ██▄▄▄
"#;

pub const MASCO: &str = r#"
    ▃▃▛████▜▃▃
 █▟▟▜████████▛▙▙█
    ▞ ▘    ▝ ▚
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
            .constraints([Constraint::Length(4), Constraint::Length(3)].as_ref())
            .split(chunks[0]);

        let logo_lines: Vec<Line> = LOGO
            .trim()
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let color = if i == 2 {
                    darken_color(Color::Rgb(255, 140, 0), 0.7)
                } else {
                    Color::Rgb(255, 140, 0)
                };
                Line::styled(
                    line,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )
            })
            .collect();

        let logo = Paragraph::new(Text::from(logo_lines)).alignment(Alignment::Center);

        let welcome_row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(25), Constraint::Min(1)].as_ref())
            .split(top_chunks[1]);

        let mascot_lines: Vec<Line> = MASCO
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                Line::styled(
                    line,
                    Style::default()
                        .fg(Color::Rgb(255, 140, 0))
                        .add_modifier(Modifier::BOLD),
                )
            })
            .collect();

        let mascot = Paragraph::new(Text::from(mascot_lines));

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

        let welcome_text_col = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(welcome_row[1]);

        let welcome = Paragraph::new(welcome_text)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });

        f.render_widget(logo, top_chunks[0]);
        f.render_widget(mascot, welcome_row[0]);
        f.render_widget(welcome, welcome_text_col[1]);
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
