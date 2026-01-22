use anyhow::Result;
use async_trait::async_trait;

use super::common::{Provider, ProviderStream};
use crate::model::types::ModelConfig;
use crate::streaming::client::StreamClient;

const DEFAULT_API_URL: &str = "https://api.z.ai/v1/chat/completions";

pub struct Zai {
    api_url: String,
}

impl Zai {
    pub fn new() -> Self {
        Self {
            api_url: DEFAULT_API_URL.to_string(),
        }
    }

    pub fn with_api_url(mut self, api_url: String) -> Self {
        self.api_url = api_url;
        self
    }
}

impl Default for Zai {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for Zai {
    fn provider_id(&self) -> &str {
        "z-ai"
    }

    async fn stream(&self, prompt: &str, config: &ModelConfig) -> Result<ProviderStream> {
        let api_key = config.api_key.as_deref();
        let mut client = StreamClient::new();
        let stream = client
            .stream(&self.api_url, prompt, api_key, &config.model_id)
            .await?;
        Ok(Box::pin(stream))
    }

    fn supports_model(&self, model_id: &str) -> bool {
        matches!(model_id, "coding-plan" | "coding-full")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zai_provider_id() {
        let provider = Zai::new();
        assert_eq!(provider.provider_id(), "z-ai");
    }

    #[test]
    fn test_zai_default() {
        let provider = Zai::default();
        assert_eq!(provider.provider_id(), "z-ai");
    }

    #[test]
    fn test_zai_with_api_url() {
        let provider =
            Zai::new().with_api_url("https://custom.api.com/v1/chat/completions".to_string());
        assert_eq!(
            provider.api_url,
            "https://custom.api.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_zai_supports_model() {
        let provider = Zai::new();
        assert!(provider.supports_model("coding-plan"));
        assert!(provider.supports_model("coding-full"));
        assert!(!provider.supports_model("other-model"));
    }
}
