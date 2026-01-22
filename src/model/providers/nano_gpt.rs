use anyhow::Result;
use async_trait::async_trait;

use super::common::{Provider, ProviderStream};
use crate::model::types::ModelConfig;
use crate::streaming::client::StreamClient;

const DEFAULT_API_URL: &str = "https://api.nano-gpt.com/v1/chat/completions";

pub struct NanoGpt {
    api_url: String,
    client: StreamClient,
}

impl NanoGpt {
    pub fn new() -> Self {
        Self {
            api_url: DEFAULT_API_URL.to_string(),
            client: StreamClient::new(),
        }
    }

    pub fn with_api_url(mut self, api_url: String) -> Self {
        self.api_url = api_url;
        self
    }
}

impl Default for NanoGpt {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for NanoGpt {
    fn provider_id(&self) -> &str {
        "nano-gpt"
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
        matches!(model_id, "gpt-4-mini" | "gpt-4" | "gpt-3.5-turbo")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nano_gpt_provider_id() {
        let provider = NanoGpt::new();
        assert_eq!(provider.provider_id(), "nano-gpt");
    }

    #[test]
    fn test_nano_gpt_default() {
        let provider = NanoGpt::default();
        assert_eq!(provider.provider_id(), "nano-gpt");
    }

    #[test]
    fn test_nano_gpt_with_api_url() {
        let provider =
            NanoGpt::new().with_api_url("https://custom.api.com/v1/chat/completions".to_string());
        assert_eq!(
            provider.api_url,
            "https://custom.api.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_nano_gpt_supports_model() {
        let provider = NanoGpt::new();
        assert!(provider.supports_model("gpt-4-mini"));
        assert!(provider.supports_model("gpt-4"));
        assert!(provider.supports_model("gpt-3.5-turbo"));
        assert!(!provider.supports_model("other-model"));
    }
}
