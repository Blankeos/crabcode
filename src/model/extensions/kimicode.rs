use crate::model::discovery::{Modalities, Model, ModelProvider, Provider};
use reqwest::Client;
use std::collections::HashMap;

/// "Kimi For Coding" is a separate Moonshot product that is **not** part of the
/// general Moonshot (api.moonshot.ai) API. It exposes the Anthropic Messages
/// protocol at `https://api.kimi.com/coding/v1` and is authenticated with a
/// distinct `sk-kimi-...` key. The `moonshotai` provider therefore cannot reach
/// it (it hits api.moonshot.ai with OpenAI-compatible chat/completions), which
/// is why a bare key produced a 401.
///
/// This extension registers `kimi-for-coding` as a first-class provider that
/// uses the Anthropic transport, so the user only needs to drop a key into
/// auth.json (mirroring how OpenCode wires it up):
///
/// ```json
/// "kimi-for-coding": { "type": "api", "key": "sk-kimi-..." }
/// ```
pub const PROVIDER_ID: &str = "kimi-for-coding";
pub const PROVIDER_NAME: &str = "Kimi For Coding";
pub const BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub const DOC_URL: &str = "https://www.kimi.com/code/docs/en/kimi-code/models";
pub const NPM_PACKAGE: &str = "@ai-sdk/anthropic";
pub const API_KEY_ENV: &str = "KIMI_API_KEY";

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
        providers: &'a mut HashMap<String, Provider>,
        _cached: Option<&'a HashMap<String, Provider>>,
        _client: &'a Client,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            if providers.contains_key(PROVIDER_ID) {
                return false;
            }
            providers.insert(PROVIDER_ID.to_string(), static_provider());
            true
        })
    }
}

fn anthropic_override() -> ModelProvider {
    ModelProvider {
        npm: Some(NPM_PACKAGE.to_string()),
        api: Some(BASE_URL.to_string()),
    }
}

fn static_provider() -> Provider {
    let mut models = HashMap::new();

    models.insert(
        "kimi-for-coding".to_string(),
        Model {
            id: "kimi-for-coding".to_string(),
            name: "Kimi K2.7 Code".to_string(),
            family: "kimi".to_string(),
            attachment: true,
            reasoning: false,
            reasoning_options: Vec::new(),
            tool_call: true,
            structured_output: false,
            temperature: true,
            knowledge: String::new(),
            release_date: String::new(),
            last_updated: String::new(),
            status: Some("stable".to_string()),
            modalities: Some(Modalities {
                input: vec!["text".to_string(), "image".to_string(), "video".to_string()],
                output: vec!["text".to_string()],
            }),
            open_weights: false,
            cost: None,
            limit: Some(crate::model::discovery::Limit {
                context: 256_000,
                output: 16_384,
            }),
            provider: Some(anthropic_override()),
        },
    );

    models.insert(
        "kimi-for-coding-highspeed".to_string(),
        Model {
            id: "kimi-for-coding-highspeed".to_string(),
            name: "Kimi For Coding HighSpeed".to_string(),
            family: "kimi".to_string(),
            attachment: true,
            reasoning: false,
            reasoning_options: Vec::new(),
            tool_call: true,
            structured_output: false,
            temperature: true,
            knowledge: String::new(),
            release_date: String::new(),
            last_updated: String::new(),
            status: Some("stable".to_string()),
            modalities: Some(Modalities {
                input: vec!["text".to_string(), "image".to_string(), "video".to_string()],
                output: vec!["text".to_string()],
            }),
            open_weights: false,
            cost: None,
            limit: Some(crate::model::discovery::Limit {
                context: 256_000,
                output: 16_384,
            }),
            provider: Some(anthropic_override()),
        },
    );

    models.insert(
        "k3".to_string(),
        Model {
            id: "k3".to_string(),
            name: "Kimi K3".to_string(),
            family: "kimi".to_string(),
            attachment: true,
            reasoning: true,
            reasoning_options: vec![crate::model::reasoning::ReasoningOption {
                kind: "effort".to_string(),
                values: vec!["low".to_string(), "high".to_string(), "max".to_string()],
            }],
            tool_call: true,
            structured_output: false,
            temperature: true,
            knowledge: String::new(),
            release_date: String::new(),
            last_updated: String::new(),
            status: Some("stable".to_string()),
            modalities: Some(Modalities {
                input: vec!["text".to_string(), "image".to_string(), "video".to_string()],
                output: vec!["text".to_string()],
            }),
            open_weights: false,
            cost: None,
            limit: Some(crate::model::discovery::Limit {
                context: 1_000_000,
                output: 16_384,
            }),
            provider: Some(anthropic_override()),
        },
    );

    models.insert(
        "k3-256k".to_string(),
        Model {
            id: "k3-256k".to_string(),
            name: "Kimi K3 256K".to_string(),
            family: "kimi".to_string(),
            attachment: true,
            reasoning: true,
            reasoning_options: vec![crate::model::reasoning::ReasoningOption {
                kind: "effort".to_string(),
                values: vec!["low".to_string(), "high".to_string(), "max".to_string()],
            }],
            tool_call: true,
            structured_output: false,
            temperature: true,
            knowledge: String::new(),
            release_date: String::new(),
            last_updated: String::new(),
            status: Some("stable".to_string()),
            modalities: Some(Modalities {
                input: vec!["text".to_string(), "image".to_string()],
                output: vec!["text".to_string()],
            }),
            open_weights: false,
            cost: None,
            limit: Some(crate::model::discovery::Limit {
                context: 256_000,
                output: 16_384,
            }),
            provider: Some(anthropic_override()),
        },
    );

    Provider {
        id: PROVIDER_ID.to_string(),
        name: PROVIDER_NAME.to_string(),
        api: BASE_URL.to_string(),
        doc: DOC_URL.to_string(),
        env: vec![API_KEY_ENV.to_string()],
        npm: NPM_PACKAGE.to_string(),
        header: vec![("User-Agent".to_string(), "KimiCLI/1.5".to_string())],
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_uses_anthropic_transport() {
        let provider = static_provider();
        assert_eq!(provider.id, PROVIDER_ID);
        assert_eq!(provider.npm, "@ai-sdk/anthropic");
        assert_eq!(provider.api, "https://api.kimi.com/coding/v1");
    }

    #[test]
    fn all_models_route_through_anthropic() {
        let provider = static_provider();
        for model in provider.models.values() {
            let route = model.provider.as_ref().expect("model provider override");
            assert_eq!(route.npm.as_deref(), Some("@ai-sdk/anthropic"));
            assert_eq!(route.api.as_deref(), Some("https://api.kimi.com/coding/v1"));
        }
    }
}
