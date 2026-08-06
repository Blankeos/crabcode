use crate::config::configuration::LoadedConfig;
use crate::model::discovery::{is_model_selectable, merge_dialog_models, Discovery};
use crate::model::extensions::ModelExtensions;
use crate::model::types::Model;
use crate::persistence::AuthDAO;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;

pub async fn selectable_models(
    config: &LoadedConfig,
    provider_filter: Option<&str>,
) -> Result<Vec<Model>> {
    let connected_providers = AuthDAO::new()
        .context("failed to initialize auth storage")?
        .load()
        .context("failed to load providers")?;
    let connected_provider_ids = connected_providers.keys().cloned().collect::<HashSet<_>>();
    let discovery = Discovery::new_with_custom(Some(config.merged_config.custom_providers.clone()))
        .context("failed to initialize model discovery")?;
    let configured_provider_ids = discovery.custom_provider_ids();

    let filter_matches_runtime = provider_filter.is_some_and(|filter| {
        ModelExtensions::runtime()
            .iter()
            .any(|integration| integration.provider_id() == filter)
    });
    let has_runtime = ModelExtensions::runtime()
        .iter()
        .any(|integration| connected_providers.contains_key(integration.provider_id()))
        || (connected_providers.is_empty() && provider_filter.is_none())
        || filter_matches_runtime;
    let has_persistent = connected_providers
        .keys()
        .any(|provider_id| !ModelExtensions::is_runtime_provider(provider_id))
        || provider_filter.is_none()
        || provider_filter.is_some_and(|filter| configured_provider_ids.contains(filter))
        || provider_filter
            .is_some_and(|filter| ModelExtensions::is_unauthenticated_free_provider(filter));

    let snapshot_models = crate::model::effective_catalog::models_for_dialog()
        .context("failed to load effective model catalog")?;
    let mut models = if let Some(models) = snapshot_models {
        models
    } else if has_persistent {
        match discovery.fetch_models().await {
            Ok(models) => models
                .into_iter()
                .filter(|model| !ModelExtensions::is_runtime_provider(&model.provider_id))
                .collect(),
            Err(error) if has_runtime => {
                crate::emit_log!("Skipped persistent model catalog: {}", error);
                Vec::new()
            }
            Err(error) => return Err(error).context("failed to fetch models"),
        }
    } else {
        Vec::new()
    };
    discovery.apply_custom_models_to_dialog(&mut models);

    let mut runtime_errors = Vec::new();
    if has_runtime {
        let runtime = ModelExtensions::runtime_models_for_dialog_cached().await;
        merge_dialog_models(&mut models, runtime.models);
        runtime_errors = runtime.errors;
    }

    models.retain(|model| {
        is_model_selectable(model, &connected_provider_ids, &configured_provider_ids)
            && provider_is_enabled(&config.merged_config, &model.provider_id)
            && provider_matches(&model.provider_id, provider_filter)
    });
    models.sort_by(|left, right| {
        left.provider_id
            .cmp(&right.provider_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut seen = HashSet::new();
    models.retain(|model| seen.insert((model.provider_id.clone(), model.id.clone())));

    if models.is_empty() && has_runtime && (filter_matches_runtime || provider_filter.is_none()) {
        if let Some(error) = runtime_errors.first() {
            bail!(
                "failed to fetch {} models: {}",
                error.provider_name,
                error.error
            );
        }
    }

    Ok(models)
}

pub fn model_ref(model: &Model) -> String {
    format!("{}/{}", model.provider_id, model.id)
}

fn provider_matches(provider_id: &str, provider_filter: Option<&str>) -> bool {
    provider_filter.is_none_or(|filter| provider_id == filter)
}

fn provider_is_enabled(
    config: &crate::config::configuration::MergedConfig,
    provider_id: &str,
) -> bool {
    !config.disabled_providers.contains(provider_id)
        && (config.enabled_providers.is_empty() || config.enabled_providers.contains(provider_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_canonical_model_ref() {
        let model = Model {
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            family: "gpt".into(),
            provider_id: "openai".into(),
            provider_name: "OpenAI".into(),
            attachment: false,
            structured_output: false,
            free: false,
            local: false,
            reasoning_options: Vec::new(),
            context_window: None,
        };

        assert_eq!(model_ref(&model), "openai/gpt-5");
    }

    #[test]
    fn provider_filter_is_exact() {
        assert!(provider_matches("opencode", Some("opencode")));
        assert!(!provider_matches("opencode-go", Some("opencode")));
        assert!(provider_matches("opencode-go", None));
    }

    #[test]
    fn provider_policy_applies_allowlist_and_blocklist() {
        let mut config = crate::config::configuration::MergedConfig::default();
        assert!(provider_is_enabled(&config, "openai"));

        config.enabled_providers.insert("anthropic".into());
        assert!(provider_is_enabled(&config, "anthropic"));
        assert!(!provider_is_enabled(&config, "openai"));

        config.disabled_providers.insert("anthropic".into());
        assert!(!provider_is_enabled(&config, "anthropic"));
    }
}
