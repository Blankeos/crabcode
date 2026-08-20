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
    pub header: Vec<(String, String)>,
    #[serde(default)]
    pub models: HashMap<String, Model>,
}

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static MEMORY_CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<CacheEntry>>>> = OnceLock::new();
static MEMORY_MODEL_CACHE: OnceLock<Mutex<HashMap<(PathBuf, Vec<String>), CachedModels>>> =
    OnceLock::new();

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

fn memory_model_cache() -> &'static Mutex<HashMap<(PathBuf, Vec<String>), CachedModels>> {
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
    custom_providers:
        Option<std::collections::HashMap<String, crate::config::CustomProviderConfig>>,
    disabled_providers: std::collections::BTreeSet<String>,
    enabled_providers: std::collections::BTreeSet<String>,
}

pub fn is_model_selectable(
    model: &crate::model::types::Model,
    connected_provider_ids: &std::collections::HashSet<String>,
    configured_provider_ids: &std::collections::HashSet<String>,
) -> bool {
    connected_provider_ids.contains(&model.provider_id)
        || configured_provider_ids.contains(&model.provider_id)
        || crate::model::extensions::ModelExtensions::is_available_without_connection(model)
}

pub fn merge_dialog_models(
    models: &mut Vec<crate::model::types::Model>,
    additional_models: impl IntoIterator<Item = crate::model::types::Model>,
) {
    let mut known = models
        .iter()
        .map(|model| (model.provider_id.clone(), model.id.clone()))
        .collect::<std::collections::HashSet<_>>();
    for model in additional_models {
        if known.insert((model.provider_id.clone(), model.id.clone())) {
            models.push(model);
        }
    }
}

impl Discovery {
    pub fn custom_provider_ids(&self) -> std::collections::HashSet<String> {
        self.custom_providers
            .as_ref()
            .map(|providers| providers.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn custom_provider_dialog_signature(&self) -> Vec<String> {
        let Some(custom_providers) = &self.custom_providers else {
            return Vec::new();
        };

        let mut signature = Vec::new();
        for (provider_id, provider) in custom_providers {
            signature.push(format!(
                "provider:{provider_id}:{}",
                provider.name.as_deref().unwrap_or_default()
            ));
            for (model_id, model) in &provider.models {
                let mut input_modalities = model
                    .modalities
                    .as_ref()
                    .map(|modalities| modalities.input.clone())
                    .unwrap_or_default();
                input_modalities.sort();
                let mut output_modalities = model
                    .modalities
                    .as_ref()
                    .map(|modalities| modalities.output.clone())
                    .unwrap_or_default();
                output_modalities.sort();
                signature.push(format!(
                    "model:{provider_id}:{model_id}:{}:{:?}:{}:{}",
                    model.name.as_deref().unwrap_or_default(),
                    model.attachment,
                    input_modalities.join(","),
                    output_modalities.join(",")
                ));
            }
        }
        signature.sort();
        signature
    }

    pub fn custom_provider_matches_filter(&self, filter: &str) -> bool {
        let filter = filter.trim().to_ascii_lowercase();
        self.custom_providers.as_ref().is_some_and(|providers| {
            providers.iter().any(|(provider_id, provider)| {
                provider_id.contains(&filter)
                    || provider
                        .name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains(&filter))
            })
        })
    }

    pub fn apply_custom_models_to_dialog(&self, models: &mut Vec<crate::model::types::Model>) {
        let Some(custom_providers) = &self.custom_providers else {
            return;
        };

        for (provider_id, provider) in custom_providers {
            let provider_name = provider.name.as_deref().unwrap_or(provider_id);
            for (model_id, custom_model) in &provider.models {
                let model = models
                    .iter_mut()
                    .find(|model| model.provider_id == *provider_id && model.id == *model_id);

                if let Some(model) = model {
                    model.provider_name = provider_name.to_string();
                    if let Some(name) = &custom_model.name {
                        model.name.clone_from(name);
                    }
                    if let Some(modalities) = &custom_model.modalities {
                        model.attachment = modalities.input.iter().any(|input| input == "image");
                    }
                    if let Some(attachment) = custom_model.attachment {
                        model.attachment = attachment;
                    }
                    continue;
                }

                let attachment = custom_model.attachment.unwrap_or_else(|| {
                    custom_model.modalities.as_ref().is_some_and(|modalities| {
                        modalities.input.iter().any(|input| input == "image")
                    })
                });
                models.push(crate::model::types::Model {
                    id: model_id.clone(),
                    name: custom_model
                        .name
                        .clone()
                        .unwrap_or_else(|| model_id.clone()),
                    family: String::new(),
                    provider_id: provider_id.clone(),
                    provider_name: provider_name.to_string(),
                    attachment,
                    structured_output: false,
                    free: false,
                    local: false,
                    reasoning_options: Vec::new(),
                    context_window: custom_model.context_window,
                });
            }
        }
    }

    pub fn custom_provider_api_key(&self, provider_id: &str) -> Option<String> {
        self.custom_providers
            .as_ref()?
            .get(&provider_id.trim().to_ascii_lowercase())?
            .resolved_api_key()
    }

    pub fn new() -> Result<Self> {
        let loaded = crate::config::ConfigLoader::load().ok();
        let custom_providers = loaded
            .as_ref()
            .map(|loaded| loaded.merged_config.custom_providers.clone());
        let disabled_providers = loaded
            .as_ref()
            .map(|loaded| loaded.merged_config.disabled_providers.clone())
            .unwrap_or_default();
        let enabled_providers = loaded
            .as_ref()
            .map(|loaded| loaded.merged_config.enabled_providers.clone())
            .unwrap_or_default();
        Self::new_with_config(custom_providers, disabled_providers, enabled_providers)
    }

    pub fn new_with_custom(
        custom_providers: Option<
            std::collections::HashMap<String, crate::config::CustomProviderConfig>,
        >,
    ) -> Result<Self> {
        Self::new_with_config(custom_providers, Default::default(), Default::default())
    }

    pub fn new_with_config(
        custom_providers: Option<
            std::collections::HashMap<String, crate::config::CustomProviderConfig>,
        >,
        disabled_providers: std::collections::BTreeSet<String>,
        enabled_providers: std::collections::BTreeSet<String>,
    ) -> Result<Self> {
        if cfg!(test) || env::var("CRABCODE_TEST_MODE").is_ok() {
            let cache_dir = PathBuf::from("/tmp/crabcode_test_cache");
            fs::create_dir_all(&cache_dir).context("Failed to create test cache directory")?;

            let cache_path = cache_dir.join("models_dev_cache.json");

            Ok(Self {
                client: shared_http_client()?,
                cache_path,
                custom_providers,
                disabled_providers,
                enabled_providers,
            })
        } else {
            crate::persistence::ensure_cache_dir().context("Failed to create cache directory")?;
            let cache_dir = crate::persistence::get_cache_dir();

            let cache_path = cache_dir.join("models_dev_cache.json");

            Ok(Self {
                client: shared_http_client()?,
                cache_path,
                custom_providers,
                disabled_providers,
                enabled_providers,
            })
        }
    }

    pub fn provider_is_enabled(&self, provider_id: &str) -> bool {
        !self.disabled_providers.contains(provider_id)
            && (self.enabled_providers.is_empty() || self.enabled_providers.contains(provider_id))
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
            cache.retain(|(path, _), _| path != cache_path);
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
                                header: vec![],
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
        self.apply_custom_provider_overlays(&mut providers);

        Ok(providers)
    }

    pub async fn refresh_cache(&self) -> Result<HashMap<String, Provider>> {
        let cached = self.load_from_cache().ok().flatten();
        let mut providers = self.fetch_with_internal_providers(cached.as_ref()).await?;
        self.save_to_cache(&providers)?;
        crate::model::extensions::ModelExtensions::augment_runtime_catalog(&mut providers);
        self.apply_custom_provider_overlays(&mut providers);
        Ok(providers)
    }

    fn apply_custom_provider_overlays(&self, providers: &mut HashMap<String, Provider>) {
        let Some(custom_providers) = &self.custom_providers else {
            return;
        };

        for (provider_id, custom_provider) in custom_providers {
            let provider = providers
                .entry(provider_id.clone())
                .or_insert_with(|| Provider {
                    id: provider_id.clone(),
                    name: custom_provider
                        .name
                        .clone()
                        .unwrap_or_else(|| provider_id.clone()),
                    api: String::new(),
                    doc: String::new(),
                    env: Vec::new(),
                    npm: String::new(),
                    header: vec![],
                    models: HashMap::new(),
                });

            if let Some(name) = &custom_provider.name {
                provider.name.clone_from(name);
            }
            if let Some(base_url) = &custom_provider.base_url {
                provider.api.clone_from(base_url);
            }
            if let Some(npm) = &custom_provider.npm {
                provider.npm.clone_from(npm);
            }

            for (model_id, custom_model) in &custom_provider.models {
                let model = provider
                    .models
                    .entry(model_id.clone())
                    .or_insert_with(|| Model {
                        id: model_id.clone(),
                        name: custom_model
                            .name
                            .clone()
                            .unwrap_or_else(|| model_id.clone()),
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
                        modalities: Some(Modalities {
                            input: vec!["text".to_string()],
                            output: vec!["text".to_string()],
                        }),
                        open_weights: false,
                        cost: None,
                        limit: None,
                        provider: None,
                    });

                if let Some(name) = &custom_model.name {
                    model.name.clone_from(name);
                }
                if let Some(reasoning) = custom_model.reasoning {
                    model.reasoning = reasoning;
                }
                if let Some(reasoning_options) = &custom_model.reasoning_options {
                    model.reasoning_options.clone_from(reasoning_options);
                }
                if let Some(temperature) = custom_model.temperature {
                    model.temperature = temperature;
                }
                if let Some(tool_call) = custom_model.tool_call {
                    model.tool_call = tool_call;
                }
                if let Some(modalities) = &custom_model.modalities {
                    model.modalities = Some(Modalities {
                        input: modalities.input.clone(),
                        output: modalities.output.clone(),
                    });
                    model.attachment = modalities.input.iter().any(|input| input == "image");
                }
                if let Some(attachment) = custom_model.attachment {
                    model.attachment = attachment;
                    if attachment {
                        let modalities = model.modalities.get_or_insert_with(|| Modalities {
                            input: vec!["text".to_string()],
                            output: vec!["text".to_string()],
                        });
                        if !modalities.input.iter().any(|input| input == "image") {
                            modalities.input.push("image".to_string());
                        }
                    } else if let Some(modalities) = model.modalities.as_mut() {
                        modalities.input.retain(|input| input != "image");
                    }
                }
                if custom_model.context_window.is_some() || custom_model.max_tokens.is_some() {
                    let current = model.limit.as_ref();
                    if let Some(context) = custom_model
                        .context_window
                        .or_else(|| current.map(|limit| limit.context))
                    {
                        model.limit = Some(Limit {
                            context,
                            output: custom_model
                                .max_tokens
                                .or_else(|| current.map(|limit| limit.output))
                                .unwrap_or(context),
                        });
                    }
                }

                if custom_provider.npm.is_some() || custom_provider.base_url.is_some() {
                    let model_provider = model.provider.get_or_insert_with(|| ModelProvider {
                        npm: None,
                        api: None,
                    });
                    if let Some(npm) = &custom_provider.npm {
                        model_provider.npm = Some(npm.clone());
                    }
                    if let Some(base_url) = &custom_provider.base_url {
                        model_provider.api = Some(base_url.clone());
                    }
                }
            }
        }
    }

    pub async fn fetch_models(&self) -> Result<Vec<crate::model::types::Model>> {
        let mut models = crate::model::extensions::ModelExtensions::runtime_models_from_cache();
        models.retain(|model| self.provider_is_enabled(&model.provider_id));
        let cache_key = (
            self.get_cache_path().clone(),
            self.custom_provider_dialog_signature(),
        );
        if let Some(cached) = memory_model_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&cache_key).cloned())
            .filter(|cached| cached.cached_at.elapsed().as_secs() <= CACHE_TTL_SECONDS)
        {
            models.extend(cached.models);
            models.retain(|model| self.provider_is_enabled(&model.provider_id));
            return Ok(models);
        }

        let providers = match self.fetch_providers().await {
            Ok(providers) => providers,
            Err(_err) if !models.is_empty() => return Ok(models),
            Err(err) => return Err(err),
        };

        let mut persistent_models = Vec::new();

        for (provider_id, provider) in providers {
            if !self.provider_is_enabled(&provider_id) {
                continue;
            }
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
                        context_window: model
                            .limit
                            .as_ref()
                            .map(|limit| limit.context)
                            .filter(|context| *context > 0),
                    });
                }
            }
        }

        if let Ok(mut cache) = memory_model_cache().lock() {
            cache.insert(
                cache_key,
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
    use crate::config::configuration::{
        CustomModelConfig, CustomModelModalities, CustomProviderConfig,
    };

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

    #[test]
    fn configured_provider_models_are_selectable_without_auth() {
        let model = crate::model::types::Model {
            id: "configured-model".to_string(),
            name: "Configured Model".to_string(),
            family: String::new(),
            provider_id: "configured-provider".to_string(),
            provider_name: "Configured Provider".to_string(),
            attachment: false,
            structured_output: false,
            free: false,
            local: false,
            reasoning_options: Vec::new(),
            context_window: None,
        };
        let connected_provider_ids = std::collections::HashSet::new();
        let configured_provider_ids =
            std::collections::HashSet::from(["configured-provider".to_string()]);

        assert!(is_model_selectable(
            &model,
            &connected_provider_ids,
            &configured_provider_ids,
        ));
        assert!(!is_model_selectable(
            &model,
            &connected_provider_ids,
            &std::collections::HashSet::new(),
        ));

        let mut models = vec![model.clone()];
        merge_dialog_models(&mut models, [model]);
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn custom_provider_ids_and_names_match_model_filters() {
        let providers = HashMap::from([(
            "mygateway".to_string(),
            CustomProviderConfig {
                name: Some("My Gateway".to_string()),
                npm: None,
                base_url: None,
                api_key: None,
                models: HashMap::from([(
                    "configured-model".to_string(),
                    CustomModelConfig {
                        name: Some("Configured Model".to_string()),
                        context_window: None,
                        max_tokens: None,
                        attachment: Some(true),
                        reasoning: None,
                        reasoning_options: None,
                        temperature: None,
                        tool_call: None,
                        modalities: None,
                        launch: false,
                    },
                )]),
            },
        )]);
        let discovery = Discovery::new_with_custom(Some(providers)).expect("discovery");

        assert_eq!(
            discovery.custom_provider_ids(),
            std::collections::HashSet::from(["mygateway".to_string()])
        );
        assert!(discovery.custom_provider_matches_filter("mygateway"));
        assert!(discovery.custom_provider_matches_filter("gateway"));
        assert!(!discovery.custom_provider_matches_filter("openai"));
        assert_eq!(
            discovery.custom_provider_dialog_signature(),
            vec![
                "model:mygateway:configured-model:Configured Model:Some(true)::".to_string(),
                "provider:mygateway:My Gateway".to_string(),
            ]
        );

        let mut models = Vec::new();
        discovery.apply_custom_models_to_dialog(&mut models);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider_id, "mygateway");
        assert_eq!(models[0].provider_name, "My Gateway");
        assert_eq!(models[0].id, "configured-model");
        assert_eq!(models[0].name, "Configured Model");
        assert!(models[0].attachment);
    }

    #[test]
    fn custom_model_overlay_preserves_unspecified_catalog_metadata() {
        let mut providers = HashMap::from([(
            "custom".to_string(),
            Provider {
                id: "custom".to_string(),
                name: "Catalog Provider".to_string(),
                api: "https://catalog.example/v1".to_string(),
                doc: "https://catalog.example/docs".to_string(),
                env: vec!["CATALOG_KEY".to_string()],
                npm: "@ai-sdk/openai-compatible".to_string(),
                header: vec![],
                models: HashMap::from([(
                    "vision-model".to_string(),
                    Model {
                        id: "vision-model".to_string(),
                        name: "Catalog Model".to_string(),
                        family: "catalog-family".to_string(),
                        attachment: true,
                        reasoning: true,
                        reasoning_options: Vec::new(),
                        tool_call: true,
                        structured_output: true,
                        temperature: true,
                        knowledge: "2025-01".to_string(),
                        release_date: "2025-01-01".to_string(),
                        last_updated: "2025-02-01".to_string(),
                        status: None,
                        modalities: Some(Modalities {
                            input: vec!["text".to_string(), "image".to_string()],
                            output: vec!["text".to_string()],
                        }),
                        open_weights: false,
                        cost: None,
                        limit: Some(Limit {
                            context: 64000,
                            output: 4096,
                        }),
                        provider: Some(ModelProvider {
                            npm: Some("@ai-sdk/openai-compatible".to_string()),
                            api: Some("https://catalog.example/v1".to_string()),
                        }),
                    },
                )]),
            },
        )]);
        let custom_providers = HashMap::from([(
            "custom".to_string(),
            CustomProviderConfig {
                name: None,
                npm: None,
                base_url: None,
                api_key: None,
                models: HashMap::from([(
                    "vision-model".to_string(),
                    CustomModelConfig {
                        name: Some("Configured Model".to_string()),
                        context_window: Some(128000),
                        max_tokens: None,
                        attachment: None,
                        reasoning: None,
                        reasoning_options: Some(vec![crate::model::reasoning::ReasoningOption {
                            kind: "effort".to_string(),
                            values: vec!["low".to_string(), "max".to_string()],
                        }]),
                        temperature: None,
                        tool_call: None,
                        modalities: None,
                        launch: false,
                    },
                )]),
            },
        )]);
        let discovery = Discovery::new_with_custom(Some(custom_providers)).expect("discovery");

        discovery.apply_custom_provider_overlays(&mut providers);

        let provider = providers.get("custom").expect("provider");
        assert_eq!(provider.name, "Catalog Provider");
        assert_eq!(provider.api, "https://catalog.example/v1");
        assert_eq!(provider.npm, "@ai-sdk/openai-compatible");
        let model = provider.models.get("vision-model").expect("model");
        assert_eq!(model.name, "Configured Model");
        assert!(model.attachment);
        assert!(model.reasoning);
        assert_eq!(
            model.reasoning_options,
            vec![crate::model::reasoning::ReasoningOption {
                kind: "effort".to_string(),
                values: vec!["low".to_string(), "max".to_string()],
            }]
        );
        assert!(model.tool_call);
        assert!(model.structured_output);
        assert!(model.temperature);
        assert_eq!(model.family, "catalog-family");
        assert_eq!(
            model
                .modalities
                .as_ref()
                .map(|value| value.input.as_slice()),
            Some(["text".to_string(), "image".to_string()].as_slice())
        );
        assert_eq!(
            model.limit.as_ref().map(|limit| limit.context),
            Some(128000)
        );
        assert_eq!(model.limit.as_ref().map(|limit| limit.output), Some(4096));
    }

    #[test]
    fn custom_model_modalities_enable_image_input() {
        let mut providers = HashMap::new();
        let custom_providers = HashMap::from([(
            "custom".to_string(),
            CustomProviderConfig {
                name: None,
                npm: Some("@ai-sdk/openai-compatible".to_string()),
                base_url: Some("https://example.com/v1".to_string()),
                api_key: None,
                models: HashMap::from([(
                    "vision-model".to_string(),
                    CustomModelConfig {
                        name: None,
                        context_window: Some(128000),
                        max_tokens: Some(8192),
                        attachment: None,
                        reasoning: Some(true),
                        reasoning_options: Some(vec![crate::model::reasoning::ReasoningOption {
                            kind: "effort".to_string(),
                            values: vec!["none".to_string(), "high".to_string()],
                        }]),
                        temperature: Some(true),
                        tool_call: Some(true),
                        modalities: Some(CustomModelModalities {
                            input: vec!["text".to_string(), "image".to_string()],
                            output: vec!["text".to_string()],
                        }),
                        launch: false,
                    },
                )]),
            },
        )]);
        let discovery = Discovery::new_with_custom(Some(custom_providers)).expect("discovery");

        discovery.apply_custom_provider_overlays(&mut providers);

        let model = providers["custom"]
            .models
            .get("vision-model")
            .expect("model");
        assert!(model.attachment);
        assert!(model.reasoning);
        assert_eq!(
            model.reasoning_options,
            vec![crate::model::reasoning::ReasoningOption {
                kind: "effort".to_string(),
                values: vec!["none".to_string(), "high".to_string()],
            }]
        );
        assert!(model.temperature);
        assert!(model.tool_call);
        assert_eq!(
            model.limit.as_ref().map(|limit| limit.context),
            Some(128000)
        );
        assert_eq!(model.limit.as_ref().map(|limit| limit.output), Some(8192));
    }

    #[test]
    fn custom_model_attachment_flag_updates_modalities() {
        let mut providers = HashMap::new();
        let custom_providers = HashMap::from([(
            "custom".to_string(),
            CustomProviderConfig {
                name: None,
                npm: None,
                base_url: None,
                api_key: None,
                models: HashMap::from([(
                    "vision-model".to_string(),
                    CustomModelConfig {
                        name: None,
                        context_window: None,
                        max_tokens: None,
                        attachment: Some(true),
                        reasoning: None,
                        reasoning_options: None,
                        temperature: None,
                        tool_call: None,
                        modalities: None,
                        launch: false,
                    },
                )]),
            },
        )]);
        let discovery = Discovery::new_with_custom(Some(custom_providers)).expect("discovery");

        discovery.apply_custom_provider_overlays(&mut providers);

        let model = providers["custom"]
            .models
            .get("vision-model")
            .expect("model");
        assert!(model.attachment);
        assert!(model
            .modalities
            .as_ref()
            .is_some_and(|modalities| modalities.input.iter().any(|input| input == "image")));
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
                header: vec![],
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
            "stable-model".to_string(),
            serde_json::from_value(serde_json::json!({
                "id": "stable-model",
                "name": "Stable Model",
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
            "fixture-provider".to_string(),
            Provider {
                id: "fixture-provider".to_string(),
                name: "Fixture Provider".to_string(),
                api: "https://example.invalid/v1".to_string(),
                doc: String::new(),
                env: Vec::new(),
                npm: "@ai-sdk/openai-compatible".to_string(),
                header: vec![],
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

        assert!(
            model_ids.contains(&"stable-model".to_string()),
            "expected stable model in {model_ids:?}"
        );
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
                header: vec![],
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
                    header: vec![],
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
                header: vec![],
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
