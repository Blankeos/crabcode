use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{ensure_cache_dir, get_cache_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub pricing: Option<Pricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCache {
    pub providers: Vec<Provider>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedProvider {
    pub id: String,
    pub name: String,
    pub models: Vec<ModelInfo>,
    pub auth_type: String,
}

pub struct ProviderDAO {
    cache_path: PathBuf,
}

impl ProviderDAO {
    pub fn new() -> Result<Self> {
        let cache_dir = get_cache_dir();
        ensure_cache_dir()?;
        Ok(Self {
            cache_path: cache_dir.join("providers.json"),
        })
    }

    pub fn load(&self) -> Result<Option<ProviderCache>> {
        if !self.cache_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&self.cache_path)?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    pub fn save(&self, cache: &ProviderCache) -> Result<()> {
        let content = serde_json::to_string_pretty(cache)?;
        std::fs::write(&self.cache_path, content)?;
        Ok(())
    }

    pub fn update(&self, providers: Vec<Provider>) -> Result<()> {
        let cache = ProviderCache {
            providers,
            updated_at: chrono::Utc::now().timestamp(),
        };
        self.save(&cache)
    }

    pub fn get_providers(&self) -> Result<Vec<Provider>> {
        match self.load()? {
            Some(cache) => Ok(cache.providers),
            None => Ok(vec![]),
        }
    }

    pub fn get_model(&self, model_id: &str) -> Result<Option<ModelInfo>> {
        let cache = self.load()?;
        if let Some(cache) = cache {
            for provider in &cache.providers {
                for model in &provider.models {
                    if model.id == model_id {
                        return Ok(Some(model.clone()));
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn get_configured_providers(
        &self,
        auth_dao: &super::auth::AuthDAO,
    ) -> Result<Vec<AuthenticatedProvider>> {
        let configured_auth = auth_dao.load()?;
        let cache = self.load()?;

        let mut result = vec![];

        if let Some(cache) = cache {
            for provider in &cache.providers {
                if let Some(auth_config) = configured_auth.get(&provider.id) {
                    let auth_type = match auth_config {
                        super::auth::AuthConfig::Api { .. } => "api",
                        super::auth::AuthConfig::OAuth { .. } => "oauth",
                    };

                    result.push(AuthenticatedProvider {
                        id: provider.id.clone(),
                        name: provider.name.clone(),
                        models: provider.models.clone(),
                        auth_type: auth_type.to_string(),
                    });
                }
            }
        }

        Ok(result)
    }

    pub fn display_configured_providers(&self, auth_dao: &super::auth::AuthDAO) -> Result<String> {
        let providers = self.get_configured_providers(auth_dao)?;

        if providers.is_empty() {
            return Ok(
                "No configured providers found. Add credentials using AuthDAO::set_provider()."
                    .to_string(),
            );
        }

        let mut output = String::new();
        output.push_str("Configured Providers:\n");
        output.push_str("=====================\n\n");

        for provider in &providers {
            output.push_str(&format!("📦 {} ({})\n", provider.name, provider.id));
            output.push_str(&format!("   Auth Type: {}\n", provider.auth_type));

            if !provider.models.is_empty() {
                output.push_str("   Models:\n");
                for model in &provider.models {
                    output.push_str(&format!("     • {} ({})\n", model.name, model.id));
                    if let Some(pricing) = &model.pricing {
                        output.push_str(&format!(
                            "       Pricing: ${}/1M input, ${}/1M output\n",
                            pricing.input, pricing.output
                        ));
                    }
                }
            } else {
                output.push_str("   No models available\n");
            }
            output.push('\n');
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authenticated_provider_creation() {
        let provider = AuthenticatedProvider {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            models: vec![ModelInfo {
                id: "claude-3-sonnet".to_string(),
                name: "Claude 3 Sonnet".to_string(),
                provider: "anthropic".to_string(),
                pricing: Some(Pricing {
                    input: 3.0,
                    output: 15.0,
                }),
            }],
            auth_type: "api".to_string(),
        };

        assert_eq!(provider.id, "anthropic");
        assert_eq!(provider.auth_type, "api");
        assert_eq!(provider.models.len(), 1);
    }

    #[test]
    fn test_model_info_creation() {
        let model = ModelInfo {
            id: "claude-3-opus".to_string(),
            name: "Claude 3 Opus".to_string(),
            provider: "anthropic".to_string(),
            pricing: Some(Pricing {
                input: 15.0,
                output: 75.0,
            }),
        };

        assert_eq!(model.id, "claude-3-opus");
        assert_eq!(model.provider, "anthropic");
        assert!(model.pricing.is_some());
    }

    #[test]
    fn test_pricing_creation() {
        let pricing = Pricing {
            input: 10.0,
            output: 30.0,
        };

        assert_eq!(pricing.input, 10.0);
        assert_eq!(pricing.output, 30.0);
    }
}
