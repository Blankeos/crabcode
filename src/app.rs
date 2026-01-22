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

use crate::ui::components::popup::Popup;
use crate::ui::components::status_bar::StatusBar;
use crate::utils::git;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppFocus {
    Landing,
    Chat,
}

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
    pub focus: AppFocus,
    ctrl_c_press_count: u8,
    last_ctrl_c_time: std::time::Instant,
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
            focus: AppFocus::Landing,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
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
        use ratatui::widgets::Paragraph;

        let size = f.area();

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
            .split(size);

        if self.focus == AppFocus::Landing {
            let landing_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Min(0),
                        Constraint::Length(3),
                        Constraint::Length(1),
                    ]
                    .as_ref(),
                )
                .split(main_chunks[0]);

            let landing_logo_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(0),
                    Constraint::Length(5),
                    Constraint::Min(0),
                ])
                .split(landing_chunks[0]);
            let logo = Paragraph::new(Text::from(crate::ui::components::landing::LOGO.trim()))
                .style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(Alignment::Center);
            // let welcome = Paragraph::new(Text::from(vec![Line::from(vec![
            //     Span::styled(
            //         "Crabcode",
            //         Style::default()
            //             .fg(Color::Green)
            //             .add_modifier(Modifier::BOLD),
            //     ),
            //     Span::raw(" - "),
            //     Span::styled(
            //         "Rust AI CLI Coding Agent",
            //         Style::default().fg(Color::White),
            //     ),
            // ])]))
            // .alignment(Alignment::Center)
            // .wrap(ratatui::widgets::Wrap { trim: true });

            f.render_widget(logo, landing_logo_chunks[1]);

            // f.render_widget(welcome, landing_chunks[0]);

            self.input.render(f, landing_chunks[1]);

            if self.popup.is_visible() {
                self.popup.render(f, landing_chunks[1]);
            }

            let help_text = vec![
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::raw(" commands  "),
                Span::styled("tab", Style::default().fg(Color::Cyan)),
                Span::raw(" agents  "),
                Span::styled("ctrl+cc", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ];
            let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
            f.render_widget(help, landing_chunks[2]);
        } else {
            let chat_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Min(0),
                        Constraint::Length(3),
                        Constraint::Length(1),
                    ]
                    .as_ref(),
                )
                .split(main_chunks[0]);

            self.chat.render(f, chat_chunks[0]);
            self.input.render(f, chat_chunks[1]);

            if self.popup.is_visible() {
                self.popup.render(f, chat_chunks[1]);
            }

            let help_text = vec![
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::raw(" commands  "),
                Span::styled("tab", Style::default().fg(Color::Cyan)),
                Span::raw(" agents  "),
                Span::styled("ctrl+cc", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ];
            let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
            f.render_widget(help, chat_chunks[2]);
        }

        let branch = git::get_current_branch();
        let status_bar = StatusBar::new(
            self.version.clone(),
            self.cwd.clone(),
            branch,
            self.agent.clone(),
            self.model.clone(),
        );
        status_bar.render(f, main_chunks[1]);
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if key.modifiers == event::KeyModifiers::CONTROL => {
                let now = std::time::Instant::now();
                if now.duration_since(self.last_ctrl_c_time).as_secs() < 1 {
                    self.ctrl_c_press_count += 1;
                    if self.ctrl_c_press_count >= 2 {
                        self.quit();
                    }
                } else {
                    self.ctrl_c_press_count = 1;
                }
                self.last_ctrl_c_time = now;
                if self.ctrl_c_press_count == 1 {
                    self.input.clear();
                }
            }
            KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
                if self.popup.is_visible() {
                    self.autocomplete_and_submit();
                } else {
                    let input_text = self.input.get_text();
                    if !input_text.is_empty() {
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_input(&input_text));
                        });
                        self.input.clear();
                        self.popup.clear();
                    }
                }
            }
            KeyCode::Tab => {
                if self.popup.is_visible() {
                    if let Some(selected) = self.popup.get_selected() {
                        self.input.set_text(&format!("/{}", selected.name));
                    }
                    self.popup.clear();
                } else {
                    self.input.handle_event(key);
                    if self.input.is_slash_at_end() {
                        self.update_suggestions();
                    }
                }
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
        if self.input.should_show_suggestions() {
            let suggestions = self.input.get_autocomplete_suggestions();
            if !suggestions.is_empty() {
                self.popup.set_suggestions(suggestions);
            } else {
                self.popup.clear();
            }
        } else {
            self.popup.clear();
        }
    }

    fn autocomplete_and_submit(&mut self) {
        if let Some(selected) = self.popup.get_selected() {
            self.input.set_text(&format!("/{}", selected.name));

            let input_text = self.input.get_text();
            if !input_text.is_empty() {
                tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(self.process_input(&input_text));
                });
                self.input.clear();
            }
        }
        self.popup.clear();
    }

    async fn process_input(&mut self, input: &str) {
        use crate::command::parser::parse_input;

        match parse_input(input) {
            InputType::Command(parsed) => {
                let result = self
                    .command_registry
                    .execute(&parsed, &mut self.session_manager)
                    .await;
                match result {
                    crate::command::registry::CommandResult::Success(msg) => {
                        if parsed.name == "new" {
                            self.chat.clear();
                            self.focus = AppFocus::Landing;
                        } else if self.focus == AppFocus::Landing {
                            self.focus = AppFocus::Chat;
                        }
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
                    if self.focus == AppFocus::Landing {
                        self.focus = AppFocus::Chat;
                    }
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
        assert!(app.running);
    }

    #[test]
    fn test_handle_key_event_ctrl_c_single() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key_event(key);
        assert!(app.running);
        assert_eq!(app.input.get_text(), "");
    }

    #[test]
    fn test_handle_key_event_ctrl_c_double() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        app.handle_key_event(key);
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

    #[tokio::test]
    async fn test_process_input_message() {
        let mut app = App::new();
        app.process_input("hello world").await;
        assert_eq!(app.chat.messages.len(), 1);
        assert_eq!(app.chat.messages[0].content, "hello world");
    }

    #[tokio::test]
    async fn test_process_input_command() {
        let mut app = App::new();
        app.process_input("/sessions").await;
        assert_eq!(app.chat.messages.len(), 1);
        assert_eq!(
            app.chat.messages[0].role,
            crate::session::types::MessageRole::Assistant
        );
    }

    #[tokio::test]
    async fn test_process_input_empty() {
        let mut app = App::new();
        app.process_input("").await;
        assert_eq!(app.chat.messages.len(), 0);
    }
}
