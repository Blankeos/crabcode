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
        msg.push_str(&format!("  {:<10}crabcode -s {}\n", "Continue", info.session_id));
    }

    msg
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
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
    // Use a shorter poll duration for smoother animations (16ms = ~60fps max)
    const POLL_DURATION: Duration = Duration::from_millis(16);

    while app.running {
        let loop_start = std::time::Instant::now();

        app.process_streaming_chunks();
        app.update_animations();
        remove_expired_toasts();
        terminal.draw(|f| app.render(f))?;

        // Calculate how long the loop iteration took
        let elapsed = loop_start.elapsed();

        // Poll for events, but with a dynamic timeout to maintain consistent frame timing
        // If we spent less than POLL_DURATION processing, wait for the remainder
        let poll_timeout = if elapsed < POLL_DURATION {
            POLL_DURATION - elapsed
        } else {
            Duration::from_millis(0)
        };

        if event::poll(poll_timeout)? {
            let event = event::read()?;

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
    }
    Ok(())
}
