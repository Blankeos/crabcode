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

## 2026-05-22 Recurrence

### User-Visible Symptom

Crabcode was asked to add an opencode-style `ctrl+p` command palette and reduce the footer hint text. It completed a long implementation turn early with another preamble-shaped message:

> I’ll add Ctrl+P handling before other base shortcuts.

The visible transcript still had unfinished work: the command palette was only partially wired, the footer text had not been changed, and validation had not run.

The user's immediate follow-up was `Continue`, but that follow-up was cancelled almost immediately. Treat that cancellation as a separate event when reading `app.log`.

### `app.log` Evidence

Primary session id: `ypw8yixa4em0rg8v9hldfkl3`.

Relevant sequence:

- `23:51:08` through `00:05:08`: the primary turn executed steps 1-29, including reads, searches, plan updates, a new `src/views/command_palette.rs`, and several `src/app.rs` / `src/views/mod.rs` edits.
- `00:05:08`: `edit` call `call_64` succeeded and tool results were added.
- `00:05:08`: provider step 30 started with the same primary session.
- `00:05:10`: metadata said `assistant_message_phase=final_answer`.
- `00:05:10`: metadata said `response.completed end_turn=None`.
- `00:05:10`: AISDK logged `provider_step_finish step=30 has_tool_call=false end_turn=None last_phase=final_answer assistant_text_chars=55 action=finish preview="I’ll add Ctrl+P handling before other base shortcuts."`
- `00:05:10`: crabcode marked the stream complete:
  - `outcome=Exhausted`
  - `effective_outcome=Finished`
  - `stop_reason=Some(Finish)`

Important separation from the user's cancelled `Continue`:

- `00:05:15`: a new stream started for the same session with `input_messages=97`.
- `00:05:17`: that new stream was cancelled by the user.
- `00:05:26+`: tool calls `call_65` and later continued after cancellation, with `ui_send_failed`. These belong to the cancelled `Continue` stream, not the original premature-complete stream.

### Reference Parity Check

Codex reference behavior in `.devrefs/references/openai/codex/codex-rs/core/src/session/turn.rs` is still structurally similar to crabcode's current loop:

- A sampling request follows up when completed output includes tool work or `response.completed end_turn == Some(false)`.
- A closed stream before `response.completed` is an error.
- A non-commentary assistant message with no tool call and no `end_turn=false` is treated as a completed model turn.

opencode reference behavior is also structurally similar:

- `packages/opencode/src/session/processor.ts` drains the AI SDK `fullStream`, records tool parts, and marks the assistant message completed in cleanup.
- `packages/opencode/src/session/prompt.ts` keeps looping when the last assistant finish is `tool-calls` or when assistant parts still include unresolved non-provider-executed tool calls.
- It does not inspect assistant prose or todo/plan wording to decide whether a turn is complete.

This means a runtime guard based on text like "I'll ..." or on plan item status would diverge from both references. Requiring `end_turn=true` instead of accepting `None` would also diverge from Codex-style handling, which only treats `Some(false)` as a structured follow-up signal.

### Current Working Theory

This recurrence is not the prior "post-completion tools from the same primary stream" suspicion. The new diagnostics show the original primary stream ended cleanly at `00:05:10` on a provider step that had no tool call and no structured follow-up signal.

The recurrence is best explained as a prompt/protocol parity gap:

1. The model emitted a progress/preamble sentence as `final_answer`.
2. The provider gave `end_turn=None`, not `false`.
3. Crabcode followed the same structured completion rules as Codex/opencode and finished the turn.
4. The active crabcode Codex prompt is much weaker than the upstream Codex prompt. In particular, upstream Codex's GPT-5.2 prompt explicitly requires persistence until the task is fully handled, maintaining plan status, not leaving the plan stale, and finishing with all plan items complete or explicitly canceled/deferred before ending. Crabcode's local `src/prompt/mod.rs` only has a short "only terminate when solved" / "use final answers only when complete" version.

### Separate Cancellation Finding

The cancelled `Continue` run exposed a different issue: cancelling the relay stops UI consumption, but the underlying AISDK tool loop can still execute tools afterward. Evidence is `00:05:26+` tool calls with `ui_send_failed` after `[STREAM_CANCELLED]`.

That is not the premature-complete recurrence the user asked to ignore, but it should probably become a separate cancellation-abort bug.

### Next Debugging Targets

1. Bring `src/prompt/mod.rs` Codex prompt closer to upstream `gpt_5_2_prompt.md`, especially persistence, plan-status, and final-answer criteria.
2. Keep runtime completion gates reference-shaped: tool calls, tool results needing follow-up, `end_turn=false`, commentary phase, and terminal-event enforcement.
3. Do not add natural-language final-answer heuristics or `update_plan` completion gates unless intentionally choosing to diverge from Codex/opencode.
4. Track the cancellation issue separately: cancellation should abort `stream_with_tools` and any in-flight tool execution rather than merely closing the UI sender.

## 2026-05-22 Plan Loop Regression

### User-Visible Symptom

After the premature-completion prompt/protocol fix, crabcode was asked to add syntax highlighting during `Edited` tool calls. Instead of inspecting files, it repeatedly emitted preambles like:

> I’ll activate the plan and inspect edited-call rendering paths.

and repeatedly called `update_plan` with the same plan:

- `Locate edited tool-call rendering path` as `in_progress`
- remaining items as `pending`

The visible tool result rendered every item as unchecked, so the transcript looked like the active plan never took effect.

### `app.log` Evidence

Primary session id: `f8m29e6gfpx6rmj3ydxzdajb`.

Relevant sequence:

- `00:28:13`: stream started for the syntax-highlighting request.
- `00:28:18`: the model called `skill ratatui` once.
- `00:28:24` through `00:30:58`: steps 2-23 repeatedly called `update_plan` with the same `in_progress` item and no file-search/read/edit tools.
- `00:31:00`: user cancelled the stream.
- `00:31:07+`: the underlying tool loop still executed additional `update_plan` calls with `ui_send_failed`, matching the separate cancellation-abort issue.

### Root Cause

The previous prompt fix made active-plan state more important, but `src/tools/update_plan.rs` returned the same plain-text marker for `in_progress` and `pending`:

- `in_progress` -> `□`
- `pending` -> `□`

The model only receives the tool output text, not the UI color styling. It therefore saw its `in_progress` update echoed back as still unchecked, then tried to activate the plan again. The TUI had the same problem in transcript/plain-text captures because active plan rows differed only by color.

### Fix Applied

- `src/tools/update_plan.rs`
  - Tool output now uses distinct markers:
    - `in_progress` -> `[•]`
    - `pending` -> `[ ]`
    - `completed` -> `[x]`
  - Added `format_plan_output_preserves_in_progress_status`.
- `src/ui/components/chat.rs`
  - Plan rendering now shows `•` for active rows, so plain-text transcripts preserve active status.
  - Added `test_updated_plan_renders_in_progress_distinctly`.
- `src/prompt/mod.rs`
  - Added a planning rule that after `update_plan` succeeds, the model should proceed with concrete tool work and not repeat the same plan unless content or statuses changed.

Validation:

- `cargo test -q format_plan_output_preserves_in_progress_status`
- `cargo test -q test_updated_plan_renders_in_progress_distinctly`
- `cargo test -q codex_prompt_separates_progress_from_final_answers`

### Follow-up

The cancellation-abort issue remains separate: after `[STREAM_CANCELLED]`, `stream_with_tools` can still execute tool calls whose UI sender is already closed.

## 2026-05-22 Active Plan Premature Final Recurrence

### User-Visible Symptom

Crabcode was asked whether Codex-style `update_plan` preamble rendering was relevant and whether crabcode should support it. It found the relevant renderer path and made one partial edit, then ended the turn with another progress-update-shaped final answer:

> Now I’ll add regression coverage for the preamble case.

The task was visibly incomplete: the regression test had not been added, validation had not run, and the active plan still had unfinished items.

The partial UI/parser change from that interrupted task was removed from `src/ui/components/chat.rs` during this follow-up because it was unrelated to the premature-completion fix and had not been wired into rendering.

### `app.log` Evidence

Primary session id: `q5vx4soz1d46hnliwovqord7`.

Relevant sequence:

- `00:43:43`: the model called `update_plan` with one `in_progress` item and pending validation.
- `00:44:38`: the model updated the plan to two completed items, one `in_progress` implementation item, and one pending validation item.
- `00:45:01`: `edit` call `call_19` succeeded in `src/ui/components/chat.rs`.
- `00:45:01`: provider step 11 started after the edit result.
- `00:45:02`: metadata said `assistant_message_phase=final_answer`.
- `00:45:02`: metadata said `response.completed end_turn=None`.
- `00:45:02`: AISDK logged `provider_step_finish step=11 has_tool_call=false end_turn=None last_phase=final_answer assistant_text_chars=57 action=finish preview="Now I’ll add regression coverage for the preamble case."`
- `00:45:02`: crabcode marked the stream complete with `stop_reason=Some(Finish)`.

Unlike the earlier cancellation finding, this recurrence had no late same-stream tool execution after completion. The model simply emitted a preamble as final output and the runtime accepted it.

### Root Cause

The provider emitted a normal final-answer phase with no tool call, so crabcode finished the turn. The Codex reference loop similarly does not use `update_plan` state as a completion gate; it relies on model instructions plus structured stream/tool lifecycle signals. That means the non-parity fix is not to special-case active plan items in AISDK.

The more direct parity gap found during this follow-up was tool history fidelity. Crabcode stores tool-call arguments in the chat message JSON, but both live follow-up observations and persisted-session replay collapsed tool messages to only the tool result text. For tools like `edit`, the model could see `Replaced at line N` without seeing the original `old_string` / `new_string` it had requested.

### Superseded Runtime Fix

An `aisdk/src/response.rs` guard was briefly added to keep the turn alive when the latest `update_plan` / `todowrite` state still had `in_progress` or `pending` items. That prevented this symptom but diverged from the Codex reference loop, which does not use plan status as completion control. The guard and its regression test were removed.

### Prompt-Parity Fix Applied

- `src/prompt/mod.rs`
  - Reworked the Codex prompt toward the reference prompt shape: Personality, Autonomy and Persistence, Progress Updates and Final Answers, Planning, Task Execution, and Validation.
  - Strengthened model-facing instructions to persist through implementation, verification, and outcome reporting.
  - Kept the completion semantics in prompt/protocol space instead of runtime plan-state gating.

### Structured Tool-History Parity Fix Applied

The follow-up fix moved crabcode toward the Codex reference behavior instead of relying on flattened observation text. Codex keeps function calls and function-call outputs as structured conversation items, including call ids and arguments; crabcode now preserves that shape at the AISDK boundary and when replaying persisted tool history.

- `aisdk/src/message.rs`
  - Added structured `ToolCall` and `ToolOutput` message variants.
- `aisdk/src/response.rs`
  - Live tool execution now appends a structured tool-call message before execution and a structured tool-output message after execution.
  - OpenAI Responses tool-call accumulation now preserves the Responses `call_id` separately from the response item id, so function-call outputs correlate with the correct call id.
- `aisdk/src/providers/openai.rs`
  - Serializes structured tool history as Responses `function_call` and `function_call_output` input items.
- `aisdk/src/providers/compatible.rs`
  - Serializes structured tool history as Chat Completions assistant `tool_calls` and `tool` messages.
- `aisdk/src/providers/anthropic.rs`
  - Serializes structured tool history as Anthropic `tool_use` and `tool_result` content blocks.
- `src/llm/client.rs`
  - Persisted crabcode tool messages now replay to the model as structured tool-call plus tool-output pairs when the stored JSON has call id, name, args, and output.
  - The older text observation path remains only as a fallback for malformed or legacy tool records.
- `src/tools/update_plan.rs`
  - `update_plan` now returns Codex-style model output text: `Plan updated`.
  - The explanation and plan remain available as structured metadata for crabcode's UI.
- `src/session/compaction.rs`
  - Compaction is still text-based in crabcode, so it includes tool-call arguments explicitly to avoid losing edit/write context during summary generation.
- `src/app.rs`
  - `/copy` transcripts now include tool arguments and label tool output explicitly. This is export/UI fidelity, not agent-loop completion control.

Validation:

- `cargo fmt --check`
- `cargo test -q -p aisdk`
- `cargo test -q -p aisdk uses_responses_call_id_for_tool_output_correlation`
- `cargo test -q -p aisdk maps_responses_function_call_item_to_tool_call_shape`
- `cargo test -q -p aisdk serializes_structured_tool_history_for_responses_input`
- `cargo test -q -p aisdk tool_execution_error_is_returned_to_model_without_failing_stream`
- `cargo test -q tool_history_replays_structured_tool_call_and_output`
- `cargo test -q parse_update_plan_accepts_codex_shape`
- `cargo test -q execute_returns_codex_style_ack_with_structured_metadata`
- `cargo check`
- `cargo test -q compaction_prompt_preserves_tool_call_arguments`
- `cargo test -q -p aisdk continues_when_provider_marks_response_as_non_final`
- `cargo test -q -p aisdk`
- `cargo check`

### Follow-up

The cancellation-abort issue remains separate and still needs a dedicated fix: cancelling a stream can leave the underlying AISDK tool loop running after the UI receiver closes.

## 2026-05-25 Long-Running Turn Cost / Websocket Idle Finding

### User-Visible Symptom

While dogfooding an image-tag opener feature, the turn ran for a long time after a delayed permission approval. The user saw high token/cost usage and then a stream failure:

> websocket closed before response.completed

This was not a premature-completion recurrence. It was a long-running turn / transport recovery problem.

### `app.log` Evidence

Primary session id: `npa3foyel6u2co8n721sxtwv`.

Relevant sequence:

- `19:28:16`: the primary stream failed with `websocket closed before response.completed`.
- The failed stream had `elapsed_ms=1552634`, `response_completed=0`, and `agent_max_steps=None`.
- The last metadata included `openai_transport=responses_websocket previous_response_id=false input_items=277`, indicating a full-history websocket request rather than a compact delta.
- `19:28:23`: the follow-up stream restarted with `input_messages=156`, `messages=279`, and `previous_response_id=false input_items=278`.

### Root Cause

Two issues amplified the cost:

1. Crabcode reused cached websocket connections without considering long idle gaps between provider steps. A permission prompt or long tool execution can leave the physical websocket stale before the next request.
2. The websocket delta cache missed append-only continuations too often. Provider response message items use Responses API shapes such as `{"type":"message","role":"assistant","content":[...]}`, while crabcode's local history serializes assistant messages as `{"role":"assistant","content":"..."}`. Prefix comparison treated these equivalent assistant messages as different and fell back to sending full input. It also rejected empty deltas even though Codex allows them when `previous_response_id` is available.

### Fix Applied

- `aisdk/src/providers/openai.rs`
  - Track when a cached websocket was last successfully used.
  - Discard idle cached websocket connections before sending another request, while preserving `last_response` history so `previous_response_id` can still be used on a fresh socket.
  - If sending on a reused websocket fails, clear the cached connection, reconnect once, and resend the same request on the fresh websocket.
  - Clear cached physical websocket state on runtime close/error before `response.completed`.
  - Normalize Responses assistant message items to crabcode's local assistant message shape during prefix comparison.
  - Allow empty websocket deltas with `previous_response_id`, matching Codex's `allow_empty_delta` behavior.

### Validation

- `cargo fmt --check`
- `cargo test -p aisdk websocket`
- `cargo test -p aisdk`
- `cargo check`

### Follow-up

This does not yet add a full Codex-style sampling retry loop around partially streamed websocket failures. The next cost-control target is a bounded stream retry/fallback policy plus sane default `agent_max_steps` for normal Build turns.
