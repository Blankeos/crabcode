use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::{ensure_data_dir, get_data_dir, migrations::run_migrations};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub total_time_sec: f64,
    pub avg_tokens_per_sec: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session_id: i64,
    pub role: String,
    pub parts: Vec<MessagePart>,
    pub timestamp: i64,
    pub tokens_used: i32,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub agent_mode: Option<String>,
    pub duration_ms: i64,
    pub t0_ms: Option<i64>,
    pub t1_ms: Option<i64>,
    pub tn_ms: Option<i64>,
    pub output_tokens: Option<i64>,
}

pub struct HistoryDAO {
    conn: Connection,
}

impl HistoryDAO {
    pub fn new() -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        let db_path = data_dir.join("data.db");

        let mut conn = Connection::open(&db_path)?;
        run_migrations(&mut conn)?;

        Ok(Self { conn })
    }

    pub fn create_session(&self, name: String) -> Result<i64> {
        self.conn
            .execute("INSERT INTO sessions (name) VALUES (?1)", params![name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at, updated_at, total_tokens, total_cost, total_time_sec, avg_tokens_per_sec
             FROM sessions ORDER BY updated_at DESC"
        )?;

        let session_iter = stmt.query_map([], |row| {
            Ok(Session {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                total_tokens: row.get(4)?,
                total_cost: row.get(5)?,
                total_time_sec: row.get(6)?,
                avg_tokens_per_sec: row.get(7)?,
            })
        })?;

        let result: Result<Vec<_>, _> = session_iter.collect();
        result.map_err(Into::into)
    }

    pub fn get_session(&self, id: i64) -> Result<Option<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at, updated_at, total_tokens, total_cost, total_time_sec, avg_tokens_per_sec
             FROM sessions WHERE id = ?1"
        )?;

        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Session {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                total_tokens: row.get(4)?,
                total_cost: row.get(5)?,
                total_time_sec: row.get(6)?,
                avg_tokens_per_sec: row.get(7)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn add_message(&self, msg: &Message) -> Result<()> {
        let parts_json = serde_json::to_string(&msg.parts)?;

        self.conn.execute(
            "INSERT INTO messages (
                 id, session_id, role, parts, tokens_used, model, provider, agent_mode, duration_ms,
                 t0_ms, t1_ms, tn_ms, output_tokens
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                &msg.id,
                msg.session_id,
                &msg.role,
                &parts_json,
                msg.tokens_used,
                msg.model.as_deref(),
                msg.provider.as_deref(),
                msg.agent_mode.as_deref(),
                msg.duration_ms,
                msg.t0_ms,
                msg.t1_ms,
                msg.tn_ms,
                msg.output_tokens,
            ],
        )?;

        self.update_session_stats(msg.session_id, msg.tokens_used, 0.0, msg.timestamp)?;
        Ok(())
    }

    pub fn get_messages(&self, session_id: i64) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, parts, timestamp, tokens_used, model, provider, agent_mode, duration_ms,
                    t0_ms, t1_ms, tn_ms, output_tokens
             FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC",
        )?;

        let message_iter = stmt.query_map(params![session_id], |row| {
            let parts_json: String = row.get(3)?;
            let parts: Vec<MessagePart> = serde_json::from_str(&parts_json)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok(Message {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                parts,
                timestamp: row.get(4)?,
                tokens_used: row.get(5)?,
                model: row.get(6)?,
                provider: row.get(7)?,
                agent_mode: row.get(8)?,
                duration_ms: row.get(9)?,
                t0_ms: row.get(10)?,
                t1_ms: row.get(11)?,
                tn_ms: row.get(12)?,
                output_tokens: row.get(13)?,
            })
        })?;

        let result: Result<Vec<_>, _> = message_iter.collect();
        result.map_err(Into::into)
    }

    pub fn update_session_stats(
        &self,
        session_id: i64,
        tokens: i32,
        cost: f64,
        msg_timestamp: i64,
    ) -> Result<()> {
        let session = self.get_session(session_id)?;

        if let Some(session) = session {
            let total_tokens_new = session.total_tokens + tokens as i64;
            let total_cost_new = session.total_cost + cost;

            let total_time_sec_new = (msg_timestamp - session.created_at) as f64;
            let avg_tokens_per_sec_new = if total_time_sec_new > 0.0 {
                total_tokens_new as f64 / total_time_sec_new
            } else {
                0.0
            };

            self.conn.execute(
                "UPDATE sessions
                 SET total_tokens = ?1,
                     total_cost = ?2,
                     total_time_sec = ?3,
                     avg_tokens_per_sec = ?4,
                     updated_at = ?5
                 WHERE id = ?6",
                params![
                    total_tokens_new,
                    total_cost_new,
                    total_time_sec_new,
                    avg_tokens_per_sec_new,
                    msg_timestamp,
                    session_id,
                ],
            )?;
        }

        Ok(())
    }

    pub fn delete_session(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn rename_session(&self, id: i64, name: String) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET name = ?1, updated_at = strftime('%s', 'now') WHERE id = ?2",
            params![name, id],
        )?;
        Ok(())
    }

    pub fn get_full_session(&self, id: i64) -> Result<Option<(Session, Vec<Message>)>> {
        let session = self.get_session(id)?;
        if let Some(session) = session {
            let messages = self.get_messages(id)?;
            Ok(Some((session, messages)))
        } else {
            Ok(None)
        }
    }
}
