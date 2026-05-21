# Premature Complete Bug

This is the running memory for dogfooding reports where crabcode ends a turn before the task is actually done, compared against Codex behavior.

## Protocol

1. Dogfood crabcode on crabcode.
2. If crabcode completes prematurely, capture the visible chat history and `app.log`.
3. Use Codex to inspect the history/logs, add a focused fix or diagnostic, and append the findings here.
4. Treat this file as the durable thread across repeated incidents.

## 2026-05-21 Incident

### User-Visible Symptom

Crabcode was asked to make tool calls more permissive like Codex. It started the work, made partial edits, then ended with an intermediary-style message:

> I’ll remove noisy comments and keep the policy readable.

From the user's perspective this was not a final answer: the plan still had unfinished validation/wrap-up work.

### `app.log` Evidence

Relevant sequence:

- `21:50:55`: `edit` succeeded in `src/tools/permission.rs`.
- `21:50:55`: provider step 21 started with 42 messages.
- `21:50:57-21:50:58`: text chunks streamed for the message above.
- `21:50:58`: metadata said `assistant_message_phase=final_answer`.
- `21:50:58`: metadata said `response.completed end_turn=None`.
- `21:50:58`: AISDK logged `provider_step_finish step=21 has_tool_call=false end_turn=None last_phase=final_answer assistant_text_chars=58 action=finish preview="I’ll remove noisy comments and keep the policy readable."`
- `21:50:58`: relay exhausted and crabcode marked the stream completed:
  - `outcome=Exhausted`
  - `effective_outcome=Finished`
  - `stop_reason=Some(Finish)`

Important secondary signal: tool execution logs continued after the primary stream was already marked complete:

- `21:51:16`: `write` created `TOOL_PERMISSIONS_CHANGES.md`.
- `21:51:46`: `write` created `PERMISSIVE_TOOL_CALLS_SUMMARY.md`.
- `21:52:05`: `task` returned a result.
- `21:52:12`: another `task` started and failed with `Provider stream ended without a terminal completion event`.
- `21:52:18+`: more `read`, `edit`, and `bash` attempts were logged.

The existing logs do not include enough session/tool-call identity on those late tool logs, so we cannot yet prove whether they came from the same stream, a subagent, or another active/background stream.

### Current Working Theory

There are likely two overlapping issues:

1. The model/provider classified an intermediary update as `final_answer` with `end_turn=None`. `aisdk::stream_with_tools` treats `final_answer + no tool call + end_turn != false` as a real finish.
2. The tool lifecycle logs can outlive the visible primary completion, but they currently lack `session_id`, `call_id`, `agent_mode`, and subagent parent/child context. That makes post-completion tool execution hard to attribute.

Codex reference behavior in `.devrefs/references/openai/codex/codex-rs/core/src/session/turn.rs` treats a closed stream before `response.completed` as an error. Crabcode already has a similar guard inside `aisdk/src/response.rs` for provider streams without a terminal completion event. This incident is different: the provider did emit `response.completed`, but the text looked like progress-update content, not a genuine final response.

## Changes Made So Far

### Runtime Fix Applied 2026-05-21

The attempted `update_plan`-state guard was rejected in favor of stricter reference parity.

- Codex continues from structured stream/tool lifecycle state: tool output needing follow-up, pending input, and `response.completed end_turn == Some(false)`.
- opencode exits from persisted assistant finish state only when there are no unresolved tool-call parts; it does not inspect assistant prose or todo/plan wording.
- Neither reference uses `update_plan` or natural-language progress phrasing as a completion gate.

Applied two reference-shaped fixes:

- `src/app.rs` now defers session completion if an `End` arrives while tool messages from the current streaming boundary are still `running`. Completion resumes after the pending tool result resolves. This mirrors opencode's unresolved tool-part exit condition and Codex's in-flight tool drain boundary.
- `src/prompt/mod.rs` now tells Codex-style models to treat preambles/progress updates as interim commentary and reserve final answers for completed work. This is a prompt/protocol correction, not an assistant-text keyword matcher.

AISDK remains limited to reference-style stream signals: tool calls, `end_turn=false`, phase/lifecycle events, terminal-event enforcement, and bounded max-step handling. It still does not special-case `update_plan` inside argument parsing.

Validation:

- `cargo fmt --check`
- `cargo test -p aisdk`
- `cargo test stream_finish_waits_for_running_tool_result`
- `cargo test codex_prompt_separates_progress_from_final_answers`
- `cargo check`

### Permission-Policy Changes From Dogfooding Run

These were already modified by crabcode before the premature completion:

- `src/tools/permission.rs`
  - Read/search style operations no longer prompt for sensitive paths or paths outside the working directory.
  - Write/edit operations still check sensitive paths, external paths, and gitignored writes.
  - Bash permission prompting was removed in the current dirty worktree.
  - Added/updated permission tests for permissive reads and write-based allow-always behavior.
- `src/tools/bash.rs`
  - Dangerous command pattern checks were removed in the current dirty worktree.

These changes are related to the original task, but they are not the premature-complete fix. They should be reviewed separately for safety before landing.

### Diagnostics Added For Premature Completion

Added narrow lifecycle logging to make the next recurrence attributable.

- `src/llm/client.rs`
  - `GOING TO STREAM` now logs `session_id`, provider, model, agent mode, max steps, and input message count.
  - `Stream completed` now logs `session_id`.
  - `session_id` is cloned before passing into AISDK tool conversion so it remains available for completion logging.

- `src/tools/aisdk_bridge.rs`
  - Tool logs now include `tool`, generated `call_id`, `session_id`, `message_id`, `agent_mode`, sender presence, duration, and output/error bytes.
  - UI send failures for `ToolCalls` and `ToolResult` now log as `ui_send_failed`.
  - This should reveal whether late tool calls are attached to the completed primary session, a child session, or a different stream.

- `src/tools/task.rs`
  - Task tool now logs `[TASK] start`, `[TASK] finish`, and `[TASK] error` with parent session, child session, subagent type, duration, output bytes, and child tool-call count.
  - Child-session forwarding now logs start and close.

- `src/agent/subagent.rs`
  - Subagent streams now log `[SUBAGENT] stream_start`, `[SUBAGENT] stream_finish`, and `[SUBAGENT] stream_failed`.
  - Subagent metadata is mirrored into `app.log` as `[SUBAGENT_METADATA]`.
  - Fixed a borrow-after-move compile issue by cloning `session_id` when passing it into AISDK tool conversion.

## Verification State

- `cargo fmt --check` passes as of the runtime fix.
- `cargo test -p aisdk` passes for the existing reference-style AISDK lifecycle behavior.
- `cargo test stream_finish_waits_for_running_tool_result` passes.
- `cargo test codex_prompt_separates_progress_from_final_answers` passes.
- `cargo check` passes with existing warnings. The permission-policy and diagnostic edits from the earlier dogfooding run remain separate dirty work and should be validated before landing.

## Next Debugging Targets

1. Dogfood the same class of task again and inspect new log fields:
   - Match `[AISDK_TOOL] call/result/error` by `session_id` and `call_id`.
   - Check whether any tool call occurs after `Stream completed` for the same `session_id`.
   - Check `[TASK]` and `[SUBAGENT]` lines to identify child-session activity.
2. If the same session still emits post-completion tools, check whether those events bypass `ToolCallViewState` and need a lower-level in-flight counter.
3. If provider output still misclassifies progress as `final_answer`, inspect the raw Responses events and prompt text to verify whether the updated final/commentary contract is being sent.

## Open Questions

- Were the post-`21:50:58` tool calls from the same primary stream, a subagent, or another concurrent/background stream?
- Does the UI mark a turn complete solely when the relay exhausts, even if task/subagent senders still exist?
- Should `final_answer + end_turn=None` be trusted for ChatGPT OAuth/Codex transport, or should `end_turn=true` be required for final completion when tools are enabled?
- What should be the canonical crabcode pending-work signal that can keep the turn alive without inspecting assistant prose or plan/todo text?
