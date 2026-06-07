use anyhow::{Context, Result};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const PROVIDER_ID: &str = "ollama";
pub const PROVIDER_NAME: &str = "Ollama (Local)";
pub const BASE_URL: &str = "http://localhost:11434/v1";
pub const NPM_PACKAGE: &str = "@ai-sdk/openai-compatible";

const OLLAMA_LS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModel {
    pub id: String,
    pub name: String,
}

static MODEL_CACHE: OnceLock<Mutex<Option<Vec<OllamaModel>>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<Vec<OllamaModel>>> {
    MODEL_CACHE.get_or_init(|| Mutex::new(None))
}

pub fn is_ollama_provider(provider_id: &str) -> bool {
    provider_id == PROVIDER_ID
}

pub fn provider() -> crate::model::discovery::Provider {
    crate::model::discovery::Provider {
        id: PROVIDER_ID.to_string(),
        name: PROVIDER_NAME.to_string(),
        api: BASE_URL.to_string(),
        doc: "https://ollama.com".to_string(),
        env: Vec::new(),
        npm: NPM_PACKAGE.to_string(),
        models: cached_discovery_models().unwrap_or_default(),
    }
}

pub fn inject_provider(
    providers: &mut std::collections::HashMap<String, crate::model::discovery::Provider>,
) {
    providers.insert(PROVIDER_ID.to_string(), provider());
}

pub async fn list_models_cached() -> Result<Vec<OllamaModel>> {
    if let Some(models) = cache().lock().ok().and_then(|guard| guard.clone()) {
        return Ok(models);
    }

    refresh_model_cache().await
}

pub async fn refresh_model_cache() -> Result<Vec<OllamaModel>> {
    let models = list_models_from_cli().await?;
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(models.clone());
    }
    Ok(models)
}

pub fn models_from_runtime_cache() -> Vec<crate::model::types::Model> {
    cache()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
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
        capabilities: vec!["local".to_string()],
        reasoning: false,
        id: model.id,
        name: model.name,
    }
}

fn cached_discovery_models(
) -> Option<std::collections::HashMap<String, crate::model::discovery::Model>> {
    let models = cache().lock().ok().and_then(|guard| guard.clone())?;
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
                        tool_call: true,
                        structured_output: false,
                        temperature: true,
                        knowledge: String::new(),
                        release_date: String::new(),
                        last_updated: String::new(),
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
        *guard = Some(models);
    }
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
}
