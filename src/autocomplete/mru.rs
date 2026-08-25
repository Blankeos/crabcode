//! Slash-command MRU (most-recently-used) store.
//!
//! Flat per-command timestamps with a soft-decay recency score used as a
//! ranking boost during **search only**. Empty `/` menus keep registry order.
//! Persisted as the `slash_mru` prefs key in `data.db`.

use crate::persistence::{get_data_dir, PrefsDAO};
use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

/// Soft half-life (~7 days).
const HALF_LIFE_SECS: f64 = 7.0 * 86_400.0;
const MAX_ENTRIES: usize = 256;
/// Legacy sidecar; migrated into prefs once then deleted.
const LEGACY_STORE_FILE: &str = "slash_mru.json";

/// Persistent slash-command recency store.
#[derive(Debug, Clone)]
pub struct SlashMru {
    by_command: HashMap<String, u64>,
    loaded: bool,
    dirty: bool,
    persist_enabled: bool,
}

impl Default for SlashMru {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashMru {
    pub fn new() -> Self {
        Self {
            by_command: HashMap::new(),
            loaded: false,
            dirty: false,
            persist_enabled: true,
        }
    }

    /// Unit-test helper: never touches disk / DB.
    pub fn new_in_memory() -> Self {
        Self {
            by_command: HashMap::new(),
            loaded: true,
            dirty: false,
            persist_enabled: false,
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn canonicalize(name: &str) -> String {
        name.trim().trim_start_matches('/').to_ascii_lowercase()
    }

    /// Soft-decay score in \[0, 1\]. `last_used == 0` → 0.
    pub fn recency_score(last_used: u64, now: u64) -> u32 {
        if last_used == 0 || now < last_used {
            return 0;
        }
        let age = (now - last_used) as f64;
        let score = 0.5_f64.powf(age / HALF_LIFE_SECS);
        (score * 1_000_000.0).round() as u32
    }

    fn ensure_loaded(&mut self) {
        if self.loaded || !self.persist_enabled {
            return;
        }
        self.by_command = Self::load_from_prefs().unwrap_or_default();
        if self.by_command.is_empty() {
            if let Some(legacy) = Self::load_legacy_file() {
                self.by_command = legacy;
                self.dirty = true; // rewrite into prefs, then drop sidecar
                let _ = Self::delete_legacy_file();
            }
        }
        self.loaded = true;
    }

    fn load_from_prefs() -> Option<HashMap<String, u64>> {
        let dao = PrefsDAO::new().ok()?;
        dao.get_slash_mru().ok()
    }

    fn load_legacy_file() -> Option<HashMap<String, u64>> {
        let path = get_data_dir().join(LEGACY_STORE_FILE);
        let bytes = fs::read(&path).ok()?;
        #[derive(serde::Deserialize)]
        struct Legacy {
            #[serde(default)]
            by_command: HashMap<String, u64>,
        }
        serde_json::from_slice::<Legacy>(&bytes)
            .ok()
            .map(|l| l.by_command)
    }

    fn delete_legacy_file() -> std::io::Result<()> {
        let path = get_data_dir().join(LEGACY_STORE_FILE);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn last_used(&mut self, name: &str) -> u64 {
        self.ensure_loaded();
        let key = Self::canonicalize(name);
        self.by_command.get(&key).copied().unwrap_or(0)
    }

    pub fn rank_score(&mut self, name: &str) -> u32 {
        let last = self.last_used(name);
        Self::recency_score(last, Self::now_secs())
    }

    pub fn touch(&mut self, name: &str) {
        self.ensure_loaded();
        let key = Self::canonicalize(name);
        if key.is_empty() {
            return;
        }
        self.by_command.insert(key, Self::now_secs());
        if self.by_command.len() > MAX_ENTRIES {
            let mut entries: Vec<_> = self
                .by_command
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1));
            entries.truncate(MAX_ENTRIES);
            self.by_command = entries.into_iter().collect();
        }
        if self.persist_enabled {
            self.dirty = true;
        }
    }

    pub fn persist_if_dirty(&mut self) {
        if !self.dirty || !self.persist_enabled {
            return;
        }
        if Self::write_to_prefs(&self.by_command).is_ok() {
            self.dirty = false;
        }
    }

    fn write_to_prefs(by_command: &HashMap<String, u64>) -> anyhow::Result<()> {
        let dao = PrefsDAO::new()?;
        dao.set_slash_mru(by_command)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_strips_slash_and_lowercases() {
        let mut mru = SlashMru::new_in_memory();
        mru.touch("/Model");
        assert!(mru.last_used("model") > 0);
        assert_eq!(mru.last_used("/model"), mru.last_used("model"));
    }

    #[test]
    fn recency_decays_stale_entries() {
        let now = 1_700_000_000_u64;
        let recent = SlashMru::recency_score(now - 60, now);
        let week_old = SlashMru::recency_score(now - 7 * 86_400, now);
        let month_old = SlashMru::recency_score(now - 30 * 86_400, now);
        assert!(recent > week_old);
        assert!(week_old > month_old);
        assert!(month_old > 0);
        assert_eq!(SlashMru::recency_score(0, now), 0);
    }

    #[test]
    fn in_memory_never_dirties() {
        let mut mru = SlashMru::new_in_memory();
        mru.touch("plan");
        assert!(!mru.dirty);
    }

    #[test]
    fn more_recent_command_scores_higher() {
        let mut mru = SlashMru::new_in_memory();
        mru.by_command
            .insert("compact-mode".to_string(), 1_700_000_000);
        mru.by_command.insert("compact".to_string(), 1_700_000_100);
        let now = 1_700_000_200;
        let compact = SlashMru::recency_score(mru.by_command["compact"], now);
        let mode = SlashMru::recency_score(mru.by_command["compact-mode"], now);
        assert!(compact > mode);
    }
}
