#![allow(dead_code)]

mod acp;
mod agent;
mod aisdk;
mod app;
mod auth;
mod autocomplete;
mod command;
mod config;
mod llm;
mod logging;
mod mcp;
mod model;
mod notify;
mod persistence;
mod prompt;
mod remote;
mod remote_mcp;
mod session;
mod skill;
mod sound;
mod streaming;
mod terminal_title;
mod theme;
mod toast;
mod tools;
mod ui;
mod upgrade;
mod utils;
mod version;
mod views;

mod chunk {
    pub use crate::aisdk::chunk::*;
}

mod error {
    pub use crate::aisdk::error::*;
}

mod message {
    pub use crate::aisdk::message::*;
}

mod provider {
    pub use crate::aisdk::provider::*;
}

mod retry {
    pub use crate::aisdk::retry::*;
}

mod stop {
    pub use crate::aisdk::stop::*;
}

mod tool {
    pub use crate::aisdk::tool::*;
}

use crate::toast::{Toast, ToastManager};
use anyhow::{Context, Result};
use app::App;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, shells};
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, MouseButton,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, buffer::Buffer, style::Color, Terminal};
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::Mutex;
use std::time::Duration;

const POST_CLOSE_LOGO: &str = include_str!("../crabcode-logo.txt");
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";
const EVENT_DRAIN_LIMIT: usize = 256;

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

fn handle_terminal_event(app: &mut App, event: event::Event) {
    match event {
        event::Event::Mouse(mouse) => app.handle_mouse_event(mouse),
        event::Event::Key(key) => {
            app.handle_keys(key);
            if app.take_just_closed_overlay() {
                drain_pending_terminal_events(Duration::from_millis(12));
            }
        }
        event::Event::Paste(text) => {
            app.handle_paste(text);
        }
        event::Event::FocusGained => {
            app.set_terminal_focused(true);
        }
        event::Event::FocusLost => {
            app.set_terminal_focused(false);
        }
        event::Event::Resize(_, _) => {}
    }
}

fn mouse_scroll_kind(kind: MouseEventKind) -> bool {
    matches!(kind, MouseEventKind::ScrollDown | MouseEventKind::ScrollUp)
}

fn restore_terminal_modes(
    backend: &mut CrosstermBackend<io::Stdout>,
    keyboard_enhancement: bool,
) -> Result<()> {
    drain_pending_terminal_events(Duration::from_millis(0));

    let restore_result = if keyboard_enhancement {
        execute!(
            backend,
            DisableMouseCapture,
            DisableFocusChange,
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            LeaveAlternateScreen
        )
    } else {
        execute!(
            backend,
            DisableMouseCapture,
            DisableFocusChange,
            DisableBracketedPaste,
            LeaveAlternateScreen
        )
    };
    let flush_result = backend.flush();

    drain_pending_terminal_events(Duration::from_millis(25));
    let raw_mode_result = disable_raw_mode();

    restore_result.context("failed to restore terminal modes")?;
    flush_result.context("failed to flush terminal restore commands")?;
    raw_mode_result.context("failed to disable raw mode")?;

    Ok(())
}

fn send_test_notification() -> Result<()> {
    let loaded_config = crate::config::ConfigLoader::load()?;
    let event = crate::sound::SoundEvent::Complete;

    let (sounds, warnings) =
        crate::sound::resolve_effective_sounds(&loaded_config.merged_config.notifications);
    for warning in warnings {
        eprintln!("{warning}");
    }

    if let Some(path) = sounds.path_for_event(event) {
        eprintln!("playing configured sound: {}", path.display());
        crate::sound::play_file(path);
    } else {
        eprintln!("no sound configured for complete notifications");
    }

    crate::notify::notify_event_with_options(
        event,
        Some("local app icon test"),
        crate::notify::NotificationOptions {
            workspace_name: loaded_config
                .cwd
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string),

            #[cfg(target_os = "macos")]
            macos_backend: loaded_config.merged_config.notifications.macos_backend,
        },
    );

    Ok(())
}

pub fn push_startup_diag(msg: String) {
    if crate::logging::enabled() {
        let _ = crate::logging::log(&msg);
    }
}

#[macro_export]
macro_rules! startup_diag {
    ($($arg:tt)*) => {
        $crate::push_startup_diag(format!($($arg)*))
    };
}

struct PostCloseInfo {
    session_id: String,
    session_title: String,
}

fn ansi_fg(color: Color) -> String {
    match color {
        Color::Black => "\x1b[30m".to_string(),
        Color::Red => "\x1b[31m".to_string(),
        Color::Green => "\x1b[32m".to_string(),
        Color::Yellow => "\x1b[33m".to_string(),
        Color::Blue => "\x1b[34m".to_string(),
        Color::Magenta => "\x1b[35m".to_string(),
        Color::Cyan => "\x1b[36m".to_string(),
        Color::Gray => "\x1b[37m".to_string(),
        Color::DarkGray => "\x1b[90m".to_string(),
        Color::LightRed => "\x1b[91m".to_string(),
        Color::LightGreen => "\x1b[92m".to_string(),
        Color::LightYellow => "\x1b[93m".to_string(),
        Color::LightBlue => "\x1b[94m".to_string(),
        Color::LightMagenta => "\x1b[95m".to_string(),
        Color::LightCyan => "\x1b[96m".to_string(),
        Color::White => "\x1b[97m".to_string(),
        Color::Indexed(index) => format!("\x1b[38;5;{}m", index),
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{};{};{}m", r, g, b),
        Color::Reset => String::new(),
    }
}

fn push_styled_line(msg: &mut String, line: &str, style: &str) {
    msg.push_str(style);
    msg.push_str(line);
    msg.push_str(ANSI_RESET);
    msg.push('\n');
}

fn format_post_close_message(
    info: Option<&PostCloseInfo>,
    colors: &crate::theme::ThemeColors,
) -> String {
    let mut msg = String::new();
    let logo_primary = ansi_fg(colors.primary);
    let logo_bottom = ansi_fg(crate::theme::darken_color(colors.primary, 0.7));
    let label_color = ansi_fg(colors.text_weak);
    let value_color = ansi_fg(colors.text);

    for (i, line) in POST_CLOSE_LOGO.lines().enumerate() {
        let logo_color = if i == 2 { &logo_bottom } else { &logo_primary };
        push_styled_line(&mut msg, line, logo_color);
    }

    if let Some(info) = info {
        msg.push('\n');
        msg.push_str(&format!(
            "  {dim}{label_color}{:<10}{reset}{value_color}{}{reset}\n",
            "Session",
            info.session_title,
            dim = ANSI_DIM,
            label_color = label_color,
            value_color = value_color,
            reset = ANSI_RESET,
        ));
        msg.push_str(&format!(
            "  {dim}{label_color}{:<10}{reset}{value_color}crabcode -s {}{reset}\n",
            "Continue",
            info.session_id,
            dim = ANSI_DIM,
            label_color = label_color,
            value_color = value_color,
            reset = ANSI_RESET,
        ));
    }

    msg
}

async fn run_print_mode(
    prompt: &str,
    model_override: Option<&str>,
    reasoning_override: Option<crate::model::reasoning::ReasoningEffort>,
    no_session_persistence: bool,
    dangerously_skip_permissions: bool,
) -> Result<()> {
    use crate::llm::client::stream_llm_with_cancellation;
    use crate::session::types::Message;
    use tokio::sync::mpsc;

    // Load config and model preferences
    let loaded_config = crate::config::ConfigLoader::load()?;
    let (sounds, notification_warnings) =
        crate::sound::resolve_effective_sounds(&loaded_config.merged_config.notifications);
    for warning in notification_warnings {
        eprintln!("Notification warning: {warning}");
    }
    crate::skill::init_skill_store(&loaded_config.xdg_config_home, &loaded_config.project_root);
    let prefs_dao = crate::persistence::PrefsDAO::new().ok();

    let (provider_name, model_id) = {
        let active = prefs_dao
            .as_ref()
            .and_then(|d| d.get_active_model().ok().flatten());
        if let Some(model) = model_override {
            let (pid, mid) = crate::app::parse_model_ref(model);
            (pid, mid)
        } else if let Some((pid, mid)) = active {
            (pid, mid)
        } else if let Some(m) = loaded_config.merged_config.model.clone() {
            let (pid, mid) = crate::app::parse_model_ref(&m);
            (pid, mid)
        } else {
            ("opencode".to_string(), "big-pickle".to_string())
        }
    };

    let agent_mode = loaded_config
        .merged_config
        .default_agent
        .clone()
        .unwrap_or_else(|| "Build".to_string());

    let saved_reasoning = prefs_dao
        .as_ref()
        .and_then(|dao| {
            dao.get_model_reasoning_effort(&provider_name, &model_id)
                .ok()
        })
        .flatten();
    let requested_reasoning = reasoning_override.or(saved_reasoning);
    let discovery = crate::model::discovery::Discovery::new().ok();
    let reasoning_effort = discovery
        .as_ref()
        .and_then(|discovery| discovery.get_model_reasoning_capability(&provider_name, &model_id))
        .and_then(|capability| {
            let resolved = capability.resolve(requested_reasoning)?;
            if resolved == crate::model::reasoning::ReasoningEffort::None {
                None
            } else {
                Some(resolved)
            }
        });

    let cwd = loaded_config.cwd.to_string_lossy().to_string();

    let is_git_repo = crate::utils::git::is_git_repo(&cwd).unwrap_or(false);

    let (sender, mut receiver) = mpsc::unbounded_channel();

    let agent_registry = loaded_config.merged_config.agent_registry.clone();
    let websearch_config = loaded_config.merged_config.websearch.clone();
    let mcp_config = loaded_config.merged_config.mcp.clone();
    let mut agent_policies = crate::tools::AgentToolPolicies::default();
    for (mode, tools) in agent_registry.tool_policy_map() {
        agent_policies = agent_policies.with_custom_tools(mode, tools);
    }
    let permission_rules =
        print_mode_permission_rules(loaded_config.merged_config.permission_rules.clone());
    let tool_permissions = crate::tools::ToolPermissions::new(std::path::PathBuf::from(&cwd))
        .with_agent_policies(agent_policies)
        .with_permission_rules(permission_rules)
        .with_agent_permission_rules(agent_registry.permission_rules_map())
        .dangerously_skip_permissions(dangerously_skip_permissions);
    let agent_max_steps = agent_registry
        .get(&agent_mode)
        .and_then(|agent| agent.max_steps);
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let prompt_registry = crate::tools::initialize_tool_registry_with_dynamic_config(
        Some(sender.clone()),
        tool_permissions.clone(),
        agent_registry.clone(),
        cancel_token.clone(),
        Some(&provider_name),
        &websearch_config,
        &mcp_config,
        &cwd,
    )
    .await;
    let prompt_registry = crate::tools::scope_tool_registry_for_agent(
        &prompt_registry,
        &tool_permissions,
        &agent_mode,
    )
    .await;

    // Build messages with system prompt
    let composer = crate::prompt::SystemPromptComposer::new(
        &model_id,
        &cwd,
        is_git_repo,
        std::env::consts::OS,
    )
    .with_tool_registry(prompt_registry.clone())
    .with_agent_registry(agent_registry.clone())
    .with_print_mode(true);
    let system_prompt = composer.compose().await;
    let messages = vec![Message::system(system_prompt), Message::user(prompt)];
    preflight_print_mode_prompt_size(discovery.as_ref(), &provider_name, &model_id, &messages)?;

    let provider_name_clone = provider_name.clone();
    let model_clone = model_id.clone();
    let completion_sender = sender.clone();

    tokio::spawn(async move {
        if let Err(err) = stream_llm_with_cancellation(
            cancel_token,
            cuid2::create_id(),
            provider_name_clone,
            model_clone,
            reasoning_effort,
            agent_mode.clone(),
            agent_max_steps,
            agent_registry,
            tool_permissions,
            websearch_config,
            mcp_config,
            cwd,
            Some(prompt_registry),
            messages,
            sender,
        )
        .await
        {
            let _ = completion_sender.send(crate::llm::ChunkMessage::Failed(err.to_string()));
        }

        let _ = completion_sender.send(crate::llm::ChunkMessage::End);
    });

    while let Some(chunk) = receiver.recv().await {
        match chunk {
            crate::llm::ChunkMessage::Text(text) => {
                print!("{}", text);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            crate::llm::ChunkMessage::ToolCalls(_)
            | crate::llm::ChunkMessage::ToolResult(_)
            | crate::llm::ChunkMessage::Metrics { .. }
            | crate::llm::ChunkMessage::Cancelled
            | crate::llm::ChunkMessage::Reasoning(_)
            | crate::llm::ChunkMessage::Retry(_)
            | crate::llm::ChunkMessage::StreamRollback { .. }
            | crate::llm::ChunkMessage::SubagentStarted { .. }
            | crate::llm::ChunkMessage::SubagentChunk { .. }
            | crate::llm::ChunkMessage::TerminalSessionEvent { .. } => {}
            crate::llm::ChunkMessage::End => {
                println!();
                play_resolved_sound(&sounds, crate::sound::SoundEvent::Complete);
                break;
            }
            crate::llm::ChunkMessage::Failed(error) => {
                play_resolved_sound(&sounds, crate::sound::SoundEvent::Error);
                return Err(anyhow::anyhow!(error));
            }
            crate::llm::ChunkMessage::Warning(warning) => {
                eprintln!("Warning: {}", warning);
            }
            crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                play_resolved_sound(&sounds, crate::sound::SoundEvent::Permission);
                eprintln!(
                    "Permission required: {}. Re-run with --dangerously-skip-permissions to allow non-interactive tool execution.",
                    prompt.reason
                );
                let _ = prompt
                    .response_tx
                    .send(crate::tools::PermissionResponse::Deny);
            }
            crate::llm::ChunkMessage::QuestionRequest { response_tx, .. } => {
                play_resolved_sound(&sounds, crate::sound::SoundEvent::Question);
                let _ = response_tx.send(serde_json::json!({
                    "skipped": true,
                    "reason": "Question prompts are unavailable in non-interactive print mode"
                }));
            }
            crate::llm::ChunkMessage::TerminalSessionRequest(request) => {
                play_resolved_sound(&sounds, crate::sound::SoundEvent::Permission);
                eprintln!(
                    "Terminal session required ({}): interactive PTY is unavailable in non-interactive print mode; stopping session.",
                    request.start.description
                );
                let _ = request
                    .control_tx
                    .send(crate::tools::TerminalSessionControl::Stop);
            }
        }
    }

    let _ = no_session_persistence;
    Ok(())
}

fn play_resolved_sound(
    sounds: &crate::sound::ResolvedSoundsConfig,
    event: crate::sound::SoundEvent,
) {
    if let Some(path) = sounds.path_for_event(event) {
        crate::sound::play_file(path);
    }
}

fn preflight_print_mode_prompt_size(
    discovery: Option<&crate::model::discovery::Discovery>,
    provider_id: &str,
    model_id: &str,
    messages: &[crate::session::types::Message],
) -> Result<()> {
    let Some(context_limit) =
        discovery.and_then(|discovery| discovery.get_model_limit(provider_id, model_id))
    else {
        return Ok(());
    };

    ensure_estimated_prompt_fits_context(
        provider_id,
        model_id,
        estimate_prompt_tokens(messages),
        context_limit,
    )
}

fn ensure_estimated_prompt_fits_context(
    provider_id: &str,
    model_id: &str,
    estimated_tokens: usize,
    context_limit: u32,
) -> Result<()> {
    if context_limit == 0 || estimated_tokens < context_limit as usize {
        return Ok(());
    }

    anyhow::bail!(
        "Prompt is too large for {}/{}: estimated input is {} tokens, model context limit is {} tokens. Reduce the staged diff or choose a larger-context model.",
        provider_id,
        model_id,
        estimated_tokens,
        context_limit
    )
}

fn estimate_prompt_tokens(messages: &[crate::session::types::Message]) -> usize {
    messages
        .iter()
        .map(|message| estimate_text_tokens(&message.content) + 4)
        .sum()
}

fn estimate_text_tokens(content: &str) -> usize {
    content.chars().count().max(1) / 4
}

fn print_mode_permission_rules(
    mut rules: crate::tools::PermissionRules,
) -> crate::tools::PermissionRules {
    for tool_id in ["question", "update_plan"] {
        rules.push(crate::tools::PermissionRule {
            permission: tool_id.to_string(),
            pattern: "*".to_string(),
            action: crate::tools::PermissionPolicyAction::Deny,
        });
    }
    rules
}

fn parse_reasoning_effort_arg(
    value: &str,
) -> Result<crate::model::reasoning::ReasoningEffort, String> {
    value.parse().map_err(|_| {
        "reasoning effort must be one of none, minimal, low, medium, high, xhigh, or max"
            .to_string()
    })
}

lazy_static::lazy_static! {
    static ref TOAST_MANAGER: Mutex<ToastManager> = Mutex::new(ToastManager::new());
}

pub fn push_toast(toast: Toast) {
    TOAST_MANAGER.lock().unwrap().add(toast);
}

pub fn remove_expired_toasts() {
    TOAST_MANAGER.lock().unwrap().remove_expired();
}

pub fn get_toast_manager() -> &'static Mutex<ToastManager> {
    &TOAST_MANAGER
}

#[derive(Parser, Debug)]
#[command(
    author,
    version = crate::version::CURRENT,
    about,
    long_about = None
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Resume a session by ID
    #[arg(short = 's', long = "session")]
    session: Option<String>,

    /// Run in print mode (non-interactive, streams output to stdout)
    #[arg(short = 'p', long = "print")]
    print_mode: bool,

    /// Attach print mode or interactive attach to a remote crabcode host
    #[arg(long = "attach", value_name = "URL_OR_ALIAS")]
    attach: Option<String>,

    /// Do not persist session data to disk
    #[arg(long = "no-session-persistence")]
    no_session_persistence: bool,

    /// Model to use for this invocation, formatted as provider/model
    #[arg(short = 'm', long = "model")]
    model: Option<String>,

    /// Reasoning effort to use for this invocation: none, minimal, low, medium, high, xhigh, or max
    #[arg(long = "reasoning-effort", value_parser = parse_reasoning_effort_arg)]
    reasoning_effort: Option<crate::model::reasoning::ReasoningEffort>,

    /// Skip permission prompts in print mode. Intended for isolated benchmark/CI workspaces.
    #[arg(long = "dangerously-skip-permissions")]
    dangerously_skip_permissions: bool,

    #[arg(long = "emit-logs", hide = true)]
    emit_logs: bool,

    #[arg(long = "test-notification", hide = true)]
    test_notification: bool,

    /// The prompt to run (positional, used in print mode)
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List available models, optionally filtered by provider
    Models {
        /// Exact provider ID to filter by
        provider: Option<String>,
    },

    /// Start an Agent Client Protocol server over stdin/stdout
    Acp {
        /// Working directory used for the initial ACP workspace
        #[arg(long)]
        cwd: Option<PathBuf>,
    },

    /// Generate shell completion script
    Completion,

    /// Host the current workspace for browser and CLI clients
    Serve {
        /// Address to bind, for example 127.0.0.1:8421 or 0.0.0.0:8421
        #[arg(long = "bind", default_value = "127.0.0.1:8421")]
        bind: String,

        /// Require pairing with this code, or use "random" to generate one
        #[arg(long = "paircode", alias = "pair-code", value_name = "CODE_OR_RANDOM")]
        pair_code: Option<String>,
    },

    /// Attach to a remote crabcode host
    Attach {
        /// Host URL or remembered alias
        target: String,
    },

    /// List remembered remote hosts
    Hosts,

    /// Upgrade crabcode to the latest or a specific version
    Upgrade {
        /// Version to install, for example 0.1.0. Defaults to latest.
        target: Option<String>,
    },
}

fn is_completion_help(args: &[String]) -> bool {
    matches!(args, [command, help] if command == "completion" && matches!(help.as_str(), "--help" | "-h"))
}

fn completion_shell(shell: Option<&str>) -> shells::Shell {
    match shell.and_then(|shell| shell.rsplit('/').next()) {
        Some("zsh") => shells::Shell::Zsh,
        _ => shells::Shell::Bash,
    }
}

fn generate_completion(shell: shells::Shell) -> Vec<u8> {
    let mut command = Args::command();
    let mut output = Vec::new();
    generate(shell, &mut command, "crabcode", &mut output);
    output
}

fn root_help() -> Result<String> {
    let mut command = Args::command();
    let mut output = Vec::new();
    command.write_long_help(&mut output)?;
    Ok(String::from_utf8(output).expect("Clap help is valid UTF-8"))
}

fn print_completion() -> Result<()> {
    let shell = completion_shell(std::env::var("SHELL").ok().as_deref());
    io::stdout().write_all(&generate_completion(shell))?;
    Ok(())
}

fn merge_prompt_with_stdin(prompt: &str, stdin: &str) -> String {
    if stdin.trim().is_empty() {
        return prompt.to_string();
    }

    let mut merged = String::with_capacity(prompt.len() + stdin.len() + 24);
    merged.push_str(prompt);
    merged.push_str("\n\n<stdin>\n");
    merged.push_str(stdin);
    if !stdin.ends_with('\n') {
        merged.push('\n');
    }
    merged.push_str("</stdin>");
    merged
}

fn read_print_mode_prompt(prompt: &str) -> Result<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(prompt.to_string());
    }

    let mut stdin_content = Vec::new();
    stdin.read_to_end(&mut stdin_content)?;
    Ok(merge_prompt_with_stdin(
        prompt,
        &String::from_utf8_lossy(&stdin_content),
    ))
}

fn launch_remote_serve(request: app::RemoteLaunchRequest) -> Result<()> {
    let exe = std::env::current_exe().context("failed to locate crabcode executable")?;
    let mut command = ProcessCommand::new(exe);
    command.arg("serve").arg("--bind").arg(request.bind);
    if let Some(pair_code) = request.pair_code {
        command.arg("--paircode").arg(pair_code);
    }

    let status = command.status().context("failed to start crabcode serve")?;
    if !status.success() {
        anyhow::bail!("crabcode serve exited with {}", status);
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if is_completion_help(&raw_args) {
        println!("{}", root_help()?);
        return Ok(());
    }

    let args = Args::parse();
    crate::logging::set_enabled(args.emit_logs);

    if args.test_notification {
        send_test_notification()?;
        return Ok(());
    }

    match &args.command {
        Some(Command::Models { provider }) => {
            let config = crate::config::ConfigLoader::load()?;
            let models =
                crate::model::catalog::selectable_models(&config, provider.as_deref()).await?;
            if models.is_empty() {
                if let Some(provider) = provider {
                    anyhow::bail!("no models found for provider: {provider}");
                }
                anyhow::bail!("no models available");
            }
            for model in models {
                println!("{}", crate::model::catalog::model_ref(&model));
            }
            return Ok(());
        }
        Some(Command::Acp { cwd }) => {
            return crate::acp::run(cwd.clone()).await;
        }
        Some(Command::Completion) => {
            print_completion()?;
            return Ok(());
        }
        Some(Command::Serve { bind, pair_code }) => {
            return crate::remote::serve(crate::remote::ServeOptions {
                bind: bind.clone(),
                model_override: args.model.clone(),
                pair_code: pair_code.clone(),
            })
            .await;
        }
        Some(Command::Attach { target }) => {
            return crate::remote::attach(target).await;
        }
        Some(Command::Hosts) => {
            crate::remote::list_hosts()?;
            return Ok(());
        }
        Some(Command::Upgrade { target }) => {
            return crate::upgrade::upgrade(target.as_deref()).await;
        }
        None => {}
    }

    if let Some(target) = args.attach.as_deref() {
        if args.print_mode {
            let prompt = args.prompt.join(" ");
            if prompt.trim().is_empty() {
                eprintln!("Error: No prompt provided for remote print mode.");
                eprintln!("Usage: crabcode -p --attach <URL_OR_ALIAS> \"<PROMPT>\"");
                std::process::exit(1);
            }
            let prompt = read_print_mode_prompt(&prompt)?;
            return crate::remote::print_attach(target, &prompt).await;
        }

        if !args.prompt.is_empty() {
            eprintln!("Error: --attach with a prompt requires -p.");
            eprintln!("Usage: crabcode -p --attach <URL_OR_ALIAS> \"<PROMPT>\"");
            std::process::exit(1);
        }

        return crate::remote::attach(target).await;
    }

    if args.print_mode {
        let prompt = args.prompt.join(" ");
        if prompt.trim().is_empty() {
            eprintln!("Error: No prompt provided for print mode.");
            eprintln!("Usage: crabcode -p \"<PROMPT>\"");
            std::process::exit(1);
        }
        let prompt = read_print_mode_prompt(&prompt)?;
        return run_print_mode(
            &prompt,
            args.model.as_deref(),
            args.reasoning_effort,
            args.no_session_persistence,
            args.dangerously_skip_permissions,
        )
        .await;
    }

    let mut app = App::new_with_model_override(args.model.as_deref())?;

    if let Some(ref session_id) = args.session {
        if app.session_manager.ensure_session_loaded(session_id) {
            app.session_manager.switch_session(session_id);
            if let Some(session) = app.session_manager.get_session(session_id) {
                app.chat_state.chat.clear();
                let messages = session.messages.clone();
                for message in messages {
                    app.chat_state.chat.add_message(message);
                }
            }
        }
        app.base_focus = app::BaseFocus::Chat;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();

    let keyboard_enhancement = supports_keyboard_enhancement()?;
    if keyboard_enhancement {
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
        )?;
    } else {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange,
            EnableBracketedPaste
        )?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app).await;
    let remote_launch_request = app.take_remote_launch_request();

    let close_info = {
        let session_id = app.session_manager.get_current_session_id().cloned();
        let session_title = app
            .session_manager
            .get_current_session()
            .map(|s| s.title.clone());
        match (session_id, session_title) {
            (Some(session_id), Some(session_title)) => Some(PostCloseInfo {
                session_id,
                session_title,
            }),
            _ => None,
        }
    };

    let post_close_colors = app.get_current_theme_colors();
    app.clear_terminal_title_signal();

    restore_terminal_modes(terminal.backend_mut(), keyboard_enhancement)?;
    terminal.show_cursor()?;

    if let Some(request) = remote_launch_request {
        if let Err(err) = result {
            return Err(err);
        }
        return launch_remote_serve(request);
    }

    print!(
        "{}",
        format_post_close_message(close_info.as_ref(), &post_close_colors)
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_after_print_prompt() {
        let args = Args::try_parse_from([
            "crabcode",
            "-p",
            "hi",
            "--model",
            "opencode-go/deepseek-v4-flash",
        ])
        .unwrap();

        assert_eq!(args.prompt, vec!["hi"]);
        assert_eq!(args.model.as_deref(), Some("opencode-go/deepseek-v4-flash"));
    }

    #[test]
    fn parses_model_with_no_session_persistence_after_print_prompt() {
        let args = Args::try_parse_from([
            "crabcode",
            "-p",
            "hi",
            "--no-session-persistence",
            "--model",
            "opencode-go/kimi-k2.5",
        ])
        .unwrap();

        assert_eq!(args.prompt, vec!["hi"]);
        assert!(args.no_session_persistence);
        assert_eq!(args.model.as_deref(), Some("opencode-go/kimi-k2.5"));
    }

    #[test]
    fn parses_short_model_alias() {
        let args = Args::try_parse_from(["crabcode", "-p", "hi", "-m", "openai/gpt-5.2"]).unwrap();

        assert_eq!(args.prompt, vec!["hi"]);
        assert_eq!(args.model.as_deref(), Some("openai/gpt-5.2"));
    }

    #[test]
    fn parses_print_reasoning_effort_override() {
        let args =
            Args::try_parse_from(["crabcode", "-p", "hi", "--reasoning-effort", "medium"]).unwrap();

        assert_eq!(
            args.reasoning_effort,
            Some(crate::model::reasoning::ReasoningEffort::Medium)
        );
    }

    #[test]
    fn parses_serve_command() {
        let args = Args::try_parse_from(["crabcode", "serve", "--bind", "0.0.0.0:8421"]).unwrap();

        match args.command {
            Some(Command::Serve { bind, pair_code }) => {
                assert_eq!(bind, "0.0.0.0:8421");
                assert_eq!(pair_code.as_deref(), None);
            }
            other => panic!("expected serve command, got {other:?}"),
        }
    }

    #[test]
    fn parses_acp_command_with_workspace() {
        let args = Args::try_parse_from(["crabcode", "acp", "--cwd", "/tmp/workspace"]).unwrap();

        match args.command {
            Some(Command::Acp { cwd }) => assert_eq!(cwd, Some(PathBuf::from("/tmp/workspace"))),
            other => panic!("expected acp command, got {other:?}"),
        }
    }

    #[test]
    fn parses_models_command_with_optional_provider() {
        let args = Args::try_parse_from(["crabcode", "models", "openai"]).unwrap();

        match args.command {
            Some(Command::Models { provider }) => assert_eq!(provider.as_deref(), Some("openai")),
            other => panic!("expected models command, got {other:?}"),
        }

        let args = Args::try_parse_from(["crabcode", "models"]).unwrap();
        match args.command {
            Some(Command::Models { provider }) => assert!(provider.is_none()),
            other => panic!("expected models command, got {other:?}"),
        }
    }

    #[test]
    fn generates_bash_completion() {
        let script =
            String::from_utf8(generate_completion(completion_shell(Some("/bin/bash")))).unwrap();

        assert!(script.contains("_crabcode"));
        assert!(script.contains("complete"));
        assert!(script.contains("crabcode"));
    }

    #[test]
    fn generates_zsh_completion() {
        let script =
            String::from_utf8(generate_completion(completion_shell(Some("/bin/zsh")))).unwrap();

        assert!(script.starts_with("#compdef crabcode"));
        assert!(script.contains("_crabcode"));
    }

    #[test]
    fn completion_help_uses_root_help() {
        assert!(is_completion_help(&[
            "completion".to_string(),
            "--help".to_string()
        ]));
        assert!(is_completion_help(&[
            "completion".to_string(),
            "-h".to_string()
        ]));
        assert!(!is_completion_help(&["completion".to_string()]));
        assert!(!is_completion_help(&[
            "serve".to_string(),
            "--help".to_string()
        ]));

        let help = root_help().unwrap();
        assert!(help.contains("Usage: crabcode"));
        assert!(help.contains("completion  Generate shell completion script"));
        assert!(help.contains("serve       Host the current workspace"));
    }

    #[test]
    fn parses_serve_paircode() {
        let args = Args::try_parse_from(["crabcode", "serve", "--paircode", "random"]).unwrap();

        match args.command {
            Some(Command::Serve { pair_code, .. }) => {
                assert_eq!(pair_code.as_deref(), Some("random"));
            }
            other => panic!("expected serve command, got {other:?}"),
        }
    }

    #[test]
    fn parses_attach_command() {
        let args = Args::try_parse_from(["crabcode", "attach", "http://127.0.0.1:8421"]).unwrap();

        match args.command {
            Some(Command::Attach { target }) => assert_eq!(target, "http://127.0.0.1:8421"),
            other => panic!("expected attach command, got {other:?}"),
        }
    }

    #[test]
    fn parses_upgrade_command() {
        let args = Args::try_parse_from(["crabcode", "upgrade"]).unwrap();

        assert!(matches!(
            args.command,
            Some(Command::Upgrade { target: None })
        ));
    }

    #[test]
    fn parses_upgrade_target() {
        let args = Args::try_parse_from(["crabcode", "upgrade", "0.1.0"]).unwrap();

        match args.command {
            Some(Command::Upgrade { target }) => assert_eq!(target.as_deref(), Some("0.1.0")),
            other => panic!("expected upgrade command, got {other:?}"),
        }
    }

    #[test]
    fn parses_print_attach_flag() {
        let args = Args::try_parse_from([
            "crabcode", "-p", "--attach", "devbox", "continue", "the", "refactor",
        ])
        .unwrap();

        assert!(args.print_mode);
        assert_eq!(args.attach.as_deref(), Some("devbox"));
        assert_eq!(args.prompt, vec!["continue", "the", "refactor"]);
    }

    #[test]
    fn double_dash_keeps_model_like_tokens_in_prompt() {
        let args = Args::try_parse_from([
            "crabcode",
            "-p",
            "hi",
            "--",
            "--model",
            "opencode-go/deepseek-v4-flash",
        ])
        .unwrap();

        assert_eq!(
            args.prompt,
            vec!["hi", "--model", "opencode-go/deepseek-v4-flash"]
        );
        assert_eq!(args.model, None);
    }

    #[test]
    fn merge_prompt_with_stdin_ignores_empty_input() {
        assert_eq!(
            merge_prompt_with_stdin("Generate a commit message.", "\n \t\n"),
            "Generate a commit message."
        );
    }

    #[test]
    fn merge_prompt_with_stdin_wraps_piped_input() {
        assert_eq!(
            merge_prompt_with_stdin("Examine the diff.", "diff --git a/a b/a\n+change"),
            "Examine the diff.\n\n<stdin>\ndiff --git a/a b/a\n+change\n</stdin>"
        );
    }

    #[test]
    fn estimate_prompt_tokens_includes_all_messages() {
        let messages = vec![
            crate::session::types::Message::system("a".repeat(8)),
            crate::session::types::Message::user("b".repeat(4)),
        ];

        assert_eq!(estimate_prompt_tokens(&messages), 11);
    }

    #[test]
    fn prompt_size_preflight_rejects_context_overflow() {
        let err =
            ensure_estimated_prompt_fits_context("openai", "gpt-5.3-codex-spark", 128_000, 128_000)
                .unwrap_err();

        assert!(err.to_string().contains("Prompt is too large"));
        assert!(err.to_string().contains("openai/gpt-5.3-codex-spark"));
    }

    #[test]
    fn prompt_size_preflight_allows_unknown_context() {
        ensure_estimated_prompt_fits_context("provider", "model", usize::MAX, 0).unwrap();
    }

    #[test]
    fn print_mode_denies_interactive_tools() {
        let rules = print_mode_permission_rules(Vec::new());

        assert!(rules.iter().any(|rule| {
            rule.permission == "question"
                && rule.pattern == "*"
                && rule.action == crate::tools::PermissionPolicyAction::Deny
        }));
        assert!(rules.iter().any(|rule| {
            rule.permission == "update_plan"
                && rule.pattern == "*"
                && rule.action == crate::tools::PermissionPolicyAction::Deny
        }));
    }
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    // Adaptive poll duration: fast when animations run (home page / streaming),
    // slow otherwise to avoid wasting CPU on unnecessary re-renders.
    const FAST_POLL: Duration = Duration::from_millis(16); // ~60fps for interactive animations
    const STREAMING_POLL: Duration = Duration::from_millis(40); // 25fps, matches wave spinner
    const SLOW_POLL: Duration = Duration::from_millis(250); // ~4fps idle

    let mut needs_redraw = true;
    let mut last_complete_frame: Option<Buffer> = None;
    let mut last_full_render_at = std::time::Instant::now();

    while app.running {
        let loop_start = std::time::Instant::now();

        let animation_needed = app.is_animation_running();

        let poll_duration = if animation_needed && app.is_streaming_animation_only() {
            STREAMING_POLL
        } else if animation_needed {
            FAST_POLL
        } else {
            SLOW_POLL
        };

        let elapsed_before_poll = loop_start.elapsed();
        let poll_timeout = if needs_redraw {
            Duration::from_millis(0)
        } else if elapsed_before_poll < poll_duration {
            poll_duration - elapsed_before_poll
        } else {
            Duration::from_millis(0)
        };

        let mut had_input = false;
        if event::poll(poll_timeout)? {
            had_input = true;
            let event = event::read()?;

            if std::env::var_os("CRABCODE_MOUSE_TRACE").is_some() {
                if let event::Event::Mouse(mouse) = &event {
                    crate::emit_log!("Mouse event: {:?}", mouse);
                }
            }

            // DO NOT REMOVE THIS LOG THAT I UNCOMMENT SOMETIMES. I USE IT FOR DEBUGGING
            // push_toast(Toast::new(
            //     format!("Event: {:?}", event),
            //     crate::toast::ToastLevel::Info,
            //     None,
            // ));

            match event {
                event::Event::Mouse(mouse) => {
                    if mouse_scroll_kind(mouse.kind) {
                        let mut last_scroll = mouse;
                        let mut scroll_count = 1usize;
                        let mut applied_scroll = false;

                        while event::poll(Duration::from_millis(0))? {
                            let next = event::read()?;
                            match next {
                                event::Event::Mouse(next_mouse) => {
                                    if mouse_scroll_kind(next_mouse.kind) {
                                        if next_mouse.kind == last_scroll.kind {
                                            scroll_count = scroll_count.saturating_add(1);
                                        } else {
                                            last_scroll = next_mouse;
                                            scroll_count = 1;
                                        }
                                    } else {
                                        app.handle_coalesced_mouse_scroll(
                                            last_scroll,
                                            scroll_count,
                                        );
                                        applied_scroll = true;
                                        app.handle_mouse_event(next_mouse);
                                        break;
                                    }
                                }
                                next => {
                                    app.handle_coalesced_mouse_scroll(last_scroll, scroll_count);
                                    applied_scroll = true;
                                    handle_terminal_event(app, next);
                                    break;
                                }
                            }
                        }

                        if !applied_scroll {
                            app.handle_coalesced_mouse_scroll(last_scroll, scroll_count);
                        }
                    } else if matches!(
                        mouse.kind,
                        MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                    ) {
                        let mut latest_mouse = mouse;
                        let mut applied_mouse = false;

                        while event::poll(Duration::from_millis(0))? {
                            let next = event::read()?;
                            match next {
                                event::Event::Mouse(next_mouse)
                                    if next_mouse.kind == mouse.kind
                                        && next_mouse.modifiers == mouse.modifiers =>
                                {
                                    latest_mouse = next_mouse;
                                }
                                event::Event::Mouse(next_mouse) => {
                                    app.handle_mouse_event(latest_mouse);
                                    applied_mouse = true;
                                    app.handle_mouse_event(next_mouse);
                                    break;
                                }
                                next => {
                                    app.handle_mouse_event(latest_mouse);
                                    applied_mouse = true;
                                    handle_terminal_event(app, next);
                                    break;
                                }
                            }
                        }

                        if !applied_mouse {
                            app.handle_mouse_event(latest_mouse);
                        }
                    } else {
                        app.handle_mouse_event(mouse);
                    }
                    needs_redraw = true;
                }
                event::Event::Key(key) => {
                    app.handle_keys(key);
                    if app.take_just_closed_overlay() {
                        drain_pending_terminal_events(Duration::from_millis(12));
                    }
                    needs_redraw = true;
                }
                event::Event::Paste(text) => {
                    app.handle_paste(text);
                    needs_redraw = true;
                }
                event::Event::FocusGained => {
                    app.set_terminal_focused(true);
                }
                event::Event::FocusLost => {
                    app.set_terminal_focused(false);
                }
                event::Event::Resize(_, _) => {
                    needs_redraw = true;
                }
            }
        }

        app.process_streaming_chunks();
        app.update_animations();
        app.update_terminal_title_signal();
        remove_expired_toasts();
        let isolated_spinner_interval = app.isolated_subagent_spinner_interval();
        let full_render_due = isolated_spinner_interval.is_none_or(|interval| {
            last_complete_frame.is_none() || last_full_render_at.elapsed() >= interval
        });
        if needs_redraw || had_input || animation_needed {
            if isolated_spinner_interval.is_some()
                && !needs_redraw
                && !had_input
                && !full_render_due
            {
                if let Some(base) = last_complete_frame.as_ref() {
                    terminal.draw(|f| {
                        let buffer = f.buffer_mut();
                        buffer.area = base.area;
                        buffer.content.clone_from(&base.content);
                        app.render_isolated_subagent_spinner(buffer);
                    })?;
                }
            } else {
                let completed = terminal.draw(|f| app.render(f))?;
                // Keep a copy of the frame only while the isolated subagent
                // spinner fast-path can use it; cloning the full cell grid on
                // every frame is wasted work otherwise.
                if isolated_spinner_interval.is_some() {
                    match last_complete_frame.as_mut() {
                        Some(frame) => {
                            frame.area = completed.buffer.area;
                            frame.content.clone_from(&completed.buffer.content);
                        }
                        None => last_complete_frame = Some(completed.buffer.clone()),
                    }
                } else {
                    last_complete_frame = None;
                }
                last_full_render_at = std::time::Instant::now();
            }
            needs_redraw = false;
        }
    }
    Ok(())
}
