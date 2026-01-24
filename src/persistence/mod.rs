use anyhow::Result;
use std::path::PathBuf;

pub mod auth;
pub mod conversions;
pub mod history;
pub mod migrations;
pub mod providers;

pub use auth::{AuthConfig, AuthDAO};
pub use conversions::{persistence_to_session, session_to_persistence};
pub use history::{HistoryDAO, Message, MessagePart, Session};
pub use providers::{
    AuthenticatedProvider, ModelInfo, Pricing, Provider, ProviderCache, ProviderDAO,
};

pub fn get_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crabcode")
}

pub fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crabcode")
}

pub fn ensure_data_dir() -> Result<()> {
    let dir = get_data_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(())
}

pub fn ensure_cache_dir() -> Result<()> {
    let dir = get_cache_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(())
}
