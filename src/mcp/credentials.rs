//! Persistent credential storage for remote MCP OAuth tokens.
//!
//! File: `$XDG_STATE_HOME/crabcode/mcp-auth.json` (default `~/.local/state/crabcode/mcp-auth.json`).
//! Keyed by `"{server_name}:{server_url}"` so a renamed URL does not reuse tokens.

use anyhow::{Context, Result};
use oauth2::TokenResponse;
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use sha2::{Digest, Sha256};
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
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    restrict_file_permissions(temp.path())?;
    serde_json::to_writer_pretty(&mut temp, file)?;
    temp.flush()?;
    temp.persist(path)
        .with_context(|| format!("failed to write {}", path.display()))?;
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

/// Expiry-aware credential status shown by `mcp list`.
///
/// `has_credentials` stays presence-only on purpose: expired credentials are
/// still usable by the transport layer to attempt a refresh, so it must keep
/// reporting them as present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStatus {
    Authenticated,
    Expired,
    NeedsAuth,
}

impl CredentialStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialStatus::Authenticated => "authenticated",
            CredentialStatus::Expired => "expired",
            CredentialStatus::NeedsAuth => "needs_auth",
        }
    }
}

impl std::fmt::Display for CredentialStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pure, deterministic expiry check: `true` once `received_at + expires_in <= now`.
/// Missing timing data means "no expiry info" and reports `false` (authenticated).
pub fn is_expired_at(
    token_received_at: Option<u64>,
    expires_in_secs: Option<u64>,
    now_secs: u64,
) -> bool {
    match (token_received_at, expires_in_secs) {
        (Some(received_at), Some(expires_in)) => received_at.saturating_add(expires_in) <= now_secs,
        _ => false,
    }
}

/// Pure status for already-loaded credentials at an explicit `now_secs`.
/// No token (or no credentials at all) => `NeedsAuth`; no expiry data => `Authenticated`.
pub fn status_for_credentials_at(
    credentials: Option<&StoredCredentials>,
    now_secs: u64,
) -> CredentialStatus {
    let Some(stored) = credentials else {
        return CredentialStatus::NeedsAuth;
    };
    let Some(token) = stored.token_response.as_ref() else {
        return CredentialStatus::NeedsAuth;
    };
    if is_expired_at(
        stored.token_received_at,
        token.expires_in().map(|duration| duration.as_secs()),
        now_secs,
    ) {
        CredentialStatus::Expired
    } else {
        CredentialStatus::Authenticated
    }
}

/// Expiry-aware status for `mcp list`. Missing entries and unreadable/invalid
/// stores report `NeedsAuth`.
pub fn credential_status(server_name: &str, server_url: &str) -> CredentialStatus {
    match load(server_name, server_url) {
        Ok(credentials) => status_for_credentials_at(credentials.as_ref(), now_epoch_secs()),
        Err(_) => CredentialStatus::NeedsAuth,
    }
}

pub fn load(server_name: &str, server_url: &str) -> Result<Option<StoredCredentials>> {
    load_at(&store_path(), server_name, server_url)
}

fn load_at(path: &Path, server_name: &str, server_url: &str) -> Result<Option<StoredCredentials>> {
    let _guard = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let file = load_from(path)?;
    Ok(file
        .entries
        .get(&store_key(server_name, server_url))
        .cloned())
}

fn save_at(
    path: &Path,
    server_name: &str,
    server_url: &str,
    credentials: StoredCredentials,
) -> Result<()> {
    let _guard = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _file_guard = lock_store(path)?;
    let mut file = load_from(path)?;
    file.entries
        .insert(store_key(server_name, server_url), credentials);
    save_to(path, &file)
}

pub fn delete(server_name: &str, server_url: &str) -> Result<bool> {
    delete_at(&store_path(), server_name, server_url)
}

fn delete_at(path: &Path, server_name: &str, server_url: &str) -> Result<bool> {
    let _guard = store_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _file_guard = lock_store(path)?;
    let mut file = load_from(path)?;
    let removed = file
        .entries
        .remove(&store_key(server_name, server_url))
        .is_some();
    if removed {
        save_to(path, &file)?;
    }
    Ok(removed)
}

fn open_lock(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn lock_store(path: &Path) -> std::io::Result<std::fs::File> {
    let file = open_lock(&path.with_extension("lock"))?;
    file.lock()?;
    Ok(file)
}

pub(super) fn refresh_lock_path(name: &str, url: &str) -> PathBuf {
    let key = Sha256::digest(store_key(name, url).as_bytes());
    store_path()
        .parent()
        .unwrap()
        .join("mcp-auth-locks")
        .join(format!("{key:x}.lock"))
}

pub(super) async fn lock_refresh(path: &Path) -> std::io::Result<std::fs::File> {
    let file = open_lock(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await
            }
            Err(std::fs::TryLockError::Error(err)) => return Err(err),
        }
    }
}

pub struct FileCredentialStore {
    server_name: String,
    server_url: String,
    path: PathBuf,
}

impl FileCredentialStore {
    pub fn new(server_name: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            server_url: server_url.into(),
            path: store_path(),
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(name: &str, url: &str, path: PathBuf) -> Self {
        Self {
            server_name: name.into(),
            server_url: url.into(),
            path,
        }
    }
}

#[async_trait::async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        let name = self.server_name.clone();
        let url = self.server_url.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            load_at(&path, &name, &url).map_err(|e| AuthError::InternalError(e.to_string()))
        })
        .await
        .map_err(|e| AuthError::InternalError(e.to_string()))?
    }

    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        let name = self.server_name.clone();
        let url = self.server_url.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            save_at(&path, &name, &url, credentials)
                .map_err(|e| AuthError::InternalError(e.to_string()))
        })
        .await
        .map_err(|e| AuthError::InternalError(e.to_string()))?
    }

    async fn clear(&self) -> std::result::Result<(), AuthError> {
        let name = self.server_name.clone();
        let url = self.server_url.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            delete_at(&path, &name, &url)
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

    /// Build in-memory credentials without touching the file store or env.
    fn stored_with_expiry(
        expires_in_secs: Option<u64>,
        received_at: Option<u64>,
    ) -> StoredCredentials {
        let mut token_json =
            serde_json::json!({"access_token": "test-token", "token_type": "Bearer"});
        if let Some(secs) = expires_in_secs {
            token_json["expires_in"] = serde_json::json!(secs);
        }
        let token_response = serde_json::from_value(token_json).expect("test token");
        StoredCredentials::new(
            "test-client".to_string(),
            Some(token_response),
            vec![],
            received_at,
        )
    }

    #[test]
    fn status_reports_authenticated_when_token_valid() {
        let creds = stored_with_expiry(Some(3600), Some(1000));
        assert_eq!(
            status_for_credentials_at(Some(&creds), 1000),
            CredentialStatus::Authenticated
        );
        assert_eq!(
            status_for_credentials_at(Some(&creds), 1000 + 3599),
            CredentialStatus::Authenticated
        );
    }

    #[test]
    fn status_reports_expired_at_boundary() {
        let creds = stored_with_expiry(Some(3600), Some(1000));
        // Exactly at received_at + expires_in counts as expired.
        assert_eq!(
            status_for_credentials_at(Some(&creds), 1000 + 3600),
            CredentialStatus::Expired
        );
        assert_eq!(
            status_for_credentials_at(Some(&creds), 1000 + 3601),
            CredentialStatus::Expired
        );
    }

    #[test]
    fn status_needs_auth_without_token() {
        assert_eq!(
            status_for_credentials_at(None, 2000),
            CredentialStatus::NeedsAuth
        );
        let creds = StoredCredentials::new("test-client".to_string(), None, vec![], Some(1000));
        assert_eq!(
            status_for_credentials_at(Some(&creds), 2000),
            CredentialStatus::NeedsAuth
        );
    }

    #[test]
    fn status_authenticated_without_expiry_data() {
        // No expires_in.
        let no_expiry = stored_with_expiry(None, Some(1000));
        assert_eq!(
            status_for_credentials_at(Some(&no_expiry), 1_000_000),
            CredentialStatus::Authenticated
        );
        // No received_at.
        let no_received = stored_with_expiry(Some(3600), None);
        assert_eq!(
            status_for_credentials_at(Some(&no_received), 1_000_000),
            CredentialStatus::Authenticated
        );
        // Neither.
        let neither = stored_with_expiry(None, None);
        assert_eq!(
            status_for_credentials_at(Some(&neither), 1_000_000),
            CredentialStatus::Authenticated
        );
    }

    #[test]
    fn expiry_check_handles_edge_cases() {
        assert!(!is_expired_at(None, Some(10), 20));
        assert!(!is_expired_at(Some(10), None, 20));
        assert!(!is_expired_at(None, None, 20));
        // Saturating add must not panic near u64::MAX.
        assert!(is_expired_at(Some(u64::MAX), Some(u64::MAX), u64::MAX));
    }

    #[test]
    fn store_key_scopes_credentials_by_url() {
        assert_ne!(
            store_key("server", "https://a.example/mcp"),
            store_key("server", "https://b.example/mcp")
        );
        assert_ne!(
            store_key("a", "https://example/mcp"),
            store_key("b", "https://example/mcp")
        );
    }
}
