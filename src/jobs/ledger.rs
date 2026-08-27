use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::persistence::get_data_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Exited,
    Killed,
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Killed => "killed",
            Self::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMeta {
    pub id: String,
    pub pid: u32,
    #[serde(default)]
    pub pgid: Option<u32>,
    pub command: String,
    pub name: String,
    pub workdir: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    pub status: JobStatus,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

pub fn jobs_root() -> PathBuf {
    get_data_dir().join("jobs")
}

pub fn job_dir(id: &str) -> PathBuf {
    jobs_root().join(id)
}

pub fn meta_path(id: &str) -> PathBuf {
    job_dir(id).join("meta.json")
}

pub fn log_path(id: &str) -> PathBuf {
    job_dir(id).join("output.log")
}

pub fn canonicalize_workdir(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

pub fn save_meta(meta: &JobMeta) -> Result<()> {
    let dir = job_dir(&meta.id);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create job dir {}", dir.display()))?;
    let path = meta_path(&meta.id);
    let json = serde_json::to_vec_pretty(meta).context("serialize job meta")?;
    // Write via temp + rename when possible; fall back to direct write.
    let tmp = dir.join(format!("meta.{}.tmp", std::process::id()));
    match fs::write(&tmp, &json) {
        Ok(()) => {
            if let Err(err) = fs::rename(&tmp, &path) {
                // Best-effort cleanup + direct write fallback (e.g. cross-device).
                let _ = fs::remove_file(&tmp);
                fs::write(&path, &json).with_context(|| {
                    format!("write {} (after rename failed: {err})", path.display())
                })?;
            }
        }
        Err(err) => {
            fs::write(&path, &json)
                .with_context(|| format!("write {} (tmp write failed: {err})", path.display()))?;
        }
    }
    Ok(())
}

pub fn load_meta(id: &str) -> Result<JobMeta> {
    let path = meta_path(id);
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let meta: JobMeta = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse job meta {}", path.display()))?;
    Ok(meta)
}

pub fn list_metas() -> Result<Vec<JobMeta>> {
    let root = jobs_root();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name();
        let Some(id) = id.to_str() else {
            continue;
        };
        match load_meta(id) {
            Ok(meta) => out.push(meta),
            Err(_) => continue,
        }
    }

    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(out)
}

pub fn list_for_project(workdir: &Path) -> Result<Vec<JobMeta>> {
    let wanted = canonicalize_workdir(workdir);
    let wanted_str = wanted.to_string_lossy();
    let mut out = Vec::new();
    for meta in list_metas()? {
        let job_wd = canonicalize_workdir(Path::new(&meta.workdir));
        if job_wd.to_string_lossy() == wanted_str {
            out.push(meta);
        }
    }
    Ok(out)
}

/// If a job is still marked `running` but its pid is dead, mark it exited.
/// Keeps meta + log on disk (no aggressive delete).
pub fn prune_dead() -> Result<usize> {
    let mut updated = 0usize;
    for mut meta in list_metas()? {
        if meta.status != JobStatus::Running {
            continue;
        }
        if is_pid_alive(meta.pid) {
            continue;
        }
        meta.status = JobStatus::Exited;
        meta.ended_at = Some(Utc::now());
        save_meta(&meta)?;
        updated += 1;
    }
    Ok(updated)
}

pub fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) is a liveness probe; no signal is delivered.
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        // EPERM means the process exists but we can't signal it.
        return err.raw_os_error() == Some(libc::EPERM);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Best-effort: try OpenProcess via tasklist / query. Prefer CreateToolhelp.
        // Fall back to probing via `tasklist` is slow; use Win32 OpenProcess.
        windows_pid_alive(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(windows)]
fn windows_pid_alive(pid: u32) -> bool {
    // SYNCHRONIZE access is enough to probe existence.
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
    }
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 || handle == -1 {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

pub fn new_job_id() -> String {
    format!("job_{}", cuid2::create_id())
}

pub fn mark_status(id: &str, status: JobStatus, exit_code: Option<i32>) -> Result<JobMeta> {
    let mut meta = load_meta(id)?;
    if meta.status.is_terminal() && status != JobStatus::Killed {
        // Already terminal; keep existing unless explicitly killed after.
        return Ok(meta);
    }
    meta.status = status;
    meta.exit_code = exit_code.or(meta.exit_code);
    if meta.ended_at.is_none() {
        meta.ended_at = Some(Utc::now());
    }
    save_meta(&meta)?;
    Ok(meta)
}

pub fn ensure_jobs_root() -> Result<()> {
    fs::create_dir_all(jobs_root()).with_context(|| format!("create {}", jobs_root().display()))?;
    Ok(())
}

/// Refresh a single meta if its pid died while status was still running.
pub fn refresh_if_dead(meta: &mut JobMeta) -> Result<bool> {
    if meta.status != JobStatus::Running {
        return Ok(false);
    }
    if is_pid_alive(meta.pid) {
        return Ok(false);
    }
    meta.status = JobStatus::Exited;
    meta.ended_at = Some(Utc::now());
    save_meta(meta)?;
    Ok(true)
}

/// Scope for finished-job cleanup.
#[derive(Debug, Clone)]
pub enum CleanupScope {
    /// Jobs stamped with this session id (also drops unscoped jobs in `workdir` if set).
    Session {
        session_id: String,
        /// When set, also include finished jobs in this project with no session_id
        /// (legacy jobs spawned before session stamping).
        workdir: Option<PathBuf>,
    },
    /// All jobs whose workdir matches this project.
    Project { workdir: PathBuf },
    /// Every finished job crabcode knows about.
    Global,
}

fn scope_matches(meta: &JobMeta, scope: &CleanupScope) -> bool {
    match scope {
        CleanupScope::Global => true,
        CleanupScope::Project { workdir } => {
            let wanted = canonicalize_workdir(workdir);
            canonicalize_workdir(Path::new(&meta.workdir)) == wanted
        }
        CleanupScope::Session {
            session_id,
            workdir,
        } => {
            if meta.session_id.as_deref() == Some(session_id.as_str()) {
                return true;
            }
            // Legacy: no session stamp — only if in the same project.
            if meta.session_id.is_none() {
                if let Some(wd) = workdir {
                    let wanted = canonicalize_workdir(wd);
                    return canonicalize_workdir(Path::new(&meta.workdir)) == wanted;
                }
            }
            false
        }
    }
}

/// Delete finished jobs older than `max_age` within `scope`.
/// Returns (removed, skipped_running_or_fresh_or_out_of_scope).
pub fn cleanup_finished(
    max_age: std::time::Duration,
    dry_run: bool,
    scope: &CleanupScope,
) -> Result<(usize, usize)> {
    prune_dead()?;
    let chrono_max =
        chrono::Duration::from_std(max_age).unwrap_or_else(|_| chrono::Duration::days(7));
    let cutoff = Utc::now() - chrono_max;
    let mut removed = 0usize;
    let mut skipped = 0usize;
    for meta in list_metas()? {
        if !scope_matches(&meta, scope) {
            skipped += 1;
            continue;
        }
        if matches!(meta.status, JobStatus::Running) {
            skipped += 1;
            continue;
        }
        let ended = meta.ended_at.unwrap_or(meta.started_at);
        if ended > cutoff {
            skipped += 1;
            continue;
        }
        if !dry_run {
            let dir = job_dir(&meta.id);
            let _ = fs::remove_dir_all(&dir);
        }
        removed += 1;
    }
    Ok((removed, skipped))
}

/// Global cleanup (auto / maintenance default).
pub fn cleanup_finished_global(
    max_age: std::time::Duration,
    dry_run: bool,
) -> Result<(usize, usize)> {
    cleanup_finished(max_age, dry_run, &CleanupScope::Global)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::test_env::TempState;

    #[test]
    fn save_load_roundtrip() {
        let _state = TempState::new();
        let meta = JobMeta {
            id: "job_test1".into(),
            pid: 42,
            pgid: Some(42),
            command: "echo hi".into(),
            name: "echo".into(),
            workdir: "/tmp/proj".into(),
            session_id: None,
            started_at: Utc::now(),
            ended_at: None,
            status: JobStatus::Running,
            exit_code: None,
        };
        save_meta(&meta).unwrap();
        let loaded = load_meta("job_test1").unwrap();
        assert_eq!(loaded.id, meta.id);
        assert_eq!(loaded.pid, 42);
        assert_eq!(loaded.command, "echo hi");
        assert_eq!(loaded.status, JobStatus::Running);
    }

    #[test]
    fn list_for_project_filters_by_canonical_workdir() {
        let _state = TempState::new();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let meta_a = JobMeta {
            id: "job_a".into(),
            pid: 1,
            pgid: Some(1),
            command: "a".into(),
            name: "a".into(),
            workdir: a.path().to_string_lossy().into(),
            session_id: None,
            started_at: Utc::now(),
            ended_at: None,
            status: JobStatus::Running,
            exit_code: None,
        };
        let meta_b = JobMeta {
            id: "job_b".into(),
            pid: 2,
            pgid: Some(2),
            command: "b".into(),
            name: "b".into(),
            workdir: b.path().to_string_lossy().into(),
            session_id: None,
            started_at: Utc::now(),
            ended_at: None,
            status: JobStatus::Exited,
            exit_code: Some(0),
        };
        save_meta(&meta_a).unwrap();
        save_meta(&meta_b).unwrap();

        let listed = list_for_project(a.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "job_a");

        let listed_b = list_for_project(b.path()).unwrap();
        assert_eq!(listed_b.len(), 1);
        assert_eq!(listed_b[0].id, "job_b");
    }

    #[test]
    fn prune_marks_dead_running_jobs() {
        let _state = TempState::new();
        let meta = JobMeta {
            id: "job_dead".into(),
            // Unlikely to be a live pid we own; 1 may be init and alive on unix.
            // Use a very high pid that shouldn't exist.
            pid: u32::MAX - 7,
            pgid: Some(u32::MAX - 7),
            command: "gone".into(),
            name: "gone".into(),
            workdir: "/tmp".into(),
            session_id: None,
            started_at: Utc::now(),
            ended_at: None,
            status: JobStatus::Running,
            exit_code: None,
        };
        save_meta(&meta).unwrap();
        let n = prune_dead().unwrap();
        assert!(n >= 1);
        let loaded = load_meta("job_dead").unwrap();
        assert_eq!(loaded.status, JobStatus::Exited);
        assert!(loaded.ended_at.is_some());
    }

    #[test]
    fn cleanup_finished_removes_old_keeps_running_and_fresh() {
        let _state = TempState::new();
        let now = Utc::now();
        let old_ended = now - chrono::Duration::days(10);
        let fresh_ended = now - chrono::Duration::hours(1);

        let old = JobMeta {
            id: "job_old".into(),
            pid: 1,
            pgid: None,
            command: "echo old".into(),
            name: "old".into(),
            workdir: "/tmp/proj".into(),
            session_id: None,
            started_at: old_ended,
            ended_at: Some(old_ended),
            status: JobStatus::Exited,
            exit_code: Some(0),
        };
        let fresh = JobMeta {
            id: "job_fresh".into(),
            pid: 2,
            pgid: None,
            command: "echo fresh".into(),
            name: "fresh".into(),
            workdir: "/tmp/proj".into(),
            session_id: None,
            started_at: fresh_ended,
            ended_at: Some(fresh_ended),
            status: JobStatus::Exited,
            exit_code: Some(0),
        };
        let running = JobMeta {
            id: "job_running".into(),
            pid: std::process::id(),
            pgid: None,
            command: "sleep 999".into(),
            name: "running".into(),
            workdir: "/tmp/proj".into(),
            session_id: None,
            started_at: old_ended,
            ended_at: None,
            status: JobStatus::Running,
            exit_code: None,
        };
        save_meta(&old).unwrap();
        save_meta(&fresh).unwrap();
        save_meta(&running).unwrap();

        let (removed, skipped) = cleanup_finished(
            std::time::Duration::from_secs(7 * 24 * 3600),
            false,
            &CleanupScope::Global,
        )
        .unwrap();
        assert_eq!(removed, 1);
        assert!(skipped >= 2);
        assert!(load_meta("job_old").is_err());
        assert!(load_meta("job_fresh").is_ok());
        assert!(load_meta("job_running").is_ok());
    }

    #[test]
    fn cleanup_finished_dry_run_does_not_delete() {
        let _state = TempState::new();
        let old_ended = Utc::now() - chrono::Duration::days(10);
        let old = JobMeta {
            id: "job_dry".into(),
            pid: 1,
            pgid: None,
            command: "echo dry".into(),
            name: "dry".into(),
            workdir: "/tmp/proj".into(),
            session_id: None,
            started_at: old_ended,
            ended_at: Some(old_ended),
            status: JobStatus::Failed,
            exit_code: Some(1),
        };
        save_meta(&old).unwrap();
        let (removed, _) = cleanup_finished(
            std::time::Duration::from_secs(7 * 24 * 3600),
            true,
            &CleanupScope::Global,
        )
        .unwrap();
        assert_eq!(removed, 1);
        assert!(load_meta("job_dry").is_ok());
    }
}
