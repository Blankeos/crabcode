use ratatui::crossterm::event::{self, KeyCode, KeyEvent, MouseEvent};

use crate::autocomplete::AutoComplete;
use crate::command::handlers::register_all_commands;
use crate::command::parser::InputType;
use crate::command::registry::Registry;
use crate::llm::client::stream_llm_with_cancellation;
use crate::session::manager::SessionManager;

use crate::push_toast;
use crate::ui::components::chat::Chat;
use crate::ui::components::input::Input;
use crate::ui::components::popup::Popup;
use crate::utils::git;

use crate::views::chat::{init_chat, render_chat};
use crate::views::connect_dialog::{
    get_pending_selection, handle_connect_dialog_key_event, handle_connect_dialog_mouse_event,
    init_connect_dialog, render_connect_dialog,
};
use crate::views::home::{init_home, render_home};
use crate::views::models_dialog::{
    handle_models_dialog_key_event, handle_models_dialog_mouse_event, init_models_dialog,
    render_models_dialog,
};
use crate::views::session_rename_dialog::{
    handle_session_rename_dialog_key_event, init_session_rename_dialog,
    render_session_rename_dialog, RenameAction,
};
use crate::views::sessions_dialog::{
    handle_sessions_dialog_key_event, handle_sessions_dialog_mouse_event, init_sessions_dialog,
    render_sessions_dialog, SessionsDialogAction,
};
use crate::views::suggestions_popup::{
    clear_suggestions, get_selected_suggestion, handle_suggestions_popup_key_event,
    init_suggestions_popup, is_suggestions_visible, render_suggestions_popup, set_suggestions,
};
use crate::views::{
    ChatState, ConnectDialogState, HomeState, ModelsDialogState, SessionRenameDialogState,
    SessionsDialogState, SuggestionsPopupState,
};

use crate::{
    get_toast_manager, render_toasts,
    theme::{self, Theme},
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
    ConnectDialog,
    ApiKeyInput,
    SuggestionsPopup,
    SessionsDialog,
    SessionRenameDialog,
    WhichKey,
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
    pub connect_dialog_state: ConnectDialogState,
    pub sessions_dialog_state: SessionsDialogState,
    pub session_rename_dialog_state: SessionRenameDialogState,
    pub which_key_state: crate::views::which_key::WhichKeyState,
    pub api_key_input: crate::ui::components::api_key_input::ApiKeyInput,
    pub prefs_dao: Option<crate::persistence::PrefsDAO>,
    pub agent: String,
    pub model: String,
    pub provider_name: String,
    pub cwd: String,
    pub base_focus: BaseFocus,
    pub overlay_focus: OverlayFocus,
    ctrl_c_press_count: u8,
    last_ctrl_c_time: std::time::Instant,
    pub themes: Vec<Theme>,
    pub current_theme_index: usize,
    pub dark_mode: bool,
    pub is_streaming: bool,
    chunk_sender: Option<crate::llm::ChunkSender>,
    chunk_receiver: Option<crate::llm::ChunkReceiver>,
    streaming_cancel_token: Option<tokio_util::sync::CancellationToken>,
    last_frame_size: ratatui::layout::Rect,
    streaming_model: Option<String>,
    streaming_provider: Option<String>,
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

        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "?".to_string());

        let theme = theme::Theme::load_from_file("src/theme.json")
            .unwrap_or_else(|_| theme::Theme::load_from_file("src/themes/ayu.json").unwrap());
        let colors = theme.get_colors(true);

        let home_state = init_home();
        let chat_state = init_chat(Chat::new());
        let suggestions_popup_state = init_suggestions_popup(Popup::new());
        let models_dialog_state = init_models_dialog("Models", vec![]);
        let connect_dialog_state = init_connect_dialog();
        let sessions_dialog_state = init_sessions_dialog("Sessions", vec![]);
        let session_rename_dialog_state = init_session_rename_dialog(colors);
        let which_key_state = crate::views::which_key::init_which_key();
        let api_key_input = crate::ui::components::api_key_input::ApiKeyInput::new();

        let session_manager = SessionManager::new()
            .with_history()
            .unwrap_or_else(|_| SessionManager::new());

        let prefs_dao = match crate::persistence::PrefsDAO::new() {
            Ok(dao) => Some(dao),
            Err(e) => {
                eprintln!("Warning: Failed to initialize preferences DAO: {}", e);
                None
            }
        };

        let active_model_info = if let Some(ref dao) = prefs_dao {
            dao.get_active_model().ok().flatten()
        } else {
            None
        };

        let (active_model, active_provider_name) =
            if let Some((provider_id, model_id)) = active_model_info {
                (model_id.clone(), provider_id.clone())
            } else {
                ("claude-3-sonnet".to_string(), "anthropic".to_string())
            };

        Self {
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            input,
            command_registry: registry,
            session_manager,
            home_state,
            chat_state,
            suggestions_popup_state,
            models_dialog_state,
            connect_dialog_state,
            sessions_dialog_state,
            session_rename_dialog_state,
            which_key_state,
            api_key_input,
            prefs_dao,
            agent: "Plan".to_string(),
            model: active_model,
            provider_name: active_provider_name,
            cwd,
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
            themes: vec![theme],
            current_theme_index: 0,
            dark_mode: true,
            is_streaming: false,
            chunk_sender: None,
            chunk_receiver: None,
            streaming_cancel_token: None,
            last_frame_size: ratatui::layout::Rect::default(),
            streaming_model: None,
            streaming_provider: None,
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

    pub fn get_current_theme_colors(&self) -> theme::ThemeColors {
        if self.themes.is_empty() {
            return theme::ThemeColors {
                primary: ratatui::style::Color::Rgb(255, 140, 0),
                background: ratatui::style::Color::Reset,
                text: ratatui::style::Color::Reset,
                text_weak: ratatui::style::Color::Reset,
                text_strong: ratatui::style::Color::Reset,
                border: ratatui::style::Color::Reset,
                border_weak_focus: ratatui::style::Color::Rgb(255, 200, 100),
                border_focus: ratatui::style::Color::Rgb(255, 140, 0),
                border_strong_focus: ratatui::style::Color::Rgb(255, 100, 0),
                success: ratatui::style::Color::Rgb(0, 255, 0),
                warning: ratatui::style::Color::Rgb(255, 255, 0),
                error: ratatui::style::Color::Rgb(255, 0, 0),
                info: ratatui::style::Color::Rgb(0, 255, 255),
            };
        }

        let theme = &self.themes[self.current_theme_index];
        theme.get_colors(self.dark_mode)
    }

    pub fn cycle_theme(&mut self) {
        if !self.themes.is_empty() {
            self.current_theme_index = (self.current_theme_index + 1) % self.themes.len();
        }
    }

    pub fn toggle_dark_mode(&mut self) {
        self.dark_mode = !self.dark_mode;
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
                let action = handle_models_dialog_key_event(&mut self.models_dialog_state, key);

                match action {
                    crate::views::models_dialog::ModelsDialogAction::SelectModel {
                        provider_id,
                        model_id,
                    } => {
                        let model_id_clone = model_id.clone();
                        let provider_id_clone = provider_id.clone();
                        self.model = model_id_clone.clone();
                        self.provider_name = provider_id_clone.clone();

                        if let Some(ref dao) = self.prefs_dao {
                            if let Err(e) =
                                dao.set_active_model(provider_id.clone(), model_id_clone.clone())
                            {
                                eprintln!("Failed to save active model: {}", e);
                            }
                        }

                        push_toast(ratatui_toolkit::Toast::new(
                            format!("Switched to: {}", model_id_clone),
                            ratatui_toolkit::ToastLevel::Info,
                            None,
                        ));
                    }
                    crate::views::models_dialog::ModelsDialogAction::ToggleFavorite {
                        provider_id,
                        model_id,
                    } => {
                        let is_favorite = if let Some(ref dao) = self.prefs_dao {
                            dao.toggle_favorite(provider_id.clone(), model_id.clone())
                                .unwrap_or(false)
                        } else {
                            false
                        };

                        push_toast(ratatui_toolkit::Toast::new(
                            if is_favorite {
                                "Added to favorites"
                            } else {
                                "Removed from favorites"
                            },
                            ratatui_toolkit::ToastLevel::Info,
                            None,
                        ));

                        self.refresh_models_dialog();
                    }
                    crate::views::models_dialog::ModelsDialogAction::None => {}
                }

                if !self.models_dialog_state.dialog.is_visible() {
                    self.overlay_focus = OverlayFocus::None;
                }
                true
            }
            OverlayFocus::ConnectDialog => {
                if handle_connect_dialog_key_event(&mut self.connect_dialog_state, key) {
                    return;
                }
                if !self.connect_dialog_state.dialog.is_visible() {
                    if let Some(selected_item) =
                        get_pending_selection(&mut self.connect_dialog_state)
                    {
                        self.api_key_input.show(&selected_item.id);
                        self.overlay_focus = OverlayFocus::ApiKeyInput;
                        return;
                    }
                    self.overlay_focus = OverlayFocus::None;
                }
                false
            }
            OverlayFocus::ApiKeyInput => {
                let action = self.api_key_input.handle_key_event(key);
                match action {
                    crate::ui::components::api_key_input::InputAction::Submitted {
                        api_key,
                        provider_name,
                    } => {
                        if let Some(auth_dao) = crate::persistence::AuthDAO::new().ok() {
                            let _ = auth_dao.set_provider(
                                provider_name,
                                crate::persistence::AuthConfig::Api { key: api_key },
                            );
                            self.connect_dialog_state = init_connect_dialog();
                        }
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    crate::ui::components::api_key_input::InputAction::Cancelled => {
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    crate::ui::components::api_key_input::InputAction::Continue => false,
                }
            }
            OverlayFocus::SessionsDialog => {
                let action = handle_sessions_dialog_key_event(&mut self.sessions_dialog_state, key);
                match action {
                    SessionsDialogAction::Handled => true,
                    SessionsDialogAction::NotHandled => false,
                    SessionsDialogAction::Close => {
                        if !self.sessions_dialog_state.dialog.is_visible() {
                            self.overlay_focus = OverlayFocus::None;
                        }
                        false
                    }
                    SessionsDialogAction::Select(id) => {
                        self.session_manager.switch_session(&id);
                        if let Some(session) = self.session_manager.get_session(&id) {
                            self.chat_state.chat.clear();
                            for message in &session.messages {
                                self.chat_state.chat.add_message(message.clone());
                            }
                        }
                        self.base_focus = BaseFocus::Chat;
                        self.sessions_dialog_state.dialog.hide();
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    SessionsDialogAction::Delete(id) => {
                        self.session_manager.delete_session(&id);
                        if let Some(pending) = crate::views::sessions_dialog::get_pending_delete(
                            &mut self.sessions_dialog_state,
                        ) {
                            self.session_manager.delete_session(&pending);
                        }
                        self.refresh_sessions_dialog();
                        true
                    }
                    SessionsDialogAction::Rename(id, title) => {
                        self.session_rename_dialog_state
                            .set_colors(self.get_current_theme_colors());
                        self.session_rename_dialog_state.show(id, title);
                        self.overlay_focus = OverlayFocus::SessionRenameDialog;
                        true
                    }
                }
            }
            OverlayFocus::SessionRenameDialog => {
                let action = handle_session_rename_dialog_key_event(
                    &mut self.session_rename_dialog_state,
                    key,
                );
                match action {
                    RenameAction::Handled => true,
                    RenameAction::NotHandled => false,
                    RenameAction::Cancel => {
                        if !self.session_rename_dialog_state.is_visible() {
                            self.overlay_focus = OverlayFocus::SessionsDialog;
                        }
                        false
                    }
                    RenameAction::Submit(id, new_title) => {
                        let _ = self.session_manager.rename_session(&id, new_title);
                        self.refresh_sessions_dialog();
                        self.sessions_dialog_state.dialog.show();
                        self.overlay_focus = OverlayFocus::SessionsDialog;
                        true
                    }
                }
            }
            OverlayFocus::WhichKey => {
                let action = self.which_key_state.handle_key_event(key);
                match action {
                    crate::views::which_key::WhichKeyAction::ShowModels => {
                        self.overlay_focus = OverlayFocus::None;
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_input("/models"));
                        });
                    }
                    crate::views::which_key::WhichKeyAction::ShowSessions => {
                        self.overlay_focus = OverlayFocus::None;
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_input("/sessions"));
                        });
                    }
                    crate::views::which_key::WhichKeyAction::NewSession => {
                        self.overlay_focus = OverlayFocus::None;
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_input("/new"));
                        });
                    }
                    crate::views::which_key::WhichKeyAction::Quit => {
                        self.overlay_focus = OverlayFocus::None;
                        self.quit();
                    }
                    crate::views::which_key::WhichKeyAction::None => {
                        self.overlay_focus = OverlayFocus::None;
                    }
                }
                true
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
            KeyCode::Char('x') if key.modifiers == event::KeyModifiers::CONTROL => {
                self.overlay_focus = OverlayFocus::WhichKey;
                self.which_key_state.show();
                true
            }
            KeyCode::Tab => {
                if self.agent == "Plan" {
                    self.agent = "Build".to_string();
                } else {
                    self.agent = "Plan".to_string();
                }
                true
            }
            KeyCode::Esc => {
                if self.is_streaming {
                    self.cancel_streaming();
                    return true;
                }
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
                    use crate::command::parser::parse_input;

                    match parse_input(&input_text) {
                        crate::command::parser::InputType::Command(mut parsed) => {
                            tokio::task::block_in_place(|| {
                                let rt = tokio::runtime::Handle::current();
                                rt.block_on(self.process_command_input(parsed));
                            });
                        }
                        crate::command::parser::InputType::Message(msg) => {
                            self.handle_message_input(msg);
                        }
                    }

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
        if self.overlay_focus == OverlayFocus::ModelsDialog {
            handle_models_dialog_mouse_event(&mut self.models_dialog_state, mouse);
        } else if self.overlay_focus == OverlayFocus::ConnectDialog {
            handle_connect_dialog_mouse_event(&mut self.connect_dialog_state, mouse);
        } else if self.overlay_focus == OverlayFocus::SessionsDialog {
            handle_sessions_dialog_mouse_event(&mut self.sessions_dialog_state, mouse);
        } else if self.overlay_focus == OverlayFocus::None {
            // Handle mouse events for chat scrolling when in chat mode
            if self.base_focus == BaseFocus::Chat {
                let size = self.last_frame_size;
                // We need to calculate the chat area similar to render_chat
                let main_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints([ratatui::layout::Constraint::Min(0), ratatui::layout::Constraint::Length(1)].as_ref())
                    .split(size);
                let input_height = self.input.get_height() as u16;
                let above_status_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints(
                        [
                            ratatui::layout::Constraint::Min(0),
                            ratatui::layout::Constraint::Length(input_height),
                            ratatui::layout::Constraint::Length(1),
                            ratatui::layout::Constraint::Length(1),
                        ]
                        .as_ref(),
                    )
                    .split(main_chunks[0]);
                let chat_area = above_status_chunks[0];
                
                if self.chat_state.chat.handle_mouse_event(mouse, chat_area) {
                    return;
                }
            }
            
            // Handle mouse events for the main input when no overlay is focused
            if self.input.handle_mouse_event(mouse) {
                self.update_suggestions();
            }
        }
    }

    pub fn handle_paste(&mut self, text: String) {
        const MAX_PASTE_SIZE: usize = 20 * 1024 * 1024;

        if text.len() > MAX_PASTE_SIZE {
            push_toast(ratatui_toolkit::Toast::new(
                format!(
                    "Paste content too large ({}MB). Maximum is 20MB.",
                    text.len() / 1024 / 1024
                ),
                ratatui_toolkit::ToastLevel::Warning,
                None,
            ));
            return;
        }

        match (self.base_focus, self.overlay_focus) {
            (BaseFocus::Home, OverlayFocus::None) | (BaseFocus::Chat, OverlayFocus::None) => {
                self.input.insert_str(&text);
            }
            (_, OverlayFocus::ModelsDialog) => {
                self.models_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.models_dialog_state.dialog.set_search_query(
                    self.models_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
                self.models_dialog_state.dialog.selected_index = 0;
            }
            (_, OverlayFocus::ConnectDialog) => {
                self.connect_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.connect_dialog_state.dialog.set_search_query(
                    self.connect_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
                self.connect_dialog_state.dialog.selected_index = 0;
            }
            (_, OverlayFocus::SessionsDialog) => {
                self.sessions_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.sessions_dialog_state.dialog.set_search_query(
                    self.sessions_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
                self.sessions_dialog_state.dialog.selected_index = 0;
            }
            (_, OverlayFocus::SessionRenameDialog) => {
                self.session_rename_dialog_state
                    .input_textarea
                    .insert_str(&text);
            }
            (_, OverlayFocus::ApiKeyInput) => {
                self.api_key_input.text_area.insert_str(&text);
            }
            (_, OverlayFocus::SuggestionsPopup) => {
                self.input.insert_str(&text);
                self.update_suggestions();
            }
            _ => {}
        }
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
    }

    async fn process_input(&mut self, input: &str) {
        use crate::command::parser::parse_input;

        match parse_input(input) {
            InputType::Command(mut parsed) => {
                parsed.prefs_dao = self.prefs_dao.as_ref();
                parsed.active_model_id = Some(self.model.clone());

                let result = self
                    .command_registry
                    .execute(&parsed, &mut self.session_manager)
                    .await;
                match result {
                    crate::command::registry::CommandResult::Success(msg) => {
                        if parsed.name == "new" || parsed.name == "home" {
                            self.chat_state.chat.clear();
                            self.base_focus = BaseFocus::Home;
                            self.session_manager.clear_current_session();
                        } else if self.base_focus == BaseFocus::Home {
                            self.base_focus = BaseFocus::Chat;
                        }
                        // Only add non-empty messages to the chat
                        if !msg.is_empty() {
                            let assistant_message =
                                crate::session::types::Message::assistant(msg.clone());
                            let _ = self
                                .session_manager
                                .add_message_to_current_session(&assistant_message);
                            self.chat_state.chat.add_assistant_message(msg);
                        }
                        if parsed.name == "exit" {
                            self.quit();
                        }
                    }
                    crate::command::registry::CommandResult::Error(msg) => {
                        let error_msg = format!("Error: {}", msg);
                        let error_message =
                            crate::session::types::Message::assistant(error_msg.clone());
                        let _ = self
                            .session_manager
                            .add_message_to_current_session(&error_message);
                        self.chat_state.chat.add_assistant_message(error_msg);
                    }
                    crate::command::registry::CommandResult::ShowDialog { title, items } => {
                        if title == "Connect a provider" {
                            let dialog_items: Vec<crate::ui::components::dialog::DialogItem> =
                                items
                                    .into_iter()
                                    .map(|item| crate::ui::components::dialog::DialogItem {
                                        id: item.id,
                                        name: item.name,
                                        group: item.group,
                                        description: item.description,
                                        tip: item.tip,
                                        provider_id: item.provider_id.clone(),
                                    })
                                    .collect();
                            self.connect_dialog_state = crate::views::ConnectDialogState::new(
                                crate::ui::components::dialog::Dialog::with_items(
                                    title,
                                    dialog_items,
                                ),
                            );
                            self.connect_dialog_state.dialog.show();
                            self.overlay_focus = OverlayFocus::ConnectDialog;
                        } else if title == "Sessions" {
                            let dialog_items: Vec<crate::ui::components::dialog::DialogItem> =
                                items
                                    .into_iter()
                                    .map(|item| crate::ui::components::dialog::DialogItem {
                                        id: item.id,
                                        name: item.name,
                                        group: item.group,
                                        description: item.description,
                                        tip: item.tip,
                                        provider_id: item.provider_id.clone(),
                                    })
                                    .collect();
                            self.sessions_dialog_state = init_sessions_dialog(title, dialog_items);
                            self.sessions_dialog_state.dialog.show();
                            self.overlay_focus = OverlayFocus::SessionsDialog;
                        } else {
                            let dialog_items: Vec<crate::ui::components::dialog::DialogItem> =
                                items
                                    .into_iter()
                                    .map(|item| crate::ui::components::dialog::DialogItem {
                                        id: item.id,
                                        name: item.name,
                                        group: item.group,
                                        description: item.description,
                                        tip: item.tip,
                                        provider_id: item.provider_id.clone(),
                                    })
                                    .collect();
                            self.models_dialog_state = init_models_dialog(title, dialog_items);
                            self.models_dialog_state.dialog.show();
                            self.overlay_focus = OverlayFocus::ModelsDialog;
                        }
                    }
                }
            }
            InputType::Message(msg) => {
                self.handle_message_input(msg);
            }
        }
    }

    async fn process_command_input(
        &mut self,
        mut parsed: crate::command::parser::ParsedCommand<'_>,
    ) {
        parsed.prefs_dao = self.prefs_dao.as_ref();
        parsed.active_model_id = Some(self.model.clone());

        let result = self
            .command_registry
            .execute(&parsed, &mut self.session_manager)
            .await;
        match result {
            crate::command::registry::CommandResult::Success(msg) => {
                if parsed.name == "new" || parsed.name == "home" {
                    self.chat_state.chat.clear();
                    self.base_focus = BaseFocus::Home;
                    self.session_manager.clear_current_session();
                } else if self.base_focus == BaseFocus::Home {
                    self.base_focus = BaseFocus::Chat;
                }
                let assistant_message = crate::session::types::Message::assistant(msg.clone());
                let _ = self
                    .session_manager
                    .add_message_to_current_session(&assistant_message);
                self.chat_state.chat.add_assistant_message(msg);
                if parsed.name == "exit" {
                    self.quit();
                }
            }
            crate::command::registry::CommandResult::Error(msg) => {
                let error_msg = format!("Error: {}", msg);
                let error_message = crate::session::types::Message::assistant(error_msg.clone());
                let _ = self
                    .session_manager
                    .add_message_to_current_session(&error_message);
                self.chat_state.chat.add_assistant_message(error_msg);
            }
            crate::command::registry::CommandResult::ShowDialog { title, items } => {
                if title == "Connect a provider" {
                    let dialog_items: Vec<crate::ui::components::dialog::DialogItem> = items
                        .into_iter()
                        .map(|item| crate::ui::components::dialog::DialogItem {
                            id: item.id,
                            name: item.name,
                            group: item.group,
                            description: item.description,
                            tip: item.tip,
                            provider_id: item.provider_id.clone(),
                        })
                        .collect();
                    self.connect_dialog_state = crate::views::ConnectDialogState::new(
                        crate::ui::components::dialog::Dialog::with_items(title, dialog_items),
                    );
                    self.connect_dialog_state.dialog.show();
                    self.overlay_focus = OverlayFocus::ConnectDialog;
                } else if title == "Sessions" {
                    let dialog_items: Vec<crate::ui::components::dialog::DialogItem> = items
                        .into_iter()
                        .map(|item| crate::ui::components::dialog::DialogItem {
                            id: item.id,
                            name: item.name,
                            group: item.group,
                            description: item.description,
                            tip: item.tip,
                            provider_id: item.provider_id.clone(),
                        })
                        .collect();
                    self.sessions_dialog_state = init_sessions_dialog(title, dialog_items);
                    self.sessions_dialog_state.dialog.show();
                    self.overlay_focus = OverlayFocus::SessionsDialog;
                } else {
                    let dialog_items: Vec<crate::ui::components::dialog::DialogItem> = items
                        .into_iter()
                        .map(|item| crate::ui::components::dialog::DialogItem {
                            id: item.id,
                            name: item.name,
                            group: item.group,
                            description: item.description,
                            tip: item.tip,
                            provider_id: item.provider_id.clone(),
                        })
                        .collect();
                    self.models_dialog_state = init_models_dialog(title, dialog_items);
                    self.models_dialog_state.dialog.show();
                    self.overlay_focus = OverlayFocus::ModelsDialog;
                }
            }
        }
    }

    fn generate_title_from_message(message: &str) -> String {
        message
            .chars()
            .take(30)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn refresh_sessions_dialog(&mut self) {
        use chrono::{DateTime, Local, Utc};

        let mut sessions = self.session_manager.list_sessions();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        let items: Vec<crate::ui::components::dialog::DialogItem> = sessions
            .into_iter()
            .map(|session| {
                let date_group = {
                    let datetime: DateTime<Local> = session.updated_at.into();
                    let now: DateTime<Local> = Utc::now().into();
                    let duration = now.signed_duration_since(datetime);

                    if duration.num_days() == 0 {
                        "Today".to_string()
                    } else {
                        datetime.format("%a %b %d %Y").to_string()
                    }
                };

                let time = {
                    let datetime: DateTime<Local> = session.updated_at.into();
                    datetime.format("%-I:%M %p").to_string()
                };

                crate::ui::components::dialog::DialogItem {
                    id: session.id.clone(),
                    name: session.title.clone(),
                    group: date_group,
                    description: String::new(),
                    tip: Some(time),
                    provider_id: String::new(),
                }
            })
            .collect();

        self.sessions_dialog_state.refresh_items(items);
    }

    fn refresh_models_dialog(&mut self) {
        use crate::model::discovery::Discovery;
        use crate::model::types::Model as ModelType;
        use crate::ui::components::dialog::DialogItem;

        let auth_dao = match crate::persistence::AuthDAO::new() {
            Ok(dao) => dao,
            Err(_) => return,
        };

        let connected_providers = match auth_dao.load() {
            Ok(providers) => providers,
            Err(_) => return,
        };

        if connected_providers.is_empty() {
            return;
        }

        let discovery = match Discovery::new() {
            Ok(d) => d,
            Err(_) => return,
        };

        let models = match tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(discovery.fetch_models())
        }) {
            Ok(models) => models,
            Err(_) => return,
        };

        let prefs = self
            .prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_preferences().ok());

        let mut model_lookup: std::collections::HashMap<(String, String), ModelType> =
            std::collections::HashMap::new();

        for model in &models {
            if connected_providers.contains_key(&model.provider_id) {
                model_lookup.insert((model.provider_id.clone(), model.id.clone()), model.clone());
            }
        }

        let favorites_set = prefs
            .as_ref()
            .map(|p| {
                p.favorite
                    .iter()
                    .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();

        let recent_set = prefs
            .as_ref()
            .map(|p| {
                p.recent
                    .iter()
                    .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();

        let mut items: Vec<DialogItem> = Vec::new();

        let add_model_item = |items: &mut Vec<DialogItem>, model: &ModelType, group: &str| {
            let is_active = self.model == model.id;
            let is_favorite =
                favorites_set.contains(&(model.provider_id.clone(), model.id.clone()));

            let tip = if is_active {
                Some("✓ Active".to_string())
            } else if is_favorite {
                Some("★ Favorite".to_string())
            } else {
                None
            };

            let description = if group == "Favorite" || group == "Recent" {
                model.provider_name.clone()
            } else {
                format!(
                    "{} | {}",
                    model.provider_name,
                    model.capabilities.join(", ")
                )
            };

            items.push(DialogItem {
                id: model.id.clone(),
                name: model.name.clone(),
                group: group.to_string(),
                description,
                tip,
                provider_id: model.provider_id.clone(),
            });
        };

        let favorites_list = prefs
            .as_ref()
            .map(|p| p.favorite.clone())
            .unwrap_or_default();

        let mut favorite_models = Vec::new();
        for fav in &favorites_list {
            if let Some(model) = model_lookup.get(&(fav.provider_id.clone(), fav.model_id.clone()))
            {
                favorite_models.push(model.clone());
            }
        }

        for model in &favorite_models {
            add_model_item(&mut items, model, "Favorite");
        }

        let recent_list = prefs.as_ref().map(|p| p.recent.clone()).unwrap_or_default();

        let mut recent_models = Vec::new();
        for recent in &recent_list {
            if favorites_set.contains(&(recent.provider_id.clone(), recent.model_id.clone())) {
                continue;
            }
            if let Some(model) =
                model_lookup.get(&(recent.provider_id.clone(), recent.model_id.clone()))
            {
                recent_models.push(model.clone());
            }
        }

        for model in &recent_models {
            add_model_item(&mut items, model, "Recent");
        }

        let mut provider_models: std::collections::HashMap<String, Vec<ModelType>> =
            std::collections::HashMap::new();

        for model in models {
            let model_key = (model.provider_id.clone(), model.id.clone());
            if favorites_set.contains(&model_key) || recent_set.contains(&model_key) {
                continue;
            }

            if connected_providers.contains_key(&model.provider_id) {
                provider_models
                    .entry(model.provider_name.clone())
                    .or_default()
                    .push(model);
            }
        }

        for (provider_name, models_list) in provider_models {
            for model in &models_list {
                add_model_item(&mut items, model, &provider_name);
            }
        }

        items.sort_by(|a, b| {
            let is_a_special = a.group == "Favorite" || a.group == "Recent";
            let is_b_special = b.group == "Favorite" || b.group == "Recent";

            if is_a_special && !is_b_special {
                return std::cmp::Ordering::Less;
            }
            if !is_a_special && is_b_special {
                return std::cmp::Ordering::Greater;
            }

            if is_a_special && is_b_special {
                if a.group == "Favorite" && b.group != "Favorite" {
                    return std::cmp::Ordering::Less;
                }
                if a.group != "Favorite" && b.group == "Favorite" {
                    return std::cmp::Ordering::Greater;
                }
                return std::cmp::Ordering::Equal;
            }

            a.group.cmp(&b.group).then(a.name.cmp(&b.name))
        });

        self.models_dialog_state.refresh_items(items);
    }

    fn cleanup_streaming(&mut self) {
        self.chunk_sender = None;
        self.chunk_receiver = None;
        self.streaming_cancel_token = None;
    }

    fn cancel_streaming(&mut self) {
        if let Some(token) = &self.streaming_cancel_token {
            token.cancel();
        }
    }

    pub fn process_streaming_chunks(&mut self) {
        let mut chunks = Vec::new();

        if let Some(receiver) = &mut self.chunk_receiver {
            while let Ok(chunk) = receiver.try_recv() {
                chunks.push(chunk);
            }
        }

        for chunk in chunks {
            match chunk {
                crate::llm::ChunkMessage::Text(text) => {
                    self.chat_state.chat.append_to_last_assistant(&text);
                }
                crate::llm::ChunkMessage::Reasoning(reasoning) => {
                    self.chat_state.chat.append_reasoning_to_last_assistant(&reasoning);
                }
                crate::llm::ChunkMessage::End => {
                    // Finalize streaming metrics from the chat's tracked values
                    self.chat_state.chat.finalize_streaming_metrics();

                    if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
                        last_msg.mark_complete();
                        // Set model and provider metadata before persisting
                        // Use the captured values from when streaming started
                        last_msg.model = self.streaming_model.clone();
                        last_msg.provider = self.streaming_provider.clone();
                        let _ = self
                            .session_manager
                            .add_message_to_current_session(last_msg);
                    }
                    self.is_streaming = false;
                    self.streaming_model = None;
                    self.streaming_provider = None;
                    self.cleanup_streaming();
                }
                crate::llm::ChunkMessage::Failed(error) => {
                    self.is_streaming = false;
                    self.chat_state.chat.finalize_streaming_metrics();
                    push_toast(ratatui_toolkit::Toast::new(
                        format!("LLM error: {}", error),
                        ratatui_toolkit::ToastLevel::Error,
                        None,
                    ));
                    if self.chat_state.chat.messages.last().is_some_and(|m| {
                        m.role == crate::session::types::MessageRole::Assistant && !m.is_complete
                    }) {
                        self.chat_state.chat.messages.pop();
                    }
                    self.cleanup_streaming();
                }
                crate::llm::ChunkMessage::Cancelled => {
                    self.is_streaming = false;
                    self.chat_state.chat.finalize_streaming_metrics();
                    push_toast(ratatui_toolkit::Toast::new(
                        "Streaming cancelled",
                        ratatui_toolkit::ToastLevel::Info,
                        None,
                    ));
                    if self.chat_state.chat.messages.last().is_some_and(|m| {
                        m.role == crate::session::types::MessageRole::Assistant && !m.is_complete
                    }) {
                        self.chat_state.chat.messages.pop();
                    }
                    self.cleanup_streaming();
                }
                crate::llm::ChunkMessage::Metrics { .. } => {
                    // Metrics are now calculated locally from streaming data
                    // This arm is kept for backward compatibility but ignored
                }

            }
        }
    }

    fn start_llm_streaming(
        &mut self,
        _user_message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::sync::mpsc;

        let (sender, receiver) = mpsc::unbounded_channel();
        let sender_clone = sender.clone();
        self.chunk_sender = Some(sender);
        self.chunk_receiver = Some(receiver);

        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.streaming_cancel_token = Some(cancel_token.clone());

        self.is_streaming = true;

        // Capture the current model and provider at the start of streaming
        // so they don't change if the user switches models during streaming
        self.streaming_model = Some(self.model.clone());
        self.streaming_provider = Some(self.provider_name.clone());

        self.chat_state.chat.add_assistant_message("");
        if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
            last_msg.is_complete = false;
        }

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let messages = self.chat_state.chat.messages.clone();

        tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(300),
                stream_llm_with_cancellation(
                    cancel_token,
                    provider_name,
                    model,
                    messages,
                    sender_clone.clone(),
                ),
            )
            .await;

            let _ = match result {
                Ok(Ok(())) => sender_clone.send(crate::llm::ChunkMessage::End),
                Ok(Err(e)) => sender_clone.send(crate::llm::ChunkMessage::Failed(e.to_string())),
                Err(_) => sender_clone.send(crate::llm::ChunkMessage::Failed(
                    "Timeout: No response within 5 minutes".to_string(),
                )),
            };
        });

        Ok(())
    }

    fn handle_message_input(&mut self, msg: String) {
        if !msg.is_empty() && self.base_focus == BaseFocus::Home {
            if self.session_manager.get_current_session_id().is_none() {
                let session_title = Self::generate_title_from_message(&msg);
                self.session_manager.create_session(Some(session_title));
            }
            let mut user_message = crate::session::types::Message::user(&msg);
            user_message.agent_mode = Some(self.agent.clone());
            user_message.model = Some(self.model.clone());
            user_message.provider = Some(self.provider_name.clone());
            let _ = self
                .session_manager
                .add_message_to_current_session(&user_message);
            self.chat_state.chat.add_user_message_with_agent_mode(&msg, self.agent.clone());
            self.base_focus = BaseFocus::Chat;

            if let Err(e) = self.start_llm_streaming(&msg) {
                push_toast(ratatui_toolkit::Toast::new(
                    format!("LLM error: {}", e),
                    ratatui_toolkit::ToastLevel::Error,
                    None,
                ));
            }
        } else if !msg.is_empty() && self.base_focus == BaseFocus::Chat {
            let mut user_message = crate::session::types::Message::user(&msg);
            user_message.agent_mode = Some(self.agent.clone());
            user_message.model = Some(self.model.clone());
            user_message.provider = Some(self.provider_name.clone());
            let _ = self
                .session_manager
                .add_message_to_current_session(&user_message);
            self.chat_state.chat.add_user_message_with_agent_mode(&msg, self.agent.clone());

            if let Err(e) = self.start_llm_streaming(&msg) {
                push_toast(ratatui_toolkit::Toast::new(
                    format!("LLM error: {}", e),
                    ratatui_toolkit::ToastLevel::Error,
                    None,
                ));
            }
        }
    }

    pub fn render(&mut self, f: &mut ratatui::Frame) {
        let size = f.area();
        self.last_frame_size = size;
        let colors = self.get_current_theme_colors();

        match self.base_focus {
            BaseFocus::Home => {
                render_home(
                    f,
                    &mut self.input,
                    self.version.clone(),
                    self.cwd.clone(),
                    git::get_current_branch(),
                    self.agent.clone(),
                    self.model.clone(),
                    self.provider_name.clone(),
                    &colors,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::ModelsDialog
                {
                    let main_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints([ratatui::layout::Constraint::Min(0)].as_ref())
                        .split(size);
                    let input_height = self.input.get_height();
                    let home_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints(
                            [
                                ratatui::layout::Constraint::Min(0),
                                ratatui::layout::Constraint::Length(input_height),
                            ]
                            .as_ref(),
                        )
                        .split(main_chunks[0]);
                    render_suggestions_popup(
                        f,
                        &self.suggestions_popup_state,
                        home_chunks[1],
                        self.overlay_focus == OverlayFocus::SuggestionsPopup,
                        colors,
                    );
                }
            }
            BaseFocus::Chat => {
                render_chat(
                    f,
                    &mut self.chat_state,
                    &mut self.input,
                    self.version.clone(),
                    self.cwd.clone(),
                    git::get_current_branch(),
                    self.agent.clone(),
                    self.model.clone(),
                    self.provider_name.clone(),
                    &colors,
                    self.is_streaming,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::ModelsDialog
                {
                    let input_height = self.input.get_height();
                    let main_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints([ratatui::layout::Constraint::Min(0)].as_ref())
                        .split(size);
                    let chat_chunks = ratatui::layout::Layout::default()
                        .direction(ratatui::layout::Direction::Vertical)
                        .constraints(
                            [
                                ratatui::layout::Constraint::Min(0),
                                ratatui::layout::Constraint::Length(input_height),
                            ]
                            .as_ref(),
                        )
                        .split(main_chunks[0]);
                    render_suggestions_popup(
                        f,
                        &self.suggestions_popup_state,
                        chat_chunks[1],
                        self.overlay_focus == OverlayFocus::SuggestionsPopup,
                        colors,
                    );
                }
            }
        }

        if self.overlay_focus == OverlayFocus::ModelsDialog
            && self.models_dialog_state.dialog.is_visible()
        {
            render_models_dialog(f, &mut self.models_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::ConnectDialog
            && self.connect_dialog_state.dialog.is_visible()
        {
            render_connect_dialog(f, &mut self.connect_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::ApiKeyInput && self.api_key_input.is_visible() {
            self.api_key_input.render(f, size);
        }

        if self.overlay_focus == OverlayFocus::SessionsDialog
            && self.sessions_dialog_state.dialog.is_visible()
        {
            render_sessions_dialog(f, &mut self.sessions_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::SessionRenameDialog
            && self.session_rename_dialog_state.is_visible()
        {
            render_session_rename_dialog(f, &mut self.session_rename_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::WhichKey {
            crate::views::which_key::render_which_key(f, &self.which_key_state, &colors);
        }

        render_toasts(f, &get_toast_manager().lock().unwrap());
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
