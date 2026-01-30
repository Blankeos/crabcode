use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use super::{ensure_data_dir, get_data_dir, migrations::run_migrations};

const MAX_HISTORY_SIZE: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptEntry {
    pub id: i64,
    pub prompt: String,
    pub timestamp: i64,
}

pub struct PromptHistoryDAO {
    conn: Connection,
}

impl PromptHistoryDAO {
    pub fn new() -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        let db_path = data_dir.join("data.db");

        let mut conn = Connection::open(&db_path)?;
        run_migrations(&mut conn)?;

        Ok(Self { conn })
    }

    pub fn add_prompt(&self, prompt: &str) -> Result<()> {
        if prompt.trim().is_empty() {
            return Ok(());
        }

        self.conn.execute(
            "INSERT INTO prompt_history (prompt, timestamp) VALUES (?1, strftime('%s', 'now'))",
            params![prompt],
        )?;

        self.cleanup_old_entries()?;
        Ok(())
    }

    fn cleanup_old_entries(&self) -> Result<()> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM prompt_history", [], |row| row.get(0))?;

        if count > MAX_HISTORY_SIZE as i64 {
            let to_delete = count - MAX_HISTORY_SIZE as i64;
            self.conn.execute(
                "DELETE FROM prompt_history WHERE id IN (
                    SELECT id FROM prompt_history ORDER BY timestamp ASC LIMIT ?1
                )",
                params![to_delete],
            )?;
        }

        Ok(())
    }

    pub fn get_recent_prompts(&self, limit: usize) -> Result<Vec<PromptEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, timestamp FROM prompt_history ORDER BY timestamp DESC LIMIT ?1",
        )?;

        let prompt_iter = stmt.query_map(params![limit as i64], |row| {
            Ok(PromptEntry {
                id: row.get(0)?,
                prompt: row.get(1)?,
                timestamp: row.get(2)?,
            })
        })?;

        let result: Result<Vec<_>, _> = prompt_iter.collect();
        result.map_err(Into::into)
    }

    pub fn clear_history(&self) -> Result<()> {
        self.conn.execute("DELETE FROM prompt_history", [])?;
        Ok(())
    }
}

pub struct PromptHistoryCache {
    prompts: VecDeque<String>,
    current_index: Option<usize>,
    dao: PromptHistoryDAO,
}

impl PromptHistoryCache {
    pub fn new() -> Result<Self> {
        let dao = PromptHistoryDAO::new()?;
        let prompts = dao
            .get_recent_prompts(MAX_HISTORY_SIZE)?
            .into_iter()
            .map(|entry| entry.prompt)
            .collect();

        Ok(Self {
            prompts,
            current_index: None,
            dao,
        })
    }

    pub fn add_prompt(&mut self, prompt: &str) -> Result<()> {
        if prompt.trim().is_empty() {
            return Ok(());
        }

        self.dao.add_prompt(prompt)?;

        if let Some(pos) = self.prompts.iter().position(|p| p == prompt) {
            self.prompts.remove(pos);
        }

        self.prompts.push_front(prompt.to_string());

        if self.prompts.len() > MAX_HISTORY_SIZE {
            self.prompts.pop_back();
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.prompts.len()
    }

    pub fn navigate_up(&mut self, current_text: &str) -> Option<String> {
        if self.prompts.is_empty() {
            return None;
        }

        match self.current_index {
            None => {
                self.current_index = Some(0);
                self.prompts.front().cloned()
            }
            Some(index) => {
                if index + 1 < self.prompts.len() {
                    self.current_index = Some(index + 1);
                    self.prompts.get(index + 1).cloned()
                } else {
                    None
                }
            }
        }
    }

    pub fn navigate_down(&mut self, current_text: &str) -> Option<String> {
        match self.current_index {
            None => None,
            Some(0) => {
                self.current_index = None;
                Some(String::new())
            }
            Some(index) => {
                self.current_index = Some(index - 1);
                self.prompts.get(index - 1).cloned()
            }
        }
    }

    pub fn reset_navigation(&mut self) {
        self.current_index = None;
    }

    pub fn is_navigating(&self) -> bool {
        self.current_index.is_some()
    }
}
