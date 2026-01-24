# Persistence Layer for Crabcode

## Overview

Crabcode uses a hybrid persistence strategy combining SQLite for structured data (sessions, messages) and a simple JSON file for credentials.

## Storage Locations

- **SQLite Database**: `~/.local/share/crabcode/data.db`
- **Credentials**: `~/.local/share/crabcode/auth.json`
- **Provider Cache**: `~/.cache/crabcode/providers.json` (provider list and models)

## SQLite Schema

### Versioning
Database version tracked via `PRAGMA user_version`. Current version: 1.

### Tables

#### `sessions`
```sql
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    total_time_sec REAL NOT NULL DEFAULT 0,
    avg_tokens_per_sec REAL NOT NULL DEFAULT 0
);

CREATE INDEX idx_sessions_created ON sessions(created_at DESC);
CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC);
```

**Session Generation Rules**:
- Default name: First user message truncated to 50 characters
- Auto-generate name using AI model if configured (future feature)

#### `messages`
```sql
CREATE TABLE messages (
    id TEXT PRIMARY KEY, -- UUID matching API response
    session_id INTEGER NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'tool')),
    parts TEXT NOT NULL, -- JSON array of message parts (text, tool calls, images, etc.)
    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    tokens_used INTEGER DEFAULT 0,
    model TEXT,
    provider TEXT,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX idx_messages_session ON messages(session_id, timestamp);
```

**Message Parts Structure**:
```json
{
  "parts": [
    { "type": "text", "text": "Hello!" },
    { "type": "tool-call", "toolName": "websearch", "args": {...} },
    { "type": "tool-result", "toolName": "websearch", "result": "..." }
  ]
}
```



#### `migrations`
```sql
CREATE TABLE migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
```

## auth.json Format

```json
{
  "opencode": {
    "type": "api",
    "key": "sk-..."
  },
  "anthropic": {
    "type": "api",
    "key": "sk-ant-..."
  },
  "google": {
    "type": "oauth",
    "refresh": "1//askdmamksm9192",
    "access": "ya29.n2njasd",
    "expires": 178237812391
  }
}
```

## Auto-Upgrade Strategy

1. Check `PRAGMA user_version` on database open
2. If version < current, run migrations in order
3. Each migration:
   - Wrapped in transaction
   - Bumps version after completion
   - Idempotent (safe to re-run)
4. Fallback: If migration fails, keep old version and log error

### Migration Functions

```rust
fn run_migrations(db: &Connection) -> Result<(), Error> {
    let current_version: i32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if current_version < 1 {
        migrate_to_v1(db)?;
    }

    // Future migrations: v2, v3, etc.

    Ok(())
}

fn migrate_to_v1(db: &Connection) -> Result<(), Error> {
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
```

## Key Operations

### Session Management
- **Create**: Insert session with name from first message
- **List**: Get all sessions, sorted by `updated_at DESC`
- **Get**: Retrieve session with all messages
- **Update**: Increment tokens/cost, recalculate avg tokens/sec, update `updated_at`
- **Delete**: Cascade deletes messages
- **Rename**: Allow manual name editing

### Message Management
- **Append**: Add message with `parts` array, update session stats
- **List**: Get messages for a session in order, deserialize `parts`
- **Search**: Full-text search on message `parts` (future)
- **ID Generation**: Use CUID2, match with API response IDs

## Cost Estimation

When saving messages, estimate cost using per-message model info:
```rust
fn estimate_cost(tokens: u32, model: &str, provider: &str) -> f64 {
    // Load pricing from models.dev or local cache
    // Calculate: input_tokens * input_price + output_tokens * output_price
}
```

## Dependencies

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
dirs = "5.0"
cuid2 = "0.1"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1.0"
```

## File System Utilities

```rust
fn get_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crabcode")
}

fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("crabcode")
}

fn ensure_data_dir() -> Result<(), Error> {
    let dir = get_data_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(())
}

fn ensure_cache_dir() -> Result<(), Error> {
    let dir = get_cache_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(())
}
```

## DAO Pattern

```rust
use anyhow::Result as Error;
use std::collections::HashMap;
use std::path::PathBuf;
```

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub pricing: Option<Pricing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCache {
    pub providers: Vec<Provider>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub models: Vec<ModelInfo>,
}

pub struct ProviderDAO {
    cache_path: PathBuf,
}

impl ProviderDAO {
    pub fn new() -> Result<Self, Error> {
        let cache_dir = get_cache_dir();
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self {
            cache_path: cache_dir.join("providers.json"),
        })
    }

    pub fn load(&self) -> Result<Option<ProviderCache>, Error> {
        if !self.cache_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&self.cache_path)?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    pub fn save(&self, cache: &ProviderCache) -> Result<(), Error> {
        let content = serde_json::to_string_pretty(cache)?;
        std::fs::write(&self.cache_path, content)?;
        Ok(())
    }

    pub fn update(&self, providers: Vec<Provider>) -> Result<(), Error> {
        let cache = ProviderCache {
            providers,
            updated_at: chrono::Utc::now().timestamp(),
        };
        self.save(&cache)
    }

    pub fn get_providers(&self) -> Result<Vec<Provider>, Error> {
        match self.load()? {
            Some(cache) => Ok(cache.providers),
            None => Ok(vec![]),
        }
    }

    pub fn get_model(&self, model_id: &str) -> Result<Option<ModelInfo>, Error> {
        let cache = self.load()?;
        if let Some(cache) = cache {
            for provider in &cache.providers {
                for model in &provider.models {
                    if model.id == model_id {
                        return Ok(Some(model.clone()));
                    }
                }
            }
        }
        Ok(None)
    }
}
```

### AuthDAO - Credential Management

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthConfig {
    #[serde(rename = "api")]
    Api { key: String },
    #[serde(rename = "oauth")]
    OAuth { refresh: String, access: String, expires: i64 },
}

pub struct AuthDAO {
    auth_path: PathBuf,
}

impl AuthDAO {
    pub fn new() -> Result<Self, Error> {
        let data_dir = get_data_dir();
        std::fs::create_dir_all(&data_dir)?;
        Ok(Self {
            auth_path: data_dir.join("auth.json"),
        })
    }

    pub fn load(&self) -> Result<HashMap<String, AuthConfig>, Error> {
        if !self.auth_path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&self.auth_path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, providers: &HashMap<String, AuthConfig>) -> Result<(), Error> {
        let content = serde_json::to_string_pretty(providers)?;
        std::fs::write(&self.auth_path, content)?;
        Ok(())
    }

    pub fn set_provider(&self, name: String, config: AuthConfig) -> Result<(), Error> {
        let mut providers = self.load()?;
        providers.insert(name, config);
        self.save(&providers)
    }

    pub fn remove_provider(&self, name: &str) -> Result<(), Error> {
        let mut providers = self.load()?;
        providers.remove(name);
        self.save(&providers)
    }

    pub fn get_api_key(&self, name: &str) -> Result<Option<String>, Error> {
        let providers = self.load()?;
        Ok(providers.get(name).and_then(|c| match c {
            AuthConfig::Api { key } => Some(key.clone()),
            AuthConfig::OAuth { access, .. } => Some(access.clone()),
        }))
    }
}
```

### HistoryDAO - Session & Message Management

```rust
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
}

pub struct HistoryDAO {
    conn: Connection,
}

impl HistoryDAO {
    pub fn new() -> Result<Self, Error> {
        let data_dir = get_data_dir();
        std::fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("data.db");

        let conn = Connection::open(&db_path)?;
        run_migrations(&conn)?;

        Ok(Self { conn })
    }

    pub fn create_session(&self, name: String) -> Result<i64, Error> {
        self.conn.execute(
            "INSERT INTO sessions (name) VALUES (?1)",
            params![name],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at, updated_at, total_tokens, total_cost, total_time_sec, avg_tokens_per_sec
             FROM sessions ORDER BY updated_at DESC"
        )?;

        stmt.query_map([], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
    }

    pub fn get_session(&self, id: i64) -> Result<Option<Session>, Error> {
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

    pub fn add_message(&self, msg: &Message) -> Result<(), Error> {
        let parts_json = serde_json::to_string(&msg.parts)?;

        self.conn.execute(
            "INSERT INTO messages (id, session_id, role, parts, tokens_used, model, provider)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &msg.id,
                msg.session_id,
                &msg.role,
                &parts_json,
                msg.tokens_used,
                msg.model.as_deref(),
                msg.provider.as_deref(),
            ],
        )?;

        self.update_session_stats(msg.session_id, msg.tokens_used, 0.0, msg.timestamp)?;
        Ok(())
    }

    pub fn get_messages(&self, session_id: i64) -> Result<Vec<Message>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, parts, timestamp, tokens_used, model, provider
             FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC"
        )?;

        stmt.query_map(params![session_id], |row| {
            let parts_json: String = row.get(3)?;
            let parts: Vec<MessagePart> = serde_json::from_str(&parts_json)?;
            Ok(Message {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                parts,
                timestamp: row.get(4)?,
                tokens_used: row.get(5)?,
                model: row.get(6)?,
                provider: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
    }

    pub fn update_session_stats(&self, session_id: i64, tokens: i32, cost: f64, msg_timestamp: i64) -> Result<(), Error> {
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
                params![total_tokens_new, total_cost_new, total_time_sec_new, avg_tokens_per_sec_new, msg_timestamp, session_id],
            )?;
        }

        Ok(())
    }

    pub fn delete_session(&self, id: i64) -> Result<(), Error> {
        self.conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn rename_session(&self, id: i64, name: String) -> Result<(), Error> {
        self.conn.execute(
            "UPDATE sessions SET name = ?1, updated_at = strftime('%s', 'now') WHERE id = ?2",
            params![name, id],
        )?;
        Ok(())
    }
}
```

### Usage Example

```rust
fn main() -> Result<(), Error> {
    let provider_dao = ProviderDAO::new()?;
    let auth_dao = AuthDAO::new()?;
    let history_dao = HistoryDAO::new()?;

    // Load providers from cache
    let providers = provider_dao.get_providers()?;
    println!("Available providers: {}", providers.len());

    // Set up provider credentials
    auth_dao.set_provider(
        "anthropic".to_string(),
        AuthConfig::Api { key: "sk-ant-...".to_string() },
    )?;

    // Create session
    let session_id = history_dao.create_session("Chat with Claude".to_string())?;

    // Add message
    let now = chrono::Utc::now().timestamp();
    let msg = Message {
        id: Uuid::new_v4().to_string(),
        session_id,
        role: "user".to_string(),
        parts: vec![MessagePart {
            part_type: "text".to_string(),
            data: serde_json::json!({"text": "Hello!"}),
        }],
        timestamp: now,
        tokens_used: 10,
        model: Some("claude-3-sonnet".to_string()),
        provider: Some("anthropic".to_string()),
    };
    history_dao.add_message(&msg)?;

    // List sessions
    for session in history_dao.list_sessions()? {
        println!(
            "{} | ${:.4} | {} tokens | {:.1} tokens/sec",
            session.name, session.total_cost, session.total_tokens, session.avg_tokens_per_sec
        );
    }

    Ok(())
}
```
