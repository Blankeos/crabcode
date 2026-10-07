//! Last-known endpoint descriptors for offline startup metadata lookups.
//!
//! This is deliberately separate from both models.dev and the live discovery
//! cache: a disk read must never make an endpoint look freshly discovered.

use super::Model;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 1;
type MemoryCache = HashMap<PathBuf, Option<Arc<EndpointModels>>>;
static MEMORY: OnceLock<Mutex<MemoryCache>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
pub(super) struct EndpointModels {
    pub ids: Vec<String>,
    pub metadata: HashMap<String, Model>,
    pub updated_at: u64,
}

impl EndpointModels {
    pub fn is_fresh(&self, ttl_secs: u64) -> bool {
        now().saturating_sub(self.updated_at) <= ttl_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptors(id: &str) -> EndpointModels {
        EndpointModels {
            ids: vec![id.to_string()],
            metadata: HashMap::new(),
            updated_at: now(),
        }
    }

    #[test]
    fn missing_corrupt_and_old_cache_files_are_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("catalog.json");
        assert!(load(&catalog, "missing").is_none());
        let corrupt = path(&catalog, "corrupt");
        std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
        std::fs::write(corrupt, b"not json").unwrap();
        assert!(load(&catalog, "corrupt").is_none());
        std::fs::write(
            path(&catalog, "old"),
            serde_json::to_vec(&Snapshot {
                schema_version: 0,
                models: descriptors("old"),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(load(&catalog, "old").is_none());
        // A successful discovery repairs even a memoized missing/corrupt cache.
        store(&catalog, "missing", descriptors("new")).unwrap();
        assert_eq!(load(&catalog, "missing").unwrap().ids, ["new"]);
        store(&catalog, "corrupt", descriptors("repaired")).unwrap();
        assert_eq!(load(&catalog, "corrupt").unwrap().ids, ["repaired"]);
    }

    #[test]
    fn repeated_lookups_reuse_memory_and_live_updates_replace_it() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("catalog.json");
        store(&catalog, "endpoint", descriptors("first")).unwrap();
        let first = load(&catalog, "endpoint").unwrap();
        std::fs::remove_file(path(&catalog, "endpoint")).unwrap();
        let repeated = load(&catalog, "endpoint").unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        store(&catalog, "endpoint", descriptors("second")).unwrap();
        assert_eq!(load(&catalog, "endpoint").unwrap().ids, ["second"]);
        assert_eq!(first.ids, ["first"]);
    }

    #[test]
    fn failed_disk_write_still_publishes_successful_discovery_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("catalog.json");
        store(&catalog, "endpoint", descriptors("good")).unwrap();
        std::fs::remove_file(path(&catalog, "endpoint")).unwrap();
        std::fs::remove_dir(catalog.with_extension("endpoints")).unwrap();
        std::fs::write(catalog.with_extension("endpoints"), b"blocked directory").unwrap();
        assert!(store(&catalog, "endpoint", descriptors("fresh")).is_err());
        assert_eq!(load(&catalog, "endpoint").unwrap().ids, ["fresh"]);
    }
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    schema_version: u32,
    models: EndpointModels,
}

fn path(catalog_path: &Path, key: &str) -> PathBuf {
    catalog_path
        .with_extension("endpoints")
        .join(format!("{key}.json"))
}

fn memory() -> &'static Mutex<MemoryCache> {
    MEMORY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn load(catalog_path: &Path, key: &str) -> Option<Arc<EndpointModels>> {
    let path = path(catalog_path, key);
    let mut cache = memory().lock().ok()?;
    if let Some(cached) = cache.get(&path) {
        return cached.clone();
    }
    // Missing, old or corrupt caches are harmless. Memoize misses as well so
    // rendering never repeatedly reads a missing/corrupt file.
    let models = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok())
        .filter(|snapshot| snapshot.schema_version == SCHEMA_VERSION)
        .map(|snapshot| Arc::new(snapshot.models));
    cache.insert(path, models.clone());
    models
}

pub(super) fn store(catalog_path: &Path, key: &str, models: EndpointModels) -> Result<()> {
    // Serialize in-process reads and writes so a racing first read cannot
    // overwrite freshly published memory with an older descriptor or a miss.
    let mut cache = memory().lock().ok();
    let path = path(catalog_path, key);
    let models = Arc::new(models);
    if let Some(cache) = cache.as_mut() {
        cache.insert(path.clone(), Some(models.clone()));
    }
    #[derive(Serialize)]
    struct SnapshotRef<'a> {
        schema_version: u32,
        models: &'a EndpointModels,
    }
    let dir = path.parent().context("endpoint metadata cache directory")?;
    crate::persistence::create_private_dir_all(dir)?;
    // Unique temporary files prevent concurrent Crabcode processes from
    // publishing partially written JSON or racing on a shared .tmp file.
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    let snapshot = SnapshotRef {
        schema_version: SCHEMA_VERSION,
        models: &models,
    };
    serde_json::to_writer(temp.as_file_mut(), &snapshot)?;
    temp.persist(&path)
        .context("publish endpoint model metadata")?;
    Ok(())
}

#[cfg(test)]
pub(super) fn clear_memory_for_test() {
    if let Ok(mut cache) = memory().lock() {
        cache.clear();
    }
}
