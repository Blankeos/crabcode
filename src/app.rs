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

use crate::command::handlers::register_all_commands;
use crate::command::registry::Registry;
use crate::command::parser::InputType;
use crate::ui::components::input::Input;
use crate::ui::components::landing::Landing;

pub struct App {
    pub running: bool,
    pub version: String,
    pub input: Input,
    pub command_registry: Registry,
    pub last_message: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        
        Self {
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            input: Input::new(),
            command_registry: registry,
            last_message: None,
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
        use ratatui::text::{Line, Span};
        use ratatui::widgets::Paragraph;

        let size = f.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3), Constraint::Length(1)].as_ref())
            .split(size);

        let landing = Landing::new();
        landing.render(f);

        self.input.render(f, chunks[1]);

        let status_text = vec![
            Span::raw("crabcode "),
            Span::styled(
                &self.version,
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];

        let status = Paragraph::new(Line::from(status_text))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Left);

        f.render_widget(status, chunks[2]);
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => {
                self.quit();
            }
            KeyCode::Char('c') if key.modifiers == event::KeyModifiers::CONTROL => {
                self.quit();
            }
            KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
                let input_text = self.input.get_text();
                if !input_text.is_empty() {
                    self.process_input(&input_text);
                    self.input.clear();
                }
            }
            _ => {
                self.input.handle_event(key);
            }
        }
    }

    fn process_input(&mut self, input: &str) {
        use crate::command::parser::parse_input;
        
        match parse_input(input) {
            InputType::Command(parsed) => {
                let result = self.command_registry.execute(&parsed);
                match result {
                    crate::command::registry::CommandResult::Success(msg) => {
                        self.last_message = Some(msg);
                        if parsed.name == "exit" {
                            self.quit();
                        }
                    }
                    crate::command::registry::CommandResult::Error(msg) => {
                        self.last_message = Some(format!("Error: {}", msg));
                    }
                }
            }
            InputType::Message(msg) => {
                self.last_message = Some(format!("Message: {}", msg));
            }
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
