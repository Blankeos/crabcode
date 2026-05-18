use anyhow::Result;
use rusqlite::{params, Connection};

pub fn run_migrations(db: &mut Connection) -> Result<()> {
    let current_version: i32 = get_current_version(db)?;

    if current_version < 1 {
        migrate_to_v1(db)?;
    }

    if current_version < 2 {
        migrate_to_v2(db)?;
    }

    Ok(())
}

fn get_current_version(db: &Connection) -> Result<i32> {
    match db.prepare("SELECT MAX(version) FROM migrations") {
        Ok(mut stmt) => {
            let result: Option<i32> = stmt.query_row([], |row| row.get(0))?;
            Ok(result.unwrap_or(0))
        }
        Err(_) => Ok(0),
    }
}

fn migrate_to_v1(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;

    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_identifier TEXT NOT NULL,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            total_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost REAL NOT NULL DEFAULT 0,
            total_time_sec REAL NOT NULL DEFAULT 0,
            avg_tokens_per_sec REAL NOT NULL DEFAULT 0
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_identifier ON sessions(session_identifier);

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id INTEGER NOT NULL,
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'tool')),
            parts TEXT NOT NULL,
            timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            tokens_used INTEGER DEFAULT 0,
            model TEXT,
            provider TEXT,
            agent_mode TEXT,
            duration_ms INTEGER DEFAULT 0,
            t0_ms INTEGER,
            t1_ms INTEGER,
            tn_ms INTEGER,
            output_tokens INTEGER,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS responses (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            message_id TEXT NOT NULL,
            token_count INTEGER DEFAULT 0,
            duration_ms INTEGER DEFAULT 0,
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE TABLE IF NOT EXISTS prefs (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE TABLE IF NOT EXISTS prompt_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            prompt TEXT NOT NULL,
            timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
        CREATE INDEX IF NOT EXISTS idx_prefs_updated ON prefs(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_prompt_history_timestamp ON prompt_history(timestamp DESC);
        "#,
    )?;

    tx.execute(
        "INSERT INTO migrations (version, applied_at) VALUES (1, strftime('%s', 'now'))",
        params![],
    )?;

    tx.commit()?;
    Ok(())
}

fn migrate_to_v2(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;

    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS workspaces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            root_path TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0,
            archived_at INTEGER,
            last_opened_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE INDEX IF NOT EXISTS idx_workspaces_sort ON workspaces(sort_order ASC, id ASC);
        CREATE INDEX IF NOT EXISTS idx_workspaces_path ON workspaces(root_path);
        "#,
    )?;

    let _ = tx.execute("ALTER TABLE sessions ADD COLUMN workspace_id INTEGER", []);
    let _ = tx.execute(
        "ALTER TABLE sessions ADD COLUMN status TEXT NOT NULL DEFAULT 'idle'",
        [],
    );
    let _ = tx.execute(
        "ALTER TABLE sessions ADD COLUMN active_generation_id TEXT",
        [],
    );
    let _ = tx.execute("ALTER TABLE sessions ADD COLUMN last_error TEXT", []);
    let _ = tx.execute("ALTER TABLE sessions ADD COLUMN pinned_at INTEGER", []);
    let _ = tx.execute("ALTER TABLE sessions ADD COLUMN archived_at INTEGER", []);

    tx.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_sessions_workspace ON sessions(workspace_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
        CREATE INDEX IF NOT EXISTS idx_sessions_pinned ON sessions(pinned_at DESC);
        CREATE INDEX IF NOT EXISTS idx_sessions_archived ON sessions(archived_at);
        "#,
    )?;

    tx.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (2, strftime('%s', 'now'))",
        params![],
    )?;

    tx.commit()?;
    Ok(())
}
