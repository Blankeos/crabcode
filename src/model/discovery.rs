use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
const CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;
const CACHE_SCHEMA_VERSION: u32 = 5;

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

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static MEMORY_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<CacheEntry>>>> = OnceLock::new();
static MEMORY_MODEL_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedModels>>> = OnceLock::new();

#[derive(Clone)]
struct CachedModels {
    models: Vec<crate::model::types::Model>,
    cached_at: std::time::Instant,
}

fn shared_http_client() -> Result<Client> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client.clone());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;
    let _ = HTTP_CLIENT.set(client.clone());
    Ok(client)
}

fn memory_cache() -> &'static Mutex<HashMap<PathBuf, Arc<CacheEntry>>> {
    MEMORY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn memory_model_cache() -> &'static Mutex<HashMap<PathBuf, CachedModels>> {
    MEMORY_MODEL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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
    pub reasoning_options: Vec<crate::model::reasoning::ReasoningOption>,
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
    pub status: Option<String>,
    #[serde(default)]
    pub modalities: Option<Modalities>,
    #[serde(default)]
    pub open_weights: bool,
    #[serde(default)]
    pub cost: Option<Cost>,
    #[serde(default)]
    pub limit: Option<Limit>,
    #[serde(default)]
    pub provider: Option<ModelProvider>,
}

impl Model {
    pub fn reasoning_efforts(&self) -> Option<Vec<crate::model::reasoning::ReasoningEffort>> {
        crate::model::reasoning::efforts_from_options(&self.reasoning_options)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProvider {
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    data: HashMap<String, Provider>,
    timestamp: u64,
    #[serde(default)]
    schema_version: u32,
}

pub struct Discovery {
    client: Client,
    cache_path: PathBuf,
    custom_providers: Option<std::collections::HashMap<String, crate::config::CustomProviderConfig>>,
}
impl Discovery {
    pub fn new() -> Result<Self> {
        // Try to load custom providers from config file
        let custom_providers = crate::config::ConfigLoader::load()
            .map(|loaded| loaded.merged_config.custom_providers)
            .ok();
        Self::new_with_custom(custom_providers)
    }

    pub fn new_with_custom(
        custom_providers: Option<std::collections::HashMap<String, crate::config::CustomProviderConfig>>,
    ) -> Result<Self> {
        if cfg!(test) || env::var("CRABCODE_TEST_MODE").is_ok() {
            let cache_dir = PathBuf::from("/tmp/crabcode_test_cache");
            fs::create_dir_all(&cache_dir).context("Failed to create test cache directory")?;

            let cache_path = cache_dir.join("models_dev_cache.json");

            Ok(Self {
                client: shared_http_client()?,
                cache_path,
                custom_providers,
            })
        } else {
            crate::persistence::ensure_cache_dir().context("Failed to create cache directory")?;
            let cache_dir = crate::persistence::get_cache_dir();

            let cache_path = cache_dir.join("models_dev_cache.json");

            Ok(Self {
                client: shared_http_client()?,
                cache_path,
                custom_providers,
            })
        }
    }

    pub fn cache_path(&self) -> &PathBuf {
        &self.cache_path
    }

    fn get_cache_path(&self) -> &PathBuf {
        &self.cache_path
    }

    async fn fetch_from_api(&self) -> Result<HashMap<String, Provider>> {
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

        Ok(providers)
    }

    async fn fetch_with_internal_providers(
        &self,
        cached: Option<&HashMap<String, Provider>>,
    ) -> Result<HashMap<String, Provider>> {
        let mut providers = self.fetch_from_api().await?;
        crate::model::extensions::ModelExtensions::augment_persistent_catalog(
            &mut providers,
            cached,
            &self.client,
        )
        .await;
        Ok(providers)
    }

    fn load_cache_entry(&self) -> Result<Option<Arc<CacheEntry>>> {
        let cache_path = self.get_cache_path();

        if let Some(entry) = memory_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(cache_path).cloned())
        {
            return Ok(Some(entry));
        }

        if !cache_path.exists() {
            return Ok(None);
        }

        let cached_json = fs::read_to_string(cache_path).context("Failed to read cache file")?;

        let entry = Arc::new(
            serde_json::from_str::<CacheEntry>(&cached_json)
                .context("Failed to parse cache file")?,
        );

        if let Ok(mut cache) = memory_cache().lock() {
            cache.insert(cache_path.clone(), entry.clone());
        }

        Ok(Some(entry))
    }

    fn load_from_cache(&self) -> Result<Option<HashMap<String, Provider>>> {
        let Some(entry) = self.load_cache_entry()? else {
            return Ok(None);
        };

        if entry.schema_version < CACHE_SCHEMA_VERSION {
            return Ok(None);
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("System time is before UNIX epoch")?
            .as_secs();

        if now.saturating_sub(entry.timestamp) > CACHE_TTL_SECONDS {
            return Ok(None);
        }

        Ok(Some(entry.data.clone()))
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
            schema_version: CACHE_SCHEMA_VERSION,
        };

        let serialized =
            serde_json::to_string_pretty(&entry).context("Failed to serialize cache data")?;

        fs::write(cache_path, serialized).context("Failed to write cache file")?;

        if let Ok(mut cache) = memory_cache().lock() {
            cache.insert(cache_path.clone(), Arc::new(entry));
        }
        if let Ok(mut cache) = memory_model_cache().lock() {
            cache.remove(cache_path);
        }

        Ok(())
    }

    pub async fn fetch_providers(&self) -> Result<HashMap<String, Provider>> {
        let mut providers = if let Some(cached) = self.load_from_cache()? {
            let mut providers = cached;
            let mut cache_changed = false;
            cache_changed |= crate::model::extensions::ModelExtensions::augment_persistent_catalog(
                &mut providers,
                None,
                &self.client,
            )
            .await;
            if cache_changed {
                let _ = self.save_to_cache(&providers);
            }
            providers
        } else if cfg!(test) || env::var("CRABCODE_TEST_MODE").is_ok() {
            // In test mode, avoid hard network dependency so unit tests are reliable.
            match self.fetch_from_api().await {
                Ok(providers) => {
                    let mut providers = providers;
                    crate::model::extensions::ModelExtensions::augment_persistent_catalog(
                        &mut providers,
                        None,
                        &self.client,
                    )
                    .await;
                    let _ = self.save_to_cache(&providers);
                    providers
                }
                Err(_) => {
                    let mut providers: HashMap<String, Provider> = HashMap::new();
                    for (id, name) in [
                        ("opencode", "OpenCode"),
                        ("anthropic", "Anthropic"),
                        ("openai", "OpenAI"),
                        ("google", "Google"),
                    ] {
                        providers.insert(
                            id.to_string(),
                            Provider {
                                id: id.to_string(),
                                name: name.to_string(),
                                api: String::new(),
                                doc: String::new(),
                                env: Vec::new(),
                                npm: String::new(),
                                models: HashMap::new(),
                            },
                        );
                    }
                    providers
                }
            }
        } else {
            let providers = self.fetch_with_internal_providers(None).await?;
            self.save_to_cache(&providers)?;
            providers
        };

        crate::model::extensions::ModelExtensions::augment_runtime_catalog(&mut providers);
        // Apply custom providers from config as final overlay
        if let Some(custom) = &self.custom_providers {
            for (id, custom_provider) in custom {
                let provider = providers.entry(id.clone()).or_insert_with(|| -> Provider {
                    Provider {
                        id: id.clone(),
                        name: custom_provider.name.clone(),
                        api: String::new(),
                        doc: String::new(),
                        env: Vec::new(),
                        npm: String::new(),
                        models: HashMap::new(),
                    }
                });
                
                // Override with custom values if present
                if !custom_provider.name.is_empty() {
                    provider.name = custom_provider.name.clone();
                }
                if !custom_provider.base_url.is_empty() {
                    provider.api = custom_provider.base_url.clone();
                }
                if !custom_provider.npm.is_empty() {
                    provider.npm = custom_provider.npm.clone();
                }
                
                // Add/override models
                for (model_id, model_cfg) in &custom_provider.models {
                    let model = crate::model::discovery::Model {
                        id: String::from(model_id),
                        name: String::from(&model_cfg.name),
                        family: String::new(),
                        attachment: false,
                        reasoning: false,
                        reasoning_options: Vec::new(),
                        tool_call: false,
                        structured_output: false,
                        temperature: false,
                        knowledge: String::new(),
                        release_date: String::new(),
                        last_updated: String::new(),
                        status: None,
                        modalities: Some(crate::model::discovery::Modalities {
                            input: vec!["text".to_string()],
                            output: vec!["text".to_string()],
                        }),
                        open_weights: false,
                        cost: None,
                        limit: model_cfg.context_window.map(|cw| crate::model::discovery::Limit {
                            context: cw,
                            output: model_cfg.max_tokens.unwrap_or(cw),
                        }),
                        provider: Some(crate::model::discovery::ModelProvider {
                            npm: if custom_provider.npm.is_empty() { None } else { Some(custom_provider.npm.clone()) },
                            api: if custom_provider.base_url.is_empty() { None } else { Some(custom_provider.base_url.clone()) },
                        }),
                    };
                    provider.models.insert(String::from(model_id), model);
                }
            }
        }

        Ok(providers)
    }

    pub async fn refresh_cache(&self) -> Result<HashMap<String, Provider>> {
        let cached = self.load_from_cache().ok().flatten();
        let mut providers = self.fetch_with_internal_providers(cached.as_ref()).await?;
        self.save_to_cache(&providers)?;
        crate::model::extensions::ModelExtensions::augment_runtime_catalog(&mut providers);
        Ok(providers)
    }

    pub async fn fetch_models(&self) -> Result<Vec<crate::model::types::Model>> {
        let mut models = crate::model::extensions::ModelExtensions::runtime_models_from_cache();
        if let Some(cached) = memory_model_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(self.get_cache_path()).cloned())
            .filter(|cached| cached.cached_at.elapsed().as_secs() <= CACHE_TTL_SECONDS)
        {
            models.extend(cached.models);
            return Ok(models);
        }

        let providers = match self.fetch_providers().await {
            Ok(providers) => providers,
            Err(_err) if !models.is_empty() => return Ok(models),
            Err(err) => return Err(err),
        };

        let mut persistent_models = Vec::new();

        for (provider_id, provider) in providers {
            if crate::model::extensions::ModelExtensions::is_runtime_provider(&provider_id) {
                continue;
            }

            let provider_name = provider.name.clone();
            for (model_id, model) in provider.models {
                if matches!(model.status.as_deref(), Some("alpha" | "deprecated")) {
                    continue;
                }

                let free =
                    crate::model::extensions::ModelExtensions::is_unauthenticated_free_provider(
                        &provider_id,
                    ) && model.cost.as_ref().is_some_and(|cost| cost.input == 0.0);

                let is_text_model = model.modalities.as_ref().map_or(true, |m| {
                    m.output.contains(&"text".to_string())
                        && !m.output.contains(&"image".to_string())
                });

                if is_text_model {
                    persistent_models.push(crate::model::types::Model {
                        id: model_id.clone(),
                        name: model.name.clone(),
                        family: model.family.clone(),
                        provider_id: provider_id.clone(),
                        provider_name: provider_name.clone(),
                        attachment: model.attachment,
                        structured_output: model.structured_output,
                        free,
                        local: false,
                        reasoning_options: model.reasoning_options.clone(),
                    });
                }
            }
        }

        if let Ok(mut cache) = memory_model_cache().lock() {
            cache.insert(
                self.get_cache_path().clone(),
                CachedModels {
                    models: persistent_models.clone(),
                    cached_at: std::time::Instant::now(),
                },
            );
        }
        models.extend(persistent_models);

        Ok(models)
    }

    pub fn get_model_pricing(&self, provider_id: &str, model_id: &str) -> Option<Cost> {
        let entry = self.load_cache_entry().ok()??;
        let provider = entry.data.get(provider_id)?;
        let model = provider.models.get(model_id)?;
        model.cost.clone()
    }

    pub fn get_model_limit(&self, provider_id: &str, model_id: &str) -> Option<u32> {
        let entry = self.load_cache_entry().ok()??;
        let provider = entry.data.get(provider_id)?;
        let model = provider.models.get(model_id)?;
        model.limit.as_ref().map(|l| l.context)
    }

    pub fn get_model_reasoning_capability(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<crate::model::reasoning::ReasoningCapability> {
        let entry = self.load_cache_entry().ok()??;
        let provider = entry.data.get(provider_id)?;
        let model = provider.models.get(model_id)?;
        let provider_npm = model
            .provider
            .as_ref()
            .and_then(|provider| provider.npm.as_deref())
            .filter(|npm| !npm.trim().is_empty())
            .unwrap_or(provider.npm.as_str());
        Some(crate::model::reasoning::capability_for_model_with_options(
            provider_id,
            provider_npm,
            model_id,
            &model.id,
            &model.name,
            &model.family,
            &model.release_date,
            model.reasoning,
            &model.reasoning_options,
        ))
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

                let tags = model.display_tags();
                if !tags.is_empty() {
                    output.push_str(&format!(" [{}]", tags.join(", ")));
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

    fn unique_test_cache_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        PathBuf::from(format!(
            "/tmp/crabcode_test_cache/{}_{}_{}.json",
            name,
            std::process::id(),
            nanos
        ))
    }

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
            schema_version: CACHE_SCHEMA_VERSION,
        };

        let serialized = serde_json::to_string(&entry).unwrap();
        let deserialized: CacheEntry = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.data.len(), 1);
        assert_eq!(deserialized.timestamp, 123456);
        assert_eq!(deserialized.schema_version, CACHE_SCHEMA_VERSION);
    }

    #[test]
    fn test_model_provider_override_deserialization() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "qwen3.7-max",
            "name": "Qwen3.7 Max",
            "release_date": "2026-05-21",
            "last_updated": "2026-05-21",
            "provider": {
                "npm": "@ai-sdk/anthropic"
            }
        }))
        .unwrap();

        let provider = model.provider.expect("provider override");
        assert_eq!(provider.npm.as_deref(), Some("@ai-sdk/anthropic"));
        assert_eq!(provider.api, None);
    }

    #[test]
    fn model_reasoning_options_deserialize_effort_values() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "grok-4.5",
            "name": "Grok 4.5",
            "reasoning": true,
            "reasoning_options": [
                { "type": "effort", "values": ["low", "medium", "high"] },
                { "type": "budget_tokens", "min": 1024 }
            ]
        }))
        .unwrap();

        assert_eq!(
            model.reasoning_efforts().as_deref(),
            Some(
                &[
                    crate::model::reasoning::ReasoningEffort::Low,
                    crate::model::reasoning::ReasoningEffort::Medium,
                    crate::model::reasoning::ReasoningEffort::High,
                ][..]
            )
        );
    }

    #[test]
    fn model_reasoning_options_ignore_non_string_values() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "odd-model",
            "name": "Odd Model",
            "reasoning": true,
            "reasoning_options": [
                { "type": "effort", "values": ["low", null, "default", "high"] }
            ]
        }))
        .unwrap();

        assert_eq!(
            model.reasoning_efforts().as_deref(),
            Some(
                &[
                    crate::model::reasoning::ReasoningEffort::Low,
                    crate::model::reasoning::ReasoningEffort::High,
                ][..]
            )
        );
    }

    #[tokio::test]
    async fn fetch_models_filters_deprecated_models() {
        let mut discovery = Discovery::new().unwrap();
        let cache_path = unique_test_cache_path("deprecated_model_filter");
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        discovery.cache_path = cache_path.clone();

        let mut models = HashMap::new();
        models.insert(
            "big-pickle".to_string(),
            serde_json::from_value(serde_json::json!({
                "id": "big-pickle",
                "name": "Big Pickle",
                "release_date": "2025-10-17",
                "last_updated": "2025-10-17",
                "attachment": false,
                "reasoning": true,
                "temperature": true,
                "tool_call": true,
                "cost": { "input": 0.0, "output": 0.0 },
                "modalities": { "input": ["text"], "output": ["text"] }
            }))
            .unwrap(),
        );
        models.insert(
            "kimi-k2.5-free".to_string(),
            serde_json::from_value(serde_json::json!({
                "id": "kimi-k2.5-free",
                "name": "Kimi K2.5 Free",
                "release_date": "2026-01-27",
                "last_updated": "2026-01-27",
                "status": "deprecated",
                "attachment": true,
                "reasoning": true,
                "temperature": true,
                "tool_call": true,
                "cost": { "input": 0.0, "output": 0.0 },
                "modalities": { "input": ["text"], "output": ["text"] }
            }))
            .unwrap(),
        );

        let mut providers = HashMap::new();
        providers.insert(
            "opencode".to_string(),
            Provider {
                id: "opencode".to_string(),
                name: "OpenCode Zen".to_string(),
                api: "https://opencode.ai/zen/v1".to_string(),
                doc: String::new(),
                env: vec!["OPENCODE_API_KEY".to_string()],
                npm: "@ai-sdk/openai-compatible".to_string(),
                models,
            },
        );
        discovery.save_to_cache(&providers).unwrap();

        let model_ids: Vec<_> = discovery
            .fetch_models()
            .await
            .unwrap()
            .into_iter()
            .map(|model| model.id)
            .collect();

        assert!(model_ids.contains(&"big-pickle".to_string()));
        assert!(!model_ids.contains(&"kimi-k2.5-free".to_string()));

        let _ = fs::remove_file(cache_path);
    }

    #[tokio::test]
    async fn cached_xai_provider_is_migrated_and_saved_with_composer() {
        const XAI_PROVIDER_ID: &str = "xai";
        const GROK_COMPOSER_2_5_FAST_ID: &str = "grok-composer-2.5-fast";

        let mut discovery = Discovery::new().unwrap();
        let cache_path = unique_test_cache_path("xai_composer_cache_migration");
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        discovery.cache_path = cache_path.clone();

        let mut providers = HashMap::new();
        providers.insert(
            XAI_PROVIDER_ID.to_string(),
            Provider {
                id: XAI_PROVIDER_ID.to_string(),
                name: "xAI".to_string(),
                api: String::new(),
                doc: String::new(),
                env: vec!["XAI_API_KEY".to_string()],
                npm: "@ai-sdk/xai".to_string(),
                models: HashMap::new(),
            },
        );
        discovery.save_to_cache(&providers).unwrap();

        let loaded = discovery.fetch_providers().await.unwrap();
        assert!(loaded
            .get(XAI_PROVIDER_ID)
            .is_some_and(|provider| { provider.models.contains_key(GROK_COMPOSER_2_5_FAST_ID) }));

        let cached = discovery.load_from_cache().unwrap().unwrap();
        assert!(cached
            .get(XAI_PROVIDER_ID)
            .is_some_and(|provider| { provider.models.contains_key(GROK_COMPOSER_2_5_FAST_ID) }));

        let _ = fs::remove_file(cache_path);
    }

    #[tokio::test]
    async fn test_cache_persistence() {
        let mut discovery = Discovery::new().unwrap();
        let cache_path = unique_test_cache_path("models_dev_cache_persistence");
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        discovery.cache_path = cache_path.clone();

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

    #[test]
    fn parsed_cache_is_reused_from_memory() {
        let mut discovery = Discovery::new().unwrap();
        let cache_path = unique_test_cache_path("models_dev_memory_cache");
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        discovery.cache_path = cache_path.clone();

        let providers = HashMap::from([(
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
        )]);
        discovery.save_to_cache(&providers).unwrap();

        let first = discovery.load_cache_entry().unwrap().unwrap();
        let second = discovery.load_cache_entry().unwrap().unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        let _ = fs::remove_file(cache_path);
    }
}
