use anyhow::Result;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub mod auth;
pub mod conversions;
pub mod db;
pub mod history;
pub mod migrations;
pub mod prefs;
pub mod prompt_history;
pub mod providers;

pub use auth::{AuthConfig, AuthDAO};
pub use conversions::persistence_to_session;
pub use db::{get_db_conn, DbConn};
pub use history::{HistoryDAO, Message, MessagePart, Session};
pub use prefs::PrefsDAO;
pub use prompt_history::PromptHistoryCache;

pub fn get_data_dir() -> PathBuf {
    state_home().join("crabcode")
}

pub fn get_cache_dir() -> PathBuf {
    get_data_dir().join("cache")
}

pub fn ensure_data_dir() -> Result<()> {
    let dir = get_data_dir();
    create_private_dir_all(&dir)?;
    Ok(())
}

pub fn ensure_cache_dir() -> Result<()> {
    ensure_data_dir()?;
    let dir = get_cache_dir();
    create_private_dir_all(&dir)?;
    Ok(())
}

fn state_home() -> PathBuf {
    resolve_state_home(std::env::var_os("XDG_STATE_HOME"), dirs::home_dir())
}

fn resolve_state_home(xdg_state_home: Option<OsString>, home_dir: Option<PathBuf>) -> PathBuf {
    if let Some(path) = xdg_state_home {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }

    home_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("state")
}

fn create_private_dir_all(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    restrict_dir_permissions(dir)?;
    Ok(())
}

#[cfg(unix)]
fn restrict_dir_permissions(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_dir_permissions(_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_home_prefers_xdg_state_home() {
        let path = resolve_state_home(
            Some(OsString::from("/tmp/custom-state")),
            Some(PathBuf::from("/home/alice")),
        );

        assert_eq!(path, PathBuf::from("/tmp/custom-state"));
    }

    #[test]
    fn state_home_falls_back_to_local_state() {
        let path = resolve_state_home(None, Some(PathBuf::from("/home/alice")));

        assert_eq!(path, PathBuf::from("/home/alice/.local/state"));
    }

    #[test]
    fn empty_xdg_state_home_uses_fallback() {
        let path = resolve_state_home(Some(OsString::from("")), Some(PathBuf::from("/home/alice")));

        assert_eq!(path, PathBuf::from("/home/alice/.local/state"));
    }
}
