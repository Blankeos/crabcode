use crate::views::variants_dialog::render_variants_dialog;
use ratatui::crossterm::event::{
    self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::autocomplete::AutoComplete;
use crate::command::handlers::register_all_commands;
use crate::command::parser::InputType;
use crate::command::registry::Registry;
use crate::llm::client::stream_llm_with_cancellation;
use crate::session::manager::SessionManager;
use crate::tools::{PermissionResponse, ToolHandler};

use crate::push_toast;
use crate::toast::{self, Toast, ToastLevel};
use crate::ui::components::action_dialog::{ActionDialog, ActionDialogEvent, ActionDialogItem};
use crate::ui::components::chat::{Chat, ChatImageTarget};
use crate::ui::components::find::{FindBar, FindBarAction};
use crate::ui::components::input::Input;
use crate::ui::components::popup::Popup;
use crate::ui::hyperlink::HyperlinkTarget;
use crate::utils::git;

use crate::tools::TerminalSessionEvent;
use crate::views::agents_dialog::{
    handle_agents_dialog_key_event, handle_agents_dialog_mouse_event, init_agents_dialog,
    render_agents_dialog, AgentsDialogAction,
};
use crate::views::chat::{
    agent_color_for_tab, init_chat, queued_messages_height, render_chat,
    render_subagent_spinner_only, SubagentTab, SubagentTabs, SUBAGENT_FOOTER_HEIGHT,
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
use crate::views::jobs_dialog::{
    handle_jobs_dialog_key_event, handle_jobs_dialog_mouse_event, init_jobs_dialog,
    render_jobs_dialog, JobsDialogAction,
};
use crate::views::mcp_dialog::{
    handle_mcp_dialog_key_event, handle_mcp_dialog_mouse_event, init_mcp_dialog, render_mcp_dialog,
    McpDialogAction,
};
use crate::views::models_dialog::{
    handle_models_dialog_key_event, handle_models_dialog_mouse_event, init_models_dialog,
    render_models_dialog,
};
use crate::views::move_session_dialog::{
    handle_move_session_dialog_key_event, handle_move_session_dialog_mouse_event,
    init_move_session_dialog, render_move_session_dialog, MoveSessionDialogAction,
};
use crate::views::permission_dialog::{
    handle_permission_dialog_key_event, handle_permission_dialog_mouse_event,
    init_permission_dialog, render_permission_dialog, PermissionDialogAction,
};
use crate::views::provider_oauth_flow::{
    handle_provider_oauth_flow_key_event, handle_provider_oauth_flow_mouse_event,
    init_provider_oauth_flow, render_provider_oauth_flow, ProviderOAuthFlowAction,
};
use crate::views::question_dialog::{
    handle_question_dialog_key_event, handle_question_dialog_mouse_event, init_question_dialog,
    render_question_dialog, QuestionDialogAction,
};
use crate::views::remote_dialog::{
    handle_remote_dialog_key_event, handle_remote_dialog_mouse_event, init_remote_dialog,
    render_remote_dialog, RemoteDialogAction, RemoteDialogSubmission,
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
use crate::views::terminal_session_dialog::{
    handle_terminal_session_dialog_key_event, init_terminal_session_dialog,
    render_terminal_session_dialog, TerminalSessionResponse,
};
use crate::views::themes_dialog::{
    handle_themes_dialog_key_event, handle_themes_dialog_mouse_event, init_themes_dialog,
    render_themes_dialog,
};
use crate::views::title_dialog::{
    handle_title_dialog_key_event, handle_title_dialog_mouse_event, init_title_dialog,
    render_title_dialog, TitleDialogAction,
};
use crate::views::{
    AgentsDialogState, ChatState, ConnectDialogState, HomeState, JobsDialogState, McpDialogState,
    ModelsDialogState, MoveSessionDialogState, PermissionDialogState, ProviderOAuthFlowState,
    QuestionDialogState, RemoteDialogState, SessionRenameDialogState, SessionsDialogState,
    StorageDialogState, SuggestionsPopupState, TerminalSessionDialogState, ThemesDialogState,
    TitleDialogState, VariantsDialogState,
};

use crate::{
    get_toast_manager,
    theme::{self, Theme},
};

use anyhow::{Context, Result};

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

fn is_default_session_title(title: &str) -> bool {
    title
        .trim()
        .strip_prefix("session-")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn is_auto_session_title_for_prompt(title: &str, prompt: &str) -> bool {
    let title = title.trim();
    is_default_session_title(title) || title == App::generate_title_from_message(prompt)
}

fn first_user_prompt(chat: &Chat) -> Option<String> {
    chat.messages
        .iter()
        .find(|message| message.role == crate::session::types::MessageRole::User)
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

fn has_single_user_message(chat: &Chat) -> bool {
    chat.messages
        .iter()
        .filter(|message| message.role == crate::session::types::MessageRole::User)
        .take(2)
        .count()
        == 1
}

fn models_dialog_provider_ids() -> Option<Vec<String>> {
    let mut signature = crate::persistence::AuthDAO::new()
        .and_then(|dao| dao.load())
        .ok()?
        .into_keys()
        .map(|provider_id| format!("auth:{provider_id}"))
        .collect::<Vec<_>>();

    if let Ok(discovery) = crate::model::discovery::Discovery::new() {
        signature.extend(discovery.custom_provider_dialog_signature());
    }

    signature.sort();
    signature.dedup();
    Some(signature)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingRetryStatus {
    pub attempt: usize,
    pub message: String,
    pub next_epoch_ms: u64,
}

impl From<crate::aisdk::retry::RetryStatus> for StreamingRetryStatus {
    fn from(status: crate::aisdk::retry::RetryStatus) -> Self {
        Self {
            attempt: status.attempt,
            message: status.message,
            next_epoch_ms: status.next_epoch_ms,
        }
    }
}

pub(crate) fn titlecase_agent_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "Build".to_string();
    }

    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return "Build".to_string();
    };

    format!(
        "{}{}",
        first.to_uppercase().collect::<String>(),
        chars.as_str()
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BaseFocus {
    Home,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayFocus {
    None,
    AgentsDialog,
    ModelsDialog,
    VariantsDialog,
    RefreshModelsDialog,
    ThemesDialog,
    ConnectDialog,
    ProviderOAuthFlow,
    ApiKeyInput,
    SuggestionsPopup,
    SessionsDialog,
    SessionRenameDialog,
    MoveSessionDialog,
    PermissionDialog,
    QuestionDialog,
    TerminalSessionDialog,
    RemoteDialog,
    SkillsDialog,
    McpDialog,
    TimelineDialog,
    JobsDialog,
    CopyActions,
    MessageActions,
    CommandPalette,
    FindBar,
    StorageDialog,
    TitleDialog,
    WhichKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectDialogMode {
    ProviderSelection,
    OpenAIMethodSelection,
    XAIMethodSelection,
}

#[derive(Debug)]
enum ProviderOAuthTaskMessage {
    HeadlessCode {
        code: String,
        url: String,
    },
    Success {
        provider: OAuthProvider,
        credentials: crate::auth::OAuthCredentials,
    },
    Failed {
        provider: OAuthProvider,
        error: String,
    },
}

#[derive(Debug)]
enum McpOAuthTaskMessage {
    Success { name: String },
    Failed { name: String, error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OAuthProvider {
    OpenAI,
    XAI,
}

impl OAuthProvider {
    fn provider_id(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::XAI => "xai",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::OpenAI => "OpenAI",
            Self::XAI => "xAI",
        }
    }

    fn connected_message(self) -> &'static str {
        match self {
            Self::OpenAI => "Connected OpenAI via ChatGPT Plus/Pro OAuth",
            Self::XAI => "Connected xAI via Grok OAuth",
        }
    }

    fn default_model(self) -> &'static str {
        match self {
            Self::OpenAI => "gpt-5.3-codex",
            Self::XAI => "grok-build-0.1",
        }
    }
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
    Cancelled {
        session_id: String,
    },
}

#[derive(Debug)]
enum StorageTaskMessage {
    Loaded(crate::utils::storage::StorageReport),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelsTaskKind {
    Load,
    Refresh,
}

#[derive(Debug)]
struct ModelsTaskMessage {
    kind: ModelsTaskKind,
    result: crate::command::registry::CommandResult,
    provider_signature: Option<Vec<String>>,
}

#[derive(Debug)]
enum TitleGenerationTaskMessage {
    Generated { session_id: String, title: String },
}

#[derive(Debug, Clone)]
struct SmallModelConfig {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteLaunchRequest {
    pub bind: String,
    pub pair_code: Option<String>,
}

#[derive(Debug, Clone)]
struct CompactionPending {
    session_id: String,
    before_tokens: usize,
    cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Debug)]
struct SessionStreamState {
    chunk_receiver: crate::llm::ChunkReceiver,
    cancel_token: tokio_util::sync::CancellationToken,
    streaming_model: Option<String>,
    streaming_provider: Option<String>,
    chat_len_before_assistant: usize,
    last_message_snapshot: std::time::Instant,
    pending_message_snapshot: bool,
}

impl SessionStreamState {
    fn new(
        chunk_receiver: crate::llm::ChunkReceiver,
        cancel_token: tokio_util::sync::CancellationToken,
        streaming_model: Option<String>,
        streaming_provider: Option<String>,
        chat_len_before_assistant: usize,
    ) -> Self {
        Self {
            chunk_receiver,
            cancel_token,
            streaming_model,
            streaming_provider,
            chat_len_before_assistant,
            last_message_snapshot: std::time::Instant::now(),
            pending_message_snapshot: false,
        }
    }
}

#[derive(Debug, Clone)]
struct ExternalStreamState {
    streaming_model: Option<String>,
    streaming_provider: Option<String>,
    chat_len_before_assistant: usize,
    last_message_snapshot: std::time::Instant,
    pending_message_snapshot: bool,
}

impl ExternalStreamState {
    fn new(
        streaming_model: Option<String>,
        streaming_provider: Option<String>,
        chat_len_before_assistant: usize,
    ) -> Self {
        Self {
            streaming_model,
            streaming_provider,
            chat_len_before_assistant,
            last_message_snapshot: std::time::Instant::now(),
            pending_message_snapshot: false,
        }
    }
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
    JobsDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionActionBarState {
    target: SelectionActionTarget,
    can_open_in_editor: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionAction {
    AddToPrompt,
    Copy,
    OpenInEditor,
    Dismiss,
}

const TERMINAL_TITLE_SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TERMINAL_TITLE_SPINNER_INTERVAL_MS: u128 = 100;
const STREAM_CHUNK_DRAIN_LIMIT: usize = 8 * 1024;
/// Bound downstream coalescing and mutation work, not only time spent in `try_recv`.
const STREAM_CHUNK_GLOBAL_DRAIN_LIMIT: usize = 1024;
/// Total time spent draining stream chunks across all sessions per event-loop iteration.
const STREAM_CHUNK_GLOBAL_DRAIN_TIME_BUDGET: std::time::Duration =
    std::time::Duration::from_millis(4);
const SESSIONS_DIALOG_METADATA_PROBE_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(200);
const STREAM_MESSAGE_SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

fn disconnected_stream_warning_message(error: &str) -> Option<String> {
    let trimmed = error.trim();
    let normalized = trimmed
        .strip_prefix("Streaming failed:")
        .unwrap_or(trimmed)
        .trim();
    let lower = normalized.to_ascii_lowercase();

    if lower.starts_with("stream disconnected before completion") {
        let suffix = normalized["stream disconnected before completion".len()..]
            .trim_start_matches(':')
            .trim();
        return Some(if suffix.is_empty() {
            "Stream disconnected before completion".to_string()
        } else {
            format!("Stream disconnected before completion: {suffix}")
        });
    }

    let disconnected = lower.contains("before response.completed")
        || lower.contains("ended without a terminal completion event")
        || lower.contains("ended before sending a completion event");

    disconnected.then(|| format!("Stream disconnected before completion: {normalized}"))
}

fn coalesce_streaming_chunks(
    chunks: Vec<crate::llm::ChunkMessage>,
) -> Vec<crate::llm::ChunkMessage> {
    use crate::llm::ChunkMessage;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum StreamChunkKind {
        Text,
        Reasoning,
        Retry,
    }

    fn split_route(chunk: ChunkMessage) -> (Vec<String>, ChunkMessage) {
        let mut route = Vec::new();
        let mut current = chunk;
        loop {
            match current {
                ChunkMessage::SubagentChunk { session_id, chunk } => {
                    route.push(session_id);
                    current = *chunk;
                }
                leaf => return (route, leaf),
            }
        }
    }

    fn wrap_route(route: Vec<String>, mut chunk: ChunkMessage) -> ChunkMessage {
        for session_id in route.into_iter().rev() {
            chunk = ChunkMessage::SubagentChunk {
                session_id,
                chunk: Box::new(chunk),
            };
        }
        chunk
    }

    fn routed_leaf_mut<'a>(
        chunk: &'a mut ChunkMessage,
        route: &[String],
    ) -> Option<&'a mut ChunkMessage> {
        let Some((session_id, remaining)) = route.split_first() else {
            return Some(chunk);
        };
        match chunk {
            ChunkMessage::SubagentChunk {
                session_id: chunk_session_id,
                chunk,
            } if chunk_session_id == session_id => routed_leaf_mut(chunk, remaining),
            _ => None,
        }
    }

    fn route_is_prefix(prefix: &[String], route: &[String]) -> bool {
        prefix.len() <= route.len()
            && prefix
                .iter()
                .zip(route.iter())
                .all(|(prefix, route)| prefix == route)
    }

    fn clear_pending_for_event(
        pending: &mut std::collections::HashMap<(Vec<String>, StreamChunkKind), usize>,
        route: &[String],
        keep: Option<StreamChunkKind>,
    ) {
        pending.retain(|(pending_route, pending_kind), _| {
            if pending_route == route {
                return keep == Some(*pending_kind);
            }

            // A nested event is an ordering boundary for every ancestor route.
            // Unrelated sibling routes remain mergeable across each other.
            !route_is_prefix(pending_route, route)
        });
    }

    let mut coalesced = Vec::with_capacity(chunks.len());
    let mut pending = std::collections::HashMap::<(Vec<String>, StreamChunkKind), usize>::new();
    for chunk in chunks {
        let (route, chunk) = split_route(chunk);
        match chunk {
            ChunkMessage::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                clear_pending_for_event(&mut pending, &route, Some(StreamChunkKind::Text));
                let key = (route.clone(), StreamChunkKind::Text);
                if let Some(index) = pending.get(&key).copied() {
                    if let Some(ChunkMessage::Text(previous)) =
                        routed_leaf_mut(&mut coalesced[index], &route)
                    {
                        previous.push_str(&text);
                        continue;
                    }
                }
                let index = coalesced.len();
                coalesced.push(wrap_route(route, ChunkMessage::Text(text)));
                pending.insert(key, index);
            }
            ChunkMessage::Reasoning(reasoning) => {
                if reasoning.is_empty() {
                    continue;
                }
                clear_pending_for_event(&mut pending, &route, Some(StreamChunkKind::Reasoning));
                let key = (route.clone(), StreamChunkKind::Reasoning);
                if let Some(index) = pending.get(&key).copied() {
                    if let Some(ChunkMessage::Reasoning(previous)) =
                        routed_leaf_mut(&mut coalesced[index], &route)
                    {
                        previous.push_str(&reasoning);
                        continue;
                    }
                }
                let index = coalesced.len();
                coalesced.push(wrap_route(route, ChunkMessage::Reasoning(reasoning)));
                pending.insert(key, index);
            }
            ChunkMessage::Retry(status) => {
                clear_pending_for_event(&mut pending, &route, Some(StreamChunkKind::Retry));
                let key = (route.clone(), StreamChunkKind::Retry);
                if let Some(index) = pending.get(&key).copied() {
                    if let Some(ChunkMessage::Retry(previous)) =
                        routed_leaf_mut(&mut coalesced[index], &route)
                    {
                        *previous = status;
                        continue;
                    }
                }
                let index = coalesced.len();
                coalesced.push(wrap_route(route, ChunkMessage::Retry(status)));
                pending.insert(key, index);
            }
            other => {
                clear_pending_for_event(&mut pending, &route, None);
                coalesced.push(wrap_route(route, other));
            }
        }
    }

    coalesced
}

fn drain_streaming_chunks_global(
    sessions: &mut [(
        String,
        &mut tokio::sync::mpsc::UnboundedReceiver<crate::llm::ChunkMessage>,
    )],
    per_session_limit: usize,
    global_limit: usize,
    global_time_budget: std::time::Duration,
    rotation: usize,
) -> (Vec<(String, Vec<crate::llm::ChunkMessage>, bool)>, usize) {
    if sessions.is_empty() {
        return (Vec::new(), rotation);
    }

    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    let session_count = sessions.len();
    let start = rotation % session_count;

    let budget_started = std::time::Instant::now();
    let mut drained = Vec::new();
    let mut next_rotation = rotation;

    let mut total_drained = 0usize;
    while total_drained < global_limit && budget_started.elapsed() < global_time_budget {
        let mut round_progress = false;
        for step in 0..session_count {
            let remaining = global_time_budget.saturating_sub(budget_started.elapsed());
            if remaining == std::time::Duration::ZERO {
                break;
            }

            let sessions_left = session_count - step;
            let per_turn_budget = remaining
                .checked_div(sessions_left as u32)
                .unwrap_or(remaining)
                .max(std::time::Duration::from_nanos(1));

            let index = (start + step) % session_count;
            let (session_id, receiver) = &mut sessions[index];
            let remaining_limit = global_limit.saturating_sub(total_drained);
            let (chunks, disconnected) = drain_streaming_chunks(
                receiver,
                per_session_limit.min(remaining_limit),
                per_turn_budget,
            );
            total_drained = total_drained.saturating_add(chunks.len());

            if !chunks.is_empty() || disconnected {
                drained.push((session_id.clone(), chunks, disconnected));
                round_progress = true;
            }

            next_rotation = (index + 1) % session_count;

            if total_drained >= global_limit || budget_started.elapsed() >= global_time_budget {
                break;
            }
        }

        if !round_progress {
            break;
        }
    }

    (drained, next_rotation)
}

fn drain_streaming_chunks(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<crate::llm::ChunkMessage>,
    limit: usize,
    time_budget: std::time::Duration,
) -> (Vec<crate::llm::ChunkMessage>, bool) {
    let started_at = std::time::Instant::now();
    let mut chunks = Vec::new();
    let mut disconnected = false;

    for _ in 0..limit {
        match receiver.try_recv() {
            Ok(chunk) => chunks.push(chunk),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                disconnected = true;
                break;
            }
        }
        if chunks.len().is_multiple_of(64) && started_at.elapsed() >= time_budget {
            break;
        }
    }

    (chunks, disconnected)
}

type ReasoningEffortOverrides =
    std::collections::HashMap<(String, String), crate::model::reasoning::ReasoningEffort>;
type ModelReasoningOptions =
    std::collections::HashMap<(String, String), Vec<crate::model::reasoning::ReasoningOption>>;

fn reasoning_effort_overrides_from_prefs(
    prefs: &crate::persistence::prefs::ModelPreferences,
) -> ReasoningEffortOverrides {
    let mut overrides = ReasoningEffortOverrides::new();
    let Some(map) = prefs.variant.as_object() else {
        return overrides;
    };

    for (key, value) in map {
        let Some((provider_id, model_id)) = key.split_once('/') else {
            continue;
        };
        let Some(effort) = value.as_str().and_then(|value| {
            value
                .parse::<crate::model::reasoning::ReasoningEffort>()
                .ok()
        }) else {
            continue;
        };
        if effort == crate::model::reasoning::ReasoningEffort::None {
            continue;
        }
        overrides.insert((provider_id.to_string(), model_id.to_string()), effort);
    }

    overrides
}

#[derive(Debug, Clone)]
struct QueuedUserMessage {
    text: String,
    image_paths: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone)]
enum QueuedItem {
    Message(QueuedUserMessage),
    Compact,
}

impl QueuedItem {
    fn preview(&self) -> String {
        match self {
            Self::Message(message) => {
                if !message.text.trim().is_empty() {
                    return message.text.replace('\n', " ");
                }
                match message.image_paths.len() {
                    0 => String::new(),
                    1 => "[Image]".to_string(),
                    count => format!("[{} images]", count),
                }
            }
            Self::Compact => "/compact".to_string(),
        }
    }
}

#[derive(Debug)]
struct ClientSessionState {
    chat: Chat,
    input_draft: String,
    stream: Option<SessionStreamState>,
    external_stream: Option<ExternalStreamState>,
    tool_calls: ToolCallViewState,
    queued_items: std::collections::VecDeque<QueuedItem>,
    find_bar: FindBar,
    unread_completed: bool,
    retry_status: Option<StreamingRetryStatus>,
}

impl ClientSessionState {
    fn with_chat(chat: Chat) -> Self {
        Self {
            chat,
            input_draft: String::new(),
            stream: None,
            external_stream: None,
            tool_calls: ToolCallViewState::default(),
            queued_items: std::collections::VecDeque::new(),
            find_bar: FindBar::new(),
            unread_completed: false,
            retry_status: None,
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
    pub agents_dialog_state: AgentsDialogState,
    pub models_dialog_state: ModelsDialogState,
    pub variants_dialog_state: VariantsDialogState,
    pub themes_dialog_state: ThemesDialogState,
    themes_dialog_original_theme_index: usize,
    themes_dialog_original_dark_mode: bool,
    themes_dialog_committed: bool,
    pub connect_dialog_state: ConnectDialogState,
    connect_dialog_mode: ConnectDialogMode,
    provider_oauth_flow_state: ProviderOAuthFlowState,
    pub sessions_dialog_state: SessionsDialogState,
    pub move_session_dialog_state: MoveSessionDialogState,
    pub session_rename_dialog_state: SessionRenameDialogState,
    pub permission_dialog_state: PermissionDialogState,
    pub question_dialog_state: QuestionDialogState,
    pub terminal_session_dialog_state: TerminalSessionDialogState,
    pub remote_dialog_state: RemoteDialogState,
    pub skills_dialog_state: crate::views::SkillsDialogState,
    pub mcp_dialog_state: McpDialogState,
    pub command_palette_state: crate::views::command_palette::CommandPaletteState,
    pub find_bar: FindBar,
    pub storage_dialog_state: StorageDialogState,
    pub title_dialog_state: TitleDialogState,
    pub which_key_state: crate::views::which_key::WhichKeyState,
    pub timeline_dialog_state: crate::views::timeline_dialog::TimelineDialogState,
    pub jobs_dialog_state: JobsDialogState,
    /// Last-rendered jobs chip hit area (chat status line); set during render.
    jobs_chip_area: Option<ratatui::layout::Rect>,
    /// First Esc arms a double-Esc gesture (cancel while streaming, timeline when idle).
    /// Matches OpenCode: second Esc confirms; arm expires after [`Self::ESC_ARM_TIMEOUT`].
    esc_primed_at: Option<std::time::Instant>,
    pub copy_actions_dialog: Option<ActionDialog>,
    pub message_actions_index: Option<usize>,
    pub message_actions_dialog: Option<ActionDialog>,
    message_actions_return_focus: OverlayFocus,
    selection_action_bar: Option<SelectionActionBarState>,
    pending_chat_message_click: Option<usize>,
    pub api_key_input: crate::ui::components::api_key_input::ApiKeyInput,
    provider_oauth_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<ProviderOAuthTaskMessage>>,
    provider_oauth_in_progress: Option<OAuthProvider>,
    mcp_oauth_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<McpOAuthTaskMessage>>,
    mcp_oauth_in_progress: Option<String>,
    compaction_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<CompactionTaskMessage>>,
    compaction_pending: Option<CompactionPending>,
    storage_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<StorageTaskMessage>>,
    models_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<ModelsTaskMessage>>,
    models_dialog_provider_ids: Option<Vec<String>>,
    title_generation_receiver:
        Option<tokio::sync::mpsc::UnboundedReceiver<TitleGenerationTaskMessage>>,
    pub prefs_dao: Option<crate::persistence::PrefsDAO>,
    pub agent: String,
    pub agent_registry: crate::agent::definition::AgentRegistry,
    pub agent_steps: std::collections::HashMap<String, usize>,
    pub provider_timeouts: std::collections::HashMap<String, crate::config::ProviderTimeout>,
    pub model: String,
    pub provider_name: String,
    small_model: Option<SmallModelConfig>,
    // Reasoning/thinking effort is loaded from persisted preferences once, then kept process-local.
    // Changes are persisted for future starts but are not re-read into other running terminals.
    reasoning_efforts: ReasoningEffortOverrides,
    model_reasoning_options: ModelReasoningOptions,
    pub cwd: String,
    pub base_focus: BaseFocus,
    pub overlay_focus: OverlayFocus,
    just_closed_overlay: bool,
    ctrl_c_press_count: u8,
    last_ctrl_c_time: std::time::Instant,
    pub themes: Vec<Theme>,
    pub current_theme_index: usize,
    pub dark_mode: bool,
    /// When true, main UI background is Color::Reset (terminal shows through).
    pub theme_transparent: bool,
    pub sounds: crate::sound::ResolvedSoundsConfig,
    pub notifications: crate::config::NotificationsConfig,
    pub images: crate::config::ImagesConfig,
    pub editor: crate::config::EditorConfig,
    pending_editor_suspend: Option<String>,
    pub websearch: crate::config::configuration::WebsearchConfig,
    pub mcp: crate::config::configuration::McpConfig,
    mcp_manager: Option<std::sync::Arc<tokio::sync::Mutex<crate::mcp::McpManager>>>,
    mcp_summary: crate::views::home::McpSummary,
    pub config_raw_merged: serde_json::Value,
    custom_instructions: String,
    terminal_focused: bool,
    pub tool_permissions: crate::tools::ToolPermissions,
    pub skills_dirs: Vec<std::path::PathBuf>,
    pub is_streaming: bool,
    pending_session_title: Option<String>,
    session_view_states: std::collections::HashMap<String, ClientSessionState>,
    session_spinner_frame: usize,
    stream_drain_rotation: usize,
    sessions_dialog_live_dirty: bool,
    last_sessions_dialog_metadata_probe: std::time::Instant,
    last_frame_size: ratatui::layout::Rect,
    last_animation_update: std::time::Instant,
    /// Last keyboard/mouse/paste (or Home entry). Home blink runs only briefly after this.
    last_user_activity: std::time::Instant,
    last_session_spinner_update: std::time::Instant,
    cached_git_branch: Option<String>,
    cached_git_branch_path: String,
    last_git_branch_check: std::time::Instant,
    discovery: Option<crate::model::discovery::Discovery>,
    cached_usage_text: String,
    cached_usage_check: (usize, u64, usize),
    cached_usage_streaming_base: Option<StreamingUsageBase>,
    terminal_title_enabled: bool,
    terminal_title_items: Vec<crate::terminal_title::TerminalTitleItem>,
    terminal_title_last: Option<String>,
    terminal_title_animation_origin: std::time::Instant,
    remote_launch_request: Option<RemoteLaunchRequest>,
    /// False until config/prefs/themes/skills hydrate after first paint.
    startup_hydrated: bool,
    pending_model_override: Option<String>,
    pending_cli_agent: Option<String>,
    /// Shared background/interactive process registry (jobs UI next).
    pub process_registry: std::sync::Arc<crate::tools::ProcessRegistry>,
}

/// Cached sum of context tokens for all completed messages of the currently
/// viewed streaming session; only the streaming message changes per refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamingUsageBase {
    session_id: Option<String>,
    message_count: usize,
    streaming_idx: Option<usize>,
    base_tokens: usize,
}

impl App {
    const INTERRUPTED_TURN_CONTINUATION_GUIDANCE: &'static str = "The previous turn was interrupted. Address the newest request, then resume unfinished work unless the user canceled or redirected it. Do not claim completion prematurely.";

    fn apply_turn_guidance(
        messages: &mut Vec<crate::session::types::Message>,
        turn_guidance: Option<&str>,
    ) {
        let Some(guidance) = turn_guidance
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };

        if let Some(system_message) = messages
            .iter_mut()
            .find(|message| message.role == crate::session::types::MessageRole::System)
        {
            if !system_message.content.trim().is_empty() {
                system_message.content.push_str("\n\n");
            }
            system_message.content.push_str(guidance);
        } else {
            messages.insert(0, crate::session::types::Message::system(guidance));
        }
    }

    pub fn new() -> Result<Self> {
        Self::new_with_model_override(None, None)
    }

    /// Load SQLite session index if needed. Deferred past first TUI paint.
    pub fn ensure_session_history(&mut self) {
        let _ = self.session_manager.ensure_history();
    }

    pub fn new_with_model_override(
        model_override: Option<&str>,
        cli_agent: Option<&str>,
    ) -> Result<Self> {
        Self::new_shell(model_override, cli_agent)
    }

    /// Minimal App for first paint. Heavy config/prefs/themes/skills load in
    /// [`Self::ensure_startup_hydrated`].
    fn new_shell(model_override: Option<&str>, cli_agent: Option<&str>) -> Result<Self> {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);

        let mut input = Input::new();
        let placeholder = Self::get_random_placeholder();
        let placeholder_static: &'static str = Box::leak(placeholder.into_boxed_str());
        input.set_placeholder(placeholder_static);
        input.set_image_open_config(crate::config::ImagesConfig::default());

        let mut chat = Chat::new();
        chat.set_agent_mention_names(Vec::new());

        let popup = Popup::new();
        let home_state = init_home();
        let suggestions_popup_state = init_suggestions_popup(popup);
        let agents_dialog_state = init_agents_dialog("Select agent", vec![]);
        let models_dialog_state = init_models_dialog("Models", vec![]);
        let variants_dialog_state = VariantsDialogState::new();
        let themes_dialog_state = init_themes_dialog("Themes", vec![], false);
        let connect_dialog_state = init_connect_dialog();
        let provider_oauth_flow_state = init_provider_oauth_flow();
        let sessions_dialog_state = init_sessions_dialog("Sessions", vec![]);
        let move_session_dialog_state = init_move_session_dialog();
        let permission_dialog_state = init_permission_dialog();
        let question_dialog_state = init_question_dialog();
        let terminal_session_dialog_state = init_terminal_session_dialog();
        let remote_dialog_state = init_remote_dialog();
        let skills_dialog_state = crate::views::skills_dialog::init_skills_dialog("Skills", vec![]);
        let mcp_dialog_state = init_mcp_dialog("MCP", vec![]);
        let which_key_state = crate::views::which_key::init_which_key();
        let timeline_dialog_state = crate::views::timeline_dialog::init_timeline_dialog();
        let jobs_dialog_state = init_jobs_dialog();
        let command_palette_state = init_command_palette();
        let find_bar = FindBar::new();
        let storage_dialog_state = init_storage_dialog();
        let title_dialog_state = init_title_dialog();
        let api_key_input = crate::ui::components::api_key_input::ApiKeyInput::new();
        let session_manager = SessionManager::new();

        let cwd_path = crate::utils::cwd::current_dir_or_dot();
        let cwd = cwd_path.display().to_string();

        let (active_model, active_provider_name) = if let Some(model) = model_override {
            let (provider, model_id) = parse_model_ref(model);
            (model_id, provider)
        } else {
            ("big-pickle".to_string(), "opencode".to_string())
        };
        let agent = cli_agent
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(titlecase_agent_name)
            .unwrap_or_else(|| "Build".to_string());

        // Resolve real theme before first paint (avoids builtin flash).
        let prefs_dao = crate::persistence::PrefsDAO::new().ok();
        let prefs_theme_id = prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_active_theme().ok().flatten());
        let theme_transparent = prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_theme_transparent().ok())
            .unwrap_or(false);
        let (themes, current_theme_index, dark_mode, theme_transparent) =
            crate::config::resolve_startup_theme(
                &cwd_path,
                prefs_theme_id.as_deref(),
                theme_transparent,
            );
        let theme_for_colors = themes
            .get(current_theme_index)
            .or_else(|| themes.first())
            .cloned()
            .unwrap_or_else(theme::Theme::load_builtin_default);
        let colors = theme_for_colors.get_colors_with(dark_mode, theme_transparent);
        let chat_state = init_chat(chat, &agent, &colors, true);
        let session_rename_dialog_state = init_session_rename_dialog(colors);
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
            agents_dialog_state,
            models_dialog_state,
            variants_dialog_state,
            themes_dialog_state,
            themes_dialog_original_theme_index: 0,
            themes_dialog_original_dark_mode: true,
            themes_dialog_committed: false,
            connect_dialog_state,
            connect_dialog_mode: ConnectDialogMode::ProviderSelection,
            provider_oauth_flow_state,
            sessions_dialog_state,
            move_session_dialog_state,
            session_rename_dialog_state,
            permission_dialog_state,
            question_dialog_state,
            terminal_session_dialog_state,
            remote_dialog_state,
            skills_dialog_state,
            mcp_dialog_state,
            command_palette_state,
            find_bar,
            storage_dialog_state,
            title_dialog_state,
            which_key_state,
            timeline_dialog_state,
            jobs_dialog_state,
            jobs_chip_area: None,
            esc_primed_at: None,
            copy_actions_dialog: None,
            message_actions_index: None,
            message_actions_dialog: None,
            message_actions_return_focus: OverlayFocus::TimelineDialog,
            selection_action_bar: None,
            pending_chat_message_click: None,
            api_key_input,
            provider_oauth_receiver: None,
            provider_oauth_in_progress: None,
            mcp_oauth_receiver: None,
            mcp_oauth_in_progress: None,
            compaction_receiver: None,
            compaction_pending: None,
            storage_receiver: None,
            models_receiver: None,
            models_dialog_provider_ids: None,
            title_generation_receiver: None,
            prefs_dao,
            agent,
            agent_registry: crate::agent::definition::AgentRegistry::default(),
            agent_steps: std::collections::HashMap::new(),
            provider_timeouts: std::collections::HashMap::new(),
            model: active_model,
            provider_name: active_provider_name,
            small_model: None,
            reasoning_efforts: ReasoningEffortOverrides::new(),
            model_reasoning_options: ModelReasoningOptions::new(),
            cwd,
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            just_closed_overlay: false,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: now,
            themes,
            current_theme_index,
            dark_mode,
            theme_transparent,
            sounds: crate::sound::ResolvedSoundsConfig::default(),
            notifications: crate::config::NotificationsConfig::default(),
            images: crate::config::ImagesConfig::default(),
            editor: crate::config::EditorConfig::default(),
            pending_editor_suspend: None,
            websearch: crate::config::configuration::WebsearchConfig::default(),
            mcp: crate::config::configuration::McpConfig::default(),
            mcp_manager: None,
            mcp_summary: crate::views::home::McpSummary::default(),
            config_raw_merged: serde_json::json!({}),
            custom_instructions: String::new(),
            terminal_focused: true,
            tool_permissions: crate::tools::ToolPermissions::new(cwd_path.clone()),
            skills_dirs: Vec::new(),
            is_streaming: false,
            pending_session_title: None,
            session_view_states: std::collections::HashMap::new(),
            session_spinner_frame: 0,
            stream_drain_rotation: 0,
            sessions_dialog_live_dirty: true,
            last_sessions_dialog_metadata_probe: now,
            last_frame_size: ratatui::layout::Rect::default(),
            last_animation_update: now,
            last_user_activity: now,
            last_session_spinner_update: now,
            cached_git_branch: None,
            cached_git_branch_path: String::new(),
            last_git_branch_check: now,
            discovery: None,
            cached_usage_text: String::new(),
            cached_usage_check: (0, 0, 0),
            cached_usage_streaming_base: None,
            terminal_title_enabled: crate::notify::terminal_title_supported(),
            terminal_title_items: crate::terminal_title::default_items(),
            terminal_title_last: None,
            terminal_title_animation_origin: now,
            remote_launch_request: None,
            startup_hydrated: false,
            pending_model_override: model_override.map(str::to_string),
            pending_cli_agent: cli_agent.map(str::to_string),
            process_registry: std::sync::Arc::new(crate::tools::ProcessRegistry::with_workdir(
                cwd_path,
            )),
        })
    }

    /// Load config/prefs/themes/skills after first paint (or immediately for remote/CLI).
    pub fn ensure_startup_hydrated(&mut self) -> Result<()> {
        if self.startup_hydrated {
            return Ok(());
        }

        let model_override = self.pending_model_override.as_deref();
        let cli_agent = self.pending_cli_agent.as_deref();
        let cwd_path = crate::utils::cwd::current_dir_or_dot();

        // Prefer prefs already opened in new_shell (avoids double SQLite open).
        let prefs_dao = match self.prefs_dao.take() {
            Some(dao) => Some(dao),
            None => match crate::persistence::PrefsDAO::new() {
                Ok(dao) => Some(dao),
                Err(e) => {
                    crate::startup_diag!("Warning: Failed to initialize preferences DAO: {}", e);
                    None
                }
            },
        };

        let loaded_config = crate::config::ConfigLoader::load()?;
        let mut mcp_config = loaded_config.merged_config.mcp.clone();
        crate::remote_mcp::apply_mcp_overrides(&mut mcp_config, prefs_dao.as_ref());
        if !mcp_config.is_empty() {
            let warm_cwd =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            self.mcp_manager = Some(crate::mcp::McpManager::ensure(mcp_config.clone(), warm_cwd));
        }
        self.input
            .set_image_open_config(loaded_config.merged_config.images.clone());
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
            self.command_registry.register_custom(command);
        }
        crate::command::handlers::register_skill_commands(&mut self.command_registry);
        let agent_registry = loaded_config.merged_config.agent_registry.clone();
        self.chat_state
            .chat
            .set_agent_mention_names(agent_registry.visible_agent_names_for_mentions());
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
        self.input.autocomplete = Some(
            AutoComplete::new_at_with_file_config(
                crate::autocomplete::CommandAuto::new(&self.command_registry),
                &cwd_path,
                loaded_config.merged_config.watcher.is_enabled(),
                loaded_config.merged_config.watcher.ignored_paths().to_vec(),
            )
            .with_agents(agent_suggestions),
        );

        let mut agent = self.agent.clone();
        if let Some(default_agent) = loaded_config.merged_config.default_agent.clone() {
            if !default_agent.trim().is_empty() && self.pending_cli_agent.is_none() {
                agent = default_agent;
            }
        }
        if let Some(name) = cli_agent.map(str::trim).filter(|name| !name.is_empty()) {
            if agent_registry.primary_agent(name).is_none() {
                anyhow::bail!(
                    "Unknown agent '{}'. Available: {}",
                    name,
                    agent_registry.visible_primary_agent_names().join(", ")
                );
            }
            agent = titlecase_agent_name(name);
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
        let terminal_title_items = prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_terminal_title_items().ok().flatten())
            .unwrap_or_else(crate::terminal_title::default_items);

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
        let small_model = loaded_config
            .merged_config
            .small_model
            .as_deref()
            .map(parse_model_ref)
            .map(|(provider, model)| SmallModelConfig { provider, model });

        let reasoning_efforts = prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_preferences().ok())
            .map(|prefs| reasoning_effort_overrides_from_prefs(&prefs))
            .unwrap_or_default();

        // Theme already resolved in new_shell for first paint; keep it.
        let agent_steps = agent_registry.max_steps_map();
        let provider_timeouts = loaded_config.merged_config.provider_timeouts.clone();
        let colors = self.get_current_theme_colors();

        let configured_compact_mode = loaded_config.merged_config.tui_compact_mode;
        let persisted_compact_mode = if configured_compact_mode.is_none() {
            prefs_dao
                .as_ref()
                .and_then(|dao| dao.get_compact_mode().ok().flatten())
        } else {
            None
        };
        let compact_mode = configured_compact_mode
            .or(persisted_compact_mode)
            .unwrap_or(true);
        self.chat_state.compact_mode = compact_mode;
        let agent_color = crate::theme::agent_color(&agent, &colors);
        self.chat_state.wave_spinner.set_color(agent_color);
        self.session_rename_dialog_state.set_colors(colors);

        let runtime = crate::config::ConfigRuntime::from_merged(
            &loaded_config.merged_config,
            cwd_path.clone(),
            crate::config::ConfigRuntimeOptions::default(),
        );

        self.prefs_dao = prefs_dao;
        self.agent = agent;
        self.agent_registry = agent_registry;
        self.agent_steps = agent_steps;
        self.provider_timeouts = provider_timeouts;
        self.model = active_model;
        self.provider_name = active_provider_name;
        self.small_model = small_model;
        self.reasoning_efforts = reasoning_efforts;
        self.sounds = resolved_sounds;
        self.notifications = loaded_config.merged_config.notifications.clone();
        self.images = loaded_config.merged_config.images.clone();
        self.editor = loaded_config.merged_config.editor.clone();
        self.websearch = loaded_config.merged_config.websearch.clone();
        self.mcp = mcp_config;
        self.config_raw_merged = loaded_config.raw_merged;
        self.custom_instructions = runtime.custom_instructions;
        self.tool_permissions = runtime.tool_permissions;
        self.skills_dirs = loaded_config.inventory.opencode_skills_dirs;
        self.discovery = runtime.discovery;
        self.terminal_title_items = terminal_title_items;
        self.startup_hydrated = true;
        self.pending_model_override = None;
        self.pending_cli_agent = None;
        Ok(())
    }

    fn open_variants_dialog(&mut self, args: &[String]) {
        if !args.is_empty() {
            push_toast(Toast::new(
                "Usage: /variants",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        let Some(capability) =
            self.reasoning_capability_for_model(&self.provider_name, &self.model)
        else {
            push_toast(Toast::new(
                "The active model has no variants",
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        };
        if capability.values().is_empty() {
            push_toast(Toast::new(
                "The active model has no variants",
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        let selected = self.reasoning_effort_override_for_model(&self.provider_name, &self.model);
        self.variants_dialog_state.show(&capability, selected);
        self.overlay_focus = OverlayFocus::VariantsDialog;
    }

    fn select_variant(&mut self) {
        let Some(effort) = self.variants_dialog_state.selected_effort() else {
            return;
        };
        let provider_id = self.provider_name.clone();
        let model_id = self.model.clone();
        match self.set_reasoning_effort_override_for_model(provider_id, model_id, effort) {
            Ok(()) => self.variants_dialog_state.dialog.hide(),
            Err(error) => push_toast(Toast::new(
                format!("Failed to save variant: {error}"),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            )),
        }
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
            crate::notify::notify_event_with_options(
                event,
                detail,
                crate::notify::NotificationOptions {
                    workspace_name: Some(self.terminal_title_project_name()),

                    #[cfg(target_os = "macos")]
                    macos_backend: self.notifications.macos_backend,
                },
            );
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
            crate::sound::SoundEvent::SubagentComplete => {
                self.notifications.subagent_complete.terminal
            }
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

        let items = self.terminal_title_items.clone();
        match self.terminal_title_text_for_items(&items) {
            Some(title) if self.terminal_title_last.as_deref() != Some(title.as_str()) => {
                if crate::notify::set_terminal_title(&title).is_ok() {
                    self.terminal_title_last = Some(title);
                }
            }
            None => self.clear_terminal_title_signal(),
            _ => {}
        }
    }

    pub fn clear_terminal_title_signal(&mut self) {
        if self.terminal_title_last.take().is_some() {
            let _ = crate::notify::clear_terminal_title();
        }
    }

    fn terminal_title_text(&mut self) -> String {
        let items = self.terminal_title_items.clone();
        self.terminal_title_text_for_items(&items)
            .unwrap_or_default()
    }

    fn terminal_title_text_for_items(
        &mut self,
        items: &[crate::terminal_title::TerminalTitleItem],
    ) -> Option<String> {
        let mut title = String::new();
        let mut previous = None;

        for item in items.iter().copied() {
            let value = match item {
                crate::terminal_title::TerminalTitleItem::Activity => {
                    if self.terminal_title_requires_action() {
                        Some("[!]".to_string())
                    } else if self.terminal_title_has_active_progress() {
                        Some(self.terminal_title_spinner_frame().to_string())
                    } else {
                        None
                    }
                }
                crate::terminal_title::TerminalTitleItem::ProjectName => {
                    Some(self.terminal_title_project_name())
                }
                crate::terminal_title::TerminalTitleItem::RunState => {
                    Some(self.terminal_title_run_state().to_string())
                }
                crate::terminal_title::TerminalTitleItem::ThreadTitle => {
                    self.terminal_title_thread_title().map(ToOwned::to_owned)
                }
                crate::terminal_title::TerminalTitleItem::ThreadTitleTruncated => self
                    .terminal_title_thread_title()
                    .map(|value| Self::truncate_terminal_title_part(value, 48)),
                crate::terminal_title::TerminalTitleItem::GitBranch => {
                    let cwd = self.active_workspace_path();
                    self.current_git_branch(&cwd)
                        .map(|branch| Self::truncate_terminal_title_part(&branch, 32))
                }
            };

            let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
                continue;
            };
            title.push_str(item.separator_from_previous(previous));
            title.push_str(&value);
            previous = Some(item);
        }

        (!title.is_empty()).then_some(title)
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
            OverlayFocus::PermissionDialog
                | OverlayFocus::QuestionDialog
                | OverlayFocus::TerminalSessionDialog
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

    fn terminal_title_run_state(&self) -> &'static str {
        if !self.terminal_title_has_active_progress() {
            return "Ready";
        }

        let is_thinking = self.chat_state.chat.messages.last().is_some_and(|message| {
            !message.is_complete
                && message
                    .reasoning
                    .as_deref()
                    .is_some_and(|reasoning| !reasoning.trim().is_empty())
                && message.content.trim().is_empty()
        });
        if is_thinking {
            "Thinking"
        } else {
            "Working"
        }
    }

    fn terminal_title_thread_title(&mut self) -> Option<&str> {
        self.session_manager
            .get_current_session()
            .map(|session| session.title.trim())
            .filter(|title| !title.is_empty())
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
        Self::completion_notification_stats_for_chat(&self.chat_state.chat)
    }

    fn completion_notification_stats_for_chat(chat: &Chat) -> Option<String> {
        let message = chat.messages.iter().rev().find(|msg| {
            msg.role == crate::session::types::MessageRole::Assistant && msg.is_complete
        })?;

        let format_tps = |precomputed: Option<f64>, tokens: usize, decode_ms: u64| -> Option<f64> {
            if let Some(tps) = precomputed {
                if tps.is_finite() && tps > 0.0 {
                    return Some(tps);
                }
            }
            // OpenCode inter-token: (n - 1) / duration; need >1 token.
            if decode_ms == 0 || tokens < 2 {
                return None;
            }
            let tps = ((tokens - 1) as f64) / (decode_ms as f64 / 1000.0);
            if tps.is_finite() && tps > 0.0 {
                Some(tps)
            } else {
                None
            }
        };

        if let (Some(t0), Some(t1), Some(tn)) = (message.t0_ms, message.t1_ms, message.tn_ms) {
            let output_tokens = message.output_tokens.or(message.token_count).unwrap_or(0);
            let ttft_ms = t1.saturating_sub(t0);
            let decode_ms = message.duration_ms.unwrap_or_else(|| tn.saturating_sub(t1));
            let total_ms = ttft_ms.saturating_add(decode_ms);
            let total_sec = total_ms as f64 / 1000.0;

            if let Some(tokens_per_sec) =
                format_tps(message.tokens_per_sec, output_tokens, decode_ms)
            {
                return Some(format!("{:.1}s | {:.0}t/s", total_sec, tokens_per_sec));
            }
            return Some(format!("{:.1}s", total_sec));
        }

        if let (Some(token_count), Some(duration_ms)) = (message.token_count, message.duration_ms) {
            let duration_sec = duration_ms as f64 / 1000.0;
            if let Some(tokens_per_sec) =
                format_tps(message.tokens_per_sec, token_count, duration_ms)
            {
                return Some(format!("{:.1}s | {:.0}t/s", duration_sec, tokens_per_sec));
            }
            return Some(format!("{:.1}s", duration_sec));
        }

        None
    }

    fn is_active_session(&self, session_id: &str) -> bool {
        self.session_manager
            .get_current_session_id()
            .is_some_and(|current| current == session_id)
    }

    /// Build a Chat pre-seeded with the current agent mention names so
    /// `@mentions` render with the same colors as the input composer.
    fn new_chat(&self) -> Chat {
        Chat::new().with_agent_mention_names(self.agent_registry.visible_agent_names_for_mentions())
    }

    fn chat_with_messages(&self, messages: Vec<crate::session::types::Message>) -> Chat {
        Chat::with_messages(messages)
            .with_agent_mention_names(self.agent_registry.visible_agent_names_for_mentions())
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
            ClientSessionState::with_chat(self.chat_with_messages(messages)),
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
            state.find_bar = std::mem::take(&mut self.find_bar);
            state.input_draft = if is_child_session {
                String::new()
            } else {
                self.input.submission_text()
            };
        }
    }

    /// Free the rebuildable render caches of background chats that are not
    /// part of the current session family (shared root session). Chats inside
    /// the family keep their caches so cycling between subagent tabs stays a
    /// warm-cache render, while memory does not scale with every session
    /// visited during a run.
    fn release_render_caches_outside_current_family(&mut self) {
        let current_root = self
            .session_manager
            .get_current_session_id()
            .and_then(|id| self.session_manager.root_session_id_for(id));
        let current_id = self.session_manager.get_current_session_id().cloned();
        let manager = &self.session_manager;

        for (id, state) in self.session_view_states.iter_mut() {
            if current_id.as_deref() == Some(id.as_str()) {
                continue;
            }
            let in_family = current_root
                .as_deref()
                .is_some_and(|root| manager.root_session_id_for(id).as_deref() == Some(root));
            if !in_family {
                state.chat.release_render_caches();
            }
        }
    }

    fn load_session_view_state(&mut self, session_id: &str) {
        self.ensure_session_view_state(session_id);
        let is_child_session = self.session_manager.parent_id_of(session_id).is_some();

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            self.chat_state.chat = std::mem::take(&mut state.chat);
            self.find_bar = std::mem::take(&mut state.find_bar);
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
        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
    }

    fn switch_to_session(&mut self, session_id: &str) -> bool {
        if !self.session_manager.ensure_session_loaded(session_id) {
            return false;
        }
        self.save_active_session_view_state();
        self.session_manager.switch_session(session_id);
        self.pending_session_title = None;
        self.load_session_view_state(session_id);
        self.release_render_caches_outside_current_family();
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

    fn open_selection_in_editor(&mut self) -> bool {
        let Some(location) = self.selected_chat_editor_location() else {
            push_toast(Toast::new(
                "Selection is not on an editable code line",
                ToastLevel::Error,
                None,
            ));
            return true;
        };

        match crate::utils::image_attachment::open_file_path_at_location(
            &location.path,
            location.line,
            location.column,
            &self.editor,
        ) {
            Ok(crate::utils::image_attachment::OpenOutcome::Spawned) => {
                push_toast(Toast::new(
                    format!(
                        "Opened {}:{}:{}",
                        location.path.display(),
                        location.line,
                        location.column
                    ),
                    ToastLevel::Info,
                    None,
                ));
                self.dismiss_selection_actions();
            }
            Ok(crate::utils::image_attachment::OpenOutcome::Copied(text)) => {
                push_toast(Toast::new(
                    format!("Copied {}", text),
                    ToastLevel::Info,
                    None,
                ));
                self.dismiss_selection_actions();
            }
            Ok(crate::utils::image_attachment::OpenOutcome::Suspend(command)) => {
                self.pending_editor_suspend = Some(command);
                self.dismiss_selection_actions();
            }
            Err(err) => push_toast(Toast::new(
                format!("Failed to open editor: {}", err),
                ToastLevel::Error,
                None,
            )),
        }
        true
    }

    fn active_primary_agent_definition(&self) -> Option<crate::agent::definition::AgentDefinition> {
        self.agent_registry.primary_agent(&self.agent).cloned()
    }

    fn active_primary_agent_model_provider(&self) -> (String, String) {
        self.active_primary_agent_definition()
            .and_then(|agent| agent.model)
            .map(|model| parse_model_ref(&model))
            .unwrap_or_else(|| (self.provider_name.clone(), self.model.clone()))
    }

    fn active_primary_agent_reasoning_effort(
        &self,
    ) -> Option<crate::model::reasoning::ReasoningEffort> {
        self.active_primary_agent_definition()
            .and_then(|agent| agent.reasoning_effort)
            .or_else(|| self.active_reasoning_effort())
    }

    fn is_subagent_session_active(&self) -> bool {
        self.session_manager
            .get_current_session_id()
            .is_some_and(|id| self.session_manager.parent_id_of(id).is_some())
    }

    /// Rows under the chat viewport (queue/input/help/status) for dialog overlap math.
    fn dialog_below_chat_height(&self, size: ratatui::layout::Rect) -> u16 {
        let is_subagent = self.is_subagent_session_active();
        let input_height = if is_subagent {
            SUBAGENT_FOOTER_HEIGHT
        } else {
            self.input.get_height_for_width(size.width)
        };
        let help_height = if is_subagent { 0 } else { 1 };
        let queue_height = if is_subagent {
            0
        } else {
            crate::views::chat::queued_messages_height(
                &self.queued_message_previews_for_current_session(),
            )
        };
        // Matches render_chat: queue + input + help + inner status row + outer status bar.
        queue_height
            .saturating_add(input_height)
            .saturating_add(help_height)
            .saturating_add(1)
            .saturating_add(1)
    }

    fn should_handle_child_session_arrow(&self) -> bool {
        if self.base_focus != BaseFocus::Chat {
            return false;
        }

        self.session_manager
            .get_current_session_id()
            .is_some_and(|id| self.session_manager.parent_id_of(id).is_some())
    }

    fn switch_to_latest_child_session(&mut self) -> bool {
        let Some(current_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        let Some(root_id) = self.session_manager.root_session_id_for(&current_id) else {
            return false;
        };
        let Some(latest_child) = self
            .session_manager
            .descendant_sessions(&root_id)
            .last()
            .cloned()
        else {
            return false;
        };

        self.switch_to_session(&latest_child.id)
    }

    fn switch_to_parent_session(&mut self) -> bool {
        let Some(current_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        let Some(parent_id) = self.session_manager.root_session_id_for(&current_id) else {
            return false;
        };
        if parent_id == current_id {
            return false;
        }

        self.switch_to_session(&parent_id)
    }

    fn switch_child_session(&mut self, direction: isize) -> bool {
        let Some(current_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        let Some(root_id) = self.session_manager.root_session_id_for(&current_id) else {
            return false;
        };

        let children = self.session_manager.descendant_sessions(&root_id);
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

    pub fn subagent_tabs_for_current_session(&self) -> Option<SubagentTabs> {
        let current_id = self.session_manager.get_current_session_id()?.clone();
        let root_id = self.session_manager.root_session_id_for(&current_id)?;
        let root = self.session_manager.get_session_ref(&root_id)?;
        let children = self.session_manager.descendant_sessions(&root_id);
        if children.is_empty() {
            return None;
        }

        let mut tabs = Vec::with_capacity(children.len() + 1);
        let root_agent = self.agent.clone();
        let root_model = self
            .session_active_stream_model(&root_id)
            .unwrap_or_else(|| self.model.clone());
        tabs.push(SubagentTab {
            session_id: root_id.clone(),
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
                session_id: child.id.clone(),
                label,
                agent,
                model,
                active: current_id == child.id,
                running,
                color: agent_color_for_tab(idx, &colors),
            });
        }

        Some(SubagentTabs {
            root_session_id: root_id.clone(),
            is_child_session: current_id != root_id,
            tabs,
        })
    }

    fn current_session_agent_model_for_display(&self) -> (String, String) {
        let Some(session_id) = self.session_manager.get_current_session_id() else {
            return (self.agent.clone(), self.model.clone());
        };
        if self.session_manager.parent_id_of(session_id).is_none() {
            let (_, configured_model) = self.active_primary_agent_model_provider();
            return (
                self.agent.clone(),
                self.session_active_stream_model(session_id)
                    .unwrap_or(configured_model),
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
        self.note_user_activity();
        self.sync_active_streaming_flag();
        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
        self.refresh_sessions_dialog();
    }

    fn create_new_session(&mut self, title: Option<String>) -> String {
        self.save_active_session_view_state();
        self.pending_session_title = None;
        let session_id = self.session_manager.create_session(title);
        self.session_view_states.insert(
            session_id.clone(),
            ClientSessionState::with_chat(self.new_chat()),
        );
        self.chat_state.chat.clear();
        self.input.clear();
        self.base_focus = BaseFocus::Home;
        self.note_user_activity();
        self.sync_active_streaming_flag();
        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
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

    fn persist_chat_messages_for_session(&mut self, session_id: &str) -> bool {
        let Some(messages) = self
            .chat_for_session(session_id)
            .map(|chat| chat.messages.clone())
        else {
            return false;
        };

        self.session_manager
            .replace_session_messages(session_id, messages)
            .is_ok()
    }

    fn mark_streaming_snapshot_pending(&mut self, session_id: &str) {
        if let Some(stream) = self.stream_for_session_mut(session_id) {
            stream.pending_message_snapshot = true;
        } else if let Some(stream) = self
            .session_view_states
            .get_mut(session_id)
            .and_then(|state| state.external_stream.as_mut())
        {
            stream.pending_message_snapshot = true;
        }
    }

    fn maybe_persist_streaming_snapshot_for_session(&mut self, session_id: &str, force: bool) {
        let should_persist = self
            .session_view_states
            .get(session_id)
            .is_some_and(|state| {
                state.stream.as_ref().is_some_and(|stream| {
                    stream.pending_message_snapshot
                        && (force
                            || stream.last_message_snapshot.elapsed()
                                >= STREAM_MESSAGE_SNAPSHOT_INTERVAL)
                }) || state.external_stream.as_ref().is_some_and(|stream| {
                    stream.pending_message_snapshot
                        && (force
                            || stream.last_message_snapshot.elapsed()
                                >= STREAM_MESSAGE_SNAPSHOT_INTERVAL)
                })
            });

        if !should_persist || !self.persist_chat_messages_for_session(session_id) {
            return;
        }

        if let Some(stream) = self.stream_for_session_mut(session_id) {
            stream.pending_message_snapshot = false;
            stream.last_message_snapshot = std::time::Instant::now();
        }
        if let Some(stream) = self
            .session_view_states
            .get_mut(session_id)
            .and_then(|state| state.external_stream.as_mut())
        {
            stream.pending_message_snapshot = false;
            stream.last_message_snapshot = std::time::Instant::now();
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

    fn current_session_retry_status(&self) -> Option<StreamingRetryStatus> {
        let session_id = self.session_manager.get_current_session_id()?;
        self.session_view_states
            .get(session_id)
            .and_then(|state| state.retry_status.clone())
    }

    fn has_active_retry_status(&self) -> bool {
        self.session_view_states
            .values()
            .any(|state| state.retry_status.is_some())
    }

    fn set_session_retry_status(&mut self, session_id: &str, status: Option<StreamingRetryStatus>) {
        self.ensure_session_view_state(session_id);
        if let Some(state) = self.session_view_states.get_mut(session_id) {
            if state.retry_status == status {
                return;
            }
            state.retry_status = status;
        }
    }

    fn session_has_active_compaction(&self, session_id: &str) -> bool {
        self.compaction_receiver.is_some()
            && self
                .compaction_pending
                .as_ref()
                .is_some_and(|pending| pending.session_id == session_id)
    }

    fn queued_message_previews_for_current_session(&self) -> Vec<String> {
        let Some(session_id) = self.session_manager.get_current_session_id() else {
            return Vec::new();
        };

        self.session_view_states
            .get(session_id)
            .map(|state| state.queued_items.iter().map(QueuedItem::preview).collect())
            .unwrap_or_default()
    }

    fn has_queued_messages_for_session(&self, session_id: &str) -> bool {
        self.session_view_states
            .get(session_id)
            .is_some_and(|state| !state.queued_items.is_empty())
    }

    fn has_queued_user_messages_for_session(&self, session_id: &str) -> bool {
        self.session_view_states
            .get(session_id)
            .is_some_and(|state| {
                state
                    .queued_items
                    .iter()
                    .any(|item| matches!(item, QueuedItem::Message(_)))
            })
    }

    fn queue_message_for_current_session(
        &mut self,
        text: String,
        image_paths: Vec<std::path::PathBuf>,
    ) -> bool {
        self.queue_item_for_current_session(QueuedItem::Message(QueuedUserMessage {
            text,
            image_paths,
        }))
    }

    fn queue_compact_for_current_session(&mut self) -> bool {
        self.queue_item_for_current_session(QueuedItem::Compact)
    }

    fn queue_item_for_current_session(&mut self, item: QueuedItem) -> bool {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };

        self.ensure_session_view_state(&session_id);
        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            // Avoid stacking duplicate /compact entries.
            if matches!(item, QueuedItem::Compact)
                && state
                    .queued_items
                    .iter()
                    .any(|queued| matches!(queued, QueuedItem::Compact))
            {
                return true;
            }
            state.queued_items.push_back(item);
            return true;
        }

        false
    }

    fn drain_queued_items_for_session(&mut self, session_id: &str) -> Vec<QueuedItem> {
        self.session_view_states
            .get_mut(session_id)
            .map(|state| state.queued_items.drain(..).collect())
            .unwrap_or_default()
    }

    fn combine_queued_messages(queued_messages: Vec<QueuedUserMessage>) -> QueuedUserMessage {
        let mut text_parts = Vec::with_capacity(queued_messages.len());
        let mut image_paths = Vec::new();

        for queued in queued_messages {
            let image_offset = image_paths.len();
            let image_count = queued.image_paths.len();
            let text = Self::queued_message_text_for_combined_submission(
                &queued.text,
                image_offset,
                image_count,
            );

            if !text.is_empty() {
                text_parts.push(text);
            }
            image_paths.extend(queued.image_paths);
        }

        QueuedUserMessage {
            text: text_parts.join("\n"),
            image_paths,
        }
    }

    fn queued_message_text_for_combined_submission(
        text: &str,
        image_offset: usize,
        image_count: usize,
    ) -> String {
        let text = Self::renumber_image_placeholders(text, image_offset, image_count);
        if !text.trim().is_empty() || image_count == 0 {
            return text;
        }

        (0..image_count)
            .map(|idx| format!("[Image #{}]", image_offset + idx + 1))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn renumber_image_placeholders(text: &str, image_offset: usize, image_count: usize) -> String {
        if image_offset == 0 || image_count == 0 || !text.contains("[Image #") {
            return text.to_string();
        }

        let mut output = String::with_capacity(text.len());
        let mut remaining = text;

        while let Some(start) = remaining.find("[Image #") {
            output.push_str(&remaining[..start]);

            let placeholder_start = &remaining[start..];
            let Some(end_offset) = placeholder_start.find(']') else {
                output.push_str(placeholder_start);
                return output;
            };
            let end = start + end_offset + 1;
            let placeholder = &remaining[start..end];

            let image_number = placeholder
                .strip_prefix("[Image #")
                .and_then(|value| value.strip_suffix(']'))
                .and_then(|value| value.parse::<usize>().ok());

            match image_number {
                Some(number) if (1..=image_count).contains(&number) => {
                    output.push_str(&format!("[Image #{}]", image_offset + number));
                }
                _ => output.push_str(placeholder),
            }

            remaining = &remaining[end..];
        }

        output.push_str(remaining);
        output
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
        let was_streaming = self.is_streaming;
        self.is_streaming = self.compaction_receiver.is_some()
            || self
                .session_manager
                .get_current_session_id()
                .and_then(|id| self.session_view_states.get(id))
                .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some());
        // Drop a cancel-arm if the stream finished before the second Esc.
        if was_streaming && !self.is_streaming {
            self.reset_esc_primed_state();
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

    fn session_usage_text(&mut self) -> String {
        let total_tokens = if self.is_streaming {
            self.streaming_context_tokens_cached()
        } else {
            crate::session::compaction::total_context_tokens(&self.chat_state.chat.messages)
        };
        let messages = &self.chat_state.chat.messages;

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
            let suffix = format!("last compact {}", stats.change_description());
            return append_usage_suffix(text, suffix);
        }

        text
    }

    /// Streaming variant of the usage token count with a cached base.
    ///
    /// `message_context_tokens` re-serializes tool args and re-parses tool
    /// payloads, so walking the whole transcript on every streaming layout
    /// refresh is O(transcript bytes). Completed messages cannot change while
    /// their count and the streaming index stay the same, so only the actively
    /// streaming message (already tracked by the chat's token counter) needs
    /// per-refresh accounting.
    fn streaming_context_tokens_cached(&mut self) -> usize {
        let session_id = self.session_manager.get_current_session_id().cloned();
        let messages = &self.chat_state.chat.messages;
        let message_count = messages.len();
        let streaming_idx = messages.iter().rposition(|message| {
            message.role == crate::session::types::MessageRole::Assistant && !message.is_complete
        });

        let cache_valid = self
            .cached_usage_streaming_base
            .as_ref()
            .is_some_and(|base| {
                base.session_id == session_id
                    && base.message_count == message_count
                    && base.streaming_idx == streaming_idx
            });
        if !cache_valid {
            let base_tokens = messages
                .iter()
                .enumerate()
                .filter(|(idx, _)| Some(*idx) != streaming_idx)
                .map(|(_, message)| crate::session::compaction::message_context_tokens(message))
                .sum();
            self.cached_usage_streaming_base = Some(StreamingUsageBase {
                session_id,
                message_count,
                streaming_idx,
                base_tokens,
            });
        }

        let base_tokens = self
            .cached_usage_streaming_base
            .as_ref()
            .map(|base| base.base_tokens)
            .unwrap_or(0);
        if streaming_idx.is_some() {
            base_tokens.saturating_add(self.chat_state.chat.streaming_token_count())
        } else {
            base_tokens
        }
    }

    fn reasoning_capability_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningCapability> {
        if let Some(capability) = self
            .model_reasoning_options
            .get(&(provider_id.to_string(), model_id.to_string()))
            .and_then(|options| crate::model::reasoning::capability_from_options(options))
        {
            return Some(capability);
        }

        self.discovery
            .as_ref()
            .and_then(|discovery| discovery.get_model_reasoning_capability(provider_id, model_id))
    }

    fn reasoning_effort_override_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningEffort> {
        self.reasoning_efforts
            .get(&(provider_id.to_string(), model_id.to_string()))
            .copied()
    }

    fn set_reasoning_effort_override_for_model(
        &mut self,
        provider_id: String,
        model_id: String,
        effort: Option<crate::model::reasoning::ReasoningEffort>,
    ) -> anyhow::Result<()> {
        if let Some(ref dao) = self.prefs_dao {
            if let Some(effort) = effort {
                dao.set_model_reasoning_effort(provider_id.clone(), model_id.clone(), effort)?;
            } else {
                dao.clear_model_reasoning_effort(&provider_id, &model_id)?;
            }
        }

        let key = (provider_id, model_id);
        if let Some(effort) = effort {
            self.reasoning_efforts.insert(key, effort);
        } else {
            self.reasoning_efforts.remove(&key);
        }

        Ok(())
    }

    fn start_session_title_generation(&mut self, session_id: &str, user_message: &str) {
        if self.small_model.is_none() || self.title_generation_receiver.is_some() {
            return;
        }
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        self.title_generation_receiver = Some(receiver);
        self.maybe_spawn_session_title_generation(session_id, user_message, sender);
    }

    fn maybe_spawn_session_title_generation(
        &self,
        session_id: &str,
        user_message: &str,
        sender: tokio::sync::mpsc::UnboundedSender<TitleGenerationTaskMessage>,
    ) {
        let Some(small_model) = self.small_model.clone() else {
            return;
        };
        let user_message = user_message.trim();
        if user_message.is_empty()
            || !self.session_prompt_is_first_user_message(session_id, user_message)
            || !self.session_has_auto_title_for_prompt(session_id, user_message)
        {
            return;
        }

        let session_id = session_id.to_string();
        let user_message = user_message.to_string();
        tokio::spawn(async move {
            match crate::llm::client::generate_session_title(
                small_model.provider,
                small_model.model,
                user_message,
            )
            .await
            {
                Ok(title) => {
                    let _ =
                        sender.send(TitleGenerationTaskMessage::Generated { session_id, title });
                }
                Err(err) => {
                    crate::emit_log!("Title generation skipped: {}", err);
                }
            }
        });
    }

    fn session_has_auto_title_for_prompt(&self, session_id: &str, prompt: &str) -> bool {
        self.session_manager
            .get_session_ref(session_id)
            .is_some_and(|session| is_auto_session_title_for_prompt(&session.title, prompt))
    }

    fn session_prompt_is_first_user_message(&self, session_id: &str, prompt: &str) -> bool {
        let Some(chat) = self.chat_for_session(session_id) else {
            return false;
        };
        let mut user_messages = chat
            .messages
            .iter()
            .filter(|message| message.role == crate::session::types::MessageRole::User);
        let Some(first) = user_messages.next() else {
            return false;
        };

        first.content.trim() == prompt.trim() && user_messages.next().is_none()
    }

    fn resolved_reasoning_effort_for_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningEffort> {
        let capability = self.reasoning_capability_for_model(provider_id, model_id)?;
        let requested = self.reasoning_effort_override_for_model(provider_id, model_id);
        let resolved = capability.resolve(requested)?;
        if resolved == crate::model::reasoning::ReasoningEffort::None {
            return None;
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

    fn model_name_for_display(&self, provider_id: &str, model_id: &str) -> String {
        self.discovery
            .as_ref()
            .and_then(|discovery| discovery.get_model_name(provider_id, model_id))
            .unwrap_or_else(|| model_id.to_string())
    }

    fn provider_name_for_display(&self, provider_id: &str) -> String {
        self.discovery
            .as_ref()
            .and_then(|discovery| discovery.get_provider_name(provider_id))
            .unwrap_or_else(|| provider_id.to_string())
    }

    fn refresh_mcp_summary(&mut self) {
        let enabled = self.mcp.values().filter(|server| server.enabled()).count();
        let Some(manager) = self.mcp_manager.as_ref() else {
            self.mcp_summary = crate::views::home::McpSummary {
                connected: 0,
                enabled,
                has_error: false,
            };
            return;
        };
        let Ok(manager) = manager.try_lock() else {
            return;
        };
        let views = manager.views();
        self.mcp_summary = crate::views::home::McpSummary {
            connected: views
                .iter()
                .filter(|server| server.enabled && server.status == "connected")
                .count(),
            enabled,
            has_error: views
                .iter()
                .any(|server| server.enabled && server.status == "failed"),
        };
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
        let current = self.reasoning_effort_override_for_model(&provider_id, &model_id);
        let Some(next) = capability.cycle_override(current, direction) else {
            return false;
        };

        self.set_reasoning_effort_override_for_model(provider_id, model_id, next)
            .is_ok()
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
        theme.get_colors_with(self.dark_mode, self.theme_transparent)
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

    pub fn remote_workspace_path(&self) -> String {
        self.session_manager
            .get_current_session_id()
            .and_then(|id| self.session_manager.get_session_ref(id))
            .map(|session| session.workspace_path.trim())
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let path = self.session_manager.current_workspace_path().trim();
                if path.is_empty() {
                    self.cwd.clone()
                } else {
                    path.to_string()
                }
            })
    }

    pub fn remote_workspace_name(&self) -> String {
        self.session_manager
            .get_current_session_id()
            .and_then(|id| self.session_manager.get_session_ref(id))
            .map(|session| session.workspace_name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let name = self.session_manager.current_workspace_name().trim();
                if !name.is_empty() {
                    return name.to_string();
                }

                std::path::Path::new(&self.remote_workspace_path())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or("Workspace")
                    .to_string()
            })
    }

    fn resolve_remote_workspace_path(&self, raw: &str) -> Result<std::path::PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            anyhow::bail!("folder path cannot be empty");
        }

        let expanded = if raw == "~" {
            dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(raw))
        } else if let Some(rest) = raw.strip_prefix("~/") {
            dirs::home_dir()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| std::path::PathBuf::from(raw))
        } else {
            std::path::PathBuf::from(raw)
        };

        let absolute = if expanded.is_absolute() {
            expanded
        } else {
            std::path::PathBuf::from(self.remote_workspace_path()).join(expanded)
        };
        let canonical = std::fs::canonicalize(&absolute).with_context(|| {
            format!("folder not found or not accessible: {}", absolute.display())
        })?;

        if !canonical.is_dir() {
            anyhow::bail!("folder path is not a directory: {}", canonical.display());
        }

        Ok(canonical)
    }

    fn set_remote_workspace_path(&mut self, path: std::path::PathBuf) -> Result<()> {
        let path = std::fs::canonicalize(&path)
            .with_context(|| format!("folder not found or not accessible: {}", path.display()))?;
        if !path.is_dir() {
            anyhow::bail!("folder path is not a directory: {}", path.display());
        }

        let path_text = path.to_string_lossy().to_string();
        std::env::set_current_dir(&path)
            .with_context(|| format!("failed to switch to {}", path.display()))?;
        self.cwd = path_text.clone();
        self.cached_git_branch_path.clear();
        self.tool_permissions = self.tool_permissions.clone().with_workdir(path.clone());
        self.process_registry.set_workdir_blocking(&path);
        self.session_manager
            .switch_current_workspace_path(&path_text)
            .map_err(|err| anyhow::anyhow!("{err:?}"))?;
        self.refresh_sessions_dialog();

        Ok(())
    }

    fn current_selection_action_bar_area(&self) -> Option<Rect> {
        self.selection_action_bar.map(|state| match state.target {
            SelectionActionTarget::Chat => chat_selection_action_bar_area(
                self.current_chat_area(),
                self.chat_state.chat.resolved_scroll_offset(),
                &self.chat_state.chat.selection,
                state,
            ),
            SelectionActionTarget::Input => input_selection_action_bar_area(
                self.last_frame_size,
                self.suggestions_popup_anchor_area(),
                self.input.selection_screen_row(),
            ),
            SelectionActionTarget::JobsDetail => {
                let content = self.jobs_dialog_state.detail_content_area();
                let selection = &self.jobs_dialog_state.selection;
                let ((s_line, _), (e_line, _)) = selection.range();
                let top_line = s_line.min(e_line);
                let scroll = self.jobs_dialog_state.detail_scroll as usize;
                let visible_row = top_line.saturating_sub(scroll) as u16;
                let row = content
                    .y
                    .saturating_add(visible_row)
                    .saturating_sub(1)
                    .max(content.y.saturating_sub(1));
                let width = selection_action_bar_width(state);
                let x = content
                    .x
                    .saturating_add(content.width.saturating_sub(width) / 2);
                Rect {
                    x,
                    y: row.min(content.y.saturating_add(content.height.saturating_sub(1))),
                    width: width.min(content.width.max(1)),
                    height: 1,
                }
            }
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
            if let Some(theme_id) = self
                .themes
                .get(self.current_theme_index)
                .map(|theme| theme.id.clone())
            {
                self.persist_theme_selection(&theme_id);
            }
        }
    }

    fn preview_theme_by_id(&mut self, theme_id: &str) {
        if let Some((idx, theme)) = self
            .themes
            .iter()
            .enumerate()
            .find(|(_, theme)| theme.id == theme_id)
        {
            self.current_theme_index = idx;
            self.dark_mode = matches!(theme.appearance, theme::ThemeAppearance::Dark);
        }
    }

    fn commit_theme_by_id(&mut self, theme_id: &str) -> Option<String> {
        let (idx, selected_theme_id, appearance) = self
            .themes
            .iter()
            .enumerate()
            .find(|(_, theme)| theme.id == theme_id)
            .map(|(idx, theme)| (idx, theme.id.clone(), theme.appearance))?;

        self.current_theme_index = idx;
        self.dark_mode = matches!(appearance, theme::ThemeAppearance::Dark);
        self.themes_dialog_committed = true;
        self.persist_theme_selection(&selected_theme_id);
        Some(selected_theme_id)
    }

    fn apply_theme_transparent(&mut self, transparent: bool) {
        self.theme_transparent = transparent;
        self.themes_dialog_state.set_transparent(transparent);
        if let Some(ref dao) = self.prefs_dao {
            if let Err(e) = dao.set_theme_transparent(transparent) {
                eprintln!("Failed to save theme transparency: {}", e);
            }
        }
    }

    fn persist_theme_selection(&self, theme_id: &str) {
        if let Some(ref dao) = self.prefs_dao {
            if let Err(e) = dao.set_active_theme(theme_id.to_string()) {
                eprintln!("Failed to save active theme: {}", e);
            }
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

        if let Some(text) = self.jobs_dialog_state.selected_text() {
            self.copy_text_with_toast(&text, "Copied to clipboard");
            self.jobs_dialog_state.clear_selection();
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
        if self.jobs_dialog_state.selection.active {
            self.jobs_dialog_state.clear_selection();
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
            SelectionActionTarget::JobsDetail => self.jobs_dialog_state.selected_text(),
        }
    }

    fn selected_chat_editor_location(&self) -> Option<crate::ui::components::chat::EditorLocation> {
        self.chat_state.chat.editor_location_for_selection()
    }

    fn show_selection_action_bar_for(&mut self, target: SelectionActionTarget) {
        let can_open_in_editor =
            target == SelectionActionTarget::Chat && self.selected_chat_editor_location().is_some();
        self.selection_action_bar =
            self.selected_text_for_action(target)
                .map(|_| SelectionActionBarState {
                    target,
                    can_open_in_editor,
                });
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
            KeyCode::Char('e')
                if key.modifiers == event::KeyModifiers::NONE && state.can_open_in_editor =>
            {
                self.open_selection_in_editor()
            }
            KeyCode::Esc if key.modifiers == event::KeyModifiers::NONE => {
                self.dismiss_selection_actions();
                self.reset_esc_primed_state();
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
        // Prefer the last-rendered chat content rect (excludes compact chrome).
        self.chat_state
            .last_chat_area
            .unwrap_or_else(|| self.chat_area_for_size(self.last_frame_size))
    }

    /// Forward chat mouse events while a permission/question dialog is open.
    /// Clicks on dialog controls are handled by the dialog; everything else
    /// (scroll + text selection) reaches the chat behind it.
    fn forward_chat_mouse_through_dialog(&mut self, mouse: MouseEvent) {
        if self.base_focus != BaseFocus::Chat {
            return;
        }

        let is_scroll = matches!(
            mouse.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
        );
        let is_selection = matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::Up(MouseButton::Left)
        );
        if !is_scroll && !is_selection {
            return;
        }

        let chat_area = self.current_chat_area();
        let was_dragging = self.chat_state.chat.selection.is_dragging;
        if !self.chat_state.chat.handle_mouse_event(mouse, chat_area) {
            return;
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.selection_action_bar = None;
        }

        // Same as the normal chat path: show actions as soon as a drag creates a
        // selection, because mouse-up may never arrive if released outside the terminal.
        if was_dragging
            && self.chat_state.chat.selection.is_dragging
            && matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left))
        {
            self.show_selection_action_bar_for(SelectionActionTarget::Chat);
        } else if was_dragging && !self.chat_state.chat.selection.is_dragging {
            self.show_selection_action_bar_for(SelectionActionTarget::Chat);
        }
    }

    /// Region where a mouse wheel scrolls the chat. In compact mode this
    /// extends above the chat content to include the 3-row header (and the
    /// sticky overlay which sits inside the transcript top), so scrolling
    /// works even when the pointer is over that chrome.
    fn chat_scroll_region(&self) -> Rect {
        let chat_area = self.current_chat_area();
        if !self.chat_state.compact_mode {
            return chat_area;
        }
        // Sticky is an overlay inside chat_area; only the header sits above it.
        let top = chat_area.y.saturating_sub(3); // header rows
        Rect {
            x: chat_area.x,
            y: top,
            width: chat_area.width,
            height: chat_area.bottom().saturating_sub(top),
        }
    }

    pub fn handle_coalesced_mouse_scroll(&mut self, mouse: MouseEvent, notches: usize) {
        if matches!(
            self.overlay_focus,
            OverlayFocus::None | OverlayFocus::FindBar
        ) && self.base_focus == BaseFocus::Chat
        {
            let chat_area = self.chat_scroll_region();
            if chat_area.contains(Position::new(mouse.column, mouse.row))
                && self
                    .chat_state
                    .chat
                    .handle_mouse_scroll(mouse.kind, notches)
            {
                return;
            }
        }

        for _ in 0..notches.max(1) {
            self.handle_mouse_event(mouse);
        }
    }

    pub fn handle_keys(&mut self, key: KeyEvent) {
        // Kitty keyboard protocol can report key releases. Without this, the
        // Enter release that arrives after `/models` or `/sessions` opens a
        // dialog is interpreted by that fresh dialog as a submit.
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.note_user_activity();

        if self.overlay_focus == OverlayFocus::FindBar && !self.can_open_find_bar() {
            self.close_find_bar_focus();
        }

        let overlay_before_key = if key.code == KeyCode::Esc {
            self.overlay_focus
        } else {
            OverlayFocus::None
        };

        if key.code != KeyCode::Esc {
            self.reset_esc_primed_state();
        }

        if self.overlay_focus == OverlayFocus::TerminalSessionDialog
            && self.terminal_session_dialog_state.has_active()
        {
            let job_id = self
                .terminal_session_dialog_state
                .active_job_id()
                .map(str::to_string);
            let resp = handle_terminal_session_dialog_key_event(
                &mut self.terminal_session_dialog_state,
                key,
            );
            match resp {
                TerminalSessionResponse::Close => {
                    if let Some(id) = job_id {
                        let _ = self.process_registry.kill_blocking(&id);
                    }
                    self.after_terminal_session_overlay_closed();
                }
                TerminalSessionResponse::Minimize => {
                    // Park session: keep running, dismiss overlay only.
                    self.overlay_focus = OverlayFocus::None;
                }
                TerminalSessionResponse::Handled | TerminalSessionResponse::NotHandled => {}
            }
            self.record_overlay_close_after_key(overlay_before_key);
            return;
        }

        if key.code == KeyCode::Char('p')
            && key.modifiers == event::KeyModifiers::CONTROL
            && matches!(
                self.overlay_focus,
                OverlayFocus::None | OverlayFocus::SuggestionsPopup | OverlayFocus::CommandPalette
            )
        {
            self.open_command_palette();
            self.record_overlay_close_after_key(overlay_before_key);
            return;
        }

        if key.code == KeyCode::Char('f')
            && key.modifiers == event::KeyModifiers::CONTROL
            && matches!(
                self.overlay_focus,
                OverlayFocus::None | OverlayFocus::SuggestionsPopup | OverlayFocus::FindBar
            )
            && self.can_open_find_bar()
        {
            self.open_find_bar();
            self.record_overlay_close_after_key(overlay_before_key);
            return;
        }

        // ctrl-t / ctrl-x must work while slash suggestions are open (e.g. `/compact|`).
        if key.code == KeyCode::Char('t')
            && key.modifiers == event::KeyModifiers::CONTROL
            && matches!(
                self.overlay_focus,
                OverlayFocus::None | OverlayFocus::SuggestionsPopup
            )
        {
            self.cycle_active_reasoning_effort();
            self.record_overlay_close_after_key(overlay_before_key);
            return;
        }
        if key.code == KeyCode::Char('x')
            && key.modifiers == event::KeyModifiers::CONTROL
            && matches!(
                self.overlay_focus,
                OverlayFocus::None | OverlayFocus::SuggestionsPopup
            )
        {
            self.overlay_focus = OverlayFocus::WhichKey;
            self.which_key_state
                .set_chat_active(self.base_focus == BaseFocus::Chat);
            self.which_key_state.show();
            self.record_overlay_close_after_key(overlay_before_key);
            return;
        }

        if self.handle_selection_action_key(key) {
            self.record_overlay_close_after_key(overlay_before_key);
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
                    self.record_overlay_close_after_key(overlay_before_key);
                    return;
                }
                self.handle_clipboard_image_paste();
                self.record_overlay_close_after_key(overlay_before_key);
                return;
            }
            KeyCode::Char('c') if key.modifiers == event::KeyModifiers::CONTROL => {
                if self.try_copy_selection() {
                    self.record_overlay_close_after_key(overlay_before_key);
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
                self.record_overlay_close_after_key(overlay_before_key);
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
            OverlayFocus::AgentsDialog => {
                let action = handle_agents_dialog_key_event(&mut self.agents_dialog_state, key);
                match action {
                    AgentsDialogAction::SelectAgent { agent } => {
                        self.select_agent_from_dialog(&agent);
                        self.overlay_focus = OverlayFocus::None;
                    }
                    AgentsDialogAction::None => {}
                }

                if !self.agents_dialog_state.dialog.is_visible() {
                    self.overlay_focus = OverlayFocus::None;
                }
                true
            }
            OverlayFocus::FindBar => {
                self.handle_find_bar_key(key);
                true
            }
            OverlayFocus::ModelsDialog => {
                if !self.models_dialog_state.is_loading()
                    && key.code == KeyCode::Char('a')
                    && key.modifiers == event::KeyModifiers::CONTROL
                {
                    self.models_dialog_state.dialog.hide();
                    if let crate::command::parser::InputType::Command(parsed) =
                        crate::command::parser::parse_input("/connect")
                    {
                        tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(self.process_command_input(parsed));
                        });
                    }
                    self.record_overlay_close_after_key(overlay_before_key);
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
                        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);

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
            OverlayFocus::RefreshModelsDialog => {
                if key.code == KeyCode::Esc {
                    self.overlay_focus = OverlayFocus::None;
                }
                true
            }
            OverlayFocus::ThemesDialog => {
                let action = handle_themes_dialog_key_event(&mut self.themes_dialog_state, key);

                match action {
                    crate::views::themes_dialog::ThemesDialogAction::PreviewTheme { theme_id } => {
                        self.preview_theme_by_id(&theme_id);
                    }
                    crate::views::themes_dialog::ThemesDialogAction::SelectTheme { theme_id } => {
                        if let Some(selected_theme_id) = self.commit_theme_by_id(&theme_id) {
                            push_toast(Toast::new(
                                format!("Theme: {}", selected_theme_id),
                                ToastLevel::Info,
                                None,
                            ));
                        }
                    }
                    crate::views::themes_dialog::ThemesDialogAction::ToggleTransparent => {
                        self.apply_theme_transparent(self.themes_dialog_state.transparent);
                    }
                    crate::views::themes_dialog::ThemesDialogAction::None => {}
                }

                if !self.themes_dialog_state.dialog.is_visible() {
                    if !self.themes_dialog_committed {
                        self.current_theme_index = self.themes_dialog_original_theme_index;
                        self.dark_mode = self.themes_dialog_original_dark_mode;
                    }
                    self.overlay_focus = OverlayFocus::None;
                }
                true
            }
            OverlayFocus::ConnectDialog => {
                if key.code == KeyCode::Char('d') && key.modifiers == event::KeyModifiers::CONTROL {
                    self.disconnect_selected_provider();
                    self.record_overlay_close_after_key(overlay_before_key);
                    return;
                }

                if handle_connect_dialog_key_event(&mut self.connect_dialog_state, key) {
                    self.record_overlay_close_after_key(overlay_before_key);
                    return;
                }
                if !self.connect_dialog_state.dialog.is_visible() {
                    if let Some(selected_item) =
                        get_pending_selection(&mut self.connect_dialog_state)
                    {
                        self.handle_connect_dialog_selection(selected_item);
                        self.record_overlay_close_after_key(overlay_before_key);
                        return;
                    }
                    self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
                    self.overlay_focus = OverlayFocus::None;
                }
                false
            }
            OverlayFocus::ProviderOAuthFlow => {
                let action =
                    handle_provider_oauth_flow_key_event(&mut self.provider_oauth_flow_state, key);
                match action {
                    ProviderOAuthFlowAction::Handled => true,
                    ProviderOAuthFlowAction::NotHandled => false,
                    ProviderOAuthFlowAction::Close => {
                        self.overlay_focus = OverlayFocus::None;
                        true
                    }
                    ProviderOAuthFlowAction::CopyLink(url) => {
                        match crate::utils::clipboard::copy_text(&url) {
                            Ok(_) => push_toast(Toast::new(
                                format!(
                                    "Copied {} login link",
                                    self.provider_oauth_in_progress
                                        .map(|provider| provider.label())
                                        .unwrap_or("OAuth")
                                ),
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
                            self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
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
                            self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
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
            OverlayFocus::MoveSessionDialog => {
                let action =
                    handle_move_session_dialog_key_event(&mut self.move_session_dialog_state, key);
                self.handle_move_session_dialog_action(action)
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
                            self.resume_remote_wait_if_clear();
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
                            self.resume_remote_wait_if_clear();
                        }
                        true
                    }
                    QuestionDialogAction::Cancel => {
                        self.question_dialog_state.clear_with_empty();
                        self.chat_state.chat.resume_streaming_tps_timer();
                        self.restore_focus_after_priority_overlay();
                        self.cancel_streaming();
                        true
                    }
                    QuestionDialogAction::Handled => true,
                    QuestionDialogAction::NotHandled => true,
                }
            }
            OverlayFocus::TerminalSessionDialog => true,
            OverlayFocus::RemoteDialog => {
                let submit_enabled = self.can_launch_remote_now();
                let action = handle_remote_dialog_key_event(
                    &mut self.remote_dialog_state,
                    key,
                    submit_enabled,
                );
                self.handle_remote_dialog_action(action)
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
            OverlayFocus::McpDialog => {
                let action = handle_mcp_dialog_key_event(&mut self.mcp_dialog_state, key);
                self.handle_mcp_dialog_action(action);
                if !self.mcp_dialog_state.dialog.is_visible() {
                    self.overlay_focus = OverlayFocus::None;
                }
                true
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
            OverlayFocus::JobsDialog => {
                if !(key.code == KeyCode::Char('y')
                    && key.modifiers == event::KeyModifiers::NONE
                    && self.jobs_dialog_state.is_detail_open()
                    && self.try_copy_selection())
                {
                    let action = handle_jobs_dialog_key_event(
                        &mut self.jobs_dialog_state,
                        key,
                        &self.process_registry,
                        self.session_spinner_frame,
                    );
                    self.handle_jobs_dialog_action(action);
                }
                true
            }
            OverlayFocus::CopyActions => {
                if let Some(ref mut dialog) = self.copy_actions_dialog {
                    let event = dialog.handle_key_event(key);
                    self.handle_copy_actions_event(event)
                } else {
                    false
                }
            }
            OverlayFocus::MessageActions => {
                if let Some(ref mut dialog) = self.message_actions_dialog {
                    match dialog.handle_key_event(key) {
                        ActionDialogEvent::Close => {
                            self.close_message_actions();
                            true
                        }
                        ActionDialogEvent::Select => {
                            let action = self
                                .message_actions_dialog
                                .as_ref()
                                .and_then(|dialog| dialog.get_selected())
                                .map(|selected| selected.id.clone());
                            if let Some(action) = action {
                                self.execute_message_action(&action);
                            }
                            true
                        }
                        ActionDialogEvent::Shortcut(key) => {
                            let action = self
                                .message_actions_dialog
                                .as_ref()
                                .and_then(|dialog| dialog.item_id_for_shortcut(key));
                            if let Some(action) = action {
                                self.execute_message_action(&action);
                            }
                            true
                        }
                        ActionDialogEvent::None => true,
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
            OverlayFocus::VariantsDialog => {
                if self.variants_dialog_state.handle_key_event(key) {
                    self.select_variant();
                }
                if !self.variants_dialog_state.dialog.is_visible() {
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
            OverlayFocus::TitleDialog => {
                let action = handle_title_dialog_key_event(&mut self.title_dialog_state, key);
                self.handle_title_dialog_action(action);
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
                    crate::views::which_key::WhichKeyAction::ShowJobs => {
                        self.overlay_focus = OverlayFocus::None;
                        self.open_jobs_dialog();
                    }
                    crate::views::which_key::WhichKeyAction::ToggleThinking => {
                        self.overlay_focus = OverlayFocus::None;
                        self.chat_state.chat.toggle_thinking_visible();
                    }
                    crate::views::which_key::WhichKeyAction::GoChild => {
                        self.overlay_focus = OverlayFocus::None;
                        let _ = self.switch_to_latest_child_session();
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
                    crate::views::which_key::WhichKeyAction::ScrollToTop => {
                        self.overlay_focus = OverlayFocus::None;
                        self.chat_state.chat.scroll_to_top();
                    }
                    crate::views::which_key::WhichKeyAction::ScrollToBottom => {
                        self.overlay_focus = OverlayFocus::None;
                        self.chat_state.chat.scroll_to_bottom();
                    }
                    crate::views::which_key::WhichKeyAction::None => {
                        // Keep focus while which-key is still open (e.g. nested submenu).
                        if !self.which_key_state.is_visible() {
                            self.overlay_focus = OverlayFocus::None;
                        }
                    }
                }
                true
            }
            OverlayFocus::None => {
                if self.handle_base_keys(key) {
                    self.record_overlay_close_after_key(overlay_before_key);
                    return;
                }
                false
            }
        };

        if handled {
            self.record_overlay_close_after_key(overlay_before_key);
            return;
        }

        if self.overlay_focus == OverlayFocus::None {
            self.handle_input_and_app_keys(key);
        }
        self.record_overlay_close_after_key(overlay_before_key);
    }

    fn record_overlay_close_after_key(&mut self, overlay_before_key: OverlayFocus) {
        if !matches!(
            overlay_before_key,
            OverlayFocus::None | OverlayFocus::SuggestionsPopup
        ) && self.overlay_focus == OverlayFocus::None
        {
            self.just_closed_overlay = true;
        }
    }

    pub fn take_just_closed_overlay(&mut self) -> bool {
        std::mem::take(&mut self.just_closed_overlay)
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
        // ctrl-t / ctrl-x are handled earlier in handle_keys so they also work
        // while OverlayFocus::SuggestionsPopup is open.
        match key.code {
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
                    self.reset_esc_primed_state();
                    return true;
                }
                if self.is_streaming {
                    return self.handle_streaming_esc_key(key);
                }
                if self.overlay_focus == OverlayFocus::SuggestionsPopup {
                    self.reset_esc_primed_state();
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

    fn set_compact_mode(&mut self, enabled: bool) {
        if self.chat_state.compact_mode == enabled {
            return;
        }
        self.chat_state.compact_mode = enabled;
        if let Some(dao) = &self.prefs_dao {
            if let Err(error) = dao.set_compact_mode(enabled) {
                eprintln!("Failed to persist compact mode preference: {error}");
            }
        }
        push_toast(Toast::new(
            if enabled {
                "Compact mode enabled"
            } else {
                "Compact mode disabled"
            },
            ToastLevel::Info,
            Some(std::time::Duration::from_secs(2)),
        ));
    }

    fn toggle_agent_mode(&mut self) {
        let agents = self.agent_registry.visible_primary_agent_names();
        if agents.is_empty() {
            return;
        }

        let current = self.agent.to_ascii_lowercase();
        let current_index = agents
            .iter()
            .position(|agent| agent.eq_ignore_ascii_case(&current));
        let next_index = current_index.map_or(0, |index| (index + 1) % agents.len());
        let _ = self.set_agent_mode(&agents[next_index]);
    }

    fn set_agent_mode(&mut self, agent: &str) -> bool {
        let agent = agent.trim();
        let Some(definition) = self.agent_registry.primary_agent(agent) else {
            return false;
        };

        self.agent = titlecase_agent_name(&definition.name);
        let colors = self.get_current_theme_colors();
        let agent_color = crate::theme::agent_color(&self.agent, &colors);
        self.chat_state.wave_spinner.set_color(agent_color);
        true
    }

    pub fn remote_toggle_agent_mode(&mut self) {
        self.toggle_agent_mode();
    }

    pub fn remote_set_agent_mode(&mut self, agent: String) -> bool {
        self.set_agent_mode(&agent)
    }

    pub fn remote_reasoning_effort_label(&self) -> Option<String> {
        if self.remote_reasoning_effort_options().is_empty() {
            return None;
        }

        self.active_reasoning_effort_label()
    }

    pub fn remote_reasoning_effort_options(&self) -> Vec<String> {
        let Some(capability) =
            self.reasoning_capability_for_model(&self.provider_name, &self.model)
        else {
            return Vec::new();
        };

        let supported = capability
            .values()
            .iter()
            .filter(|effort| **effort != crate::model::reasoning::ReasoningEffort::None)
            .map(|effort| effort.as_str().to_string())
            .collect::<Vec<_>>();
        if supported.is_empty() {
            return Vec::new();
        }

        let mut options = vec!["off".to_string()];
        options.extend(supported);
        options.dedup();
        options
    }

    pub fn remote_set_reasoning_effort(&mut self, effort: Option<String>) -> Result<bool> {
        let Some(capability) =
            self.reasoning_capability_for_model(&self.provider_name, &self.model)
        else {
            return Ok(false);
        };

        let effort = effort.unwrap_or_default();
        let effort = effort.trim();
        if effort.is_empty() || effort.eq_ignore_ascii_case("off") {
            self.set_reasoning_effort_override_for_model(
                self.provider_name.clone(),
                self.model.clone(),
                None,
            )?;
            return Ok(true);
        }

        let parsed = effort
            .parse::<crate::model::reasoning::ReasoningEffort>()
            .map_err(|_| anyhow::anyhow!("unknown reasoning effort: {effort}"))?;
        if !capability.values().contains(&parsed)
            || parsed == crate::model::reasoning::ReasoningEffort::None
        {
            return Ok(false);
        }

        self.set_reasoning_effort_override_for_model(
            self.provider_name.clone(),
            self.model.clone(),
            Some(parsed),
        )?;

        Ok(true)
    }

    /// OpenCode-style arm window for double-Esc cancel (see session.interrupt).
    const ESC_ARM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    fn reset_esc_primed_state(&mut self) {
        self.esc_primed_at = None;
    }

    fn esc_is_primed(&self) -> bool {
        self.esc_primed_at
            .is_some_and(|primed_at| primed_at.elapsed() < Self::ESC_ARM_TIMEOUT)
    }

    fn refresh_esc_primed_state(&mut self) {
        if self.esc_primed_at.is_some() && !self.esc_is_primed() {
            self.esc_primed_at = None;
        }
    }

    fn arm_esc_primed(&mut self) {
        self.esc_primed_at = Some(std::time::Instant::now());
    }

    fn handle_streaming_esc_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers != event::KeyModifiers::NONE {
            self.reset_esc_primed_state();
            return false;
        }

        // OpenCode: first Esc arms, second Esc within ESC_ARM_TIMEOUT interrupts.
        self.refresh_esc_primed_state();
        if self.esc_is_primed() {
            self.reset_esc_primed_state();
            if let Some(session_id) = self.session_manager.get_current_session_id().cloned() {
                if self.interrupt_streaming_to_send_queued_for_session(&session_id) {
                    return true;
                }
            }
            self.cancel_streaming();
            return true;
        }

        self.arm_esc_primed();
        true
    }

    fn handle_timeline_esc_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers != event::KeyModifiers::NONE
            || self.base_focus != BaseFocus::Chat
            || !self.input.is_empty()
            || self.is_subagent_session_active()
        {
            self.reset_esc_primed_state();
            return false;
        }

        self.refresh_esc_primed_state();
        if self.esc_is_primed() {
            self.reset_esc_primed_state();
            self.open_timeline_dialog();
        } else {
            self.arm_esc_primed();
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
            if Self::is_input_navigation_key(key) {
                self.input.handle_event(key);
            }
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
                            self.input.clear();
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
                            let active_session_can_queue = self
                                .session_manager
                                .get_current_session_id()
                                .is_some_and(|id| {
                                    self.session_has_active_stream(id)
                                        || self.session_has_active_compaction(id)
                                });
                            if self.is_streaming && active_session_can_queue {
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
                    if !self.input.is_empty() {
                        self.input.clear();
                    }
                    self.clear_suggestions_and_blur();
                }
            }
            _ => {
                self.input.handle_event(key);
                self.update_suggestions();
            }
        }
    }

    fn is_input_navigation_key(key: KeyEvent) -> bool {
        let command = key
            .modifiers
            .intersects(event::KeyModifiers::SUPER | event::KeyModifiers::META);
        let control = key.modifiers.contains(event::KeyModifiers::CONTROL);

        matches!(key.code, KeyCode::Left | KeyCode::Right if command)
            || matches!(key.code, KeyCode::Char('a' | 'e') if control)
    }

    fn can_submit_input(input_type: &InputType, is_streaming: bool) -> bool {
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

        // A terminal cannot report a mouse-up that happens outside its surface.
        // Selection actions are therefore shown as soon as a drag selects text.
        // A subsequent click on the bar proves that the original button was
        // released, so finalize that stale drag before handling the click.
        let selection_is_dragging = match state.target {
            SelectionActionTarget::Chat => self.chat_state.chat.selection.is_dragging,
            SelectionActionTarget::Input => self.input.is_selection_dragging(),
            SelectionActionTarget::JobsDetail => self.jobs_dialog_state.selection.is_dragging,
        };
        if selection_is_dragging {
            if area.contains(point) && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            {
                match state.target {
                    SelectionActionTarget::Chat => self.chat_state.chat.finish_selection_drag(),
                    SelectionActionTarget::Input => self.input.finish_selection_drag(),
                    SelectionActionTarget::JobsDetail => self.jobs_dialog_state.selection.finish(),
                }
            } else {
                return false;
            }
        }

        if !area.contains(point) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => true,
            MouseEventKind::Up(MouseButton::Left) => {
                let rel = mouse.column.saturating_sub(area.x) as usize;
                match selection_action_for_column(state, rel) {
                    SelectionAction::AddToPrompt => self.add_selection_to_prompt(state.target),
                    SelectionAction::Copy => {
                        let _ = self.try_copy_selection();
                        true
                    }
                    SelectionAction::OpenInEditor => self.open_selection_in_editor(),
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

        if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left))
            && self.input.has_selection()
            && !self.input.get_selected_text().is_empty()
        {
            self.show_selection_action_bar_for(SelectionActionTarget::Input);
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

    fn open_chat_hyperlink_target(&mut self, target: &HyperlinkTarget) {
        match target {
            HyperlinkTarget::File(target) => {
                let result = if let Some(line) = target.line {
                    crate::utils::image_attachment::open_file_path_at_location(
                        &target.path,
                        line,
                        target.column.unwrap_or(1),
                        &self.editor,
                    )
                } else {
                    crate::utils::image_attachment::open_file_path(&target.path, &self.editor)
                };
                match result {
                    Ok(crate::utils::image_attachment::OpenOutcome::Spawned) => {
                        push_toast(Toast::new(
                            format!("Opened {}", target.path.display()),
                            ToastLevel::Info,
                            None,
                        ))
                    }
                    Ok(crate::utils::image_attachment::OpenOutcome::Copied(text)) => push_toast(
                        Toast::new(format!("Copied {}", text), ToastLevel::Info, None),
                    ),
                    Ok(crate::utils::image_attachment::OpenOutcome::Suspend(command)) => {
                        self.pending_editor_suspend = Some(command);
                    }
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
        if !matches!(mouse.kind, MouseEventKind::Moved) {
            self.note_user_activity();
        }
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

        // Bottom-right jobs chip → open jobs dialog.
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && mouse.modifiers.is_empty()
            && self.overlay_focus == OverlayFocus::None
        {
            if let Some(area) = self.jobs_chip_area {
                if area.contains(ratatui::layout::Position::new(mouse.column, mouse.row)) {
                    self.open_jobs_dialog();
                    return;
                }
            }
        }

        if matches!(mouse.kind, MouseEventKind::Moved) && self.base_focus != BaseFocus::Chat {
            self.chat_state.chat.clear_hovered_image();
            self.chat_state.chat.clear_hovered_hyperlink();
        }

        if self.overlay_focus == OverlayFocus::TerminalSessionDialog
            && self.terminal_session_dialog_state.has_active()
        {
            return;
        }

        // If text is selected and user clicks on an overlay, clear selection instead.
        // Permission/question dialogs intentionally forward outside clicks to chat so
        // text can still be highlighted (same idea as scroll-through).
        if self.overlay_focus != OverlayFocus::None
            && !matches!(
                self.overlay_focus,
                OverlayFocus::PermissionDialog | OverlayFocus::QuestionDialog
            )
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

        if self.overlay_focus == OverlayFocus::AgentsDialog {
            let action = handle_agents_dialog_mouse_event(&mut self.agents_dialog_state, mouse);
            match action {
                AgentsDialogAction::SelectAgent { agent } => {
                    self.select_agent_from_dialog(&agent);
                    self.overlay_focus = OverlayFocus::None;
                }
                AgentsDialogAction::None => {}
            }
            if !self.agents_dialog_state.dialog.is_visible() {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::ModelsDialog {
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
                    self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);

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
        } else if self.overlay_focus == OverlayFocus::VariantsDialog {
            if self.variants_dialog_state.dialog.handle_mouse_event(mouse) {
                self.select_variant();
            }
            if !self.variants_dialog_state.dialog.is_visible() {
                self.overlay_focus = OverlayFocus::None;
            }
        } else if self.overlay_focus == OverlayFocus::PermissionDialog {
            let action =
                handle_permission_dialog_mouse_event(&mut self.permission_dialog_state, mouse);
            let handled = !matches!(action, PermissionDialogAction::NotHandled);
            if let PermissionDialogAction::Respond(response) = action {
                self.remote_respond_permission(response);
            }
            if !handled {
                // Allow chat scroll + text selection outside the permission dialog.
                self.forward_chat_mouse_through_dialog(mouse);
            }
        } else if self.overlay_focus == OverlayFocus::QuestionDialog {
            let action = handle_question_dialog_mouse_event(&mut self.question_dialog_state, mouse);
            let handled = !matches!(action, QuestionDialogAction::NotHandled);
            match action {
                QuestionDialogAction::Submit => {
                    self.question_dialog_state.submit_current();
                    if self.question_dialog_state.has_active() {
                        self.overlay_focus = OverlayFocus::QuestionDialog;
                    } else {
                        self.resume_remote_wait_if_clear();
                    }
                }
                QuestionDialogAction::Cancel => {
                    self.question_dialog_state.clear_with_empty();
                    self.resume_remote_wait_if_clear();
                    self.cancel_streaming();
                }
                QuestionDialogAction::Handled | QuestionDialogAction::NotHandled => {}
            }
            if !handled {
                // Allow chat scroll + text selection outside the question dialog.
                self.forward_chat_mouse_through_dialog(mouse);
            }
        } else if self.overlay_focus == OverlayFocus::RemoteDialog {
            let action = handle_remote_dialog_mouse_event(&mut self.remote_dialog_state, mouse);
            let _ = self.handle_remote_dialog_action(action);
        } else if self.overlay_focus == OverlayFocus::ThemesDialog {
            let action = handle_themes_dialog_mouse_event(&mut self.themes_dialog_state, mouse);

            match action {
                crate::views::themes_dialog::ThemesDialogAction::PreviewTheme { theme_id } => {
                    self.preview_theme_by_id(&theme_id);
                }
                crate::views::themes_dialog::ThemesDialogAction::SelectTheme { theme_id } => {
                    if let Some(selected_theme_id) = self.commit_theme_by_id(&theme_id) {
                        push_toast(Toast::new(
                            format!("Theme: {}", selected_theme_id),
                            ToastLevel::Info,
                            None,
                        ));
                    }
                }
                crate::views::themes_dialog::ThemesDialogAction::ToggleTransparent => {
                    self.apply_theme_transparent(self.themes_dialog_state.transparent);
                }
                crate::views::themes_dialog::ThemesDialogAction::None => {}
            }

            if !self.themes_dialog_state.dialog.is_visible() {
                if !self.themes_dialog_committed {
                    self.current_theme_index = self.themes_dialog_original_theme_index;
                    self.dark_mode = self.themes_dialog_original_dark_mode;
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
        } else if self.overlay_focus == OverlayFocus::ProviderOAuthFlow {
            let action =
                handle_provider_oauth_flow_mouse_event(&mut self.provider_oauth_flow_state, mouse);
            match action {
                ProviderOAuthFlowAction::Handled | ProviderOAuthFlowAction::NotHandled => {}
                ProviderOAuthFlowAction::Close => {
                    self.overlay_focus = OverlayFocus::None;
                }
                ProviderOAuthFlowAction::CopyLink(url) => {
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
        } else if self.overlay_focus == OverlayFocus::McpDialog {
            let action = handle_mcp_dialog_mouse_event(&mut self.mcp_dialog_state, mouse);
            self.handle_mcp_dialog_action(action);
            if !self.mcp_dialog_state.dialog.is_visible() {
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
        } else if self.overlay_focus == OverlayFocus::JobsDialog {
            let action = handle_jobs_dialog_mouse_event(
                &mut self.jobs_dialog_state,
                mouse,
                &self.process_registry,
                self.session_spinner_frame,
            );
            self.handle_jobs_dialog_action(action);
            // Click-outside hides via Dialog::handle_mouse_event → Handled, not Close.
            // Mirror Sessions/Skills/Mcp: clear overlay when the dialog is gone, otherwise
            // keys stay trapped in JobsDialog and the TUI looks frozen (ctrl-cc still works).
            if !self.jobs_dialog_state.is_visible() {
                self.selection_action_bar = None;
                self.overlay_focus = OverlayFocus::None;
            } else if self.jobs_dialog_state.is_detail_open() {
                match mouse.kind {
                    MouseEventKind::Up(MouseButton::Left)
                        if self.jobs_dialog_state.selection.active
                            && !self.jobs_dialog_state.selection.is_dragging =>
                    {
                        self.show_selection_action_bar_for(SelectionActionTarget::JobsDetail);
                    }
                    MouseEventKind::Down(MouseButton::Left)
                        if !self.jobs_dialog_state.selection.active
                            && self.selection_action_bar
                                == Some(SelectionActionBarState {
                                    target: SelectionActionTarget::JobsDetail,
                                    can_open_in_editor: false,
                                }) =>
                    {
                        self.selection_action_bar = None;
                    }
                    _ => {}
                }
            }
        } else if self.overlay_focus == OverlayFocus::CopyActions {
            if let Some(ref mut dialog) = self.copy_actions_dialog {
                let event = dialog.handle_mouse_event(mouse);
                self.handle_copy_actions_event(event);
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

            let action_event = if let Some(ref mut dialog) = self.message_actions_dialog {
                dialog.handle_mouse_event(mouse)
            } else {
                ActionDialogEvent::None
            };
            match action_event {
                ActionDialogEvent::Close => self.close_message_actions(),
                ActionDialogEvent::Select => {
                    let action = self
                        .message_actions_dialog
                        .as_ref()
                        .and_then(|dialog| dialog.get_selected())
                        .map(|selected| selected.id.clone());
                    if let Some(action) = action {
                        self.execute_message_action(&action);
                    }
                }
                ActionDialogEvent::Shortcut(key) => {
                    let action = self
                        .message_actions_dialog
                        .as_ref()
                        .and_then(|dialog| dialog.item_id_for_shortcut(key));
                    if let Some(action) = action {
                        self.execute_message_action(&action);
                    }
                }
                ActionDialogEvent::None => {}
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
        } else if self.overlay_focus == OverlayFocus::MoveSessionDialog {
            let action =
                handle_move_session_dialog_mouse_event(&mut self.move_session_dialog_state, mouse);
            self.handle_move_session_dialog_action(action);
            if !self.move_session_dialog_state.dialog.is_visible()
                && self.overlay_focus == OverlayFocus::MoveSessionDialog
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
        } else if self.overlay_focus == OverlayFocus::TitleDialog {
            let action = handle_title_dialog_mouse_event(&mut self.title_dialog_state, mouse);
            self.handle_title_dialog_action(action);
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

                // Compact-mode sticky user message: click to scroll to that message.
                if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                    && mouse.modifiers.is_empty()
                {
                    if let Some((sticky_rect, msg_idx)) = self.chat_state.sticky_click_target {
                        if sticky_rect.contains(Position::new(mouse.column, mouse.row)) {
                            self.chat_state.chat.scroll_to_message_index(msg_idx);
                            // Clear sticky state so the scrolled-to message re-enters
                            // the viewport cleanly without residual sticky chrome.
                            self.chat_state.sticky_message_index = None;
                            self.chat_state.sticky_click_target = None;
                            self.pending_chat_message_click = None;
                            return;
                        }
                    }
                }

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

                    // Show actions as soon as a drag creates a non-empty selection. Terminal mouse
                    // protocols do not send us mouse-up when the pointer is released outside the
                    // terminal, so waiting only for release can leave the selection without actions.
                    if was_dragging
                        && self.chat_state.chat.selection.is_dragging
                        && matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left))
                    {
                        self.show_selection_action_bar_for(SelectionActionTarget::Chat);
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
        self.note_user_activity();
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
            (_, OverlayFocus::AgentsDialog) => {
                self.agents_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.agents_dialog_state.dialog.set_search_query(
                    self.agents_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
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
            }
            (_, OverlayFocus::VariantsDialog) => {
                self.variants_dialog_state
                    .dialog
                    .search_textarea
                    .insert_str(&text);
                self.variants_dialog_state.dialog.set_search_query(
                    self.variants_dialog_state
                        .dialog
                        .search_textarea
                        .lines()
                        .join(""),
                );
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

                if let Some(theme_id) = self
                    .themes_dialog_state
                    .dialog
                    .get_selected()
                    .map(|it| it.id.clone())
                {
                    self.preview_theme_by_id(&theme_id);
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
            }
            (_, OverlayFocus::FindBar) => {
                self.find_bar.insert_text(&text);
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
            (_, OverlayFocus::TerminalSessionDialog) => {
                self.terminal_session_dialog_state.insert_paste(&text);
            }
            (_, OverlayFocus::RemoteDialog) => {
                self.remote_dialog_state.insert_text(&text);
            }
            _ => {}
        }
    }

    fn autocomplete_and_submit(&mut self) {
        if let Some(selected) = get_selected_suggestion(&self.suggestions_popup_state).cloned() {
            match selected.kind {
                crate::autocomplete::SuggestionKind::Command => {
                    if self.command_registry.is_custom_command(&selected.name) {
                        // Custom commands may take args — fill `/cmd ` only.
                        self.input.apply_suggestion(&selected);
                        self.update_suggestions();
                    } else {
                        // Builtins (`/compact`, `/refreshmodels`, …) run immediately.
                        let command = format!("/{}", selected.replacement);
                        self.process_command_from_input(&command);
                    }
                }
                crate::autocomplete::SuggestionKind::Agent
                | crate::autocomplete::SuggestionKind::File => {
                    self.input.apply_suggestion(&selected);
                    self.update_suggestions();
                }
            }
        }
        self.clear_suggestions_and_blur();
    }

    fn process_command_from_input(&mut self, command: &str) {
        self.input.clear();
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(self.process_input(command));
        });
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
        let thinking_visible = self.chat_state.chat.thinking_visible();
        let compact_mode = self.chat_state.compact_mode;
        let is_chat = self.can_open_find_bar();
        self.command_palette_state.refresh_items(
            &self.command_registry,
            is_chat,
            thinking_visible,
            compact_mode,
        );
        self.command_palette_state.show();
        self.overlay_focus = OverlayFocus::CommandPalette;
    }

    fn open_title_dialog(&mut self) {
        clear_suggestions(&mut self.suggestions_popup_state);
        self.title_dialog_state.show(&self.terminal_title_items);
        self.overlay_focus = OverlayFocus::TitleDialog;
        self.preview_title_dialog_items();
    }

    fn preview_title_dialog_items(&mut self) {
        if !self.terminal_title_enabled {
            return;
        }
        let items = self.title_dialog_state.enabled_items();
        match self.terminal_title_text_for_items(&items) {
            Some(title) => {
                if crate::notify::set_terminal_title(&title).is_ok() {
                    self.terminal_title_last = Some(title);
                }
            }
            None => self.clear_terminal_title_signal(),
        }
    }

    fn handle_title_dialog_action(&mut self, action: TitleDialogAction) {
        match action {
            TitleDialogAction::Changed => self.preview_title_dialog_items(),
            TitleDialogAction::Confirm => {
                let items = self.title_dialog_state.enabled_items();
                if let Some(dao) = self.prefs_dao.as_ref() {
                    if let Err(err) = dao.set_terminal_title_items(&items) {
                        push_toast(Toast::new(
                            format!("Failed to save terminal title: {err}"),
                            ToastLevel::Error,
                            None,
                        ));
                        self.update_terminal_title_signal();
                        self.overlay_focus = OverlayFocus::None;
                        return;
                    }
                }
                self.terminal_title_items = items;
                self.overlay_focus = OverlayFocus::None;
                self.update_terminal_title_signal();
            }
            TitleDialogAction::Cancel => {
                self.overlay_focus = OverlayFocus::None;
                self.update_terminal_title_signal();
            }
            TitleDialogAction::None => {}
        }
    }

    fn can_open_find_bar(&self) -> bool {
        self.base_focus == BaseFocus::Chat
            && self.session_manager.get_current_session_id().is_some()
    }

    fn open_find_bar(&mut self) {
        if !self.can_open_find_bar() {
            return;
        }
        clear_suggestions(&mut self.suggestions_popup_state);
        self.chat_state.chat.clear_search();
        self.find_bar.show();
        self.overlay_focus = OverlayFocus::FindBar;
    }

    fn close_find_bar_focus(&mut self) {
        self.find_bar.close();
        self.find_bar.clear_matches();
        self.chat_state.chat.clear_search();
        if self.overlay_focus == OverlayFocus::FindBar {
            self.overlay_focus = OverlayFocus::None;
        }
    }

    fn restore_focus_after_priority_overlay(&mut self) {
        self.overlay_focus = if self.find_bar.is_active() && self.can_open_find_bar() {
            OverlayFocus::FindBar
        } else {
            OverlayFocus::None
        };
    }

    fn current_chat_search_width(&self) -> usize {
        self.current_chat_area().width.saturating_sub(2).max(1) as usize
    }

    fn commit_find_query(&mut self) {
        let query = self.find_bar.committed_query().to_string();
        if query.trim().is_empty() {
            self.chat_state.chat.clear_search();
            self.find_bar.clear_matches();
            return;
        }

        let max_width = self.current_chat_search_width();
        let colors = self.get_current_theme_colors();
        let count = self
            .chat_state
            .chat
            .set_search_query(&query, max_width, &self.model, &colors);
        self.find_bar
            .set_match_status(count, self.chat_state.chat.search_active_match_index());
    }

    fn cycle_find_match(&mut self, direction: isize) {
        let count = self.chat_state.chat.search_match_count();
        if count == 0 && !self.find_bar.committed_query().is_empty() {
            self.commit_find_query();
            return;
        }

        self.chat_state.chat.cycle_search_match(direction);
        self.find_bar.set_match_status(
            self.chat_state.chat.search_match_count(),
            self.chat_state.chat.search_active_match_index(),
        );
    }

    fn handle_find_bar_key(&mut self, key: KeyEvent) {
        let query_before = self.find_bar.query();
        match self.find_bar.handle_key_event(key) {
            FindBarAction::CommitSearch => self.commit_find_query(),
            FindBarAction::Next => self.cycle_find_match(1),
            FindBarAction::Previous => self.cycle_find_match(-1),
            FindBarAction::Close => self.close_find_bar_focus(),
            FindBarAction::None => {
                if self.find_bar.query() != query_before {
                    self.find_bar.clear_matches();
                    self.chat_state.chat.clear_search();
                }
            }
        }
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
                if let InputType::Command(parsed) = crate::command::parser::parse_input(&command) {
                    tokio::task::block_in_place(|| {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(self.process_command_input(parsed));
                    });
                }
                self.clear_suggestions_and_blur();
            }
            CommandPaletteAction::RunAppAction(action) => {
                self.overlay_focus = OverlayFocus::None;
                match action {
                    CommandPaletteAppAction::ToggleAgentMode => self.toggle_agent_mode(),
                    CommandPaletteAppAction::OpenAgentsDialog => self.open_agents_dialog(),
                    CommandPaletteAppAction::OpenFind => self.open_find_bar(),
                    CommandPaletteAppAction::SetThinkingVisible(visible) => {
                        self.chat_state.chat.set_thinking_visible(visible);
                    }
                    CommandPaletteAppAction::CycleReasoningEffort => {
                        let _ = self.cycle_active_reasoning_effort();
                    }
                    CommandPaletteAppAction::SetCompactMode(enabled) => {
                        self.set_compact_mode(enabled);
                    }
                    CommandPaletteAppAction::OpenStorage => self.open_storage_dialog(),
                    CommandPaletteAppAction::OpenSkillsDialog => self.show_skills_dialog(),
                    CommandPaletteAppAction::OpenMcpDialog => self.show_mcp_dialog(),
                    CommandPaletteAppAction::OpenJobs => self.open_jobs_dialog(),
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

    fn open_copy_actions_dialog(&mut self) {
        let mut dialog = ActionDialog::with_items(
            "Copy",
            vec![
                ActionDialogItem {
                    id: "transcript".to_string(),
                    key: 't',
                    label: "Copy session transcript".to_string(),
                    description: "Full conversation as Markdown".to_string(),
                },
                ActionDialogItem {
                    id: "id".to_string(),
                    key: 'i',
                    label: "Copy session id".to_string(),
                    description: "Current session identifier".to_string(),
                },
                ActionDialogItem {
                    id: "title".to_string(),
                    key: 'n',
                    label: "Copy session title".to_string(),
                    description: "Current session name".to_string(),
                },
            ],
        );
        dialog.show();
        self.copy_actions_dialog = Some(dialog);
        self.overlay_focus = OverlayFocus::CopyActions;
    }

    fn execute_copy_action(&mut self, action: &str) {
        match action {
            "transcript" => self.copy_session_transcript(),
            "id" => {
                let Some(id) = self.session_manager.get_current_session_id().cloned() else {
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        "No active session id to copy",
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                    self.close_copy_actions_dialog();
                    return;
                };
                self.copy_text_with_toast(&id, "Session id copied to clipboard");
            }
            "title" => {
                let Some(title) = self
                    .session_manager
                    .get_current_session()
                    .map(|session| session.title.clone())
                else {
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        "No active session title to copy",
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                    self.close_copy_actions_dialog();
                    return;
                };
                self.copy_text_with_toast(&title, "Session title copied to clipboard");
            }
            _ => {}
        }

        self.close_copy_actions_dialog();
    }

    fn copy_text_with_toast(&mut self, text: &str, success_message: &'static str) {
        match crate::utils::clipboard::copy_text(text) {
            Ok(_) => push_toast(Toast::new(success_message, ToastLevel::Info, None)),
            Err(e) => {
                self.play_sound_event(crate::sound::SoundEvent::Error);
                push_toast(Toast::new(
                    format!("Failed to copy: {}", e),
                    ToastLevel::Error,
                    Some(std::time::Duration::from_secs(3)),
                ));
            }
        }
    }

    fn handle_copy_actions_event(&mut self, event: ActionDialogEvent) -> bool {
        match event {
            ActionDialogEvent::Close => {
                self.close_copy_actions_dialog();
                true
            }
            ActionDialogEvent::Select => {
                let action = self
                    .copy_actions_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.get_selected())
                    .map(|selected| selected.id.clone());
                if let Some(action) = action {
                    self.execute_copy_action(&action);
                }
                true
            }
            ActionDialogEvent::Shortcut(key) => {
                let action = self
                    .copy_actions_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.item_id_for_shortcut(key));
                if let Some(action) = action {
                    self.execute_copy_action(&action);
                }
                true
            }
            ActionDialogEvent::None => true,
        }
    }

    fn close_copy_actions_dialog(&mut self) {
        self.copy_actions_dialog = None;
        self.overlay_focus = OverlayFocus::None;
    }

    fn focus_pending_priority_overlay(&mut self) -> bool {
        if self.permission_dialog_state.has_active() {
            self.overlay_focus = OverlayFocus::PermissionDialog;
            true
        } else if self.question_dialog_state.has_active() {
            self.overlay_focus = OverlayFocus::QuestionDialog;
            true
        } else if self.terminal_session_dialog_state.has_active() {
            self.overlay_focus = OverlayFocus::TerminalSessionDialog;
            true
        } else {
            false
        }
    }

    fn after_terminal_session_overlay_closed(&mut self) {
        if self.terminal_session_dialog_state.has_active() {
            self.overlay_focus = OverlayFocus::TerminalSessionDialog;
            return;
        }
        if self.focus_pending_priority_overlay() {
            return;
        }
        self.resume_remote_wait_if_clear();
    }

    fn handle_terminal_session_stream_event(
        &mut self,
        tool_call_id: &str,
        event: TerminalSessionEvent,
    ) {
        let auto_close = matches!(
            event,
            TerminalSessionEvent::Exited { .. } | TerminalSessionEvent::Stopped
        );
        let is_current = self
            .terminal_session_dialog_state
            .apply_event(tool_call_id, event);
        if auto_close && is_current {
            self.terminal_session_dialog_state.close_current();
            self.after_terminal_session_overlay_closed();
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

    fn command_matches(&self, command_name: &str, canonical_name: &str) -> bool {
        self.command_registry
            .get(command_name)
            .is_some_and(|command| command.name.as_str() == canonical_name)
    }

    fn push_command_error(&mut self, message: impl Into<String>) {
        self.play_sound_event(crate::sound::SoundEvent::Error);
        push_toast(Toast::new(
            message.into(),
            ToastLevel::Error,
            Some(std::time::Duration::from_secs(3)),
        ));
    }

    fn handle_fork_command(&mut self, args: &[String]) {
        if !args.is_empty() {
            self.push_command_error("Usage: /fork");
            return;
        }

        let _ = self.fork_current_session(None);
    }

    async fn compact_current_session(&mut self) {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "No active session to compact",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        };

        // Never stack another /compact while one is already in flight.
        if self.session_has_active_compaction(&session_id) || self.compaction_receiver.is_some() {
            push_toast(Toast::new(
                "Already compacting...",
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(2)),
            ));
            return;
        }

        // Queue like a message when the session is streaming a normal reply.
        if self.session_has_active_stream(&session_id) || self.is_streaming {
            if self.queue_compact_for_current_session() {
                push_toast(Toast::new(
                    "Queued /compact",
                    ToastLevel::Info,
                    Some(std::time::Duration::from_secs(2)),
                ));
            }
            return;
        }

        self.start_compact_session(&session_id);
    }

    fn start_compact_session(&mut self, session_id: &str) {
        if self.compaction_receiver.is_some() {
            push_toast(Toast::new(
                "Compaction is already running",
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        if self.session_has_active_stream(session_id) {
            if self.queue_compact_for_current_session() {
                push_toast(Toast::new(
                    "Queued /compact",
                    ToastLevel::Info,
                    Some(std::time::Duration::from_secs(2)),
                ));
            }
            return;
        }

        let messages = if self.is_active_session(session_id) {
            self.chat_state.chat.messages.clone()
        } else {
            self.session_manager
                .get_session_ref(session_id)
                .map(|session| session.messages.clone())
                .unwrap_or_default()
        };

        // Manual /compact is seamless (OpenCode/Grok): allow tiny heads.
        // Auto paths still use MIN_COMPACTABLE_TOKENS via the default selector
        // when they call select_messages_for_compaction directly later.
        let Some(selection) = crate::session::compaction::select_messages_for_compaction_with_min(
            &messages,
            crate::session::compaction::DEFAULT_TAIL_TURNS,
            0,
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
        let before_messages =
            crate::session::compaction::filter_messages_for_context(&messages).len();
        let prompt = crate::session::compaction::build_prompt(&selection.messages_to_summarize);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<CompactionTaskMessage>();
        self.compaction_receiver = Some(receiver);
        self.compaction_pending = Some(CompactionPending {
            session_id: session_id.to_string(),
            before_tokens,
            cancel_token: cancel_token.clone(),
        });
        self.is_streaming = true;
        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
        let _ = self.session_manager.set_session_status(
            session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );
        push_toast(Toast::new(
            "Compacting session...",
            ToastLevel::Info,
            Some(std::time::Duration::from_secs(2)),
        ));

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        // Compaction is a rewrite, not a reasoning task. Only clamp when the
        // active model actually exposes effort levels; non-reasoning models
        // keep None so we never send an unsupported parameter.
        let reasoning_effort =
            match self.reasoning_capability_for_model(&self.provider_name, &self.model) {
                Some(cap) if !cap.values().is_empty() => {
                    use crate::model::reasoning::ReasoningEffort;
                    let preferred = [
                        ReasoningEffort::None,
                        ReasoningEffort::Minimal,
                        ReasoningEffort::Low,
                    ];
                    preferred
                        .into_iter()
                        .find(|e| cap.values().contains(e))
                        .or_else(|| self.active_reasoning_effort())
                        .and_then(|e| {
                            if e == ReasoningEffort::None {
                                None
                            } else {
                                Some(e)
                            }
                        })
                }
                _ => None,
            };
        let agent = self.agent.clone();
        let original_messages = messages;
        let task_session_id = session_id.to_string();

        tokio::spawn(async move {
            let result = crate::llm::client::summarize_for_compaction(
                provider_name.clone(),
                model.clone(),
                reasoning_effort,
                prompt,
                cancel_token.clone(),
            )
            .await
            .and_then(|summary| {
                if cancel_token.is_cancelled() {
                    return Err(anyhow::anyhow!("Compaction cancelled by user").into());
                }

                // First pass builds the soft transcript; token stats are computed from
                // the filtered model context and written onto the marker.
                let mut messages = crate::session::compaction::apply_soft_compaction(
                    &original_messages,
                    &selection,
                    &summary,
                    Some(model),
                    Some(provider_name),
                    Some(agent),
                    crate::session::types::CompactionStats {
                        before_tokens,
                        after_tokens: 0,
                        before_messages,
                        after_messages: 0,
                    },
                );
                // Count post-boundary context only (new layout:
                // [history][summary][tail…][marker] — marker excluded).
                let after_tokens = crate::session::compaction::total_context_tokens(&messages);
                let after_messages =
                    crate::session::compaction::filter_messages_for_context(&messages).len();
                let stats = crate::session::types::CompactionStats {
                    before_tokens,
                    after_tokens,
                    before_messages,
                    after_messages,
                };
                // Reject growth/no-shrink so we never commit a worse context.
                if after_tokens >= before_tokens {
                    return Err(anyhow::anyhow!(
                        "Compaction did not reduce context ({})",
                        crate::session::compaction::format_compaction_stats(stats)
                    )
                    .into());
                }
                if let Some(marker) = messages
                    .iter_mut()
                    .rev()
                    .find(|message| crate::session::compaction::is_compaction_marker(message))
                {
                    marker.compaction_stats = Some(stats);
                }
                Ok((messages, stats))
            });

            let message = match result {
                Ok((messages, stats)) => CompactionTaskMessage::Success {
                    session_id: task_session_id,
                    messages,
                    stats,
                },
                Err(err) => {
                    let error = err.to_string();
                    if cancel_token.is_cancelled()
                        || error.to_ascii_lowercase().contains("cancelled")
                    {
                        CompactionTaskMessage::Cancelled {
                            session_id: task_session_id,
                        }
                    } else {
                        CompactionTaskMessage::Failed {
                            session_id: task_session_id,
                            error,
                        }
                    }
                }
            };
            let _ = sender.send(message);
        });
    }

    async fn process_input(&mut self, input: &str) {
        use crate::command::parser::parse_input;

        match parse_input(input) {
            InputType::Command(mut parsed) => {
                // Popup Accept / autocomplete_and_submit land here — must record MRU
                // (process_command_input is only used by some Enter paths).
                if let Some(autocomplete) = self.input.autocomplete.as_ref() {
                    autocomplete.command_auto.touch_mru(&parsed.name);
                }
                if self.command_registry.is_custom_command(&parsed.name) {
                    parsed.prefs_data = self
                        .prefs_dao
                        .as_ref()
                        .and_then(|dao| dao.get_model_preferences().ok());
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
                if self.start_models_command(&mut parsed) {
                    return;
                }
                if parsed.name == "variants" {
                    self.open_variants_dialog(&parsed.args);
                    return;
                }
                if parsed.name == "copy" && self.base_focus == BaseFocus::Chat {
                    self.open_copy_actions_dialog();
                    return;
                }
                if parsed.name == "sessions" {
                    self.open_sessions_dialog();
                    return;
                }
                if parsed.name == "agents" {
                    self.open_agents_dialog();
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
                if parsed.name == "mcp" {
                    self.handle_mcp_slash(&parsed.args);
                    return;
                }
                if parsed.name == "title" {
                    self.open_title_dialog();
                    return;
                }
                if parsed.name == "remote" {
                    self.handle_remote_command_args(&parsed.args);
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
                if parsed.name == "move" && self.base_focus == BaseFocus::Chat {
                    self.handle_move_command(&parsed.args);
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
                if parsed.name == "compact-mode" && self.base_focus == BaseFocus::Chat {
                    self.set_compact_mode(!self.chat_state.compact_mode);
                    return;
                }
                if self.command_matches(&parsed.name, "fork") && self.base_focus == BaseFocus::Chat
                {
                    self.handle_fork_command(&parsed.args);
                    return;
                }
                if self.reject_chat_only_command_outside_chat(&parsed.name) {
                    return;
                }
                parsed.prefs_data = self
                    .prefs_dao
                    .as_ref()
                    .and_then(|dao| dao.get_model_preferences().ok());
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
                            self.note_user_activity();
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
                                        active: item.active,
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
                                        active: item.active,
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

    async fn process_command_input(&mut self, mut parsed: crate::command::parser::ParsedCommand) {
        if let Some(autocomplete) = self.input.autocomplete.as_ref() {
            autocomplete.command_auto.touch_mru(&parsed.name);
        }
        if self.command_registry.is_custom_command(&parsed.name) {
            parsed.prefs_data = self
                .prefs_dao
                .as_ref()
                .and_then(|dao| dao.get_model_preferences().ok());
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
        if self.start_models_command(&mut parsed) {
            return;
        }
        if parsed.name == "copy" && self.base_focus == BaseFocus::Chat {
            self.open_copy_actions_dialog();
            return;
        }
        if parsed.name == "sessions" {
            self.open_sessions_dialog();
            return;
        }
        if parsed.name == "agents" {
            self.open_agents_dialog();
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
        if parsed.name == "mcp" {
            self.handle_mcp_slash(&parsed.args);
            return;
        }
        if parsed.name == "title" {
            self.open_title_dialog();
            return;
        }
        if parsed.name == "remote" {
            self.handle_remote_command_args(&parsed.args);
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
        if parsed.name == "move" && self.base_focus == BaseFocus::Chat {
            self.handle_move_command(&parsed.args);
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
        if parsed.name == "compact-mode" && self.base_focus == BaseFocus::Chat {
            self.set_compact_mode(!self.chat_state.compact_mode);
            return;
        }
        if self.command_matches(&parsed.name, "fork") && self.base_focus == BaseFocus::Chat {
            self.handle_fork_command(&parsed.args);
            return;
        }
        if self.reject_chat_only_command_outside_chat(&parsed.name) {
            return;
        }
        parsed.prefs_data = self
            .prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_preferences().ok());
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
                    self.note_user_activity();
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
                            active: item.active,
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
                            active: item.active,
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

    fn build_sessions_dialog_list(
        &self,
    ) -> (
        Vec<crate::ui::components::dialog::DialogItem>,
        crate::views::sessions_dialog::SessionsDialogListSignature,
        std::collections::HashMap<String, i64>,
    ) {
        use crate::views::sessions_dialog::{SessionsDialogFilter, SessionsDialogRowSignature};

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
        let mut signature_rows = Vec::with_capacity(sessions.len());
        let items: Vec<crate::ui::components::dialog::DialogItem> = sessions
            .into_iter()
            .map(|session| {
                let view_state = self.session_view_states.get(&session.id);
                let is_streaming = view_state
                    .is_some_and(|state| state.stream.is_some() || state.external_stream.is_some())
                    || session.status.is_active();
                let unread_completed = view_state.is_some_and(|state| state.unread_completed);
                let marker = if is_streaming {
                    format!(
                        "{} ",
                        crate::views::sessions_dialog::session_loading_glyph(0)
                    )
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
                let title = session.title.clone();
                let name = format!("{}{}{}", marker, pin, title);
                let group = if session.workspace_name.trim().is_empty() {
                    session.workspace_path.clone()
                } else {
                    session.workspace_name.clone()
                };
                workspace_group_ids
                    .entry(group.clone())
                    .or_insert(session.workspace_id);

                let tip = Some(crate::utils::time::relative_readable_time_from_now(
                    session.updated_at,
                ));
                signature_rows.push(SessionsDialogRowSignature {
                    id: session.id.clone(),
                    title: title.clone(),
                    pinned: session.pinned_at.is_some(),
                    tip: tip.clone(),
                    group: group.clone(),
                    is_streaming,
                    unread_completed,
                });

                crate::ui::components::dialog::DialogItem {
                    id: session.id.clone(),
                    name,
                    group,
                    description: String::new(),
                    tip,
                    provider_id: title,
                    active: false,
                }
            })
            .collect();

        let signature = crate::views::sessions_dialog::SessionsDialogListSignature {
            rows: signature_rows,
        };

        (items, signature, workspace_group_ids)
    }

    fn mark_sessions_dialog_live_dirty(&mut self) {
        self.sessions_dialog_live_dirty = true;
    }

    fn refresh_sessions_dialog(&mut self) {
        let current_workspace_id = self.session_manager.current_workspace_id();
        self.sessions_dialog_state
            .set_current_workspace_id(current_workspace_id);

        let (items, signature, workspace_group_ids) = self.build_sessions_dialog_list();
        self.sessions_dialog_state.last_list_signature = None;
        self.sessions_dialog_state
            .refresh_items_if_changed(items, signature);
        self.sessions_dialog_state
            .set_workspace_group_ids(workspace_group_ids);
        self.sessions_dialog_live_dirty = false;
        self.last_sessions_dialog_metadata_probe = std::time::Instant::now();
    }

    fn update_sessions_dialog_live_state(&mut self, spinner_frame_advanced: bool) {
        if self.overlay_focus != OverlayFocus::SessionsDialog
            || !self.sessions_dialog_state.dialog.is_visible()
            || self.sessions_dialog_state.dialog.is_dragging_scrollbar
        {
            return;
        }

        let metadata_due = self.sessions_dialog_live_dirty
            || self.last_sessions_dialog_metadata_probe.elapsed()
                >= SESSIONS_DIALOG_METADATA_PROBE_INTERVAL;

        if metadata_due {
            let current_workspace_id = self.session_manager.current_workspace_id();
            self.sessions_dialog_state
                .set_current_workspace_id(current_workspace_id);

            let (items, signature, workspace_group_ids) = self.build_sessions_dialog_list();
            if self.sessions_dialog_state.last_list_signature.as_ref() != Some(&signature) {
                self.sessions_dialog_state
                    .refresh_items_if_changed(items, signature);
                self.sessions_dialog_state
                    .set_workspace_group_ids(workspace_group_ids);
            }
            self.sessions_dialog_live_dirty = false;
            self.last_sessions_dialog_metadata_probe = std::time::Instant::now();
        }

        if !spinner_frame_advanced {
            return;
        }

        let streaming_ids: Vec<String> = self
            .sessions_dialog_state
            .last_list_signature
            .as_ref()
            .map(|signature| {
                signature
                    .rows
                    .iter()
                    .filter(|row| row.is_streaming)
                    .map(|row| row.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.sessions_dialog_state
            .apply_streaming_row_markers(&streaming_ids, self.session_spinner_frame);
    }

    fn open_timeline_dialog(&mut self) {
        self.reset_esc_primed_state();

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

    fn open_jobs_dialog(&mut self) {
        self.reset_esc_primed_state();
        self.jobs_dialog_state
            .refresh_from_registry(&self.process_registry, self.session_spinner_frame);
        self.jobs_dialog_state.show();
        self.overlay_focus = OverlayFocus::JobsDialog;
    }

    fn handle_jobs_dialog_action(&mut self, action: JobsDialogAction) {
        match action {
            JobsDialogAction::Close => {
                self.jobs_dialog_state.hide();
                self.selection_action_bar = None;
                if self.overlay_focus == OverlayFocus::JobsDialog {
                    self.overlay_focus = OverlayFocus::None;
                }
            }
            JobsDialogAction::NotHandled => {}
            JobsDialogAction::Handled => {}
            JobsDialogAction::Kill(id) => {
                let _ = self.process_registry.kill_blocking(&id);
                // If we killed the parked interactive session, drop it too.
                if self.terminal_session_dialog_state.active_job_id() == Some(id.as_str()) {
                    self.terminal_session_dialog_state.close_current();
                    if self.overlay_focus == OverlayFocus::TerminalSessionDialog {
                        self.after_terminal_session_overlay_closed();
                    }
                }
                if self.jobs_dialog_state.is_detail_open() {
                    let _ = self
                        .jobs_dialog_state
                        .refresh_detail_output(&self.process_registry);
                } else {
                    self.jobs_dialog_state
                        .refresh_from_registry(&self.process_registry, self.session_spinner_frame);
                }
            }
            JobsDialogAction::Restart(id) => match self.process_registry.restart_blocking(&id) {
                Ok(_) => {
                    if self.jobs_dialog_state.is_detail_open() {
                        let _ = self
                            .jobs_dialog_state
                            .refresh_detail_output(&self.process_registry);
                    } else {
                        self.jobs_dialog_state.refresh_from_registry(
                            &self.process_registry,
                            self.session_spinner_frame,
                        );
                    }
                }
                Err(err) => {
                    crate::emit_log!("[JOBS] restart failed id={} err={}", id, err);
                }
            },
            JobsDialogAction::FocusInteractive(id) => {
                self.jobs_dialog_state.hide();
                self.selection_action_bar = None;
                if self.terminal_session_dialog_state.active_job_id() == Some(id.as_str())
                    && self.terminal_session_dialog_state.has_active()
                {
                    self.overlay_focus = OverlayFocus::TerminalSessionDialog;
                } else if self.overlay_focus == OverlayFocus::JobsDialog {
                    self.overlay_focus = OverlayFocus::None;
                }
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
        let can_undo = self.selected_message_can_undo(idx);
        let can_copy_response_markdown = self
            .session_manager
            .get_current_session()
            .and_then(|session| session.messages.get(idx))
            .and_then(message_response_markdown)
            .is_some();
        self.message_actions_index = Some(idx);
        self.message_actions_return_focus = return_focus;

        let mut items = vec![
            ActionDialogItem {
                id: "copy".to_string(),
                key: 'c',
                label: "Copy".to_string(),
                description: "Copy message to clipboard".to_string(),
            },
            ActionDialogItem {
                id: "fork".to_string(),
                key: 'f',
                label: "Fork at this point".to_string(),
                description: "Create new session (Will include this message)".to_string(),
            },
        ];

        if can_copy_response_markdown {
            items.insert(
                1,
                ActionDialogItem {
                    id: "copy_response_markdown".to_string(),
                    key: 'm',
                    label: "Copy text response as markdown".to_string(),
                    description: "Copy only the assistant's text response as Markdown".to_string(),
                },
            );
        }

        if can_undo {
            items.push(ActionDialogItem {
                id: "undo".to_string(),
                key: 'u',
                label: "Undo".to_string(),
                description: "Remove messages from here onward".to_string(),
            });
        }

        let mut dialog = ActionDialog::with_items("Message Actions", items);
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

    fn current_session_messages_to_fork(
        &mut self,
        through_idx: Option<usize>,
    ) -> Option<Vec<crate::session::types::Message>> {
        let session = self.session_manager.get_current_session()?;
        let end = through_idx
            .map(|idx| {
                crate::session::types::logical_message_block_range(&session.messages, idx)
                    .map(|range| range.end)
                    .unwrap_or_else(|| idx.saturating_add(1).min(session.messages.len()))
            })
            .unwrap_or(session.messages.len());

        Some(session.messages.iter().take(end).cloned().collect())
    }

    fn fork_current_session(&mut self, through_idx: Option<usize>) -> bool {
        let Some(messages_to_fork) = self.current_session_messages_to_fork(through_idx) else {
            self.push_command_error("No active session to fork");
            return false;
        };

        if messages_to_fork.is_empty() {
            self.push_command_error("Nothing to fork");
            return false;
        }

        let fork_title = self
            .session_manager
            .get_current_session()
            .map(|session| fork_title_from_session_title(&session.title))
            .unwrap_or_else(|| fork_title_from_session_title("fork"));

        let _ = self.create_new_session(Some(fork_title));
        for msg in &messages_to_fork {
            let _ = self.session_manager.add_message_to_current_session(msg);
        }

        self.chat_state.chat.clear();
        self.chat_state.chat.replace_messages(messages_to_fork);
        self.chat_state.chat.scroll_offset = usize::MAX;
        self.chat_state.chat.clear_highlighted_message();
        self.base_focus = BaseFocus::Chat;

        let toast = through_idx
            .map(|idx| format!("Forked session from message {}", idx + 1))
            .unwrap_or_else(|| "Forked session".to_string());
        push_toast(Toast::new(toast, ToastLevel::Info, None));
        true
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
            "copy_response_markdown" => {
                let response = self
                    .session_manager
                    .get_current_session()
                    .and_then(|session| session.messages.get(idx))
                    .and_then(message_response_markdown);

                if let Some(response) = response {
                    self.copy_text_with_toast(&response, "Text response copied as Markdown");
                }
                self.close_message_actions();
            }
            "fork" => {
                if self.fork_current_session(Some(idx)) {
                    self.close_message_actions();
                    self.timeline_dialog_state.hide();
                    self.overlay_focus = OverlayFocus::None;
                }
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
        self.terminal_session_dialog_state.clear_all_with_stop();
        self.running = false;
    }

    pub fn take_remote_launch_request(&mut self) -> Option<RemoteLaunchRequest> {
        self.remote_launch_request.take()
    }

    pub fn take_editor_suspend(&mut self) -> Option<String> {
        self.pending_editor_suspend.take()
    }

    fn open_remote_dialog(&mut self) {
        self.remote_dialog_state.show();
        self.overlay_focus = OverlayFocus::RemoteDialog;
    }

    fn handle_remote_command_args(&mut self, args: &[String]) {
        if !args.is_empty() {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "Usage: /remote",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        self.open_remote_dialog();
    }

    fn can_launch_remote_now(&self) -> bool {
        !self.is_streaming
            && self.compaction_receiver.is_none()
            && self
                .session_view_states
                .values()
                .all(|state| state.stream.is_none() && state.external_stream.is_none())
    }

    fn handle_remote_dialog_action(&mut self, action: RemoteDialogAction) -> bool {
        match action {
            RemoteDialogAction::Submit(submission) => {
                self.submit_remote_launch(submission);
                true
            }
            RemoteDialogAction::BlockedStreaming => {
                push_toast(Toast::new(
                    "Wait for the current response to finish before starting remote mode",
                    ToastLevel::Warning,
                    Some(std::time::Duration::from_secs(3)),
                ));
                true
            }
            RemoteDialogAction::Cancel => {
                self.overlay_focus = OverlayFocus::None;
                true
            }
            RemoteDialogAction::Handled => true,
            RemoteDialogAction::NotHandled => false,
        }
    }

    fn submit_remote_launch(&mut self, submission: RemoteDialogSubmission) {
        if !self.can_launch_remote_now() {
            self.handle_remote_dialog_action(RemoteDialogAction::BlockedStreaming);
            return;
        }

        self.remote_launch_request = Some(RemoteLaunchRequest {
            bind: submission.bind,
            pair_code: submission.pair_code,
        });
        self.overlay_focus = OverlayFocus::None;
        self.quit();
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
        let connected_provider_ids = connected_providers
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<String>>();

        let discovery = Discovery::new();
        let configured_provider_ids = discovery
            .as_ref()
            .map(Discovery::custom_provider_ids)
            .unwrap_or_default();

        let include_runtime = crate::model::extensions::ModelExtensions::runtime()
            .iter()
            .any(|integration| connected_providers.contains_key(integration.provider_id()))
            || crate::model::extensions::ModelExtensions::is_runtime_provider(&self.provider_name)
            || self.models_dialog_state.dialog.items.iter().any(|item| {
                crate::model::extensions::ModelExtensions::is_runtime_provider(&item.provider_id)
            });

        let include_unauthenticated_free = connected_providers.is_empty()
            || crate::model::extensions::ModelExtensions::is_unauthenticated_free_provider(
                &self.provider_name,
            )
            || self.models_dialog_state.dialog.items.iter().any(|item| {
                crate::model::extensions::ModelExtensions::is_unauthenticated_free_provider(
                    &item.provider_id,
                )
            });

        if connected_providers.is_empty()
            && configured_provider_ids.is_empty()
            && !include_runtime
            && !include_unauthenticated_free
        {
            return;
        }

        let has_persistent = connected_providers.keys().any(|provider_id| {
            !crate::model::extensions::ModelExtensions::is_runtime_provider(provider_id)
        }) || !configured_provider_ids.is_empty()
            || include_unauthenticated_free
            || connected_providers.is_empty();

        let models = if has_persistent {
            match discovery.as_ref() {
                Ok(discovery) => match tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(discovery.fetch_models())
                }) {
                    Ok(models) => models,
                    Err(err) if include_runtime => {
                        let runtime_models = tokio::task::block_in_place(|| {
                            let rt = tokio::runtime::Handle::current();
                            rt.block_on(crate::model::extensions::ModelExtensions::runtime_models_for_dialog_cached_or_empty())
                        });
                        if runtime_models.is_empty() {
                            push_toast(Toast::new(
                                format!("Failed to refresh models: {}", err),
                                ToastLevel::Warning,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                        }
                        runtime_models
                    }
                    Err(_) => return,
                },
                Err(_) if include_runtime => tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(crate::model::extensions::ModelExtensions::runtime_models_for_dialog_cached_or_empty())
                }),
                Err(_) => return,
            }
        } else if include_runtime {
            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(crate::model::extensions::ModelExtensions::runtime_models_for_dialog_cached_or_empty())
            })
        } else {
            return;
        };
        let mut models = models;
        if include_runtime {
            let runtime_models = tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(
                    crate::model::extensions::ModelExtensions::runtime_models_for_dialog_cached_or_empty(),
                )
            });
            crate::model::discovery::merge_dialog_models(&mut models, runtime_models);
        }
        if let Ok(discovery) = discovery.as_ref() {
            discovery.apply_custom_models_to_dialog(&mut models);
        }

        self.model_reasoning_options = models
            .iter()
            .map(|model| {
                (
                    (model.provider_id.clone(), model.id.clone()),
                    model.reasoning_options.clone(),
                )
            })
            .collect();

        let prefs = self
            .prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_preferences().ok());

        let mut model_lookup: std::collections::HashMap<(String, String), ModelType> =
            std::collections::HashMap::new();

        let is_model_selectable = |model: &ModelType| {
            crate::model::discovery::is_model_selectable(
                model,
                &connected_provider_ids,
                &configured_provider_ids,
            )
        };

        for model in &models {
            if is_model_selectable(model) {
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

            let tip = if is_favorite {
                Some("❤︎".to_string())
            } else {
                None
            };

            let description = model.dialog_description();

            items.push(DialogItem {
                id: model.id.clone(),
                name: model.name.clone(),
                group: group.to_string(),
                description,
                tip,
                provider_id: model.provider_id.clone(),
                active: is_active,
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

            if is_model_selectable(&model) {
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

    fn agent_dialog_items(&self) -> Vec<crate::ui::components::dialog::DialogItem> {
        let active = self.agent.to_ascii_lowercase();
        let mut items: Vec<crate::ui::components::dialog::DialogItem> = self
            .agent_registry
            .visible_primary_agents()
            .into_iter()
            .map(|agent| {
                let mode = match agent.mode {
                    crate::agent::definition::AgentMode::Primary => "Primary",
                    crate::agent::definition::AgentMode::All => "Primary + Subagent",
                    crate::agent::definition::AgentMode::Subagent => "Subagent",
                };
                let model = agent
                    .model
                    .as_deref()
                    .map(str::trim)
                    .filter(|model| !model.is_empty());
                let mut hidden_tokens = vec![agent.name.clone(), mode.to_ascii_lowercase()];
                if let Some(model) = model {
                    hidden_tokens.push(model.to_string());
                }

                crate::ui::components::dialog::DialogItem {
                    id: agent.name.clone(),
                    name: agent.name.clone(),
                    group: mode.to_string(),
                    description: agent.description.clone(),
                    tip: model.map(str::to_string),
                    provider_id: hidden_tokens.join(" "),
                    active: agent.name.eq_ignore_ascii_case(&active),
                }
            })
            .collect();

        items.sort_by(|left, right| {
            left.group
                .cmp(&right.group)
                .then_with(|| left.name.cmp(&right.name))
        });
        items
    }

    fn refresh_agents_dialog(&mut self) {
        let items = self.agent_dialog_items();
        self.agents_dialog_state.refresh_items(items);
        let _ = self
            .agents_dialog_state
            .dialog
            .select_item_by_id(&self.agent.to_ascii_lowercase());
    }

    fn open_agents_dialog(&mut self) {
        self.refresh_agents_dialog();
        self.agents_dialog_state.dialog.show();
        let _ = self
            .agents_dialog_state
            .dialog
            .select_item_by_id(&self.agent.to_ascii_lowercase());
        self.overlay_focus = OverlayFocus::AgentsDialog;
    }

    fn select_agent_from_dialog(&mut self, agent: &str) {
        if self.set_agent_mode(agent) {
            push_toast(Toast::new(
                format!("Switched agent to: {}", self.agent),
                ToastLevel::Info,
                None,
            ));
            self.refresh_agents_dialog();
        } else {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                format!("Unknown primary agent: {}", agent),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
        }
    }

    fn show_models_dialog(
        &mut self,
        title: impl Into<String>,
        mut items: Vec<crate::ui::components::dialog::DialogItem>,
    ) {
        for item in &mut items {
            let is_active = item.id == self.model && item.provider_id == self.provider_name;
            item.active = is_active;
        }

        self.models_dialog_state = init_models_dialog(title, items);
        self.models_dialog_state.dialog.show();
        let _ = self
            .models_dialog_state
            .dialog
            .select_item_by_key(&self.model, &self.provider_name);
        self.overlay_focus = OverlayFocus::ModelsDialog;
    }

    fn start_models_command(&mut self, parsed: &mut crate::command::parser::ParsedCommand) -> bool {
        let kind = match parsed.name.as_str() {
            "models" => ModelsTaskKind::Load,
            "refreshmodels" => ModelsTaskKind::Refresh,
            _ => return false,
        };

        if self.models_receiver.is_some() {
            push_toast(Toast::new(
                "Model discovery is already running",
                ToastLevel::Info,
                Some(std::time::Duration::from_secs(2)),
            ));
            return true;
        }

        let connected_provider_ids = models_dialog_provider_ids();

        if kind == ModelsTaskKind::Load
            && parsed.args.is_empty()
            && !self.models_dialog_state.dialog.items.is_empty()
            && self.models_dialog_provider_ids == connected_provider_ids
        {
            let mut items = self.models_dialog_state.dialog.items.clone();
            for item in &mut items {
                item.active = item.id == self.model && item.provider_id == self.provider_name;
            }
            self.models_dialog_state.dialog.update_items_in_place(items);
            self.models_dialog_state.dialog.show();
            let _ = self
                .models_dialog_state
                .dialog
                .select_item_by_key(&self.model, &self.provider_name);
            self.overlay_focus = OverlayFocus::ModelsDialog;
            return true;
        }

        parsed.prefs_data = self
            .prefs_dao
            .as_ref()
            .and_then(|dao| dao.get_model_preferences().ok());
        parsed.active_model_id = Some(self.model.clone());

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        self.models_receiver = Some(receiver);

        match kind {
            ModelsTaskKind::Load => {
                self.show_models_dialog("Available Models", Vec::new());
                self.models_dialog_state.start_loading();
            }
            ModelsTaskKind::Refresh => {
                self.overlay_focus = OverlayFocus::RefreshModelsDialog;
            }
        }

        let parsed = parsed.clone();
        let provider_signature = models_dialog_provider_ids();
        tokio::spawn(async move {
            let result = match kind {
                ModelsTaskKind::Load => crate::command::handlers::load_models(parsed).await,
                ModelsTaskKind::Refresh => crate::command::handlers::refresh_models().await,
            };
            let _ = sender.send(ModelsTaskMessage {
                kind,
                result,
                provider_signature,
            });
        });
        true
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
        if self
            .sessions_dialog_state
            .select_first_item_in_workspace(current_workspace_id)
        {
            return;
        }
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

    fn open_move_session_dialog(&mut self) {
        if self.session_manager.get_current_session_id().is_none() {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "No active session to move",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        let current_workspace_id = self
            .session_manager
            .get_current_session_id()
            .and_then(|id| self.session_manager.get_session_ref(id))
            .map(|session| session.workspace_id)
            .unwrap_or_else(|| self.session_manager.current_workspace_id());
        self.move_session_dialog_state
            .refresh_workspaces(self.session_manager.list_workspaces(), current_workspace_id);
        self.move_session_dialog_state.show();
        self.overlay_focus = OverlayFocus::MoveSessionDialog;
    }

    fn handle_move_command(&mut self, args: &[String]) {
        if !args.is_empty() {
            self.play_sound_event(crate::sound::SoundEvent::Error);
            push_toast(Toast::new(
                "Usage: /move",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        self.open_move_session_dialog();
    }

    fn handle_move_session_dialog_action(&mut self, action: MoveSessionDialogAction) -> bool {
        match action {
            MoveSessionDialogAction::None => false,
            MoveSessionDialogAction::Close => {
                self.overlay_focus = OverlayFocus::None;
                true
            }
            MoveSessionDialogAction::MoveToWorkspace(workspace_id) => {
                self.overlay_focus = OverlayFocus::None;
                let Some(session_id) = self.session_manager.get_current_session_id().cloned()
                else {
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        "No active session to move",
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                    return true;
                };

                match self
                    .session_manager
                    .move_session_to_workspace(&session_id, workspace_id)
                {
                    Ok(true) => {
                        self.refresh_sessions_dialog();
                        push_toast(Toast::new(
                            "Session moved",
                            ToastLevel::Info,
                            Some(std::time::Duration::from_secs(3)),
                        ));
                    }
                    Ok(false) => {}
                    Err(err) => {
                        self.play_sound_event(crate::sound::SoundEvent::Error);
                        push_toast(Toast::new(
                            format!("Failed to move session: {:?}", err),
                            ToastLevel::Error,
                            Some(std::time::Duration::from_secs(3)),
                        ));
                    }
                }
                true
            }
        }
    }

    fn apply_generated_session_title(&mut self, session_id: &str, title: String) {
        let title = title.trim();
        let current_title = self
            .session_manager
            .get_session_ref(session_id)
            .map(|session| session.title.clone());
        let chat = self.chat_for_session(session_id);
        let first_prompt = chat.and_then(first_user_prompt);
        let has_only_first_user_message = chat.is_some_and(has_single_user_message);
        let can_replace = current_title.as_deref().is_some_and(|current| {
            has_only_first_user_message
                && (is_default_session_title(current)
                    || first_prompt
                        .as_deref()
                        .is_some_and(|prompt| is_auto_session_title_for_prompt(current, prompt)))
        });

        if title.is_empty() || !can_replace {
            return;
        }
        let _ = self
            .session_manager
            .rename_session(session_id, title.to_string());
        self.refresh_sessions_dialog();
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
                    group: t.appearance.as_str().to_string(),
                    // Searchable: type "light" or "dark" to filter by appearance.
                    description: t.appearance.as_str().to_string(),
                    tip: None,
                    provider_id: String::new(),
                    active: is_active,
                }
            })
            .collect();

        items.sort_by(|a, b| a.id.cmp(&b.id));

        self.themes_dialog_state = init_themes_dialog("Themes", items, self.theme_transparent);

        if let Some(theme_id) = current_id.as_deref() {
            let _ = self
                .themes_dialog_state
                .dialog
                .select_item_by_key(theme_id, "");
        }

        self.themes_dialog_state.dialog.show();
        self.themes_dialog_original_theme_index = self.current_theme_index;
        self.themes_dialog_original_dark_mode = self.dark_mode;
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
                    active: false,
                });
            }
        }

        items.sort_by(|a, b| a.id.cmp(&b.id));

        self.skills_dialog_state = crate::views::skills_dialog::init_skills_dialog("Skills", items);
        self.skills_dialog_state.dialog.show();
        self.overlay_focus = OverlayFocus::SkillsDialog;
    }

    fn handle_mcp_slash(&mut self, args: &[String]) {
        if args.first().map(|s| s.as_str()) == Some("auth") {
            match args.get(1).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                Some(name) => self.begin_mcp_oauth(name),
                None => self.show_mcp_dialog(),
            }
            return;
        }
        self.show_mcp_dialog();
    }

    fn show_mcp_dialog(&mut self) {
        let selected = self
            .mcp_dialog_state
            .dialog
            .get_selected()
            .map(|item| item.id.clone());
        self.refresh_mcp_dialog_items();
        if let Some(selected) = selected {
            self.mcp_dialog_state
                .dialog
                .select_item_by_key(&selected, "");
        }
        self.mcp_dialog_state.dialog.show();
        self.overlay_focus = OverlayFocus::McpDialog;
    }

    fn refresh_mcp_dialog_items(&mut self) {
        use crate::ui::components::dialog::DialogItem;

        let live = self.live_mcp_views();
        let mut items = self
            .remote_mcp_servers()
            .into_iter()
            .map(|server| {
                let live = live.iter().find(|view| view.name == server.name);
                let status = live
                    .map(|view| view.status.as_str())
                    .unwrap_or(if server.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    });
                let detail = live.and_then(|view| view.detail.as_deref()).unwrap_or("");
                let tip = if detail.is_empty() {
                    status.to_string()
                } else {
                    format!("{status}: {detail}")
                };
                DialogItem {
                    id: server.name.clone(),
                    name: server.name,
                    group: "MCP".to_string(),
                    description: String::new(),
                    tip: Some(tip),
                    provider_id: String::new(),
                    active: server.enabled,
                }
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        self.mcp_dialog_state = init_mcp_dialog("MCP", items);
    }

    fn live_mcp_views(&self) -> Vec<crate::mcp::McpServerView> {
        let manager =
            crate::mcp::McpManager::ensure(self.mcp.clone(), std::path::PathBuf::from(&self.cwd));
        manager
            .try_lock()
            .map(|guard| guard.views())
            .unwrap_or_default()
    }

    fn handle_mcp_dialog_action(&mut self, action: McpDialogAction) {
        match action {
            McpDialogAction::None => {}
            McpDialogAction::Toggle { server_id } => {
                let selected = server_id.clone();
                match self.remote_toggle_mcp_server(&server_id) {
                    Ok(_) => {
                        self.refresh_mcp_dialog_items();
                        self.mcp_dialog_state
                            .dialog
                            .select_item_by_key(&selected, "");
                        self.mcp_dialog_state.dialog.show();
                    }
                    Err(err) => {
                        self.play_sound_event(crate::sound::SoundEvent::Error);
                        push_toast(Toast::new(
                            format!("Failed to toggle MCP server: {err}"),
                            ToastLevel::Error,
                            Some(std::time::Duration::from_secs(3)),
                        ));
                    }
                }
            }
            McpDialogAction::Auth { server_id } => self.begin_mcp_oauth(&server_id),
        }
    }

    fn begin_mcp_oauth(&mut self, name: &str) {
        if self.mcp_oauth_in_progress.is_some() || self.provider_oauth_in_progress.is_some() {
            push_toast(Toast::new(
                "An OAuth flow is already in progress",
                ToastLevel::Info,
                None,
            ));
            return;
        }
        let Some(crate::config::McpServerConfig::Remote(remote)) = self.mcp.get(name).cloned()
        else {
            push_toast(Toast::new(
                format!("MCP server '{name}' is not a remote server"),
                ToastLevel::Error,
                None,
            ));
            return;
        };
        if !remote.oauth_enabled {
            push_toast(Toast::new(
                format!("OAuth is disabled for MCP server '{name}'"),
                ToastLevel::Info,
                None,
            ));
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<McpOAuthTaskMessage>();
        self.mcp_oauth_receiver = Some(receiver);
        self.mcp_oauth_in_progress = Some(name.to_string());
        self.provider_oauth_flow_state
            .show_browser_waiting_for(format!("MCP {name}"));
        self.mcp_dialog_state.dialog.hide();
        self.overlay_focus = OverlayFocus::ProviderOAuthFlow;

        let server_name = name.to_string();
        let mcp_config = self.mcp.clone();
        let workspace = std::path::PathBuf::from(&self.cwd);
        tokio::spawn(async move {
            let auth =
                crate::mcp::oauth::authenticate_with_url_callback(&server_name, &remote, |url| {
                    let _ = crate::utils::image_attachment::open_url(url);
                })
                .await;
            let result = match auth {
                Ok(()) => {
                    let manager = crate::mcp::McpManager::ensure(mcp_config, workspace);
                    let mut guard = manager.lock().await;
                    guard.reconnect(&server_name).await
                }
                Err(err) => Err(err),
            };
            let _ = match result {
                Ok(()) => sender.send(McpOAuthTaskMessage::Success { name: server_name }),
                Err(err) => sender.send(McpOAuthTaskMessage::Failed {
                    name: server_name,
                    error: err.to_string(),
                }),
            };
        });
    }

    fn process_mcp_oauth_events(&mut self) {
        let mut events = Vec::new();
        if let Some(receiver) = &mut self.mcp_oauth_receiver {
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                McpOAuthTaskMessage::Success { name } => {
                    self.mcp_oauth_in_progress = None;
                    self.mcp_oauth_receiver = None;
                    self.provider_oauth_flow_state.hide();
                    push_toast(Toast::new(
                        format!("Authenticated MCP server \"{name}\""),
                        ToastLevel::Success,
                        None,
                    ));
                    self.show_mcp_dialog();
                }
                McpOAuthTaskMessage::Failed { name, error } => {
                    self.mcp_oauth_in_progress = None;
                    self.mcp_oauth_receiver = None;
                    self.provider_oauth_flow_state.hide();
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        format!("MCP \"{name}\" auth failed: {error}"),
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(6)),
                    ));
                    if self.overlay_focus == OverlayFocus::ProviderOAuthFlow {
                        self.show_mcp_dialog();
                    }
                }
            }
        }
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
                active: false,
            },
            DialogItem {
                id: "openai-oauth-headless".to_string(),
                name: "ChatGPT Plus/Pro (headless)".to_string(),
                group: "OpenAI".to_string(),
                description: "Device code login flow".to_string(),
                tip: None,
                provider_id: "openai".to_string(),
                active: false,
            },
            DialogItem {
                id: "openai-api-key".to_string(),
                name: "Manually enter API key".to_string(),
                group: "OpenAI".to_string(),
                description: "Use OpenAI API key".to_string(),
                tip: None,
                provider_id: "openai".to_string(),
                active: false,
            },
        ];

        self.connect_dialog_state = crate::views::ConnectDialogState::new(
            crate::ui::components::dialog::Dialog::with_items("Connect OpenAI", items),
        );
        self.connect_dialog_state.dialog.show();
        self.connect_dialog_mode = ConnectDialogMode::OpenAIMethodSelection;
        self.overlay_focus = OverlayFocus::ConnectDialog;
    }

    fn show_xai_connect_methods(&mut self) {
        use crate::ui::components::dialog::DialogItem;

        let items = vec![
            DialogItem {
                id: "xai-oauth-browser".to_string(),
                name: "xAI Grok OAuth (SuperGrok Subscription)".to_string(),
                group: "xAI".to_string(),
                description: "OAuth via browser callback".to_string(),
                tip: None,
                provider_id: "xai".to_string(),
                active: false,
            },
            DialogItem {
                id: "xai-oauth-headless".to_string(),
                name: "xAI Grok OAuth (Headless / Remote / VPS)".to_string(),
                group: "xAI".to_string(),
                description: "Device code login flow".to_string(),
                tip: None,
                provider_id: "xai".to_string(),
                active: false,
            },
            DialogItem {
                id: "xai-api-key".to_string(),
                name: "Manually enter API key".to_string(),
                group: "xAI".to_string(),
                description: "Use xAI API key".to_string(),
                tip: None,
                provider_id: "xai".to_string(),
                active: false,
            },
        ];

        self.connect_dialog_state = crate::views::ConnectDialogState::new(
            crate::ui::components::dialog::Dialog::with_items("Connect xAI", items),
        );
        self.connect_dialog_state.dialog.show();
        self.connect_dialog_mode = ConnectDialogMode::XAIMethodSelection;
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
                .select_item_by_id(provider_id);
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
                if crate::model::extensions::ModelExtensions::is_runtime_provider(&selected_item.id)
                {
                    self.connect_local_provider(&selected_item.id);
                    return;
                }

                if selected_item.id == "openai" {
                    self.show_openai_connect_methods();
                    return;
                }

                if selected_item.id == "xai" {
                    self.show_xai_connect_methods();
                    return;
                }

                self.api_key_input.show(&selected_item.id);
                self.overlay_focus = OverlayFocus::ApiKeyInput;
            }
            ConnectDialogMode::OpenAIMethodSelection => match selected_item.id.as_str() {
                "openai-oauth-browser" => {
                    self.begin_provider_oauth_browser(OAuthProvider::OpenAI);
                }
                "openai-oauth-headless" => {
                    self.begin_provider_oauth_headless(OAuthProvider::OpenAI);
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
            ConnectDialogMode::XAIMethodSelection => match selected_item.id.as_str() {
                "xai-oauth-browser" => {
                    self.begin_provider_oauth_browser(OAuthProvider::XAI);
                }
                "xai-oauth-headless" => {
                    self.begin_provider_oauth_headless(OAuthProvider::XAI);
                }
                "xai-api-key" => {
                    self.api_key_input.show("xai");
                    self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
                    self.overlay_focus = OverlayFocus::ApiKeyInput;
                }
                _ => {
                    self.overlay_focus = OverlayFocus::None;
                }
            },
        }
    }

    fn connect_local_provider(&mut self, provider_id: &str) {
        let Some(integration) =
            crate::model::extensions::ModelExtensions::runtime_provider(provider_id)
        else {
            push_toast(Toast::new(
                format!("Unknown local model provider: {}", provider_id),
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(5)),
            ));
            self.overlay_focus = OverlayFocus::None;
            return;
        };

        let models_result = tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(integration.refresh_models())
        });

        let summary = match models_result {
            Ok(summary) => summary,
            Err(err) => {
                push_toast(Toast::new(
                    format!("Failed to connect {}: {}", integration.provider_name(), err),
                    ToastLevel::Error,
                    Some(std::time::Duration::from_secs(5)),
                ));
                self.overlay_focus = OverlayFocus::None;
                return;
            }
        };

        match crate::persistence::AuthDAO::new().and_then(|dao| {
            dao.set_provider(
                integration.provider_id().to_string(),
                crate::persistence::AuthConfig::Local,
            )
        }) {
            Ok(()) => {
                push_toast(Toast::new(
                    format!(
                        "Connected {} ({} local models)",
                        integration.provider_name(),
                        summary.model_count
                    ),
                    ToastLevel::Success,
                    None,
                ));
                self.connect_dialog_state = init_connect_dialog();
                self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
            }
            Err(err) => {
                push_toast(Toast::new(
                    format!(
                        "Failed to save {} connection: {}",
                        integration.provider_name(),
                        err
                    ),
                    ToastLevel::Error,
                    None,
                ));
            }
        }

        self.overlay_focus = OverlayFocus::None;
    }

    fn begin_provider_oauth_browser(&mut self, provider: OAuthProvider) {
        if let Some(active_provider) = self.provider_oauth_in_progress {
            push_toast(Toast::new(
                format!("{} OAuth is already in progress", active_provider.label()),
                ToastLevel::Info,
                None,
            ));
            self.overlay_focus = OverlayFocus::None;
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<ProviderOAuthTaskMessage>();
        self.provider_oauth_receiver = Some(receiver);
        self.provider_oauth_in_progress = Some(provider);
        self.provider_oauth_flow_state
            .show_browser_waiting_for(provider.label());
        self.overlay_focus = OverlayFocus::ProviderOAuthFlow;
        self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
        self.connect_dialog_state = init_connect_dialog();

        tokio::spawn(async move {
            let result = match provider {
                OAuthProvider::OpenAI => crate::auth::openai_oauth::authorize_browser().await,
                OAuthProvider::XAI => crate::auth::xai_oauth::authorize_browser().await,
            };

            let _ = match result {
                Ok(credentials) => sender.send(ProviderOAuthTaskMessage::Success {
                    provider,
                    credentials,
                }),
                Err(err) => sender.send(ProviderOAuthTaskMessage::Failed {
                    provider,
                    error: err.to_string(),
                }),
            };
        });
    }

    fn begin_provider_oauth_headless(&mut self, provider: OAuthProvider) {
        if let Some(active_provider) = self.provider_oauth_in_progress {
            push_toast(Toast::new(
                format!("{} OAuth is already in progress", active_provider.label()),
                ToastLevel::Info,
                None,
            ));
            self.overlay_focus = OverlayFocus::None;
            return;
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<ProviderOAuthTaskMessage>();
        self.provider_oauth_receiver = Some(receiver);
        self.provider_oauth_in_progress = Some(provider);
        self.provider_oauth_flow_state
            .show_headless_preparing_for(provider.label());
        self.overlay_focus = OverlayFocus::ProviderOAuthFlow;
        self.connect_dialog_mode = ConnectDialogMode::ProviderSelection;
        self.connect_dialog_state = init_connect_dialog();

        tokio::spawn(async move {
            let result = match provider {
                OAuthProvider::OpenAI => {
                    let code_sender = sender.clone();
                    crate::auth::openai_oauth::authorize_headless(move |code, url| {
                        let _ =
                            code_sender.send(ProviderOAuthTaskMessage::HeadlessCode { code, url });
                    })
                    .await
                }
                OAuthProvider::XAI => {
                    let code_sender = sender.clone();
                    crate::auth::xai_oauth::authorize_headless(move |code, url| {
                        let _ =
                            code_sender.send(ProviderOAuthTaskMessage::HeadlessCode { code, url });
                    })
                    .await
                }
            };

            let _ = match result {
                Ok(credentials) => sender.send(ProviderOAuthTaskMessage::Success {
                    provider,
                    credentials,
                }),
                Err(err) => sender.send(ProviderOAuthTaskMessage::Failed {
                    provider,
                    error: err.to_string(),
                }),
            };
        });
    }

    fn process_provider_oauth_events(&mut self) {
        let mut events = Vec::new();

        if let Some(receiver) = &mut self.provider_oauth_receiver {
            while let Ok(event) = receiver.try_recv() {
                events.push(event);
            }
        }

        for event in events {
            match event {
                ProviderOAuthTaskMessage::HeadlessCode { code, url } => {
                    self.provider_oauth_flow_state.set_headless_code(code, url);
                    self.overlay_focus = OverlayFocus::ProviderOAuthFlow;
                }
                ProviderOAuthTaskMessage::Success {
                    provider,
                    credentials,
                } => {
                    if let Ok(auth_dao) = crate::persistence::AuthDAO::new() {
                        let _ = auth_dao.set_provider(
                            provider.provider_id().to_string(),
                            crate::persistence::AuthConfig::OAuth {
                                refresh: credentials.refresh,
                                access: credentials.access,
                                expires: credentials.expires,
                                account_id: credentials.account_id,
                                enterprise_url: credentials.enterprise_url,
                            },
                        );
                    }

                    let default_model = provider.default_model();
                    if let Some(prefs_dao) = self.prefs_dao.as_ref() {
                        let _ = prefs_dao.set_active_model(
                            provider.provider_id().to_string(),
                            default_model.to_string(),
                        );
                    }

                    self.provider_name = provider.provider_id().to_string();
                    self.model = default_model.to_string();
                    self.provider_oauth_in_progress = None;
                    self.provider_oauth_receiver = None;
                    self.provider_oauth_flow_state.hide();
                    if self.overlay_focus == OverlayFocus::ProviderOAuthFlow {
                        self.overlay_focus = OverlayFocus::None;
                    }

                    push_toast(Toast::new(
                        provider.connected_message(),
                        ToastLevel::Info,
                        None,
                    ));
                }
                ProviderOAuthTaskMessage::Failed { provider, error } => {
                    self.provider_oauth_in_progress = None;
                    self.provider_oauth_receiver = None;
                    self.provider_oauth_flow_state.hide();
                    if self.overlay_focus == OverlayFocus::ProviderOAuthFlow {
                        self.overlay_focus = OverlayFocus::None;
                    }
                    push_toast(Toast::new(
                        format!("{} OAuth failed: {}", provider.label(), error),
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
        let disconnected_session_id = self
            .compaction_pending
            .as_ref()
            .filter(|_| self.compaction_receiver.is_some())
            .map(|pending| pending.session_id.clone());

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
            self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
        }

        let mut completed_compaction_sessions = Vec::new();

        for event in events {
            match event {
                CompactionTaskMessage::Success {
                    session_id,
                    messages,
                    stats,
                } => {
                    let completed_session_id = session_id.clone();
                    match self
                        .session_manager
                        .replace_session_messages(&session_id, messages.clone())
                    {
                        Ok(()) => {
                            let is_active = self.is_active_session(&session_id);
                            // Marker is last in soft layout — pin to bottom so the
                            // "Context compacted" line is visible without mid-history jump.
                            // Prefer replace_messages on the live chat: rebuilding via
                            // chat_with_messages zeros content_height and can desync
                            // sticky/live scroll state until the next session load.
                            let marker_idx = messages
                                .iter()
                                .rposition(|m| crate::session::compaction::is_compaction_marker(m));
                            if is_active {
                                self.chat_state.chat.replace_messages(messages.clone());
                                self.chat_state.chat.scroll_to_bottom_on_next_render();
                                if let Some(marker_idx) = marker_idx {
                                    self.chat_state
                                        .chat
                                        .set_highlighted_message(Some(marker_idx));
                                } else {
                                    self.chat_state.chat.clear_highlighted_message();
                                }
                            }

                            self.ensure_session_view_state(&session_id);
                            // Build parked chat before mutably borrowing session_view_states
                            // (chat_with_messages needs &self).
                            let parked_chat = if !is_active {
                                let mut view_chat = self.chat_with_messages(messages);
                                view_chat.scroll_to_bottom_on_next_render();
                                if let Some(marker_idx) = marker_idx {
                                    view_chat.set_highlighted_message(Some(marker_idx));
                                } else {
                                    view_chat.clear_highlighted_message();
                                }
                                Some(view_chat)
                            } else {
                                None
                            };
                            if let Some(state) = self.session_view_states.get_mut(&session_id) {
                                // Keep the active session's live chat out of
                                // session_view_states (same invariant as
                                // load_session_view_state / switch_to_session).
                                // Never park an empty new_chat() here — that would
                                // wipe the marker on the next session restore.
                                if let Some(view_chat) = parked_chat {
                                    state.chat = view_chat;
                                }
                                state.tool_calls = ToolCallViewState::default();
                                state.unread_completed = !is_active;
                            }

                            let _ = self.session_manager.set_session_status(
                                &session_id,
                                crate::session::types::SessionStatus::Idle,
                                None,
                            );
                            self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
                            self.refresh_sessions_dialog();
                            push_toast(Toast::new(
                                format!(
                                    "Context compacted ({})",
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
                    completed_compaction_sessions.push(completed_session_id);
                }
                CompactionTaskMessage::Failed { session_id, error } => {
                    let completed_session_id = session_id.clone();
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
                    completed_compaction_sessions.push(completed_session_id);
                }
                CompactionTaskMessage::Cancelled { session_id } => {
                    let completed_session_id = session_id.clone();
                    let _ = self.session_manager.set_session_status(
                        &session_id,
                        crate::session::types::SessionStatus::Idle,
                        None,
                    );
                    push_toast(Toast::new(
                        "Compaction cancelled",
                        ToastLevel::Info,
                        Some(std::time::Duration::from_secs(2)),
                    ));
                    completed_compaction_sessions.push(completed_session_id);
                }
            }
        }

        if disconnected && completed_compaction_sessions.is_empty() {
            if let Some(session_id) = disconnected_session_id {
                completed_compaction_sessions.push(session_id);
            }
        }

        for session_id in completed_compaction_sessions {
            self.submit_queued_messages_for_session(&session_id);
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

    fn process_models_events(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;

        if let Some(receiver) = &mut self.models_receiver {
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
            self.models_receiver = None;
        }

        if disconnected && events.is_empty() {
            self.models_dialog_state.finish_loading();
            if matches!(
                self.overlay_focus,
                OverlayFocus::ModelsDialog | OverlayFocus::RefreshModelsDialog
            ) {
                self.overlay_focus = OverlayFocus::None;
            }
            push_toast(Toast::new(
                "Model discovery ended before returning results",
                ToastLevel::Error,
                Some(std::time::Duration::from_secs(3)),
            ));
            return;
        }

        for event in events {
            let ModelsTaskMessage {
                kind,
                result,
                provider_signature,
            } = event;
            match (kind, result) {
                (
                    ModelsTaskKind::Load,
                    crate::command::registry::CommandResult::ShowDialog { title, items },
                ) => {
                    let dialog_items = items
                        .into_iter()
                        .map(|item| crate::ui::components::dialog::DialogItem {
                            id: item.id,
                            name: item.name,
                            group: item.group,
                            description: item.description,
                            tip: item.tip,
                            provider_id: item.provider_id,
                            active: item.active,
                        })
                        .collect();
                    self.models_dialog_state.finish_loading();
                    let current_signature = models_dialog_provider_ids();
                    self.models_dialog_provider_ids = (provider_signature == current_signature)
                        .then_some(provider_signature)
                        .flatten();
                    if self.overlay_focus == OverlayFocus::ModelsDialog {
                        self.show_models_dialog(title, dialog_items);
                    }
                }
                (ModelsTaskKind::Load, crate::command::registry::CommandResult::Error(message)) => {
                    self.models_dialog_state.finish_loading();
                    if self.overlay_focus == OverlayFocus::ModelsDialog {
                        self.overlay_focus = OverlayFocus::None;
                    }
                    self.play_sound_event(crate::sound::SoundEvent::Error);
                    push_toast(Toast::new(
                        message,
                        ToastLevel::Error,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                }
                (ModelsTaskKind::Refresh, _) => {
                    self.models_dialog_state = init_models_dialog("Available Models", Vec::new());
                    self.models_dialog_provider_ids = None;
                    if self.overlay_focus == OverlayFocus::RefreshModelsDialog {
                        self.overlay_focus = OverlayFocus::None;
                    }
                }
                (ModelsTaskKind::Load, _) => {
                    self.models_dialog_state.finish_loading();
                    if self.overlay_focus == OverlayFocus::ModelsDialog {
                        self.overlay_focus = OverlayFocus::None;
                    }
                }
            }
        }
    }

    fn process_title_generation_events(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;

        if let Some(receiver) = &mut self.title_generation_receiver {
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

        if disconnected {
            self.title_generation_receiver = None;
        }

        for event in events {
            match event {
                TitleGenerationTaskMessage::Generated { session_id, title } => {
                    self.apply_generated_session_title(&session_id, title);
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
            state.retry_status = None;
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
            self.terminal_session_dialog_state.clear_all_with_stop();
            if self.overlay_focus == OverlayFocus::TerminalSessionDialog {
                self.overlay_focus = OverlayFocus::None;
            }
        }

        self.sync_active_streaming_flag();
        self.mark_sessions_dialog_live_dirty();
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
        } else if self
            .session_view_states
            .get(session_id)
            .is_some_and(|state| state.external_stream.is_some())
        {
            if let Some(parent_id) = self
                .session_manager
                .parent_id_of(session_id)
                .map(str::to_string)
            {
                self.cancel_streaming_for_session(&parent_id);
            }
        }

        // Compact rides the same Esc interrupt path as streaming turns.
        if self.session_has_active_compaction(session_id) {
            if let Some(pending) = self.compaction_pending.as_ref() {
                pending.cancel_token.cancel();
            }
        }
    }

    fn interrupt_streaming_to_send_queued_for_session(&mut self, session_id: &str) -> bool {
        if !self.is_active_session(session_id)
            || !self.has_queued_user_messages_for_session(session_id)
            || !self.session_has_active_stream(session_id)
        {
            return false;
        }

        // Match manual cancel: stop nested subagents so they don't stay "loading"
        // (active stream / Streaming status) after the parent is interrupted.
        self.interrupt_child_streams_for_parent(
            session_id,
            "Stopped because parent agent was interrupted",
        );
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
        self.submit_queued_messages_for_session_after_interruption(session_id)
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
            self.update_sessions_dialog_live_state(true);
            if self.jobs_dialog_state.is_visible() {
                if self.jobs_dialog_state.is_detail_open() {
                    let _ = self
                        .jobs_dialog_state
                        .refresh_detail_output(&self.process_registry);
                } else {
                    self.jobs_dialog_state
                        .refresh_from_registry(&self.process_registry, self.session_spinner_frame);
                }
            }
        }
    }

    /// How long the Home cursor blink keeps the ~60fps loop alive after activity.
    const HOME_ANIM_IDLE: std::time::Duration = std::time::Duration::from_secs(3);

    pub fn note_user_activity(&mut self) {
        self.last_user_activity = std::time::Instant::now();
    }

    pub fn is_animation_running(&self) -> bool {
        let home_animating = self.base_focus == BaseFocus::Home
            && self.last_user_activity.elapsed() < Self::HOME_ANIM_IDLE;
        home_animating
            || self.has_active_selection_edge_scroll()
            || self.is_streaming
            || self.chat_state.chat.has_active_tool_messages()
            || self.has_active_retry_status()
            || self.compaction_receiver.is_some()
            || self.storage_receiver.is_some()
            || self.models_receiver.is_some()
            || self.title_generation_receiver.is_some()
            || self.terminal_session_dialog_state.has_active()
            || self
                .session_view_states
                .values()
                .any(|state| state.stream.is_some() || state.external_stream.is_some())
            || (self.overlay_focus == OverlayFocus::SessionsDialog
                && self.sessions_dialog_state.dialog.is_visible()
                && self.sessions_dialog_has_streaming_rows())
            // Keep ticking while the jobs dialog is open so duration/spinner
            // stay live — never call running_count_blocking here (can stall UI).
            || self.jobs_dialog_state.is_visible()
    }

    fn sessions_dialog_has_streaming_rows(&self) -> bool {
        if let Some(signature) = self.sessions_dialog_state.last_list_signature.as_ref() {
            return signature.rows.iter().any(|row| row.is_streaming);
        }

        self.session_view_states
            .values()
            .any(|state| state.stream.is_some() || state.external_stream.is_some())
    }

    /// Sessions dialog open while only background (non-current-chat) streams run.
    fn sessions_dialog_passive_streaming_view(&self) -> bool {
        if self.overlay_focus != OverlayFocus::SessionsDialog
            || !self.sessions_dialog_state.dialog.is_visible()
        {
            return false;
        }

        let current = self
            .session_manager
            .get_current_session_id()
            .map(|id| id.as_str());

        self.session_view_states.iter().any(|(id, state)| {
            (state.stream.is_some() || state.external_stream.is_some())
                && current != Some(id.as_str())
        })
    }

    pub fn is_streaming_animation_only(&self) -> bool {
        let streaming_only = (self.is_streaming || self.chat_state.chat.has_active_tool_messages())
            && self.base_focus != BaseFocus::Home
            && !self.has_active_selection_edge_scroll()
            && self.current_session_retry_status().is_none()
            && self.compaction_receiver.is_none()
            && self.storage_receiver.is_none()
            && self.models_receiver.is_none()
            && self.title_generation_receiver.is_none();

        if self.overlay_focus == OverlayFocus::SessionsDialog
            && self.sessions_dialog_state.dialog.is_visible()
        {
            return streaming_only || self.sessions_dialog_passive_streaming_view();
        }

        streaming_only && self.overlay_focus != OverlayFocus::SessionsDialog
    }

    /// Whether the currently viewed session is a subagent child session.
    /// Cheap equivalent of building the full tab list and checking
    /// `is_child_session`; used on every event-loop iteration.
    fn current_session_is_subagent_child(&self) -> bool {
        self.session_manager
            .get_current_session_id()
            .is_some_and(|id| self.session_manager.parent_id_of(id).is_some())
    }

    pub fn isolated_subagent_spinner_interval(&self) -> Option<std::time::Duration> {
        if self.base_focus != BaseFocus::Chat
            || self.overlay_focus != OverlayFocus::None
            || !self.is_streaming
            || !self.current_session_is_subagent_child()
            || self.current_session_retry_status().is_some()
            || self.compaction_receiver.is_some()
        {
            return None;
        }
        self.chat_state.chat.tool_heavy_streaming_render_interval()
    }

    /// Spinner color for the currently viewed session without materializing
    /// the full subagent tab list on every spinner frame.
    fn current_session_spinner_color(&self) -> Option<ratatui::style::Color> {
        let current_id = self.session_manager.get_current_session_id()?;
        let root_id = self.session_manager.root_session_id_for(current_id)?;
        if *current_id == root_id {
            return Some(crate::theme::agent_color(
                &self.agent,
                &self.get_current_theme_colors(),
            ));
        }
        let idx = self
            .session_manager
            .descendant_position(&root_id, current_id)?;
        Some(agent_color_for_tab(idx, &self.get_current_theme_colors()))
    }

    pub fn render_isolated_subagent_spinner(
        &mut self,
        buffer: &mut ratatui::buffer::Buffer,
    ) -> bool {
        let Some(color) = self.current_session_spinner_color() else {
            return false;
        };
        render_subagent_spinner_only(buffer, &mut self.chat_state.wave_spinner, color)
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
        self.process_provider_oauth_events();
        self.process_mcp_oauth_events();
        self.process_compaction_events();
        self.process_storage_events();
        self.process_models_events();
        self.process_title_generation_events();

        let drained = {
            let mut receivers = Vec::new();
            for (id, state) in &mut self.session_view_states {
                if let Some(stream) = state.stream.as_mut() {
                    receivers.push((id.clone(), &mut stream.chunk_receiver));
                }
            }
            let (drained, next_rotation) = drain_streaming_chunks_global(
                &mut receivers,
                STREAM_CHUNK_DRAIN_LIMIT,
                STREAM_CHUNK_GLOBAL_DRAIN_LIMIT,
                STREAM_CHUNK_GLOBAL_DRAIN_TIME_BUDGET,
                self.stream_drain_rotation,
            );
            self.stream_drain_rotation = next_rotation;
            drained
        };

        for (session_id, chunks, mut disconnected) in drained {
            let mut keep_current_stream = true;
            for chunk in coalesce_streaming_chunks(chunks) {
                if !self.process_streaming_chunk_for_session(&session_id, chunk) {
                    keep_current_stream = false;
                    break;
                }
            }

            if keep_current_stream {
                self.maybe_persist_streaming_snapshot_for_session(&session_id, false);
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
                    "stream disconnected before completion: stream task ended before sending a completion event"
                        .to_string(),
                );
            }
        }

        self.sync_active_streaming_flag();
        self.update_sessions_dialog_live_state(false);
    }

    fn process_streaming_chunk_for_session(
        &mut self,
        session_id: &str,
        chunk: crate::llm::ChunkMessage,
    ) -> bool {
        match chunk {
            crate::llm::ChunkMessage::Text(text) => {
                self.set_session_retry_status(session_id, None);
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.append_to_last_assistant(&text);
                }
                self.mark_streaming_snapshot_pending(session_id);
                true
            }
            crate::llm::ChunkMessage::Reasoning(reasoning) => {
                self.set_session_retry_status(session_id, None);
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.append_reasoning_to_last_assistant(&reasoning);
                }
                self.mark_streaming_snapshot_pending(session_id);
                true
            }
            crate::llm::ChunkMessage::Retry(status) => {
                self.set_session_retry_status(session_id, Some(status.into()));
                true
            }
            crate::llm::ChunkMessage::StreamRollback { text, reasoning } => {
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    if !chat.rollback_streamed_output(&text, &reasoning) {
                        crate::emit_log!(
                            "[STREAM_ROLLBACK_MISMATCH] session_id={} text_chars={} reasoning_chars={}",
                            session_id,
                            text.len(),
                            reasoning.len(),
                        );
                    }
                }
                self.mark_streaming_snapshot_pending(session_id);
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
                self.set_session_retry_status(session_id, None);
                // Close the generation sample as a tool-calls finish (excluded from
                // TPS) and pause timing for the duration of tool execution.
                if let Some(chat) = self.chat_for_session_mut(session_id) {
                    chat.end_generation_for_tool_calls();
                }
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
                if !self.session_has_active_stream(&session_id) {
                    crate::emit_log!(
                        "[SUBAGENT] ignoring_late_chunk session_id={} reason=no_active_child_stream",
                        session_id
                    );
                    return true;
                }
                if self.process_streaming_chunk_for_session(&session_id, *chunk) {
                    self.maybe_persist_streaming_snapshot_for_session(&session_id, false);
                }
                true
            }
            crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                self.maybe_persist_streaming_snapshot_for_session(session_id, true);
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
                if !matches!(
                    self.overlay_focus,
                    OverlayFocus::PermissionDialog
                        | OverlayFocus::QuestionDialog
                        | OverlayFocus::TerminalSessionDialog
                ) {
                    self.overlay_focus = OverlayFocus::PermissionDialog;
                }
                self.mark_sessions_dialog_live_dirty();
                true
            }
            crate::llm::ChunkMessage::QuestionRequest {
                questions,
                response_tx,
            } => {
                self.maybe_persist_streaming_snapshot_for_session(session_id, true);
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
                if !matches!(
                    self.overlay_focus,
                    OverlayFocus::PermissionDialog
                        | OverlayFocus::QuestionDialog
                        | OverlayFocus::TerminalSessionDialog
                ) {
                    self.overlay_focus = OverlayFocus::QuestionDialog;
                }
                self.mark_sessions_dialog_live_dirty();
                true
            }
            crate::llm::ChunkMessage::TerminalSessionRequest(request) => {
                self.maybe_persist_streaming_snapshot_for_session(session_id, true);
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
                self.terminal_session_dialog_state.enqueue(request);
                if !matches!(
                    self.overlay_focus,
                    OverlayFocus::PermissionDialog | OverlayFocus::QuestionDialog
                ) {
                    self.overlay_focus = OverlayFocus::TerminalSessionDialog;
                }
                self.mark_sessions_dialog_live_dirty();
                true
            }
            crate::llm::ChunkMessage::TerminalSessionEvent {
                tool_call_id,
                event,
            } => {
                self.handle_terminal_session_stream_event(&tool_call_id, event);
                true
            }
            crate::llm::ChunkMessage::BackgroundJobEvent { .. } => {
                if self.jobs_dialog_state.is_visible() {
                    if self.jobs_dialog_state.is_detail_open() {
                        let _ = self
                            .jobs_dialog_state
                            .refresh_detail_output(&self.process_registry);
                    } else {
                        self.jobs_dialog_state.refresh_from_registry(
                            &self.process_registry,
                            self.session_spinner_frame,
                        );
                    }
                }
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

        let fresh_chat = self.new_chat();
        if let Some(state) = self.session_view_states.get_mut(&session_id) {
            state.chat = fresh_chat;
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
            state.retry_status = None;
            state.external_stream = Some(ExternalStreamState::new(
                model.or_else(|| Some(self.model.clone())),
                provider.or_else(|| Some(self.provider_name.clone())),
                1,
            ));
            state.unread_completed = true;
        }

        self.persist_chat_messages_for_session(&session_id);

        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );

        self.refresh_sessions_dialog();
        self.sync_active_streaming_flag();
    }

    fn finish_streaming_session(&mut self, session_id: &str) {
        self.maybe_persist_streaming_snapshot_for_session(session_id, true);

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
        let completion_event = if self.session_manager.parent_id_of(session_id).is_some() {
            crate::sound::SoundEvent::SubagentComplete
        } else {
            crate::sound::SoundEvent::Complete
        };
        self.play_sound_event_with_notification_detail(
            completion_event,
            completion_stats.as_deref(),
        );
        self.notify_terminal_event(completion_event);
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
        if message.has_running_tool_parts() {
            return true;
        }

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
                    }
                    _ => {}
                }
            }
            chat.mark_render_dirty();

            Self::completion_notification_stats_for_chat(chat)
        } else {
            None
        };

        self.persist_chat_messages_for_session(session_id);

        Some(completion_stats)
    }

    fn mark_running_tool_messages_failed(chat: &mut Chat, start: usize, error: &str) {
        for msg in chat.messages.iter_mut().skip(start) {
            msg.mark_running_tool_parts_failed(error);

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
                    chat.mark_render_dirty_from(idx);
                    return;
                }
            }
        }
    }

    fn fail_streaming_session(&mut self, session_id: &str, error: String) {
        self.maybe_persist_streaming_snapshot_for_session(session_id, true);

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
        self.notify_terminal_event(crate::sound::SoundEvent::Error);
        if let Some(warning) = disconnected_stream_warning_message(&error) {
            push_toast(Toast::new(warning, ToastLevel::Warning, None));
        } else {
            push_toast(Toast::new(
                format!("LLM error: {}", error),
                ToastLevel::Error,
                None,
            ));
        }
        self.cleanup_streaming_for_session(session_id);
        self.submit_queued_messages_for_session(session_id);
    }

    fn cancelled_streaming_session(&mut self, session_id: &str) {
        self.interrupt_child_streams_for_parent(
            session_id,
            "Stopped because parent agent was interrupted",
        );
        self.interrupt_streaming_session_with_reason(
            session_id,
            "Streaming cancelled by user",
            true,
        );
    }

    fn interrupt_child_streams_for_parent(&mut self, parent_session_id: &str, reason: &str) {
        let child_session_ids: Vec<String> = self
            .session_manager
            .child_sessions(parent_session_id)
            .into_iter()
            .map(|session| session.id)
            .collect();

        for child_session_id in child_session_ids {
            self.interrupt_child_streams_for_parent(&child_session_id, reason);

            if self.session_has_active_stream(&child_session_id) {
                self.interrupt_streaming_session_with_reason(&child_session_id, reason, false);
            } else if self
                .session_manager
                .get_session_ref(&child_session_id)
                .is_some_and(|session| session.status.is_active())
            {
                let _ = self.session_manager.set_session_status(
                    &child_session_id,
                    crate::session::types::SessionStatus::Interrupted,
                    None,
                );
            }
        }
    }

    fn interrupt_streaming_session_with_reason(
        &mut self,
        session_id: &str,
        reason: &str,
        show_toast: bool,
    ) {
        self.mark_streamed_assistant_interrupted(session_id);
        self.maybe_persist_streaming_snapshot_for_session(session_id, true);

        if self
            .finalize_and_persist_streamed_messages(session_id, Some(reason))
            .is_none()
        {
            return;
        }

        let _ = self.session_manager.set_session_status(
            session_id,
            crate::session::types::SessionStatus::Interrupted,
            None,
        );

        if show_toast {
            push_toast(Toast::new("Streaming cancelled", ToastLevel::Info, None));
        }
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
                    for call in tool_calls {
                        let args_value: serde_json::Value =
                            serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| {
                                serde_json::Value::String(call.function.arguments.clone())
                            });

                        let call_id = call.id.clone();
                        // Upsert: hosted search may emit running then completed with same id.
                        if let Some(existing) = msg.parts.iter_mut().find(|part| {
                            part.part_type == "tool_call"
                                && part.tool_id() == Some(call_id.as_str())
                        }) {
                            if let Some(obj) = existing.data.as_object_mut() {
                                obj.insert(
                                    "name".into(),
                                    serde_json::Value::String(call.function.name.clone()),
                                );
                                // Hosted search completed events sometimes omit sources /
                                // wipe query to ""; don't replace richer running args.
                                let incoming_hollow =
                                    crate::llm::client::hosted_search_args_are_hollow(&args_value);
                                let existing_hollow = obj
                                    .get("args")
                                    .map(crate::llm::client::hosted_search_args_are_hollow)
                                    .unwrap_or(true);
                                if !incoming_hollow || existing_hollow {
                                    obj.insert("args".into(), args_value);
                                }
                                if matches!(
                                    call.function.name.as_str(),
                                    "x_search" | "web_search" | "file_search"
                                ) {
                                    obj.insert(
                                        "provider_executed".into(),
                                        serde_json::Value::Bool(true),
                                    );
                                }
                            }
                        } else {
                            msg.add_tool_call_part(
                                call.id.clone(),
                                call.function.name.clone(),
                                args_value,
                            );
                            if matches!(
                                call.function.name.as_str(),
                                "x_search" | "web_search" | "file_search"
                            ) {
                                if let Some(part) = msg.parts.last_mut() {
                                    if let Some(obj) = part.data.as_object_mut() {
                                        obj.insert(
                                            "provider_executed".into(),
                                            serde_json::Value::Bool(true),
                                        );
                                    }
                                }
                            }
                            inserted.push((call_id, idx));
                        }
                    }
                    chat.mark_streaming_tool_render_pending(idx);
                }
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
        self.mark_streaming_snapshot_pending(session_id);
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
                    let mut v = if msg.role == crate::session::types::MessageRole::Assistant {
                        msg.tool_result_part_data(&result.tool_call_id)
                            .or_else(|| msg.tool_call_part_data(&result.tool_call_id))
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}))
                    } else {
                        serde_json::from_str::<serde_json::Value>(&msg.content)
                            .unwrap_or_else(|_| serde_json::json!({}))
                    };
                    v["id"] = serde_json::Value::String(result.tool_call_id.clone());
                    v["name"] = serde_json::Value::String(result.name.clone());
                    if matches!(
                        result.name.as_str(),
                        "x_search" | "web_search" | "file_search"
                    ) {
                        v["provider_executed"] = serde_json::Value::Bool(true);
                    }

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

                    // Hosted search completed SSE often omits sources / clears query;
                    // reuse running tool_call args for preview + result card header.
                    if matches!(
                        result.name.as_str(),
                        "web_search" | "x_search" | "file_search"
                    ) {
                        if let Some(args) = msg
                            .tool_call_part_data(&result.tool_call_id)
                            .and_then(|part| part.get("args"))
                            .filter(|a| !crate::llm::client::hosted_search_args_are_hollow(a))
                            .cloned()
                        {
                            let result_args_hollow = v
                                .get("args")
                                .map(crate::llm::client::hosted_search_args_are_hollow)
                                .unwrap_or(true);
                            if result_args_hollow {
                                v["args"] = args.clone();
                            }

                            let preview_is_stub = v
                                .get("output_preview")
                                .and_then(|p| p.as_str())
                                .map(|s| s.trim().is_empty() || s.starts_with("Provider-executed "))
                                .unwrap_or(true);
                            if preview_is_stub {
                                let enriched = crate::llm::client::hosted_search_output_preview(
                                    &result.name,
                                    v.get("status").and_then(|s| s.as_str()).unwrap_or("ok"),
                                    &serde_json::json!({ "arguments": args }),
                                );
                                if !enriched.starts_with("Provider-executed ") {
                                    v["output_preview"] = serde_json::Value::String(enriched);
                                }
                            }
                        }
                    }

                    if msg.role == crate::session::types::MessageRole::Assistant {
                        msg.add_or_update_tool_result_part(v);
                    } else {
                        msg.content = v.to_string();
                    }
                    chat.mark_streaming_tool_render_pending(idx);
                    handled = true;
                }
            }

            if !handled {
                let content = serde_json::json!({
                    "id": result.tool_call_id.clone(),
                    "name": result.name.clone(),
                    "status": "ok",
                    "output_preview": result.content.clone(),
                });

                if let Some(idx) = chat.messages.iter().rposition(|message| {
                    message.role == crate::session::types::MessageRole::Assistant
                }) {
                    let msg = &mut chat.messages[idx];
                    msg.add_or_update_tool_result_part(content);
                    chat.mark_streaming_tool_render_pending(idx);
                } else {
                    chat.add_message(crate::session::types::Message::tool(content.to_string()));
                }
            }
        }

        self.mark_streaming_snapshot_pending(session_id);

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
        self.start_llm_streaming_with_guidance(_user_message, None)
    }

    fn start_llm_streaming_with_guidance(
        &mut self,
        _user_message: &str,
        turn_guidance: Option<&str>,
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
            state.stream = Some(SessionStreamState::new(
                receiver,
                cancel_token.clone(),
                streaming_model.clone(),
                streaming_provider.clone(),
                chat_len_before_assistant,
            ));
            state.tool_calls = ToolCallViewState::default();
            state.unread_completed = false;
            state.retry_status = None;
        }
        self.persist_chat_messages_for_session(&session_id);
        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );
        self.mark_sessions_dialog_live_dirty();

        let agent_mode = self.agent.clone();
        let (provider_name, model) = self.active_primary_agent_model_provider();
        let reasoning_effort = self.active_primary_agent_reasoning_effort();
        let provider_timeout = self
            .provider_timeouts
            .get(&provider_name.to_ascii_lowercase())
            .copied();
        let agent_max_steps = self
            .agent_steps
            .get(&self.agent.to_ascii_lowercase())
            .copied();
        let tool_permissions = self.tool_permissions.clone();
        let agent_registry = self.agent_registry.clone();
        let websearch_config = self.websearch.clone();
        let mcp_config = self.mcp.clone();
        let custom_instructions = self.custom_instructions.clone();
        let process_registry = self.process_registry.clone();
        let cwd = self.cwd.clone();
        let is_git_repo = crate::utils::git::is_git_repo(&cwd).unwrap_or(false);

        self.start_session_title_generation(&session_id, _user_message);

        // Build messages with system prompt
        let mut messages = self.chat_state.chat.messages.clone();

        // Check if we already have a system message
        let has_system = messages
            .iter()
            .any(|m| m.role == crate::session::types::MessageRole::System);

        if !has_system {
            let prompt_registry = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let registry = crate::tools::initialize_tool_registry_with_dynamic_config(
                        Some(sender.clone()),
                        tool_permissions.clone(),
                        agent_registry.clone(),
                        cancel_token.clone(),
                        Some(&provider_name),
                        &websearch_config,
                        &mcp_config,
                        &cwd,
                        process_registry.clone(),
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
            .with_agent_registry(agent_registry.clone())
            .with_active_agent(agent_mode.clone())
            .with_custom_instructions(custom_instructions);
            let system_prompt = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async { composer.compose().await })
            });
            let system_msg = crate::session::types::Message::system(system_prompt);
            messages.insert(0, system_msg);
        }

        Self::apply_turn_guidance(&mut messages, turn_guidance);

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
                websearch_config,
                mcp_config,
                cwd,
                None,
                messages,
                sender_clone.clone(),
                process_registry,
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

    pub async fn remote_submit_input(&mut self, prompt: String) -> Result<String> {
        self.remote_submit_input_with_images(prompt, Vec::new())
            .await
    }

    fn resume_remote_wait_if_clear(&mut self) {
        if self.focus_pending_priority_overlay() {
            return;
        }

        self.chat_state.chat.resume_streaming_tps_timer();
        if let Some(session_id) = self.session_manager.get_current_session_id().cloned() {
            let _ = self.session_manager.set_session_status(
                &session_id,
                crate::session::types::SessionStatus::Streaming,
                None,
            );
        }
        self.overlay_focus = OverlayFocus::None;
    }

    pub fn remote_respond_permission(&mut self, response: PermissionResponse) -> bool {
        if !self.permission_dialog_state.has_active() {
            return false;
        }

        self.permission_dialog_state.respond_current(response);
        if self.permission_dialog_state.has_active() {
            self.overlay_focus = OverlayFocus::PermissionDialog;
        } else {
            self.resume_remote_wait_if_clear();
        }
        true
    }

    pub fn remote_answer_question(&mut self, answers: serde_json::Value) -> bool {
        if !self.question_dialog_state.has_active() {
            return false;
        }

        self.question_dialog_state.respond_current(answers);
        if self.question_dialog_state.has_active() {
            self.overlay_focus = OverlayFocus::QuestionDialog;
        } else {
            self.resume_remote_wait_if_clear();
        }
        true
    }

    pub fn remote_cancel_question(&mut self) -> bool {
        if !self.question_dialog_state.has_active() {
            return false;
        }

        self.question_dialog_state.clear_with_empty();
        self.resume_remote_wait_if_clear();
        self.cancel_streaming();
        true
    }

    pub async fn remote_submit_input_with_images(
        &mut self,
        prompt: String,
        image_paths: Vec<std::path::PathBuf>,
    ) -> Result<String> {
        let prompt = Self::remote_prompt_with_image_placeholders(prompt, image_paths.len());
        if prompt.trim().is_empty() {
            anyhow::bail!("prompt cannot be empty");
        }

        let input = prompt.trim();
        let parsed_input = crate::command::parser::parse_input(input);
        let is_message = matches!(parsed_input, crate::command::parser::InputType::Message(_));
        let agent_mention = match &parsed_input {
            crate::command::parser::InputType::AgentMention(mention) => {
                Some((mention.agent.clone(), mention.prompt.trim().is_empty()))
            }
            _ => None,
        };

        if let crate::command::parser::InputType::Command(parsed) = &parsed_input {
            if !image_paths.is_empty() {
                anyhow::bail!("Images can only be attached to chat prompts");
            }
            let command = self
                .command_registry
                .get(&parsed.name)
                .ok_or_else(|| anyhow::anyhow!("Unknown command: {}", parsed.name))?;
            if is_remote_browser_unsupported_command(&command.name) {
                anyhow::bail!(
                    "Command /{} is not available in the browser UI",
                    command.name
                );
            }
            if self.base_focus != BaseFocus::Chat
                && self.command_registry.is_chat_only(&parsed.name)
            {
                anyhow::bail!("Command /{} requires an active chat", command.name);
            }
        }

        if let Some(session_id) = self.session_manager.get_current_session_id().cloned() {
            self.ensure_session_view_state(&session_id);
            let active_session_can_queue = self.session_has_active_stream(&session_id)
                || self.session_has_active_compaction(&session_id);

            if is_message && self.is_streaming && active_session_can_queue {
                if self.queue_message_for_current_session(prompt.clone(), image_paths.clone()) {
                    return Ok(session_id);
                }
            }
        }

        match parsed_input {
            crate::command::parser::InputType::Command(_) => {
                self.process_input(input).await;
            }
            crate::command::parser::InputType::Message(msg) => {
                self.handle_message_input_with_images(msg, image_paths);
            }
            crate::command::parser::InputType::AgentMention(mention) => {
                if self.is_streaming {
                    anyhow::bail!("Cannot start @{} while streaming", mention.agent);
                }
                self.handle_agent_mention_input(mention, image_paths);
            }
        }

        let session_id = self.session_manager.get_current_session_id().cloned();

        if is_message {
            let Some(session_id) = session_id.as_deref() else {
                anyhow::bail!("failed to create or select a session");
            };
            if !self.session_has_active_stream(session_id) {
                anyhow::bail!("failed to start generation");
            }
        }

        if let Some((agent, prompt_empty)) = agent_mention {
            if prompt_empty {
                anyhow::bail!("Usage: @{} <task>", agent);
            }
            if session_id
                .as_deref()
                .is_none_or(|id| !self.session_has_active_stream(id))
            {
                anyhow::bail!("failed to start @{} agent", agent);
            }
        }

        Ok(session_id.unwrap_or_default())
    }

    fn remote_prompt_with_image_placeholders(prompt: String, image_count: usize) -> String {
        if image_count == 0 {
            return prompt;
        }

        let mut output = prompt;
        for index in 1..=image_count {
            let placeholder = format!("[Image #{}]", index);
            if !output.contains(&placeholder) {
                if !output.is_empty() && !output.chars().last().is_some_and(char::is_whitespace) {
                    output.push(' ');
                }
                output.push_str(&placeholder);
            }
        }
        output
    }

    pub fn remote_autocomplete_suggestions(
        &self,
        trigger: &str,
        query: &str,
        is_chat: bool,
    ) -> Vec<crate::autocomplete::Suggestion> {
        match trigger {
            "slash" => {
                let suggestions = self
                    .input
                    .autocomplete
                    .as_ref()
                    .map(|ac| ac.command_auto.get_suggestions(query, is_chat))
                    .unwrap_or_else(|| {
                        crate::autocomplete::CommandAuto::new(&self.command_registry)
                            .get_suggestions(query, is_chat)
                    });
                suggestions
                    .into_iter()
                    .filter(|suggestion| !is_remote_browser_unsupported_command(&suggestion.name))
                    .collect()
            }
            "mention" => {
                let query_lower = query.to_ascii_lowercase();
                let mut suggestions = self
                    .agent_registry
                    .visible_subagents()
                    .into_iter()
                    .filter(|agent| agent.name.to_ascii_lowercase().starts_with(&query_lower))
                    .map(|agent| {
                        crate::autocomplete::Suggestion::agent(
                            agent.name.clone(),
                            agent.description.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                suggestions.extend(
                    self.input
                        .autocomplete
                        .as_ref()
                        .map(|autocomplete| autocomplete.file_auto.get_suggestions(query))
                        .unwrap_or_default(),
                );
                suggestions
            }
            _ => Vec::new(),
        }
    }

    pub fn remote_skills(&self) -> Vec<crate::skill::SkillInfo> {
        crate::skill::get_skill_store()
            .map(|store| store.all().into_iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn remote_mcp_servers(&self) -> Vec<crate::remote_mcp::RemoteMcpServer> {
        crate::remote_mcp::remote_mcp_servers(&self.mcp)
    }

    pub fn remote_toggle_mcp_server(
        &mut self,
        name: &str,
    ) -> Result<Vec<crate::remote_mcp::RemoteMcpServer>> {
        let prefs = self
            .prefs_dao
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("preferences unavailable"))?;
        crate::remote_mcp::remote_toggle_mcp_server(prefs, &mut self.mcp, name)
    }

    pub fn remote_queued_message_previews(&self) -> Vec<String> {
        self.queued_message_previews_for_current_session()
    }

    pub fn remote_send_queued_now(&mut self) -> bool {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };
        self.interrupt_streaming_to_send_queued_for_session(&session_id)
    }

    pub fn remote_cancel_current(&mut self) -> bool {
        let Some(session_id) = self.session_manager.get_current_session_id().cloned() else {
            return false;
        };

        if !self.session_has_active_stream(&session_id) {
            return false;
        }

        self.cancel_streaming_for_session(&session_id);
        true
    }

    pub fn remote_start_blank_session(&mut self, workspace_path: Option<String>) -> Result<()> {
        if let Some(workspace_path) = workspace_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = self.resolve_remote_workspace_path(workspace_path)?;
            self.set_remote_workspace_path(path)?;
        }
        self.start_blank_session(None);
        Ok(())
    }

    pub fn remote_select_workspace(&mut self, workspace_path: String) -> Result<()> {
        let path = self.resolve_remote_workspace_path(&workspace_path)?;
        self.set_remote_workspace_path(path)?;
        self.start_blank_session(None);
        Ok(())
    }

    pub fn remote_archive_session(&mut self, session_id: &str) -> Result<()> {
        let was_current = self
            .session_manager
            .get_current_session_id()
            .map_or(false, |current| current == session_id);
        self.session_manager
            .set_session_archived(session_id, true)
            .map_err(|err| anyhow::anyhow!("{err:?}"))?;
        if was_current {
            self.save_active_session_view_state();
            self.pending_session_title = None;
            self.session_manager.clear_current_session();
            self.chat_state.chat.clear();
            self.input.clear();
            self.base_focus = BaseFocus::Home;
            self.sync_active_streaming_flag();
            self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
        }
        self.refresh_sessions_dialog();
        Ok(())
    }

    pub fn remote_archive_workspace(&mut self, workspace_path: String) -> Result<()> {
        let path_text = workspace_path.trim().to_string();
        if path_text.is_empty() {
            anyhow::bail!("workspace path cannot be empty");
        }
        let active_workspace = self.remote_workspace_path() == path_text;
        let _ = self
            .session_manager
            .set_workspace_archived(&path_text, true)
            .map_err(|err| anyhow::anyhow!("{err:?}"))?;

        if active_workspace {
            self.save_active_session_view_state();
            self.pending_session_title = None;
            self.session_manager.clear_current_session();
            self.chat_state.chat.clear();
            self.input.clear();
            self.base_focus = BaseFocus::Home;
            self.sync_active_streaming_flag();
            self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);

            let fallback_path = self.session_manager.current_workspace_path().to_string();
            if fallback_path != path_text && !fallback_path.trim().is_empty() {
                let _ = self.set_remote_workspace_path(std::path::PathBuf::from(fallback_path));
            }
        }

        self.refresh_sessions_dialog();
        Ok(())
    }

    pub fn remote_switch_session(&mut self, session_id: &str) -> bool {
        let workspace_path = self
            .session_manager
            .get_session_ref(session_id)
            .map(|session| session.workspace_path.clone());

        if !self.switch_to_session(session_id) {
            return false;
        }

        if let Some(workspace_path) = workspace_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            if let Ok(path) = self.resolve_remote_workspace_path(workspace_path) {
                let _ = self.set_remote_workspace_path(path);
            }
        }

        true
    }

    pub fn remote_model_items(&mut self) -> Vec<crate::ui::components::dialog::DialogItem> {
        self.refresh_models_dialog();
        self.models_dialog_state.dialog.items.clone()
    }

    pub fn remote_set_model(&mut self, provider_id: String, model_id: String) -> bool {
        let exists = self
            .remote_model_items()
            .into_iter()
            .any(|item| item.provider_id == provider_id && item.id == model_id);

        if !exists {
            return false;
        }

        self.model = model_id.clone();
        self.provider_name = provider_id.clone();
        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);

        if let Some(ref dao) = self.prefs_dao {
            let _ = dao.set_active_model(provider_id, model_id);
        }

        true
    }

    pub fn remote_recover_after_client_quit(&mut self) {
        self.running = true;

        if self.session_manager.get_current_session_id().is_none() {
            self.base_focus = BaseFocus::Home;
            self.note_user_activity();
            self.overlay_focus = OverlayFocus::None;
            self.pending_session_title = None;
            self.input.clear();
            self.chat_state.chat.clear();
            self.clear_suggestions_and_blur();
        }
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
            state.stream = Some(SessionStreamState::new(
                receiver,
                cancel_token.clone(),
                streaming_model,
                streaming_provider,
                chat_len_before_assistant,
            ));
            state.tool_calls = ToolCallViewState::default();
            state.unread_completed = false;
            state.retry_status = None;
        }
        self.persist_chat_messages_for_session(&session_id);
        let _ = self.session_manager.set_session_status(
            &session_id,
            crate::session::types::SessionStatus::Streaming,
            None,
        );
        self.mark_sessions_dialog_live_dirty();

        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let reasoning_effort = self.active_reasoning_effort();
        let parent_agent = self.agent.clone();
        let tool_permissions = self.tool_permissions.clone();
        let agent_registry = self.agent_registry.clone();
        let process_registry = self.process_registry.clone();
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
                    process_registry,
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
        self.cached_usage_check = (usize::MAX, u64::MAX, usize::MAX);
    }

    fn submit_queued_messages_for_session(&mut self, session_id: &str) -> bool {
        self.submit_queued_messages_for_session_with_guidance(session_id, None)
    }

    fn submit_queued_messages_for_session_after_interruption(&mut self, session_id: &str) -> bool {
        self.submit_queued_messages_for_session_with_guidance(
            session_id,
            Some(Self::INTERRUPTED_TURN_CONTINUATION_GUIDANCE),
        )
    }

    fn submit_queued_messages_for_session_with_guidance(
        &mut self,
        session_id: &str,
        turn_guidance: Option<&str>,
    ) -> bool {
        if !self.is_active_session(session_id)
            || self.session_has_active_stream(session_id)
            || self.session_has_active_compaction(session_id)
        {
            return false;
        }

        let queued_items = self.drain_queued_items_for_session(session_id);
        if queued_items.is_empty() {
            return false;
        }

        // Preserve order: batch leading messages, then /compact, then re-queue the rest.
        let mut leading_messages = Vec::new();
        let mut rest = Vec::new();
        let mut saw_compact = false;
        let mut run_compact = false;

        for item in queued_items {
            if saw_compact {
                rest.push(item);
                continue;
            }
            match item {
                QueuedItem::Message(message) => leading_messages.push(message),
                QueuedItem::Compact => {
                    saw_compact = true;
                    run_compact = true;
                }
            }
        }

        if let Some(state) = self.session_view_states.get_mut(session_id) {
            for item in rest {
                state.queued_items.push_back(item);
            }
        }

        self.base_focus = BaseFocus::Chat;

        if !leading_messages.is_empty() {
            // Re-queue compact so it runs after this message turn finishes.
            if run_compact {
                if let Some(state) = self.session_view_states.get_mut(session_id) {
                    state.queued_items.push_front(QueuedItem::Compact);
                }
            }

            let queued = Self::combine_queued_messages(leading_messages);
            let prompt = queued.text.clone();
            self.append_user_message_to_current_session(queued.text, queued.image_paths);

            if let Err(e) = self.start_llm_streaming_with_guidance(&prompt, turn_guidance) {
                self.play_sound_event(crate::sound::SoundEvent::Error);
                self.notify_terminal_event(crate::sound::SoundEvent::Error);
                push_toast(Toast::new(
                    format!("LLM error: {}", e),
                    ToastLevel::Error,
                    None,
                ));
                return false;
            }

            return true;
        }

        if run_compact {
            self.start_compact_session(session_id);
            return true;
        }

        false
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
                self.play_sound_event(crate::sound::SoundEvent::Error);
                self.notify_terminal_event(crate::sound::SoundEvent::Error);
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
                self.play_sound_event(crate::sound::SoundEvent::Error);
                self.notify_terminal_event(crate::sound::SoundEvent::Error);
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

        // Solid canvas when transparency is off. Color::Reset (transparent on)
        // leaves the terminal's own background showing through.
        if colors.background != ratatui::style::Color::Reset {
            f.render_widget(
                Block::default().style(Style::default().bg(colors.background)),
                size,
            );
        }

        let fingerprint = (
            self.chat_state.chat.messages.len(),
            self.chat_state.chat.render_revision(),
            if self.is_streaming {
                self.chat_state.chat.streaming_token_count() / 256
            } else {
                0
            },
        );
        if self.cached_usage_check != fingerprint {
            self.cached_usage_check = fingerprint;
            self.cached_usage_text = self.session_usage_text();
        }
        let status_cwd = self.active_workspace_path();
        let branch = self.current_git_branch(&status_cwd);
        let reasoning_effort = self.active_reasoning_effort_label();
        self.refresh_mcp_summary();
        let mcp_summary = self.mcp_summary;
        let model_name = self.model_name_for_display(&self.provider_name, &self.model);
        let provider_name = self.provider_name_for_display(&self.provider_name);
        let usage_text = &self.cached_usage_text;

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
                    model_name,
                    provider_name,
                    reasoning_effort.clone(),
                    mcp_summary,
                    &colors,
                    usage_text,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::AgentsDialog
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
                let retry_status = self.current_session_retry_status();
                let below_chat = self.dialog_below_chat_height(size);
                let scroll_padding = if self.overlay_focus == OverlayFocus::QuestionDialog
                    && self.question_dialog_state.has_active()
                {
                    self.question_dialog_state
                        .chat_scroll_bottom_padding(below_chat) as usize
                } else if self.overlay_focus == OverlayFocus::PermissionDialog
                    && self.permission_dialog_state.has_active()
                {
                    self.permission_dialog_state
                        .chat_scroll_bottom_padding(below_chat) as usize
                } else {
                    0
                };
                self.chat_state
                    .chat
                    .set_scroll_bottom_padding(scroll_padding);
                let is_streaming = self.is_streaming;
                let is_compacting = self.compaction_receiver.is_some();
                let esc_cancel_primed = is_streaming && self.esc_is_primed();
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
                    is_streaming,
                    is_compacting,
                    esc_cancel_primed,
                    retry_status.as_ref(),
                    usage_text,
                    subagent_tabs,
                    &queued_messages,
                    &mut self.find_bar,
                    self.overlay_focus == OverlayFocus::None,
                    self.session_manager
                        .get_current_session()
                        .map(|s| s.title.as_str()),
                    self.process_registry.running_count(),
                    &mut self.jobs_chip_area,
                );

                if is_suggestions_visible(&self.suggestions_popup_state)
                    && self.overlay_focus != OverlayFocus::AgentsDialog
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

        if self.overlay_focus == OverlayFocus::AgentsDialog
            && self.agents_dialog_state.dialog.is_visible()
        {
            render_agents_dialog(f, &mut self.agents_dialog_state, size, colors);
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

        if self.overlay_focus == OverlayFocus::VariantsDialog
            && self.variants_dialog_state.dialog.is_visible()
        {
            render_variants_dialog(f, &mut self.variants_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::RefreshModelsDialog {
            crate::views::models_dialog::render_refresh_models_dialog(
                f,
                size,
                colors,
                self.session_spinner_frame,
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

        if self.overlay_focus == OverlayFocus::ProviderOAuthFlow
            && self.provider_oauth_flow_state.is_visible()
        {
            render_provider_oauth_flow(f, &mut self.provider_oauth_flow_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::ApiKeyInput && self.api_key_input.is_visible() {
            self.api_key_input.render(f, size, &colors);
        }

        if self.overlay_focus == OverlayFocus::SessionsDialog
            && self.sessions_dialog_state.dialog.is_visible()
        {
            render_sessions_dialog(f, &mut self.sessions_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::MoveSessionDialog
            && self.move_session_dialog_state.dialog.is_visible()
        {
            render_move_session_dialog(f, &mut self.move_session_dialog_state, size, colors);
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

        if self.overlay_focus == OverlayFocus::McpDialog
            && self.mcp_dialog_state.dialog.is_visible()
        {
            render_mcp_dialog(f, &mut self.mcp_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::TitleDialog && self.title_dialog_state.is_visible() {
            render_title_dialog(f, &mut self.title_dialog_state, size, colors);
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

        if self.jobs_dialog_state.is_visible()
            && (self.overlay_focus == OverlayFocus::JobsDialog
                || self.jobs_dialog_state.dialog.is_visible())
        {
            render_jobs_dialog(f, &mut self.jobs_dialog_state, size, colors);
        }

        if self.overlay_focus == OverlayFocus::CopyActions {
            if let Some(ref mut dialog) = self.copy_actions_dialog {
                dialog.render(f, size, colors);
            }
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

        let below_chat = self.dialog_below_chat_height(size);
        if self.overlay_focus == OverlayFocus::PermissionDialog
            && self.permission_dialog_state.has_active()
        {
            render_permission_dialog(f, &mut self.permission_dialog_state, size, colors);
            let padding = self
                .permission_dialog_state
                .chat_scroll_bottom_padding(below_chat) as usize;
            self.chat_state.chat.set_scroll_bottom_padding(padding);
        } else if self.overlay_focus == OverlayFocus::QuestionDialog
            && self.question_dialog_state.has_active()
        {
            render_question_dialog(f, &mut self.question_dialog_state, size, colors);
            let padding = self
                .question_dialog_state
                .chat_scroll_bottom_padding(below_chat) as usize;
            self.chat_state.chat.set_scroll_bottom_padding(padding);
        } else {
            self.chat_state.chat.set_scroll_bottom_padding(0);
        }

        if self.overlay_focus == OverlayFocus::TerminalSessionDialog
            && self.terminal_session_dialog_state.has_active()
        {
            render_terminal_session_dialog(
                f,
                &mut self.terminal_session_dialog_state,
                size,
                colors,
            );
        }

        if self.overlay_focus == OverlayFocus::RemoteDialog && self.remote_dialog_state.is_visible()
        {
            let submit_enabled = self.can_launch_remote_now();
            render_remote_dialog(
                f,
                &mut self.remote_dialog_state,
                size,
                colors,
                submit_enabled,
            );
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
                    self.current_chat_area(),
                    self.chat_state.chat.resolved_scroll_offset(),
                    &self.chat_state.chat.selection,
                    state,
                ),
                SelectionActionTarget::Input => input_selection_action_bar_area(
                    size,
                    self.suggestions_popup_anchor_area(),
                    self.input.selection_screen_row(),
                ),
                SelectionActionTarget::JobsDetail => {
                    self.current_selection_action_bar_area().unwrap_or_default()
                }
            };
            render_selection_action_bar(f, area, state, &colors);
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

const SELECTION_ACTION_BAR_WIDTH: u16 = 47;
const CHAT_SELECTION_ACTION_ADD_TO_PROMPT_COL: usize = 8;
const CHAT_SELECTION_ACTION_OPEN_IN_EDITOR_COL: usize = 24;
const CHAT_SELECTION_ACTION_ESC_COL: usize = 43;
const CHAT_SELECTION_ACTION_ESC_COL_NO_EDITOR: usize = 24;
const INPUT_SELECTION_ACTION_ESC_COL: usize = 8;

fn selection_action_for_column(state: SelectionActionBarState, column: usize) -> SelectionAction {
    match state.target {
        SelectionActionTarget::Chat if column < CHAT_SELECTION_ACTION_ADD_TO_PROMPT_COL => {
            SelectionAction::Copy
        }
        SelectionActionTarget::Chat if column < CHAT_SELECTION_ACTION_OPEN_IN_EDITOR_COL => {
            SelectionAction::AddToPrompt
        }
        SelectionActionTarget::Chat
            if state.can_open_in_editor && column < CHAT_SELECTION_ACTION_ESC_COL =>
        {
            SelectionAction::OpenInEditor
        }
        SelectionActionTarget::Chat
            if !state.can_open_in_editor && column < CHAT_SELECTION_ACTION_ESC_COL_NO_EDITOR =>
        {
            SelectionAction::Dismiss
        }
        SelectionActionTarget::Chat => SelectionAction::Dismiss,
        SelectionActionTarget::Input | SelectionActionTarget::JobsDetail
            if column < INPUT_SELECTION_ACTION_ESC_COL =>
        {
            SelectionAction::Copy
        }
        SelectionActionTarget::Input | SelectionActionTarget::JobsDetail => {
            SelectionAction::Dismiss
        }
    }
}

fn chat_selection_action_bar_area(
    chat_area: Rect,
    scroll_offset: usize,
    selection: &crate::ui::selection::Selection,
    state: SelectionActionBarState,
) -> Rect {
    let content_area = Rect {
        x: chat_area.x,
        y: chat_area.y,
        width: chat_area.width.saturating_sub(2),
        height: chat_area.height,
    };
    let ((start_line, start_col), (end_line, _)) = selection.range();
    // Stick-to-bottom stores `scroll_offset = usize::MAX`. Resolve it here too so
    // the bar still anchors to the selection if a caller forgets.
    let visible_start = if scroll_offset == usize::MAX {
        start_line.saturating_sub(content_area.height.saturating_sub(1) as usize)
    } else {
        scroll_offset
    };
    selection_action_bar_area_for_anchor(
        content_area,
        visible_start,
        start_line,
        end_line,
        start_col,
        selection_action_bar_width(state),
    )
}

fn input_selection_action_bar_area(
    frame_area: Rect,
    input_area: Rect,
    selection_row: Option<u16>,
) -> Rect {
    // Prefer one row above the selection; fall back to one above the input box.
    let y = selection_row.unwrap_or(input_area.y).saturating_sub(1);
    let x = input_area.x.saturating_add(1);
    clamp_action_bar_area(
        frame_area,
        Rect::new(
            x,
            y,
            selection_action_bar_width(SelectionActionBarState {
                target: SelectionActionTarget::Input,
                can_open_in_editor: false,
            })
            .min(frame_area.width),
            1,
        ),
    )
}

fn selection_action_bar_width(state: SelectionActionBarState) -> u16 {
    match state.target {
        SelectionActionTarget::Chat if state.can_open_in_editor => SELECTION_ACTION_BAR_WIDTH,
        SelectionActionTarget::Chat => CHAT_SELECTION_ACTION_ESC_COL_NO_EDITOR as u16 + 4,
        SelectionActionTarget::Input | SelectionActionTarget::JobsDetail => {
            INPUT_SELECTION_ACTION_ESC_COL as u16 + 4
        }
    }
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
    state: SelectionActionBarState,
    colors: &theme::ThemeColors,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    f.render_widget(Clear, area);
    let bg = colors.info;
    let fg = theme::contrast_text(bg);
    let key_style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(fg).bg(bg);
    let line = if state.target == SelectionActionTarget::Chat {
        let mut spans = vec![
            Span::raw(" "),
            Span::styled("y", key_style),
            Span::styled(" copy ", label_style),
            Span::styled("i", key_style),
            Span::styled(" add to prompt ", label_style),
        ];
        if state.can_open_in_editor {
            spans.push(Span::styled("e", key_style));
            spans.push(Span::styled(" open in editor ", label_style));
        }
        spans.push(Span::styled("esc", key_style));
        spans.push(Span::raw(" "));
        Line::from(spans)
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

fn message_response_markdown(message: &crate::session::types::Message) -> Option<String> {
    (message.role == crate::session::types::MessageRole::Assistant)
        .then(|| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

fn is_remote_browser_unsupported_command(name: &str) -> bool {
    matches!(
        name,
        "connect"
            | "exit"
            | "home"
            | "remote"
            | "sessions"
            | "skills"
            | "mcp"
            | "themes"
            | "timeline"
    )
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

    if matches!(message.role, crate::session::types::MessageRole::Assistant) {
        for part in &message.parts {
            if !matches!(part.part_type.as_str(), "tool_call" | "tool_result") {
                continue;
            }

            let content =
                serde_json::to_string_pretty(&part.data).unwrap_or_else(|_| part.data.to_string());
            let label = if part.part_type == "tool_call" {
                "Tool Call"
            } else {
                "Tool Result"
            };
            sections.push(format!("{label}:\n{content}"));
        }
    }

    sections
}

fn fork_title_from_session_title(title: &str) -> String {
    let title = title.trim();
    let Some(after_marker) = title.strip_prefix("[fork") else {
        return format!("[fork1] {}", non_empty_fork_base_title(title));
    };

    let Some((number, rest)) = after_marker.split_once(']') else {
        return format!("[fork1] {}", non_empty_fork_base_title(title));
    };

    let Ok(number) = number.parse::<usize>() else {
        return format!("[fork1] {}", non_empty_fork_base_title(title));
    };

    format!(
        "[fork{}] {}",
        number.saturating_add(1),
        non_empty_fork_base_title(rest.trim_start())
    )
}

fn non_empty_fork_base_title(title: &str) -> &str {
    if title.is_empty() {
        "fork"
    } else {
        title
    }
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
    use crate::tools::{PermissionAction, PermissionPrompt};
    use serde_json::json;

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
            chat_state: init_chat(Chat::new(), "Build", &colors, true),
            suggestions_popup_state: init_suggestions_popup(Popup::new()),
            agents_dialog_state: init_agents_dialog("Select agent", vec![]),
            models_dialog_state: init_models_dialog("Models", vec![]),
            variants_dialog_state: VariantsDialogState::new(),
            themes_dialog_state: init_themes_dialog("Themes", vec![], false),
            themes_dialog_original_theme_index: 0,
            themes_dialog_original_dark_mode: true,
            themes_dialog_committed: false,
            connect_dialog_state: init_connect_dialog(),
            connect_dialog_mode: ConnectDialogMode::ProviderSelection,
            provider_oauth_flow_state: init_provider_oauth_flow(),
            sessions_dialog_state: init_sessions_dialog("Sessions", vec![]),
            move_session_dialog_state: init_move_session_dialog(),
            session_rename_dialog_state: init_session_rename_dialog(colors),
            permission_dialog_state: init_permission_dialog(),
            question_dialog_state: init_question_dialog(),
            terminal_session_dialog_state: init_terminal_session_dialog(),
            remote_dialog_state: init_remote_dialog(),
            skills_dialog_state: crate::views::skills_dialog::init_skills_dialog("Skills", vec![]),
            mcp_dialog_state: init_mcp_dialog("MCP", vec![]),
            command_palette_state: init_command_palette(),
            find_bar: FindBar::new(),
            storage_dialog_state: init_storage_dialog(),
            title_dialog_state: init_title_dialog(),
            which_key_state: crate::views::which_key::init_which_key(),
            timeline_dialog_state: crate::views::timeline_dialog::init_timeline_dialog(),
            jobs_dialog_state: init_jobs_dialog(),
            jobs_chip_area: None,
            esc_primed_at: None,
            copy_actions_dialog: None,
            message_actions_index: None,
            message_actions_dialog: None,
            message_actions_return_focus: OverlayFocus::TimelineDialog,
            selection_action_bar: None,
            pending_chat_message_click: None,
            api_key_input: crate::ui::components::api_key_input::ApiKeyInput::new(),
            provider_oauth_receiver: None,
            provider_oauth_in_progress: None,
            mcp_oauth_receiver: None,
            mcp_oauth_in_progress: None,
            compaction_receiver: None,
            compaction_pending: None,
            storage_receiver: None,
            models_receiver: None,
            models_dialog_provider_ids: None,
            title_generation_receiver: None,
            prefs_dao: None,
            agent: "Build".to_string(),
            agent_registry: crate::agent::definition::AgentRegistry::default(),
            agent_steps: std::collections::HashMap::new(),
            provider_timeouts: std::collections::HashMap::new(),
            model: "test-model".to_string(),
            provider_name: "test-provider".to_string(),
            small_model: None,
            reasoning_efforts: ReasoningEffortOverrides::new(),
            model_reasoning_options: ModelReasoningOptions::new(),
            cwd: ".".to_string(),
            base_focus: BaseFocus::Home,
            overlay_focus: OverlayFocus::None,
            just_closed_overlay: false,
            ctrl_c_press_count: 0,
            last_ctrl_c_time: std::time::Instant::now(),
            themes: vec![theme],
            current_theme_index: 0,
            dark_mode: true,
            theme_transparent: false,
            sounds: crate::sound::ResolvedSoundsConfig::default(),
            notifications: crate::config::NotificationsConfig::default(),
            images: crate::config::ImagesConfig::default(),
            editor: crate::config::EditorConfig::default(),
            pending_editor_suspend: None,
            websearch: crate::config::configuration::WebsearchConfig::default(),
            mcp: crate::config::configuration::McpConfig::default(),
            mcp_manager: None,
            mcp_summary: crate::views::home::McpSummary::default(),
            config_raw_merged: serde_json::json!({}),
            custom_instructions: String::new(),
            terminal_focused: true,
            tool_permissions: crate::tools::ToolPermissions::new(".".to_string()),
            skills_dirs: Vec::new(),
            is_streaming: false,
            pending_session_title: None,
            session_view_states: std::collections::HashMap::new(),
            session_spinner_frame: 0,
            stream_drain_rotation: 0,
            sessions_dialog_live_dirty: true,
            last_sessions_dialog_metadata_probe: std::time::Instant::now(),
            last_frame_size: ratatui::layout::Rect::default(),
            last_animation_update: std::time::Instant::now(),
            last_user_activity: std::time::Instant::now(),
            last_session_spinner_update: std::time::Instant::now(),
            cached_git_branch: None,
            cached_git_branch_path: ".".to_string(),
            last_git_branch_check: std::time::Instant::now(),
            discovery: None,
            cached_usage_text: String::new(),
            cached_usage_check: (0, 0, 0),
            cached_usage_streaming_base: None,
            terminal_title_enabled: false,
            terminal_title_items: crate::terminal_title::default_items(),
            terminal_title_last: None,
            terminal_title_animation_origin: std::time::Instant::now(),
            remote_launch_request: None,
            startup_hydrated: true,
            pending_model_override: None,
            pending_cli_agent: None,
            process_registry: std::sync::Arc::new(crate::tools::ProcessRegistry::with_workdir(
                std::path::PathBuf::from("."),
            )),
        }
    }

    fn message_action_names(app: &App) -> Vec<String> {
        app.message_actions_dialog
            .as_ref()
            .map(|dialog| dialog.items.iter().map(|item| item.label.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn coalesced_mouse_scroll_reaches_chat_while_find_bar_is_focused() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.base_focus = BaseFocus::Chat;
        app.overlay_focus = OverlayFocus::FindBar;
        app.chat_state.chat.viewport_height = 10;
        app.chat_state.chat.content_height = 100;
        let chat_area = app.current_chat_area();

        app.handle_coalesced_mouse_scroll(
            mouse(MouseEventKind::ScrollDown, chat_area.x, chat_area.y),
            1,
        );

        assert!(app.chat_state.chat.scroll_offset > 0);
    }

    #[test]
    fn reasoning_effort_overrides_are_instance_local() {
        let mut first = test_app();
        let second = test_app();

        first
            .set_reasoning_effort_override_for_model(
                "openai".to_string(),
                "gpt-5".to_string(),
                Some(crate::model::reasoning::ReasoningEffort::High),
            )
            .unwrap();

        assert_eq!(
            first.reasoning_effort_override_for_model("openai", "gpt-5"),
            Some(crate::model::reasoning::ReasoningEffort::High)
        );
        assert_eq!(
            second.reasoning_effort_override_for_model("openai", "gpt-5"),
            None
        );
    }

    #[test]
    fn reasoning_effort_overrides_load_from_persisted_preferences() {
        let mut prefs = crate::persistence::prefs::ModelPreferences::default();
        prefs.set_reasoning_effort(
            "openai".to_string(),
            "gpt-5".to_string(),
            crate::model::reasoning::ReasoningEffort::High,
        );

        let overrides = reasoning_effort_overrides_from_prefs(&prefs);

        assert_eq!(
            overrides.get(&("openai".to_string(), "gpt-5".to_string())),
            Some(&crate::model::reasoning::ReasoningEffort::High)
        );
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
            state.external_stream = Some(ExternalStreamState::new(
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                0,
            ));
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

    #[test]
    fn resolving_permission_promotes_pending_question() {
        let mut app = test_app();
        let (permission_tx, _permission_rx) = tokio::sync::oneshot::channel();
        app.permission_dialog_state.enqueue(PermissionPrompt {
            tool_id: "list".to_string(),
            action: PermissionAction::List,
            permission: "external_directory".to_string(),
            patterns: vec!["/tmp/*".to_string()],
            target: Some("/tmp".to_string()),
            command: None,
            workdir: None,
            reason: "approval required".to_string(),
            response_tx: permission_tx,
        });
        let (question_tx, _question_rx) = tokio::sync::oneshot::channel();
        app.question_dialog_state.enqueue(
            json!([{
                "question": "Continue?",
                "options": [{ "label": "Yes" }, { "label": "No" }]
            }]),
            question_tx,
        );
        app.overlay_focus = OverlayFocus::PermissionDialog;

        assert!(app.remote_respond_permission(PermissionResponse::Deny));

        assert_eq!(app.overlay_focus, OverlayFocus::QuestionDialog);
        assert!(app.question_dialog_state.has_active());
    }

    #[test]
    fn resolving_question_promotes_pending_permission() {
        let mut app = test_app();
        let (permission_tx, _permission_rx) = tokio::sync::oneshot::channel();
        app.permission_dialog_state.enqueue(PermissionPrompt {
            tool_id: "list".to_string(),
            action: PermissionAction::List,
            permission: "external_directory".to_string(),
            patterns: vec!["/tmp/*".to_string()],
            target: Some("/tmp".to_string()),
            command: None,
            workdir: None,
            reason: "approval required".to_string(),
            response_tx: permission_tx,
        });
        let (question_tx, _question_rx) = tokio::sync::oneshot::channel();
        app.question_dialog_state.enqueue(
            json!([{
                "question": "Continue?",
                "options": [{ "label": "Yes" }, { "label": "No" }]
            }]),
            question_tx,
        );
        app.overlay_focus = OverlayFocus::QuestionDialog;

        assert!(app.remote_answer_question(json!([["Yes"]])));

        assert_eq!(app.overlay_focus, OverlayFocus::PermissionDialog);
        assert!(app.permission_dialog_state.has_active());
    }

    #[tokio::test]
    async fn title_command_opens_terminal_title_dialog() {
        let mut app = test_app();

        app.process_input("/title").await;

        assert_eq!(app.overlay_focus, OverlayFocus::TitleDialog);
        assert!(app.title_dialog_state.is_visible());
        assert_eq!(
            app.title_dialog_state.enabled_items(),
            crate::terminal_title::default_items()
        );
    }

    #[tokio::test]
    async fn agents_command_opens_primary_agent_dialog() {
        let mut app = test_app();

        app.process_input("/agents").await;

        assert_eq!(app.overlay_focus, OverlayFocus::AgentsDialog);
        assert!(app.agents_dialog_state.dialog.is_visible());
        assert_eq!(app.agents_dialog_state.dialog.title, "Select agent");
        assert!(app
            .agents_dialog_state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "build" && item.name == "build" && item.active));
        assert!(app
            .agents_dialog_state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "plan"));
        assert!(!app
            .agents_dialog_state
            .dialog
            .items
            .iter()
            .any(|item| item.id == "general"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn command_palette_commands_preserve_chat_input_draft() {
        let mut app = test_app();
        app.input.insert_str("draft prompt");

        app.handle_command_palette_action(CommandPaletteAction::RunCommand("agents".to_string()));

        assert_eq!(app.input.get_text(), "draft prompt");
        assert_eq!(app.overlay_focus, OverlayFocus::AgentsDialog);
        assert!(app.agents_dialog_state.dialog.is_visible());
    }

    #[test]
    fn agent_dialog_includes_config_all_mode_agents() {
        let mut app = test_app();
        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "designer": {
                    "description": "Design UI",
                    "mode": "all",
                    "model": "openai/gpt-5"
                },
                "reviewer": {
                    "description": "Review code",
                    "mode": "subagent"
                }
            })),
            &mut warnings,
        );
        app.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(None, defs);

        let items = app.agent_dialog_items();

        assert!(warnings.is_empty());
        assert!(items.iter().any(|item| {
            item.id == "designer"
                && item.name == "designer"
                && item.group == "Primary + Subagent"
                && item.tip.as_deref() == Some("openai/gpt-5")
        }));
        assert!(!items.iter().any(|item| item.id == "reviewer"));
    }

    #[test]
    fn agent_dialog_excludes_subagents_even_when_they_are_mentionable() {
        let mut app = test_app();
        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "frontend-agent": {
                    "description": "Fast frontend subagent",
                    "mode": "subagent",
                    "model": "xai/grok-composer-2.5-fast",
                    "prompt": "Build polished frontends."
                }
            })),
            &mut warnings,
        );
        app.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(None, defs);

        assert!(warnings.is_empty());
        assert!(app
            .agent_registry
            .visible_agent_names_for_mentions()
            .contains(&"frontend-agent".to_string()));
        assert!(!app
            .agent_dialog_items()
            .iter()
            .any(|item| item.id == "frontend-agent"));
        assert!(!app.set_agent_mode("frontend-agent"));
    }

    #[test]
    fn mode_all_agent_can_be_selected_and_overrides_model_and_reasoning() {
        let mut app = test_app();
        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "frontend-agent": {
                    "description": "Fast frontend agent",
                    "mode": "all",
                    "model": "xai/grok-composer-2.5-fast",
                    "reasoningEffort": "low",
                    "prompt": "Build polished frontends."
                }
            })),
            &mut warnings,
        );
        app.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(None, defs);

        assert!(warnings.is_empty());
        assert!(app.set_agent_mode("frontend-agent"));
        assert_eq!(app.agent, "Frontend-agent");
        assert_eq!(
            app.active_primary_agent_model_provider(),
            ("xai".to_string(), "grok-composer-2.5-fast".to_string())
        );
        assert_eq!(
            app.active_primary_agent_reasoning_effort(),
            Some(crate::model::reasoning::ReasoningEffort::Low)
        );
    }

    #[test]
    fn tab_cycles_through_config_primary_agents() {
        let mut app = test_app();
        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "designer": {
                    "description": "Design UI",
                    "mode": "all"
                }
            })),
            &mut warnings,
        );
        app.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(None, defs);

        assert!(warnings.is_empty());
        assert_eq!(app.agent, "Build");
        app.toggle_agent_mode();
        assert_eq!(app.agent, "Designer");
        app.toggle_agent_mode();
        assert_eq!(app.agent, "Plan");
    }

    #[tokio::test]
    async fn remote_command_opens_dialog() {
        let mut app = test_app();

        app.process_input("/remote").await;

        assert_eq!(app.overlay_focus, OverlayFocus::RemoteDialog);
        assert!(app.remote_dialog_state.is_visible());
        assert!(app.take_remote_launch_request().is_none());
    }

    #[test]
    fn remote_dialog_enter_is_blocked_while_streaming() {
        let mut app = test_app();
        app.open_remote_dialog();
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.running);
        assert!(app.remote_dialog_state.is_visible());
        assert!(app.take_remote_launch_request().is_none());
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

    #[test]
    fn message_response_markdown_only_returns_assistant_text() {
        let mut assistant = crate::session::types::Message::assistant(
            "\n## Result\n\n```rust\nfn main() {}\n```\n",
        );
        assistant.reasoning = Some("Internal reasoning".to_string());

        assert_eq!(
            message_response_markdown(&assistant).as_deref(),
            Some("## Result\n\n```rust\nfn main() {}\n```")
        );
        assert!(
            message_response_markdown(&crate::session::types::Message::user("Prompt")).is_none()
        );
        assert!(
            message_response_markdown(&crate::session::types::Message::assistant("   ")).is_none()
        );
    }

    #[test]
    fn assistant_message_actions_include_markdown_copy() {
        let mut app = test_app();
        app.create_new_session(Some("Copy response".to_string()));
        add_current_session_message(
            &mut app,
            crate::session::types::Message::assistant("**Markdown**"),
        );

        app.show_message_actions(0);

        let dialog = app
            .message_actions_dialog
            .as_ref()
            .expect("message actions");
        assert!(dialog
            .items
            .iter()
            .any(|item| item.id == "copy_response_markdown"));
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
    fn jobs_dialog_click_outside_clears_overlay_focus() {
        // Regression: Dialog::handle_mouse_event hides on outside click and
        // returns Handled (not Close). Without clearing overlay_focus here,
        // keys stay trapped in JobsDialog → TUI looks frozen (ctrl-cc still works).
        let mut app = test_app();
        app.open_jobs_dialog();
        assert_eq!(app.overlay_focus, OverlayFocus::JobsDialog);
        assert!(app.jobs_dialog_state.is_visible());

        app.jobs_dialog_state.dialog.dialog_area = ratatui::layout::Rect::new(10, 5, 40, 12);
        app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));

        assert!(
            !app.jobs_dialog_state.is_visible(),
            "click outside should hide the jobs dialog"
        );
        assert_eq!(
            app.overlay_focus,
            OverlayFocus::None,
            "overlay focus must clear so the input can receive keys again"
        );
    }

    #[test]
    fn selection_action_bar_column_mapping_matches_rendered_labels() {
        let chat_without_editor = SelectionActionBarState {
            target: SelectionActionTarget::Chat,
            can_open_in_editor: false,
        };
        let chat_with_editor = SelectionActionBarState {
            target: SelectionActionTarget::Chat,
            can_open_in_editor: true,
        };
        let input = SelectionActionBarState {
            target: SelectionActionTarget::Input,
            can_open_in_editor: false,
        };
        assert_eq!(
            selection_action_for_column(chat_without_editor, 1),
            SelectionAction::Copy
        );
        assert_eq!(
            selection_action_for_column(chat_without_editor, 8),
            SelectionAction::AddToPrompt
        );
        assert_eq!(
            selection_action_for_column(chat_without_editor, 24),
            SelectionAction::Dismiss
        );
        assert_eq!(
            selection_action_for_column(chat_with_editor, 24),
            SelectionAction::OpenInEditor
        );
        assert_eq!(
            selection_action_for_column(chat_with_editor, 43),
            SelectionAction::Dismiss
        );
        assert_eq!(selection_action_for_column(input, 1), SelectionAction::Copy);
        assert_eq!(
            selection_action_for_column(input, 8),
            SelectionAction::Dismiss
        );
    }

    #[test]
    fn chat_selection_action_bar_anchors_near_selection_when_stuck_to_bottom() {
        let chat_area = Rect::new(0, 0, 80, 20);
        let selection = crate::ui::selection::Selection {
            active: true,
            start_line: 100,
            start_col: 4,
            end_line: 100,
            end_col: 10,
            is_dragging: false,
            anchor: None,
        };
        let state = SelectionActionBarState {
            target: SelectionActionTarget::Chat,
            can_open_in_editor: false,
        };

        // Stick-to-bottom sentinel must resolve instead of pinning to the top.
        let stuck = chat_selection_action_bar_area(chat_area, usize::MAX, &selection, state);
        assert_eq!(stuck.y, chat_area.y + 18);
        assert!(stuck.y > chat_area.y + 10);

        // Explicit resolved scroll (selection near bottom of viewport) matches.
        let resolved_start = 100usize.saturating_sub(19);
        let area = chat_selection_action_bar_area(chat_area, resolved_start, &selection, state);
        assert_eq!(area.y, chat_area.y + 18); // one above last viewport row
        assert_eq!(area.y, stuck.y);
    }

    #[test]
    fn input_selection_action_bar_anchors_above_selection_row() {
        let frame = Rect::new(0, 0, 80, 24);
        let input_area = Rect::new(2, 18, 40, 4);
        let area = input_selection_action_bar_area(frame, input_area, Some(20));
        assert_eq!(area.y, 19);
        let fallback = input_selection_action_bar_area(frame, input_area, None);
        assert_eq!(fallback.y, 17);
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
        assert_eq!(
            app.selection_action_bar.map(|state| state.target),
            Some(SelectionActionTarget::Input)
        );
        assert_eq!(app.current_selection_action_bar_area().unwrap().width, 12);

        let action_area = app.current_selection_action_bar_area().unwrap();
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            action_area.x,
            action_area.y,
        ));
        assert!(!app.input.is_selection_dragging());
        app.handle_mouse_event(mouse(
            MouseEventKind::Up(MouseButton::Left),
            action_area.x,
            action_area.y,
        ));
        assert!(app.selection_action_bar.is_none());
        assert!(!app.input.has_selection());

        // Recreate a selection to retain coverage for a normal release outside the input.
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

        assert!(app.handle_input_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 7, 19)));

        assert_eq!(
            app.selection_action_bar.map(|state| state.target),
            Some(SelectionActionTarget::Input)
        );
    }

    #[test]
    fn permission_dialog_forwards_chat_text_selection() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::assistant(
                "alpha beta gamma",
            ));
        app.chat_state.chat.content_height = 25;
        app.chat_state.chat.viewport_height = 18;
        app.chat_state.chat.scroll_offset = 0;
        let (permission_tx, _permission_rx) = tokio::sync::oneshot::channel();
        app.permission_dialog_state.enqueue(PermissionPrompt {
            tool_id: "list".to_string(),
            action: PermissionAction::List,
            permission: "external_directory".to_string(),
            patterns: vec!["/tmp/*".to_string()],
            target: Some("/tmp".to_string()),
            command: None,
            workdir: None,
            reason: "approval required".to_string(),
            response_tx: permission_tx,
        });
        app.overlay_focus = OverlayFocus::PermissionDialog;

        // Outside dialog controls: start + drag selection on chat text.
        app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        app.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), 8, 1));

        assert!(
            app.chat_state.chat.has_selection() || app.chat_state.chat.selection.is_dragging,
            "chat text should be selectable while permission dialog is open"
        );
    }

    #[test]
    fn question_dialog_forwards_chat_text_selection() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::assistant(
                "alpha beta gamma",
            ));
        app.chat_state.chat.content_height = 25;
        app.chat_state.chat.viewport_height = 18;
        app.chat_state.chat.scroll_offset = 0;
        let (question_tx, _question_rx) = tokio::sync::oneshot::channel();
        app.question_dialog_state.enqueue(
            json!([{
                "question": "Continue?",
                "options": [{ "label": "Yes" }, { "label": "No" }]
            }]),
            question_tx,
        );
        app.overlay_focus = OverlayFocus::QuestionDialog;

        app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        app.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), 8, 1));

        assert!(
            app.chat_state.chat.has_selection() || app.chat_state.chat.selection.is_dragging,
            "chat text should be selectable while question dialog is open"
        );
    }

    #[test]
    fn chat_selection_action_bar_shows_before_mouse_up() {
        let mut app = test_app();
        app.last_frame_size = ratatui::layout::Rect::new(0, 0, 80, 24);
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::assistant("alpha beta"));
        app.chat_state.chat.selection.start(0, 0);
        app.chat_state.chat.selection.extend(0, "alpha".len());

        app.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 0));

        assert!(app.chat_state.chat.selection.is_dragging);
        assert_eq!(
            app.selection_action_bar.map(|state| state.target),
            Some(SelectionActionTarget::Chat)
        );

        let action_area = app.current_selection_action_bar_area().unwrap();
        app.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            action_area.x,
            action_area.y,
        ));
        assert!(!app.chat_state.chat.selection.is_dragging);
        app.handle_mouse_event(mouse(
            MouseEventKind::Up(MouseButton::Left),
            action_area.x,
            action_area.y,
        ));

        assert!(app.selection_action_bar.is_none());
        assert!(!app.chat_state.chat.has_selection());
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
    fn copy_command_opens_action_dialog_with_transcript_default() {
        let mut app = test_app();
        app.create_new_session(Some("Copy me".to_string()));
        app.base_focus = BaseFocus::Chat;

        tokio_test::block_on(app.process_input("/copy"));

        let dialog = app.copy_actions_dialog.as_ref().expect("copy dialog");
        assert_eq!(app.overlay_focus, OverlayFocus::CopyActions);
        assert_eq!(dialog.selected_index(), 0);
        assert_eq!(
            dialog.get_selected().map(|item| item.id.as_str()),
            Some("transcript")
        );
        assert_eq!(dialog.items[0].key, 't');
    }

    #[test]
    fn message_actions_have_shortcuts() {
        let mut app = test_app();
        app.create_new_session(Some("Timeline".to_string()));
        app.session_manager
            .add_message_to_current_session(&crate::session::types::Message::user("Prompt"))
            .unwrap();

        app.show_message_actions(0);
        let dialog = app
            .message_actions_dialog
            .as_ref()
            .expect("message actions");

        assert_eq!(dialog.item_id_for_shortcut('c').as_deref(), Some("copy"));
        assert_eq!(dialog.item_id_for_shortcut('f').as_deref(), Some("fork"));
        assert_eq!(dialog.item_id_for_shortcut('u').as_deref(), Some("undo"));
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
    fn remote_prompt_adds_missing_image_placeholders() {
        assert_eq!(
            App::remote_prompt_with_image_placeholders("look here".to_string(), 2),
            "look here [Image #1] [Image #2]"
        );
        assert_eq!(
            App::remote_prompt_with_image_placeholders("[Image #1] describe".to_string(), 1),
            "[Image #1] describe"
        );
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

    #[tokio::test(flavor = "multi_thread")]
    async fn fork_command_clones_current_session() {
        let mut app = test_app();
        let original_id = app.create_new_session(Some("Original".to_string()));
        app.base_focus = BaseFocus::Chat;
        add_current_session_message(&mut app, crate::session::types::Message::user("Prompt"));
        add_current_session_message(
            &mut app,
            crate::session::types::Message::assistant("Answer"),
        );

        app.process_input("/fork").await;

        let forked_id = app
            .session_manager
            .get_current_session_id()
            .cloned()
            .expect("forked session should be active");
        assert_ne!(forked_id, original_id);
        assert_eq!(app.base_focus, BaseFocus::Chat);
        assert_eq!(app.chat_state.chat.messages.len(), 2);
        assert_eq!(app.chat_state.chat.messages[0].content, "Prompt");
        assert_eq!(app.chat_state.chat.messages[1].content, "Answer");
        assert_eq!(
            app.session_manager
                .get_session_ref(&original_id)
                .unwrap()
                .messages
                .len(),
            2
        );
        assert_eq!(
            app.session_manager
                .get_session_ref(&forked_id)
                .unwrap()
                .title,
            "[fork1] Original"
        );
        assert_eq!(
            app.session_manager
                .get_session_ref(&forked_id)
                .unwrap()
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["Prompt", "Answer"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fork_command_submitted_from_input_does_not_restore_as_original_session_draft() {
        let mut app = test_app();
        let original_id = app.create_new_session(Some("Original".to_string()));
        app.base_focus = BaseFocus::Chat;
        add_current_session_message(&mut app, crate::session::types::Message::user("Prompt"));
        app.input.set_text("/fork");

        app.handle_keys(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let forked_id = app
            .session_manager
            .get_current_session_id()
            .cloned()
            .expect("forked session should be active");
        assert_ne!(forked_id, original_id);

        assert!(app.switch_to_session(&original_id));
        assert!(app.input.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn branch_alias_forks_current_session() {
        let mut app = test_app();
        let original_id = app.create_new_session(Some("Original".to_string()));
        app.base_focus = BaseFocus::Chat;
        add_current_session_message(&mut app, crate::session::types::Message::user("Prompt"));

        app.process_input("/branch").await;

        let forked_id = app
            .session_manager
            .get_current_session_id()
            .cloned()
            .expect("forked session should be active");
        assert_ne!(forked_id, original_id);
        assert_eq!(
            app.session_manager
                .get_session_ref(&forked_id)
                .unwrap()
                .title,
            "[fork1] Original"
        );
        assert_eq!(
            app.session_manager
                .get_session_ref(&forked_id)
                .unwrap()
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["Prompt"]
        );
    }

    #[test]
    fn fork_title_prefixes_current_session_title() {
        assert_eq!(
            fork_title_from_session_title("Original"),
            "[fork1] Original"
        );
        assert_eq!(
            fork_title_from_session_title("  Renamed fork  "),
            "[fork1] Renamed fork"
        );
    }

    #[test]
    fn fork_title_increments_existing_left_fork_prefix() {
        assert_eq!(
            fork_title_from_session_title("[fork1] Original"),
            "[fork2] Original"
        );
        assert_eq!(
            fork_title_from_session_title("[fork12] Original"),
            "[fork13] Original"
        );
    }

    #[test]
    fn commands_can_submit_while_streaming() {
        let input_type = parse_input("/models");

        assert!(App::can_submit_input(&input_type, true));
    }

    #[test]
    fn loaded_models_are_applied_from_receiver() {
        let mut app = test_app();
        app.show_models_dialog("Available Models", Vec::new());
        app.models_dialog_state.start_loading();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(ModelsTaskMessage {
                kind: ModelsTaskKind::Load,
                result: crate::command::registry::CommandResult::ShowDialog {
                    title: "Available Models".to_string(),
                    items: vec![crate::command::registry::DialogItem {
                        id: "test-model".to_string(),
                        name: "Test Model".to_string(),
                        group: "Test".to_string(),
                        description: String::new(),
                        tip: None,
                        provider_id: "test-provider".to_string(),
                        active: false,
                    }],
                },
                provider_signature: models_dialog_provider_ids(),
            })
            .unwrap();
        app.models_receiver = Some(receiver);

        app.process_models_events();

        assert!(app.models_receiver.is_none());
        assert!(!app.models_dialog_state.is_loading());
        assert_eq!(app.overlay_focus, OverlayFocus::ModelsDialog);
        assert_eq!(app.models_dialog_state.dialog.items.len(), 1);
        assert!(app.models_dialog_state.dialog.items[0].active);
    }

    #[test]
    fn loaded_models_reopen_without_starting_another_task() {
        let mut app = test_app();
        app.show_models_dialog(
            "Available Models",
            vec![crate::ui::components::dialog::DialogItem {
                id: "test-model".to_string(),
                name: "Test Model".to_string(),
                group: "Test".to_string(),
                description: String::new(),
                tip: None,
                provider_id: "test-provider".to_string(),
                active: false,
            }],
        );
        // Match the auth/config snapshot used by the reopen cache check.
        app.models_dialog_provider_ids = models_dialog_provider_ids();
        app.models_dialog_state.dialog.hide();
        app.overlay_focus = OverlayFocus::None;

        let mut parsed = match parse_input("/models") {
            crate::command::parser::InputType::Command(parsed) => parsed,
            other => panic!("expected models command, got {other:?}"),
        };

        assert!(app.start_models_command(&mut parsed));
        assert!(app.models_receiver.is_none());
        assert_eq!(app.overlay_focus, OverlayFocus::ModelsDialog);
        assert!(app.models_dialog_state.dialog.is_visible());
        assert!(!app.models_dialog_state.is_loading());
        assert!(app.models_dialog_state.dialog.items[0].active);
    }

    #[test]
    fn refresh_completion_closes_compact_dialog() {
        let mut app = test_app();
        app.show_models_dialog(
            "Available Models",
            vec![crate::ui::components::dialog::DialogItem {
                id: "stale-model".to_string(),
                name: "Stale Model".to_string(),
                group: "Test".to_string(),
                description: String::new(),
                tip: None,
                provider_id: "test-provider".to_string(),
                active: false,
            }],
        );
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(ModelsTaskMessage {
                kind: ModelsTaskKind::Refresh,
                result: crate::command::registry::CommandResult::Success(String::new()),
                provider_signature: models_dialog_provider_ids(),
            })
            .unwrap();
        app.models_receiver = Some(receiver);
        app.overlay_focus = OverlayFocus::RefreshModelsDialog;

        app.process_models_events();

        assert!(app.models_receiver.is_none());
        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert!(app.models_dialog_state.dialog.items.is_empty());
    }

    #[test]
    fn model_tasks_keep_animation_loop_active() {
        let mut app = test_app();
        app.base_focus = BaseFocus::Chat;
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.models_receiver = Some(receiver);

        assert!(app.is_animation_running());
        assert!(!app.is_streaming_animation_only());
    }

    #[test]
    fn home_animation_freezes_after_idle() {
        let mut app = test_app();
        app.base_focus = BaseFocus::Home;
        app.note_user_activity();
        assert!(app.is_animation_running());

        app.last_user_activity = std::time::Instant::now() - std::time::Duration::from_secs(4);
        assert!(
            !app.is_animation_running(),
            "Home alone must not pin the 60fps loop after idle"
        );
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
        app.session_view_states.get_mut(&session_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                0,
            ));
        app.is_streaming = true;
        app.input.insert_str("Then about riolu");

        app.handle_keys(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let state = app.session_view_states.get(&session_id).unwrap();
        assert_eq!(state.queued_items.len(), 1);
        assert!(matches!(
            &state.queued_items[0],
            QueuedItem::Message(m) if m.text == "Then about riolu"
        ));
        assert_eq!(
            app.queued_message_previews_for_current_session(),
            vec!["Then about riolu".to_string()]
        );
        assert!(app.input.is_empty());
        assert!(app.chat_state.chat.messages.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn builtin_command_suggestion_autosubmits() {
        let mut app = test_app();
        app.base_focus = BaseFocus::Chat;
        app.create_new_session(Some("Compact suggest".to_string()));
        app.input.insert_str("/comp");
        set_suggestions(
            &mut app.suggestions_popup_state,
            vec![crate::autocomplete::Suggestion::command(
                "compact",
                "Compact conversation",
            )],
        );

        app.autocomplete_and_submit();

        // Builtin ran immediately — input should not keep `/compact `.
        assert!(app.input.is_empty());
        assert!(!is_suggestions_visible(&app.suggestions_popup_state));
    }

    #[test]
    fn custom_command_suggestion_fills_without_submit() {
        let mut app = test_app();
        app.base_focus = BaseFocus::Chat;
        app.command_registry
            .register_custom(crate::command::custom::CustomCommand {
                name: "shipit".to_string(),
                description: Some("Ship it".to_string()),
                agent: None,
                model: None,
                subtask: None,
                template: "Ship $ARGUMENTS".to_string(),
                source: crate::command::custom::CustomCommandSource::Config(
                    std::path::PathBuf::from("test"),
                ),
                workdir: std::path::PathBuf::from("."),
            });
        app.input.insert_str("/ship");
        set_suggestions(
            &mut app.suggestions_popup_state,
            vec![crate::autocomplete::Suggestion::command(
                "shipit", "Ship it",
            )],
        );

        app.autocomplete_and_submit();

        assert_eq!(app.input.get_text(), "/shipit ");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_while_streaming_is_queued() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Queue compact".to_string()));
        app.base_focus = BaseFocus::Chat;
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState::new(
            receiver,
            tokio_util::sync::CancellationToken::new(),
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            0,
        ));
        app.is_streaming = true;

        app.compact_current_session().await;

        assert!(app.compaction_receiver.is_none());
        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(matches!(
            state.queued_items.front(),
            Some(QueuedItem::Compact)
        ));
        assert_eq!(
            app.queued_message_previews_for_current_session(),
            vec!["/compact".to_string()]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_while_already_compacting_is_rejected() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Already compacting".to_string()));
        app.base_focus = BaseFocus::Chat;
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.compaction_receiver = Some(receiver);
        app.compaction_pending = Some(CompactionPending {
            session_id: session_id.clone(),
            before_tokens: 1_000,
            cancel_token: tokio_util::sync::CancellationToken::new(),
        });
        app.is_streaming = true;

        app.compact_current_session().await;

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(
            state.queued_items.is_empty(),
            "should not queue another /compact while compacting"
        );
        assert!(app.compaction_receiver.is_some());
    }

    #[test]
    fn double_esc_cancels_active_compaction() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Cancel compact".to_string()));
        app.base_focus = BaseFocus::Chat;
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed = cancel_token.clone();
        app.compaction_receiver = Some(receiver);
        app.compaction_pending = Some(CompactionPending {
            session_id: session_id.clone(),
            before_tokens: 1_000,
            cancel_token,
        });
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!observed.is_cancelled());
        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(observed.is_cancelled());
    }

    #[test]
    fn messages_entered_while_compacting_are_queued() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Compact queue".to_string()));
        app.base_focus = BaseFocus::Chat;
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.compaction_receiver = Some(receiver);
        app.compaction_pending = Some(CompactionPending {
            session_id: session_id.clone(),
            before_tokens: 1_000,
            cancel_token: tokio_util::sync::CancellationToken::new(),
        });
        app.sync_active_streaming_flag();
        app.input.insert_str("Then about eevee");

        app.handle_keys(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let state = app.session_view_states.get(&session_id).unwrap();
        assert_eq!(state.queued_items.len(), 1);
        assert!(matches!(
            &state.queued_items[0],
            QueuedItem::Message(m) if m.text == "Then about eevee"
        ));
        assert_eq!(
            app.queued_message_previews_for_current_session(),
            vec!["Then about eevee".to_string()]
        );
        assert!(app.input.is_empty());
        assert!(app.chat_state.chat.messages.is_empty());
    }

    #[test]
    fn streaming_text_chunks_are_coalesced() {
        let chunks = coalesce_streaming_chunks(vec![
            crate::llm::ChunkMessage::Text("hello".to_string()),
            crate::llm::ChunkMessage::Text(" ".to_string()),
            crate::llm::ChunkMessage::Text("world".to_string()),
            crate::llm::ChunkMessage::Warning("careful".to_string()),
            crate::llm::ChunkMessage::Reasoning("think".to_string()),
            crate::llm::ChunkMessage::Reasoning("ing".to_string()),
        ]);

        assert_eq!(chunks.len(), 3);
        match &chunks[0] {
            crate::llm::ChunkMessage::Text(text) => assert_eq!(text, "hello world"),
            _ => panic!("expected coalesced text chunk"),
        }
        match &chunks[2] {
            crate::llm::ChunkMessage::Reasoning(text) => assert_eq!(text, "thinking"),
            _ => panic!("expected coalesced reasoning chunk"),
        }
    }

    #[test]
    fn interleaved_subagent_text_chunks_are_coalesced_per_session() {
        use crate::llm::ChunkMessage;

        let subagent_text = |session_id: &str, text: &str| ChunkMessage::SubagentChunk {
            session_id: session_id.to_string(),
            chunk: Box::new(ChunkMessage::Text(text.to_string())),
        };
        let mut interleaved = Vec::with_capacity(3 * 1024);
        for _ in 0..1024 {
            interleaved.push(subagent_text("a", "a"));
            interleaved.push(subagent_text("b", "b"));
            interleaved.push(subagent_text("c", "c"));
        }
        let chunks = coalesce_streaming_chunks(interleaved);

        assert_eq!(chunks.len(), 3);
        for (chunk, (expected_session, expected_text)) in chunks.iter().zip([
            ("a", "a".repeat(1024)),
            ("b", "b".repeat(1024)),
            ("c", "c".repeat(1024)),
        ]) {
            match chunk {
                ChunkMessage::SubagentChunk { session_id, chunk } => {
                    assert_eq!(session_id, expected_session);
                    assert!(
                        matches!(chunk.as_ref(), ChunkMessage::Text(text) if text == &expected_text)
                    );
                }
                _ => panic!("expected subagent chunk"),
            }
        }
    }

    #[test]
    fn subagent_coalescing_does_not_cross_same_session_tool_events() {
        use crate::llm::ChunkMessage;

        let wrapped = |chunk| ChunkMessage::SubagentChunk {
            session_id: "child".to_string(),
            chunk: Box::new(chunk),
        };
        let chunks = coalesce_streaming_chunks(vec![
            wrapped(ChunkMessage::Text("before".to_string())),
            wrapped(ChunkMessage::Warning("tool boundary".to_string())),
            wrapped(ChunkMessage::Text("after".to_string())),
        ]);

        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn coalescing_preserves_parent_text_boundaries_around_nested_subagent_events() {
        use crate::llm::ChunkMessage;

        let chunks = coalesce_streaming_chunks(vec![
            ChunkMessage::Text("before".to_string()),
            ChunkMessage::SubagentChunk {
                session_id: "child".to_string(),
                chunk: Box::new(ChunkMessage::Text("nested".to_string())),
            },
            ChunkMessage::Text("after".to_string()),
        ]);

        assert_eq!(chunks.len(), 3);
        assert!(matches!(&chunks[0], ChunkMessage::Text(text) if text == "before"));
        assert!(matches!(&chunks[1], ChunkMessage::SubagentChunk { .. }));
        assert!(matches!(&chunks[2], ChunkMessage::Text(text) if text == "after"));
    }

    #[test]
    fn global_stream_drain_respects_shared_time_budget() {
        use crate::llm::ChunkMessage;

        let (sender_a, mut receiver_a) = tokio::sync::mpsc::unbounded_channel();
        let (sender_b, mut receiver_b) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..512 {
            sender_a.send(ChunkMessage::Text("a".to_string())).unwrap();
            sender_b.send(ChunkMessage::Text("b".to_string())).unwrap();
        }
        drop(sender_a);
        drop(sender_b);

        let mut receivers = [
            ("a".to_string(), &mut receiver_a),
            ("b".to_string(), &mut receiver_b),
        ];
        let (drained, _) = drain_streaming_chunks_global(
            &mut receivers,
            STREAM_CHUNK_DRAIN_LIMIT,
            STREAM_CHUNK_GLOBAL_DRAIN_LIMIT,
            std::time::Duration::from_secs(1),
            0,
        );

        let total_chunks: usize = drained.iter().map(|(_, chunks, _)| chunks.len()).sum();
        assert_eq!(total_chunks, 1024);
        let drained_a: usize = drained
            .iter()
            .filter(|(id, _, _)| id == "a")
            .map(|(_, chunks, _)| chunks.len())
            .sum();
        let drained_b: usize = drained
            .iter()
            .filter(|(id, _, _)| id == "b")
            .map(|(_, chunks, _)| chunks.len())
            .sum();
        assert_eq!(drained_a, 512);
        assert_eq!(drained_b, 512);
    }

    #[test]
    fn global_stream_drain_caps_downstream_chunk_work() {
        use crate::llm::ChunkMessage;

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..4096 {
            sender.send(ChunkMessage::Text("x".to_string())).unwrap();
        }

        let mut receivers = [("session".to_string(), &mut receiver)];
        let (drained, _) = drain_streaming_chunks_global(
            &mut receivers,
            STREAM_CHUNK_DRAIN_LIMIT,
            128,
            std::time::Duration::from_secs(1),
            0,
        );

        let total_chunks: usize = drained.iter().map(|(_, chunks, _)| chunks.len()).sum();
        assert_eq!(total_chunks, 128);
        assert_eq!(receiver.len(), 4096 - 128);
    }

    #[test]
    fn global_stream_drain_rotates_fairly_under_tight_budget() {
        use crate::llm::ChunkMessage;

        let (sender_a, mut receiver_a) = tokio::sync::mpsc::unbounded_channel();
        let (sender_b, mut receiver_b) = tokio::sync::mpsc::unbounded_channel();
        let (sender_c, mut receiver_c) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..256 {
            sender_a.send(ChunkMessage::Text("a".to_string())).unwrap();
            sender_b.send(ChunkMessage::Text("b".to_string())).unwrap();
            sender_c.send(ChunkMessage::Text("c".to_string())).unwrap();
        }
        drop(sender_a);
        drop(sender_b);
        drop(sender_c);

        let budget = std::time::Duration::from_micros(200);
        let mut rotation = 0usize;
        let mut served = std::collections::HashSet::new();

        for _ in 0..48 {
            let mut receivers = [
                ("a".to_string(), &mut receiver_a),
                ("b".to_string(), &mut receiver_b),
                ("c".to_string(), &mut receiver_c),
            ];
            let (drained, next_rotation) =
                drain_streaming_chunks_global(&mut receivers, 64, 192, budget, rotation);
            rotation = next_rotation;
            for (id, chunks, _) in drained {
                if !chunks.is_empty() {
                    served.insert(id);
                }
            }
            if served.len() == 3 {
                break;
            }
        }

        assert_eq!(served.len(), 3);
        assert!(served.contains("a"));
        assert!(served.contains("b"));
        assert!(served.contains("c"));
    }

    #[test]
    fn sessions_dialog_streaming_only_uses_streaming_poll_cadence() {
        let mut app = test_app();
        app.overlay_focus = OverlayFocus::SessionsDialog;
        app.sessions_dialog_state.dialog.show();
        app.is_streaming = false;
        app.base_focus = BaseFocus::Home;

        assert!(!app.is_streaming_animation_only());

        let background_id = app.create_new_session(Some("bg".to_string()));
        let _current_id = app.create_new_session(Some("foreground".to_string()));
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states
            .get_mut(&background_id)
            .unwrap()
            .stream = Some(SessionStreamState::new(
            receiver,
            tokio_util::sync::CancellationToken::new(),
            None,
            None,
            0,
        ));

        app.refresh_sessions_dialog();
        assert!(app.is_animation_running());
        assert!(app.is_streaming_animation_only());
    }

    #[test]
    fn background_retry_does_not_force_foreground_stream_to_fast_animation_cadence() {
        let mut app = test_app();
        let background_id = app.create_new_session(Some("background".to_string()));
        let foreground_id = app.create_new_session(Some("foreground".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.is_streaming = true;
        app.session_view_states
            .get_mut(&background_id)
            .unwrap()
            .retry_status = Some(StreamingRetryStatus {
            attempt: 1,
            message: "retrying background stream".to_string(),
            next_epoch_ms: 1,
        });

        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some(foreground_id.as_str())
        );
        assert!(app.is_streaming_animation_only());
    }

    #[test]
    fn streaming_drain_consumes_bursts_larger_than_the_old_frame_cap() {
        use crate::llm::ChunkMessage;

        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..1024 {
            sender.send(ChunkMessage::Text("x".to_string())).unwrap();
        }
        drop(sender);

        let (chunks, disconnected) = drain_streaming_chunks(
            &mut receiver,
            STREAM_CHUNK_DRAIN_LIMIT,
            std::time::Duration::from_secs(1),
        );

        assert_eq!(chunks.len(), 1024);
        assert!(disconnected);
        assert_eq!(coalesce_streaming_chunks(chunks).len(), 1);
    }

    #[test]
    fn disconnected_stream_errors_show_warning_text() {
        assert_eq!(
            disconnected_stream_warning_message(
                "Streaming failed: stream disconnected before completion: websocket closed by server before response.completed",
            )
            .as_deref(),
            Some(
                "Stream disconnected before completion: websocket closed by server before response.completed"
            )
        );
        assert_eq!(
            disconnected_stream_warning_message(
                "Provider stream ended without a terminal completion event",
            )
            .as_deref(),
            Some(
                "Stream disconnected before completion: Provider stream ended without a terminal completion event"
            )
        );
    }

    #[test]
    fn non_disconnected_stream_errors_keep_error_toast() {
        assert!(disconnected_stream_warning_message("OpenAI API error: status=401").is_none());
    }

    #[test]
    fn streaming_text_persistence_is_throttled_until_terminal_chunk() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Streaming throttle".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete(""));
        app.chat_state.chat.begin_streaming_turn();
        app.chat_state
            .chat
            .prepare_streaming_token_counter("test-model");

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(crate::llm::ChunkMessage::Text("hello".to_string()))
            .unwrap();
        sender
            .send(crate::llm::ChunkMessage::Text(" world".to_string()))
            .unwrap();

        app.session_view_states.get_mut(&session_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                0,
            ));
        app.is_streaming = true;

        app.process_streaming_chunks();

        assert_eq!(app.chat_state.chat.messages[0].content, "hello world");
        assert_eq!(
            app.session_manager
                .get_session_ref(&session_id)
                .unwrap()
                .messages
                .get(0)
                .map(|message| message.content.as_str()),
            None
        );

        sender.send(crate::llm::ChunkMessage::End).unwrap();
        app.process_streaming_chunks();

        assert_eq!(
            app.session_manager
                .get_session_ref(&session_id)
                .unwrap()
                .messages[0]
                .content,
            "hello world"
        );
    }

    #[test]
    fn streaming_tool_events_share_snapshot_throttling() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Tool snapshot throttle".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete(""));
        app.chat_state.chat.begin_streaming_turn();

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states.get_mut(&session_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                0,
            ));
        app.is_streaming = true;

        app.add_tool_calls_to_session(
            &session_id,
            vec![crate::llm::ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: crate::llm::FunctionCall {
                    name: "read".to_string(),
                    arguments: serde_json::json!({ "path": "Cargo.toml" }).to_string(),
                },
            }],
        );
        app.add_tool_result_to_session(
            &session_id,
            crate::llm::ToolCallResult {
                tool_call_id: "call_1".to_string(),
                role: "tool".to_string(),
                name: "read".to_string(),
                content: serde_json::json!({
                    "status": "ok",
                    "output_preview": "contents"
                })
                .to_string(),
            },
        );

        assert_eq!(app.chat_state.chat.messages[0].parts.len(), 2);
        assert!(app
            .session_view_states
            .get(&session_id)
            .and_then(|state| state.stream.as_ref())
            .is_some_and(|stream| stream.pending_message_snapshot));
        assert!(app
            .session_manager
            .get_session_ref(&session_id)
            .unwrap()
            .messages
            .iter()
            .all(|message| message.tool_call_part_data("call_1").is_none()
                && message.tool_result_part_data("call_1").is_none()));

        app.finish_streaming_session(&session_id);

        let persisted = &app
            .session_manager
            .get_session_ref(&session_id)
            .unwrap()
            .messages[0];
        assert_eq!(persisted.parts.len(), 2);
        assert!(persisted.tool_result_part_data("call_1").is_some());
    }

    #[test]
    fn streaming_text_chunks_do_not_dirty_sessions_dialog_live_state() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Text only".to_string()));
        app.overlay_focus = OverlayFocus::SessionsDialog;
        app.sessions_dialog_state.dialog.show();
        app.refresh_sessions_dialog();

        app.base_focus = BaseFocus::Chat;
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete(""));
        app.chat_state.chat.begin_streaming_turn();
        // Warm tiktoken before probing so load cost is not charged to the
        // sessions-dialog probe interval during process_streaming_chunks.
        app.chat_state
            .chat
            .prepare_streaming_token_counter("test-model");

        // Reset probe after any setup cost (e.g. tiktoken warm-up) so the
        // assertion only covers process_streaming_chunks itself.
        app.sessions_dialog_live_dirty = false;
        app.last_sessions_dialog_metadata_probe = std::time::Instant::now();
        let probe_before = app.last_sessions_dialog_metadata_probe;

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(crate::llm::ChunkMessage::Text("hello".to_string()))
            .unwrap();
        sender
            .send(crate::llm::ChunkMessage::Text(" world".to_string()))
            .unwrap();

        app.session_view_states.get_mut(&session_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                0,
            ));
        app.is_streaming = true;

        app.process_streaming_chunks();

        assert!(!app.sessions_dialog_live_dirty);
        assert_eq!(app.last_sessions_dialog_metadata_probe, probe_before);

        sender.send(crate::llm::ChunkMessage::End).unwrap();
        app.process_streaming_chunks();

        assert!(!app.sessions_dialog_live_dirty);
        let signature = app
            .sessions_dialog_state
            .last_list_signature
            .as_ref()
            .expect("sessions list refreshed after stream end");
        let row = signature
            .rows
            .iter()
            .find(|row| row.id == session_id)
            .expect("session row present");
        assert!(!row.is_streaming);
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
        state.stream = Some(SessionStreamState::new(
            receiver,
            cancel_token,
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            0,
        ));
        state
            .tool_calls
            .tool_call_message_indices
            .insert("call_1".to_string(), 0);
        state.tool_calls.tool_call_order.push("call_1".to_string());
        state
            .queued_items
            .push_back(QueuedItem::Message(QueuedUserMessage {
                text: "then about pikachu".to_string(),
                image_paths: Vec::new(),
            }));
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
    async fn single_esc_does_not_cancel_streaming() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Single esc".to_string()));
        app.base_focus = BaseFocus::Chat;
        let boundary = app.chat_state.chat.messages.len();
        app.chat_state
            .chat
            .add_assistant_message("partial response");
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed_cancel_token = cancel_token.clone();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState::new(
            receiver,
            cancel_token,
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            boundary,
        ));
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!observed_cancel_token.is_cancelled());
        assert!(app.esc_is_primed());
        assert!(app.is_streaming);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn double_esc_cancels_streaming() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Double esc".to_string()));
        app.base_focus = BaseFocus::Chat;
        let boundary = app.chat_state.chat.messages.len();
        app.chat_state
            .chat
            .add_assistant_message("partial response");
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed_cancel_token = cancel_token.clone();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState::new(
            receiver,
            cancel_token,
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            boundary,
        ));
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(observed_cancel_token.is_cancelled());
        assert!(!app.esc_is_primed());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn esc_arm_expires_before_second_press() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Esc arm timeout".to_string()));
        app.base_focus = BaseFocus::Chat;
        let boundary = app.chat_state.chat.messages.len();
        app.chat_state
            .chat
            .add_assistant_message("partial response");
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed_cancel_token = cancel_token.clone();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState::new(
            receiver,
            cancel_token,
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            boundary,
        ));
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.esc_is_primed());
        // Simulate OpenCode's 5s arm window expiring.
        app.esc_primed_at = Some(
            std::time::Instant::now() - App::ESC_ARM_TIMEOUT - std::time::Duration::from_millis(1),
        );

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!observed_cancel_token.is_cancelled());
        assert!(app.esc_is_primed());
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
        state.stream = Some(SessionStreamState::new(
            receiver,
            cancel_token,
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            boundary,
        ));
        state
            .queued_items
            .push_back(QueuedItem::Message(QueuedUserMessage {
                text: "Then about riolu".to_string(),
                image_paths: Vec::new(),
            }));
        app.is_streaming = true;

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!observed_cancel_token.is_cancelled());
        assert!(app.esc_is_primed());

        app.handle_keys(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(observed_cancel_token.is_cancelled());
        assert!(!app.esc_is_primed());
        assert!(app
            .session_view_states
            .get(&session_id)
            .unwrap()
            .queued_items
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

    #[tokio::test(flavor = "multi_thread")]
    async fn queued_messages_submit_as_single_user_record_with_line_breaks() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Queue batch".to_string()));
        app.base_focus = BaseFocus::Chat;
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        for text in ["nice", "nice", "nice"] {
            state
                .queued_items
                .push_back(QueuedItem::Message(QueuedUserMessage {
                    text: text.to_string(),
                    image_paths: Vec::new(),
                }));
        }

        assert!(app.submit_queued_messages_for_session(&session_id));

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(state.queued_items.is_empty());
        assert!(state.stream.is_some());
        assert_eq!(
            app.chat_state
                .chat
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["nice\nnice\nnice", ""]
        );

        let persisted_messages = &app
            .session_manager
            .get_session_ref(&session_id)
            .unwrap()
            .messages;
        assert_eq!(persisted_messages.len(), 2);
        assert_eq!(persisted_messages[0].content, "nice\nnice\nnice");
        assert_eq!(
            persisted_messages[1].role,
            crate::session::types::MessageRole::Assistant
        );
        assert!(!persisted_messages[1].is_complete);
    }

    #[test]
    fn interruption_guidance_augments_the_request_system_prompt() {
        let mut messages = vec![
            crate::session::types::Message::system("base prompt"),
            crate::session::types::Message::user("new request"),
        ];

        App::apply_turn_guidance(
            &mut messages,
            Some(App::INTERRUPTED_TURN_CONTINUATION_GUIDANCE),
        );

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, crate::session::types::MessageRole::System);
        assert!(messages[0].content.starts_with("base prompt\n\n"));
        assert!(messages[0].content.contains("resume unfinished work"));
    }

    #[test]
    fn no_interruption_guidance_leaves_messages_unchanged() {
        let mut messages = vec![crate::session::types::Message::system("base prompt")];
        let expected = messages.clone();

        App::apply_turn_guidance(&mut messages, None);

        assert_eq!(messages, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn queued_image_messages_submit_as_single_record_with_renumbered_placeholders() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Queue images".to_string()));
        app.base_focus = BaseFocus::Chat;
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state
            .queued_items
            .push_back(QueuedItem::Message(QueuedUserMessage {
                text: "first [Image #1]".to_string(),
                image_paths: vec![std::path::PathBuf::from("/tmp/first.png")],
            }));
        state
            .queued_items
            .push_back(QueuedItem::Message(QueuedUserMessage {
                text: "second [Image #1]".to_string(),
                image_paths: vec![std::path::PathBuf::from("/tmp/second.png")],
            }));

        assert!(app.submit_queued_messages_for_session(&session_id));

        let user_message = app
            .chat_state
            .chat
            .messages
            .iter()
            .find(|message| message.role == crate::session::types::MessageRole::User)
            .unwrap();
        assert_eq!(user_message.content, "first [Image #1]\nsecond [Image #2]");
        assert_eq!(
            user_message.local_image_paths,
            vec!["/tmp/first.png".to_string(), "/tmp/second.png".to_string()]
        );
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
        app.session_view_states.get_mut(&session_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                1,
            ));
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
        app.session_view_states.get_mut(&session_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("test-model".to_string()),
                Some("test-provider".to_string()),
                1,
            ));
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
    fn streamed_tool_call_and_result_persist_as_single_assistant_message() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Logical assistant".to_string()));

        let user_message = crate::session::types::Message::user("Prompt");
        app.chat_state.chat.add_message(user_message.clone());
        app.session_manager
            .add_message_to_current_session(&user_message)
            .unwrap();

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("Checking."));
        app.chat_state.chat.begin_streaming_turn();

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let state = app.session_view_states.get_mut(&session_id).unwrap();
        state.stream = Some(SessionStreamState::new(
            receiver,
            tokio_util::sync::CancellationToken::new(),
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            1,
        ));
        app.is_streaming = true;

        app.add_tool_calls_to_session(
            &session_id,
            vec![crate::llm::ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: crate::llm::FunctionCall {
                    name: "read".to_string(),
                    arguments: serde_json::json!({ "path": "Cargo.toml" }).to_string(),
                },
            }],
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
        app.finish_streaming_session(&session_id);

        assert_eq!(app.chat_state.chat.messages.len(), 2);
        let session = app.session_manager.get_session_ref(&session_id).unwrap();
        assert_eq!(session.messages.len(), 2);
        let assistant = &session.messages[1];
        assert_eq!(
            assistant.role,
            crate::session::types::MessageRole::Assistant
        );
        assert_eq!(
            assistant
                .parts
                .iter()
                .map(|part| part.part_type.as_str())
                .collect::<Vec<_>>(),
            vec!["text", "tool_call", "tool_result"]
        );
        assert_eq!(assistant.content, "Checking.");
        assert!(assistant.tool_call_part_data("call_1").is_some());
        assert_eq!(
            assistant
                .tool_result_part_data("call_1")
                .and_then(|payload| payload.get("output_preview"))
                .and_then(|value| value.as_str()),
            Some("contents")
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
        state.stream = Some(SessionStreamState::new(
            receiver,
            tokio_util::sync::CancellationToken::new(),
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            1,
        ));
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
        state.stream = Some(SessionStreamState::new(
            receiver,
            tokio_util::sync::CancellationToken::new(),
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            1,
        ));
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
            "stream disconnected before completion: stream task ended before sending a completion event"
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
        state.stream = Some(SessionStreamState::new(
            receiver,
            tokio_util::sync::CancellationToken::new(),
            Some("test-model".to_string()),
            Some("test-provider".to_string()),
            1,
        ));
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
        assert!(app.reject_chat_only_command_outside_chat("branch"));

        app.base_focus = BaseFocus::Chat;
        assert!(!app.reject_chat_only_command_outside_chat("compact"));
        assert!(!app.reject_chat_only_command_outside_chat("branch"));
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
            cancel_token: tokio_util::sync::CancellationToken::new(),
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

    #[tokio::test(flavor = "multi_thread")]
    async fn queued_messages_submit_after_compaction_result() {
        let mut app = test_app();
        let session_id = app.create_new_session(Some("Compact queue submit".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.session_view_states
            .get_mut(&session_id)
            .unwrap()
            .queued_items
            .push_back(QueuedItem::Message(QueuedUserMessage {
                text: "Then about jolteon".to_string(),
                image_paths: Vec::new(),
            }));

        let stats = crate::session::types::CompactionStats {
            before_tokens: 1_000,
            after_tokens: 120,
            before_messages: 5,
            after_messages: 1,
        };
        let compacted_messages = vec![crate::session::types::Message::assistant("summary")];
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        sender
            .send(CompactionTaskMessage::Success {
                session_id: session_id.clone(),
                messages: compacted_messages,
                stats,
            })
            .unwrap();
        drop(sender);
        app.compaction_receiver = Some(receiver);
        app.compaction_pending = Some(CompactionPending {
            session_id: session_id.clone(),
            before_tokens: stats.before_tokens,
            cancel_token: tokio_util::sync::CancellationToken::new(),
        });
        app.is_streaming = true;

        app.process_compaction_events();

        let state = app.session_view_states.get(&session_id).unwrap();
        assert!(state.queued_items.is_empty());
        assert!(state.stream.is_some());
        assert!(app.is_streaming);
        assert_eq!(
            app.chat_state
                .chat
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["summary", "Then about jolteon", ""]
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

        assert_eq!(
            app.session_usage_text(),
            "360 \u{00b7} last compact saved 97%"
        );
    }

    #[test]
    fn streaming_usage_base_caches_completed_messages_and_tracks_appends() {
        let mut app = test_app();
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::user("hello there"));
        let mut done = crate::session::types::Message::assistant("finished answer");
        done.token_count = Some(100);
        app.chat_state.chat.add_message(done);
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("streaming..."));

        let fresh = |app: &App| -> usize {
            let messages = &app.chat_state.chat.messages;
            let streaming_idx = messages.iter().rposition(|message| {
                message.role == crate::session::types::MessageRole::Assistant
                    && !message.is_complete
            });
            messages
                .iter()
                .enumerate()
                .map(|(idx, message)| {
                    if Some(idx) == streaming_idx {
                        app.chat_state.chat.streaming_token_count()
                    } else {
                        crate::session::compaction::message_context_tokens(message)
                    }
                })
                .sum()
        };

        let expected = fresh(&app);
        assert_eq!(app.streaming_context_tokens_cached(), expected);
        // Cached path must agree with a fresh walk on repeat calls.
        assert_eq!(app.streaming_context_tokens_cached(), expected);
        let cached_base = app
            .cached_usage_streaming_base
            .clone()
            .expect("base cached");

        // Appending a message invalidates the cached base.
        let mut extra = crate::session::types::Message::assistant("more context");
        extra.token_count = Some(40);
        let last_idx = app.chat_state.chat.messages.len() - 1;
        app.chat_state.chat.messages.insert(last_idx, extra);
        let expected_after = fresh(&app);
        assert_eq!(app.streaming_context_tokens_cached(), expected_after);
        assert_ne!(
            app.cached_usage_streaming_base,
            Some(cached_base),
            "cache should refresh when the message count changes"
        );
        assert!(expected_after > expected);
    }

    #[test]
    fn switching_sessions_keeps_family_render_caches_and_releases_others() {
        let mut app = test_app();
        let colors = app.get_current_theme_colors();

        let other = app.create_new_session(Some("Other".to_string()));
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::user("other session"));
        app.chat_state
            .chat
            .ensure_render_cache(80, "model", &colors);
        assert!(app.chat_state.chat.has_render_cache());

        let root = app.create_new_session(Some("Root".to_string()));
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::user("root session"));
        let child_a = app.session_manager.create_child_session(
            root.clone(),
            "child-a".to_string(),
            "Subagent A".to_string(),
        );
        let child_b = app.session_manager.create_child_session(
            root.clone(),
            "child-b".to_string(),
            "Subagent B".to_string(),
        );
        app.session_manager.switch_session(&root);

        // Warm the render cache of child A, then switch to child B.
        assert!(app.switch_to_session(&child_a));
        app.chat_state
            .chat
            .add_message(crate::session::types::Message::user("child a transcript"));
        app.chat_state
            .chat
            .ensure_render_cache(80, "model", &colors);
        assert!(app.switch_to_session(&child_b));

        // Child A shares the current root: its caches must stay warm.
        assert!(app
            .session_view_states
            .get(&child_a)
            .is_some_and(|state| state.chat.has_render_cache()));
        // The unrelated session's caches must be released.
        assert!(app
            .session_view_states
            .get(&other)
            .is_some_and(|state| !state.chat.has_render_cache()));
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
    fn auto_session_title_matches_first_prompt_heuristic() {
        assert!(is_auto_session_title_for_prompt(
            "You are so cool",
            "You are so cool"
        ));
        assert!(is_auto_session_title_for_prompt(
            "You the coolest!",
            "You the coolest!"
        ));
        assert!(is_auto_session_title_for_prompt("session-12", "anything"));
        assert!(!is_auto_session_title_for_prompt(
            "Manual title",
            "You are so cool"
        ));
    }

    #[test]
    fn generated_title_replaces_auto_title_but_not_manual_title() {
        let mut app = test_app();
        let prompt = "You are so cool";
        let auto_id = app.create_new_session(Some(App::generate_title_from_message(prompt)));
        app.chat_state.chat.add_user_message(prompt);
        app.apply_generated_session_title(&auto_id, "Friendly greeting".to_string());
        assert_eq!(
            app.session_manager
                .get_session_ref(&auto_id)
                .map(|session| session.title.as_str()),
            Some("Friendly greeting")
        );

        let manual_id = app.create_new_session(Some("Manual title".to_string()));
        app.chat_state.chat.add_user_message(prompt);
        app.apply_generated_session_title(&manual_id, "Should not apply".to_string());
        assert_eq!(
            app.session_manager
                .get_session_ref(&manual_id)
                .map(|session| session.title.as_str()),
            Some("Manual title")
        );
    }

    #[test]
    fn title_generation_only_applies_to_first_user_message() {
        let mut app = test_app();
        let first_prompt = "First prompt";
        let session_id =
            app.create_new_session(Some(App::generate_title_from_message(first_prompt)));
        app.chat_state.chat.add_user_message(first_prompt);
        assert!(app.session_prompt_is_first_user_message(&session_id, first_prompt));

        app.chat_state.chat.add_assistant_message("response");
        app.chat_state.chat.add_user_message("Second prompt");
        assert!(!app.session_prompt_is_first_user_message(&session_id, "Second prompt"));

        app.apply_generated_session_title(&session_id, "Late title".to_string());
        assert_eq!(
            app.session_manager
                .get_session_ref(&session_id)
                .map(|session| session.title.as_str()),
            Some("First prompt")
        );
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
    fn ctrl_e_moves_input_cursor_to_end_from_chat() {
        let mut app = test_app();
        app.base_focus = BaseFocus::Chat;
        app.input.insert_str("draft prompt");
        app.handle_keys(KeyEvent::new(
            KeyCode::Char('a'),
            event::KeyModifiers::CONTROL,
        ));
        assert_eq!(app.input.cursor(), (0, 0));
        assert!(app.chat_state.chat.thinking_visible());

        app.handle_keys(KeyEvent::new(
            KeyCode::Char('e'),
            event::KeyModifiers::CONTROL,
        ));

        assert!(app.chat_state.chat.thinking_visible());
        assert_eq!(app.input.cursor(), (0, "draft prompt".chars().count()));
    }

    #[test]
    fn ctrl_x_e_toggles_thinking_from_chat() {
        let mut app = test_app();
        app.base_focus = BaseFocus::Chat;
        assert!(app.chat_state.chat.thinking_visible());

        app.handle_keys(KeyEvent::new(
            KeyCode::Char('x'),
            event::KeyModifiers::CONTROL,
        ));
        app.handle_keys(KeyEvent::new(KeyCode::Char('e'), event::KeyModifiers::NONE));

        assert_eq!(app.overlay_focus, OverlayFocus::None);
        assert!(!app.chat_state.chat.thinking_visible());
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
            None
        );
        let selected = app.sessions_dialog_state.dialog.get_selected().unwrap();
        assert_eq!(selected.group, app.session_manager.current_workspace_name());
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
        assert!(app.switch_to_latest_child_session());
        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some("child-b")
        );

        assert!(app.handle_base_keys(KeyEvent::new(KeyCode::Right, event::KeyModifiers::NONE,)));
        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some("child-a")
        );

        assert!(app.handle_base_keys(KeyEvent::new(KeyCode::Left, event::KeyModifiers::NONE,)));
        assert_eq!(
            app.session_manager
                .get_current_session_id()
                .map(String::as_str),
            Some("child-b")
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

        assert!(app.switch_to_latest_child_session());
        app.handle_keys(KeyEvent::new(KeyCode::Char('h'), event::KeyModifiers::NONE));
        app.handle_paste(" pasted".to_string());

        assert_eq!(app.input.get_text(), "");
    }

    #[test]
    fn parent_cancellation_interrupts_active_subagent_sessions() {
        let mut app = test_app();
        let parent_id = app.create_new_session(Some("Parent".to_string()));
        app.base_focus = BaseFocus::Chat;

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("Parent partial"));
        app.chat_state.chat.begin_streaming_turn();
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states.get_mut(&parent_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("parent-model".to_string()),
                Some("parent-provider".to_string()),
                0,
            ));
        app.is_streaming = true;

        app.start_subagent_session(
            parent_id.clone(),
            "child-a".to_string(),
            "General task (@general subagent)".to_string(),
            "general".to_string(),
            Some("sub-model".to_string()),
            Some("sub-provider".to_string()),
            "General task".to_string(),
            "Check implementation".to_string(),
        );

        assert!(app.session_has_active_stream("child-a"));

        app.cancelled_streaming_session(&parent_id);

        assert!(!app.session_has_active_stream("child-a"));
        let child_session = app.session_manager.get_session_ref("child-a").unwrap();
        assert_eq!(
            child_session.status,
            crate::session::types::SessionStatus::Interrupted
        );
        assert_eq!(child_session.messages.len(), 2);
        assert!(child_session.messages[1].is_complete);
        assert!(child_session.messages[1].was_interrupted);
    }

    #[test]
    fn queue_interrupt_also_interrupts_active_subagent_sessions() {
        // Mirrors parent_cancellation, but exercises the queue-interrupt path's
        // child cleanup (without submit_queued, which needs a live model/runtime).
        let mut app = test_app();
        let parent_id = app.create_new_session(Some("Parent".to_string()));
        app.base_focus = BaseFocus::Chat;

        app.chat_state
            .chat
            .add_message(crate::session::types::Message::incomplete("Parent partial"));
        app.chat_state.chat.begin_streaming_turn();
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        app.session_view_states.get_mut(&parent_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                tokio_util::sync::CancellationToken::new(),
                Some("parent-model".to_string()),
                Some("parent-provider".to_string()),
                0,
            ));
        app.is_streaming = true;

        app.start_subagent_session(
            parent_id.clone(),
            "child-a".to_string(),
            "General task (@general subagent)".to_string(),
            "general".to_string(),
            Some("sub-model".to_string()),
            Some("sub-provider".to_string()),
            "General task".to_string(),
            "Check implementation".to_string(),
        );

        assert!(app.session_has_active_stream("child-a"));
        assert!(app.switch_to_session(&parent_id));
        assert!(app.queue_message_for_current_session("queued follow-up".to_string(), Vec::new()));
        assert!(app.has_queued_messages_for_session(&parent_id));
        assert!(app.session_has_active_stream(&parent_id));

        // Same first step as interrupt_streaming_to_send_queued_for_session.
        app.interrupt_child_streams_for_parent(
            &parent_id,
            "Stopped because parent agent was interrupted",
        );
        app.cancel_streaming_for_session(&parent_id);
        app.mark_streamed_assistant_interrupted(&parent_id);
        let _ = app.finalize_and_persist_streamed_messages(
            &parent_id,
            Some("Streaming interrupted to send queued messages"),
        );
        let _ = app.session_manager.set_session_status(
            &parent_id,
            crate::session::types::SessionStatus::Interrupted,
            None,
        );
        app.cleanup_streaming_for_session(&parent_id);

        assert!(!app.session_has_active_stream("child-a"));
        let child_session = app.session_manager.get_session_ref("child-a").unwrap();
        assert_eq!(
            child_session.status,
            crate::session::types::SessionStatus::Interrupted
        );
        assert!(child_session.messages[1].was_interrupted);
    }

    #[test]
    fn cancelling_from_subagent_session_cancels_parent_stream_token() {
        let mut app = test_app();
        let parent_id = app.create_new_session(Some("Parent".to_string()));
        app.base_focus = BaseFocus::Chat;

        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let observed_cancel_token = cancel_token.clone();
        app.session_view_states.get_mut(&parent_id).unwrap().stream =
            Some(SessionStreamState::new(
                receiver,
                cancel_token,
                Some("parent-model".to_string()),
                Some("parent-provider".to_string()),
                0,
            ));

        app.start_subagent_session(
            parent_id.clone(),
            "child-a".to_string(),
            "General task (@general subagent)".to_string(),
            "general".to_string(),
            Some("sub-model".to_string()),
            Some("sub-provider".to_string()),
            "General task".to_string(),
            "Check implementation".to_string(),
        );

        app.cancel_streaming_for_session("child-a");

        assert!(observed_cancel_token.is_cancelled());
    }

    #[test]
    fn subagent_session_allows_input_cursor_navigation() {
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

        assert!(app.switch_to_latest_child_session());
        app.input.insert_str("draft prompt");
        app.input
            .handle_event(KeyEvent::new(KeyCode::Left, event::KeyModifiers::NONE));
        app.handle_keys(KeyEvent::new(KeyCode::Right, event::KeyModifiers::SUPER));

        assert_eq!(app.input.get_text(), "draft prompt");
        assert_eq!(app.input.cursor(), (0, "draft prompt".chars().count()));
    }

    #[test]
    fn chat_session_allows_command_right_input_cursor_navigation() {
        let mut app = test_app();
        app.create_new_session(Some("Chat".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.input.insert_str("draft prompt");
        app.handle_keys(KeyEvent::new(KeyCode::Left, event::KeyModifiers::SUPER));
        assert_eq!(app.input.cursor(), (0, 0));

        app.handle_keys(KeyEvent::new(KeyCode::Right, event::KeyModifiers::SUPER));

        assert_eq!(app.input.get_text(), "draft prompt");
        assert_eq!(app.input.cursor(), (0, "draft prompt".chars().count()));
    }

    #[test]
    fn chat_session_allows_control_e_terminal_encoding_for_command_right() {
        let mut app = test_app();
        app.create_new_session(Some("Chat".to_string()));
        app.base_focus = BaseFocus::Chat;
        app.input.insert_str("draft prompt");
        app.handle_keys(KeyEvent::new(
            KeyCode::Char('a'),
            event::KeyModifiers::CONTROL,
        ));
        assert_eq!(app.input.cursor(), (0, 0));

        app.handle_keys(KeyEvent::new(
            KeyCode::Char('e'),
            event::KeyModifiers::CONTROL,
        ));

        assert_eq!(app.input.get_text(), "draft prompt");
        assert_eq!(app.input.cursor(), (0, "draft prompt".chars().count()));
        assert!(app.chat_state.chat.thinking_visible());
    }

    #[test]
    fn subagent_session_still_ignores_editing_shortcuts() {
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

        assert!(app.switch_to_latest_child_session());
        app.input.insert_str("draft prompt");
        app.handle_keys(KeyEvent::new(
            KeyCode::Char('u'),
            event::KeyModifiers::CONTROL,
        ));
        app.handle_keys(KeyEvent::new(
            KeyCode::Backspace,
            event::KeyModifiers::SUPER,
        ));

        assert_eq!(app.input.get_text(), "draft prompt");
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
