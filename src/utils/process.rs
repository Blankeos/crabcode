use std::os::unix::process::CommandExt;
use std::process::Command;

/// Isolate a non-interactive child from the TUI's controlling terminal.
///
/// Redirecting stdio and setting a process group are not enough: descendants
/// can still open `/dev/tty` and change its modes or foreground process group.
/// `setsid` removes that access and also creates a PID-led process group, so
/// existing group-based cancellation continues to work. Callers must redirect
/// stdio separately and must not also set `process_group(0)` (which makes
/// `setsid` fail because the child is already a process group leader).
pub fn detach_from_terminal(command: &mut Command) {
    // SAFETY: only the async-signal-safe setsid syscall and errno retrieval run
    // in the child after fork; the closure does not allocate or acquire locks.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::jobs::{spawn, test_env::TempState};
    use crate::tools::{bash::BashTool, ToolContext, ToolHandler};
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::path::Path;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    const PROBE_ENV: &str = "CRABCODE_TEST_JOB_TTY_PROBE";
    const PROBE_TEST: &str = "utils::process::tests::non_interactive_job_pty_probe";
    const PROBE_COMMAND: &str = "if (exec 3<>/dev/tty) 2>/dev/null; then printf 'tty-attached'; else printf 'tty-detached'; fi";

    #[test]
    fn non_interactive_jobs_cannot_access_controlling_terminal() {
        // Running under a PTY is essential: ordinary CI stdin often has no
        // controlling terminal, which would hide inherited-session bugs.
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("probe.log");
        let mut command = CommandBuilder::new("sh");
        command.args([
            "-c",
            "exec \"$CRABCODE_TEST_BINARY\" --exact \"$CRABCODE_TEST_NAME\" --nocapture --test-threads=1 >\"$CRABCODE_TEST_LOG\" 2>&1",
        ]);
        command.env("CRABCODE_TEST_BINARY", std::env::current_exe().unwrap());
        command.env("CRABCODE_TEST_NAME", PROBE_TEST);
        command.env("CRABCODE_TEST_LOG", &log);
        command.env(PROBE_ENV, "1");
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);

        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                if let Some(pid) = child.process_id() {
                    unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
                }
                let _ = child.kill();
                let _ = child.wait();
                panic!("PTY probe hung or was suspended by terminal job control");
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let output = std::fs::read_to_string(log).unwrap();
        assert!(status.success(), "PTY probe failed: {output}");
        assert!(output.contains("job-tty-isolation-ok"), "{output}");
    }

    #[tokio::test]
    async fn non_interactive_job_pty_probe() {
        if std::env::var_os(PROBE_ENV).is_none() {
            return;
        }
        let tty = std::fs::File::open("/dev/tty").expect("probe must have a controlling terminal");
        use std::os::fd::AsRawFd;
        let foreground_group = unsafe { libc::tcgetpgrp(tty.as_raw_fd()) };
        assert_eq!(foreground_group, unsafe { libc::getpgrp() });

        let ctx =
            ToolContext::from_cancel_token("session", "message", "Build", CancellationToken::new());
        let result = BashTool::new()
            .execute(serde_json::json!({"command": PROBE_COMMAND}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            result.output, "tty-detached",
            "foreground job inherited tty"
        );

        let _state = TempState::new();
        let meta = spawn::spawn_detached_blocking(spawn::SpawnDetachedOpts {
            command: PROBE_COMMAND,
            name: "tty-isolation-test",
            workdir: Path::new("."),
            session_id: None,
        })
        .unwrap();
        let (output, _, exited) = spawn::wait_for_log_growth(&meta.id, 0, 2000).await.unwrap();
        assert_eq!(output, "tty-detached", "background job inherited tty");
        // Wait for the reaper before restarting so the old incarnation cannot
        // write output into the new incarnation's log.
        if !exited {
            for _ in 0..100 {
                if !crate::jobs::ledger::is_pid_alive(meta.pid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        spawn::restart_job(&meta.id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (output, _, _) = spawn::read_log_from(&meta.id, 0).unwrap();
            assert!(
                !output.contains("tty-attached"),
                "restarted job inherited tty"
            );
            if output.matches("tty-detached").count() == 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "restarted job did not produce output"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            unsafe { libc::tcgetpgrp(tty.as_raw_fd()) },
            foreground_group
        );
        println!("job-tty-isolation-ok");
    }
}
