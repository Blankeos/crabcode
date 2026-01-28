use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub doc: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub npm: String,
    #[serde(default)]
    pub models: HashMap<String, Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub attachment: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub structured_output: bool,
    #[serde(default)]
    pub temperature: bool,
    #[serde(default)]
    pub knowledge: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub last_updated: String,
    #[serde(default)]
    pub modalities: Option<Modalities>,
    #[serde(default)]
    pub open_weights: bool,
    #[serde(default)]
    pub cost: Option<Cost>,
    #[serde(default)]
    pub limit: Option<Limit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Modalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limit {
    #[serde(default)]
    pub context: u32,
    #[serde(default)]
    pub output: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    data: HashMap<String, Provider>,
    timestamp: u64,
}

pub struct Discovery {
    client: Client,
    cache_path: PathBuf,
}

impl Discovery {
    pub fn new() -> Result<Self> {
        if cfg!(test) || env::var("CRABCODE_TEST_MODE").is_ok() {
            let cache_dir = PathBuf::from("/tmp/crabcode_test_cache");
            fs::create_dir_all(&cache_dir).context("Failed to create test cache directory")?;

            let cache_path = cache_dir.join("models_dev_cache.json");

            Ok(Self {
                client: Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .context("Failed to create HTTP client")?,
                cache_path,
            })
        } else {
            let cache_dir = dirs::home_dir()
                .context("Could not find home directory")?
                .join(".cache")
                .join("crabcode");

            fs::create_dir_all(&cache_dir).context("Failed to create cache directory")?;

            let cache_path = cache_dir.join("models_dev_cache.json");

            Ok(Self {
                client: Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .context("Failed to create HTTP client")?,
                cache_path,
            })
        }
    }

    fn get_cache_path(&self) -> &PathBuf {
        &self.cache_path
    }

    fn load_from_cache(&self) -> Result<Option<HashMap<String, Provider>>> {
        let cache_path = self.get_cache_path();

        if !cache_path.exists() {
            return Ok(None);
        }

        let cached_json = fs::read_to_string(cache_path).context("Failed to read cache file")?;

        let entry: CacheEntry =
            serde_json::from_str(&cached_json).context("Failed to parse cache file")?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("System time is before UNIX epoch")?
            .as_secs();

        if now - entry.timestamp > CACHE_TTL_SECONDS {
            return Ok(None);
        }

        Ok(Some(entry.data))
    }

    fn save_to_cache(&self, data: &HashMap<String, Provider>) -> Result<()> {
        let cache_path = self.get_cache_path();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("System time is before UNIX epoch")?
            .as_secs();

        let entry = CacheEntry {
            data: data.clone(),
            timestamp: now,
        };

        let serialized =
            serde_json::to_string_pretty(&entry).context("Failed to serialize cache data")?;

        fs::write(cache_path, serialized).context("Failed to write cache file")?;

        Ok(())
    }

    pub async fn fetch_providers(&self) -> Result<HashMap<String, Provider>> {
        if let Some(cached) = self.load_from_cache()? {
            return Ok(cached);
        }

        let response = self
            .client
            .get(MODELS_DEV_API_URL)
            .send()
            .await
            .context("Failed to fetch from models.dev API")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Models.dev API returned error status: {}",
                response.status()
            ));
        }

        let providers: HashMap<String, Provider> = response
            .json()
            .await
            .context("Failed to parse models.dev API response")?;

        self.save_to_cache(&providers)?;

        Ok(providers)
    }

    pub async fn fetch_models(&self) -> Result<Vec<crate::model::types::Model>> {
        let providers = self.fetch_providers().await?;

        let mut models = Vec::new();

        for (provider_id, provider) in providers {
            for (model_id, model) in provider.models {
                let mut capabilities = Vec::new();

                if model.attachment {
                    capabilities.push("attachment".to_string());
                }
                if model.reasoning {
                    capabilities.push("reasoning".to_string());
                }
                if model.tool_call {
                    capabilities.push("tool_call".to_string());
                }
                if model.structured_output {
                    capabilities.push("structured_output".to_string());
                }

                let is_text_model = model.modalities.as_ref().map_or(true, |m| {
                    m.output.contains(&"text".to_string())
                        && !m.output.contains(&"image".to_string())
                });

                if is_text_model {
                    models.push(crate::model::types::Model {
                        id: model_id.clone(),
                        name: model.name.clone(),
                        provider_id: provider_id.clone(),
                        provider_name: provider.name.clone(),
                        capabilities,
                    });
                }
            }
        }

        Ok(models)
    }

    pub async fn list_models(&self, provider_filter: Option<&str>) -> Result<String> {
        let models = self.fetch_models().await?;

        let mut grouped: HashMap<String, Vec<&crate::model::types::Model>> = HashMap::new();

        for model in &models {
            if let Some(filter) = provider_filter {
                if !model.provider_id.contains(filter)
                    && !model.provider_name.to_lowercase().contains(filter)
                {
                    continue;
                }
            }

            grouped
                .entry(model.provider_name.clone())
                .or_default()
                .push(model);
        }

        if grouped.is_empty() {
            if let Some(filter) = provider_filter {
                return Ok(format!("No models found for provider: {}", filter));
            }
            return Ok("No models available".to_string());
        }

        let mut output = String::from("Available models:\n");

        let mut provider_names: Vec<_> = grouped.keys().collect();
        provider_names.sort();

        for provider_name in provider_names {
            output.push_str(&format!("  {}:\n", provider_name));

            let mut models: Vec<_> = grouped.get(provider_name).unwrap().clone();
            models.sort_by(|a, b| a.name.cmp(&b.name));

            for model in models {
                output.push_str(&format!("    - {} ({})", model.name, model.id));

                if !model.capabilities.is_empty() {
                    output.push_str(&format!(" [{}]", model.capabilities.join(", ")));
                }

                output.push('\n');
            }
        }

        Ok(output)
    }

    #[cfg(test)]
    pub fn cleanup_test() -> Result<()> {
        let cache_path = PathBuf::from("/tmp/crabcode_test_cache/models_dev_cache.json");
        if cache_path.exists() {
            fs::remove_file(&cache_path).context("Failed to remove test cache file")?;
        }
        Ok(())
    }
}

impl Default for Discovery {
    fn default() -> Self {
        Self::new().expect("Failed to create Discovery")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discovery_creation() {
        let discovery = Discovery::new();
        assert!(discovery.is_ok());
    }

    #[tokio::test]
    async fn test_fetch_providers() {
        let discovery = Discovery::new().unwrap();

        let providers = discovery.fetch_providers().await;

        if providers.is_ok() {
            let providers_map = providers.unwrap();
            assert!(!providers_map.is_empty());

            for (provider_id, provider) in providers_map.iter().take(1) {
                assert_eq!(provider.id, *provider_id);
                assert!(!provider.name.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn test_fetch_models() {
        let _ = Discovery::cleanup_test();
        let discovery = Discovery::new().unwrap();

        let models = discovery.fetch_models().await;

        if models.is_ok() {
            let model_list = models.unwrap();
            if !model_list.is_empty() {
                for model in model_list.iter().take(3) {
                    assert!(!model.id.is_empty());
                    assert!(!model.name.is_empty());
                    assert!(!model.provider_id.is_empty());
                    assert!(!model.provider_name.is_empty());
                }
            }
        }
        let _ = Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_list_models() {
        let _ = Discovery::cleanup_test();
        let discovery = Discovery::new().unwrap();

        let result = discovery.list_models(None).await;

        if result.is_ok() {
            let output = result.unwrap();
            assert!(output.contains("Available models:") || output.contains("No models available"));
        }
        let _ = Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_list_models_with_filter() {
        let discovery = Discovery::new().unwrap();

        let result = discovery.list_models(Some("open")).await;

        if result.is_ok() {
            let output = result.unwrap();
            assert!(output.contains("Available models:") || output.contains("No models found"));
        }
    }

    #[test]
    fn test_cache_entry_serialization() {
        let mut providers = HashMap::new();
        providers.insert(
            "test-provider".to_string(),
            Provider {
                id: "test-provider".to_string(),
                name: "Test Provider".to_string(),
                api: String::new(),
                doc: String::new(),
                env: Vec::new(),
                npm: String::new(),
                models: HashMap::new(),
            },
        );

        let entry = CacheEntry {
            data: providers.clone(),
            timestamp: 123456,
        };

        let serialized = serde_json::to_string(&entry).unwrap();
        let deserialized: CacheEntry = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.data.len(), 1);
        assert_eq!(deserialized.timestamp, 123456);
    }

    #[tokio::test]
    async fn test_cache_persistence() {
        let discovery = Discovery::new().unwrap();

        let cache_path = discovery.get_cache_path().clone();

        let test_data = {
            let mut providers = HashMap::new();
            providers.insert(
                "test-provider".to_string(),
                Provider {
                    id: "test-provider".to_string(),
                    name: "Test Provider".to_string(),
                    api: String::new(),
                    doc: String::new(),
                    env: Vec::new(),
                    npm: String::new(),
                    models: HashMap::new(),
                },
            );
            providers
        };

        discovery.save_to_cache(&test_data).unwrap();
        let loaded = discovery.load_from_cache().unwrap();

        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().len(), 1);

        let _ = fs::remove_file(cache_path);
    }
}
