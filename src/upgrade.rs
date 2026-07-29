use anyhow::{bail, Context, Result};
use semver::Version;
use serde::Deserialize;
use std::process::Command;

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/Blankeos/crabcode/releases/latest";
const INSTALLER_URL: &str = "https://github.com/Blankeos/crabcode/releases/download";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

#[derive(Debug, PartialEq, Eq)]
enum VersionCheck {
    UpToDate { current: Version, latest: Version },
    UpdateAvailable { current: Version, latest: Version },
}

fn check_version(current: &str, release_tag: &str) -> Result<VersionCheck> {
    let current = Version::parse(current).context("invalid current version")?;
    let latest = Version::parse(release_tag.trim_start_matches('v'))
        .with_context(|| format!("invalid release tag {release_tag:?}"))?;

    if latest > current {
        Ok(VersionCheck::UpdateAvailable { current, latest })
    } else {
        Ok(VersionCheck::UpToDate { current, latest })
    }
}

fn target_release_tag(target: &str) -> Result<String> {
    let version = Version::parse(target.trim_start_matches('v'))
        .with_context(|| format!("invalid target version {target:?}; expected a semver version"))?;

    Ok(format!("v{version}"))
}

fn installer_command(release_tag: &str) -> String {
    format!(
        "curl --proto '=https' --tlsv1.2 -LsSf {INSTALLER_URL}/{release_tag}/crabcode-installer.sh | sh"
    )
}

fn run_upgrade<F>(check: VersionCheck, install: F) -> Result<VersionCheck>
where
    F: FnOnce() -> Result<()>,
{
    if matches!(check, VersionCheck::UpdateAvailable { .. }) {
        install()?;
    }

    Ok(check)
}

async fn latest_release_tag() -> Result<String> {
    let release = reqwest::Client::new()
        .get(LATEST_RELEASE_URL)
        .header(reqwest::header::USER_AGENT, "crabcode-upgrade")
        .send()
        .await
        .context("failed to check for the latest crabcode release")?
        .error_for_status()
        .context("failed to check for the latest crabcode release")?
        .json::<LatestRelease>()
        .await
        .context("failed to read the latest crabcode release")?;

    Ok(release.tag_name)
}

#[cfg(not(target_os = "windows"))]
fn run_installer(release_tag: &str) -> Result<()> {
    let status = Command::new("sh")
        .args(["-c", &installer_command(release_tag)])
        .status()
        .context("failed to start the crabcode installer")?;

    if !status.success() {
        bail!("crabcode installer exited with {status}");
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn run_installer(_release_tag: &str) -> Result<()> {
    bail!("automatic upgrades are not supported on Windows; reinstall with npm, cargo, or the latest GitHub release")
}

pub async fn upgrade(target: Option<&str>) -> Result<()> {
    let release_tag = match target {
        Some(target) if target != "latest" => target_release_tag(target)?,
        _ => latest_release_tag().await?,
    };
    let check = check_version(env!("CARGO_PKG_VERSION"), &release_tag)?;
    let check = run_upgrade(check, || run_installer(&release_tag))?;

    match check {
        VersionCheck::UpToDate { current, .. } => {
            println!("crabcode v{current} is already up to date.")
        }
        VersionCheck::UpdateAvailable { current, latest } => {
            println!("Upgraded crabcode from v{current} to v{latest}.")
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_newer_release_tag() {
        assert_eq!(
            check_version("0.0.9", "v0.0.10").unwrap(),
            VersionCheck::UpdateAvailable {
                current: Version::parse("0.0.9").unwrap(),
                latest: Version::parse("0.0.10").unwrap(),
            }
        );
    }

    #[test]
    fn detects_an_up_to_date_release_tag() {
        assert_eq!(
            check_version("0.1.0", "v0.1.0").unwrap(),
            VersionCheck::UpToDate {
                current: Version::parse("0.1.0").unwrap(),
                latest: Version::parse("0.1.0").unwrap(),
            }
        );
    }

    #[test]
    fn normalizes_an_explicit_target_version() {
        assert_eq!(target_release_tag("0.1.0").unwrap(), "v0.1.0");
        assert_eq!(target_release_tag("v0.1.0").unwrap(), "v0.1.0");
    }

    #[test]
    fn rejects_an_invalid_explicit_target_version() {
        let error = target_release_tag("nightly").unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid target version \"nightly\"; expected a semver version"
        );
    }

    #[test]
    fn builds_an_installer_command_for_the_requested_release() {
        assert_eq!(
            installer_command("v0.1.0"),
            "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Blankeos/crabcode/releases/download/v0.1.0/crabcode-installer.sh | sh"
        );
    }

    #[test]
    fn runs_the_installer_only_when_an_upgrade_is_available() {
        let check = check_version("0.0.9", "v0.1.0").unwrap();
        let mut installed = false;

        let result = run_upgrade(check, || {
            installed = true;
            Ok(())
        })
        .unwrap();

        assert!(installed);
        assert!(matches!(result, VersionCheck::UpdateAvailable { .. }));
    }

    #[test]
    fn reports_installer_failures() {
        let check = check_version("0.0.9", "v0.1.0").unwrap();

        let error = run_upgrade(check, || bail!("installer failed")).unwrap_err();

        assert_eq!(error.to_string(), "installer failed");
    }
}
