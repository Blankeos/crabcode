use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub family: String,
    pub provider_id: String,
    pub provider_name: String,
    /// Mirrors models.dev `attachment`.
    pub attachment: bool,
    /// Mirrors models.dev `structured_output`.
    pub structured_output: bool,
    /// Crabcode-local availability flag for models usable without provider auth.
    pub free: bool,
    /// Crabcode-local availability flag for runtime/local providers, e.g. Ollama.
    pub local: bool,
    /// Mirrors models.dev `reasoning_options`.
    pub reasoning_options: Vec<crate::model::reasoning::ReasoningOption>,
}

impl Model {
    pub fn dialog_description(&self) -> String {
        self.provider_name.clone()
    }

    pub fn display_tags(&self) -> Vec<&'static str> {
        let mut tags = Vec::new();
        if self.attachment {
            tags.push("attachment");
        }
        if self.structured_output {
            tags.push("structured_output");
        }
        if !self.reasoning_options.is_empty() {
            tags.push("reasoning_options");
        }
        if self.free {
            tags.push("free");
        }
        if self.local {
            tags.push("local");
        }
        tags
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider_id: String,
    pub model_id: String,
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl ModelConfig {
    pub fn new(provider_id: String, model_id: String) -> Self {
        Self {
            provider_id,
            model_id,
            api_key: None,
            temperature: None,
            max_tokens: None,
        }
    }

    pub fn with_api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_new() {
        let config = ModelConfig::new("nano-gpt".to_string(), "gpt-4-mini".to_string());
        assert_eq!(config.provider_id, "nano-gpt");
        assert_eq!(config.model_id, "gpt-4-mini");
        assert!(config.api_key.is_none());
        assert!(config.temperature.is_none());
        assert!(config.max_tokens.is_none());
    }

    #[test]
    fn test_model_config_builder() {
        let config = ModelConfig::new("nano-gpt".to_string(), "gpt-4-mini".to_string())
            .with_api_key("test-key".to_string())
            .with_temperature(0.7)
            .with_max_tokens(4096);

        assert_eq!(config.provider_id, "nano-gpt");
        assert_eq!(config.model_id, "gpt-4-mini");
        assert_eq!(config.api_key, Some("test-key".to_string()));
        assert_eq!(config.temperature, Some(0.7));
        assert_eq!(config.max_tokens, Some(4096));
    }

    #[test]
    fn test_model_config_serialization() {
        let config = ModelConfig::new("z-ai".to_string(), "coding-plan".to_string())
            .with_api_key("secret-key".to_string())
            .with_temperature(0.5)
            .with_max_tokens(2048);

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: ModelConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.provider_id, config.provider_id);
        assert_eq!(deserialized.model_id, config.model_id);
        assert_eq!(deserialized.api_key, config.api_key);
        assert_eq!(deserialized.temperature, config.temperature);
        assert_eq!(deserialized.max_tokens, config.max_tokens);
    }

    #[test]
    fn model_dialog_description_omits_metadata_tags() {
        let model = Model {
            id: "gpt-5".to_string(),
            name: "GPT-5".to_string(),
            family: "gpt".to_string(),
            provider_id: "openai".to_string(),
            provider_name: "OpenAI".to_string(),
            attachment: true,
            structured_output: false,
            free: false,
            local: false,
            reasoning_options: vec![crate::model::reasoning::ReasoningOption {
                kind: "effort".to_string(),
                values: vec!["low".to_string()],
            }],
        };

        let description = model.dialog_description();

        assert_eq!(description, "OpenAI");
        assert!(!description.contains('|'));
        assert!(!description.contains("attachment"));
        assert!(!description.contains("reasoning"));
    }
}
