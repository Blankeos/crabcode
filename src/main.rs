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

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    let _args = Args::parse();
    let mut app = App::new();
    app.run().await
}
