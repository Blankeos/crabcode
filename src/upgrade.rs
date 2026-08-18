//! Self-upgrade: detect install method and reinstall via the matching tool.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

const GITHUB_REPO: &str = "Blankeos/crabcode";
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

fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: crabcode-upgrade",
            &url,
        ])
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
        let manager = detect_js_manager_from_path(&path_str);
        return InstallMethod::Js { manager };
    }

    // cargo install / cargo binstall
    if let Some(home) = home_dir() {
        let cargo_bin = home.join(".cargo").join("bin");
        if path.starts_with(&cargo_bin) {
            return InstallMethod::Cargo {
                use_binstall: command_exists("cargo-binstall"),
            };
        }
    }

    // install.sh default destination
    if path_str.contains("/.local/bin/") || path_str.contains("\\.local\\bin\\") {
        return InstallMethod::InstallScript;
    }

    // Heuristics when path alone is ambiguous
    if brew_owns_formula(BINARY_NAME) {
        return InstallMethod::Homebrew;
    }
    if cargo_install_list_has(BINARY_NAME) {
        return InstallMethod::Cargo {
            use_binstall: command_exists("cargo-binstall"),
        };
    }
    if js_global_has(NPM_PACKAGE) {
        let manager = detect_js_manager_available();
        return InstallMethod::Js { manager };
    }

    InstallMethod::Unknown {
        path: path.to_path_buf(),
    }
}

fn detect_js_manager_from_path(path_str: &str) -> JsPackageManager {
    if path_str.contains("/.bun/") || path_str.contains("\\.bun\\") {
        JsPackageManager::Bun
    } else if path_str.contains("pnpm") {
        JsPackageManager::Pnpm
    } else if path_str.contains("yarn") {
        JsPackageManager::Yarn
    } else {
        detect_js_manager_available()
    }
}

fn detect_js_manager_available() -> JsPackageManager {
    if command_exists("bun") && js_global_has_with("bun", NPM_PACKAGE) {
        return JsPackageManager::Bun;
    }
    if command_exists("pnpm") && js_global_has_with("pnpm", NPM_PACKAGE) {
        return JsPackageManager::Pnpm;
    }
    if command_exists("yarn") {
        return JsPackageManager::Yarn;
    }
    if command_exists("bun") {
        return JsPackageManager::Bun;
    }
    JsPackageManager::Npm
}

fn run_method_upgrade(method: &InstallMethod, target_version: &str) -> Result<()> {
    match method {
        InstallMethod::Homebrew => upgrade_brew(target_version),
        InstallMethod::Js { manager } => upgrade_js(manager, target_version),
        InstallMethod::Cargo { use_binstall } => upgrade_cargo(*use_binstall, target_version),
        InstallMethod::InstallScript => upgrade_install_script(target_version),
        InstallMethod::Unknown { path } => {
            bail!(
                "could not determine install method for `{}`.\n\
                 Reinstall with one of:\n\
                 • brew install {BREW_FORMULA}\n\
                 • npm install -g {NPM_PACKAGE}\n\
                 • cargo binstall {BINARY_NAME}\n\
                 • curl --proto '=https' --tlsv1.2 -LsSf https://github.com/{GITHUB_REPO}/releases/latest/download/{BINARY_NAME}-installer.sh | sh",
                path.display()
            )
        }
    }
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
            .args(["-fsSL", &url])
            .output()
            .context("failed to download installer")?;

        let installer_bytes = if script.status.success() && !script.stdout.is_empty() {
            script.stdout
        } else {
            let fallback = Command::new("curl")
                .args(["-fsSL", &latest_url])
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
            .output(),
        "bun" => Command::new("bun")
            .args(["pm", "ls", "-g"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output(),
        "pnpm" => Command::new("pnpm")
            .args(["list", "-g", "--depth=0", package])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_version_prefixes() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn detects_npm_node_modules_path() {
        let path = PathBuf::from(
            "/Users/me/.local/share/fnm/node-versions/v22/installation/lib/node_modules/crabcode/bin/crabcode",
        );
        let method = detect_install_method_from_path(&path);
        assert!(matches!(method, InstallMethod::Js { .. }), "{method:?}");
    }

    #[test]
    fn detects_bun_path() {
        let path =
            PathBuf::from("/Users/me/.bun/install/global/node_modules/crabcode/bin/crabcode");
        match detect_install_method_from_path(&path) {
            InstallMethod::Js {
                manager: JsPackageManager::Bun,
            } => {}
            other => panic!("expected bun, got {other:?}"),
        }
    }

    #[test]
    fn detects_cargo_bin_path() {
        let home = home_dir().expect("home");
        let path = home.join(".cargo/bin/crabcode");
        let method = detect_install_method_from_path(&path);
        assert!(matches!(method, InstallMethod::Cargo { .. }), "{method:?}");
    }

    #[test]
    fn detects_install_script_path() {
        let path = PathBuf::from("/Users/me/.local/bin/crabcode");
        assert_eq!(
            detect_install_method_from_path(&path),
            InstallMethod::InstallScript
        );
    }

    #[test]
    fn detects_homebrew_cellar_path() {
        let path = PathBuf::from("/opt/homebrew/Cellar/crabcode/0.0.10/bin/crabcode");
        assert_eq!(
            detect_install_method_from_path(&path),
            InstallMethod::Homebrew
        );
    }

    #[test]
    fn skips_upgrade_when_versions_match() {
        let check = resolve_target_version("0.0.10", Some("v0.0.10")).unwrap();
        assert!(!check.needs_upgrade);
        assert_eq!(check.target, "0.0.10");
    }
}
