use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{ensure_data_dir, get_data_dir};

const MODEL_PREFS_KEY: &str = "model_preferences";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

impl PartialEq for ModelRef {
    fn eq(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id && self.model_id == other.model_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreferences {
    pub recent: Vec<ModelRef>,
    pub favorite: Vec<ModelRef>,
    pub variant: serde_json::Value,
}

impl Default for ModelPreferences {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            favorite: Vec::new(),
            variant: serde_json::json!({}),
        }
    }
}

impl ModelPreferences {
    pub fn get_active_model(&self) -> Option<&ModelRef> {
        self.recent.first()
    }

    pub fn add_recent(&mut self, provider_id: String, model_id: String) -> bool {
        let new_ref = ModelRef {
            provider_id,
            model_id,
        };

        self.recent.retain(|m| m != &new_ref);

        self.recent.insert(0, new_ref);

        if self.recent.len() > 10 {
            self.recent.truncate(10);
        }

        true
    }

    pub fn toggle_favorite(&mut self, provider_id: String, model_id: String) {
        let new_ref = ModelRef {
            provider_id,
            model_id,
        };

        if let Some(pos) = self.favorite.iter().position(|m| m == &new_ref) {
            self.favorite.remove(pos);
        } else {
            self.favorite.push(new_ref);
        }
    }

    pub fn is_favorite(&self, provider_id: &str, model_id: &str) -> bool {
        self.favorite
            .iter()
            .any(|m| m.provider_id == provider_id && m.model_id == model_id)
    }
}

#[derive(Debug)]
pub struct PrefsDAO {
    conn: Connection,
}

impl PrefsDAO {
    pub fn new() -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        let db_path = data_dir.join("data.db");

        let mut conn = Connection::open(&db_path)?;

        super::migrations::run_migrations(&mut conn)?;

        Ok(Self { conn })
    }

    fn get_pref(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM prefs WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;

        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    fn set_pref(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO prefs (key, value, updated_at) VALUES (?1, ?2, strftime('%s', 'now'))",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_model_preferences(&self) -> Result<ModelPreferences> {
        match self.get_pref(MODEL_PREFS_KEY)? {
            Some(json_str) => {
                let prefs: ModelPreferences = serde_json::from_str(&json_str)?;
                Ok(prefs)
            }
            None => Ok(ModelPreferences::default()),
        }
    }

    pub fn set_model_preferences(&self, prefs: &ModelPreferences) -> Result<()> {
        let json_str = serde_json::to_string(prefs)?;
        self.set_pref(MODEL_PREFS_KEY, &json_str)
    }

    pub fn get_active_model(&self) -> Result<Option<(String, String)>> {
        let prefs = self.get_model_preferences()?;
        if let Some(model_ref) = prefs.get_active_model() {
            Ok(Some((
                model_ref.provider_id.clone(),
                model_ref.model_id.clone(),
            )))
        } else {
            Ok(None)
        }
    }

    pub fn set_active_model(&self, provider_id: String, model_id: String) -> Result<()> {
        let mut prefs = self.get_model_preferences()?;
        prefs.add_recent(provider_id, model_id);
        self.set_model_preferences(&prefs)
    }

    pub fn toggle_favorite(&self, provider_id: String, model_id: String) -> Result<bool> {
        let mut prefs = self.get_model_preferences()?;
        let was_favorite = prefs.is_favorite(&provider_id, &model_id);
        prefs.toggle_favorite(provider_id, model_id);
        self.set_model_preferences(&prefs)?;
        Ok(!was_favorite)
    }

    pub fn is_favorite(&self, provider_id: &str, model_id: &str) -> Result<bool> {
        let prefs = self.get_model_preferences()?;
        Ok(prefs.is_favorite(provider_id, model_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_dao() -> PrefsDAO {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();
        super::super::migrations::run_migrations(&conn).unwrap();
        PrefsDAO { conn }
    }

    #[test]
    fn test_model_preferences_default() {
        let prefs = ModelPreferences::default();
        assert!(prefs.recent.is_empty());
        assert!(prefs.favorite.is_empty());
    }

    #[test]
    fn test_model_preferences_get_active_model_empty() {
        let prefs = ModelPreferences::default();
        assert!(prefs.get_active_model().is_none());
    }

    #[test]
    fn test_model_preferences_add_recent() {
        let mut prefs = ModelPreferences::default();
        prefs.add_recent("provider1".to_string(), "model1".to_string());

        assert_eq!(prefs.recent.len(), 1);
        assert_eq!(prefs.recent[0].provider_id, "provider1");
        assert_eq!(prefs.recent[0].model_id, "model1");
    }

    #[test]
    fn test_model_preferences_add_recent_moves_to_front() {
        let mut prefs = ModelPreferences::default();
        prefs.add_recent("provider1".to_string(), "model1".to_string());
        prefs.add_recent("provider2".to_string(), "model2".to_string());
        prefs.add_recent("provider1".to_string(), "model1".to_string());

        assert_eq!(prefs.recent.len(), 2);
        assert_eq!(prefs.recent[0].provider_id, "provider1");
        assert_eq!(prefs.recent[1].provider_id, "provider2");
    }

    #[test]
    fn test_model_preferences_add_recent_limits_to_10() {
        let mut prefs = ModelPreferences::default();
        for i in 0..15 {
            prefs.add_recent("provider".to_string(), format!("model{}", i));
        }

        assert_eq!(prefs.recent.len(), 10);
    }

    #[test]
    fn test_model_preferences_toggle_favorite() {
        let mut prefs = ModelPreferences::default();
        prefs.toggle_favorite("provider1".to_string(), "model1".to_string());

        assert_eq!(prefs.favorite.len(), 1);
        assert!(prefs.is_favorite("provider1", "model1"));

        prefs.toggle_favorite("provider1".to_string(), "model1".to_string());
        assert_eq!(prefs.favorite.len(), 0);
        assert!(!prefs.is_favorite("provider1", "model1"));
    }

    #[test]
    fn test_model_ref_equality() {
        let ref1 = ModelRef {
            provider_id: "provider1".to_string(),
            model_id: "model1".to_string(),
        };
        let ref2 = ModelRef {
            provider_id: "provider1".to_string(),
            model_id: "model1".to_string(),
        };
        let ref3 = ModelRef {
            provider_id: "provider2".to_string(),
            model_id: "model1".to_string(),
        };

        assert_eq!(ref1, ref2);
        assert_ne!(ref1, ref3);
    }
}
