[DONE]

# Refactoring Plan

## Overview

Refactor crabcode to follow iconmate's architecture pattern with proper view isolation and minimal app state management.

## New Folder Structure

```
src/
├── views/
│   ├── mod.rs
│   ├── home.rs           (formerly landing)
│   ├── chat.rs
│   ├── models_dialog.rs  (formerly dialog - specific use case)
│   └── suggestions_popup.rs (formerly popup - specific use case)
├── app.rs                (minimal routing logic, stays as app.rs)
├── main.rs               (event loop, like iconmate's tui.rs)
├── ui/
│   └── components/
│       ├── input.rs      (keep as-is)
│       └── status_bar.rs (keep as-is)
```

## Key Changes

### 1. app.rs → Minimal Root App

- Contains only `AppFocus` enum with two layers: BaseFocus and OverlayFocus
- Contains only the App struct with focus state and references to view states
- Each view has its own state struct (like iconmate's MainState, AddPopupState)
- Key routing logic: `handlekeys()` delegates to active view based on AppFocus
- **No UI rendering code** - only state and routing

### 2. views/ - Self-Contained Views

Each view file contains:

- View-specific state struct
- `init_*()` methods (impl App) to initialize view state
- `handlekeys_*()` methods (impl App) for key handling
- `render_*()` function for UI rendering

**View responsibilities:**

- **home.rs**: Landing page with logo, input, status bar
- **chat.rs**: Chat interface with messages, input, status bar
- **models_dialog.rs**: Model selection dialog (overlay)
- **suggestions_popup.rs**: Command suggestions popup (overlay)

### 3. main.rs → Event Loop (like iconmate's tui.rs)

- Runs the event loop
- Calls `terminal.draw(|f| render_ui(f, app))`
- Delegates key events to `app.handlekeys()`
- Handles terminal setup/teardown

### 4. Focus Management (Two-Layer System)

```rust
enum BaseFocus {
    Home,
    Chat,
}

enum OverlayFocus {
    None,
    ModelsDialog,
    SuggestionsPopup,
}

enum AppFocus {
    Base(BaseFocus),
    Overlay(OverlayFocus),
}
```

**Key Behavior:**

- Base views render first
- Overlay renders on top (with Clear widget)
- When Overlay is active, Base `handlekeys` are NOT called
- When Overlay is None, Base `handlekeys` ARE called

### 5. Naming Changes

- `Dialog` → `ModelsDialog` (specific use case)
- `Popup` → `SuggestionsPopup` (specific use case)

## Implementation Steps

1. Create `src/views/` directory
2. Create `src/views/mod.rs` with exports
3. Create `src/views/home.rs` with HomeState and handlers
4. Create `src/views/chat.rs` with ChatState and handlers
5. Create `src/views/models_dialog.rs` with ModelsDialogState and handlers
6. Create `src/views/suggestions_popup.rs` with SuggestionsPopupState and handlers
7. Refactor `app.rs` to minimal routing
8. Update `main.rs` to be event loop only
9. Remove all tests from refactored files
10. Update imports across the codebase
