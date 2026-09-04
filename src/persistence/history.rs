use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
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

fn message_cost(message: &Message) -> f64 {
    message
        .cost
        .unwrap_or_else(|| usage_cost_from_parts(&message.parts))
}

#[cfg(test)]
mod usage_part_tests {
    use super::*;

    fn test_dao() -> HistoryDAO {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        let workspace_id = ensure_workspace(&conn, "/tmp/workspace", "workspace").unwrap();
        HistoryDAO {
            conn,
            current_workspace_id: workspace_id,
            current_workspace_path: "/tmp/workspace".to_string(),
            current_workspace_name: "workspace".to_string(),
        }
    }

    #[test]
    fn authoritative_usage_updates_message_and_session_totals() {
        let dao = test_dao();
        let session_id = dao
            .create_session("session", "Session".to_string())
            .unwrap();
        let message = Message {
            id: "message".to_string(),
            session_id,
            role: "assistant".to_string(),
            parts: Vec::new(),
            timestamp: chrono::Utc::now().timestamp(),
            tokens_used: 5,
            model: Some("model".to_string()),
            provider: Some("provider".to_string()),
            agent_mode: None,
            duration_ms: 10,
            t0_ms: None,
            t1_ms: None,
            tn_ms: None,
            output_tokens: Some(25),
            input_tokens: Some(100),
            cache_read_tokens: Some(60),
            cache_write_tokens: Some(10),
            cost: Some(0.0125),
            usage_authoritative: true,
            tokens_per_sec: Some(50.0),
        };

        dao.add_message(&message).unwrap();
        let restored = dao.get_messages(session_id).unwrap();
        assert_eq!(restored[0].input_tokens, Some(100));
        assert_eq!(restored[0].output_tokens, Some(25));
        assert_eq!(restored[0].cache_read_tokens, Some(60));
        assert_eq!(restored[0].cache_write_tokens, Some(10));
        assert_eq!(restored[0].cost, Some(0.0125));
        assert!(restored[0].usage_authoritative);
        assert_eq!(restored[0].tokens_per_sec, Some(50.0));
        let session = dao.get_session(session_id).unwrap().unwrap();
        assert_eq!(session.total_tokens, 125);
        assert!((session.total_cost - 0.0125).abs() < f64::EPSILON);
    }
}

fn message_total_tokens(message: &Message) -> i32 {
    if message.usage_authoritative {
        let total = message
            .input_tokens
            .unwrap_or(0)
            .saturating_add(message.output_tokens.unwrap_or(0));
        i32::try_from(total).unwrap_or(i32::MAX)
    } else {
        message.tokens_used
    }
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
    pub workspace_sort_order: i64,
    pub status: String,
    pub pinned_at: Option<i64>,
    pub archived_at: Option<i64>,
    #[serde(default)]
    pub message_count: usize,
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
    pub input_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost: Option<f64>,
    pub usage_authoritative: bool,
    pub tokens_per_sec: Option<f64>,
}

pub struct HistoryDAO {
    conn: Connection,
    current_workspace_id: i64,
    current_workspace_path: String,
    current_workspace_name: String,
}

impl HistoryDAO {
    pub fn new() -> Result<Self> {
        Self::new_for_workspace(crate::utils::cwd::current_dir_or_dot())
    }

    pub fn new_for_workspace(workspace: impl AsRef<std::path::Path>) -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        let db_path = data_dir.join("data.db");

        let mut conn = Connection::open(&db_path)?;
        // WAL keeps readers non-blocking and turns the frequent streaming
        // snapshot writes into cheap log appends instead of full journal
        // rewrites with an fsync per statement.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
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

        let current_workspace_path = workspace
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| workspace.as_ref().to_path_buf())
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
        self.create_session_with_parent_in_workspace(
            identifier,
            name,
            parent_identifier,
            self.current_workspace_id,
        )
    }

    pub fn create_session_with_parent_in_workspace(
        &self,
        identifier: &str,
        name: String,
        parent_identifier: Option<&str>,
        workspace_id: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sessions (
                 session_identifier, parent_session_identifier, name, workspace_id, status
             )
             VALUES (?1, ?2, ?3, ?4, 'idle')",
            params![identifier, parent_identifier, name, workspace_id],
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

    pub fn ensure_workspace_path(&self, root_path: &str) -> Result<Workspace> {
        let display_name = workspace_display_name(root_path);
        let id = ensure_workspace(&self.conn, root_path, &display_name)?;
        self.conn.execute(
            "UPDATE workspaces
             SET archived_at = NULL,
                 last_opened_at = strftime('%s', 'now')
             WHERE id = ?1",
            params![id],
        )?;

        self.workspace_by_id(id)
    }

    fn workspace_by_id(&self, id: i64) -> Result<Workspace> {
        self.conn
            .query_row(
                "SELECT id, root_path, display_name, sort_order, archived_at, last_opened_at
                 FROM workspaces
                 WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Workspace {
                        id: row.get(0)?,
                        root_path: row.get(1)?,
                        display_name: row.get(2)?,
                        sort_order: row.get(3)?,
                        archived_at: row.get(4)?,
                        last_opened_at: row.get(5)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn set_workspace_archived(&self, root_path: &str, archived: bool) -> Result<bool> {
        let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM workspaces WHERE root_path = ?1",
                params![root_path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        else {
            return Ok(false);
        };

        if archived {
            self.conn.execute(
                "UPDATE workspaces
                 SET archived_at = strftime('%s', 'now')
                 WHERE id = ?1",
                params![id],
            )?;
            self.conn.execute(
                "UPDATE sessions
                 SET archived_at = COALESCE(archived_at, strftime('%s', 'now')),
                     updated_at = strftime('%s', 'now')
                 WHERE workspace_id = ?1",
                params![id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE workspaces
                 SET archived_at = NULL
                 WHERE id = ?1",
                params![id],
            )?;
        }

        Ok(true)
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.session_identifier, s.parent_session_identifier,
                    s.name, s.created_at, s.updated_at,
                    s.total_tokens, s.total_cost, s.total_time_sec, s.avg_tokens_per_sec,
                    COALESCE(s.workspace_id, ?1) AS workspace_id,
                     COALESCE(w.root_path, ?2) AS workspace_path,
                     COALESCE(w.display_name, ?3) AS workspace_name,
                     COALESCE(w.sort_order, COALESCE(s.workspace_id, ?1)) AS workspace_sort_order,
                     COALESCE(s.status, 'idle') AS status,
                     s.pinned_at,
                     s.archived_at,
                     (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS message_count
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
                    workspace_sort_order: row.get(13)?,
                    status: row.get(14)?,
                    pinned_at: row.get(15)?,
                    archived_at: row.get(16)?,
                    message_count: row.get::<_, i64>(17)?.max(0) as usize,
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
                     COALESCE(w.sort_order, COALESCE(s.workspace_id, ?2)) AS workspace_sort_order,
                     COALESCE(s.status, 'idle') AS status,
                     s.pinned_at,
                     s.archived_at,
                     (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS message_count
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
                workspace_sort_order: row.get(13)?,
                status: row.get(14)?,
                pinned_at: row.get(15)?,
                archived_at: row.get(16)?,
                message_count: row.get::<_, i64>(17)?.max(0) as usize,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn move_workspace_sort_order(&self, workspace_id: i64, offset: isize) -> Result<bool> {
        let mut workspaces = self.list_workspaces()?;
        let Some(index) = workspaces
            .iter()
            .position(|workspace| workspace.id == workspace_id)
        else {
            return Ok(false);
        };

        let target_index = if offset < 0 {
            index.checked_sub(1)
        } else if offset > 0 && index + 1 < workspaces.len() {
            Some(index + 1)
        } else {
            None
        };

        let Some(target_index) = target_index else {
            return Ok(false);
        };

        workspaces.swap(index, target_index);

        for (sort_order, workspace) in workspaces.iter().enumerate() {
            self.conn.execute(
                "UPDATE workspaces SET sort_order = ?1 WHERE id = ?2",
                params![sort_order as i64, workspace.id],
            )?;
        }

        Ok(true)
    }

    pub fn move_session_to_workspace(&self, session_id: i64, workspace_id: i64) -> Result<bool> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM workspaces WHERE id = ?1",
                params![workspace_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(false);
        }

        let changed = self.conn.execute(
            "UPDATE sessions
             SET workspace_id = ?1,
                 updated_at = strftime('%s', 'now')
             WHERE id = ?2",
            params![workspace_id, session_id],
        )?;

        Ok(changed > 0)
    }

    pub fn add_message(&self, msg: &Message) -> Result<()> {
        let parts_json = serde_json::to_string(&msg.parts)?;

        self.conn.execute(
            "INSERT INTO messages (
                 id, session_id, role, parts, timestamp, tokens_used, model, provider, agent_mode, duration_ms,
                 t0_ms, t1_ms, tn_ms, output_tokens, input_tokens, cache_read_tokens,
                 cache_write_tokens, cost, usage_authoritative, tokens_per_sec
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                &msg.id,
                msg.session_id,
                &msg.role,
                &parts_json,
                msg.timestamp,
                msg.tokens_used,
                msg.model.as_deref(),
                msg.provider.as_deref(),
                msg.agent_mode.as_deref(),
                msg.duration_ms,
                msg.t0_ms,
                msg.t1_ms,
                msg.tn_ms,
                msg.output_tokens,
                msg.input_tokens,
                msg.cache_read_tokens,
                msg.cache_write_tokens,
                msg.cost,
                msg.usage_authoritative,
                msg.tokens_per_sec,
            ],
        )?;

        let tokens = message_total_tokens(msg);
        self.update_session_stats(msg.session_id, tokens, message_cost(msg), msg.timestamp)?;
        Ok(())
    }

    pub fn replace_messages(&self, session_id: i64, messages: &[Message]) -> Result<()> {
        // A single transaction turns the delete + N inserts into one commit.
        // This runs on the UI thread every streaming snapshot, so per-statement
        // autocommits (each with their own fsync) caused visible lag on long
        // transcripts.
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1",
            params![session_id],
        )?;

        let mut total_tokens: i64 = 0;
        let mut total_cost = 0.0;
        let mut updated_at = chrono::Utc::now().timestamp();

        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO messages (
                     id, session_id, role, parts, timestamp, tokens_used, model, provider, agent_mode, duration_ms,
                     t0_ms, t1_ms, tn_ms, output_tokens, input_tokens, cache_read_tokens,
                     cache_write_tokens, cost, usage_authoritative, tokens_per_sec
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            )?;

            for msg in messages {
                let parts_json = serde_json::to_string(&msg.parts)?;
                total_tokens += i64::from(message_total_tokens(msg));
                total_cost += message_cost(msg);
                updated_at = msg.timestamp;

                insert.execute(params![
                    &msg.id,
                    session_id,
                    &msg.role,
                    &parts_json,
                    msg.timestamp,
                    msg.tokens_used,
                    msg.model.as_deref(),
                    msg.provider.as_deref(),
                    msg.agent_mode.as_deref(),
                    msg.duration_ms,
                    msg.t0_ms,
                    msg.t1_ms,
                    msg.tn_ms,
                    msg.output_tokens,
                    msg.input_tokens,
                    msg.cache_read_tokens,
                    msg.cache_write_tokens,
                    msg.cost,
                    msg.usage_authoritative,
                    msg.tokens_per_sec,
                ])?;
            }
        }

        let session = self.get_session(session_id)?;
        let total_time_sec = session
            .as_ref()
            .map(|session| (updated_at - session.created_at).max(0) as f64)
            .unwrap_or(0.0);
        let avg_tokens_per_sec = if total_time_sec > 0.0 {
            total_tokens as f64 / total_time_sec
        } else {
            0.0
        };

        tx.execute(
            "UPDATE sessions
              SET total_tokens = ?1,
                  total_cost = ?2,
                  total_time_sec = ?3,
                  avg_tokens_per_sec = ?4,
                  updated_at = ?5
              WHERE id = ?6",
            params![
                total_tokens,
                total_cost,
                total_time_sec,
                avg_tokens_per_sec,
                updated_at,
                session_id
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    pub fn get_messages(&self, session_id: i64) -> Result<Vec<Message>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, parts, timestamp, tokens_used, model, provider, agent_mode, duration_ms,
                    t0_ms, t1_ms, tn_ms, output_tokens, input_tokens, cache_read_tokens,
                    cache_write_tokens, cost, usage_authoritative, tokens_per_sec
             FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC, rowid ASC",
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
                input_tokens: row.get(14)?,
                cache_read_tokens: row.get(15)?,
                cache_write_tokens: row.get(16)?,
                cost: row.get(17)?,
                usage_authoritative: row.get(18)?,
                tokens_per_sec: row.get(19)?,
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

fn usage_cost_from_parts(parts: &[MessagePart]) -> f64 {
    parts
        .iter()
        .filter(|part| part.part_type == "usage")
        .filter_map(|part| part.data.get("cost")?.as_f64())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_cost_sums_usage_parts() {
        let parts = vec![
            MessagePart {
                part_type: "text".to_string(),
                data: serde_json::json!({ "text": "hi" }),
            },
            MessagePart {
                part_type: "usage".to_string(),
                data: serde_json::json!({
                    "input": 1000,
                    "output": 100,
                    "cache_read": 500,
                    "cache_write": 50,
                    "cost": 0.01,
                }),
            },
            MessagePart {
                part_type: "usage".to_string(),
                data: serde_json::json!({ "cost": 0.02 }),
            },
        ];

        assert!((usage_cost_from_parts(&parts) - 0.03).abs() < f64::EPSILON);
    }
}
