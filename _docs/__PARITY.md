# Crabcode Harness Parity Audit

Checked: 2026-05-19.

Scope: core harness behavior only: agent loop, system prompt, subagents, tool calling, skill loading, agent config, commands, and permissions. This compares crabcode source against the local opencode reference in `.devrefs/references/anomalyco/opencode` plus the requested opencode behavior.

## Feature Matrix

| # | Feature | OpenCode | Crabcode | Gap |
|---|---------|----------|----------|-----|
| 1.1 | Multi-step agentic iteration | Streams model responses, accumulates tool calls, executes tools, appends observations, and continues until stop or step limit. | Present. `src/llm/client.rs` calls `stream_with_tools`; `aisdk/src/response.rs` loops over steps, tool calls, and observations. | No major parity gap for the core loop. |
| 1.2 | Cancellation token support | User interruption cancels active generation and agent work. | Present for model streaming. `src/llm/client.rs` relays cancellation and emits `ChunkMessage::Cancelled`; tools get abort channels through the AI SDK bridge. | Tool cancellation is weaker: `src/tools/aisdk_bridge.rs` creates a fresh abort channel instead of wiring the top-level cancellation token through every long-running tool. |
| 1.3 | Step limit and fallback | Enforces configured max steps, then injects a max-steps prompt and performs a text-only completion. | Mostly present. `MAX_STEPS_REACHED_PROMPT` is injected and `src/llm/client.rs` calls the provider again with no tools when the loop reaches the limit. | `maxSteps` alias is explicitly unsupported; behavior is tied to the current `steps` config path. |
| 1.4 | Chunk-based streaming | Emits text, reasoning, tool calls, tool results, errors, metrics, and cancellation chunks. | Present. `src/llm/mod.rs` defines `Text`, `Reasoning`, `ToolCalls`, `ToolResult`, `Failed`, `Metrics`, `Cancelled`, plus permission, question, and subagent chunks. | No major parity gap for the listed chunk types. |
| 1.5 | Plan/Build mode toggle | Plan mode is read-only; build mode can execute write-capable tools. | Present. `src/app.rs` toggles Plan/Build; `src/tools/permission.rs` denies `write`, `edit`, and `bash` in Plan. | This is mode-based policy only, not the full opencode agent-mode registry. |
| 1.6 | Permission preflight during tool execution | Tool calls are preflighted and can surface permission dialogs mid-run. | Present but limited. `src/tools/aisdk_bridge.rs` preflights before execution; `src/tools/permission.rs` emits permission requests; `src/app.rs` handles the dialog. | Policy inputs are hardcoded/in-memory rather than driven by opencode-style config rules. |
| 1.7 | Configurable max steps per agent | Each agent can define max steps; limit injects the max-steps prompt. | Partially present. `src/config/configuration.rs` parses `agent.<name>.steps`; app and print paths pass agent-specific step counts into the LLM call. | Only `steps` is supported; broader per-agent config and deprecated `maxSteps` compatibility are missing. |
| 2.1 | Provider-specific prompt header and behavior | Chooses provider/model-specific prompts such as Beast/OpenAI, Anthropic, Gemini, Codex, and other provider variants. | Partial. `src/prompt/mod.rs` has Beast, Anthropic, Gemini, and Codex prompt branches. | Prompt set is simpler than opencode, with fewer provider/model variants and less complete behavioral parity. |
| 2.2 | Environment context block | Includes workdir, git status/repo, platform, and date in the system prompt. | Present. `src/prompt/mod.rs` emits workdir, git repo status, platform, and current date. | Minor wording/content differences only. |
| 2.3 | Tool schemas block | Lists all registered tools as JSON in the system prompt. | Partial. `SystemPromptComposer` can emit tool schemas if built with a tool registry, but runtime app and print composition do not call `.with_tool_registry(...)`. | Actual runtime system prompts do not include the tool schemas block even though provider requests still receive tool schemas through the AI SDK. |
| 2.4 | Custom instructions discovery | Walks up for project instructions and supports global fallback. | Partial. `src/prompt/rules.rs` finds local `AGENTS.md`/`CLAUDE.md` and global crabcode/Claude files. | Does not stop at git worktree boundary, does not include opencode global paths, and does not support config-driven instruction entries. |
| 2.5 | Available skills block | Emits `<available_skills>` with names and descriptions. | Present in interactive mode. `src/prompt/mod.rs` renders skills when `SkillStore` is attached; `src/app.rs` initializes the store. | Print mode does not initialize/attach the skill store, so the block can be absent outside the app path. |
| 2.6 | Available subagents block | Lists subagent names and descriptions so the primary agent can pick a Task target. | Present. `src/prompt/mod.rs` emits `<available_subagents>`; `src/agent/subagent.rs` supplies definitions. | Only the currently implemented subagents are listed; missing scout, VLM, and hidden/system agents. |
| 2.7 | Prompt-level subagent selection guidance | Primary agent sees when to use the Task tool and which subagent to choose. | Partial. The prompt lists subagent descriptions and the Task tool schema constrains allowed types. | No task-permission matrix or hidden-agent metadata in the prompt. |
| 3.1 | Task tool | Primary agents spawn subagents through a Task tool. | Present. `src/tools/task.rs` implements Task; `src/tools/init.rs` registers it dynamically. | Missing opencode parameters such as background execution, task IDs, command routing, and task status. |
| 3.2 | `explore` subagent | Fast read-only subagent with glob, grep, read, and list. | Present. `src/agent/subagent.rs` defines `Explore` with those read-only tools. | No major parity gap for the basic explore profile. |
| 3.3 | `general` subagent | Full multi-step subagent, excluding `todowrite`. | Present. `src/agent/subagent.rs` defines `General` with broad tools and excludes `todowrite`. | Permission behavior is still governed by crabcode's simpler policy engine. |
| 3.4 | `scout` subagent | Read-only external research agent that can clone repositories. | Missing. `SubAgentType` only has `Explore` and `General`. | Need scout definition, repo clone/overview tools, and external research permissions. |
| 3.5 | `vlm-agent` | Image-analysis subagent. | Missing. | Need image input plumbing, VLM model selection, and a VLM-capable subagent definition. |
| 3.6 | Hidden/system agents | Compaction, title, and summary agents run automatically and are hidden from user autocomplete. | Missing as an agent system. | Need hidden agent definitions and automatic invocation hooks for compaction, title, and summary flows. |
| 3.7 | Child sessions | Subagent work is represented as child sessions with parent/child navigation. | Partial. `src/session/manager.rs` supports child sessions; `src/tools/task.rs` creates subagent sessions; `src/app.rs` renders subagent events. | Lacks opencode-style background tasks, task status tracking, and richer child-session lifecycle controls. |
| 3.8 | `@mention` subagent invocation | User input can invoke subagents by mention. | Missing. Slash command parsing exists in `src/command/parser.rs`; autocomplete focuses on files/commands, not subagents. | Need parser, autocomplete, and dispatch path for `@subagent` invocation. |
| 3.9 | Agent mode: primary, subagent, all | Agent definitions declare where they are available. | Missing. | Need agent registry fields and enforcement for primary-only, subagent-only, and all-mode agents. |
| 3.10 | Hidden agents from autocomplete | Agents can be invokable but hidden from autocomplete. | Missing. | Requires hidden metadata in agent definitions and autocomplete filtering. |
| 3.11 | Task permissions | Controls which agents can invoke which subagents. | Missing. Task validates only the hardcoded subagent enum. | Need per-agent task permission rules and enforcement before spawning a child agent. |
| 4.1 | `bash` tool | Executes shell commands. | Present. `src/tools/bash.rs`; registered in `src/tools/init.rs`. | Policy granularity differs from opencode. |
| 4.2 | `edit` tool | Exact string replacement in files. | Present. `src/tools/edit.rs`; registered in `src/tools/init.rs`. | No major parity gap for the basic tool. |
| 4.3 | `write` tool | Creates or overwrites files. | Present. `src/tools/fs/write.rs`; registered in `src/tools/init.rs`. | No major parity gap for the basic tool. |
| 4.4 | `read` tool | Reads files with offset/limit pagination and can inspect directories. | Present. `src/tools/fs/read.rs`; registered in `src/tools/init.rs`. | Confirm directory behavior stays aligned with opencode's separate `list` semantics during future changes. |
| 4.5 | `grep` tool | Regex search with include filters. | Present. `src/tools/fs/grep.rs`; registered in `src/tools/init.rs`. | No major parity gap for the basic tool. |
| 4.6 | `glob` tool | File pattern matching. | Present. `src/tools/fs/glob.rs`; registered in `src/tools/init.rs`. | No major parity gap for the basic tool. |
| 4.7 | `list` tool | Deliberate tree-style directory listing, separate from read-directory behavior. | Partial. `src/tools/fs/list.rs` lists direct entries. | Needs recursive/tree-style output parity with opencode's `list`. |
| 4.8 | `skill` tool | Loads `SKILL.md` by name and lists available skills in its description. | Present. `src/tools/skill.rs`; registered in `src/tools/init.rs`. | Availability filtering does not honor skill permission patterns. |
| 4.9 | `task` tool | Spawns subagents. | Present. `src/tools/task.rs`; dynamically registered in `src/tools/init.rs`. | Missing background/status/command/task-permission behavior. |
| 4.10 | `todowrite` tool | Manages structured task lists. | Present. `src/tools/todowrite.rs`; registered in `src/tools/init.rs`. | No major parity gap for registration; behavior should be checked separately if exact todo schema parity is required. |
| 4.11 | `webfetch` tool | Fetches web content and converts it to readable text/markdown. | Present. `src/tools/webfetch.rs`; registered in `src/tools/init.rs`. | Network and markdown-conversion fidelity may differ, but the core tool exists. |
| 4.12 | `websearch` tool | Searches the web through Exa AI. | Missing. | Need search provider integration, schema, permissions, and registration. |
| 4.13 | `question` tool | Asks the user questions during execution. | Present. `src/tools/question.rs`; dynamically registered in `src/tools/init.rs`. | No major parity gap for basic interactive questions. |
| 4.14 | `extract-images` tool | Saves session images to disk. | Missing. | Need session attachment/image storage model and tool registration. |
| 4.15 | `apply_patch` tool | Applies diffs/patches. | Missing. | Need patch application tool, schema, permissions, and safe failure handling. |
| 4.16 | `lsp` tool | Experimental LSP code intelligence. | Missing. | Need LSP client/session management and tool schema. |
| 5.1 | Skill discovery locations | Searches project/global opencode, Claude, and agents skill locations. | Partial. `src/skill/mod.rs` scans crabcode/opencode globals plus project/global `.claude` and `.agents`. | Missing config `skills.paths` and URL-based skills; project `.opencode`/`.crabcode` discovery is rooted, not fully walk-up like `.claude`/`.agents`. |
| 5.2 | Walk-up to git worktree | Walks up project directories until the git worktree boundary. | Partial. `src/skill/mod.rs` walks up for `.claude` and `.agents`. | Walk-up does not stop at the git worktree boundary and is not consistently applied to all project skill roots. |
| 5.3 | Skill frontmatter | Requires YAML frontmatter with `name` and `description`. | Partial. `name` is required, but `description` is optional in `src/skill/mod.rs`. | Enforce required descriptions for opencode compatibility. |
| 5.4 | Pattern-based skill permissions | Supports allow/deny rules such as `internal-* = deny`. | Missing. | Need skill permission parsing, glob matching, and filtering before prompt/tool exposure. |
| 5.5 | Skill tool list in description | Skill tool description lists available skills. | Present. `src/tools/skill.rs` builds a description from `SkillStore::list()`. | Should be filtered by skill permissions once those exist. |
| 6.1 | JSON agent config | Supports agent config in `opencode.json`. | Partial. Crabcode config parses agent tool allowlists and `steps` from JSON/JSONC. | Missing most opencode agent fields and full `opencode.json` agent compatibility. |
| 6.2 | Markdown agent config | Supports `~/.config/opencode/agents/<name>.md` frontmatter. | Missing as runtime config. `src/config/configuration.rs` inventories agent markdown files but does not parse/apply them. | Need markdown frontmatter parser and merge logic. |
| 6.3 | Per-agent execution fields | Supports description, temperature, model, max steps, mode, hidden, color, top_p, permissions, and task permissions. | Mostly missing. `src/agent/config.rs` is global LLM session state; config currently supports only tool policies and `steps`. | Need a first-class agent definition model and enforcement path. |
| 6.4 | Agent creation wizard | `opencode agent create` scaffolds new agent config. | Missing. | Add command/CLI flow to create agent markdown or JSON config entries. |
| 7.1 | User-defined command files | Loads `.opencode/commands/<name>.md`. | Missing. `src/command` only implements built-in slash commands and skill-backed commands. | Need command file discovery, parsing, and registration. |
| 7.2 | Command frontmatter | Supports description, agent, model, and subtask. | Missing. | Need command frontmatter schema and dispatch behavior. |
| 7.3 | Template variables | Expands `$ARGUMENTS`, `$1`, `$2`, and similar variables. | Missing. | Add command template expansion before sending prompts. |
| 7.4 | Shell output injection | Expands command-substitution snippets inside command prompts. | Missing. | Add shell execution path with permission checks and output injection. |
| 7.5 | File references | Expands `@path/to/file` references inside command prompts. | Missing. | Reuse file reference parsing/attachment code or add command-specific resolver. |
| 7.6 | Subtask command routing | Commands can run as subtasks through the Task tool. | Missing. | Add `subtask` handling that routes through Task with the requested agent. |
| 8.1 | Per-tool permissions | Per-tool rules support allow, deny, and ask. | Partial. `src/tools/permission.rs` has Plan/Build defaults and in-memory allow/deny/ask outcomes. | No config-level per-tool allow/deny/ask matrix. |
| 8.2 | Wildcard permission patterns | Supports patterns such as `mymcp_* = deny`. | Missing. | Add wildcard matcher and config schema. |
| 8.3 | Bash command patterns | Supports command-specific bash permissions such as `git push = ask` and `git * = allow`. | Missing. | Replace hardcoded bash ask behavior with ordered pattern-specific rules. |
| 8.4 | Per-agent permission override | Agent config can override global permissions. | Missing. | Requires first-class agent config plus permission merge order. |
| 8.5 | External directory gating | Writes/commands outside workspace are gated. | Present. `src/tools/permission.rs` checks external paths and sensitive paths. | No major parity gap for the basic safety gate, but rule configurability is missing. |
| 8.6 | Doom loop recovery prompts | Detects repeated permission/operation loops and prompts for recovery. | Missing. | Need loop detection in agent execution and recovery prompt injection. |

## Priority Gaps

### CRITICAL

1. Runtime system prompt omits the tool schemas block.
   - Files: `src/prompt/mod.rs`, `src/app.rs`, `src/main.rs`, `src/tools/init.rs`.
   - `SystemPromptComposer` can render tool schemas, but the app and print paths do not pass a registry with `.with_tool_registry(...)`. Build the static and dynamic tool registries before composing the system prompt, then include `question` and `task` as dynamic schemas.

2. OpenCode-compatible custom commands are absent.
   - Files: `src/command/parser.rs`, `src/command/registry.rs`, `src/command/handlers.rs`.
   - Add discovery for `.opencode/commands/<name>.md`, frontmatter parsing for `description`, `agent`, `model`, and `subtask`, template expansion for `$ARGUMENTS` and positional args, command-substitution injection with permission checks, file-reference expansion, and Task routing for subtask commands.

3. Permission system is not OpenCode-compatible.
   - Files: `src/tools/permission.rs`, `src/config/configuration.rs`, `crabcode.schema.json`, `_docs/config.mdx`.
   - Add config-driven `allow`, `deny`, and `ask` rules; wildcard tool matching; ordered bash command patterns; per-agent override merging; task permissions; skill permissions; and durable approvals where appropriate.

4. First-class agent registry/config is missing.
   - Files: `src/agent/config.rs`, `src/agent/subagent.rs`, `src/config/configuration.rs`.
   - Introduce an agent definition model covering description, model, temperature, top_p, steps/max_steps aliases, mode, hidden, color, permissions, and task permissions. Parse both JSON config and markdown frontmatter agent files.

### HIGH

1. Missing subagent set beyond `explore` and `general`.
   - Files: `src/agent/subagent.rs`, `src/tools/task.rs`, `src/tools/init.rs`.
   - Add `scout`, `vlm-agent`, and hidden `compaction`, `title`, and `summary` agents. Scout also needs repo clone/overview tools and external research permissions; VLM needs image input/model routing.

2. Task tool lacks background/status/command parity.
   - Files: `src/tools/task.rs`, `src/session/manager.rs`, `src/app.rs`.
   - Add `task_id`, `background`, `command`, and task-status support. Enforce task permissions before child session creation and expose background task lifecycle events.

3. Missing or partial built-in tools.
   - Files: `src/tools/init.rs`, `src/tools/fs/list.rs`, new tool modules.
   - Add `websearch`, `extract-images`, `apply_patch`, and `lsp`. Update `list` to produce opencode-style tree output rather than only direct directory entries.

4. Instruction discovery is incomplete.
   - Files: `src/prompt/rules.rs`, `src/config/configuration.rs`, `src/main.rs`.
   - Stop walk-up at the git worktree boundary, include opencode global instruction paths, support config-provided instruction entries, and attach `SkillStore` in print mode so available skills appear consistently.

### MEDIUM

1. Skill loader needs compatibility hardening.
   - Files: `src/skill/mod.rs`, `src/tools/skill.rs`, `src/config/configuration.rs`.
   - Add config `skills.paths` and URL skills, enforce required descriptions, apply permission-pattern filtering, and bound walk-up discovery by the git worktree.

2. `@mention` subagent invocation is missing.
   - Files: `src/command/parser.rs`, `src/autocomplete`, `src/app.rs`.
   - Add parser/autocomplete support for subagent mentions and route mentioned subagents into the Task flow with the rest of the message as the prompt.

3. Tool-call cancellation should propagate into tools.
   - Files: `src/llm/client.rs`, `src/tools/aisdk_bridge.rs`, tool implementations that can block.
   - Wire the top-level cancellation token into the per-tool abort channel so bash, webfetch, and subagent execution stop promptly on user interruption.

4. Max-step compatibility should accept OpenCode aliases.
   - Files: `src/config/configuration.rs`, `crabcode.schema.json`, `_docs/config.mdx`.
   - Accept `max_steps` and deprecated `maxSteps` as aliases for `steps`, with a warning only if necessary.

### LOW

1. Agent creation wizard is missing.
   - Files: `src/command/handlers.rs`, `src/main.rs`.
   - Add `crabcode agent create` or a slash-command equivalent after the agent definition format is implemented.

2. Provider prompt set is simplified.
   - Files: `src/prompt/mod.rs`.
   - Add remaining opencode provider/model prompt variants only after the core config, command, permission, and tool gaps are closed.

3. Per-agent visual metadata can wait.
   - Files: `src/agent/config.rs`, UI consumers later.
   - `color` is part of opencode agent config, but it does not affect harness execution and should be implemented after execution semantics are compatible.
