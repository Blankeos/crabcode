use anyhow::Result;
use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
        KeyboardEnhancementFlags, MouseEvent, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
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
use crate::{
    get_toast_manager, remove_expired_toasts, render_toasts,
};

use crate::ui::components::dialog::Dialog;
use crate::ui::components::popup::Popup;
use crate::ui::components::status_bar::StatusBar;
use crate::utils::git;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppFocus {
    Landing,
    Chat,
    Dialog,
}

pub struct App {
    pub running: bool,
    pub version: String,
    pub input: Input,
    pub command_registry: Registry,
    pub session_manager: SessionManager,
    pub chat: Chat,
    pub popup: Popup,
    pub dialog: Dialog,
    pub agent: String,
    pub model: String,
    pub cwd: String,
    pub focus: AppFocus,
    pub popup_has_focus: bool,
    ctrl_c_press_count: u8,
    last_ctrl_c_time: std::time::Instant,
}

impl App {
    fn get_random_placeholder() -> String {
        let suggestions = vec![
            "Fix a TODO in the codebase",
            "What is the tech stack of this project?",
            "Write unit tests for this module",
            "Refactor this function for better performance",
            "Add error handling to this code",
            "Explain how this code works",
            "Find and fix a bug in this module",
            "Add documentation to this function",
            "Create a new feature for X",
            "Optimize this database query",
            "Add type hints to this code",
            "Implement caching for this endpoint",
        ];

        use std::time::{SystemTime, UNIX_EPOCH};
        let index = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as usize
            % suggestions.len();

        format!("Ask anything... \"{}\"", suggestions[index])
    }

    pub fn new() -> Self {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let autocomplete = AutoComplete::new(crate::autocomplete::CommandAuto::new(&registry));
        let placeholder = Self::get_random_placeholder();
        let placeholder_static: &'static str = Box::leak(placeholder.into_boxed_str());
        let mut input = Input::new().with_autocomplete(autocomplete);
        input.set_placeholder(placeholder_static);

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
            dialog: Dialog::new("Dialog"),
            agent: "PLAN".to_string(),
            model: "nano-gpt".to_string(),
            cwd,
            focus: AppFocus::Landing,
            popup_has_focus: false,
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

        if supports_keyboard_enhancement()? {
            execute!(
                stdout,
                EnterAlternateScreen,
                EnableMouseCapture,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        } else {
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.run_event_loop(&mut terminal).await;

        disable_raw_mode()?;
        if supports_keyboard_enhancement().unwrap_or(false) {
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                PopKeyboardEnhancementFlags
            )?;
        } else {
            execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
        }
        terminal.show_cursor()?;

        result
    }

    async fn run_event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        while self.running {
            remove_expired_toasts();
            terminal.draw(|f| self.ui(f))?;

            if event::poll(Duration::from_millis(100))? {
                let event = event::read()?;

                match event {
                    Event::Key(key) => {
                        self.handle_key_event(key);
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse_event(mouse);
                    }
                    _ => {}
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

        if self.dialog.is_visible() {
            self.dialog.render(f, size);
        }

        render_toasts(f, &get_toast_manager().lock().unwrap());

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Min(0),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(size);

        if self.focus == AppFocus::Landing {
            let input_height = self.input.get_height();
            let landing_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Min(0),
                        Constraint::Length(input_height),
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
                        .fg(Color::Rgb(255, 140, 0))
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
                self.popup
                    .render(f, landing_chunks[1], self.popup_has_focus);
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
                self.popup.render(f, chat_chunks[1], self.popup_has_focus);
            }

            let help_text = vec![
                Span::styled("/", Style::default().fg(Color::Rgb(255, 140, 0))),
                Span::raw(" commands  "),
                Span::styled("tab", Style::default().fg(Color::Rgb(255, 140, 0))),
                Span::raw(" agents  "),
                Span::styled("ctrl+cc", Style::default().fg(Color::Rgb(255, 140, 0))),
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
        status_bar.render(f, main_chunks[2]);
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
                if self.focus == AppFocus::Dialog {
                    if let Some(_selected) = self.dialog.get_selected() {
                        self.dialog.hide();
                        if self.focus == AppFocus::Dialog {
                            self.focus = AppFocus::Landing;
                        }
                    }
                } else if self.popup.is_visible() {
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
                        self.popup_has_focus = false;
                    }
                }
            }
            KeyCode::Enter => {}
            KeyCode::Tab => {
                if self.agent == "PLAN" {
                    self.agent = "BUILD".to_string();
                } else {
                    self.agent = "PLAN".to_string();
                }
            }
            KeyCode::Esc => {
                if self.focus == AppFocus::Dialog {
                    if self.dialog.handle_key_event(key) {
                        self.focus = AppFocus::Landing;
                    }
                } else if self.popup.is_visible() {
                    self.input.clear();
                    self.popup.clear();
                    self.popup_has_focus = false;
                }
            }
            _ => {
                if self.focus == AppFocus::Dialog {
                    self.dialog.handle_key_event(key);
                } else if self.input.handle_event(key) {
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
                self.popup_has_focus = true;
            } else {
                self.popup.clear();
                self.popup_has_focus = false;
            }
        } else {
            self.popup.clear();
            self.popup_has_focus = false;
        }
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        if self.dialog.handle_mouse_event(mouse) {}
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
        self.popup_has_focus = false;
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
                    crate::command::registry::CommandResult::ShowDialog { title, items } => {
                        let dialog_items: Vec<crate::ui::components::dialog::DialogItem> = items
                            .into_iter()
                            .map(|item| crate::ui::components::dialog::DialogItem {
                                id: item.id,
                                name: item.name,
                                group: item.group,
                                description: item.description,
                            })
                            .collect();
                        self.dialog = Dialog::with_items(title, dialog_items);
                        self.dialog.show();
                        self.focus = AppFocus::Dialog;
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
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    #[test]
    fn test_app_creation() {
        let app = App::new();
        assert_eq!(app.version, "0.0.1");
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
        assert_eq!(app.version, "0.0.1");
        assert!(app.running);
        assert!(app.chat.messages.is_empty());
    }

    #[test]
    fn test_handle_key_event_q() {
        let mut app = App::new();
        let key = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
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
