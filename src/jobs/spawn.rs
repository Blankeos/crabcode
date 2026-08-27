use anyhow::{Context, Result};
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};

use super::ledger::{
    canonicalize_workdir, ensure_jobs_root, is_pid_alive, job_dir, load_meta, log_path,
    mark_status, new_job_id, save_meta, JobMeta, JobStatus,
};

pub struct SpawnDetachedOpts<'a> {
    pub command: &'a str,
    pub name: &'a str,
    pub workdir: &'a Path,
    pub session_id: Option<String>,
}

/// Spawn a detached background job that survives crabcode exit.
///
/// Unix: `/bin/sh -c command` in its own process group, stdout/stderr → output.log,
/// Child dropped immediately (lazy status updates via prune).
/// Windows: best-effort CREATE_NEW_PROCESS_GROUP + log redirect.
pub async fn spawn_detached(opts: SpawnDetachedOpts<'_>) -> Result<JobMeta> {
    spawn_detached_blocking(opts)
}

/// Sync spawn path used by CLI and by `restart_job`.
pub fn spawn_detached_blocking(opts: SpawnDetachedOpts<'_>) -> Result<JobMeta> {
    ensure_jobs_root()?;
    let id = new_job_id();
    spawn_detached_into(id, &opts, None)
}

fn spawn_detached_into(
    id: String,
    opts: &SpawnDetachedOpts<'_>,
    log_prefix: Option<&str>,
) -> Result<JobMeta> {
    ensure_jobs_root()?;

    let workdir = canonicalize_workdir(opts.workdir);
    let dir = job_dir(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("create job dir {}", dir.display()))?;

    let log = log_path(&id);
    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("open log {}", log.display()))?;
    if let Some(prefix) = log_prefix {
        write!(log_file, "{prefix}")
            .with_context(|| format!("write restart marker {}", log.display()))?;
        let _ = log_file.sync_all();
    }
    let _ = log_file.sync_all();
    let log_err = log_file
        .try_clone()
        .with_context(|| format!("clone log fd {}", log.display()))?;

    let mut cmd = if cfg!(windows) {
        let mut c = StdCommand::new("cmd");
        c.arg("/C").arg(opts.command);
        c
    } else {
        let mut c = StdCommand::new("/bin/sh");
        c.arg("-c").arg(opts.command);
        c
    };

    cmd.current_dir(&workdir);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(log_file));
    cmd.stderr(Stdio::from(log_err));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Put child in its own process group (equivalent to setpgid(0,0)).
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("spawn detached: {}", opts.command))?;

    let pid = child.id();

    // Detach: drop Child without waiting / killing.
    drop(child);

    let meta = JobMeta {
        id: id.clone(),
        pid,
        pgid: Some(pid),
        command: opts.command.to_string(),
        name: opts.name.to_string(),
        workdir: workdir.to_string_lossy().into_owned(),
        session_id: opts.session_id.clone(),
        started_at: Utc::now(),
        ended_at: None,
        status: JobStatus::Running,
        exit_code: None,
    };
    save_meta(&meta)?;
    Ok(meta)
}

/// Kill a ledger-backed job by process group (Unix) or pid (Windows), then update meta.
pub fn kill_job(id: &str) -> Result<JobMeta> {
    let mut meta = load_meta(id).with_context(|| format!("unknown job {id}"))?;

    if meta.status.is_terminal() && !is_pid_alive(meta.pid) {
        return Ok(meta);
    }

    terminate_job_process(&meta);

    meta = mark_status(id, JobStatus::Killed, None)?;
    Ok(meta)
}

/// Restart a ledger-backed job, reusing the same id.
///
/// If the process is still alive, terminates it (without finalizing as permanently killed),
/// appends a restart marker to `output.log`, then re-spawns the same command/cwd/name.
/// Already-dead jobs are still restarted (primary use case).
pub fn restart_job(id: &str) -> Result<JobMeta> {
    let meta = load_meta(id).with_context(|| format!("unknown job {id}"))?;

    if is_pid_alive(meta.pid) || meta.status == JobStatus::Running {
        terminate_job_process(&meta);
        // Wait briefly for death so ports/files can be released before respawn.
        for _ in 0..40 {
            if !is_pid_alive(meta.pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    let marker = format!(
        "\n--- restarted at {} ---\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    let opts = SpawnDetachedOpts {
        command: &meta.command,
        name: &meta.name,
        workdir: Path::new(&meta.workdir),
        session_id: meta.session_id.clone(),
    };
    spawn_detached_into(meta.id.clone(), &opts, Some(&marker))
}

/// Sync wrapper matching `kill_job` style (restart is already sync).
pub fn restart_job_blocking(id: &str) -> Result<JobMeta> {
    restart_job(id)
}

fn terminate_job_process(meta: &JobMeta) {
    let target = meta.pgid.unwrap_or(meta.pid);
    kill_process_group(target);

    // Brief grace: if still alive, escalate.
    if is_pid_alive(meta.pid) {
        std::thread::sleep(std::time::Duration::from_millis(50));
        kill_process_group(target);
    }
}

fn kill_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        // Best-effort: taskkill the process tree.
        let _ = StdCommand::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Read output.log from a byte offset. Returns (content, next_offset, file_len).
pub fn read_log_from(id: &str, since_byte: usize) -> Result<(String, usize, usize)> {
    let path = log_path(id);
    if !path.exists() {
        return Ok((String::new(), since_byte, 0));
    }
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let len = bytes.len();
    let start = since_byte.min(len);
    let slice = &bytes[start..];
    // Lossy is fine for mixed binary-ish tool output.
    let content = String::from_utf8_lossy(slice).into_owned();
    Ok((content, len, len))
}

/// Poll log growth / process death up to `wait_ms`.
pub async fn wait_for_log_growth(
    id: &str,
    since_byte: usize,
    wait_ms: u64,
) -> Result<(String, usize, bool)> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    let mut meta = load_meta(id)?;

    loop {
        let (content, next, _len) = read_log_from(id, since_byte)?;
        if !content.is_empty() {
            let alive = meta.status == JobStatus::Running && is_pid_alive(meta.pid);
            return Ok((content, next, !alive));
        }

        if meta.status == JobStatus::Running && !is_pid_alive(meta.pid) {
            let _ = super::ledger::refresh_if_dead(&mut meta);
            let (content, next, _) = read_log_from(id, since_byte)?;
            return Ok((content, next, true));
        }

        if meta.status.is_terminal() {
            let (content, next, _) = read_log_from(id, since_byte)?;
            return Ok((content, next, true));
        }

        if tokio::time::Instant::now() >= deadline {
            let (content, next, _) = read_log_from(id, since_byte)?;
            let exited = meta.status.is_terminal() || !is_pid_alive(meta.pid);
            return Ok((content, next, exited));
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(m) = load_meta(id) {
            meta = m;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::test_env::TempState;
    use std::time::Duration;

    #[tokio::test]
    async fn spawn_and_kill_sleep() {
        let _state = TempState::new();
        let meta = spawn_detached(SpawnDetachedOpts {
            command: "sleep 30",
            name: "sleep-test",
            workdir: Path::new("."),
            session_id: None,
        })
        .await
        .unwrap();
        assert!(is_pid_alive(meta.pid));
        let killed = kill_job(&meta.id).unwrap();
        assert_eq!(killed.status, JobStatus::Killed);
        // Give the OS a moment.
        std::thread::sleep(Duration::from_millis(100));
        assert!(!is_pid_alive(meta.pid) || killed.status == JobStatus::Killed);
    }

    #[tokio::test]
    async fn read_log_captures_output() {
        let _state = TempState::new();
        let meta = spawn_detached(SpawnDetachedOpts {
            command: "printf 'hello-world\\n'",
            name: "echo-test",
            workdir: Path::new("."),
            session_id: None,
        })
        .await
        .unwrap();
        // Wait for process to exit and flush.
        for _ in 0..50 {
            if !is_pid_alive(meta.pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (content, _, _) = read_log_from(&meta.id, 0).unwrap();
        assert!(
            content.contains("hello-world"),
            "log content was: {content:?}"
        );
    }

    #[test]
    fn restart_reuses_id_and_gets_new_pid() {
        let _state = TempState::new();
        let meta = spawn_detached_blocking(SpawnDetachedOpts {
            command: "sleep 30",
            name: "restart-test",
            workdir: Path::new("."),
            session_id: None,
        })
        .unwrap();
        let old_pid = meta.pid;
        assert!(is_pid_alive(old_pid));

        let restarted = restart_job(&meta.id).unwrap();
        assert_eq!(restarted.id, meta.id);
        assert_eq!(restarted.status, JobStatus::Running);
        assert!(restarted.ended_at.is_none());
        assert_ne!(restarted.pid, old_pid);
        assert!(is_pid_alive(restarted.pid));

        let (log, _, _) = read_log_from(&meta.id, 0).unwrap();
        assert!(
            log.contains("--- restarted at "),
            "missing restart marker in log: {log:?}"
        );

        let _ = kill_job(&meta.id);
    }

    #[test]
    fn restart_exited_job() {
        let _state = TempState::new();
        let meta = spawn_detached_blocking(SpawnDetachedOpts {
            command: "true",
            name: "restart-exited",
            workdir: Path::new("."),
            session_id: None,
        })
        .unwrap();
        for _ in 0..50 {
            if !is_pid_alive(meta.pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut loaded = load_meta(&meta.id).unwrap();
        let _ = crate::jobs::ledger::refresh_if_dead(&mut loaded);

        let restarted = restart_job(&meta.id).unwrap();
        assert_eq!(restarted.id, meta.id);
        assert_eq!(restarted.status, JobStatus::Running);
        assert!(is_pid_alive(restarted.pid));
        let _ = kill_job(&meta.id);
    }
}
