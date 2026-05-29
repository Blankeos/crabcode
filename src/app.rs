use ratatui::crossterm::event::{
    self, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::autocomplete::AutoComplete;
use crate::command::handlers::register_all_commands;
use crate::command::parser::InputType;
use crate::command::registry::Registry;
use crate::llm::client::stream_llm_with_cancellation;
use crate::session::manager::SessionManager;
use crate::tools::ToolHandler;

use crate::push_toast;
use crate::toast::{self, Toast, ToastLevel};
use crate::ui::components::chat::{Chat, ChatImageTarget};
use crate::ui::components::input::Input;
use crate::ui::components::popup::Popup;
use crate::ui::hyperlink::HyperlinkTarget;
use crate::utils::git;

use crate::views::chat::{
    agent_color_for_tab, init_chat, queued_messages_height, render_chat, SubagentTab, SubagentTabs,
    SUBAGENT_FOOTER_HEIGHT,
};
use crate::views::command_palette::{
    handle_command_palette_key_event, handle_command_palette_mouse_event, init_command_palette,
    render_command_palette, CommandPaletteAction, CommandPaletteAppAction,
};
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
use crate::views::question_dialog::{
    handle_question_dialog_key_event, handle_question_dialog_mouse_event, init_question_dialog,
    render_question_dialog, QuestionDialogAction,
};
use crate::views::session_rename_dialog::{
    handle_session_rename_dialog_key_event, init_session_rename_dialog,
    render_session_rename_dialog, RenameAction,
};
use crate::views::sessions_dialog::{
    handle_sessions_dialog_key_event, handle_sessions_dialog_mouse_event, init_sessions_dialog,
    render_sessions_dialog, SessionsDialogAction, SessionsDialogFilter,
};
use crate::views::storage_dialog::{
    handle_storage_dialog_key_event, handle_storage_dialog_mouse_event, init_storage_dialog,
    render_storage_dialog, StorageDialogAction,
};
use crate::views::suggestions_popup::{
    clear_suggestions, get_selected_suggestion, handle_suggestions_popup_key_event,
    handle_suggestions_popup_mouse_event, init_suggestions_popup, is_suggestions_visible,
    render_suggestions_popup, set_suggestions,
};
use crate::views::themes_dialog::{
    handle_themes_dialog_key_event, handle_themes_dialog_mouse_event, init_themes_dialog,
    render_themes_dialog,
};
use crate::views::{
    ChatState, ConnectDialogState, HomeState, ModelsDialogState, OpenAIOAuthFlowState,
    PermissionDialogState, QuestionDialogState, SessionRenameDialogState, SessionsDialogState,
    StorageDialogState, SuggestionsPopupState, ThemesDialogState,
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
    QuestionDialog,
    SkillsDialog,
    TimelineDialog,
    MessageActions,
    CommandPalette,
    StorageDialog,
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

#[derive(Debug)]
enum CompactionTaskMessage {
    Success {
        session_id: String,
        messages: Vec<crate::session::types::Message>,
        stats: crate::session::types::CompactionStats,
    },
    Failed {
        session_id: String,
        error: String,
    },
}

#[derive(Debug)]
enum StorageTaskMessage {
    Loaded(crate::utils::storage::StorageReport),
}

#[derive(Debug, Clone)]
struct CompactionPending {
    session_id: String,
    before_tokens: usize,
}

#[derive(Debug)]
struct SessionStreamState {
    chunk_receiver: crate::llm::ChunkReceiver,
    cancel_token: tokio_util::sync::CancellationToken,
    streaming_model: Option<String>,
    streaming_provider: Option<String>,
    chat_len_before_assistant: usize,
}

#[derive(Debug, Clone)]
struct ExternalStreamState {
    streaming_model: Option<String>,
    streaming_provider: Option<String>,
    chat_len_before_assistant: usize,
}

#[derive(Debug, Default)]
struct ToolCallViewState {
    tool_call_message_indices: std::collections::HashMap<String, usize>,
    tool_call_order: Vec<String>,
    deferred_finish: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionActionTarget {
    Chat,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionActionBarState {
    target: SelectionActionTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionAction {
    AddToPrompt,
    Copy,
    Dismiss,
}

const TERMINAL_TITLE_SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TERMINAL_TITLE_SPINNER_INTERVAL_MS: u128 = 100;

#[derive(Debug)]
struct ClientSessionState {
    chat: Chat,
    input_draft: String,
    stream: Option<SessionStreamState>,
    external_stream: Option<ExternalStreamState>,
    tool_calls: ToolCallViewState,
    queued_messages: std::collections::VecDeque<QueuedUserMessage>,
    unread_completed: bool,
}

#[derive(Debug, Clone)]
struct QueuedUserMessage {
    text: String,
    image_paths: Vec<std::path::PathBuf>,
}

impl ClientSessionState {
    fn with_messages(messages: Vec<crate::session::types::Message>) -> Self {
        Self {
            chat: Chat::with_messages(messages),
            input_draft: String::new(),
            stream: None,
            external_stream: None,
            tool_calls: ToolCallViewState::default(),
            queued_messages: std::collections::VecDeque::new(),
            unread_completed: false,
        }
    }
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
    pub question_dialog_state: QuestionDialogState,
    pub skills_dialog_state: crate::views::SkillsDialogState,
    pub command_palette_state: crate::views::command_palette::CommandPaletteState,
    pub storage_dialog_state: StorageDialogState,
    pub which_key_state: crate::views::which_key::WhichKeyState,
    pub timeline_dialog_state: crate::views::timeline_dialog::TimelineDialogState,
    esc_timeline_primed: bool,
    pub message_actions_index: Option<usize>,
    pub message_actions_dialog: Option<crate::ui::components::dialog::Dialog>,
    message_actions_return_focus: OverlayFocus,
    selection_action_bar: Option<SelectionActionBarState>,
    pending_chat_message_click: Option<usize>,
    pub api_key_input: crate::ui::components::api_key_input::ApiKeyInput,
    openai_oauth_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<OpenAIOAuthTaskMessage>>,
    openai_oauth_in_progress: bool,
    compaction_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<CompactionTaskMessage>>,
    compaction_pending: Option<CompactionPending>,
    storage_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<StorageTaskMessage>>,
    pub prefs_dao: Option<crate::persistence::PrefsDAO>,
    pub agent: String,
    pub agent_registry: crate::agent::definition::AgentRegistry,
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
    pub notifications: crate::config::NotificationsConfig,
    pub images: crate::config::ImagesConfig,
    terminal_focused: bool,
    pub tool_permissions: crate::tools::ToolPermissions,
    pub skills_dirs: Vec<std::path::PathBuf>,
    pub is_streaming: bool,
    pending_session_title: Option<String>,
    session_view_states: std::collections::HashMap<String, ClientSessionState>,
    session_spinner_frame: usize,
    last_frame_size: ratatui::layout::Rect,
    last_animation_update: std::time::Instant,
    last_session_spinner_update: std::time::Instant,
    cached_git_branch: Option<String>,
    cached_git_branch_path: String,
    last_git_branch_check: std::time::Instant,
    discovery: Option<crate::model::discovery::Discovery>,
    cached_usage_text: String,
    cached_usage_check: (usize, u64),
    terminal_title_enabled: bool,
    terminal_title_last: Option<String>,
    terminal_title_animation_origin: std::time::Instant,
}

impl App {
    pub fn new() -> Result<Self> {
        Self::new_with_model_override(None)
    }

    pub fn new_with_model_override(model_override: Option<&str>) -> Result<Self> {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let placeholder = Self::get_random_placeholder();
        let placeholder_static: &'static str = Box::leak(placeholder.into_boxed_str());
        let mut input = Input::new();
        input.set_placeholder(placeholder_static);

        let cwd_path = crate::utils::cwd::current_dir()?;
        let cwd = cwd_path
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "?".to_string());

        let home_state = init_home();
        let mut agent = "Build".to_string();
        let chat = Chat::new();
        let suggestions_popup_state = init_suggestions_popup(Popup::new());
        let models_dialog_state = init_models_dialog("Models", vec![]);
        let themes_dialog_state = init_themes_dialog("Themes", vec![]);
        let connect_dialog_state = init_connect_dialog();
        let openai_oauth_flow_state = init_openai_oauth_flow();
        let sessions_dialog_state = init_sessions_dialog("Sessions", vec![]);
        let permission_dialog_state = init_permission_dialog();
        let question_dialog_state = init_question_dialog();
        let skills_dialog_state = crate::views::skills_dialog::init_skills_dialog("Skills", vec![]);
        let which_key_state = crate::views::which_key::init_which_key();
        let timeline_dialog_state = crate::views::timeline_dialog::init_timeline_dialog();
        let command_palette_state = init_command_palette();
        let storage_dialog_state = init_storage_dialog();
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
        input.set_image_open_config(loaded_config.merged_config.images.clone());
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
        for command in loaded_config.merged_config.commands.clone() {
            registry.register_custom(command);
        }
        crate::command::handlers::register_skill_commands(&mut registry);
        let agent_registry = loaded_config.merged_config.agent_registry.clone();
        let agent_suggestions = agent_registry
            .visible_subagents()
            .into_iter()
            .map(|agent| {
                crate::autocomplete::Suggestion::agent(
                    agent.name.clone(),
                    agent.description.clone(),
                )
            })
            .collect();
        input.autocomplete = Some(
            AutoComplete::new(crate::autocomplete::CommandAuto::new(&registry))
                .with_agents(agent_suggestions),
        );

        if let Some(default_agent) = loaded_config.merged_config.default_agent.clone() {
            if !default_agent.trim().is_empty() {
                agent = default_agent;
            }
        }

        let (resolved_sounds, notification_warnings) =
            crate::sound::resolve_effective_sounds(&loaded_config.merged_config.notifications);
        if !notification_warnings.is_empty() {
            for msg in &notification_warnings {
                crate::startup_diag!("Notification warning: {}", msg);
            }
        }

        let model_override = model_override.map(parse_model_ref);
        let active_model_info = if model_override.is_none() {
            prefs_dao
                .as_ref()
                .and_then(|dao| dao.get_active_model().ok().flatten())
        } else {
            None
        };

        if model_override.is_none() && active_model_info.is_none() {
            if let (Some(ref dao), Some(model_str)) = (
                prefs_dao.as_ref(),
                loaded_config.merged_config.model.clone(),
            ) {
                let (provider_id, model_id) = parse_model_ref(&model_str);
                let _ = dao.set_active_model(provider_id, model_id);
            }
        }

        let active_model_info = if model_override.is_none() {
            prefs_dao
                .as_ref()
                .and_then(|dao| dao.get_active_model().ok().flatten())
        } else {
            None
        };

        let (active_model, active_provider_name) =
            if let Some((provider_id, model_id)) = model_override {
                (model_id, provider_id)
            } else if let Some((provider_id, model_id)) = active_model_info {
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
        let agent_steps = agent_registry.max_steps_map();
        let provider_timeouts = loaded_config.merged_config.provider_timeouts.clone();

        let theme_for_colors = themes
            .get(current_theme_index)
            .or_else(|| themes.first())
            .cloned()
            .unwrap_or_else(theme::Theme::load_builtin_default);
        let colors = theme_for_colors.get_colors(true);

        let chat_state = init_chat(chat, &agent, &colors);
        let session_rename_dialog_state = init_session_rename_dialog(colors);
        let mut agent_policies = crate::tools::AgentToolPolicies::default();
        for (mode, tools) in agent_registry.tool_policy_map() {
            agent_policies = agent_policies.with_custom_tools(mode.clone(), tools.clone());
        }
        let tool_permissions = crate::tools::ToolPermissions::new(cwd_path.clone())
            .with_agent_policies(agent_policies)
            .with_permission_rules(loaded_config.merged_config.permission_rules.clone())
            .with_agent_permission_rules(agent_registry.permission_rules_map());

        let discovery = crate::model::discovery::Discovery::new().ok();
        let cached_git_branch = git::get_branch_for_path(&cwd);
        let now = std::time::Instant::now();

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
            question_dialog_state,
            skills_dialog_state,
            command_palette_state,
            storage_dialog_state,
            which_key_state,
            timeline_dialog_state,
            esc_timeline_primed: false,
            message_actions_index: None,
            message_actions_dialog: None,
            message_actions_return_focus: OverlayFocus::TimelineDialog,
            selection_action_bar: None,
            pending_chat_message_click: None,
            api_key_input,
            openai_oauth_receiver: None,
            openai_oauth_in_progress: false,
            compaction_receiver: None,
            compaction_pending: None,
            storage_receiver: None,
            prefs_dao,
            agent,
            agent_registry,
            agent_steps,
            provider_timeouts,
            model: active_model,
            provider_name: active_provider_name,
            cwd: cwd.clone(),
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
            themes,
            current_theme_index,
            dark_mode: true,
            sounds: resolved_sounds,
            notifications: loaded_config.merged_config.notifications,
            images: loaded_config.merged_config.images,
            terminal_focused: true,
            tool_permissions,
            skills_dirs: loaded_config.inventory.opencode_skills_dirs,
            // Note: skills_dirs is legacy; skill loading is now handled by src/skill/mod.rs
            is_streaming: false,
            pending_session_title: None,
            session_view_states: std::collections::HashMap::new(),
            session_spinner_frame: 0,
            last_frame_size: ratatui::layout::Rect::default(),
            last_animation_update: now,
            last_session_spinner_update: now,
            cached_git_branch,
            cached_git_branch_path: cwd.clone(),
            last_git_branch_check: now,
            discovery,
            cached_usage_text: String::new(),
            cached_usage_check: (0, 0),
            terminal_title_enabled: crate::notify::terminal_title_supported(),
            terminal_title_last: None,
            terminal_title_animation_origin: now,
        })
    }

    fn play_sound_event(&self, event: crate::sound::SoundEvent) {
        self.play_sound_event_with_notification_detail(event, None);
    }

    pub fn set_terminal_focused(&mut self, focused: bool) {
        self.terminal_focused = focused;
    }

    fn play_sound_event_with_notification_detail(
        &self,
        event: crate::sound::SoundEvent,
        detail: Option<&str>,
    ) {
        if let Some(path) = self.sounds.path_for_event(event) {
            crate::sound::play_file(path);
        }

        if self.notifications.desktop_for_event(event) {
            crate::notify::notify_event(event, detail);
        }
    }

    fn notify_terminal_event(&self, event: crate::sound::SoundEvent) {
        use crate::config::{TerminalNotificationCondition, TerminalNotificationMode};

        if self.notifications.terminal_condition == TerminalNotificationCondition::Unfocused
            && self.terminal_focused
        {
            return;
        }

        let mode = match event {
            crate::sound::SoundEvent::Complete => self.notifications.complete.terminal,
            crate::sound::SoundEvent::Permission => self.notifications.permission.terminal,
            crate::sound::SoundEvent::Question => self.notifications.question.terminal,
            crate::sound::SoundEvent::Error => self.notifications.error.terminal,
        };

        let should_emit = match mode {
            TerminalNotificationMode::Auto => crate::notify::terminal_bell_supported(),
            TerminalNotificationMode::Enabled => true,
            TerminalNotificationMode::Disabled => false,
        };

        if should_emit {
            crate::notify::notify_terminal_bell();
        }
    }

    pub fn update_terminal_title_signal(&mut self) {
        if !self.terminal_title_enabled {
            return;
        }

        let title = self.terminal_title_text();
        if self.terminal_title_last.as_deref() == Some(title.as_str()) {
            return;
        }

        if crate::notify::set_terminal_title(&title).is_ok() {
            self.terminal_title_last = Some(title);
        }
    }

    pub fn clear_terminal_title_signal(&mut self) {
        if self.terminal_title_last.take().is_some() {
            let _ = crate::notify::clear_terminal_title();
        }
    }

    fn terminal_title_text(&self) -> String {
        let project = self.terminal_title_project_name();

        if self.terminal_title_requires_action() {
            return format!("[!] {}", project);
        }

        if self.terminal_title_has_active_progress() {
            return format!("{} {}", self.terminal_title_spinner_frame(), project);
        }

        project
    }

    fn terminal_title_project_name(&self) -> String {
        let workspace = self.active_workspace_path();
        let name = std::path::Path::new(&workspace)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::trim)
            .filter(|name| !name.is_empty() && *name != "." && *name != "..")
            .unwrap_or("crabcode");

        Self::truncate_terminal_title_part(name, 48)
    }

    fn terminal_title_requires_action(&self) -> bool {
        if matches!(
            self.overlay_focus,
            OverlayFocus::PermissionDialog | OverlayFocus::QuestionDialog
        ) {
            return true;
        }

        self.session_manager
            .get_current_session_id()
            .and_then(|id| self.session_manager.get_session_ref(id))
            .is_some_and(|session| session.status == crate::session::types::SessionStatus::Waiting)
    }

    fn terminal_title_has_active_progress(&self) -> bool {
        self.compaction_receiver.is_some()
            || self
                .session_view_states
                .values()
                .any(|state| state.stream.is_some() || state.external_stream.is_some())
    }

    fn terminal_title_spinner_frame(&self) -> &'static str {
        let frame_index = self.terminal_title_animation_origin.elapsed().as_millis()
            / TERMINAL_TITLE_SPINNER_INTERVAL_MS;
        TERMINAL_TITLE_SPINNER_FRAMES[frame_index as usize % TERMINAL_TITLE_SPINNER_FRAMES.len()]
    }

    fn truncate_terminal_title_part(value: &str, max_chars: usize) -> String {
        if max_chars == 0 {
            return String::new();
        }

        let head = value.chars().take(max_chars).collect::<String>();
        if value.chars().count() <= max_chars || max_chars <= 3 {
            return head;
        }

        let mut truncated = head.chars().take(max_chars - 3).collect::<String>();
        truncated.push_str("...");
        truncated
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

    fn completion_notification_stats_for_chat(chat: &Chat) -> Option<String> {
        let message = chat.messages.iter().rev().find(|msg| {
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

    fn is_active_session(&self, session_id: &str) -> bool {
        self.session_manager
            .get_current_session_id()
            .is_some_and(|current| current == session_id)
    }

    fn ensure_session_view_state(&mut self, session_id: &str) {
        if self.session_view_states.contains_key(session_id) {
            return;
        }

        let messages = self
            .session_manager
            .get_session(session_id)
            .map(|session| session.messages.clone())
            .unwrap_or_default();

        self.session_view_states.insert(
            session_id.to_string(),
            ClientSessionState::with_messages(messages),
        );
    }

    fn save_active_session_view_state(&mut self) {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            return;
        };
        let is_child_session = self.session_manager.parent_id_of(&session_id).is_some();

        self.ensure_session_view_state(&session_id);

        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            state.chat = std::mem::take(&mut self.chat_state.chat);
            state.input_draft = if is_child_session {
                String::new()
            } else {
                self.input.submission_text()
            };
        }
    }

    fn load_session_view_state(&mut self, session_id: &str) {
        self.ensure_session_view_state(session_id);
        let is_child_session = self.session_manager.parent_id_of(session_id).is_some();

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            self.chat_state.chat = std::mem::take(&mut state.chat);
            self.chat_state.chat.scroll_to_bottom_on_next_render();
            if is_child_session {
                self.input.clear();
                state.input_draft.clear();
            } else {
                self.input.set_text(&state.input_draft);
            }
            state.unread_completed = false;
        } else {
            self.chat_state.chat.clear();
            self.input.clear();
        }

        self.sync_active_streaming_flag();
        self.cached_usage_check = (usize::MAX, u64::MAX);
    }

    fn switch_to_session(&mut self, session_id: &str) -> bool {
        if self.session_manager.get_session_ref(session_id).is_none() {
            return false;
        }
        self.save_active_session_view_state();
        self.session_manager.switch_session(session_id);
        self.pending_session_title = None;
        self.load_session_view_state(session_id);
        let is_child_session = self.session_manager.parent_id_of(session_id).is_some();
        self.base_focus = if !is_child_session
            && self.chat_state.chat.messages.is_empty()
            && !self.is_streaming
        {
            BaseFocus::Home
        } else {
            BaseFocus::Chat
        };
        if !is_child_session
            && self.has_queued_messages_for_session(session_id)
            && !self.session_has_active_stream(session_id)
        {
            self.submit_queued_messages_for_session(session_id);
        }
        true
    }

    fn is_subagent_session_active(&self) -> bool {
        self.session_manager
            .get_current_session_id()
            .is_some_and(|id| self.session_manager.parent_id_of(id).is_some())
    }

    fn should_handle_child_session_arrow(&self) -> bool {
        if self.base_focus != BaseFocus::Chat {
            return false;
        }

        self.session_manager
            .get_current_session_id()
            .is_some_and(|id| self.session_manager.parent_id_of(id).is_some())
    }

    fn switch_to_first_child_session(&mut self) -> bool {
        let Some(current_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        let Some(root_id) = self.session_manager.root_session_id_for(&current_id) else {
            return false;
        };
        let Some(first_child) = self
            .session_manager
            .child_sessions(&root_id)
            .first()
            .cloned()
        else {
            return false;
        };

        self.switch_to_session(&first_child.id)
    }

    fn switch_to_parent_session(&mut self) -> bool {
        let Some(current_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        let Some(parent_id) = self
            .session_manager
            .parent_id_of(&current_id)
            .map(str::to_string)
        else {
            return false;
        };

        self.switch_to_session(&parent_id)
    }

    fn switch_child_session(&mut self, direction: isize) -> bool {
        let Some(current_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        let Some(root_id) = self.session_manager.root_session_id_for(&current_id) else {
            return false;
        };

        let children = self.session_manager.child_sessions(&root_id);
        if children.len() <= 1 {
            return false;
        }

        let Some(current_idx) = children.iter().position(|child| child.id == current_id) else {
            return false;
        };

        let len = children.len() as isize;
        let next_idx = (current_idx as isize + direction).rem_euclid(len) as usize;
        self.switch_to_session(&children[next_idx].id)
    }

    fn subagent_tabs_for_current_session(&self) -> Option<SubagentTabs> {
        let current_id = self.session_manager.get_current_session_id()?.clone();
        let root_id = self.session_manager.root_session_id_for(&current_id)?;
        let root = self.session_manager.get_session_ref(&root_id)?;
        let children = self.session_manager.child_sessions(&root_id);
        if children.is_empty() {
            return None;
        }

        let mut tabs = Vec::with_capacity(children.len() + 1);
        let root_agent = self.agent.clone();
        let root_model = self
            .session_active_stream_model(&root_id)
            .unwrap_or_else(|| self.model.clone());
        tabs.push(SubagentTab {
            label: "main".to_string(),
            agent: root_agent,
            model: root_model,
            active: current_id == root_id,
            running: root.status.is_active()
                || self
                    .session_view_states
                    .get(&root_id)
                    .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some()),
            color: crate::theme::agent_color(&self.agent, &self.get_current_theme_colors()),
        });

        let colors = self.get_current_theme_colors();
        for (idx, child) in children.into_iter().enumerate() {
            let label = subagent_tab_label(&child.title, &child.id);
            let (agent, model) =
                self.session_agent_model_for_display(&child.id, "Subagent", &self.model);
            let running = child.status.is_active()
                || self
                    .session_view_states
                    .get(&child.id)
                    .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some());
            tabs.push(SubagentTab {
                label,
                agent,
                model,
                active: current_id == child.id,
                running,
                color: agent_color_for_tab(idx, &colors),
            });
        }

        Some(SubagentTabs {
            is_child_session: current_id != root_id,
            tabs,
        })
    }

    fn current_session_agent_model_for_display(&self) -> (String, String) {
        let Some(session_id) = self.session_manager.get_current_session_id() else {
            return (self.agent.clone(), self.model.clone());
        };
        if self.session_manager.parent_id_of(session_id).is_none() {
            return (
                self.agent.clone(),
                self.session_active_stream_model(session_id)
                    .unwrap_or_else(|| self.model.clone()),
            );
        }
        self.session_agent_model_for_display(session_id, &self.agent, &self.model)
    }

    fn session_agent_model_for_display(
        &self,
        session_id: &str,
        fallback_agent: &str,
        fallback_model: &str,
    ) -> (String, String) {
        let agent = self
            .session_view_states
            .get(session_id)
            .and_then(|state| first_agent_mode(&state.chat.messages))
            .or_else(|| {
                self.session_manager
                    .get_session_ref(session_id)
                    .and_then(|session| first_agent_mode(&session.messages))
            })
            .unwrap_or_else(|| fallback_agent.to_string());

        let model = self.session_model_for_display(session_id, fallback_model);

        (agent, model)
    }

    fn session_model_for_display(&self, session_id: &str, fallback_model: &str) -> String {
        self.session_active_stream_model(session_id)
            .or_else(|| {
                self.session_view_states
                    .get(session_id)
                    .and_then(|state| latest_message_model(&state.chat.messages))
            })
            .or_else(|| {
                self.session_manager
                    .get_session_ref(session_id)
                    .and_then(|session| latest_message_model(&session.messages))
            })
            .unwrap_or_else(|| fallback_model.to_string())
    }

    fn session_active_stream_model(&self, session_id: &str) -> Option<String> {
        self.session_view_states.get(session_id).and_then(|state| {
            state
                .stream
                .as_ref()
                .and_then(|stream| stream.streaming_model.clone())
                .or_else(|| {
                    state
                        .external_stream
                        .as_ref()
                        .and_then(|stream| stream.streaming_model.clone())
                })
        })
    }

    fn start_blank_session(&mut self, title: Option<String>) {
        self.save_active_session_view_state();
        self.pending_session_title = title.and_then(|title| {
            let title = title.trim().to_string();
            if title.is_empty() {
                None
            } else {
                Some(title)
            }
        });
        self.session_manager.clear_current_session();
        self.chat_state.chat.clear();
        self.input.clear();
        self.base_focus = BaseFocus::Home;
        self.sync_active_streaming_flag();
        self.cached_usage_check = (usize::MAX, u64::MAX);
        self.refresh_sessions_dialog();
    }

    fn create_new_session(&mut self, title: Option<String>) -> String {
        self.save_active_session_view_state();
        self.pending_session_title = None;
        let session_id = self.session_manager.create_session(title);
        self.session_view_states.insert(
            session_id.clone(),
            ClientSessionState::with_messages(Vec::new()),
        );
        self.chat_state.chat.clear();
        self.input.clear();
        self.base_focus = BaseFocus::Home;
        self.sync_active_streaming_flag();
        self.cached_usage_check = (usize::MAX, u64::MAX);
        self.refresh_sessions_dialog();
        session_id
    }

    fn chat_for_session_mut(&mut self, session_id: &str) -> Option<&mut Chat> {
        if self.is_active_session(session_id) {
            Some(&mut self.chat_state.chat)
        } else {
            self.ensure_session_view_state(session_id);
            self.session_view_states
                .get_mut(session_id)
                .map(|state| &mut state.chat)
        }
    }

    fn chat_for_session(&self, session_id: &str) -> Option<&Chat> {
        if self.is_active_session(session_id) {
            Some(&self.chat_state.chat)
        } else {
            self.session_view_states
                .get(session_id)
                .map(|state| &state.chat)
        }
    }

    fn stream_for_session_mut(&mut self, session_id: &str) -> Option<&mut SessionStreamState> {
        self.session_view_states
            .get_mut(session_id)
            .and_then(|state| state.stream.as_mut())
    }

    fn session_has_active_stream(&self, session_id: &str) -> bool {
        self.session_view_states
            .get(session_id)
            .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some())
    }

    fn queued_message_previews_for_current_session(&self) -> Vec<String> {
        let Some(session_id) = self.session_manager.get_current_session_id() else {
            return Vec::new();
        };

        self.session_view_states
            .get(session_id)
            .map(|state| {
                state
                    .queued_messages
                    .iter()
                    .map(Self::queued_message_preview)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn queued_message_preview(message: &QueuedUserMessage) -> String {
        if !message.text.trim().is_empty() {
            return message.text.replace('\n', " ");
        }

        match message.image_paths.len() {
            0 => String::new(),
            1 => "[Image]".to_string(),
            count => format!("[{} images]", count),
        }
    }

    fn has_queued_messages_for_session(&self, session_id: &str) -> bool {
        self.session_view_states
            .get(session_id)
            .is_some_and(|state| !state.queued_messages.is_empty())
    }

    fn queue_message_for_current_session(
        &mut self,
        text: String,
        image_paths: Vec<std::path::PathBuf>,
    ) -> bool {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };

        self.ensure_session_view_state(&session_id);
        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            state
                .queued_messages
                .push_back(QueuedUserMessage { text, image_paths });
            return true;
        }

        false
    }

    fn drain_queued_messages_for_session(&mut self, session_id: &str) -> Vec<QueuedUserMessage> {
        self.session_view_states
            .get_mut(session_id)
            .map(|state| state.queued_messages.drain(..).collect())
            .unwrap_or_default()
    }

    fn streaming_boundary_for_session(
        &self,
        session_id: &str,
    ) -> Option<(usize, Option<String>, Option<String>)> {
        let state = self.session_view_states.get(session_id)?;
        if let Some(stream) = state.stream.as_ref() {
            return Some((
                stream.chat_len_before_assistant,
                stream.streaming_model.clone(),
                stream.streaming_provider.clone(),
            ));
        }

        state.external_stream.as_ref().map(|stream| {
            (
                stream.chat_len_before_assistant,
                stream.streaming_model.clone(),
                stream.streaming_provider.clone(),
            )
        })
    }

    fn sync_active_streaming_flag(&mut self) {
        self.is_streaming = self.compaction_receiver.is_some()
            || self
                .session_manager
                .get_current_session_id()
                .and_then(|id| self.session_view_states.get(id))
                .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some());
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
        let messages = &self.chat_state.chat.messages;
        let total_tokens = crate::session::compaction::total_context_tokens(messages);

        let mut text = if total_tokens == 0 {
            String::new()
        } else {
            crate::session::compaction::format_token_count(total_tokens)
        };

        if total_tokens > 0 {
            if let Some(ref discovery) = self.discovery {
                if let Some(limit) =
                    discovery.get_model_limit(&self.provider_name.to_lowercase(), &self.model)
                {
                    if limit > 0 {
                        let pct = ((total_tokens as f64 / limit as f64) * 100.0).round() as u32;
                        text = format!("{} ({}%)", text, pct);
                    }
                }

                if let Some(cost) =
                    discovery.get_model_pricing(&self.provider_name.to_lowercase(), &self.model)
                {
                    let output_tokens: usize =
                        messages.iter().filter_map(|m| m.output_tokens).sum();
                    let total = (output_tokens.max(total_tokens)) as f64;
                    let price = total / 1_000_000.0 * cost.output;
                    if price > 0.001 {
                        text = format!("{} \u{00b7} ${:.2}", text, price);
                    }
                }
            }
        }

        if let Some(pending) = self.compaction_pending.as_ref().filter(|pending| {
            self.session_manager
                .get_current_session_id()
                .is_some_and(|id| id == &pending.session_id)
        }) {
            let suffix = format!(
                "compacting {}",
                crate::session::compaction::format_token_count(pending.before_tokens)
            );
            return append_usage_suffix(text, suffix);
        }

        if let Some(stats) = crate::session::compaction::latest_compaction_stats(messages) {
            let suffix = format!("last compact {}%", stats.reduction_percent());
            return append_usage_suffix(text, suffix);
        }

        text
    }

    fn reasoning_capability_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningCapability> {
        self.discovery
            .as_ref()
            .and_then(|discovery| discovery.get_model_reasoning_capability(provider_id, model_id))
    }

    fn saved_reasoning_effort_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningEffort> {
        self.prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_reasoning_effort(provider_id, model_id).ok())
            .flatten()
    }

    fn resolved_reasoning_effort_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningEffort> {
        let capability = self.reasoning_capability_for_model(provider_id, model_id)?;
        let saved = self.saved_reasoning_effort_for_model(provider_id, model_id)?;
        let resolved = capability.resolve(Some(saved))?;
        if resolved == crate::model::reasoning::ReasoningEffort::None {
            return None;
        }
        if saved != resolved {
            if let Some(ref dao) = self.prefs_dao {
                let _ = dao.set_model_reasoning_effort(
                    provider_id.to_string(),
                    model_id.to_string(),
                    resolved,
                );
            }
        }
        Some(resolved)
    }

    fn reasoning_control_label_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<String> {
        let capability = self.reasoning_capability_for_model(provider_id, model_id)?;
        if capability.values().is_empty() {
            return None;
        }

        Some(
            self.resolved_reasoning_effort_for_model(provider_id, model_id)
                .map(|effort| effort.as_str().to_string())
                .unwrap_or_else(|| "off".to_string()),
        )
    }

    fn selected_model_reasoning_control_label(&self) -> Option<String> {
        let selected = self.models_dialog_state.dialog.get_selected()?;
        self.reasoning_control_label_for_model(&selected.provider_id, &selected.id)
    }

    fn active_reasoning_effort(&self) -> Option<crate::model::reasoning::ReasoningEffort> {
        self.resolved_reasoning_effort_for_model(&self.provider_name, &self.model)
    }

    fn active_reasoning_effort_label(&self) -> Option<String> {
        self.active_reasoning_effort()
            .map(|effort| effort.as_str().to_string())
    }

    fn cycle_reasoning_effort_for_model(
        &mut self,
        provider_id: String,
        model_id: String,
        direction: i8,
    ) -> bool {
        let Some(capability) = self.reasoning_capability_for_model(&provider_id, &model_id) else {
            return false;
        };
        let saved = self.saved_reasoning_effort_for_model(&provider_id, &model_id);
        let Some(next) = capability.cycle_override(saved, direction) else {
            return false;
        };

        if let Some(ref dao) = self.prefs_dao {
            let result = if let Some(next) = next {
                dao.set_model_reasoning_effort(provider_id, model_id, next)
            } else {
                dao.clear_model_reasoning_effort(&provider_id, &model_id)
            };

            if result.is_err() {
                return false;
            }
        }

        true
    }

    fn cycle_active_reasoning_effort(&mut self) -> bool {
        self.cycle_reasoning_effort_for_model(self.provider_name.clone(), self.model.clone(), 1)
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
                diff_add: ratatui::style::Color::Rgb(0, 255, 0),
                diff_add_bg: ratatui::style::Color::Rgb(0, 60, 0),
                diff_remove: ratatui::style::Color::Rgb(255, 0, 0),
                diff_remove_bg: ratatui::style::Color::Rgb(60, 0, 0),
                diff_gutter: ratatui::style::Color::Rgb(140, 140, 140),
            };
        }

        let theme = &self.themes[self.current_theme_index];
        theme.get_colors(self.dark_mode)
    }

    fn active_workspace_path(&self) -> String {
        self.session_manager
            .get_current_session_id()
            .and_then(|id| self.session_manager.get_session_ref(id))
            .map(|session| session.workspace_path.trim())
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.cwd.clone())
    }

    fn current_selection_action_bar_area(&self) -> Option<Rect> {
        self.selection_action_bar.map(|state| match state.target {
            SelectionActionTarget::Chat => chat_selection_action_bar_area(
                self.current_chat_area(),
                self.chat_state.chat.scroll_offset,
                &self.chat_state.chat.selection,
            ),
            SelectionActionTarget::Input => input_selection_action_bar_area(
                self.last_frame_size,
                self.suggestions_popup_anchor_area(),
            ),
        })
    }

    fn current_git_branch(&mut self, cwd: &str) -> Option<String> {
        const GIT_BRANCH_REFRESH: std::time::Duration = std::time::Duration::from_secs(2);

        if self.cached_git_branch_path != cwd
            || self.last_git_branch_check.elapsed() >= GIT_BRANCH_REFRESH
        {
            self.cached_git_branch = git::get_branch_for_path(cwd);
            self.cached_git_branch_path = cwd.to_string();
            self.last_git_branch_check = std::time::Instant::now();
        }

        self.cached_git_branch.clone()
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
        if self.chat_state.chat.has_selection() {
            let _ = self.copy_chat_selection();
            self.chat_state.chat.selection.clear();
            self.selection_action_bar = None;
            return true;
        }

        if self.input.has_selection() {
            let _ = self.copy_input_selection();
            self.input.clear_selection();
            self.selection_action_bar = None;
            return true;
        }

        false
    }

    fn clear_selection(&mut self) -> bool {
        self.selection_action_bar = None;
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

    fn copy_input_selection(&mut self) -> bool {
        if !self.input.has_selection() {
            return false;
        }

        let text = self.input.get_selected_text();
        if text.is_empty() {
            return false;
        }

        let _ = crate::utils::clipboard::copy_text(&text);
        push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
        true
    }

    fn selected_chat_text(&self) -> Option<String> {
        if !self.chat_state.chat.has_selection() {
            return None;
        }

        let ((s_line, s_col), (e_line, e_col)) = self.chat_state.chat.selection.range();
        if s_line == e_line && s_col == e_col {
            return None;
        }

        let colors = self.get_current_theme_colors();
        let model = self.model.clone();
        let chat_area = self.current_chat_area();
        let max_width = chat_area.width.saturating_sub(2) as usize;
        self.chat_state
            .chat
            .get_selected_text(max_width.max(1), &model, &colors)
            .filter(|text| !text.trim().is_empty())
    }

    fn selected_text_for_action(&self, target: SelectionActionTarget) -> Option<String> {
        match target {
            SelectionActionTarget::Chat => self.selected_chat_text(),
            SelectionActionTarget::Input => self
                .input
                .has_selection()
                .then(|| self.input.get_selected_text())
                .filter(|text| !text.is_empty()),
        }
    }

    fn show_selection_action_bar_for(&mut self, target: SelectionActionTarget) {
        self.selection_action_bar = self
            .selected_text_for_action(target)
            .map(|_| SelectionActionBarState { target });
    }

    fn dismiss_selection_actions(&mut self) -> bool {
        let had_selection = self.clear_selection();
        self.pending_chat_message_click = None;
        had_selection
    }

    fn add_selection_to_prompt(&mut self, target: SelectionActionTarget) -> bool {
        if target != SelectionActionTarget::Chat {
            return false;
        }

        let Some(text) = self.selected_text_for_action(target) else {
            return self.dismiss_selection_actions();
        };

        if !self.input.is_empty() {
            self.input.insert_str("\n");
        }
        self.input
            .insert_str(&format_selection_prompt_addition(&text));
        self.dismiss_selection_actions();
        push_toast(Toast::new(
            "Added selection to prompt",
            ToastLevel::Info,
            None,
        ));
        true
    }

    fn handle_selection_action_key(&mut self, key: KeyEvent) -> bool {
        let Some(state) = self.selection_action_bar else {
            return false;
        };

        match key.code {
            KeyCode::Char('y') if key.modifiers == event::KeyModifiers::NONE => {
                let _ = self.try_copy_selection();
                true
            }
            KeyCode::Char('i')
                if key.modifiers == event::KeyModifiers::NONE
                    && state.target == SelectionActionTarget::Chat =>
            {
                self.add_selection_to_prompt(state.target)
            }
            KeyCode::Esc if key.modifiers == event::KeyModifiers::NONE => {
                self.dismiss_selection_actions();
                self.reset_esc_timeline_state();
                true
            }
            _ => false,
        }
    }

    fn copy_chat_selection(&mut self) -> bool {
        let Some(text) = self.selected_chat_text() else {
            return false;
        };

        let _ = crate::utils::clipboard::copy_text(&text);
        push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
        true
    }

    fn chat_area_for_size(&self, size: Rect) -> Rect {
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
        let input_height = if self.is_subagent_session_active() {
            SUBAGENT_FOOTER_HEIGHT
        } else {
            self.input.get_height_for_width(size.width)
        };
        let help_height = if self.is_subagent_session_active() {
            0
        } else {
            1
        };
        let queued_messages = self.queued_message_previews_for_current_session();
        let queue_height = if self.is_subagent_session_active() {
            0
        } else {
            queued_messages_height(&queued_messages)
        };
        let above_status_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints(
                [
                    ratatui::layout::Constraint::Length(0),
                    ratatui::layout::Constraint::Min(0),
                    ratatui::layout::Constraint::Length(0),
                    ratatui::layout::Constraint::Length(queue_height),
                    ratatui::layout::Constraint::Length(input_height),
                    ratatui::layout::Constraint::Length(help_height),
                    ratatui::layout::Constraint::Length(1),
                ]
                .as_ref(),
            )
            .split(main_chunks[0]);

        above_status_chunks[1]
    }

    fn current_chat_area(&self) -> Rect {
        self.chat_area_for_size(self.last_frame_size)
    }

    pub fn handle_keys(&mut self, key: KeyEvent) {
        if key.code != KeyCode::Esc {
            self.reset_esc_timeline_state();
        }

        if key.code == KeyCode::Char('p')
            && key.modifiers == event::KeyModifiers::CONTROL
            && matches!(
                self.overlay_focus,
                OverlayFocus::None | OverlayFocus::SuggestionsPopup | OverlayFocus::CommandPalette
            )
        {
            self.open_command_palette();
            return;
        }

        if self.handle_selection_action_key(key) {
            return;
        }

        match key.code {
            KeyCode::Char('v') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                if self.is_subagent_session_active()
                    && matches!(
                        self.overlay_focus,
                        OverlayFocus::None | OverlayFocus::SuggestionsPopup
                    )
                {
                    return;
                }
                self.handle_clipboard_image_paste();
                return;
            }
            KeyCode::Char('c') if key.modifiers == event::KeyModifiers::CONTROL => {
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
                } else if self.is_subagent_session_active() {
                    clear_suggestions(&mut self.suggestions_popup_state);
                    self.overlay_focus = OverlayFocus::None;
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
                        self.cached_usage_check = (usize::MAX, u64::MAX);

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
                    crate::views::models_dialog::ModelsDialogAction::CycleReasoning {
                        provider_id,
                        model_id,
                        direction,
                    } => {
                        if self.cycle_reasoning_effort_for_model(provider_id, model_id, direction) {
                            self.refresh_models_dialog();
                        }
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
                        self.sessions_dialog_state.dialog.pending_delete_id = Some(_id.clone());
                        true
                    }
                    SessionsDialogAction::Select(id) => {
                        self.switch_to_session(&id);
                        self.sessions_dialog_state.dialog.hide();
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    SessionsDialogAction::NewSession => {
                        self.start_blank_session(None);
                        self.sessions_dialog_state.dialog.hide();
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    SessionsDialogAction::ChangeFilter(_) => {
                        self.refresh_sessions_dialog();
                        true
                    }
                    SessionsDialogAction::TogglePin(id) => {
                        match self.session_manager.toggle_session_pin(&id) {
                            Ok(true) => {
                                push_toast(Toast::new("Pinned session", ToastLevel::Info, None))
                            }
                            Ok(false) => {
                                push_toast(Toast::new("Unpinned session", ToastLevel::Info, None))
                            }
                            Err(err) => push_toast(Toast::new(
                                format!("Failed to pin session: {:?}", err),
                                ToastLevel::Error,
                                None,
                            )),
                        }
                        self.refresh_sessions_dialog();
                        self.sessions_dialog_state.dialog.select_item_by_id(&id);
                        true
                    }
                    SessionsDialogAction::Archive(id) => {
                        let previous_selected_index =
                            self.sessions_dialog_state.dialog.selected_index;
                        let archived =
                            self.sessions_dialog_state.filter != SessionsDialogFilter::Archived;
                        let was_current = self
                            .session_manager
                            .get_current_session_id()
                            .map_or(false, |current| *current == id);
                        let _ = self.session_manager.set_session_archived(&id, archived);
                        if was_current && archived {
                            self.save_active_session_view_state();
                            self.pending_session_title = None;
                            self.session_manager.clear_current_session();
                            self.chat_state.chat.clear();
                            self.input.clear();
                            self.base_focus = BaseFocus::Home;
                            self.sync_active_streaming_flag();
                            self.cached_usage_check = (usize::MAX, u64::MAX);
                        }
                        self.refresh_sessions_dialog();
                        let _ = self
                            .sessions_dialog_state
                            .dialog
                            .select_index_clamped(previous_selected_index);
                        true
                    }
                    SessionsDialogAction::Delete(id) => {
                        let previous_selected_index =
                            self.sessions_dialog_state.dialog.selected_index;
                        let was_current = self
                            .session_manager
                            .get_current_session_id()
                            .map_or(false, |current| *current == id);
                        self.session_manager.delete_session(&id);
                        self.session_view_states.remove(&id);
                        if let Some(pending) = crate::views::sessions_dialog::get_pending_delete(
                            &mut self.sessions_dialog_state,
                        ) {
                            self.session_manager.delete_session(&pending);
                            self.session_view_states.remove(&pending);
                        }
                        self.refresh_sessions_dialog();
                        let _ = self
                            .sessions_dialog_state
                            .dialog
                            .select_index_clamped(previous_selected_index);
                        if was_current {
                            self.pending_session_title = None;
                            self.chat_state.chat.clear();
                            self.input.clear();
                            self.base_focus = BaseFocus::Home;
                            self.sync_active_streaming_flag();
                            self.cached_usage_check = (usize::MAX, u64::MAX);
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
                    SessionsDialogAction::MoveWorkspaceGroup {
                        workspace_id,
                        group,
                        direction,
                    } => {
                        match self
                            .session_manager
                            .move_workspace_sort_order(workspace_id, direction.offset())
                        {
                            Ok(true) => {
                                self.refresh_sessions_dialog();
                                let _ =
                                    self.sessions_dialog_state.dialog.focus_group_header(&group);
                            }
                            Ok(false) => {}
                            Err(err) => push_toast(Toast::new(
                                format!("Failed to move workspace: {:?}", err),
                                ToastLevel::Error,
                                None,
                            )),
                        }
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
                        let _ = self.sessions_dialog_state.dialog.select_item_by_id(&id);
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
                            if let Some(session_id) =
                                self.session_manager.get_current_session_id().cloned()
                            {
                                let _ = self.session_manager.set_session_status(
                                    &session_id,
                                    crate::session::types::SessionStatus::Streaming,
                                    None,
                                );
                            }
                            self.overlay_focus = OverlayFocus::None;
                        }
                        true
                    }
                    PermissionDialogAction::Handled => true,
                    PermissionDialogAction::NotHandled => true,
                }
            }
            OverlayFocus::QuestionDialog => {
                let action = handle_question_dialog_key_event(&mut self.question_dialog_state, key);
                match action {
                    QuestionDialogAction::Submit => {
                        self.question_dialog_state.submit_current();
                        if self.question_dialog_state.has_active() {
                            self.overlay_focus = OverlayFocus::QuestionDialog;
                        } else {
                            self.chat_state.chat.resume_streaming_tps_timer();
                            if let Some(session_id) =
                                self.session_manager.get_current_session_id().cloned()
                            {
                                let _ = self.session_manager.set_session_status(
                                    &session_id,
                                    crate::session::types::SessionStatus::Streaming,
                                    None,
                                );
                            }
                            self.overlay_focus = OverlayFocus::None;
                        }
                        true
                    }
                    QuestionDialogAction::Cancel => {
                        self.question_dialog_state.clear_with_empty();
                        self.chat_state.chat.resume_streaming_tps_timer();
                        self.overlay_focus = OverlayFocus::None;
                        self.cancel_streaming();
                        true
                    }
                    QuestionDialogAction::Handled => true,
                    QuestionDialogAction::NotHandled => true,
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
                        self.show_message_actions_from(idx, OverlayFocus::TimelineDialog);
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
            OverlayFocus::CommandPalette => {
                let action = handle_command_palette_key_event(&mut self.command_palette_state, key);
                self.handle_command_palette_action(action);
                if !self.command_palette_state.dialog.is_visible()
                    && self.overlay_focus == OverlayFocus::CommandPalette
                {
                    self.overlay_focus = OverlayFocus::None;
                }
                true
            }
            OverlayFocus::StorageDialog => {
                let action = handle_storage_dialog_key_event(&mut self.storage_dialog_state, key);
                self.handle_storage_dialog_action(action);
                if !self.storage_dialog_state.is_visible()
                    && self.overlay_focus == OverlayFocus::StorageDialog
                {
                    self.overlay_focus = OverlayFocus::None;
                }
                true
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
                    crate::views::which_key::WhichKeyAction::GoChild => {
                        self.overlay_focus = OverlayFocus::None;
                        let _ = self.switch_to_first_child_session();
                    }
                    crate::views::which_key::WhichKeyAction::GoParent => {
                        self.overlay_focus = OverlayFocus::None;
                        let _ = self.switch_to_parent_session();
                    }
                    crate::views::which_key::WhichKeyAction::PreviousChild => {
                        self.overlay_focus = OverlayFocus::None;
                        let _ = self.switch_child_session(-1);
                    }
                    crate::views::which_key::WhichKeyAction::NextChild => {
                        self.overlay_focus = OverlayFocus::None;
                        let _ = self.switch_child_session(1);
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
            KeyCode::Char('t') if key.modifiers == event::KeyModifiers::CONTROL => {
                self.cycle_active_reasoning_effort()
            }
            KeyCode::Left
                if key.modifiers == event::KeyModifiers::NONE
                    && self.should_handle_child_session_arrow() =>
            {
                self.switch_child_session(-1)
            }
            KeyCode::Right
                if key.modifiers == event::KeyModifiers::NONE
                    && self.should_handle_child_session_arrow() =>
            {
                self.switch_child_session(1)
            }
            KeyCode::Up
                if key.modifiers == event::KeyModifiers::NONE
                    && self.should_handle_child_session_arrow() =>
            {
                self.switch_to_parent_session()
            }
            KeyCode::Tab => {
                self.toggle_agent_mode();
                true
            }
            KeyCode::Esc => {
                // If text is selected, clear selection first
                if self.clear_selection() {
                    self.reset_esc_timeline_state();
                    return true;
                }
                if self.is_streaming {
                    self.reset_esc_timeline_state();
                    if let Some(session_id) = self.session_manager.get_current_session_id().cloned()
                    {
                        if self.interrupt_streaming_to_send_queued_for_session(&session_id) {
                            return true;
                        }
                    }
                    self.cancel_streaming();
                    return true;
                }
                if self.overlay_focus == OverlayFocus::SuggestionsPopup {
                    self.reset_esc_timeline_state();
                    self.input.clear();
                    clear_suggestions(&mut self.suggestions_popup_state);
                    self.overlay_focus = OverlayFocus::None;
                    true
                } else {
                    self.handle_timeline_esc_key(key)
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

    fn toggle_agent_mode(&mut self) {
        if self.agent.eq_ignore_ascii_case("plan") {
            self.agent = "Build".to_string();
        } else {
            self.agent = "Plan".to_string();
        }

        let colors = self.get_current_theme_colors();
        let agent_color = crate::theme::agent_color(&self.agent, &colors);
        self.chat_state.wave_spinner.set_color(agent_color);
    }

    fn reset_esc_timeline_state(&mut self) {
        self.esc_timeline_primed = false;
    }

    fn handle_timeline_esc_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers != event::KeyModifiers::NONE
            || self.base_focus != BaseFocus::Chat
            || !self.input.is_empty()
            || self.is_subagent_session_active()
        {
            self.reset_esc_timeline_state();
            return false;
        }

        if self.esc_timeline_primed {
            self.reset_esc_timeline_state();
            self.open_timeline_dialog();
        } else {
            self.esc_timeline_primed = true;
        }

        true
    }

    fn handle_input_and_app_keys(&mut self, key: KeyEvent) {
        if self.selection_action_bar.is_some() {
            self.dismiss_selection_actions();
        } else {
            self.chat_state.chat.selection.clear();
        }

        if self.is_subagent_session_active() {
            clear_suggestions(&mut self.suggestions_popup_state);
            self.overlay_focus = OverlayFocus::None;
            return;
        }

        match key.code {
            KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
                let image_paths = self.input.local_image_paths_for_submission();
                let input_text = self.input.submission_text();
                if !input_text.is_empty() || !image_paths.is_empty() {
                    use crate::command::parser::parse_input;

                    let input_type = parse_input(&input_text);
                    match input_type {
                        crate::command::parser::InputType::Command(parsed) => {
                            // Don't save commands to prompt history
                            tokio::task::block_in_place(|| {
                                let rt = tokio::runtime::Handle::current();
                                rt.block_on(self.process_command_input(parsed));
                            });
                        }
                        crate::command::parser::InputType::AgentMention(mention) => {
                            if image_paths.is_empty() {
                                self.input.save_current_to_history();
                            }
                            if !self.is_streaming {
                                self.handle_agent_mention_input(mention, image_paths);
                            } else {
                                return;
                            }
                        }
                        crate::command::parser::InputType::Message(msg) => {
                            // Only save messages (not commands) to prompt history
                            if image_paths.is_empty() {
                                self.input.save_current_to_history();
                            }
                            let active_session_streaming = self
                                .session_manager
                                .get_current_session_id()
                                .is_some_and(|id| self.session_has_active_stream(id));
                            if self.is_streaming && active_session_streaming {
                                self.queue_message_for_current_session(
                                    msg.to_string(),
                                    image_paths,
                                );
                            } else if !self.is_streaming {
                                self.handle_message_input_with_images(msg, image_paths);
                            } else {
                                return;
                            }
                        }
                    }

                    self.input.clear();
                    self.clear_suggestions_and_blur();
                }
            }
            _ => {
                self.input.handle_event(key);
                self.update_suggestions();
            }
        }
    }

    fn can_submit_input(input_type: &InputType<'_>, is_streaming: bool) -> bool {
        matches!(input_type, InputType::Command(_)) || !is_streaming
    }

    fn update_suggestions(&mut self) {
        if self.input.should_show_suggestions() {
            let suggestions = self
                .input
                .get_autocomplete_suggestions(self.base_focus == BaseFocus::Chat);
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

    fn suggestions_popup_anchor_area(&self) -> ratatui::layout::Rect {
        let main_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([ratatui::layout::Constraint::Min(0)].as_ref())
            .split(self.last_frame_size);
        let input_height = self.input.get_height_for_width(self.last_frame_size.width);
        let queued_messages = self.queued_message_previews_for_current_session();
        let queue_height =
            if self.base_focus == BaseFocus::Chat && !self.is_subagent_session_active() {
                queued_messages_height(&queued_messages)
            } else {
                0
            };
        let input_chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints(
                [
                    ratatui::layout::Constraint::Min(0),
                    ratatui::layout::Constraint::Length(queue_height),
                    ratatui::layout::Constraint::Length(input_height),
                ]
                .as_ref(),
            )
            .split(main_chunks[0]);

        input_chunks[2]
    }

    fn handle_selection_action_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(state) = self.selection_action_bar else {
            return false;
        };
        let Some(area) = self.current_selection_action_bar_area() else {
            return false;
        };

        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
        if !area.contains(point) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => true,
            MouseEventKind::Up(MouseButton::Left) => {
                let rel = mouse.column.saturating_sub(area.x) as usize;
                match selection_action_for_column(state.target, rel) {
                    SelectionAction::AddToPrompt => self.add_selection_to_prompt(state.target),
                    SelectionAction::Copy => {
                        let _ = self.try_copy_selection();
                        true
                    }
                    SelectionAction::Dismiss => self.dismiss_selection_actions(),
                }
            }
            _ => true,
        }
    }

    fn handle_input_mouse_event(&mut self, mouse: MouseEvent) -> bool {
        if self.is_subagent_session_active() {
            return false;
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.selection_action_bar = None;
        }

        if matches!(mouse.kind, MouseEventKind::Moved) && !self.input.contains_mouse(mouse) {
            self.input.clear_hover();
        }

        if !self.input.handle_mouse_event(mouse) {
            if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left))
                && self.input.has_selection()
                && !self.input.get_selected_text().is_empty()
            {
                self.show_selection_action_bar_for(SelectionActionTarget::Input);
                self.update_suggestions();
                return true;
            }

            return false;
        }

        if matches!(
            mouse.kind,
            ratatui::crossterm::event::MouseEventKind::Up(
                ratatui::crossterm::event::MouseButton::Left
            )
        ) {
            if self.input.has_selection() && !self.input.get_selected_text().is_empty() {
                self.show_selection_action_bar_for(SelectionActionTarget::Input);
            } else {
                self.selection_action_bar = None;
            }
        }
        self.update_suggestions();
        true
    }

    fn open_chat_image_target(&self, target: &ChatImageTarget) {
        let path = std::path::Path::new(&target.path);
        match crate::utils::image_attachment::open_path(path, &self.images) {
            Ok(()) => push_toast(Toast::new(
                format!("Opened {}", target.placeholder),
                ToastLevel::Info,
                None,
            )),
            Err(err) => push_toast(Toast::new(
                format!("Failed to open image: {}", err),
                ToastLevel::Error,
                None,
            )),
        }
    }

    fn open_chat_hyperlink_target(&self, target: &HyperlinkTarget) {
        match target {
            HyperlinkTarget::File(path) => {
                match crate::utils::image_attachment::open_file_path(path) {
                    Ok(()) => push_toast(Toast::new(
                        format!("Opened {}", path.display()),
                        ToastLevel::Info,
                        None,
                    )),
                    Err(err) => push_toast(Toast::new(
                        format!("Failed to open file: {}", err),
                        ToastLevel::Error,
                        None,
                    )),
                }
            }
            HyperlinkTarget::Url(url) => match crate::utils::image_attachment::open_url(url) {
                Ok(()) => push_toast(Toast::new(
                    format!("Opened {}", url),
                    ToastLevel::Info,
                    None,
                )),
                Err(err) => push_toast(Toast::new(
                    format!("Failed to open link: {}", err),
                    ToastLevel::Error,
                    None,
                )),
            },
        }
    }

    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        if std::env::var_os("CRABCODE_MOUSE_TRACE").is_some() {
            crate::emit_log!(
                "Handle mouse: kind={:?} modifiers={:?} col={} row={} base={:?} overlay={:?}",
                mouse.kind,
                mouse.modifiers,
                mouse.column,
                mouse.row,
                self.base_focus,
                self.overlay_focus
            );
        }

        if matches!(mouse.kind, MouseEventKind::Moved) && !self.input.contains_mouse(mouse) {
            self.input.clear_hover();
        }

        if self.handle_selection_action_mouse(mouse) {
            return;
        }

        if self.selection_action_bar.is_some()
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.dismiss_selection_actions();
            return;
        }

        if matches!(mouse.kind, MouseEventKind::Moved) && self.base_focus != BaseFocus::Chat {
            self.chat_state.chat.clear_hovered_image();
            self.chat_state.chat.clear_hovered_hyperlink();
        }

        // If text is selected and user clicks on an overlay, clear selection instead
        if self.overlay_focus != OverlayFocus::None
            && (self.chat_state.chat.has_selection() || self.input.has_selection())
            && self.selection_action_bar.is_none()
            && matches!(
                mouse.kind,
                ratatui::crossterm::event::MouseEventKind::Down(_)
            )
        {
            self.dismiss_selection_actions();
            return;
        }

        if self.overlay_focus == OverlayFocus::ModelsDialog {
            let action = handle_models_dialog_mouse_event(&mut self.models_dialog_state, mouse);
            match action {
                crate::views::models_dialog::ModelsDialogAction::SelectModel {
                    provider_id,
                    model_id,
                } => {
                    let model_id_clone = model_id.clone();
                    let provider_id_clone = provider_id.clone();
                    self.model = model_id_clone.clone();
                    self.provider_name = provider_id_clone;
                    self.cached_usage_check = (usize::MAX, u64::MAX);

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
                crate::views::models_dialog::ModelsDialogAction::CycleReasoning {
                    provider_id,
                    model_id,
                    direction,
                } => {
                    if self.cycle_reasoning_effort_for_model(provider_id, model_id, direction) {
                        self.refresh_models_dialog();
                    }
                }
                crate::views::models_dialog::ModelsDialogAction::None => {}
            }
            if !self.models_dialog_state.dialog.is_visible() {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::PermissionDialog {
            let handled =
                handle_permission_dialog_mouse_event(&mut self.permission_dialog_state, mouse);
            if !handled
                && matches!(
                    mouse.kind,
                    ratatui::crossterm::event::MouseEventKind::ScrollDown
                        | ratatui::crossterm::event::MouseEventKind::ScrollUp
                )
                && self.base_focus == BaseFocus::Chat
            {
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
                let input_height = self.input.get_height_for_width(size.width);
                let input_height = if self.is_subagent_session_active() {
                    SUBAGENT_FOOTER_HEIGHT
                } else {
                    input_height
                };
                let help_height = if self.is_subagent_session_active() {
                    0
                } else {
                    1
                };
                let above_status_chunks = ratatui::layout::Layout::default()
                    .direction(ratatui::layout::Direction::Vertical)
                    .constraints(
                        [
                            ratatui::layout::Constraint::Length(0),
                            ratatui::layout::Constraint::Min(0),
                            ratatui::layout::Constraint::Length(0),
                            ratatui::layout::Constraint::Length(input_height),
                            ratatui::layout::Constraint::Length(help_height),
                            ratatui::layout::Constraint::Length(1),
                        ]
                        .as_ref(),
                    )
                    .split(main_chunks[0]);
                let chat_area = above_status_chunks[1];
                let _ = self.chat_state.chat.handle_mouse_event(mouse, chat_area);
            }
        } else if self.overlay_focus == OverlayFocus::QuestionDialog {
            let _ = handle_question_dialog_mouse_event(&mut self.question_dialog_state, mouse);
        } else if self.overlay_focus == OverlayFocus::ThemesDialog {
            let action = handle_themes_dialog_mouse_event(&mut self.themes_dialog_state, mouse);

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
                return;
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
            let action = handle_sessions_dialog_mouse_event(&mut self.sessions_dialog_state, mouse);
            match action {
                SessionsDialogAction::Select(id) => {
                    self.switch_to_session(&id);
                    self.sessions_dialog_state.dialog.hide();
                    self.overlay_focus = OverlayFocus::None;
                }
                SessionsDialogAction::Close => {
                    self.overlay_focus = OverlayFocus::None;
                }
                _ => {
                    if !self.sessions_dialog_state.dialog.is_visible() {
                        self.overlay_focus = OverlayFocus::None;
                    }
                }
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
            let action = crate::views::timeline_dialog::handle_timeline_dialog_mouse_event(
                &mut self.timeline_dialog_state,
                mouse,
            );
            match action {
                crate::views::timeline_dialog::TimelineDialogAction::Close => {
                    self.chat_state.chat.clear_highlighted_message();
                    self.overlay_focus = OverlayFocus::None;
                }
                crate::views::timeline_dialog::TimelineDialogAction::Select(idx) => {
                    self.chat_state.chat.scroll_to_message_index(idx);
                    self.chat_state.chat.set_highlighted_message(Some(idx));
                    self.show_message_actions_from(idx, OverlayFocus::TimelineDialog);
                }
                crate::views::timeline_dialog::TimelineDialogAction::Navigate(idx) => {
                    self.chat_state.chat.scroll_to_message_index(idx);
                    self.chat_state.chat.set_highlighted_message(Some(idx));
                }
                crate::views::timeline_dialog::TimelineDialogAction::Handled
                | crate::views::timeline_dialog::TimelineDialogAction::NotHandled => {}
            }
            if !self.timeline_dialog_state.dialog.is_visible() {
                self.chat_state.chat.clear_highlighted_message();
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::MessageActions {
            if matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            ) {
                let chat_area = self.current_chat_area();
                let click_inside_popup = self
                    .message_actions_dialog
                    .as_ref()
                    .map(|dialog| dialog.contains_position(mouse.column, mouse.row))
                    .unwrap_or(false);
                if !click_inside_popup {
                    if let Some(target) = self.chat_state.chat.image_at_position(mouse, chat_area) {
                        self.chat_state.chat.set_hovered_image(Some(target.clone()));
                        self.pending_chat_message_click = None;
                        self.close_message_actions();
                        self.open_chat_image_target(&target);
                        return;
                    }

                    if let Some(target) =
                        self.chat_state.chat.hyperlink_at_position(mouse, chat_area)
                    {
                        self.pending_chat_message_click = None;
                        self.close_message_actions();
                        self.open_chat_hyperlink_target(&target);
                        return;
                    }
                }
            }

            let maybe_action = if let Some(ref mut dialog) = self.message_actions_dialog {
                let clicked_item = if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                {
                    dialog.item_index_at_position(mouse.column, mouse.row)
                } else {
                    None
                };
                let handled = dialog.handle_mouse_event(mouse);
                if handled && clicked_item.is_some() {
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
        } else if self.overlay_focus == OverlayFocus::CommandPalette {
            let action = handle_command_palette_mouse_event(&mut self.command_palette_state, mouse);
            self.handle_command_palette_action(action);
            if !self.command_palette_state.dialog.is_visible()
                && self.overlay_focus == OverlayFocus::CommandPalette
            {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::StorageDialog {
            let action = handle_storage_dialog_mouse_event(&mut self.storage_dialog_state, mouse);
            self.handle_storage_dialog_action(action);
            if !self.storage_dialog_state.is_visible()
                && self.overlay_focus == OverlayFocus::StorageDialog
            {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::SuggestionsPopup {
            let anchor_area = self.suggestions_popup_anchor_area();
            let action = handle_suggestions_popup_mouse_event(
                &mut self.suggestions_popup_state,
                mouse,
                anchor_area,
            );
            match action {
                crate::ui::components::popup::PopupAction::Handled => {}
                crate::ui::components::popup::PopupAction::Autocomplete => {
                    self.autocomplete_and_submit();
                }
                crate::ui::components::popup::PopupAction::NotHandled => {
                    if self.handle_input_mouse_event(mouse) {
                        return;
                    }
                    if matches!(
                        mouse.kind,
                        ratatui::crossterm::event::MouseEventKind::Down(
                            ratatui::crossterm::event::MouseButton::Left
                        )
                    ) {
                        self.clear_suggestions_and_blur();
                    }
                }
            }
        } else if self.overlay_focus == OverlayFocus::None {
            // If chat has a selection and user clicks outside chat area, clear it.
            // Dragging is handled by the chat component so edge scrolling can continue.
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && self.chat_state.chat.has_selection()
                && !self.chat_state.chat.selection.is_dragging
                && self.base_focus == BaseFocus::Chat
            {
                let chat_area = self.current_chat_area();

                let point = ratatui::layout::Position::new(mouse.column, mouse.row);
                if !chat_area.contains(point) {
                    self.dismiss_selection_actions();
                }
            }

            // Handle mouse events for chat scrolling/selection when in chat mode
            if self.base_focus == BaseFocus::Chat {
                let chat_area = self.current_chat_area();

                match mouse.kind {
                    MouseEventKind::Moved
                        if !self.chat_state.chat.has_selection()
                            && !self.chat_state.chat.selection.is_dragging =>
                    {
                        let hovered_image =
                            self.chat_state.chat.image_at_position(mouse, chat_area);
                        let hovered_hyperlink = if hovered_image.is_none() {
                            self.chat_state
                                .chat
                                .hyperlink_hover_at_position(mouse, chat_area)
                        } else {
                            None
                        };
                        let hovered_message = self
                            .chat_state
                            .chat
                            .message_index_at_position(mouse, chat_area);
                        self.chat_state.chat.set_hovered_image(hovered_image);
                        self.chat_state
                            .chat
                            .set_hovered_hyperlink(hovered_hyperlink);
                        if hovered_message.is_some() {
                            return;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if (mouse.modifiers.is_empty()
                            || mouse.modifiers.contains(KeyModifiers::SUPER)
                            || mouse.modifiers.contains(KeyModifiers::META))
                            && !self.chat_state.chat.has_selection()
                            && !self.chat_state.chat.selection.is_dragging =>
                    {
                        if let Some(target) =
                            self.chat_state.chat.image_at_position(mouse, chat_area)
                        {
                            self.chat_state.chat.set_hovered_image(Some(target.clone()));
                            self.pending_chat_message_click = None;
                            self.open_chat_image_target(&target);
                            return;
                        }

                        if let Some(target) =
                            self.chat_state.chat.hyperlink_at_position(mouse, chat_area)
                        {
                            self.pending_chat_message_click = None;
                            self.open_chat_hyperlink_target(&target);
                            return;
                        }

                        if mouse.modifiers.is_empty() {
                            self.pending_chat_message_click =
                                self.message_actions_index_at_position(mouse, chat_area);
                        }
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        self.pending_chat_message_click = None;
                    }
                    _ => {}
                }

                let had_selection = self.chat_state.chat.has_selection();
                let was_dragging = self.chat_state.chat.selection.is_dragging;
                let released_pending_message =
                    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left))
                        && !mouse.modifiers.contains(KeyModifiers::SHIFT)
                    {
                        self.pending_chat_message_click.and_then(|idx| {
                            (self
                                .chat_state
                                .chat
                                .message_index_at_position(mouse, chat_area)
                                == Some(idx))
                            .then_some(idx)
                        })
                    } else {
                        None
                    };

                if self.chat_state.chat.handle_mouse_event(mouse, chat_area) {
                    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                        self.selection_action_bar = None;
                    }

                    if let Some(idx) = released_pending_message {
                        if !self.chat_state.chat.has_selection() {
                            self.pending_chat_message_click = None;
                            self.chat_state.chat.set_highlighted_message(Some(idx));
                            self.show_message_actions_from(idx, OverlayFocus::None);
                            return;
                        }
                    }

                    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                        self.pending_chat_message_click = None;
                    }

                    // Show copy/add-to-prompt actions when selection is finalized (mouse up after drag)
                    if !had_selection && self.chat_state.chat.has_selection() {
                        // New selection just started, don't show actions yet
                    } else if was_dragging && !self.chat_state.chat.selection.is_dragging {
                        self.show_selection_action_bar_for(SelectionActionTarget::Chat);
                    }
                    return;
                }
            }

            // Handle mouse events for the main input when no overlay is focused
            self.chat_state.chat.clear_hovered_image();
            self.chat_state.chat.clear_hovered_hyperlink();
            self.handle_input_mouse_event(mouse);
        }
    }

    fn handle_clipboard_image_paste(&mut self) {
        if self.is_subagent_session_active() {
            return;
        }

        if !matches!(
            (self.base_focus, self.overlay_focus),
            (BaseFocus::Home, OverlayFocus::None)
                | (BaseFocus::Chat, OverlayFocus::None)
                | (_, OverlayFocus::SuggestionsPopup)
        ) {
            return;
        }

        match crate::utils::image_attachment::paste_image_to_temp_png() {
            Ok(path) => {
                self.input.attach_image(path);
                self.input.insert_str(" ");
                self.update_suggestions();
                push_toast(Toast::new(
                    "Attached image from clipboard",
                    ToastLevel::Info,
                    None,
                ));
            }
            Err(err) => push_toast(Toast::new(
                format!("Clipboard image paste failed: {}", err),
                ToastLevel::Warning,
                None,
            )),
        }
    }

    fn try_attach_pasted_image_paths(&mut self, text: &str) -> bool {
        let image_paths = crate::utils::image_attachment::image_paths_from_paste(text);
        if image_paths.is_empty() {
            return false;
        }

        let exact_single_image = crate::utils::image_attachment::normalize_pasted_path(text)
            .map(|path| crate::utils::image_attachment::is_supported_image_path(&path))
            .unwrap_or(false);
        let token_count = shlex::split(text)
            .map(|parts| {
                parts
                    .into_iter()
                    .filter(|part| !part.trim().is_empty())
                    .count()
            })
            .unwrap_or_else(|| text.lines().filter(|line| !line.trim().is_empty()).count());

        if !exact_single_image && token_count != image_paths.len() {
            return false;
        }

        let count = image_paths.len();
        for path in image_paths {
            self.input.attach_image(path);
            self.input.insert_str(" ");
        }
        self.update_suggestions();
        push_toast(Toast::new(
            if count == 1 {
                "Attached image".to_string()
            } else {
                format!("Attached {} images", count)
            },
            ToastLevel::Info,
            None,
        ));
        true
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
                if self.is_subagent_session_active() {
                    return;
                }
                if self.try_attach_pasted_image_paths(&text) {
                    return;
                }
                self.input.insert_paste(&text);
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
            (_, OverlayFocus::CommandPalette) => {
                self.command_palette_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.command_palette_state.dialog.set_search_query(
                    self.command_palette_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
                self.command_palette_state.dialog.selected_index = 0;
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
                if self.is_subagent_session_active() {
                    clear_suggestions(&mut self.suggestions_popup_state);
                    self.overlay_focus = OverlayFocus::None;
                    return;
                }
                if self.try_attach_pasted_image_paths(&text) {
                    return;
                }
                self.input.insert_paste(&text);
                self.update_suggestions();
            }
            (_, OverlayFocus::QuestionDialog) => {
                self.question_dialog_state.insert_text(&text);
            }
            _ => {}
        }
    }

    fn autocomplete_and_submit(&mut self) {
        if let Some(selected) = get_selected_suggestion(&self.suggestions_popup_state).cloned() {
            match selected.kind {
                crate::autocomplete::SuggestionKind::Command => {
                    let command = format!("/{}", selected.name);

                    tokio::task::block_in_place(|| {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(self.process_input(&command));
                    });

                    self.input.clear();
                }
                crate::autocomplete::SuggestionKind::Agent => {
                    self.input.apply_suggestion(&selected);
                    self.update_suggestions();
                }
                crate::autocomplete::SuggestionKind::File => {
                    self.input.apply_suggestion(&selected);
                    self.update_suggestions();
                }
            }
        }
        self.clear_suggestions_and_blur();
    }

    fn open_command_palette(&mut self) {
        if self.overlay_focus == OverlayFocus::CommandPalette
            && self.command_palette_state.dialog.is_visible()
        {
            self.command_palette_state.dialog.hide();
            self.overlay_focus = OverlayFocus::None;
            return;
        }

        clear_suggestions(&mut self.suggestions_popup_state);
        self.command_palette_state
            .refresh_items(&self.command_registry, self.base_focus == BaseFocus::Chat);
        self.command_palette_state.show();
        self.overlay_focus = OverlayFocus::CommandPalette;
    }

    fn open_storage_dialog(&mut self) {
        clear_suggestions(&mut self.suggestions_popup_state);
        self.storage_dialog_state.show();
        self.overlay_focus = OverlayFocus::StorageDialog;

        if !self.storage_dialog_state.has_report() && !self.storage_dialog_state.is_checking() {
            self.start_storage_refresh();
        }
    }

    fn start_storage_refresh(&mut self) {
        if self.storage_receiver.is_some() {
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<StorageTaskMessage>();
        self.storage_receiver = Some(receiver);
        self.storage_dialog_state.start_checking();

        tokio::task::spawn_blocking(move || {
            let report = crate::utils::storage::collect_storage_report();
            let _ = sender.send(StorageTaskMessage::Loaded(report));
        });
    }

    fn handle_storage_dialog_action(&mut self, action: StorageDialogAction) {
        match action {
            StorageDialogAction::None => {}
            StorageDialogAction::Close => {
                self.overlay_focus = OverlayFocus::None;
            }
            StorageDialogAction::Refresh => {
                self.start_storage_refresh();
            }
            StorageDialogAction::Open(category) => {
                self.open_storage_category(category);
            }
        }
    }

    fn open_storage_category(&mut self, category: crate::utils::storage::StorageCategory) {
        let Some(path) = self.storage_dialog_state.open_path_for(category) else {
            push_toast(Toast::new(
                "Storage location is not available yet",
                ToastLevel::Warning,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        };

        match crate::utils::storage::open_folder(&path) {
            Ok(()) => push_toast(Toast::new(
                format!("Opened {}", path.display()),
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(2)),
            )),
            Err(err) => push_toast(Toast::new(
                format!("Failed to open storage folder: {}", err),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            )),
        }
    }

    fn handle_command_palette_action(&mut self, action: CommandPaletteAction) {
        match action {
            CommandPaletteAction::RunCommand(command) => {
                self.overlay_focus = OverlayFocus::None;
                let command = format!("/{}", command);

                tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(self.process_input(&command));
                });

                self.input.clear();
                self.clear_suggestions_and_blur();
            }
            CommandPaletteAction::RunAppAction(action) => {
                self.overlay_focus = OverlayFocus::None;
                match action {
                    CommandPaletteAppAction::ToggleAgentMode => self.toggle_agent_mode(),
                    CommandPaletteAppAction::CycleReasoningEffort => {
                        let _ = self.cycle_active_reasoning_effort();
                    }
                    CommandPaletteAppAction::OpenStorage => self.open_storage_dialog(),
                    CommandPaletteAppAction::OpenSkillsDialog => self.show_skills_dialog(),
                }
                self.clear_suggestions_and_blur();
            }
            CommandPaletteAction::None => {}
        }
    }

    fn clear_suggestions_and_blur(&mut self) {
        clear_suggestions(&mut self.suggestions_popup_state);
        if self.overlay_focus == OverlayFocus::SuggestionsPopup {
            self.overlay_focus = OverlayFocus::None;
        }
    }

    fn copy_session_transcript(&mut self) {
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
                    transcript.push_str(&format!("## Assistant ({agent} · {model}{duration})\n\n"));
                    transcript.push_str(&msg.content);
                    transcript.push_str("\n\n---\n\n");
                }
                crate::session::types::MessageRole::Tool => {
                    transcript.push_str("**Tool Result**\n\n");
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg.content) {
                        if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                            transcript.push_str(&format!("**Tool:** {}\n", name));
                        }
                        if let Some(args) = v.get("args") {
                            let args = serde_json::to_string_pretty(args)
                                .unwrap_or_else(|_| args.to_string());
                            transcript
                                .push_str(&format!("**Arguments:**\n```json\n{}\n```\n", args));
                        }
                        if let Some(preview) = v.get("output_preview").and_then(|p| p.as_str()) {
                            transcript.push_str(&format!("**Output:**\n```\n{}\n```\n", preview));
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
    }

    fn reject_chat_only_command_outside_chat(&mut self, command_name: &str) -> bool {
        if self.base_focus == BaseFocus::Chat || !self.command_registry.is_chat_only(command_name) {
            return false;
        }

        self.play_sound_event(crate::sound::SoundEvent::Error);
        push_toast(Toast::new(
            format!("/{command_name} is only available during chat"),
            ToastLevel::Error,
            Some(std::time::Duration::from_secs(3)),
        ));
        true
    }

    async fn compact_current_session(&mut self) {
        if self.compaction_receiver.is_some() {
            push_toast(Toast::new(
                "Compaction is already running",
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        if self.is_streaming {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "Cannot compact while a response is running",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "No active session to compact",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        };

        let messages = self.chat_state.chat.messages.clone();
        let Some(selection) = crate::session::compaction::select_messages(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
        ) else {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "Nothing to compact",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        };

        let before_tokens = crate::session::compaction::total_context_tokens(&messages);
        let before_messages = messages.len();
        let prompt = crate::session::compaction::build_prompt(&selection.messages_to_summarize);
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<CompactionTaskMessage>();
        self.compaction_receiver = Some(receiver);
        self.compaction_pending = Some(CompactionPending {
            session_id: session_id.clone(),
            before_tokens,
        });
        self.is_streaming = true;
        self.cached_usage_check = (usize::MAX, u64::MAX);
        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Waiting,
            None,
        );
        push_toast(Toast::new(
            "Compacting session...",
            ToastLevel::Info,
            Some(std::time::Duration::from_secs(2)),
        ));

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let reasoning_effort = self.active_reasoning_effort();
        let agent = self.agent.clone();
        let tail_messages = selection.tail_messages;
        let task_session_id = session_id.clone();

        tokio::spawn(async move {
            let result = crate::llm::client::summarize_for_compaction(
                provider_name.clone(),
                model.clone(),
                reasoning_effort,
                prompt,
            )
            .await
            .map(|summary| {
                let mut messages = crate::session::compaction::build_compacted_messages(
                    &summary,
                    tail_messages,
                    Some(model),
                    Some(provider_name),
                    Some(agent),
                    None,
                );
                let after_tokens = crate::session::compaction::total_context_tokens(&messages);
                let stats = crate::session::types::CompactionStats {
                    before_tokens,
                    after_tokens,
                    before_messages,
                    after_messages: messages.len(),
                };
                crate::session::compaction::append_compaction_marker(&mut messages, stats);
                (messages, stats)
            });

            let message = match result {
                Ok((messages, stats)) => CompactionTaskMessage::Success {
                    session_id: task_session_id,
                    messages,
                    stats,
                },
                Err(err) => CompactionTaskMessage::Failed {
                    session_id: task_session_id,
                    error: err.to_string(),
                },
            };
            let _ = sender.send(message);
        });
    }

    async fn process_input(&mut self, input: &str) {
        use crate::command::parser::parse_input;

        match parse_input(input) {
            InputType::Command(mut parsed) => {
                if self.command_registry.is_custom_command(&parsed.name) {
                    parsed.prefs_dao = self.prefs_dao.as_ref();
                    parsed.active_model_id = Some(self.model.clone());
                    let result = self
                        .command_registry
                        .execute(&parsed, &mut self.session_manager)
                        .await;
                    match result {
                        crate::command::registry::CommandResult::RunPrompt {
                            prompt,
                            agent,
                            model,
                            subtask,
                        } => self.run_custom_command_prompt(prompt, agent, model, subtask),
                        crate::command::registry::CommandResult::Error(msg) => {
                            self.play_sound_event(crate::sound::SoundEvent::Error);
                            push_toast(Toast::new(
                                msg,
                                ToastLevel::Error,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                        }
                        _ => {}
                    }
                    return;
                }
                if parsed.name == "copy" && self.base_focus == BaseFocus::Chat {
                    self.copy_session_transcript();
                    return;
                }
                if parsed.name == "sessions" {
                    self.open_sessions_dialog();
                    return;
                }
                if parsed.name == "new" {
                    let title = if parsed.args.is_empty() {
                        None
                    } else {
                        Some(parsed.args.join(" "))
                    };
                    self.start_blank_session(title);
                    return;
                }
                if parsed.name == "home" {
                    self.start_blank_session(None);
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
                if parsed.name == "rename"
                    && parsed.args.is_empty()
                    && self.base_focus == BaseFocus::Chat
                {
                    let session_info = self
                        .session_manager
                        .get_current_session()
                        .map(|session| (session.id.clone(), session.title.clone()));
                    if let Some((id, title)) = session_info {
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
                if parsed.name == "compact" && self.base_focus == BaseFocus::Chat {
                    if !parsed.args.is_empty() {
                        self.play_sound_event(crate::sound::SoundEvent::Error);
                        push_toast(Toast::new(
                            "Usage: /compact",
                            ToastLevel::Error,
                            Some(std::time::Duration::from_secs(3)),
                        ));
                    } else {
                        self.compact_current_session().await;
                    }
                    return;
                }
                if self.reject_chat_only_command_outside_chat(&parsed.name) {
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
                            self.pending_session_title = None;
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
                    crate::command::registry::CommandResult::RunPrompt {
                        prompt,
                        agent,
                        model,
                        subtask,
                    } => self.run_custom_command_prompt(prompt, agent, model, subtask),
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
                            self.open_sessions_dialog();
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
                            self.show_models_dialog(title, dialog_items);
                        }
                    }
                }
            }
            InputType::Message(msg) => {
                self.handle_message_input(msg);
            }
            InputType::AgentMention(mention) => {
                self.handle_agent_mention_input(mention, Vec::new());
            }
        }
    }

    async fn process_command_input(
        &mut self,
        mut parsed: crate::command::parser::ParsedCommand<'_>,
    ) {
        if self.command_registry.is_custom_command(&parsed.name) {
            parsed.prefs_dao = self.prefs_dao.as_ref();
            parsed.active_model_id = Some(self.model.clone());
            let result = self
                .command_registry
                .execute(&parsed, &mut self.session_manager)
                .await;
            match result {
                crate::command::registry::CommandResult::RunPrompt {
                    prompt,
                    agent,
                    model,
                    subtask,
                } => self.run_custom_command_prompt(prompt, agent, model, subtask),
                crate::command::registry::CommandResult::Error(msg) => {
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        msg,
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                }
                _ => {}
            }
            return;
        }
        if parsed.name == "copy" && self.base_focus == BaseFocus::Chat {
            self.copy_session_transcript();
            return;
        }
        if parsed.name == "sessions" {
            self.open_sessions_dialog();
            return;
        }
        if parsed.name == "new" {
            let title = if parsed.args.is_empty() {
                None
            } else {
                Some(parsed.args.join(" "))
            };
            self.start_blank_session(title);
            return;
        }
        if parsed.name == "home" {
            self.start_blank_session(None);
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
            let session_info = self
                .session_manager
                .get_current_session()
                .map(|session| (session.id.clone(), session.title.clone()));
            if let Some((id, title)) = session_info {
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
        if parsed.name == "compact" && self.base_focus == BaseFocus::Chat {
            if !parsed.args.is_empty() {
                self.play_sound_event(crate::sound::SoundEvent::Error);
                push_toast(Toast::new(
                    "Usage: /compact",
                    ToastLevel::Error,
                    Some(std::time::Duration::from_secs(3)),
                ));
            } else {
                self.compact_current_session().await;
            }
            return;
        }
        if self.reject_chat_only_command_outside_chat(&parsed.name) {
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
                    self.pending_session_title = None;
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
            crate::command::registry::CommandResult::RunPrompt {
                prompt,
                agent,
                model,
                subtask,
            } => self.run_custom_command_prompt(prompt, agent, model, subtask),
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
                    self.open_sessions_dialog();
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
                    self.show_models_dialog(title, dialog_items);
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
        let mut sessions = self.session_manager.list_sessions();
        let current_workspace_id = self.session_manager.current_workspace_id();
        let filter = self.sessions_dialog_state.filter;

        sessions.retain(|session| {
            if session.parent_id.is_some() {
                return false;
            }

            let is_archived = session.archived_at.is_some();
            let is_running = session.status.is_active()
                || self
                    .session_view_states
                    .get(&session.id)
                    .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some());

            match filter {
                SessionsDialogFilter::Active => {
                    !is_archived && (session.workspace_id == current_workspace_id || is_running)
                }
                SessionsDialogFilter::All => !is_archived,
                SessionsDialogFilter::Archived => is_archived,
            }
        });

        sessions.sort_by(|a, b| {
            a.workspace_sort_order
                .cmp(&b.workspace_sort_order)
                .then_with(|| a.workspace_id.cmp(&b.workspace_id))
                .then_with(|| b.pinned_at.is_some().cmp(&a.pinned_at.is_some()))
                .then_with(|| b.status.is_active().cmp(&a.status.is_active()))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });

        let mut workspace_group_ids = std::collections::HashMap::new();
        let items: Vec<crate::ui::components::dialog::DialogItem> = sessions
            .into_iter()
            .map(|session| {
                let view_state = self.session_view_states.get(&session.id);
                let is_streaming = view_state
                    .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some())
                    || session.status.is_active();
                let unread_completed = view_state.is_some_and(|state| state.unread_completed);
                let marker = if is_streaming {
                    format!("{} ", self.session_loading_glyph())
                } else if unread_completed {
                    "● ".to_string()
                } else {
                    String::new()
                };
                let pin = if session.pinned_at.is_some() {
                    "★ "
                } else {
                    ""
                };
                let name = format!("{}{}{}", marker, pin, session.title);
                let group = if session.workspace_name.trim().is_empty() {
                    session.workspace_path.clone()
                } else {
                    session.workspace_name.clone()
                };
                workspace_group_ids
                    .entry(group.clone())
                    .or_insert(session.workspace_id);

                crate::ui::components::dialog::DialogItem {
                    id: session.id.clone(),
                    name,
                    group,
                    description: String::new(),
                    tip: Some(crate::utils::time::relative_readable_time_from_now(
                        session.updated_at,
                    )),
                    provider_id: session.title.clone(),
                }
            })
            .collect();

        self.sessions_dialog_state.refresh_items(items);
        self.sessions_dialog_state
            .set_workspace_group_ids(workspace_group_ids);
    }

    fn session_loading_glyph(&self) -> &'static str {
        const SPINNER_CHARS: &[&str] = &["·", "✻", "✽", "✶", "✳", "✢"];
        SPINNER_CHARS[self.session_spinner_frame % SPINNER_CHARS.len()]
    }

    fn open_timeline_dialog(&mut self) {
        self.reset_esc_timeline_state();

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
        let return_focus = if self.overlay_focus == OverlayFocus::TimelineDialog {
            OverlayFocus::TimelineDialog
        } else {
            OverlayFocus::None
        };
        self.show_message_actions_from(idx, return_focus);
    }

    fn message_actions_index_at_position(
        &self,
        mouse: MouseEvent,
        chat_area: Rect,
    ) -> Option<usize> {
        self.chat_state
            .chat
            .message_index_at_position(mouse, chat_area)
            .filter(|idx| {
                self.chat_state
                    .chat
                    .messages
                    .get(*idx)
                    .is_some_and(|message| {
                        message.role != crate::session::types::MessageRole::Assistant
                    })
            })
    }

    fn show_message_actions_from(&mut self, idx: usize, return_focus: OverlayFocus) {
        use crate::ui::components::dialog::{Dialog, DialogItem};

        let can_undo = self.selected_message_can_undo(idx);
        self.message_actions_index = Some(idx);
        self.message_actions_return_focus = return_focus;

        let mut items = vec![
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
        ];

        if can_undo {
            items.push(DialogItem {
                id: "undo".to_string(),
                name: "Undo".to_string(),
                group: String::new(),
                description: "Remove messages from here onward".to_string(),
                tip: None,
                provider_id: "undo".to_string(),
            });
        }

        let mut dialog = Dialog::with_items("Message Actions", items);
        dialog.show();
        self.message_actions_dialog = Some(dialog);
        self.overlay_focus = OverlayFocus::MessageActions;
    }

    fn selected_message_can_undo(&self, idx: usize) -> bool {
        let Some(session_id) = self.session_manager.get_current_session_id() else {
            return false;
        };

        self.session_manager
            .get_session_ref(session_id)
            .and_then(|session| session.messages.get(idx))
            .map(|message| message.role == crate::session::types::MessageRole::User)
            .unwrap_or(false)
    }

    fn execute_message_action(&mut self, action: &str) {
        let idx = match self.message_actions_index {
            Some(i) => i,
            None => return,
        };

        match action {
            "copy" => {
                let copy_text = self
                    .session_manager
                    .get_current_session()
                    .and_then(|session| {
                        crate::session::types::logical_message_block_range(&session.messages, idx)
                            .map(|range| message_block_clipboard_text(&session.messages, range))
                    });

                if let Some(text) = copy_text {
                    let _ = crate::utils::clipboard::copy_text(&text);
                    push_toast(Toast::new("Copied to clipboard", ToastLevel::Info, None));
                }
                self.close_message_actions();
            }
            "fork" => {
                let messages_to_fork: Vec<crate::session::types::Message> = {
                    if let Some(session) = self.session_manager.get_current_session() {
                        let end = crate::session::types::logical_message_block_range(
                            &session.messages,
                            idx,
                        )
                        .map(|range| range.end)
                        .unwrap_or_else(|| idx.saturating_add(1).min(session.messages.len()));

                        session.messages.iter().take(end).cloned().collect()
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

                let _ = self.create_new_session(Some(fork_title));
                for msg in &messages_to_fork {
                    let _ = self.session_manager.add_message_to_current_session(msg);
                }

                self.chat_state.chat.clear();
                self.chat_state.chat.replace_messages(messages_to_fork);
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
                if !self.selected_message_can_undo(idx) {
                    self.close_message_actions();
                    return;
                }

                let undone_message: Option<crate::session::types::Message> = {
                    if let Some(session) = self.session_manager.get_current_session() {
                        let message = session.messages.get(idx).cloned();
                        session.messages.truncate(idx);
                        message
                    } else {
                        return;
                    }
                };

                let remaining: Vec<crate::session::types::Message> = {
                    if let Some(session) = self.session_manager.get_current_session() {
                        session.messages.clone()
                    } else {
                        return;
                    }
                };

                self.chat_state.chat.replace_messages(remaining);
                self.chat_state.chat.scroll_offset = usize::MAX;
                self.chat_state.chat.clear_highlighted_message();

                if let Some(message) = undone_message {
                    let image_paths = message
                        .local_image_paths
                        .iter()
                        .map(std::path::PathBuf::from)
                        .collect();
                    self.input
                        .set_text_with_local_images(&message.content, image_paths);
                }

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
        let return_focus = self.message_actions_return_focus;
        self.message_actions_return_focus = OverlayFocus::TimelineDialog;
        if return_focus == OverlayFocus::None {
            self.chat_state.chat.clear_highlighted_message();
        }
        self.overlay_focus = return_focus;
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

        let include_ollama = connected_providers.contains_key(crate::model::ollama::PROVIDER_ID)
            || self.provider_name == crate::model::ollama::PROVIDER_ID
            || self
                .models_dialog_state
                .dialog
                .items
                .iter()
                .any(|item| item.provider_id == crate::model::ollama::PROVIDER_ID);

        if connected_providers.is_empty() && !include_ollama {
            return;
        }

        let has_non_ollama = connected_providers
            .keys()
            .any(|provider_id| !crate::model::ollama::is_ollama_provider(provider_id));

        let models = if has_non_ollama {
            match Discovery::new() {
                Ok(discovery) => match tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(discovery.fetch_models())
                }) {
                    Ok(models) => models,
                    Err(err) if include_ollama => {
                        let ollama_models = tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(crate::model::ollama::models_for_dialog_cached_or_empty())
                        });
                        if ollama_models.is_empty() {
                            push_toast(Toast::new(
                                format!("Failed to refresh models: {}", err),
                                ToastLevel::Warning,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                        }
                        ollama_models
                    }
                    Err(_) => return,
                },
                Err(_) if include_ollama => tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(crate::model::ollama::models_for_dialog_cached_or_empty())
                }),
                Err(_) => return,
            }
        } else if include_ollama {
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(crate::model::ollama::models_for_dialog_cached_or_empty())
            })
        } else {
            return;
        };

        let prefs = self
            .prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_preferences().ok());

        let mut model_lookup: std::collections::HashMap<(String, String), ModelType> =
            std::collections::HashMap::new();

        for model in &models {
            if connected_providers.contains_key(&model.provider_id)
                || crate::model::ollama::is_ollama_provider(&model.provider_id)
            {
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
            let is_active = self.model == model.id && self.provider_name == model.provider_id;
            let is_favorite =
                favorites_set.contains(&(model.provider_id.clone(), model.id.clone()));

            let tip = if is_active {
                Some("Active".to_string())
            } else if is_favorite {
                Some("❤︎".to_string())
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

            if connected_providers.contains_key(&model.provider_id)
                || crate::model::ollama::is_ollama_provider(&model.provider_id)
            {
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

    fn show_models_dialog(
        &mut self,
        title: impl Into<String>,
        mut items: Vec<crate::ui::components::dialog::DialogItem>,
    ) {
        for item in &mut items {
            let is_active = item.id == self.model && item.provider_id == self.provider_name;
            if is_active {
                item.tip = Some("Active".to_string());
            } else if item.tip.as_deref() == Some("Active") {
                item.tip = None;
            }
        }

        self.models_dialog_state = init_models_dialog(title, items);
        self.models_dialog_state.dialog.show();
        let _ = self
            .models_dialog_state
            .dialog
            .select_item_by_key(&self.model, &self.provider_name);
        self.overlay_focus = OverlayFocus::ModelsDialog;
    }

    fn focus_current_session_or_workspace_in_sessions_dialog(&mut self) {
        if let Some(session_id) = self.session_manager.get_current_session_id().cloned() {
            if self
                .sessions_dialog_state
                .dialog
                .select_item_by_id(&session_id)
            {
                return;
            }
        }

        let current_workspace_id = self.session_manager.current_workspace_id();
        let _ = self
            .sessions_dialog_state
            .focus_workspace(current_workspace_id);
    }

    fn open_sessions_dialog(&mut self) {
        self.refresh_sessions_dialog();
        self.focus_current_session_or_workspace_in_sessions_dialog();

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
                if crate::model::ollama::is_ollama_provider(&selected_item.id) {
                    self.connect_local_ollama();
                    return;
                }

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

    fn connect_local_ollama(&mut self) {
        let models_result = tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(crate::model::ollama::refresh_model_cache())
        });

        let models = match models_result {
            Ok(models) => models,
            Err(err) => {
                push_toast(Toast::new(
                    format!("Failed to connect Ollama: {}", err),
                    ToastLevel::Error,
                    Some(std::time::Duration::from_secs(5)),
                ));
                self.overlay_focus = OverlayFocus::None;
                return;
            }
        };

        match crate::persistence::AuthDAO::new().and_then(|dao| {
            dao.set_provider(
                crate::model::ollama::PROVIDER_ID.to_string(),
                crate::persistence::AuthConfig::Local,
            )
        }) {
            Ok(()) => {
                push_toast(Toast::new(
                    format!("Connected Ollama ({} local models)", models.len()),
                    ToastLevel::Success,
                    None,
                ));
                self.connect_dialog_state = init_connect_dialog();
                self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
            }
            Err(err) => {
                push_toast(Toast::new(
                    format!("Failed to save Ollama connection: {}", err),
                    ToastLevel::Error,
                    None,
                ));
            }
        }

        self.overlay_focus = OverlayFocus::None;
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

    fn process_compaction_events(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;

        if let Some(receiver) = &mut self.compaction_receiver {
            loop {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if disconnected || !events.is_empty() {
            self.compaction_receiver = None;
            self.compaction_pending = None;
            self.cached_usage_check = (usize::MAX, u64::MAX);
        }

        for event in events {
            match event {
                CompactionTaskMessage::Success {
                    session_id,
                    messages,
                    stats,
                } => {
                    match self
                        .session_manager
                        .replace_session_messages(&session_id, messages.clone())
                    {
                        Ok(()) => {
                            let is_active = self.is_active_session(&session_id);
                            if is_active {
                                self.chat_state.chat = Chat::with_messages(messages.clone());
                                self.chat_state.chat.scroll_to_bottom_on_next_render();
                                self.chat_state.chat.clear_highlighted_message();
                            }

                            self.ensure_session_view_state(&session_id);
                            if let Some(state) = self.session_view_states.get_mut(&session_id) {
                                state.chat = if is_active {
                                    Chat::new()
                                } else {
                                    Chat::with_messages(messages)
                                };
                                state.tool_calls = ToolCallViewState::default();
                                state.unread_completed = !is_active;
                            }

                            let _ = self.session_manager.set_session_status(
                                &session_id,
                                crate::session::types::SessionStatus::Idle,
                                None,
                            );
                            self.cached_usage_check = (usize::MAX, u64::MAX);
                            self.refresh_sessions_dialog();
                            push_toast(Toast::new(
                                format!(
                                    "Session compacted: {}",
                                    crate::session::compaction::format_compaction_stats(stats)
                                ),
                                ToastLevel::Info,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                        }
                        Err(err) => {
                            let _ = self.session_manager.set_session_status(
                                &session_id,
                                crate::session::types::SessionStatus::Idle,
                                None,
                            );
                            self.play_sound_event(crate::sound::SoundEvent::Error);
                            push_toast(Toast::new(
                                format!("Failed to save compacted session: {:?}", err),
                                ToastLevel::Error,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                        }
                    }
                }
                CompactionTaskMessage::Failed { session_id, error } => {
                    let _ = self.session_manager.set_session_status(
                        &session_id,
                        crate::session::types::SessionStatus::Idle,
                        None,
                    );
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        format!("Failed to compact session: {}", error),
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                }
            }
        }

        self.sync_active_streaming_flag();
    }

    fn process_storage_events(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;

        if let Some(receiver) = &mut self.storage_receiver {
            loop {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if disconnected || !events.is_empty() {
            self.storage_receiver = None;
        }

        if disconnected && events.is_empty() {
            self.storage_dialog_state
                .set_error("storage check ended before returning results");
            return;
        }

        for event in events {
            match event {
                StorageTaskMessage::Loaded(report) => {
                    self.storage_dialog_state.set_report(report);
                }
            }
        }
    }

    fn cleanup_streaming(&mut self) {
        if let Some(session_id) = self.session_manager.get_current_session_id().cloned() {
            self.cleanup_streaming_for_session(&session_id);
        }
    }

    fn cleanup_streaming_for_session(&mut self, session_id: &str) {
        let was_active = self.is_active_session(session_id);

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            state.stream = None;
            state.external_stream = None;
            state.tool_calls.deferred_finish = false;
        }

        if was_active {
            self.chat_state.chat.resume_streaming_tps_timer();
            if self.overlay_focus == OverlayFocus::PermissionDialog {
                self.permission_dialog_state.clear_with_deny();
                self.overlay_focus = OverlayFocus::None;
            }
            if self.overlay_focus == OverlayFocus::QuestionDialog {
                self.question_dialog_state.clear_with_empty();
                self.overlay_focus = OverlayFocus::None;
            }
        }

        self.sync_active_streaming_flag();
    }

    fn cancel_streaming(&mut self) {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            return;
        };

        self.cancel_streaming_for_session(&session_id);
    }

    fn cancel_streaming_for_session(&mut self, session_id: &str) {
        if let Some(stream) = self.stream_for_session_mut(&session_id) {
            stream.cancel_token.cancel();
        }
    }

    fn interrupt_streaming_to_send_queued_for_session(&mut self, session_id: &str) -> bool {
        if !self.is_active_session(session_id)
            || !self.has_queued_messages_for_session(session_id)
            || !self.session_has_active_stream(session_id)
        {
            return false;
        }

        self.cancel_streaming_for_session(session_id);
        self.mark_streamed_assistant_interrupted(session_id);
        let _ = self.finalize_and_persist_streamed_messages(
            session_id,
            Some("Streaming interrupted to send queued messages"),
        );
        let _ = self.session_manager.set_session_status(
            session_id,
            crate::session::types::SessionStatus::Interrupted,
            None,
        );
        self.cleanup_streaming_for_session(session_id);
        self.submit_queued_messages_for_session(session_id)
    }

    pub fn update_animations(&mut self) {
        // Only update animations at 20fps (50ms intervals) regardless of render rate
        const ANIMATION_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
        const SESSION_SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(160);

        if self.last_animation_update.elapsed() >= ANIMATION_INTERVAL {
            self.chat_state.wave_spinner.update();
            self.home_state.tick();
            if self.tick_selection_edge_scroll() {
                self.selection_action_bar = None;
                self.pending_chat_message_click = None;
                self.update_suggestions();
            }
            self.last_animation_update = std::time::Instant::now();
        }

        if self.last_session_spinner_update.elapsed() >= SESSION_SPINNER_INTERVAL {
            self.session_spinner_frame = (self.session_spinner_frame + 1) % 6;
            self.last_session_spinner_update = std::time::Instant::now();
        }
    }

    pub fn is_animation_running(&self) -> bool {
        self.base_focus == BaseFocus::Home
            || self.has_active_selection_edge_scroll()
            || self.is_streaming
            || self.chat_state.chat.has_active_tool_messages()
            || self.compaction_receiver.is_some()
            || self.storage_receiver.is_some()
            || self
                .session_view_states
                .values()
                .any(|state| state.stream.is_some() || state.external_stream.is_some())
            || (self.overlay_focus == OverlayFocus::SessionsDialog
                && self.sessions_dialog_state.dialog.is_visible())
    }

    fn has_active_selection_edge_scroll(&self) -> bool {
        self.input.has_active_selection_edge_scroll()
            || self.chat_state.chat.has_active_selection_edge_scroll()
    }

    fn tick_selection_edge_scroll(&mut self) -> bool {
        let input_scrolled = self.input.tick_selection_edge_scroll();
        let chat_scrolled = self.chat_state.chat.tick_selection_edge_scroll();
        input_scrolled || chat_scrolled
    }

    pub fn process_streaming_chunks(&mut self) {
        self.process_openai_oauth_events();
        self.process_compaction_events();
        self.process_storage_events();

        let streaming_ids: Vec<String> = self
            .session_view_states
            .iter()
            .filter_map(|(id, state)| state.stream.as_ref().map(|_| id.clone()))
            .collect();

        for session_id in streaming_ids {
            let mut chunks = Vec::new();
            let mut disconnected = false;

            if let Some(stream) = self.stream_for_session_mut(&session_id) {
                loop {
                    match stream.chunk_receiver.try_recv() {
                        Ok(chunk) => chunks.push(chunk),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            let mut keep_current_stream = true;
            for chunk in chunks {
                if !self.process_streaming_chunk_for_session(&session_id, chunk) {
                    keep_current_stream = false;
                    break;
                }
            }

            if !keep_current_stream {
                disconnected = false;
            }

            if disconnected
                && self
                    .session_view_states
                    .get(&session_id)
                    .is_some_and(|state| state.stream.is_some())
            {
                crate::emit_log!(
                    "[STREAM_DISCONNECTED] session_id={} reason=stream_receiver_disconnected_without_terminal_chunk",
                    session_id
                );
                self.fail_streaming_session(
                    &session_id,
                    "Stream task ended before sending a completion event".to_string(),
                );
            }
        }

        self.sync_active_streaming_flag();

        if self.overlay_focus == OverlayFocus::SessionsDialog
            && self.sessions_dialog_state.dialog.is_visible()
            && !self.sessions_dialog_state.dialog.is_dragging_scrollbar
        {
            self.refresh_sessions_dialog();
        }
    }

    fn process_streaming_chunk_for_session(
        &mut self,
        session_id: &str,
        chunk: crate::llm::ChunkMessage,
    ) -> bool {
        match chunk {
            crate::llm::ChunkMessage::Text(text) => {
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.append_to_last_assistant(&text);
                }
                true
            }
            crate::llm::ChunkMessage::Reasoning(reasoning) => {
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.append_reasoning_to_last_assistant(&reasoning);
                }
                true
            }
            crate::llm::ChunkMessage::Warning(msg) => {
                push_toast(Toast::new(msg, ToastLevel::Warning, None));
                true
            }
            crate::llm::ChunkMessage::End => {
                self.finish_streaming_session(session_id);
                false
            }
            crate::llm::ChunkMessage::Failed(error) => {
                self.fail_streaming_session(session_id, error);
                false
            }
            crate::llm::ChunkMessage::Cancelled => {
                self.cancelled_streaming_session(session_id);
                false
            }
            crate::llm::ChunkMessage::Metrics { .. } => true,
            crate::llm::ChunkMessage::ToolCalls(tool_calls) => {
                self.add_tool_calls_to_session(session_id, tool_calls);
                true
            }
            crate::llm::ChunkMessage::ToolResult(result) => {
                self.add_tool_result_to_session(session_id, result)
            }
            crate::llm::ChunkMessage::SubagentStarted {
                parent_session_id,
                session_id,
                title,
                subagent_type,
                model,
                provider,
                description,
                prompt,
            } => {
                self.start_subagent_session(
                    parent_session_id,
                    session_id,
                    title,
                    subagent_type,
                    model,
                    provider,
                    description,
                    prompt,
                );
                true
            }
            crate::llm::ChunkMessage::SubagentChunk { session_id, chunk } => {
                let _ = self.process_streaming_chunk_for_session(&session_id, *chunk);
                true
            }
            crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                let _ = self.session_manager.set_session_status(
                    session_id,
                    crate::session::types::SessionStatus::Waiting,
                    None,
                );
                if !self.is_active_session(session_id) {
                    let _ = self.switch_to_session(session_id);
                }
                self.play_sound_event(crate::sound::SoundEvent::Permission);
                self.notify_terminal_event(crate::sound::SoundEvent::Permission);
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.pause_streaming_tps_timer();
                }
                self.permission_dialog_state.enqueue(prompt);
                self.overlay_focus = OverlayFocus::PermissionDialog;
                true
            }
            crate::llm::ChunkMessage::QuestionRequest {
                questions,
                response_tx,
            } => {
                let _ = self.session_manager.set_session_status(
                    session_id,
                    crate::session::types::SessionStatus::Waiting,
                    None,
                );
                if !self.is_active_session(session_id) {
                    let _ = self.switch_to_session(session_id);
                }
                self.play_sound_event(crate::sound::SoundEvent::Question);
                self.notify_terminal_event(crate::sound::SoundEvent::Question);
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.pause_streaming_tps_timer();
                }
                self.question_dialog_state.enqueue(questions, response_tx);
                self.overlay_focus = OverlayFocus::QuestionDialog;
                true
            }
        }
    }

    fn start_subagent_session(
        &mut self,
        parent_session_id: String,
        session_id: String,
        title: String,
        subagent_type: String,
        model: Option<String>,
        provider: Option<String>,
        description: String,
        prompt: String,
    ) {
        if self.session_manager.get_session_ref(&session_id).is_none() {
            self.session_manager.create_child_session(
                parent_session_id,
                session_id.clone(),
                title.clone(),
            );
        }

        self.ensure_session_view_state(&session_id);

        let user_content = format!(
            "## Task Description\n{}\n\n## Task Prompt\n{}",
            description, prompt
        );

        let mut user_message = crate::session::types::Message::user(&user_content);
        user_message.agent_mode = Some(subagent_type.clone());
        user_message.model = model.clone();
        user_message.provider = provider.clone();

        let mut persist_user = false;
        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            state.chat = Chat::with_messages(Vec::new());
            state.tool_calls = ToolCallViewState::default();
            state.chat.add_message(user_message.clone());
            state.chat.add_assistant_message("");
            if let Some(last_msg) = state.chat.messages.last_mut() {
                last_msg.is_complete = false;
                last_msg.agent_mode = Some(subagent_type);
                last_msg.model = model.clone();
                last_msg.provider = provider.clone();
            }
            state.chat.mark_render_dirty();
            state.chat.begin_streaming_turn();
            state.external_stream = Some(ExternalStreamState {
                streaming_model: model.or_else(|| Some(self.model.clone())),
                streaming_provider: provider.or_else(|| Some(self.provider_name.clone())),
                chat_len_before_assistant: 1,
            });
            state.unread_completed = true;
            persist_user = true;
        }

        if persist_user {
            let _ = self
                .session_manager
                .add_message_to_session(&session_id, &user_message);
        }

        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );

        self.refresh_sessions_dialog();
        self.sync_active_streaming_flag();
    }

    fn finish_streaming_session(&mut self, session_id: &str) {
        if self.defer_finish_if_tools_are_running(session_id) {
            return;
        }

        let Some(completion_stats) = self.finalize_and_persist_streamed_messages(session_id, None)
        else {
            return;
        };

        let _ = self.session_manager.set_session_status(
            session_id,
            crate::session::types::SessionStatus::Idle,
            None,
        );

        if !self.is_active_session(session_id) {
            if let Some(state) = self.session_view_states.get_mut(session_id) {
                state.unread_completed = true;
            }
        }

        self.cleanup_streaming_for_session(session_id);
        if self.submit_queued_messages_for_session(session_id) {
            return;
        }
        self.play_sound_event_with_notification_detail(
            crate::sound::SoundEvent::Complete,
            completion_stats.as_deref(),
        );
        self.notify_terminal_event(crate::sound::SoundEvent::Complete);
    }

    fn defer_finish_if_tools_are_running(&mut self, session_id: &str) -> bool {
        if !self.session_has_running_tool_messages(session_id) {
            return false;
        }

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            state.tool_calls.deferred_finish = true;
        }

        crate::emit_log!(
            "[STREAM_DEFERRED] session_id={} reason=running_tool_messages",
            session_id
        );
        true
    }

    fn finish_deferred_streaming_session_if_ready(&mut self, session_id: &str) -> bool {
        let deferred = self
            .session_view_states
            .get(session_id)
            .is_some_and(|state| state.tool_calls.deferred_finish);

        if !deferred || self.session_has_running_tool_messages(session_id) {
            return false;
        }

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            state.tool_calls.deferred_finish = false;
        }

        self.finish_streaming_session(session_id);
        true
    }

    fn session_has_running_tool_messages(&self, session_id: &str) -> bool {
        let Some((start, _, _)) = self.streaming_boundary_for_session(session_id) else {
            return false;
        };
        let Some(chat) = self.chat_for_session(session_id) else {
            return false;
        };

        chat.messages
            .iter()
            .skip(start)
            .any(Self::is_running_tool_message)
    }

    fn is_running_tool_message(message: &crate::session::types::Message) -> bool {
        if message.role != crate::session::types::MessageRole::Tool {
            return false;
        }

        serde_json::from_str::<serde_json::Value>(&message.content)
            .ok()
            .and_then(|value| {
                value
                    .get("status")
                    .and_then(|status| status.as_str())
                    .map(|status| status == "running")
            })
            .unwrap_or(true)
    }

    fn finalize_and_persist_streamed_messages(
        &mut self,
        session_id: &str,
        terminal_error: Option<&str>,
    ) -> Option<Option<String>> {
        let (start, model, provider) = self.streaming_boundary_for_session(session_id)?;
        let mut messages_to_persist = Vec::new();
        let completion_stats = if let Some(chat) = self.chat_for_session_mut(session_id) {
            chat.mark_streaming_end();
            chat.finalize_streaming_metrics();

            if let Some(error) = terminal_error {
                Self::mark_running_tool_messages_failed(chat, start, error);
            }

            for msg in chat.messages.iter_mut().skip(start) {
                match msg.role {
                    crate::session::types::MessageRole::Assistant => {
                        if !msg.is_complete {
                            msg.mark_complete();
                        }
                        msg.model = model.clone();
                        msg.provider = provider.clone();
                        messages_to_persist.push(msg.clone());
                    }
                    crate::session::types::MessageRole::Tool => {
                        messages_to_persist.push(msg.clone());
                    }
                    _ => {}
                }
            }
            chat.mark_render_dirty();

            Self::completion_notification_stats_for_chat(chat)
        } else {
            None
        };

        for msg in &messages_to_persist {
            let _ = self.session_manager.add_message_to_session(session_id, msg);
        }

        Some(completion_stats)
    }

    fn mark_running_tool_messages_failed(chat: &mut Chat, start: usize, error: &str) {
        for msg in chat.messages.iter_mut().skip(start) {
            if msg.role != crate::session::types::MessageRole::Tool {
                continue;
            }

            let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&msg.content) else {
                continue;
            };

            let is_running = value
                .get("status")
                .and_then(|status| status.as_str())
                .map(|status| status == "running")
                .unwrap_or(true);

            if !is_running {
                continue;
            }

            value["status"] = serde_json::Value::String("error".to_string());
            value["title"] = serde_json::Value::String("Tool failed".to_string());
            value["output_preview"] = serde_json::Value::String(error.to_string());
            msg.content = value.to_string();
        }
    }

    fn mark_streamed_assistant_interrupted(&mut self, session_id: &str) {
        let Some((start, _, _)) = self.streaming_boundary_for_session(session_id) else {
            return;
        };

        if let Some(chat) = self.chat_for_session_mut(session_id) {
            for idx in (start..chat.messages.len()).rev() {
                if chat.messages[idx].role == crate::session::types::MessageRole::Assistant {
                    chat.messages[idx].mark_interrupted();
                    chat.mark_render_dirty();
                    return;
                }
            }
        }
    }

    fn fail_streaming_session(&mut self, session_id: &str, error: String) {
        if self
            .finalize_and_persist_streamed_messages(session_id, Some(&error))
            .is_none()
        {
            return;
        }

        let _ = self.session_manager.set_session_status(
            session_id,
            crate::session::types::SessionStatus::Failed,
            Some(&error),
        );

        self.play_sound_event(crate::sound::SoundEvent::Error);
        push_toast(Toast::new(
            format!("LLM error: {}", error),
            ToastLevel::Error,
            None,
        ));
        self.cleanup_streaming_for_session(session_id);
        self.submit_queued_messages_for_session(session_id);
    }

    fn cancelled_streaming_session(&mut self, session_id: &str) {
        self.mark_streamed_assistant_interrupted(session_id);

        if self
            .finalize_and_persist_streamed_messages(session_id, Some("Streaming cancelled by user"))
            .is_none()
        {
            return;
        }

        let _ = self.session_manager.set_session_status(
            session_id,
            crate::session::types::SessionStatus::Interrupted,
            None,
        );

        push_toast(Toast::new("Streaming cancelled", ToastLevel::Info, None));
        self.cleanup_streaming_for_session(session_id);
        self.submit_queued_messages_for_session(session_id);
    }

    fn add_tool_calls_to_session(
        &mut self,
        session_id: &str,
        tool_calls: Vec<crate::llm::ToolCall>,
    ) {
        let mut inserted = Vec::new();

        if let Some(chat) = self.chat_for_session_mut(session_id) {
            if let Some(idx) = chat
                .messages
                .iter()
                .rposition(|m| m.role == crate::session::types::MessageRole::Assistant)
            {
                if let Some(msg) = chat.messages.get_mut(idx) {
                    if !msg.is_complete {
                        msg.mark_complete();
                        chat.mark_render_dirty();
                    }
                }
            }

            for call in tool_calls {
                let args_value: serde_json::Value = serde_json::from_str(&call.function.arguments)
                    .unwrap_or_else(|_| serde_json::Value::String(call.function.arguments.clone()));

                let call_id = call.id.clone();
                let content = serde_json::json!({
                    "id": call.id,
                    "name": call.function.name,
                    "status": "running",
                    "args": args_value,
                })
                .to_string();

                chat.add_message(crate::session::types::Message::tool(content));

                let idx = chat.messages.len().saturating_sub(1);
                inserted.push((call_id, idx));
            }
        }

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            for (call_id, idx) in inserted {
                state
                    .tool_calls
                    .tool_call_message_indices
                    .insert(call_id.clone(), idx);
                state.tool_calls.tool_call_order.push(call_id);
            }
        }
    }

    fn add_tool_result_to_session(
        &mut self,
        session_id: &str,
        result: crate::llm::ToolCallResult,
    ) -> bool {
        let target_idx = self.session_view_states.get(session_id).and_then(|state| {
            state
                .tool_calls
                .tool_call_message_indices
                .get(&result.tool_call_id)
                .copied()
        });

        let mut handled = false;

        if let Some(chat) = self.chat_for_session_mut(session_id) {
            if let Some(idx) = target_idx {
                if let Some(msg) = chat.messages.get_mut(idx) {
                    let mut v: serde_json::Value = serde_json::from_str(&msg.content)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    v["id"] = serde_json::Value::String(result.tool_call_id.clone());
                    v["name"] = serde_json::Value::String(result.name.clone());

                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&result.content)
                    {
                        if payload.is_object() {
                            if v.get("status").is_none() {
                                v["status"] = payload
                                    .get("status")
                                    .cloned()
                                    .unwrap_or_else(|| serde_json::Value::String("ok".to_string()));
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
                            v["output_preview"] = serde_json::Value::String(result.content.clone());
                        }
                    } else {
                        let status = if result.content.trim_start().starts_with("Error:") {
                            "error"
                        } else {
                            "ok"
                        };
                        v["status"] = serde_json::Value::String(status.to_string());
                        v["output_preview"] = serde_json::Value::String(result.content.clone());
                    }

                    msg.content = v.to_string();
                    chat.mark_render_dirty();
                    handled = true;
                }
            }

            if !handled {
                let content = serde_json::json!({
                    "id": result.tool_call_id,
                    "name": result.name,
                    "status": "ok",
                    "output_preview": result.content,
                })
                .to_string();
                chat.add_message(crate::session::types::Message::tool(content));
            }
        }

        if self.finish_deferred_streaming_session_if_ready(session_id) {
            return false;
        }
        if self.session_has_active_stream(session_id)
            && self.has_queued_messages_for_session(session_id)
            && !self.session_has_running_tool_messages(session_id)
        {
            return !self.interrupt_streaming_to_send_queued_for_session(session_id);
        }
        true
    }

    fn start_llm_streaming(
        &mut self,
        _user_message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::sync::mpsc;

        let session_id = self
            .session_manager
            .get_current_session_id()
            .cloned()
            .ok_or_else(|| "No active session".to_string())?;
        self.ensure_session_view_state(&session_id);

        let (sender, receiver) = mpsc::unbounded_channel();
        let sender_clone = sender.clone();

        let cancel_token = tokio_util::sync::CancellationToken::new();

        self.is_streaming = true;

        // Track the message boundary for this streaming turn so terminal paths
        // can persist or roll back only the assistant/tool messages from this turn.
        let chat_len_before_assistant = self.chat_state.chat.messages.len();

        // Capture the current model and provider at the start of streaming
        // so they don't change if the user switches models during streaming
        let streaming_model = Some(self.model.clone());
        let streaming_provider = Some(self.provider_name.clone());
        self.chat_state
            .chat
            .prepare_streaming_token_counter(&self.model);

        self.chat_state.chat.add_assistant_message("");
        if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
            last_msg.is_complete = false;
        }
        self.chat_state.chat.mark_render_dirty();

        // Initialize per-turn streaming timing primitives (T0).
        self.chat_state.chat.begin_streaming_turn();

        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            state.stream = Some(SessionStreamState {
                chunk_receiver: receiver,
                cancel_token: cancel_token.clone(),
                streaming_model: streaming_model.clone(),
                streaming_provider: streaming_provider.clone(),
                chat_len_before_assistant,
            });
            state.tool_calls = ToolCallViewState::default();
            state.unread_completed = false;
        }
        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let reasoning_effort = self.active_reasoning_effort();
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
        let agent_registry = self.agent_registry.clone();
        let cwd = self.cwd.clone();
        let is_git_repo = crate::utils::git::is_git_repo(&cwd).unwrap_or(false);

        // Build messages with system prompt
        let mut messages = self.chat_state.chat.messages.clone();

        // Check if we already have a system message
        let has_system = messages
            .iter()
            .any(|m| m.role == crate::session::types::MessageRole::System);

        if !has_system {
            let prompt_registry = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let registry = crate::tools::initialize_tool_registry_with_dynamic(
                        Some(sender.clone()),
                        tool_permissions.clone(),
                        agent_registry.clone(),
                        cancel_token.clone(),
                    )
                    .await;
                    crate::tools::scope_tool_registry_for_agent(
                        &registry,
                        &tool_permissions,
                        &agent_mode,
                    )
                    .await
                })
            });

            // Create system prompt with tools
            let composer = crate::prompt::SystemPromptComposer::new(
                &model,
                &cwd,
                is_git_repo,
                std::env::consts::OS,
            )
            .with_tool_registry(prompt_registry)
            .with_agent_registry(agent_registry.clone());
            let system_prompt = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async { composer.compose().await })
            });
            let system_msg = crate::session::types::Message::system(system_prompt);
            messages.insert(0, system_msg);
        }

        tokio::spawn(async move {
            let stream = stream_llm_with_cancellation(
                cancel_token,
                session_id,
                provider_name,
                model,
                reasoning_effort,
                agent_mode,
                agent_max_steps,
                agent_registry,
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
        self.handle_message_input_with_images(msg, Vec::new());
    }

    fn handle_agent_mention_input(
        &mut self,
        mention: crate::command::parser::ParsedAgentMention,
        image_paths: Vec<std::path::PathBuf>,
    ) {
        if image_paths.is_empty() && mention.prompt.trim().is_empty() {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                format!("Usage: @{} <task>", mention.agent),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        let Some(agent) = self.agent_registry.task_target(&mention.agent).cloned() else {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            let available = self
                .agent_registry
                .visible_agent_names_for_mentions()
                .join(", ");
            let suffix = if available.is_empty() {
                String::new()
            } else {
                format!(" Available agents: {}", available)
            };
            push_toast(Toast::new(
                format!("Unknown agent: @{}.{}", mention.agent, suffix),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(4)),
            ));
            return;
        };

        if !agent.visible_subagent() {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                format!(
                    "Agent @{} is not available for direct mention",
                    mention.agent
                ),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        if !self
            .agent_registry
            .can_agent_invoke(&self.agent, &agent.name)
        {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                format!("{} cannot invoke @{}", self.agent, agent.name),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        if self.base_focus == BaseFocus::Home
            && self.session_manager.get_current_session_id().is_none()
        {
            let session_title = self
                .pending_session_title
                .take()
                .unwrap_or_else(|| Self::generate_title_from_message(&mention.raw));
            self.create_new_session(Some(session_title));
        }

        if self.session_manager.get_current_session_id().is_none() {
            self.create_new_session(Some(Self::generate_title_from_message(&mention.raw)));
        }

        self.append_user_message_to_current_session(mention.raw.clone(), image_paths);
        self.base_focus = BaseFocus::Chat;

        if let Err(err) = self.start_agent_mention_task(agent.name, mention.prompt) {
            push_toast(Toast::new(
                format!("Agent error: {}", err),
                ToastLevel::Error,
                None,
            ));
        }
    }

    fn start_agent_mention_task(
        &mut self,
        agent_name: String,
        prompt: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::sync::mpsc;

        let session_id = self
            .session_manager
            .get_current_session_id()
            .cloned()
            .ok_or_else(|| "No active session".to_string())?;
        self.ensure_session_view_state(&session_id);

        let (sender, receiver) = mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.is_streaming = true;

        let chat_len_before_assistant = self.chat_state.chat.messages.len();
        let streaming_model = Some(self.model.clone());
        let streaming_provider = Some(self.provider_name.clone());
        self.chat_state
            .chat
            .prepare_streaming_token_counter(&self.model);
        self.chat_state.chat.add_assistant_message("");
        if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
            last_msg.is_complete = false;
        }
        self.chat_state.chat.mark_render_dirty();
        self.chat_state.chat.begin_streaming_turn();

        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            state.stream = Some(SessionStreamState {
                chunk_receiver: receiver,
                cancel_token: cancel_token.clone(),
                streaming_model,
                streaming_provider,
                chat_len_before_assistant,
            });
            state.tool_calls = ToolCallViewState::default();
            state.unread_completed = false;
        }
        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let reasoning_effort = self.active_reasoning_effort();
        let parent_agent = self.agent.clone();
        let tool_permissions = self.tool_permissions.clone();
        let agent_registry = self.agent_registry.clone();
        let task_description = format!("{} mention", agent_name);
        let sender_for_error = sender.clone();

        tokio::spawn(async move {
            let result = async {
                crate::llm::client::configure_subagent_llm_session(
                    &provider_name,
                    model,
                    reasoning_effort,
                    &sender,
                )
                .await
                .map_err(|err| err.to_string())?;

                let registry = crate::tools::initialize_tool_registry_with_dynamic(
                    Some(sender.clone()),
                    tool_permissions.clone(),
                    agent_registry.clone(),
                    cancel_token.clone(),
                )
                .await;
                let task = crate::tools::TaskTool::new(registry)
                    .with_sender_opt(Some(sender.clone()))
                    .with_runtime_options(tool_permissions, agent_registry, cancel_token.clone());
                let params = serde_json::json!({
                    "subagent_type": agent_name,
                    "description": task_description,
                    "prompt": prompt,
                });
                let ctx = crate::tools::ToolContext::from_cancel_token(
                    session_id.clone(),
                    "agent-mention",
                    parent_agent,
                    cancel_token,
                );

                task.execute(params, &ctx)
                    .await
                    .map_err(|err| err.to_string())
            }
            .await;

            match result {
                Ok(tool_result) => {
                    let _ = sender.send(crate::llm::ChunkMessage::Text(tool_result.output));
                    let _ = sender.send(crate::llm::ChunkMessage::End);
                }
                Err(err) => {
                    let _ = sender_for_error.send(crate::llm::ChunkMessage::Failed(err));
                }
            }
        });

        Ok(())
    }

    fn append_user_message_to_current_session(
        &mut self,
        msg: String,
        image_paths: Vec<std::path::PathBuf>,
    ) {
        let mut user_message = crate::session::types::Message::user(&msg);
        user_message.local_image_paths = image_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        user_message.agent_mode = Some(self.agent.clone());
        user_message.model = Some(self.model.clone());
        user_message.provider = Some(self.provider_name.clone());
        let _ = self
            .session_manager
            .add_message_to_current_session(&user_message);
        self.chat_state.chat.add_message(user_message);
        self.cached_usage_check = (usize::MAX, u64::MAX);
    }

    fn submit_queued_messages_for_session(&mut self, session_id: &str) -> bool {
        if !self.is_active_session(session_id) || self.session_has_active_stream(session_id) {
            return false;
        }

        let queued_messages = self.drain_queued_messages_for_session(session_id);
        if queued_messages.is_empty() {
            return false;
        }

        self.base_focus = BaseFocus::Chat;
        let mut last_text = String::new();
        for queued in queued_messages {
            last_text = queued.text.clone();
            self.append_user_message_to_current_session(queued.text, queued.image_paths);
        }

        if let Err(e) = self.start_llm_streaming(&last_text) {
            push_toast(Toast::new(
                format!("LLM error: {}", e),
                ToastLevel::Error,
                None,
            ));
            return false;
        }

        true
    }

    fn run_custom_command_prompt(
        &mut self,
        prompt: String,
        agent: Option<String>,
        model: Option<String>,
        _subtask: Option<bool>,
    ) {
        if prompt.trim().is_empty() {
            return;
        }

        if self.is_streaming {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "Cannot run a custom command while streaming",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        let previous_agent = self.agent.clone();
        let previous_model = self.model.clone();
        let previous_provider = self.provider_name.clone();

        if let Some(agent) = agent.filter(|value| !value.trim().is_empty()) {
            self.agent = agent;
        }

        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            let (provider_id, model_id) = parse_model_ref(&model);
            self.provider_name = provider_id;
            self.model = model_id;
        }

        self.handle_message_input(prompt);

        self.agent = previous_agent;
        self.model = previous_model;
        self.provider_name = previous_provider;
    }

    fn handle_message_input_with_images(
        &mut self,
        msg: String,
        image_paths: Vec<std::path::PathBuf>,
    ) {
        if (!msg.is_empty() || !image_paths.is_empty()) && self.base_focus == BaseFocus::Home {
            if self.session_manager.get_current_session_id().is_none() {
                let session_title = self
                    .pending_session_title
                    .take()
                    .unwrap_or_else(|| Self::generate_title_from_message(&msg));
                self.create_new_session(Some(session_title));
            }
            self.append_user_message_to_current_session(msg.clone(), image_paths);
            self.base_focus = BaseFocus::Chat;

            if let Err(e) = self.start_llm_streaming(&msg) {
                push_toast(Toast::new(
                    format!("LLM error: {}", e),
                    ToastLevel::Error,
                    None,
                ));
            }
        } else if (!msg.is_empty() || !image_paths.is_empty()) && self.base_focus == BaseFocus::Chat
        {
            if let Some(session_id) = self.session_manager.get_current_session_id().cloned() {
                self.ensure_session_view_state(&session_id);
            }
            self.append_user_message_to_current_session(msg.clone(), image_paths);

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

        let fingerprint = (
            self.chat_state.chat.messages.len(),
            self.chat_state.chat.render_revision(),
        );
        if self.cached_usage_check != fingerprint {
            self.cached_usage_check = fingerprint;
            self.cached_usage_text = self.session_usage_text();
        }
        let status_cwd = self.active_workspace_path();
        let branch = self.current_git_branch(&status_cwd);
        let usage_text = &self.cached_usage_text;
        let reasoning_effort = self.active_reasoning_effort_label();

        match self.base_focus {
            BaseFocus::Home => {
                render_home(
                    f,
                    &mut self.input,
                    &self.home_state,
                    self.version.clone(),
                    status_cwd.clone(),
                    branch.clone(),
                    self.agent.clone(),
                    self.model.clone(),
                    self.provider_name.clone(),
                    reasoning_effort.clone(),
                    &colors,
                    &usage_text,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::ModelsDialog
                    && self.overlay_focus != OverlayFocus::ThemesDialog
                {
                    let anchor_area = self.suggestions_popup_anchor_area();
                    render_suggestions_popup(
                        f,
                        &self.suggestions_popup_state,
                        anchor_area,
                        self.overlay_focus == OverlayFocus::SuggestionsPopup,
                        colors,
                    );
                }
            }
            BaseFocus::Chat => {
                let subagent_tabs = self.subagent_tabs_for_current_session();
                let queued_messages = self.queued_message_previews_for_current_session();
                let (display_agent, display_model) = self.current_session_agent_model_for_display();
                render_chat(
                    f,
                    &mut self.chat_state,
                    &mut self.input,
                    self.version.clone(),
                    status_cwd.clone(),
                    branch,
                    display_agent,
                    display_model,
                    self.provider_name.clone(),
                    reasoning_effort,
                    &colors,
                    self.is_streaming,
                    self.compaction_receiver.is_some(),
                    &usage_text,
                    subagent_tabs,
                    &queued_messages,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::ModelsDialog
                    && self.overlay_focus != OverlayFocus::ThemesDialog
                {
                    let anchor_area = self.suggestions_popup_anchor_area();
                    render_suggestions_popup(
                        f,
                        &self.suggestions_popup_state,
                        anchor_area,
                        self.overlay_focus == OverlayFocus::SuggestionsPopup,
                        colors,
                    );
                }
            }
        }

        if self.overlay_focus == OverlayFocus::ModelsDialog
            && self.models_dialog_state.dialog.is_visible()
        {
            let reasoning_effort = self.selected_model_reasoning_control_label();
            render_models_dialog(
                f,
                &mut self.models_dialog_state,
                size,
                colors,
                reasoning_effort.as_deref(),
            );
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

        if self.overlay_focus == OverlayFocus::QuestionDialog
            && self.question_dialog_state.has_active()
        {
            render_question_dialog(f, &mut self.question_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::CommandPalette
            && self.command_palette_state.dialog.is_visible()
        {
            render_command_palette(f, &mut self.command_palette_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::StorageDialog
            && self.storage_dialog_state.is_visible()
        {
            render_storage_dialog(f, &mut self.storage_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::WhichKey {
            crate::views::which_key::render_which_key(f, &self.which_key_state, &colors);
        }

        if let Some(state) = self.selection_action_bar {
            let area = match state.target {
                SelectionActionTarget::Chat => chat_selection_action_bar_area(
                    self.chat_area_for_size(size),
                    self.chat_state.chat.scroll_offset,
                    &self.chat_state.chat.selection,
                ),
                SelectionActionTarget::Input => {
                    input_selection_action_bar_area(size, self.suggestions_popup_anchor_area())
                }
            };
            render_selection_action_bar(f, area, state.target, &colors);
        }

        toast::render_toasts(f, &get_toast_manager().lock().unwrap(), &colors);
    }
}

fn format_selection_prompt_addition(text: &str) -> String {
    let text = text.trim();
    if text.lines().count() <= 1 {
        format!("`{}`", text)
    } else {
        format!("```\n{}\n```", text)
    }
}

const SELECTION_ACTION_BAR_WIDTH: u16 = 28;
const CHAT_SELECTION_ACTION_COPY_COL: usize = 16;
const CHAT_SELECTION_ACTION_ESC_COL: usize = 23;
const INPUT_SELECTION_ACTION_ESC_COL: usize = 8;

fn selection_action_for_column(target: SelectionActionTarget, column: usize) -> SelectionAction {
    match target {
        SelectionActionTarget::Chat if column < CHAT_SELECTION_ACTION_COPY_COL => {
            SelectionAction::AddToPrompt
        }
        SelectionActionTarget::Chat if column < CHAT_SELECTION_ACTION_ESC_COL => {
            SelectionAction::Copy
        }
        SelectionActionTarget::Chat => SelectionAction::Dismiss,
        SelectionActionTarget::Input if column < INPUT_SELECTION_ACTION_ESC_COL => {
            SelectionAction::Copy
        }
        SelectionActionTarget::Input => SelectionAction::Dismiss,
    }
}

fn chat_selection_action_bar_area(
    chat_area: Rect,
    scroll_offset: usize,
    selection: &crate::ui::selection::Selection,
) -> Rect {
    let content_area = Rect {
        x: chat_area.x,
        y: chat_area.y,
        width: chat_area.width.saturating_sub(2),
        height: chat_area.height,
    };
    let ((start_line, start_col), (end_line, _)) = selection.range();
    selection_action_bar_area_for_anchor(
        content_area,
        scroll_offset,
        start_line,
        end_line,
        start_col,
        SELECTION_ACTION_BAR_WIDTH,
    )
}

fn input_selection_action_bar_area(frame_area: Rect, input_area: Rect) -> Rect {
    let y = input_area.y.saturating_sub(1);
    let x = input_area.x.saturating_add(1);
    clamp_action_bar_area(
        frame_area,
        Rect::new(x, y, SELECTION_ACTION_BAR_WIDTH.min(frame_area.width), 1),
    )
}

fn selection_action_bar_area_for_anchor(
    area: Rect,
    scroll_offset: usize,
    start_line: usize,
    end_line: usize,
    start_col: usize,
    width: u16,
) -> Rect {
    let visible_start_line = start_line.saturating_sub(scroll_offset);
    let visible_end_line = end_line.saturating_sub(scroll_offset);
    let y = if visible_start_line > 0 {
        area.y.saturating_add(visible_start_line as u16 - 1)
    } else {
        area.y.saturating_add(
            (visible_end_line + 1).min(area.height.saturating_sub(1) as usize) as u16,
        )
    };
    let x = area.x.saturating_add(start_col as u16).min(
        area.x
            .saturating_add(area.width.saturating_sub(width.max(1))),
    );

    clamp_action_bar_area(area, Rect::new(x, y, width.min(area.width), 1))
}

fn clamp_action_bar_area(container: Rect, mut area: Rect) -> Rect {
    area.width = area.width.min(container.width);
    if area.width == 0 || container.width == 0 || container.height == 0 {
        return Rect::new(container.x, container.y, 0, 0);
    }

    let max_x = container
        .x
        .saturating_add(container.width.saturating_sub(area.width));
    area.x = area.x.clamp(container.x, max_x);
    let max_y = container
        .y
        .saturating_add(container.height.saturating_sub(1));
    area.y = area.y.clamp(container.y, max_y);
    area.height = 1;
    area
}

fn render_selection_action_bar(
    f: &mut ratatui::Frame,
    area: Rect,
    target: SelectionActionTarget,
    colors: &theme::ThemeColors,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    f.render_widget(Clear, area);
    let bg = colors.dialog_background;
    let key_style = Style::default()
        .fg(colors.text_strong)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(colors.text_weak).bg(bg);
    let line = if target == SelectionActionTarget::Chat {
        Line::from(vec![
            Span::raw(" "),
            Span::styled("i", key_style),
            Span::styled(" add to prompt ", label_style),
            Span::styled("y", key_style),
            Span::styled(" copy ", label_style),
            Span::styled("esc", key_style),
            Span::raw(" "),
        ])
    } else {
        Line::from(vec![
            Span::raw(" "),
            Span::styled("y", key_style),
            Span::styled(" copy ", label_style),
            Span::styled("esc", key_style),
            Span::raw(" "),
        ])
    };
    f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), area);
}

fn message_block_clipboard_text(
    messages: &[crate::session::types::Message],
    range: std::ops::Range<usize>,
) -> String {
    messages
        .get(range)
        .unwrap_or(&[])
        .iter()
        .flat_map(message_clipboard_sections)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn message_clipboard_sections(message: &crate::session::types::Message) -> Vec<String> {
    let mut sections = Vec::new();

    if let Some(reasoning) = message.reasoning.as_deref().map(str::trim) {
        if !reasoning.is_empty() {
            sections.push(format!("Thinking:\n{}", reasoning));
        }
    }

    let content = message.content.trim();
    if !content.is_empty() {
        if matches!(message.role, crate::session::types::MessageRole::Tool) {
            let content = serde_json::from_str::<serde_json::Value>(content)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| content.to_string());
            sections.push(format!("Tool:\n{}", content));
        } else {
            sections.push(message.content.clone());
        }
    }

    sections
}

fn append_usage_suffix(mut text: String, suffix: String) -> String {
    if text.is_empty() {
        suffix
    } else {
        text.push_str(" \u{00b7} ");
        text.push_str(&suffix);
        text
    }
}

fn subagent_tab_label(title: &str, fallback: &str) -> String {
    if let Some(start) = title.find("(@") {
        let after_marker = &title[start + 2..];
        if let Some(agent) = after_marker.strip_suffix(" subagent)") {
            return titlecase_ascii(agent);
        }
    }

    let label = title
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    if label.is_empty() {
        fallback.to_string()
    } else {
        label
    }
}

fn first_agent_mode(messages: &[crate::session::types::Message]) -> Option<String> {
    messages
        .iter()
        .find_map(|message| message.agent_mode.as_deref())
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .map(ToOwned::to_owned)
}

fn latest_message_model(messages: &[crate::session::types::Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find_map(|message| message.model.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

fn titlecase_ascii(value: &str) -> String {
    let mut out = String::new();
    let mut word_start = true;
    for ch in value.trim().chars() {
        if matches!(ch, '-' | '_' | ' ') {
            out.push(ch);
            word_start = true;
        } else if word_start {
            out.push(ch.to_ascii_uppercase());
            word_start = false;
        } else {
            out.push(ch);
        }
    }
    out
}

impl Default for App {
    fn default() -> Self {
        Self::new().expect("Failed to initialize App")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::parser::parse_input;

    fn test_app() -> App {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let theme = Theme::load_builtin_default();
        let colors = theme.get_colors(true);

        App {
            running: true,
            version: "test".to_string(),
            input: {
                let mut input = Input::new();
                input.set_image_open_config(crate::config::ImagesConfig::default());
                input
            },
            command_registry: registry,
            session_manager: SessionManager::new(),
            home_state: init_home(),
            chat_state: init_chat(Chat::new(), "Build", &colors),
            suggestions_popup_state: init_suggestions_popup(Popup::new()),
            models_dialog_state: init_models_dialog("Models", vec![]),
            themes_dialog_state: init_themes_dialog("Themes", vec![]),
            themes_dialog_original_theme_index: 0,
            themes_dialog_committed: false,
            connect_dialog_state: init_connect_dialog(),
            connect_dialog_mode: ConnectDialogMode::ProviderSelection,
            openai_oauth_flow_state: init_openai_oauth_flow(),
            sessions_dialog_state: init_sessions_dialog("Sessions", vec![]),
            session_rename_dialog_state: init_session_rename_dialog(colors),
            permission_dialog_state: init_permission_dialog(),
            question_dialog_state: init_question_dialog(),
            skills_dialog_state: crate::views::skills_dialog::init_skills_dialog("Skills", vec![]),
            command_palette_state: init_command_palette(),
            storage_dialog_state: init_storage_dialog(),
            which_key_state: crate::views::which_key::init_which_key(),
            timeline_dialog_state: crate::views::timeline_dialog::init_timeline_dialog(),
            esc_timeline_primed: false,
            message_actions_index: None,
            message_actions_dialog: None,
            message_actions_return_focus: OverlayFocus::TimelineDialog,
            selection_action_bar: None,
            pending_chat_message_click: None,
            api_key_input: crate::ui::components::api_key_input::ApiKeyInput::new(),
            openai_oauth_receiver: None,
            openai_oauth_in_progress: false,
            compaction_receiver: None,
            compaction_pending: None,
            storage_receiver: None,
            prefs_dao: None,
            agent: "Build".to_string(),
            agent_registry: crate::agent::definition::AgentRegistry::default(),
            agent_steps: std::collections::HashMap::new(),
            provider_timeouts: std::collections::HashMap::new(),
            model: "test-model".to_string(),
            provider_name: "test-provider".to_string(),
            cwd: ".".to_string(),
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
            themes: vec![theme],
            current_theme_index: 0,
            dark_mode: true,
            sounds: crate::sound::ResolvedSoundsConfig::default(),
            notifications: crate::config::NotificationsConfig::default(),
            images: crate::config::ImagesConfig::default(),
            terminal_focused: true,
            tool_permissions: crate::tools::ToolPermissions::new(".".to_string()),
            skills_dirs: Vec::new(),
            is_streaming: false,
            pending_session_title: None,
            session_view_states: std::collections::HashMap::new(),
            session_spinner_frame: 0,
            last_frame_size: ratatui::layout::Rect::default(),
            last_animation_update: std::time::Instant::now(),
            last_session_spinner_update: std::time::Instant::now(),
            cached_git_branch: None,
            cached_git_branch_path: ".".to_string(),
            last_git_branch_check: std::time::Instant::now(),
            discovery: None,
            cached_usage_text: String::new(),
            cached_usage_check: (0, 0),
            terminal_title_enabled: false,
            terminal_title_last: None,
            terminal_title_animation_origin: std::time::Instant::now(),
        }
    }

    fn message_action_names(app: &App) -> Vec<String> {
        app.message_actions_dialog
            .as_ref()
            .map(|dialog| dialog.items.iter().map(|item| item.name.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn terminal_title_uses_workspace_leaf_when_idle() {
        let mut app = test_app();
        app.cwd = "/tmp/sheetpilot".to_string();

        assert_eq!(app.terminal_title_text(), "sheetpilot");
    }

    #[test]
    fn terminal_title_prefixes_spinner_while_streaming() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("test".to_string()));
        if let Some(session) = app.session_manager.sessions.get_mut(&session_id) {
            session.workspace_path = "/tmp/sheetpilot".to_string();
        }
        if let Some(state) = app.session_view_states.get_mut(&session_id) {
            state.external_stream = Some(ExternalStreamState {
                streaming_model: Some("test-model".to_string()),
                streaming_provider: Some("test-provider".to_string()),
                chat_len_before_assistant: 0,
            });
        }

        let title = app.terminal_title_text();
        assert!(TERMINAL_TITLE_SPINNER_FRAMES
            .iter()
            .any(|frame| title == format!("{frame} sheetpilot")));
    }

    #[test]
    fn terminal_title_marks_action_required() {
        let mut app = test_app();
        app.cwd = "/tmp/sheetpilot".to_string();
        app.overlay_focus = OverlayFocus::PermissionDialog;

        assert_eq!(app.terminal_title_text(), "[!] sheetpilot");
    }

    fn add_current_session_message(app: &mut App, message: crate::session::types::Message) {
        app.chat_state.chat.add_message(message.clone());
        app.session_manager
            .add_message_to_current_session(&message)
            .unwrap();
    }

    #[test]
    fn double_esc_opens_timeline_at_most_recent_message() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        app.base_focus = BaseFocus::Chat;
        add_current_session_message(&mut app, crate::session::types::Message::user("Prompt"));
        add_current_session_message(
            &mut app,
            crate::session::types::Message::assistant("Answer"),
        );

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert!(!app.timeline_dialog_state.dialog.is_visible());

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::TimelineDialog);
        assert!(app.timeline_dialog_state.dialog.is_visible());
        assert_eq!(
            app.timeline_dialog_state
                .dialog
                .get_selected()
                .map(|item| item.id.as_str()),
            Some("1")
        );
        assert_eq!(app.chat_state.chat.highlighted_message_index, Some(1));
    }

    #[test]
    fn non_esc_key_clears_pending_double_esc_timeline_open() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        app.base_focus = BaseFocus::Chat;
        add_current_session_message(&mut app, crate::session::types::Message::user("Prompt"));

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_keys(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.input.clear();
        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert!(!app.timeline_dialog_state.dialog.is_visible());

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::TimelineDialog);
        assert!(app.timeline_dialog_state.dialog.is_visible());
    }

    #[test]
    fn esc_with_draft_does_not_prime_timeline_open() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        app.base_focus = BaseFocus::Chat;
        add_current_session_message(&mut app, crate::session::types::Message::user("Prompt"));

        app.input.set_text("draft");
        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.input.clear();
        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert!(!app.timeline_dialog_state.dialog.is_visible());
    }

    #[test]
    fn message_block_clipboard_text_includes_assistant_turn_parts() {
        let mut assistant = crate::session::types::Message::assistant("Final answer");
        assistant.reasoning = Some("Check files".to_string());
        let messages = vec![
            crate::session::types::Message::user("Prompt"),
            assistant,
            crate::session::types::Message::tool(
                serde_json::json!({
                    "name": "read",
                    "status": "ok",
                    "output_preview": "contents",
                })
                .to_string(),
            ),
        ];

        let text = message_block_clipboard_text(&messages, 1..3);

        assert!(text.contains("Thinking:\nCheck files"));
        assert!(text.contains("Final answer"));
        assert!(text.contains("Tool:\n{"));
        assert!(text.contains("\"output_preview\": \"contents\""));
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    #[test]
    fn selection_action_bar_column_mapping_matches_rendered_labels() {
        assert_eq!(
            selection_action_for_column(SelectionActionTarget::Chat, 1),
            SelectionAction::AddToPrompt
        );
        assert_eq!(
            selection_action_for_column(SelectionActionTarget::Chat, 16),
            SelectionAction::Copy
        );
        assert_eq!(
            selection_action_for_column(SelectionActionTarget::Chat, 23),
            SelectionAction::Dismiss
        );
        assert_eq!(
            selection_action_for_column(SelectionActionTarget::Input, 1),
            SelectionAction::Copy
        );
        assert_eq!(
            selection_action_for_column(SelectionActionTarget::Input, 8),
            SelectionAction::Dismiss
        );
    }

    #[test]
    fn chat_selection_action_i_adds_to_prompt_and_dismisses_selection() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::assistant("alpha beta"));
        app.chat_state.chat.selection.active = true;
        app.chat_state.chat.selection.start_line = 0;
        app.chat_state.chat.selection.start_col = 0;
        app.chat_state.chat.selection.end_line = 0;
        app.chat_state.chat.selection.end_col = "alpha".len();

        app.show_selection_action_bar_for(SelectionActionTarget::Chat);
        assert_eq!(
            app.selection_action_bar.map(|state| state.target),
            Some(SelectionActionTarget::Chat)
        );

        app.handle_keys(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

        assert_eq!(app.input.get_text(), "`alpha`");
        assert!(app.selection_action_bar.is_none());
        assert!(!app.chat_state.chat.has_selection());
    }

    #[test]
    fn input_selection_action_bar_shows_when_drag_releases_outside_input() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.input.set_text("alpha beta");
        app.input
            .set_textarea_area_for_test(ratatui::layout::Rect::new(2, 20, 20, 1));

        assert!(app.handle_input_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            20
        )));
        assert!(app.handle_input_mouse_event(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            7,
            20
        )));
        assert!(app.input.has_selection());
        assert_eq!(app.input.get_selected_text(), "alpha");

        assert!(app.handle_input_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 7, 19)));

        assert_eq!(
            app.selection_action_bar.map(|state| state.target),
            Some(SelectionActionTarget::Input)
        );
    }

    #[test]
    fn clicking_chat_message_opens_message_actions() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        let _session_id = app.create_new_session(Some("Chat click".to_string()));
        app.base_focus = BaseFocus::Chat;
        let message = crate::session::types::Message::user("click me");
        app.chat_state.chat.add_message(message.clone());
        app.session_manager
            .add_message_to_current_session(&message)
            .unwrap();
        let colors = app.get_current_theme_colors();
        let positions = app
            .chat_state
            .chat
            .get_message_line_positions(78, &app.model, &colors);
        app.chat_state.chat.message_line_positions = positions;
        app.chat_state.chat.content_height = 25;
        app.chat_state.chat.viewport_height = 18;
        app.chat_state.chat.scroll_offset = 3;
        let scroll_offset_before_click = app.chat_state.chat.scroll_offset;
        assert_eq!(
            app.chat_state.chat.message_index_at_position(
                mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
                app.current_chat_area(),
            ),
            Some(0)
        );

        app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));

        assert_eq!(app.overlay_focus, OverlayFocus::MessageActions);
        assert_eq!(app.message_actions_index, Some(0));
        assert_eq!(
            app.chat_state.chat.scroll_offset,
            scroll_offset_before_click
        );
        assert!(message_action_names(&app).contains(&"Undo".to_string()));
    }

    #[test]
    fn clicking_assistant_chat_message_does_not_open_message_actions() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        let _session_id = app.create_new_session(Some("Chat click".to_string()));
        app.base_focus = BaseFocus::Chat;
        let message = crate::session::types::Message::assistant("click me");
        app.chat_state.chat.add_message(message.clone());
        app.session_manager
            .add_message_to_current_session(&message)
            .unwrap();
        let colors = app.get_current_theme_colors();
        let positions = app
            .chat_state
            .chat
            .get_message_line_positions(78, &app.model, &colors);
        app.chat_state.chat.message_line_positions = positions;
        app.chat_state.chat.content_height = 4;
        app.chat_state.chat.viewport_height = 18;
        app.chat_state.chat.scroll_offset = 0;
        assert_eq!(
            app.chat_state.chat.message_index_at_position(
                mouse(MouseEventKind::Down(MouseButton::Left), 1, 1),
                app.current_chat_area(),
            ),
            Some(0)
        );

        app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert_eq!(app.message_actions_index, None);
        assert_eq!(app.chat_state.chat.highlighted_message_index, None);
    }

    #[test]
    fn hovering_chat_message_does_not_set_timeline_highlight() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        let _session_id = app.create_new_session(Some("Chat hover".to_string()));
        app.base_focus = BaseFocus::Chat;
        let message = crate::session::types::Message::assistant("hover me");
        app.chat_state.chat.add_message(message.clone());
        app.session_manager
            .add_message_to_current_session(&message)
            .unwrap();
        let colors = app.get_current_theme_colors();
        let positions = app
            .chat_state
            .chat
            .get_message_line_positions(78, &app.model, &colors);
        app.chat_state.chat.message_line_positions = positions;
        app.chat_state.chat.content_height = 4;
        app.chat_state.chat.viewport_height = 18;
        app.chat_state.chat.scroll_offset = 0;
        assert_eq!(
            app.chat_state.chat.message_index_at_position(
                mouse(MouseEventKind::Moved, 1, 1),
                app.current_chat_area(),
            ),
            Some(0)
        );

        app.handle_mouse_event(mouse(MouseEventKind::Moved, 1, 1));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert_eq!(app.chat_state.chat.highlighted_message_index, None);
    }

    #[test]
    fn closing_direct_chat_message_actions_returns_to_chat() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        let _session_id = app.create_new_session(Some("Chat click".to_string()));
        app.base_focus = BaseFocus::Chat;
        let message = crate::session::types::Message::user("click me");
        app.chat_state.chat.add_message(message.clone());
        app.session_manager
            .add_message_to_current_session(&message)
            .unwrap();
        let colors = app.get_current_theme_colors();
        let positions = app
            .chat_state
            .chat
            .get_message_line_positions(78, &app.model, &colors);
        app.chat_state.chat.message_line_positions = positions;
        app.chat_state.chat.content_height = 4;
        app.chat_state.chat.viewport_height = 18;
        app.chat_state.chat.scroll_offset = 0;

        app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 1, 1));
        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert_eq!(app.message_actions_index, None);
        assert_eq!(app.chat_state.chat.highlighted_message_index, None);
    }

    #[test]
    fn message_actions_include_undo_for_user_messages() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        app.session_manager
            .add_message_to_current_session(&crate::session::types::Message::user("Prompt"))
            .unwrap();

        app.show_message_actions(0);

        assert!(message_action_names(&app).contains(&"Undo".to_string()));
    }

    #[test]
    fn undo_user_message_restores_local_image_attachments_to_input() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        let mut user_message = crate::session::types::Message::user("see [Image #1]");
        user_message.local_image_paths = vec!["/tmp/example.png".to_string()];
        add_current_session_message(&mut app, user_message);
        add_current_session_message(
            &mut app,
            crate::session::types::Message::assistant("Answer"),
        );
        app.message_actions_index = Some(0);

        app.execute_message_action("undo");

        assert_eq!(app.input.get_text(), "see [Image #1]");
        assert_eq!(
            app.input.local_image_paths_for_submission(),
            vec![std::path::PathBuf::from("/tmp/example.png")]
        );
        assert!(app.chat_state.chat.messages.is_empty());
    }

    #[test]
    fn message_actions_omit_undo_for_agent_messages() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        app.session_manager
            .add_message_to_current_session(&crate::session::types::Message::user("Prompt"))
            .unwrap();
        app.session_manager
            .add_message_to_current_session(&crate::session::types::Message::assistant("Answer"))
            .unwrap();

        app.show_message_actions(1);

        assert!(!message_action_names(&app).contains(&"Undo".to_string()));
    }

    #[test]
    fn commands_can_submit_while_streaming() {
        let input_type = parse_input("/models");

        assert!(App::can_submit_input(&input_type, true));
    }

    #[test]
    fn messages_wait_until_streaming_finishes() {
        let input_type = parse_input("send another prompt");

        assert!(!App::can_submit_input(&input_type, true));
        assert!(App::can_submit_input(&input_type, false));
    }

    #[test]
    fn messages_entered_while_streaming_are_queued() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Queue".to_string()));
        app.base_focus = BaseFocus::Chat;
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states.get_mut(&session_id).unwrap().stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 0,
        });
        app.is_streaming = true;
        app.input.insert_str("Then about riolu");

        app.handle_keys(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let state = app.session_view_states.get(&session_id).unwrap();
        assert_eq!(state.queued_messages.len(), 1);
        assert_eq!(state.queued_messages[0].text, "Then about riolu");
        assert_eq!(
            app.queued_message_previews_for_current_session(),
            vec!["Then about riolu".to_string()]
        );
        assert!(app.input.is_empty());
        assert!(app.chat_state.chat.messages.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn queued_messages_cancel_stream_after_next_tool_result() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Queue after tool".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::tool(
                serde_json::json!({
                    "id": "call_1",
                    "name": "bash",
                    "status": "running",
                })
                .to_string(),
            ));
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed_cancel_token = cancel_token.clone();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token,
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 0,
        });
        state
            .tool_calls
            .tool_call_message_indices
            .insert("call_1".to_string(), 0);
        state.tool_calls.tool_call_order.push("call_1".to_string());
        state.queued_messages.push_back(QueuedUserMessage {
            text: "then about pikachu".to_string(),
            image_paths: Vec::new(),
        });
        app.is_streaming = true;

        app.add_tool_result_to_session(
            &session_id,
            crate::llm::ToolCallResult {
                tool_call_id: "call_1".to_string(),
                role: "tool".to_string(),
                name: "bash".to_string(),
                content: "done".to_string(),
            },
        );

        assert!(observed_cancel_token.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn esc_with_queued_messages_interrupts_and_submits_immediately() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Queue esc".to_string()));
        app.base_focus = BaseFocus::Chat;
        let boundary = app.chat_state.chat.messages.len();
        app.chat_state
            .chat
            .add_assistant_message("partial response");
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed_cancel_token = cancel_token.clone();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token,
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: boundary,
        });
        state.queued_messages.push_back(QueuedUserMessage {
            text: "Then about riolu".to_string(),
            image_paths: Vec::new(),
        });
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(observed_cancel_token.is_cancelled());
        assert!(app
            .session_view_states
            .get(&session_id)
            .unwrap()
            .queued_messages
            .is_empty());
        assert!(app
            .chat_state
            .chat
            .messages
            .iter()
            .any(
                |message| message.role == crate::session::types::MessageRole::User
                    && message.content == "Then about riolu"
            ));
    }

    #[test]
    fn failed_stream_persists_partial_messages() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Failure".to_string()));

        let user_message = crate::session::types::Message::user("Prompt");
        app.chat_state.chat.add_message(user_message.clone());
        app.session_manager
            .add_message_to_current_session(&user_message)
            .unwrap();

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete(
                "I'll inspect that file.",
            ));
        app.chat_state.chat.begin_streaming_turn();
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::tool(
                serde_json::json!({
                    "id": "call_1",
                    "name": "read",
                    "status": "running",
                    "args": { "path": "/private/file" },
                })
                .to_string(),
            ));

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states.get_mut(&session_id).unwrap().stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 1,
        });
        app.is_streaming = true;

        app.fail_streaming_session(&session_id, "Permission denied by user".to_string());

        assert_eq!(app.chat_state.chat.messages.len(), 3);

        let session_messages = &app
            .session_manager
            .get_session_ref(&session_id)
            .unwrap()
            .messages;
        assert_eq!(session_messages.len(), 3);
        assert_eq!(
            session_messages[1].role,
            crate::session::types::MessageRole::Assistant
        );
        assert!(session_messages[1].is_complete);
        assert_eq!(session_messages[1].model.as_deref(), Some("test-model"));
        assert_eq!(
            session_messages[1].provider.as_deref(),
            Some("test-provider")
        );

        let tool_payload: serde_json::Value =
            serde_json::from_str(&session_messages[2].content).unwrap();
        assert_eq!(tool_payload["status"], "error");
        assert_eq!(tool_payload["output_preview"], "Permission denied by user");

        app.fail_streaming_session(&session_id, "duplicate terminal chunk".to_string());

        assert_eq!(app.chat_state.chat.messages.len(), 3);
        assert_eq!(
            app.session_manager
                .get_session_ref(&session_id)
                .unwrap()
                .messages
                .len(),
            3
        );
    }

    #[test]
    fn interrupted_stream_persists_partial_messages() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Interrupted".to_string()));

        let user_message = crate::session::types::Message::user("Prompt");
        app.chat_state.chat.add_message(user_message.clone());
        app.session_manager
            .add_message_to_current_session(&user_message)
            .unwrap();

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete(
                "Partial answer.",
            ));
        app.chat_state.chat.begin_streaming_turn();
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::tool(
                serde_json::json!({
                    "id": "call_1",
                    "name": "read",
                    "status": "running",
                    "args": { "path": "Cargo.toml" },
                })
                .to_string(),
            ));

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states.get_mut(&session_id).unwrap().stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 1,
        });
        app.is_streaming = true;

        app.cancelled_streaming_session(&session_id);

        assert_eq!(app.chat_state.chat.messages.len(), 3);

        let session = app.session_manager.get_session_ref(&session_id).unwrap();
        assert_eq!(
            session.status,
            crate::session::types::SessionStatus::Interrupted
        );
        assert_eq!(session.messages.len(), 3);
        assert_eq!(
            session.messages[1].role,
            crate::session::types::MessageRole::Assistant
        );
        assert_eq!(session.messages[1].content, "Partial answer.");
        assert!(session.messages[1].is_complete);
        assert!(session.messages[1].was_interrupted);
        assert_eq!(session.messages[1].model.as_deref(), Some("test-model"));
        assert_eq!(
            session.messages[1].provider.as_deref(),
            Some("test-provider")
        );

        let tool_payload: serde_json::Value =
            serde_json::from_str(&session.messages[2].content).unwrap();
        assert_eq!(tool_payload["status"], "error");
        assert_eq!(
            tool_payload["output_preview"],
            "Streaming cancelled by user"
        );
    }

    #[test]
    fn stream_finish_waits_for_running_tool_result() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Deferred".to_string()));

        let user_message = crate::session::types::Message::user("Prompt");
        app.chat_state.chat.add_message(user_message.clone());
        app.session_manager
            .add_message_to_current_session(&user_message)
            .unwrap();

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("Checking."));
        app.chat_state.chat.begin_streaming_turn();
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::tool(
                serde_json::json!({
                    "id": "call_1",
                    "name": "read",
                    "status": "running",
                    "args": { "path": "Cargo.toml" },
                })
                .to_string(),
            ));

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 1,
        });
        state
            .tool_calls
            .tool_call_message_indices
            .insert("call_1".to_string(), 2);
        state.tool_calls.tool_call_order.push("call_1".to_string());
        app.is_streaming = true;

        app.finish_streaming_session(&session_id);

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(state.stream.is_some());
        assert!(state.tool_calls.deferred_finish);
        assert!(!app.chat_state.chat.messages[1].is_complete);
        assert_eq!(
            app.session_manager
                .get_session_ref(&session_id)
                .unwrap()
                .messages
                .len(),
            1
        );

        app.add_tool_result_to_session(
            &session_id,
            crate::llm::ToolCallResult {
                tool_call_id: "call_1".to_string(),
                role: "tool".to_string(),
                name: "read".to_string(),
                content: serde_json::json!({
                    "status": "ok",
                    "title": "Read",
                    "output_preview": "contents"
                })
                .to_string(),
            },
        );

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(state.stream.is_none());
        assert!(!state.tool_calls.deferred_finish);
        assert!(app.chat_state.chat.messages[1].is_complete);

        let session_messages = &app
            .session_manager
            .get_session_ref(&session_id)
            .unwrap()
            .messages;
        assert_eq!(session_messages.len(), 3);
        let tool_payload: serde_json::Value =
            serde_json::from_str(&session_messages[2].content).unwrap();
        assert_eq!(tool_payload["status"], "ok");
    }

    #[test]
    fn disconnected_stream_receiver_marks_running_tools_failed() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Disconnected".to_string()));

        let user_message = crate::session::types::Message::user("Prompt");
        app.chat_state.chat.add_message(user_message.clone());
        app.session_manager
            .add_message_to_current_session(&user_message)
            .unwrap();

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("Working."));
        app.chat_state.chat.begin_streaming_turn();
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::tool(
                serde_json::json!({
                    "id": "call_1",
                    "name": "bash",
                    "status": "running",
                    "args": { "command": "cargo test" },
                })
                .to_string(),
            ));

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        drop(sender);

        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 1,
        });
        state
            .tool_calls
            .tool_call_message_indices
            .insert("call_1".to_string(), 2);
        state.tool_calls.tool_call_order.push("call_1".to_string());
        app.is_streaming = true;

        app.process_streaming_chunks();

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(state.stream.is_none());
        assert!(!app.is_streaming);

        let session = app.session_manager.get_session_ref(&session_id).unwrap();
        assert_eq!(session.status, crate::session::types::SessionStatus::Failed);
        assert_eq!(session.messages.len(), 3);

        let tool_payload: serde_json::Value =
            serde_json::from_str(&session.messages[2].content).unwrap();
        assert_eq!(tool_payload["status"], "error");
        assert_eq!(
            tool_payload["output_preview"],
            "Stream task ended before sending a completion event"
        );
    }

    #[test]
    fn disconnected_stream_receiver_processes_queued_tool_result_before_failing() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Tool result then disconnect".to_string()));

        let user_message = crate::session::types::Message::user("Prompt");
        app.chat_state.chat.add_message(user_message.clone());
        app.session_manager
            .add_message_to_current_session(&user_message)
            .unwrap();

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("Working."));
        app.chat_state.chat.begin_streaming_turn();
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::tool(
                serde_json::json!({
                    "id": "call_1",
                    "name": "bash",
                    "status": "running",
                    "args": { "command": "cargo test" },
                })
                .to_string(),
            ));

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(crate::llm::ChunkMessage::ToolResult(
                crate::llm::ToolCallResult {
                    tool_call_id: "call_1".to_string(),
                    role: "tool".to_string(),
                    name: "bash".to_string(),
                    content: serde_json::json!({
                        "status": "ok",
                        "title": "Bash",
                        "output_preview": "tests passed"
                    })
                    .to_string(),
                },
            ))
            .unwrap();
        drop(sender);

        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState {
            chunk_receiver: receiver,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            streaming_model: Some("test-model".to_string()),
            streaming_provider: Some("test-provider".to_string()),
            chat_len_before_assistant: 1,
        });
        state
            .tool_calls
            .tool_call_message_indices
            .insert("call_1".to_string(), 2);
        state.tool_calls.tool_call_order.push("call_1".to_string());
        app.is_streaming = true;

        app.process_streaming_chunks();

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(state.stream.is_none());
        assert!(!app.is_streaming);

        let session = app.session_manager.get_session_ref(&session_id).unwrap();
        assert_eq!(session.status, crate::session::types::SessionStatus::Failed);
        assert_eq!(session.messages.len(), 3);

        let tool_payload: serde_json::Value =
            serde_json::from_str(&session.messages[2].content).unwrap();
        assert_eq!(tool_payload["status"], "ok");
        assert_eq!(tool_payload["output_preview"], "tests passed");
    }

    #[test]
    fn chat_only_commands_are_rejected_outside_chat() {
        let mut app = test_app();

        assert!(app.reject_chat_only_command_outside_chat("compact"));

        app.base_focus = BaseFocus::Chat;
        assert!(!app.reject_chat_only_command_outside_chat("compact"));
    }

    #[test]
    fn compaction_result_is_applied_from_receiver() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Compact".to_string()));
        app.base_focus = BaseFocus::Chat;

        let stats = crate::session::types::CompactionStats {
            before_tokens: 1_000,
            after_tokens: 120,
            before_messages: 5,
            after_messages: 1,
        };
        let mut summary = crate::session::types::Message::user("summary");
        summary.compaction_stats = Some(stats);
        let compacted_messages = vec![summary];
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(CompactionTaskMessage::Success {
                session_id: session_id.clone(),
                messages: compacted_messages.clone(),
                stats,
            })
            .unwrap();
        drop(sender);
        app.compaction_receiver = Some(receiver);
        app.compaction_pending = Some(CompactionPending {
            session_id: session_id.clone(),
            before_tokens: stats.before_tokens,
        });
        app.is_streaming = true;

        app.process_compaction_events();

        assert!(app.compaction_receiver.is_none());
        assert!(app.compaction_pending.is_none());
        assert!(!app.is_streaming);
        assert_eq!(app.chat_state.chat.messages, compacted_messages);
        assert_eq!(
            app.session_manager
                .get_session_ref(&session_id)
                .map(|session| session.messages.clone()),
            Some(compacted_messages)
        );
    }

    #[test]
    fn session_usage_text_includes_compaction_stats() {
        let mut app = test_app();
        let stats = crate::session::types::CompactionStats {
            before_tokens: 12_000,
            after_tokens: 360,
            before_messages: 8,
            after_messages: 2,
        };
        let mut summary = crate::session::types::Message::user("summary");
        summary.token_count = Some(stats.after_tokens);
        summary.compaction_stats = Some(stats);
        app.chat_state.chat.add_message(summary);

        assert_eq!(app.session_usage_text(), "360 \u{00b7} last compact 97%");
    }

    #[test]
    fn start_blank_session_does_not_create_session_record() {
        let mut app = test_app();
        app.create_new_session(Some("Existing".to_string()));

        app.start_blank_session(None);

        assert!(app.session_manager.get_current_session_id().is_none());
        assert_eq!(app.session_manager.list_sessions().len(), 1);
        assert_eq!(app.base_focus, BaseFocus::Home);
    }

    #[test]
    fn start_blank_session_keeps_optional_title_for_next_real_session() {
        let mut app = test_app();

        app.start_blank_session(Some("  Named draft  ".to_string()));

        assert!(app.session_manager.list_sessions().is_empty());
        assert_eq!(app.pending_session_title.as_deref(), Some("Named draft"));
    }

    #[test]
    fn ctrl_n_is_not_a_global_new_session_shortcut() {
        let mut app = test_app();
        app.create_new_session(Some("Existing".to_string()));

        let handled = app.handle_base_keys(KeyEvent::new(
            KeyCode::Char('n'),
            event::KeyModifiers::CONTROL,
        ));

        assert!(!handled);
        assert!(app.session_manager.get_current_session_id().is_some());
        assert_eq!(app.session_manager.list_sessions().len(), 1);
    }

    #[test]
    fn sessions_dialog_defaults_to_all_unarchived_workspaces() {
        let mut app = test_app();
        let current_id = app.create_new_session(Some("Current".to_string()));
        let other_id = app.create_new_session(Some("Other".to_string()));
        let other_session = app.session_manager.get_session(&other_id).unwrap();
        other_session.workspace_id = 42;
        other_session.workspace_path = "/tmp/other-workspace".to_string();
        other_session.workspace_name = "other-workspace".to_string();

        app.open_sessions_dialog();

        assert_eq!(app.sessions_dialog_state.filter, SessionsDialogFilter::All);
        let items = &app.sessions_dialog_state.dialog.items;
        assert!(items.iter().any(|item| item.id == current_id));
        assert!(items
            .iter()
            .any(|item| item.id == other_id && item.group == "other-workspace"));
    }

    #[test]
    fn sessions_dialog_focuses_current_workspace_from_home_without_current_session() {
        let mut app = test_app();
        let current_id = app.create_new_session(Some("Current".to_string()));
        let other_id = app.create_new_session(Some("Other".to_string()));
        let other_session = app.session_manager.get_session(&other_id).unwrap();
        other_session.workspace_id = -1;
        other_session.workspace_path = "/tmp/other-workspace".to_string();
        other_session.workspace_name = "other-workspace".to_string();

        app.start_blank_session(None);
        app.open_sessions_dialog();

        assert_eq!(app.base_focus, BaseFocus::Home);
        assert!(app.session_manager.get_current_session_id().is_none());
        assert_eq!(app.sessions_dialog_state.filter, SessionsDialogFilter::All);
        assert_eq!(
            app.sessions_dialog_state.dialog.get_focused_group_header(),
            Some(app.session_manager.current_workspace_name())
        );
        assert!(app
            .sessions_dialog_state
            .dialog
            .items
            .iter()
            .any(|item| item.id == current_id));
        assert!(app
            .sessions_dialog_state
            .dialog
            .items
            .iter()
            .any(|item| item.id == other_id));
    }

    #[test]
    fn status_workspace_path_follows_active_session() {
        let mut app = test_app();
        app.cwd = "/tmp/fallback-workspace".to_string();
        let first_id = app.create_new_session(Some("First".to_string()));
        let second_id = app.create_new_session(Some("Second".to_string()));

        app.session_manager
            .get_session(&first_id)
            .unwrap()
            .workspace_path = "/tmp/workspace-a".to_string();
        app.session_manager
            .get_session(&second_id)
            .unwrap()
            .workspace_path = "/tmp/workspace-b".to_string();

        assert!(app.switch_to_session(&first_id));
        assert_eq!(app.active_workspace_path(), "/tmp/workspace-a");

        assert!(app.switch_to_session(&second_id));
        assert_eq!(app.active_workspace_path(), "/tmp/workspace-b");

        app.session_manager.clear_current_session();
        assert_eq!(app.active_workspace_path(), "/tmp/fallback-workspace");
    }

    #[test]
    fn deleting_current_session_keeps_sessions_dialog_focused() {
        let mut app = test_app();
        app.create_new_session(Some("First".to_string()));
        app.create_new_session(Some("Second".to_string()));
        app.open_sessions_dialog();

        assert!(app
            .sessions_dialog_state
            .dialog
            .select_index_clamped(usize::MAX));
        let deleted_id = app
            .sessions_dialog_state
            .dialog
            .get_selected()
            .map(|item| item.id.clone())
            .expect("selected session");
        assert!(app.switch_to_session(&deleted_id));

        app.handle_keys(KeyEvent::new(
            KeyCode::Char('d'),
            event::KeyModifiers::CONTROL,
        ));
        app.handle_keys(KeyEvent::new(
            KeyCode::Char('d'),
            event::KeyModifiers::CONTROL,
        ));

        assert_eq!(app.overlay_focus, OverlayFocus::SessionsDialog);
        assert!(app.sessions_dialog_state.dialog.is_visible());
        assert!(app.session_manager.get_current_session_id().is_none());
        assert!(app.session_manager.get_session_ref(&deleted_id).is_none());
        assert_eq!(app.sessions_dialog_state.dialog.selected_index, 0);
        assert_ne!(
            app.sessions_dialog_state
                .dialog
                .get_selected()
                .map(|item| item.id.as_str()),
            Some(deleted_id.as_str())
        );
    }

    #[test]
    fn deleting_only_current_session_keeps_empty_sessions_dialog_open() {
        let mut app = test_app();
        app.create_new_session(Some("Only".to_string()));
        app.open_sessions_dialog();

        app.handle_keys(KeyEvent::new(
            KeyCode::Char('d'),
            event::KeyModifiers::CONTROL,
        ));
        app.handle_keys(KeyEvent::new(
            KeyCode::Char('d'),
            event::KeyModifiers::CONTROL,
        ));

        assert_eq!(app.overlay_focus, OverlayFocus::SessionsDialog);
        assert!(app.sessions_dialog_state.dialog.is_visible());
        assert!(app.session_manager.list_sessions().is_empty());
        assert!(app.session_manager.get_current_session_id().is_none());
        assert!(app.sessions_dialog_state.dialog.get_selected().is_none());
    }

    #[test]
    fn archiving_last_visible_current_session_focuses_previous_session() {
        let mut app = test_app();
        app.create_new_session(Some("First".to_string()));
        app.create_new_session(Some("Second".to_string()));
        app.open_sessions_dialog();

        assert!(app
            .sessions_dialog_state
            .dialog
            .select_index_clamped(usize::MAX));
        let archived_id = app
            .sessions_dialog_state
            .dialog
            .get_selected()
            .map(|item| item.id.clone())
            .expect("selected session");
        assert!(app.switch_to_session(&archived_id));

        app.handle_keys(KeyEvent::new(
            KeyCode::Char('a'),
            event::KeyModifiers::CONTROL,
        ));

        assert_eq!(app.overlay_focus, OverlayFocus::SessionsDialog);
        assert!(app.sessions_dialog_state.dialog.is_visible());
        assert!(app.session_manager.get_current_session_id().is_none());
        assert!(app
            .session_manager
            .get_session_ref(&archived_id)
            .and_then(|session| session.archived_at)
            .is_some());
        assert_eq!(app.sessions_dialog_state.dialog.selected_index, 0);
        assert_ne!(
            app.sessions_dialog_state
                .dialog
                .get_selected()
                .map(|item| item.id.as_str()),
            Some(archived_id.as_str())
        );
    }

    #[test]
    fn child_session_navigation_matches_opencode_flow() {
        let mut app = test_app();
        let parent_id = app.create_new_session(Some("Parent".to_string()));
        app.base_focus = BaseFocus::Chat;

        app.start_subagent_session(
            parent_id.clone(),
            "child-a".to_string(),
            "Explore task (@explore subagent)".to_string(),
            "explore".to_string(),
            None,
            None,
            "Explore task".to_string(),
            "Find files".to_string(),
        );
        app.start_subagent_session(
            parent_id.clone(),
            "child-b".to_string(),
            "General task (@general subagent)".to_string(),
            "general".to_string(),
            None,
            None,
            "General task".to_string(),
            "Check implementation".to_string(),
        );

        assert_eq!(
            app.session_manager.get_current_session_id(),
            Some(&parent_id)
        );
        assert!(app.switch_to_first_child_session());
        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some("child-a")
        );

        assert!(app.handle_base_keys(KeyEvent::new(KeyCode::Right, event::KeyModifiers::NONE,)));
        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some("child-b")
        );

        assert!(app.handle_base_keys(KeyEvent::new(KeyCode::Left, event::KeyModifiers::NONE,)));
        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some("child-a")
        );

        assert!(app.handle_base_keys(KeyEvent::new(KeyCode::Up, event::KeyModifiers::NONE,)));
        assert_eq!(
            app.session_manager.get_current_session_id(),
            Some(&parent_id)
        );
    }

    #[test]
    fn subagent_session_ignores_text_input() {
        let mut app = test_app();
        let parent_id = app.create_new_session(Some("Parent".to_string()));
        app.base_focus = BaseFocus::Chat;

        app.start_subagent_session(
            parent_id,
            "child-a".to_string(),
            "General task (@general subagent)".to_string(),
            "general".to_string(),
            Some("sub-model".to_string()),
            Some("sub-provider".to_string()),
            "General task".to_string(),
            "Check implementation".to_string(),
        );

        assert!(app.switch_to_first_child_session());
        app.handle_keys(KeyEvent::new(KeyCode::Char('h'), event::KeyModifiers::NONE));
        app.handle_paste(" pasted".to_string());

        assert_eq!(app.input.get_text(), "");
    }

    #[test]
    fn subagent_tab_label_prefers_agent_type_marker() {
        assert_eq!(
            subagent_tab_label("Find files (@explore subagent)", "fallback"),
            "Explore"
        );
        assert_eq!(
            subagent_tab_label("Analyze image (@vlm-agent subagent)", "fallback"),
            "Vlm-Agent"
        );
        assert_eq!(subagent_tab_label("", "fallback"), "fallback");
    }
}
