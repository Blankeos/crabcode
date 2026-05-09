use ratatui::crossterm::event::{self, KeyCode, KeyEvent, MouseEvent};

use crate::autocomplete::AutoComplete;
use crate::command::handlers::register_all_commands;
use crate::command::parser::InputType;
use crate::command::registry::Registry;
use crate::llm::client::stream_llm_with_cancellation;
use crate::session::manager::SessionManager;

use crate::push_toast;
use crate::toast::{self, Toast, ToastLevel};
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
use crate::views::openai_oauth_flow::{
    handle_openai_oauth_flow_key_event, handle_openai_oauth_flow_mouse_event,
    init_openai_oauth_flow, render_openai_oauth_flow, OpenAIOAuthFlowAction,
};
use crate::views::permission_dialog::{
    handle_permission_dialog_key_event, handle_permission_dialog_mouse_event,
    init_permission_dialog, render_permission_dialog, PermissionDialogAction,
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
use crate::views::themes_dialog::{
    handle_themes_dialog_key_event, handle_themes_dialog_mouse_event, init_themes_dialog,
    render_themes_dialog,
};
use crate::views::{
    ChatState, ConnectDialogState, HomeState, ModelsDialogState, OpenAIOAuthFlowState,
    PermissionDialogState, SessionRenameDialogState, SessionsDialogState, SuggestionsPopupState,
    ThemesDialogState,
};

use crate::{
    get_toast_manager,
    theme::{self, Theme},
};

use anyhow::Result;

pub fn parse_model_ref(model: &str) -> (String, String) {
    let model = model.trim();
    if let Some((provider_id, model_id)) = model.split_once('/') {
        let provider_id = provider_id.trim();
        let model_id = model_id.trim();
        if !provider_id.is_empty() && !model_id.is_empty() {
            return (provider_id.to_string(), model_id.to_string());
        }
    }
    ("opencode".to_string(), model.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BaseFocus {
    Home,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayFocus {
    None,
    ModelsDialog,
    ThemesDialog,
    ConnectDialog,
    OpenAIOAuthFlow,
    ApiKeyInput,
    SuggestionsPopup,
    SessionsDialog,
    SessionRenameDialog,
    PermissionDialog,
    SkillsDialog,
    TimelineDialog,
    MessageActions,
    WhichKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectDialogMode {
    ProviderSelection,
    OpenAIMethodSelection,
}

#[derive(Debug)]
enum OpenAIOAuthTaskMessage {
    HeadlessCode { code: String, url: String },
    Success(crate::auth::OAuthCredentials),
    Failed(String),
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
    pub themes_dialog_state: ThemesDialogState,
    themes_dialog_original_theme_index: usize,
    themes_dialog_committed: bool,
    pub connect_dialog_state: ConnectDialogState,
    connect_dialog_mode: ConnectDialogMode,
    openai_oauth_flow_state: OpenAIOAuthFlowState,
    pub sessions_dialog_state: SessionsDialogState,
    pub session_rename_dialog_state: SessionRenameDialogState,
    pub permission_dialog_state: PermissionDialogState,
    pub skills_dialog_state: crate::views::SkillsDialogState,
    pub which_key_state: crate::views::which_key::WhichKeyState,
    pub timeline_dialog_state: crate::views::timeline_dialog::TimelineDialogState,
    pub message_actions_index: Option<usize>,
    pub message_actions_dialog: Option<crate::ui::components::dialog::Dialog>,
    pub api_key_input: crate::ui::components::api_key_input::ApiKeyInput,
    openai_oauth_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<OpenAIOAuthTaskMessage>>,
    openai_oauth_in_progress: bool,
    pub prefs_dao: Option<crate::persistence::PrefsDAO>,
    pub agent: String,
    pub agent_steps: std::collections::HashMap<String, usize>,
    pub provider_timeouts: std::collections::HashMap<String, crate::config::ProviderTimeout>,
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
    pub sounds: crate::sound::ResolvedSoundsConfig,
    pub tool_permissions: crate::tools::ToolPermissions,
    pub skills_dirs: Vec<std::path::PathBuf>,
    pub is_streaming: bool,
    chunk_sender: Option<crate::llm::ChunkSender>,
    chunk_receiver: Option<crate::llm::ChunkReceiver>,
    streaming_cancel_token: Option<tokio_util::sync::CancellationToken>,
    last_frame_size: ratatui::layout::Rect,
    streaming_model: Option<String>,
    streaming_provider: Option<String>,
    last_animation_update: std::time::Instant,
    streaming_chat_len_before_assistant: usize,
    tool_call_message_indices: std::collections::HashMap<String, usize>,
    tool_call_order: Vec<String>,
    discovery: Option<crate::model::discovery::Discovery>,
    cached_usage_text: String,
    cached_usage_check: (usize, usize),
}

impl App {
    pub fn new() -> Result<Self> {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let autocomplete = AutoComplete::new(crate::autocomplete::CommandAuto::new(&registry));
        let placeholder = Self::get_random_placeholder();
        let placeholder_static: &'static str = Box::leak(placeholder.into_boxed_str());
        let mut input = Input::new().with_autocomplete(autocomplete);
        input.set_placeholder(placeholder_static);

        let cwd_path = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let cwd = cwd_path
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "?".to_string());

        let home_state = init_home();
        let mut agent = "Plan".to_string();
        let chat = Chat::new();
        let suggestions_popup_state = init_suggestions_popup(Popup::new());
        let models_dialog_state = init_models_dialog("Models", vec![]);
        let themes_dialog_state = init_themes_dialog("Themes", vec![]);
        let connect_dialog_state = init_connect_dialog();
        let openai_oauth_flow_state = init_openai_oauth_flow();
        let sessions_dialog_state = init_sessions_dialog("Sessions", vec![]);
        let permission_dialog_state = init_permission_dialog();
        let skills_dialog_state = crate::views::skills_dialog::init_skills_dialog("Skills", vec![]);
        let which_key_state = crate::views::which_key::init_which_key();
        let timeline_dialog_state = crate::views::timeline_dialog::init_timeline_dialog();
        let api_key_input = crate::ui::components::api_key_input::ApiKeyInput::new();

        let session_manager = SessionManager::new()
            .with_history()
            .unwrap_or_else(|_| SessionManager::new());

        let prefs_dao = match crate::persistence::PrefsDAO::new() {
            Ok(dao) => Some(dao),
            Err(e) => {
                crate::startup_diag!("Warning: Failed to initialize preferences DAO: {}", e);
                None
            }
        };

        let loaded_config = crate::config::ConfigLoader::load()?;
        if !loaded_config.diagnostics.info.is_empty() {
            for msg in &loaded_config.diagnostics.info {
                crate::startup_diag!("Config: {}", msg);
            }
        }
        if !loaded_config.diagnostics.warnings.is_empty() {
            for msg in &loaded_config.diagnostics.warnings {
                crate::startup_diag!("Config warning: {}", msg);
            }
        }
        if !loaded_config.diagnostics.unimplemented_keys.is_empty() {
            crate::startup_diag!(
                "Config: unimplemented keys present: {}",
                loaded_config.diagnostics.unimplemented_keys.join(", ")
            );
        }

        crate::skill::init_skill_store(&loaded_config.xdg_config_home, &loaded_config.project_root);
        crate::command::handlers::register_skill_commands(&mut registry);

        if let Some(default_agent) = loaded_config.merged_config.default_agent.clone() {
            if !default_agent.trim().is_empty() {
                agent = default_agent;
            }
        }

        let (resolved_sounds, sound_warnings) =
            crate::sound::resolve_effective_sounds(&loaded_config.merged_config.sounds);
        if !sound_warnings.is_empty() {
            for msg in &sound_warnings {
                crate::startup_diag!("Sound warning: {}", msg);
            }
        }

        let active_model_info = if let Some(ref dao) = prefs_dao {
            dao.get_active_model().ok().flatten()
        } else {
            None
        };

        if active_model_info.is_none() {
            if let (Some(ref dao), Some(model_str)) = (
                prefs_dao.as_ref(),
                loaded_config.merged_config.model.clone(),
            ) {
                let (provider_id, model_id) = parse_model_ref(&model_str);
                let _ = dao.set_active_model(provider_id, model_id);
            }
        }

        let active_model_info = if let Some(ref dao) = prefs_dao {
            dao.get_active_model().ok().flatten()
        } else {
            None
        };

        let (active_model, active_provider_name) =
            if let Some((provider_id, model_id)) = active_model_info {
                (model_id.clone(), provider_id.clone())
            } else if let Some(model_str) = loaded_config.merged_config.model.clone() {
                let (provider_id, model_id) = parse_model_ref(&model_str);
                (model_id, provider_id)
            } else {
                ("big-pickle".to_string(), "opencode".to_string())
            };

        let (themes, current_theme_index) = crate::config::discover_themes(
            &loaded_config.xdg_config_home,
            &loaded_config.project_root,
            &loaded_config.cwd,
            loaded_config.merged_config.theme.as_deref(),
        );
        let agent_steps = loaded_config.merged_config.agent_steps.clone();
        let provider_timeouts = loaded_config.merged_config.provider_timeouts.clone();

        let theme_for_colors = themes
            .get(current_theme_index)
            .or_else(|| themes.first())
            .cloned()
            .unwrap_or_else(|| {
                theme::Theme::load_from_file("src/theme.json").unwrap_or_else(|_| {
                    theme::Theme::load_from_file("src/generated_themes/ayu.json").unwrap()
                })
            });
        let colors = theme_for_colors.get_colors(true);

        let chat_state = init_chat(chat, &agent, &colors);
        let session_rename_dialog_state = init_session_rename_dialog(colors);
        let mut agent_policies = crate::tools::AgentToolPolicies::default();
        for (mode, tools) in &loaded_config.merged_config.agent_tool_policies {
            agent_policies = agent_policies.with_custom_tools(mode.clone(), tools.clone());
        }
        let tool_permissions = crate::tools::ToolPermissions::new(cwd_path.clone())
            .with_agent_policies(agent_policies);

        let discovery = crate::model::discovery::Discovery::new().ok();

        Ok(Self {
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            input,
            command_registry: registry,
            session_manager,
            home_state,
            chat_state,
            suggestions_popup_state,
            models_dialog_state,
            themes_dialog_state,
            themes_dialog_original_theme_index: 0,
            themes_dialog_committed: false,
            connect_dialog_state,
            connect_dialog_mode: ConnectDialogMode::ProviderSelection,
            openai_oauth_flow_state,
            sessions_dialog_state,
            session_rename_dialog_state,
            permission_dialog_state,
            skills_dialog_state,
            which_key_state,
            timeline_dialog_state,
            message_actions_index: None,
            message_actions_dialog: None,
            api_key_input,
            openai_oauth_receiver: None,
            openai_oauth_in_progress: false,
            prefs_dao,
            agent,
            agent_steps,
            provider_timeouts,
            model: active_model,
            provider_name: active_provider_name,
            cwd,
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
            themes,
            current_theme_index,
            dark_mode: true,
            sounds: resolved_sounds,
            tool_permissions,
            skills_dirs: loaded_config.inventory.opencode_skills_dirs,
            // Note: skills_dirs is legacy; skill loading is now handled by src/skill/mod.rs
            is_streaming: false,
            chunk_sender: None,
            chunk_receiver: None,
            streaming_cancel_token: None,
            last_frame_size: ratatui::layout::Rect::default(),
            streaming_model: None,
            streaming_provider: None,
            last_animation_update: std::time::Instant::now(),
            streaming_chat_len_before_assistant: 0,
            tool_call_message_indices: std::collections::HashMap::new(),
            tool_call_order: Vec::new(),
            discovery,
            cached_usage_text: String::new(),
            cached_usage_check: (0, 0),
        })
    }

    fn play_sound_event(&self, event: crate::sound::SoundEvent) {
        self.play_sound_event_with_notification_detail(event, None);
    }

    fn play_sound_event_with_notification_detail(
        &self,
        event: crate::sound::SoundEvent,
        detail: Option<&str>,
    ) {
        if let Some(path) = self.sounds.path_for_event(event) {
            crate::sound::play_file(path);
        }

        if self.sounds.notify_for_event(event) {
            crate::notify::notify_event(event, detail);
        }
    }

    fn completion_notification_stats(&self) -> Option<String> {
        let message = self.chat_state.chat.messages.iter().rev().find(|msg| {
            msg.role == crate::session::types::MessageRole::Assistant && msg.is_complete
        })?;

        if let (Some(t0), Some(t1), Some(tn)) = (message.t0_ms, message.t1_ms, message.tn_ms) {
            let output_tokens = message.output_tokens.or(message.token_count).unwrap_or(0);
            let total_ms = tn.saturating_sub(t0);
            let decode_ms = tn.saturating_sub(t1);

            let total_sec = total_ms as f64 / 1000.0;
            let tokens_per_sec = if decode_ms > 0 && output_tokens > 0 {
                (output_tokens as f64) / (decode_ms as f64 / 1000.0)
            } else {
                0.0
            };

            return Some(format!("{:.1}s | {:.0}t/s", total_sec, tokens_per_sec));
        }

        if let (Some(token_count), Some(duration_ms)) = (message.token_count, message.duration_ms) {
            let duration_sec = duration_ms as f64 / 1000.0;
            let tokens_per_sec = if duration_ms > 0 {
                (token_count as f64) / (duration_ms as f64 / 1000.0)
            } else {
                0.0
            };

            return Some(format!("{:.1}s | {:.0}t/s", duration_sec, tokens_per_sec));
        }

        None
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

    fn session_usage_text(&self) -> String {
        let total_tokens: usize = self
            .chat_state
            .chat
            .messages
            .iter()
            .filter_map(|m| m.token_count)
            .sum();

        if total_tokens == 0 {
            return String::new();
        }

        let token_text = format_token_count(total_tokens);
        let mut text = token_text;

        if let Some(ref discovery) = self.discovery {
            if let Some(limit) =
                discovery.get_model_limit(&self.provider_name.to_lowercase(), &self.model)
            {
                if limit > 0 {
                    let pct = ((total_tokens as f64 / limit as f64) * 100.0).round() as u32;
                    text = format!("{} ({}%)", text, pct);
                }
            }

            if let Some(cost) = discovery.get_model_pricing(
                &self.provider_name.to_lowercase(),
                &self.model,
            ) {
                let output_tokens: usize = self
                    .chat_state
                    .chat
                    .messages
                    .iter()
                    .filter_map(|m| m.output_tokens)
                    .sum();
                let total = (output_tokens.max(total_tokens)) as f64;
                let price = total / 1_000_000.0 * cost.output;
                if price > 0.001 {
                    return format!("{} \u{00b7} ${:.2}", text, price);
                }
            }
        }

        text
    }

    pub fn get_current_theme_colors(&self) -> theme::ThemeColors {
        if self.themes.is_empty() {
            return theme::ThemeColors {
                primary: ratatui::style::Color::Rgb(255, 140, 0),
                secondary: ratatui::style::Color::Rgb(255, 140, 0),
                accent: ratatui::style::Color::Rgb(255, 140, 0),
                interactive: ratatui::style::Color::Rgb(255, 140, 0),
                background: ratatui::style::Color::Reset,
                dialog_background: ratatui::style::Color::Reset,
                background_element: ratatui::style::Color::Reset,
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
                markdown_text: ratatui::style::Color::Reset,
                markdown_heading: ratatui::style::Color::Rgb(255, 140, 0),
                markdown_link: ratatui::style::Color::Rgb(0, 255, 255),
                markdown_link_text: ratatui::style::Color::Rgb(0, 255, 255),
                markdown_code: ratatui::style::Color::Rgb(0, 255, 0),
                markdown_block_quote: ratatui::style::Color::Rgb(255, 255, 0),
                markdown_emph: ratatui::style::Color::Rgb(255, 255, 0),
                markdown_strong: ratatui::style::Color::Rgb(255, 140, 0),
                markdown_horizontal_rule: ratatui::style::Color::Reset,
                markdown_list_item: ratatui::style::Color::Rgb(255, 140, 0),
                markdown_list_enumeration: ratatui::style::Color::Rgb(0, 255, 255),
                markdown_image: ratatui::style::Color::Rgb(255, 140, 0),
                markdown_image_text: ratatui::style::Color::Rgb(0, 255, 255),
                markdown_code_block: ratatui::style::Color::Reset,
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

    fn try_copy_selection(&mut self) -> bool {
        // Check chat selection
        if self.chat_state.chat.has_selection() {
            let colors = self.get_current_theme_colors();
            let model = self.model.clone();
            // Use a default max_width for text extraction
            let max_width = 80;
            if let Some(text) = self
                .chat_state
                .chat
                .get_selected_text(max_width, &model, &colors)
            {
                let _ = crate::utils::clipboard::copy_text(&text);
                push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
            }
            self.chat_state.chat.selection.clear();
            return true;
        }
        // Check input selection
        if self.input.has_selection() {
            let text = self.input.get_selected_text();
            if !text.is_empty() {
                let _ = crate::utils::clipboard::copy_text(&text);
                push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
            }
            self.input.clear_selection();
            return true;
        }
        false
    }

    fn clear_selection(&mut self) -> bool {
        if self.chat_state.chat.has_selection() {
            self.chat_state.chat.selection.clear();
            return true;
        }
        if self.input.has_selection() {
            self.input.clear_selection();
            return true;
        }
        false
    }

    fn copy_chat_selection(&mut self) {
        if !self.chat_state.chat.has_selection() {
            return;
        }
        // Don't copy zero-width selections (e.g., single click without drag)
        let ((s_line, s_col), (e_line, e_col)) = self.chat_state.chat.selection.range();
        if s_line == e_line && s_col == e_col {
            return;
        }
        let colors = self.get_current_theme_colors();
        let model = self.model.clone();
        let max_width = self.last_frame_size.width.saturating_sub(4) as usize;
        if let Some(text) =
            self.chat_state
                .chat
                .get_selected_text(max_width.max(40), &model, &colors)
        {
            if !text.trim().is_empty() {
                let _ = crate::utils::clipboard::copy_text(&text);
                push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
            }
        }
    }

    pub fn handle_keys(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if key.modifiers == event::KeyModifiers::CONTROL => {
                // If text is selected (chat or input), copy to clipboard first
                if self.try_copy_selection() {
                    return;
                }
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
                // When the suggestions popup is open, the keystroke should be handled either by the
                // popup itself (navigation/autocomplete) or by the input. If we return `false` here
                // and the popup closes during `update_suggestions()`, the same key event can be
                // processed again by the base input handler, resulting in duplicated characters.
                let popup_handled = self.handle_suggestions_popup_keys(key);
                if popup_handled {
                    true
                } else {
                    let input_handled = self.input.handle_event(key);
                    self.update_suggestions();
                    input_handled
                }
            }
            OverlayFocus::ModelsDialog => {
                if key.code == KeyCode::Char('a') && key.modifiers == event::KeyModifiers::CONTROL {
                    self.models_dialog_state.dialog.hide();
                    if let crate::command::parser::InputType::Command(parsed) =
                        crate::command::parser::parse_input("/connect")
                    {
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_command_input(parsed));
                        });
                    }
                    return;
                }
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

                        push_toast(Toast::new(
                            format!("Switched to: {}", model_id_clone),
                            ToastLevel::Info,
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

                        push_toast(Toast::new(
                            if is_favorite {
                                "Added to favorites"
                            } else {
                                "Removed from favorites"
                            },
                            ToastLevel::Info,
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
            OverlayFocus::ThemesDialog => {
                let action = handle_themes_dialog_key_event(&mut self.themes_dialog_state, key);

                match action {
                    crate::views::themes_dialog::ThemesDialogAction::PreviewTheme { theme_id } => {
                        if let Some((idx, _)) = self
                            .themes
                            .iter()
                            .enumerate()
                            .find(|(_, t)| t.id == theme_id)
                        {
                            self.current_theme_index = idx;
                        }
                    }
                    crate::views::themes_dialog::ThemesDialogAction::SelectTheme { theme_id } => {
                        if let Some((idx, theme)) = self
                            .themes
                            .iter()
                            .enumerate()
                            .find(|(_, t)| t.id == theme_id)
                        {
                            self.current_theme_index = idx;
                            self.themes_dialog_committed = true;
                            push_toast(Toast::new(
                                format!("Theme: {}", theme.id),
                                ToastLevel::Info,
                                None,
                            ));
                        }
                    }
                    crate::views::themes_dialog::ThemesDialogAction::None => {}
                }

                if !self.themes_dialog_state.dialog.is_visible() {
                    if !self.themes_dialog_committed {
                        self.current_theme_index = self.themes_dialog_original_theme_index;
                    }
                    self.overlay_focus = OverlayFocus::None;
                }
                true
            }
            OverlayFocus::ConnectDialog => {
                if key.code == KeyCode::Char('d') && key.modifiers == event::KeyModifiers::CONTROL {
                    self.disconnect_selected_provider();
                    return;
                }

                if handle_connect_dialog_key_event(&mut self.connect_dialog_state, key) {
                    return;
                }
                if !self.connect_dialog_state.dialog.is_visible() {
                    if let Some(selected_item) =
                        get_pending_selection(&mut self.connect_dialog_state)
                    {
                        self.handle_connect_dialog_selection(selected_item);
                        return;
                    }
                    self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
                    self.overlay_focus = OverlayFocus::None;
                }
                false
            }
            OverlayFocus::OpenAIOAuthFlow => {
                let action =
                    handle_openai_oauth_flow_key_event(&mut self.openai_oauth_flow_state, key);
                match action {
                    OpenAIOAuthFlowAction::Handled => true,
                    OpenAIOAuthFlowAction::NotHandled => false,
                    OpenAIOAuthFlowAction::Close => {
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    OpenAIOAuthFlowAction::CopyLink(url) => {
                        match crate::utils::clipboard::copy_text(&url) {
                            Ok(_) => push_toast(Toast::new(
                                "Copied OpenAI login link",
                                ToastLevel::Info,
                                None,
                            )),
                            Err(err) => push_toast(Toast::new(
                                format!("Failed to copy link: {}", err),
                                ToastLevel::Error,
                                None,
                            )),
                        }
                        true
                    }
                }
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
                            self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
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
                    SessionsDialogAction::PendingDelete(_id) => {
                        self.sessions_dialog_state.dialog.pending_delete_id =
                            Some(_id.clone());
                        true
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
                        let was_current = self
                            .session_manager
                            .get_current_session_id()
                            .map_or(false, |current| *current == id);
                        self.session_manager.delete_session(&id);
                        if let Some(pending) = crate::views::sessions_dialog::get_pending_delete(
                            &mut self.sessions_dialog_state,
                        ) {
                            self.session_manager.delete_session(&pending);
                        }
                        let remaining = self.session_manager.list_sessions();
                        if remaining.is_empty() {
                            self.sessions_dialog_state.dialog.hide();
                            self.overlay_focus = OverlayFocus::None;
                        }
                        self.refresh_sessions_dialog();
                        if was_current {
                            self.chat_state.chat.clear();
                            self.base_focus = BaseFocus::Home;
                            self.sessions_dialog_state.dialog.hide();
                            self.overlay_focus = OverlayFocus::None;
                        }
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
            OverlayFocus::PermissionDialog => {
                let action =
                    handle_permission_dialog_key_event(&mut self.permission_dialog_state, key);
                match action {
                    PermissionDialogAction::Respond(response) => {
                        self.permission_dialog_state.respond_current(response);
                        if self.permission_dialog_state.has_active() {
                            self.overlay_focus = OverlayFocus::PermissionDialog;
                        } else {
                            self.chat_state.chat.resume_streaming_tps_timer();
                            self.overlay_focus = OverlayFocus::None;
                        }
                        true
                    }
                    PermissionDialogAction::Handled => true,
                    PermissionDialogAction::NotHandled => true,
                }
            }
            OverlayFocus::SkillsDialog => {
                let action = crate::views::skills_dialog::handle_skills_dialog_key_event(
                    &mut self.skills_dialog_state,
                    key,
                );
                match action {
                    crate::views::skills_dialog::SkillsDialogAction::SelectSkill {
                        skill_id: _,
                    } => {
                        if !self.skills_dialog_state.dialog.is_visible() {
                            self.overlay_focus = OverlayFocus::None;
                        }
                        true
                    }
                    crate::views::skills_dialog::SkillsDialogAction::None => {
                        if !self.skills_dialog_state.dialog.is_visible() {
                            self.overlay_focus = OverlayFocus::None;
                        }
                        false
                    }
                }
            }
            OverlayFocus::TimelineDialog => {
                let action = crate::views::timeline_dialog::handle_timeline_dialog_key_event(
                    &mut self.timeline_dialog_state,
                    key,
                );
                match action {
                    crate::views::timeline_dialog::TimelineDialogAction::Close => {
                        self.chat_state.chat.clear_highlighted_message();
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    crate::views::timeline_dialog::TimelineDialogAction::Select(idx) => {
                        self.chat_state.chat.scroll_to_message_index(idx);
                        self.chat_state.chat.set_highlighted_message(Some(idx));
                        self.show_message_actions(idx);
                        true
                    }
                    crate::views::timeline_dialog::TimelineDialogAction::Navigate(idx) => {
                        self.chat_state.chat.scroll_to_message_index(idx);
                        self.chat_state.chat.set_highlighted_message(Some(idx));
                        true
                    }
                    crate::views::timeline_dialog::TimelineDialogAction::Handled => true,
                    crate::views::timeline_dialog::TimelineDialogAction::NotHandled => false,
                }
            }
            OverlayFocus::MessageActions => {
                if let Some(ref mut dialog) = self.message_actions_dialog {
                    if key.code == KeyCode::Esc {
                        self.close_message_actions();
                        true
                    } else if key.code == KeyCode::Enter {
                        if let Some(selected) = dialog.get_selected() {
                            let action_clone = selected.provider_id.clone();
                            self.execute_message_action(&action_clone);
                            true
                        } else {
                            dialog.handle_key_event(key)
                        }
                    } else {
                        dialog.handle_key_event(key)
                    }
                } else {
                    false
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
                    crate::views::which_key::WhichKeyAction::ShowThemes => {
                        self.overlay_focus = OverlayFocus::None;
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_input("/themes"));
                        });
                    }
                    crate::views::which_key::WhichKeyAction::ShowSessions => {
                        self.overlay_focus = OverlayFocus::None;
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_input("/sessions"));
                        });
                    }
                    crate::views::which_key::WhichKeyAction::ShowTimeline => {
                        self.overlay_focus = OverlayFocus::None;
                        self.open_timeline_dialog();
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
                    crate::views::which_key::WhichKeyAction::ScrollUp => {
                        self.overlay_focus = OverlayFocus::None;
                        self.chat_state.chat.scroll_up(1);
                    }
                    crate::views::which_key::WhichKeyAction::ScrollDown => {
                        self.overlay_focus = OverlayFocus::None;
                        self.chat_state.chat.scroll_down(1);
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
                self.which_key_state
                    .set_chat_active(self.base_focus == BaseFocus::Chat);
                self.which_key_state.show();
                true
            }
            KeyCode::Tab => {
                if self.agent == "Plan" {
                    self.agent = "Build".to_string();
                } else {
                    self.agent = "Plan".to_string();
                }

                let colors = self.get_current_theme_colors();
                let agent_color = crate::theme::agent_color(&self.agent, &colors);
                self.chat_state.wave_spinner.set_color(agent_color);
                true
            }
            KeyCode::Esc => {
                // If text is selected, clear selection first
                if self.clear_selection() {
                    return true;
                }
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
                    if self.is_streaming {
                        return true;
                    }
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
        // If chat text is selected and user presses a key, clear the selection
        // (unless it's Ctrl+C or Escape which are handled earlier)
        self.chat_state.chat.selection.clear();

        match key.code {
            KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
                if self.is_streaming {
                    return;
                }
                let input_text = self.input.get_text();
                if !input_text.is_empty() {
                    use crate::command::parser::parse_input;

                    match parse_input(&input_text) {
                        crate::command::parser::InputType::Command(parsed) => {
                            // Don't save commands to prompt history
                            tokio::task::block_in_place(|| {
                                let rt = tokio::runtime::Handle::current();
                                rt.block_on(self.process_command_input(parsed));
                            });
                        }
                        crate::command::parser::InputType::Message(msg) => {
                            // Only save messages (not commands) to prompt history
                            self.input.save_current_to_history();
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
            let suggestions = self.input.get_autocomplete_suggestions(self.base_focus == BaseFocus::Chat);
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
        // If text is selected and user clicks on an overlay, clear selection instead
        if self.overlay_focus != OverlayFocus::None
            && self.chat_state.chat.has_selection()
            && matches!(
                mouse.kind,
                ratatui::crossterm::event::MouseEventKind::Down(_)
            )
        {
            self.copy_chat_selection();
            self.chat_state.chat.selection.clear();
            return;
        }

        if self.overlay_focus == OverlayFocus::ModelsDialog {
            handle_models_dialog_mouse_event(&mut self.models_dialog_state, mouse);
            if !self.models_dialog_state.dialog.is_visible() {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::PermissionDialog {
            let _ = handle_permission_dialog_mouse_event(&mut self.permission_dialog_state, mouse);
        } else if self.overlay_focus == OverlayFocus::ThemesDialog {
            let before = self
                .themes_dialog_state
                .dialog
                .get_selected()
                .map(|it| it.id.clone());

            handle_themes_dialog_mouse_event(&mut self.themes_dialog_state, mouse);

            if !self.themes_dialog_state.dialog.is_visible() {
                if !self.themes_dialog_committed {
                    self.current_theme_index = self.themes_dialog_original_theme_index;
                }
                self.overlay_focus = OverlayFocus::None;
                return;
            }

            let after = self
                .themes_dialog_state
                .dialog
                .get_selected()
                .map(|it| it.id.clone());

            if before != after {
                if let Some(theme_id) = after {
                    if let Some((idx, _)) = self
                        .themes
                        .iter()
                        .enumerate()
                        .find(|(_, t)| t.id == theme_id)
                    {
                        self.current_theme_index = idx;
                    }
                }
            }
        } else if self.overlay_focus == OverlayFocus::ConnectDialog {
            handle_connect_dialog_mouse_event(&mut self.connect_dialog_state, mouse);
            if !self.connect_dialog_state.dialog.is_visible() {
                if let Some(selected_item) = get_pending_selection(&mut self.connect_dialog_state) {
                    self.handle_connect_dialog_selection(selected_item);
                    return;
                }
                self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::OpenAIOAuthFlow {
            let action =
                handle_openai_oauth_flow_mouse_event(&mut self.openai_oauth_flow_state, mouse);
            match action {
                OpenAIOAuthFlowAction::Handled | OpenAIOAuthFlowAction::NotHandled => {}
                OpenAIOAuthFlowAction::Close => {
                    self.overlay_focus = OverlayFocus::None;
                }
                OpenAIOAuthFlowAction::CopyLink(url) => {
                    match crate::utils::clipboard::copy_text(&url) {
                        Ok(_) => push_toast(Toast::new(
                            "Copied OpenAI login link",
                            ToastLevel::Info,
                            None,
                        )),
                        Err(err) => push_toast(Toast::new(
                            format!("Failed to copy link: {}", err),
                            ToastLevel::Error,
                            None,
                        )),
                    }
                }
            }
        } else if self.overlay_focus == OverlayFocus::SessionsDialog {
            handle_sessions_dialog_mouse_event(&mut self.sessions_dialog_state, mouse);
            if !self.sessions_dialog_state.dialog.is_visible() {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::SkillsDialog {
            crate::views::skills_dialog::handle_skills_dialog_mouse_event(
                &mut self.skills_dialog_state,
                mouse,
            );
            if !self.skills_dialog_state.dialog.is_visible() {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::TimelineDialog {
            if let Some(idx) = crate::views::timeline_dialog::handle_timeline_dialog_mouse_event(
                &mut self.timeline_dialog_state,
                mouse,
            ) {
                self.chat_state.chat.scroll_to_message_index(idx);
                self.chat_state.chat.set_highlighted_message(Some(idx));
            }
            if !self.timeline_dialog_state.dialog.is_visible() {
                self.chat_state.chat.clear_highlighted_message();
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::MessageActions {
            let maybe_action = if let Some(ref mut dialog) = self.message_actions_dialog {
                let handled = dialog.handle_mouse_event(mouse);
                if handled {
                    dialog.get_selected().map(|s| s.provider_id.clone())
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(action) = maybe_action {
                self.execute_message_action(&action);
            }
            if self
                .message_actions_dialog
                .as_ref()
                .map(|d| !d.is_visible())
                .unwrap_or(false)
            {
                self.close_message_actions();
            }
        } else if self.overlay_focus == OverlayFocus::None {
            // If chat has a selection and user clicks outside chat area, clear it
            if self.chat_state.chat.has_selection() && self.base_focus == BaseFocus::Chat {
                let size = self.last_frame_size;
                let main_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints(
                        [
                            ratatui::layout::Constraint::Min(0),
                            ratatui::layout::Constraint::Length(1),
                        ]
                        .as_ref(),
                    )
                    .split(size);
                let input_height = self.input.get_height() as u16;
                let above_status_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints(
                        [
                            ratatui::layout::Constraint::Length(1), // Top padding
                            ratatui::layout::Constraint::Min(0),    // Chat content
                            ratatui::layout::Constraint::Length(1), // Bottom padding
                            ratatui::layout::Constraint::Length(input_height),
                            ratatui::layout::Constraint::Length(1), // Help bar
                            ratatui::layout::Constraint::Length(1), // Blank
                        ]
                        .as_ref(),
                    )
                    .split(main_chunks[0]);
                let chat_area = above_status_chunks[1];

                let point = ratatui::layout::Position::new(mouse.column, mouse.row);
                if !chat_area.contains(point) {
                    // Click outside chat area, copy selection before clearing
                    self.copy_chat_selection();
                    self.chat_state.chat.selection.clear();
                }
            }

            // Handle mouse events for chat scrolling/selection when in chat mode
            if self.base_focus == BaseFocus::Chat {
                let size = self.last_frame_size;
                let main_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints(
                        [
                            ratatui::layout::Constraint::Min(0),
                            ratatui::layout::Constraint::Length(1),
                        ]
                        .as_ref(),
                    )
                    .split(size);
                let input_height = self.input.get_height() as u16;
                let above_status_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints(
                        [
                            ratatui::layout::Constraint::Length(1), // Top padding
                            ratatui::layout::Constraint::Min(0),    // Chat content
                            ratatui::layout::Constraint::Length(1), // Bottom padding
                            ratatui::layout::Constraint::Length(input_height),
                            ratatui::layout::Constraint::Length(1), // Help bar
                            ratatui::layout::Constraint::Length(1), // Blank
                        ]
                        .as_ref(),
                    )
                    .split(main_chunks[0]);
                let chat_area = above_status_chunks[1];

                let had_selection = self.chat_state.chat.has_selection();
                let was_dragging = self.chat_state.chat.selection.is_dragging;

                if self.chat_state.chat.handle_mouse_event(mouse, chat_area) {
                    // Auto-copy when selection is finalized (mouse up after drag)
                    if !had_selection && self.chat_state.chat.has_selection() {
                        // New selection just started, don't copy yet
                    } else if was_dragging && !self.chat_state.chat.selection.is_dragging {
                        // Selection was just finalized (mouse up)
                        self.copy_chat_selection();
                    }
                    return;
                }
            }

            // Handle mouse events for the main input when no overlay is focused
            if self.input.handle_mouse_event(mouse) {
                // Auto-copy input selection on mouse up (after drag select)
                if matches!(
                    mouse.kind,
                    ratatui::crossterm::event::MouseEventKind::Up(
                        ratatui::crossterm::event::MouseButton::Left
                    )
                ) {
                    let text = self.input.get_selected_text();
                    if !text.is_empty() {
                        let _ = crate::utils::clipboard::copy_text(&text);
                        push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
                    }
                }
                self.update_suggestions();
            }
        }
    }

    pub fn handle_paste(&mut self, text: String) {
        const MAX_PASTE_SIZE: usize = 20 * 1024 * 1024;

        if text.len() > MAX_PASTE_SIZE {
            push_toast(Toast::new(
                format!(
                    "Paste content too large ({}MB). Maximum is 20MB.",
                    text.len() / 1024 / 1024
                ),
                ToastLevel::Warning,
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
            (_, OverlayFocus::ThemesDialog) => {
                self.themes_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.themes_dialog_state.dialog.set_search_query(
                    self.themes_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
                self.themes_dialog_state.dialog.selected_index = 0;

                if let Some(theme_id) = self
                    .themes_dialog_state
                    .dialog
                    .get_selected()
                    .map(|it| it.id.clone())
                {
                    if let Some((idx, _)) = self
                        .themes
                        .iter()
                        .enumerate()
                        .find(|(_, t)| t.id == theme_id)
                    {
                        self.current_theme_index = idx;
                    }
                }
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
            (_, OverlayFocus::SkillsDialog) => {
                self.skills_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.skills_dialog_state.dialog.set_search_query(
                    self.skills_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
                self.skills_dialog_state.dialog.selected_index = 0;
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
        if self.is_streaming {
            return;
        }
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
        if parsed.name == "copy" && self.base_focus == BaseFocus::Chat {
            let messages = &self.chat_state.chat.messages;
            let session_title = self
                .session_manager
                .get_current_session()
                .map(|s| s.title.clone())
                .unwrap_or_else(|| "Untitled".to_string());
            let mut transcript = format!("# {}\n\n", session_title);
            for msg in messages {
                match msg.role {
                    crate::session::types::MessageRole::User => {
                        transcript.push_str("## User\n\n");
                        transcript.push_str(&msg.content);
                        transcript.push_str("\n\n---\n\n");
                    }
                    crate::session::types::MessageRole::Assistant => {
                        let agent = msg.agent_mode.as_ref().map_or("Build", |a| a.as_str());
                        let model = msg.model.as_deref().unwrap_or("unknown");
                        let duration = msg
                            .duration_ms
                            .map(|ms| format!(" · {:.1}s", ms as f64 / 1000.0))
                            .unwrap_or_default();
                        transcript.push_str(&format!(
                            "## Assistant ({agent} · {model}{duration})\n\n"
                        ));
                        transcript.push_str(&msg.content);
                        transcript.push_str("\n\n---\n\n");
                    }
                    crate::session::types::MessageRole::Tool => {
                        transcript.push_str("**Tool Result**\n\n");
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                            if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                                transcript.push_str(&format!("**Tool:** {}\n", name));
                            }
                            if let Some(preview) = v.get("output_preview").and_then(|p| p.as_str())
                            {
                                transcript.push_str(&format!("```\n{}\n```\n", preview));
                            }
                        }
                        transcript.push_str("\n---\n\n");
                    }
                    _ => {}
                }
            }
            match crate::utils::clipboard::copy_text(&transcript) {
                Ok(_) => {
                    push_toast(Toast::new(
                        "Session transcript copied to clipboard!",
                        ToastLevel::Info,
                        None,
                    ));
                }
                Err(e) => {
                    push_toast(Toast::new(
                        format!("Failed to copy: {}", e),
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                }
            }
            return;
        }
        if parsed.name == "themes" {
                    self.show_themes_dialog();
                    return;
                }
                if parsed.name == "skills" {
                    self.show_skills_dialog();
                    return;
                }
                if parsed.name == "rename" && parsed.args.is_empty() && self.base_focus == BaseFocus::Chat {
                    if let Some(session) = self.session_manager.get_current_session() {
                        let id = session.id.clone();
                        let title = session.title.clone();
                        drop(session);
                        self.session_rename_dialog_state
                            .set_colors(self.get_current_theme_colors());
                        self.session_rename_dialog_state.show(id, title);
                        self.overlay_focus = OverlayFocus::SessionRenameDialog;
                    }
                    return;
                }
                if parsed.name == "timeline" && self.base_focus == BaseFocus::Chat {
                    self.open_timeline_dialog();
                    return;
                }
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
                        } else if self.base_focus == BaseFocus::Home
                            && parsed.name != "refreshmodels"
                        {
                            self.base_focus = BaseFocus::Chat;
                        }
                        // Only add non-empty messages to the chat, and don't add exit message
                        if parsed.name != "exit" && !msg.is_empty() {
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
                        self.play_sound_event(crate::sound::SoundEvent::Error);
                        if msg.starts_with("Unknown command:") {
                            push_toast(Toast::new(
                                msg,
                                ToastLevel::Error,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                        } else {
                            let error_msg = format!("Error: {}", msg);
                            let error_message =
                                crate::session::types::Message::assistant(error_msg.clone());
                            let _ = self
                                .session_manager
                                .add_message_to_current_session(&error_message);
                            self.chat_state.chat.add_assistant_message(error_msg);
                        }
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
                            self.connect_dialog_state =
                                crate::views::ConnectDialogState::with_items(title, dialog_items);
                            self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
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
                            self.show_sessions_dialog(title, dialog_items);
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
        if parsed.name == "themes" {
            self.show_themes_dialog();
            return;
        }
        if parsed.name == "rename" && parsed.args.is_empty() && self.base_focus == BaseFocus::Chat {
            if let Some(session) = self.session_manager.get_current_session() {
                let id = session.id.clone();
                let title = session.title.clone();
                drop(session);
                self.session_rename_dialog_state
                    .set_colors(self.get_current_theme_colors());
                self.session_rename_dialog_state.show(id, title);
                self.overlay_focus = OverlayFocus::SessionRenameDialog;
            }
            return;
        }
        if parsed.name == "timeline" && self.base_focus == BaseFocus::Chat {
            self.open_timeline_dialog();
            return;
        }
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
                } else if self.base_focus == BaseFocus::Home && parsed.name != "refreshmodels" {
                    self.base_focus = BaseFocus::Chat;
                }
                // Don't add exit message to chat
                if parsed.name != "exit" && !msg.is_empty() {
                    let assistant_message = crate::session::types::Message::assistant(msg.clone());
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
                self.play_sound_event(crate::sound::SoundEvent::Error);
                if msg.starts_with("Unknown command:") {
                    push_toast(Toast::new(
                        msg,
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                } else {
                    let error_msg = format!("Error: {}", msg);
                    let error_message =
                        crate::session::types::Message::assistant(error_msg.clone());
                    let _ = self
                        .session_manager
                        .add_message_to_current_session(&error_message);
                    self.chat_state.chat.add_assistant_message(error_msg);
                }
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
                    self.connect_dialog_state =
                        crate::views::ConnectDialogState::with_items(title, dialog_items);
                    self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
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
                    self.show_sessions_dialog(title, dialog_items);
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
        use chrono::{DateTime, Local, Timelike, Utc};

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
                    let hour = datetime.time().hour12();
                    let am_pm = if hour.0 { "PM" } else { "AM" };
                    format!("{}:{:02} {}", hour.1, datetime.time().minute(), am_pm)
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

    fn open_timeline_dialog(&mut self) {
        let messages: Vec<crate::session::types::Message> =
            match self.session_manager.get_current_session() {
                Some(s) => s.messages.clone(),
                None => return,
            };

        self.timeline_dialog_state.refresh_messages(&messages);
        self.timeline_dialog_state.show();
        self.overlay_focus = OverlayFocus::TimelineDialog;

        if let Some(selected) = self.timeline_dialog_state.dialog.get_selected() {
            if let Ok(idx) = selected.id.parse::<usize>() {
                self.chat_state.chat.scroll_to_message_index(idx);
                self.chat_state.chat.set_highlighted_message(Some(idx));
            }
        }
    }

    fn show_message_actions(&mut self, idx: usize) {
        use crate::ui::components::dialog::{Dialog, DialogItem};

        self.message_actions_index = Some(idx);

        let items = vec![
            DialogItem {
                id: "copy".to_string(),
                name: "Copy".to_string(),
                group: String::new(),
                description: "Copy message to clipboard".to_string(),
                tip: None,
                provider_id: "copy".to_string(),
            },
            DialogItem {
                id: "fork".to_string(),
                name: "Fork at this point".to_string(),
                group: String::new(),
                description: "Create new session (Will include this message)".to_string(),
                tip: None,
                provider_id: "fork".to_string(),
            },
            DialogItem {
                id: "undo".to_string(),
                name: "Undo".to_string(),
                group: String::new(),
                description: "Remove messages from here onward".to_string(),
                tip: None,
                provider_id: "undo".to_string(),
            },
        ];

        let mut dialog = Dialog::with_items("Message Actions", items);
        dialog.show();
        self.message_actions_dialog = Some(dialog);
        self.overlay_focus = OverlayFocus::MessageActions;
    }

    fn execute_message_action(&mut self, action: &str) {
        let idx = match self.message_actions_index {
            Some(i) => i,
            None => return,
        };

        match action {
            "copy" => {
                if let Some(session) = self.session_manager.get_current_session() {
                    if let Some(msg) = session.messages.get(idx) {
                        let _ = crate::utils::clipboard::copy_text(&msg.content);
                        push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
                    }
                }
                self.close_message_actions();
            }
            "fork" => {
                let messages_to_fork: Vec<crate::session::types::Message> = {
                    if let Some(session) = self.session_manager.get_current_session() {
                        session.messages.iter().take(idx + 1).cloned().collect()
                    } else {
                        return;
                    }
                };

                let fork_title = messages_to_fork
                    .last()
                    .map(|msg| {
                        let preview = msg
                            .content
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("fork");
                        let truncated: String = preview.chars().take(40).collect();
                        if truncated.len() < preview.len() {
                            format!("{}...", truncated)
                        } else {
                            truncated
                        }
                    })
                    .unwrap_or_default();

                let _ = self.session_manager.create_session(Some(fork_title));
                for msg in &messages_to_fork {
                    let _ = self.session_manager.add_message_to_current_session(msg);
                }

                self.chat_state.chat.clear();
                self.chat_state.chat.messages = messages_to_fork;
                self.chat_state.chat.scroll_offset = usize::MAX;
                self.chat_state.chat.clear_highlighted_message();
                self.base_focus = BaseFocus::Chat;

                push_toast(Toast::new(
                    format!("Forked session from message {}", idx + 1),
                    ToastLevel::Info,
                    None,
                ));

                self.close_message_actions();
                self.timeline_dialog_state.hide();
                self.overlay_focus = OverlayFocus::None;
            }
            "undo" => {
                let remaining: Vec<crate::session::types::Message> = {
                    if let Some(session) = self.session_manager.get_current_session() {
                        session.messages.truncate(idx);
                        session.messages.clone()
                    } else {
                        return;
                    }
                };

                self.chat_state.chat.clear();
                for msg in &remaining {
                    self.chat_state.chat.add_message(msg.clone());
                }
                self.chat_state.chat.scroll_offset = usize::MAX;
                self.chat_state.chat.clear_highlighted_message();

                push_toast(Toast::new(
                    format!("Removed {} message(s)", idx),
                    ToastLevel::Info,
                    None,
                ));

                self.close_message_actions();
                self.timeline_dialog_state.hide();
                self.overlay_focus = OverlayFocus::None;
            }
            _ => {}
        }
    }

    fn quit(&mut self) {
        self.running = false;
    }

    fn close_message_actions(&mut self) {
        self.message_actions_index = None;
        self.message_actions_dialog = None;
        self.overlay_focus = OverlayFocus::TimelineDialog;
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
                Some("Active".to_string())
            } else if is_favorite {
                Some("♥︎ Favorite".to_string())
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

    fn show_sessions_dialog(
        &mut self,
        title: impl Into<String>,
        items: Vec<crate::ui::components::dialog::DialogItem>,
    ) {
        self.sessions_dialog_state = init_sessions_dialog(title, items);

        let current_session_id = self.session_manager.get_current_session_id().cloned();
        if let Some(session_id) = current_session_id {
            let _ = self
                .sessions_dialog_state
                .dialog
                .select_item_by_key(&session_id, "");
        }

        self.sessions_dialog_state.dialog.show();
        self.overlay_focus = OverlayFocus::SessionsDialog;
    }

    fn show_themes_dialog(&mut self) {
        use crate::ui::components::dialog::DialogItem;

        let current_id = self
            .themes
            .get(self.current_theme_index)
            .map(|t| t.id.clone());

        let mut items: Vec<DialogItem> = self
            .themes
            .iter()
            .map(|t| {
                let is_active = current_id.as_deref() == Some(t.id.as_str());
                DialogItem {
                    id: t.id.clone(),
                    name: t.id.clone(),
                    group: String::new(),
                    description: String::new(),
                    tip: if is_active {
                        Some("Active".to_string())
                    } else {
                        None
                    },
                    provider_id: String::new(),
                }
            })
            .collect();

        items.sort_by(|a, b| a.id.cmp(&b.id));

        self.themes_dialog_state = init_themes_dialog("Themes", items);

        if let Some(theme_id) = current_id.as_deref() {
            let _ = self
                .themes_dialog_state
                .dialog
                .select_item_by_key(theme_id, "");
        }

        self.themes_dialog_state.dialog.show();
        self.themes_dialog_original_theme_index = self.current_theme_index;
        self.themes_dialog_committed = false;
        self.overlay_focus = OverlayFocus::ThemesDialog;
    }

    fn show_skills_dialog(&mut self) {
        use crate::ui::components::dialog::DialogItem;

        let mut items: Vec<DialogItem> = Vec::new();

        if let Some(store) = crate::skill::get_skill_store() {
            for skill in store.all() {
                items.push(DialogItem {
                    id: skill.name.clone(),
                    name: skill.name.clone(),
                    group: "Skills".to_string(),
                    description: skill.description.clone().unwrap_or_default(),
                    tip: if skill.description.is_some() {
                        None
                    } else {
                        Some("No description".to_string())
                    },
                    provider_id: String::new(),
                });
            }
        }

        items.sort_by(|a, b| a.id.cmp(&b.id));

        self.skills_dialog_state = crate::views::skills_dialog::init_skills_dialog("Skills", items);
        self.skills_dialog_state.dialog.show();
        self.overlay_focus = OverlayFocus::SkillsDialog;
    }

    fn show_openai_connect_methods(&mut self) {
        use crate::ui::components::dialog::DialogItem;

        let items = vec![
            DialogItem {
                id: "openai-oauth-browser".to_string(),
                name: "ChatGPT Plus/Pro (browser)".to_string(),
                group: "OpenAI".to_string(),
                description: "OAuth via browser callback".to_string(),
                tip: None,
                provider_id: "openai".to_string(),
            },
            DialogItem {
                id: "openai-oauth-headless".to_string(),
                name: "ChatGPT Plus/Pro (headless)".to_string(),
                group: "OpenAI".to_string(),
                description: "Device code login flow".to_string(),
                tip: None,
                provider_id: "openai".to_string(),
            },
            DialogItem {
                id: "openai-api-key".to_string(),
                name: "Manually enter API key".to_string(),
                group: "OpenAI".to_string(),
                description: "Use OpenAI API key".to_string(),
                tip: None,
                provider_id: "openai".to_string(),
            },
        ];

        self.connect_dialog_state = crate::views::ConnectDialogState::new(
            crate::ui::components::dialog::Dialog::with_items("Connect OpenAI", items),
        );
        self.connect_dialog_state.dialog.show();
        self.connect_dialog_mode = ConnectDialogMode::OpenAIMethodSelection;
        self.overlay_focus = OverlayFocus::ConnectDialog;
    }

    fn reopen_connect_dialog(&mut self, select_provider_id: Option<&str>) {
        if let crate::command::parser::InputType::Command(parsed) =
            crate::command::parser::parse_input("/connect")
        {
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(self.process_command_input(parsed));
            });
        }

        if let Some(provider_id) = select_provider_id {
            let _ = self
                .connect_dialog_state
                .dialog
                .select_item_by_key(provider_id, "");
        }
    }

    fn disconnect_selected_provider(&mut self) {
        if self.connect_dialog_mode != ConnectDialogMode::ProviderSelection {
            push_toast(Toast::new(
                "Disconnect is available in provider list",
                ToastLevel::Info,
                None,
            ));
            return;
        }

        let selected_item = match self.connect_dialog_state.dialog.get_selected() {
            Some(item) => item.clone(),
            None => {
                push_toast(Toast::new("No provider selected", ToastLevel::Info, None));
                return;
            }
        };

        let provider_id = selected_item.id;
        let provider_name = selected_item.name;

        let auth_dao = match crate::persistence::AuthDAO::new() {
            Ok(dao) => dao,
            Err(err) => {
                push_toast(Toast::new(
                    format!("Failed to open auth store: {}", err),
                    ToastLevel::Error,
                    None,
                ));
                return;
            }
        };

        match auth_dao.get_provider(&provider_id) {
            Ok(Some(_)) => {
                if let Err(err) = auth_dao.remove_provider(&provider_id) {
                    push_toast(Toast::new(
                        format!("Failed to disconnect {}: {}", provider_name, err),
                        ToastLevel::Error,
                        None,
                    ));
                    return;
                }

                push_toast(Toast::new(
                    format!("Disconnected {}", provider_name),
                    ToastLevel::Info,
                    None,
                ));

                self.reopen_connect_dialog(Some(&provider_id));
            }
            Ok(None) => {
                push_toast(Toast::new(
                    format!("{} is not connected", provider_name),
                    ToastLevel::Info,
                    None,
                ));
            }
            Err(err) => {
                push_toast(Toast::new(
                    format!("Failed to inspect provider auth: {}", err),
                    ToastLevel::Error,
                    None,
                ));
            }
        }
    }

    fn handle_connect_dialog_selection(
        &mut self,
        selected_item: crate::ui::components::dialog::DialogItem,
    ) {
        match self.connect_dialog_mode {
            ConnectDialogMode::ProviderSelection => {
                if selected_item.id == "openai" {
                    self.show_openai_connect_methods();
                    return;
                }

                self.api_key_input.show(&selected_item.id);
                self.overlay_focus = OverlayFocus::ApiKeyInput;
            }
            ConnectDialogMode::OpenAIMethodSelection => match selected_item.id.as_str() {
                "openai-oauth-browser" => {
                    self.begin_openai_oauth_browser();
                }
                "openai-oauth-headless" => {
                    self.begin_openai_oauth_headless();
                }
                "openai-api-key" => {
                    self.api_key_input.show("openai");
                    self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
                    self.overlay_focus = OverlayFocus::ApiKeyInput;
                }
                _ => {
                    self.overlay_focus = OverlayFocus::None;
                }
            },
        }
    }

    fn begin_openai_oauth_browser(&mut self) {
        if self.openai_oauth_in_progress {
            push_toast(Toast::new(
                "OpenAI OAuth is already in progress",
                ToastLevel::Info,
                None,
            ));
            self.overlay_focus = OverlayFocus::None;
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<OpenAIOAuthTaskMessage>();
        self.openai_oauth_receiver = Some(receiver);
        self.openai_oauth_in_progress = true;
        self.openai_oauth_flow_state.show_browser_waiting();
        self.overlay_focus = OverlayFocus::OpenAIOAuthFlow;
        self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
        self.connect_dialog_state = init_connect_dialog();

        tokio::spawn(async move {
            match crate::auth::openai_oauth::authorize_browser().await {
                Ok(credentials) => {
                    let _ = sender.send(OpenAIOAuthTaskMessage::Success(credentials));
                }
                Err(err) => {
                    let _ = sender.send(OpenAIOAuthTaskMessage::Failed(err.to_string()));
                }
            }
        });
    }

    fn begin_openai_oauth_headless(&mut self) {
        if self.openai_oauth_in_progress {
            push_toast(Toast::new(
                "OpenAI OAuth is already in progress",
                ToastLevel::Info,
                None,
            ));
            self.overlay_focus = OverlayFocus::None;
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<OpenAIOAuthTaskMessage>();
        self.openai_oauth_receiver = Some(receiver);
        self.openai_oauth_in_progress = true;
        self.openai_oauth_flow_state.show_headless_preparing();
        self.overlay_focus = OverlayFocus::OpenAIOAuthFlow;
        self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
        self.connect_dialog_state = init_connect_dialog();

        tokio::spawn(async move {
            let code_sender = sender.clone();
            let result = crate::auth::openai_oauth::authorize_headless(move |code, url| {
                let _ = code_sender.send(OpenAIOAuthTaskMessage::HeadlessCode { code, url });
            })
            .await;

            match result {
                Ok(credentials) => {
                    let _ = sender.send(OpenAIOAuthTaskMessage::Success(credentials));
                }
                Err(err) => {
                    let _ = sender.send(OpenAIOAuthTaskMessage::Failed(err.to_string()));
                }
            }
        });
    }

    fn process_openai_oauth_events(&mut self) {
        let mut events = Vec::new();

        if let Some(receiver) = &mut self.openai_oauth_receiver {
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
        }

        for event in events {
            match event {
                OpenAIOAuthTaskMessage::HeadlessCode { code, url } => {
                    self.openai_oauth_flow_state.set_headless_code(code, url);
                    self.overlay_focus = OverlayFocus::OpenAIOAuthFlow;
                }
                OpenAIOAuthTaskMessage::Success(credentials) => {
                    if let Ok(auth_dao) = crate::persistence::AuthDAO::new() {
                        let _ = auth_dao.set_provider(
                            "openai".to_string(),
                            crate::persistence::AuthConfig::OAuth {
                                refresh: credentials.refresh,
                                access: credentials.access,
                                expires: credentials.expires,
                                account_id: credentials.account_id,
                                enterprise_url: credentials.enterprise_url,
                            },
                        );
                    }

                    if let Some(prefs_dao) = self.prefs_dao.as_ref() {
                        let _ = prefs_dao
                            .set_active_model("openai".to_string(), "gpt-5.3-codex".to_string());
                    }

                    self.provider_name = "openai".to_string();
                    self.model = "gpt-5.3-codex".to_string();
                    self.openai_oauth_in_progress = false;
                    self.openai_oauth_receiver = None;
                    self.openai_oauth_flow_state.hide();
                    if self.overlay_focus == OverlayFocus::OpenAIOAuthFlow {
                        self.overlay_focus = OverlayFocus::None;
                    }

                    push_toast(Toast::new(
                        "Connected OpenAI via ChatGPT Plus/Pro OAuth",
                        ToastLevel::Info,
                        None,
                    ));
                }
                OpenAIOAuthTaskMessage::Failed(error) => {
                    self.openai_oauth_in_progress = false;
                    self.openai_oauth_receiver = None;
                    self.openai_oauth_flow_state.hide();
                    if self.overlay_focus == OverlayFocus::OpenAIOAuthFlow {
                        self.overlay_focus = OverlayFocus::None;
                    }
                    push_toast(Toast::new(
                        format!("OpenAI OAuth failed: {}", error),
                        ToastLevel::Error,
                        None,
                    ));
                }
            }
        }
    }

    fn cleanup_streaming(&mut self) {
        self.chat_state.chat.resume_streaming_tps_timer();
        self.permission_dialog_state.clear_with_deny();
        if self.overlay_focus == OverlayFocus::PermissionDialog {
            self.overlay_focus = OverlayFocus::None;
        }
        self.chunk_sender = None;
        self.chunk_receiver = None;
        self.streaming_cancel_token = None;
    }

    fn cancel_streaming(&mut self) {
        if let Some(token) = &self.streaming_cancel_token {
            token.cancel();
        }
    }

    pub fn update_animations(&mut self) {
        // Only update animations at 20fps (50ms intervals) regardless of render rate
        const ANIMATION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

        if self.last_animation_update.elapsed() >= ANIMATION_INTERVAL {
            self.chat_state.wave_spinner.update();
            self.home_state.tick();
            self.last_animation_update = std::time::Instant::now();
        }
    }

    pub fn is_animation_running(&self) -> bool {
        self.base_focus == BaseFocus::Home || self.is_streaming
    }

    pub fn process_streaming_chunks(&mut self) {
        self.process_openai_oauth_events();

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
                    self.chat_state
                        .chat
                        .append_reasoning_to_last_assistant(&reasoning);
                }
                crate::llm::ChunkMessage::Warning(msg) => {
                    push_toast(Toast::new(msg, ToastLevel::Warning, None));
                }
                crate::llm::ChunkMessage::End => {
                    // Capture end timestamp for TTFT/TPS/latency calculations.
                    self.chat_state.chat.mark_streaming_end();

                    // Finalize streaming metrics from the chat's tracked values
                    self.chat_state.chat.finalize_streaming_metrics();

                    // Persist all new assistant/tool messages for this streaming turn.
                    let start = self.streaming_chat_len_before_assistant;
                    for msg in self.chat_state.chat.messages.iter_mut().skip(start) {
                        match msg.role {
                            crate::session::types::MessageRole::Assistant => {
                                if !msg.is_complete {
                                    msg.mark_complete();
                                }
                                msg.model = self.streaming_model.clone();
                                msg.provider = self.streaming_provider.clone();
                                let _ = self.session_manager.add_message_to_current_session(msg);
                            }
                            crate::session::types::MessageRole::Tool => {
                                let _ = self.session_manager.add_message_to_current_session(msg);
                            }
                            _ => {}
                        }
                    }
                    self.is_streaming = false;
                    self.streaming_model = None;
                    self.streaming_provider = None;
                    self.cleanup_streaming();

                    let completion_stats = self.completion_notification_stats();
                    self.play_sound_event_with_notification_detail(
                        crate::sound::SoundEvent::Complete,
                        completion_stats.as_deref(),
                    );
                }
                crate::llm::ChunkMessage::Failed(error) => {
                    self.is_streaming = false;
                    self.chat_state.chat.mark_streaming_end();
                    self.chat_state.chat.finalize_streaming_metrics();
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        format!("LLM error: {}", error),
                        ToastLevel::Error,
                        None,
                    ));
                    self.chat_state
                        .chat
                        .messages
                        .truncate(self.streaming_chat_len_before_assistant);
                    self.cleanup_streaming();
                }
                crate::llm::ChunkMessage::Cancelled => {
                    self.is_streaming = false;
                    self.chat_state.chat.mark_streaming_end();
                    self.chat_state.chat.finalize_streaming_metrics();
                    push_toast(Toast::new("Streaming cancelled", ToastLevel::Info, None));
                    self.chat_state
                        .chat
                        .messages
                        .truncate(self.streaming_chat_len_before_assistant);
                    self.cleanup_streaming();
                }
                crate::llm::ChunkMessage::Metrics { .. } => {
                    // Metrics are now calculated locally from streaming data
                    // This arm is kept for backward compatibility but ignored
                }
                crate::llm::ChunkMessage::ToolCalls(tool_calls) => {
                    // Seal the current assistant segment so subsequent model text can appear
                    // after tool rows (interleaved timeline).
                    if let Some(idx) = self
                        .chat_state
                        .chat
                        .messages
                        .iter()
                        .rposition(|m| m.role == crate::session::types::MessageRole::Assistant)
                    {
                        if let Some(msg) = self.chat_state.chat.messages.get_mut(idx) {
                            if !msg.is_complete {
                                msg.mark_complete();
                            }
                        }
                    }

                    for call in tool_calls {
                        let args_value: serde_json::Value =
                            serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| {
                                serde_json::Value::String(call.function.arguments.clone())
                            });

                        let content = serde_json::json!({
                            "id": call.id,
                            "name": call.function.name,
                            "status": "running",
                            "args": args_value,
                        })
                        .to_string();

                        self.chat_state
                            .chat
                            .add_message(crate::session::types::Message::tool(content));

                        let idx = self.chat_state.chat.messages.len().saturating_sub(1);
                        self.tool_call_message_indices.insert(call.id.clone(), idx);
                        self.tool_call_order.push(call.id);
                    }
                }
                crate::llm::ChunkMessage::ToolResult(result) => {
                    if let Some(idx) = self
                        .tool_call_message_indices
                        .get(&result.tool_call_id)
                        .copied()
                    {
                        if let Some(msg) = self.chat_state.chat.messages.get_mut(idx) {
                            let mut v: serde_json::Value = serde_json::from_str(&msg.content)
                                .unwrap_or_else(|_| serde_json::json!({}));
                            v["id"] = serde_json::Value::String(result.tool_call_id.clone());
                            v["name"] = serde_json::Value::String(result.name.clone());

                            // Merge structured payloads from the AISDK bridge if present.
                            if let Ok(payload) =
                                serde_json::from_str::<serde_json::Value>(&result.content)
                            {
                                if payload.is_object() {
                                    if v.get("status").is_none() {
                                        v["status"] =
                                            payload.get("status").cloned().unwrap_or_else(|| {
                                                serde_json::Value::String("ok".to_string())
                                            });
                                    } else {
                                        v["status"] = payload
                                            .get("status")
                                            .cloned()
                                            .unwrap_or_else(|| v["status"].clone());
                                    }
                                    if let Some(title) = payload.get("title") {
                                        v["title"] = title.clone();
                                    }
                                    if let Some(meta) = payload.get("metadata") {
                                        v["metadata"] = meta.clone();
                                    }
                                    if let Some(line_count) = payload.get("line_count") {
                                        v["line_count"] = line_count.clone();
                                    }
                                    if let Some(out) = payload.get("output_preview") {
                                        v["output_preview"] = out.clone();
                                    }
                                } else {
                                    v["status"] = serde_json::Value::String("ok".to_string());
                                    v["output_preview"] =
                                        serde_json::Value::String(result.content.clone());
                                }
                            } else {
                                let status = if result.content.trim_start().starts_with("Error:") {
                                    "error"
                                } else {
                                    "ok"
                                };
                                v["status"] = serde_json::Value::String(status.to_string());
                                v["output_preview"] =
                                    serde_json::Value::String(result.content.clone());
                            }

                            msg.content = v.to_string();
                        }
                    } else {
                        let content = serde_json::json!({
                            "id": result.tool_call_id,
                            "name": result.name,
                            "status": "ok",
                            "output_preview": result.content,
                        })
                        .to_string();
                        self.chat_state
                            .chat
                            .add_message(crate::session::types::Message::tool(content));
                    }
                }
                crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                    self.play_sound_event(crate::sound::SoundEvent::Permission);
                    self.chat_state.chat.pause_streaming_tps_timer();
                    self.permission_dialog_state.enqueue(prompt);
                    self.overlay_focus = OverlayFocus::PermissionDialog;
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

        // Track the message boundary for this streaming turn so we can cleanly
        // roll back assistant/tool messages on failure or cancellation.
        self.streaming_chat_len_before_assistant = self.chat_state.chat.messages.len();
        self.tool_call_message_indices.clear();
        self.tool_call_order.clear();

        // Capture the current model and provider at the start of streaming
        // so they don't change if the user switches models during streaming
        self.streaming_model = Some(self.model.clone());
        self.streaming_provider = Some(self.provider_name.clone());
        self.chat_state
            .chat
            .prepare_streaming_token_counter(&self.model);

        self.chat_state.chat.add_assistant_message("");
        if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
            last_msg.is_complete = false;
        }

        // Initialize per-turn streaming timing primitives (T0).
        self.chat_state.chat.begin_streaming_turn();

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let agent_mode = self.agent.clone();
        let provider_timeout = self
            .provider_timeouts
            .get(&self.provider_name.to_ascii_lowercase())
            .copied();
        let agent_max_steps = self
            .agent_steps
            .get(&self.agent.to_ascii_lowercase())
            .copied();
        let tool_permissions = self.tool_permissions.clone();
        let cwd = self.cwd.clone();
        let is_git_repo = crate::utils::git::is_git_repo(&cwd).unwrap_or(false);

        // Build messages with system prompt
        let mut messages = self.chat_state.chat.messages.clone();

        // Check if we already have a system message
        let has_system = messages
            .iter()
            .any(|m| m.role == crate::session::types::MessageRole::System);

        if !has_system {
            // Create system prompt with tools
            let composer = crate::prompt::SystemPromptComposer::new(
                &model,
                &cwd,
                is_git_repo,
                std::env::consts::OS,
            );

            let system_prompt = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async { composer.compose().await })
            });
            let system_msg = crate::session::types::Message::system(system_prompt);
            messages.insert(0, system_msg);
        }

        tokio::spawn(async move {
            let stream = stream_llm_with_cancellation(
                cancel_token,
                provider_name,
                model,
                agent_mode,
                agent_max_steps,
                tool_permissions,
                messages,
                sender_clone.clone(),
            );

            let result: Result<Result<(), Box<dyn std::error::Error>>, u64> = match provider_timeout
            {
                Some(crate::config::ProviderTimeout::Millis(ms)) => {
                    match tokio::time::timeout(std::time::Duration::from_millis(ms), stream).await {
                        Ok(inner) => Ok(inner),
                        Err(_) => Err(ms),
                    }
                }
                Some(crate::config::ProviderTimeout::Disabled) | None => Ok(stream.await),
            };

            let _ = match result {
                Ok(Ok(())) => sender_clone.send(crate::llm::ChunkMessage::End),
                Ok(Err(e)) => sender_clone.send(crate::llm::ChunkMessage::Failed(e.to_string())),
                Err(ms) => sender_clone.send(crate::llm::ChunkMessage::Failed(format!(
                    "Timeout: No response within {} ms",
                    ms
                ))),
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
            self.chat_state
                .chat
                .add_user_message_with_agent_mode(&msg, self.agent.clone());
            self.base_focus = BaseFocus::Chat;

            if let Err(e) = self.start_llm_streaming(&msg) {
                push_toast(Toast::new(
                    format!("LLM error: {}", e),
                    ToastLevel::Error,
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
            self.chat_state
                .chat
                .add_user_message_with_agent_mode(&msg, self.agent.clone());

            if let Err(e) = self.start_llm_streaming(&msg) {
                push_toast(Toast::new(
                    format!("LLM error: {}", e),
                    ToastLevel::Error,
                    None,
                ));
            }
        }
    }

    pub fn render(&mut self, f: &mut ratatui::Frame) {
        let size = f.area();
        self.last_frame_size = size;
        let colors = self.get_current_theme_colors();

        let fingerprint: (usize, usize) = (
            self.chat_state.chat.messages.len(),
            self.chat_state
                .chat
                .messages
                .iter()
                .filter_map(|m| m.token_count)
                .sum(),
        );
        if self.cached_usage_check != fingerprint {
            self.cached_usage_check = fingerprint;
            self.cached_usage_text = self.session_usage_text();
        }
        let usage_text = &self.cached_usage_text;

        match self.base_focus {
            BaseFocus::Home => {
                render_home(
                    f,
                    &mut self.input,
                    &self.home_state,
                    self.version.clone(),
                    self.cwd.clone(),
                    git::get_current_branch(),
                    self.agent.clone(),
                    self.model.clone(),
                    self.provider_name.clone(),
                    &colors,
                    &usage_text,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::ModelsDialog
                    && self.overlay_focus != OverlayFocus::ThemesDialog
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
                    &usage_text,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::ModelsDialog
                    && self.overlay_focus != OverlayFocus::ThemesDialog
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

        if self.overlay_focus == OverlayFocus::ThemesDialog
            && self.themes_dialog_state.dialog.is_visible()
        {
            render_themes_dialog(f, &mut self.themes_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::ConnectDialog
            && self.connect_dialog_state.dialog.is_visible()
        {
            render_connect_dialog(f, &mut self.connect_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::OpenAIOAuthFlow
            && self.openai_oauth_flow_state.is_visible()
        {
            render_openai_oauth_flow(f, &mut self.openai_oauth_flow_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::ApiKeyInput && self.api_key_input.is_visible() {
            self.api_key_input.render(f, size, &colors);
        }

        if self.overlay_focus == OverlayFocus::SessionsDialog
            && self.sessions_dialog_state.dialog.is_visible()
        {
            render_sessions_dialog(f, &mut self.sessions_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::SkillsDialog
            && self.skills_dialog_state.dialog.is_visible()
        {
            crate::views::skills_dialog::render_skills_dialog(
                f,
                &mut self.skills_dialog_state,
                size,
                colors,
            );
        }

        if self.overlay_focus == OverlayFocus::TimelineDialog
            && self.timeline_dialog_state.dialog.is_visible()
        {
            crate::views::timeline_dialog::render_timeline_dialog(
                f,
                &mut self.timeline_dialog_state,
                size,
                colors,
            );
        }

        if self.overlay_focus == OverlayFocus::MessageActions {
            if let Some(ref mut dialog) = self.message_actions_dialog {
                dialog.render(f, size, colors);
            }
        }

        if self.overlay_focus == OverlayFocus::SessionRenameDialog
            && self.session_rename_dialog_state.is_visible()
        {
            render_session_rename_dialog(f, &mut self.session_rename_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::PermissionDialog
            && self.permission_dialog_state.has_active()
        {
            render_permission_dialog(f, &mut self.permission_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::WhichKey {
            crate::views::which_key::render_which_key(f, &self.which_key_state, &colors);
        }

        toast::render_toasts(f, &get_toast_manager().lock().unwrap(), &colors);
    }
}

fn format_token_count(count: usize) -> String {
    if count < 1000 {
        return count.to_string();
    }
    if count < 1_000_000 {
        let k = count as f64 / 1000.0;
        return format!("{:.1}K", k);
    }
    let m = count as f64 / 1_000_000.0;
    format!("{:.1}M", m)
}

impl Default for App {
    fn default() -> Self {
        Self::new().expect("Failed to initialize App")
    }
}
