use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::{ensure_data_dir, get_data_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthConfig {
    #[serde(rename = "api")]
    Api { key: String },
    #[serde(rename = "oauth")]
    OAuth {
        refresh: String,
        access: String,
        expires: i64,
    },
}

pub struct AuthDAO {
    auth_path: PathBuf,
}

impl AuthDAO {
    pub fn new() -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        Ok(Self {
            auth_path: data_dir.join("auth.json"),
        })
    }

    pub fn load(&self) -> Result<HashMap<String, AuthConfig>> {
        if !self.auth_path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&self.auth_path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, providers: &HashMap<String, AuthConfig>) -> Result<()> {
        let content = serde_json::to_string_pretty(providers)?;
        std::fs::write(&self.auth_path, content)?;
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
            AuthConfig::OAuth { access, .. } => Some(access.clone()),
        }))
    }
}
