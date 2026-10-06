//! Endpoint validation, Design MCP defaults, and metadata mapping for opt-in Meridian.

use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

use crate::config::configuration::{McpRemoteConfig, McpServerConfig, MergedConfig};
use crate::config::CustomProviderConfig;
use crate::model::discovery;
use crate::persistence::AuthConfig;

pub const PROVIDER_ID: &str = "meridian";
pub const PROVIDER_NAME: &str = "Meridian";
pub const BASE_URL: &str = "http://127.0.0.1:3456/v1";
pub const NPM_PACKAGE: &str = "@ai-sdk/openai-compatible";
pub const API_KEY_ENV: &str = "MERIDIAN_API_KEY";
pub const DOC_URL: &str = "https://github.com/rynfar/meridian/blob/main/docs/agents.md";
pub const DESIGN_MCP_NAME: &str = "claude-design";
pub const DESIGN_SETTINGS_URL: &str = "https://claude.ai/design/settings";

const DEFAULT_OUTPUT_LIMIT: u32 = 8_192;

fn design_policy() -> crate::config::mcp_capability::McpCapabilityPolicy {
    use crate::config::mcp_capability::{
        AuthorizationAction, McpCapabilityPolicy, Recovery, RecoveryKind, RecoveryRule,
    };
    let consent = format!("Enable Claude Design access: turn on 'Claude product access' at {DESIGN_SETTINGS_URL} for Meridian's active account, then retry. This grants access to Design projects, not all chat artifacts.");
    McpCapabilityPolicy {
        authorization: Some(AuthorizationAction {
            url: DESIGN_SETTINGS_URL.to_string(),
            instructions: format!("{consent} For auth_error, use Meridian's /design-login flow. No login is started automatically."),
        }),
        recovery_rules: vec![
            RecoveryRule { markers: vec!["needs_consent".into()], recovery: Recovery { kind: RecoveryKind::Consent, message: consent } },
            RecoveryRule { markers: vec!["auth_error".into(), "401".into(), "403".into()], recovery: Recovery { kind: RecoveryKind::Authorization, message: "Claude Design needs authorization. Check your Meridian endpoint key and account login; if upstream reports auth_error, use Meridian's /design-login flow, then retry. Crabcode does not start this login automatically.".into() } },
            RecoveryRule { markers: vec!["404".into(), "405".into()], recovery: Recovery { kind: RecoveryKind::Unavailable, message: "Claude Design is unavailable on this Meridian endpoint; update Meridian or disable it in /mcp. Model chat is unaffected.".into() } },
        ],
    }
}

/// Add the Design transport only for an explicitly configured or saved connection.
/// This only constructs configuration: it never connects or starts authentication.
pub fn add_design_mcp(config: &mut MergedConfig, connection: Option<&AuthConfig>) {
    let configured_key = config
        .custom_providers
        .get(PROVIDER_ID)
        .and_then(CustomProviderConfig::resolved_api_key);
    add_design_mcp_with_key(config, connection, configured_key, || {
        std::env::var(API_KEY_ENV).ok()
    });
}

fn add_design_mcp_with_key(
    config: &mut MergedConfig,
    connection: Option<&AuthConfig>,
    configured_key: Option<String>,
    environment_key: impl FnOnce() -> Option<String>,
) {
    for server in config.mcp.values_mut() {
        if let McpServerConfig::Remote(remote) = server {
            if !remote.oauth_enabled && is_design_mcp(remote) {
                remote.capability = design_policy();
            }
        }
    }
    let provider = config.custom_providers.get(PROVIDER_ID);
    if !config.provider_is_enabled(PROVIDER_ID)
        || (provider.is_none()
            && !matches!(connection, Some(AuthConfig::Local | AuthConfig::Api { .. })))
        || config.mcp.contains_key(DESIGN_MCP_NAME)
    {
        return;
    }

    let base = provider
        .and_then(|provider| provider.base_url.as_deref())
        .unwrap_or(BASE_URL);
    let Some(mut base) = normalized_design_url(base) else {
        return;
    };
    if let Some(path) = base.path().strip_suffix("/models") {
        let path = path.to_string();
        base.set_path(&path);
    }
    let Ok(mut url) = discovery::openai_models_endpoint(base.as_str()) else {
        return;
    };
    let Some(path) = url.path().strip_suffix("/models") else {
        return;
    };
    url.set_path(&format!("{path}/design/mcp"));
    if config.mcp.values().any(|server| matches!(server,
        McpServerConfig::Remote(remote) if normalized_design_url(&remote.url) == Some(url.clone())
    )) {
        return;
    }
    let key = endpoint_key(connection, configured_key.or_else(environment_key));
    let mut headers = std::collections::HashMap::new();
    if let Some(key) = key.as_deref().map(str::trim).filter(|key| !key.is_empty()) {
        let value = format!("Bearer {key}");
        // Do not create unusable headers or allow a key to inject another header.
        if reqwest::header::HeaderValue::from_str(&value).is_ok() {
            headers.insert("Authorization".to_string(), value);
        }
    }
    config.mcp.insert(
        DESIGN_MCP_NAME.to_string(),
        McpServerConfig::Remote(McpRemoteConfig {
            capability: design_policy(),
            url: url.to_string(),
            headers,
            enabled: true,
            timeout_ms: Some(5_000),
            oauth_enabled: false,
            oauth_client_id: None,
            oauth_client_secret: None,
            oauth_scope: None,
        }),
    );
    // Account-backed defaults must not silently grant edit/publish privileges.
    // Explicit user rules appear later and retain their usual precedence.
    config.permission_rules.insert(
        0,
        crate::tools::permission::PermissionRule {
            permission: "claude-design_*".to_string(),
            pattern: "*".to_string(),
            action: crate::tools::permission::PermissionPolicyAction::Ask,
        },
    );
}

fn normalized_design_url(value: &str) -> Option<reqwest::Url> {
    let mut url = reqwest::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Some(url)
}

/// Recognize Design endpoints by their parsed route, not a user-chosen MCP name.
pub fn is_design_mcp(remote: &McpRemoteConfig) -> bool {
    normalized_design_url(&remote.url).is_some_and(|url| url.path().ends_with("/design/mcp"))
}

pub fn provider() -> discovery::Provider {
    discovery::Provider {
        id: PROVIDER_ID.to_string(),
        name: PROVIDER_NAME.to_string(),
        api: BASE_URL.to_string(),
        doc: DOC_URL.to_string(),
        // An optional key for the endpoint itself, never an upstream credential.
        env: vec![API_KEY_ENV.to_string()],
        npm: NPM_PACKAGE.to_string(),
        models: Default::default(),
    }
}

/// Only Crabcode's own endpoint connection state is relevant here. A keyless
/// connection intentionally supersedes a configured/old placeholder key.
pub fn endpoint_key(
    connection: Option<&crate::persistence::AuthConfig>,
    configured_key: Option<String>,
) -> Option<String> {
    match connection {
        Some(crate::persistence::AuthConfig::Local) => None,
        Some(crate::persistence::AuthConfig::Api { key }) => Some(key.clone()),
        _ => configured_key,
    }
}

pub fn apply_connection(
    providers: &mut Option<std::collections::HashMap<String, CustomProviderConfig>>,
    connection: Option<&crate::persistence::AuthConfig>,
) {
    if matches!(
        connection,
        Some(crate::persistence::AuthConfig::Local | crate::persistence::AuthConfig::Api { .. })
    ) {
        providers
            .get_or_insert_with(Default::default)
            .entry(PROVIDER_ID.to_string())
            .or_insert_with(|| {
                with_defaults(CustomProviderConfig {
                    name: None,
                    npm: None,
                    base_url: None,
                    api_key: None,
                    models: Default::default(),
                })
            });
    }
    if let Some(config) = providers.as_mut().and_then(|p| p.get_mut(PROVIDER_ID)) {
        *config = with_defaults(config.clone());
        if matches!(connection, Some(crate::persistence::AuthConfig::Local)) {
            config.api_key = None;
        }
    }
}

/// Fill transport defaults without opting in to authentication or changing models.
pub fn with_defaults(mut config: CustomProviderConfig) -> CustomProviderConfig {
    config.name.get_or_insert_with(|| PROVIDER_NAME.to_string());
    config.npm.get_or_insert_with(|| NPM_PACKAGE.to_string());
    config.base_url.get_or_insert_with(|| BASE_URL.to_string());
    config
}

/// Validate the endpoint's model list without starting Meridian or using upstream auth.
/// `endpoint_key` is already resolved by the caller; no credential stores are read here.
pub async fn check_connection(
    config: CustomProviderConfig,
    endpoint_key: Option<String>,
) -> anyhow::Result<usize> {
    #[derive(serde::Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelId>,
    }

    #[derive(serde::Deserialize)]
    struct ModelId {
        id: String,
    }

    let config = with_defaults(config);
    let endpoint = discovery::openai_models_endpoint(
        config.base_url.as_deref().unwrap_or(BASE_URL),
    )
    .map_err(|_| anyhow::anyhow!("Invalid Meridian endpoint URL; check the configured base URL"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| anyhow::anyhow!("Could not create the Meridian endpoint HTTP client"))?;
    let mut request = client.get(endpoint).header("Accept", "application/json");
    if let Some(key) = endpoint_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
    {
        request = request.bearer_auth(key);
    }

    let response = request.send().await.map_err(connection_error)?;
    let status = response.status();
    if matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        anyhow::bail!(
            "Meridian endpoint is protected (HTTP {status}); supply a valid Meridian endpoint key, not backend account credentials"
        );
    }
    if !status.is_success() {
        anyhow::bail!("Meridian endpoint returned HTTP {status} while listing models");
    }

    let models = response.json::<ModelsResponse>().await.map_err(|error| {
        if error.is_decode() {
            // Even JSON decoding errors can quote payload values. Do not expose them.
            anyhow::anyhow!(
                "Invalid Meridian models payload; expected JSON with a data array of models with string IDs"
            )
        } else {
            connection_error(error)
        }
    })?;
    let ids: HashSet<_> = models
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() {
        anyhow::bail!(
            "Meridian returned no models with nonempty IDs; connection could not be validated. Check Meridian's passthrough configuration and retry"
        );
    }
    Ok(ids.len())
}

fn connection_error(error: reqwest::Error) -> anyhow::Error {
    // Avoid retaining reqwest's URL or other potentially sensitive error details.
    if error.is_timeout() {
        anyhow::anyhow!(
            "Meridian connection timed out after 5 seconds. It may not be running; start Meridian in passthrough mode and retry"
        )
    } else if error.is_connect() {
        anyhow::anyhow!(
            "Could not connect to Meridian. It may not be running; start Meridian in passthrough mode and retry"
        )
    } else {
        anyhow::anyhow!(
            "Could not request Meridian models; check the endpoint URL and optional endpoint key"
        )
    }
}

pub fn model_from_metadata(id: &str, metadata: &Value) -> discovery::Model {
    let name = ["display_name", "name"]
        .iter()
        .find_map(|key| {
            metadata
                .get(*key)?
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .unwrap_or(id);
    let context = positive_limit(metadata, &["context_window", "context_length"]).unwrap_or(0);
    let output = positive_limit(metadata, &["max_output_tokens", "max_tokens"])
        .unwrap_or(DEFAULT_OUTPUT_LIMIT);
    let output = if context > 0 {
        output.min(context)
    } else {
        output
    };
    let image_input = capability(metadata, &["image_input"]).unwrap_or(false);
    let pdf_input = capability(metadata, &["pdf_input"]).unwrap_or(false);
    let mut input = vec!["text".to_string()];
    if image_input {
        input.push("image".to_string());
    }
    if pdf_input {
        input.push("pdf".to_string());
    }

    discovery::Model {
        id: id.to_string(),
        name: name.to_string(),
        family: String::new(),
        attachment: image_input || pdf_input,
        reasoning: capability(metadata, &["thinking", "reasoning"]).unwrap_or(false),
        // Advertised thinking support does not verify reasoning transport semantics.
        reasoning_options: Vec::new(),
        // Some backend catalogs omit a tool flag; this endpoint supports client tools.
        tool_call: capability(
            metadata,
            &["tool_call", "tools", "tool_use", "function_calling"],
        )
        .unwrap_or(true),
        structured_output: capability(metadata, &["structured_outputs"]).unwrap_or(false),
        temperature: capability(metadata, &["temperature"]).unwrap_or(true),
        knowledge: String::new(),
        release_date: String::new(),
        last_updated: String::new(),
        status: None,
        modalities: Some(discovery::Modalities {
            input,
            output: vec!["text".to_string()],
        }),
        open_weights: false,
        // Subscription-backed endpoint usage must not inherit upstream API pricing.
        cost: None,
        limit: Some(discovery::Limit { context, output }),
        // Keep routing on the configured endpoint, regardless of the backend or ID.
        provider: None,
    }
}

fn positive_limit(metadata: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| {
        let value = u32::try_from(metadata.get(*key)?.as_u64()?).ok()?;
        (value > 0).then_some(value)
    })
}

fn capability(metadata: &Value, keys: &[&str]) -> Option<bool> {
    // Prefer the declared capability object; allow flat backend metadata as well.
    // First valid declaration wins, including false (which is not a missing value).
    metadata
        .get("capabilities")
        .into_iter()
        .chain(std::iter::once(metadata))
        .find_map(|source| {
            keys.iter().find_map(|key| {
                let value = source.get(*key)?;
                value
                    .as_bool()
                    .or_else(|| value.get("supported").and_then(Value::as_bool))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn empty_config() -> CustomProviderConfig {
        CustomProviderConfig {
            name: None,
            npm: None,
            base_url: None,
            api_key: None,
            models: HashMap::new(),
        }
    }

    #[test]
    fn meridian_keyless_connection_rehydrates_and_overrides_placeholder_key() {
        use crate::persistence::AuthConfig;
        let mut providers = None;
        apply_connection(&mut providers, Some(&AuthConfig::Local));
        assert_eq!(
            providers.as_ref().unwrap()[PROVIDER_ID].base_url.as_deref(),
            Some(BASE_URL)
        );
        let config = providers.as_mut().unwrap().get_mut(PROVIDER_ID).unwrap();
        config.api_key = Some("old-placeholder".to_string());
        config.base_url = Some("http://127.0.0.1:4567/antigravity/v1".to_string());
        apply_connection(&mut providers, Some(&AuthConfig::Local));
        let config = &providers.as_ref().unwrap()[PROVIDER_ID];
        assert!(config.api_key.is_none());
        assert_eq!(
            config.base_url.as_deref(),
            Some("http://127.0.0.1:4567/antigravity/v1")
        );
        assert!(endpoint_key(
            Some(&AuthConfig::Local),
            Some("old-placeholder".to_string())
        )
        .is_none());
        assert_eq!(
            endpoint_key(
                Some(&AuthConfig::Api {
                    key: "endpoint-key".to_string()
                }),
                None
            )
            .as_deref(),
            Some("endpoint-key")
        );
        let encoded = serde_json::to_string(&AuthConfig::Local).unwrap();
        assert_eq!(encoded, r#"{"type":"local"}"#);
        assert!(matches!(
            serde_json::from_str::<AuthConfig>(&encoded).unwrap(),
            AuthConfig::Local
        ));
        let mut disconnected = None;
        apply_connection(&mut disconnected, None);
        assert!(disconnected.is_none());
    }

    fn design_config(base: Option<&str>) -> MergedConfig {
        let mut config = MergedConfig::default();
        let mut provider = empty_config();
        provider.base_url = base.map(str::to_string);
        config
            .custom_providers
            .insert(PROVIDER_ID.to_string(), provider);
        config
    }

    fn design_remote(config: &MergedConfig) -> &McpRemoteConfig {
        match &config.mcp[DESIGN_MCP_NAME] {
            McpServerConfig::Remote(remote) => remote,
            _ => panic!("expected remote"),
        }
    }

    #[test]
    fn design_adapter_owns_recovery_and_authorization_guidance() {
        use crate::config::mcp_capability::RecoveryKind;
        let mut config = design_config(None);
        add_design_mcp_with_key(&mut config, None, None, || None);
        let policy = &design_remote(&config).capability;
        assert_eq!(
            policy.authorization.as_ref().unwrap().url,
            DESIGN_SETTINGS_URL
        );
        let consent = policy.recovery("NEEDS_CONSENT 401").unwrap();
        assert_eq!(consent.kind, RecoveryKind::Consent);
        assert!(consent.message.contains(DESIGN_SETTINGS_URL));
        for message in ["auth_error", "401 Unauthorized", "403 Forbidden"] {
            let recovery = policy.recovery(message).unwrap();
            assert_eq!(recovery.kind, RecoveryKind::Authorization);
            assert!(recovery.message.contains("/design-login"));
        }
        assert_eq!(
            policy.recovery("404 Not Found").unwrap().kind,
            RecoveryKind::Unavailable
        );
        assert!(policy.recovery("connection refused").is_none());

        // Existing manual aliases receive recovery guidance without transport changes.
        let mut manual = config.mcp.remove(DESIGN_MCP_NAME).unwrap();
        if let McpServerConfig::Remote(remote) = &mut manual {
            remote.capability = Default::default();
            remote.enabled = false;
        }
        config.mcp.insert("manual-alias".into(), manual);
        add_design_mcp_with_key(&mut config, None, None, || None);
        assert_eq!(config.mcp.len(), 1);
        let McpServerConfig::Remote(remote) = &config.mcp["manual-alias"] else {
            unreachable!()
        };
        assert!(!remote.enabled);
        assert!(remote.capability.recovery("needs_consent").is_some());
    }

    #[test]
    fn design_default_permissions_ask_and_preserve_user_precedence() {
        use crate::tools::permission::{PermissionPolicyAction, PermissionRule};
        let mut config = design_config(None);
        let explicit = PermissionRule {
            permission: "claude-design_*".to_string(),
            pattern: "*".to_string(),
            action: PermissionPolicyAction::Deny,
        };
        config.permission_rules.push(explicit.clone());
        add_design_mcp_with_key(&mut config, None, None, || None);
        assert_eq!(config.permission_rules.len(), 2);
        assert_eq!(
            config.permission_rules[0].action,
            PermissionPolicyAction::Ask
        );
        assert_eq!(config.permission_rules[1], explicit);
        // Reapplying defaults must not duplicate rules or overwrite manual config.
        add_design_mcp_with_key(&mut config, None, None, || None);
        assert_eq!(config.permission_rules.len(), 2);
    }

    #[test]
    fn design_requires_opt_in_and_respects_provider_filters() {
        let mut disconnected = MergedConfig::default();
        add_design_mcp_with_key(&mut disconnected, None, None, || Some("env-key".into()));
        assert!(disconnected.mcp.is_empty());
        for connection in [
            None,
            Some(AuthConfig::Local),
            Some(AuthConfig::Api { key: "key".into() }),
            Some(AuthConfig::OAuth {
                access: "upstream".into(),
                refresh: "refresh".into(),
                expires: 0,
                account_id: None,
                enterprise_url: None,
            }),
        ] {
            let mut config = MergedConfig::default();
            add_design_mcp_with_key(&mut config, connection.as_ref(), None, || None);
            assert_eq!(
                !config.mcp.is_empty(),
                matches!(connection, Some(AuthConfig::Local | AuthConfig::Api { .. }))
            );
        }
        for disabled in [false, true] {
            let mut config = design_config(None);
            if disabled {
                config.disabled_providers.insert(PROVIDER_ID.into());
            } else {
                config.enabled_providers.insert("openai".into());
            }
            add_design_mcp_with_key(&mut config, Some(&AuthConfig::Local), None, || None);
            assert!(config.mcp.is_empty());
        }
        let mut config = design_config(None);
        config.enabled_providers.insert(PROVIDER_ID.into());
        add_design_mcp_with_key(&mut config, None, None, || None);
        let remote = design_remote(&config);
        assert_eq!(remote.url, "http://127.0.0.1:3456/v1/design/mcp");
        assert_eq!(remote.timeout_ms, Some(5000));
        assert!(!remote.oauth_enabled);
        assert!(remote.headers.is_empty());
        assert!(is_design_mcp(remote));
    }

    #[test]
    fn design_derives_urls_and_rejects_unsafe_bases() {
        for (base, expected) in [
            (
                "https://EXAMPLE.com:443",
                "https://example.com/v1/design/mcp",
            ),
            (
                "http://localhost:3456/v1/",
                "http://localhost:3456/v1/design/mcp",
            ),
            (
                "https://example.com/proxy",
                "https://example.com/proxy/v1/design/mcp",
            ),
            (
                "https://example.com/proxy/v1/models/",
                "https://example.com/proxy/v1/design/mcp",
            ),
            (
                "https://example.com/proxy/v4",
                "https://example.com/proxy/v4/design/mcp",
            ),
        ] {
            let mut config = design_config(Some(base));
            add_design_mcp_with_key(&mut config, None, None, || None);
            assert_eq!(design_remote(&config).url, expected);
        }
        for base in [
            "",
            "not a URL",
            "file:///tmp/v1",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?q=1",
            "https://example.com/v1#fragment",
        ] {
            let mut config = design_config(Some(base));
            add_design_mcp_with_key(&mut config, None, None, || None);
            assert!(config.mcp.is_empty(), "{base}");
        }
    }

    #[test]
    fn design_preserves_manual_names_and_disabled_aliases() {
        for name in [DESIGN_MCP_NAME, "my-design"] {
            for enabled in [true, false] {
                let mut config = design_config(None);
                add_design_mcp_with_key(&mut config, None, None, || None);
                let mut manual = config.mcp.remove(DESIGN_MCP_NAME).unwrap();
                manual.set_enabled(enabled);
                if let McpServerConfig::Remote(remote) = &mut manual {
                    remote.url.push('/');
                    remote
                        .headers
                        .insert("x-meridian-profile".into(), "personal".into());
                }
                config.mcp.insert(name.into(), manual.clone());
                add_design_mcp_with_key(&mut config, Some(&AuthConfig::Local), None, || None);
                assert_eq!(config.mcp.len(), 1);
                assert_eq!(config.mcp[name], manual);
            }
        }
        let mut config = design_config(None);
        let manual = McpServerConfig::Local(crate::config::configuration::McpLocalConfig {
            command: vec!["custom".into()],
            cwd: None,
            environment: HashMap::new(),
            enabled: false,
            timeout_ms: None,
        });
        config.mcp.insert(DESIGN_MCP_NAME.into(), manual.clone());
        add_design_mcp_with_key(&mut config, None, None, || None);
        assert_eq!(config.mcp[DESIGN_MCP_NAME], manual);

        let mut config = design_config(Some("https://example.com/proxy/v1"));
        add_design_mcp_with_key(&mut config, None, None, || None);
        let mut alias = config.mcp.remove(DESIGN_MCP_NAME).unwrap();
        if let McpServerConfig::Remote(remote) = &mut alias {
            remote.url = "https://EXAMPLE.com:443/proxy/v1/design/mcp/".into();
            remote.enabled = false;
        }
        config.mcp.insert("alias".into(), alias.clone());
        add_design_mcp_with_key(&mut config, None, None, || None);
        assert_eq!(config.mcp.len(), 1);
        assert_eq!(config.mcp["alias"], alias);
    }

    #[test]
    fn design_uses_only_endpoint_keys_with_saved_connections_taking_precedence() {
        let oauth = AuthConfig::OAuth {
            access: "upstream-secret".into(),
            refresh: "refresh".into(),
            expires: 0,
            account_id: None,
            enterprise_url: None,
        };
        for (connection, configured, environment, expected) in [
            (
                None,
                Some("configured"),
                Some("env"),
                Some("Bearer configured"),
            ),
            (None, None, Some("env"), Some("Bearer env")),
            (None, None, None, None),
            (
                Some(AuthConfig::Local),
                Some("configured"),
                Some("env"),
                None,
            ),
            (
                Some(AuthConfig::Api {
                    key: " saved ".into(),
                }),
                Some("configured"),
                Some("env"),
                Some("Bearer saved"),
            ),
            (Some(oauth.clone()), None, None, None),
            (
                Some(oauth),
                Some("configured"),
                None,
                Some("Bearer configured"),
            ),
            (
                Some(AuthConfig::Api {
                    key: "bad\r\nheader".into(),
                }),
                None,
                None,
                None,
            ),
        ] {
            let mut config = design_config(None);
            add_design_mcp_with_key(
                &mut config,
                connection.as_ref(),
                configured.map(str::to_string),
                || environment.map(str::to_string),
            );
            assert_eq!(
                design_remote(&config)
                    .headers
                    .get("Authorization")
                    .map(String::as_str),
                expected
            );
        }
        // Explicit env references use the same resolver as model requests without
        // mutating the process-wide environment in tests.
        let mut provider = empty_config();
        provider.api_key = Some("{env:PATH}".into());
        assert_eq!(
            provider.resolved_api_key(),
            std::env::var("PATH")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        );
    }

    #[test]
    fn design_recognition_is_route_based() {
        let mut config = design_config(None);
        add_design_mcp_with_key(&mut config, None, None, || None);
        let mut remote = design_remote(&config).clone();
        for (url, expected) in [
            ("https://example.com/design/mcp", true),
            ("https://example.com/proxy/v1/design/mcp/", true),
            ("https://example.com/v1/models", false),
            ("https://example.com/v1/design/mcp-other", false),
            ("https://example.com/?path=/v1/design/mcp", false),
            ("invalid/v1/design/mcp", false),
        ] {
            remote.url = url.into();
            assert_eq!(is_design_mcp(&remote), expected);
        }
    }

    async fn mock_endpoint(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), async move {
                let (mut stream, _) = listener.accept().await.expect("connection");
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    let count = stream.read(&mut buffer).await.expect("request");
                    assert!(count > 0, "connection closed before headers arrived");
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() <= 16_384, "request headers are too large");
                }
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("response");
                String::from_utf8(request).expect("request headers")
            })
            .await
            .expect("mock request timed out")
        });
        (format!("http://{address}"), server)
    }

    fn authorization(request: &str) -> Option<&str> {
        request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim())
        })
    }

    #[tokio::test]
    async fn check_connection_without_key_counts_unique_nonempty_ids_and_uses_models_url() {
        for (suffix, path, key) in [
            ("/v1/", "/v1/models", None),
            ("/api", "/api/v1/models", Some(String::new())),
            (
                "/api/v4/?ignored=true#fragment",
                "/api/v4/models",
                Some(" \n\t".to_string()),
            ),
        ] {
            let (base_url, server) = mock_endpoint(
                "200 OK",
                &json!({"data": [
                    {"id": "model-a"},
                    {"id": "model-a"},
                    {"id": " model-a "},
                    {"id": "model-b"},
                    {"id": ""},
                    {"id": " \n\t"}
                ]})
                .to_string(),
            )
            .await;
            let mut config = empty_config();
            config.base_url = Some(format!("{base_url}{suffix}"));
            // The helper must use only the already-selected endpoint_key argument.
            config.api_key = Some("must-not-be-used-as-an-endpoint-key".to_string());

            let count = check_connection(config, key).await.expect("connection");
            let request = server.await.expect("server");

            assert_eq!(count, 2);
            assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
            assert_eq!(authorization(&request), None);
        }
    }

    #[tokio::test]
    async fn check_connection_with_protected_endpoint_uses_trimmed_bearer_key() {
        let (base_url, server) = mock_endpoint("200 OK", r#"{"data":[{"id":"model-a"}]}"#).await;
        let mut config = empty_config();
        config.base_url = Some(format!("{base_url}/v1"));
        config.api_key = Some("different-config-key".to_string());

        let count = check_connection(config, Some(" \tendpoint-key\n ".to_string()))
            .await
            .expect("protected connection");
        let request = server.await.expect("server");

        assert_eq!(count, 1);
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert_eq!(authorization(&request), Some("Bearer endpoint-key"));
    }

    #[tokio::test]
    async fn check_connection_rejects_unauthorized_and_forbidden_without_exposing_body() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            for key in [None, Some("incorrect-endpoint-key".to_string())] {
                let (base_url, server) =
                    mock_endpoint(status, r#"{"error":"private-response-secret"}"#).await;
                let mut config = empty_config();
                config.base_url = Some(base_url);

                let error = check_connection(config, key).await.unwrap_err();
                server.await.expect("server");
                let message = format!("{error:#}");

                assert!(message.contains(&format!("HTTP {status}")), "{message}");
                assert!(message.contains("endpoint is protected"), "{message}");
                assert!(
                    message.contains("endpoint key, not backend account credentials"),
                    "{message}"
                );
                assert!(!message.contains("private-response-secret"));
                assert!(!message.contains("incorrect-endpoint-key"));
            }
        }
    }

    #[tokio::test]
    async fn check_connection_rejects_other_http_errors_without_exposing_body() {
        let (base_url, server) =
            mock_endpoint("500 Internal Server Error", "private-response-secret").await;
        let mut config = empty_config();
        config.base_url = Some(base_url);

        let error = check_connection(config, None).await.unwrap_err();
        server.await.expect("server");
        let message = format!("{error:#}");

        assert!(
            message.contains("HTTP 500 Internal Server Error"),
            "{message}"
        );
        assert!(message.contains("while listing models"), "{message}");
        assert!(!message.contains("private-response-secret"));
    }

    #[tokio::test]
    async fn check_connection_rejects_empty_model_lists_including_empty_ids() {
        for body in [
            json!({"data": []}),
            json!({"data": [{"id": ""}, {"id": " \n\t"}]}),
        ] {
            let (base_url, server) = mock_endpoint("200 OK", &body.to_string()).await;
            let mut config = empty_config();
            config.base_url = Some(base_url);

            let error = check_connection(config, None).await.unwrap_err();
            server.await.expect("server");
            let message = error.to_string();

            assert!(message.contains("no models with nonempty IDs"), "{message}");
            assert!(message.contains("could not be validated"), "{message}");
        }
    }

    #[tokio::test]
    async fn check_connection_rejects_malformed_json_and_invalid_payload_shapes() {
        for body in [
            "private-response-secret is not JSON",
            r#"{"error":"private-response-secret"}"#,
            r#"{"data":null}"#,
            r#"{"data":{}}"#,
            r#"{"data":[{}]}"#,
            r#"{"data":[{"id":{"private-response-secret":true}}]}"#,
        ] {
            let (base_url, server) = mock_endpoint("200 OK", body).await;
            let mut config = empty_config();
            config.base_url = Some(base_url);

            let error = check_connection(config, None).await.unwrap_err();
            server.await.expect("server");
            let message = format!("{error:#}");

            assert!(
                message.contains("Invalid Meridian models payload"),
                "{message}"
            );
            assert!(!message.contains("private-response-secret"));
        }
    }

    #[tokio::test]
    async fn check_connection_refused_explains_how_to_start_meridian() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        drop(listener);
        let mut config = empty_config();
        config.base_url = Some(format!("http://{address}/v1"));

        let error = check_connection(config, None).await.unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("Could not connect to Meridian"),
            "{message}"
        );
        assert!(message.contains("not be running"), "{message}");
        assert!(
            message.contains("start Meridian in passthrough mode"),
            "{message}"
        );
    }

    fn assert_text_only_defaults(model: &discovery::Model) {
        assert!(model.tool_call);
        assert!(model.temperature);
        assert!(!model.attachment);
        assert!(!model.reasoning);
        assert!(!model.structured_output);
        assert!(!model.open_weights);
        assert!(model.reasoning_options.is_empty());
        assert!(model.cost.is_none());
        assert!(model.provider.is_none());
        let modalities = model.modalities.as_ref().unwrap();
        assert_eq!(modalities.input, ["text"]);
        assert_eq!(modalities.output, ["text"]);
        let limit = model.limit.as_ref().unwrap();
        assert_eq!(limit.context, 0);
        assert_eq!(limit.output, DEFAULT_OUTPUT_LIMIT);
    }

    #[test]
    fn provider_has_no_static_models_and_only_an_optional_endpoint_key() {
        let provider = provider();

        assert_eq!(provider.id, "meridian");
        assert_eq!(provider.name, "Meridian");
        assert_eq!(provider.api, "http://127.0.0.1:3456/v1");
        assert_eq!(provider.npm, "@ai-sdk/openai-compatible");
        assert_eq!(provider.doc, DOC_URL);
        assert_eq!(provider.env, ["MERIDIAN_API_KEY"]);
        assert!(provider.models.is_empty());
    }

    #[test]
    fn config_defaults_leave_api_key_unset() {
        let config = with_defaults(empty_config());

        assert_eq!(config.name.as_deref(), Some(PROVIDER_NAME));
        assert_eq!(config.npm.as_deref(), Some(NPM_PACKAGE));
        assert_eq!(config.base_url.as_deref(), Some(BASE_URL));
        assert!(config.api_key.is_none());
        assert!(config.models.is_empty());
    }

    #[test]
    fn config_defaults_preserve_explicit_values_and_models() {
        let mut config = empty_config();
        config.name = Some("Private bridge".to_string());
        config.npm = Some("custom-transport".to_string());
        config.base_url = Some("http://localhost:4567/v1".to_string());
        config.api_key = Some("{env:PRIVATE_ENDPOINT_KEY}".to_string());
        config.models.insert(
            "custom-model".to_string(),
            crate::config::configuration::CustomModelConfig {
                name: Some("Custom model".to_string()),
                context_window: Some(32_768),
                max_tokens: Some(4_096),
                attachment: Some(false),
                reasoning: Some(false),
                reasoning_options: None,
                temperature: Some(false),
                tool_call: Some(false),
                modalities: None,
                launch: true,
            },
        );
        let expected = format!("{config:?}");

        let config = with_defaults(with_defaults(config));

        assert_eq!(format!("{config:?}"), expected);
    }

    #[test]
    fn config_defaults_only_fill_absent_fields() {
        let mut config = empty_config();
        config.name = Some(String::new());
        config.base_url = Some(String::new());
        config.api_key = Some(String::new());

        let config = with_defaults(config);

        assert_eq!(config.name.as_deref(), Some(""));
        assert_eq!(config.npm.as_deref(), Some(NPM_PACKAGE));
        assert_eq!(config.base_url.as_deref(), Some(""));
        assert_eq!(config.api_key.as_deref(), Some(""));
    }

    #[test]
    fn maps_actual_claude_metadata_without_upstream_routing_or_reasoning_controls() {
        // Meridian's buildModelList/FULL_CAPABILITIES shape, with no tool field.
        let metadata = json!({
            "id": "claude-sonnet-5-5",
            "object": "model",
            "created": 1_791_244_800,
            "owned_by": "anthropic",
            "display_name": "Claude Sonnet 5.5",
            "context_window": 200_000,
            "capabilities": {
                "batch": {"supported": true},
                "citations": {"supported": true},
                "code_execution": {"supported": true},
                "context_management": {
                    "supported": true,
                    "clear_thinking_20251015": {"supported": true},
                    "clear_tool_uses_20250919": {"supported": true},
                    "compact_20260112": {"supported": true}
                },
                "effort": {
                    "supported": true,
                    "low": {"supported": true},
                    "medium": {"supported": true},
                    "high": {"supported": true},
                    "xhigh": {"supported": true},
                    "max": {"supported": true}
                },
                "image_input": {"supported": true},
                "pdf_input": {"supported": true},
                "structured_outputs": {"supported": true},
                "thinking": {
                    "supported": true,
                    "types": {
                        "adaptive": {"supported": true},
                        "enabled": {"supported": true}
                    }
                }
            }
        });

        let model = model_from_metadata("claude-sonnet-5-5", &metadata);

        assert_eq!(model.id, "claude-sonnet-5-5");
        assert_eq!(model.name, "Claude Sonnet 5.5");
        assert!(model.tool_call);
        assert!(model.temperature);
        assert!(model.attachment);
        assert!(model.reasoning);
        assert!(model.structured_output);
        assert!(model.reasoning_options.is_empty());
        assert!(model.reasoning_efforts().is_none());
        assert!(model.provider.is_none());
        assert!(model.cost.is_none());
        let modalities = model.modalities.unwrap();
        assert_eq!(modalities.input, ["text", "image", "pdf"]);
        assert_eq!(modalities.output, ["text"]);
        let limit = model.limit.unwrap();
        assert_eq!(limit.context, 200_000);
        assert_eq!(limit.output, DEFAULT_OUTPUT_LIMIT);
    }

    #[test]
    fn maps_actual_gemini_metadata_without_guessing_claude_capabilities() {
        // The Antigravity backend currently returns only identity metadata.
        let metadata = json!({
            "id": "gemini-3.1-pro",
            "type": "model",
            "object": "model",
            "display_name": "gemini-3.1-pro",
            "owned_by": "antigravity"
        });

        let model = model_from_metadata("gemini-3.1-pro", &metadata);

        assert_eq!(model.id, "gemini-3.1-pro");
        assert_eq!(model.name, "gemini-3.1-pro");
        assert_text_only_defaults(&model);
    }

    #[test]
    fn maps_declared_gemini_limits_and_boolean_capabilities() {
        let model = model_from_metadata(
            "gemini-model",
            &json!({
                "name": " Gemini Model ",
                "context_length": 1_048_576,
                "max_tokens": 65_536,
                "capabilities": {
                    "function_calling": true,
                    "image_input": true,
                    "pdf_input": false,
                    "structured_outputs": true,
                    "thinking": false,
                    "temperature": false
                }
            }),
        );

        assert_eq!(model.name, "Gemini Model");
        assert!(model.tool_call);
        assert!(model.attachment);
        assert!(model.structured_output);
        assert!(!model.reasoning);
        assert!(!model.temperature);
        assert!(model.reasoning_options.is_empty());
        assert!(model.provider.is_none());
        assert!(model.cost.is_none());
        assert_eq!(model.modalities.unwrap().input, ["text", "image"]);
        let limit = model.limit.unwrap();
        assert_eq!(limit.context, 1_048_576);
        assert_eq!(limit.output, 65_536);
    }

    #[test]
    fn names_prefer_clean_display_name_then_name_then_id() {
        for (metadata, expected) in [
            (
                json!({"display_name": " Display ", "name": "Name"}),
                "Display",
            ),
            (json!({"display_name": " \n\t", "name": " Name "}), "Name"),
            (json!({"display_name": false, "name": " Name "}), "Name"),
            (json!({"display_name": {}, "name": ""}), "model-id"),
            (json!({"name": 42, "id": "other-id"}), "model-id"),
        ] {
            let model = model_from_metadata("model-id", &metadata);
            assert_eq!(model.id, "model-id");
            assert_eq!(model.name, expected);
        }
    }

    #[test]
    fn empty_and_malformed_metadata_use_conservative_defaults() {
        for metadata in [
            Value::Null,
            json!({}),
            json!([]),
            json!("not metadata"),
            json!(false),
            json!({
                "context_window": "200000",
                "context_length": -1,
                "max_output_tokens": 1.5,
                "max_tokens": 0,
                "capabilities": {
                    "tool_call": "false",
                    "image_input": {"supported": "true"},
                    "pdf_input": 1,
                    "structured_outputs": [],
                    "thinking": {"types": {"adaptive": {"supported": true}}},
                    "temperature": {"supported": null}
                }
            }),
            json!({"capabilities": ["image_input", "thinking"]}),
        ] {
            let model = model_from_metadata("claude-model", &metadata);
            assert_eq!(model.name, "claude-model");
            assert_text_only_defaults(&model);
        }
    }

    #[test]
    fn limits_use_positive_u32_values_and_clamp_output_to_context() {
        for (metadata, context, output) in [
            (json!({"context_window": 4_096}), 4_096, 4_096),
            (json!({"context_window": 1, "max_tokens": 10}), 1, 1),
            (
                json!({"context_window": 10_000, "max_output_tokens": 20_000}),
                10_000,
                10_000,
            ),
            (
                json!({"context_window": 10_000, "max_output_tokens": 2_000, "max_tokens": 3_000}),
                10_000,
                2_000,
            ),
            (
                json!({"context_window": 20_000, "context_length": 30_000}),
                20_000,
                8_192,
            ),
            (
                json!({"context_window": 0, "context_length": 5_000, "max_output_tokens": 0, "max_tokens": 1_000}),
                5_000,
                1_000,
            ),
            (
                json!({"context_window": u64::MAX, "context_length": 6_000, "max_output_tokens": u64::MAX, "max_tokens": 2_000}),
                6_000,
                2_000,
            ),
            (json!({"max_output_tokens": 32_768}), 0, 32_768),
            (
                json!({"context_window": u32::MAX, "max_output_tokens": u32::MAX}),
                u32::MAX,
                u32::MAX,
            ),
        ] {
            let model = model_from_metadata("model", &metadata);
            let limit = model.limit.unwrap();
            assert_eq!(limit.context, context, "{metadata}");
            assert_eq!(limit.output, output, "{metadata}");
        }

        for invalid in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("8192"),
            json!(u64::MAX),
            Value::Null,
        ] {
            let model = model_from_metadata(
                "model",
                &json!({
                    "context_window": invalid,
                    "context_length": invalid,
                    "max_output_tokens": invalid,
                    "max_tokens": invalid
                }),
            );
            let limit = model.limit.unwrap();
            assert_eq!(limit.context, 0);
            assert_eq!(limit.output, DEFAULT_OUTPUT_LIMIT);
        }
    }

    #[test]
    fn tool_aliases_accept_booleans_and_supported_objects_including_false() {
        for key in ["tool_call", "tools", "tool_use", "function_calling"] {
            for supported in [false, true] {
                for declaration in [json!(supported), json!({"supported": supported})] {
                    for metadata in [
                        json!({"capabilities": {key: declaration}}),
                        json!({key: declaration}),
                    ] {
                        let model = model_from_metadata("model", &metadata);
                        assert_eq!(model.tool_call, supported, "{metadata}");
                    }
                }
            }
        }
    }

    #[test]
    fn explicit_false_is_not_replaced_by_defaults_or_other_declarations() {
        let model = model_from_metadata(
            "claude-model",
            &json!({
                "tool_call": true,
                "image_input": true,
                "temperature": true,
                "capabilities": {
                    "tool_call": {"supported": false},
                    "tools": true,
                    "image_input": false,
                    "pdf_input": {"supported": false},
                    "structured_outputs": false,
                    "thinking": {"supported": false},
                    "temperature": {"supported": false}
                },
                "modalities": {"input": ["image", "pdf"]},
                "cost": {"input": 3, "output": 15},
                "provider": {"npm": "@ai-sdk/anthropic", "api": "https://api.anthropic.com"}
            }),
        );

        assert!(!model.tool_call);
        assert!(!model.attachment);
        assert!(!model.reasoning);
        assert!(!model.structured_output);
        assert!(!model.temperature);
        assert!(model.reasoning_options.is_empty());
        assert!(model.cost.is_none());
        assert!(model.provider.is_none());
        assert_eq!(model.modalities.unwrap().input, ["text"]);
    }

    #[test]
    fn flat_metadata_can_declare_pdf_input_and_reasoning_support() {
        let model = model_from_metadata(
            "model",
            &json!({
                "capabilities": null,
                "pdf_input": true,
                "image_input": false,
                "structured_outputs": {"supported": true},
                "reasoning": {"supported": true},
                "temperature": false
            }),
        );

        assert!(model.attachment);
        assert!(model.reasoning);
        assert!(model.structured_output);
        assert!(!model.temperature);
        assert!(model.reasoning_options.is_empty());
        let modalities = model.modalities.unwrap();
        assert_eq!(modalities.input, ["text", "pdf"]);
        assert_eq!(modalities.output, ["text"]);
    }
}
