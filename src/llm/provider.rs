use crate::model::discovery::{Discovery, Provider as ModelsDevProvider};
use std::collections::HashMap;

pub struct ModelRegistry {
    discovery: Discovery,
    providers: Option<HashMap<String, ModelsDevProvider>>,
}

impl ModelRegistry {
    pub fn new(discovery: Discovery) -> Self {
        Self {
            discovery,
            providers: None,
        }
    }

    pub async fn load_providers(
        &mut self,
    ) -> Result<&HashMap<String, ModelsDevProvider>, Box<dyn std::error::Error>> {
        if self.providers.is_none() {
            let providers = self.discovery.fetch_providers().await?;
            self.providers = Some(providers);
        }
        Ok(self.providers.as_ref().unwrap())
    }

    pub fn get_provider(
        &mut self,
        provider_id: &str,
    ) -> Result<ProviderConfig, Box<dyn std::error::Error>> {
        if let Some(ref providers) = self.providers {
            if let Some(provider) = providers.get(provider_id) {
                return Ok(ProviderConfig {
                    id: provider.id.clone(),
                    name: provider.name.clone(),
                    base_url: provider.api.clone(),
                });
            }

            for (prov_id, provider) in providers.iter() {
                if provider.models.contains_key(provider_id) {
                    return Ok(ProviderConfig {
                        id: prov_id.clone(),
                        name: provider.name.clone(),
                        base_url: provider.api.clone(),
                    });
                }
            }
        }

        Err(Box::<dyn std::error::Error>::from(anyhow::anyhow!(
            "Provider not found: {}",
            provider_id
        )))
    }

    pub fn get_model(
        &mut self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<&crate::model::discovery::Model, Box<dyn std::error::Error>> {
        let providers = self.providers.as_ref().ok_or_else(|| {
            Box::<dyn std::error::Error>::from(anyhow::anyhow!("Providers not loaded"))
        })?;

        let provider = providers.get(provider_id).ok_or_else(|| {
            Box::<dyn std::error::Error>::from(anyhow::anyhow!(
                "Provider not found: {}",
                provider_id
            ))
        })?;

        provider.models.get(model_id).ok_or_else(|| {
            Box::<dyn std::error::Error>::from(anyhow::anyhow!(
                "Model not found: {}/{}",
                provider_id,
                model_id
            ))
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
}
