//! Slash-command MRU (most-recently-used) store.
//!
//! Mirrors grok-build's `slash/mru.rs`: flat per-command timestamps with a
//! soft-decay recency score used as a ranking boost during **search only**.
//! Empty `/` menus keep registry order unchanged.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Soft half-life (~7 days). Matches grok-build.
const HALF_LIFE_SECS: f64 = 7.0 * 86_400.0;
const MAX_ENTRIES: usize = 256;
const STORE_FILE: &str = "slash_mru.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct MruFile {
    /// Canonical command name (no leading `/`) → unix seconds of last use.
    #[serde(default)]
    by_command: HashMap<String, u64>,
}

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

    /// Tests / ephemeral: never touches disk.
    pub fn new_in_memory() -> Self {
        Self {
            by_command: HashMap::new(),
            loaded: true,
            dirty: false,
            persist_enabled: false,
        }
    }

    fn store_path() -> PathBuf {
        crate::persistence::get_data_dir().join(STORE_FILE)
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn normalize_command(name: &str) -> Option<String> {
        let trimmed = name.trim().trim_start_matches('/').trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_ascii_lowercase())
    }

    /// Soft-decay score: recent ≫ week-old ≫ month-old; never-used → 0.
    pub fn recency_score(last_used: u64, now: u64) -> u64 {
        if last_used == 0 || now < last_used {
            return 0;
        }
        let age = (now - last_used) as f64;
        let score = (1_000_000.0_f64) * (-age / HALF_LIFE_SECS).exp();
        score.round().clamp(0.0, u64::MAX as f64) as u64
    }

    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        if !self.persist_enabled {
            self.loaded = true;
            return;
        }
        let path = Self::store_path();
        match fs::read(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.loaded = true;
            }
            Err(_) => {
                // Best-effort: empty store, skip disk for this session.
                self.loaded = true;
                self.persist_enabled = false;
            }
            Ok(bytes) => match serde_json::from_slice::<MruFile>(&bytes) {
                Ok(file) => {
                    self.by_command = file.by_command;
                    self.trim_to_cap();
                    self.loaded = true;
                }
                Err(_) => {
                    // Corrupt file: start fresh.
                    self.loaded = true;
                }
            },
        }
    }

    fn trim_to_cap(&mut self) {
        if self.by_command.len() <= MAX_ENTRIES {
            return;
        }
        let mut entries: Vec<(String, u64)> = self.by_command.drain().collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(MAX_ENTRIES);
        self.by_command = entries.into_iter().collect();
    }

    /// Record use of a canonical command name.
    pub fn touch(&mut self, command_name: &str) {
        let Some(cmd) = Self::normalize_command(command_name) else {
            return;
        };
        self.ensure_loaded();
        self.by_command.insert(cmd, Self::now_secs());
        self.trim_to_cap();
        if self.persist_enabled {
            self.dirty = true;
        }
    }

    pub fn last_used(&mut self, command_name: &str) -> u64 {
        let Some(cmd) = Self::normalize_command(command_name) else {
            return 0;
        };
        self.ensure_loaded();
        self.by_command.get(&cmd).copied().unwrap_or(0)
    }

    pub fn rank_score(&mut self, command_name: &str) -> u64 {
        let ts = self.last_used(command_name);
        Self::recency_score(ts, Self::now_secs())
    }

    /// Persist if dirty. Best-effort; clears dirty on success.
    pub fn persist_if_dirty(&mut self) {
        if !self.persist_enabled || !self.dirty {
            return;
        }
        if crate::persistence::ensure_data_dir().is_err() {
            return;
        }
        let file = MruFile {
            by_command: self.by_command.clone(),
        };
        let path = Self::store_path();
        if let Ok(bytes) = serde_json::to_vec(&file) {
            if fs::write(&path, bytes).is_ok() {
                self.dirty = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_records_flat_by_command() {
        let mut mru = SlashMru::new_in_memory();
        mru.touch("compact");
        mru.touch("/compact-mode");
        assert!(mru.last_used("compact") > 0);
        assert!(mru.last_used("compact-mode") > 0);
        assert_eq!(mru.last_used("/compact"), mru.last_used("compact"));
    }

    #[test]
    fn strips_leading_slash() {
        let mut mru = SlashMru::new_in_memory();
        mru.touch("/model");
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
