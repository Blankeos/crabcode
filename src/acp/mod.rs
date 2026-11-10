//! Agent Client Protocol support.
//!
//! ACP uses JSON-RPC over standard input/output. This module intentionally
//! avoids constructing the Ratatui application so editors can launch Crabcode
//! as a normal subprocess.

mod login;
mod server;
mod service;

use anyhow::Result;
use std::path::PathBuf;

pub async fn run(cwd: Option<PathBuf>, dangerously_skip_permissions: bool) -> Result<()> {
    server::run(cwd, dangerously_skip_permissions).await
}

pub async fn login(cwd: Option<PathBuf>) -> Result<()> {
    login::run(cwd).await
}
