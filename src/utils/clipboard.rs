use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn copy_text(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        return run_command_with_stdin("pbcopy", &[], text);
    }

    #[cfg(target_os = "linux")]
    {
        let candidates: [(&str, &[&str]); 3] = [
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ];

        for (cmd, args) in candidates {
            if run_command_with_stdin(cmd, args, text).is_ok() {
                return Ok(());
            }
        }

        bail!("no clipboard command found (tried wl-copy, xclip, xsel)");
    }

    #[cfg(target_os = "windows")]
    {
        return run_command_with_stdin("cmd", &["/C", "clip"], text);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("clipboard copy is not supported on this platform")
    }
}

fn run_command_with_stdin(cmd: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch '{}'", cmd))?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("failed to write to '{}' stdin", cmd))?;
    }

    let status = child
        .wait()
        .with_context(|| format!("failed waiting for '{}'", cmd))?;

    if status.success() {
        Ok(())
    } else {
        bail!("'{}' exited with status {}", cmd, status)
    }
}
