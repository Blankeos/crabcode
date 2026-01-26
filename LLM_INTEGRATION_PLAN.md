# LLM Integration & Chat Streaming Plan

## Current State Analysis

### Existing Infrastructure
✅ **Persistence Layer**
- SQLite database via `HistoryDAO` (`src/persistence/history.rs`)
- Chat sessions stored with messages
- API keys stored in `auth.json` via `AuthDAO` (`src/persistence/auth.rs`)

✅ **Session Management**
- `SessionManager` handles CRUD operations (`src/session/manager.rs`)
- Session types defined in `src/session/types.rs`
- Messages support `is_complete` flag and streaming updates

✅ **Streaming Infrastructure**
- `StreamClient` for HTTP streaming (`src/streaming/client.rs`)
- `StreamParser` for SSE parsing (`src/streaming/parser.rs`)
- Supports `TextDelta`, `Done`, and `Error` events

✅ **UI Components**
- `Input` component with multi-line support (`src/ui/components/input.rs`)
- `Chat` component with message display (`src/ui/components/chat.rs`)
- `append_to_last_assistant()` for streaming updates

## Issue: Spacing Inconsistency Between Home and Chat Views

### Current Implementation

**Home View** (`src/views/home.rs:50-62`)
```rust
let input_height = input.get_height();  // DYNAMIC HEIGHT
let home_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),           // Content
        Constraint::Length(input_height),  // Dynamic input height
        Constraint::Length(1),        // Help text
        Constraint::Length(1),        // Blank line
    ].as_ref())
    .split(main_chunks[0]);
```

**Chat View** (`src/views/chat.rs:48-59`)
```rust
let above_status_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),      // Chat content
        Constraint::Length(3),  // FIXED INPUT HEIGHT
        Constraint::Length(1),  // Help text
        Constraint::Length(1),  // Blank line
    ].as_ref())
    .split(main_chunks[0]);
```

### Problem
- Home uses dynamic input height based on `input.get_height()`
- Chat uses fixed height of `3`
- This causes spacing inconsistency between the two views
- The blank line below help text is present in both, so that's not the issue

### Solution
Change Chat view to use dynamic input height like Home:
```rust
let input_height = input.get_height();
let above_status_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(0),
        Constraint::Length(input_height),  // Use dynamic height
        Constraint::Length(1),
        Constraint::Length(1),
    ].as_ref())
    .split(main_chunks[0]);
```

## AISDK Integration Approach

### Option 1: Use Existing Streaming Infrastructure (RECOMMENDED)

**Pros:**
- No new dependencies
- Already tested and working
- More control over the implementation
- Can adapt to any OpenAI-compatible API

**Cons:**
- Manual provider configuration needed
- More boilerplate code

**Implementation:**
1. Use existing `StreamClient` and `StreamParser`
2. Add provider URL configuration based on `provider_name`
3. Use API key from `AuthDAO`
4. Model ID already available in `self.model`

### Option 2: Integrate AISDK

**Pros:**
- Abstracted provider interface
- Built-in streaming support
- Type-safe API

**Cons:**
- Additional dependency
- May need adapter layer for existing data structures
- Less control over specific provider quirks

**Implementation:**
```toml
# Cargo.toml
[dependencies]
aisdk = { version = "0.4", features = ["openai"] }
```

```rust
use aisdk::core::LanguageModelRequest;
use aisdk::providers::OpenAI;

// Build request with streaming
let result = LanguageModelRequest::builder()
    .model(OpenAI::from_id(&self.model))
    .prompt(&user_input)
    .build()
    .stream_text()
    .await?;

// Process stream
while let Some(chunk) = result.next().await {
    // Append to chat
    chat_state.chat.append_to_last_assistant(&chunk);
}
```

**Recommendation:** Start with **Option 1** (existing infrastructure) since:
- It's already implemented
- No new dependencies
- We have full control
- Can migrate to aisdk later if needed

## Streaming Implementation Flow

### Success Criteria Mapping

#### 1. Chat in the input → Session gets created → Go to chat page → See first message

**Current Implementation** (`src/app.rs:697-707`)
```rust
InputType::Message(msg) => {
    if !msg.is_empty() && self.base_focus == BaseFocus::Home {
        if self.session_manager.get_current_session_id().is_none() {
            let session_title = Self::generate_title_from_message(&msg);
            self.session_manager.create_session(Some(session_title));
        }
        let user_message = crate::session::types::Message::user(&msg);
        let _ = self.session_manager.add_message_to_current_session(&user_message);
        self.chat_state.chat.add_user_message(&msg);
        self.base_focus = BaseFocus::Chat;
    }
}
```

**Status:** ✅ Already implemented

**What's missing:** LLM call when in Chat focus with a message

#### 2. Show "Streaming..." label below input (left side) while streaming

**Location to add:** `src/views/chat.rs:63-73` (currently help text on right)

**Current code:**
```rust
let help_text = vec![
    Span::styled("/", Style::default().fg(colors.info)),
    Span::raw(" commands  "),
    Span::styled("tab", Style::default().fg(colors.info)),
    Span::raw(" agents  "),
    Span::styled("ctrl+cc", Style::default().fg(colors.info)),
    Span::raw(" quit"),
];
let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
f.render_widget(help, above_status_chunks[2]);
```

**Proposed change:** Split the row into left (streaming status) and right (help)

```rust
let status_chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
        Constraint::Min(0),  // Streaming status (left)
        Constraint::Length(20),  // Help text (right)
    ])
    .split(above_status_chunks[2]);

// Streaming status (left)
let streaming_text = if is_streaming {
    vec![
        Span::styled("Streaming...", Style::default().fg(colors.info)),
    ]
} else {
    vec![]
};
let streaming_paragraph = Paragraph::new(Line::from(streaming_text));
f.render_widget(streaming_paragraph, status_chunks[0]);

// Help text (right)
let help_text = vec![
    Span::styled("/", Style::default().fg(colors.info)),
    Span::raw(" commands  "),
    Span::styled("tab", Style::default().fg(colors.info)),
    Span::raw(" agents  "),
    Span::styled("ctrl+cc", Style::default().fg(colors.info)),
    Span::raw(" quit"),
];
let help = Paragraph::new(Line::from(help_text)).alignment(Alignment::Right);
f.render_widget(help, status_chunks[1]);
```

**State tracking:** Add `is_streaming: bool` to `ChatState` or `App`

#### 3. Stream message as it arrives

**Approach 1: Polling-based (Simplest)**
- Use `tokio::task::spawn_local` or channel to receive stream updates
- Update `chat_state.chat.append_to_last_assistant(chunk)` on each chunk
- Ratatui's render loop will pick up changes

**Approach 2: Channel-based (Better)**
- Create `tokio::sync::mpsc::channel` for streaming updates
- Stream client sends chunks through channel
- App receives chunks in render loop or event handler
- Update chat state and trigger re-render

**Recommended:** Approach 2 for cleaner separation

#### 4. Remove "Streaming..." label when done

- When `StreamEvent::Done` is received, set `is_streaming = false`
- Mark last message as complete: `message.mark_complete()`
- Persist to database

## Implementation Steps

### Step 1: Fix Chat View Spacing
**File:** `src/views/chat.rs`
- Change input height from fixed `3` to dynamic `input.get_height()`
- Matches Home view behavior

### Step 2: Add Streaming State
**File:** `src/app.rs` or `src/views/chat.rs`
- Add `is_streaming: bool` to track streaming status
- Add `Option<tokio::sync::mpsc::Sender<String>>` for streaming control

### Step 3: Implement LLM Streaming Logic
**File:** `src/app.rs` (in `process_input` method)

```rust
InputType::Message(msg) => {
    match self.base_focus {
        BaseFocus::Home => {
            // Existing: Create session, add user message, switch to chat
            // ...
        }
        BaseFocus::Chat => {
            // NEW: Send to LLM and stream response
            let api_key = self.get_api_key(&self.provider_name);
            let model_url = self.get_model_url(&self.provider_name);

            // Create assistant message placeholder
            self.chat_state.chat.add_assistant_message("");
            self.chat_state.chat.messages.last_mut()
                .map(|m| m.is_complete = false);

            // Start streaming
            self.start_llm_stream(&msg, &model_url, api_key, &self.model).await;
        }
    }
}
```

### Step 4: Create Streaming Task
**File:** `src/app.rs`

```rust
async fn start_llm_stream(
    &mut self,
    user_message: &str,
    url: &str,
    api_key: Option<String>,
    model: &str,
) -> Result<()> {
    self.is_streaming = true;

    let mut stream_client = StreamClient::new();
    let stream = stream_client.stream(
        url,
        user_message,
        api_key.as_deref(),
        model
    ).await?;

    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::TextDelta(chunk) => {
                self.chat_state.chat.append_to_last_assistant(&chunk);
            }
            StreamEvent::Done => {
                if let Some(msg) = self.chat_state.chat.messages.last_mut() {
                    msg.mark_complete();
                }
                self.is_streaming = false;
                break;
            }
            StreamEvent::Error(err) => {
                // Handle error
                self.is_streaming = false;
                break;
            }
        }
    }

    Ok(())
}
```

### Step 5: Update Chat View Rendering
**File:** `src/views/chat.rs`
- Add streaming status display in input row
- Update `render_chat` to accept `is_streaming` parameter
- Show/hide "Streaming..." label

### Step 6: Provider Configuration
**File:** New file or existing configuration

```rust
// Map provider names to API endpoints
fn get_provider_url(provider_name: &str) -> &'static str {
    match provider_name {
        "openai" => "https://api.openai.com/v1/chat/completions",
        "anthropic" => "https://api.anthropic.com/v1/messages",
        "zai" => "https://api.zai.ai/v1/chat/completions",
        _ => panic!("Unknown provider: {}", provider_name),
    }
}
```

## Key Files to Modify

| File | Changes |
|------|---------|
| `src/views/chat.rs` | Fix input height, add streaming status UI |
| `src/app.rs` | Add streaming state, implement LLM call logic |
| `src/streaming/client.rs` | May need adapter for different providers |
| `src/persistence/auth.rs` | Ensure API key retrieval works |
| `src/session/manager.rs` | Add method to mark message complete |

## Data Flow Diagram

```
User Input (Enter)
    ↓
app.rs: handle_input_and_app_events()
    ↓
app.rs: process_input()
    ↓
┌─────────────────────────────────────┐
│ If BaseFocus::Home                 │
│   - Create session if needed       │
│   - Add user message to DB         │
│   - Add to chat UI                 │
│   - Switch to Chat focus           │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│ If BaseFocus::Chat                 │
│   - Add user message to DB         │
│   - Create empty assistant message │
│   - Start LLM streaming task       │
│   - Set is_streaming = true        │
└─────────────────────────────────────┘
    ↓
StreamClient::stream()
    ↓
tokio task runs in background
    ↓
┌─────────────────────────────────────┐
│ StreamEvent::TextDelta              │
│   - Append to chat.chat.messages    │
│   - UI renders automatically        │
└─────────────────────────────────────┘
    ↓
┌─────────────────────────────────────┐
│ StreamEvent::Done                   │
│   - Mark message complete           │
│   - Set is_streaming = false        │
│   - Save to DB                      │
└─────────────────────────────────────┘
```

## Alternative Approaches Considered

### 1. Using AISDK Directly
- **Pros:** Provider abstraction, less provider-specific code
- **Cons:** May need to adapt message formats, additional dependency
- **Decision:** Use existing streaming infrastructure for now, can migrate later

### 2. Polling Instead of Streaming
- **Pros:** Simpler, no async complexity
- **Cons:** Poor UX, no real-time feedback
- **Decision:** Streaming is required by success criteria

### 3. Custom UI Library for Streaming
- **Pros:** Better streaming support
- **Cons:** User specifically said "No installing libraries for UI just yet"
- **Decision:** Use ratatui's existing capabilities

## Testing Strategy

1. **Unit Tests:**
   - Test streaming state management
   - Test message appending
   - Test completion marking

2. **Integration Tests:**
   - Test full flow from input to streaming
   - Test session creation on first message
   - Test persistence

3. **Manual Tests:**
   - Test with actual API keys
   - Test streaming UI updates
   - Test error handling

## Open Questions

1. **Model Provider URLs:** Need to confirm exact API endpoints for each provider
2. **API Key Format:** Ensure auth.json keys work with streaming requests
3. **Message Format:** Verify AISDK/Streaming client expects OpenAI-compatible format
4. **Error Handling:** How to display errors to user during streaming
5. **Session Persistence:** When to save streaming messages to DB (on each chunk or on complete?)

## Next Steps

1. ✅ Fix Chat view spacing (Quick win, improves consistency)
2. ✅ Add streaming state tracking
3. ✅ Implement LLM streaming using existing infrastructure
4. ✅ Add "Streaming..." label to Chat view
5. ✅ Test full flow end-to-end
6. ✅ Handle errors gracefully
7. ✅ Persist completed messages to database

## Notes

- The existing streaming infrastructure is well-designed and testable
- Ratatui's render loop will automatically pick up changes to `chat_state.chat`
- Using tokio tasks for streaming ensures UI remains responsive
- The `is_complete` flag on messages is perfect for tracking streaming state
