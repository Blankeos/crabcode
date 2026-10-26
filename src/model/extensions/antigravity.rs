use std::collections::HashMap;

use crate::model::discovery::Provider;
use crate::persistence::{AuthConfig, AuthDAO};

pub const PROVIDER_NAME: &str = "Google Antigravity";
const MODELS: &[(&str, &str, &str)] = &[
    ("gemini-3-flash", "Gemini 3 Flash", "gemini-3-flash-preview"),
    (
        "gemini-3-pro-low",
        "Gemini 3 Pro Low",
        "gemini-3-pro-preview",
    ),
    (
        "gemini-3-pro-high",
        "Gemini 3 Pro High",
        "gemini-3-pro-preview",
    ),
    (
        "gemini-3.1-pro-low",
        "Gemini 3.1 Pro Low",
        "gemini-3.1-pro-preview",
    ),
    (
        "gemini-3.1-pro-high",
        "Gemini 3.1 Pro High",
        "gemini-3.1-pro-preview",
    ),
];

pub fn apply_to_dialog_models(models: &mut Vec<crate::model::types::Model>, oauth: bool) {
    for model in models
        .iter_mut()
        .filter(|model| model.provider_id == "google")
    {
        model.provider_name = PROVIDER_NAME.into();
    }
    if !oauth {
        return;
    }
    let mapped = MODELS
        .iter()
        .filter_map(|&(id, name, reference)| {
            let mut model = models
                .iter()
                .find(|model| model.provider_id == "google" && model.id == id)
                .or_else(|| {
                    models
                        .iter()
                        .find(|model| model.provider_id == "google" && model.id == reference)
                })?
                .clone();
            model.id = id.into();
            model.name = name.into();
            Some(model)
        })
        .collect::<Vec<_>>();
    models.retain(|model| model.provider_id != "google");
    models.extend(mapped);
}

pub fn uses_oauth() -> bool {
    matches!(
        AuthDAO::new().and_then(|dao| dao.get_provider("google")),
        Ok(Some(AuthConfig::OAuth { .. }))
    )
}

pub fn rename_provider(providers: &mut HashMap<String, Provider>) {
    if let Some(provider) = providers.get_mut("google") {
        provider.name = PROVIDER_NAME.to_string();
    }
}

pub fn augment_catalog(providers: &mut HashMap<String, Provider>) {
    if let Some(provider) = providers.get_mut("google") {
        apply_to_provider(provider, uses_oauth());
    }
}

pub fn apply_to_provider(provider: &mut Provider, oauth: bool) {
    if provider.id != "google" {
        return;
    }
    provider.name = PROVIDER_NAME.to_string();
    if !oauth {
        return;
    }

    // These are backend IDs, not the plugin's antigravity-* picker aliases.
    // See opencode-antigravity-auth/src/plugin/transform/model-resolver.ts.
    provider.models = MODELS
        .iter()
        .filter_map(|&(id, name, reference)| {
            let mut model = provider
                .models
                .get(id)
                .or_else(|| provider.models.get(reference))?
                .clone();
            model.id = id.to_string();
            model.name = name.to_string();
            Some((id.to_string(), model))
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> Provider {
        serde_json::from_value(serde_json::json!({
            "id": "google", "name": "Google",
            "models": {
                "gemini-3-flash-preview": {
                    "id": "gemini-3-flash-preview", "name": "Gemini 3 Flash Preview",
                    "limit": {"context": 1048576, "output": 65536},
                    "cost": {"input": 0.5, "output": 3.0}
                },
                "gemini-3-pro-preview": {"id": "gemini-3-pro-preview", "name": "Gemini 3 Pro Preview"},
                "gemini-2.0-flash": {"id": "gemini-2.0-flash", "name": "Gemini 2.0 Flash"}
            }
        })).unwrap()
    }

    #[test]
    fn api_key_catalog_only_changes_label() {
        let mut provider = provider();
        let before = serde_json::to_value(&provider.models).unwrap();
        apply_to_provider(&mut provider, false);
        assert_eq!(provider.name, PROVIDER_NAME);
        assert_eq!(provider.id, "google");
        assert_eq!(serde_json::to_value(&provider.models).unwrap(), before);
    }

    #[test]
    fn warm_dialog_rows_use_antigravity_labels_and_oauth_model_ids() {
        let row = crate::model::types::Model {
            id: "gemini-3-flash-preview".into(),
            name: "Gemini 3 Flash Preview".into(),
            family: "gemini".into(),
            provider_id: "google".into(),
            provider_name: "Google".into(),
            attachment: true,
            structured_output: true,
            free: false,
            local: false,
            reasoning_options: vec![],
            context_window: Some(1048576),
        };
        let mut rows = vec![row];
        apply_to_dialog_models(&mut rows, false);
        assert_eq!(rows[0].provider_name, PROVIDER_NAME);
        assert_eq!(rows[0].id, "gemini-3-flash-preview");
        apply_to_dialog_models(&mut rows, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "gemini-3-flash");
        assert_eq!(rows[0].context_window, Some(1048576));
        apply_to_dialog_models(&mut rows, true);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn oauth_catalog_uses_wire_ids_and_existing_metadata() {
        let mut provider = provider();
        apply_to_provider(&mut provider, true);
        assert_eq!(provider.name, PROVIDER_NAME);
        assert_eq!(provider.models.len(), 3);
        let flash = &provider.models["gemini-3-flash"];
        assert_eq!(flash.id, "gemini-3-flash");
        assert_eq!(flash.name, "Gemini 3 Flash");
        assert_eq!(flash.limit.as_ref().unwrap().context, 1048576);
        assert_eq!(flash.cost.as_ref().unwrap().input, 0.5);
        assert!(provider.models.contains_key("gemini-3-pro-high"));
        assert!(!provider.models.contains_key("gemini-3.1-pro-high"));
        assert!(!provider.models.contains_key("gemini-2.0-flash"));
        let before = serde_json::to_value(&provider).unwrap();
        apply_to_provider(&mut provider, true);
        assert_eq!(serde_json::to_value(provider).unwrap(), before);
    }

    #[test]
    fn missing_metadata_is_not_invented() {
        let mut provider = provider();
        provider.models.clear();
        apply_to_provider(&mut provider, true);
        assert!(provider.models.is_empty());
    }
}
