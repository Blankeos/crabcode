# Active Model Switching Plan

## Overview

Implement active model switching functionality that allows users to select a model from the `/models` dialog and persist the selection across application restarts. Also implement model favorites feature and reorganize the models dialog with special groups for quick access.

**UI Changes:**

- Add "Favorites" group for starred models
- Add "Recent" group for MRU models
- Provider groups follow these special groups
- Models display "✓ Active" or " 🩷 Favorite" indicators

**Models Dialog Structure:**

```
┌─────────────────────────────────────────────────┐
│ Available Models                        esc   │
│                                                 │
│ <search...>                                    │
│                                                 │
│ 🩷 Favorites                                      │
│   ★ Claude Opus                         ✓ Active│
│   GPT-4 Turbo                               │
│                                                 │
│ Recent                                          │
│   Claude Sonnet                                │
│   GPT-4                                       │
│                                                 │
│ Anthropic                                     │
│   Claude Haiku                                 │
│                                                 │
│ OpenAI                                        │
│   GPT-3.5 Turbo                               │
│                                                 │
│                                              Favorite│
│ ctrl+f                                      │
└─────────────────────────────────────────────────┘
```

### Table Schema

**File: `src/persistence/migrations.rs`**

Add to existing v1 migration (or create v2 if needed):

```sql
CREATE TABLE IF NOT EXISTS prefs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_prefs_updated ON prefs(updated_at DESC);
```

### Data Structure

The `model_preferences` key stores JSON following opencode's exact structure:

```json
{
  "recent": [
    { "providerID": "anthropic", "modelID": "claude-3-sonnet-20240229" },
    { "providerID": "openai", "modelID": "gpt-4-turbo" }
  ],
  "favorite": [
    { "providerID": "anthropic", "modelID": "claude-3-opus-20240229" },
    { "providerID": "openai", "modelID": "gpt-4" }
  ],
  "variant": {}
}
```

- **Active model**: Always the first item in `recent` array
- **Recent**: MRU (Most Recently Used) list, max ~10 items
- **Favorite**: User-favorited models
- **Variant**: Reserved for future use (currently empty)

## Implementation

### 1. Update Migrations

**File: `src/persistence/migrations.rs`**

Add `prefs` table to v1 migration:

```rust
fn migrate_to_v1(db: &Connection) -> Result<(), Error> {
    let tx = db.transaction()?;

    tx.execute_batch(
        r#"
        -- Existing tables...

        CREATE TABLE IF NOT EXISTS prefs (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );

        CREATE INDEX IF NOT EXISTS idx_prefs_updated ON prefs(updated_at DESC);
        "#,
    )?;

    tx.pragma_update(None, "user_version", 1)?;
    tx.commit()?;
    Ok(())
}
```

### 2. Create PrefsDAO

**File: `src/persistence/prefs.rs`**

```rust
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
    /// Get the active model (first in recent list)
    pub fn get_active_model(&self) -> Option<&ModelRef> {
        self.recent.first()
    }

    /// Add a model to the recent list (MRU)
    /// Returns true if it was newly added (not already in top position)
    pub fn add_recent(&mut self, provider_id: String, model_id: String) -> bool {
        let new_ref = ModelRef { provider_id, model_id };

        // Remove if exists elsewhere
        self.recent.retain(|m| m != &new_ref);

        // Add to front
        self.recent.insert(0, new_ref);

        // Limit to 10 items
        if self.recent.len() > 10 {
            self.recent.truncate(10);
        }

        true
    }

    /// Add/remove model from favorites
    pub fn toggle_favorite(&mut self, provider_id: String, model_id: String) {
        let new_ref = ModelRef { provider_id, model_id };

        if let Some(pos) = self.favorite.iter().position(|m| m == &new_ref) {
            self.favorite.remove(pos);
        } else {
            self.favorite.push(new_ref);
        }
    }

    /// Check if model is in favorites
    pub fn is_favorite(&self, provider_id: &str, model_id: &str) -> bool {
        self.favorite.iter().any(|m| m.provider_id == provider_id && m.model_id == model_id)
    }
}

pub struct PrefsDAO {
    conn: Connection,
}

impl PrefsDAO {
    pub fn new() -> Result<Self> {
        let data_dir = get_data_dir();
        ensure_data_dir()?;
        let db_path = data_dir.join("data.db");

        let conn = Connection::open(&db_path)?;

        // Run migrations
        super::migrations::run_migrations(&conn)?;

        Ok(Self { conn })
    }

    fn get_pref(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM prefs WHERE key = ?1")?;
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

    /// Get model preferences
    pub fn get_model_preferences(&self) -> Result<ModelPreferences> {
        match self.get_pref(MODEL_PREFS_KEY)? {
            Some(json_str) => {
                let prefs: ModelPreferences = serde_json::from_str(&json_str)?;
                Ok(prefs)
            }
            None => Ok(ModelPreferences::default()),
        }
    }

    /// Save model preferences
    pub fn set_model_preferences(&self, prefs: &ModelPreferences) -> Result<()> {
        let json_str = serde_json::to_string(prefs)?;
        self.set_pref(MODEL_PREFS_KEY, &json_str)
    }

    /// Get active model (convenience method)
    pub fn get_active_model(&self) -> Result<Option<(String, String)>> {
        let prefs = self.get_model_preferences()?;
        if let Some(model_ref) = prefs.get_active_model() {
            Ok(Some((model_ref.provider_id.clone(), model_ref.model_id.clone())))
        } else {
            Ok(None)
        }
    }

    /// Set active model (add to recent list)
    pub fn set_active_model(&self, provider_id: String, model_id: String) -> Result<()> {
        let mut prefs = self.get_model_preferences()?;
        prefs.add_recent(provider_id, model_id);
        self.set_model_preferences(&prefs)
    }

    /// Toggle model favorite
    pub fn toggle_favorite(&self, provider_id: String, model_id: String) -> Result<bool> {
        let mut prefs = self.get_model_preferences()?;
        let was_favorite = prefs.is_favorite(&provider_id, &model_id);
        prefs.toggle_favorite(provider_id, model_id);
        self.set_model_preferences(&prefs)?;
        Ok(!was_favorite) // Returns new favorite status
    }

    /// Check if model is favorite
    pub fn is_favorite(&self, provider_id: &str, model_id: &str) -> Result<bool> {
        let prefs = self.get_model_preferences()?;
        Ok(prefs.is_favorite(provider_id, model_id))
    }
}
```

### 3. Update Persistence Module

**File: `src/persistence/mod.rs`**

```rust
pub mod prefs;

pub use prefs::{ModelPreferences, ModelRef, PrefsDAO};
```

### 4. Update Dialog Component for Actions

**File: `src/ui/components/dialog.rs`**

Update footer actions display to support dynamic actions:

```rust
// Already exists - no changes needed if using existing footer_actions field
```

### 5. Update Models Dialog Handler

**File: `src/views/models_dialog.rs`**

Update to handle Enter (select) and Ctrl+F (favorite):

```rust
pub enum ModelsDialogAction {
    SelectModel { provider_id: String, model_id: String },
    ToggleFavorite { provider_id: String, model_id: String },
    None,
}

pub fn handle_models_dialog_key_event(
    dialog_state: &mut ModelsDialogState,
    event: KeyEvent,
) -> ModelsDialogAction {
    let was_visible = dialog_state.dialog.is_visible();
    let handled = dialog_state.dialog.handle_key_event(event);

    if was_visible && !dialog_state.dialog.is_visible() {
        // Dialog was closed
        if event.code == KeyCode::Enter {
            if let Some(selected) = dialog_state.dialog.get_selected() {
                return ModelsDialogAction::SelectModel {
                    provider_id: selected.group.clone(),
                    model_id: selected.id.clone(),
                };
            }
        }
    }

    if !handled {
        // Check for Ctrl+F (toggle favorite)
        if let KeyCode::Char('f') = event.code {
            if event.modifiers == KeyModifiers::CONTROL {
                if let Some(selected) = dialog_state.dialog.get_selected() {
                    return ModelsDialogAction::ToggleFavorite {
                        provider_id: selected.group.clone(),
                        model_id: selected.id.clone(),
                    };
                }
            }
        }
    }

    ModelsDialogAction::None
}
```

### 6. Update App State

**File: `src/app.rs`**

Add PrefsDAO and load active model on startup (only time database is read for active model):

```rust
pub struct App {
    // ... existing fields ...
    pub prefs_dao: Option<crate::persistence::PrefsDAO>,
    // ... existing fields ...
}

impl App {
    pub fn new() -> Self {
        // ... existing initialization ...

        let prefs_dao = match crate::persistence::PrefsDAO::new() {
            Ok(dao) => Some(dao),
            Err(e) => {
                eprintln!("Warning: Failed to initialize preferences DAO: {}", e);
                None
            }
        };

        // Load active model from preferences (ONLY time we read active model from DB)
        let active_model = if let Some(ref dao) = prefs_dao {
            dao.get_active_model()
                .ok()
                .flatten()
                .map(|(provider_id, model_id)| model_id)
                .unwrap_or_else(|| "claude-3-sonnet".to_string())
        } else {
            "claude-3-sonnet".to_string()
        };

        Self {
            // ... existing fields ...
            model: active_model,  // In-memory variable - used for all runtime operations
            prefs_dao,           // Used only for: startup load, saving changes
            // ... rest of initialization ...
        }
    }
}
```

**Database Access Pattern:**

- **READ**: Only once at startup to load active model into `self.model`
- **WRITE**: When user selects new model (save to DB for next restart)
- **RUNTIME**: Always use `self.model` variable, never re-read from DB

### 7. Handle Model Selection and Favorites in App

**File: `src/app.rs`**

Update key event handler for models dialog:

```rust
pub fn handle_keys(&mut self, key: KeyEvent) {
    match self.overlay_focus {
        OverlayFocus::ModelsDialog => {
            let action = handle_models_dialog_key_event(
                &mut self.models_dialog_state,
                key,
            );

            match action {
                ModelsDialogAction::SelectModel { provider_id, model_id } => {
                    // Update in-memory active model (used for all runtime operations)
                    self.model = model_id.clone();

                    // Persist to database for next app startup
                    // After this, always use self.model, never re-read from DB
                    if let Some(ref dao) = self.prefs_dao {
                        if let Err(e) = dao.set_active_model(provider_id.clone(), model_id) {
                            eprintln!("Failed to save active model: {}", e);
                        }
                    }

                    // Show feedback
                    get_toast_manager().add_toast(format!(
                        "Switched to: {}",
                        model_id
                    ));
                }
                ModelsDialogAction::ToggleFavorite { provider_id, model_id } => {
                    let is_favorite = if let Some(ref dao) = self.prefs_dao {
                        dao.toggle_favorite(provider_id.clone(), model_id.clone())
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    // Show feedback
                    get_toast_manager().add_toast(if is_favorite {
                        "Added to favorites"
                    } else {
                        "Removed from favorites"
                    });

                    // Refresh dialog to show updated favorite indicator
                    // This requires re-fetching models - for now, indicator updates on next dialog open
                }
                ModelsDialogAction::None => {}
            }
        }
        // ... rest of the code ...
    }
}
```

**Important Runtime Behavior:**

- Active model is loaded from database **only once** during `App::new()` startup
- All runtime operations use `self.model` in-memory variable
- Database is only written to when model selection changes (for persistence on next restart)
- Never read from database during normal runtime after initial load

### 8. Update Models Dialog to Show Active/Favorite Indicators with Special Groups

**File: `src/command/handlers.rs`**

```rust
pub fn handle_models<'a>(
    parsed: &'a ParsedCommand,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    // ... existing code ...

    // Use runtime variable (not DB) for active model indicator
    let active_model_id = parsed.active_model_id.clone();

    // Load preferences
    let prefs = if let Some(ref dao) = parsed.prefs_dao {
        dao.get_model_preferences().ok()
    } else {
        None
    };

    // Build model lookup map (provider_id + model_id -> model info)
    let mut model_lookup: std::collections::HashMap<(String, String), crate::model::discovery::Model> =
        std::collections::HashMap::new();

    for model in &models {
        if connected_providers.contains_key(&model.provider_id)
            && if let Some(filter) = &provider_filter {
                model.provider_id.contains(filter)
                    || model.provider_name.to_lowercase().contains(filter)
            } else {
                true
            }
        {
            model_lookup.insert(
                (model.provider_id.clone(), model.id.clone()),
                model.clone(),
            );
        }
    }

    let favorites_set = prefs
        .as_ref()
        .map(|p| {
            p.favorite
                .iter()
                .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let recent_set = prefs
        .as_ref()
        .map(|p| {
            p.recent
                .iter()
                .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let mut items: Vec<DialogItem> = Vec::new();

    // Helper function to add model as dialog item
    let add_model_item = |items: &mut Vec<DialogItem>, model: &crate::model::discovery::Model, group: &str| {
        let is_active = active_model_id.as_ref() == Some(&model.id);
        let is_favorite = favorites_set.contains(&(model.provider_id.clone(), model.id.clone()));

        let tip = if is_active {
            Some("✓ Active".to_string())
        } else if is_favorite {
            Some("★ Favorite".to_string())
        } else {
            None
        };

        items.push(DialogItem {
            id: model.id.clone(),
            name: model.name.clone(),
            group: group.to_string(),
            description: format!(
                "{} | {}",
                model.provider_name,
                model.capabilities.join(", ")
            ),
            connected: false,
            tip,
        });
    };

    // 1. Add Favorite group (favorite models that are connected)
    let favorites_list = prefs
        .as_ref()
        .map(|p| p.favorite.clone())
        .unwrap_or_default();

    if !favorites_list.is_empty() {
        let mut favorite_models = Vec::new();
        for fav in &favorites_list {
            if let Some(model) = model_lookup.get(&(fav.provider_id.clone(), fav.model_id.clone())) {
                favorite_models.push(model.clone());
            }
        }

        if !favorite_models.is_empty() {
            for model in &favorite_models {
                add_model_item(&mut items, model, "Favorite");
            }
        }
    }

    // 2. Add Recent group (recent models not in favorites, that are connected)
    let recent_list = prefs
        .as_ref()
        .map(|p| p.recent.clone())
        .unwrap_or_default();

    if !recent_list.is_empty() {
        let mut recent_models = Vec::new();
        for recent in &recent_list {
            // Skip if already in favorites
            if favorites_set.contains(&(recent.provider_id.clone(), recent.model_id.clone())) {
                continue;
            }
            if let Some(model) = model_lookup.get(&(recent.provider_id.clone(), recent.model_id.clone())) {
                recent_models.push(model.clone());
            }
        }

        if !recent_models.is_empty() {
            for model in &recent_models {
                add_model_item(&mut items, model, "Recent");
            }
        }
    }

    // 3. Add all models by provider (excluding those already in Favorite or Recent)
    let mut provider_models: std::collections::HashMap<String, Vec<crate::model::discovery::Model>> =
        std::collections::HashMap::new();

    for model in models {
        // Skip if already in favorites or recent
        let model_key = (model.provider_id.clone(), model.id.clone());
        if favorites_set.contains(&model_key) || recent_set.contains(&model_key) {
            continue;
        }

        if connected_providers.contains_key(&model.provider_id)
            && if let Some(filter) = &provider_filter {
                model.provider_id.contains(filter)
                    || model.provider_name.to_lowercase().contains(filter)
            } else {
                true
            }
        {
            provider_models
                .entry(model.provider_name.clone())
                .or_default()
                .push(model);
        }
    }

    for (provider_name, models_list) in provider_models {
        for model in &models_list {
            add_model_item(&mut items, model, &provider_name);
        }
    }

    // No need for sorting - items are already in order:
    // 1. Favorite (by order in preferences)
    // 2. Recent (by order in preferences, excluding favorites)
    // 3. By provider (alphabetically by provider name, then model name)

    // Sort provider groups alphabetically
    items.sort_by(|a, b| {
        let is_a_special = a.group == "Favorite" || a.group == "Recent";
        let is_b_special = b.group == "Favorite" || b.group == "Recent";

        if is_a_special && !is_b_special {
            return std::cmp::Ordering::Less;
        }
        if !is_a_special && is_b_special {
            return std::cmp::Ordering::Greater;
        }

        if is_a_special && is_b_special {
            // Favorite before Recent
            if a.group == "Favorite" && b.group != "Favorite" {
                return std::cmp::Ordering::Less;
            }
            if a.group != "Favorite" && b.group == "Favorite" {
                return std::cmp::Ordering::Greater;
            }
            return std::cmp::Ordering::Equal;
        }

        // Sort providers alphabetically
        a.group.cmp(&b.group).then(a.name.cmp(&b.name))
    });

    // ... rest of existing code ...
}
```

**UI Grouping Order:**

1. **Favorite** - All favorited models (in preference order)
2. **Recent** - Recently used models not in favorites (in MRU order)
3. **Provider Groups** - All other models grouped by provider (alphabetically)

**Dialog Component Update:**

The `Dialog::group_items()` method in `src/ui/components/dialog.rs` needs to recognize "Favorite" and "Recent" as special groups (similar to existing "Popular" and "Other") to ensure they appear first:

```rust
const SPECIAL_GROUPS: &[&str] = &["Favorite", "Recent", "Popular", "Other"];
```

This ensures the special groups appear in the correct order before regular provider groups.

Note: This requires passing `prefs_dao` to command handler. Update `ParsedCommand` struct and command registry accordingly.

### 9. Update Dialog Footer

**File: `src/views/models_dialog.rs`**

Add custom footer actions:

```rust
// When initializing dialog
let dialog = dialog.with_actions(vec![
    DialogAction {
        label: "Favorite".to_string(),
        key: "ctrl+f".to_string(),
    },
]);
```

**Important:** Uses runtime `active_model_id` from `ParsedCommand` (from `App::model`), not from database.

### 10. Update Command Registry for PrefsDAO Access

**File: `src/command/parser.rs`**

Add prefs_dao and active_model_id to ParsedCommand:

```rust
pub struct ParsedCommand<'a> {
    pub name: String,
    pub args: Vec<String>,
    pub raw: String,
    pub prefs_dao: Option<&'a crate::persistence::PrefsDAO>,
    pub active_model_id: Option<String>,  // Runtime value from App::model
}
```

**File: `src/command/registry.rs`**

Update command execution to pass prefs_dao and active_model_id:

```rust
pub async fn execute_command(
    &self,
    input: &str,
    app: &mut App,
    sm: &mut SessionManager,
) -> CommandResult {
    let parsed = parser::parse(input, app.prefs_dao.as_ref(), Some(app.model.clone()));

    // ... existing code ...
}
```

## Graceful Fallback Handling

The active model might not exist in the cached models list. This is handled naturally:

1. User selects a model → stored in `recent`
2. User sends a message → streaming client attempts to use the model
3. If model doesn't exist or provider is not connected → error occurs during API call
4. Error handling during chat (out of scope for this feature) will display user-friendly error

This approach keeps the model switching simple and defers validation to when the model is actually used.

## Runtime Behavior Summary

**Critical:** Active model is a runtime variable, not continuously read from database:

1. **App startup**: Load active model from DB into `self.model` (only DB read for active model)
2. **During runtime**: Always use `self.model` variable - never re-read from DB
3. **User selects new model**:
   - Update `self.model` immediately (runtime)
   - Write to DB via PrefsDAO (for next restart)
4. **Models dialog refresh**: Use `self.model` to show "✓ Active" indicator (not DB)
5. **Next app restart**: Load saved model from DB into `self.model` again

This pattern ensures:

- Fast runtime (no DB queries)
- Simple state management (single source of truth in memory)
- Persistence across restarts (saved to DB on changes)

**UI Grouping:**

- "Favorite" and "Recent" groups use preference data (read from DB on dialog open)
- These groups provide quick access to commonly-used models
- Groups are ordered: Favorite → Recent → Providers (alphabetically)

## Implementation Order

1. Add `prefs` table to migrations
2. Create PrefsDAO with all methods
3. Update persistence module exports
4. Update Dialog component to recognize "Favorite" and "Recent" as special groups
5. Add PrefsDAO to App struct
6. Load active model in App::new()
7. Update models dialog handler to return actions
8. Update app.rs handle_keys() to process actions
9. Implement Favorite/Recent grouping in handle_models()
10. Add custom footer actions to dialog
11. Update command registry to pass prefs_dao and active_model_id
12. Test complete flow

## Testing Strategy

1. **Unit Tests**: Test PrefsDAO methods (get/set active model, toggle favorite)
2. **Integration Tests**: Test persistence across restarts
3. **UI Tests**:
   - Select model with Enter
   - Toggle favorite with Ctrl+F
   - Verify indicators update correctly
   - Verify sorting (active → favorite → others)
4. **Manual Tests**:
   - Switch models and restart app
   - Add/remove favorites
   - Verify MRU behavior (recent list updates correctly)

## Edge Cases

- Empty recent list (no active model)
- Model in preferences not in current provider cache (handled gracefully)
- Database errors during prefs operations (log error, continue with defaults)
- Corrupted JSON in prefs table (return defaults, log error)

## File Changes Summary

1. **Modified**: `src/persistence/migrations.rs` - Add prefs table
2. **New**: `src/persistence/prefs.rs` - PrefsDAO implementation
3. **Modified**: `src/persistence/mod.rs` - Export prefs module
4. **Modified**: `src/ui/components/dialog.rs` - Add "Favorite" and "Recent" to SPECIAL_GROUPS
5. **Modified**: `src/views/models_dialog.rs` - Handle Select/Favorite actions
6. **Modified**: `src/app.rs` - Add PrefsDAO, load/save active model, handle actions
7. **Modified**: `src/command/parser.rs` - Add prefs_dao and active_model_id to ParsedCommand
8. **Modified**: `src/command/registry.rs` - Pass prefs_dao and active_model_id to commands
9. **Modified**: `src/command/handlers.rs` - Show active/favorite indicators, implement Favorite/Recent grouping

## Future Enhancements

- `/model` command to quick-switch models
- `/favorites` command to show only favorite models
- `/recent` command to show recently used models
- Favorite/recent count badges in dialog
- Model variant support (using the `variant` field)
