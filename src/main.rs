#![allow(dead_code)]

mod agent;
mod app;
mod autocomplete;
mod command;
mod config;
mod model;
mod session;
mod streaming;
mod ui;
mod utils;

use anyhow::Result;
use app::App;
use clap::Parser;
use ratatui::layout::Rect;
use ratatui_toolkit::{Toast, ToastLevel, ToastManager, render_toasts};
use std::sync::Mutex;

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

pub fn get_toast_surface_area(frame_area: Rect) -> Rect {
    let toasts = get_toast_manager().lock().unwrap();
    let active_toasts = toasts.get_active();
    
    if active_toasts.is_empty() {
        return Rect::new(frame_area.x, frame_area.y, 0, 0);
    }
    
    const TOAST_HEIGHT: u16 = 3;
    const TOAST_GAP: u16 = 1;
    const TOAST_MARGIN: u16 = 2;
    
    let total_height = (active_toasts.len() as u16 * (TOAST_HEIGHT + TOAST_GAP)) + TOAST_MARGIN;
    let width = frame_area.width.saturating_sub(4);
    
    Rect::new(
        frame_area.x + 2,
        frame_area.y + frame_area.height.saturating_sub(total_height),
        width,
        total_height.min(frame_area.height),
    )
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    let _args = Args::parse();
    let mut app = App::new();
    app.run().await
}
