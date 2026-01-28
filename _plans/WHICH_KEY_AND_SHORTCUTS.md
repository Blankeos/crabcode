# Which-Key Feature Implementation Plan

## Overview
Add a which-key style popup that displays available key bindings when the user presses a prefix key (Ctrl+X), similar to Emacs or Vim's which-key plugin.

## Requirements

### Key Bindings
- **Ctrl+X** - Prefix key to trigger which-key popup
- **Ctrl+X, M** - Open Models dialog (/models)
- **Ctrl+X, L** - Open Sessions dialog (/sessions)
- **Ctrl+X, N** - Create new session (/new)

### UI Components

1. **WhichKeyState** - New view state for managing the which-key popup
   - Track whether we're in "prefix mode" (after Ctrl+X)
   - Store the available key bindings
   - Handle timeout (auto-dismiss after inactivity)

2. **WhichKeyPopup** - Visual component
   - Display as an overlay popup
   - Show available key bindings in a grid or list format
   - Highlight the prefix key that was pressed
   - Use theme colors for styling

### Implementation Steps

#### 1. Create New View Module
**File**: `src/views/which_key.rs`

```rust
pub struct WhichKeyState {
    pub visible: bool,
    pub bindings: Vec<KeyBinding>,
    pub last_key_time: Instant,
}

pub struct KeyBinding {
    pub key: String,
    pub description: String,
    pub action: WhichKeyAction,
}

pub enum WhichKeyAction {
    ShowModels,
    ShowSessions,
    NewSession,
    None,
}
```

#### 2. Add to App State
**File**: `src/app.rs`

Add to `OverlayFocus` enum:
```rust
pub enum OverlayFocus {
    None,
    ModelsDialog,
    ConnectDialog,
    ApiKeyInput,
    SuggestionsPopup,
    SessionsDialog,
    SessionRenameDialog,
    WhichKey,  // NEW
}
```

Add to `App` struct:
```rust
pub which_key_state: WhichKeyState,
```

#### 3. Handle Key Events
**File**: `src/app.rs` in `handle_keys()`

Add handling for Ctrl+X:
```rust
KeyCode::Char('x') if key.modifiers == event::KeyModifiers::CONTROL => {
    self.overlay_focus = OverlayFocus::WhichKey;
    self.which_key_state.show();
    true
}
```

When in WhichKey mode, handle the follow-up keys:
```rust
OverlayFocus::WhichKey => {
    match key.code {
        KeyCode::Char('m') | KeyCode::Char('M') => {
            self.execute_command("/models").await;
            self.overlay_focus = OverlayFocus::None;
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            self.execute_command("/sessions").await;
            self.overlay_focus = OverlayFocus::None;
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            self.execute_command("/new").await;
            self.overlay_focus = OverlayFocus::None;
        }
        KeyCode::Esc => {
            self.overlay_focus = OverlayFocus::None;
        }
        _ => {}
    }
    true
}
```

#### 4. Render the Popup
**File**: `src/views/which_key.rs`

Create render function that displays:
- Title: "Key Bindings"
- List of available bindings with their keys and descriptions
- Styled with theme colors

#### 5. Update Main Render Loop
**File**: `src/app.rs` in `render()`

Add rendering for which-key popup when `overlay_focus == OverlayFocus::WhichKey`.

### Key Bindings Display Format

```
┌─────────────────────────────┐
│ Key Bindings                │
├─────────────────────────────┤
│ m  Open Models dialog       │
│ l  Open Sessions dialog     │
│ n  Create new session       │
│                             │
│ Press ESC to cancel         │
└─────────────────────────────┘
```

### Future Extensibility

The which-key system should be easily extensible:
- Store bindings in a configurable map
- Allow users to customize key bindings
- Support multiple prefix keys (e.g., Ctrl+C for custom commands)
- Add more actions as needed

## Files to Modify

1. `src/views/mod.rs` - Add which_key module
2. `src/views/which_key.rs` - NEW: Which-key implementation
3. `src/app.rs` - Add WhichKey to OverlayFocus, handle key events, render
4. `src/views/home.rs` - Update help text to mention Ctrl+X

## Testing Checklist

- [ ] Ctrl+X opens which-key popup
- [ ] Pressing 'm' opens Models dialog
- [ ] Pressing 'l' opens Sessions dialog
- [ ] Pressing 'n' creates new session
- [ ] Pressing ESC closes popup without action
- [ ] Popup auto-dismisses after timeout (optional)
- [ ] Theme colors are applied correctly
- [ ] Works in both Home and Chat views
