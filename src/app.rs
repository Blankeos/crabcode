use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;

use crate::autocomplete::AutoComplete;
use crate::command::handlers::register_all_commands;
use crate::command::parser::InputType;
use crate::command::registry::Registry;
use crate::session::manager::SessionManager;
use crate::ui::components::chat::Chat;
use crate::ui::components::input::Input;
use crate::ui::components::landing::Landing;
use crate::ui::components::popup::Popup;
use crate::ui::components::status_bar::StatusBar;
use crate::utils::git;

pub struct App {
    pub running: bool,
    pub version: String,
    pub input: Input,
    pub command_registry: Registry,
    pub session_manager: SessionManager,
    pub chat: Chat,
    pub popup: Popup,
    pub agent: String,
    pub model: String,
    pub cwd: String,
}

impl App {
    pub fn new() -> Self {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let autocomplete = AutoComplete::new(crate::autocomplete::CommandAuto::new(&registry));
        let input = Input::new().with_autocomplete(autocomplete);

        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "?".to_string());

        Self {
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            input,
            command_registry: registry,
            session_manager: SessionManager::new(),
            chat: Chat::new(),
            popup: Popup::new(),
            agent: "PLAN".to_string(),
            model: "nano-gpt".to_string(),
            cwd,
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
        use ratatui::layout::{Constraint, Direction, Layout};

        let size = f.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Min(0),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(size);

        if self.chat.messages.is_empty() {
            let landing = Landing::new();
            landing.render(f);
        } else {
            self.chat.render(f, chunks[0]);
        }

        self.input.render(f, chunks[1]);

        if self.popup.is_visible() {
            self.popup.render(f, chunks[1]);
        }

        let branch = git::get_current_branch();
        let status_bar = StatusBar::new(
            self.version.clone(),
            self.cwd.clone(),
            branch,
            self.agent.clone(),
            self.model.clone(),
        );
        status_bar.render(f, chunks[2]);
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
                    self.popup.clear();
                }
            }
            KeyCode::Tab => {
                self.input.handle_event(key);
                self.popup.clear();
            }
            KeyCode::Up => {
                if self.popup.is_visible() {
                    self.popup.previous();
                }
            }
            KeyCode::Down => {
                if self.popup.is_visible() {
                    self.popup.next();
                }
            }
            KeyCode::Esc => {
                self.popup.clear();
            }
            _ => {
                if self.input.handle_event(key) {
                    self.update_suggestions();
                }
            }
        }
    }

    fn update_suggestions(&mut self) {
        let suggestions = self.input.get_autocomplete_suggestions();
        if !suggestions.is_empty() {
            self.popup.set_suggestions(suggestions);
        } else {
            self.popup.clear();
        }
    }

    fn process_input(&mut self, input: &str) {
        use crate::command::parser::parse_input;

        match parse_input(input) {
            InputType::Command(parsed) => {
                let result = self
                    .command_registry
                    .execute(&parsed, &mut self.session_manager);
                match result {
                    crate::command::registry::CommandResult::Success(msg) => {
                        self.chat.add_assistant_message(msg);
                        if parsed.name == "exit" {
                            self.quit();
                        }
                    }
                    crate::command::registry::CommandResult::Error(msg) => {
                        self.chat.add_assistant_message(format!("Error: {}", msg));
                    }
                }
            }
            InputType::Message(msg) => {
                if !msg.is_empty() {
                    self.chat.add_user_message(&msg);
                }
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
        assert!(app.chat.messages.is_empty());
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
        assert!(app.chat.messages.is_empty());
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

    #[test]
    fn test_process_input_message() {
        let mut app = App::new();
        app.process_input("hello world");
        assert_eq!(app.chat.messages.len(), 1);
        assert_eq!(app.chat.messages[0].content, "hello world");
    }

    #[test]
    fn test_process_input_command() {
        let mut app = App::new();
        app.process_input("/sessions");
        assert_eq!(app.chat.messages.len(), 1);
        assert_eq!(
            app.chat.messages[0].role,
            crate::session::types::MessageRole::Assistant
        );
    }

    #[test]
    fn test_process_input_empty() {
        let mut app = App::new();
        app.process_input("");
        assert_eq!(app.chat.messages.len(), 0);
    }
}
