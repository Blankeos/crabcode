use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyConfig {
    pub api_keys: HashMap<String, String>,
}

impl Default for ApiKeyConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiKeyConfig {
    pub fn new() -> Self {
        Self {
            api_keys: HashMap::new(),
        }
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            let config: ApiKeyConfig = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::new())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn set_api_key(&mut self, provider: String, api_key: String) {
        self.api_keys.insert(provider, api_key);
    }

    pub fn get_api_key(&self, provider: &str) -> Option<&String> {
        self.api_keys.get(provider)
    }

    pub fn list_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self.api_keys.keys().cloned().collect();
        providers.sort();
        providers
    }

    fn config_path() -> PathBuf {
        if cfg!(test) || env::var("CRABCODE_TEST_MODE").is_ok() {
            PathBuf::from("/tmp/crabcode_test_api_keys.json")
        } else {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("crabcode")
                .join("api_keys.json")
        }
    }

    #[cfg(test)]
    pub fn load_test() -> Result<Self> {
        let path = PathBuf::from("/tmp/crabcode_test_api_keys.json");
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            let config: ApiKeyConfig = serde_json::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::new())
        }
    }

    #[cfg(test)]
    pub fn save_test(&self) -> Result<()> {
        let path = PathBuf::from("/tmp/crabcode_test_api_keys.json");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn cleanup_test() -> Result<()> {
        let path = PathBuf::from("/tmp/crabcode_test_api_keys.json");
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_config_new() {
        let config = ApiKeyConfig::new();
        assert!(config.api_keys.is_empty());
    }

    #[test]
    fn test_api_key_config_default() {
        let config = ApiKeyConfig::default();
        assert!(config.api_keys.is_empty());
    }

    #[test]
    fn test_set_api_key() {
        let mut config = ApiKeyConfig::new();
        config.set_api_key("nano-gpt".to_string(), "test-key-123".to_string());
        assert_eq!(
            config.get_api_key("nano-gpt"),
            Some(&"test-key-123".to_string())
        );
    }

    #[test]
    fn test_get_api_key_nonexistent() {
        let config = ApiKeyConfig::new();
        assert_eq!(config.get_api_key("nonexistent"), None);
    }

    #[test]
    fn test_list_providers_empty() {
        let config = ApiKeyConfig::new();
        assert!(config.list_providers().is_empty());
    }

    #[test]
    fn test_list_providers() {
        let mut config = ApiKeyConfig::new();
        config.set_api_key("z-ai".to_string(), "key1".to_string());
        config.set_api_key("nano-gpt".to_string(), "key2".to_string());
        let providers = config.list_providers();
        assert_eq!(providers.len(), 2);
        assert!(providers.contains(&"nano-gpt".to_string()));
        assert!(providers.contains(&"z-ai".to_string()));
    }

    #[test]
    fn test_list_providers_sorted() {
        let mut config = ApiKeyConfig::new();
        config.set_api_key("z-ai".to_string(), "key1".to_string());
        config.set_api_key("nano-gpt".to_string(), "key2".to_string());
        let providers = config.list_providers();
        assert_eq!(providers[0], "nano-gpt");
        assert_eq!(providers[1], "z-ai");
    }

    #[test]
    fn test_save_and_load_test() -> Result<()> {
        ApiKeyConfig::cleanup_test()?;

        let mut config = ApiKeyConfig::new();
        config.set_api_key("nano-gpt".to_string(), "test-key".to_string());
        config.save_test()?;

        let loaded = ApiKeyConfig::load_test()?;
        assert_eq!(
            loaded.get_api_key("nano-gpt"),
            Some(&"test-key".to_string())
        );

        ApiKeyConfig::cleanup_test()?;
        Ok(())
    }

    #[test]
    fn test_serialization() {
        let mut config = ApiKeyConfig::new();
        config.set_api_key("nano-gpt".to_string(), "test-key".to_string());

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: ApiKeyConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(
            deserialized.get_api_key("nano-gpt"),
            Some(&"test-key".to_string())
        );
    }
}
