use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

pub const PROVIDER_ID: &str = "commandcode";
pub const PROVIDER_NAME: &str = "Command Code";
pub const BASE_URL: &str = "https://api.commandcode.ai/provider/v1";
pub const DOC_URL: &str = "https://commandcode.ai/docs/provider-api";
pub const NPM_PACKAGE: &str = "@ai-sdk/openai-compatible";
pub const ANTHROPIC_NPM_PACKAGE: &str = "@ai-sdk/anthropic";
pub const API_KEY_ENV: &str = "CMD_API_KEY";

const DEFAULT_OUTPUT_LIMIT: u32 = 8_192;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CommandCodeModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context_length: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<CommandCodeModel>,
}

pub fn is_commandcode_provider(provider_id: &str) -> bool {
    provider_id == PROVIDER_ID
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

pub fn inject_provider(
    providers: &mut HashMap<String, crate::model::discovery::Provider>,
    provider: crate::model::discovery::Provider,
) {
    providers.insert(PROVIDER_ID.to_string(), provider);
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
                let model = discovery_model(&id, model.name, model.context_length);
                (id, model)
            })
            .collect(),
    }
}

fn discovery_model(
    id: &str,
    name: String,
    context_length: Option<u32>,
) -> crate::model::discovery::Model {
    let family = model_family(id);
    let is_anthropic = is_anthropic_model(id);
    let reasoning = supports_reasoning_effort(id, &name);

    crate::model::discovery::Model {
        id: id.to_string(),
        name: if name.trim().is_empty() {
            id.to_string()
        } else {
            name
        },
        family,
        attachment: true,
        reasoning,
        tool_call: true,
        structured_output: false,
        temperature: true,
        knowledge: String::new(),
        release_date: String::new(),
        last_updated: String::new(),
        modalities: Some(crate::model::discovery::Modalities {
            input: vec!["text".to_string(), "image".to_string()],
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

    #[test]
    fn provider_from_models_maps_openai_compatible_models() {
        let provider = provider_from_models(vec![CommandCodeModel {
            id: "deepseek/deepseek-v4-flash".to_string(),
            name: "DeepSeek V4 Flash".to_string(),
            context_length: Some(1_000_000),
        }]);

        assert_eq!(provider.id, PROVIDER_ID);
        assert_eq!(provider.api, BASE_URL);
        assert_eq!(provider.npm, NPM_PACKAGE);
        let model = provider.models.get("deepseek/deepseek-v4-flash").unwrap();
        assert_eq!(model.name, "DeepSeek V4 Flash");
        assert!(model.reasoning);
        assert!(model.tool_call);
        assert!(model.attachment);
        assert_eq!(
            model.limit.as_ref().map(|limit| limit.context),
            Some(1_000_000)
        );
        assert!(model.provider.is_none());
    }

    #[test]
    fn provider_from_models_routes_claude_to_anthropic_transport() {
        let provider = provider_from_models(vec![CommandCodeModel {
            id: "claude-sonnet-4-6".to_string(),
            name: "Claude Sonnet 4.6".to_string(),
            context_length: Some(1_000_000),
        }]);

        let model = provider.models.get("claude-sonnet-4-6").unwrap();
        let route = model.provider.as_ref().expect("anthropic route");
        assert_eq!(route.npm.as_deref(), Some(ANTHROPIC_NPM_PACKAGE));
        assert_eq!(route.api.as_deref(), Some(BASE_URL));
        assert!(model.reasoning);
    }

    #[test]
    fn provider_from_models_keeps_unknown_reasoning_conservative() {
        let provider = provider_from_models(vec![CommandCodeModel {
            id: "openai/gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            context_length: None,
        }]);

        let model = provider.models.get("openai/gpt-4o").unwrap();
        assert!(!model.reasoning);
    }

    #[test]
    fn provider_from_models_drops_empty_model_ids() {
        let provider = provider_from_models(vec![CommandCodeModel {
            id: " ".to_string(),
            name: "Empty".to_string(),
            context_length: None,
        }]);

        assert!(provider.models.is_empty());
    }
}
