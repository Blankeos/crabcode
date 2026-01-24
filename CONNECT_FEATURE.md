# Connect Dialog Feature Plan

## Overview
Implement a new `/connect` dialog that allows users to view and configure API providers. The UI will be similar to the existing models dialog with some key differences, including provider connection status indicators and an API key input overlay.

## UI Requirements

### Main Dialog - "Connect a provider"
- **Title**: "Connect a provider" (instead of "Available Models")
- **ESC behavior**: Same as models dialog - closes the dialog
- **Search**: Same UX with fuzzy search using nucleo_matcher
- **List layout**: Grouped scrollable list (exactly the same as models dialog)

### List Items with Connection Status
Each provider item displays with a connection status indicator:
```
<Provider Name>                🟢 Connected
```
- Provider name left-aligned
- Connection status right-aligned (only shown when connected)
- Uses `justify_between` layout
- Green circle emoji (🟢) + "Connected" text

### API Key Input Overlay
When Enter is pressed on a provider, opens a separate overlay:
```
API key                  esc

Paste here (this is a placeholder)

enter submit
```
- Title: "API key"
- Control tip: "esc" (right-aligned)
- Input area: "Paste here" placeholder text
- Control tip: "enter submit" (muted/dimmed style)

## Persistence Integration (auth.json)

The connect dialog integrates with the persistence layer defined in PERSISTENCE.md:

### auth.json Location
- Path: `~/.local/share/crabcode/auth.json`
- Current implementation discrepancy: `src/config.rs::ApiKeyConfig` uses `~/.config/crabcode/api_keys.json`
- **Action needed**: Migrate to use correct auth.json location

### auth.json Format
```json
{
  "providers": {
    "openai": "sk-...",
    "anthropic": "sk-ant-...",
    "openrouter": "sk-or-..."
  },
  "default_provider": "openai"
}
```

### Connection Status Detection
A provider is considered "connected" if:
- Provider ID exists in `auth.json.providers`
- The API key value is non-empty

### Implementation Approach

#### Option A: Update Existing ApiKeyConfig
Modify `src/config.rs::ApiKeyConfig` to use auth.json format:
- Change path to `~/.local/share/crabcode/auth.json`
- Update JSON structure to match PERSISTENCE.md format
- Add `default_provider` field
- Maintain backward compatibility or migrate existing keys

#### Option B: Create New AuthConfig
Create new `src/persistence/auth.rs` with proper auth.json handling:
- New `AuthConfig` struct matching PERSISTENCE.md spec
- Methods: `load()`, `save()`, `has_provider()`, `get_provider_key()`, `set_provider_key()`, `set_default_provider()`
- Deprecate or migrate `ApiKeyConfig`

**Recommendation**: Option B - Create new persistence layer module and migrate to it, keeping the architecture aligned with PERSISTENCE.md.

## Component Reuse Analysis

### Dialog Component (src/ui/components/dialog.rs)
**Current Usage**: Used by models_dialog.rs as the base dialog widget.

**Highly Reusable Parts**:
- Fuzzy search functionality with nucleo_matcher
- Grouping and filtering logic
- Scrollbar management
- Key event handling (ESC, arrow keys)
- Mouse event handling (scroll, click selection)

**Modifications Needed**:
1. **Connection Status**: DialogItem needs to track whether provider is connected
   - Option: Add `connected: bool` field to DialogItem
   - Option: Create new `ProviderDialogItem` type

2. **Item Rendering**: Modify render loop to show connection status
   - Current: `  {item.name}` with simple padding
   - New: `{item_name}` with `{status}` on right when connected

3. **Footer Control Tips**: Update to show relevant actions
   - Current: Shows "Connect provider ctrl+a" and "Favorite ctrl+f"
   - New: Show "Enter to configure" or similar action hint

### Connection Status Detection Logic
```rust
// In connect dialog initialization, check auth.json:
fn is_provider_connected(provider_id: &str) -> bool {
    let auth = AuthConfig::load().unwrap_or_default();
    auth.providers.contains_key(provider_id)
        && auth.providers.get(provider_id)
           .map(|key| !key.is_empty())
           .unwrap_or(false)
}
```

### Recommendation: Create Generic Dialog with Configurable Item Renderer
The Dialog component is already quite generic. The main difference is the item rendering logic. Consider:
- **Option A**: Add `connected` field to existing DialogItem (simpler, less code)
- **Option B**: Make item rendering configurable via a closure/trait (more flexible, more complex)
- **Option C**: Create new ProviderDialog component based on Dialog (clean separation)

**Recommendation**: Option A - Add `connected: bool` to DialogItem and make rendering conditional on this field. This is minimal and keeps the component reusable.

## Implementation Plan

### Phase 1: Update Dialog Component
1. **Modify DialogItem** (src/ui/components/dialog.rs):
   ```rust
   pub struct DialogItem {
       pub id: String,
       pub name: String,
       pub group: String,
       pub description: String,
       pub connected: bool,  // NEW: Track connection status
   }
   ```

2. **Update Dialog::render** (src/ui/components/dialog.rs:561-578):
   - Modify item rendering to show connection status
   - Use `ratatui::layout::Alignment::Left` with padding for justification
   - Add green circle + "Connected" on right when `connected: true`

3. **Update Dialog::handle_key_event** (src/ui/components/dialog.rs:295-324):
   - Modify Enter key behavior to return an action that indicates item was selected
   - Currently returns `true` (handled) but doesn't communicate which item
   - Need to return selected item info

### Phase 2: Create Connect Dialog View
1. **New file**: src/views/connect_dialog.rs
   - Similar structure to src/views/models_dialog.rs
   - Wrap Dialog component with state management
   - Initialize items with `connected: bool` based on auth.json check
   - Export functions:
     - `init_connect_dialog(title, items) -> ConnectDialogState`
     - `render_connect_dialog(f, state, area)`
     - `handle_connect_dialog_key_event(state, event) -> Option<DialogItem>`
     - `handle_connect_dialog_mouse_event(state, event)`

2. **Update src/views/mod.rs**:
   - Add `mod connect_dialog;`
   - Export `ConnectDialogState`

### Phase 3: Create API Key Input Overlay
1. **New file**: src/ui/components/api_key_input.rs
   - Similar structure to Dialog but simpler
   - State:
     ```rust
     pub struct ApiKeyInput {
         pub visible: bool,
         pub provider_name: String,
         pub api_key: String,
         pub text_area: TextArea<'static>,
     }
     ```
   - Methods:
     - `show(provider_name)`
     - `hide()`
     - `render(f, area)`
     - `handle_key_event(event) -> InputAction`
   - InputAction enum: `Submitted(key)`, `Cancelled`, `Continue`

2. **Update src/ui/components/mod.rs**:
   - Add `mod api_key_input;`

### Phase 4: Update Command Handler
1. **Modify handle_connect** (src/command/handlers.rs:51-98):
   - Get list of available providers from model/discovery or hardcoded list
   - Load auth.json to check which providers are configured
   - Build DialogItem list with `connected: bool` set based on auth.json
   - Return `CommandResult::ShowDialog` with provider items

2. **Provider Discovery** (if needed):
   - May need to add provider listing functionality
   - Could use existing Discovery or add ProviderRegistry
   - For now, could start with hardcoded list of supported providers

### Phase 5: Update App State
1. **Modify App struct** (src/app.rs:38-58):
   ```rust
   pub struct App {
       // ... existing fields
       pub connect_dialog_state: ConnectDialogState,
       pub api_key_input_state: ApiKeyInput,
   }
   ```

2. **Update OverlayFocus enum** (src/app.rs:31-36):
   ```rust
   pub enum OverlayFocus {
       None,
       ModelsDialog,
       ConnectDialog,     // NEW
       ApiKeyInput,       // NEW
       SuggestionsPopup,
   }
   ```

3. **Update App::new** (src/app.rs:61-105):
   - Initialize connect_dialog_state
   - Initialize api_key_input_state

4. **Update App::handle_keys** (src/app.rs:167-221):
   - Add case for OverlayFocus::ConnectDialog
   - Add case for OverlayFocus::ApiKeyInput
   - Handle dialog item selection to open API key input

5. **Update App::render** (src/app.rs:376-438):
   - Render connect_dialog when OverlayFocus::ConnectDialog
   - Render api_key_input when OverlayFocus::ApiKeyInput
   - Ensure proper layering (both overlay over main content)

### Phase 6: Create/Update Persistence Layer

1. **Create src/persistence/auth.rs** (NEW):
   ```rust
   use serde::{Deserialize, Serialize};
   use std::collections::HashMap;
   use std::path::PathBuf;

   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct AuthConfig {
       pub providers: HashMap<String, String>,
       pub default_provider: String,
   }

   impl Default for AuthConfig {
       fn default() -> Self {
           Self {
               providers: HashMap::new(),
               default_provider: String::new(),
           }
       }
   }

   impl AuthConfig {
       pub fn load() -> Result<Self, Error> {
           let path = Self::auth_path();
           if path.exists() {
               let content = fs::read_to_string(&path)?;
               let config: AuthConfig = serde_json::from_str(&content)?;
               Ok(config)
           } else {
               Ok(Self::default())
           }
       }

       pub fn save(&self) -> Result<()> {
           let path = Self::auth_path();
           if let Some(parent) = path.parent() {
               fs::create_dir_all(parent)?;
           }
           let content = serde_json::to_string_pretty(self)?;
           fs::write(&path, content)?;
           Ok(())
       }

       pub fn set_provider_key(&mut self, provider: String, api_key: String) {
           self.providers.insert(provider, api_key);
       }

       pub fn get_provider_key(&self, provider: &str) -> Option<&String> {
           self.providers.get(provider)
       }

       pub fn has_provider(&self, provider: &str) -> bool {
           self.providers.contains_key(provider)
               && self.providers.get(provider)
                  .map(|key| !key.is_empty())
                  .unwrap_or(false)
       }

       pub fn set_default_provider(&mut self, provider: String) {
           self.default_provider = provider;
       }

       fn auth_path() -> PathBuf {
           dirs::data_local_dir()
               .unwrap_or_else(|| PathBuf::from("."))
               .join("crabcode")
               .join("auth.json")
       }
   }
   ```

2. **Create src/persistence/mod.rs** (NEW):
   ```rust
   pub mod auth;

   pub use auth::AuthConfig;
   ```

3. **Update src/lib.rs or src/main.rs**:
   - Add `pub mod persistence;` to expose the module

4. **Integration with connect dialog**:
   - When API key input submits:
     - Load AuthConfig
     - Call `auth_config.set_provider_key(provider_id, api_key)`
     - Call `auth_config.save()`
     - Update connection status in dialog
     - Close API key input and return to connect dialog

5. **Migration from old ApiKeyConfig** (OPTIONAL):
   - If existing users have `~/.config/crabcode/api_keys.json`:
     - On startup, check if old file exists
     - Migrate keys to new `~/.local/share/crabcode/auth.json`
     - Optionally backup old file

## File Structure

```
src/
├── ui/
│   └── components/
│       ├── dialog.rs              // MODIFIED: Add connected field
│       ├── api_key_input.rs       // NEW: API key input overlay
│       └── mod.rs                // MODIFIED: Export api_key_input
├── views/
│   ├── connect_dialog.rs          // NEW: Connect dialog view
│   ├── models_dialog.rs           // (reference)
│   └── mod.rs                     // MODIFIED: Export ConnectDialogState
├── command/
│   └── handlers.rs                // MODIFIED: Update handle_connect
├── persistence/                   // NEW: Persistence layer
│   ├── mod.rs                     // Export AuthConfig
│   └── auth.rs                    // AuthConfig for auth.json handling
├── app.rs                         // MODIFIED: Add connect_dialog and api_key_input states
└── config.rs                      // (DEPRECATED: Migrate to persistence::AuthConfig)
```

## Key Design Decisions

### 1. Persistence Layer Migration
Create new `persistence::AuthConfig` instead of using existing `config::ApiKeyConfig` because:
- Aligns with PERSISTENCE.md specification
- Uses correct file location: `~/.local/share/crabcode/auth.json`
- Supports `default_provider` field
- Better separation of concerns (credentials vs config)
- Enables future migration from old API key storage

### 2. DialogItem Extension
Add `connected: bool` to DialogItem instead of creating ProviderDialogItem because:
- Minimal code changes
- Keeps Dialog component generic and reusable
- Other dialogs can simply set `connected: false` or ignore the field

### 3. API Key Input as Separate Component
Create dedicated ApiKeyInput component instead of reusing Dialog because:
- Simpler UI (no search, no grouping, single item)
- Different behavior (input field vs list selection)
- Cleaner separation of concerns

### 4. OverlayFocus Management
Add separate variants for ConnectDialog and ApiKeyInput because:
- They need to be mutually exclusive
- Different key handling requirements
- Clear focus management hierarchy

## Testing Considerations

### Unit Tests Needed
1. **Dialog with connected status**:
   - Test item rendering shows status when connected
   - Test item rendering hides status when not connected

2. **ConnectDialog**:
   - Test initialization with items
   - Test key event handling
   - Test selection returns correct item

3. **ApiKeyInput**:
   - Test show/hide functionality
   - Test text input handling
   - Test Enter returns submitted key
   - Test Esc cancels input

4. **Integration**:
   - Test command handler returns ShowDialog with correct items
   - Test API key saving to config
   - Test connection status updates

## Dependencies

No new dependencies required. The feature uses existing dependencies from PERSISTENCE.md:
- `rusqlite` (already present for SQLite)
- `serde`, `serde_json` (already present for JSON handling)
- `dirs` (already present for path resolution)
- `ratatui`, `tui_textarea`, `nucleo_matcher` (already present for UI)

All persistence utilities match existing PERSISTENCE.md dependencies:

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
dirs = "5.0"
```

## Open Questions

1. **Migration Strategy**: How to handle existing users with `~/.config/crabcode/api_keys.json`?
   - Option A: Auto-migrate on startup (silent migration)
   - Option B: Prompt user to migrate (requires UI flow)
   - Option C: Support both formats simultaneously (complex)
   - Option D: Start fresh, users must re-enter keys (simple but disruptive)

2. **Provider Source**: Where do we get the list of available providers?
   - Option: From model/discovery (may need provider listing)
   - Option: Hardcoded list in handlers.rs
   - Option: New provider registry

2. **Provider Groups**: Should providers be grouped like models are?
   - If so, what are the groups? (e.g., "OpenAI-compatible", "Custom", etc.)

3. **Existing /connect behavior**: The current /connect command accepts args like `/connect nano-gpt sk-key`
   - Should this still work?
   - Or should the dialog be the only interface?

4. **Error Handling**: What happens if API key validation fails?
   - Show error in toast?
   - Keep API key input open with error message?

## Success Criteria

- [ ] `/connect` command opens "Connect a provider" dialog
- [ ] Providers are listed in a grouped scrollable list
- [ ] Fuzzy search works on provider names
- [ ] Connected providers show "🟢 Connected" status (checked against auth.json)
- [ ] Pressing Enter on a provider opens API key input overlay
- [ ] API key input overlay has correct layout and styling
- [ ] Submitting API key saves to auth.json and updates status
- [ ] ESC closes both dialog and overlay appropriately
- [ ] Existing models dialog continues to work unchanged
- [ ] Migration from old ApiKeyConfig to new AuthConfig is handled gracefully
