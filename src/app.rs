use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use std::io;
use std::time::Duration;

pub struct App {
    pub running: bool,
    pub version: String,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.run_event_loop(&mut terminal).await;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        result
    }

    async fn run_event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        while self.running {
            terminal.draw(|f| self.ui(f))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key);
                }
            }
        }
        Ok(())
    }

    fn ui(&self, f: &mut ratatui::Frame) {
        use ratatui::layout::{Alignment, Constraint, Direction, Layout};
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span, Text};
        use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

        let size = f.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
            .split(size);

        let status_text = vec![
            Span::raw("crabcode "),
            Span::styled(
                &self.version,
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];

        let status = Paragraph::new(Line::from(status_text))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });

        let main_text = Text::from(vec![
            Line::from("Crabcode - Rust AI CLI Coding Agent"),
            Line::from(""),
            Line::from("Press 'q' to quit"),
        ]);

        let main = Paragraph::new(main_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Crabcode")
                    .title_alignment(Alignment::Center),
            )
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });

        f.render_widget(main, chunks[0]);
        f.render_widget(status, chunks[1]);
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.quit();
            }
            KeyCode::Char('c') if key.modifiers == event::KeyModifiers::CONTROL => {
                self.quit();
            }
            _ => {}
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_app_creation() {
        let app = App::new();
        assert_eq!(app.version, "0.1.0");
        assert!(app.running);
    }

    #[test]
    fn test_app_quit() {
        let mut app = App::new();
        app.quit();
        assert!(!app.running);
    }

    #[test]
    fn test_app_default() {
        let app = App::default();
        assert_eq!(app.version, "0.1.0");
        assert!(app.running);
    }

    #[test]
    fn test_handle_key_event_q() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::empty(),
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key_event(key);
        assert!(!app.running);
    }

    #[test]
    fn test_handle_key_event_ctrl_c() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key_event(key);
        assert!(!app.running);
    }

    #[test]
    fn test_handle_key_event_other() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::empty(),
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key_event(key);
        assert!(app.running);
    }

    #[test]
    fn test_handle_key_event_escape() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::empty(),
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key_event(key);
        assert!(app.running);
    }
}
