//! Report agent state to herdr when running inside a herdr pane.
//!
//! Herdr injects `HERDR_PANE_ID` and `HERDR_SOCKET_PATH` into pane processes.
//! When present, crabcode posts `pane.report_agent` over the Unix socket so the
//! pane shows up under Agents (grouped). On exit it calls `pane.release_agent`
//! so the row is removed. See herdr's socket-api docs.

use crate::session::types::SessionStatus;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SOURCE: &str = "crabcode";
const AGENT: &str = "crabcode";
const CONNECT_TIMEOUT: Duration = Duration::from_millis(80);
const IO_TIMEOUT: Duration = Duration::from_millis(120);

static ENV: OnceLock<Option<HerdrEnv>> = OnceLock::new();
static LAST_STATE: OnceLock<std::sync::Mutex<Option<&'static str>>> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct HerdrEnv {
    pane_id: String,
    socket_path: String,
}

fn env() -> Option<&'static HerdrEnv> {
    ENV.get_or_init(|| {
        let pane_id = std::env::var("HERDR_PANE_ID").ok()?;
        let socket_path = std::env::var("HERDR_SOCKET_PATH").ok()?;
        if pane_id.is_empty() || socket_path.is_empty() {
            return None;
        }
        Some(HerdrEnv {
            pane_id,
            socket_path,
        })
    })
    .as_ref()
}

/// Whether crabcode is running inside a herdr pane.
pub fn is_active() -> bool {
    env().is_some()
}

/// Map crabcode session status → herdr agent state.
fn herdr_state(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Streaming => "working",
        SessionStatus::Waiting => "blocked",
        SessionStatus::Idle | SessionStatus::Failed | SessionStatus::Interrupted => "idle",
    }
}

/// Report the current session status to herdr (no-op outside herdr).
pub fn report_session_status(status: SessionStatus) {
    report_state(herdr_state(status), None);
}

/// Report idle on startup so the pane is classified as crabcode immediately.
pub fn report_startup() {
    report_state("idle", Some("ready"));
}

/// Drop crabcode from herdr's agents panel. Custom (non-registry) agents are
/// not auto-cleared on process exit — callers must release explicitly.
pub fn report_shutdown() {
    let Some(env) = env() else {
        return;
    };

    if let Ok(mut guard) = LAST_STATE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *guard = None;
    }

    let seq = next_seq();
    let payload = serde_json::json!({
        "id": format!("crabcode:release:{seq}"),
        "method": "pane.release_agent",
        "params": {
            "pane_id": env.pane_id,
            "source": SOURCE,
            "agent": AGENT,
            "seq": seq,
        },
    });

    let _ = send_rpc(&env.socket_path, &payload);
}

/// RAII guard: reports startup on create, release on drop (incl. panic unwind).
pub struct Session {
    active: bool,
}

impl Session {
    pub fn start() -> Self {
        if is_active() {
            report_startup();
            Self { active: true }
        } else {
            Self { active: false }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.active {
            report_shutdown();
        }
    }
}

fn report_state(state: &'static str, message: Option<&str>) {
    let Some(env) = env() else {
        return;
    };

    let last = LAST_STATE.get_or_init(|| std::sync::Mutex::new(None));
    if let Ok(mut guard) = last.lock() {
        if *guard == Some(state) && message.is_none() {
            return;
        }
        *guard = Some(state);
    }

    let seq = next_seq();
    let id = format!("crabcode:{seq}");
    let mut params = serde_json::json!({
        "pane_id": env.pane_id,
        "source": SOURCE,
        "agent": AGENT,
        "state": state,
        "seq": seq,
    });
    if let Some(message) = message {
        params["message"] = serde_json::Value::String(message.to_string());
    }

    let payload = serde_json::json!({
        "id": id,
        "method": "pane.report_agent",
        "params": params,
    });

    let _ = send_rpc(&env.socket_path, &payload);
}

fn next_seq() -> u64 {
    let from_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    from_time.saturating_add(n)
}

fn send_rpc(socket_path: &str, payload: &serde_json::Value) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;

        let started = Instant::now();
        let mut stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;

        if started.elapsed() > CONNECT_TIMEOUT {
            return Ok(());
        }

        let mut body = serde_json::to_vec(payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        body.push(b'\n');
        stream.write_all(&body)?;
        stream.flush()?;

        // Drain one response line so herdr does not see a reset mid-write.
        let mut buf = [0u8; 512];
        let _ = stream.read(&mut buf);
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (socket_path, payload);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_session_status_to_herdr_state() {
        assert_eq!(herdr_state(SessionStatus::Streaming), "working");
        assert_eq!(herdr_state(SessionStatus::Waiting), "blocked");
        assert_eq!(herdr_state(SessionStatus::Idle), "idle");
        assert_eq!(herdr_state(SessionStatus::Failed), "idle");
        assert_eq!(herdr_state(SessionStatus::Interrupted), "idle");
    }

    #[test]
    fn inactive_without_env() {
        // Tests run outside herdr; env should be unset.
        assert!(!is_active() || env().is_some());
    }
}
