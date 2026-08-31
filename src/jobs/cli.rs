use anyhow::{Context, Result};
use chrono::Utc;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use super::ledger::{
    canonicalize_workdir, is_pid_alive, list_for_project, list_metas, load_meta, log_path,
    prune_dead, refresh_if_dead, CleanupScope, JobMeta, JobStatus,
};
use super::spawn::{kill_job, restart_job};

#[derive(Debug, Clone)]
pub struct ListOpts {
    pub all: bool,
    /// Human-friendly table (future: interactive TUI picker; for now just a pretty table)
    pub interactive: bool,
    /// Current process cwd — used when `all` is false.
    pub cwd: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub struct LogsOpts {
    pub id: String,
    pub follow: bool,
    pub tail: usize,
}

pub fn run_list(opts: ListOpts) -> Result<()> {
    crate::maintenance::run_lazy_once();
    let _ = prune_dead();
    let mut metas = if opts.all {
        list_metas()?
    } else {
        list_for_project(&opts.cwd)?
    };

    // Refresh any that died since prune raced.
    for meta in &mut metas {
        let _ = refresh_if_dead(meta);
    }

    if opts.interactive {
        print_table(&metas, opts.all);
    } else {
        print_tsv(&metas);
    }
    Ok(())
}

pub fn run_logs(opts: LogsOpts) -> Result<()> {
    let _ = prune_dead();
    let mut meta = load_meta(&opts.id).with_context(|| format!("unknown job {}", opts.id))?;
    let _ = refresh_if_dead(&mut meta);

    let path = log_path(&opts.id);
    if !path.exists() {
        eprintln!("(no log yet for {})", opts.id);
        if !opts.follow {
            return Ok(());
        }
    }

    let content = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(opts.tail);
    for line in &lines[start..] {
        println!("{line}");
    }

    if !opts.follow {
        return Ok(());
    }

    // Follow like tail -f: poll for new bytes.
    let mut offset = content.len();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        if path.exists() {
            if let Ok(bytes) = std::fs::read(&path) {
                if bytes.len() > offset {
                    let chunk = String::from_utf8_lossy(&bytes[offset..]);
                    let _ = write!(out, "{chunk}");
                    let _ = out.flush();
                    offset = bytes.len();
                }
            }
        }

        if let Ok(mut m) = load_meta(&opts.id) {
            let _ = refresh_if_dead(&mut m);
            if m.status.is_terminal() && !is_pid_alive(m.pid) {
                // Final drain.
                if let Ok(bytes) = std::fs::read(&path) {
                    if bytes.len() > offset {
                        let chunk = String::from_utf8_lossy(&bytes[offset..]);
                        let _ = write!(out, "{chunk}");
                        let _ = out.flush();
                    }
                }
                break;
            }
        }

        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

pub fn run_stop(id: &str) -> Result<()> {
    let meta = kill_job(id)?;
    println!(
        "stopped {}\t{}\tpid={}",
        meta.id,
        meta.status.as_str(),
        meta.pid
    );
    Ok(())
}

/// Stop every running job in scope (current project by default, or `--all`).
pub fn run_stop_all(all: bool, cwd: &Path) -> Result<()> {
    crate::maintenance::run_lazy_once();
    let _ = prune_dead();
    let metas = if all {
        list_metas()?
    } else {
        list_for_project(cwd)?
    };

    let running: Vec<JobMeta> = metas
        .into_iter()
        .filter(|m| matches!(m.status, JobStatus::Running) || is_pid_alive(m.pid))
        .collect();

    if running.is_empty() {
        if all {
            println!("(no running jobs)");
        } else {
            println!("(no running jobs in this project — try --all)");
        }
        return Ok(());
    }

    let mut stopped = 0usize;
    for meta in running {
        match kill_job(&meta.id) {
            Ok(m) => {
                println!("stopped {}\t{}\tpid={}", m.id, m.status.as_str(), m.pid);
                stopped += 1;
            }
            Err(err) => eprintln!("failed {}\t{err:#}", meta.id),
        }
    }
    println!("stopped {stopped}");
    Ok(())
}

pub fn run_restart(id: &str) -> Result<()> {
    let meta = restart_job(id)?;
    println!("restarted {}\tpid={}\t{}", meta.id, meta.pid, meta.name);
    Ok(())
}

fn print_tsv(metas: &[JobMeta]) {
    if metas.is_empty() {
        println!("(no jobs)");
        return;
    }
    for m in metas {
        let dur = format_duration(m);
        let status = m.status.as_str();
        let exit = m
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            m.id, status, m.pid, dur, exit, m.name
        );
    }
}

/// Human-friendly table (future: interactive TUI picker; for now just a pretty table)
fn print_table(metas: &[JobMeta], show_workdir: bool) {
    if metas.is_empty() {
        println!("(no jobs)");
        return;
    }

    println!(
        "{:<22} {:<8} {:<20} {:<10} {}",
        "ID",
        "STATUS",
        "NAME",
        "DURATION",
        if show_workdir {
            "COMMAND  (workdir)"
        } else {
            "COMMAND"
        }
    );
    for m in metas {
        let icon = status_icon(m.status);
        let dur = format_duration(m);
        let cmd = truncate(&m.command, 48);
        if show_workdir {
            println!(
                "{:<22} {icon}{:<7} {:<20} {:<10} {}  ({})",
                m.id,
                m.status.as_str(),
                truncate(&m.name, 20),
                dur,
                cmd,
                m.workdir
            );
        } else {
            println!(
                "{:<22} {icon}{:<7} {:<20} {:<10} {}",
                m.id,
                m.status.as_str(),
                truncate(&m.name, 20),
                dur,
                cmd
            );
        }
    }
}

fn status_icon(status: JobStatus) -> char {
    match status {
        JobStatus::Running => '●',
        JobStatus::Exited => '✓',
        JobStatus::Killed => '✗',
        JobStatus::Failed => '!',
    }
}

fn format_duration(m: &JobMeta) -> String {
    let end = m.ended_at.unwrap_or_else(Utc::now);
    let secs = (end - m.started_at).num_seconds().max(0) as u64;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Resolve whether `id` looks like a ledger job (vs interactive in-memory).
pub fn is_ledger_job(id: &str) -> bool {
    Path::new(&log_path(id))
        .parent()
        .map(|p| p.join("meta.json").exists())
        .unwrap_or(false)
        || load_meta(id).is_ok()
}

/// Clean scopes:
/// - default: current session (most recently updated session in this project)
/// - `--all`: current project
/// - `--global`: everything crabcode knows
pub fn run_clean(
    all: bool,
    global: bool,
    older_than: &str,
    dry_run: bool,
    cwd: &Path,
) -> Result<()> {
    if all && global {
        anyhow::bail!("use either --all (project) or --global, not both");
    }

    // `--all` / `--global` imply wipe finished jobs regardless of age.
    let max_age = if all || global {
        std::time::Duration::from_secs(0)
    } else {
        crate::maintenance::parse_age(older_than)?
    };

    let scope = if global {
        CleanupScope::Global
    } else if all {
        CleanupScope::Project {
            workdir: cwd.to_path_buf(),
        }
    } else {
        let session_id = resolve_current_session_id(cwd)?;
        CleanupScope::Session {
            session_id,
            workdir: Some(cwd.to_path_buf()),
        }
    };

    let mut m = crate::maintenance::Maintenance { tasks: vec![] };
    m.register(Box::new(crate::maintenance::tasks::JobCleanupWithAge {
        max_age,
        scope,
    }));
    let report = m.run(&crate::maintenance::RunOpts {
        dry_run,
        only: Some("jobs".into()),
    })?;
    if let Some(t) = report.tasks.first() {
        println!("{}", t.message);
    } else {
        println!("(no jobs maintenance task ran)");
    }
    Ok(())
}

/// Most recently updated session in this project workspace (for CLI default clean).
fn resolve_current_session_id(cwd: &Path) -> Result<String> {
    let history = crate::persistence::history::HistoryDAO::new_for_workspace(cwd)?;
    let sessions = history.list_sessions()?;
    let wanted = canonicalize_workdir(cwd);
    let session = sessions
        .into_iter()
        .find(|s| canonicalize_workdir(Path::new(&s.workspace_path)) == wanted)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no session found for this project — pass --all to clean the project, or --global"
            )
        })?;
    Ok(session.session_identifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_tsv_empty_prints_no_jobs() {
        // Empty TSV path should print the human empty message (agents parse rows only when present).
        let metas: Vec<JobMeta> = Vec::new();
        print_tsv(&metas);
    }
}
