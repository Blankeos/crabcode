//! Login-only terminal entry point. Shares the credential store and OAuth
//! implementations with /connect, without constructing an App or a session.
use anyhow::{bail, Context, Result};
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use crate::persistence::{AuthConfig, AuthDAO};

pub async fn run(cwd: Option<PathBuf>) -> Result<()> {
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };
    let config = crate::config::ConfigLoader::load_for(&cwd)
        .context("failed to load configuration for provider login")?;
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("provider login requires an interactive terminal; run crabcode acp --login in a terminal");
    }

    fn open_url(url: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg(url)
                .spawn()
                .map(|_| ())
                .map_err(Into::into)
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(url)
                .spawn()
                .map(|_| ())
                .map_err(Into::into)
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn()
                .map(|_| ())
                .map_err(Into::into)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = url;
            Ok(())
        }
    }
    eprintln!("Connect a provider (no chat will be started).");
    eprintln!(
        "Provider examples: openai, anthropic, xai, google (Google Antigravity), ollama; custom provider IDs are also accepted."
    );
    let provider = prompt("Provider ID (empty to cancel): ")?;
    validate_provider_id(&provider)?;
    if !config.merged_config.provider_is_enabled(&provider) {
        bail!("that provider is disabled by configuration");
    }
    let dao = AuthDAO::new().context("failed to open credential store")?;
    if needs_no_auth(&provider) {
        dao.set_provider(provider, AuthConfig::Local)
            .map_err(|_| anyhow::anyhow!("failed to save provider configuration"))?;
        eprintln!("Provider connected; no authentication is required.");
        return Ok(());
    }
    if dao
        .get_provider(&provider)
        .map_err(|_| anyhow::anyhow!("failed to read credential store"))?
        .as_ref()
        .is_some_and(usable_credentials)
        || config
            .merged_config
            .custom_providers
            .get(&provider)
            .and_then(|custom| custom.resolved_api_key())
            .is_some()
    {
        eprintln!("Provider has saved credentials. They may need refreshing or replacement.");
        match prompt("Keep existing credentials? [y/N]: ")?
            .to_ascii_lowercase()
            .as_str()
        {
            "y" | "yes" => return Ok(()),
            "" | "n" | "no" => {}
            _ => bail!("invalid choice; provider login cancelled"),
        }
    }
    let method = if matches!(provider.as_str(), "openai" | "xai") {
        eprintln!("1: browser OAuth   2: headless/device OAuth   3: API key");
        prompt("Login method (empty to cancel): ")?
    } else if provider == "google" {
        eprintln!("1: Google Antigravity browser OAuth   3: API key");
        prompt("Login method (empty to cancel): ")?
    } else {
        "3".into()
    };
    let credentials = match (provider.as_str(), method.as_str()) {
        ("google", "1") => {
            let authorization = crate::auth::antigravity_oauth::BrowserAuthorization::new()?;
            eprintln!("This unofficial third-party integration uses Antigravity. Google account eligibility, service terms, and account restrictions apply.\nOpen this URL in a browser:\n{}", authorization.authorization_url());
            let _ = open_url(authorization.authorization_url());
            eprintln!("The localhost page may be unavailable; copy its complete URL from the address bar anyway.");
            let callback =
                read_hidden("Complete callback URL (hidden; Esc to cancel): ", 16 * 1024)?;
            oauth_config(
                authorization
                    .complete(&callback)
                    .await
                    .map_err(|_| anyhow::anyhow!("OAuth login failed; retry provider login"))?,
            )
        }
        ("openai", "1") => oauth_config(
            crate::auth::openai_oauth::authorize_browser()
                .await
                .map_err(|_| anyhow::anyhow!("OAuth login failed; retry provider login"))?,
        ),
        ("xai", "1") => oauth_config(
            crate::auth::xai_oauth::authorize_browser()
                .await
                .map_err(|_| anyhow::anyhow!("OAuth login failed; retry provider login"))?,
        ),
        ("openai", "2") => oauth_config(
            crate::auth::openai_oauth::authorize_headless(show_device_code)
                .await
                .map_err(|_| anyhow::anyhow!("OAuth login failed; retry provider login"))?,
        ),
        ("xai", "2") => oauth_config(
            crate::auth::xai_oauth::authorize_headless(show_device_code)
                .await
                .map_err(|_| anyhow::anyhow!("OAuth login failed; retry provider login"))?,
        ),
        (_, "3") => AuthConfig::Api {
            key: read_api_key()?,
        },
        _ => bail!("provider login cancelled or invalid login method"),
    };
    dao.set_provider(provider, credentials)
        .map_err(|_| anyhow::anyhow!("failed to save provider credentials"))?;
    eprintln!("Provider connected. Reconnect your ACP client.");
    Ok(())
}

fn needs_no_auth(provider: &str) -> bool {
    crate::model::extensions::ModelExtensions::is_runtime_provider(provider)
        || crate::model::extensions::ModelExtensions::is_unauthenticated_free_provider(provider)
}

fn validate_provider_id(provider: &str) -> Result<()> {
    if provider.is_empty() {
        bail!("provider login cancelled");
    }
    if !provider
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("invalid provider ID; use the provider ID from your configuration or /connect");
    }
    Ok(())
}

fn usable_credentials(auth: &AuthConfig) -> bool {
    match auth {
        AuthConfig::Api { key } => !key.trim().is_empty(),
        AuthConfig::Local => true,
        AuthConfig::OAuth {
            access,
            refresh,
            expires,
            ..
        } => {
            !refresh.is_empty()
                || (!access.is_empty() && *expires > chrono::Utc::now().timestamp_millis())
        }
    }
}

fn oauth_config(credentials: crate::auth::OAuthCredentials) -> AuthConfig {
    AuthConfig::OAuth {
        refresh: credentials.refresh,
        access: credentials.access,
        expires: credentials.expires,
        account_id: credentials.account_id,
        enterprise_url: credentials.enterprise_url,
        project_id: credentials.project_id,
    }
}

fn show_device_code(code: String, url: String) {
    // Device authorization codes are user-facing, not access/refresh tokens.
    eprintln!("Open {url} and enter the device code: {code}");
}

fn prompt(label: &str) -> Result<String> {
    eprint!("{label}");
    io::stderr().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_owned())
}

struct RawMode;
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn read_api_key() -> Result<String> {
    read_hidden("API key (hidden; Esc to cancel): ", 16 * 1024)
}

fn read_hidden(label: &str, max_bytes: usize) -> Result<String> {
    eprint!("{label}");
    io::stderr().flush()?;
    enable_raw_mode().context("failed to hide credential input")?;
    let guard = RawMode;
    let mut value = String::new();
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Esc => bail!("provider login cancelled"),
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                bail!("provider login cancelled");
            }
            KeyCode::Enter => break,
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if value.len() + ch.len_utf8() > max_bytes {
                    bail!("credential input is too long");
                }
                value.push(ch);
            }
            _ => {}
        }
    }
    drop(guard);
    eprintln!();
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("credential input cannot be empty");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_providers_need_no_credentials() {
        assert!(needs_no_auth("ollama"));
        assert!(!needs_no_auth("openai"));
    }

    #[test]
    fn provider_ids_reject_cancellation_and_control_characters() {
        assert!(validate_provider_id("").is_err());
        assert!(validate_provider_id("openai\nsecret").is_err());
        assert!(validate_provider_id("custom-provider_1").is_ok());
    }

    #[test]
    fn credential_readiness_is_provider_free() {
        assert!(!usable_credentials(&AuthConfig::Api { key: "  ".into() }));
        assert!(usable_credentials(&AuthConfig::Api { key: "test".into() }));
        assert!(usable_credentials(&AuthConfig::Local));
        let auth = oauth_config(crate::auth::OAuthCredentials {
            access: String::new(),
            refresh: "test".into(),
            expires: 0,
            account_id: None,
            enterprise_url: None,
            project_id: None,
        });
        assert!(usable_credentials(&auth));
        assert!(!usable_credentials(&AuthConfig::OAuth {
            access: "expired".into(),
            refresh: String::new(),
            expires: 0,
            account_id: None,
            enterprise_url: None,
            project_id: None,
        }));
    }
}
