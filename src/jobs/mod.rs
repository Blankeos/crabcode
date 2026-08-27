//! Survive-quit background jobs: on-disk ledger + detached OS processes.
//!
//! No daemon. Jobs are keyed under `~/.local/state/crabcode/jobs/<id>/` and
//! filtered by project `workdir`. Interactive PTY jobs stay in-memory only.

pub mod cli;
pub mod ledger;
pub mod spawn;

#[cfg(test)]
pub(crate) mod test_env {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Global lock so concurrent tests don't clobber `XDG_STATE_HOME`.
    pub fn lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub struct TempState {
        _guard: MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
    }

    impl TempState {
        pub fn new() -> Self {
            let guard = lock();
            let dir = tempfile::tempdir().expect("tempdir");
            let prev = std::env::var_os("XDG_STATE_HOME");
            std::env::set_var("XDG_STATE_HOME", dir.path());
            Self {
                _guard: guard,
                dir,
                prev,
            }
        }

        pub fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    impl Drop for TempState {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }

    #[allow(dead_code)]
    pub fn data_dir(state: &TempState) -> PathBuf {
        state.path().join("crabcode")
    }
}
