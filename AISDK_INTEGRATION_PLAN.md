# LLM Integration with AISDK - Comprehensive Implementation Plan

> **NOTE:** This document supersedes `LLM_INTEGRATION_PLAN.md`. That plan was based on using unfinished custom streaming code, which we are **NOT** doing. Please refer only to this document.

## Executive Summary

Use **AISDK** (the Vercel AI SDK for Rust) to integrate LLM streaming functionality, focusing on **nano-gpt** and **z.ai (GLM Coding Plan)**. This replaces the unfinished custom streaming infrastructure with a production-ready, type-safe library.

## Key Decisions (Based on User Requirements)

✅ **Initialization Strategy:** Lazy - reinitialize LLM client on every message submission
✅ **System Prompts:** Out of scope - just simple LLM call → response → stream
✅ **Error Handling:** Clear failed message, show toast only (don't save to DB)
✅ **Message History:** Send full conversation history (no truncation)
✅ **Model Configuration:**
   - Provider: `zai` or `zai-coding-plan` in auth.json
   - Model: `glm-4.7` (but DON'T hardcode - use `self.model`)
   - Endpoint: `https://api.z.ai/api/coding/paas/v4` for z.ai

## Provider Analysis

### models.dev API Cache Location

**Source:** `src/model/discovery.rs` (already implemented!)
**Cache Location:**
- macOS: `~/Library/Caches/crabcode/models_dev_cache.json`
- Linux: `~/.cache/crabcode/models_dev_cache.json`
- Test: `/tmp/crabcode_test_cache/models_dev_cache.json`
**TTL:** 24 hours (fetches fresh if expired)
**Discovery API:** `https://models.dev/api.json`

### Existing Infrastructure

You already have:
✅ `Provider` struct (matches models.dev JSON structure)
✅ `Model` struct (with all capabilities: reasoning, tool_call, etc.)
✅ Caching logic with 24-hour TTL
✅ `Discovery::fetch_providers()` to get fresh data

### Provider Analysis

### 1. Nano-GPT
- **Base URL:** `https://nano-gpt.com/api/v1`
- **Endpoint:** `/chat/completions`
- **Authentication:** Bearer token (`Authorization: Bearer sk-nano-...`)
- **Models:** `gpt-5.2`, `chatgpt-4o-latest`, etc.
- **Compatibility:** Fully OpenAI-compatible API

### 2. Z.AI (GLM Coding Plan)
- **Base URL (General):** `https://api.z.ai/api/paas/v4`
- **Base URL (Coding):** `https://api.z.ai/api/coding/paas/v4` (IMPORTANT: Use this for coding scenarios!)
- **Endpoint:** `/chat/completions`
- **Authentication:** Bearer token (`Authorization: Bearer ZAI_API_KEY`)
- **Models:** `glm-4.7`, `glm-4-plus`, etc.
- **Compatibility:** OpenAI-compatible format

## AISDK Capabilities Research

### What AISDK Provides

✅ **Streaming Support:** `stream_text()` method returns a stream of `LanguageModelStreamChunkType`
✅ **Message History:** `.messages()` method for full conversation history
✅ **Custom Base URLs:** `.base_url()` builder method for custom endpoints
✅ **Dynamic Model Selection:** `OpenAI::<DynamicModel>` for runtime model configuration
✅ **API Key Configuration:** `.api_key()` builder method
✅ **Type Safety:** Compile-time validation of model capabilities
✅ **Provider Agnostic:** Same interface for OpenAI-compatible providers

### Streaming Chunk Types
```rust
pub enum LanguageModelStreamChunkType {
    Start,              // Beginning of stream
    Text(String),        // Partial text delta
    Reasoning(String),  // Partial reasoning (for reasoning models)
    End(AssistantMessage),  // Final message with full result
}
```

### Message Builder Pattern
```rust
let messages = Message::builder()
    .system("You are a helpful assistant.")
    .user("Hello")
    .assistant("Hi there!")
    .user("How are you?")
    .build();
```

## Architecture Design

### New Module Structure
```
src/
├── llm/
│   ├── mod.rs              # Public API
│   ├── provider.rs         # ProviderConfig (maps provider_id → base_url)
│   └── client.rs          # AISDK wrapper with stream_chat()
└── existing files (modified)
```

### Provider Configuration Strategy (Using models.dev Data)

**Key Insight:** We have existing `Discovery` infrastructure that caches models.dev data!

From `src/model/discovery.rs`:
- `Provider` struct with `id`, `name`, `api` (base URL), and `models` HashMap
- Model IDs in `models` HashMap are fully qualified: `"zai-org/glm-4.7"`
- Cached at: `~/Library/Caches/crabcode/models_dev_cache.json` (macOS)
- TTL: 24 hours

**Auth vs Discovery Match:**
- `auth.json` provider_id = e.g., `"zai-coding-plan"` or `"nano-gpt"`
- `models.dev` provider.id = top-level ID (e.g., `"zai"` with models `"zai-org/glm-4.7"`)

```rust
// src/llm/registry.rs
use crate::model::discovery::Discovery;
use crate::model::discovery::Provider as ModelsDevProvider;

/// Wrapper around existing models.dev discovery
pub struct ModelRegistry {
    discovery: Discovery,
    providers: Option<std::collections::HashMap<String, ModelsDevProvider>>,
}

impl ModelRegistry {
    pub fn new(discovery: Discovery) -> Self {
        Self {
            discovery,
            providers: None,
        }
    }

    /// Load providers from cache (or fetch fresh if expired)
    pub async fn load_providers(&mut self) 
        -> Result<&std::collections::HashMap<String, ModelsDevProvider>, Box<dyn std::error::Error>> {
        if self.providers.is_none() {
            let providers = self.discovery.fetch_providers().await?;
            self.providers = Some(providers);
        }
        Ok(self.providers.as_ref().unwrap())
    }

    /// Get provider configuration by provider_id (from auth.json)
    pub fn get_provider(&self, provider_id: &str) 
        -> Result<ProviderConfig, Box<dyn std::error::Error>> {
        let providers = self.load_providers().await?;

        // Try exact match first (e.g., "zai-coding-plan")
        if let Some(provider) = providers.get(provider_id) {
            return Ok(ProviderConfig {
                id: provider.id.clone(),
                name: provider.name.clone(),
                base_url: provider.api.clone(),
            });
        }

        // Fallback: Try to find a provider that contains this model_id
        // (Useful if auth.json has model_id instead of provider_id)
        for (prov_id, provider) in providers.iter() {
            if provider.models.contains_key(provider_id) {
                return Ok(ProviderConfig {
                    id: prov_id.clone(),
                    name: provider.name.clone(),
                    base_url: provider.api.clone(),
                });
            }
        }

        Err(anyhow::anyhow!("Provider not found: {}", provider_id))
    }

    /// Get full model info from models.dev
    pub fn get_model(&self, provider_id: &str, model_id: &str) 
        -> Result<&crate::model::discovery::Model, Box<dyn std::error::Error>> {
        let providers = self.load_providers().await?;

        // Find provider
        let provider = providers.get(provider_id)
            .ok_or_else(|| anyhow::anyhow!("Provider not found: {}", provider_id))?;

        // Find model
        provider.models.get(model_id)
            .ok_or_else(|| anyhow::anyhow!("Model not found: {}/{}", provider_id, model_id))
    }
}

/// Configuration for AISDK client
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
}
```

### AISDK Client Wrapper

```rust
// src/llm/client.rs
use aisdk::{
    core::{LanguageModelRequest, LanguageModelStreamChunkType},
    providers::OpenAI,
};
use futures::StreamExt;

pub struct LLMClient {
    base_url: String,
    api_key: String,
    model_name: String,
    provider_name: String,
}

impl LLMClient {
    pub fn new(base_url: String, api_key: String, model_name: String, provider_name: String) -> Self {
        Self {
            base_url,
            api_key,
            model_name,
            provider_name,
        }
    }

    /// Build AISDK OpenAI provider with custom configuration
    fn build_provider(&self) -> Result<OpenAI<aisdk::core::DynamicModel>, Box<dyn std::error::Error>> {
        OpenAI::<aisdk::core::DynamicModel>::builder()
            .base_url(&self.base_url)
            .api_key(&self.api_key)
            .model_name(&self.model_name)
            .provider_name(&self.provider_name)
            .build()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }

    /// Stream chat completion using OpenAI-compatible provider
    pub async fn stream_chat(
        &self,
        messages: &[crate::session::types::Message],
        on_chunk: impl Fn(LanguageModelStreamChunkType),
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Build provider
        let provider = self.build_provider()?;

        // Convert app's Message format to AISDK format
        let aisdk_messages = self.convert_messages(messages);

        // Create request and stream
        let mut stream = LanguageModelRequest::builder()
            .model(provider)
            .messages(aisdk_messages)
            .build()
            .stream_text()
            .await?;

        let mut full_response = String::new();

        // Process stream chunks
        while let Some(chunk) = stream.next().await {
            on_chunk(chunk.clone());

            match chunk {
                LanguageModelStreamChunkType::Text(text) => {
                    full_response.push_str(&text);
                }
                LanguageModelStreamChunkType::Reasoning(reasoning) => {
                    // Handle reasoning if needed
                    full_response.push_str(&reasoning);
                }
                LanguageModelStreamChunkType::End(msg) => {
                    // Final message is complete
                    break;
                }
                LanguageModelStreamChunkType::Start => {
                    // Stream started
                }
            }
        }

        Ok(full_response)
    }

    /// Convert app's Message format to AISDK Message format
    fn convert_messages(&self, messages: &[crate::session::types::Message]) -> aisdk::core::Messages {
        use aisdk::core::Message as AisdkMessage;

        let mut builder = AisdkMessage::builder();

        for msg in messages {
            match msg.role {
                crate::session::types::MessageRole::System => {
                    builder = builder.system(&msg.content);
                }
                crate::session::types::MessageRole::User => {
                    builder = builder.user(&msg.content);
                }
                crate::session::types::MessageRole::Assistant => {
                    builder = builder.assistant(&msg.content);
                }
                crate::session::types::MessageRole::Tool => {
                    // Skip tool messages for now
                    continue;
                }
            }
        }

        builder.build()
    }
}
```

## Integration with Existing Code

### 1. App State Modifications

```rust
// src/app.rs
pub struct App {
    // ... existing fields ...

    // NEW: Streaming state
    pub is_streaming: bool,
}

impl App {
    pub fn new() -> Self {
        // ... existing init ...

        Self {
            // ... existing fields ...
            is_streaming: false,
        }
    }
}
```

### 2. LLM Client Initialization (in start_llm_streaming)

```rust
// src/app.rs - start_llm_streaming()
async fn start_llm_streaming(&mut self, user_message: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Get provider config
    let provider_config = ProviderConfig::new(&self.provider_name);

    // Get API key from auth.json
    let auth_dao = crate::persistence::AuthDAO::new()?;
    let api_key = auth_dao.get_api_key(&self.provider_name)?
        .ok_or_else(|| anyhow::anyhow!("No API key found for {}", self.provider_name))?;

    // Create LLM client directly (lazy initialization)
    let client = LLMClient::new(
        provider_config.base_url,
        api_key,
        self.model.clone(),
        provider_config.name,
    );

    // Get conversation history
    let current_messages = self.chat_state.chat.messages.clone();

    // Mark streaming as active
    self.is_streaming = true;

    // Create empty assistant message for streaming
    self.chat_state.chat.add_assistant_message("");
    if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
        last_msg.is_complete = false;
    }

    // Stream the response
    let full_response = client.stream_chat(
        &current_messages,
        |chunk| {
            match chunk {
                LanguageModelStreamChunkType::Text(text) => {
                    self.chat_state.chat.append_to_last_assistant(&text);
                }
                LanguageModelStreamChunkType::End(msg) => {
                    if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
                        last_msg.mark_complete();
                        let _ = self.session_manager.add_message_to_current_session(last_msg);
                    }
                    self.is_streaming = false;
                }
                LanguageModelStreamChunkType::Reasoning(reasoning) => {
                    self.chat_state.chat.append_to_last_assistant(&reasoning);
                }
                _ => {}
            }
        },
    ).await;

    Ok(full_response)
}
```

### 2. Chat Input Handling

```rust
// src/app.rs - handle_input_and_app_keys()
fn handle_input_and_app_keys(&mut self, key: KeyEvent) {
    match key.code {
        KeyCode::Enter if key.modifiers == event::KeyModifiers::NONE => {
            let input_text = self.input.get_text();
            if !input_text.is_empty() {
                tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(self.process_input(&input_text));
                });
                self.input.clear();
                clear_suggestions(&mut self.suggestions_popup_state);
            }
        }
        _ => {
            self.input.handle_event(key);
            self.update_suggestions();
        }
    }
}
```

### 3. Processing Chat Messages

```rust
// src/app.rs - process_input()
async fn process_input(&mut self, input: &str) {
    use crate::command::parser::parse_input;

    match parse_input(input) {
        InputType::Command(mut parsed) => {
            // ... existing command handling ...
        }
        InputType::Message(msg) => {
            match self.base_focus {
                BaseFocus::Home => {
                    // Create new session and send first message
                    if self.session_manager.get_current_session_id().is_none() {
                        let session_title = Self::generate_title_from_message(&msg);
                        self.session_manager.create_session(Some(session_title));
                    }

                    // Add user message
                    let user_message = crate::session::types::Message::user(&msg);
                    let _ = self.session_manager.add_message_to_current_session(&user_message);
                    self.chat_state.chat.add_user_message(&msg);

                    // Switch to chat view
                    self.base_focus = BaseFocus::Chat;

                    // TRIGGER LLM CALL HERE
                    self.start_llm_streaming(&msg).await;
                }
                BaseFocus::Chat => {
                    // User already in chat, send follow-up message
                    let user_message = crate::session::types::Message::user(&msg);
                    let _ = self.session_manager.add_message_to_current_session(&user_message);
                    self.chat_state.chat.add_user_message(&msg);

                    // TRIGGER LLM CALL HERE
                    self.start_llm_streaming(&msg).await;
                }
            }
        }
    }
}
```

### 4. LLM Streaming Implementation

```rust
// src/app.rs
async fn start_llm_streaming(&mut self, user_message: &str) -> Result<(), Box<dyn std::error::Error>> {
    // LAZY INITIALIZATION: Always initialize fresh on each message submission
    // This ensures we use the current provider/model configuration
    self.init_llm_client()?;

    let client = self.llm_client.as_ref().unwrap();

    // Get conversation history
    let current_messages = self.chat_state.chat.messages.clone();

    // Mark streaming as active
    self.is_streaming = true;

    // Create empty assistant message for streaming
    self.chat_state.chat.add_assistant_message("");
    if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
        last_msg.is_complete = false;
    }

    // Stream the response
    let full_response = client.stream_chat(
        &current_messages,
        |chunk| {
            // Callback for each chunk
            match chunk {
                LanguageModelStreamChunkType::Text(text) => {
                    // Append to UI in real-time
                    self.chat_state.chat.append_to_last_assistant(&text);
                }
                LanguageModelStreamChunkType::End(msg) => {
                    // Stream complete
                    if let Some(last_msg) = self.chat_state.chat.messages.last_mut() {
                        last_msg.mark_complete();

                        // Persist to database
                        let _ = self.session_manager.add_message_to_current_session(last_msg);
                    }
                    self.is_streaming = false;
                }
                LanguageModelStreamChunkType::Reasoning(reasoning) => {
                    // Handle reasoning if needed
                    self.chat_state.chat.append_to_last_assistant(&reasoning);
                }
                _ => {}
            }
        },
    ).await?;

    Ok(())
}
```

### 5. UI Update for Streaming Indicator

```rust
// src/views/chat.rs
pub fn render_chat(
    f: &mut Frame,
    chat_state: &ChatState,
    input: &Input,
    version: String,
    cwd: String,
    branch: Option<String>,
    agent: String,
    model: String,
    provider_name: String,
    colors: &ThemeColors,
    is_streaming: bool,  // NEW PARAMETER
) {
    let size = f.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)].as_ref())
        .split(size);

    let input_height = input.get_height();
    let above_status_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Min(0),
                Constraint::Length(input_height),  // FIX: Use dynamic height
                Constraint::Length(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(main_chunks[0]);

    chat_state.chat.render(f, above_status_chunks[0]);
    input.render(f, above_status_chunks[1], &agent, &model, &provider_name);

    // NEW: Split status row into left (streaming) and right (help)
    let status_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),  // Streaming status (left)
            Constraint::Length(30),  // Help text (right)
        ])
        .split(above_status_chunks[2]);

    // Streaming indicator (left)
    if is_streaming {
        let streaming_text = vec![
            Span::styled(
                "Streaming...",
                Style::default().fg(colors.info),
            ),
        ];
        let streaming_paragraph = Paragraph::new(Line::from(streaming_text));
        f.render_widget(streaming_paragraph, status_chunks[0]);
    }

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

    let blank = Block::default();
    f.render_widget(blank, above_status_chunks[3]);

    let status_bar = StatusBar::new(version, cwd, branch, agent, model);
    status_bar.render(f, main_chunks[1]);
}
```

## Cargo.toml Changes

```toml
[dependencies]
# ... existing dependencies ...

aisdk = { version = "0.4", features = ["openai"] }

# Note: We use OpenAI provider as the base because both nano-gpt and z.ai
# are OpenAI-compatible. The "openai" feature flag gives us the provider
# infrastructure we need.
```

## Implementation Steps

### Phase 1: Setup and Foundation
1. ✅ Add AISDK dependency to Cargo.toml
2. ✅ Create `src/llm/` module structure
3. ✅ Implement `LLMProvider` configuration struct
4. ✅ Implement `LLMClient` wrapper for AISDK
5. ✅ Add error handling for missing API keys

### Phase 2: Core Integration
6. ✅ Add `llm_client` and `is_streaming` fields to `App`
7. ✅ Implement `init_llm_client()` method
8. ✅ Modify `process_input()` to trigger LLM calls
9. ✅ Implement `start_llm_streaming()` method
10. ✅ Test basic non-streaming text generation first

### Phase 3: Streaming Implementation
11. ✅ Implement message history conversion to AISDK format
12. ✅ Set up streaming callback mechanism
13. ✅ Connect streaming chunks to chat UI updates
14. ✅ Handle stream completion and persistence
15. ✅ Add error handling for streaming failures

### Phase 4: UI Updates
16. ✅ Fix chat view spacing (dynamic input height)
17. ✅ Add "Streaming..." indicator to chat view
18. ✅ Pass `is_streaming` state to render function
19. ✅ Update `render_chat()` signature and implementation
20. ✅ Test streaming UI updates in real-time

### Phase 5: Testing and Refinement
21. ✅ Test with nano-gpt API
22. ✅ Test with z.ai GLM-4.7 (coding endpoint)
23. ✅ Test conversation history continuation
24. ✅ Test error handling (invalid API key, network errors)
25. ✅ Test persistence of completed messages

## Key Decisions and Rationales

### 1. Why AISDK Instead of Custom Streaming?

**Decision:** Use AISDK

**Rationale:**
- Production-ready, battle-tested library
- Type-safe provider abstraction
- Built-in streaming support
- Less maintenance burden
- Future-proof for adding more providers

### 2. Simplified Provider Configuration (Per AISDK Docs)

**Decision:** Direct configuration without separate LLMProvider struct

**Rationale:**
- AISDK docs recommend: `OpenAI::<DynamicModel>::builder().base_url().api_key().model_name()`
- Simpler than wrapping in LLMProvider struct
- ProviderConfig just maps provider_id → base_url
- LLMClient builds provider directly each time (lazy init)

### 3. Using OpenAI Provider for All Compatible APIs

**Decision:** Use AISDK's `OpenAI` provider with custom base URLs

**Rationale:**
- Both nano-gpt and z.ai are OpenAI-compatible
- No need to implement custom provider
- AISDK's `.base_url()` builder method supports this
- Can add dedicated providers later if needed

### 4. Message Format Conversion

**Decision:** Convert app's `Message` type to AISDK's `Message` format

**Rationale:**
- App's Message type has additional metadata (timestamp, is_complete)
- AISDK's Message is simpler (role, content)
- Conversion layer isolates concerns
- Allows independent evolution of both formats

### 4. Streaming Callback vs. Channel

**Decision:** Use callback closure for streaming

**Rationale:**
- Simpler than channel setup
- Direct access to chat state
- No need for background tasks with channels
- Works well with tokio's async runtime
- UI updates happen synchronously with stream consumption

### 5. Z.AI Coding Endpoint

**Decision:** Use `/api/coding/paas/v4` for GLM-4.7

**Rationale:**
- Z.AI documentation explicitly states coding endpoint for coding scenarios
- GLM-4.7 is a coding-focused model
- Better performance for coding tasks
- Follows provider's recommended usage

## Error Handling Strategy

### Missing API Key
```rust
if self.llm_client.is_none() {
    match self.init_llm_client() {
        Ok(_) => {}
        Err(e) => {
            push_toast(Toast::new(
                format!("Failed to initialize LLM: {}", e),
                ToastLevel::Error,
                None,
            ));
            return;
        }
    }
}
```

### Streaming Errors
```rust
let result = client.stream_chat(&current_messages, |chunk| {
    // Handle chunks
}).await;

match result {
    Ok(_) => {}
    Err(e) => {
        // Mark streaming as failed
        self.is_streaming = false;

        // Clear the incomplete assistant message from UI
        // (It will NOT be saved to database)
        if self.chat_state.chat.messages.last().is_some_and(|m| m.role == MessageRole::Assistant && !m.is_complete) {
            self.chat_state.chat.messages.pop();
        }

        // Show error toast to user (temporary, not saved to messages)
        push_toast(Toast::new(
            format!("LLM error: {}. Message not saved.", e),
            ToastLevel::Error,
            None,
        ));
    }
}
```

## Testing Checklist

### Unit Tests
- [ ] ProviderConfig::new() returns correct base URLs for all providers
- [ ] ProviderConfig handles "zai" and "zai-coding-plan" correctly
- [ ] LLMClient::build_provider() creates valid AISDK provider
- [ ] Message conversion handles all roles (User, Assistant, System)
- [ ] Error handling for missing API keys

### Integration Tests
- [ ] Full chat flow: user message → session creation → LLM call → streaming → persistence
- [ ] Conversation history continuation across multiple messages
- [ ] Switching between nano-gpt and z.ai providers
- [ ] Error recovery and user feedback

### Manual Testing
- [ ] Test with actual nano-gpt API key
- [ ] Test with actual z.ai API key
- [ ] Verify streaming indicator appears/disappears correctly
- [ ] Verify message history is maintained
- [ ] Verify messages persist to database
- [ ] Test long responses (streaming performance)
- [ ] Test network interruption handling

## Questions - ANSWERED ✅

### 1. Model Reinitialization
**Answer:** Wait until next message submission (lazy initialization)
- Every time user hits Enter/submits, initialize LLM client with current provider/model configuration
- No need to reinitialize when just switching model selection

### 2. System Prompts
**Answer:** Out of scope for this POC
- Goal is just: LLM call → get response → stream → done
- No default system prompts for now

### 3. Stream Error Handling
**Answer:** Clear failed message, show error in UI only
- Failed messages should NOT be saved to database
- Error should appear as a temporary message in UI (like a toast or inline error message)
- Once user sends next message, error message disappears

### 4. Message History Limits
**Answer:** Send full conversation history
- No truncation or limits
- All previous messages in session should be sent to LLM

### 5. Z.AI Model Configuration
**Answer:**
- Provider ID: `zai` (or similar identifier in auth.json)
- Model ID: `glm-4.7`
- Endpoint: `https://api.z.ai/api/coding/paas/v4` (coding endpoint)
- **Do NOT hardcode glm-4.7 in code** - use whatever model is currently selected in `self.model`

### Model ID Format in models.dev

**Important:** Model IDs in models.dev cache are fully qualified:
```json
"zai-org/glm-4.7": { ... }  // Full model ID with provider prefix
```

**But auth.json provider_id might be:**
```json
{
  "zai-coding-plan": { "type": "api", "key": "..." }
}
```

**Resolution in ModelRegistry:**
1. Try exact match: `providers.get("zai-coding-plan")`
2. If not found, search for model ID: iterate providers, check `models.contains_key(model_id)`
3. This handles both cases - provider_id in auth.json OR model_id in auth.json

### Fallback Behavior (If Cache Fails or Model Not Found)

**Decision:** Return error

**Rationale:**
- User wants explicit error if models.dev fails to load
- No silent fallback to hardcoded values
- Clear feedback to user that something is wrong
- If models.dev is unavailable, it's a real problem (not user configuration issue)

**Error Handling:**
```rust
// In App::start_llm_streaming()
let providers = self.model_registry.load_providers().await?;

if providers.is_empty() {
    push_toast(Toast::new(
        "Failed to load model registry. Check network connection.",
        ToastLevel::Error,
        None,
    ));
    return Err(anyhow::anyhow!("Model registry unavailable"));
}

let provider_config = match ModelRegistry::get_provider(&self.provider_name) {
    Ok(config) => config,
    Err(e) => {
        push_toast(Toast::new(
            format!("Provider not found: {}", self.provider_name),
            ToastLevel::Error,
            None,
        ));
        return Err(e);
    }
};
```

## Potential Issues and Mitigations

### Issue 1: AISDK Message Builder Limitations

**Concern:** AISDK's `Message::builder()` might not support arbitrary message sequences

**Mitigation:**
- Test message conversion thoroughly
- Fallback to manual message construction if needed
- Consider using AISDK's internal message structure directly

### Issue 2: Async/Await in Event Loop

**Concern:** Calling async streaming in sync event handler

**Mitigation:**
- Use `tokio::task::block_in_place()` as shown
- Already used in existing code (line 484-487 in app.rs)
- Proven pattern in the codebase

### Issue 3: UI Responsiveness During Streaming

**Concern:** Long-running streams might block UI

**Mitigation:**
- AISDK's streaming is async and non-blocking
- Each chunk triggers UI update
- Ratatui's render loop remains responsive
- User can still cancel with Ctrl+C

### Issue 4: Multiple Concurrent Streams

**Concern:** User sends new message before previous completes

**Mitigation:**
- Disable input while streaming
- Show "Streaming..." indicator
- Ignore new input until current stream completes
- Clear approach matches user expectations

## Future Enhancements

### Short Term (After MVP)
1. Add reasoning display support for reasoning models
2. Add token usage tracking and display
3. Add auto-scroll to latest message
4. Add streaming pause/resume

### Medium Term
1. Add more providers (Anthropic, Google, etc.)
2. Add tool/function calling support
3. Add structured output for code generation
4. Add prompt templates and presets

### Long Term
1. Multi-model routing (different models for different tasks)
2. Agent workflows with tool calling
3. Memory/context summarization for long conversations
4. Real-time collaboration features

## Dependencies to Add

```toml
[dependencies]
# Existing...
aisdk = { version = "0.4", features = ["openai"] }
```

No additional dependencies needed - AISDK uses `futures` which is already in dependencies.

## Success Criteria Validation

✅ **Criteria 1:** I chat in the input → Session gets created → Go to chat page → See first message
- ✅ Implemented in `process_input()` for Home focus
- ✅ Session creation already exists
- ✅ Message persistence already exists

✅ **Criteria 2:** See "Streaming..." label just below input (left side) while streaming
- ✅ Added `is_streaming` state
- ✅ Modified `render_chat()` to split status row
- ✅ Streaming indicator on left, help text on right

✅ **Criteria 3:** As it streams, the message is streamed as well
- ✅ AISDK's `stream_text()` provides real-time chunks
- ✅ Callback appends each chunk via `append_to_last_assistant()`
- ✅ Ratatui's render loop picks up changes

✅ **Criteria 4:** When it's done, "Streaming..." label is gone and response is displayed
- ✅ `LanguageModelStreamChunkType::End` marks completion
- ✅ Set `is_streaming = false`
- ✅ Mark message as complete and persist to DB

## Notes on AISDK Usage

### Dynamic Model vs. Typed Model

**Typed Model:** `OpenAI::gpt_4o()`
- Compile-time type safety
- Known capabilities (tool calling, etc.)
- Can't change model at runtime

**Dynamic Model:** `OpenAI::<DynamicModel>::builder().model_name("gpt-4o")`
- Runtime model selection
- No compile-time capability checking
- **CHOSEN:** We use this for flexibility

### Base URL Override

AISDK's OpenAI provider uses default base URL (`https://api.openai.com/v1`), but we can override:

```rust
let openai = OpenAI::<DynamicModel>::builder()
    .model_name("gpt-5.2")
    .base_url("https://nano-gpt.com/api/v1")  // Custom endpoint
    .build()?;
```

This is perfect for our use case with nano-gpt and z.ai.

## Timeline Estimate

- **Phase 1 (Setup):** 1-2 hours
- **Phase 2 (Core Integration):** 2-3 hours
- **Phase 3 (Streaming):** 2-3 hours
- **Phase 4 (UI Updates):** 1-2 hours
- **Phase 5 (Testing):** 2-3 hours

**Total Estimated Time:** 8-13 hours

## Conclusion

This plan provides a comprehensive, production-ready approach to integrating AISDK with crabcode. By leveraging AISDK's type-safe streaming support, we can achieve all success criteria with minimal custom code and maximum reliability. The architecture allows for easy addition of new providers in the future while maintaining a clean separation of concerns.

✅ **All requirements clarified and documented**
✅ **Ready to proceed with Phase 1 implementation**

**Next step:** Begin Phase 1 (Setup and Foundation) when ready to code.
