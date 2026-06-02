#![allow(dead_code)]

mod agent;
mod app;
mod auth;
mod autocomplete;
mod command;
mod config;
mod llm;
mod logging;
mod model;
mod notify;
mod persistence;
mod prompt;
mod remote;
mod session;
mod skill;
mod sound;
mod streaming;
mod theme;
mod toast;
mod tools;
mod ui;
mod utils;
mod views;

use crate::toast::{Toast, ToastManager};
use anyhow::Result;
use app::App;
use clap::{Parser, Subcommand};
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, style::Color, Terminal};
use std::io::{self, IsTerminal, Read};
use std::sync::Mutex;
use std::time::Duration;

const POST_CLOSE_LOGO: &str = include_str!("../crabcode-logo.txt");
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";

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
    let reasoning_effort = crate::model::discovery::Discovery::new()
        .ok()
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

    let prompt_registry = crate::tools::initialize_tool_registry_with_dynamic(
        Some(sender.clone()),
        tool_permissions.clone(),
        agent_registry.clone(),
        cancel_token.clone(),
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
    .with_tool_registry(prompt_registry)
    .with_agent_registry(agent_registry.clone())
    .with_print_mode(true);
    let system_prompt = composer.compose().await;
    let messages = vec![Message::system(system_prompt), Message::user(prompt)];

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
            crate::llm::ChunkMessage::ToolCalls(_) | crate::llm::ChunkMessage::ToolResult(_) => {}
            crate::llm::ChunkMessage::End => {
                println!();
                break;
            }
            crate::llm::ChunkMessage::Failed(error) => {
                eprintln!("\nError: {}", error);
                break;
            }
            crate::llm::ChunkMessage::Warning(warning) => {
                eprintln!("Warning: {}", warning);
            }
            crate::llm::ChunkMessage::PermissionRequest(prompt) => {
                eprintln!(
                    "Permission required: {}. Re-run with --dangerously-skip-permissions to allow non-interactive tool execution.",
                    prompt.reason
                );
                let _ = prompt
                    .response_tx
                    .send(crate::tools::PermissionResponse::Deny);
            }
            crate::llm::ChunkMessage::QuestionRequest { response_tx, .. } => {
                let _ = response_tx.send(serde_json::json!({
                    "skipped": true,
                    "reason": "Question prompts are unavailable in non-interactive print mode"
                }));
            }
            _ => {}
        }
    }

    let _ = no_session_persistence;
    Ok(())
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
#[command(author, version, about, long_about = None)]
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

    /// The prompt to run (positional, used in print mode)
    prompt: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    crate::logging::set_enabled(args.emit_logs);

    match &args.command {
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
        app.session_manager.switch_session(session_id);
        if let Some(session) = app.session_manager.get_session(session_id) {
            app.chat_state.chat.clear();
            let messages = session.messages.clone();
            for message in messages {
                app.chat_state.chat.add_message(message);
            }
        }
        app.base_focus = app::BaseFocus::Chat;
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();

    if supports_keyboard_enhancement()? {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
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

    disable_raw_mode()?;
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableFocusChange,
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste
        )?;
    } else {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableFocusChange,
            DisableBracketedPaste
        )?;
    }
    terminal.show_cursor()?;

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
    const FAST_POLL: Duration = Duration::from_millis(16); // ~60fps for animations
    const SLOW_POLL: Duration = Duration::from_millis(250); // ~4fps idle

    let mut needs_redraw = true;

    while app.running {
        let loop_start = std::time::Instant::now();

        let animation_needed = app.is_animation_running();

        app.process_streaming_chunks();
        app.update_animations();
        app.update_terminal_title_signal();
        remove_expired_toasts();
        if needs_redraw || animation_needed {
            terminal.draw(|f| app.render(f))?;
            needs_redraw = false;
        }

        let poll_duration = if animation_needed {
            FAST_POLL
        } else {
            SLOW_POLL
        };

        // Calculate how long the loop iteration took
        let elapsed = loop_start.elapsed();

        let poll_timeout = if elapsed < poll_duration {
            poll_duration - elapsed
        } else {
            Duration::from_millis(0)
        };

        if event::poll(poll_timeout)? {
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
                    if matches!(
                        mouse.kind,
                        event::MouseEventKind::ScrollDown | event::MouseEventKind::ScrollUp
                    ) {
                        const MAX_SCROLL_PER_FRAME: usize = 6;
                        let mut last_scroll = mouse;
                        let mut scroll_count = 1usize;

                        while event::poll(Duration::from_millis(0))? {
                            let next = event::read()?;
                            match next {
                                event::Event::Mouse(next_mouse) => {
                                    if matches!(
                                        next_mouse.kind,
                                        event::MouseEventKind::ScrollDown
                                            | event::MouseEventKind::ScrollUp
                                    ) {
                                        if next_mouse.kind == last_scroll.kind {
                                            scroll_count = scroll_count.saturating_add(1);
                                        } else {
                                            last_scroll = next_mouse;
                                            scroll_count = 1;
                                        }
                                    } else {
                                        app.handle_mouse_event(next_mouse);
                                    }
                                }
                                event::Event::Key(key) => {
                                    app.handle_keys(key);
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

                        let repeat = scroll_count.min(MAX_SCROLL_PER_FRAME);
                        for _ in 0..repeat {
                            app.handle_mouse_event(last_scroll);
                        }
                    } else {
                        app.handle_mouse_event(mouse);
                    }
                    needs_redraw = true;
                }
                event::Event::Key(key) => {
                    app.handle_keys(key);
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
    }
    Ok(())
}
