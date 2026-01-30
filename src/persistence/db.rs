use anyhow::Result;
use rusqlite::Connection;
use std::sync::{Arc, Mutex, OnceLock};

use super::{ensure_data_dir, get_data_dir, migrations::run_migrations};

pub type DbConn = Arc<Mutex<Connection>>;

fn init_db_conn() -> Result<DbConn> {
    ensure_data_dir()?;
    let db_path = get_data_dir().join("data.db");

    let mut conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    run_migrations(&mut conn)?;

    Ok(Arc::new(Mutex::new(conn)))
}

pub fn get_db_conn() -> Result<DbConn> {
    static DB: OnceLock<DbConn> = OnceLock::new();
    if let Some(conn) = DB.get() {
        return Ok(conn.clone());
    }

    let conn = init_db_conn()?;
    let _ = DB.set(conn.clone());
    Ok(conn)
}
