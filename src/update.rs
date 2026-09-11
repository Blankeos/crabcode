//! Lazy, nonblocking update check with a 24-hour file cache.
//!
//! The TUI triggers one background check per process after first paint (see
//! `App::maybe_start_update_check`). A fresh cache is read synchronously — no
//! thread, no network — while a stale/missing cache spawns a single blocking
//! lookup. All check failures are silent (`None`); only a strictly newer
//! release produces the ephemeral `New version available · Upgrade` toast.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Successful version lookups stay valid for 24 hours.
pub const UPDATE_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const UPDATE_CACHE_FILE: &str = "update_check.json";
const FETCH_TIMEOUT_SECS: &str = "10";

/// Opt-out: any `CRABCODE_NO_UPDATE_CHECK` value disables the check.
pub fn update_check_disabled() -> bool {
    std::env::var_os("CRABCODE_NO_UPDATE_CHECK").is_some()
}

/// UI-testing preview: `CRABCODE_FORCE_UPDATE_NOTICE` forces the update toast
/// without cache/network/version checks. Never auto-upgrades; a click still
/// runs the normal upgrade flow. `CRABCODE_NO_UPDATE_CHECK` wins when both set.
pub fn force_update_notice() -> bool {
    std::env::var("CRABCODE_FORCE_UPDATE_NOTICE")
        .map(|v| is_truthy_flag(&v))
        .unwrap_or(false)
}

fn is_truthy_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

/// Effective preview after precedence: force shows unless disabled.
/// Pure helper so the disable-wins rule is unit-testable without env.
pub(crate) fn forced_preview_active(disabled: bool, forced: bool) -> bool {
    forced && !disabled
}

/// Background version-check outcome: `Some(latest)` when a newer release is
/// available, `None` when up to date or the check failed silently.
pub type UpdateCheckResult = Option<String>;

/// Background upgrade outcome: `Ok(version)` after installers succeed,
/// `Err(message)` with a short human-readable failure.
pub type UpgradeOutcome = Result<String, String>;

/// Spawn blocking update/upgrade work on a detached OS thread and deliver the
/// result over the (runtime-independent) unbounded channel.
///
/// Must not use `tokio::task::spawn_blocking` here: the Tokio runtime waits
/// indefinitely on shutdown for started blocking tasks even when the
/// `JoinHandle` is dropped, so a minutes-long `cargo install`/`brew upgrade`
/// would hang TUI quit. A detached `std` thread is untracked by the runtime,
/// so shutdown stays immediate; the installer child is reparented on TUI exit
/// and reaped via `output()` while attached. If the TUI already exited, the
/// result send just fails silently. No auto-restart, no process kills.
pub(crate) fn spawn_detached_update_worker<T: Send + 'static>(
    thread_name: &str,
    sender: tokio::sync::mpsc::UnboundedSender<T>,
    work: impl FnOnce() -> T + Send + 'static,
) {
    let _ = std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            let out = work();
            let _ = sender.send(out);
        });
    // JoinHandle intentionally dropped (detached). Spawn failure drops the
    // closure (and its sender), so receivers observe `Disconnected` and clear
    // promptly instead of polling forever: silent for the version check,
    // "background task ended" toast for the upgrade.
}

/// Format an upgrade failure for the error toast.
pub fn upgrade_failure_message(err: &str) -> String {
    let trimmed = err.trim();
    if trimmed.is_empty() {
        return "Update failed: unknown error".to_string();
    }
    // Avoid doubling the prefix when the backend already says it.
    if trimmed.starts_with("Update failed:") {
        trimmed.to_string()
    } else {
        format!("Update failed: {trimmed}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UpdateCache {
    checked_at: u64,
    latest: String,
}

pub(crate) fn cache_path() -> PathBuf {
    crate::persistence::get_cache_dir().join(UPDATE_CACHE_FILE)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache_at(path: &Path) -> Option<UpdateCache> {
    let bytes = std::fs::read(path).ok()?;
    let cache: UpdateCache = serde_json::from_slice(&bytes).ok()?;
    if cache.latest.trim().is_empty() {
        return None;
    }
    Some(cache)
}

fn write_cache_at(path: &Path, latest: &str, now: u64) {
    // Cache writes are best-effort: a failed write just means retrying sooner.
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let cache = UpdateCache {
        checked_at: now,
        latest: latest.to_string(),
    };
    let Ok(bytes) = serde_json::to_vec(&cache) else {
        return;
    };
    // Atomic temp+rename so concurrent TUI processes or a crash mid-write
    // can never leave a truncated JSON that forces wasteful refetches.
    // Corrupt reads already fall back to refetch (`read_cache_at` => None).
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("update_check"),
        std::process::id()
    );
    let temp_path = parent.join(temp_name);
    if std::fs::write(&temp_path, &bytes).is_err() {
        let _ = std::fs::remove_file(&temp_path);
        return;
    }
    if std::fs::rename(&temp_path, path).is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
}

fn is_fresh_at(checked_at: u64, now: u64) -> bool {
    if checked_at > now {
        // Future timestamps (clock skew) count as fresh to avoid refetch loops.
        return true;
    }
    now - checked_at < UPDATE_CACHE_TTL_SECS
}

/// True when `latest` is strictly newer than `current`.
///
/// Parses both sides as semver after stripping a leading `v` (so `0.0.13`
/// beats `0.0.12`, while equal or older releases stay quiet). Tags that are
/// not semver fall back to inequality — the same eligibility `crabcode
/// upgrade` uses — so unusual tags still surface instead of vanishing.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let current = current.trim().trim_start_matches('v');
    let latest = latest.trim().trim_start_matches('v');
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(current), Ok(latest)) => latest > current,
        _ => !current.eq_ignore_ascii_case(latest),
    }
}

/// Toast eligibility: show the nudge only for strictly newer releases.
pub fn is_update_available(current: &str, latest: &str) -> bool {
    is_newer_version(current, latest)
}

/// Fresh cached release newer than `current`, without touching the network.
pub(crate) fn load_cached_update(current: &str) -> Option<String> {
    load_cached_update_at(&cache_path(), current, now_secs())
}

fn load_cached_update_at(path: &Path, current: &str, now: u64) -> Option<String> {
    let cache = read_cache_at(path)?;
    if !is_fresh_at(cache.checked_at, now) {
        return None;
    }
    is_update_available(current, &cache.latest).then(|| cache.latest)
}

/// Whether the cache is missing/stale and a background fetch is warranted.
pub(crate) fn should_fetch_update() -> bool {
    should_fetch_update_at(&cache_path(), now_secs())
}

fn should_fetch_update_at(path: &Path, now: u64) -> bool {
    match read_cache_at(path) {
        None => true,
        Some(cache) => !is_fresh_at(cache.checked_at, now),
    }
}

/// Blocking check for the background thread: fresh cache first, else one
/// GitHub lookup. Successes refresh the cache; failures stay silent (`None`).
pub(crate) fn check_and_cache(current: &str) -> UpdateCheckResult {
    let path = cache_path();
    let now = now_secs();
    if let Some(latest) = load_cached_update_at(&path, current, now) {
        return Some(latest);
    }
    if !should_fetch_update_at(&path, now) {
        // Fresh cache that is already current — respect the 24h TTL.
        return None;
    }
    let latest = match fetch_latest_tag_blocking() {
        Ok(tag) => tag,
        Err(_) => return None,
    };
    write_cache_at(&path, &latest, now_secs());
    is_update_available(current, &latest).then_some(latest)
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
}

fn fetch_latest_tag_blocking() -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        crate::upgrade::GITHUB_REPO
    );
    let output = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            FETCH_TIMEOUT_SECS,
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: crabcode-update-check",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("failed to run curl")?;

    if !output.status.success() {
        anyhow::bail!("GitHub release lookup failed with {}", output.status);
    }

    let release: GithubRelease =
        serde_json::from_slice(&output.stdout).context("failed to parse release")?;
    Ok(release.tag_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_triggers_toast() {
        assert!(is_update_available("0.0.12", "0.0.13"));
        assert!(is_update_available("v0.0.12", "v0.0.13"));
        assert!(is_update_available("0.0.12", "v0.0.13"));
    }

    #[test]
    fn equal_versions_stay_quiet() {
        assert!(!is_update_available("0.0.12", "0.0.12"));
        assert!(!is_update_available("v0.0.12", "0.0.12"));
        assert!(!is_update_available(" 0.0.12 ", "v0.0.12"));
    }

    #[test]
    fn older_releases_stay_quiet() {
        // A dev build ahead of the latest release must not toast.
        assert!(!is_update_available("0.0.13", "0.0.12"));
        assert!(!is_update_available("0.1.0", "0.0.99"));
        assert!(!is_update_available("1.0.0", "0.9.9"));
    }

    #[test]
    fn prerelease_counts_as_older_than_release() {
        assert!(is_update_available("0.0.13-dev", "0.0.13"));
        assert!(!is_update_available("0.0.13", "0.0.13-dev"));
    }

    #[test]
    fn non_semver_tags_fall_back_to_inequality() {
        assert!(!is_update_available("abc", "abc"));
        assert!(is_update_available("abc", "def"));
    }

    #[test]
    fn fresh_cache_counts_but_stale_does_not() {
        let now = 1_700_000_000;
        assert!(is_fresh_at(now - 60, now));
        assert!(is_fresh_at(now - (UPDATE_CACHE_TTL_SECS - 1), now));
        assert!(!is_fresh_at(now - UPDATE_CACHE_TTL_SECS, now));
        assert!(!is_fresh_at(now - (UPDATE_CACHE_TTL_SECS + 1), now));
        assert!(!is_fresh_at(0, now));
    }

    #[test]
    fn future_cache_timestamps_count_as_fresh() {
        let now = 1_700_000_000;
        assert!(is_fresh_at(now + 60, now));
    }

    #[test]
    fn missing_or_corrupt_cache_forces_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("update_check.json");
        assert!(should_fetch_update_at(&missing, 1_700_000_000));
        assert!(read_cache_at(&missing).is_none());

        std::fs::write(&missing, b"not json").unwrap();
        assert!(should_fetch_update_at(&missing, 1_700_000_000));
        assert!(read_cache_at(&missing).is_none());

        std::fs::write(&missing, r#"{"checked_at":1,"latest":""}"#).unwrap();
        assert!(read_cache_at(&missing).is_none());
    }

    #[test]
    fn cache_roundtrip_and_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update_check.json");
        let now = 1_700_000_000;
        write_cache_at(&path, "0.0.13", now);

        let cache = read_cache_at(&path).unwrap();
        assert_eq!(
            cache,
            UpdateCache {
                checked_at: now,
                latest: "0.0.13".to_string(),
            }
        );
        assert!(!should_fetch_update_at(&path, now));
        assert!(should_fetch_update_at(&path, now + UPDATE_CACHE_TTL_SECS));
    }

    #[test]
    fn cached_update_only_when_fresh_and_newer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update_check.json");
        let now = 1_700_000_000;

        // Fresh + newer → toast eligible.
        write_cache_at(&path, "0.0.13", now);
        assert_eq!(
            load_cached_update_at(&path, "0.0.12", now),
            Some("0.0.13".to_string())
        );
        // Fresh + equal → quiet.
        assert_eq!(load_cached_update_at(&path, "0.0.13", now), None);
        // Fresh + older → quiet (ahead of release).
        assert_eq!(load_cached_update_at(&path, "0.0.14", now), None);
        // Stale even when newer → no toast without a refetch.
        assert_eq!(
            load_cached_update_at(&path, "0.0.12", now + UPDATE_CACHE_TTL_SECS),
            None
        );
    }

    #[test]
    fn force_flag_truthy_values() {
        for value in [
            "1", "true", "TRUE", "True", "yes", "YES", "y", "Y", "on", "ON", " 1 ",
        ] {
            assert!(is_truthy_flag(value), "{value:?} should be truthy");
        }
    }

    #[test]
    fn force_flag_falsy_values() {
        for value in ["", " ", "0", "false", "no", "off", "2", "maybe"] {
            assert!(!is_truthy_flag(value), "{value:?} should be falsy");
        }
    }

    #[test]
    fn disable_wins_over_force_for_preview() {
        assert!(forced_preview_active(false, true));
        // Disable wins: forced toast stays off when opted out.
        assert!(!forced_preview_active(true, true));
        assert!(!forced_preview_active(false, false));
        assert!(!forced_preview_active(true, false));
    }

    #[test]
    fn upgrade_failure_message_adds_prefix_once() {
        assert_eq!(upgrade_failure_message("boom"), "Update failed: boom");
        assert_eq!(
            upgrade_failure_message("Update failed: boom"),
            "Update failed: boom"
        );
        assert_eq!(
            upgrade_failure_message("  "),
            "Update failed: unknown error"
        );
        assert_eq!(upgrade_failure_message(""), "Update failed: unknown error");
    }

    #[test]
    fn detached_worker_does_not_block_tokio_shutdown() {
        use std::time::{Duration, Instant};

        // Same helper the TUI upgrade/check paths use, but with fake parked
        // work instead of real installers or network probes.
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u32>();

        rt.block_on(async {
            spawn_detached_update_worker("crabcode-test-detached", tx, move || {
                // Park without touching installers/network. `recv_timeout`
                // bounds the failure mode: if a refactor accidentally moves
                // this back to `spawn_blocking`, shutdown blocks ~10s then
                // fails instead of deadlocking the suite forever.
                let _ = gate_rx.recv_timeout(Duration::from_secs(10));
                42
            });
            // Let the worker start and park before shutting down.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
        });

        // Dropping the runtime must not wait for the parked detached worker.
        // `spawn_blocking` would wait indefinitely here (docs.rs: shutdown
        // waits indefinitely for started blocking tasks even if the handle is
        // dropped); a detached `std` thread is untracked so this is immediate.
        let start = Instant::now();
        drop(rt);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "detached update worker blocked Tokio shutdown for {elapsed:?}; \
             upgrade/check must use detached std threads, not spawn_blocking"
        );

        // Release the parked worker and prove the result still arrives without
        // a running runtime (unbounded send is runtime-independent).
        let _ = gate_tx.send(());
        let deadline = Instant::now() + Duration::from_secs(2);
        let got = loop {
            match rx.try_recv() {
                Ok(v) => break Some(v),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break None,
            }
        };
        assert_eq!(
            got,
            Some(42),
            "detached worker must still deliver via channel"
        );
    }
}
