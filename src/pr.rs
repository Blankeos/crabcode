use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

const PR_VIEW_FIELDS: &str = "headRepository,headRepositoryOwner,isCrossRepository,headRefName";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestInfo {
    #[serde(default)]
    head_repository: Option<Repository>,
    #[serde(default)]
    head_repository_owner: Option<RepositoryOwner>,
    #[serde(default)]
    is_cross_repository: bool,
    #[serde(default)]
    head_ref_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryOwner {
    login: String,
}

pub fn run(number: u64) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    ensure_git_repository(&cwd)?;

    let local_branch = format!("pr/{number}");
    println!("Fetching and checking out PR #{number}...");

    let checkout = Command::new("gh")
        .args([
            "pr",
            "checkout",
            &number.to_string(),
            "--branch",
            &local_branch,
            "--force",
        ])
        .current_dir(&cwd)
        .status();

    if !checkout.is_ok_and(|status| status.success()) {
        bail!(
            "Failed to checkout PR #{number}. Make sure you have gh CLI installed and authenticated."
        );
    }

    if let Some(info) = pull_request_info(number, &cwd)? {
        configure_fork_remote(&info, &local_branch, &cwd)?;
    }

    println!("Successfully checked out PR #{number} as branch '{local_branch}'");
    println!();
    println!("Starting crabcode...");
    println!();

    let executable = std::env::current_exe().context("failed to locate crabcode executable")?;
    let status = Command::new(executable)
        .current_dir(&cwd)
        .status()
        .context("failed to start crabcode")?;
    if !status.success() {
        bail!("crabcode exited with {status}");
    }

    Ok(())
}

fn ensure_git_repository(cwd: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .output();

    if output.is_ok_and(|output| {
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true"
    }) {
        return Ok(());
    }

    bail!("Could not find git repository. Please run this command from a git repository.")
}

fn pull_request_info(number: u64, cwd: &Path) -> Result<Option<PullRequestInfo>> {
    let output = match Command::new("gh")
        .args(["pr", "view", &number.to_string(), "--json", PR_VIEW_FIELDS])
        .current_dir(cwd)
        .output()
    {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => output,
        _ => return Ok(None),
    };

    serde_json::from_slice(&output.stdout)
        .context("failed to parse GitHub pull request information")
        .map(Some)
}

fn configure_fork_remote(info: &PullRequestInfo, local_branch: &str, cwd: &Path) -> Result<()> {
    if !info.is_cross_repository {
        return Ok(());
    }

    let (Some(repository), Some(owner), Some(head_ref_name)) = (
        info.head_repository.as_ref(),
        info.head_repository_owner.as_ref(),
        info.head_ref_name.as_deref(),
    ) else {
        return Ok(());
    };

    let remotes = Command::new("git")
        .arg("remote")
        .current_dir(cwd)
        .output()
        .context("failed to list git remotes")?;
    if !remotes.status.success() {
        bail!("git remote exited with {}", remotes.status);
    }

    let remote_name = &owner.login;
    if !remote_exists(&remotes.stdout, remote_name) {
        let remote_url = format!("https://github.com/{}/{}.git", owner.login, repository.name);
        run_git(cwd, ["remote", "add", remote_name, &remote_url])?;
        println!("Added fork remote: {remote_name}");
    }

    let upstream = format!("--set-upstream-to={remote_name}/{head_ref_name}");
    run_git(cwd, ["branch", &upstream, local_branch])
}

fn remote_exists(stdout: &[u8], remote_name: &str) -> bool {
    String::from_utf8_lossy(stdout)
        .lines()
        .any(|remote| remote == remote_name)
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .context("failed to run git")?;
    if !status.success() {
        bail!("git exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cross_repository_pull_request_info() {
        let info: PullRequestInfo = serde_json::from_str(
            r#"{
                "headRepository": { "name": "crabcode" },
                "headRepositoryOwner": { "login": "contributor" },
                "isCrossRepository": true,
                "headRefName": "feat/pr-command"
            }"#,
        )
        .unwrap();

        assert!(info.is_cross_repository);
        assert_eq!(info.head_repository.unwrap().name, "crabcode");
        assert_eq!(info.head_repository_owner.unwrap().login, "contributor");
        assert_eq!(info.head_ref_name.as_deref(), Some("feat/pr-command"));
    }

    #[test]
    fn detects_only_exact_remote_names() {
        let remotes = b"origin\ncontributor-tools\ncontributor\n";
        assert!(remote_exists(remotes, "contributor"));
        assert!(!remote_exists(remotes, "contribute"));
    }
}
