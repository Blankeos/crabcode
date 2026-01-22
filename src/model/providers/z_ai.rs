use anyhow::Result;
use async_trait::async_trait;
use futures::stream;

use super::common::{Provider, ProviderStream};
use crate::model::types::ModelConfig;

pub struct Zai;

impl Zai {
    pub fn new() -> Self {
        Self
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

    async fn stream(&self, _prompt: &str, _config: &ModelConfig) -> Result<ProviderStream> {
        Ok(Box::pin(stream::empty()))
    }

    fn supports_model(&self, model_id: &str) -> bool {
        model_id == "coding-plan"
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

    #[tokio::test]
    async fn test_zai_stream() {
        let provider = Zai::new();
        let config = ModelConfig::new("z-ai".to_string(), "coding-plan".to_string());
        let result = provider.stream("test prompt", &config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_zai_supports_model() {
        let provider = Zai::new();
        assert!(provider.supports_model("coding-plan"));
        assert!(!provider.supports_model("other-model"));
    }
}
