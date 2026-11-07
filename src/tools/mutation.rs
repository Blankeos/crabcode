use crate::tools::ToolError;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

pub const STALE_FILE_MESSAGE: &str = "File changed before write. Read it again before editing.";

lazy_static! {
    static ref FILE_LOCKS: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> = Mutex::new(HashMap::new());
}

pub struct FileMutation;

pub struct LockedFile<'a> {
    path: PathBuf,
    _guard: MutexGuard<'a, ()>,
}

#[derive(Debug, Clone)]
pub struct WriteOutcome {
    pub existed: bool,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct RemoveOutcome {
    pub existed: bool,
}

impl FileMutation {
    fn lock_for_key(key: PathBuf) -> Result<Arc<Mutex<()>>, ToolError> {
        let mut locks = FILE_LOCKS.lock().map_err(|_| {
            ToolError::Execution("Failed to acquire file lock registry".to_string())
        })?;
        Ok(locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone())
    }

    pub fn with_lock_path<R>(
        path: impl AsRef<Path>,
        f: impl FnOnce(&LockedFile<'_>) -> Result<R, ToolError>,
    ) -> Result<R, ToolError> {
        let path = path.as_ref();
        let key = canonical_lock_key(path)?;
        let lock = Self::lock_for_key(key)?;

        let guard = lock
            .lock()
            .map_err(|_| ToolError::Execution("Failed to acquire file lock".to_string()))?;
        let locked = LockedFile {
            path: path.to_path_buf(),
            _guard: guard,
        };
        f(&locked)
    }

    pub fn with_two_lock_paths<R>(
        first_path: impl AsRef<Path>,
        second_path: impl AsRef<Path>,
        f: impl FnOnce(&LockedFile<'_>, &LockedFile<'_>) -> Result<R, ToolError>,
    ) -> Result<R, ToolError> {
        let first_path = first_path.as_ref();
        let second_path = second_path.as_ref();
        let first_key = canonical_lock_key(first_path)?;
        let second_key = canonical_lock_key(second_path)?;

        if first_key == second_key {
            return Self::with_lock_path(first_path, |locked| f(locked, locked));
        }

        let first_before_second = first_key < second_key;
        let (lock_path_a, lock_path_b, key_a, key_b) = if first_before_second {
            (first_path, second_path, first_key, second_key)
        } else {
            (second_path, first_path, second_key, first_key)
        };

        let lock_a = Self::lock_for_key(key_a)?;
        let lock_b = Self::lock_for_key(key_b)?;
        let guard_a = lock_a
            .lock()
            .map_err(|_| ToolError::Execution("Failed to acquire file lock".to_string()))?;
        let guard_b = lock_b
            .lock()
            .map_err(|_| ToolError::Execution("Failed to acquire file lock".to_string()))?;
        let locked_a = LockedFile {
            path: lock_path_a.to_path_buf(),
            _guard: guard_a,
        };
        let locked_b = LockedFile {
            path: lock_path_b.to_path_buf(),
            _guard: guard_b,
        };

        if first_before_second {
            f(&locked_a, &locked_b)
        } else {
            f(&locked_b, &locked_a)
        }
    }

    pub fn write(
        path: impl AsRef<Path>,
        content: impl AsRef<[u8]>,
    ) -> Result<WriteOutcome, ToolError> {
        let content = content.as_ref();
        Self::with_lock_path(path, |locked| locked.write(content))
    }

    pub fn create_new(
        path: impl AsRef<Path>,
        content: impl AsRef<[u8]>,
    ) -> Result<WriteOutcome, ToolError> {
        let content = content.as_ref();
        Self::with_lock_path(path, |locked| locked.create_new(content))
    }

    pub fn remove(path: impl AsRef<Path>) -> Result<RemoveOutcome, ToolError> {
        Self::with_lock_path(path, |locked| locked.remove())
    }

    pub fn write_if_unchanged(
        path: impl AsRef<Path>,
        expected: &[u8],
        content: impl AsRef<[u8]>,
    ) -> Result<WriteOutcome, ToolError> {
        let content = content.as_ref();
        Self::with_lock_path(path, |locked| locked.write_if_unchanged(expected, content))
    }

    pub fn remove_if_unchanged(
        path: impl AsRef<Path>,
        expected: &[u8],
    ) -> Result<RemoveOutcome, ToolError> {
        Self::with_lock_path(path, |locked| locked.remove_if_unchanged(expected))
    }
}

impl LockedFile<'_> {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn is_file(&self) -> bool {
        self.path.is_file()
    }

    pub fn read(&self) -> Result<Vec<u8>, ToolError> {
        fs::read(&self.path)
            .map_err(|e| ToolError::Execution(format!("Failed to read file: {}", e)))
    }

    pub fn write(&self, content: impl AsRef<[u8]>) -> Result<WriteOutcome, ToolError> {
        let content = content.as_ref();
        let existed = self.path.exists();
        write_atomic_locked(&self.path, content, true)?;
        let bytes = fs::metadata(&self.path)
            .map(|m| m.len())
            .unwrap_or(content.len() as u64);
        Ok(WriteOutcome { existed, bytes })
    }

    pub fn create_new(&self, content: impl AsRef<[u8]>) -> Result<WriteOutcome, ToolError> {
        let content = content.as_ref();
        if self.path.exists() {
            return Err(ToolError::Execution(format!(
                "Refusing to overwrite existing file: {}",
                self.path.display()
            )));
        }
        write_atomic_locked(&self.path, content, false)?;
        let bytes = fs::metadata(&self.path)
            .map(|m| m.len())
            .unwrap_or(content.len() as u64);
        Ok(WriteOutcome {
            existed: false,
            bytes,
        })
    }

    pub fn write_if_unchanged(
        &self,
        expected: &[u8],
        content: impl AsRef<[u8]>,
    ) -> Result<WriteOutcome, ToolError> {
        let current = self.read()?;
        if current != expected {
            return Err(ToolError::Execution(STALE_FILE_MESSAGE.to_string()));
        }
        self.write(content)
    }

    pub fn remove(&self) -> Result<RemoveOutcome, ToolError> {
        let existed = self.path.exists();
        fs::remove_file(&self.path)
            .map_err(|e| ToolError::Execution(format!("Failed to delete file: {}", e)))?;
        Ok(RemoveOutcome { existed })
    }

    pub fn remove_if_unchanged(&self, expected: &[u8]) -> Result<RemoveOutcome, ToolError> {
        let current = self.read()?;
        if current != expected {
            return Err(ToolError::Execution(STALE_FILE_MESSAGE.to_string()));
        }
        self.remove()
    }
}

fn canonical_lock_key(path: &Path) -> Result<PathBuf, ToolError> {
    if path.exists() {
        fs::canonicalize(path).map_err(|e| {
            ToolError::Execution(format!(
                "Failed to canonicalize path for locking {}: {}",
                path.display(),
                e
            ))
        })
    } else {
        let parent = usable_parent(path);
        let canonical_parent = if parent.exists() {
            fs::canonicalize(parent).map_err(|e| {
                ToolError::Execution(format!(
                    "Failed to canonicalize parent for locking {}: {}",
                    parent.display(),
                    e
                ))
            })?
        } else {
            let mut existing = parent;
            let mut missing = Vec::new();
            while !existing.exists() {
                if let Some(name) = existing.file_name() {
                    missing.push(name.to_os_string());
                }
                existing = existing.parent().unwrap_or_else(|| Path::new("."));
            }
            let mut base = fs::canonicalize(existing).map_err(|e| {
                ToolError::Execution(format!(
                    "Failed to canonicalize parent for locking {}: {}",
                    existing.display(),
                    e
                ))
            })?;
            for part in missing.into_iter().rev() {
                base.push(part);
            }
            base
        };
        Ok(canonical_parent.join(path.file_name().ok_or_else(|| {
            ToolError::Validation(format!("Invalid file path: {}", path.display()))
        })?))
    }
}

fn usable_parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn write_atomic_locked(path: &Path, content: &[u8], overwrite: bool) -> Result<(), ToolError> {
    let parent = usable_parent(path);
    if !parent.exists() {
        fs::create_dir_all(parent)
            .map_err(|e| ToolError::Execution(format!("Failed to create directories: {}", e)))?;
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("crabcode-write");
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{}.crabcode-", file_name))
        .tempfile_in(parent)
        .map_err(|e| ToolError::Execution(format!("Failed to create temp file: {}", e)))?;

    temp.write_all(content)
        .map_err(|e| ToolError::Execution(format!("Failed to write temp file: {}", e)))?;
    temp.as_file_mut()
        .sync_all()
        .map_err(|e| ToolError::Execution(format!("Failed to flush temp file: {}", e)))?;

    if overwrite {
        temp.persist(path)
            .map_err(|e| ToolError::Execution(format!("Failed to rename file: {}", e.error)))?;
    } else {
        temp.persist_noclobber(path).map_err(|e| {
            if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                ToolError::Execution(format!(
                    "Refusing to overwrite existing file: {}",
                    path.display()
                ))
            } else {
                ToolError::Execution(format!("Failed to rename file: {}", e.error))
            }
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn write_if_unchanged_rejects_stale_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stale.txt");
        fs::write(&file, "current").unwrap();

        let err = FileMutation::write_if_unchanged(&file, b"older", b"replacement").unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("Execution error: {}", STALE_FILE_MESSAGE)
        );
        assert_eq!(fs::read_to_string(file).unwrap(), "current");
    }

    #[test]
    fn write_supports_relative_file_in_current_directory() {
        let file_name = format!("crabcode-mutation-test-{}.txt", std::process::id());
        let path = PathBuf::from(&file_name);
        let _ = fs::remove_file(&path);

        FileMutation::write(&path, b"content").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "content");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn per_file_lock_serializes_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shared.txt");
        fs::write(&file, "initial").unwrap();

        let (tx, rx) = mpsc::channel();
        let file_for_thread = file.clone();
        let handle = FileMutation::with_lock_path(&file, |_| {
            let handle = thread::spawn(move || {
                FileMutation::with_lock_path(&file_for_thread, |_| {
                    tx.send(()).unwrap();
                    Ok(())
                })
                .unwrap();
            });
            assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
            Ok(handle)
        })
        .unwrap();

        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.join().unwrap();
    }
}
