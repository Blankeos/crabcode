# Anthropic multi-tool + Kimi (direct & AI Gateway) [DONE]

Handover plan. Complements caching work in
[`ANTHROPIC_PROMPT_CACHING.md`](./ANTHROPIC_PROMPT_CACHING.md).

Last updated: 2026-08-06  
Status: **partially fixed** — Anthropic multi-tool grouping landed; gateway /
OpenAI kimi-k3 multi-tool still needs investigation (user repro).

---

## Context

Session investigation: `y138x81p0pltmvpq72b0ym4n` (and later manual repros).

Symptoms:

- Kimi-for-coding / k3 multi-tool turns → **400 Bad Request**
- Errors like:
  - `tool_call_ids did not have response messages: edit:18`
  - `unexpected tool_use_id found in tool_result blocks`
  - `tool_use ids were found without tool_result blocks immediately after`
- Auth was **already working** on `main` (PR #15 `feat/kimi-for-coding-provider`
  is **not** required for auth; optional for headers/model aliases only).

User also saw failures with **kimi-k3 via Vercel AI Gateway** using the same
multi-tool repro — so this is not only the direct Anthropic coding endpoint.

---

## Two different transports (important)

| Path | Example model | `ProviderKind` | Base | Multi-tool history shape |
|---|---|---|---|---|
| Direct Kimi coding | `kimi-for-coding/*`, k3 on coding API | **Anthropic** | `https://api.kimi.com/coding` | Anthropic `tool_use` / `tool_result` |
| Vercel AI Gateway | `vercel` / `moonshotai/kimi-k3` (commit `107ec2cc` test) | **OpenAI** | `https://ai-gateway.vercel.sh` | OpenAI `tool_calls[]` + `role: tool` |
| OpenAI-compatible | many providers | **OpenAICompatible** | varies | already groups adjacent tools |

At `107ec2cc`, gateway kimi-k3 worked in casual use because it used the
**OpenAI** client (which already grouped multi-tools). That did **not** prove
the Anthropic path was fine — and gateway multi-tool may still be broken for
other reasons (see open bugs below).

---

## Bug A — Anthropic multi-tool message shape (mostly fixed)

### Root cause

`src/aisdk/providers/anthropic.rs` used to map **each** `Message::ToolCall` /
`Message::ToolOutput` to its **own** API message:

```text
// BAD (old)
assistant: [tool_use A]
assistant: [tool_use B]
user:      [tool_result A]
user:      [tool_result B]
```

Anthropic (and Kimi coding) require:

```text
// GOOD
assistant: [tool_use A, tool_use B]
user:      [tool_result A, tool_result B]
```

OpenAI-compatible already did grouping in
`src/aisdk/providers/compatible.rs` (`openai_compatible_messages` / adjacent
flush). Anthropic did not — so **Claude multi-tool would hit the same bug**.

Evidence from `app.log`:

- Steps 1–14: single tool each → OK  
- Step 15: first **dual `edit`** → step 16 400  
- Stream IDs were real (`tool_q…`); `edit:18` is likely provider-side
  `name:index` labeling in the error text, not our synthetic ID.

### Fix landed

**Commit:** `6322075` — `fix(anthropic): group adjacent tool calls and outputs in API payload`

- Added `anthropic_messages()` that:
  - merges adjacent `ToolCall`s → one assistant content array  
  - merges adjacent `ToolOutput`s → one user content array  
  - flushes pending groups on role change / non-tool messages  
- Unit test: `groups_adjacent_tool_calls_and_results`  
- Live path: `src/aisdk/providers/anthropic.rs` (aisdk package re-exports
  `../src/aisdk` — edit the shared tree, not a dead `aisdk/src/providers` copy)

### Still verify

- [ ] Fresh session on kimi-for-coding k3 with dual-edit repro  
- [ ] Claude multi-tool (if available)  
- [ ] Session **resume** with prior multi-tool history still valid  
- [ ] Interleaved `ToolCall, ToolOutput, ToolCall, ToolOutput` (agent may
      append that way) still pairs correctly after grouping  

If resume still 400s, check history rebuild in `src/llm/client.rs`
(`tool_messages_for_model` / `append_assistant_parts` / pair flush) — IDs and
ordering must stay consistent with what we send.

---

## Bug B — Kimi-k3 via Vercel AI Gateway (open)

### What we know

- Gateway routes through **OpenAI** client + `prompt_cache_key` session sticky.
- Compatible/OpenAI path already groups adjacent tool calls into one
  `assistant.tool_calls` message.
- User still reproduced multi-tool failures on **gateway kimi-k3** with the
  same prompts as below → **not fully explained by Bug A alone**.

### Likely suspects (investigate in order)

1. **Tool call ID format / length**  
   Gateway or Moonshot may reject certain `call_id` shapes, or rewrite IDs
   between stream and history.

2. **History rebuild mismatch**  
   Stream emits id `X`; stored/replayed tool result uses id `Y` → next step
   400. Check `ToolCallAccumulator` (`item_id` vs `id` vs `call_id`) and
   session parts → LLM messages.

3. **Parallel tool calls unsupported / partial**  
   Some gateway models accept only one tool per turn; second call may be
   dropped or error oddly.

4. **`tool_choice` / strict tools / schema**  
   Gateway may require different strictness than direct API.

5. **Message role ordering after tools**  
   OpenAI requires: assistant(+tool_calls) then N× `role:tool` then next
   user/assistant. Confirm we never insert assistant text between tool
   results incorrectly.

6. **Gateway-specific headers / model id**  
   Confirm model string is exactly what gateway expects
   (`moonshotai/kimi-k3` vs aliases).

### Investigation steps

1. Reproduce on gateway with logging:
   - `provider_kind`, `base_url`, model id  
   - full request `messages` JSON on 400 (temporary debug log)  
   - every tool `call_id` on stream finish vs tool result  
2. Compare a **working single-tool** request body vs **failing dual-tool**.  
3. If possible, hit gateway with a minimal curl of the same multi-tool
   history to isolate crabcode vs upstream.  
4. Check whether OpenAI Responses API path vs Chat Completions differs for
   this model on gateway.

### Acceptance for Bug B

- [ ] Dual-tool turn succeeds on `vercel` + kimi-k3 (or documented upstream
      limitation)  
- [ ] No orphan tool_call / tool result IDs across steps  
- [ ] Error messages from gateway logged with enough body detail to debug  

---

## Repro prompts (use fresh session)

Force **one assistant turn with 2+ tools**:

```text
Create two files in /tmp/crab-repro/: a.txt and b.txt.
Write "hello a" into a.txt and "hello b" into b.txt in the same step.
Do both edits together, don't do them one after another.
```

```text
In one response: read Cargo.toml and run `ls src`. Don't wait between them.
```

```text
Create /tmp/crab-repro/x.rs and /tmp/crab-repro/y.rs with different content.
Then immediately fix both to add `// ok` at the top in the same turn.
```

### How to know

- **Broken:** `app.log` → `400` / `provider_step_error` / tool_use_id pairing  
- **Fixed:** multi-tool step completes; next step streams without 400  

Prefer **new session** so old malformed history doesn’t mask the fix.

Matrix to run:

| Provider | Model | Expected after Bug A fix |
|---|---|---|
| kimi-for-coding (Anthropic) | k3 / k2p5 | should work |
| Anthropic Claude | any tool-capable | should work |
| Vercel AI Gateway | moonshotai/kimi-k3 | **still verify** (Bug B) |

---

## Related code map

| Area | Path |
|---|---|
| Anthropic request + grouping | `src/aisdk/providers/anthropic.rs` |
| OpenAI-compatible grouping | `src/aisdk/providers/compatible.rs` |
| OpenAI client | `src/aisdk/providers/openai.rs` (if present) / client builder in `src/llm/client.rs` |
| Tool stream accumulation | `src/aisdk/response.rs` (`ToolCallAccumulator`) |
| Session → LLM messages | `src/llm/client.rs` (`tool_messages_for_model`, etc.) |
| Tool bridge IDs | `src/tools/aisdk_bridge.rs` |
| Provider kind / base URL logs | `src/llm/client.rs` stream request logs |
| Opencode reference (tools + cache) | `.devrefs/references/anomalyco/opencode/packages/llm/` |

---

## Explicit non-goals / discarded

- **PR #15** (`feat/kimi-for-coding-provider`): not needed for current auth on
  `main`. Only revisit for official model list / required headers polish.
- Treating gateway success at `107ec2cc` as proof Anthropic multi-tool was fine.
- Soft tool-output pruning (`tool_outputs_pruned`) as the root cause — it
  trims content, does not drop tool pairs.

---

## Suggested work order for next agent

1. Confirm Bug A fix on direct Anthropic + kimi-for-coding with dual-edit.  
2. If gateway still fails → capture request JSON + id pairing (Bug B).  
3. Fix Bug B (likely ID/history), add regression test if feasible.  
4. Then Anthropic prompt caching parity
   ([`ANTHROPIC_PROMPT_CACHING.md`](./ANTHROPIC_PROMPT_CACHING.md)).  

---

## Acceptance summary

- [x] Anthropic adjacent tool_use / tool_result grouping + unit test (`6322075`)  
- [ ] Live verify kimi-for-coding multi-tool  
- [ ] Live verify Claude multi-tool  
- [ ] Live verify / fix Vercel gateway kimi-k3 multi-tool  
- [ ] Optional: better 400 logging (truncated request messages + tool ids)  
