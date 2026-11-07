use crate::config::configuration::LoadedConfig;
use agent_client_protocol::schema::v1::McpServer;

pub(super) fn merge_acp_mcp_servers(config: &mut LoadedConfig, servers: Vec<McpServer>) {
    for server in servers {
        let (name, server) = match server {
            McpServer::Stdio(server) => {
                let mut command = vec![server.command.to_string_lossy().into_owned()];
                command.extend(server.args);
                (
                    server.name,
                    crate::config::configuration::McpServerConfig::Local(
                        crate::config::configuration::McpLocalConfig {
                            command,
                            cwd: None,
                            environment: server
                                .env
                                .into_iter()
                                .map(|variable| (variable.name, variable.value))
                                .collect(),
                            enabled: true,
                            timeout_ms: None,
                        },
                    ),
                )
            }
            McpServer::Http(server) => (
                server.name,
                crate::config::configuration::McpServerConfig::Remote(
                    crate::config::configuration::McpRemoteConfig {
                        url: server.url,
                        headers: server
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        enabled: true,
                        timeout_ms: None,
                        oauth_enabled: false,
                        oauth_client_id: None,
                        oauth_client_secret: None,
                        oauth_scope: None,
                    },
                ),
            ),
            McpServer::Sse(server) => (
                server.name,
                crate::config::configuration::McpServerConfig::Remote(
                    crate::config::configuration::McpRemoteConfig {
                        url: server.url,
                        headers: server
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                        enabled: true,
                        timeout_ms: None,
                        oauth_enabled: false,
                        oauth_client_id: None,
                        oauth_client_secret: None,
                        oauth_scope: None,
                    },
                ),
            ),
            #[allow(unreachable_patterns)]
            _ => continue,
        };
        config.merged_config.mcp.insert(name, server);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::config_with_command;

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn merges_client_mcp_servers_into_session_config() {
        let mut config = config_with_command(crate::command::custom::CustomCommand {
            name: "custom".to_string(),
            description: None,
            template: "$ARGUMENTS".to_string(),
            agent: None,
            model: None,
            subtask: Some(false),
            source: crate::command::custom::CustomCommandSource::Config(PathBuf::from(
                "/tmp/opencode.jsonc",
            )),
            workdir: PathBuf::from("/tmp"),
        });
        merge_acp_mcp_servers(
            &mut config,
            vec![serde_json::from_value(serde_json::json!({
                "name": "zed-fs",
                "command": "mcp-server",
                "args": ["--stdio"],
                "env": [{"name": "ROOT", "value": "/workspace"}]
            }))
            .expect("ACP stdio MCP server")],
        );

        let server = config
            .merged_config
            .mcp
            .get("zed-fs")
            .expect("merged ACP MCP server");
        assert_eq!(server.kind(), "local");
        assert!(server.enabled());
    }
}
