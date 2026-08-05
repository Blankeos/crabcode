use anyhow::{Context, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const PROVIDER_ID: &str = "ollama";
pub const PROVIDER_NAME: &str = "Ollama (Local)";
pub const BASE_URL: &str = "http://localhost:11434/v1";
pub const NPM_PACKAGE: &str = "@ai-sdk/openai-compatible";

const OLLAMA_LS_TIMEOUT: Duration = Duration::from_secs(5);
const OLLAMA_ERROR_CACHE_TTL: Duration = Duration::from_secs(30);

pub static EXTENSION: Extension = Extension;

pub struct Extension;

impl crate::model::extensions::ProviderCatalogExtension for Extension {
    fn provider_id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn provider_name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn provider_description(&self) -> &'static str {
        "Local Ollama CLI"
    }
}

impl crate::model::extensions::RuntimeProviderCatalogExtension for Extension {
    fn provider(&self) -> crate::model::discovery::Provider {
        provider()
    }

    fn refresh_models<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<crate::model::extensions::RefreshSummary>> + Send + 'a>>
    {
        Box::pin(async move {
            let model_count = refresh_model_cache().await?.len();
            Ok(crate::model::extensions::RefreshSummary { model_count })
        })
    }

    fn models_from_cache(&self) -> Vec<crate::model::types::Model> {
        models_from_runtime_cache()
    }

    fn models_for_dialog_cached<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<crate::model::types::Model>>> + Send + 'a>> {
        Box::pin(async move { models_for_dialog_cached().await })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
enum ModelCacheEntry {
    Models(Vec<OllamaModel>),
    Error { message: String, cached_at: Instant },
}

static MODEL_CACHE: OnceLock<Mutex<Option<ModelCacheEntry>>> = OnceLock::new();

#[cfg(test)]
static TEST_CACHE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<ModelCacheEntry>> {
    MODEL_CACHE.get_or_init(|| Mutex::new(None))
}

pub fn provider() -> crate::model::discovery::Provider {
    crate::model::discovery::Provider {
        id: PROVIDER_ID.to_string(),
        name: PROVIDER_NAME.to_string(),
        api: BASE_URL.to_string(),
        doc: "https://ollama.com".to_string(),
        env: Vec::new(),
        npm: NPM_PACKAGE.to_string(),
        header: vec![],
        models: cached_discovery_models().unwrap_or_default(),
    }
}

pub async fn list_models_cached() -> Result<Vec<OllamaModel>> {
    if let Some(entry) = cache().lock().ok().and_then(|guard| guard.clone()) {
        match entry {
            ModelCacheEntry::Models(models) => return Ok(models),
            ModelCacheEntry::Error { message, cached_at }
                if cached_at.elapsed() < OLLAMA_ERROR_CACHE_TTL =>
            {
                return Err(anyhow::anyhow!(message));
            }
            ModelCacheEntry::Error { .. } => {}
        }
    }

    refresh_model_cache().await
}

pub async fn refresh_model_cache() -> Result<Vec<OllamaModel>> {
    match list_models_from_cli().await {
        Ok(models) => {
            if let Ok(mut guard) = cache().lock() {
                *guard = Some(ModelCacheEntry::Models(models.clone()));
            }
            Ok(models)
        }
        Err(err) => {
            if let Ok(mut guard) = cache().lock() {
                *guard = Some(ModelCacheEntry::Error {
                    message: err.to_string(),
                    cached_at: Instant::now(),
                });
            }
            Err(err)
        }
    }
}

pub fn models_from_runtime_cache() -> Vec<crate::model::types::Model> {
    cache()
        .lock()
        .ok()
        .and_then(|guard| match guard.clone() {
            Some(ModelCacheEntry::Models(models)) => Some(models),
            Some(ModelCacheEntry::Error { .. }) | None => None,
        })
        .unwrap_or_default()
        .into_iter()
        .map(model_for_dialog)
        .collect()
}

pub async fn models_for_dialog_cached() -> Result<Vec<crate::model::types::Model>> {
    Ok(list_models_cached()
        .await?
        .into_iter()
        .map(model_for_dialog)
        .collect())
}

pub async fn models_for_dialog_cached_or_empty() -> Vec<crate::model::types::Model> {
    models_for_dialog_cached().await.unwrap_or_default()
}

pub fn model_for_dialog(model: OllamaModel) -> crate::model::types::Model {
    crate::model::types::Model {
        family: model_family(&model.id),
        provider_id: PROVIDER_ID.to_string(),
        provider_name: PROVIDER_NAME.to_string(),
        attachment: false,
        structured_output: false,
        free: false,
        local: true,
        reasoning_options: Vec::new(),
        id: model.id,
        name: model.name,
    }
}

fn cached_discovery_models(
) -> Option<std::collections::HashMap<String, crate::model::discovery::Model>> {
    let models = cache().lock().ok().and_then(|guard| match guard.clone() {
        Some(ModelCacheEntry::Models(models)) => Some(models),
        Some(ModelCacheEntry::Error { .. }) | None => None,
    })?;
    Some(
        models
            .into_iter()
            .map(|model| {
                let id = model.id;
                let family = model_family(&id);
                (
                    id.clone(),
                    crate::model::discovery::Model {
                        id: id.clone(),
                        name: model.name,
                        family,
                        attachment: false,
                        reasoning: false,
                        reasoning_options: Vec::new(),
                        tool_call: true,
                        structured_output: false,
                        temperature: true,
                        knowledge: String::new(),
                        release_date: String::new(),
                        last_updated: String::new(),
                        status: None,
                        modalities: Some(crate::model::discovery::Modalities {
                            input: vec!["text".to_string()],
                            output: vec!["text".to_string()],
                        }),
                        open_weights: true,
                        cost: None,
                        limit: None,
                        provider: None,
                    },
                )
            })
            .collect(),
    )
}

async fn list_models_from_cli() -> Result<Vec<OllamaModel>> {
    let output = tokio::time::timeout(
        OLLAMA_LS_TIMEOUT,
        tokio::process::Command::new("ollama").arg("ls").output(),
    )
    .await
    .context("timed out running `ollama ls`")?
    .context("failed to run `ollama ls`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("`ollama ls` exited with status {}", output.status)
        } else {
            format!(
                "`ollama ls` exited with status {}: {}",
                output.status, stderr
            )
        };
        return Err(anyhow::anyhow!(message));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_ollama_ls_output(&stdout))
}

pub fn parse_ollama_ls_output(output: &str) -> Vec<OllamaModel> {
    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some(name) = line.split_whitespace().next() else {
            continue;
        };

        if name.eq_ignore_ascii_case("name") || !seen.insert(name.to_string()) {
            continue;
        }

        models.push(OllamaModel {
            id: name.to_string(),
            name: name.to_string(),
        });
    }

    models.sort_by(|a, b| a.name.cmp(&b.name));
    models
}

fn model_family(model_id: &str) -> String {
    model_id
        .split([':', '/'])
        .next()
        .filter(|family| !family.trim().is_empty())
        .unwrap_or(model_id)
        .to_string()
}

#[cfg(test)]
pub fn set_cached_models_for_test(models: Vec<OllamaModel>) {
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(ModelCacheEntry::Models(models));
    }
}

#[cfg(test)]
pub fn test_cache_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_CACHE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("ollama test cache lock")
}

#[cfg(test)]
pub fn clear_cache_for_test() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ollama_ls_output() {
        let output = "NAME                    ID              SIZE      MODIFIED\nllama3.2:latest         a80c4f17acd5    2.0 GB    3 weeks ago\nqwen2.5-coder:7b        2b0496514337    4.7 GB    2 days ago\n";

        let models = parse_ollama_ls_output(output);

        assert_eq!(
            models,
            vec![
                OllamaModel {
                    id: "llama3.2:latest".to_string(),
                    name: "llama3.2:latest".to_string(),
                },
                OllamaModel {
                    id: "qwen2.5-coder:7b".to_string(),
                    name: "qwen2.5-coder:7b".to_string(),
                },
            ]
        );
    }

    #[test]
    fn provider_uses_cached_models_without_running_cli() {
        let _guard = test_cache_lock();
        set_cached_models_for_test(vec![OllamaModel {
            id: "llama3.2:latest".to_string(),
            name: "llama3.2:latest".to_string(),
        }]);

        let provider = provider();

        assert_eq!(provider.id, PROVIDER_ID);
        assert_eq!(provider.name, PROVIDER_NAME);
        assert!(provider.models.contains_key("llama3.2:latest"));
        clear_cache_for_test();
    }

    #[tokio::test]
    async fn recent_cli_errors_are_cached() {
        let _guard = test_cache_lock();
        if let Ok(mut guard) = cache().lock() {
            *guard = Some(ModelCacheEntry::Error {
                message: "ollama unavailable".to_string(),
                cached_at: Instant::now(),
            });
        }

        let error = list_models_cached().await.unwrap_err();

        assert_eq!(error.to_string(), "ollama unavailable");
        clear_cache_for_test();
    }
}
