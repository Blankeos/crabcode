# Crabcode vs OpenCode — Core Harness Feature Parity Audit

> Generated: 2026-05-11 | Updated: 2026-05-19 | Scope: agent loop, system prompt, subagents, tool calling, skill loading, agent config, commands, permissions.

## Feature Table

| # | Feature | OpenCode | Crabcode | Gap |
|---|---------|----------|----------|-----|
| **1.1** | Multi-step agentic iteration (LLM streaming + tool calling) | `stream_text()` with `step_count_is(N)` hook, tool execution loop | `stream_llm_with_cancellation()` at `src/llm/client.rs:82`, `stop_when(step_count_is(max_steps))` at `:377` | **OK** |
| **1.2** | Cancellation token for user interruption | `CancellationToken`, checked in relay loop | `CancellationToken` at `src/llm/client.rs:83`, emits `ChunkMessage::Cancelled` at `:474` | **OK** |
| **1.3** | Step limit enforcement with text-only summary fallback | `stop_when(step_count_is(N))` + follow-up request with `MAX_STEPS_REACHED` prompt, tools stripped | `MAX_STEPS_REACHED_PROMPT` at `src/llm/client.rs:18`, `reached_step_limit()` at `:514`, follow-up stream at `:161-173` with empty tools vec | **OK** |
| **1.4** | Chunk relay: text, reasoning, tool_calls, tool_results, errors, metrics, cancelled | `ChunkType` dispatched per-kind to UI | `ChunkMessage` at `src/llm/mod.rs:9` — Text, Reasoning, ToolCalls, ToolResult, PermissionRequest, QuestionRequest, End, Failed, Cancelled, Metrics, Warning | **OK** |
| **1.5** | Plan/Build mode toggle | User-toggleable mode; plan = read-only tools | `AgentToolPolicies` at `src/tools/permission.rs:71` — plan blocks write/edit/bash, build allows all. No user-facing toggle; mode set at stream start | **Partial**: Mode exists but not user-toggleable mid-conversation |
| **1.6** | Permission preflight during tool execution | `preflight()` checks before each tool call, mid-stream permission dialogs | `permissions.preflight()` in `aisdk_bridge.rs:90-98`, sends `PermissionRequest` chunk, awaits UI response via oneshot | **OK** |
| **1.7** | Configurable max steps per agent | Per-agent `max_steps` in config; "max steps reached" prompt injected | `agent_max_steps: Option<usize>` at `src/llm/client.rs:87` | **OK** |
| **1.8** | Model-visible replayable history across turns | `MessageV2` persists text, reasoning, tool calls/results, attachments, and reconstructs provider messages | `convert_messages()` now replays `MessageRole::Tool` as model-visible observations via `tool_message_observation()` | **Partial**: tool results are visible across turns, but not canonical typed tool-call/result parts with attachments |
| **2.1** | Provider-specific header (Beast for OpenAI) | Detailed "beast" prompt for OpenAI, concise for Anthropic | `get_beast_prompt()` at `src/prompt/mod.rs:100`, `get_anthropic_prompt()` at `:135`, `get_codex_prompt()` at `:187` | **OK** |
| **2.2** | Provider-specific behavior instructions | Anthropic-specific, Gemini-specific, Codex-specific | `get_gemini_prompt()` at `src/prompt/mod.rs:160`, `get_codex_prompt()` at `:187` | **OK** |
| **2.3** | Environment context block (workdir, git, platform, date) | `<env>` XML block | `get_environment_context()` at `src/prompt/mod.rs:224` | **OK** |
| **2.4** | Tool schemas block (all registered tools as JSON) | All tools rendered as JSON schemas | `get_tools_context()` at `src/prompt/mod.rs:239` — `registry.list_schemas()` serialized as pretty JSON | **OK** |
| **2.5** | Custom instructions from AGENTS.md/CLAUDE.md (walk-up + global) | Walk-up directory discovery + global fallback at `~/.config/opencode/AGENTS.md` and `~/.claude/CLAUDE.md` | `src/prompt/rules.rs` — `resolve_local_rules()` walks up from workdir for AGENTS.md then CLAUDE.md; `resolve_global_rules()` checks `~/.config/crabcode/AGENTS.md` and `~/.claude/CLAUDE.md` | **OK** |
| **2.6** | Available skills as `<available_skills>` XML | Lists skill name, description, location | `src/prompt/mod.rs:267-295` — iterates `SkillStore::all()`, emits `<available_skills>` XML | **OK** |
| **2.7** | Available subagents listing in system prompt | Lists subagent names and descriptions so primary agent knows when to use Task tool | `src/prompt/mod.rs:298-320` — iterates `SubAgentDef::all()`, emits `<available_subagents>` XML | **OK** |
| **3.1** | Task tool (primary agent spawns subagents) | `task` tool with subagent_type, description, prompt params | `src/tools/task.rs` — full TaskTool with explore/general enum validation | **OK** |
| **3.2** | Explore subagent | Read-only: glob, grep, read, list. Fast codebase exploration | `src/agent/subagent.rs:4` — ExploreAgent with EXPLORE_SYSTEM_PROMPT, scoped to glob/grep/read/list | **OK** |
| **3.3** | General subagent | Full tool access (minus todowrite). Complex multi-step tasks | `src/agent/subagent.rs:23` — GeneralAgent with GENERAL_SYSTEM_PROMPT, scoped to bash/edit/write/read/grep/glob/list/skill/webfetch | **OK** |
| **3.4** | Scout subagent | Read-only, can clone repos for external docs/deps research | **Not implemented** | **GAP** |
| **3.5** | VLM-agent subagent | For image analysis (delegates to vision models) | **Not implemented** | **GAP** |
| **3.6** | Compaction/Title/Summary hidden agents | System agents that run automatically for session compaction, title generation, summarization | **Not implemented** | **GAP** |
| **3.7** | Subagent multi-step iteration (tool-calling loop within subagent) | Subagents run full agentic loops (stream + tool execution + recursion) | `run_subagent()` uses `stream_with_tools()` with a scoped registry and relays text/reasoning/tool rows into the child stream | **OK** |
| **3.8** | Child sessions / session tree (parent/child navigation) | Subagents create child sessions, navigable in UI | Task calls create persisted child sessions with `parent_id`, stream transcript rows into them, and expose OpenCode-style `ctrl+x` down / left-right / up navigation | **Partial**: no background resume/task_status integration yet |
| **3.9** | Agent mode system (primary vs subagent vs all) | Each agent has a `mode` that controls visibility and invocation | No mode field. Plan/build handled separately via policies | **GAP** |
| **3.10** | Hidden agents (hidden from autocomplete, invokable via Task) | Agents can be marked `hidden: true` | No hidden agent concept | **GAP** |
| **3.11** | Task permissions (which agents can invoke which subagents) | Per-agent `task_permissions` control | No task permission system. Primary agent can always invoke explore/general | **GAP** |
| **3.12** | @mention subagent invocation from user input | `@explore` / `@general` in user input routes to subagent | Not implemented | **GAP** |
| **4.1** | bash | ✓ | `src/tools/bash.rs` | **OK** |
| **4.2** | edit | ✓ | `src/tools/edit.rs` (exact string replacement, fuzzy fallback) | **OK** |
| **4.3** | write | ✓ | `src/tools/fs/write.rs` (atomic write via temp+rename) | **OK** |
| **4.4** | read | ✓ | `src/tools/fs/read.rs` (offset/limit pagination, also reads dirs) | **OK** |
| **4.5** | grep | ✓ | `src/tools/fs/grep.rs` (regex + include filters) | **OK** |
| **4.6** | glob | ✓ | `src/tools/fs/glob.rs` (pattern matching) | **OK** |
| **4.7** | list | ✓ | `src/tools/fs/list.rs` (tree-style directory listing) | **OK** |
| **4.8** | skill | ✓ | `src/tools/skill.rs` (loads SKILL.md by name, injects content) | **OK** |
| **4.9** | task | ✓ | `src/tools/task.rs` (spawns explore/general subagents) | **OK** |
| **4.10** | todowrite | ✓ | `src/tools/todowrite.rs` (JSON-validated structured task list) | **OK** |
| **4.11** | webfetch | ✓ | `src/tools/webfetch.rs` (fetch + handcrafted HTML-to-markdown) | **OK** |
| **4.12** | question | ✓ | `src/tools/question.rs` (oneshot-based UI question prompts) | **OK** |
| **4.13** | websearch | Exa/Parallel web search | **Not implemented** | **GAP** |
| **4.14** | Tool/media attachments | Tool outputs can carry attachments; images/resources are normalized and replayed to the model | **Not implemented** | **GAP** |
| **4.15** | apply_patch | Apply diffs/patch files | **Not implemented** | **GAP** |
| **4.16** | lsp | LSP code intelligence (experimental) | **Not implemented** | **GAP** |
| **4.17** | task_status | Poll/wait for background subagent tasks | **Not implemented** | **GAP** |
| **4.18** | repo_clone | Clone external repositories into managed cache for scout/reference workflows | **Not implemented** | **GAP** |
| **4.19** | repo_overview | Summarize cached/local repository structure and dependency files | **Not implemented** | **GAP** |
| **4.20** | plan_exit | Model-callable plan approval / build-agent handoff | **Not implemented** | **GAP** |
| **4.21** | invalid | Fallback tool for unknown/invalid tool calls | **Not implemented** | **GAP** |
| **5.1** | Discovery: `.opencode/skills/<name>/SKILL.md` | OpenCode native layout | Scanned via `{skill,skills}/**/SKILL.md` in `.opencode/`, `.crabcode/`, config dirs at `src/skill/mod.rs:67-77` | **OK** |
| **5.2** | Discovery: `~/.config/opencode/skills/<name>/SKILL.md` | Global config skills | `global_opencode` at `src/skill/mod.rs:39` | **OK** |
| **5.3** | Discovery: `.claude/skills/` (project + home) | Claude Code compat | Walk-up `.claude/skills/**/SKILL.md` + `~/.claude/skills/**/SKILL.md` at `src/skill/mod.rs:46-64` | **OK** |
| **5.4** | Discovery: `.agents/skills/` (project + home) | OpenCode compat | Walk-up `.agents/skills/**/SKILL.md` + `~/.agents/skills/**/SKILL.md` at `src/skill/mod.rs:46-64` | **OK** |
| **5.5** | Walk-up bounded to git worktree | Walks up only to git root | Walks up to filesystem root (no git boundary) at `src/skill/mod.rs:50-64` | **Partial**: No git worktree boundary for walk-up |
| **5.6** | YAML frontmatter with `name` and `description` | Required in SKILL.md | Parsed at `src/skill/mod.rs:184-233`, with fallback YAML sanitization for Claude Code compat | **OK** |
| **5.7** | Pattern-based skill permissions | `"internal-*": "deny"` style glob patterns | **Not implemented** | **GAP** |
| **5.8** | Skill tool lists available skills in description | Skill names embedded in tool definition description | `build_description()` at `src/tools/skill.rs:15-48` appends `<available_skills>` XML to tool description | **OK** |
| **6.1** | Agent config via `opencode.json` | `agent` field in JSON config | Crabcode reads opencode.json, but only applies limited `agent.*.tools` and `agent.*.steps` fields in `src/config/configuration.rs` | **Partial** |
| **6.2** | Agent config via `~/.config/opencode/agents/<name>.md` | Markdown frontmatter with agent definitions | **Not implemented** | **GAP** |
| **6.3** | Per-agent: description, model, temperature, max_steps | Full per-agent override of all params | Only has global `LlmSessionConfig` at `src/agent/config.rs:4` (provider, model, api_key). No per-agent overrides | **GAP** |
| **6.4** | Per-agent: mode (primary/subagent/all) | Controls where agent is visible/usable | Not implemented (only plan/build context) | **GAP** |
| **6.5** | Per-agent: hidden, color, top_p, permissions, task_permissions | Agent metadata fields | Not implemented | **GAP** |
| **6.6** | Agent creation wizard (`opencode agent create`) | Interactive agent creation | Not implemented | **GAP** |
| **6.7** | Config `instructions` array | Additional instruction files/patterns beyond AGENTS/CLAUDE discovery | Key is allowed but marked unimplemented by `collect_unimplemented_keys()` | **GAP** |
| **6.8** | Config `reference` aliases | Named local/git references that can be mentioned as `@alias` or `@alias/path` | Not implemented; key is not accepted by `opencode_allowed_keys()` | **GAP** |
| **6.9** | Config `small_model`, `username` | Small model for title/summary agents and display username override | Not implemented | **GAP** |
| **6.10** | Provider config merge and enable/disable filters | Custom provider/model overrides plus `enabled_providers` / `disabled_providers` | Only `provider.*.options.timeout` is parsed; provider/model merge and filters are not applied | **GAP** |
| **6.11** | Formatter, LSP, attachment, tool_output config | Runtime config for formatters, LSP servers, image limits, and truncation thresholds | Keys are mostly absent or marked unimplemented; no corresponding runtime services | **GAP** |
| **7.1** | User-defined commands via `.opencode/commands/<name>.md` | Markdown files define custom slash commands | Not implemented. Only Rust function handlers for built-in commands | **MAJOR GAP** |
| **7.2** | Command frontmatter: description, agent, model, subtask | YAML frontmatter in custom command files | Not implemented | **GAP** |
| **7.3** | Template variables ($ARGUMENTS, $INPUT, $CWD, etc.) | Template substitution in custom commands | Not implemented | **GAP** |
| **7.4** | Shell output injection (`$(command)`) | Inline shell execution in commands | Not implemented | **GAP** |
| **7.5** | File references (`@path/to/file`) | File content insertion in command text | Not implemented | **GAP** |
| **8.1** | Per-tool: allow, deny, ask | Global permission rules per tool | `AgentToolPolicies` at `src/tools/permission.rs:71` — per-mode tool allowlists only (not global per-tool deny/ask rules) | **Partial** |
| **8.2** | Wildcard pattern permissions | `"mymcp_*": "deny"` | Not implemented. Only exact tool name matching | **GAP** |
| **8.3** | Pattern-specific bash permissions | `"git push": "ask"`, `"git *": "allow"` | Not implemented. Bash only gets a generic "bash requires permission" check | **GAP** |
| **8.4** | Per-agent override of global permissions | Agent-level permission config overrides global | Not implemented. Only mode-based (plan/build) | **GAP** |
| **8.5** | External directory gating | Blocks/prompts for paths outside workdir | `is_outside_workdir()` at `src/tools/permission.rs:377` | **OK** |
| **8.6** | Doom loop recovery prompts | Persistent tool failures trigger recovery | Not implemented | **GAP** |
| **9.1** | MCP runtime | Stdio/SSE/Streamable HTTP clients, OAuth, tools, prompts, resources | Config key is accepted, but there is no MCP runtime or tool/resource integration | **MAJOR GAP** |
| **9.2** | Plugin runtime and hooks | Built-in/external plugins plus hooks like `tool.execute.before`, `tool.execute.after`, system transforms | OpenCode `plugin` key is ignored by Crabcode; no plugin loader or hook pipeline | **MAJOR GAP** |
| **9.3** | Local custom JS/TS tools | Loads `{tool,tools}/*.{js,ts}` from config directories and exposes exports as tools | Not implemented | **GAP** |
| **9.4** | Plugin auth/provider integrations | Internal plugins add provider/auth behavior for Codex, Copilot, GitLab, Poe, Cloudflare, Azure, DigitalOcean | Not implemented | **GAP** |
| **10.1** | Snapshots and patch parts | Tracks filesystem snapshots before/after steps and persists patch parts on messages | Not implemented | **GAP** |
| **10.2** | File-state revert/unrevert | Reverts file changes and can undo/redo snapshot state for a message range | Crabcode message action "Undo" only truncates messages; it does not revert file changes | **GAP** |
| **10.3** | Session sharing | `share: manual/auto/disabled` and shared session URLs | Not implemented; OpenCode `share` key is ignored | **GAP** |
| **10.4** | Background/resumable subagents | `task(background=true)`, `task_id` resume, `task_status`, and parent continuation | Not implemented | **GAP** |
| **10.5** | Durable tool-output truncation | Large output saved to data dir with preview and `tool_output` limits | Crabcode truncates some tool output inline; no durable output file/index | **GAP** |
| **10.6** | User and tool attachments | File/image/PDF prompt parts, image normalization, media extraction from tool results | Not implemented | **GAP** |
| **10.7** | Post-edit formatting and diagnostics | Edit/write/patch run configured formatters and surface LSP diagnostics | Not implemented | **GAP** |

## Priority-Ranked Actionable Gaps

### CRITICAL

| # | Gap | Location | Notes |
|---|-----|----------|-------|
| **C1** | **No custom user-defined commands** | `src/command/` | OpenCode's `.opencode/commands/<name>.md` system is entirely absent. Crabcode only has hardcoded Rust function handlers. Need: (a) `.opencode/commands/` + `~/.config/opencode/commands/` directory discovery, (b) Markdown file parser with YAML frontmatter, (c) template engine for `$ARGUMENTS`, `$INPUT`, `$CWD`, (d) shell injection `$(...)`, (e) `@file` references. Entirely new module needed. |

Recently addressed from the previous critical list:
- Subagents now run through the multi-step `stream_with_tools()` loop and stream into child sessions.
- Tool-result messages are now replayed into later model requests as observations. Remaining work is canonical MessageV2-style typed history with attachments/full outputs.

### HIGH

| # | Gap | Location | Notes |
|---|-----|----------|-------|
| **H1** | **No multi-agent config (per-agent model, temp, max_steps, mode)** | `src/agent/config.rs`, `src/agent/manager.rs` | `LlmSessionConfig` is a global singleton (`OnceLock`). Need a `config/agents/<name>.md` parser + per-agent struct with: description, temperature, model, max_steps, mode (primary/subagent/all), hidden, color, top_p, permissions, task_permissions. The `AgentManager::new()` at `manager.rs:42` hardcodes `name: "default"` and uses a global provider config. |
| **H2** | **No agent modes (primary/subagent/all/hidden)** | `src/agent/types.rs`, `src/agent/manager.rs` | `Agent` struct at `manager.rs:10` has no `mode` field. Need: enum `AgentMode::Primary | Subagent | All`, hidden flag, integration with tool permission filtering and system prompt visibility. |
| **H3** | **Child session tree still lacks background/task_status semantics** | `src/agent/subagent.rs`, `src/session/`, `src/app.rs` | Task-created child sessions now exist and are navigable in the TUI. Remaining OpenCode parity: background task resume, task_status polling, and richer child-session lifecycle metadata. |
| **H4** | **Wildcard and pattern-based permission system** | `src/tools/permission.rs` | `AgentToolPolicies` only supports exact tool name matching per mode. Need: glob/wildcard matching (`"mymcp_*": "deny"`), pattern-specific bash permissions (`"git push": "ask"`, `"git *": "allow"`), per-agent permission overrides. |
| **H5** | **Scout subagent** | New: `src/agent/subagent.rs` | Read-only subagent that can clone repos for researching external docs/dependencies. Similar to Explore but with git clone capability and web search. |
| **H6** | **VLM-agent subagent** | New: `src/agent/subagent.rs` | Subagent for image analysis. Needs attachment/media support, vision-capable model routing, and image/PDF prompt-part handling. |
| **H7** | **Hidden/auto agents (compaction, title, summary)** | New: `src/agent/` | System agents that run automatically: compaction (truncates conversation context), title (generates session title), summary (summarizes long contexts). These are hidden from user but invokable via Task tool. |
| **H8** | **No MCP runtime** | New: `src/mcp/`, `src/tools/registry.rs` | OpenCode supports stdio/SSE/Streamable HTTP MCP servers, OAuth, tools, prompts, and resources. Crabcode only accepts the `mcp` config key and does not expose MCP tools/resources to the model. |
| **H9** | **No plugin runtime, hooks, or dynamic custom tools** | New: `src/plugin/`, `src/tools/registry.rs` | OpenCode loads internal/external plugins, fires hooks around system/tool execution, and loads local `{tool,tools}/*.{js,ts}` exports as tools. Crabcode ignores OpenCode `plugin` config and has no plugin hook pipeline. |
| **H10** | **No snapshot-backed file revert** | New: `src/snapshot/`, `src/session/revert.rs` | OpenCode captures snapshots around steps, persists patch parts, and can revert/unrevert file changes. Crabcode's message action "Undo" only truncates messages and does not restore the filesystem. |
| **H11** | **No background/resumable task jobs** | `src/tools/task.rs`, `src/agent/subagent.rs` | OpenCode `task` supports `background`, `task_id` resume, `task_status`, background result injection, and automatic parent continuation. Crabcode task returns a single string from a fresh subagent call. |
| **H12** | **No attachment/media pipeline** | `src/session/`, `src/llm/`, `src/tools/` | OpenCode supports file/image/PDF prompt parts, image normalization, tool-result attachments, and provider-specific media replay. Crabcode has no corresponding session or provider message shape. |
| **H13** | **OpenCode config schema is only partially implemented** | `src/config/configuration.rs` | Additional OpenCode config fields such as `instructions`, `reference`, `small_model`, `username`, `enabled_providers`, `disabled_providers`, `formatter`, `lsp`, `attachment`, `tool_output`, `snapshot`, `share`, `plugin`, and `experimental` are absent, ignored, or reported as unimplemented. |

### MEDIUM

| # | Gap | Location | Notes |
|---|-----|----------|-------|
| **M1** | **No @mention subagent invocation** | `src/command/parser.rs` | User typing `@explore find all tests` should route directly to the explore subagent. Need: extend `parse_input()` to detect `@subagent_name` prefix. |
| **M2** | **No websearch tool** | New: `src/tools/websearch.rs` | OpenCode supports Exa/Parallel web search. Crabcode has no equivalent. |
| **M3** | **No tool/media attachments** | `src/tools/types.rs`, `src/session/types.rs` | Tool results cannot carry attachments or media content, so current OpenCode behavior for MCP image/resource results, webfetch image output, and vision workflows has no place to land. |
| **M4** | **No apply_patch tool** | New: `src/tools/apply_patch.rs` | Apply unified diffs to files. Needed for patch-based editing workflows. |
| **M5** | **No LSP tool** | New: `src/tools/lsp.rs` | LSP code intelligence (go-to-def, find-references, diagnostics). |
| **M6** | **No doom loop recovery** | `src/tools/permission.rs`, `src/llm/client.rs` | When tools persistently fail, inject recovery prompts to break the loop. |
| **M7** | **Skill walk-up not bounded by git root** | `src/skill/mod.rs:50-64` | Walk-up for `.claude/` and `.agents/` skill dirs goes all the way to filesystem root. Should stop at git worktree boundary (like OpenCode). |
| **M8** | **No pattern-based skill permissions** | `src/skill/mod.rs`, `src/tools/skill.rs` | OpenCode supports `"internal-*": "deny"` style skill access control. Crabcode loads all skills unconditionally. |
| **M9** | **Plan/Build mode not user-toggleable mid-conversation** | `src/app.rs` (streaming setup) | Agent mode is set once at stream start. User should be able to toggle plan/build during conversations. |
| **M10** | **No task_status tool** | New: `src/tools/task_status.rs` | Needed once background subagents exist; lets the model poll or wait for asynchronous child-session work. |
| **M11** | **No repo_clone / repo_overview tools or reference aliases** | New: `src/tools/repo_clone.rs`, `src/tools/repo_overview.rs`, `src/reference/` | Required for OpenCode's scout/reference workflow: clone external repositories into a managed cache and inspect their structure without touching the user workspace. |
| **M12** | **No plan_exit handoff tool** | `src/agent/plan.rs`, `src/tools/` | OpenCode uses a model-callable plan exit flow to ask for plan approval and switch to build agent. Crabcode has plan/build policies but no equivalent tool-mediated handoff. |
| **M13** | **No formatter service or post-edit diagnostics** | New: `src/format/`, `src/lsp/` | OpenCode runs configured formatters after write/edit/patch and reports LSP diagnostics after patch. Crabcode edits files without that post-processing loop. |
| **M14** | **No durable tool-output truncation files** | New: `src/tools/truncate.rs` | OpenCode writes large outputs to a data-dir file and returns a preview plus path. Crabcode has per-tool inline truncation only, which prevents later grep/read over the full captured output. |
| **M15** | **No `instructions` config ingestion** | `src/config/configuration.rs`, `src/prompt/` | OpenCode supports extra instruction files/patterns from config. Crabcode allows the key but marks it unimplemented and only uses AGENTS/CLAUDE-style rule discovery. |
| **M16** | **No session sharing/autoshare** | `src/session/`, `src/config/configuration.rs` | OpenCode's `share` / `autoshare` config and shared session URLs are not represented. Crabcode ignores the OpenCode `share` key. |
| **M17** | **No invalid-tool fallback** | `src/tools/registry.rs` | OpenCode has an `invalid` tool to handle unknown/invalid tool calls cleanly. Crabcode has no equivalent fallback. |

### LOW

| # | Gap | Location | Notes |
|---|-----|----------|-------|
| **L1** | **No task permission controls** | `src/tools/task.rs` | Primary agent can always invoke any subagent. OpenCode has per-agent `task_permissions` to restrict which subagents an agent can spawn. |
| **L2** | **No agent color theming** | `src/agent/config.rs` | Per-agent color for UI differentiation of which agent is speaking. |
| **L3** | **No agent creation wizard** | New: command handler | `opencode agent create` interactive wizard missing. UX feature but tied to multi-agent config. |
| **L4** | **No per-agent top_p** | `src/agent/config.rs` | Per-agent LLM sampling parameter. |
| **L5** | **No `small_model` selection** | `src/config/configuration.rs`, `src/agent/` | OpenCode uses small-model config for title/summary/utility agents. Crabcode has no utility-model selection. |
| **L6** | **No username display override** | `src/config/configuration.rs`, `src/ui/` | OpenCode config can override the displayed username in conversations. |
| **L7** | **No provider enable/disable filters** | `src/config/configuration.rs`, `src/model/discovery.rs` | OpenCode can restrict loaded providers with `enabled_providers` and `disabled_providers`; Crabcode allows but does not apply these keys. |
