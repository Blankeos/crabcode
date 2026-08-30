use crate::config::configuration::{McpRemoteConfig, McpServerConfig};
use anyhow::{anyhow, Result};

pub async fn run(command: McpCliCommand) -> Result<()> {
    match command {
        McpCliCommand::List => list().await,
        McpCliCommand::Auth { name } => auth(&name).await,
        McpCliCommand::Logout { name } => logout(&name),
    }
}

#[derive(Debug, Clone)]
pub enum McpCliCommand {
    List,
    Auth { name: String },
    Logout { name: String },
}

fn load_mcp_config() -> Result<crate::config::configuration::McpConfig> {
    let loaded = crate::config::ConfigLoader::load()?;
    let mut mcp = loaded.merged_config.mcp;
    if let Ok(prefs) = crate::persistence::PrefsDAO::new() {
        crate::remote_mcp::apply_mcp_overrides(&mut mcp, Some(&prefs));
    }
    Ok(mcp)
}

fn remote_named<'a>(
    config: &'a crate::config::configuration::McpConfig,
    name: &str,
) -> Result<(&'a str, &'a McpRemoteConfig)> {
    let (name, server) = config
        .get_key_value(name)
        .ok_or_else(|| anyhow!("MCP server '{name}' not found"))?;
    match server {
        McpServerConfig::Remote(remote) => Ok((name.as_str(), remote)),
        McpServerConfig::Local(_) => {
            anyhow::bail!("MCP server '{name}' is local (stdio); OAuth is only for remote servers")
        }
    }
}

async fn list() -> Result<()> {
    let config = load_mcp_config()?;
    if config.is_empty() {
        println!("No MCP servers configured.");
        return Ok(());
    }
    for (name, server) in &config {
        match server {
            McpServerConfig::Local(local) => {
                let enabled = if local.enabled { "enabled" } else { "disabled" };
                println!("{name}\tlocal\t{enabled}");
            }
            McpServerConfig::Remote(remote) => {
                let enabled = if remote.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                let auth = if super::credentials::has_credentials(name, &remote.url) {
                    "authenticated"
                } else if super::oauth::should_use_oauth(remote) {
                    "needs_auth"
                } else {
                    "no_oauth"
                };
                println!("{name}\tremote\t{enabled}\t{auth}\t{}", remote.url);
            }
        }
    }
    Ok(())
}

async fn auth(name: &str) -> Result<()> {
    let config = load_mcp_config()?;
    let (name, remote) = remote_named(&config, name)?;
    super::oauth::authenticate(name, remote).await?;
    println!("Authenticated MCP server \"{name}\".");
    Ok(())
}

fn logout(name: &str) -> Result<()> {
    let config = load_mcp_config()?;
    let (name, remote) = remote_named(&config, name)?;
    if super::oauth::logout(name, &remote.url)? {
        println!("Removed stored credentials for MCP server \"{name}\".");
    } else {
        println!("No stored credentials for MCP server \"{name}\".");
    }
    Ok(())
}
