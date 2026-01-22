use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use crate::model::types::ModelConfig;
use crate::streaming::parser::StreamEvent;

pub type ProviderStream = Pin<Box<dyn Stream<Item = StreamEvent> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn provider_id(&self) -> &str;

    async fn stream(&self, prompt: &str, config: &ModelConfig) -> Result<ProviderStream>;

    fn supports_model(&self, model_id: &str) -> bool;
}

use super::nano_gpt::NanoGpt;
use super::z_ai::Zai;

pub struct ProviderFactory;

impl ProviderFactory {
    pub fn create_provider(provider_id: &str) -> Result<Box<dyn Provider>> {
        match provider_id {
            "nano-gpt" => Ok(Box::new(NanoGpt::new())),
            "z-ai" => Ok(Box::new(Zai::new())),
            _ => Err(anyhow::anyhow!(
                "Provider '{}' not implemented",
                provider_id
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        id: String,
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn provider_id(&self) -> &str {
            &self.id
        }

        async fn stream(&self, _prompt: &str, _config: &ModelConfig) -> Result<ProviderStream> {
            use futures::stream;
            Ok(Box::pin(stream::empty()))
        }

        fn supports_model(&self, _model_id: &str) -> bool {
            true
        }
    }

    #[test]
    fn test_provider_id() {
        let provider = MockProvider {
            id: "test-provider".to_string(),
        };
        assert_eq!(provider.provider_id(), "test-provider");
    }

    #[tokio::test]
    async fn test_provider_stream() {
        let provider = MockProvider {
            id: "test-provider".to_string(),
        };
        let config = ModelConfig::new("test-provider".to_string(), "test-model".to_string());
        let result = provider.stream("test prompt", &config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_provider_supports_model() {
        let provider = MockProvider {
            id: "test-provider".to_string(),
        };
        assert!(provider.supports_model("any-model"));
    }

    #[test]
    fn test_provider_factory_create_nano_gpt() {
        let provider = ProviderFactory::create_provider("nano-gpt").unwrap();
        assert_eq!(provider.provider_id(), "nano-gpt");
    }

    #[test]
    fn test_provider_factory_create_z_ai() {
        let provider = ProviderFactory::create_provider("z-ai").unwrap();
        assert_eq!(provider.provider_id(), "z-ai");
    }

    #[test]
    fn test_provider_factory_create_unknown() {
        let result = ProviderFactory::create_provider("unknown");
        assert!(result.is_err());
    }
}
