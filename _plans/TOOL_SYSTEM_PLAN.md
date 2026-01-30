# Crabcode Tool System Implementation Plan

## Overview

This document outlines the implementation plan for adding OpenCode-style tool system to crabcode. We'll implement the 6 core file system tools first, building on crabcode's existing Rust architecture.

## Current State Analysis

Crabcode already has:
- Basic agent module structure (`src/agent/`)
- File autocomplete functionality (`src/autocomplete/file.rs`)
- Ignore patterns support (stub in `src/utils/ignore.rs`)
- Async runtime (Tokio)
- Serialization (serde)
- Error handling (anyhow)

## Phase 1: Core Infrastructure (Week 1)

### 1.1 Tool Framework Foundation

**New Files:**
- `src/tools/mod.rs` - Tool module root
- `src/tools/types.rs` - Core type definitions
- `src/tools/context.rs` - Tool execution context
- `src/tools/registry.rs` - Tool registration and discovery

**Key Types to Implement:**

```rust
// src/tools/types.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type ToolId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSchema {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub param_type: ParameterType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    String,
    Integer,
    Boolean,
    Array(Box<ParameterType>),
    Object(HashMap<String, ParameterType>),
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub id: ToolId,
    pub description: String,
    pub parameters: Vec<ParameterSchema>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub title: String,
    pub output: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Execution error: {0}")]
    Execution(String),
    #[error("Permission denied: {0}")]
    Permission(String),
    #[error("Not found: {0}")]
    NotFound(String),
}
```

### 1.2 Tool Context

```rust
// src/tools/context.rs
pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub agent: String,
    pub abort: tokio::sync::watch::Receiver<bool>,
    pub call_id: Option<String>,
    pub extra: Option<serde_json::Value>,
}
```

### 1.3 Tool Trait

```rust
// src/tools/mod.rs
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn definition(&self) -> Tool;
    fn validate(&self, params: &Value) -> Result<(), ToolError>;
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError>;
}
```

### 1.4 Tool Registry

```rust
// src/tools/registry.rs
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<ToolId, Arc<dyn ToolHandler>>>>,
}

impl ToolRegistry {
    pub fn new() -> Self;
    pub async fn register(&self, tool: Arc<dyn ToolHandler>);
    pub async fn get(&self, id: &str) -> Option<Arc<dyn ToolHandler>>;
    pub async fn list(&self) -> Vec<Tool>;
}
```

## Phase 2: Core File Tools (Week 2-3)

### 2.1 Tool: `glob` (Simplest)

**Purpose:** Find files by glob pattern

**Parameters:**
- `pattern` (string, required): Glob pattern
- `path` (string, optional): Base directory (default: cwd)

**Implementation:** Use `glob` crate, sort by mtime, limit to 100 results

**Dependencies:** `glob = "0.3"`

### 2.2 Tool: `list` (Directory Tree)

**Purpose:** List directory contents in tree format

**Parameters:**
- `path` (string, required): Directory path
- `ignore` (array of strings, optional): Patterns to ignore

**Implementation:** Recursive directory traversal with tree-style output (using ├── and └── connectors)

### 2.3 Tool: `read` (File Reading)

**Purpose:** Read file contents with pagination and binary detection

**Parameters:**
- `file_path` (string, required): Path to file
- `offset` (integer, optional): Line offset (0-based, default: 0)
- `limit` (integer, optional): Max lines (default: 2000)

**Implementation:**
- Check file size (<50MB)
- Binary detection (check for null bytes in first 8KB)
- Line-numbered output with pagination
- Truncation indicator if limit exceeded

### 2.4 Tool: `write` (File Creation)

**Purpose:** Create or overwrite files

**Parameters:**
- `file_path` (string, required): Path to file
- `content` (string, required): Content to write

**Implementation:**
- Create parent directories if needed
- Atomic write (write to temp file, then rename)
- Permission checks (block .env files by default)

### 2.5 Tool: `bash` (Shell Execution)

**Purpose:** Execute shell commands with security

**Parameters:**
- `command` (string, required): Command to execute
- `timeout` (integer, optional): Timeout in seconds (default: 120)
- `workdir` (string, optional): Working directory
- `description` (string, optional): Human-readable description

**Implementation:**
- Use `tokio::process::Command`
- Stream stdout/stderr
- Timeout handling with process kill
- Basic command validation (block dangerous commands)

### 2.6 Tool: `edit` (Text Replacement)

**Purpose:** Replace text in files with smart diffing

**Parameters:**
- `file_path` (string, required): Path to file
- `old_string` (string, required): Text to replace
- `new_string` (string, required): Replacement text
- `replace_all` (boolean, optional): Replace all occurrences (default: false)

**Implementation:**
- Exact match first
- Fuzzy matching with Levenshtein distance as fallback
- Line-trimmed matching
- Block anchor fallback for multi-line edits
- Generate diff output showing changes

**Dependencies:** `strsim = "0.11"` (for Levenshtein distance)

## Phase 3: Integration (Week 4)

### 3.1 Agent Integration

Update `src/agent/manager.rs` to:
- Initialize tool registry on startup
- Pass tool definitions to LLM context
- Parse tool calls from LLM responses
- Execute tools and return results

### 3.2 LLM Context Format

Tools should be exposed to LLM in OpenAI-compatible format:

```json
{
  "type": "function",
  "function": {
    "name": "read",
    "description": "Read file contents...",
    "parameters": {
      "type": "object",
      "properties": {
        "file_path": {"type": "string"},
        "offset": {"type": "integer"},
        "limit": {"type": "integer"}
      },
      "required": ["file_path"]
    }
  }
}
```

### 3.3 Response Handling

Parse tool calls from LLM responses:
- OpenAI: `tool_calls` in assistant message
- Generic: Parse function call syntax from text

Execute tools and format results back to LLM.

## Phase 4: Testing & Polish (Week 5)

### 4.1 Unit Tests

Each tool needs tests for:
- Happy path
- Error cases (file not found, permission denied)
- Edge cases (empty files, binary files, large files)
- Parameter validation

### 4.2 Integration Tests

- End-to-end agent workflows
- Tool chaining (read -> edit -> read)
- Concurrent tool execution
- Error recovery

### 4.3 Documentation

- Tool usage examples
- Parameter reference
- Error message guide

## File Structure

```
src/
  tools/
    mod.rs          # Tool trait and exports
    types.rs        # Core types (Tool, ToolResult, ToolError)
    context.rs      # ToolContext
    registry.rs     # ToolRegistry
    fs/             # File system tools
      mod.rs
      glob.rs
      list.rs
      read.rs
      write.rs
      edit.rs
    bash.rs         # Shell execution
```

## Dependencies to Add

```toml
[dependencies]
glob = "0.3"
strsim = "0.11"  # For fuzzy string matching in edit tool
tempfile = "3.0"  # For atomic file writes
```

## Implementation Order

1. **Week 1:** Core infrastructure (types, context, registry, trait)
2. **Week 2:** Simple tools (glob, list, read)
3. **Week 3:** Complex tools (write, bash, edit)
4. **Week 4:** Agent integration and LLM context
5. **Week 5:** Testing and documentation

## Success Criteria

- All 6 core tools implemented and tested
- Tools can be called from agent context
- LLM can discover and invoke tools
- Proper error handling and user feedback
- Binary file detection works
- Large file handling (truncation)
- Concurrent tool execution support

## Future Enhancements (Post-MVP)

- Search tools (grep, codesearch, websearch)
- External data tools (webfetch)
- Task coordination (subagent spawning)
- TODO management
- LSP integration
- Custom skill system
- Plugin architecture for user-defined tools
