# Codex Parity Roadmap

> Created: 2026-05-18  
> Scope: harness behavior needed to make Crabcode perform like Codex with Codex/GPT-5.x models, including GPT-5.5, while keeping Crabcode's multi-workspace UI, theming, sessions, and non-chat UX.

## Goal

Make Crabcode's Codex/GPT-5.x path, including GPT-5.5, behave like Codex CLI from the model's point of view.

This is not a product-clone checklist. It is a harness-contract checklist: prompts, model request shape, tool names, tool schemas, tool result history, turn loop behavior, subagent semantics, permissions, sandboxing, compaction, and the chat-panel rendering that makes tool work understandable.

## Reference Files

- Codex reference root: `.devrefs/references/openai/codex`
- Codex base instructions: `.devrefs/references/openai/codex/codex-rs/protocol/src/prompts/base_instructions/default.md`
- Codex turn loop: `.devrefs/references/openai/codex/codex-rs/core/src/session/turn.rs`
- Codex tool routing: `.devrefs/references/openai/codex/codex-rs/core/src/tools/spec_plan.rs`
- Codex shell tool spec: `.devrefs/references/openai/codex/codex-rs/core/src/tools/handlers/shell_spec.rs`
- Codex apply_patch spec: `.devrefs/references/openai/codex/codex-rs/core/src/tools/handlers/apply_patch_spec.rs`
- Codex multi-agent tools: `.devrefs/references/openai/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs`
- Codex tool-call UI: `.devrefs/references/openai/codex/codex-rs/tui/src/exec_cell/render.rs`
- Crabcode OpenAI/Codex transport: `src/llm/client.rs`
- Crabcode prompt composer: `src/prompt/mod.rs`
- Crabcode AI SDK bridge: `src/tools/aisdk_bridge.rs`
- Crabcode current subagents: `src/agent/subagent.rs`
- Crabcode chat tool renderer: `src/ui/components/chat.rs`

## Current Snapshot

Crabcode already has useful pieces: OpenAI OAuth token refresh, `/backend-api/codex/responses` routing, `store=false`, provider/model selection, permissions, a multi-step AI SDK tool loop, dynamic `question`/`task` tools, skills, AGENTS/CLAUDE rule loading, and session UI.

The parity blockers are still fundamental:

- OpenAI OAuth currently sets `strip_system_and_developer_messages(true)`, so `SystemPromptComposer` output, AGENTS instructions, environment context, skills, and subagents can be dropped for Codex-backed requests.
- The AI SDK loop converts tool results into synthetic user messages (`Tool x result`) instead of preserving Responses API function-call/function-output items.
- Crabcode persists UI tool panels, but `convert_messages()` skips `MessageRole::Tool`, so later turns lose model-visible tool call history.
- Tool names are Crabcode/OpenCode-style (`bash`, `read`, `grep`, `glob`, `edit`, `write`, `todowrite`, `task`) rather than Codex-style (`exec_command`, `write_stdin`, `apply_patch`, `update_plan`, `view_image`, `spawn_agent`, `wait_agent`, etc.).
- Current `task` subagents are single-shot model calls. Codex subagents are real child threads with their own turn loops, tool calls, status, waiting, resuming, and closure.
- Tool-call UI renders generic JSON tool rows. Codex renders semantic cells: `Ran`, `Running`, `Explored`, `Called`, with grouped read/search/list commands and concise output gutters.

## Priority Checklist

### P0 - Model Contract

- [ ] Add a request/response trace harness for Codex mode.
  - Capture sanitized outbound request JSON for the same fixture prompt.
  - Capture `instructions`, `input`, `tools`, `parallel_tool_calls`, model, effort, service tier, and output schema.
  - Compare Crabcode against Codex reference behavior before changing large pieces.

- [ ] Preserve Codex instructions for OpenAI OAuth.
  - Do not silently drop `SystemPromptComposer` output.
  - Move base instructions, AGENTS, environment, permissions, skills, and app/plugin instructions into fields accepted by the ChatGPT Codex backend.
  - Keep the Codex base prompt close to the reference instead of the current short fallback.

- [ ] Store model-visible conversation items.
  - Persist assistant messages, function calls, function outputs, reasoning summaries, and tool outputs in a model-replayable form.
  - Keep UI tool panels as a render layer, not the canonical model history.
  - Rehydrate the next turn from canonical Responses-style items, not only text messages.

- [ ] Replace synthetic tool result messages.
  - Stop feeding tool results back as plain user text in Codex mode.
  - Return function-call outputs using the provider's native item shape.
  - Preserve call IDs exactly.

- [ ] Match Codex request options.
  - Send `parallel_tool_calls` based on model support.
  - Support reasoning effort, reasoning summary, verbosity, service tier, and final output schema when available.
  - Keep `store=false` for ChatGPT Codex transport.

### P0 - Tool Surface

- [ ] Add a Codex tool profile.
  - Use Codex names and schemas for Codex/GPT-5.x models.
  - Keep Crabcode's existing tool profile for non-Codex providers where useful.

- [ ] Implement `exec_command`.
  - Replace model-visible `bash` with Codex's `exec_command` schema.
  - Include `cmd`, `workdir`, `shell`, `login`, `tty`, `yield_time_ms`, `max_output_tokens`, `sandbox_permissions`, `justification`, and `prefix_rule`.
  - Return structured output with wall time, exit code, session ID for background commands, original token count, and truncated output.

- [ ] Implement `write_stdin`.
  - Support polling and writing to an existing background/PTY session.
  - Preserve command session IDs across tool calls.

- [ ] Implement freeform `apply_patch`.
  - Use the Codex Lark grammar shape.
  - Do not wrap patch input in JSON.
  - Emit patch progress/diff events for the UI.

- [ ] Implement `update_plan`.
  - Replace `todowrite` in Codex mode.
  - Render plan updates as their own user-visible progress surface.

- [ ] Add Codex-compatible utility tools.
  - `view_image`
  - `web_search` when enabled
  - `request_user_input` or an equivalent bounded user-question flow
  - MCP resource tools: `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource`

### P1 - Turn Loop

- [ ] Own the Responses-style turn loop.
  - Sample model output.
  - Stream assistant text/reasoning/tool argument deltas.
  - Dispatch tool calls.
  - Append native tool outputs.
  - Continue sampling until `end_turn` or no follow-up is needed.

- [ ] Execute parallel-safe tools concurrently.
  - Use per-tool parallel support flags.
  - Serialize tools that mutate shared state or require exclusive terminal access.

- [ ] Add retry and fallback behavior.
  - Retry transient stream failures with backoff.
  - Keep the same turn-scoped client/session when retrying.
  - Surface reconnect warnings without corrupting history.

- [ ] Add compaction.
  - Pre-turn compaction when context exceeds the active model's limit.
  - Mid-turn compaction when tools or pending input require continuation.
  - Model-downshift compaction when switching to a smaller context window.

### P1 - Permissions And Sandboxing

- [ ] Move permission checks into a tool orchestrator.
  - Approval preflight.
  - Sandbox selection.
  - Retry after sandbox denial with escalation request.
  - Prefix-rule persistence for approved command families.

- [ ] Match Codex command approval semantics.
  - `sandbox_permissions="require_escalated"`
  - Required `justification`
  - Optional `prefix_rule`
  - No broad prefix rules for arbitrary scripting or destructive commands.

- [ ] Add network and filesystem policy concepts.
  - Workspace-write default.
  - Extra writable roots.
  - Network-denial handling.
  - Per-turn/session granted permissions.

### P1 - Subagents

- [ ] Replace `task` with Codex-style agent control in Codex mode.
  - `spawn_agent`
  - `send_input`
  - `wait_agent`
  - `resume_agent`
  - `close_agent`

- [ ] Make spawned agents real sessions.
  - Child thread IDs.
  - Parent/child relation.
  - Own prompt, tools, permissions, cancellation, status, and history.
  - Optional `fork_context`.
  - Model and reasoning overrides only when explicitly requested or clearly needed.

- [ ] Add subagent usage rules to the prompt.
  - Tell the model when to delegate.
  - Prevent subagent spawning unless the user explicitly asks for agents/delegation or the configured tool profile allows it.
  - Keep wait behavior sparse and non-blocking.

### P2 - Chat Panel UX

- [ ] Replace JSON tool rows with Codex-like history cells.
  - `Running <cmd>` while active.
  - `Ran <cmd>` when complete.
  - `(no output)` for empty output.
  - Tree gutters: `└`, `│`, continuation indentation.
  - Red/green status bullets.

- [ ] Add `Explored` grouping.
  - Parse `exec_command` shell commands into semantic read/list/search operations.
  - Coalesce adjacent read/search/list commands into one exploration cell.
  - Render examples like `Read dialog.rs, app.rs` and `Search shimmer_spans`.

- [ ] Add patch cells.
  - Stream partial `apply_patch` changes.
  - Show concise file-level diffs.
  - Keep full details available in transcript/history.

- [ ] Add agent cells.
  - Spawned/running/completed/errored subagent states.
  - Wait summaries.
  - Final subagent result summaries.

## First Implementation Slice

Start with a "Codex mode contract" slice before polishing UI:

1. Add a debug request recorder around `aisdk/src/providers/openai.rs` or the higher-level LLM client.
2. Add a Codex-mode prompt/request fixture test that asserts the request contains full instructions, environment, AGENTS text, model-visible tools, and no dropped context.
3. Change OpenAI OAuth request construction so full Codex instructions survive the ChatGPT Codex backend path.
4. Add a canonical model-history representation that can store native function calls and function outputs.
5. Add Codex aliases for `exec_command`, `apply_patch`, and `update_plan`, even if the first handlers delegate internally to existing Bash/edit/todo code.

Only after this slice should the chat rendering be rewritten. The Codex-style renderer depends on getting the event model right; otherwise the UI will be pretty but still model-divergent.

## Non-Goals

- Do not replace Crabcode's multi-workspace setup.
- Do not replace themes, dialogs, model picker, sessions dialog, or global app layout.
- Do not remove the existing non-Codex tool profile unless it blocks Codex mode.
- Do not chase every Codex app/plugin/cloud feature before the local harness contract is correct.
