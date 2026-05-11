---
description: Audit crabcode against opencode for harness feature parity
agent: build
---

Audit the crabcode codebase (this Rust project) against the opencode AI coding agent for 1:1 feature parity. Focus ONLY on core harness functionality (agent loop, system prompt, subagents, tool calling, skill loading, agent config, commands). Do NOT audit UX, theming, keybinds, or non-harness features.

## What to Audit

For each area below, read the relevant crabcode source files, compare against how opencode does it (I will provide opencode's behavior inline), and produce a table row: Feature | Crabcode Status | Gap

### 1. Agent Loop
- Multi-step agentic iteration via LLM streaming with tool calling
- Cancellation token support for user interruption
- Step limit enforcement with text-only summary fallback
- Chunk-based streaming: text, reasoning, tool_calls, tool_results, errors, metrics, cancelled
- Plan/Build mode toggle (plan = read-only tools)
- Permission preflight during tool execution (and mid-stream permission dialogs)
- Configurable max steps per agent (with "max steps reached" prompt injection)

### 2. System Prompt
- Provider-specific header and behavior instructions (Beast for OpenAI, Anthropic-specific, Gemini-specific, Codex-specific)
- Environment context block (workdir, git status, platform, date)
- Tool schemas block (all registered tools as JSON)
- Custom instructions from AGENTS.md/CLAUDE.md (walk-up directory discovery + global fallback)
- Available skills listing as `<available_skills>` XML block
- Available subagents listing: opencode lists subagent names and descriptions in the system prompt so the primary agent knows when to use the Task tool. Crabcode currently does NOT list subagents (there are no real subagents yet).

### 3. Subagent System
OpenCode has these subagents:
- **explore**: Fast, read-only (glob/grep/read/list tools only). For codebase searching.
- **general**: Full tool access (minus todowrite). For complex multi-step tasks.
- **scout**: Read-only, can clone repos. For external docs/dependency research.
- **vlm-agent**: For image analysis.
Additionally: **compaction**, **title**, **summary** (hidden/system agents that run automatically).

Check if crabcode has:
- Task tool (the tool primary agents use to spawn subagents)
- The explore/general/scout subagent implementations
- Child sessions for subagent work (session tree: parent/child navigation)
- Subagent descriptions in the system prompt so the primary agent can select subagents
- @mention subagent invocation from user input
- Agent mode: primary vs subagent vs all
- Hidden agents (hidden from @autocomplete but invokable via Task tool)
- Task permissions (which agents can invoke which subagents)

### 4. Tool Calling
OpenCode's built-in tools:
- **bash** - shell command execution
- **edit** - exact string replacement in files
- **write** - create/overwrite files
- **read** - read files with offset/limit pagination, also directories
- **grep** - regex search with include filters
- **glob** - file pattern matching
- **list** - tree-style directory listing (this is NOT the same as read for directories; it's a deliberate directory-tree listing tool)
- **skill** - loads SKILL.md by name
- **task** - spawn subagents
- **todowrite** - manage structured task lists
- **webfetch** - fetch web content (markdown conversion)
- **websearch** - search the web (Exa AI)
- **question** - ask user questions during execution
- **extract-images** - save session images to disk
- **apply_patch** - apply diffs
- **lsp** - LSP code intelligence (experimental)

Check crabcode's registered tools in `src/tools/init.rs` and list which are present, which are missing.

### 5. Skill Loading
OpenCode's skill system:
- Discovery locations: `.opencode/skills/<name>/SKILL.md`, `~/.config/opencode/skills/<name>/SKILL.md`, `.claude/skills/`, `.agents/skills/`, `~/.claude/skills/`, `~/.agents/skills/`
- Walk-up from project root to git worktree for project skills
- YAML frontmatter with required `name` and `description`
- Pattern-based skill permissions (e.g., `"internal-*": "deny"`)
- Skill tool lists available skills in description

Check crabcode's skill loading in `src/skill/mod.rs` against this.

### 6. Agent Configuration
OpenCode supports:
- Agent config via `opencode.json` (JSON) and `~/.config/opencode/agents/<name>.md` (markdown frontmatter)
- Per-agent: description, temperature, model, max_steps, mode (primary/subagent/all), hidden, color, top_p, permissions, task permissions
- Agent creation wizard (`opencode agent create`)

Check what crabcode has in `src/agent/` and config files.

### 7. Custom Commands
OpenCode supports:
- User-defined commands via `.opencode/commands/<name>.md` files
- Frontmatter: description, agent, model, subtask
- Template variables: $ARGUMENTS, $1, $2, etc.
- Shell output injection: `!`command``
- File references: `@path/to/file`

Check crabcode's command system in `src/command/`.

### 8. Permission System
OpenCode's permission system:
- Per-tool: allow, deny, ask
- Wildcard patterns (e.g., `"mymcp_*": "deny"`)
- Pattern-specific bash permissions (e.g., `"git push": "ask"`, `"git *": "allow"`)
- Per-agent override of global permissions
- External directory gating
- Doom loop recovery prompts

Check crabcode's permission system in `src/tools/permission.rs`.

## Output Format
Produce a markdown table with these columns:
| # | Feature | OpenCode | Crabcode | Gap |
|---|---------|----------|----------|-----|

Then a separate section with PRIORITY-ranked actionable gaps (CRITICAL/HIGH/MEDIUM/LOW) with specific file locations and implementation notes.

Write it in _docs/__PARITY.md