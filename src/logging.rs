use anyhow::Result;
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(enabled: bool) {
    LOGGING_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    LOGGING_ENABLED.load(Ordering::Relaxed)
}

#[allow(unused_must_use)]
pub fn log(message: &str) -> Result<()> {
    if !enabled() {
        return Ok(());
    }

    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_line = format!("[{}] {}\n", timestamp, message);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("app.log")?;

    file.write_all(log_line.as_bytes())?;
    Ok(())
}

#[macro_export]
macro_rules! emit_log {
    ($message:expr) => {{
        if $crate::logging::enabled() {
            let _ = $crate::logging::log($message);
        }
    }};
    ($fmt:expr, $($arg:tt)*) => {{
        if $crate::logging::enabled() {
            let _ = $crate::logging::log(&format!($fmt, $($arg)*));
        }
    }};
}
