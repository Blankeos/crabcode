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
    #[serde(default)]
    context_window: Option<u32>,
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
            context_window: model.context_window,
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
            context_window: model.context_window,
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

#[derive(Deserialize)]
struct SnapshotHeader {
    #[serde(default)]
    schema_version: u32,
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

    let contents = fs::read_to_string(path).context("read effective model catalog")?;
    let header: SnapshotHeader =
        serde_json::from_str(&contents).context("parse effective model catalog")?;
    if header.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Ok(None);
    }

    let snapshot: Snapshot =
        serde_json::from_str(&contents).context("parse effective model catalog")?;

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
            context_window: None,
        }
    }

    #[test]
    fn publish_and_read_round_trip() {
        cleanup_test_snapshot().expect("clean test snapshot");
        publish_refreshed_models(vec![model()]).expect("publish snapshot");
        let models = models_for_dialog()
            .expect("read snapshot")
            .expect("snapshot exists");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "model-1");
        cleanup_test_snapshot().expect("clean test snapshot");
    }
}
