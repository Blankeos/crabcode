use ratatui::crossterm::event::{self, KeyCode, KeyEvent, MouseEvent};

use crate::autocomplete::AutoComplete;
use crate::command::handlers::register_all_commands;
use crate::command::parser::InputType;
use crate::command::registry::Registry;
use crate::session::manager::SessionManager;
use crate::ui::components::chat::Chat;
use crate::ui::components::input::Input;
use crate::ui::components::popup::Popup;
use crate::utils::git;

use crate::views::{
    ChatState, HomeState, ModelsDialogState, SuggestionsPopupState,
};
use crate::views::home::{init_home, render_home};
use crate::views::chat::{init_chat, render_chat};
use crate::views::models_dialog::{init_models_dialog, render_models_dialog, handle_models_dialog_key_event, handle_models_dialog_mouse_event};
use crate::views::suggestions_popup::{init_suggestions_popup, render_suggestions_popup, handle_suggestions_popup_key_event, set_suggestions, clear_suggestions, get_selected_suggestion, is_suggestions_visible};

use crate::{
    get_toast_manager, render_toasts,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BaseFocus {
    Home,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayFocus {
    None,
    ModelsDialog,
    SuggestionsPopup,
}

pub struct App {
    pub running: bool,
    pub version: String,
    pub input: Input,
    pub command_registry: Registry,
    pub session_manager: SessionManager,
    pub home_state: HomeState,
    pub chat_state: ChatState,
    pub suggestions_popup_state: SuggestionsPopupState,
    pub models_dialog_state: ModelsDialogState,
    pub agent: String,
    pub model: String,
    pub cwd: String,
    pub base_focus: BaseFocus,
    pub overlay_focus: OverlayFocus,
    ctrl_c_press_count: u8,
    last_ctrl_c_time: std::time::Instant,
}

impl App {
    pub fn new() -> Self {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let autocomplete = AutoComplete::new(crate::autocomplete::CommandAuto::new(&registry));
        let placeholder = Self::get_random_placeholder();
        let placeholder_static: &'static str = Box::leak(placeholder.into_boxed_str());
        let mut input = Input::new().with_autocomplete(autocomplete);
        input.set_placeholder(placeholder_static);

        let home_state = init_home();
        let chat_state = init_chat(Chat::new());
        let suggestions_popup_state = init_suggestions_popup(Popup::new());
        let models_dialog_state = init_models_dialog("Models", vec![]);

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
            home_state,
            chat_state,
            suggestions_popup_state,
            models_dialog_state,
            agent: "PLAN".to_string(),
            model: "nano-gpt".to_string(),
            cwd,
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
        }
    }

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

    pub fn quit(&mut self) {
        self.running = false;
    }

    pub fn handle_keys(&mut self, key: KeyEvent) {
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
                return;
            }
            _ => {}
        }

        let handled = match self.overlay_focus {
            OverlayFocus::SuggestionsPopup => {
                let handled = self.handle_suggestions_popup_keys(key);
                if !handled {
                    self.input.handle_event(key);
                    self.update_suggestions();
                }
                handled
            }
            OverlayFocus::ModelsDialog => {
                if handle_models_dialog_key_event(&mut self.models_dialog_state, key) {
                    return;
                }
                if !self.models_dialog_state.dialog.is_visible() {
                    self.overlay_focus = OverlayFocus::None;
                }
                false
            }
            OverlayFocus::None => {
                if self.handle_base_keys(key) {
                    return;
                }
                false
            }
        };

        if handled {
            return;
        }

        if self.overlay_focus == OverlayFocus::None {
            self.handle_input_and_app_keys(key);
        }
    }

    fn handle_suggestions_popup_keys(&mut self, key: KeyEvent) -> bool {
        let action = handle_suggestions_popup_key_event(&mut self.suggestions_popup_state, key);
        match action {
            crate::ui::components::popup::PopupAction::Handled => true,
            crate::ui::components::popup::PopupAction::Autocomplete => {
                self.autocomplete_and_submit();
                true
            }
            crate::ui::components::popup::PopupAction::NotHandled => false,
        }
    }

    fn handle_base_keys(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Tab => {
                if self.agent == "PLAN" {
                    self.agent = "BUILD".to_string();
                } else {
                    self.agent = "PLAN".to_string();
                }
                true
            }
            KeyCode::Esc => {
                if self.overlay_focus == OverlayFocus::SuggestionsPopup {
                    self.input.clear();
                    clear_suggestions(&mut self.suggestions_popup_state);
                    self.overlay_focus = OverlayFocus::None;
                    true
                } else {
                    false
                }
            }
            KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
                if self.overlay_focus == OverlayFocus::SuggestionsPopup {
                    self.autocomplete_and_submit();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn handle_input_and_app_keys(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
                let input_text = self.input.get_text();
                if !input_text.is_empty() {
                    tokio::task::block_in_place(|| {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(self.process_input(&input_text));
                    });
                    self.input.clear();
                    clear_suggestions(&mut self.suggestions_popup_state);
                }
            }
            _ => {
                self.input.handle_event(key);
                self.update_suggestions();
            }
        }
    }

    fn update_suggestions(&mut self) {
        if self.input.should_show_suggestions() {
            let suggestions = self.input.get_autocomplete_suggestions();
            if !suggestions.is_empty() {
                set_suggestions(&mut self.suggestions_popup_state, suggestions);
                self.overlay_focus = OverlayFocus::SuggestionsPopup;
            } else {
                clear_suggestions(&mut self.suggestions_popup_state);
                self.overlay_focus = OverlayFocus::None;
            }
        } else {
            clear_suggestions(&mut self.suggestions_popup_state);
            self.overlay_focus = OverlayFocus::None;
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        handle_models_dialog_mouse_event(&mut self.models_dialog_state, mouse);
    }

    fn autocomplete_and_submit(&mut self) {
        if let Some(selected) = get_selected_suggestion(&self.suggestions_popup_state) {
            let command = format!("/{}", selected.name);

            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(self.process_input(&command));
            });

            self.input.clear();
        }
        clear_suggestions(&mut self.suggestions_popup_state);
        if !self.models_dialog_state.dialog.is_visible() {
            self.overlay_focus = OverlayFocus::None;
        }
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
                            self.chat_state.chat.clear();
                            self.base_focus = BaseFocus::Home;
                        } else if self.base_focus == BaseFocus::Home {
                            self.base_focus = BaseFocus::Chat;
                        }
                        self.chat_state.chat.add_assistant_message(msg);
                        if parsed.name == "exit" {
                            self.quit();
                        }
                    }
                    crate::command::registry::CommandResult::Error(msg) => {
                        self.chat_state.chat.add_assistant_message(format!("Error: {}", msg));
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
                        self.models_dialog_state = init_models_dialog(title, dialog_items);
                        self.models_dialog_state.dialog.show();
                        self.overlay_focus = OverlayFocus::ModelsDialog;
                    }
                }
            }
            InputType::Message(msg) => {
                if !msg.is_empty() {
                    self.chat_state.chat.add_user_message(&msg);
                    if self.base_focus == BaseFocus::Home {
                        self.base_focus = BaseFocus::Chat;
                    }
                }
            }
        }
    }

    pub fn render(&self, f: &mut ratatui::Frame) {
        let size = f.area();

        match self.base_focus {
            BaseFocus::Home => {
                render_home(
                    f,
                    &self.input,
                    self.version.clone(),
                    self.cwd.clone(),
                    git::get_current_branch(),
                    self.agent.clone(),
                    self.model.clone(),
                );

                if is_suggestions_visible(&self.suggestions_popup_state) && self.overlay_focus != OverlayFocus::ModelsDialog {
                    let main_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints([ratatui::layout::Constraint::Min(0)].as_ref())
                        .split(size);
                    let input_height = self.input.get_height();
                    let home_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints([ratatui::layout::Constraint::Min(0), ratatui::layout::Constraint::Length(input_height)].as_ref())
                        .split(main_chunks[0]);
                    render_suggestions_popup(f, &self.suggestions_popup_state, home_chunks[1], self.overlay_focus == OverlayFocus::SuggestionsPopup);
                }
            }
            BaseFocus::Chat => {
                render_chat(
                    f,
                    &self.chat_state,
                    &self.input,
                    self.version.clone(),
                    self.cwd.clone(),
                    git::get_current_branch(),
                    self.agent.clone(),
                    self.model.clone(),
                );

                if is_suggestions_visible(&self.suggestions_popup_state) && self.overlay_focus != OverlayFocus::ModelsDialog {
                    let main_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints([ratatui::layout::Constraint::Min(0)].as_ref())
                        .split(size);
                    let chat_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints([ratatui::layout::Constraint::Min(0), ratatui::layout::Constraint::Length(3)].as_ref())
                        .split(main_chunks[0]);
                    render_suggestions_popup(f, &self.suggestions_popup_state, chat_chunks[1], self.overlay_focus == OverlayFocus::SuggestionsPopup);
                }
            }
        }

        if self.overlay_focus == OverlayFocus::ModelsDialog && self.models_dialog_state.dialog.is_visible() {
            render_models_dialog(f, &self.models_dialog_state, size);
        }

        render_toasts(f, &get_toast_manager().lock().unwrap());
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
