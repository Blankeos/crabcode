//! Extensible maintenance tasks (job GC today; workspaces/caches later).
//!
//! Jobs register cleanup here — cleanup does **not** live only inside `src/jobs/`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;

pub mod tasks;

/// A named maintenance task. Easy to add more later (workspaces, caches, …).
pub trait MaintenanceTask: Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// When true, included in [`Maintenance::run_lazy`] / [`run_lazy_once`].
    fn auto_run(&self) -> bool {
        false
    }
    /// Run the task. Should be fast / nonblocking enough for CLI + lazy UI paths.
    fn run(&self, opts: &RunOpts) -> Result<TaskReport>;
}

#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// If true, don't delete — just report what would happen.
    pub dry_run: bool,
    /// Only run this task id (None = all).
    pub only: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskReport {
    pub task_id: String,
    pub removed: usize,
    pub skipped: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub tasks: Vec<TaskReport>,
}

/// Registry of all maintenance tasks.
pub struct Maintenance {
    pub(crate) tasks: Vec<Box<dyn MaintenanceTask>>,
}

impl Maintenance {
    pub fn with_defaults() -> Self {
        let mut m = Self { tasks: vec![] };
        crate::maintenance::tasks::register_defaults(&mut m);
        m
    }

    pub fn register(&mut self, task: Box<dyn MaintenanceTask>) {
        self.tasks.push(task);
    }

    pub fn list(&self) -> Vec<(&str, &str, bool)> {
        self.tasks
            .iter()
            .map(|t| (t.id(), t.description(), t.auto_run()))
            .collect()
    }

    pub fn run(&self, opts: &RunOpts) -> Result<RunReport> {
        let mut report = RunReport::default();
        for task in &self.tasks {
            if let Some(ref only) = opts.only {
                if task.id() != only.as_str() {
                    continue;
                }
            }
            let mut tr = task.run(opts)?;
            if tr.task_id.is_empty() {
                tr.task_id = task.id().to_string();
            }
            report.tasks.push(tr);
        }
        Ok(report)
    }

    /// Lazy auto path: run cheap tasks that are safe to invoke often (e.g. job GC).
    /// Must be fast and ignore errors.
    pub fn run_lazy(&self) {
        let opts = RunOpts {
            dry_run: false,
            only: None,
        };
        for task in &self.tasks {
            if !task.auto_run() {
                continue;
            }
            let _ = task.run(&opts);
        }
    }
}

static LAZY_RAN: AtomicBool = AtomicBool::new(false);

/// Run auto-run maintenance tasks at most once per process.
pub fn run_lazy_once() {
    if LAZY_RAN.swap(true, Ordering::Relaxed) {
        return;
    }
    // Best-effort background GC — must not block UI/CLI list on FS deletes.
    std::thread::Builder::new()
        .name("crabcode-maintenance".into())
        .spawn(|| {
            Maintenance::with_defaults().run_lazy();
        })
        .ok();
}

/// CLI: `crabcode maintenance run [--only …] [--dry-run]`
pub fn cli_run(only: Option<String>, dry_run: bool) -> Result<()> {
    let m = Maintenance::with_defaults();
    let report = m.run(&RunOpts { dry_run, only })?;
    if report.tasks.is_empty() {
        println!("(no matching maintenance tasks)");
        return Ok(());
    }
    for t in report.tasks {
        println!("{}: {}", t.task_id, t.message);
    }
    Ok(())
}

/// CLI: `crabcode maintenance list`
pub fn cli_list() -> Result<()> {
    let m = Maintenance::with_defaults();
    for (id, desc, auto) in m.list() {
        let auto_s = if auto { "auto" } else { "manual" };
        println!("{id}\t{auto_s}\t{desc}");
    }
    Ok(())
}

/// Parse simple age strings: `7d`, `24h`, `30m`, or bare days as integer.
pub fn parse_age(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty age");
    }
    if let Ok(days) = s.parse::<u64>() {
        return Ok(Duration::from_secs(days.saturating_mul(24 * 3600)));
    }
    let (num, unit) = s.split_at(s.len() - 1);
    let n: u64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid age '{s}' (expected Nd / Nh / Nm or days)"))?;
    Ok(match unit {
        "d" | "D" => Duration::from_secs(n.saturating_mul(24 * 3600)),
        "h" | "H" => Duration::from_secs(n.saturating_mul(3600)),
        "m" | "M" => Duration::from_secs(n.saturating_mul(60)),
        _ => anyhow::bail!("invalid age unit in '{s}' (use d/h/m)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::ledger::{save_meta, JobMeta, JobStatus};
    use crate::jobs::test_env::TempState;
    use chrono::Utc;

    #[test]
    fn parse_age_supports_d_h_m_and_bare_days() {
        assert_eq!(parse_age("7d").unwrap(), Duration::from_secs(7 * 24 * 3600));
        assert_eq!(parse_age("24h").unwrap(), Duration::from_secs(24 * 3600));
        assert_eq!(parse_age("30m").unwrap(), Duration::from_secs(30 * 60));
        assert_eq!(parse_age("7").unwrap(), Duration::from_secs(7 * 24 * 3600));
    }

    #[test]
    fn maintenance_run_only_jobs() {
        let _state = TempState::new();
        let old_ended = Utc::now() - chrono::Duration::days(10);
        save_meta(&JobMeta {
            id: "job_m".into(),
            pid: 1,
            pgid: None,
            command: "echo".into(),
            name: "m".into(),
            workdir: "/tmp".into(),
            session_id: None,
            started_at: old_ended,
            ended_at: Some(old_ended),
            status: JobStatus::Exited,
            exit_code: Some(0),
        })
        .unwrap();

        let m = Maintenance::with_defaults();
        let report = m
            .run(&RunOpts {
                dry_run: false,
                only: Some("jobs".into()),
            })
            .unwrap();
        assert_eq!(report.tasks.len(), 1);
        assert_eq!(report.tasks[0].task_id, "jobs");
        assert_eq!(report.tasks[0].removed, 1);
    }

    #[test]
    fn run_lazy_once_returns_immediately() {
        // Should not block the caller on FS deletes (spawned background thread).
        let start = std::time::Instant::now();
        run_lazy_once();
        run_lazy_once(); // second call is a no-op
        assert!(
            start.elapsed() < std::time::Duration::from_millis(200),
            "run_lazy_once blocked the caller"
        );
    }
}
