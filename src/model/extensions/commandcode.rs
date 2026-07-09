use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

pub const PROVIDER_ID: &str = "commandcode";
pub const PROVIDER_NAME: &str = "Command Code";
pub const BASE_URL: &str = "https://api.commandcode.ai/provider/v1";
pub const DOC_URL: &str = "https://commandcode.ai/docs/provider-api";
pub const NPM_PACKAGE: &str = "@ai-sdk/openai-compatible";
pub const ANTHROPIC_NPM_PACKAGE: &str = "@ai-sdk/anthropic";
pub const API_KEY_ENV: &str = "CMD_API_KEY";

const DEFAULT_OUTPUT_LIMIT: u32 = 8_192;

pub static EXTENSION: Extension = Extension;

pub struct Extension;

impl crate::model::extensions::ProviderCatalogExtension for Extension {
    fn provider_id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn provider_name(&self) -> &'static str {
        PROVIDER_NAME
    }
}

impl crate::model::extensions::PersistentProviderCatalogExtension for Extension {
    fn augment<'a>(
        &'a self,
        providers: &'a mut std::collections::HashMap<String, crate::model::discovery::Provider>,
        cached: Option<&'a std::collections::HashMap<String, crate::model::discovery::Provider>>,
        client: &'a Client,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { augment_catalog(providers, cached, client).await })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandCodeModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub modalities: Option<crate::model::discovery::Modalities>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<CommandCodeModel>,
}

pub async fn fetch_provider(client: &Client) -> Result<crate::model::discovery::Provider> {
    let response = client
        .get(format!("{BASE_URL}/models"))
        .send()
        .await
        .context("Failed to fetch CommandCode models")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "CommandCode models API returned error status: {}",
            response.status()
        ));
    }

    let payload: ModelsResponse = response
        .json()
        .await
        .context("Failed to parse CommandCode models response")?;

    Ok(provider_from_models(payload.data))
}

async fn augment_catalog(
    providers: &mut std::collections::HashMap<String, crate::model::discovery::Provider>,
    cached: Option<&std::collections::HashMap<String, crate::model::discovery::Provider>>,
    client: &Client,
) -> bool {
    if providers.contains_key(PROVIDER_ID) {
        return false;
    }

    if cfg!(test) || std::env::var("CRABCODE_TEST_MODE").is_ok() {
        return false;
    }

    match fetch_provider(client).await {
        Ok(provider) => {
            providers.insert(PROVIDER_ID.to_string(), provider);
            true
        }
        Err(err) => {
            if let Some(provider) = cached.and_then(|cached| cached.get(PROVIDER_ID).cloned()) {
                providers.insert(PROVIDER_ID.to_string(), provider);
                return true;
            }
            crate::emit_log!("Skipped CommandCode model discovery: {}", err);
            false
        }
    }
}

pub fn provider_from_models(models: Vec<CommandCodeModel>) -> crate::model::discovery::Provider {
    crate::model::discovery::Provider {
        id: PROVIDER_ID.to_string(),
        name: PROVIDER_NAME.to_string(),
        api: BASE_URL.to_string(),
        doc: DOC_URL.to_string(),
        env: vec![API_KEY_ENV.to_string()],
        npm: NPM_PACKAGE.to_string(),
        models: models
            .into_iter()
            .filter(|model| !model.id.trim().is_empty())
            .map(|model| {
                let id = model.id.trim().to_string();
                let discovery_model = discovery_model(
                    &id,
                    model.name,
                    model.context_length,
                    model.capabilities,
                    model.modalities,
                );
                (id, discovery_model)
            })
            .collect(),
    }
}

fn discovery_model(
    id: &str,
    name: String,
    context_length: Option<u32>,
    capabilities: Vec<String>,
    modalities: Option<crate::model::discovery::Modalities>,
) -> crate::model::discovery::Model {
    let family = model_family(id);
    let is_anthropic = is_anthropic_model(id);
    let reasoning = supports_reasoning_effort(id, &name);
    let supports_image_input = supports_image_input(id, &capabilities, modalities.as_ref());

    crate::model::discovery::Model {
        id: id.to_string(),
        name: if name.trim().is_empty() {
            id.to_string()
        } else {
            name
        },
        family,
        attachment: supports_image_input,
        reasoning,
        reasoning_options: Vec::new(),
        tool_call: true,
        structured_output: false,
        temperature: true,
        knowledge: String::new(),
        release_date: String::new(),
        last_updated: String::new(),
        status: None,
        modalities: Some(crate::model::discovery::Modalities {
            input: if supports_image_input {
                vec!["text".to_string(), "image".to_string()]
            } else {
                vec!["text".to_string()]
            },
            output: vec!["text".to_string()],
        }),
        open_weights: false,
        cost: None,
        limit: context_length.map(|context| crate::model::discovery::Limit {
            context,
            output: DEFAULT_OUTPUT_LIMIT,
        }),
        provider: if is_anthropic {
            Some(crate::model::discovery::ModelProvider {
                npm: Some(ANTHROPIC_NPM_PACKAGE.to_string()),
                api: Some(BASE_URL.to_string()),
            })
        } else {
            None
        },
    }
}

fn supports_image_input(
    model_id: &str,
    capabilities: &[String],
    modalities: Option<&crate::model::discovery::Modalities>,
) -> bool {
    if modalities.is_some_and(|modalities| modalities.input.iter().any(|input| input == "image")) {
        return true;
    }

    if capabilities
        .iter()
        .any(|capability| matches!(capability.to_ascii_lowercase().as_str(), "image" | "vision"))
    {
        return true;
    }

    known_vision_model(model_id)
}

fn known_vision_model(model_id: &str) -> bool {
    let normalized = model_id.trim().to_ascii_lowercase();

    // This is hand-maintained based on https://commandcode.ai/models
    [
        "moonshotai/kimi-k2.7-code",
        "moonshotai/kimi-k2.7-code-highspeed",
        "moonshotai/kimi-k2.6",
        "moonshotai/kimi-k2.5",
        "minimaxai/minimax-m3",
        "xiaomi/mimo-v2.5",
        "qwen/qwen3.6-plus",
        "qwen/qwen3.7-plus",
        "stepfun/step-3.7-flash",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-fable-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.3-codex",
        "gpt-5.4-mini",
        "google/gemini-3.5-flash",
        "google/gemini-3.1-flash-lite",
        "sakana/fugu-ultra",
    ]
    .contains(&normalized.as_str())
}

fn model_family(model_id: &str) -> String {
    let normalized = model_id.trim();
    let family_source = normalized
        .split_once('/')
        .map(|(_, model)| model)
        .unwrap_or(normalized);

    family_source
        .split(['-', '.', '_'])
        .next()
        .filter(|family| !family.trim().is_empty())
        .unwrap_or(family_source)
        .to_ascii_lowercase()
}

fn is_anthropic_model(model_id: &str) -> bool {
    model_id.to_ascii_lowercase().contains("claude")
}

fn supports_reasoning_effort(model_id: &str, model_name: &str) -> bool {
    let haystack = format!(
        "{} {}",
        model_id.to_ascii_lowercase(),
        model_name.to_ascii_lowercase()
    );

    [
        "deepseek-v4-pro",
        "deepseek v4 pro",
        "deepseek-v4-flash",
        "deepseek v4 flash",
        "claude-sonnet-4-6",
        "claude-sonnet-4.6",
        "claude sonnet 4.6",
        "claude-fable-5",
        "claude fable 5",
        "claude-opus-4-6",
        "claude-opus-4.6",
        "claude opus 4.6",
        "claude-opus-4-7",
        "claude-opus-4.7",
        "claude opus 4.7",
        "gpt-5.5",
        "gpt 5.5",
        "gpt-5.4",
        "gpt 5.4",
        "gpt-5.3-codex",
        "gpt 5.3 codex",
        "gemini-3.5-flash",
        "gemini 3.5 flash",
        "gemini-3.1-flash-lite",
        "gemini 3.1 flash lite",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commandcode_model(id: &str, name: &str) -> CommandCodeModel {
        CommandCodeModel {
            id: id.to_string(),
            name: name.to_string(),
            context_length: None,
            capabilities: Vec::new(),
            modalities: None,
        }
    }

    #[test]
    fn provider_from_models_maps_openai_compatible_models() {
        let provider = provider_from_models(vec![CommandCodeModel {
            context_length: Some(1_000_000),
            ..commandcode_model("deepseek/deepseek-v4-flash", "DeepSeek V4 Flash")
        }]);

        assert_eq!(provider.id, PROVIDER_ID);
        assert_eq!(provider.api, BASE_URL);
        assert_eq!(provider.npm, NPM_PACKAGE);
        let model = provider.models.get("deepseek/deepseek-v4-flash").unwrap();
        assert_eq!(model.name, "DeepSeek V4 Flash");
        assert!(model.reasoning);
        assert!(model.tool_call);
        assert!(!model.attachment);
        assert_eq!(
            model.limit.as_ref().map(|limit| limit.context),
            Some(1_000_000)
        );
        assert!(model.provider.is_none());
    }

    #[test]
    fn provider_from_models_routes_claude_to_anthropic_transport() {
        let provider = provider_from_models(vec![CommandCodeModel {
            context_length: Some(1_000_000),
            ..commandcode_model("claude-sonnet-4-6", "Claude Sonnet 4.6")
        }]);

        let model = provider.models.get("claude-sonnet-4-6").unwrap();
        let route = model.provider.as_ref().expect("anthropic route");
        assert_eq!(route.npm.as_deref(), Some(ANTHROPIC_NPM_PACKAGE));
        assert_eq!(route.api.as_deref(), Some(BASE_URL));
        assert!(model.reasoning);
        assert!(model.attachment);
    }

    #[test]
    fn provider_from_models_keeps_unknown_reasoning_conservative() {
        let provider = provider_from_models(vec![commandcode_model("openai/gpt-4o", "GPT-4o")]);

        let model = provider.models.get("openai/gpt-4o").unwrap();
        assert!(!model.reasoning);
        assert!(!model.attachment);
    }

    #[test]
    fn provider_from_models_marks_glm_5_2_as_text_only() {
        let provider = provider_from_models(vec![commandcode_model("zai-org/GLM-5.2", "GLM-5.2")]);

        let model = provider.models.get("zai-org/GLM-5.2").unwrap();
        assert!(!model.attachment);
        assert_eq!(model.modalities.as_ref().unwrap().input, vec!["text"]);
    }

    #[test]
    fn provider_from_models_uses_explicit_future_image_capabilities() {
        let provider = provider_from_models(vec![CommandCodeModel {
            capabilities: vec!["vision".to_string()],
            ..commandcode_model("future/vision-model", "Vision Model")
        }]);

        let model = provider.models.get("future/vision-model").unwrap();
        assert!(model.attachment);
        assert_eq!(
            model.modalities.as_ref().unwrap().input,
            vec!["text", "image"]
        );
    }

    #[test]
    fn provider_from_models_drops_empty_model_ids() {
        let provider = provider_from_models(vec![commandcode_model(" ", "Empty")]);

        assert!(provider.models.is_empty());
    }
}
