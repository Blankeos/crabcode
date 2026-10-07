//! Fire-and-forget local lifecycle. No process details escape the Meridian adapter.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::{watch, Mutex};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_secs(30);
const READY_TTL: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

static RUNTIME: LazyLock<Arc<Runtime>> = LazyLock::new(|| {
    Arc::new(Runtime {
        executable: if cfg!(windows) {
            "meridian.cmd"
        } else {
            "meridian"
        }
        .into(),
        args: Vec::new(),
        state_dir: crate::persistence::get_data_dir().join("meridian"),
        start_timeout: START_TIMEOUT,
        attempts: Mutex::new(HashMap::new()),
    })
});

/// Only schedule work here: no filesystem, networking, lock acquisition or
/// process spawn on the startup thread. A caller without Tokio stays synchronous.
pub(crate) fn warmup_endpoint(base_url: &str, key: Option<String>) {
    if automatic_start_allowed() {
        schedule_warmup(RUNTIME.clone(), base_url.to_string(), key);
    }
}

pub(crate) async fn ensure_running(base_url: &str, key: Option<&str>) -> Result<()> {
    // Existing unit/integration tests must never launch the user's real server.
    // Lifecycle tests inject their own executable and private state directory.
    if !automatic_start_allowed() {
        return Ok(());
    }
    RUNTIME.ensure_running(base_url, key).await
}

fn automatic_start_allowed() -> bool {
    !cfg!(test) && std::env::var_os("CRABCODE_TEST_MODE").is_none()
}

fn schedule_warmup(
    runtime: Arc<Runtime>,
    base_url: String,
    key: Option<String>,
) -> Option<tokio::task::JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    let address = local_address(&base_url)?;
    if let Ok(attempts) = runtime.attempts.try_lock() {
        if attempts.get(&address).is_some_and(|receiver| {
            let result = receiver.borrow();
            result.is_none() || recently_ready(result.as_ref())
        }) {
            // Discovery construction happens at startup and during requests.
            // Once warm (or warming), don't even schedule another background task.
            return None;
        }
    }
    Some(handle.spawn(async move {
        if let Err(error) = runtime.ensure_running(&base_url, key.as_deref()).await {
            crate::emit_log!("Meridian background startup: {}", error);
        }
    }))
}

struct Runtime {
    executable: OsString,
    args: Vec<OsString>,
    state_dir: PathBuf,
    start_timeout: Duration,
    // Short-held registry lock; independent endpoints never wait on each other.
    // The background task, not its callers, owns startup and its file lock.
    attempts: Mutex<HashMap<SocketAddr, watch::Receiver<StartupResult>>>,
}

type StartupResult = Option<(Instant, std::result::Result<(), String>)>;

fn recently_ready(result: Option<&(Instant, std::result::Result<(), String>)>) -> bool {
    matches!(result, Some((finished_at, Ok(()))) if finished_at.elapsed() < READY_TTL)
}

impl Runtime {
    async fn ensure_running(self: &Arc<Self>, base_url: &str, key: Option<&str>) -> Result<()> {
        let Some(address) = local_address(base_url) else {
            return Ok(());
        };
        let mut receiver = {
            let mut attempts = self.attempts.lock().await;
            let mut previous_failure = None;
            let mut existing = None;
            if let Some(receiver) = attempts.get(&address) {
                let result = receiver.borrow().clone();
                if recently_ready(result.as_ref()) {
                    // Normal prompts reuse readiness: no socket, filesystem,
                    // process check or timer task. Revalidate only on demand.
                    return Ok(());
                }
                match result {
                    None => existing = Some(receiver.clone()),
                    Some((finished_at, Err(message))) if finished_at.elapsed() < RETRY_DELAY => {
                        previous_failure = Some((finished_at, Err(message)));
                    }
                    _ => {}
                }
            }
            if let Some(receiver) = existing {
                receiver
            } else {
                let (sender, receiver) = watch::channel(None);
                attempts.insert(address, receiver.clone());
                let runtime = self.clone();
                let key = key.map(str::to_owned);
                tokio::spawn(async move {
                    // One task owns the probe as well as startup. Concurrent
                    // discovery/prompt/MCP calls share the same result.
                    let completed = match listener_available(address).await {
                        Ok(true) => (Instant::now(), Ok(())),
                        Ok(false) => match previous_failure {
                            // Preserve the failure's original timestamp: calls
                            // during cooldown must not extend it indefinitely.
                            Some(failure) => failure,
                            None => {
                                let result = runtime
                                    .start(address, key.as_deref())
                                    .await
                                    .map_err(|error| error.to_string());
                                (Instant::now(), result)
                            }
                        },
                        Err(error) => (Instant::now(), Err(error.to_string())),
                    };
                    sender.send_replace(Some(completed));
                });
                receiver
            }
        };
        tokio::time::timeout(self.start_timeout, async {
            loop {
                if let Some((_, result)) = receiver.borrow_and_update().clone() {
                    return result.map_err(anyhow::Error::msg);
                }
                receiver
                    .changed()
                    .await
                    .context("Meridian startup task stopped")?;
            }
        })
        .await
        .unwrap_or_else(|_| Err(self.readiness_error(address)))
    }

    fn log_path(&self, address: SocketAddr) -> PathBuf {
        self.state_dir.join(format!("{}.log", endpoint_id(address)))
    }

    fn readiness_error(&self, address: SocketAddr) -> anyhow::Error {
        anyhow::anyhow!(
            "Meridian did not become ready within {:?}; check {} or start it manually",
            self.start_timeout,
            self.log_path(address).display()
        )
    }

    async fn start(&self, address: SocketAddr, key: Option<&str>) -> Result<()> {
        let started_at = Instant::now();
        let state_dir = self.state_dir.clone();
        let lock = tokio::task::spawn_blocking(move || -> Result<File> {
            crate::persistence::create_private_dir_all(&state_dir)?;
            private_file(state_dir.join(format!("{}.lock", endpoint_id(address))))
        })
        .await??;
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    if listener_available(address).await? {
                        return Ok(());
                    }
                    if started_at.elapsed() >= self.start_timeout {
                        return Err(self.readiness_error(address));
                    }
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(error).context("Cannot lock Meridian startup");
                }
            }
        }
        // Another Crabcode process may have finished starting it while we waited.
        if listener_available(address).await? {
            return Ok(());
        }

        let executable = self.executable.clone();
        let args = self.args.clone();
        let state_dir = self.state_dir.clone();
        let log_path = self.log_path(address);
        let key = key
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_owned);
        let (mut child, startup_lock) = tokio::task::spawn_blocking(move || -> Result<(Child, File)> {
            let log = private_file(log_path)?;
            let mut command = std::process::Command::new(executable);
            command
                .args(args)
                .current_dir(state_dir)
                .env("MERIDIAN_HOST", address.ip().to_string())
                .env("MERIDIAN_PORT", address.port().to_string())
                .env("MERIDIAN_PASSTHROUGH", "1")
                .stdin(Stdio::null())
                .stderr(Stdio::from(log.try_clone()?))
                .stdout(Stdio::from(log));
            if let Some(key) = key {
                command.env(super::API_KEY_ENV, key);
            } else {
                command.env_remove(super::API_KEY_ENV);
            }
            #[cfg(unix)]
            crate::utils::process::detach_from_terminal(&mut command);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x00000200 | 0x08000000);
            }
            let child = Command::from(command)
                .kill_on_drop(false)
                .spawn()
                .context("Cannot start Meridian; install @rynfar/meridian and make `meridian` available on PATH, or start it manually")?;
            // Startup ownership and the lock survive caller timeout/cancellation.
            Ok((child, lock))
        })
        .await??;
        crate::emit_log!("Starting local Meridian (pid {:?})", child.id());
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::bail!(
                    "Meridian exited during startup ({status}); check {}",
                    self.log_path(address).display()
                );
            }
            if listener_available(address).await? {
                // Reap while Crabcode is alive, but never kill on app exit/cancel.
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                return Ok(());
            }
            if started_at.elapsed() >= self.start_timeout {
                // A slow/hung child still exists. Retain its startup lock until
                // exit, without polling forever or allowing duplicate launches.
                tokio::spawn(async move {
                    let _lock = startup_lock;
                    let _ = child.wait().await;
                });
                return Err(self.readiness_error(address));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

fn private_file(path: PathBuf) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .context("Cannot open Meridian lifecycle file")
}

fn endpoint_id(address: SocketAddr) -> String {
    format!(
        "{}-{}",
        address.ip().to_string().replace(':', "_"),
        address.port()
    )
}

/// We can launch the native listener, not HTTPS or reverse-proxy path prefixes.
/// Never resolve a remote hostname just to decide whether it is local.
fn local_address(base_url: &str) -> Option<SocketAddr> {
    let url = reqwest::Url::parse(base_url.trim()).ok()?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(
            url.path().trim_end_matches('/'),
            "" | "/v1" | "/design/mcp" | "/v1/design/mcp"
        )
    {
        return None;
    }
    let ip = match url.host()? {
        url::Host::Domain("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
        url::Host::Ipv4(ip) if ip.is_loopback() => IpAddr::V4(ip),
        url::Host::Ipv6(ip) if ip.is_loopback() => IpAddr::V6(ip),
        _ => return None,
    };
    let port = url.port_or_known_default().filter(|port| *port != 0)?;
    Some(SocketAddr::new(ip, port))
}

async fn listener_available(address: SocketAddr) -> Result<bool> {
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => Ok(false),
        _ => {
            anyhow::bail!("Cannot check the local Meridian listener; not starting another process")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const CHILD_ENV: &str = "CRABCODE_MERIDIAN_TEST_CHILD";
    const CHILD_TEST: &str = "model::extensions::meridian::runtime::tests::meridian_child_probe";

    struct Fixture {
        runtime: Arc<Runtime>,
        address: SocketAddr,
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let dir = tempfile::tempdir().unwrap();
            // A test-harness argument marks the child without mutating parent env.
            let runtime = Arc::new(Runtime {
                executable: std::env::current_exe().unwrap().into(),
                args: vec![
                    "--exact".into(),
                    CHILD_TEST.into(),
                    "--nocapture".into(),
                    format!("--skip={CHILD_ENV}").into(),
                ],
                state_dir: dir.path().to_path_buf(),
                start_timeout: Duration::from_secs(5),
                attempts: Mutex::new(HashMap::new()),
            });
            Self {
                runtime,
                address,
                dir,
            }
        }

        fn url(&self) -> String {
            format!("http://{}/v1", self.address)
        }

        async fn stop(&self) {
            if let Ok(mut stream) = TcpStream::connect(self.address).await {
                use tokio::io::AsyncWriteExt;
                let _ = stream.write_all(b"stop").await;
            }
        }
    }

    #[test]
    fn local_endpoints_only() {
        for url in [
            "http://127.0.0.1:3456/v1/",
            "http://localhost:3456",
            "http://[::1]:3457/design/mcp",
        ] {
            assert!(local_address(url).is_some(), "{url}");
        }
        for url in [
            "https://localhost:3456/v1",
            "http://meridian.example:3456/v1",
            "http://192.168.1.2:3456/v1",
            "http://0.0.0.0:3456/v1",
            "http://localhost:3456/proxy/v1",
            "http://localhost:3456/design/mcp/other",
            "http://key@localhost:3456/v1",
            "http://localhost:3456/v1?key=secret",
            "http://localhost:3456/v1#secret",
            "http://localhost:0/v1",
            "invalid",
        ] {
            assert!(local_address(url).is_none(), "{url}");
        }
        assert_eq!(
            local_address("http://localhost:3456/v1"),
            local_address(super::super::BASE_URL)
        );
    }

    #[test]
    fn generated_design_endpoint_is_launchable() {
        let mut config = crate::config::configuration::MergedConfig::default();
        super::super::add_design_mcp(&mut config, Some(&crate::persistence::AuthConfig::Local));
        let crate::config::configuration::McpServerConfig::Remote(remote) =
            &config.mcp[super::super::DESIGN_MCP_NAME]
        else {
            panic!("remote Design endpoint")
        };
        assert_eq!(remote.url, "http://127.0.0.1:3456/v1/design/mcp");
        assert_eq!(
            local_address(&remote.url),
            local_address(super::super::BASE_URL)
        );
    }

    #[test]
    fn warmup_without_runtime_is_a_noop() {
        let fixture = Fixture::new();
        assert!(schedule_warmup(fixture.runtime.clone(), fixture.url(), None).is_none());
        assert!(!fixture.dir.path().join("starts").exists());
    }

    #[tokio::test]
    async fn warmup_returns_before_slow_child_is_ready_and_concurrent_calls_start_once() {
        let fixture = Fixture::new();
        let warmup = schedule_warmup(
            fixture.runtime.clone(),
            fixture.url(),
            Some("endpoint-key".into()),
        )
        .unwrap();
        // On this current-thread runtime the child cannot start until we yield.
        assert!(!warmup.is_finished());
        assert!(!fixture.dir.path().join("starts").exists());
        let url = fixture.url();
        let (a, b) = tokio::join!(
            fixture.runtime.ensure_running(&url, Some("endpoint-key")),
            fixture.runtime.ensure_running(&url, Some("endpoint-key")),
        );
        warmup.await.unwrap();
        fixture.stop().await;
        a.unwrap();
        b.unwrap();
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("starts")).unwrap(),
            "start\n"
        );
        let environment = std::fs::read_to_string(fixture.dir.path().join("environment")).unwrap();
        assert_eq!(
            environment,
            format!(
                "{}\n{}\n1\nendpoint-key",
                fixture.address.ip(),
                fixture.address.port()
            )
        );
    }

    #[tokio::test]
    async fn independent_runtimes_share_the_startup_lock() {
        let fixture = Fixture::new();
        let other = Arc::new(Runtime {
            executable: fixture.runtime.executable.clone(),
            args: fixture.runtime.args.clone(),
            state_dir: fixture.runtime.state_dir.clone(),
            start_timeout: fixture.runtime.start_timeout,
            attempts: Mutex::new(HashMap::new()),
        });
        let url = fixture.url();
        let (a, b) = tokio::join!(
            fixture.runtime.ensure_running(&url, None),
            other.ensure_running(&url, None),
        );
        fixture.stop().await;
        a.unwrap();
        b.unwrap();
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("starts")).unwrap(),
            "start\n"
        );
    }

    #[tokio::test]
    async fn existing_listener_and_remote_endpoint_never_spawn() {
        let fixture = Fixture::new();
        let listener = TcpListener::bind(fixture.address).unwrap();
        fixture
            .runtime
            .ensure_running(&fixture.url(), Some("wrong-key"))
            .await
            .unwrap();
        fixture
            .runtime
            .ensure_running("https://remote.example/v1", None)
            .await
            .unwrap();
        assert!(!fixture.dir.path().join("starts").exists());
        drop(listener);
    }

    #[tokio::test]
    async fn repeated_prompts_and_warmups_use_cached_readiness_without_tcp_probes() {
        let fixture = Fixture::new();
        let listener = tokio::net::TcpListener::bind(fixture.address)
            .await
            .unwrap();
        fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap();
        // Exactly one initial TCP probe for an already-running endpoint.
        drop(listener.accept().await.unwrap());
        for _ in 0..20 {
            fixture
                .runtime
                .ensure_running(&fixture.url(), None)
                .await
                .unwrap();
            assert!(schedule_warmup(fixture.runtime.clone(), fixture.url(), None).is_none());
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "warm prompts must not probe the listener again"
        );

        // Expiration is lazy: merely aging the cache must not trigger a poll.
        let (_, expired) = watch::channel(Some((Instant::now() - READY_TTL, Ok(()))));
        fixture
            .runtime
            .attempts
            .lock()
            .await
            .insert(fixture.address, expired);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        let url = fixture.url();
        let (first, second) = tokio::join!(
            fixture.runtime.ensure_running(&url, None),
            fixture.runtime.ensure_running(&url, None),
        );
        first.unwrap();
        second.unwrap();
        drop(listener.accept().await.unwrap());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "concurrent expired-cache callers must share one TCP probe"
        );
    }

    #[tokio::test]
    async fn missing_executable_has_actionable_error_and_cooldown() {
        let mut fixture = Fixture::new();
        Arc::get_mut(&mut fixture.runtime).unwrap().executable =
            fixture.dir.path().join("missing-meridian").into();
        let error = fixture
            .runtime
            .ensure_running(&fixture.url(), Some("private-key"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("PATH"));
        assert!(!format!("{error:#}").contains("private-key"));
        let error_again = fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), error_again.to_string());
        assert_eq!(fixture.runtime.attempts.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn early_exit_is_reported_without_repeated_spawns() {
        let fixture = Fixture::new();
        std::fs::write(fixture.dir.path().join("exit-before-listen"), "").unwrap();
        let error = fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("exited during startup"),
            "{error}"
        );
        assert!(error.to_string().contains(".log"));
        fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap_err();
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("starts")).unwrap(),
            "start\n"
        );
    }

    #[tokio::test]
    async fn readiness_timeout_does_not_kill_detached_child() {
        let mut fixture = Fixture::new();
        Arc::get_mut(&mut fixture.runtime).unwrap().start_timeout = Duration::from_millis(150);
        let error = fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("did not become ready"),
            "{error}"
        );
        // A second window must not spawn a replacement while the first child
        // is still initializing, even after its original caller timed out.
        let other = Arc::new(Runtime {
            executable: fixture.runtime.executable.clone(),
            args: fixture.runtime.args.clone(),
            state_dir: fixture.runtime.state_dir.clone(),
            start_timeout: Duration::from_millis(150),
            attempts: Mutex::new(HashMap::new()),
        });
        let _ = other.ensure_running(&fixture.url(), None).await;
        tokio::time::timeout(Duration::from_secs(4), async {
            while !listener_available(fixture.address).await.unwrap() {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .expect("detached child must survive readiness cancellation");
        fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap();
        fixture.stop().await;
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("starts")).unwrap(),
            "start\n"
        );
    }

    #[tokio::test]
    async fn canceling_warmup_does_not_cancel_startup_ownership() {
        let fixture = Fixture::new();
        let warmup = schedule_warmup(fixture.runtime.clone(), fixture.url(), None).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !fixture.dir.path().join("starts").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        warmup.abort();
        fixture
            .runtime
            .ensure_running(&fixture.url(), None)
            .await
            .unwrap();
        fixture.stop().await;
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("starts")).unwrap(),
            "start\n"
        );
    }

    /// A fake installed Meridian: delayed bind, env capture, detached-session check.
    #[test]
    fn meridian_child_probe() {
        if !std::env::args().any(|arg| arg == format!("--skip={CHILD_ENV}")) {
            return;
        }
        #[cfg(unix)]
        assert_eq!(
            unsafe { libc::getsid(0) },
            std::process::id() as libc::pid_t
        );
        let mut starts = OpenOptions::new()
            .create(true)
            .append(true)
            .open("starts")
            .unwrap();
        writeln!(starts, "start").unwrap();
        std::fs::write(
            "environment",
            format!(
                "{}\n{}\n{}\n{}",
                std::env::var("MERIDIAN_HOST").unwrap(),
                std::env::var("MERIDIAN_PORT").unwrap(),
                std::env::var("MERIDIAN_PASSTHROUGH").unwrap(),
                std::env::var(super::super::API_KEY_ENV).unwrap_or_default(),
            ),
        )
        .unwrap();
        if std::path::Path::new("exit-before-listen").exists() {
            std::process::exit(7);
        }
        std::thread::sleep(Duration::from_millis(350));
        let listener = TcpListener::bind(format!(
            "{}:{}",
            std::env::var("MERIDIAN_HOST").unwrap(),
            std::env::var("MERIDIAN_PORT").unwrap()
        ))
        .unwrap();
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut bytes = [0; 4];
                if stream.read(&mut bytes).unwrap_or(0) == 4 && bytes == *b"stop" {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
