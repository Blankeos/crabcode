use crate::jobs::ledger::{
    canonicalize_workdir, list_for_project, load_meta, refresh_if_dead, JobMeta,
    JobStatus as LedgerStatus,
};
use crate::jobs::spawn::{self as jobs_spawn, SpawnDetachedOpts};
use crate::llm::{BackgroundJobEventKind, ChunkMessage, ChunkSender};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const RING_CAPACITY: usize = 1024 * 1024; // ~1MB
const MAX_FINISHED_JOBS: usize = 50;
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Background,
    Interactive,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Interactive => "interactive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Exited,
    Killed,
    Failed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Exited => "exited",
            JobStatus::Killed => "killed",
            JobStatus::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

impl From<LedgerStatus> for JobStatus {
    fn from(s: LedgerStatus) -> Self {
        match s {
            LedgerStatus::Running => Self::Running,
            LedgerStatus::Exited => Self::Exited,
            LedgerStatus::Killed => Self::Killed,
            LedgerStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessJobSnapshot {
    pub id: String,
    pub kind: JobKind,
    pub command: String,
    pub description: String,
    pub workdir: PathBuf,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub started_at: Instant,
    pub ended_at: Option<Instant>,
    pub bytes_total: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SpawnedJob {
    pub task_id: String,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct JobOutput {
    pub text: String,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    /// Absolute end offset of the returned slice in the logical output stream.
    pub end_offset: u64,
    /// Alias for `end_offset` — absolute next read offset for clients.
    pub next_offset: u64,
    /// Total bytes in the logical stream (log file or ring end offset).
    pub bytes_total: u64,
    /// Whether the in-memory ring dropped older bytes (ledger logs are not truncated).
    pub truncated: bool,
}

struct RingBuffer {
    capacity: usize,
    data: Vec<u8>,
    /// Absolute offset of `data[0]` in the logical stream.
    start_offset: u64,
    truncated: bool,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            data: Vec::new(),
            start_offset: 0,
            truncated: false,
        }
    }

    fn end_offset(&self) -> u64 {
        self.start_offset + self.data.len() as u64
    }

    fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.data.extend_from_slice(chunk);
        if self.data.len() > self.capacity {
            let overflow = self.data.len() - self.capacity;
            self.data.drain(..overflow);
            self.start_offset += overflow as u64;
            self.truncated = true;
        }
    }

    /// Bytes from absolute `since` offset (clamped to retained window).
    fn slice_from(&self, since: u64) -> (Vec<u8>, u64) {
        let start_idx = since.saturating_sub(self.start_offset) as usize;
        let start_idx = start_idx.min(self.data.len());
        (self.data[start_idx..].to_vec(), self.end_offset())
    }
}

/// In-memory interactive job (dies with the app). Background jobs live in the ledger.
struct ProcessJob {
    id: String,
    kind: JobKind,
    command: String,
    description: String,
    workdir: PathBuf,
    status: JobStatus,
    exit_code: Option<i32>,
    started_at: Instant,
    ended_at: Option<Instant>,
    ring: RingBuffer,
    /// Last absolute offset returned by `output` for since-last semantics.
    last_read_offset: u64,
    /// Background: ledger pid/pgid. Interactive: unused.
    pid: Option<u32>,
    process_group_id: Option<u32>,
    notify: Arc<Notify>,
}

impl ProcessJob {
    fn snapshot(&self) -> ProcessJobSnapshot {
        ProcessJobSnapshot {
            id: self.id.clone(),
            kind: self.kind,
            command: self.command.clone(),
            description: self.description.clone(),
            workdir: self.workdir.clone(),
            status: self.status,
            exit_code: self.exit_code,
            started_at: self.started_at,
            ended_at: self.ended_at,
            bytes_total: self.ring.end_offset(),
            truncated: self.ring.truncated,
        }
    }
}

struct ProcessRegistryInner {
    jobs: HashMap<String, ProcessJob>,
    order: Vec<String>,
    /// Legacy counter retained for tests / uniqueness of cache keys if needed.
    bg_counter: AtomicU64,
    pty_counter: AtomicU64,
    notifier: Option<ChunkSender>,
    /// Project workdir used when merging ledger jobs into list().
    workdir: PathBuf,
}

pub struct ProcessRegistry {
    inner: Arc<Mutex<ProcessRegistryInner>>,
    /// Cached running job count for the TUI chip — never block the UI to compute this.
    running_count: Arc<AtomicUsize>,
}

impl Default for ProcessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessRegistry {
    pub fn new() -> Self {
        let workdir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::with_workdir(workdir)
    }

    pub fn with_workdir(workdir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProcessRegistryInner {
                jobs: HashMap::new(),
                order: Vec::new(),
                bg_counter: AtomicU64::new(0),
                pty_counter: AtomicU64::new(0),
                notifier: None,
                workdir: canonicalize_workdir(&workdir),
            })),
            running_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Non-blocking running-job count for the status chip (updated on spawn/kill/list).
    pub fn running_count(&self) -> usize {
        self.running_count.load(Ordering::Relaxed)
    }

    fn recompute_running_count(inner: &ProcessRegistryInner, counter: &AtomicUsize) {
        let n = inner
            .jobs
            .values()
            .filter(|j| j.status == JobStatus::Running)
            .count();
        counter.store(n, Ordering::Relaxed);
    }

    fn lock_inner(&self) -> MutexGuard<'_, ProcessRegistryInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Update the project workdir used for ledger filtering (e.g. on workspace change).
    pub async fn set_workdir(&self, workdir: impl AsRef<Path>) {
        let mut inner = self.lock_inner();
        inner.workdir = canonicalize_workdir(workdir.as_ref());
    }

    pub fn set_workdir_blocking(&self, workdir: impl AsRef<Path>) {
        let path = canonicalize_workdir(workdir.as_ref());
        let mut inner = self.lock_inner();
        inner.workdir = path;
    }

    /// Attach a notifier. Prefer calling before any spawn; uses a short lock if the
    /// mutex is briefly held.
    pub fn with_notifier(self, sender: ChunkSender) -> Self {
        {
            let mut guard = self.lock_inner();
            guard.notifier = Some(sender);
        }
        self
    }

    pub async fn set_notifier(&self, sender: Option<ChunkSender>) {
        let mut guard = self.lock_inner();
        guard.notifier = sender;
    }

    fn next_pty_id(inner: &ProcessRegistryInner) -> String {
        let n = inner.pty_counter.fetch_add(1, Ordering::Relaxed) + 1;
        format!("pty_{n}")
    }

    fn emit(notifier: &Option<ChunkSender>, job_id: &str, event: BackgroundJobEventKind) {
        if let Some(tx) = notifier {
            let _ = tx.send(ChunkMessage::BackgroundJobEvent {
                job_id: job_id.to_string(),
                event,
            });
        }
    }

    fn prune_finished(inner: &mut ProcessRegistryInner) {
        // Only prune in-memory interactive jobs. Background ledger entries stay on disk.
        let finished: Vec<String> = inner
            .order
            .iter()
            .filter(|id| {
                inner
                    .jobs
                    .get(*id)
                    .map(|j| j.kind == JobKind::Interactive && j.status.is_terminal())
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        let excess = finished.len().saturating_sub(MAX_FINISHED_JOBS);
        for id in finished.into_iter().take(excess) {
            inner.jobs.remove(&id);
            inner.order.retain(|x| x != &id);
        }
    }

    fn ledger_to_snapshot(meta: &JobMeta, now: Instant) -> ProcessJobSnapshot {
        let started_at = {
            let age = (chrono::Utc::now() - meta.started_at)
                .to_std()
                .unwrap_or(Duration::ZERO);
            now.checked_sub(age).unwrap_or(now)
        };
        let ended_at = meta.ended_at.map(|ended| {
            let age = (chrono::Utc::now() - ended)
                .to_std()
                .unwrap_or(Duration::ZERO);
            now.checked_sub(age).unwrap_or(now)
        });
        let log_len = std::fs::metadata(crate::jobs::ledger::log_path(&meta.id))
            .map(|m| m.len())
            .unwrap_or(0);
        ProcessJobSnapshot {
            id: meta.id.clone(),
            kind: JobKind::Background,
            command: meta.command.clone(),
            description: meta.name.clone(),
            workdir: PathBuf::from(&meta.workdir),
            status: meta.status.into(),
            exit_code: meta.exit_code,
            started_at,
            ended_at,
            bytes_total: log_len,
            truncated: false,
        }
    }

    fn cache_background(inner: &mut ProcessRegistryInner, meta: &JobMeta) {
        if inner.jobs.contains_key(&meta.id) {
            if let Some(job) = inner.jobs.get_mut(&meta.id) {
                job.status = meta.status.into();
                job.exit_code = meta.exit_code;
                job.pid = Some(meta.pid);
                job.process_group_id = meta.pgid.or(Some(meta.pid));
                if job.status.is_terminal() && job.ended_at.is_none() {
                    job.ended_at = Some(Instant::now());
                }
            }
            return;
        }
        let now = Instant::now();
        let snap_times = Self::ledger_to_snapshot(meta, now);
        let job = ProcessJob {
            id: meta.id.clone(),
            kind: JobKind::Background,
            command: meta.command.clone(),
            description: meta.name.clone(),
            workdir: PathBuf::from(&meta.workdir),
            status: meta.status.into(),
            exit_code: meta.exit_code,
            started_at: snap_times.started_at,
            ended_at: snap_times.ended_at,
            ring: RingBuffer::new(RING_CAPACITY),
            last_read_offset: 0,
            pid: Some(meta.pid),
            process_group_id: meta.pgid.or(Some(meta.pid)),
            notify: Arc::new(Notify::new()),
        };
        inner.order.push(meta.id.clone());
        inner.jobs.insert(meta.id.clone(), job);
    }

    /// Spawn a detached background job that survives crabcode quit.
    ///
    /// `cancel` is accepted for API compatibility but does **not** kill the child —
    /// background jobs are OS-detached and managed via the on-disk ledger.
    pub async fn spawn_background(
        &self,
        command: impl Into<String>,
        description: impl Into<String>,
        workdir: impl AsRef<Path>,
        session_id: Option<String>,
        _cancel: CancellationToken,
    ) -> Result<SpawnedJob, String> {
        let command = command.into();
        let description = description.into();
        let workdir = workdir.as_ref().to_path_buf();

        let meta = jobs_spawn::spawn_detached(SpawnDetachedOpts {
            command: &command,
            name: &description,
            workdir: &workdir,
            session_id,
        })
        .await
        .map_err(|e| format!("Failed to spawn background process: {e}"))?;

        let task_id = meta.id.clone();
        let pid = meta.pid;

        {
            let mut inner = self.lock_inner();
            // Keep registry workdir in sync with spawn if unset / first use.
            if inner.workdir.as_os_str().is_empty() {
                inner.workdir = canonicalize_workdir(&workdir);
            }
            Self::cache_background(&mut inner, &meta);
            // bump legacy counter so tests observing it still move
            let _ = inner.bg_counter.fetch_add(1, Ordering::Relaxed);
            Self::recompute_running_count(&inner, &self.running_count);
            Self::emit(
                &inner.notifier,
                &task_id,
                BackgroundJobEventKind::Started {
                    command: command.clone(),
                    description: description.clone(),
                    kind: JobKind::Background.as_str().to_string(),
                },
            );
        }

        crate::emit_log!(
            "[PROCESS_REGISTRY] spawned detached background task_id={} pid={} cmd={}",
            task_id,
            pid,
            command
        );

        Ok(SpawnedJob {
            task_id,
            pid: Some(pid),
        })
    }

    /// Poll job output.
    ///
    /// Background jobs read from the on-disk `output.log`. Interactive jobs use the
    /// in-memory ring (fed by the PTY / tool).
    ///
    /// `since_byte`: absolute byte offset. If `None`, returns bytes since the last
    /// successful `output` call for this job.
    ///
    /// `wait_ms`: if set, block until new bytes arrive, the job exits, or timeout.
    pub async fn output(
        &self,
        task_id: &str,
        wait_ms: Option<u64>,
        since_byte: Option<u64>,
    ) -> Result<JobOutput, String> {
        // Prefer ledger path for job_* ids (and any id with a meta.json).
        if let Ok(mut meta) = load_meta(task_id) {
            let _ = refresh_if_dead(&mut meta);
            {
                let mut inner = self.lock_inner();
                Self::cache_background(&mut inner, &meta);
            }

            let since = {
                let inner = self.lock_inner();
                let job = inner.jobs.get(task_id);
                since_byte.unwrap_or_else(|| job.map(|j| j.last_read_offset).unwrap_or(0))
            };

            if let Some(ms) = wait_ms {
                let (text, next, _exited) =
                    jobs_spawn::wait_for_log_growth(task_id, since as usize, ms)
                        .await
                        .map_err(|e| e.to_string())?;
                let meta = load_meta(task_id).map_err(|e| e.to_string())?;
                {
                    let mut inner = self.lock_inner();
                    let notifier = inner.notifier.clone();
                    let mut emit_status = None;
                    if let Some(job) = inner.jobs.get_mut(task_id) {
                        job.last_read_offset = next as u64;
                        job.status = meta.status.into();
                        job.exit_code = meta.exit_code;
                        if job.status.is_terminal() && job.ended_at.is_none() {
                            job.ended_at = Some(Instant::now());
                            job.notify.notify_waiters();
                            emit_status = Some(job.status);
                        }
                    }
                    Self::recompute_running_count(&inner, &self.running_count);
                    if let Some(status) = emit_status {
                        match status {
                            JobStatus::Killed => {
                                Self::emit(&notifier, task_id, BackgroundJobEventKind::Killed)
                            }
                            JobStatus::Exited | JobStatus::Failed => Self::emit(
                                &notifier,
                                task_id,
                                BackgroundJobEventKind::Exited {
                                    exit_code: meta.exit_code,
                                },
                            ),
                            JobStatus::Running => {}
                        }
                    }
                }
                return Ok(JobOutput {
                    text,
                    status: meta.status.into(),
                    exit_code: meta.exit_code,
                    end_offset: next as u64,
                    next_offset: next as u64,
                    bytes_total: next as u64,
                    truncated: false,
                });
            }

            let (text, next, _) =
                jobs_spawn::read_log_from(task_id, since as usize).map_err(|e| e.to_string())?;
            {
                let mut inner = self.lock_inner();
                if let Some(job) = inner.jobs.get_mut(task_id) {
                    job.last_read_offset = next as u64;
                    job.status = meta.status.into();
                    job.exit_code = meta.exit_code;
                }
                Self::recompute_running_count(&inner, &self.running_count);
            }
            return Ok(JobOutput {
                text,
                status: meta.status.into(),
                exit_code: meta.exit_code,
                end_offset: next as u64,
                next_offset: next as u64,
                bytes_total: next as u64,
                truncated: false,
            });
        }

        // Interactive / in-memory path.
        let notify = {
            let inner = self.lock_inner();
            let job = inner
                .jobs
                .get(task_id)
                .ok_or_else(|| format!("Unknown task_id: {task_id}"))?;
            job.notify.clone()
        };

        let deadline = wait_ms.map(|ms| Instant::now() + Duration::from_millis(ms));

        loop {
            {
                let mut inner = self.lock_inner();
                let job = inner
                    .jobs
                    .get_mut(task_id)
                    .ok_or_else(|| format!("Unknown task_id: {task_id}"))?;

                let since = since_byte.unwrap_or(job.last_read_offset);
                let (bytes, end_offset) = job.ring.slice_from(since);
                let has_new = !bytes.is_empty();
                let terminal = job.status.is_terminal();

                if has_new || terminal || deadline.is_none() {
                    job.last_read_offset = end_offset;
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    return Ok(JobOutput {
                        text,
                        status: job.status,
                        exit_code: job.exit_code,
                        end_offset,
                        next_offset: end_offset,
                        bytes_total: job.ring.end_offset(),
                        truncated: job.ring.truncated,
                    });
                }
            }

            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let mut inner = self.lock_inner();
                    let job = inner
                        .jobs
                        .get_mut(task_id)
                        .ok_or_else(|| format!("Unknown task_id: {task_id}"))?;
                    let since = since_byte.unwrap_or(job.last_read_offset);
                    let (bytes, end_offset) = job.ring.slice_from(since);
                    job.last_read_offset = end_offset;
                    return Ok(JobOutput {
                        text: String::from_utf8_lossy(&bytes).into_owned(),
                        status: job.status,
                        exit_code: job.exit_code,
                        end_offset,
                        next_offset: end_offset,
                        bytes_total: job.ring.end_offset(),
                        truncated: job.ring.truncated,
                    });
                }
                tokio::select! {
                    _ = notify.notified() => {}
                    _ = tokio::time::sleep(remaining.min(OUTPUT_POLL_INTERVAL)) => {}
                }
            }
        }
    }

    pub async fn kill(&self, task_id: &str) -> Result<(), String> {
        // Ledger-backed background job.
        if load_meta(task_id).is_ok() {
            let meta = jobs_spawn::kill_job(task_id).map_err(|e| e.to_string())?;
            let mut inner = self.lock_inner();
            Self::cache_background(&mut inner, &meta);
            if let Some(job) = inner.jobs.get_mut(task_id) {
                job.status = JobStatus::Killed;
                job.ended_at = Some(Instant::now());
                job.notify.notify_waiters();
            }
            Self::recompute_running_count(&inner, &self.running_count);
            let notifier = inner.notifier.clone();
            Self::emit(&notifier, task_id, BackgroundJobEventKind::Killed);
            crate::emit_log!("[PROCESS_REGISTRY] killed ledger job task_id={}", task_id);
            return Ok(());
        }

        let mut inner = self.lock_inner();
        {
            let job = inner
                .jobs
                .get_mut(task_id)
                .ok_or_else(|| format!("Unknown task_id: {task_id}"))?;

            if job.status.is_terminal() {
                return Ok(());
            }

            // Interactive: mark killed; actual PTY teardown is handled by the session tool.
            if let Some(pgid) = job.process_group_id {
                kill_process_group(pgid);
            }
            job.status = JobStatus::Killed;
            job.ended_at = Some(Instant::now());
            job.notify.notify_waiters();
        }
        Self::recompute_running_count(&inner, &self.running_count);
        let notifier = inner.notifier.clone();
        Self::emit(&notifier, task_id, BackgroundJobEventKind::Killed);
        crate::emit_log!("[PROCESS_REGISTRY] killed task_id={}", task_id);
        Ok(())
    }

    /// Restart a ledger-backed background job (same id / command / cwd).
    pub async fn restart(&self, task_id: &str) -> Result<crate::jobs::ledger::JobMeta, String> {
        let meta = jobs_spawn::restart_job(task_id).map_err(|e| e.to_string())?;
        let mut inner = self.lock_inner();
        Self::cache_background(&mut inner, &meta);
        Self::recompute_running_count(&inner, &self.running_count);
        let notifier = inner.notifier.clone();
        Self::emit(
            &notifier,
            task_id,
            BackgroundJobEventKind::Started {
                command: meta.command.clone(),
                description: meta.name.clone(),
                kind: JobKind::Background.as_str().to_string(),
            },
        );
        crate::emit_log!(
            "[PROCESS_REGISTRY] restarted ledger job task_id={} pid={}",
            task_id,
            meta.pid
        );
        Ok(meta)
    }

    /// Sync restart for TUI / CLI helpers.
    pub fn restart_blocking(&self, task_id: &str) -> Result<crate::jobs::ledger::JobMeta, String> {
        self.block_on_async(self.restart(task_id))
    }

    /// Merge interactive (memory) + project ledger jobs into sorted snapshots.
    ///
    /// Does **not** call `prune_dead()` — age GC is handled by lazy maintenance,
    /// and pid liveness is checked lazily in get / output / kill / CLI.
    fn build_list_snapshots(
        inner: &ProcessRegistryInner,
        ledger_metas: &[JobMeta],
        now: Instant,
    ) -> Vec<ProcessJobSnapshot> {
        let mut out: Vec<ProcessJobSnapshot> = Vec::new();

        // Interactive (memory-only) first from order, then any ledger snapshots.
        for id in inner.order.iter().rev() {
            if let Some(job) = inner.jobs.get(id) {
                if job.kind == JobKind::Interactive {
                    out.push(job.snapshot());
                }
            }
        }
        for meta in ledger_metas {
            out.push(Self::ledger_to_snapshot(meta, now));
        }

        // Stable-ish: running first, then newest.
        out.sort_by(|a, b| {
            let ar = a.status == JobStatus::Running;
            let br = b.status == JobStatus::Running;
            match (ar, br) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.started_at.cmp(&a.started_at),
            }
        });
        out
    }

    fn hydrate_ledger_into_cache(inner: &mut ProcessRegistryInner, ledger_metas: &[JobMeta]) {
        let mut newly_exited = Vec::new();
        for meta in ledger_metas {
            let prev_running = inner
                .jobs
                .get(&meta.id)
                .map(|j| j.status == JobStatus::Running)
                .unwrap_or(false);
            Self::cache_background(inner, meta);
            if prev_running && meta.status != LedgerStatus::Running {
                newly_exited.push((
                    meta.id.clone(),
                    JobStatus::from(meta.status),
                    meta.exit_code,
                ));
            }
        }
        let notifier = inner.notifier.clone();
        for (id, status, exit_code) in newly_exited {
            match status {
                JobStatus::Killed => Self::emit(&notifier, &id, BackgroundJobEventKind::Killed),
                _ => Self::emit(&notifier, &id, BackgroundJobEventKind::Exited { exit_code }),
            }
        }
        Self::prune_finished(inner);
    }

    /// Merge interactive (memory) + project ledger jobs.
    pub async fn list(&self) -> Vec<ProcessJobSnapshot> {
        crate::maintenance::run_lazy_once();
        // Do NOT call prune_dead() here — it walks the FS and probes pids, which
        // freezes the TUI when list is reached via block_on from the event loop.

        let workdir = {
            let inner = self.lock_inner();
            inner.workdir.clone()
        };

        let ledger_metas = list_for_project(&workdir).unwrap_or_default();
        let now = Instant::now();

        let mut inner = self.lock_inner();
        Self::hydrate_ledger_into_cache(&mut inner, &ledger_metas);
        Self::recompute_running_count(&inner, &self.running_count);
        Self::build_list_snapshots(&inner, &ledger_metas, now)
    }

    pub async fn get(&self, task_id: &str) -> Option<ProcessJobSnapshot> {
        if let Ok(mut meta) = load_meta(task_id) {
            let _ = refresh_if_dead(&mut meta);
            let mut inner = self.lock_inner();
            Self::cache_background(&mut inner, &meta);
            Self::recompute_running_count(&inner, &self.running_count);
            return Some(Self::ledger_to_snapshot(&meta, Instant::now()));
        }
        let inner = self.lock_inner();
        inner.jobs.get(task_id).map(|j| j.snapshot())
    }

    fn block_on_async<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| handle.block_on(fut))
        } else {
            futures::executor::block_on(fut)
        }
    }

    /// Sync snapshot list for the TUI (App event loop is sync).
    ///
    /// Uses a short std Mutex lock + ledger meta reads only — no `block_on`, no `prune_dead`.
    pub fn list_blocking(&self) -> Vec<ProcessJobSnapshot> {
        crate::maintenance::run_lazy_once(); // already spawns a thread — OK
                                             // DO NOT call prune_dead() here

        let workdir = {
            let inner = self.lock_inner();
            inner.workdir.clone()
        };
        let ledger_metas = list_for_project(&workdir).unwrap_or_default();
        let now = Instant::now();

        let mut inner = self.lock_inner();
        Self::hydrate_ledger_into_cache(&mut inner, &ledger_metas);
        Self::recompute_running_count(&inner, &self.running_count);
        Self::build_list_snapshots(&inner, &ledger_metas, now)
    }

    /// Sync get for the TUI — load_meta + optional refresh_if_dead, no block_on.
    pub fn get_blocking(&self, task_id: &str) -> Option<ProcessJobSnapshot> {
        if let Ok(mut meta) = load_meta(task_id) {
            let _ = refresh_if_dead(&mut meta);
            let mut inner = self.lock_inner();
            Self::cache_background(&mut inner, &meta);
            Self::recompute_running_count(&inner, &self.running_count);
            return Some(Self::ledger_to_snapshot(&meta, Instant::now()));
        }
        let inner = self.lock_inner();
        inner.jobs.get(task_id).map(|j| j.snapshot())
    }

    /// Deprecated: prefer [`Self::running_count`] (atomic, never lists).
    pub fn running_count_blocking(&self) -> usize {
        self.running_count()
    }

    /// Sync kill for the TUI.
    pub fn kill_blocking(&self, task_id: &str) -> Result<(), String> {
        self.block_on_async(self.kill(task_id))
    }

    /// Sync output poll for the jobs dialog detail view.
    ///
    /// Prefer the fast path (no wait) without `block_on`: interactive ring or ledger log.
    /// When `wait_ms` is Some, falls back to the async waiter via `block_in_place`.
    pub fn output_blocking(
        &self,
        task_id: &str,
        wait_ms: Option<u64>,
        since_byte: Option<u64>,
    ) -> Result<JobOutput, String> {
        if wait_ms.is_none() {
            // Interactive in-memory ring
            {
                let mut inner = self.lock_inner();
                if let Some(job) = inner.jobs.get_mut(task_id) {
                    if job.kind == JobKind::Interactive {
                        let since = since_byte.unwrap_or(job.last_read_offset);
                        let (bytes, end_offset) = job.ring.slice_from(since);
                        job.last_read_offset = end_offset;
                        return Ok(JobOutput {
                            text: String::from_utf8_lossy(&bytes).into_owned(),
                            status: job.status,
                            exit_code: job.exit_code,
                            end_offset,
                            next_offset: end_offset,
                            bytes_total: job.ring.end_offset(),
                            truncated: job.ring.truncated,
                        });
                    }
                }
            }

            // Background ledger log
            if let Ok(mut meta) = load_meta(task_id) {
                let _ = refresh_if_dead(&mut meta);
                let since = {
                    let mut inner = self.lock_inner();
                    Self::cache_background(&mut inner, &meta);
                    Self::recompute_running_count(&inner, &self.running_count);
                    since_byte.unwrap_or_else(|| {
                        inner
                            .jobs
                            .get(task_id)
                            .map(|j| j.last_read_offset)
                            .unwrap_or(0)
                    })
                };
                let (text, next, _) = jobs_spawn::read_log_from(task_id, since as usize)
                    .map_err(|e| e.to_string())?;
                {
                    let mut inner = self.lock_inner();
                    if let Some(job) = inner.jobs.get_mut(task_id) {
                        job.last_read_offset = next as u64;
                        job.status = meta.status.into();
                        job.exit_code = meta.exit_code;
                    }
                    Self::recompute_running_count(&inner, &self.running_count);
                }
                return Ok(JobOutput {
                    text,
                    status: meta.status.into(),
                    exit_code: meta.exit_code,
                    end_offset: next as u64,
                    next_offset: next as u64,
                    bytes_total: next as u64,
                    truncated: false,
                });
            }
            return Err(format!("Unknown task_id: {task_id}"));
        }

        self.block_on_async(self.output(task_id, wait_ms, since_byte))
    }

    /// Register an interactive PTY job so it appears in the jobs list.
    pub async fn register_interactive(
        &self,
        command: impl Into<String>,
        description: impl Into<String>,
        workdir: impl AsRef<Path>,
    ) -> String {
        let command = command.into();
        let description = description.into();
        let workdir = workdir.as_ref().to_path_buf();

        let mut inner = self.lock_inner();
        let task_id = Self::next_pty_id(&inner);
        let job = ProcessJob {
            id: task_id.clone(),
            kind: JobKind::Interactive,
            command: command.clone(),
            description: description.clone(),
            workdir,
            status: JobStatus::Running,
            exit_code: None,
            started_at: Instant::now(),
            ended_at: None,
            ring: RingBuffer::new(RING_CAPACITY),
            last_read_offset: 0,
            pid: None,
            process_group_id: None,
            notify: Arc::new(Notify::new()),
        };
        Self::emit(
            &inner.notifier,
            &task_id,
            BackgroundJobEventKind::Started {
                command,
                description,
                kind: JobKind::Interactive.as_str().to_string(),
            },
        );
        inner.order.push(task_id.clone());
        inner.jobs.insert(task_id.clone(), job);
        Self::recompute_running_count(&inner, &self.running_count);
        task_id
    }

    pub async fn mark_interactive_status(
        &self,
        id: &str,
        status: JobStatus,
        exit_code: Option<i32>,
    ) {
        let mut inner = self.lock_inner();
        if let Some(job) = inner.jobs.get_mut(id) {
            job.status = status;
            job.exit_code = exit_code;
            job.ended_at = Some(Instant::now());
            job.notify.notify_waiters();
            let notifier = inner.notifier.clone();
            match status {
                JobStatus::Killed => Self::emit(&notifier, id, BackgroundJobEventKind::Killed),
                JobStatus::Exited | JobStatus::Failed => {
                    Self::emit(&notifier, id, BackgroundJobEventKind::Exited { exit_code })
                }
                JobStatus::Running => {}
            }
        }
        Self::prune_finished(&mut inner);
        Self::recompute_running_count(&inner, &self.running_count);
    }

    pub async fn unregister(&self, id: &str) {
        let mut inner = self.lock_inner();
        inner.jobs.remove(id);
        inner.order.retain(|existing| existing != id);
        Self::recompute_running_count(&inner, &self.running_count);
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    if pid > 0 {
        unsafe {
            let _ = libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn kill_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::test_env::TempState;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_echo_background_captures_output() {
        let _state = TempState::new();
        let workdir = tempfile::tempdir().unwrap();
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());
        let spawned = registry
            .spawn_background(
                "echo hello_bg",
                "echo",
                workdir.path(),
                None,
                CancellationToken::new(),
            )
            .await
            .expect("spawn");

        assert!(spawned.task_id.starts_with("job_"));

        let mut text = String::new();
        for _ in 0..50 {
            let out = registry
                .output(&spawned.task_id, Some(100), Some(0))
                .await
                .expect("output");
            text = out.text;
            if text.contains("hello_bg") || out.status.is_terminal() {
                break;
            }
        }
        assert!(
            text.contains("hello_bg"),
            "expected echo output in log, got: {text:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_background_job() {
        let _state = TempState::new();
        let workdir = tempfile::tempdir().unwrap();
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());
        let spawned = registry
            .spawn_background(
                "sleep 30",
                "sleep",
                workdir.path(),
                None,
                CancellationToken::new(),
            )
            .await
            .expect("spawn");

        registry.kill(&spawned.task_id).await.expect("kill");
        let snap = registry.get(&spawned.task_id).await.expect("get");
        assert_eq!(snap.status, JobStatus::Killed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_merges_ledger_for_project() {
        let _state = TempState::new();
        let workdir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());

        let a = registry
            .spawn_background(
                "echo a",
                "a",
                workdir.path(),
                None,
                CancellationToken::new(),
            )
            .await
            .expect("spawn a");
        let b = ProcessRegistry::with_workdir(other.path().to_path_buf())
            .spawn_background("echo b", "b", other.path(), None, CancellationToken::new())
            .await
            .expect("spawn b");

        tokio::time::sleep(Duration::from_millis(100)).await;
        let list = registry.list().await;
        assert!(
            list.iter().any(|j| j.id == a.task_id),
            "project job missing: {:?}",
            list.iter().map(|j| &j.id).collect::<Vec<_>>()
        );
        assert!(
            list.iter().all(|j| j.id != b.task_id),
            "other project job should not appear"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_background_job_reuses_id() {
        let _state = TempState::new();
        let workdir = tempfile::tempdir().unwrap();
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());
        let spawned = registry
            .spawn_background(
                "sleep 30",
                "restart-me",
                workdir.path(),
                None,
                CancellationToken::new(),
            )
            .await
            .expect("spawn");
        let old_meta = crate::jobs::ledger::load_meta(&spawned.task_id).expect("meta");
        let restarted = registry.restart(&spawned.task_id).await.expect("restart");
        assert_eq!(restarted.id, spawned.task_id);
        assert_ne!(restarted.pid, old_meta.pid);
        let snap = registry.get(&spawned.task_id).await.expect("get after");
        assert_eq!(snap.status, JobStatus::Running);
        let _ = registry.kill(&spawned.task_id).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interactive_register_and_mark() {
        let registry = ProcessRegistry::new();
        let id = registry
            .register_interactive("vim", "edit", PathBuf::from("/tmp"))
            .await;
        assert!(id.starts_with("pty_"));
        registry
            .mark_interactive_status(&id, JobStatus::Exited, Some(0))
            .await;
        let snap = registry.get(&id).await.unwrap();
        assert_eq!(snap.status, JobStatus::Exited);
        assert_eq!(snap.kind, JobKind::Interactive);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_count_tracks_spawn_and_kill() {
        let _state = TempState::new();
        let workdir = tempfile::tempdir().unwrap();
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());
        assert_eq!(registry.running_count(), 0);

        let spawned = registry
            .spawn_background(
                "sleep 30",
                "count-me",
                workdir.path(),
                None,
                CancellationToken::new(),
            )
            .await
            .expect("spawn");
        assert_eq!(registry.running_count(), 1);

        registry.kill(&spawned.task_id).await.expect("kill");
        assert_eq!(registry.running_count(), 0);
    }

    #[test]
    fn list_blocking_works_without_tokio_runtime() {
        let _state = TempState::new();
        let workdir = tempfile::tempdir().unwrap();
        let registry = ProcessRegistry::with_workdir(workdir.path().to_path_buf());
        // Must not panic / deadlock when called from a plain sync context.
        let list = registry.list_blocking();
        assert!(list.is_empty() || list.iter().all(|j| !j.id.is_empty()));
        assert_eq!(registry.running_count_blocking(), registry.running_count());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_count_tracks_interactive() {
        let registry = ProcessRegistry::new();
        assert_eq!(registry.running_count(), 0);
        let id = registry
            .register_interactive("vim", "edit", PathBuf::from("/tmp"))
            .await;
        assert_eq!(registry.running_count(), 1);
        registry
            .mark_interactive_status(&id, JobStatus::Exited, Some(0))
            .await;
        assert_eq!(registry.running_count(), 0);
    }
}
