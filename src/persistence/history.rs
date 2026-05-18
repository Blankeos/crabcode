use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::{ensure_data_dir, get_data_dir, migrations::run_migrations};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: i64,
    pub root_path: String,
    pub display_name: String,
    pub sort_order: i64,
    pub archived_at: Option<i64>,
    pub last_opened_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub session_identifier: String,
    pub parent_session_identifier: Option<String>,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub total_time_sec: f64,
    pub avg_tokens_per_sec: f64,
    pub workspace_id: i64,
    pub workspace_path: String,
    pub workspace_name: String,
    pub status: String,
    pub pinned_at: Option<i64>,
    pub archived_at: Option<i64>,
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
    current_workspace_id: i64,
    current_workspace_path: String,
    current_workspace_name: String,
}

impl HistoryDAO {
    pub fn new() -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        let db_path = data_dir.join("data.db");

        let mut conn = Connection::open(&db_path)?;
        run_migrations(&mut conn)?;

        // Ensure session_identifier column exists on pre-existing databases
        let _ = conn.execute(
            "ALTER TABLE sessions ADD COLUMN session_identifier TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE sessions ADD COLUMN parent_session_identifier TEXT",
            [],
        );

        let current_workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .to_string_lossy()
            .to_string();
        let current_workspace_name = workspace_display_name(&current_workspace_path);
        let current_workspace_id =
            ensure_workspace(&conn, &current_workspace_path, &current_workspace_name)?;

        conn.execute(
            "UPDATE sessions
             SET workspace_id = ?1
             WHERE workspace_id IS NULL",
            params![current_workspace_id],
        )?;
        conn.execute(
            "UPDATE workspaces
             SET last_opened_at = strftime('%s', 'now')
             WHERE id = ?1",
            params![current_workspace_id],
        )?;

        Ok(Self {
            conn,
            current_workspace_id,
            current_workspace_path,
            current_workspace_name,
        })
    }

    pub fn create_session(&self, identifier: &str, name: String) -> Result<i64> {
        self.create_session_with_parent(identifier, name, None)
    }

    pub fn create_session_with_parent(
        &self,
        identifier: &str,
        name: String,
        parent_identifier: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (
                 session_identifier, parent_session_identifier, name, workspace_id, status
             )
             VALUES (?1, ?2, ?3, ?4, 'idle')",
            params![
                identifier,
                parent_identifier,
                name,
                self.current_workspace_id
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn current_workspace_id(&self) -> i64 {
        self.current_workspace_id
    }

    pub fn current_workspace_path(&self) -> &str {
        &self.current_workspace_path
    }

    pub fn current_workspace_name(&self) -> &str {
        &self.current_workspace_name
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, root_path, display_name, sort_order, archived_at, last_opened_at
             FROM workspaces
             ORDER BY sort_order ASC, id ASC",
        )?;

        let iter = stmt.query_map([], |row| {
            Ok(Workspace {
                id: row.get(0)?,
                root_path: row.get(1)?,
                display_name: row.get(2)?,
                sort_order: row.get(3)?,
                archived_at: row.get(4)?,
                last_opened_at: row.get(5)?,
            })
        })?;

        let result: Result<Vec<_>, _> = iter.collect();
        result.map_err(Into::into)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.session_identifier, s.parent_session_identifier,
                    s.name, s.created_at, s.updated_at,
                    s.total_tokens, s.total_cost, s.total_time_sec, s.avg_tokens_per_sec,
                    COALESCE(s.workspace_id, ?1) AS workspace_id,
                    COALESCE(w.root_path, ?2) AS workspace_path,
                    COALESCE(w.display_name, ?3) AS workspace_name,
                    COALESCE(s.status, 'idle') AS status,
                    s.pinned_at,
                    s.archived_at
             FROM sessions s
             LEFT JOIN workspaces w ON w.id = s.workspace_id
             ORDER BY s.updated_at DESC",
        )?;

        let session_iter = stmt.query_map(
            params![
                self.current_workspace_id,
                self.current_workspace_path.as_str(),
                self.current_workspace_name.as_str()
            ],
            |row| {
                Ok(Session {
                    id: row.get(0)?,
                    session_identifier: row.get(1)?,
                    parent_session_identifier: row.get(2)?,
                    name: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    total_tokens: row.get(6)?,
                    total_cost: row.get(7)?,
                    total_time_sec: row.get(8)?,
                    avg_tokens_per_sec: row.get(9)?,
                    workspace_id: row.get(10)?,
                    workspace_path: row.get(11)?,
                    workspace_name: row.get(12)?,
                    status: row.get(13)?,
                    pinned_at: row.get(14)?,
                    archived_at: row.get(15)?,
                })
            },
        )?;

        let result: Result<Vec<_>, _> = session_iter.collect();
        result.map_err(Into::into)
    }

    pub fn get_session(&self, id: i64) -> Result<Option<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.session_identifier, s.parent_session_identifier,
                    s.name, s.created_at, s.updated_at,
                    s.total_tokens, s.total_cost, s.total_time_sec, s.avg_tokens_per_sec,
                    COALESCE(s.workspace_id, ?2) AS workspace_id,
                    COALESCE(w.root_path, ?3) AS workspace_path,
                    COALESCE(w.display_name, ?4) AS workspace_name,
                    COALESCE(s.status, 'idle') AS status,
                    s.pinned_at,
                    s.archived_at
             FROM sessions s
             LEFT JOIN workspaces w ON w.id = s.workspace_id
             WHERE s.id = ?1",
        )?;

        let mut rows = stmt.query(params![
            id,
            self.current_workspace_id,
            self.current_workspace_path.as_str(),
            self.current_workspace_name.as_str()
        ])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Session {
                id: row.get(0)?,
                session_identifier: row.get(1)?,
                parent_session_identifier: row.get(2)?,
                name: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                total_tokens: row.get(6)?,
                total_cost: row.get(7)?,
                total_time_sec: row.get(8)?,
                avg_tokens_per_sec: row.get(9)?,
                workspace_id: row.get(10)?,
                workspace_path: row.get(11)?,
                workspace_name: row.get(12)?,
                status: row.get(13)?,
                pinned_at: row.get(14)?,
                archived_at: row.get(15)?,
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

    pub fn set_session_status(
        &self,
        id: i64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions
             SET status = ?1,
                 last_error = ?2,
                 updated_at = strftime('%s', 'now')
             WHERE id = ?3",
            params![status, last_error, id],
        )?;
        Ok(())
    }

    pub fn set_session_pinned(&self, id: i64, pinned: bool) -> Result<Option<i64>> {
        if pinned {
            self.conn.execute(
                "UPDATE sessions
                 SET pinned_at = strftime('%s', 'now'),
                     updated_at = strftime('%s', 'now')
                 WHERE id = ?1",
                params![id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE sessions
                 SET pinned_at = NULL,
                     updated_at = strftime('%s', 'now')
                 WHERE id = ?1",
                params![id],
            )?;
        }

        let pinned_at = self.conn.query_row(
            "SELECT pinned_at FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(pinned_at)
    }

    pub fn set_session_archived(&self, id: i64, archived: bool) -> Result<Option<i64>> {
        if archived {
            self.conn.execute(
                "UPDATE sessions
                 SET archived_at = strftime('%s', 'now'),
                     updated_at = strftime('%s', 'now')
                 WHERE id = ?1",
                params![id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE sessions
                 SET archived_at = NULL,
                     updated_at = strftime('%s', 'now')
                 WHERE id = ?1",
                params![id],
            )?;
        }

        let archived_at = self.conn.query_row(
            "SELECT archived_at FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(archived_at)
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

fn workspace_display_name(root_path: &str) -> String {
    Path::new(root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(root_path)
        .to_string()
}

fn ensure_workspace(conn: &Connection, root_path: &str, display_name: &str) -> Result<i64> {
    if let Ok(id) = conn.query_row(
        "SELECT id FROM workspaces WHERE root_path = ?1",
        params![root_path],
        |row| row.get::<_, i64>(0),
    ) {
        return Ok(id);
    }

    let next_sort_order = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM workspaces",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);

    conn.execute(
        "INSERT INTO workspaces (root_path, display_name, sort_order)
         VALUES (?1, ?2, ?3)",
        params![root_path, display_name, next_sort_order],
    )?;
    Ok(conn.last_insert_rowid())
}
