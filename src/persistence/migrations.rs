use anyhow::Result;
use rusqlite::Connection;

pub fn run_migrations(db: &mut Connection) -> Result<()> {
    let current_version: i32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if current_version < 1 {
        migrate_to_v1(db)?;
    }

    Ok(())
}

fn migrate_to_v1(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;

    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            total_tokens INTEGER NOT NULL DEFAULT 0,
            total_cost REAL NOT NULL DEFAULT 0,
            total_time_sec REAL NOT NULL DEFAULT 0,
            avg_tokens_per_sec REAL NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id INTEGER NOT NULL,
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'tool')),
            parts TEXT NOT NULL,
            timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            tokens_used INTEGER DEFAULT 0,
            model TEXT,
            provider TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
        "#,
    )?;

    tx.pragma_update(None, "user_version", 1)?;

    tx.commit()?;
    Ok(())
}
