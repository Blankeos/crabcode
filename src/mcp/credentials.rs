//! Persistent credential storage for remote MCP OAuth tokens.
//!
//! File: `$XDG_STATE_HOME/crabcode/mcp-auth.json` (default `~/.local/state/crabcode/mcp-auth.json`).
//! Keyed by `"{server_name}:{server_url}"` so a renamed URL does not reuse tokens.

use anyhow::{Context, Result};
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const CREDENTIALS_FILENAME: &str = "mcp-auth.json";

static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn store_lock() -> &'static Mutex<()> {
    STORE_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct CredentialFile {
    #[serde(flatten)]
    entries: BTreeMap<String, StoredCredentials>,
}

pub fn store_key(server_name: &str, server_url: &str) -> String {
    format!("{server_name}:{server_url}")
}

pub fn store_path() -> PathBuf {
    if cfg!(test) || std::env::var("CRABCODE_TEST_MODE").is_ok() {
        PathBuf::from("/tmp/crabcode_test_data").join(CREDENTIALS_FILENAME)
    } else {
        crate::persistence::get_data_dir().join(CREDENTIALS_FILENAME)
    }
}

fn load_from(path: &Path) -> Result<CredentialFile> {
    if !path.exists() {
        return Ok(CredentialFile::default());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let _ = restrict_file_permissions(path);
    serde_json::from_str(&content).with_context(|| format!("invalid {}", path.display()))
}

fn save_to(path: &Path, file: &CredentialFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(file)?;
    std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))?;
    let _ = restrict_file_permissions(path);
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub fn has_credentials(server_name: &str, server_url: &str) -> bool {
    load(server_name, server_url)
        .ok()
        .flatten()
        .and_then(|creds| creds.token_response)
        .is_some()
}

pub fn load(server_name: &str, server_url: &str) -> Result<Option<StoredCredentials>> {
    let _guard = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let file = load_from(&store_path())?;
    Ok(file
        .entries
        .get(&store_key(server_name, server_url))
        .cloned())
}

pub fn save(server_name: &str, server_url: &str, credentials: StoredCredentials) -> Result<()> {
    let _guard = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let path = store_path();
    let mut file = load_from(&path).unwrap_or_default();
    file.entries
        .insert(store_key(server_name, server_url), credentials);
    save_to(&path, &file)
}

pub fn delete(server_name: &str, server_url: &str) -> Result<bool> {
    let _guard = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let path = store_path();
    let mut file = load_from(&path).unwrap_or_default();
    let removed = file
        .entries
        .remove(&store_key(server_name, server_url))
        .is_some();
    if removed {
        save_to(&path, &file)?;
    }
    Ok(removed)
}

pub struct FileCredentialStore {
    server_name: String,
    server_url: String,
}

impl FileCredentialStore {
    pub fn new(server_name: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            server_url: server_url.into(),
        }
    }
}

#[async_trait::async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let name = self.server_name.clone();
        let url = self.server_url.clone();
        tokio::task::spawn_blocking(move || {
            load(&name, &url).map_err(|e| AuthError::InternalError(e.to_string()))
        })
        .await
        .map_err(|e| AuthError::InternalError(e.to_string()))?
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        let name = self.server_name.clone();
        let url = self.server_url.clone();
        tokio::task::spawn_blocking(move || {
            save(&name, &url, credentials).map_err(|e| AuthError::InternalError(e.to_string()))
        })
        .await
        .map_err(|e| AuthError::InternalError(e.to_string()))?
    }

    async fn clear(&self) -> std::result::Result<(), AuthError> {
        let name = self.server_name.clone();
        let url = self.server_url.clone();
        tokio::task::spawn_blocking(move || {
            delete(&name, &url)
                .map(|_| ())
                .map_err(|e| AuthError::InternalError(e.to_string()))
        })
        .await
        .map_err(|e| AuthError::InternalError(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_env() {
        std::env::set_var("CRABCODE_TEST_MODE", "1");
        let path = store_path();
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    #[test]
    fn missing_file_is_empty() {
        isolated_env();
        assert!(!has_credentials("doop", "https://doop.design/mcp"));
        assert!(load("doop", "https://doop.design/mcp").unwrap().is_none());
    }

    #[test]
    fn delete_missing_is_false() {
        isolated_env();
        assert!(!delete("doop", "https://doop.design/mcp").unwrap());
    }
}
