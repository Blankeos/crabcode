//! Built-in maintenance task registrations (jobs today; workspaces later).

use std::time::Duration;

use anyhow::Result;

use crate::jobs::ledger::CleanupScope;

use super::{Maintenance, MaintenanceTask, RunOpts, TaskReport};

pub struct JobCleanup {
    pub max_age: Duration,
    pub scope: CleanupScope,
}

impl MaintenanceTask for JobCleanup {
    fn id(&self) -> &'static str {
        "jobs"
    }

    fn description(&self) -> &'static str {
        "Remove finished (exited/killed/failed) background jobs older than max_age"
    }

    fn auto_run(&self) -> bool {
        // Auto/lazy path is always global age-based GC.
        matches!(self.scope, CleanupScope::Global)
    }

    fn run(&self, opts: &RunOpts) -> Result<TaskReport> {
        let (removed, skipped) =
            crate::jobs::ledger::cleanup_finished(self.max_age, opts.dry_run, &self.scope)?;
        let scope_label = match &self.scope {
            CleanupScope::Session { .. } => "session",
            CleanupScope::Project { .. } => "project",
            CleanupScope::Global => "global",
        };
        let message = if opts.dry_run {
            format!("would remove {removed} finished job(s) ({scope_label}); skipped {skipped}")
        } else {
            format!("removed {removed} finished job(s) ({scope_label}); skipped {skipped}")
        };
        Ok(TaskReport {
            task_id: self.id().into(),
            removed,
            skipped,
            message,
        })
    }
}

/// Job cleanup with an explicit max age + scope (used by `crabcode jobs clean`).
pub struct JobCleanupWithAge {
    pub max_age: Duration,
    pub scope: CleanupScope,
}

impl MaintenanceTask for JobCleanupWithAge {
    fn id(&self) -> &'static str {
        "jobs"
    }

    fn description(&self) -> &'static str {
        "Remove finished background jobs older than the given max_age (scoped)"
    }

    fn auto_run(&self) -> bool {
        false
    }

    fn run(&self, opts: &RunOpts) -> Result<TaskReport> {
        JobCleanup {
            max_age: self.max_age,
            scope: self.scope.clone(),
        }
        .run(opts)
    }
}

pub fn register_defaults(m: &mut Maintenance) {
    m.register(Box::new(JobCleanup {
        max_age: Duration::from_secs(7 * 24 * 3600),
        scope: CleanupScope::Global,
    }));
    // Future:
    // m.register(Box::new(WorkspaceCleanup { … }));
}
