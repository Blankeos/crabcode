[DONE]

# Sessions Feature Plan

## Overview

Implement a comprehensive session management feature with creation, deletion, renaming, and viewing sessions through a dialog interface.

## Current State

- `SessionManager` exists in `src/session/manager.rs`
- Basic session types in `src/session/types.rs`
- `/sessions` command exists but only displays text output
- Dialog component with search functionality exists
- Existing dialog patterns: `models_dialog.rs`, `connect_dialog.rs`

## Implementation Tasks

### 0. Refactor Dialog Component for Generic Use

**File: `Cargo.toml`**

Ensure chrono is added as a dependency:

```toml
[dependencies]
chrono = "0.4"
```

**File: `src/ui/components/dialog.rs`**

Refactor `DialogItem` to be more generic and flexible:

```rust
#[derive(Debug)]
pub struct DialogItem {
    pub id: String,
    pub name: String,
    pub group: String,
    pub tip: Option<String>,  // Generic tip displayed on right (e.g., time, status)
}
```

Add footer actions support:

```rust
#[derive(Debug, Clone)]
pub struct DialogAction {
    pub label: String,
    pub key: String,
}

impl Dialog {
    pub fn with_actions(mut self, actions: Vec<DialogAction>) -> Self {
        self.actions = actions;
        self
    }
}
```

Update Dialog struct to include actions:

```rust
pub struct Dialog {
    pub title: String,
    pub items: Vec<DialogItem>,
    pub grouped_items: HashMap<String, Vec<DialogItem>>,
    pub filtered_items: Vec<(String, Vec<DialogItem>)>,
    pub groups: Vec<String>,
    pub selected_index: usize,
    pub visible: bool,
    pub search_query: String,
    pub scroll_offset: usize,
    pub dialog_area: Rect,
    pub content_area: Rect,
    pub search_textarea: TextArea<'static>,
    pub scrollbar_state: ScrollbarState,
    pub is_dragging_scrollbar: bool,
    pub visible_row_count: usize,
    pub actions: Vec<DialogAction>,  // Add
    matcher: Matcher,
}
```

Update render method to display tips and dynamic footer:

```rust
// In render(), when displaying items
let line = if let Some(tip) = &item.tip {
    let padding_len = (list_area_width as usize).saturating_sub(item.name.len() + tip.len() + 4);
    Line::from(vec![
        Span::raw(format!("  {}", item.name)),
        Span::raw(" ".repeat(padding_len)),
        Span::styled(
            tip,
            Style::default()
                .fg(Color::Rgb(150, 120, 100))
                .add_modifier(Modifier::DIM),
        ),
    ])
} else {
    let text_len = item.name.len() + 2;
    let padding_len = (list_area_width as usize).saturating_sub(text_len);
    Line::from(vec![
        Span::raw(format!("  {}", item.name)),
        Span::raw(" ".repeat(padding_len)),
    ])
};

// Render dynamic footer
let mut footer_spans = vec![];
for (i, action) in self.actions.iter().enumerate() {
    if i > 0 {
        footer_spans.push(Span::raw("  "));
    }
    footer_spans.push(Span::styled(
        &action.label,
        Style::default()
            .fg(Color::Rgb(255, 180, 120))
            .add_modifier(Modifier::BOLD),
    ));
    footer_spans.push(Span::raw("  "));
    footer_spans.push(Span::styled(
        &action.key,
        Style::default()
            .fg(Color::Rgb(150, 120, 100))
            .add_modifier(Modifier::DIM),
    ));
}
```

Update existing dialogs to use new API:

**models_dialog.rs:**

```rust
let items: Vec<DialogItem> = models
    .into_iter()
    .map(|model| DialogItem {
        id: model.id.clone(),
        name: model.name.clone(),
        group: model.provider_name.clone(),
        tip: if connected {
            Some("🟢 Connected".to_string())
        } else {
            None
        },
    })
    .collect();

let mut dialog = Dialog::with_items("Models", items);
dialog = dialog.with_actions(vec![
    DialogAction { label: "Connect provider".to_string(), key: "ctrl+a".to_string() },
    DialogAction { label: "Favorite".to_string(), key: "ctrl+f".to_string() },
]);
```

**connect_dialog.rs:**

```rust
let items: Vec<DialogItem> = providers
    .into_iter()
    .map(|provider| DialogItem {
        id: provider.id.clone(),
        name: provider.name.clone(),
        group: provider.group.clone(),
        tip: if provider.connected {
            Some("🟢 Connected".to_string())
        } else {
            None
        },
    })
    .collect();
```

### 1. Extend Session Data Model

**File: `src/session/manager.rs`**

Add session metadata fields to support the dialog UI:

```rust
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,          // Add: Session display title
    pub created_at: SystemTime, // Update: Use actual creation time
    pub updated_at: SystemTime, // Add: Last modified time
    pub message_count: usize,
}
```

Add methods to `SessionManager`:

- `rename_session(id: &str, new_title: &str) -> Result<(), SessionError>`
- Update `create_session()` to set proper timestamps
- Update `list_sessions()` to return `SessionInfo` with actual creation times

**File: `src/session/types.rs`**

Add title field to `Session`:

```rust
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Session {
    pub id: String,
    pub title: String,      // Add: Session title
    pub created_at: SystemTime,  // Add
    pub updated_at: SystemTime,  // Add
    pub messages: Vec<Message>,
}
```

### 2. Create Sessions Dialog

**File: `src/views/sessions_dialog.rs`**

Create a new dialog following the pattern of `models_dialog.rs`:

```rust
use crate::ui::components::dialog::{Dialog, DialogItem};
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{layout::Rect, Frame};

#[derive(Debug)]
pub struct SessionsDialogState {
    pub dialog: Dialog,
    pub pending_delete: Option<String>,
}

// Implement init, render, handle_key_event, handle_mouse_event
// Similar to models_dialog.rs but with delete/rename actions
```

Key features:

- Reuse existing `Dialog` component
- Group sessions by date: "Today", "Sat Jan 24 2026", "Fri Jan 23 2026"
- Display time next to each session: "11:21 AM", "11:00 AM" (muted color)
- Handle ctrl+d for delete
- Handle ctrl+r for rename

### 3. Create Session Rename Dialog

**File: `src/views/session_rename_dialog.rs`**

Create a simple input dialog for renaming sessions:

```
┌─────────────────────────────────────────────────────────┐
│ Rename session                                     esc   │
│                                                         │
│ <current session title>                                 │
│                                                         │
│                                                         │
│ enter submit                                            │
└─────────────────────────────────────────────────────────┘
```

The dialog should:

- Show the current session title
- Allow editing of the title
- Submit on Enter
- Cancel on Esc
- Use a textarea for input

### 4. Update Session Manager

Add the following to `src/session/manager.rs`:

```rust
impl SessionManager {
    pub fn rename_session(&mut self, id: &str, new_title: String) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.title = new_title;
            session.updated_at = SystemTime::now();
            Ok(())
        } else {
            Err(SessionError::NotFound(id.to_string()))
        }
    }
}

#[derive(Debug)]
pub enum SessionError {
    NotFound(String),
}
```

### 5. Update Command Handler

**File: `src/command/handlers.rs`**

Modify `/sessions` command to show dialog instead of text:

```rust
pub fn handle_sessions<'a>(
    _parsed: &'a ParsedCommand,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move {
        let sessions = sm.list_sessions();

        let items: Vec<DialogItem> = sessions
            .into_iter()
            .map(|session| {
                // Calculate date group and time
                let date_group = format_date_group(session.created_at);
                let time = format_time(session.created_at);

                DialogItem {
                    id: session.id.clone(),
                    name: session.title.clone(),
                    group: date_group,
                    tip: Some(time),
                }
            })
            .collect();

        CommandResult::ShowDialog {
            title: "Sessions".to_string(),
            items,
        }
    })
}
```

Helper functions (add chrono imports at top of file):

```rust
use chrono::{DateTime, Local, Utc};

fn format_date_group(created_at: SystemTime) -> String {
    let datetime: DateTime<Local> = created_at.into();
    let now: DateTime<Local> = Utc::now().into();
    let duration = now.signed_duration_since(datetime);

    if duration.num_days() == 0 {
        "Today".to_string()
    } else {
        datetime.format("%a %b %d %Y").to_string()
    }
}

fn format_time(created_at: SystemTime) -> String {
    let datetime: DateTime<Local> = created_at.into();
    datetime.format("%-I:%M %p").to_string() // "11:21 AM"
}
```

### 6. Update App State

**File: `src/app.rs`**

Add to `OverlayFocus` enum:

```rust
pub enum OverlayFocus {
    None,
    ModelsDialog,
    ConnectDialog,
    ApiKeyInput,
    SuggestionsPopup,
    SessionsDialog,       // Add
    SessionRenameDialog,  // Add
}
```

Add state fields:

```rust
pub struct App {
    // ... existing fields
    pub sessions_dialog_state: SessionsDialogState,
    pub session_rename_dialog_state: SessionRenameDialogState,
    // ...
}
```

Initialize in `App::new()`:

```rust
let sessions_dialog_state = init_sessions_dialog("Sessions", vec![]);
let session_rename_dialog_state = init_session_rename_dialog();
```

Add to handle_keys():

```rust
OverlayFocus::SessionsDialog => {
    // Handle key events, check for ctrl+d (delete) and ctrl+r (rename)
}
OverlayFocus::SessionRenameDialog => {
    // Handle session rename dialog events
}
```

Add to render():

```rust
if self.overlay_focus == OverlayFocus::SessionsDialog
    && self.sessions_dialog_state.dialog.is_visible()
{
    render_sessions_dialog(f, &mut self.sessions_dialog_state, size);
}

if self.overlay_focus == OverlayFocus::SessionRenameDialog
    && self.session_rename_dialog_state.is_visible()
{
    render_session_rename_dialog(f, &mut self.session_rename_dialog_state, size);
}
```

### 7. Update Views Module

**File: `src/views/mod.rs`**

Export new modules:

```rust
pub mod sessions_dialog;
pub mod session_rename_dialog;

// Add to exports
pub use sessions_dialog::{SessionsDialogState, init_sessions_dialog, render_sessions_dialog};
pub use session_rename_dialog::{SessionRenameDialogState, init_session_rename_dialog, render_session_rename_dialog};
```

### 8. Auto-Create Session on First Chat

**File: `src/app.rs`**

In `process_input()` method, when a message is sent from Home view:

```rust
InputType::Message(msg) => {
    if !msg.is_empty() && self.base_focus == BaseFocus::Home {
        // Create a new session if one doesn't exist
        if self.session_manager.current_session_id.is_none() {
            let session_title = generate_title_from_message(&msg);
            self.session_manager.create_session(Some(session_title));
        }
        self.chat_state.chat.add_user_message(&msg);
        self.base_focus = BaseFocus::Chat;
    }
}

fn generate_title_from_message(message: &str) -> String {
    // Take first 30 chars, truncate at word boundary
    message.chars().take(30).collect::<String>()
        .trim_end()
        .to_string()
}
```

### 9. Sessions Dialog Footer Actions

Update sessions dialog to include footer actions:

**File: `src/views/sessions_dialog.rs`**

```rust
let mut dialog = Dialog::with_items("Sessions", items);
dialog = dialog.with_actions(vec![
    DialogAction { label: "Delete".to_string(), key: "ctrl+d".to_string() },
    DialogAction { label: "Rename".to_string(), key: "ctrl+r".to_string() },
]);
```

### 10. Key Event Handling Details

**Sessions Dialog Actions:**

Delete (ctrl+d):

```rust
KeyCode::Char('d') if event.modifiers == KeyModifiers::CONTROL => {
    if let Some(selected) = dialog_state.dialog.get_selected() {
        let session_id = selected.id.clone();
        self.session_manager.delete_session(&session_id);
        // Refresh dialog
        dialog_state.pending_delete = None;
        return true;
    }
}
```

Rename (ctrl+r):

```rust
KeyCode::Char('r') if event.modifiers == KeyModifiers::CONTROL => {
    if let Some(selected) = dialog_state.dialog.get_selected() {
        let session_id = selected.id.clone();
        let title = selected.name.clone();
        self.session_rename_dialog_state.show(session_id, title);
        self.overlay_focus = OverlayFocus::SessionRenameDialog;
        return true;
    }
}
```

**Session Rename Dialog:**

Enter to submit:

```rust
KeyCode::Enter => {
    if let Some((session_id, _)) = dialog_state.get_rename_info() {
        let new_title = dialog_state.get_input_text();
        let _ = self.session_manager.rename_session(&session_id, new_title);
        dialog_state.hide();
        self.overlay_focus = OverlayFocus::SessionsDialog;
        // Refresh sessions dialog
    }
}
```

Esc to cancel:

```rust
KeyCode::Esc => {
    dialog_state.hide();
    self.overlay_focus = OverlayFocus::SessionsDialog;
}
```

## Testing Strategy

1. **Unit Tests**
   - Test `rename_session()` in `SessionManager`
   - Test date/time formatting functions
   - Test title generation from messages

2. **Integration Tests**
   - Test creating session on first message
   - Test deleting session via dialog
   - Test renaming session via dialog
   - Test date grouping logic

3. **UI Tests**
   - Test dialog rendering with date groups
   - Test time display formatting
   - Test keyboard shortcuts (ctrl+d, ctrl+r)
   - Test rename dialog state management

## Implementation Order

0. Refactor Dialog component (add `tip` field to DialogItem, add footer actions)
1. Extend session data model (SessionInfo with title, timestamps)
2. Add `rename_session()` to SessionManager
3. Create `sessions_dialog.rs` with delete/rename handling
4. Create `session_rename_dialog.rs` for title editing
5. Update `/sessions` command to show dialog
6. Update App state to include new dialogs
7. Implement auto-create session on first chat
8. Add tests

## Dependencies

**New dependencies:**

- `chrono` - for date/time formatting (if not already in Cargo.toml)

**Existing:**

- `ratatui` for TUI
- `tui_textarea` for input

## Edge Cases

- Deleting the current session
- Renaming a session with empty title
- Sessions with same title (use unique IDs)
- Very long session titles (truncate in UI)
- Sessions created at different time zones
