//! Native model catalog snapshot.
//!
//! `/refreshmodels` is the source-refresh boundary. Once published, `/models`
//! reads this local snapshot rather than touching models.dev or local runtimes.
//! Provider connect/disconnect only records a local catalog revision; it never
//! performs network I/O.

use crate::model::types::Model;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const SNAPSHOT_FILE: &str = "effective_catalog.json";
const SNAPSHOT_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Serialize, Deserialize)]
struct SnapshotModel {
    id: String,
    name: String,
    family: String,
    provider_id: String,
    provider_name: String,
    attachment: bool,
    structured_output: bool,
    free: bool,
    local: bool,
    reasoning_options: Vec<crate::model::reasoning::ReasoningOption>,
}

impl From<Model> for SnapshotModel {
    fn from(model: Model) -> Self {
        Self {
            id: model.id,
            name: model.name,
            family: model.family,
            provider_id: model.provider_id,
            provider_name: model.provider_name,
            attachment: model.attachment,
            structured_output: model.structured_output,
            free: model.free,
            local: model.local,
            reasoning_options: model.reasoning_options,
        }
    }
}

impl From<SnapshotModel> for Model {
    fn from(model: SnapshotModel) -> Self {
        Self {
            id: model.id,
            name: model.name,
            family: model.family,
            provider_id: model.provider_id,
            provider_name: model.provider_name,
            attachment: model.attachment,
            structured_output: model.structured_output,
            free: model.free,
            local: model.local,
            reasoning_options: model.reasoning_options,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    schema_version: u32,
    revision: u64,
    #[serde(default, alias = "built_at")]
    updated_at: u64,
    models: Vec<SnapshotModel>,
}

/// Lenient version gate for the snapshot envelope.
///
/// The old two-parse reader returned `None` for a stale `schema_version`
/// without ever deserializing the body, so an incompatible old body (missing
/// fields, different shapes) never surfaced as an error. A single typed
/// `Snapshot` parse breaks that: a stale version with an incompatible body
/// (or a missing `schema_version`, which is required on `Snapshot`) errors
/// instead of returning `None`.
///
/// This envelope preserves the old semantics with one cheap JSON scan for the
/// stale path: `schema_version` defaults to `0` when missing (stale), and the
/// body is captured as borrowed `RawValue` so incompatible old shapes never
/// fail the gate. The strict `Snapshot` parse runs only when the version is
/// current, so current-schema corruptions still surface as errors.
#[derive(Deserialize)]
struct SnapshotEnvelope<'a> {
    #[serde(default)]
    schema_version: u32,
    #[serde(borrow)]
    models: Option<&'a serde_json::value::RawValue>,
}

fn snapshot_path() -> Result<PathBuf> {
    crate::persistence::ensure_cache_dir()?;
    Ok(crate::persistence::get_cache_dir().join(SNAPSHOT_FILE))
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn load_snapshot() -> Result<Option<Snapshot>> {
    let path = snapshot_path()?;
    if !path.is_file() {
        return Ok(None);
    }

    let contents = fs::read(&path).context("read effective model catalog")?;
    // Version gate first: a single cheap scan that never validates the body
    // beyond JSON syntax. Stale/missing versions return `None` without a
    // strict body parse, matching the old header-then-body reader.
    let envelope: SnapshotEnvelope =
        serde_json::from_slice(&contents).context("parse effective model catalog")?;
    if envelope.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Ok(None);
    }

    // Current version only: strict parse so corruptions surface as errors.
    let snapshot: Snapshot =
        serde_json::from_slice(&contents).context("parse effective model catalog")?;

    Ok(Some(snapshot))
}

fn write_snapshot(snapshot: &Snapshot) -> Result<()> {
    let path = snapshot_path()?;
    let temp_path = path.with_extension("json.tmp");
    let content =
        serde_json::to_vec_pretty(snapshot).context("serialize effective model catalog")?;
    fs::write(&temp_path, content).context("write effective model catalog")?;
    fs::rename(&temp_path, path).context("publish effective model catalog")?;
    Ok(())
}

/// Returns the already-published catalog for a dialog read.
///
/// `None` means this installation has not published its first snapshot yet and
/// callers should use their legacy compatibility path once.
pub fn models_for_dialog() -> Result<Option<Vec<Model>>> {
    Ok(load_snapshot()?.map(|snapshot| snapshot.models.into_iter().map(Model::from).collect()))
}

/// Publishes models produced by an explicit source refresh.
pub fn publish_refreshed_models(models: Vec<Model>) -> Result<()> {
    let revision = load_snapshot()?
        .map(|snapshot| snapshot.revision + 1)
        .unwrap_or(1);
    write_snapshot(&Snapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        revision,
        updated_at: now_epoch_secs(),
        models: models.into_iter().map(SnapshotModel::from).collect(),
    })
}

/// Records an auth lifecycle event without refreshing any catalog source.
///
/// The effective model rows are intentionally unchanged: connected-provider
/// filtering remains a caller concern, while this revision gives later catalog
/// implementations a stable event hook.
pub fn reconcile_after_provider_change() -> Result<()> {
    let Some(mut snapshot) = load_snapshot()? else {
        return Ok(());
    };
    snapshot.revision += 1;
    snapshot.updated_at = now_epoch_secs();
    write_snapshot(&snapshot)
}

#[cfg(test)]
pub fn cleanup_test_snapshot() -> Result<()> {
    let path = snapshot_path()?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Serializes tests that mutate the snapshot file.
///
/// The snapshot path is shared by every test in the binary, so
/// file-mutating tests must hold this lock and stash/restore via
/// [`StashedSnapshot`] instead of clobbering each other.
#[cfg(test)]
static SNAPSHOT_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

#[cfg(test)]
pub fn lock_snapshot_for_test() -> std::sync::MutexGuard<'static, ()> {
    // Recover from poisoning: an unrelated test panic must not cascade into
    // every snapshot test holding this lock.
    SNAPSHOT_TEST_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Stashes the current snapshot file (if any) and restores it on drop.
#[cfg(test)]
pub struct StashedSnapshot {
    backup: Option<Vec<u8>>,
}

#[cfg(test)]
impl StashedSnapshot {
    pub fn stash() -> Result<Self> {
        let path = snapshot_path()?;
        let backup = if path.is_file() {
            Some(fs::read(&path).context("stash effective model catalog")?)
        } else {
            None
        };
        if path.exists() {
            fs::remove_file(&path).context("clear effective model catalog for test")?;
        }
        Ok(Self { backup })
    }
}

#[cfg(test)]
impl Drop for StashedSnapshot {
    fn drop(&mut self) {
        let Ok(path) = snapshot_path() else {
            return;
        };
        match &self.backup {
            Some(bytes) => {
                let _ = fs::write(&path, bytes);
            }
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model {
            id: "model-1".into(),
            name: "Model 1".into(),
            family: "test".into(),
            provider_id: "provider".into(),
            provider_name: "Provider".into(),
            attachment: false,
            structured_output: false,
            free: false,
            local: false,
            reasoning_options: Vec::new(),
        }
    }

    #[test]
    fn publish_and_read_round_trip() {
        let _guard = lock_snapshot_for_test();
        let _stashed = StashedSnapshot::stash().expect("stash snapshot");
        publish_refreshed_models(vec![model()]).expect("publish snapshot");
        let models = models_for_dialog()
            .expect("read snapshot")
            .expect("snapshot exists");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "model-1");
    }

    #[test]
    fn stale_schema_version_is_ignored() {
        // The version gate must still honor stale versions: an old snapshot
        // is treated as absent so callers take the cold path once.
        let _guard = lock_snapshot_for_test();
        let _stashed = StashedSnapshot::stash().expect("stash snapshot");
        let path = snapshot_path().expect("snapshot path");
        fs::write(
            &path,
            serde_json::json!({
                "schema_version": 1,
                "revision": 7,
                "updated_at": 0,
                "models": [],
            })
            .to_string(),
        )
        .expect("write stale snapshot");
        let models = models_for_dialog().expect("read stale snapshot");
        assert!(models.is_none());
    }

    #[test]
    fn stale_schema_with_incompatible_body_is_ignored() {
        // Old bodies may miss fields required by the current schema (or carry
        // entirely different shapes). The old header-then-body reader returned
        // `None` for a stale version without deserializing the body; the gate
        // must preserve that instead of surfacing a body error.
        let _guard = lock_snapshot_for_test();
        let _stashed = StashedSnapshot::stash().expect("stash snapshot");
        let path = snapshot_path().expect("snapshot path");
        // Missing required `SnapshotModel` fields (`name`, `provider_id`, …).
        fs::write(
            &path,
            serde_json::json!({
                "schema_version": 1,
                "revision": 7,
                "updated_at": 0,
                "models": [{ "id": "old-model" }],
            })
            .to_string(),
        )
        .expect("write stale snapshot");
        let models = models_for_dialog().expect("stale incompatible body is None");
        assert!(models.is_none());

        // Entirely different body shape (models not an array at all).
        fs::write(
            &path,
            serde_json::json!({
                "schema_version": 1,
                "revision": 7,
                "updated_at": 0,
                "models": "not-an-array",
            })
            .to_string(),
        )
        .expect("write stale snapshot");
        let models = models_for_dialog().expect("stale wrong-type body is None");
        assert!(models.is_none());
    }

    #[test]
    fn missing_version_is_ignored() {
        // `schema_version` is required on the current `Snapshot`, but the old
        // header defaulted a missing version to `0` (stale) and returned
        // `None` without a body error. Preserve that for both compatible and
        // incompatible bodies.
        let _guard = lock_snapshot_for_test();
        let _stashed = StashedSnapshot::stash().expect("stash snapshot");
        let path = snapshot_path().expect("snapshot path");
        fs::write(
            &path,
            serde_json::json!({
                "revision": 7,
                "updated_at": 0,
                "models": [],
            })
            .to_string(),
        )
        .expect("write missing-version snapshot");
        let models = models_for_dialog().expect("missing version is None");
        assert!(models.is_none());

        fs::write(
            &path,
            serde_json::json!({
                "revision": 7,
                "models": [{ "id": "old-model" }],
            })
            .to_string(),
        )
        .expect("write missing-version snapshot");
        let models = models_for_dialog().expect("missing version + bad body is None");
        assert!(models.is_none());
    }

    #[test]
    fn corrupt_snapshot_surfaces_error() {
        let _guard = lock_snapshot_for_test();
        let _stashed = StashedSnapshot::stash().expect("stash snapshot");
        let path = snapshot_path().expect("snapshot path");
        fs::write(&path, "{ not valid json").expect("write corrupt snapshot");
        assert!(models_for_dialog().is_err());
    }

    #[test]
    fn round_trip_preserves_dialog_fields() {
        let _guard = lock_snapshot_for_test();
        let _stashed = StashedSnapshot::stash().expect("stash snapshot");
        let mut dialog_model = model();
        dialog_model.provider_id = "ollama".to_string();
        dialog_model.provider_name = "Ollama (Local)".to_string();
        dialog_model.local = true;
        dialog_model.attachment = true;
        dialog_model.reasoning_options = vec![crate::model::reasoning::ReasoningOption {
            kind: "effort".to_string(),
            values: vec!["low".to_string(), "high".to_string()],
        }];
        publish_refreshed_models(vec![dialog_model]).expect("publish snapshot");
        let models = models_for_dialog()
            .expect("read snapshot")
            .expect("snapshot exists");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider_id, "ollama");
        assert!(models[0].local);
        assert!(models[0].attachment);
        assert_eq!(models[0].reasoning_options.len(), 1);
    }
}
