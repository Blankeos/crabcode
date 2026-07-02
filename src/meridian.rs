use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 3456;
pub const BASE_URL: &str = "http://127.0.0.1:3456";
const HEALTH_URL: &str = "http://127.0.0.1:3456/health";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const PROBE_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthHealth>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthHealth {
    #[serde(default, rename = "loggedIn")]
    pub logged_in: bool,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default, rename = "subscriptionType")]
    pub subscription_type: Option<String>,
}

pub async fn ensure_running() -> Result<Health> {
    match probe_health().await {
        Ok(health) => return validate_health(health),
        Err(_) => start_meridian().await?,
    }

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut last_error = None;
    while Instant::now() < deadline {
        match probe_health().await {
            Ok(health) => return validate_health(health),
            Err(err) => last_error = Some(err),
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }

    Err(anyhow!(
        "Meridian did not become healthy on {BASE_URL}.{}",
        last_error
            .map(|err| format!(" Last error: {err}"))
            .unwrap_or_default()
    ))
}

async fn probe_health() -> Result<Health> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?
        .get(HEALTH_URL)
        .send()
        .await
        .context("failed to reach Meridian health endpoint")?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Meridian health endpoint returned {status}"));
    }

    response
        .json::<Health>()
        .await
        .context("failed to parse Meridian health response")
}

fn validate_health(health: Health) -> Result<Health> {
    if !health.status.eq_ignore_ascii_case("healthy") {
        return Err(anyhow!(
            "Meridian is reachable but not healthy (status: {})",
            empty_label(&health.status)
        ));
    }

    let mode = health.mode.as_deref().unwrap_or_default();
    if !mode.eq_ignore_ascii_case("passthrough") {
        return Err(anyhow!(
            "Meridian is running in {} mode. Restart it with MERIDIAN_PASSTHROUGH=1 so Crabcode keeps control of tools.",
            empty_label(mode)
        ));
    }

    if health.auth.as_ref().is_some_and(|auth| !auth.logged_in) {
        return Err(anyhow!(
            "Meridian is running, but Claude Code is not authenticated. Run `claude login`, then try again."
        ));
    }

    Ok(health)
}

async fn start_meridian() -> Result<()> {
    if !binary_in_path("meridian") {
        return Err(anyhow!(
            "Meridian CLI was not found. Install it with your preferred package manager, for example `npm install -g @rynfar/meridian`, then try again."
        ));
    }

    let mut command = Command::new("meridian");

    command
        .env("MERIDIAN_PASSTHROUGH", "1")
        .env("MERIDIAN_HOST", HOST)
        .env("MERIDIAN_PORT", PORT.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = command.spawn().context(
        "failed to start Meridian. Make sure the `meridian` CLI is installed and available on PATH",
    )?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    Ok(())
}

fn binary_in_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var).any(|dir| executable_exists(&dir, name))
}

#[cfg(unix)]
fn executable_exists(dir: &Path, name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(windows)]
fn executable_exists(dir: &Path, name: &str) -> bool {
    ["", ".exe", ".cmd", ".bat", ".ps1"]
        .iter()
        .any(|suffix| dir.join(format!("{name}{suffix}")).is_file())
}

fn empty_label(value: &str) -> &str {
    if value.trim().is_empty() {
        "unknown"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_health_requires_passthrough() {
        let err = validate_health(Health {
            status: "healthy".to_string(),
            version: None,
            mode: Some("internal".to_string()),
            auth: Some(AuthHealth {
                logged_in: true,
                email: None,
                subscription_type: None,
            }),
        })
        .unwrap_err()
        .to_string();

        assert!(err.contains("MERIDIAN_PASSTHROUGH=1"));
    }

    #[test]
    fn validate_health_accepts_passthrough() {
        validate_health(Health {
            status: "healthy".to_string(),
            version: None,
            mode: Some("passthrough".to_string()),
            auth: Some(AuthHealth {
                logged_in: true,
                email: None,
                subscription_type: None,
            }),
        })
        .unwrap();
    }
}
