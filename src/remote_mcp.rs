use anyhow::{anyhow, Result};
use std::collections::BTreeMap;

use crate::config::McpConfig;
use crate::persistence::PrefsDAO;

pub const MCP_OVERRIDES_PREFS_KEY: &str = "remote_mcp_enabled_overrides";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RemoteMcpServer {
    pub name: String,
    pub enabled: bool,
    pub status: String,
    pub kind: String,
}

pub fn apply_mcp_overrides(config: &mut McpConfig, prefs: Option<&PrefsDAO>) {
    for (name, enabled) in load_overrides(prefs) {
        if let Some(server) = config.get_mut(&name) {
            server.set_enabled(enabled);
        }
    }
}

pub fn remote_mcp_servers(config: &McpConfig) -> Vec<RemoteMcpServer> {
    config
        .iter()
        .map(|(name, server)| RemoteMcpServer {
            name: name.clone(),
            enabled: server.enabled(),
            status: if server.enabled() {
                "enabled"
            } else {
                "disabled"
            }
            .to_string(),
            kind: server.kind().to_string(),
        })
        .collect()
}

pub fn remote_toggle_mcp_server(
    prefs: &PrefsDAO,
    config: &mut McpConfig,
    name: &str,
) -> Result<Vec<RemoteMcpServer>> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("mcp server name required"));
    }

    let Some(server) = config.get_mut(trimmed) else {
        return Err(anyhow!("mcp server not found"));
    };

    let next = !server.enabled();
    server.set_enabled(next);

    let mut overrides = load_overrides(Some(prefs));
    overrides.insert(trimmed.to_string(), next);
    save_overrides(prefs, &overrides)?;

    Ok(remote_mcp_servers(config))
}

fn load_overrides(prefs: Option<&PrefsDAO>) -> BTreeMap<String, bool> {
    let Some(prefs) = prefs else {
        return BTreeMap::new();
    };
    prefs
        .get_json_pref(MCP_OVERRIDES_PREFS_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

fn save_overrides(prefs: &PrefsDAO, overrides: &BTreeMap<String, bool>) -> Result<()> {
    prefs.set_json_pref(MCP_OVERRIDES_PREFS_KEY, &serde_json::to_value(overrides)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpLocalConfig, McpServerConfig};
    use std::collections::HashMap;

    #[test]
    fn lists_servers_with_config_enabled_defaults() {
        let mut config = McpConfig::new();
        config.insert(
            "local".to_string(),
            McpServerConfig::Local(McpLocalConfig {
                command: vec!["mcp".to_string()],
                cwd: None,
                environment: HashMap::new(),
                enabled: false,
                timeout_ms: None,
            }),
        );
        let list = remote_mcp_servers(&config);
        assert_eq!(list.len(), 1);
        let local = list.iter().find(|s| s.name == "local").unwrap();
        assert!(!local.enabled);
        assert_eq!(local.status, "disabled");
    }
}
