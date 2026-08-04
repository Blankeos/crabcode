//! Agent Client Protocol support.
//!
//! ACP uses JSON-RPC over standard input/output. This module intentionally
//! avoids constructing the Ratatui application so editors can launch Crabcode
//! as a normal subprocess.

mod server;
mod service;

use anyhow::Result;
use std::path::PathBuf;

pub async fn run(cwd: Option<PathBuf>) -> Result<()> {
    server::run(cwd).await
}
