//! Self-upgrade: detect install method and reinstall via the matching tool.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub(crate) const GITHUB_REPO: &str = "Blankeos/crabcode";
const BREW_FORMULA: &str = "blankeos/tap/crabcode";
const NPM_PACKAGE: &str = "crabcode";
const BINARY_NAME: &str = "crabcode";

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsPackageManager {
    Npm,
    Bun,
    Pnpm,
    Yarn,
}

impl JsPackageManager {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Bun => "bun",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
        }
    }

    fn install_global_cmd(&self, package_spec: &str) -> (String, Vec<String>) {
        match self {
            Self::Npm => (
                "npm".into(),
                vec!["install".into(), "-g".into(), package_spec.into()],
            ),
            Self::Bun => (
                "bun".into(),
                vec!["install".into(), "-g".into(), package_spec.into()],
            ),
            Self::Pnpm => (
                "pnpm".into(),
                vec!["add".into(), "-g".into(), package_spec.into()],
            ),
            Self::Yarn => (
                "yarn".into(),
                vec!["global".into(), "add".into(), package_spec.into()],
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallMethod {
    Homebrew,
    Js { manager: JsPackageManager },
    Cargo { use_binstall: bool },
    InstallScript,
    Unknown { path: PathBuf },
}

impl InstallMethod {
    fn label(&self) -> String {
        match self {
            Self::Homebrew => "brew".into(),
            Self::Js { manager } => manager.as_str().into(),
            Self::Cargo { use_binstall: true } => "cargo-binstall".into(),
            Self::Cargo {
                use_binstall: false,
            } => "cargo".into(),
            Self::InstallScript => "install.sh".into(),
            Self::Unknown { .. } => "unknown".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
}

struct VersionCheck {
    current: String,
    target: String,
    needs_upgrade: bool,
}

/// Current binary version (`CARGO_PKG_VERSION`).
pub(crate) fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Upgrade without touching the caller's terminal: stdin is `/dev/null` (so a
/// missing tool fails fast instead of prompting invisibly under the TUI),
/// stdout/stderr are captured (so package-manager output never redraws over
/// the active terminal), and helpers get no-input env. Returns the target
/// version. Errors carry the failing command plus the tail of its output.
pub(crate) fn upgrade_noninteractive(target: Option<&str>) -> Result<String> {
    let current = current_version();
    let method = detect_install_method()?;
    let check = resolve_target_version(&current, target)?;
    if !check.needs_upgrade {
        return Ok(check.target);
    }
    run_method_upgrade_captured(&method, &check.target)?;
    Ok(check.target)
}

/// Upgrade crabcode to the latest release, or to a specific target version.
pub fn upgrade(target: Option<&str>) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let method = detect_install_method()?;
    let exe = resolve_install_path()?;

    println!("→ Current version: v{current}");
    println!("→ Binary path:     {}", exe.display());
    println!("→ Detected `{}`", method.label());

    let check = resolve_target_version(&current, target)?;
    if !check.needs_upgrade {
        println!("✓ Already on v{} — nothing to do.", check.current);
        return Ok(());
    }

    println!(
        "→ Upgrading: v{} → {}",
        check.current,
        display_version(&check.target)
    );

    run_method_upgrade(&method, &check.target)?;

    println!(
        "✓ Upgrade complete. Restart crabcode to use {}.",
        display_version(&check.target)
    );
    Ok(())
}

fn resolve_target_version(current: &str, target: Option<&str>) -> Result<VersionCheck> {
    let requested = match target {
        Some(t) if !t.eq_ignore_ascii_case("latest") => normalize_version(t),
        _ => {
            let latest =
                fetch_latest_tag().context("failed to fetch latest release from GitHub")?;
            normalize_version(&latest)
        }
    };

    let current_norm = normalize_version(current);
    let needs_upgrade = current_norm != requested;

    Ok(VersionCheck {
        current: current_norm,
        target: requested,
        needs_upgrade,
    })
}

fn normalize_version(version: &str) -> String {
    version.trim().trim_start_matches('v').to_string()
}

fn display_version(version: &str) -> String {
    format!("v{}", normalize_version(version))
}

/// Bounded GitHub lookup for the upgrade target (mirrors the 10s update
/// check). Null stdin + no-prompt env so a background upgrade thread can
/// never hang forever waiting on a prompt; `--max-time` bounds the network.
fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: crabcode-upgrade",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("failed to run curl (is it installed?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("GitHub release lookup failed: {stderr}");
    }

    let release: GithubRelease = serde_json::from_slice(&output.stdout)
        .context("failed to parse GitHub releases response")?;
    Ok(release.tag_name)
}

fn resolve_install_path() -> Result<PathBuf> {
    let exe = env::current_exe().context("failed to resolve current executable")?;
    let canonical = exe.canonicalize().unwrap_or(exe);

    // When running a cargo-built binary from the workspace, prefer the PATH install
    // so upgrades hit the real install rather than target/debug.
    if is_dev_build(&canonical) {
        if let Some(from_path) = find_binary_on_path(BINARY_NAME) {
            if from_path.canonicalize().ok().as_ref() != Some(&canonical) {
                return Ok(from_path.canonicalize().unwrap_or(from_path));
            }
        }
    }

    Ok(canonical)
}

fn is_dev_build(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/target/debug/")
        || s.contains("/target/release/")
        || s.contains("\\target\\debug\\")
        || s.contains("\\target\\release\\")
}

fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate_exe = dir.join(format!("{name}.exe"));
            if candidate_exe.is_file() {
                return Some(candidate_exe);
            }
        }
    }
    None
}

fn detect_install_method() -> Result<InstallMethod> {
    let path = resolve_install_path()?;
    Ok(detect_install_method_from_path(&path))
}

fn detect_install_method_from_path(path: &Path) -> InstallMethod {
    // Gather expensive ownership signals lazily: only the cargo-bin branch
    // needs receipt/cargo claims, and only ambiguous paths need brew/cargo/js
    // probes. Pure helpers below keep this unit-testable without spawning.
    detect_install_method_from_path_impl(
        path,
        &cargo_bin_dir(),
        || load_shell_install_receipt(),
        || cargo_install_list_has(BINARY_NAME),
        || brew_owns_formula(BINARY_NAME),
        || detect_js_manager_strict(),
        || js_global_has(NPM_PACKAGE),
        command_exists("cargo-binstall"),
    )
}

#[allow(clippy::too_many_arguments)]
fn detect_install_method_from_path_impl(
    path: &Path,
    cargo_bin: &Path,
    load_receipt: impl FnOnce() -> Option<InstallReceipt>,
    cargo_claims: impl FnOnce() -> bool,
    brew_claims: impl FnOnce() -> bool,
    js_owner: impl FnOnce() -> Option<JsPackageManager>,
    js_any_claims: impl FnOnce() -> bool,
    use_binstall: bool,
) -> InstallMethod {
    let path_str = path.to_string_lossy();

    // Homebrew Cellar / opt paths (symlink targets usually land under Cellar)
    if path_str.contains("/Cellar/")
        || path_str.contains("/Homebrew/")
        || path_str.contains("\\Homebrew\\")
        || path_str.contains("/linuxbrew/")
        || (path_str.contains("/opt/homebrew/") && !path_str.contains("node_modules"))
        || (path_str.contains("/home/linuxbrew/") && !path_str.contains("node_modules"))
    {
        return InstallMethod::Homebrew;
    }

    // JS package managers (npm/bun/pnpm/yarn global installs live under node_modules)
    if path_str.contains("node_modules") {
        // Path markers for bun/pnpm/yarn are authoritative; a generic
        // node_modules path (fnm/nvm/npm) must prove single-owner via the
        // managers' global lists, otherwise fail safe to Unknown instead of
        // guessing an available-but-unrelated manager.
        match detect_js_manager_from_path_strict(&path_str, js_owner) {
            Some(manager) => return InstallMethod::Js { manager },
            None => {
                return InstallMethod::Unknown {
                    path: path.to_path_buf(),
                }
            }
        }
    }

    // cargo install / cargo binstall vs cargo-dist shell installer.
    //
    // Both default to `$CARGO_HOME/bin` (or `~/.cargo/bin` when `CARGO_HOME`
    // is unset), so a path prefix alone cannot tell them apart. Compare the
    // shell install receipt against cargo's install metadata:
    // - receipt claims + cargo silent => shell installer (InstallScript)
    // - cargo claims + no receipt   => cargo install
    // - neither or both             => Unknown (fail safe, never guess)
    if path.starts_with(cargo_bin) {
        let receipt = load_receipt();
        let receipt_claims = receipt.as_ref().is_some_and(|r| r.claims_path(path));
        let cargo_owns = cargo_claims();
        return classify_cargo_bin_path(path, receipt_claims, cargo_owns, use_binstall);
    }

    // install.sh default destination
    if path_str.contains("/.local/bin/") || path_str.contains("\\.local\\bin\\") {
        return InstallMethod::InstallScript;
    }

    // Heuristics when path alone is ambiguous
    if brew_claims() {
        return InstallMethod::Homebrew;
    }
    if cargo_claims() {
        return InstallMethod::Cargo { use_binstall };
    }
    if js_any_claims() {
        // Only upgrade via the owning manager; ambiguous (multi-owner) or
        // unowned-but-listed states fail safe to Unknown.
        match js_owner() {
            Some(manager) => return InstallMethod::Js { manager },
            None => {
                return InstallMethod::Unknown {
                    path: path.to_path_buf(),
                }
            }
        }
    }

    InstallMethod::Unknown {
        path: path.to_path_buf(),
    }
}

/// Pure cargo-bin disambiguation so tests never spawn `cargo`/receipt I/O.
fn classify_cargo_bin_path(
    path: &Path,
    receipt_claims: bool,
    cargo_claims: bool,
    use_binstall: bool,
) -> InstallMethod {
    match (receipt_claims, cargo_claims) {
        (true, false) => InstallMethod::InstallScript,
        (false, true) => InstallMethod::Cargo { use_binstall },
        // Neither signal (unmanaged copy?) or both (double install) must
        // never guess: surface the reinstall help instead of running the
        // wrong updater against the same destination.
        _ => InstallMethod::Unknown {
            path: path.to_path_buf(),
        },
    }
}

/// Strict JS manager from a node_modules path: bun/pnpm/yarn markers are
/// authoritative; generic paths require exactly one global-list owner.
fn detect_js_manager_from_path_strict(
    path_str: &str,
    js_owner: impl FnOnce() -> Option<JsPackageManager>,
) -> Option<JsPackageManager> {
    if path_str.contains("/.bun/") || path_str.contains("\\.bun\\") {
        Some(JsPackageManager::Bun)
    } else if path_str.contains("pnpm") {
        Some(JsPackageManager::Pnpm)
    } else if path_str.contains("yarn") {
        Some(JsPackageManager::Yarn)
    } else {
        js_owner()
    }
}

/// Owning JS manager or `None` when zero/multi owners (fail safe).
/// Never falls back to a merely-available manager: upgrading via a
/// non-owning manager would write a different global prefix and leave the
/// real install stale.
fn detect_js_manager_strict() -> Option<JsPackageManager> {
    let mut owners = Vec::new();
    if js_global_has_with("npm", NPM_PACKAGE) {
        owners.push(JsPackageManager::Npm);
    }
    if js_global_has_with("bun", NPM_PACKAGE) {
        owners.push(JsPackageManager::Bun);
    }
    if js_global_has_with("pnpm", NPM_PACKAGE) {
        owners.push(JsPackageManager::Pnpm);
    }
    if js_global_has_with("yarn", NPM_PACKAGE) {
        owners.push(JsPackageManager::Yarn);
    }
    if owners.len() == 1 {
        owners.into_iter().next()
    } else {
        None
    }
}

fn run_method_upgrade(method: &InstallMethod, target_version: &str) -> Result<()> {
    match method {
        InstallMethod::Homebrew => upgrade_brew(target_version),
        InstallMethod::Js { manager } => upgrade_js(manager, target_version),
        InstallMethod::Cargo { use_binstall } => upgrade_cargo(*use_binstall, target_version),
        InstallMethod::InstallScript => upgrade_install_script(target_version),
        InstallMethod::Unknown { path } => {
            bail!("{}", unknown_method_help(path));
        }
    }
}

fn unknown_method_help(path: &Path) -> String {
    format!(
        "could not determine install method for `{}`.\n\
         Reinstall with one of:\n\
         • brew install {BREW_FORMULA}\n\
         • npm install -g {NPM_PACKAGE}\n\
         • cargo binstall {BINARY_NAME}\n\
         • curl --proto '=https' --tlsv1.2 -LsSf https://github.com/{GITHUB_REPO}/releases/latest/download/{BINARY_NAME}-installer.sh | sh",
        path.display()
    )
}

/// Captured mirror of [`run_method_upgrade`] for TUI use: nothing inherits the
/// terminal, nothing prints, nothing can prompt.
fn run_method_upgrade_captured(method: &InstallMethod, target_version: &str) -> Result<()> {
    match method {
        InstallMethod::Homebrew => upgrade_brew_captured(target_version),
        InstallMethod::Js { manager } => upgrade_js_captured(manager, target_version),
        InstallMethod::Cargo { use_binstall } => {
            upgrade_cargo_captured(*use_binstall, target_version)
        }
        InstallMethod::InstallScript => upgrade_install_script_captured(target_version),
        InstallMethod::Unknown { path } => {
            bail!("{}", unknown_method_help(path));
        }
    }
}

fn upgrade_brew_captured(target_version: &str) -> Result<()> {
    if !command_exists("brew") {
        bail!("detected Homebrew install, but `brew` is not on PATH");
    }

    // Third-party formulae typically only track latest; specific versions aren't pin-installable.
    let _ = target_version;
    run_command_captured("brew", &["upgrade", BREW_FORMULA])
}

fn upgrade_js_captured(manager: &JsPackageManager, target_version: &str) -> Result<()> {
    let spec = format!("{NPM_PACKAGE}@{target_version}");
    let (bin, args) = manager.install_global_cmd(&spec);
    if !command_exists(&bin) {
        bail!(
            "detected `{}` install, but `{bin}` is not on PATH",
            manager.as_str()
        );
    }
    run_command_captured(&bin, &args.iter().map(String::as_str).collect::<Vec<_>>())
}

fn upgrade_cargo_captured(use_binstall: bool, target_version: &str) -> Result<()> {
    if use_binstall && command_exists("cargo-binstall") {
        let args = [
            "binstall".to_string(),
            "-y".to_string(),
            format!("{BINARY_NAME}@{target_version}"),
        ];
        return run_command_captured(
            "cargo",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }

    if !command_exists("cargo") {
        bail!("detected cargo install, but `cargo` is not on PATH");
    }

    let args = [
        "install".to_string(),
        BINARY_NAME.to_string(),
        "--locked".to_string(),
        "--force".to_string(),
        "--version".to_string(),
        target_version.to_string(),
    ];
    run_command_captured(
        "cargo",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

fn upgrade_install_script_captured(target_version: &str) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = target_version;
        bail!(
            "automatic upgrades via install script are not supported on Windows; \
             reinstall with npm, cargo, or the latest GitHub release"
        );
    }

    #[cfg(not(windows))]
    {
        let tag = format!("v{}", normalize_version(target_version));
        let url = format!(
            "https://github.com/{GITHUB_REPO}/releases/download/{tag}/{BINARY_NAME}-installer.sh"
        );
        let latest_url = format!(
            "https://github.com/{GITHUB_REPO}/releases/latest/download/{BINARY_NAME}-installer.sh"
        );

        let installer_bytes = download_bytes_captured(&url)
            .filter(|bytes| !bytes.is_empty())
            .or_else(|| download_bytes_captured(&latest_url).filter(|bytes| !bytes.is_empty()))
            .context("failed to download installer")?;

        // Secure temp-file execution (no stdin-pipe deadlock):
        // the old code wrote the whole script to the child's stdin while
        // stdout/stderr pipes were undrained — a large script plus verbose
        // installer output fills both 64 KiB pipe buffers and deadlocks
        // (parent blocked on stdin write, child blocked on stdout write).
        // Executing a temp file with null stdin drains stdout/stderr via
        // `output()` (no pipe stall), fails fast on prompts (EOF), and the
        // `NamedTempFile` deletes on drop even on failure while `output()`
        // reaps the child (no zombie, no unrelated-process kill).
        use std::io::Write;
        let mut script_file =
            tempfile::NamedTempFile::with_suffix(".sh").context("failed to stage installer")?;
        script_file
            .write_all(&installer_bytes)
            .context("failed to stage installer")?;
        script_file.flush().context("failed to stage installer")?;

        let mut cmd = Command::new("sh");
        cmd.arg(script_file.path())
            .arg(&tag)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NONINTERACTIVE", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        // Pin the reinstall to the original receipt prefix so an upgrade
        // overwrites the same destination even if CARGO_HOME drifted since
        // install. Both the per-app var and the generic dist override are
        // set; unknown installer versions ignore unknown env harmlessly.
        if let Some(prefix) = load_shell_install_receipt().and_then(|r| r.reinstall_prefix()) {
            cmd.env("CRABCODE_INSTALL_DIR", &prefix);
            cmd.env("CARGO_DIST_FORCE_INSTALL_DIR", &prefix);
        }
        let output = cmd.output().context("installer failed to run")?;
        // `script_file` deletes here on all paths (success/failure/panic
        // unwind via drop); `output()` already waited+reaped the child.
        if !output.status.success() {
            let tail = tail_text(&output.stderr, MAX_ERROR_TAIL_CHARS);
            bail!("installer exited with {}\n{tail}", output.status);
        }
        Ok(())
    }
}

fn download_bytes_captured(url: &str) -> Option<Vec<u8>> {
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "25", url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    Some(output.stdout)
}

/// Max trailing chars of captured stderr kept in upgrade errors (the toast
/// truncates display anyway, but the full message stays copyable).
const MAX_ERROR_TAIL_CHARS: usize = 800;

/// Keep the tail of command output for errors, on char boundaries.
fn tail_text(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    trimmed
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Run a package-manager upgrade with no terminal attachment: stdin is null
/// (prompts get EOF and fail fast), output is captured, and helpers are told
/// not to prompt.
fn run_command_captured(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("HOMEBREW_NO_INPUT", "1")
        .env("NONINTERACTIVE", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("failed to run `{program}`"))?;

    if !output.status.success() {
        let tail = tail_text(&output.stderr, MAX_ERROR_TAIL_CHARS);
        bail!(
            "`{program} {}` failed with {}\n{tail}",
            args.join(" "),
            output.status
        );
    }
    Ok(())
}

fn upgrade_brew(target_version: &str) -> Result<()> {
    if !command_exists("brew") {
        bail!("detected Homebrew install, but `brew` is not on PATH");
    }

    // Third-party formulae typically only track latest; specific versions aren't pin-installable.
    let _ = target_version;
    let args = ["upgrade", BREW_FORMULA];
    println!("→ Doing `brew {}`", args.join(" "));
    run_command("brew", &args)
}

fn upgrade_js(manager: &JsPackageManager, target_version: &str) -> Result<()> {
    let spec = format!("{NPM_PACKAGE}@{target_version}");
    let (bin, args) = manager.install_global_cmd(&spec);
    if !command_exists(&bin) {
        bail!(
            "detected `{}` install, but `{bin}` is not on PATH",
            manager.as_str()
        );
    }
    println!("→ Doing `{} {}`", bin, args.join(" "));
    run_command(&bin, &args.iter().map(String::as_str).collect::<Vec<_>>())
}

fn upgrade_cargo(use_binstall: bool, target_version: &str) -> Result<()> {
    if use_binstall && command_exists("cargo-binstall") {
        let args = [
            "binstall".to_string(),
            "-y".to_string(),
            format!("{BINARY_NAME}@{target_version}"),
        ];
        println!("→ Doing `cargo {}`", args.join(" "));
        return run_command(
            "cargo",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }

    if !command_exists("cargo") {
        bail!("detected cargo install, but `cargo` is not on PATH");
    }

    let args = [
        "install".to_string(),
        BINARY_NAME.to_string(),
        "--locked".to_string(),
        "--force".to_string(),
        "--version".to_string(),
        target_version.to_string(),
    ];
    println!("→ Doing `cargo {}`", args.join(" "));
    run_command(
        "cargo",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

fn upgrade_install_script(target_version: &str) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = target_version;
        bail!(
            "automatic upgrades via install script are not supported on Windows; \
             reinstall with npm, cargo, or the latest GitHub release"
        );
    }

    #[cfg(not(windows))]
    {
        let tag = format!("v{}", normalize_version(target_version));
        let url = format!(
            "https://github.com/{GITHUB_REPO}/releases/download/{tag}/{BINARY_NAME}-installer.sh"
        );
        let latest_url = format!(
            "https://github.com/{GITHUB_REPO}/releases/latest/download/{BINARY_NAME}-installer.sh"
        );

        println!("→ Doing `curl ... | sh -s -- {tag}`");

        let script = Command::new("curl")
            .args(["-fsSL", "--max-time", "25", &url])
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .context("failed to download installer")?;

        let installer_bytes = if script.status.success() && !script.stdout.is_empty() {
            script.stdout
        } else {
            let fallback = Command::new("curl")
                .args(["-fsSL", "--max-time", "25", &latest_url])
                .stdin(Stdio::null())
                .stderr(Stdio::piped())
                .env("GIT_TERMINAL_PROMPT", "0")
                .output()
                .context("failed to download installer")?;
            if !fallback.status.success() {
                bail!(
                    "failed to download installer: {}",
                    String::from_utf8_lossy(&fallback.stderr)
                );
            }
            fallback.stdout
        };

        let mut child = Command::new("sh")
            .arg("-s")
            .arg("--")
            .arg(&tag)
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to start installer shell")?;

        use std::io::Write;
        child
            .stdin
            .as_mut()
            .context("failed to open installer stdin")?
            .write_all(&installer_bytes)
            .context("failed to pipe installer script")?;

        let status = child.wait().context("installer failed to run")?;
        if !status.success() {
            bail!("installer exited with {status}");
        }
        Ok(())
    }
}

fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to run `{program}`"))?;

    if !status.success() {
        bail!("`{program} {}` failed with {status}", args.join(" "));
    }
    Ok(())
}

fn command_exists(name: &str) -> bool {
    #[cfg(unix)]
    {
        Command::new("which")
            .arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        Command::new("where")
            .arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn brew_owns_formula(name: &str) -> bool {
    if !command_exists("brew") {
        return false;
    }
    Command::new("brew")
        .args(["list", "--formula", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("HOMEBREW_NO_INPUT", "1")
        .env("NONINTERACTIVE", "1")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cargo_install_list_has(name: &str) -> bool {
    if !command_exists("cargo") {
        return false;
    }
    let output = Command::new("cargo")
        .args(["install", "--list"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines().any(|line| {
                line.starts_with(&format!("{name} ")) || line.starts_with(&format!("{name} v"))
            })
        }
        _ => false,
    }
}

fn js_global_has(package: &str) -> bool {
    js_global_has_with("npm", package)
        || js_global_has_with("bun", package)
        || js_global_has_with("pnpm", package)
        || js_global_has_with("yarn", package)
}

fn js_global_has_with(manager: &str, package: &str) -> bool {
    if !command_exists(manager) {
        return false;
    }
    let output = match manager {
        "npm" => Command::new("npm")
            .args(["list", "-g", "--depth=0", package])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output(),
        "bun" => Command::new("bun")
            .args(["pm", "ls", "-g"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output(),
        "pnpm" => Command::new("pnpm")
            .args(["list", "-g", "--depth=0", package])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output(),
        // Yarn classic lists globals via `yarn global list`; Berry has no
        // stable global-list. A failed/unknown layout means "not proven
        // owner" (false) so we fail safe to Unknown rather than guessing.
        "yarn" => Command::new("yarn")
            .args(["global", "list", "--depth=0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output(),
        _ => return false,
    };
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.contains(package)
        }
        _ => false,
    }
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Cargo home respecting `CARGO_HOME` (custom installs) with `~/.cargo`
/// fallback. Both `cargo install` and the cargo-dist shell installer default
/// to `$CARGO_HOME/bin`, so detection and receipt checks must use this — not
/// a hardcoded `~/.cargo/bin`.
fn cargo_home() -> PathBuf {
    if let Some(dir) = env::var_os("CARGO_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cargo")
}

fn cargo_bin_dir() -> PathBuf {
    cargo_home().join("bin")
}

/// cargo-dist shell-install receipt (written unless `*_UNMANAGED_INSTALL`).
///
/// Shell installer writes `$HOME/.config/{app}/{app}-receipt.json`
/// (dist ≥0.27 also respects `XDG_CONFIG_HOME`). The receipt records the
/// install prefix/layout/binaries so an updater can tell a shell install in
/// `$CARGO_HOME/bin` apart from a `cargo install` in the same directory.
#[derive(Debug, Clone, Deserialize, Default)]
struct InstallReceipt {
    #[serde(default)]
    binaries: Vec<String>,
    #[serde(default)]
    binary_aliases: std::collections::HashMap<String, Vec<String>>,
    #[serde(default)]
    install_prefix: String,
    #[serde(default)]
    install_layout: String,
}

impl InstallReceipt {
    fn owns_binary(&self, binary: &str) -> bool {
        self.binaries.iter().any(|b| b == binary)
            || self.binary_aliases.keys().any(|k| k == binary)
            || self
                .binary_aliases
                .values()
                .flatten()
                .any(|alias| alias == binary)
    }

    /// True when this receipt pins `binary_path` as its install destination.
    fn claims_path(&self, binary_path: &Path) -> bool {
        if !self.owns_binary(BINARY_NAME) {
            return false;
        }
        let prefix = expand_receipt_prefix(&self.install_prefix);
        if prefix.as_os_str().is_empty() {
            return false;
        }
        // cargo-home layout installs to `<prefix>/bin`; flat layouts install
        // directly under prefix. `starts_with(prefix)` covers both without
        // guessing layout strings.
        binary_path.starts_with(&prefix)
    }

    /// Prefix to force on reinstall so the upgrade overwrites the original
    /// destination even if `CARGO_HOME` changed since install.
    fn reinstall_prefix(&self) -> Option<String> {
        let prefix = self.install_prefix.trim();
        if prefix.is_empty() || !self.owns_binary(BINARY_NAME) {
            return None;
        }
        Some(prefix.to_string())
    }
}

fn expand_receipt_prefix(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return PathBuf::new();
    }
    // Receipts are absolute paths, but expand common placeholders defensively.
    if let Some(rest) = trimmed
        .strip_prefix("$CARGO_HOME")
        .or_else(|| trimmed.strip_prefix("${CARGO_HOME}"))
    {
        let rest = rest.trim_start_matches('/').trim_start_matches('\\');
        return cargo_home().join(rest);
    }
    if let Some(rest) = trimmed
        .strip_prefix("$HOME")
        .or_else(|| trimmed.strip_prefix("${HOME}"))
    {
        let rest = rest.trim_start_matches('/').trim_start_matches('\\');
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
        return PathBuf::from(rest);
    }
    if let Some(rest) = trimmed.strip_prefix('~') {
        let rest = rest.trim_start_matches('/').trim_start_matches('\\');
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
        return PathBuf::from(rest);
    }
    PathBuf::from(trimmed)
}

fn shell_receipt_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // dist ≥0.27 respects XDG_CONFIG_HOME; older installers used HOME/.config.
    // Check both (deduplicated) so upgrades work regardless of installer age.
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            let dir = PathBuf::from(xdg).join("crabcode");
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    if let Some(home) = home_dir() {
        let dir = home.join(".config").join("crabcode");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    // Windows PowerShell installer receipt location.
    #[cfg(windows)]
    {
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            if !local.is_empty() {
                let dir = PathBuf::from(local).join("crabcode");
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
    }
    dirs
}

fn load_shell_install_receipt() -> Option<InstallReceipt> {
    let dirs = shell_receipt_dirs();
    // Preferred exact receipt name first (`{app}-receipt.json`).
    for dir in &dirs {
        let candidate = dir.join(format!("{BINARY_NAME}-receipt.json"));
        if let Ok(bytes) = std::fs::read(&candidate) {
            if let Ok(receipt) = serde_json::from_slice::<InstallReceipt>(&bytes) {
                if receipt.owns_binary(BINARY_NAME) {
                    return Some(receipt);
                }
            }
        }
    }
    // Fallback: scan receipt dirs for any JSON receipt owning our binary
    // (tolerates future installer renames without misattributing others).
    for dir in &dirs {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(receipt) = serde_json::from_slice::<InstallReceipt>(&bytes) {
                    if receipt.owns_binary(BINARY_NAME) {
                        return Some(receipt);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_version_prefixes() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
    }

    /// Hermetic detection helper: injects ownership signals instead of
    /// spawning `cargo`/`brew`/`npm` (no test-env races, no PATH dependence).
    fn detect_with(
        path: &Path,
        cargo_bin: &Path,
        receipt: Option<InstallReceipt>,
        cargo_claims: bool,
        brew_claims: bool,
        js_owner: Option<JsPackageManager>,
        js_any: bool,
    ) -> InstallMethod {
        let receipt_clone = receipt.clone();
        let js_owner_clone = js_owner.clone();
        detect_install_method_from_path_impl(
            path,
            cargo_bin,
            move || receipt_clone.clone(),
            move || cargo_claims,
            move || brew_claims,
            move || js_owner_clone.clone(),
            move || js_any,
            false,
        )
    }

    fn fake_cargo_bin() -> PathBuf {
        PathBuf::from("/Users/me/.cargo/bin")
    }

    fn shell_receipt_for(prefix: &str) -> InstallReceipt {
        InstallReceipt {
            binaries: vec![BINARY_NAME.to_string()],
            binary_aliases: Default::default(),
            install_prefix: prefix.to_string(),
            install_layout: "cargo-home".to_string(),
        }
    }

    #[test]
    fn detects_npm_node_modules_path_with_single_owner() {
        let path = PathBuf::from(
            "/Users/me/.local/share/fnm/node-versions/v22/installation/lib/node_modules/crabcode/bin/crabcode",
        );
        // Generic node_modules + exactly one global-list owner => that manager.
        let method = detect_with(
            &path,
            &fake_cargo_bin(),
            None,
            false,
            false,
            Some(JsPackageManager::Npm),
            true,
        );
        assert!(
            matches!(
                method,
                InstallMethod::Js {
                    manager: JsPackageManager::Npm
                }
            ),
            "{method:?}"
        );
    }

    #[test]
    fn generic_node_modules_without_single_owner_fails_safe() {
        let path = PathBuf::from(
            "/Users/me/.local/share/fnm/node-versions/v22/installation/lib/node_modules/crabcode/bin/crabcode",
        );
        // No owner (fresh env) => Unknown, never guess an available manager.
        let none = detect_with(&path, &fake_cargo_bin(), None, false, false, None, false);
        assert!(matches!(none, InstallMethod::Unknown { .. }), "{none:?}");
        // Multi-owner (npm+bun both list it) => Unknown (ambiguous).
        // `detect_with` takes a single resolved owner; `None` models the
        // strict resolver's ambiguous output (js_any true but no single owner).
        let ambiguous = detect_with(&path, &fake_cargo_bin(), None, false, false, None, true);
        assert!(
            matches!(ambiguous, InstallMethod::Unknown { .. }),
            "{ambiguous:?}"
        );
    }

    #[test]
    fn detects_bun_path_by_marker_without_global_probe() {
        let path =
            PathBuf::from("/Users/me/.bun/install/global/node_modules/crabcode/bin/crabcode");
        // Bun path marker is authoritative even when the ownership probe is
        // unavailable (e.g. `bun` not on PATH in this env).
        let method = detect_with(&path, &fake_cargo_bin(), None, false, false, None, false);
        match method {
            InstallMethod::Js {
                manager: JsPackageManager::Bun,
            } => {}
            other => panic!("expected bun, got {other:?}"),
        }
    }

    #[test]
    fn cargo_bin_with_cargo_claim_only_is_cargo() {
        let cargo_bin = fake_cargo_bin();
        let path = cargo_bin.join("crabcode");
        let method = detect_with(&path, &cargo_bin, None, true, false, None, false);
        assert!(matches!(method, InstallMethod::Cargo { .. }), "{method:?}");
    }

    #[test]
    fn cargo_bin_with_receipt_only_is_shell_installer() {
        // cargo-dist shell installer defaults to $CARGO_HOME/bin: receipt
        // present + cargo silent must NOT be misclassified as Cargo.
        let cargo_bin = fake_cargo_bin();
        let path = cargo_bin.join("crabcode");
        let receipt = shell_receipt_for("/Users/me/.cargo");
        assert!(receipt.claims_path(&path));
        let method = detect_with(&path, &cargo_bin, Some(receipt), false, false, None, false);
        assert_eq!(method, InstallMethod::InstallScript);
    }

    #[test]
    fn cargo_bin_with_neither_or_both_claims_is_unknown() {
        let cargo_bin = fake_cargo_bin();
        let path = cargo_bin.join("crabcode");
        // Neither signal (unmanaged copy) => fail safe.
        let neither = detect_with(&path, &cargo_bin, None, false, false, None, false);
        assert!(
            matches!(neither, InstallMethod::Unknown { .. }),
            "{neither:?}"
        );
        // Both signals (double install) => ambiguous, fail safe.
        let receipt = shell_receipt_for("/Users/me/.cargo");
        let both = detect_with(&path, &cargo_bin, Some(receipt), true, false, None, false);
        assert!(matches!(both, InstallMethod::Unknown { .. }), "{both:?}");
    }

    #[test]
    fn custom_cargo_home_bin_is_disambiguated_same_as_default() {
        // Custom CARGO_HOME must use the same receipt-vs-cargo rule, not a
        // hardcoded ~/.cargo/bin prefix.
        let cargo_bin = PathBuf::from("/custom/cargo/bin");
        let path = cargo_bin.join("crabcode");
        let receipt = shell_receipt_for("/custom/cargo");
        assert!(receipt.claims_path(&path));
        let shell = detect_with(&path, &cargo_bin, Some(receipt), false, false, None, false);
        assert_eq!(shell, InstallMethod::InstallScript);
        let cargo = detect_with(&path, &cargo_bin, None, true, false, None, false);
        assert!(matches!(cargo, InstallMethod::Cargo { .. }), "{cargo:?}");
    }

    #[test]
    fn receipt_must_own_binary_and_prefix_to_claim() {
        let path = PathBuf::from("/Users/me/.cargo/bin/crabcode");
        let mut receipt = shell_receipt_for("/Users/me/.cargo");
        assert!(receipt.claims_path(&path));
        // Wrong binary name => no claim.
        receipt.binaries = vec!["other-tool".to_string()];
        assert!(!receipt.claims_path(&path));
        // Wrong prefix => no claim.
        receipt.binaries = vec![BINARY_NAME.to_string()];
        receipt.install_prefix = "/other/prefix".to_string();
        assert!(!receipt.claims_path(&path));
        // Alias ownership still counts.
        receipt.install_prefix = "/Users/me/.cargo".to_string();
        receipt.binaries.clear();
        receipt
            .binary_aliases
            .insert(BINARY_NAME.to_string(), vec!["alias".to_string()]);
        assert!(receipt.claims_path(&path));
    }

    #[test]
    fn receipt_prefix_placeholders_expand() {
        // $CARGO_HOME placeholder resolves via cargo_home() (no panic on
        // unset env); absolute paths pass through unchanged.
        let abs = expand_receipt_prefix("/Users/me/.cargo");
        assert_eq!(abs, PathBuf::from("/Users/me/.cargo"));
        assert!(expand_receipt_prefix("").as_os_str().is_empty());
        // Placeholder branches must not panic regardless of env.
        let _ = expand_receipt_prefix("$CARGO_HOME/bin");
        let _ = expand_receipt_prefix("$HOME/.cargo");
        let _ = expand_receipt_prefix("~/.cargo");
    }

    #[test]
    fn ambiguous_path_prefers_strict_js_owner_over_guess() {
        // Non-node_modules path with a global-list hit but no single owner
        // must fail safe instead of upgrading via an unrelated manager.
        let path = PathBuf::from("/usr/local/bin/crabcode");
        let ambiguous = detect_with(&path, &fake_cargo_bin(), None, false, false, None, true);
        assert!(
            matches!(ambiguous, InstallMethod::Unknown { .. }),
            "{ambiguous:?}"
        );
        let owned = detect_with(
            &path,
            &fake_cargo_bin(),
            None,
            false,
            false,
            Some(JsPackageManager::Npm),
            true,
        );
        assert!(
            matches!(
                owned,
                InstallMethod::Js {
                    manager: JsPackageManager::Npm
                }
            ),
            "{owned:?}"
        );
    }

    #[test]
    fn detects_install_script_path() {
        let path = PathBuf::from("/Users/me/.local/bin/crabcode");
        let method = detect_with(&path, &fake_cargo_bin(), None, false, false, None, false);
        assert_eq!(method, InstallMethod::InstallScript);
    }

    #[test]
    fn detects_homebrew_cellar_path() {
        let path = PathBuf::from("/opt/homebrew/Cellar/crabcode/0.0.10/bin/crabcode");
        let method = detect_with(&path, &fake_cargo_bin(), None, false, false, None, false);
        assert_eq!(method, InstallMethod::Homebrew);
    }

    #[test]
    fn skips_upgrade_when_versions_match() {
        let check = resolve_target_version("0.0.10", Some("v0.0.10")).unwrap();
        assert!(!check.needs_upgrade);
        assert_eq!(check.target, "0.0.10");
    }

    #[test]
    fn current_version_matches_package() {
        assert_eq!(current_version(), env!("CARGO_PKG_VERSION"));
        assert!(!current_version().is_empty());
    }

    #[test]
    fn noninteractive_upgrade_is_noop_when_already_current() {
        // Explicit same-version target never touches the network or installers.
        let current = current_version();
        let target = upgrade_noninteractive(Some(&current)).unwrap();
        assert_eq!(target, current.trim().trim_start_matches('v'));
    }

    #[test]
    fn tail_text_keeps_short_output_whole() {
        assert_eq!(tail_text(b"  boom\n", 800), "boom");
        assert_eq!(tail_text(b"", 800), "");
    }

    #[test]
    fn tail_text_truncates_to_trailing_chars() {
        let long = "x".repeat(1000);
        let tail = tail_text(long.as_bytes(), 800);
        assert_eq!(tail.len(), 800);
        assert_eq!(tail, "x".repeat(800));
    }

    #[test]
    fn unknown_method_help_names_alternatives() {
        let help = unknown_method_help(Path::new("/tmp/odd-place/crabcode"));
        assert!(help.contains("could not determine install method"));
        assert!(help.contains("brew install"));
        assert!(help.contains("npm install -g"));
    }
}
