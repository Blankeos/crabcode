use anyhow::Result;
use reqwest::Client;
use std::collections::HashMap;

use crate::model::discovery::Provider;

pub mod commandcode;
pub mod ollama;

const CATALOG_EXTENSIONS_JSON: &str = include_str!("catalog_extensions.json");
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
    merge_catalog(providers, parse_catalog_extensions())
}

fn parse_catalog_extensions() -> HashMap<String, Provider> {
    serde_json::from_str(CATALOG_EXTENSIONS_JSON).unwrap_or_else(|err| {
        crate::emit_log!("Failed to parse model catalog extensions: {}", err);
        HashMap::new()
    })
}

fn merge_catalog(
    providers: &mut HashMap<String, Provider>,
    catalog_extensions: HashMap<String, Provider>,
) -> bool {
    let mut changed = false;

    for (provider_id, extension_provider) in catalog_extensions {
        let Some(provider) = providers.get_mut(&provider_id) else {
            continue;
        };

        for (model_id, model) in extension_provider.models {
            if provider.models.contains_key(&model_id) {
                continue;
            }

            provider.models.insert(model_id, model);
            changed = true;
        }
    }

    changed
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

        assert!(xai.models.contains_key("grok-composer-2.5-fast"));
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
            context_window: None,
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
            context_window: None,
        };

        assert!(ModelExtensions::is_available_without_connection(
            &free_model
        ));
        assert!(!ModelExtensions::is_available_without_connection(
            &paid_model
        ));
    }
}
