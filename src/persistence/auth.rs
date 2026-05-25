use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use super::{ensure_data_dir, get_data_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthConfig {
    #[serde(rename = "api")]
    Api { key: String },
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "oauth")]
    OAuth {
        refresh: String,
        access: String,
        expires: i64,
        #[serde(rename = "accountId", default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
        #[serde(
            rename = "enterpriseUrl",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        enterprise_url: Option<String>,
    },
}

pub struct AuthDAO {
    auth_path: PathBuf,
}

impl AuthDAO {
    pub fn new() -> Result<Self> {
        let auth_path = Self::auth_path();
        Self::ensure_auth_parent()?;
        Ok(Self { auth_path })
    }

    fn test_mode() -> bool {
        cfg!(test) || env::var("CRABCODE_TEST_MODE").is_ok()
    }

    fn auth_path() -> PathBuf {
        if Self::test_mode() {
            PathBuf::from("/tmp/crabcode_test_data").join("auth.json")
        } else {
            let data_dir = get_data_dir();
            data_dir.join("auth.json")
        }
    }

    fn legacy_api_keys_path() -> PathBuf {
        if Self::test_mode() {
            PathBuf::from("/tmp/crabcode_test_api_keys.json")
        } else {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("crabcode")
                .join("api_keys.json")
        }
    }

    fn ensure_auth_parent() -> Result<()> {
        if Self::test_mode() {
            if let Some(parent) = Self::auth_path().parent() {
                std::fs::create_dir_all(parent)?;
            }
        } else {
            ensure_data_dir()?;
        }
        Ok(())
    }

    fn try_migrate_legacy_api_keys(&self) -> Result<()> {
        if self.auth_path.exists() {
            return Ok(());
        }

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct LegacyApiKeyConfig {
            api_keys: HashMap<String, String>,
        }

        let legacy_path = Self::legacy_api_keys_path();
        if !legacy_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&legacy_path)?;
        let legacy: LegacyApiKeyConfig = serde_json::from_str(&content)?;
        if legacy.api_keys.is_empty() {
            return Ok(());
        }

        let mut providers: HashMap<String, AuthConfig> = HashMap::new();
        for (name, key) in legacy.api_keys {
            providers.insert(name, AuthConfig::Api { key });
        }

        self.save(&providers)?;

        // Best-effort cleanup: once migrated, avoid keeping two sources of truth.
        let _ = std::fs::remove_file(&legacy_path);
        Ok(())
    }

    pub fn load(&self) -> Result<HashMap<String, AuthConfig>> {
        self.try_migrate_legacy_api_keys()?;

        if !self.auth_path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&self.auth_path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, providers: &HashMap<String, AuthConfig>) -> Result<()> {
        Self::ensure_auth_parent()?;
        let content = serde_json::to_string_pretty(providers)?;
        std::fs::write(&self.auth_path, content)?;
        restrict_auth_file_permissions(&self.auth_path)?;
        Ok(())
    }

    pub fn set_provider(&self, name: String, config: AuthConfig) -> Result<()> {
        let mut providers = self.load()?;
        providers.insert(name, config);
        self.save(&providers)
    }

    pub fn remove_provider(&self, name: &str) -> Result<()> {
        let mut providers = self.load()?;
        providers.remove(name);
        self.save(&providers)
    }

    pub fn get_api_key(&self, name: &str) -> Result<Option<String>> {
        let providers = self.load()?;
        Ok(providers.get(name).and_then(|c| match c {
            AuthConfig::Api { key } => Some(key.clone()),
            AuthConfig::Local => None,
            AuthConfig::OAuth { access, .. } => Some(access.clone()),
        }))
    }

    pub fn get_provider(&self, name: &str) -> Result<Option<AuthConfig>> {
        let providers = self.load()?;
        Ok(providers.get(name).cloned())
    }
}

#[cfg(unix)]
fn restrict_auth_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_auth_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
impl AuthDAO {
    pub fn cleanup_test() -> Result<()> {
        let auth_path = Self::auth_path();
        if auth_path.exists() {
            std::fs::remove_file(&auth_path)?;
        }

        let legacy_path = Self::legacy_api_keys_path();
        if legacy_path.exists() {
            std::fs::remove_file(&legacy_path)?;
        }

        Ok(())
    }
}
