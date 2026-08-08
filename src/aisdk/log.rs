//! Optional host-injected logging for the AI SDK.
//!
//! Neutral by default (no-op). Hosts can install a callback via [`set_logger`].

use std::sync::OnceLock;

type LogFn = Box<dyn Fn(&str) + Send + Sync + 'static>;

static LOGGER: OnceLock<LogFn> = OnceLock::new();

/// Install a host log callback. First call wins; later calls are ignored.
pub fn set_logger<F>(f: F)
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let _ = LOGGER.set(Box::new(f));
}

/// Emit a log line if a logger is installed.
pub fn log(msg: &str) {
    if let Some(logger) = LOGGER.get() {
        logger(msg);
    }
}
