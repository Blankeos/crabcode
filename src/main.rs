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
use clap::Parser;
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::sync::Mutex;
use std::time::Duration;

const POST_CLOSE_LOGO: &str = include_str!("../crabcode-logo.txt");
const DEFAULT_PRINT_MODE_AGENT_MAX_STEPS: usize = 16;

lazy_static::lazy_static! {
    static ref STARTUP_DIAGNOSTICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
}

pub fn push_startup_diag(msg: String) {
    STARTUP_DIAGNOSTICS.lock().unwrap().push(msg);
}

#[macro_export]
macro_rules! startup_diag {
    ($($arg:tt)*) => {
        $crate::push_startup_diag(format!($($arg)*))
    };
}

fn flush_startup_diagnostics() {
    let diags = std::mem::take(&mut *STARTUP_DIAGNOSTICS.lock().unwrap());
    for msg in diags {
        eprintln!("{}", msg);
    }
}

struct PostCloseInfo {
    session_id: String,
    session_title: String,
}

fn format_post_close_message(info: Option<&PostCloseInfo>) -> String {
    let mut msg = String::new();

    for line in POST_CLOSE_LOGO.lines() {
        msg.push_str(line);
        msg.push('\n');
    }

    if let Some(info) = info {
        msg.push('\n');
        msg.push_str(&format!("  {:<10}{}\n", "Session", info.session_title));
        msg.push_str(&format!(
            "  {:<10}crabcode -s {}\n",
            "Continue", info.session_id
        ));
    }

    msg
}

async fn run_print_mode(
    prompt: &str,
    no_session_persistence: bool,
    dangerously_skip_permissions: bool,
) -> Result<()> {
    use crate::llm::client::stream_llm_with_cancellation;
    use crate::session::types::Message;
    use tokio::sync::mpsc;

    // Load config and model preferences
    let loaded_config = crate::config::ConfigLoader::load()?;
    let prefs_dao = crate::persistence::PrefsDAO::new().ok();

    let (provider_name, model_id) = {
        let active = prefs_dao
            .as_ref()
            .and_then(|d| d.get_active_model().ok().flatten());
        if let Some((pid, mid)) = active {
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

    let cwd = loaded_config.cwd.to_string_lossy().to_string();

    let is_git_repo = crate::utils::git::is_git_repo(&cwd).unwrap_or(false);

    // Build messages with system prompt
    let composer = crate::prompt::SystemPromptComposer::new(
        &model_id,
        &cwd,
        is_git_repo,
        std::env::consts::OS,
    );
    let system_prompt = composer.compose().await;
    let messages = vec![Message::system(system_prompt), Message::user(prompt)];

    let (sender, mut receiver) = mpsc::unbounded_channel();

    let tool_permissions = crate::tools::ToolPermissions::new(std::path::PathBuf::from(&cwd))
        .dangerously_skip_permissions(dangerously_skip_permissions);

    let agent_max_steps = loaded_config
        .merged_config
        .agent_steps
        .get(&agent_mode.to_ascii_lowercase())
        .copied()
        .or(Some(DEFAULT_PRINT_MODE_AGENT_MAX_STEPS));

    let provider_name_clone = provider_name.clone();
    let model_clone = model_id.clone();
    let completion_sender = sender.clone();

    tokio::spawn(async move {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        if let Err(err) = stream_llm_with_cancellation(
            cancel_token,
            cuid2::create_id(),
            provider_name_clone,
            model_clone,
            agent_mode.clone(),
            agent_max_steps,
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
            _ => {}
        }
    }

    flush_startup_diagnostics();
    let _ = no_session_persistence;
    Ok(())
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
    /// Resume a session by ID
    #[arg(short = 's', long = "session")]
    session: Option<String>,

    /// Run in print mode (non-interactive, streams output to stdout)
    #[arg(short = 'p', long = "print")]
    print_mode: bool,

    /// Do not persist session data to disk
    #[arg(long = "no-session-persistence")]
    no_session_persistence: bool,

    /// Skip permission prompts in print mode. Intended for isolated benchmark/CI workspaces.
    #[arg(long = "dangerously-skip-permissions")]
    dangerously_skip_permissions: bool,

    /// The prompt to run (positional, used in print mode)
    prompt: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.print_mode {
        let prompt = args.prompt.join(" ");
        if prompt.trim().is_empty() {
            flush_startup_diagnostics();
            eprintln!("Error: No prompt provided for print mode.");
            eprintln!("Usage: crabcode -p \"<PROMPT>\"");
            std::process::exit(1);
        }
        return run_print_mode(
            &prompt,
            args.no_session_persistence,
            args.dangerously_skip_permissions,
        )
        .await;
    }

    let mut app = App::new()?;

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
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
            EnableBracketedPaste
        )?;
    } else {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
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

    disable_raw_mode()?;
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste
        )?;
    } else {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        )?;
    }
    terminal.show_cursor()?;

    flush_startup_diagnostics();

    print!("{}", format_post_close_message(close_info.as_ref()));

    result
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
                    let _ = crate::logging::log(&format!("Mouse event: {:?}", mouse));
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
                                _ => {}
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
                _ => {}
            }
        }
    }
    Ok(())
}
