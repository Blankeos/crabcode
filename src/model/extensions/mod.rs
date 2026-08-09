use anyhow::Result;
use reqwest::Client;
use std::collections::HashMap;

use crate::model::discovery::{Model, Provider};

pub mod commandcode;
pub mod ollama;

const CATALOG_EXTENSIONS_JSON: &str = include_str!("catalog_extensions.jsonc");
static CATALOG_JSON_EXTENSION: CatalogJsonExtension = CatalogJsonExtension;
static PERSISTENT_EXTENSIONS: [&dyn PersistentProviderCatalogExtension; 2] =
    [&commandcode::EXTENSION, &CATALOG_JSON_EXTENSION];
static RUNTIME_EXTENSIONS: [&dyn RuntimeProviderCatalogExtension; 1] = [&ollama::EXTENSION];

/// Model provider catalog extensions that are not available directly from
/// models.dev.
///
/// Persistent extensions are folded into `models_dev_cache.json`, so they behave
/// like normal catalog data after `/refreshmodels`. Runtime extensions stay
/// outside that cache because their model list depends on local machine state.
///
/// There are only three types of provider extensions:
/// - catalog_extensions via catalog_extensions.json i.e. composer 2.5
/// - runtime via `RuntimeProviderCatalogExtension` i.e. ollama, lm studio (future)
/// - remote via `RemoteProviderCatalogExtension` i.e. commandcode
pub struct ModelExtensions;

pub trait ProviderCatalogExtension: Sync {
    fn provider_id(&self) -> &'static str;
    fn provider_name(&self) -> &'static str;
    fn provider_description(&self) -> &'static str {
        self.provider_name()
    }
}

pub trait PersistentProviderCatalogExtension: ProviderCatalogExtension {
    fn augment<'a>(
        &'a self,
        providers: &'a mut HashMap<String, Provider>,
        cached: Option<&'a HashMap<String, Provider>>,
        client: &'a Client,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>>;
}

pub trait RuntimeProviderCatalogExtension: ProviderCatalogExtension {
    fn provider(&self) -> Provider;

    fn augment_catalog(&self, providers: &mut HashMap<String, Provider>) {
        providers.insert(self.provider_id().to_string(), self.provider());
    }

    fn refresh_models<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<RefreshSummary>> + Send + 'a>>;

    fn models_from_cache(&self) -> Vec<crate::model::types::Model>;

    fn models_for_dialog_cached<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<crate::model::types::Model>>> + Send + 'a>,
    >;
}

impl ModelExtensions {
    pub fn persistent() -> &'static [&'static dyn PersistentProviderCatalogExtension] {
        &PERSISTENT_EXTENSIONS
    }

    pub fn runtime() -> &'static [&'static dyn RuntimeProviderCatalogExtension] {
        &RUNTIME_EXTENSIONS
    }

    pub async fn augment_persistent_catalog(
        providers: &mut HashMap<String, Provider>,
        cached: Option<&HashMap<String, Provider>>,
        client: &Client,
    ) -> bool {
        let mut changed = false;
        for extension in Self::persistent() {
            changed |= extension.augment(providers, cached, client).await;
        }
        changed
    }

    pub fn augment_runtime_catalog(providers: &mut HashMap<String, Provider>) {
        for extension in Self::runtime() {
            extension.augment_catalog(providers);
        }
    }

    pub async fn refresh_runtime_models() -> Vec<RefreshResult> {
        let mut results = Vec::new();
        for extension in Self::runtime() {
            let result = match extension.refresh_models().await {
                Ok(summary) => RefreshResult::Refreshed {
                    provider_id: extension.provider_id(),
                    provider_name: extension.provider_name(),
                    model_count: summary.model_count,
                },
                Err(err) => RefreshResult::Skipped {
                    provider_id: extension.provider_id(),
                    provider_name: extension.provider_name(),
                    error: err.to_string(),
                },
            };
            results.push(result);
        }
        results
    }

    pub fn runtime_models_from_cache() -> Vec<crate::model::types::Model> {
        Self::runtime()
            .iter()
            .flat_map(|extension| extension.models_from_cache())
            .collect()
    }

    pub async fn runtime_models_for_dialog_cached() -> RuntimeDialogModelsResult {
        use futures::future::join_all;

        let mut models = Vec::new();
        let mut errors = Vec::new();

        let discoveries = Self::runtime().iter().map(|extension| async move {
            (*extension, extension.models_for_dialog_cached().await)
        });

        for (extension, result) in join_all(discoveries).await {
            match result {
                Ok(provider_models) => models.extend(provider_models),
                Err(err) => errors.push(ProviderExtensionError {
                    provider_id: extension.provider_id(),
                    provider_name: extension.provider_name(),
                    error: err.to_string(),
                }),
            }
        }

        RuntimeDialogModelsResult { models, errors }
    }

    pub async fn runtime_models_for_dialog_cached_or_empty() -> Vec<crate::model::types::Model> {
        Self::runtime_models_for_dialog_cached().await.models
    }

    pub fn is_runtime_provider(provider_id: &str) -> bool {
        Self::runtime()
            .iter()
            .any(|extension| extension.provider_id() == provider_id)
    }

    pub fn is_unauthenticated_free_provider(provider_id: &str) -> bool {
        provider_id == "opencode"
    }

    pub fn unauthenticated_free_provider_matches_filter(filter: &str) -> bool {
        let filter = filter.to_ascii_lowercase();
        ["opencode", "opencode zen"]
            .iter()
            .any(|provider| provider.contains(&filter))
    }

    pub fn is_unauthenticated_free_model(model: &crate::model::types::Model) -> bool {
        Self::is_unauthenticated_free_provider(&model.provider_id) && model.free
    }

    pub fn is_available_without_connection(model: &crate::model::types::Model) -> bool {
        model.local
            || Self::is_runtime_provider(&model.provider_id)
            || Self::is_unauthenticated_free_model(model)
    }

    pub fn model_matches_provider_filter(
        model: &crate::model::types::Model,
        provider_filter: Option<&str>,
    ) -> bool {
        provider_filter.is_none_or(|filter| {
            let filter = filter.to_ascii_lowercase();
            model.provider_id.to_ascii_lowercase().contains(&filter)
                || model.provider_name.to_ascii_lowercase().contains(&filter)
        })
    }

    pub fn runtime_provider(
        provider_id: &str,
    ) -> Option<&'static dyn RuntimeProviderCatalogExtension> {
        Self::runtime()
            .iter()
            .copied()
            .find(|extension| extension.provider_id() == provider_id)
    }

    pub fn provider_for_request(provider_id: &str) -> Option<Provider> {
        Self::runtime_provider(provider_id).map(|extension| extension.provider())
    }

    pub fn runtime_provider_description(provider_id: &str) -> Option<&'static str> {
        Self::runtime_provider(provider_id).map(|extension| extension.provider_description())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshSummary {
    pub model_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshResult {
    Refreshed {
        provider_id: &'static str,
        provider_name: &'static str,
        model_count: usize,
    },
    Skipped {
        provider_id: &'static str,
        provider_name: &'static str,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderExtensionError {
    pub provider_id: &'static str,
    pub provider_name: &'static str,
    pub error: String,
}

#[derive(Clone)]
pub struct RuntimeDialogModelsResult {
    pub models: Vec<crate::model::types::Model>,
    pub errors: Vec<ProviderExtensionError>,
}

struct CatalogJsonExtension;

impl ProviderCatalogExtension for CatalogJsonExtension {
    fn provider_id(&self) -> &'static str {
        "catalog-json"
    }

    fn provider_name(&self) -> &'static str {
        "Catalog JSON"
    }
}

impl PersistentProviderCatalogExtension for CatalogJsonExtension {
    fn augment<'a>(
        &'a self,
        providers: &'a mut HashMap<String, Provider>,
        _cached: Option<&'a HashMap<String, Provider>>,
        _client: &'a Client,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { merge_catalog_extensions(providers) })
    }
}

fn merge_catalog_extensions(providers: &mut HashMap<String, Provider>) -> bool {
    let serde_json::Value::Object(extensions) = parse_catalog_extensions() else {
        return false;
    };
    merge_catalog(providers, &extensions)
}

/// Parse the embedded `catalog_extensions.jsonc`.
///
/// JSONC (comments and trailing commas) is accepted via `json5`, so the file
/// can document why each override exists next to the data it patches.
fn parse_catalog_extensions() -> serde_json::Value {
    json5::from_str(CATALOG_EXTENSIONS_JSON).unwrap_or_else(|err| {
        crate::emit_log!("Failed to parse model catalog extensions: {}", err);
        serde_json::Value::Null
    })
}

/// Merge the catalog extensions onto the models.dev catalog.
///
/// Extensions can both add models that models.dev does not know and correct
/// models.dev entries that are wrong or stale:
///   - a new model id is inserted as-is
///   - an existing model id is deep-merged, so the fields specified in the
///     extension win and everything else keeps its models.dev value
///
/// Returns `true` if any provider/model changed, so callers only rewrite the
/// cache when the merge actually mutated something.
fn merge_catalog(
    providers: &mut HashMap<String, Provider>,
    extensions: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let mut changed = false;

    for (provider_id, extension_provider) in extensions {
        let Some(extension_models) = extension_provider
            .get("models")
            .and_then(|models| models.as_object())
        else {
            continue;
        };
        let Some(provider) = providers.get_mut(provider_id) else {
            continue;
        };

        for (model_id, extension_model) in extension_models {
            let Some(existing) = provider.models.get_mut(model_id) else {
                // Brand-new model: insert the extension spec as-is.
                let Ok(model) = serde_json::from_value::<Model>(extension_model.clone()) else {
                    crate::emit_log!(
                        "Failed to deserialize catalog extension model {provider_id}/{model_id}"
                    );
                    continue;
                };
                provider.models.insert(model_id.clone(), model);
                changed = true;
                continue;
            };

            // Override: deep-merge the extension onto the models.dev entry so
            // specified fields win and unspecified fields are preserved.
            let Ok(existing_value) = serde_json::to_value(&*existing) else {
                continue;
            };
            let merged = deep_merge(existing_value.clone(), extension_model.clone());
            if merged == existing_value {
                continue;
            }
            let Ok(model) = serde_json::from_value::<Model>(merged) else {
                crate::emit_log!(
                    "Failed to deserialize merged catalog extension model {provider_id}/{model_id}"
                );
                continue;
            };
            *existing = model;
            changed = true;
        }
    }

    changed
}

/// Recursively merge `overlay` onto `base`: objects merge key-by-key, every
/// other value (scalars, arrays, null) is replaced by the overlay.
fn deep_merge(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    match (base, overlay) {
        (serde_json::Value::Object(mut base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                let merged = match base_map.get(&key) {
                    Some(base_value) => deep_merge(base_value.clone(), overlay_value),
                    None => overlay_value,
                };
                base_map.insert(key, merged);
            }
            serde_json::Value::Object(base_map)
        }
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_expected_extensions() {
        assert!(ModelExtensions::persistent()
            .iter()
            .any(|extension| extension.provider_id() == commandcode::PROVIDER_ID));
        assert!(ModelExtensions::persistent()
            .iter()
            .any(|extension| extension.provider_id() == "catalog-json"));
        assert!(ModelExtensions::runtime()
            .iter()
            .any(|extension| extension.provider_id() == ollama::PROVIDER_ID));
    }

    #[test]
    fn catalog_extensions_parse() {
        let catalog = parse_catalog_extensions();
        let xai = catalog.get("xai").expect("xai catalog extension provider");

        assert!(xai
            .get("models")
            .and_then(|models| models.get("grok-composer-2.5-fast"))
            .is_some());
    }

    #[test]
    fn catalog_extensions_parse_jsonc() {
        // The file is JSONC: comments and trailing commas must not break it.
        let catalog = parse_catalog_extensions();

        // Comments next to the crof overrides are part of the file, so every
        // override entry must parse.
        let crof = catalog.get("crof").expect("crof catalog extension");
        let models = crof.get("models").expect("crof models");
        assert!(models.get("deepseek-v4-flash-0731").is_some());
        assert!(models.get("deepseek-v4-flash").is_some());
        assert!(models.get("greg-1-mini").is_some());
        assert!(models.get("kimi-k2.5-lightning").is_some());
    }

    #[test]
    fn catalog_extensions_add_xai_composer_to_existing_provider() {
        let mut providers = HashMap::new();
        providers.insert(
            "xai".to_string(),
            Provider {
                id: "xai".to_string(),
                name: "xAI".to_string(),
                api: String::new(),
                doc: String::new(),
                env: vec!["XAI_API_KEY".to_string()],
                npm: "@ai-sdk/xai".to_string(),
                models: HashMap::new(),
            },
        );

        assert!(merge_catalog_extensions(&mut providers));
        assert!(!merge_catalog_extensions(&mut providers));

        let model = providers
            .get("xai")
            .and_then(|provider| provider.models.get("grok-composer-2.5-fast"))
            .expect("composer model");
        assert_eq!(model.id, "grok-composer-2.5-fast");
        assert_eq!(model.name, "Composer 2.5");
        assert_eq!(model.family, "grok-build");
        assert!(!model.attachment);
        assert!(model.tool_call);
        assert!(model.structured_output);
        assert!(!model.reasoning);
        assert_eq!(
            model
                .modalities
                .as_ref()
                .map(|modalities| modalities.input.as_slice()),
            Some(["text".to_string(), "pdf".to_string()].as_slice())
        );
        assert_eq!(
            model.limit.as_ref().map(|limit| limit.context),
            Some(256_000)
        );
    }

    #[test]
    fn catalog_extensions_do_not_create_provider() {
        let mut providers = HashMap::new();

        assert!(!merge_catalog_extensions(&mut providers));
        assert!(!providers.contains_key("xai"));
    }

    #[test]
    fn catalog_extensions_override_existing_model() {
        // models.dev reports greg-1-mini as text-only, but crof.ai/pricing
        // lists it in its `visionModels` array. The extension must flip
        // attachment on while preserving the rest of the models.dev entry.
        let mut providers = HashMap::new();
        providers.insert(
            "crof".to_string(),
            Provider {
                id: "crof".to_string(),
                name: "Crof".to_string(),
                api: String::new(),
                doc: String::new(),
                env: vec!["CROF_API_KEY".to_string()],
                npm: String::new(),
                models: HashMap::from([(
                    "greg-1-mini".to_string(),
                    Model {
                        id: "greg-1-mini".to_string(),
                        name: "Greg 1 Mini".to_string(),
                        family: "greg".to_string(),
                        attachment: false,
                        reasoning: false,
                        reasoning_options: Vec::new(),
                        tool_call: true,
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
                        limit: Some(crate::model::discovery::Limit {
                            context: 229_376,
                            output: 229_376,
                        }),
                        provider: None,
                    },
                )]),
            },
        );

        assert!(merge_catalog_extensions(&mut providers));
        // Idempotent: a second pass must not rewrite the cache.
        assert!(!merge_catalog_extensions(&mut providers));

        let model = &providers["crof"].models["greg-1-mini"];
        assert!(model.attachment);
        assert!(model
            .modalities
            .as_ref()
            .is_some_and(|modalities| modalities.input.iter().any(|input| input == "image")));
        // Fields the extension did not mention keep their models.dev values.
        assert_eq!(
            model.limit.as_ref().map(|limit| limit.context),
            Some(229_376)
        );
        assert_eq!(model.name, "Greg 1 Mini");
        assert!(model.tool_call);
    }

    #[test]
    fn catalog_extensions_add_max_effort_to_crof_deepseek_flash() {
        let mut providers = HashMap::from([(
            "crof".to_string(),
            Provider {
                id: "crof".to_string(),
                name: "Crof".to_string(),
                api: String::new(),
                doc: String::new(),
                env: vec!["CROF_API_KEY".to_string()],
                npm: String::new(),
                models: HashMap::from([(
                    "deepseek-v4-flash-0731".to_string(),
                    Model {
                        id: "deepseek-v4-flash-0731".to_string(),
                        name: "DeepSeek V4 Flash 0731".to_string(),
                        family: "deepseek".to_string(),
                        attachment: false,
                        reasoning: true,
                        reasoning_options: vec![crate::model::reasoning::ReasoningOption {
                            kind: "effort".to_string(),
                            values: vec!["none".to_string(), "low".to_string()],
                        }],
                        tool_call: true,
                        structured_output: false,
                        temperature: false,
                        knowledge: String::new(),
                        release_date: String::new(),
                        last_updated: String::new(),
                        status: None,
                        modalities: None,
                        open_weights: true,
                        cost: None,
                        limit: None,
                        provider: None,
                    },
                )]),
            },
        )]);

        assert!(merge_catalog_extensions(&mut providers));

        let efforts = providers["crof"].models["deepseek-v4-flash-0731"]
            .reasoning_efforts()
            .expect("reasoning efforts");
        assert!(efforts.contains(&crate::model::reasoning::ReasoningEffort::Max));
    }

    #[test]
    fn runtime_provider_lookup_is_registry_based() {
        let provider = ModelExtensions::runtime_provider(ollama::PROVIDER_ID)
            .expect("ollama runtime provider");

        assert_eq!(provider.provider_name(), ollama::PROVIDER_NAME);
        assert_eq!(provider.provider_description(), "Local Ollama CLI");
        assert!(ModelExtensions::provider_for_request(ollama::PROVIDER_ID).is_some());
    }

    #[test]
    fn opencode_zero_cost_models_are_available_without_connection() {
        let free_model = crate::model::types::Model {
            id: "big-pickle".to_string(),
            name: "Big Pickle".to_string(),
            family: String::new(),
            provider_id: "opencode".to_string(),
            provider_name: "OpenCode Zen".to_string(),
            attachment: false,
            structured_output: false,
            free: true,
            local: false,
            reasoning_options: Vec::new(),
        };
        let paid_model = crate::model::types::Model {
            id: "gpt-5.3-codex".to_string(),
            name: "GPT-5.3 Codex".to_string(),
            family: String::new(),
            provider_id: "opencode".to_string(),
            provider_name: "OpenCode Zen".to_string(),
            attachment: false,
            structured_output: false,
            free: false,
            local: false,
            reasoning_options: Vec::new(),
        };

        assert!(ModelExtensions::is_available_without_connection(
            &free_model
        ));
        assert!(!ModelExtensions::is_available_without_connection(
            &paid_model
        ));
    }
}
