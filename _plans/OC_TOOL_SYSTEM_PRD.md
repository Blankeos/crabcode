Product Requirements Document: OpenCode Tool System Clone

Executive Summary

OpenCode provides a sophisticated multi-tool agent system for code exploration and manipulation. This PRD documents the complete architecture needed to build an equivalent system that can replicate all of opencode's capabilities.

1. Architecture Overview

The system consists of three core components:

Tool Framework – A generic tool definition and execution system
Tool Registry – Dynamic registration and management of tools
Agent System – Specialized agents with different capabilities and permissions 2. Core Tool Framework

2.1 Tool Definition Interface

typescript

namespace Tool {
interface Info<Parameters extends z.ZodType, Metadata> {
id: string
init(ctx?: InitContext): Promise<{
description: string
parameters: Parameters
execute(args: z.infer<Parameters>, ctx: Context): Promise<{
title: string
metadata: Metadata
output: string
attachments?: FilePart[]
}>
formatValidationError?(error: z.ZodError): string
}>
}

type Context<M> = {
sessionID: string
messageID: string
agent: string
abort: AbortSignal
callID?: string
extra?: Record<string, any>
metadata(input: { title?: string; metadata?: M }): void
ask(input: PermissionRequest): Promise<void>
}

function define<Parameters extends z.ZodType, Result extends Metadata>(
id: string,
init: Info<Parameters, Result>['init']
): Info<Parameters, Result>
}

Key Features:

Zod-based parameter validation
Async initialization for lazy loading
Metadata streaming during execution
Graceful abort signal handling
Permission-based access control via ctx.ask()
Automatic output truncation
2.2 Tool Definition Helper

Tools are defined using Tool.define() which provides:

Automatic parameter validation with helpful error messages
Output truncation management (respects per-agent limits)
Metadata accumulation during execution
Custom error formatting 3. Core Tools (23 Total)

3.1 File System Tools

Tool Purpose Key Parameters
read Read file contents with pagination filePath, offset, limit
write Create/overwrite files with permission checks filePath, content
edit Replace text in files with smart diffing filePath, oldString, newString, replaceAll
glob Find files by glob pattern pattern, path
list List directory contents in tree format path, ignore
bash Execute shell commands with security scanning command, timeout, workdir, description
Advanced Features:

Image/PDF support in read tool (base64 encoding)
Binary file detection
Diff-based edit strategies (simple, line-trimmed, block anchor fallback)
Levenshtein distance for fuzzy matching in edits
Smart Bash command parsing using tree-sitter for permission requests
File lock management to prevent race conditions
3.2 Search & Navigation Tools

Tool Purpose Key Parameters
grep Regex search across files pattern, path, include
codesearch AI-powered code search (via Exa MCP) query, tokensNum
websearch Web search with live crawl options query, numResults, livecrawl, type
Advanced Features:

Results limited to 100 matches max
Results sorted by modification time
SSE response parsing for web/code search
Timeout handling (25-30s)
Live crawl support for fresh content
3.3 External Data Tools

Tool Purpose Key Parameters
webfetch Fetch and convert web content url, format (text/markdown/html), timeout
Advanced Features:

HTML to Markdown conversion (turndown service)
Text extraction from HTML
Content-Type aware formatting
5MB response size limit
120s max timeout
Accept header optimization per format
3.4 Task Coordination Tools

Tool Purpose Key Parameters
task Spawn subagents for parallel work description, prompt, subagent_type, session_id
Advanced Features:

Dynamic agent list filtering based on caller permissions
Session creation/reuse
Real-time tool execution tracking
Summary aggregation from subagent work
3.5 Metadata & Admin Tools

Tool Purpose Key Parameters
question CLI-only: Interactive user questions question, options
todo.read List TODO items from workspace path, pattern
todo.write Create/update TODO items path, task, status
skill Register/invoke custom skills name, args
batch Execute multiple tools in batch (experimental) commands
lsp Language server protocol integration (experimental) -
invalid Error handler for unknown tool calls - 4. Tool Registry System

typescript

namespace ToolRegistry {
async function all(): Promise<Tool.Info[]>
async function ids(): Promise<string[]>
async function tools(providerID: string, agent?: Agent.Info): Promise<ToolDefinition[]>
async function register(tool: Tool.Info): void
}

Features:

Plugin system: Auto-discovers tools in {configDir}/tool/\*.{js,ts}
Dynamic filtering based on provider (websearch/codesearch only for "opencode" provider or with flag)
Agent-specific tool initialization
Runtime tool registration for custom extensions
Plugin Tool Format:

typescript

// tools/my_tool.ts
export default {
description: "...",
args: { param1: z.string(), ... },
execute(args, ctx): Promise<string>
}

5. Agent System

5.1 Agent Definition

typescript

namespace Agent {
interface Info {
name: string
description?: string
mode: "subagent" | "primary" | "all"
native?: boolean
hidden?: boolean
topP?: number
temperature?: number
color?: string
permission: PermissionRuleset
model?: { modelID: string; providerID: string }
prompt?: string
options?: Record<string, any>
steps?: number
}
}

5.2 Built-in Agents (8 Total)

Agent Mode Native Permission Purpose
build primary ✓ Everything + questions Build/compile automation
plan primary ✓ Read + plan editing Project planning
general subagent ✓ Everything except TODOs Parallel multi-step work
explore subagent ✓ Grep/glob/read/bash/search Fast codebase exploration
compaction primary ✓ Nothing (hidden) Message truncation
title primary ✓ Nothing (hidden) Session title generation
summary primary ✓ Nothing (hidden) Session summarization
\*custom all ✗ Per-config User-defined agents
5.3 Permission System

Agents operate under hierarchical permission rules:

typescript

PermissionRuleset: {
"_": "allow" | "deny" | "ask"
[permission]: "allow" | "deny" | "ask"
[permission]: {
"_": "allow" | "deny" | "ask"
[pattern]: "allow" | "deny" | "ask"
}
}

Default Permissions:

javascript

{
"_": "allow",
doom_loop: "ask",
external_directory: { "_": "ask", [Truncate.DIR]: "allow" },
question: "deny",
read: {
"_": "allow",
"_.env": "deny",
"_.env._": "deny",
"\*.env.example": "allow"
}
}

6. Non-Functional Requirements

6.1 Output Truncation

Per-tool limits: 2000 lines, 50KB (read), varies per tool
Global limits: Configurable per agent
Truncation strategy: Content + "..." + path to overflow file
Output modes:
Preserve full output in metadata
Streaming metadata updates during execution
6.2 Concurrency & Cancellation

AbortSignal propagation from session
Graceful timeout handling in async tools
Process tree killing (bash tool)
Token-aware request cancellation
6.3 Error Handling

Custom validation error formatters per tool
Helpful error messages (e.g., "Did you mean..." for missing files)
Schema validation before execution
Detailed error context in responses
6.4 Security

Permission evaluation before each tool call
Path validation for external directories
Binary file detection to prevent read errors
.env file blocking by default
Command parsing (tree-sitter bash) for permission verification 7. Implementation Priorities

Phase 1: Core (MVP)

Tool framework + definition system
6 core file tools (read, write, edit, glob, list, bash)
Tool registry with plugin support
Agent system with permissions
3 primary agents (build, plan, general)
Phase 2: Search & External

Grep tool
Webfetch tool
Websearch & codesearch tools (MCP integration)
Explore agent
Phase 3: Advanced

Task tool + subagent spawning
LSP integration
Todo tools
Skill system
Batch tool
Phase 4: Polish

Session title/summary generation
Message compaction
Custom agent support
Plugin ecosystem 8. Key Integration Points

Provider System: Tools need to know which LLM provider they're running under (affects feature availability)
Session Management: Tool execution tied to session/message IDs for context
File Change Notification: Bus/event system to broadcast edits
LSP Server: Optional language server for diagnostics
Permission System: Evaluator that makes allow/deny/ask decisions
Plugin Loader: Dynamic module imports from user config directories 9. Data Structures

Tool Execution Context:

typescript

{
sessionID, messageID, agent, abort, callID, extra, metadata(), ask()
}

Tool Response:

typescript

{
title: string
output: string
metadata: Record<string, any>
attachments?: { id, sessionID, messageID, type, mime, url }[]
}

Permission Request:

typescript

{
permission: string
patterns: string[]
always?: string[]
metadata: Record<string, any>
}
