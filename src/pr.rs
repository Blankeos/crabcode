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

fn checkout_command(number: u64, local_branch: &str) -> Command {
    let mut command = Command::new("gh");
    // Keep gh's default safeguards for local commits and uncommitted edits.
    command.args([
        "pr",
        "checkout",
        &number.to_string(),
        "--branch",
        local_branch,
    ]);
    command
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

    let checkout = checkout_command(number, &local_branch)
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

    // gh may have fetched only the base repository's PR ref. Adding a remote
    // does not create the remote-tracking ref required by --set-upstream-to.
    let refspec = format!("+refs/heads/{head_ref_name}:refs/remotes/{remote_name}/{head_ref_name}");
    run_git(cwd, ["fetch", "--no-tags", remote_name, &refspec])?;

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

    struct TestRepo(std::path::PathBuf);

    impl TestRepo {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "crabcode-pr-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            repo.git(&[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(&self.0)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn checkout_keeps_default_local_work_safeguards() {
        let command = checkout_command(50, "pr/50");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(args, ["pr", "checkout", "50", "--branch", "pr/50"]);
    }

    fn fork_info(branch: &str) -> PullRequestInfo {
        PullRequestInfo {
            head_repository: Some(Repository {
                name: "example".into(),
            }),
            head_repository_owner: Some(RepositoryOwner {
                login: "contributor".into(),
            }),
            is_cross_repository: true,
            head_ref_name: Some(branch.into()),
        }
    }

    #[test]
    fn fetches_missing_fork_ref_before_setting_upstream() {
        let fork = TestRepo::new();
        fork.git(&["branch", "feat/example"]);
        let repo = TestRepo::new();
        repo.git(&["branch", "pr/50"]);
        repo.git(&["remote", "add", "contributor", fork.0.to_str().unwrap()]);
        assert!(repo
            .git(&["for-each-ref", "refs/remotes/contributor"])
            .is_empty());

        configure_fork_remote(&fork_info("feat/example"), "pr/50", &repo.0).unwrap();

        assert_eq!(
            repo.git(&["rev-parse", "pr/50@{upstream}"]),
            fork.git(&["rev-parse", "feat/example"])
        );
        assert_eq!(repo.git(&["config", "branch.pr/50.remote"]), "contributor");
        assert_eq!(
            repo.git(&["config", "branch.pr/50.merge"]),
            "refs/heads/feat/example"
        );
    }

    #[test]
    fn failed_fork_fetch_does_not_change_upstream() {
        let fork = TestRepo::new();
        let repo = TestRepo::new();
        repo.git(&["branch", "pr/50"]);
        repo.git(&["remote", "add", "contributor", fork.0.to_str().unwrap()]);
        repo.git(&["config", "branch.pr/50.remote", "original"]);
        repo.git(&["config", "branch.pr/50.merge", "refs/heads/original"]);

        assert!(configure_fork_remote(&fork_info("missing"), "pr/50", &repo.0).is_err());
        assert_eq!(repo.git(&["config", "branch.pr/50.remote"]), "original");
        assert_eq!(
            repo.git(&["config", "branch.pr/50.merge"]),
            "refs/heads/original"
        );
    }

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
