use anyhow::Result;
use async_trait::async_trait;
use futures::stream;

use super::common::{Provider, ProviderStream};
use crate::model::types::ModelConfig;

pub struct NanoGpt;

impl NanoGpt {
    pub fn new() -> Self {
        Self
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

    async fn stream(&self, _prompt: &str, _config: &ModelConfig) -> Result<ProviderStream> {
        Ok(Box::pin(stream::empty()))
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

    #[tokio::test]
    async fn test_nano_gpt_stream() {
        let provider = NanoGpt::new();
        let config = ModelConfig::new("nano-gpt".to_string(), "gpt-4-mini".to_string());
        let result = provider.stream("test prompt", &config).await;
        assert!(result.is_ok());
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
