#![allow(dead_code)]

mod agent;
mod app;
mod autocomplete;
mod command;
mod config;
mod model;
mod persistence;
mod session;
mod streaming;
mod theme;
mod ui;
mod utils;
mod views;

use anyhow::Result;
use app::App;
use clap::Parser;
use ratatui::crossterm::{
    event::{self, EnableMouseCapture, DisableMouseCapture,
        PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        KeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use ratatui_toolkit::{render_toasts, Toast, ToastManager};
use std::io;
use std::sync::Mutex;
use std::time::Duration;

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
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    let _args = Args::parse();
    let mut app = App::new();

    enable_raw_mode()?;
    let mut stdout = io::stdout();

    if supports_keyboard_enhancement()? {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    } else {
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags
        )?;
    } else {
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
    }
    terminal.show_cursor()?;

    result
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    while app.running {
        remove_expired_toasts();
        terminal.draw(|f| app.render(f))?;

        if event::poll(Duration::from_millis(100))? {
            let event = event::read()?;

            match event {
                event::Event::Key(key) => {
                    app.handle_keys(key);
                }
                event::Event::Mouse(mouse) => {
                    app.handle_mouse_event(mouse);
                }
                _ => {}
            }
        }
    }
    Ok(())
}
