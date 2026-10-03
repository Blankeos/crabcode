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

    if current_version < 3 {
        migrate_to_v3(db)?;
    }

    if current_version < 4 {
        migrate_to_v4(db)?;
    }

    if current_version < 5 {
        migrate_to_v5(db)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(db: &Connection) -> Vec<String> {
        db.prepare("PRAGMA table_info(messages)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn migration_v5_adds_timing_and_authoritative_usage_columns() {
        let mut db = Connection::open_in_memory().unwrap();
        run_migrations(&mut db).unwrap();

        let columns = columns(&db);
        for column in [
            "input_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "cost",
            "usage_authoritative",
            "tokens_per_sec",
        ] {
            assert!(columns.iter().any(|candidate| candidate == column));
        }
    }

    #[test]
    fn migration_v5_converges_both_historical_v4_schemas() {
        for extra_columns in [
            "tokens_per_sec REAL",
            "input_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER, cost REAL, usage_authoritative INTEGER NOT NULL DEFAULT 0",
        ] {
            let mut db = Connection::open_in_memory().unwrap();
            db.execute_batch(&format!(
                "CREATE TABLE messages (id TEXT PRIMARY KEY, {extra_columns});
                 CREATE TABLE migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
                 INSERT INTO migrations (version, applied_at) VALUES (4, 0);"
            ))
            .unwrap();

            run_migrations(&mut db).unwrap();
            let columns = columns(&db);
            for column in [
                "tokens_per_sec",
                "input_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
                "cost",
                "usage_authoritative",
            ] {
                assert!(columns.iter().any(|candidate| candidate == column));
            }
            assert_eq!(get_current_version(&db).unwrap(), 5);
        }
    }
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
            tokens_per_sec REAL,
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

fn migrate_to_v4(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;
    // Precomputed inter-token TPS so a reloaded session shows the same t/s
    // as the live stream instead of recomputing from token estimates.
    let _ = tx.execute("ALTER TABLE messages ADD COLUMN tokens_per_sec REAL", []);
    tx.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (4, strftime('%s', 'now'))",
        params![],
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_to_v5(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;
    // v4 existed independently on the ACP branch (usage accounting) and on
    // main (precomputed TPS). Ensure databases produced by either history
    // converge on the complete schema.
    let _ = tx.execute("ALTER TABLE messages ADD COLUMN tokens_per_sec REAL", []);
    let _ = tx.execute("ALTER TABLE messages ADD COLUMN input_tokens INTEGER", []);
    let _ = tx.execute(
        "ALTER TABLE messages ADD COLUMN cache_read_tokens INTEGER",
        [],
    );
    let _ = tx.execute(
        "ALTER TABLE messages ADD COLUMN cache_write_tokens INTEGER",
        [],
    );
    let _ = tx.execute("ALTER TABLE messages ADD COLUMN cost REAL", []);
    let _ = tx.execute(
        "ALTER TABLE messages ADD COLUMN usage_authoritative INTEGER NOT NULL DEFAULT 0",
        [],
    );
    tx.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (5, strftime('%s', 'now'))",
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

fn migrate_to_v3(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;

    let _ = tx.execute(
        "ALTER TABLE sessions ADD COLUMN parent_session_identifier TEXT",
        [],
    );

    tx.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_sessions_parent_identifier
            ON sessions(parent_session_identifier, updated_at DESC);
        "#,
    )?;

    tx.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (3, strftime('%s', 'now'))",
        params![],
    )?;

    tx.commit()?;
    Ok(())
}
