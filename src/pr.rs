use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

const PR_VIEW_FIELDS: &str = "headRepository,headRepositoryOwner,isCrossRepository,headRefName,url";

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
    #[serde(default)]
    url: Option<String>,
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

fn normalize_host(host: &str) -> String {
    host.trim().to_lowercase()
}

// (host, owner, repo) lowercased; host keeps explicit port and www as distinct.
fn parse_remote_identity(remote_url: &str) -> Option<(String, String, String)> {
    let remote_url = remote_url.trim();
    if remote_url.is_empty() {
        return None;
    }
    let (raw_host, raw_path, port) = if remote_url.contains("://") {
        let parsed = url::Url::parse(remote_url).ok()?;
        if parsed.scheme() == "file" {
            return None;
        }
        (
            parsed.host_str()?.to_owned(),
            parsed.path().to_owned(),
            parsed.port(),
        )
    } else {
        let at = remote_url.rfind('@')?;
        let colon = remote_url[at..].find(':')?;
        (
            remote_url[at + 1..at + colon].to_owned(),
            remote_url[at + colon + 1..].to_owned(),
            None,
        )
    };
    let mut host = normalize_host(&raw_host);
    if host.is_empty() {
        return None;
    }
    if let Some(port) = port {
        host = format!("{host}:{port}");
    }
    let path = raw_path.trim().trim_matches('/');
    let mut parts = path.split('/');
    let owner = parts.next()?.trim().to_lowercase();
    let mut repo = parts.next()?.trim().to_lowercase();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    if let Some(stripped) = repo.strip_suffix(".git") {
        repo = stripped.to_owned();
    }
    if repo.is_empty() {
        return None;
    }
    Some((host, owner, repo))
}

// Reuse smallest remote matching the fork; else owner login or -fork fallback.
fn select_fork_remote_name(
    existing: &[(String, String)],
    fork_host: &str,
    fork_owner_lower: &str,
    fork_repo_lower: &str,
    owner_login: &str,
) -> String {
    let fork_host = normalize_host(fork_host);
    let mut matches: Vec<&str> = existing
        .iter()
        .filter(|(_, url)| {
            parse_remote_identity(url).is_some_and(|(host, owner, repo)| {
                host == fork_host && owner == fork_owner_lower && repo == fork_repo_lower
            })
        })
        .map(|(name, _)| name.as_str())
        .collect();
    if !matches.is_empty() {
        matches.sort_unstable();
        return matches[0].to_owned();
    }
    let taken = |candidate: &str| existing.iter().any(|(name, _)| name == candidate);
    if !taken(owner_login) {
        return owner_login.to_owned();
    }
    for n in 1.. {
        let candidate = if n == 1 {
            format!("{owner_login}-fork")
        } else {
            format!("{owner_login}-fork-{n}")
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn list_remote_urls(cwd: &Path) -> Result<Vec<(String, String)>> {
    let output = Command::new("git")
        .args(["config", "--get-regexp", r"^remote\..*\.url$"])
        .current_dir(cwd)
        .output()
        .context("failed to list git remotes")?;
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        bail!("failed to list git remote URLs: {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (key, url) = line.split_once(' ')?;
            let name = key.strip_prefix("remote.")?.strip_suffix(".url")?;
            let url = url.trim();
            if name.is_empty() || url.is_empty() {
                return None;
            }
            Some((name.to_owned(), url.to_owned()))
        })
        .collect())
}

// Derive (host[:port], https://host[:port]/owner/repo) from PR url. No guessing.
fn pr_base(pr_url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(pr_url.trim()).ok()?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return None;
    }
    let mut host = normalize_host(parsed.host_str()?);
    if host.is_empty() {
        return None;
    }
    if let Some(port) = parsed.port() {
        host = format!("{host}:{port}");
    }
    let parts: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 || parts[2] != "pull" {
        return None;
    }
    let (owner, repo) = (parts[0].trim(), parts[1].trim());
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let url = format!("{}://{host}/{owner}/{repo}", parsed.scheme());
    Some((host, url))
}

// Scoped git-config write preserving the already-resolved base for later gh calls.
fn pin_base_repo(cwd: &Path, base_url: &str) -> Result<()> {
    let status = Command::new("gh")
        .args(["repo", "set-default", base_url])
        .current_dir(cwd)
        .status()
        .context("failed to pin base repository")?;
    if !status.success() {
        bail!("gh repo set-default exited with {status}");
    }
    Ok(())
}

fn configure_fork_remote(info: &PullRequestInfo, local_branch: &str, cwd: &Path) -> Result<()> {
    configure_fork_remote_with_pin(info, local_branch, cwd, pin_base_repo)
}

fn configure_fork_remote_with_pin(
    info: &PullRequestInfo,
    local_branch: &str,
    cwd: &Path,
    pin: impl Fn(&Path, &str) -> Result<()>,
) -> Result<()> {
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

    let existing = list_remote_urls(cwd)?;
    let pr = info.url.as_deref().and_then(pr_base);
    let fork_host = pr
        .as_ref()
        .map(|(h, _)| h.clone())
        .unwrap_or_else(|| "github.com".to_owned());
    let remote_name = select_fork_remote_name(
        &existing,
        &fork_host,
        &owner.login.to_lowercase(),
        &repository.name.to_lowercase(),
        &owner.login,
    );

    if !existing.iter().any(|(name, _)| name == &remote_name) {
        let (_, base_url) = pr.as_ref().context("missing or invalid pull request URL")?;
        pin(cwd, base_url)?;
        let remote_url = format!(
            "https://{}/{}/{}.git",
            fork_host, owner.login, repository.name
        );
        run_git(
            cwd,
            ["remote", "add", remote_name.as_str(), remote_url.as_str()],
        )?;
        println!("Added fork remote: {remote_name}");
    }

    // gh may have fetched only the base repository's PR ref. Adding a remote
    // does not create the remote-tracking ref required by --set-upstream-to.
    let refspec = format!("+refs/heads/{head_ref_name}:refs/remotes/{remote_name}/{head_ref_name}");
    run_git(
        cwd,
        ["fetch", "--no-tags", remote_name.as_str(), refspec.as_str()],
    )?;

    let upstream = format!("--set-upstream-to={remote_name}/{head_ref_name}");
    run_git(cwd, ["branch", upstream.as_str(), local_branch])
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
    use std::cell::RefCell;

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

        fn add_github_remote(&self, name: &str, github_url: &str, local_path: &str) {
            self.git(&["remote", "add", name, github_url]);
            self.git(&["config", &format!("url.{local_path}.insteadOf"), github_url]);
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn no_pin(_: &Path, _: &str) -> Result<()> {
        panic!("pin must not run when fork remote is reused");
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
            url: Some("https://github.com/acme/example/pull/50".into()),
        }
    }

    #[test]
    fn fetches_missing_fork_ref_before_setting_upstream() {
        let fork = TestRepo::new();
        fork.git(&["branch", "feat/example"]);
        let repo = TestRepo::new();
        repo.git(&["branch", "pr/50"]);
        repo.add_github_remote(
            "contributor",
            "https://github.com/contributor/example.git",
            fork.0.to_str().unwrap(),
        );
        assert!(repo
            .git(&["for-each-ref", "refs/remotes/contributor"])
            .is_empty());

        configure_fork_remote_with_pin(&fork_info("feat/example"), "pr/50", &repo.0, no_pin)
            .unwrap();

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
        repo.add_github_remote(
            "contributor",
            "https://github.com/contributor/example.git",
            fork.0.to_str().unwrap(),
        );
        repo.git(&["config", "branch.pr/50.remote", "original"]);
        repo.git(&["config", "branch.pr/50.merge", "refs/heads/original"]);

        assert!(
            configure_fork_remote_with_pin(&fork_info("missing"), "pr/50", &repo.0, no_pin)
                .is_err()
        );
        assert_eq!(repo.git(&["config", "branch.pr/50.remote"]), "original");
        assert_eq!(
            repo.git(&["config", "branch.pr/50.merge"]),
            "refs/heads/original"
        );
    }

    #[test]
    fn reuses_alias_by_identity_without_pinning() {
        let fork = TestRepo::new();
        fork.git(&["branch", "feat/example"]);
        let repo = TestRepo::new();
        repo.git(&["branch", "pr/50"]);
        repo.add_github_remote(
            "myfork",
            "git@github.com:contributor/example.git",
            fork.0.to_str().unwrap(),
        );

        configure_fork_remote_with_pin(&fork_info("feat/example"), "pr/50", &repo.0, no_pin)
            .unwrap();

        assert_eq!(repo.git(&["config", "branch.pr/50.remote"]), "myfork");
        assert!(repo.git(&["remote"]).lines().all(|r| r != "contributor"));
    }

    #[test]
    fn pins_base_before_adding_fork_remote() {
        let fork = TestRepo::new();
        fork.git(&["branch", "feat/example"]);
        let repo = TestRepo::new();
        repo.git(&["branch", "pr/50"]);
        repo.git(&[
            "remote",
            "add",
            "contributor",
            "https://github.com/contributor/other.git",
        ]);
        repo.git(&[
            "config",
            &format!("url.{}.insteadOf", fork.0.to_str().unwrap()),
            "https://github.com/contributor/example.git",
        ]);
        let pinned = RefCell::new(Vec::new());
        configure_fork_remote_with_pin(&fork_info("feat/example"), "pr/50", &repo.0, |_, base| {
            assert!(!repo
                .git(&["remote"])
                .lines()
                .any(|name| name == "contributor-fork"));
            pinned.borrow_mut().push(base.to_owned());
            Ok(())
        })
        .unwrap();

        assert_eq!(
            pinned.borrow().as_slice(),
            ["https://github.com/acme/example"]
        );
        assert_eq!(
            repo.git(&["config", "--get", "remote.contributor.url"]),
            "https://github.com/contributor/other.git"
        );
        assert_eq!(
            repo.git(&["config", "--get", "remote.contributor-fork.url"]),
            "https://github.com/contributor/example.git"
        );
        assert_eq!(
            repo.git(&["config", "branch.pr/50.remote"]),
            "contributor-fork"
        );
        assert_eq!(
            repo.git(&["rev-parse", "pr/50@{upstream}"]),
            fork.git(&["rev-parse", "feat/example"])
        );
    }

    #[test]
    fn failed_pin_does_not_add_remote() {
        let repo = TestRepo::new();
        repo.git(&["branch", "pr/50"]);
        let err =
            configure_fork_remote_with_pin(&fork_info("feat/example"), "pr/50", &repo.0, |_, _| {
                bail!("pin failed")
            })
            .unwrap_err();
        assert!(err.to_string().contains("pin failed"));
        assert!(!repo.git(&["remote"]).lines().any(|r| r == "contributor"));
    }

    #[test]
    fn derives_base_url_from_pr_url() {
        let base_repo_url_from_pr_url = |value: &str| pr_base(value).map(|(_, url)| url);
        assert_eq!(
            base_repo_url_from_pr_url("https://github.com/acme/example/pull/50").as_deref(),
            Some("https://github.com/acme/example")
        );
        assert_eq!(
            base_repo_url_from_pr_url("https://ghe.example.com:8443/acme/example/pull/50/files")
                .as_deref(),
            Some("https://ghe.example.com:8443/acme/example")
        );
        // www is a distinct host, not stripped.
        assert_eq!(
            base_repo_url_from_pr_url("https://www.github.com/acme/example/pull/50").as_deref(),
            Some("https://www.github.com/acme/example")
        );
        assert!(base_repo_url_from_pr_url("https://github.com/acme/example").is_none());
        assert!(base_repo_url_from_pr_url("https://github.com/acme").is_none());
        assert!(base_repo_url_from_pr_url("not a url").is_none());
        assert!(base_repo_url_from_pr_url("ssh://github.com/acme/example/pull/50").is_none());
    }

    #[test]
    fn parses_remote_identity_conservatively() {
        let https = parse_remote_identity("https://github.com/contributor/example.git").unwrap();
        assert_eq!(
            parse_remote_identity("https://github.com/contributor/example").unwrap(),
            https
        );
        assert_eq!(
            parse_remote_identity("git@github.com:contributor/example.git").unwrap(),
            https
        );
        assert_eq!(
            parse_remote_identity("ssh://git@github.com/contributor/example.git").unwrap(),
            https
        );
        // Case-insensitive owner/repo, case-insensitive host.
        assert_eq!(
            parse_remote_identity("https://GitHub.com/Contributor/Example.GIT").unwrap(),
            https
        );
        // www and ports are distinct hosts.
        assert_ne!(
            parse_remote_identity("https://www.github.com/contributor/example.git").unwrap(),
            https
        );
        assert_ne!(
            parse_remote_identity("https://ghe.example.com/contributor/example.git").unwrap(),
            https
        );
        assert_ne!(
            parse_remote_identity("ssh://git@ghe.example.com:22/contributor/example.git").unwrap(),
            parse_remote_identity("https://ghe.example.com/contributor/example.git").unwrap()
        );
        assert!(parse_remote_identity("/tmp/local").is_none());
        assert!(parse_remote_identity("file:///tmp/local").is_none());
        assert!(parse_remote_identity("https://github.com/only-owner").is_none());
        assert!(parse_remote_identity("https://github.com/owner/repo/extra").is_none());
    }

    #[test]
    fn selects_fork_name_on_collision() {
        let occupied = vec![(
            "contributor".to_owned(),
            "https://github.com/contributor/other.git".to_owned(),
        )];
        assert_eq!(
            select_fork_remote_name(
                &occupied,
                "github.com",
                "contributor",
                "example",
                "contributor"
            ),
            "contributor-fork"
        );
    }

    #[test]
    fn parses_cross_repository_pull_request_info() {
        let info: PullRequestInfo = serde_json::from_str(
            r#"{
                "headRepository": { "name": "crabcode" },
                "headRepositoryOwner": { "login": "contributor" },
                "isCrossRepository": true,
                "headRefName": "feat/pr-command",
                "url": "https://github.com/acme/crabcode/pull/50"
            }"#,
        )
        .unwrap();

        assert!(info.is_cross_repository);
        assert_eq!(info.head_repository.unwrap().name, "crabcode");
        assert_eq!(info.head_repository_owner.unwrap().login, "contributor");
        assert_eq!(info.head_ref_name.as_deref(), Some("feat/pr-command"));
        assert_eq!(
            info.url.as_deref(),
            Some("https://github.com/acme/crabcode/pull/50")
        );
    }
}
