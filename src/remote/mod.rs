use crate::app::App;
use crate::session::manager::SessionInfo;
use crate::session::types::{Message, MessageRole};
use crate::tools::PermissionResponse;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use ratatui::{
    backend::{Backend, CrosstermBackend, TestBackend},
    crossterm::{
        event::{
            self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture,
            EnableBracketedPaste, EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEvent,
            KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent,
            MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{
            disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
            LeaveAlternateScreen,
        },
    },
    layout::Position,
    style::{Color, Modifier},
    Terminal,
};
use reqwest::StatusCode;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, IsTerminal, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as TokioMutex;

const DEFAULT_PAIR_TTL_SECS: i64 = 10 * 60;
const MAX_HTTP_HEADER_BYTES: usize = 32 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_REMOTE_PROMPT_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const EVENT_DRAIN_LIMIT: usize = 256;
const GIT_SUMMARY_CACHE_TTL: Duration = Duration::from_secs(15);
const MAX_GIT_DIFF_BYTES: usize = 384 * 1024;
const MAX_GIT_FILES: usize = 200;
const MAX_GIT_PATCH_LINES_PER_FILE: usize = 180;
const MAX_GIT_PATCH_LINE_CHARS: usize = 900;

fn drain_pending_terminal_events(idle_timeout: Duration) {
    for _ in 0..EVENT_DRAIN_LIMIT {
        match event::poll(idle_timeout) {
            Ok(true) => {
                if event::read().is_err() {
                    break;
                }
            }
            Ok(false) | Err(_) => break,
        }
    }
}

fn titlecase_remote_agent_name(agent: &str) -> String {
    let agent = agent.trim();
    if agent.is_empty() {
        return "Build".to_string();
    }

    let mut chars = agent.chars();
    let Some(first) = chars.next() else {
        return "Build".to_string();
    };

    format!(
        "{}{}",
        first.to_uppercase().collect::<String>(),
        chars.as_str()
    )
}
const MAX_REMOTE_PROMPT_IMAGES: usize = 8;
const MAX_REMOTE_SESSIONS_PER_WORKSPACE: usize = 24;
const HOSTS_FILE: &str = "remote-hosts.json";
const FAVICON_CANDIDATES: &[&str] = &[
    "favicon.svg",
    "favicon.ico",
    "favicon.png",
    "public/favicon.svg",
    "public/favicon.ico",
    "public/favicon.png",
    "app/favicon.ico",
    "app/favicon.png",
    "app/icon.svg",
    "app/icon.png",
    "app/icon.ico",
    "src/favicon.ico",
    "src/favicon.svg",
    "src/app/favicon.ico",
    "src/app/icon.svg",
    "src/app/icon.png",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    ".idea/icon.svg",
];
const ICON_SOURCE_FILES: &[&str] = &[
    "index.html",
    "public/index.html",
    "app/routes/__root.tsx",
    "src/routes/__root.tsx",
    "app/root.tsx",
    "src/root.tsx",
    "src/index.html",
];

mod remote_assets {
    include!(concat!(env!("OUT_DIR"), "/remote_assets.rs"));
}

#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub bind: String,
    pub model_override: Option<String>,
    pub pair_code: Option<String>,
}

#[derive(Debug)]
struct HostState {
    pair_code: Option<String>,
    trusted_token: String,
    pair_expires_at: Option<i64>,
    browser_url: String,
    suggested_alias: String,
    git_summary_cache: Mutex<Option<GitSummaryCacheEntry>>,
}

impl HostState {
    fn new(
        browser_url: String,
        suggested_alias: String,
        pair_code_arg: Option<String>,
    ) -> Result<Self> {
        let pair_code = resolve_pair_code_arg(pair_code_arg)?;
        let pair_expires_at = pair_code
            .as_ref()
            .map(|_| now_unix_secs() + DEFAULT_PAIR_TTL_SECS);

        Ok(Self {
            pair_code,
            trusted_token: cuid2::create_id(),
            pair_expires_at,
            browser_url,
            suggested_alias,
            git_summary_cache: Mutex::new(None),
        })
    }

    fn auth_required(&self) -> bool {
        self.pair_code.is_some()
    }

    fn pair_code_is_active(&self) -> bool {
        self.pair_expires_at
            .is_some_and(|expires_at| now_unix_secs() <= expires_at)
    }

    fn accepts_pair_code(&self, code: &str) -> bool {
        let Some(pair_code) = self.pair_code.as_deref() else {
            return true;
        };
        self.pair_code_is_active() && pair_codes_match(pair_code, code)
    }

    fn accepts_token(&self, token: &str) -> bool {
        if !self.auth_required() {
            return true;
        }
        !token.trim().is_empty() && token.trim() == self.trusted_token
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteStatus {
    version: String,
    workspace: String,
    cwd: String,
    provider: String,
    model: String,
    agent: String,
    primary_agents: Vec<String>,
    reasoning_effort: Option<String>,
    reasoning_efforts: Vec<String>,
    browser_url: String,
    suggested_alias: String,
    auth_required: bool,
    pair_expires_at: i64,
    theme: RemoteTheme,
    git_summary: RemoteGitSummary,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteTheme {
    primary: String,
    primary_dim: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct RemoteGitSummary {
    is_repo: bool,
    branch: Option<String>,
}

#[derive(Debug, Clone)]
struct GitSummaryCacheEntry {
    cwd: String,
    checked_at: Instant,
    summary: RemoteGitSummary,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteGitStatus {
    is_repo: bool,
    branch: Option<String>,
    changed_files: usize,
    additions: usize,
    deletions: usize,
    files: Vec<RemoteGitFileChange>,
    diff_files: Vec<RemoteGitDiffFile>,
    truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteGitFileChange {
    path: String,
    old_path: Option<String>,
    status: String,
    additions: usize,
    deletions: usize,
    binary: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteGitDiffFile {
    path: String,
    old_path: Option<String>,
    status: String,
    additions: usize,
    deletions: usize,
    binary: bool,
    lines: Vec<RemoteGitDiffLine>,
    truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteGitDiffLine {
    kind: String,
    text: String,
    old_line: Option<usize>,
    new_line: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteSession {
    id: String,
    parent_id: Option<String>,
    title: String,
    workspace: String,
    workspace_path: String,
    status: String,
    message_count: usize,
    updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteWorkspace {
    name: String,
    path: String,
    sort_order: i64,
    last_opened_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteMessage {
    role: String,
    content: String,
    reasoning: Option<String>,
    is_complete: bool,
    agent_mode: Option<String>,
    token_count: Option<usize>,
    duration_ms: Option<u64>,
    t0_ms: Option<u64>,
    t1_ms: Option<u64>,
    tn_ms: Option<u64>,
    output_tokens: Option<usize>,
    model: Option<String>,
    provider: Option<String>,
    local_image_paths: Vec<String>,
    was_interrupted: bool,
    parts: Vec<crate::session::types::MessagePart>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemotePermissionPrompt {
    tool_id: String,
    action: String,
    permission: String,
    patterns: Vec<String>,
    target: Option<String>,
    command: Option<String>,
    workdir: Option<String>,
    reason: String,
    queued_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteQuestionPrompt {
    questions: Vec<RemoteQuestionItem>,
    queued_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteQuestionItem {
    header: String,
    question: String,
    options: Vec<RemoteQuestionOption>,
    multiple: bool,
    custom: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteQuestionOption {
    label: String,
    description: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteThreadTab {
    session_id: String,
    label: String,
    agent: String,
    model: String,
    active: bool,
    running: bool,
    kind: String,
    accent: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteThreadTabs {
    root_session_id: String,
    is_child_session: bool,
    tabs: Vec<RemoteThreadTab>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteState {
    status: RemoteStatus,
    projects: Vec<RemoteWorkspace>,
    sessions: Vec<RemoteSession>,
    current_session_id: Option<String>,
    messages: Vec<RemoteMessage>,
    is_streaming: bool,
    queued_messages: Vec<String>,
    pending_permission: Option<RemotePermissionPrompt>,
    pending_question: Option<RemoteQuestionPrompt>,
    thread_tabs: Option<RemoteThreadTabs>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteModelOption {
    id: String,
    name: String,
    group: String,
    description: String,
    provider_id: String,
    active: bool,
    favorite: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteSuggestion {
    name: String,
    description: String,
    replacement: String,
    kind: String,
    is_directory: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteSkill {
    name: String,
    description: String,
    location: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteMcpServer {
    name: String,
    enabled: bool,
    status: String,
    kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct McpToggleRequest {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SwitchSessionRequest {
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchiveSessionRequest {
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchiveWorkspaceRequest {
    path: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct NewSessionRequest {
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SelectWorkspaceRequest {
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SetAgentRequest {
    agent: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SetReasoningEffortRequest {
    effort: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PermissionAnswerRequest {
    response: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct QuestionAnswerRequest {
    answers: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct SelectModelRequest {
    provider_id: String,
    model_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RemoteTerminalFrameRequest {
    width: u16,
    height: u16,
}

#[derive(Debug, Serialize, Deserialize)]
struct RemoteTerminalFrameResponse {
    width: u16,
    height: u16,
    running: bool,
    cursor: Option<RemoteCursor>,
    cells: Vec<RemoteCell>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RemoteTerminalEventResponse {
    running: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
struct RemoteCursor {
    x: u16,
    y: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteCell {
    symbol: String,
    fg: RemoteColor,
    bg: RemoteColor,
    modifier: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum RemoteColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Indexed(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RemoteTerminalEvent {
    Key {
        code: RemoteKeyCode,
        modifiers: u8,
        kind: RemoteKeyKind,
    },
    Mouse {
        kind: RemoteMouseKind,
        column: u16,
        row: u16,
        modifiers: u8,
    },
    Paste {
        text: String,
    },
    Focus {
        focused: bool,
    },
    Resize {
        width: u16,
        height: u16,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum RemoteKeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum RemoteKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    F(u8),
    Char(char),
    Null,
    Esc,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    KeypadBegin,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum RemoteMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
enum RemoteMouseKind {
    Down(RemoteMouseButton),
    Up(RemoteMouseButton),
    Drag(RemoteMouseButton),
    Moved,
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairRequest {
    code: String,
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairResponse {
    token: String,
    suggested_alias: String,
    workspace_label: String,
    browser_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptRequest {
    prompt: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    images: Vec<PromptImageRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptImageRequest {
    name: String,
    media_type: String,
    data_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AutocompleteRequest {
    trigger: String,
    query: String,
    is_chat: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptResponse {
    session_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CancelResponse {
    cancelled: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct RemoteHostsFile {
    hosts: Vec<RemoteHostEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RemoteHostEntry {
    alias: String,
    url: String,
    token: String,
    workspace_label: String,
    last_used_at: i64,
}

#[derive(Debug, Clone)]
struct ConnectedHost {
    alias: String,
    url: String,
    token: String,
    status: RemoteStatus,
}

pub async fn serve(options: ServeOptions) -> Result<()> {
    let listener = TcpListener::bind(&options.bind)
        .await
        .with_context(|| format!("failed to bind {}", options.bind))?;
    let local_addr = listener
        .local_addr()
        .context("failed to read bound address")?;
    let browser_url = browser_url_for_addr(local_addr);
    let suggested_alias = suggested_alias_for_cwd();
    let host_state = Arc::new(HostState::new(
        browser_url.clone(),
        suggested_alias.clone(),
        options.pair_code.clone(),
    )?);
    let mut app_inner = App::new_with_model_override(options.model_override.as_deref(), None)?;
    app_inner.ensure_startup_hydrated()?;
    app_inner.ensure_session_history();
    let app = Arc::new(TokioMutex::new(app_inner));

    {
        let app = app.lock().await;
        print_host_ready(&app, local_addr, &host_state);
    }

    let app_tick = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tick.tick().await;
            let mut app = app_tick.lock().await;
            tick_remote_host_app(&mut app);
        }
    });

    loop {
        let (mut socket, _peer) = listener
            .accept()
            .await
            .context("failed to accept remote connection")?;
        let app = app.clone();
        let host_state = host_state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(&mut socket, app, host_state).await {
                if !is_disconnect_error(&err) {
                    let _ = write_error_response(&mut socket, 500, &err.to_string()).await;
                }
            }
        });
    }
}

pub async fn attach(target: &str) -> Result<()> {
    let client = remote_client()?;
    let host = connect_host(&client, target).await?;
    run_remote_tui(client, host).await
}

pub async fn print_attach(target: &str, prompt: &str) -> Result<()> {
    let client = remote_client()?;
    let host = connect_host(&client, target).await?;
    stream_remote_prompt(&client, &host, prompt).await
}

fn remote_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to create remote HTTP client")
}

async fn run_remote_tui(client: reqwest::Client, host: ConnectedHost) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("interactive attach requires a terminal; use crabcode -p --attach for non-interactive prompts");
    }

    let _terminal_guard = TerminalModeGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = remote_tui_event_loop(&client, &host, &mut terminal).await;
    terminal.show_cursor()?;

    result
}

struct TerminalModeGuard {
    keyboard_enhancement: bool,
}

impl TerminalModeGuard {
    fn enter() -> Result<Self> {
        let mut stdout = io::stdout();
        let keyboard_enhancement = supports_keyboard_enhancement()?;
        enable_raw_mode()?;
        let enter_result = if keyboard_enhancement {
            execute!(
                stdout,
                EnterAlternateScreen,
                EnableMouseCapture,
                EnableFocusChange,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES,
                ),
                EnableBracketedPaste
            )
        } else {
            execute!(
                stdout,
                EnterAlternateScreen,
                EnableMouseCapture,
                EnableFocusChange,
                EnableBracketedPaste
            )
        };

        if let Err(err) = enter_result {
            let _ = disable_raw_mode();
            return Err(err).context("failed to enter terminal alternate screen");
        }

        Ok(Self {
            keyboard_enhancement,
        })
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        drain_pending_terminal_events(Duration::from_millis(0));

        let mut stdout = io::stdout();
        if self.keyboard_enhancement {
            let _ = execute!(
                stdout,
                DisableMouseCapture,
                DisableFocusChange,
                PopKeyboardEnhancementFlags,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
        } else {
            let _ = execute!(
                stdout,
                DisableMouseCapture,
                DisableFocusChange,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
        }
        let _ = stdout.flush();

        drain_pending_terminal_events(Duration::from_millis(25));
        let _ = disable_raw_mode();
    }
}

async fn remote_tui_event_loop(
    client: &reqwest::Client,
    host: &ConnectedHost,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    loop {
        let area = terminal.size()?;
        let snapshot = fetch_terminal_frame(client, host, area.width, area.height).await?;
        terminal.draw(|frame| render_remote_terminal_frame(frame, &snapshot))?;

        if event::poll(Duration::from_millis(50))? {
            loop {
                let local_event = event::read()?;
                if let Some(remote_event) = remote_terminal_event_from_crossterm(local_event) {
                    if remote_event.detaches_client() {
                        return Ok(());
                    }

                    let response = send_terminal_event(client, host, remote_event).await?;
                    if !response.running {
                        return Ok(());
                    }
                }

                if !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
        }

        if !snapshot.running {
            break;
        }
    }

    Ok(())
}

async fn fetch_terminal_frame(
    client: &reqwest::Client,
    host: &ConnectedHost,
    width: u16,
    height: u16,
) -> Result<RemoteTerminalFrameResponse> {
    post_json(
        client,
        &host.url,
        "/api/terminal/frame",
        &host.token,
        &RemoteTerminalFrameRequest { width, height },
    )
    .await
}

async fn send_terminal_event(
    client: &reqwest::Client,
    host: &ConnectedHost,
    event: RemoteTerminalEvent,
) -> Result<RemoteTerminalEventResponse> {
    post_json(
        client,
        &host.url,
        "/api/terminal/event",
        &host.token,
        &event,
    )
    .await
}

fn render_remote_terminal_frame(
    frame: &mut ratatui::Frame,
    snapshot: &RemoteTerminalFrameResponse,
) {
    let area = frame.area();
    let target = frame.buffer_mut();

    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &mut target[(area.x + x, area.y + y)];
            cell.reset();

            if x >= snapshot.width || y >= snapshot.height {
                continue;
            }

            let index = y as usize * snapshot.width as usize + x as usize;
            let Some(source) = snapshot.cells.get(index) else {
                continue;
            };

            cell.set_symbol(&source.symbol);
            cell.set_fg(source.fg.into_color());
            cell.set_bg(source.bg.into_color());
            cell.modifier = Modifier::from_bits_truncate(source.modifier);
        }
    }

    if let Some(cursor) = snapshot.cursor {
        if cursor.x < area.width && cursor.y < area.height {
            frame.set_cursor_position(Position::new(area.x + cursor.x, area.y + cursor.y));
        }
    }
}

impl RemoteColor {
    fn from_color(color: Color) -> Self {
        match color {
            Color::Reset => Self::Reset,
            Color::Black => Self::Black,
            Color::Red => Self::Red,
            Color::Green => Self::Green,
            Color::Yellow => Self::Yellow,
            Color::Blue => Self::Blue,
            Color::Magenta => Self::Magenta,
            Color::Cyan => Self::Cyan,
            Color::Gray => Self::Gray,
            Color::DarkGray => Self::DarkGray,
            Color::LightRed => Self::LightRed,
            Color::LightGreen => Self::LightGreen,
            Color::LightYellow => Self::LightYellow,
            Color::LightBlue => Self::LightBlue,
            Color::LightMagenta => Self::LightMagenta,
            Color::LightCyan => Self::LightCyan,
            Color::White => Self::White,
            Color::Indexed(value) => Self::Indexed(value),
            Color::Rgb(r, g, b) => Self::Rgb { r, g, b },
        }
    }

    fn into_color(self) -> Color {
        match self {
            Self::Reset => Color::Reset,
            Self::Black => Color::Black,
            Self::Red => Color::Red,
            Self::Green => Color::Green,
            Self::Yellow => Color::Yellow,
            Self::Blue => Color::Blue,
            Self::Magenta => Color::Magenta,
            Self::Cyan => Color::Cyan,
            Self::Gray => Color::Gray,
            Self::DarkGray => Color::DarkGray,
            Self::LightRed => Color::LightRed,
            Self::LightGreen => Color::LightGreen,
            Self::LightYellow => Color::LightYellow,
            Self::LightBlue => Color::LightBlue,
            Self::LightMagenta => Color::LightMagenta,
            Self::LightCyan => Color::LightCyan,
            Self::White => Color::White,
            Self::Indexed(value) => Color::Indexed(value),
            Self::Rgb { r, g, b } => Color::Rgb(r, g, b),
        }
    }
}

fn remote_terminal_event_from_crossterm(event: Event) -> Option<RemoteTerminalEvent> {
    match event {
        Event::Key(key) => Some(RemoteTerminalEvent::Key {
            code: RemoteKeyCode::from_key_code(key.code)?,
            modifiers: key.modifiers.bits(),
            kind: RemoteKeyKind::from_key_kind(key.kind),
        }),
        Event::Mouse(mouse) => Some(RemoteTerminalEvent::Mouse {
            kind: RemoteMouseKind::from_mouse_kind(mouse.kind)?,
            column: mouse.column,
            row: mouse.row,
            modifiers: mouse.modifiers.bits(),
        }),
        Event::Paste(text) => Some(RemoteTerminalEvent::Paste { text }),
        Event::FocusGained => Some(RemoteTerminalEvent::Focus { focused: true }),
        Event::FocusLost => Some(RemoteTerminalEvent::Focus { focused: false }),
        Event::Resize(width, height) => Some(RemoteTerminalEvent::Resize { width, height }),
    }
}

impl RemoteTerminalEvent {
    fn detaches_client(&self) -> bool {
        matches!(
            self,
            Self::Key {
                code: RemoteKeyCode::Char('c'),
                modifiers,
                kind: RemoteKeyKind::Press | RemoteKeyKind::Repeat,
            } if KeyModifiers::from_bits_truncate(*modifiers).contains(KeyModifiers::CONTROL)
        )
    }

    fn apply_to_app(self, app: &mut App) {
        match self {
            Self::Key {
                code,
                modifiers,
                kind,
            } => {
                let key = KeyEvent::new_with_kind(
                    code.into_key_code(),
                    KeyModifiers::from_bits_truncate(modifiers),
                    kind.into_key_kind(),
                );
                app.handle_keys(key);
            }
            Self::Mouse {
                kind,
                column,
                row,
                modifiers,
            } => {
                let mouse = MouseEvent {
                    kind: kind.into_mouse_kind(),
                    column,
                    row,
                    modifiers: KeyModifiers::from_bits_truncate(modifiers),
                };
                app.handle_mouse_event(mouse);
            }
            Self::Paste { text } => app.handle_paste(text),
            Self::Focus { .. } => {
                // Focus is client-local; one blurred attach client should not change shared host state.
            }
            Self::Resize { .. } => {}
        }
    }
}

impl RemoteKeyKind {
    fn from_key_kind(kind: KeyEventKind) -> Self {
        match kind {
            KeyEventKind::Press => Self::Press,
            KeyEventKind::Repeat => Self::Repeat,
            KeyEventKind::Release => Self::Release,
        }
    }

    fn into_key_kind(self) -> KeyEventKind {
        match self {
            Self::Press => KeyEventKind::Press,
            Self::Repeat => KeyEventKind::Repeat,
            Self::Release => KeyEventKind::Release,
        }
    }
}

impl RemoteKeyCode {
    fn from_key_code(code: KeyCode) -> Option<Self> {
        Some(match code {
            KeyCode::Backspace => Self::Backspace,
            KeyCode::Enter => Self::Enter,
            KeyCode::Left => Self::Left,
            KeyCode::Right => Self::Right,
            KeyCode::Up => Self::Up,
            KeyCode::Down => Self::Down,
            KeyCode::Home => Self::Home,
            KeyCode::End => Self::End,
            KeyCode::PageUp => Self::PageUp,
            KeyCode::PageDown => Self::PageDown,
            KeyCode::Tab => Self::Tab,
            KeyCode::BackTab => Self::BackTab,
            KeyCode::Delete => Self::Delete,
            KeyCode::Insert => Self::Insert,
            KeyCode::F(value) => Self::F(value),
            KeyCode::Char(value) => Self::Char(value),
            KeyCode::Null => Self::Null,
            KeyCode::Esc => Self::Esc,
            KeyCode::CapsLock => Self::CapsLock,
            KeyCode::ScrollLock => Self::ScrollLock,
            KeyCode::NumLock => Self::NumLock,
            KeyCode::PrintScreen => Self::PrintScreen,
            KeyCode::Pause => Self::Pause,
            KeyCode::Menu => Self::Menu,
            KeyCode::KeypadBegin => Self::KeypadBegin,
            KeyCode::Media(_) | KeyCode::Modifier(_) => return None,
        })
    }

    fn into_key_code(self) -> KeyCode {
        match self {
            Self::Backspace => KeyCode::Backspace,
            Self::Enter => KeyCode::Enter,
            Self::Left => KeyCode::Left,
            Self::Right => KeyCode::Right,
            Self::Up => KeyCode::Up,
            Self::Down => KeyCode::Down,
            Self::Home => KeyCode::Home,
            Self::End => KeyCode::End,
            Self::PageUp => KeyCode::PageUp,
            Self::PageDown => KeyCode::PageDown,
            Self::Tab => KeyCode::Tab,
            Self::BackTab => KeyCode::BackTab,
            Self::Delete => KeyCode::Delete,
            Self::Insert => KeyCode::Insert,
            Self::F(value) => KeyCode::F(value),
            Self::Char(value) => KeyCode::Char(value),
            Self::Null => KeyCode::Null,
            Self::Esc => KeyCode::Esc,
            Self::CapsLock => KeyCode::CapsLock,
            Self::ScrollLock => KeyCode::ScrollLock,
            Self::NumLock => KeyCode::NumLock,
            Self::PrintScreen => KeyCode::PrintScreen,
            Self::Pause => KeyCode::Pause,
            Self::Menu => KeyCode::Menu,
            Self::KeypadBegin => KeyCode::KeypadBegin,
        }
    }
}

impl RemoteMouseButton {
    fn from_mouse_button(button: MouseButton) -> Self {
        match button {
            MouseButton::Left => Self::Left,
            MouseButton::Right => Self::Right,
            MouseButton::Middle => Self::Middle,
        }
    }

    fn into_mouse_button(self) -> MouseButton {
        match self {
            Self::Left => MouseButton::Left,
            Self::Right => MouseButton::Right,
            Self::Middle => MouseButton::Middle,
        }
    }
}

impl RemoteMouseKind {
    fn from_mouse_kind(kind: MouseEventKind) -> Option<Self> {
        Some(match kind {
            MouseEventKind::Down(button) => {
                Self::Down(RemoteMouseButton::from_mouse_button(button))
            }
            MouseEventKind::Up(button) => Self::Up(RemoteMouseButton::from_mouse_button(button)),
            MouseEventKind::Drag(button) => {
                Self::Drag(RemoteMouseButton::from_mouse_button(button))
            }
            MouseEventKind::Moved => Self::Moved,
            MouseEventKind::ScrollDown => Self::ScrollDown,
            MouseEventKind::ScrollUp => Self::ScrollUp,
            MouseEventKind::ScrollLeft => Self::ScrollLeft,
            MouseEventKind::ScrollRight => Self::ScrollRight,
        })
    }

    fn into_mouse_kind(self) -> MouseEventKind {
        match self {
            Self::Down(button) => MouseEventKind::Down(button.into_mouse_button()),
            Self::Up(button) => MouseEventKind::Up(button.into_mouse_button()),
            Self::Drag(button) => MouseEventKind::Drag(button.into_mouse_button()),
            Self::Moved => MouseEventKind::Moved,
            Self::ScrollDown => MouseEventKind::ScrollDown,
            Self::ScrollUp => MouseEventKind::ScrollUp,
            Self::ScrollLeft => MouseEventKind::ScrollLeft,
            Self::ScrollRight => MouseEventKind::ScrollRight,
        }
    }
}

pub fn list_hosts() -> Result<()> {
    let hosts = load_hosts()?.hosts;
    if hosts.is_empty() {
        println!("No remembered hosts.");
        return Ok(());
    }

    for host in hosts {
        println!(
            "{}\t{}\t{}\t{}",
            host.alias,
            host.url,
            host.workspace_label,
            format_timestamp(host.last_used_at)
        );
    }

    Ok(())
}

fn tick_remote_host_app(app: &mut App) {
    keep_remote_host_app_alive(app);
    app.process_streaming_chunks();
    app.update_animations();
    crate::remove_expired_toasts();
}

fn keep_remote_host_app_alive(app: &mut App) {
    app.running = true;
}

fn render_terminal_snapshot(
    app: &mut App,
    width: u16,
    height: u16,
) -> Result<RemoteTerminalFrameResponse> {
    let width = width.max(1);
    let height = height.max(1);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;

    terminal.draw(|frame| app.render(frame))?;

    let cursor = terminal
        .backend_mut()
        .get_cursor_position()
        .ok()
        .filter(|position| position.x != 0 || position.y != 0)
        .map(|position| RemoteCursor {
            x: position.x.min(width.saturating_sub(1)),
            y: position.y.min(height.saturating_sub(1)),
        });
    let buffer = terminal.backend().buffer();
    let cells = buffer
        .content
        .iter()
        .map(|cell| RemoteCell {
            symbol: cell.symbol().to_string(),
            fg: RemoteColor::from_color(cell.fg),
            bg: RemoteColor::from_color(cell.bg),
            modifier: cell.modifier.bits(),
        })
        .collect();

    Ok(RemoteTerminalFrameResponse {
        width,
        height,
        running: app.running,
        cursor,
        cells,
    })
}

async fn handle_connection(
    socket: &mut TcpStream,
    app: Arc<TokioMutex<App>>,
    host_state: Arc<HostState>,
) -> Result<()> {
    let Some(request) = read_http_request(socket).await? else {
        return Ok(());
    };

    if request.method == "OPTIONS" {
        return write_empty_response(socket, 204).await;
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => write_remote_asset_response(socket, "/").await,
        ("GET", "/api/status") => {
            let response = {
                let app = app.lock().await;
                remote_status(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/pair") => {
            let payload: PairRequest = parse_json_body(&request)?;
            if !host_state.accepts_pair_code(&payload.code) {
                return write_error_response(socket, 401, "invalid or expired pair code").await;
            }

            let workspace_label = {
                let app = app.lock().await;
                workspace_label(&app)
            };
            let response = PairResponse {
                token: host_state.trusted_token.clone(),
                suggested_alias: host_state.suggested_alias.clone(),
                workspace_label,
                browser_url: host_state.browser_url.clone(),
            };
            write_json_response(socket, 200, &response).await
        }
        ("GET", "/api/state") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let response = {
                let mut app = app.lock().await;
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("GET", "/api/events") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            write_state_events_response(socket, app, host_state).await
        }
        ("GET", "/api/project-favicon") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let Some(cwd) = query_param(&request.query, "cwd").filter(|cwd| !cwd.trim().is_empty())
            else {
                return write_error_response(socket, 400, "cwd is required").await;
            };
            let Some(path) = resolve_project_favicon_path(&cwd) else {
                return write_error_response(socket, 404, "project favicon not found").await;
            };
            let content_type = project_favicon_content_type(&path);
            let Ok(bytes) = tokio::fs::read(&path).await else {
                return write_error_response(socket, 404, "project favicon not found").await;
            };
            write_response(socket, 200, content_type, &bytes).await
        }
        ("GET", "/api/git/status") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let cwd = {
                let app = app.lock().await;
                app.remote_workspace_path()
            };
            let response = match remote_git_status(&cwd) {
                Ok(status) => status,
                Err(err) => return write_error_response(socket, 500, &err.to_string()).await,
            };
            write_json_response(socket, 200, &response).await
        }
        ("GET", "/api/local-image") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let Some(path) =
                query_param(&request.query, "path").filter(|path| !path.trim().is_empty())
            else {
                return write_error_response(socket, 400, "path is required").await;
            };
            let path = PathBuf::from(path);
            if !crate::utils::image_attachment::is_supported_image_path(&path) {
                return write_error_response(socket, 404, "image not found").await;
            }
            let content_type = crate::utils::image_attachment::mime_type_for_path(&path);
            let Ok(bytes) = tokio::fs::read(&path).await else {
                return write_error_response(socket, 404, "image not found").await;
            };
            write_response(socket, 200, content_type, &bytes).await
        }
        ("POST", "/api/session/new") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: NewSessionRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if let Err(err) = app.remote_start_blank_session(payload.workspace_path) {
                    return write_error_response(socket, 400, &err.to_string()).await;
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/workspace/select") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: SelectWorkspaceRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if let Err(err) = app.remote_select_workspace(payload.path) {
                    return write_error_response(socket, 400, &err.to_string()).await;
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/session/switch") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: SwitchSessionRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if !app.remote_switch_session(&payload.id) {
                    None
                } else {
                    tick_remote_host_app(&mut app);
                    Some(remote_state(&app, host_state.as_ref()))
                }
            };
            let Some(response) = response else {
                return write_error_response(socket, 404, "session not found").await;
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/session/archive") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: ArchiveSessionRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if let Err(err) = app.remote_archive_session(&payload.id) {
                    return write_error_response(socket, 400, &err.to_string()).await;
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/workspace/archive") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: ArchiveWorkspaceRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if let Err(err) = app.remote_archive_workspace(payload.path) {
                    return write_error_response(socket, 400, &err.to_string()).await;
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("GET", "/api/models") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let models = {
                let mut app = app.lock().await;
                remote_models(&mut app)
            };
            write_json_response(socket, 200, &models).await
        }
        ("GET", "/api/skills") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let skills = {
                let app = app.lock().await;
                remote_skills(&app)
            };
            write_json_response(socket, 200, &skills).await
        }
        ("GET", "/api/mcp") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let servers = {
                let app = app.lock().await;
                remote_mcp_list(&app)
            };
            write_json_response(socket, 200, &servers).await
        }
        ("POST", "/api/mcp/toggle") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: McpToggleRequest = parse_json_body(&request)?;
            let servers = {
                let mut app = app.lock().await;
                match app.remote_toggle_mcp_server(payload.name.trim()) {
                    Ok(list) => list,
                    Err(err) => {
                        return write_error_response(socket, 400, &err.to_string()).await;
                    }
                }
            };
            write_json_response(socket, 200, &servers).await
        }
        ("POST", "/api/autocomplete") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: AutocompleteRequest = parse_json_body(&request)?;
            let suggestions = {
                let app = app.lock().await;
                remote_suggestions(&app, &payload)
            };
            write_json_response(socket, 200, &suggestions).await
        }
        ("POST", "/api/model") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: SelectModelRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if !app.remote_set_model(payload.provider_id, payload.model_id) {
                    None
                } else {
                    tick_remote_host_app(&mut app);
                    Some(remote_status(&app, host_state.as_ref()))
                }
            };
            let Some(response) = response else {
                return write_error_response(socket, 404, "model not found").await;
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/agent/toggle") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let response = {
                let mut app = app.lock().await;
                app.remote_toggle_agent_mode();
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/agent") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: SetAgentRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                if !app.remote_set_agent_mode(payload.agent) {
                    return write_error_response(socket, 400, "unknown agent").await;
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/reasoning") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: SetReasoningEffortRequest = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                match app.remote_set_reasoning_effort(payload.effort) {
                    Ok(true) => {}
                    Ok(false) => {
                        return write_error_response(socket, 400, "reasoning effort unavailable")
                            .await;
                    }
                    Err(err) => return write_error_response(socket, 400, &err.to_string()).await,
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/prompt") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: PromptRequest = parse_json_body(&request)?;
            let image_paths = match remote_prompt_image_paths(&payload.images) {
                Ok(paths) => paths,
                Err(err) => return write_error_response(socket, 400, &err.to_string()).await,
            };
            let session_id = {
                let mut app = app.lock().await;
                app.remote_submit_input_with_images(payload.prompt, image_paths)
                    .await?
            };
            write_json_response(socket, 200, &PromptResponse { session_id }).await
        }
        ("POST", "/api/cancel") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let cancelled = {
                let mut app = app.lock().await;
                app.remote_cancel_current()
            };
            write_json_response(socket, 200, &CancelResponse { cancelled }).await
        }
        ("POST", "/api/queue/send-now") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let response = {
                let mut app = app.lock().await;
                if !app.remote_send_queued_now() {
                    return write_error_response(socket, 400, "cannot send queued messages now")
                        .await;
                }
                tick_remote_host_app(&mut app);
                remote_state(&app, host_state.as_ref())
            };
            write_json_response(socket, 200, &response).await
        }
        ("POST", "/api/permission") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: PermissionAnswerRequest = parse_json_body(&request)?;
            let response = match parse_permission_response(&payload.response) {
                Ok(response) => response,
                Err(err) => return write_error_response(socket, 400, &err.to_string()).await,
            };
            let result = {
                let mut app = app.lock().await;
                if !app.remote_respond_permission(response) {
                    None
                } else {
                    tick_remote_host_app(&mut app);
                    Some(remote_state(&app, host_state.as_ref()))
                }
            };
            let Some(result) = result else {
                return write_error_response(socket, 409, "no pending permission request").await;
            };
            write_json_response(socket, 200, &result).await
        }
        ("POST", "/api/question") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: QuestionAnswerRequest = parse_json_body(&request)?;
            let result = {
                let mut app = app.lock().await;
                if !app.remote_answer_question(payload.answers) {
                    None
                } else {
                    tick_remote_host_app(&mut app);
                    Some(remote_state(&app, host_state.as_ref()))
                }
            };
            let Some(result) = result else {
                return write_error_response(socket, 409, "no pending question").await;
            };
            write_json_response(socket, 200, &result).await
        }
        ("POST", "/api/question/cancel") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let result = {
                let mut app = app.lock().await;
                if !app.remote_cancel_question() {
                    None
                } else {
                    tick_remote_host_app(&mut app);
                    Some(remote_state(&app, host_state.as_ref()))
                }
            };
            let Some(result) = result else {
                return write_error_response(socket, 409, "no pending question").await;
            };
            write_json_response(socket, 200, &result).await
        }
        ("POST", "/api/terminal/frame") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: RemoteTerminalFrameRequest = parse_json_body(&request)?;
            let frame = {
                let mut app = app.lock().await;
                tick_remote_host_app(&mut app);
                render_terminal_snapshot(&mut app, payload.width, payload.height)?
            };
            write_json_response(socket, 200, &frame).await
        }
        ("POST", "/api/terminal/event") => {
            if !authorized(&request, host_state.as_ref()) {
                return write_error_response(socket, 401, "pairing required").await;
            }
            let payload: RemoteTerminalEvent = parse_json_body(&request)?;
            let response = {
                let mut app = app.lock().await;
                let detach_client = payload.detaches_client();
                if !detach_client {
                    payload.apply_to_app(&mut app);
                }
                let app_requested_quit = !app.running;
                if app_requested_quit {
                    app.remote_recover_after_client_quit();
                } else {
                    keep_remote_host_app_alive(&mut app);
                }
                tick_remote_host_app(&mut app);
                RemoteTerminalEventResponse {
                    running: !detach_client && !app_requested_quit,
                }
            };
            write_json_response(socket, 200, &response).await
        }
        _ if request.method == "GET" && !request.path.starts_with("/api/") => {
            write_remote_asset_response(socket, &request.path).await
        }
        _ => write_error_response(socket, 404, "not found").await,
    }
}

async fn read_http_request(socket: &mut TcpStream) -> Result<Option<HttpRequest>> {
    let mut buffer = Vec::new();
    let mut scratch = [0_u8; 4096];
    let header_end;

    loop {
        let n = socket
            .read(&mut scratch)
            .await
            .context("failed to read request")?;
        if n == 0 {
            if buffer.is_empty() {
                return Ok(None);
            }
            bail!("connection closed before request finished");
        }
        buffer.extend_from_slice(&scratch[..n]);
        if buffer.len() > MAX_HTTP_HEADER_BYTES + MAX_HTTP_BODY_BYTES {
            bail!("request too large");
        }
        if let Some(idx) = find_header_end(&buffer) {
            header_end = idx;
            break;
        }
        if buffer.len() > MAX_HTTP_HEADER_BYTES {
            bail!("request headers too large");
        }
    }

    let header_bytes = &buffer[..header_end];
    let header_text = std::str::from_utf8(header_bytes).context("request headers are not utf-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing request method"))?
        .to_string();
    let target = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing request target"))?;
    let (path, query) = target
        .split_once('?')
        .map(|(path, query)| (path.to_string(), query.to_string()))
        .unwrap_or_else(|| (target.to_string(), String::new()));

    let mut headers = HashMap::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_HTTP_BODY_BYTES {
        bail!("request body too large");
    }

    let body_start = header_end + 4;
    while buffer.len().saturating_sub(body_start) < content_length {
        let n = socket
            .read(&mut scratch)
            .await
            .context("failed to read request body")?;
        if n == 0 {
            bail!("connection closed before request body finished");
        }
        buffer.extend_from_slice(&scratch[..n]);
    }

    let body = buffer[body_start..body_start + content_length].to_vec();
    Ok(Some(HttpRequest {
        method,
        path,
        query,
        headers,
        body,
    }))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_json_body<T: DeserializeOwned>(request: &HttpRequest) -> Result<T> {
    serde_json::from_slice(&request.body).context("invalid json body")
}

fn parse_permission_response(value: &str) -> Result<PermissionResponse> {
    match value.trim().to_ascii_lowercase().as_str() {
        "deny" | "reject" => Ok(PermissionResponse::Deny),
        "allow_once" | "allow-once" | "once" => Ok(PermissionResponse::AllowOnce),
        "allow_always" | "allow-always" | "always" => Ok(PermissionResponse::AllowAlways),
        _ => bail!("unknown permission response: {value}"),
    }
}

fn remote_prompt_image_paths(images: &[PromptImageRequest]) -> Result<Vec<PathBuf>> {
    if images.len() > MAX_REMOTE_PROMPT_IMAGES {
        bail!(
            "too many images attached ({} > {})",
            images.len(),
            MAX_REMOTE_PROMPT_IMAGES
        );
    }

    images.iter().map(remote_prompt_image_path).collect()
}

fn remote_prompt_image_path(image: &PromptImageRequest) -> Result<PathBuf> {
    let (data_url_media_type, bytes) = decode_image_data_url(&image.data_url)?;
    if bytes.len() > MAX_REMOTE_PROMPT_IMAGE_BYTES {
        bail!(
            "image {} is too large ({}MB > {}MB limit)",
            remote_prompt_image_name(&image.name),
            bytes.len() / (1024 * 1024),
            MAX_REMOTE_PROMPT_IMAGE_BYTES / (1024 * 1024)
        );
    }

    let format = image::guess_format(&bytes).context("attached image could not be decoded")?;
    let media_type = image_format_mime_type(format)
        .ok_or_else(|| anyhow!("unsupported image type: {}", data_url_media_type))?;
    if !image.media_type.trim().is_empty()
        && image.media_type.trim() != media_type
        && !image
            .media_type
            .trim()
            .eq_ignore_ascii_case(&data_url_media_type)
    {
        bail!("attached image media type does not match its contents");
    }

    let extension = image_format_extension(format)
        .ok_or_else(|| anyhow!("unsupported image type: {}", media_type))?;
    let mut temp = tempfile::Builder::new()
        .prefix("crabcode-browser-")
        .suffix(extension)
        .tempfile()
        .context("failed to create image attachment file")?;
    temp.write_all(&bytes)
        .context("failed to write image attachment file")?;
    let (_file, path) = temp.keep().context("failed to persist image attachment")?;

    if !crate::utils::image_attachment::is_supported_image_path(&path) {
        bail!("attached image could not be read");
    }

    Ok(path)
}

fn decode_image_data_url(data_url: &str) -> Result<(String, Vec<u8>)> {
    let (header, encoded) = data_url
        .split_once(',')
        .ok_or_else(|| anyhow!("invalid image data URL"))?;
    let metadata = header
        .strip_prefix("data:")
        .ok_or_else(|| anyhow!("invalid image data URL"))?;
    let mut parts = metadata.split(';');
    let media_type = parts.next().unwrap_or_default().to_ascii_lowercase();
    if !media_type.starts_with("image/") || !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        bail!("invalid image data URL");
    }

    let bytes = general_purpose::STANDARD
        .decode(encoded)
        .context("invalid image data URL encoding")?;
    Ok((media_type, bytes))
}

fn image_format_mime_type(format: image::ImageFormat) -> Option<&'static str> {
    match format {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn image_format_extension(format: image::ImageFormat) -> Option<&'static str> {
    match format {
        image::ImageFormat::Png => Some(".png"),
        image::ImageFormat::Jpeg => Some(".jpg"),
        image::ImageFormat::Gif => Some(".gif"),
        image::ImageFormat::WebP => Some(".webp"),
        _ => None,
    }
}

fn remote_prompt_image_name(name: &str) -> &str {
    let name = name.trim();
    if name.is_empty() {
        "attachment"
    } else {
        name
    }
}

fn authorized(request: &HttpRequest, host_state: &HostState) -> bool {
    if !host_state.auth_required() {
        return true;
    }

    if let Some(value) = request.headers.get("authorization") {
        if let Some(token) = value.trim().strip_prefix("Bearer ") {
            return host_state.accepts_token(token);
        }
    }

    query_param(&request.query, "token").is_some_and(|token| host_state.accepts_token(&token))
}

fn query_param(query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

fn resolve_project_favicon_path(cwd: &str) -> Option<PathBuf> {
    let project_cwd = Path::new(cwd);

    resolve_project_favicon_path_direct(project_cwd)
        .or_else(|| resolve_workspace_project_favicon_path(project_cwd))
}

fn resolve_project_favicon_path_direct(project_cwd: &Path) -> Option<PathBuf> {
    for candidate in FAVICON_CANDIDATES {
        let resolved = project_cwd.join(candidate);
        if let Some(existing) = find_existing_project_file(project_cwd, &[resolved]) {
            return Some(existing);
        }
    }

    for source_file in ICON_SOURCE_FILES {
        let source_path = project_cwd.join(source_file);
        let Ok(source) = std::fs::read_to_string(source_path) else {
            continue;
        };
        let Some(href) = extract_icon_href(&source) else {
            continue;
        };
        if let Some(existing) =
            find_existing_project_file(project_cwd, &resolve_icon_href(project_cwd, &href))
        {
            return Some(existing);
        }
    }

    None
}

fn resolve_workspace_project_favicon_path(project_cwd: &Path) -> Option<PathBuf> {
    for workspace_root in package_workspace_roots(project_cwd) {
        if let Some(existing) = resolve_project_favicon_path_direct(&workspace_root) {
            return Some(existing);
        }
    }

    None
}

fn package_workspace_roots(project_cwd: &Path) -> Vec<PathBuf> {
    let Ok(source) = std::fs::read_to_string(project_cwd.join("package.json")) else {
        return Vec::new();
    };
    let Ok(package_json) = serde_json::from_str::<serde_json::Value>(&source) else {
        return Vec::new();
    };

    workspace_patterns(&package_json)
        .into_iter()
        .flat_map(|pattern| expand_workspace_pattern(project_cwd, &pattern))
        .collect()
}

fn workspace_patterns(package_json: &serde_json::Value) -> Vec<String> {
    let Some(workspaces) = package_json.get("workspaces") else {
        return Vec::new();
    };

    if let Some(patterns) = workspaces.as_array() {
        return patterns
            .iter()
            .filter_map(|value| value.as_str())
            .filter(|pattern| !pattern.trim().is_empty() && !pattern.trim_start().starts_with('!'))
            .map(str::to_string)
            .collect();
    }

    workspaces
        .get("packages")
        .and_then(|packages| packages.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .filter(|pattern| !pattern.trim().is_empty() && !pattern.trim_start().starts_with('!'))
        .map(str::to_string)
        .collect()
}

fn expand_workspace_pattern(project_cwd: &Path, pattern: &str) -> Vec<PathBuf> {
    let pattern_path = project_cwd.join(pattern);
    let pattern_text = pattern_path.to_string_lossy();
    let Ok(entries) = glob::glob(&pattern_text) else {
        return Vec::new();
    };

    let mut roots = entries
        .filter_map(Result::ok)
        .filter(|path| is_path_within_project(project_cwd, path))
        .filter(|path| std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

fn resolve_icon_href(project_cwd: &Path, href: &str) -> Vec<PathBuf> {
    let clean = href.trim_start_matches('/');
    vec![
        project_cwd.join("public").join(clean),
        project_cwd.join(clean),
    ]
}

fn find_existing_project_file(project_cwd: &Path, candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find_map(|candidate| {
        if !is_path_within_project(project_cwd, candidate) {
            return None;
        }

        std::fs::metadata(candidate)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|_| candidate.clone())
    })
}

fn is_path_within_project(project_cwd: &Path, candidate: &Path) -> bool {
    let project = normalize_absolute_path(project_cwd);
    let candidate = normalize_absolute_path(candidate);
    candidate == project || candidate.starts_with(project)
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut absolute = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    absolute.push(path);

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn extract_icon_href(source: &str) -> Option<String> {
    let link_tag_re = regex::RegexBuilder::new(r"<link\b[^>]*>")
        .case_insensitive(true)
        .build()
        .ok()?;
    let html_rel_re = regex::RegexBuilder::new(r#"\brel=["'](?:icon|shortcut icon)["']"#)
        .case_insensitive(true)
        .build()
        .ok()?;
    let html_href_re = regex::RegexBuilder::new(r#"\bhref=["']([^"'?]+)"#)
        .case_insensitive(true)
        .build()
        .ok()?;
    let object_rel_re = regex::RegexBuilder::new(r#"\brel\s*:\s*["'](?:icon|shortcut icon)["']"#)
        .case_insensitive(true)
        .build()
        .ok()?;
    let object_href_re = regex::RegexBuilder::new(r#"\bhref\s*:\s*["']([^"'?]+)"#)
        .case_insensitive(true)
        .build()
        .ok()?;

    for link_tag in link_tag_re.find_iter(source).map(|found| found.as_str()) {
        if !html_rel_re.is_match(link_tag) {
            continue;
        }
        if let Some(href) = html_href_re
            .captures(link_tag)
            .and_then(|captures| captures.get(1))
            .map(|href| href.as_str().to_string())
        {
            return Some(href);
        }
    }

    for block in source.split('}') {
        if !object_rel_re.is_match(block) {
            continue;
        }
        if let Some(href) = object_href_re
            .captures(block)
            .and_then(|captures| captures.get(1))
            .map(|href| href.as_str().to_string())
        {
            return Some(href);
        }
    }

    None
}

fn project_favicon_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

async fn write_json_response<T: Serialize>(
    socket: &mut TcpStream,
    status: u16,
    body: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(body).context("failed to serialize response")?;
    write_response(socket, status, "application/json; charset=utf-8", &bytes).await
}

async fn write_remote_asset_response(socket: &mut TcpStream, path: &str) -> Result<()> {
    let normalized = if path == "/" { "/index.html" } else { path };

    let index_asset = remote_assets::remote_asset("/index.html");
    if index_asset.is_none() {
        return write_error_response(
            socket,
            500,
            "remote client assets are not built; run `just remote-client-build`",
        )
        .await;
    };

    let asset = remote_assets::remote_asset(normalized).or_else(|| {
        if normalized.starts_with("/assets/") {
            None
        } else {
            index_asset
        }
    });

    let Some(asset) = asset else {
        return write_error_response(socket, 404, "asset not found").await;
    };

    write_response(socket, 200, asset.content_type, asset.body).await
}

async fn write_error_response(socket: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    write_json_response(socket, status, &serde_json::json!({ "error": message })).await
}

async fn write_empty_response(socket: &mut TcpStream, status: u16) -> Result<()> {
    write_response(socket, status, "text/plain; charset=utf-8", &[]).await
}

async fn write_state_events_response(
    socket: &mut TcpStream,
    app: Arc<TokioMutex<App>>,
    host_state: Arc<HostState>,
) -> Result<()> {
    let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-store\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\n";
    socket.write_all(header.as_bytes()).await?;
    socket.flush().await?;

    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut last_state = Vec::new();
    let mut idle_ticks = 0_u16;

    loop {
        interval.tick().await;
        let state = {
            let mut app = app.lock().await;
            tick_remote_host_app(&mut app);
            remote_state(&app, host_state.as_ref())
        };
        let bytes = serde_json::to_vec(&state).context("failed to serialize state event")?;
        if bytes != last_state {
            if let Err(err) = write_sse_event(socket, "state", &bytes).await {
                return if is_disconnect_error(&err) {
                    Ok(())
                } else {
                    Err(err)
                };
            }
            last_state = bytes;
            idle_ticks = 0;
        } else {
            idle_ticks = idle_ticks.saturating_add(1);
            if idle_ticks >= 60 {
                if let Err(err) = write_sse_comment(socket, "keepalive").await {
                    return if is_disconnect_error(&err) {
                        Ok(())
                    } else {
                        Err(err)
                    };
                }
                idle_ticks = 0;
            }
        }
    }
}

async fn write_sse_event(socket: &mut TcpStream, event: &str, data: &[u8]) -> Result<()> {
    socket.write_all(b"event: ").await?;
    socket.write_all(event.as_bytes()).await?;
    socket.write_all(b"\ndata: ").await?;
    socket.write_all(data).await?;
    socket.write_all(b"\n\n").await?;
    socket.flush().await?;
    Ok(())
}

async fn write_sse_comment(socket: &mut TcpStream, comment: &str) -> Result<()> {
    socket.write_all(b": ").await?;
    socket.write_all(comment.as_bytes()).await?;
    socket.write_all(b"\n\n").await?;
    socket.flush().await?;
    Ok(())
}

async fn write_response(
    socket: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let header = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(header.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.flush().await?;
    Ok(())
}

fn remote_state(app: &App, host_state: &HostState) -> RemoteState {
    let status = remote_status(app, host_state);
    let mut session_infos = app
        .session_manager
        .list_sessions()
        .into_iter()
        .filter(|session| session.parent_id.is_none() && session.archived_at.is_none())
        .collect::<Vec<_>>();
    session_infos.sort_by(|a, b| {
        a.workspace_sort_order
            .cmp(&b.workspace_sort_order)
            .then_with(|| a.workspace_id.cmp(&b.workspace_id))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.title.cmp(&b.title))
    });
    let projects = remote_workspaces(app, &session_infos);
    let mut workspace_session_counts = HashMap::<i64, usize>::new();
    let sessions = session_infos
        .into_iter()
        .filter(|session| {
            let count = workspace_session_counts
                .entry(session.workspace_id)
                .or_insert(0);
            if *count >= MAX_REMOTE_SESSIONS_PER_WORKSPACE {
                return false;
            }

            *count += 1;
            true
        })
        .map(remote_session)
        .collect::<Vec<_>>();

    let current_session_id = app.session_manager.get_current_session_id().cloned();
    let messages = current_session_id
        .as_deref()
        .map(|session_id| active_messages_for_session(app, session_id))
        .unwrap_or_default();

    RemoteState {
        status,
        projects,
        sessions,
        current_session_id,
        messages,
        is_streaming: app.is_streaming,
        queued_messages: app.remote_queued_message_previews(),
        pending_permission: remote_permission_prompt(app),
        pending_question: remote_question_prompt(app),
        thread_tabs: remote_thread_tabs(app),
    }
}

fn remote_thread_tabs(app: &App) -> Option<RemoteThreadTabs> {
    let tabs = app.subagent_tabs_for_current_session()?;
    Some(RemoteThreadTabs {
        root_session_id: tabs.root_session_id,
        is_child_session: tabs.is_child_session,
        tabs: tabs
            .tabs
            .into_iter()
            .enumerate()
            .map(|(idx, tab)| RemoteThreadTab {
                session_id: tab.session_id,
                label: tab.label,
                agent: tab.agent,
                model: tab.model,
                active: tab.active,
                running: tab.running,
                kind: if idx == 0 {
                    "main".to_string()
                } else {
                    "subagent".to_string()
                },
                accent: color_to_css(tab.color, "#6c8ed8"),
            })
            .collect(),
    })
}

fn remote_status(app: &App, host_state: &HostState) -> RemoteStatus {
    let cwd = app.remote_workspace_path();
    RemoteStatus {
        version: app.version.clone(),
        workspace: workspace_label(app),
        cwd: cwd.clone(),
        provider: app.provider_name.clone(),
        model: app.model.clone(),
        agent: app.agent.clone(),
        primary_agents: app
            .agent_registry
            .visible_primary_agent_names()
            .into_iter()
            .map(|agent| titlecase_remote_agent_name(&agent))
            .collect(),
        reasoning_effort: app.remote_reasoning_effort_label(),
        reasoning_efforts: app.remote_reasoning_effort_options(),
        browser_url: host_state.browser_url.clone(),
        suggested_alias: host_state.suggested_alias.clone(),
        auth_required: host_state.auth_required(),
        pair_expires_at: host_state.pair_expires_at.unwrap_or(0),
        theme: remote_theme(app),
        git_summary: remote_git_summary(host_state, &cwd),
    }
}

fn remote_git_summary(host_state: &HostState, cwd: &str) -> RemoteGitSummary {
    let now = Instant::now();
    if let Ok(cache) = host_state.git_summary_cache.lock() {
        if let Some(entry) = cache.as_ref() {
            if entry.cwd == cwd && now.duration_since(entry.checked_at) < GIT_SUMMARY_CACHE_TTL {
                return entry.summary.clone();
            }
        }
    }

    let summary = git_summary_for_path(cwd).unwrap_or(RemoteGitSummary {
        is_repo: false,
        branch: None,
    });

    if let Ok(mut cache) = host_state.git_summary_cache.lock() {
        *cache = Some(GitSummaryCacheEntry {
            cwd: cwd.to_string(),
            checked_at: now,
            summary: summary.clone(),
        });
    }

    summary
}

fn git_summary_for_path(cwd: &str) -> Option<RemoteGitSummary> {
    let output = Command::new("git")
        .args(["-C", cwd, "rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()?;
    if !output.status.success() {
        return Some(RemoteGitSummary {
            is_repo: false,
            branch: None,
        });
    }

    Some(RemoteGitSummary {
        is_repo: true,
        branch: crate::utils::git::get_branch_for_path(cwd)
            .or_else(|| git_short_head_for_path(cwd))
            .or_else(|| Some("HEAD".to_string())),
    })
}

fn git_short_head_for_path(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", cwd, "rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn remote_git_status(cwd: &str) -> Result<RemoteGitStatus> {
    let Some(summary) = git_summary_for_path(cwd) else {
        return Ok(empty_remote_git_status());
    };
    if !summary.is_repo {
        return Ok(RemoteGitStatus {
            branch: summary.branch,
            ..empty_remote_git_status()
        });
    }

    let has_head = git_has_head(cwd);
    let numstat = if has_head {
        git_output(cwd, &["diff", "--numstat", "HEAD", "--"])?
    } else {
        String::new()
    };
    let (mut files, mut truncated_files) = parse_git_numstat(&numstat);
    append_untracked_git_files(cwd, &mut files, &mut truncated_files)?;
    let diff = if has_head {
        git_output_limited(
            cwd,
            &[
                "diff",
                "--no-ext-diff",
                "--unified=3",
                "--find-renames",
                "--find-copies",
                "HEAD",
                "--",
            ],
            MAX_GIT_DIFF_BYTES,
        )?
    } else {
        LimitedCommandOutput {
            output: String::new(),
            truncated: false,
        }
    };
    let mut diff_files = parse_git_diff(&diff.output);

    let diff_truncated = diff.truncated;
    if diff_truncated {
        if let Some(file) = diff_files.last_mut() {
            file.truncated = true;
        }
    }

    if files.is_empty() && !diff_files.is_empty() {
        files = diff_files
            .iter()
            .map(|file| RemoteGitFileChange {
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                status: file.status.clone(),
                additions: file.additions,
                deletions: file.deletions,
                binary: file.binary,
            })
            .collect();
    }

    // Tracked diffs come from `git diff HEAD`. Untracked files need separate
    // `git diff --no-index` patches so the UI can show full-file additions.
    append_untracked_git_diffs(cwd, &files, &mut diff_files, &mut truncated_files)?;

    merge_git_diff_file_metadata(&mut files, &diff_files);

    if files.len() > MAX_GIT_FILES {
        files.truncate(MAX_GIT_FILES);
        truncated_files = true;
    }
    if diff_files.len() > MAX_GIT_FILES {
        diff_files.truncate(MAX_GIT_FILES);
    }

    let additions = files.iter().map(|file| file.additions).sum();
    let deletions = files.iter().map(|file| file.deletions).sum();
    let changed_files = files.len();

    Ok(RemoteGitStatus {
        is_repo: true,
        branch: summary.branch,
        changed_files,
        additions,
        deletions,
        files,
        diff_files,
        truncated: truncated_files || diff_truncated,
    })
}

fn empty_remote_git_status() -> RemoteGitStatus {
    RemoteGitStatus {
        is_repo: false,
        branch: None,
        changed_files: 0,
        additions: 0,
        deletions: 0,
        files: Vec::new(),
        diff_files: Vec::new(),
        truncated: false,
    }
}

fn git_output(cwd: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "git {} failed{}",
            args.join(" "),
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct LimitedCommandOutput {
    output: String,
    truncated: bool,
}

fn git_output_limited(cwd: &str, args: &[&str], max_bytes: usize) -> Result<LimitedCommandOutput> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    let mut output = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut truncated = false;
    if let Some(stdout) = child.stdout.as_mut() {
        let mut buffer = [0_u8; 8192];
        loop {
            let read = stdout
                .read(&mut buffer)
                .with_context(|| format!("failed to read git {}", args.join(" ")))?;
            if read == 0 {
                break;
            }
            let remaining = max_bytes.saturating_sub(output.len());
            if read <= remaining {
                output.extend_from_slice(&buffer[..read]);
            } else {
                output.extend_from_slice(&buffer[..remaining]);
                truncated = true;
                let _ = child.kill();
                break;
            }
        }
    }

    let status = child
        .wait()
        .with_context(|| format!("failed to wait for git {}", args.join(" ")))?;
    if !status.success() && !truncated {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let stderr = stderr.trim();
        bail!(
            "git {} failed{}",
            args.join(" "),
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        );
    }

    Ok(LimitedCommandOutput {
        output: String::from_utf8_lossy(&output).into_owned(),
        truncated,
    })
}

fn git_has_head(cwd: &str) -> bool {
    Command::new("git")
        .args(["-C", cwd, "rev-parse", "--verify", "HEAD"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn append_untracked_git_files(
    cwd: &str,
    files: &mut Vec<RemoteGitFileChange>,
    truncated: &mut bool,
) -> Result<()> {
    let output = git_output(cwd, &["ls-files", "--others", "--exclude-standard"])?;
    for path in output.lines().filter(|line| !line.trim().is_empty()) {
        if files.len() >= MAX_GIT_FILES {
            *truncated = true;
            break;
        }
        files.push(RemoteGitFileChange {
            path: path.to_string(),
            old_path: None,
            status: "untracked".to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
        });
    }
    Ok(())
}

fn merge_git_diff_file_metadata(
    files: &mut [RemoteGitFileChange],
    diff_files: &[RemoteGitDiffFile],
) {
    let by_path = diff_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<HashMap<_, _>>();
    for file in files.iter_mut() {
        let Some(diff_file) = by_path.get(file.path.as_str()) else {
            continue;
        };
        // Keep untracked status labels from porcelain; only fill missing metadata.
        if file.status != "untracked" {
            file.status = diff_file.status.clone();
        }
        file.old_path = file.old_path.clone().or_else(|| diff_file.old_path.clone());
        file.binary = file.binary || diff_file.binary;
        if file.additions == 0 && file.deletions == 0 {
            file.additions = diff_file.additions;
            file.deletions = diff_file.deletions;
        }
    }
}

/// Build full-file addition patches for untracked files via `git diff --no-index`.
fn append_untracked_git_diffs(
    cwd: &str,
    files: &[RemoteGitFileChange],
    diff_files: &mut Vec<RemoteGitDiffFile>,
    truncated_files: &mut bool,
) -> Result<()> {
    if diff_files.len() >= MAX_GIT_FILES {
        *truncated_files = true;
        return Ok(());
    }

    let mut existing = diff_files
        .iter()
        .map(|file| file.path.clone())
        .collect::<std::collections::HashSet<_>>();

    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };

    for file in files {
        if file.status != "untracked" || existing.contains(&file.path) {
            continue;
        }
        if diff_files.len() >= MAX_GIT_FILES {
            *truncated_files = true;
            break;
        }

        // Binary / non-UTF8 files: surface as binary without loading content.
        if file_looks_binary(cwd, &file.path) {
            diff_files.push(RemoteGitDiffFile {
                path: file.path.clone(),
                old_path: None,
                status: "added".to_string(),
                additions: 0,
                deletions: 0,
                binary: true,
                lines: Vec::new(),
                truncated: false,
            });
            existing.insert(file.path.clone());
            continue;
        }

        // `git diff --no-index` exits 1 when files differ (always for untracked).
        let mut child = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-index",
                "--unified=3",
                "--",
                null_device,
                file.path.as_str(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to diff untracked file {}", file.path))?;

        let mut output = Vec::with_capacity(64 * 1024);
        let mut truncated = false;
        if let Some(stdout) = child.stdout.as_mut() {
            let mut buffer = [0_u8; 8192];
            loop {
                let read = stdout
                    .read(&mut buffer)
                    .with_context(|| format!("failed to read untracked diff for {}", file.path))?;
                if read == 0 {
                    break;
                }
                let remaining = MAX_GIT_DIFF_BYTES.saturating_sub(output.len());
                if read <= remaining {
                    output.extend_from_slice(&buffer[..read]);
                } else {
                    output.extend_from_slice(&buffer[..remaining]);
                    truncated = true;
                    let _ = child.kill();
                    break;
                }
            }
        }
        let _ = child.wait();

        let patch = String::from_utf8_lossy(&output);
        if patch.trim().is_empty() {
            // Empty untracked file: still show a minimal added-file patch.
            let mut empty = RemoteGitDiffFile {
                path: file.path.clone(),
                old_path: None,
                status: "added".to_string(),
                additions: 0,
                deletions: 0,
                binary: false,
                lines: Vec::new(),
                truncated: false,
            };
            push_git_diff_line(
                &mut empty,
                "hunk",
                "@@ -0,0 +0,0 @@".to_string(),
                None,
                None,
            );
            existing.insert(file.path.clone());
            diff_files.push(empty);
            continue;
        }

        let mut parsed = parse_git_diff(&patch);
        if let Some(parsed_file) = parsed.first_mut() {
            parsed_file.path = file.path.clone();
            parsed_file.old_path = None;
            parsed_file.status = "added".to_string();
            if truncated {
                parsed_file.truncated = true;
            }
            existing.insert(file.path.clone());
            diff_files.push(parsed_file.clone());
        } else if truncated {
            *truncated_files = true;
        }
    }

    Ok(())
}

fn file_looks_binary(cwd: &str, rel_path: &str) -> bool {
    let path = Path::new(cwd).join(rel_path);
    let Ok(mut file) = std::fs::File::open(&path) else {
        return false;
    };
    let mut sample = [0_u8; 8192];
    let Ok(read) = file.read(&mut sample) else {
        return false;
    };
    if read == 0 {
        return false;
    }
    sample[..read].contains(&0)
}

fn parse_git_numstat(output: &str) -> (Vec<RemoteGitFileChange>, bool) {
    let mut files = Vec::new();
    let mut truncated = false;
    for line in output.lines() {
        if files.len() >= MAX_GIT_FILES {
            truncated = true;
            break;
        }
        let mut fields = line.split('\t');
        let added = fields.next().unwrap_or_default();
        let deleted = fields.next().unwrap_or_default();
        let Some(raw_path) = fields.next() else {
            continue;
        };
        let binary = added == "-" || deleted == "-";
        let (old_path, path) = parse_numstat_path(raw_path);
        files.push(RemoteGitFileChange {
            path,
            old_path,
            status: "modified".to_string(),
            additions: added.parse().unwrap_or(0),
            deletions: deleted.parse().unwrap_or(0),
            binary,
        });
    }
    (files, truncated)
}

fn parse_numstat_path(raw: &str) -> (Option<String>, String) {
    if !raw.starts_with('{') || !raw.ends_with('}') {
        return (None, raw.to_string());
    }
    let inner = &raw[1..raw.len().saturating_sub(1)];
    let Some((prefix, suffix)) = inner.split_once(" => ") else {
        return (None, raw.to_string());
    };
    (Some(prefix.to_string()), suffix.to_string())
}

fn parse_git_diff(output: &str) -> Vec<RemoteGitDiffFile> {
    let mut files = Vec::new();
    let mut current: Option<RemoteGitDiffFile> = None;
    let mut old_line = 0_usize;
    let mut new_line = 0_usize;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
                if files.len() >= MAX_GIT_FILES {
                    break;
                }
            }
            let (old_path, path) = parse_diff_git_paths(rest);
            current = Some(RemoteGitDiffFile {
                path,
                old_path,
                status: "modified".to_string(),
                additions: 0,
                deletions: 0,
                binary: false,
                lines: Vec::new(),
                truncated: false,
            });
            old_line = 0;
            new_line = 0;
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };

        if line.starts_with("new file mode ") {
            file.status = "added".to_string();
            continue;
        }
        if line.starts_with("deleted file mode ") {
            file.status = "deleted".to_string();
            continue;
        }
        if line.starts_with("rename from ") {
            file.status = "renamed".to_string();
            continue;
        }
        if line.starts_with("Binary files ") {
            file.binary = true;
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("index ") {
            continue;
        }
        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            old_line = old_start;
            new_line = new_start;
            push_git_diff_line(file, "hunk", line.to_string(), None, None);
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            if line.starts_with("+++") {
                continue;
            }
            let line_number = new_line;
            new_line = new_line.saturating_add(1);
            file.additions = file.additions.saturating_add(1);
            push_git_diff_line(file, "add", text.to_string(), None, Some(line_number));
            continue;
        }
        if let Some(text) = line.strip_prefix('-') {
            if line.starts_with("---") {
                continue;
            }
            let line_number = old_line;
            old_line = old_line.saturating_add(1);
            file.deletions = file.deletions.saturating_add(1);
            push_git_diff_line(file, "remove", text.to_string(), Some(line_number), None);
            continue;
        }
        if line.starts_with(' ') {
            let line_number = new_line;
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
            push_git_diff_line(
                file,
                "context",
                line.strip_prefix(' ').unwrap_or(line).to_string(),
                Some(old_line.saturating_sub(1)),
                Some(line_number),
            );
            continue;
        }
        if line.starts_with("\\ No newline at end of file") {
            push_git_diff_line(file, "meta", line.to_string(), None, None);
        }
    }

    if let Some(file) = current.take() {
        files.push(file);
    }
    files
}

fn push_git_diff_line(
    file: &mut RemoteGitDiffFile,
    kind: &str,
    text: String,
    old_line: Option<usize>,
    new_line: Option<usize>,
) {
    if file.lines.len() >= MAX_GIT_PATCH_LINES_PER_FILE {
        file.truncated = true;
        return;
    }
    let text = truncate_chars(&text, MAX_GIT_PATCH_LINE_CHARS);
    file.lines.push(RemoteGitDiffLine {
        kind: kind.to_string(),
        text,
        old_line,
        new_line,
    });
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut iter = value.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn parse_diff_git_paths(rest: &str) -> (Option<String>, String) {
    if let Some(rest) = rest.strip_prefix("a/") {
        if let Some((old, new)) = rest.split_once(" b/") {
            let old = old.to_string();
            let new = new.to_string();
            let old_path = (old != new && !old.is_empty()).then_some(old);
            return (old_path, new);
        }
    }

    let mut parts = rest.split_whitespace();
    let old = parts.next().map(strip_diff_prefix).unwrap_or_default();
    let new = parts
        .next()
        .map(strip_diff_prefix)
        .unwrap_or_else(|| old.clone());
    let old_path = (old != new && !old.is_empty()).then_some(old);
    (old_path, new)
}

fn strip_diff_prefix(value: &str) -> String {
    value
        .strip_prefix("a/")
        .or_else(|| value.strip_prefix("b/"))
        .unwrap_or(value)
        .to_string()
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_part, rest) = rest.split_once(" +")?;
    let (new_part, _) = rest.split_once(" @@")?;
    Some((parse_hunk_start(old_part), parse_hunk_start(new_part)))
}

fn parse_hunk_start(part: &str) -> usize {
    part.split(',')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn remote_theme(app: &App) -> RemoteTheme {
    let colors = app.get_current_theme_colors();
    RemoteTheme {
        primary: color_to_css(colors.primary, "#6c8ed8"),
        primary_dim: color_to_css(crate::theme::darken_color(colors.primary, 0.7), "#4a639f"),
    }
}

fn color_to_css(color: Color, fallback: &str) -> String {
    let (r, g, b) = match color {
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (102, 102, 102),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (255, 255, 255),
        Color::Indexed(_) | Color::Reset => return fallback.to_string(),
        Color::Rgb(r, g, b) => (r, g, b),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn remote_models(app: &mut App) -> Vec<RemoteModelOption> {
    app.remote_model_items()
        .into_iter()
        .map(|item| RemoteModelOption {
            active: item.provider_id == app.provider_name && item.id == app.model,
            favorite: item.group == "Favorite",
            description: remote_model_description(&item.description, &item.provider_id),
            id: item.id,
            name: item.name,
            group: item.group,
            provider_id: item.provider_id,
        })
        .collect()
}

fn remote_model_description(description: &str, provider_id: &str) -> String {
    let label = description.split('|').next().unwrap_or(description).trim();
    if label.is_empty() {
        provider_id.to_string()
    } else {
        label.to_string()
    }
}

fn remote_suggestions(app: &App, payload: &AutocompleteRequest) -> Vec<RemoteSuggestion> {
    app.remote_autocomplete_suggestions(
        payload.trigger.trim(),
        payload.query.trim(),
        payload.is_chat,
    )
    .into_iter()
    .map(|suggestion| RemoteSuggestion {
        name: suggestion.name,
        description: suggestion.description,
        replacement: suggestion.replacement,
        kind: match suggestion.kind {
            crate::autocomplete::SuggestionKind::Command => "command",
            crate::autocomplete::SuggestionKind::Agent => "agent",
            crate::autocomplete::SuggestionKind::File => "file",
        }
        .to_string(),
        is_directory: suggestion.is_directory,
    })
    .collect()
}

fn remote_skills(app: &App) -> Vec<RemoteSkill> {
    app.remote_skills()
        .into_iter()
        .map(|skill| RemoteSkill {
            name: skill.name,
            description: skill.description.unwrap_or_default(),
            location: skill.location.to_string_lossy().to_string(),
        })
        .collect()
}

fn remote_mcp_list(app: &App) -> Vec<RemoteMcpServer> {
    app.remote_mcp_servers()
        .into_iter()
        .map(|server| RemoteMcpServer {
            name: server.name,
            enabled: server.enabled,
            status: server.status,
            kind: server.kind,
        })
        .collect()
}

fn workspace_label(app: &App) -> String {
    app.remote_workspace_name()
}

fn remote_workspaces(app: &App, sessions: &[SessionInfo]) -> Vec<RemoteWorkspace> {
    let mut by_key = HashMap::<String, RemoteWorkspace>::new();
    let current_path = app.remote_workspace_path();
    let last_opened_by_path = app
        .session_manager
        .list_workspaces()
        .into_iter()
        .map(|workspace| (workspace.path, workspace.last_opened_at))
        .collect::<HashMap<_, _>>();

    for session in sessions {
        let last_opened_at = last_opened_by_path
            .get(&session.workspace_path)
            .copied()
            .unwrap_or(0);
        insert_remote_workspace(
            &mut by_key,
            RemoteWorkspace {
                name: session.workspace_name.clone(),
                path: session.workspace_path.clone(),
                sort_order: session.workspace_sort_order,
                last_opened_at,
            },
        );
    }

    if !current_path.trim().is_empty() {
        let current_name = app.remote_workspace_name();
        let current_sort = app
            .session_manager
            .workspace_sort_order(app.session_manager.current_workspace_id());
        if let Some(existing) = by_key.get_mut(&current_path) {
            existing.last_opened_at = i64::MAX;
            existing.name = current_name;
            existing.sort_order = current_sort;
        } else {
            insert_remote_workspace(
                &mut by_key,
                RemoteWorkspace {
                    name: current_name,
                    path: current_path,
                    sort_order: current_sort,
                    last_opened_at: i64::MAX,
                },
            );
        }
    }

    let mut workspaces = by_key.into_values().collect::<Vec<_>>();
    workspaces.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    workspaces
}

fn insert_remote_workspace(
    by_key: &mut HashMap<String, RemoteWorkspace>,
    workspace: RemoteWorkspace,
) {
    let key = if workspace.path.trim().is_empty() {
        workspace.name.clone()
    } else {
        workspace.path.clone()
    };

    if key.trim().is_empty() {
        return;
    }

    by_key.entry(key).or_insert(workspace);
}

fn remote_session(session: SessionInfo) -> RemoteSession {
    RemoteSession {
        id: session.id,
        parent_id: session.parent_id,
        title: session.title,
        workspace: session.workspace_name,
        workspace_path: session.workspace_path,
        status: session.status.as_str().to_string(),
        message_count: session.message_count,
        updated_at: system_time_to_unix_secs(session.updated_at),
    }
}

fn active_messages_for_session(app: &App, session_id: &str) -> Vec<RemoteMessage> {
    if app
        .session_manager
        .get_current_session_id()
        .is_some_and(|current| current == session_id)
    {
        return app
            .chat_state
            .chat
            .messages
            .iter()
            .map(remote_message)
            .collect();
    }

    app.session_manager
        .get_session_ref(session_id)
        .map(|session| session.messages.iter().map(remote_message).collect())
        .unwrap_or_default()
}

fn remote_message(message: &Message) -> RemoteMessage {
    RemoteMessage {
        role: match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        }
        .to_string(),
        content: message.content.clone(),
        reasoning: message.reasoning.clone(),
        is_complete: message.is_complete,
        agent_mode: message.agent_mode.clone(),
        token_count: message.token_count,
        duration_ms: message.duration_ms,
        t0_ms: message.t0_ms,
        t1_ms: message.t1_ms,
        tn_ms: message.tn_ms,
        output_tokens: message.output_tokens,
        model: message.model.clone(),
        provider: message.provider.clone(),
        local_image_paths: message.local_image_paths.clone(),
        was_interrupted: message.was_interrupted,
        parts: message.parts.clone(),
    }
}

fn remote_permission_prompt(app: &App) -> Option<RemotePermissionPrompt> {
    app.permission_dialog_state
        .current_snapshot()
        .map(|prompt| RemotePermissionPrompt {
            tool_id: prompt.tool_id,
            action: prompt.action,
            permission: prompt.permission,
            patterns: prompt.patterns,
            target: prompt.target,
            command: prompt.command,
            workdir: prompt.workdir,
            reason: prompt.reason,
            queued_count: prompt.queued_count,
        })
}

fn remote_question_prompt(app: &App) -> Option<RemoteQuestionPrompt> {
    app.question_dialog_state
        .current_snapshot()
        .map(|prompt| RemoteQuestionPrompt {
            questions: prompt
                .questions
                .into_iter()
                .map(|question| RemoteQuestionItem {
                    header: question.header,
                    question: question.question,
                    options: question
                        .options
                        .into_iter()
                        .map(|option| RemoteQuestionOption {
                            label: option.label,
                            description: option.description,
                        })
                        .collect(),
                    multiple: question.multiple,
                    custom: question.custom,
                })
                .collect(),
            queued_count: prompt.queued_count,
        })
}

async fn connect_host(client: &reqwest::Client, target: &str) -> Result<ConnectedHost> {
    let mut hosts = load_hosts()?;
    let (url, stored_token, requested_alias) = resolve_host_target(&hosts, target)?;
    let status: RemoteStatus = get_public_json(client, &url, "/api/status").await?;

    if !status.auth_required {
        let alias = if looks_like_url(target) {
            status.suggested_alias.clone()
        } else {
            requested_alias.clone()
        };
        let host = ConnectedHost {
            alias,
            url,
            token: stored_token.unwrap_or_default(),
            status,
        };
        remember_host(&mut hosts, &host)?;
        return Ok(host);
    }

    if let Some(token) = stored_token {
        let host = ConnectedHost {
            alias: requested_alias.clone(),
            url: url.clone(),
            token: token.clone(),
            status: status.clone(),
        };
        if get_remote_state(client, &host).await.is_ok() {
            remember_host(&mut hosts, &host)?;
            return Ok(host);
        }
    }

    let code = read_pair_code(&status)?;
    let pair: PairResponse = post_public_json(
        client,
        &url,
        "/api/pair",
        &PairRequest {
            code,
            client_name: Some(local_client_name()),
            role: Some("cli".to_string()),
        },
    )
    .await?;

    let alias = if requested_alias == target && !looks_like_url(target) {
        requested_alias
    } else {
        pair.suggested_alias.clone()
    };

    let host = ConnectedHost {
        alias,
        url,
        token: pair.token,
        status,
    };
    remember_host(&mut hosts, &host)?;
    Ok(host)
}

fn resolve_host_target(
    hosts: &RemoteHostsFile,
    target: &str,
) -> Result<(String, Option<String>, String)> {
    if looks_like_url(target) {
        return Ok((normalize_base_url(target), None, alias_from_url(target)));
    }

    let Some(host) = hosts.hosts.iter().find(|host| host.alias == target) else {
        bail!("unknown remote host alias: {target}");
    };

    Ok((
        normalize_base_url(&host.url),
        Some(host.token.clone()),
        host.alias.clone(),
    ))
}

fn looks_like_url(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

async fn stream_remote_prompt(
    client: &reqwest::Client,
    host: &ConnectedHost,
    prompt: &str,
) -> Result<()> {
    let _: PromptResponse = post_json(
        client,
        &host.url,
        "/api/prompt",
        &host.token,
        &PromptRequest {
            prompt: prompt.to_string(),
            images: Vec::new(),
        },
    )
    .await?;

    let mut printed = String::new();
    let mut saw_assistant = false;

    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let state = get_remote_state(client, host).await?;
        let assistant = state
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant");

        if let Some(message) = assistant {
            saw_assistant = true;
            if message.content.starts_with(&printed) {
                let delta = &message.content[printed.len()..];
                print!("{delta}");
            } else {
                println!("\n{}", message.content);
            }
            io::stdout().flush()?;
            printed = message.content.clone();
        }

        if !state.is_streaming {
            break;
        }
    }

    if saw_assistant {
        println!();
    }

    Ok(())
}

async fn get_remote_state(client: &reqwest::Client, host: &ConnectedHost) -> Result<RemoteState> {
    get_json(client, &host.url, "/api/state", &host.token).await
}

async fn get_public_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
) -> Result<T> {
    let response = client
        .get(api_url(base_url, path)?)
        .send()
        .await
        .with_context(|| format!("failed to connect to {base_url}"))?;
    parse_response(response).await
}

async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    token: &str,
) -> Result<T> {
    let response = client
        .get(api_url(base_url, path)?)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("failed to connect to {base_url}"))?;
    parse_response(response).await
}

async fn post_public_json<T: Serialize, R: DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    payload: &T,
) -> Result<R> {
    let response = client
        .post(api_url(base_url, path)?)
        .json(payload)
        .send()
        .await
        .with_context(|| format!("failed to connect to {base_url}"))?;
    parse_response(response).await
}

async fn post_json<T: Serialize, R: DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    token: &str,
    payload: &T,
) -> Result<R> {
    let response = client
        .post(api_url(base_url, path)?)
        .bearer_auth(token)
        .json(payload)
        .send()
        .await
        .with_context(|| format!("failed to connect to {base_url}"))?;
    parse_response(response).await
}

async fn parse_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        bail!("pairing required or token was rejected");
    }
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read response body>".to_string());
        bail!("remote host returned {status}: {body}");
    }
    response
        .json::<T>()
        .await
        .context("invalid remote response")
}

fn api_url(base_url: &str, path: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(base_url).context("invalid remote host URL")?;
    url.set_path(path);
    Ok(url.to_string())
}

fn read_pair_code(status: &RemoteStatus) -> Result<String> {
    if !io::stdin().is_terminal() {
        bail!("pairing required, but stdin is not interactive");
    }

    eprintln!(
        "Pairing required for {} ({})",
        status.browser_url, status.workspace
    );
    eprint!("Pair code: ");
    io::stderr().flush()?;

    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    Ok(code.trim().to_string())
}

fn load_hosts() -> Result<RemoteHostsFile> {
    let path = hosts_path();
    if !path.exists() {
        return Ok(RemoteHostsFile::default());
    }

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_hosts(hosts: &RemoteHostsFile) -> Result<()> {
    crate::persistence::ensure_data_dir()?;
    let path = hosts_path();
    let contents = serde_json::to_string_pretty(hosts)?;
    std::fs::write(&path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    restrict_hosts_file_permissions(&path)?;
    Ok(())
}

fn remember_host(hosts: &mut RemoteHostsFile, host: &ConnectedHost) -> Result<()> {
    let now = now_unix_secs();
    if let Some(entry) = hosts
        .hosts
        .iter_mut()
        .find(|entry| entry.alias == host.alias || entry.url == host.url)
    {
        entry.alias = host.alias.clone();
        entry.url = host.url.clone();
        entry.token = host.token.clone();
        entry.workspace_label = host.status.workspace.clone();
        entry.last_used_at = now;
    } else {
        hosts.hosts.push(RemoteHostEntry {
            alias: host.alias.clone(),
            url: host.url.clone(),
            token: host.token.clone(),
            workspace_label: host.status.workspace.clone(),
            last_used_at: now,
        });
    }

    hosts.hosts.sort_by(|a, b| a.alias.cmp(&b.alias));
    save_hosts(hosts)
}

fn hosts_path() -> std::path::PathBuf {
    crate::persistence::get_data_dir().join(HOSTS_FILE)
}

#[cfg(unix)]
fn restrict_hosts_file_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_hosts_file_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

fn print_host_ready(app: &App, local_addr: SocketAddr, host_state: &HostState) {
    let url = &host_state.browser_url;
    let network_ip = local_network_ip();
    let qr_url = scan_url_for_addr(local_addr, network_ip);
    println!("crabcode host ready");
    println!();
    println!("Workspace: {}", app.cwd);
    println!("Browser:   {url}");
    print_phone_access_hint(local_addr, network_ip);
    println!("Attach:    crabcode attach {url}");
    println!("Prompt:    crabcode -p --attach {url} \"...\"");
    if let Some(pair_code) = host_state.pair_code.as_deref() {
        println!(
            "Pair:      {}  expires in {} minutes",
            pair_code,
            DEFAULT_PAIR_TTL_SECS / 60
        );
    } else {
        println!("Pair:      None (insecure, use --paircode <code>)");
    }
    println!();
    println!("Press Ctrl-C to stop the host.");
    if io::stdout().is_terminal() {
        println!();
        match qr_url {
            Some(qr_url) => match terminal_qr_code(&qr_url) {
                Ok(qr) => {
                    println!("QR:        {qr_url}");
                    print!("{qr}");
                }
                Err(err) => {
                    println!("QR:        unavailable ({err})");
                }
            },
            None => {
                println!(
                    "QR:        run crabcode serve --bind 0.0.0.0:{}",
                    local_addr.port()
                );
            }
        }
    }
}

fn terminal_qr_code(value: &str) -> Result<String> {
    let code = qrcode::QrCode::with_error_correction_level(value.as_bytes(), qrcode::EcLevel::L)
        .map_err(|err| anyhow!("failed to encode QR code: {err}"))?;

    Ok(render_terminal_qr_code(&code, 0))
}

fn render_terminal_qr_code(code: &qrcode::QrCode, quiet_zone: usize) -> String {
    const BLACK_ON_WHITE: &str = "\x1b[30;47m";
    const BLACK_BG: &str = "\x1b[40m";
    const WHITE_BG: &str = "\x1b[47m";
    const RESET: &str = "\x1b[0m";

    let qr_width = code.width();
    let width = qr_width + quiet_zone * 2;
    let height = width;
    let mut output = String::new();

    for y in (0..height).step_by(2) {
        for x in 0..width {
            let top = terminal_qr_module_is_dark(code, x, y, quiet_zone);
            let bottom = y + 1 < height && terminal_qr_module_is_dark(code, x, y + 1, quiet_zone);

            match (top, bottom) {
                (true, true) => output.push_str(BLACK_BG),
                (false, false) => output.push_str(WHITE_BG),
                (true, false) => {
                    output.push_str(BLACK_ON_WHITE);
                    output.push('▀');
                    continue;
                }
                (false, true) => {
                    output.push_str(BLACK_ON_WHITE);
                    output.push('▄');
                    continue;
                }
            }
            output.push(' ');
        }
        output.push_str(RESET);
        output.push('\n');
    }

    output
}

fn terminal_qr_module_is_dark(
    code: &qrcode::QrCode,
    x: usize,
    y: usize,
    quiet_zone: usize,
) -> bool {
    let qr_width = code.width();
    if x < quiet_zone || y < quiet_zone {
        return false;
    }

    let qr_x = x - quiet_zone;
    let qr_y = y - quiet_zone;
    if qr_x >= qr_width || qr_y >= qr_width {
        return false;
    }

    code[(qr_x, qr_y)] == qrcode::types::Color::Dark
}

fn print_phone_access_hint(local_addr: SocketAddr, network_ip: Option<IpAddr>) {
    let ip = local_addr.ip();
    let port = local_addr.port();

    if ip.is_loopback() {
        println!("Phone:     run crabcode serve --bind 0.0.0.0:{port}");
    } else if ip.is_unspecified() {
        if let Some(network_ip) = network_ip {
            println!("Phone:     {}", http_url_for_ip(network_ip, port));
        } else {
            println!("Phone:     use this host's LAN or tailnet address");
        }
    } else {
        println!("Phone:     {}", http_url_for_ip(ip, port));
    }
}

fn scan_url_for_addr(addr: SocketAddr, network_ip: Option<IpAddr>) -> Option<String> {
    if addr.ip().is_loopback() {
        return None;
    }

    if addr.ip().is_unspecified() {
        if let Some(network_ip) = network_ip {
            return Some(http_url_for_ip(network_ip, addr.port()));
        }
        return None;
    }

    Some(browser_url_for_addr(addr))
}

fn local_network_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

fn browser_url_for_addr(addr: SocketAddr) -> String {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => format!("http://127.0.0.1:{}", addr.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => format!("http://[::1]:{}", addr.port()),
        ip => http_url_for_ip(ip, addr.port()),
    }
}

fn http_url_for_ip(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("http://{ip}:{port}"),
        IpAddr::V6(ip) => format!("http://[{ip}]:{port}"),
    }
}

fn suggested_alias_for_cwd() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "devbox".to_string())
}

fn alias_from_url(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "remote".to_string())
}

fn local_client_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "crabcode-cli".to_string())
}

fn is_disconnect_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|io_err| {
            matches!(
                io_err.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::UnexpectedEof
            )
        })
    })
}

fn resolve_pair_code_arg(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };

    let value = value.trim();
    if value.is_empty() {
        bail!("--paircode must be a non-empty code or \"random\"");
    }
    if value.eq_ignore_ascii_case("random") {
        return Ok(Some(generate_pair_code()));
    }

    Ok(Some(value.to_string()))
}

fn generate_pair_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!(
        "{:03}-{:03}",
        rng.gen_range(0..1000),
        rng.gen_range(0..1000)
    )
}

fn normalize_pair_code(code: &str) -> String {
    code.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

fn pair_codes_match(expected: &str, actual: &str) -> bool {
    let expected = expected.trim();
    let actual = actual.trim();

    if expected == actual {
        return true;
    }

    if pair_code_uses_numeric_format(expected) && pair_code_uses_numeric_format(actual) {
        return normalize_pair_code(expected) == normalize_pair_code(actual);
    }

    false
}

fn pair_code_uses_numeric_format(code: &str) -> bool {
    let mut has_digit = false;
    let mut has_invalid = false;

    for ch in code.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if !(ch == '-' || ch.is_ascii_whitespace()) {
            has_invalid = true;
            break;
        }
    }

    has_digit && !has_invalid
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn system_time_to_unix_secs(value: SystemTime) -> i64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn format_timestamp(value: i64) -> String {
    if value <= 0 {
        return "never".to_string();
    }

    let age = now_unix_secs().saturating_sub(value);
    if age < 60 {
        "just now".to_string()
    } else if age < 60 * 60 {
        format!("{}m ago", age / 60)
    } else if age < 24 * 60 * 60 {
        format!("{}h ago", age / (60 * 60))
    } else {
        format!("{}d ago", age / (24 * 60 * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_pair_codes_to_digits() {
        assert_eq!(normalize_pair_code("482-119"), "482119");
        assert_eq!(normalize_pair_code("482 119"), "482119");
    }

    #[test]
    fn pair_codes_match_numeric_forms() {
        assert!(pair_codes_match("482-119", "482119"));
        assert!(pair_codes_match("482 119", "482-119"));
        assert!(!pair_codes_match("abc123", "123"));
    }

    #[test]
    fn omitted_pair_code_disables_auth() {
        let host = HostState::new(
            "http://127.0.0.1:8421".to_string(),
            "crabcode".to_string(),
            None,
        )
        .unwrap();

        assert!(!host.auth_required());
        assert!(host.accepts_pair_code(""));
        assert!(host.accepts_token(""));
    }

    #[test]
    fn random_pair_code_enables_auth() {
        let host = HostState::new(
            "http://127.0.0.1:8421".to_string(),
            "crabcode".to_string(),
            Some("random".to_string()),
        )
        .unwrap();

        assert!(host.auth_required());
        assert!(host
            .pair_code
            .as_deref()
            .is_some_and(|code| !code.is_empty()));
        assert!(!host.accepts_token(""));
    }

    #[tokio::test]
    async fn remote_status_exposes_visible_primary_agents() {
        let mut app = App::new_with_model_override(None, None).unwrap();
        let mut warnings = Vec::new();
        let defs = crate::agent::definition::parse_agent_definitions_from_config(
            Some(&serde_json::json!({
                "designer": {
                    "description": "Design UI",
                    "mode": "all"
                },
                "scout-only": {
                    "description": "Read-only scout",
                    "mode": "subagent"
                }
            })),
            &mut warnings,
        );
        app.agent_registry = crate::agent::definition::AgentRegistry::with_definitions(None, defs);
        let host = HostState::new(
            "http://127.0.0.1:8421".to_string(),
            "crabcode".to_string(),
            None,
        )
        .unwrap();

        let status = remote_status(&app, &host);

        assert!(warnings.is_empty());
        assert_eq!(status.primary_agents, vec!["Build", "Designer", "Plan"]);
    }

    #[test]
    fn detects_urls() {
        assert!(looks_like_url("http://127.0.0.1:8421"));
        assert!(looks_like_url("https://devbox.example"));
        assert!(!looks_like_url("devbox"));
    }

    #[test]
    fn terminal_qr_renders_ansi_blocks_for_url() {
        let qr = terminal_qr_code("http://127.0.0.1:8421").unwrap();

        assert!(qr.contains("\x1b[47m"));
        assert!(qr.contains("\x1b[40m"));
        assert!(qr.contains('▀') || qr.contains('▄'));
        assert!(qr.lines().count() >= 10);
    }

    #[test]
    fn scan_url_omits_loopback_addr() {
        let addr: SocketAddr = "127.0.0.1:8421".parse().unwrap();

        assert_eq!(scan_url_for_addr(addr, None), None);
    }

    #[test]
    fn scan_url_uses_network_ip_for_unspecified_addr() {
        let addr: SocketAddr = "0.0.0.0:8421".parse().unwrap();
        let network_ip: IpAddr = "192.168.1.20".parse().unwrap();

        assert_eq!(
            scan_url_for_addr(addr, Some(network_ip)).as_deref(),
            Some("http://192.168.1.20:8421")
        );
    }

    #[test]
    fn ctrl_c_detaches_terminal_client() {
        let event = RemoteTerminalEvent::Key {
            code: RemoteKeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL.bits(),
            kind: RemoteKeyKind::Press,
        };

        assert!(event.detaches_client());
    }

    #[test]
    fn plain_c_is_forwarded_to_host() {
        let event = RemoteTerminalEvent::Key {
            code: RemoteKeyCode::Char('c'),
            modifiers: KeyModifiers::NONE.bits(),
            kind: RemoteKeyKind::Press,
        };

        assert!(!event.detaches_client());
    }

    #[test]
    fn saves_remote_prompt_image_data_url_to_temp_file() {
        let image = PromptImageRequest {
            name: "pixel.png".to_string(),
            media_type: "image/png".to_string(),
            data_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="
                .to_string(),
        };

        let paths = remote_prompt_image_paths(&[image]).unwrap();

        assert_eq!(paths.len(), 1);
        assert!(crate::utils::image_attachment::is_supported_image_path(
            &paths[0]
        ));
        assert_eq!(
            crate::utils::image_attachment::mime_type_for_path(&paths[0]),
            "image/png"
        );
    }

    #[test]
    fn remote_message_includes_local_image_paths() {
        let mut message = Message::user("see [Image #1]");
        message.local_image_paths = vec!["/tmp/example.png".to_string()];

        let remote = remote_message(&message);

        assert_eq!(remote.local_image_paths, vec!["/tmp/example.png"]);
    }

    #[test]
    fn extracts_project_favicon_href_from_link_tag() {
        let source = r#"<html><head><link href="/brand/icon.svg?v=2" rel="icon"></head></html>"#;

        assert_eq!(
            extract_icon_href(source).as_deref(),
            Some("/brand/icon.svg")
        );
    }

    #[test]
    fn extracts_project_favicon_href_from_icon_metadata() {
        let source =
            r#"export const links = [{ href: "/app/icon.png?hash=1", rel: "shortcut icon" }]"#;

        assert_eq!(extract_icon_href(source).as_deref(), Some("/app/icon.png"));
    }

    #[test]
    fn resolves_project_favicon_candidate_before_declared_icon() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("public")).unwrap();
        std::fs::write(temp.path().join("favicon.svg"), "<svg />").unwrap();
        std::fs::write(temp.path().join("public").join("favicon.png"), []).unwrap();
        std::fs::write(
            temp.path().join("index.html"),
            r#"<link rel="icon" href="/declared.svg">"#,
        )
        .unwrap();

        assert_eq!(
            resolve_project_favicon_path(temp.path().to_str().unwrap()).as_deref(),
            Some(temp.path().join("favicon.svg").as_path())
        );
    }

    #[test]
    fn resolves_project_favicon_from_declared_icon() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("public").join("brand")).unwrap();
        std::fs::write(
            temp.path().join("index.html"),
            r#"<link href="/brand/icon.svg?hash=1" rel="icon">"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("public").join("brand").join("icon.svg"),
            "<svg />",
        )
        .unwrap();

        assert_eq!(
            resolve_project_favicon_path(temp.path().to_str().unwrap()).as_deref(),
            Some(
                temp.path()
                    .join("public")
                    .join("brand")
                    .join("icon.svg")
                    .as_path()
            )
        );
    }

    #[test]
    fn resolves_project_favicon_from_package_workspace() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("apps").join("landing").join("public")).unwrap();
        std::fs::create_dir_all(temp.path().join("packages").join("shared")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"workspaces":["apps/*","packages/*"]}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path()
                .join("apps")
                .join("landing")
                .join("public")
                .join("favicon.png"),
            [],
        )
        .unwrap();

        assert_eq!(
            resolve_project_favicon_path(temp.path().to_str().unwrap()).as_deref(),
            Some(
                temp.path()
                    .join("apps")
                    .join("landing")
                    .join("public")
                    .join("favicon.png")
                    .as_path()
            )
        );
    }

    #[test]
    fn resolves_project_favicon_from_package_workspace_object() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("sites").join("web").join("public")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"workspaces":{"packages":["sites/*"]}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path()
                .join("sites")
                .join("web")
                .join("public")
                .join("favicon.svg"),
            "<svg />",
        )
        .unwrap();

        assert_eq!(
            resolve_project_favicon_path(temp.path().to_str().unwrap()).as_deref(),
            Some(
                temp.path()
                    .join("sites")
                    .join("web")
                    .join("public")
                    .join("favicon.svg")
                    .as_path()
            )
        );
    }

    #[test]
    fn direct_project_favicon_takes_precedence_over_workspace_favicon() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("apps").join("landing").join("public")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"workspaces":["apps/*"]}"#,
        )
        .unwrap();
        std::fs::write(temp.path().join("favicon.svg"), "<svg />").unwrap();
        std::fs::write(
            temp.path()
                .join("apps")
                .join("landing")
                .join("public")
                .join("favicon.png"),
            [],
        )
        .unwrap();

        assert_eq!(
            resolve_project_favicon_path(temp.path().to_str().unwrap()).as_deref(),
            Some(temp.path().join("favicon.svg").as_path())
        );
    }

    #[test]
    fn rejects_project_favicon_paths_outside_project() {
        let temp = tempfile::tempdir().unwrap();
        let outside_candidate = temp.path().join("..").join("secret.svg");

        assert!(!is_path_within_project(temp.path(), &outside_candidate));
    }
}
