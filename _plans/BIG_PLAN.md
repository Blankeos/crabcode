# Crabcode - Rust AI CLI Coding Agent

## Overview

Crabcode is a Rust-based AI coding CLI tool inspired by [anomalyco/opencode](https://github.com/anomalyco/opencode). It provides a terminal UI (TUI) for interacting with AI coding agents, featuring auto-suggestions, multiple model support, agent switching, and streaming chat interface.

## MVP Scope

The MVP focuses on core features with minimal complexity:
- Native terminal UI using ratatui and tui-textarea
- Commands: `/sessions`, `/new`, `/connect`, `/models`, `/exit`
- Agent system with TAB switching (PLAN and BUILD agents only)
- Model support prioritizing nano-gpt and z.ai coding plan
- Auto-suggestions on `/` press for commands and `@` press for files
- Models.dev API integration for model discovery
- Chat-like streaming interface
- Logo display on landing page
- Select-to-copy functionality
- Status bar with version (bottom-right) and directory + git branch (bottom-left)

## Tech Stack

### Key Design Decision
**AI Abstraction Strategy**: crabcode uses [lazy-hq/aisdk](https://github.com/lazy-hq/aisdk) as the primary AI abstraction layer. This provides:
- Provider-agnostic LLM access (OpenAI, Anthropic, Google, Groq, etc.)
- Built-in streaming support
- Agent and tool system
- Structured output capabilities
- Avoids reinventing the wheel for AI interactions

### Core Dependencies
- **ratatui** - Terminal UI framework (Rust equivalent to React/SolidJS for TUI)
  - Popup example: https://ratatui.rs/examples/apps/popup/
- **tui-textarea** - Multi-line text input component
  - Repository: https://github.com/rhysd/tui-textarea
- **tokio** - Async runtime for streaming and HTTP requests
- **reqwest** - HTTP client for API calls
- **serde/serde_json** - Serialization/deserialization
- **anyhow** - Error handling
- **clap** - CLI argument parsing (optional, can use built-in command parsing)
- **ignore** - .gitignore file filtering
- **aisdk** - AI SDK for provider-agnostic LLM interactions
  - Repository: https://github.com/lazy-hq/aisdk
  - Documentation: https://aisdk.rs

### AI Integration
- **lazy-hq/aisdk** - Rust AI SDK (primary AI abstraction layer)
  - Documentation: https://aisdk.rs
  - Features: streaming, tools, agents, structured output
  - Provider support: OpenAI, Anthropic, Google, Groq, OpenRouter, etc.
- Provider-agnostic model interface through aisdk
- Streaming via aisdk's built-in streaming capabilities

## Architecture

### Project Structure

```
crabcode/
├── Cargo.toml              # Dependencies
├── src/
│   ├── main.rs             # Entry point
│   ├── app.rs              # Application state and lifecycle
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── components/
│   │   │   ├── mod.rs
│   │   │   ├── landing.rs       # Logo landing page
│   │   │   ├── chat.rs         # Chat message display
│   │   │   ├── input.rs        # Text input with auto-suggestions
│   │   │   ├── popup.rs        # Auto-suggestion popup
│   │   │   └── status_bar.rs   # Bottom status bar
│   │   └── layout.rs          # Main layout composition
│   ├── agent/
│   │   ├── mod.rs
│   │   ├── types.rs           # Agent definitions
│   │   ├── manager.rs         # Agent switching logic
│   │   └── plan.rs            # PLAN agent
│   │   └── build.rs           # BUILD agent
│   ├── model/
│   │   ├── mod.rs
│   │   ├── types.rs           # Model configuration
│   │   ├── discovery.rs       # Models.dev API integration
│   │   ├── aisdk_adapter.rs   # aisdk provider adapter
│   │   └── config.rs           # aisdk configuration
│   ├── session/
│   │   ├── mod.rs
│   │   ├── types.rs           # Session state
│   │   └── manager.rs         # Session CRUD
│   ├── command/
│   │   ├── mod.rs
│   │   ├── parser.rs          # Command parsing (/)
│   │   ├── registry.rs        # Command registration
│   │   └── handlers.rs        # Command implementations
│   ├── autocomplete/
│   │   ├── mod.rs
│   │   ├── command.rs         # / command suggestions
│   │   └── file.rs            # @ file suggestions
│   ├── streaming/
│   │   ├── mod.rs
│   │   ├── adapter.rs         # aisdk stream adapter
│   │   └── events.rs          # UI event mapping from aisdk
│   └── utils/
│       ├── mod.rs
│       ├── git.rs             # Git branch detection
│       ├── ignore.rs          # .gitignore parsing
│       └── frecency.rs        # File usage scoring
└── assets/
    └── logo.txt            # ASCII logo from crabcode-logo.txt
```

### Core Components

#### 1. Landing Page (`src/ui/components/landing.rs`)
- Display centered ASCII logo from `assets/logo.txt`
- Initial state when no session is active
- Transition to chat view on first user action

#### 2. Chat Interface (`src/ui/components/chat.rs`)
- Scrollable message display area
- Support for markdown rendering (basic: code blocks, bold, lists)
- User messages and AI responses styled differently
- Streaming text updates (append chunks as they arrive)
- Auto-scroll to latest message

#### 3. Input Component (`src/ui/components/input.rs`)
- Based on tui-textarea for multi-line editing
- Event handling:
  - `/` press → Show command suggestions
  - `@` press → Show file suggestions
  - Arrow keys → Navigate suggestions
  - Enter → Submit (or select suggestion if popup open)
  - TAB → Switch agent
  - Ctrl+C → Copy selected text
- History: Up/Down arrows navigate past inputs

#### 4. Auto-Suggestion Popup (`src/ui/components/popup.rs`)
- Dynamic positioning near cursor (inspired by ratatui popup example)
- Two modes: `/` (commands) and `@` (files)
- Fuzzy filtering as user types
- Keyboard navigation (Arrow Up/Down, Enter to select)
- Close on Escape or when suggestion doesn't match

#### 5. Status Bar (`src/ui/components/status_bar.rs`)
- Bottom-left: `crabcode {version} | {cwd} ({branch})`
- Bottom-right: Agent and Model display: `<PLAN> <nano-gpt>`

## Features

### Command System (`/commands`)

Commands are registered in a registry and triggered by typing `/`:

| Command | Description | Parameters |
|---------|-------------|-------------|
| `/sessions` | List all sessions | None |
| `/new` | Create new session | Optional: name |
| `/connect` | Connect/configure model | Optional: provider, model |
| `/models` | List available models | Optional: provider filter |
| `/exit` | Quit crabcode | None |

#### Command Registry Pattern
```rust
pub struct Command {
    pub name: String,
    pub description: String,
    pub handler: fn(&mut App, args: Vec<String>) -> Result<()>,
}

pub struct CommandRegistry {
    commands: HashMap<String, Command>,
}
```

### Agent System

#### Agent Types
Two primary agents for MVP (mode = "primary"):

| Agent | Description | Permissions |
|-------|-------------|-------------|
| PLAN | Read-only analysis and planning | No file writes, bash = ask |
| BUILD | Full access for implementation | All tools enabled |

#### Agent Switching (TAB)
- Circular navigation through agent list
- Filter: Only agents with `mode != "subagent"` and `!hidden`
- When switching, update:
  - Current agent name in status bar
  - Agent-specific model (if configured)
  - Agent color theme

#### Agent Configuration
```rust
pub struct Agent {
    pub name: String,
    pub mode: AgentMode,  // "primary" or "subagent"
    pub description: Option<String>,
    pub native: bool,           // Built-in vs custom
    pub hidden: bool,            // Show in TAB cycle
    pub model: Option<ModelConfig>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
}

pub enum AgentMode {
    Primary,
    Subagent,
}
```

#### Tool Integration (via aisdk)
- Use aisdk's `#[tool]` macro to expose Rust functions
- Example tools for MVP:
  - File read/write operations
  - Git operations (with user confirmation)
  - Shell command execution (ask mode for PLAN, execute for BUILD)
- Tool permissions based on agent type
- Tools registered with aisdk agent system

### Model System

#### Models.dev Integration
- Fetch from `https://models.dev/api.json`
- Parse model list with provider information
- Cache locally (TTL: 24 hours)
- Provide `/models` command to list available models
- Cross-reference with aisdk-supported providers

#### Provider Support (via aisdk)
Priority models for MVP:
1. **OpenAI** (via aisdk)
   - Models: gpt-4, gpt-3.5-turbo, etc.
   - Streaming: Built-in to aisdk
   - Auth: API key via `OPENAI_API_KEY` env var

2. **Anthropic** (via aisdk)
   - Models: claude-3-opus, claude-3-sonnet, etc.
   - Streaming: Built-in to aisdk
   - Auth: API key via `ANTHROPIC_API_KEY` env var

3. **Additional aisdk providers** (as needed)
   - Google (Gemini)
   - Groq
   - OpenRouter
   - xAI (Grok)

#### Model Configuration
```rust
pub struct Model {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub capabilities: Vec<String>,
}

pub struct ModelConfig {
    pub provider_id: String,
    pub model_id: String,
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

// aisdk provider adapter
pub fn create_provider(config: &ModelConfig) -> Result<Box<dyn aisdk::core::LanguageModel>> {
    match config.provider_id.as_str() {
        "openai" => Ok(Box::new(aisdk::providers::openai::OpenAI::new(config.model_id.clone())?)),
        "anthropic" => Ok(Box::new(aisdk::providers::anthropic::Anthropic::new(config.model_id.clone())?)),
        _ => Err(anyhow::anyhow!("Unsupported provider: {}", config.provider_id)),
    }
}
```

### Auto-Suggestions

#### Command Suggestions (`/` trigger)
Trigger: User types `/` at start of input or `/command` pattern

- Parse: `/(\w*)` regex match
- Filter: Registered commands matching query
- Display: Popup with `Command - Description` format
- On select: Insert command name with space

#### File Suggestions (`@` trigger)
Trigger: User types `@` anywhere in input

- Parse: `@(\S*)$` regex match at cursor position
- Filter: Files in current CWD (excluding .gitignore)
- Scoring: Use frecency (frequency + recency)
- Display: Popup with file paths
- On select: Insert file path with syntax highlighting

#### File Discovery
```rust
pub fn discover_files(cwd: &Path) -> Result<Vec<PathBuf>> {
    let gitignore = load_gitignore(cwd)?;
    walkdir(cwd)?
        .filter(|entry| {
            !gitignore.is_excluded(entry.path())
                && entry.file_type().is_file()
        })
        .collect()
}
```

### Streaming Chat

#### Stream Events
Inspired by opencode's stream event types, mapped from aisdk:

| Event | Meaning | UI Action |
|--------|----------|------------|
| `start` | Generation started | Show busy indicator |
| `text-delta` | Text chunk received | Append to current message |
| `tool-call` | AI calling a tool (via aisdk agent) | Display tool invocation |
| `tool-result` | Tool execution result | Show tool output |
| `done` | Stream complete | Mark message as finished |
| `error` | Stream failed | Show error state |

#### Streaming Implementation
```rust
use aisdk::core::LanguageModelRequest;
use aisdk::providers::openai::OpenAI;

pub async fn stream_request(
    model: &ModelConfig,
    prompt: &str,
) -> Result<impl Stream<Item = StreamEvent>> {
    let provider = aisdk_adapter::create_provider(model)?;
    
    let stream = LanguageModelRequest::builder()
        .model(provider)
        .prompt(prompt)
        .build()
        .stream_text()
        .await?;

    Ok(stream.map(|chunk| {
        match chunk {
            Ok(text) => StreamEvent::TextDelta(text),
            Err(e) => StreamEvent::Error(e.to_string()),
        }
    }))
}

pub enum StreamEvent {
    TextDelta(String),
    Done,
    Error(String),
}
```

### Select-to-Copy

- User selects text with mouse
- On release, copy to system clipboard
- Visual feedback: brief flash of selected text
- Use `copypasta` crate for clipboard operations

### Status Bar

#### Left Side
Format: `crabcode v{version} | {cwd} ({branch})`

```rust
pub struct StatusInfo {
    pub version: String,
    pub cwd: String,
    pub branch: Option<String>,
}
```

#### Right Side
Format: `<{agent}> <{model}>`

Color-coded by agent:
- PLAN: Green
- BUILD: Blue
- Custom: Yellow

## Implementation Phases

### Phase 1: Foundation (Week 1)
- [x] Project setup (Cargo.toml, basic structure)
- [x] Main loop with ratatui Terminal
- [x] Landing page with logo display
- [x] Basic text input using tui-textarea
- [x] Command parsing (`/` trigger)
- [x] `/exit` command implementation

### Phase 2: UI Components (Week 2)
- [x] Chat message display component
- [x] Auto-suggestion popup (ratatui popup example)
- [x] Status bar implementation
- [x] Git branch detection
- [x] CWD display
- [x] `/sessions` and `/new` commands

### Phase 3: Model Integration (Week 3)
- [x] Model configuration types
- [x] Provider trait definition
- [x] Models.dev API client
- [x] `/models` command
- [x] `/connect` command for API keys
- [ ] Add aisdk to Cargo.toml with provider features
- [ ] Implement aisdk provider adapter
- [ ] Configure OpenAI provider via aisdk
- [ ] Configure Anthropic provider via aisdk
- [ ] Test streaming with aisdk

### Phase 4: Agent System (Week 4)
- [ ] Agent type definitions
- [ ] Agent manager (TAB switching)
- [ ] PLAN agent configuration
- [ ] BUILD agent configuration
- [ ] Agent display in status bar
- [ ] Model switching with agent

### Phase 5: Auto-Suggestions (Week 5)
- [ ] Command suggestion registry
- [ ] File discovery (.gitignore filtering)
- [ ] Frecency scoring for files
- [ ] `@` trigger and popup
- [ ] `/` trigger and popup
- [ ] Keyboard navigation in popups

### Phase 6: Streaming (Week 6)
- [ ] Implement aisdk stream adapter
- [ ] Map aisdk stream events to UI events
- [ ] Chat message streaming (text-delta via aisdk)
- [ ] Tool call display (via aisdk agent system)
- [ ] Auto-scroll to latest message

### Phase 7: Polish (Week 7-8)
- [ ] Select-to-copy functionality
- [ ] Markdown rendering (basic)
- [ ] Error handling and user feedback
- [ ] Configuration file support
- [ ] Documentation
- [ ] Testing

## Configuration

### Config File Location
`~/.config/crabcode/config.toml`

### Config Structure
```toml
[general]
version = "0.1.0"

[models]
api_keys_dir = "~/.config/crabcode/keys/"
# Supported providers via aisdk: openai, anthropic, google, groq, openrouter, etc.

[agent.plan]
name = "plan"
model_provider = "openai"
model_id = "gpt-4"
temperature = 0.5

[agent.build]
name = "build"
model_provider = "anthropic"
model_id = "claude-3-5-sonnet"
temperature = 0.7

[aisdk]
# aisdk-specific configuration
enable_streaming = true
enable_tools = true
max_retries = 3

[ui]
theme = "default"  # dark, light
max_history = 100
```

## Testing Strategy

### Unit Tests
- Command parsing logic
- File discovery and .gitignore filtering
- Frecency scoring algorithm
- Model configuration parsing
- Stream event parsing

### Integration Tests
- Models.dev API client
- aisdk provider adapter
- aisdk streaming (with mock API keys)
- Agent switching logic
- Session persistence

### E2E Tests
- Full chat flow with streaming
- Command execution
- Agent switching
- File suggestion workflow

## Future Enhancements (Post-MVP)

### Additional Commands
- `/help` - Show command help
- `/clear` - Clear current session
- `/export` - Export session to file
- `/import` - Import session from file

### More Agents
- EXPLORE - Fast codebase search (like opencode's explore)
- REVIEW - Code review agent
- REFACTOR - Refactoring specialist

### Tool Support
- File read/write operations (via aisdk `#[tool]` macro)
- Git operations (commit, branch, diff)
- Shell command execution (with confirmation)
- Code execution (sandboxed)
- Leverage aisdk's tool execution engine

### Advanced Features
- Multi-session support
- Session persistence
- Collaboration mode (share sessions)
- Custom agent definitions
- Plugin system
- Leverage aisdk's advanced features:
  - Multi-agent orchestration
  - Structured output with JSON Schema
  - Prompt templating (via aisdk prompt feature)
  - Embedding support for semantic search

## References

### Inspiration
- [anomalyco/opencode](https://github.com/anomalyco/opencode) - Primary inspiration for UX and architecture
- [lazy-hq/aisdk](https://github.com/lazy-hq/aisdk) - **Primary AI SDK for provider abstraction, streaming, agents, and tools**
  - Documentation: https://aisdk.rs
- [anomalyco/models.dev](https://github.com/anomalyco/models.dev) - Model discovery API
- [ratatui](https://github.com/ratatui-org/ratatui) - TUI framework
- [rhysd/tui-textarea](https://github.com/rhysd/tui-textarea) - Text input component

### Resources
- [ratatui popup example](https://ratatui.rs/examples/apps/popup/) - Popup implementation reference
- [models.dev API](https://models.dev/api.json) - Model catalog
- [aisdk documentation](https://aisdk.rs) - AI SDK usage guide
- [aisdk API reference](https://docs.rs/aisdk/latest) - aisdk Rust API docs

## License

TBD (likely MIT or Apache-2.0)

## Contributing

After MVP completion, contribution guidelines will be established focusing on:
- Code style (rustfmt)
- Testing requirements
- Documentation standards
- Feature proposal process
