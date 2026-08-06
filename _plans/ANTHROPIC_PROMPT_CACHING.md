# Anthropic prompt caching (usage optimization)

Handover plan. Complements multi-tool / provider work in
[`ANTHROPIC_MULTI_TOOL_AND_KIMI_GATEWAY.md`](./ANTHROPIC_MULTI_TOOL_AND_KIMI_GATEWAY.md).

Last updated: 2026-08-06  
Status: **partial today** — only last system block is marked; not opencode-parity.

---

## Why this matters

On Anthropic-path models (Claude, Kimi coding / `api.kimi.com/coding`, etc.),
multi-step tool loops re-send:

1. system prompt  
2. full tool schemas  
3. conversation prefix (through the latest user message)

Without explicit `cache_control` breakpoints, most of that is billed as full
input every step. With caching, later steps in the same turn mostly hit
**cache reads** (~0.1× input on Anthropic pricing).

OpenAI / gateway paths use *implicit* prefix caching + optional
`prompt_cache_key`. Anthropic needs **explicit** breakpoints.

---

## What crabcode does today

**File:** `src/aisdk/providers/anthropic.rs` (`stream_text`)

| Breakpoint | Status |
|---|---|
| Last **system** text block → `cache_control: { type: "ephemeral" }` | ✅ done |
| Last **tool** definition | ❌ missing |
| Latest **user** message (text or tool_result group) | ❌ missing |
| Cap at **4** breakpoints (Anthropic API limit) | n/a (only 1 emitted) |
| Parse stream usage `cache_read_input_tokens` / `cache_creation_*` | ❌ missing |
| Session sticky key (`prompt_cache_key`) | OpenAI only (`src/llm/client.rs`) — N/A for Anthropic |

Current system-only snippet:

```rust
// Anthropic prompt caching: mark the last system block ephemeral so the
// stable prefix can be reused across tool steps in a session.
if let Some(last) = system_prompts.last_mut() {
    // insert cache_control ephemeral
}
```

Tools are serialized **without** `cache_control`:

```rust
serde_json::json!({
    "name": t.name,
    "description": t.description,
    "input_schema": schema,
})
```

---

## Reference: opencode (anomalyco)

**Path:** `.devrefs/references/anomalyco/opencode/packages/llm/`

### Default policy (`cache-policy.ts`)

Default `"auto"` (also used when policy is `undefined`):

```ts
{
  tools: true,                      // last tool definition
  system: true,                     // last system part
  messages: "latest-user-message",  // last user message content part
}
```

Rationale (from their comments): in a tool-use loop the **latest user message
stays put** while one user turn explodes into many assistant/tool round-trips.
Caching at that boundary makes every **intra-turn** API call hit the prefix.

Resolution:

- `undefined` / `"auto"` → tools + system + latest user  
- `"none"` → no auto placement (manual hints still flow)  
- object form → exact caller config  

Protocols that respect inline hints: `anthropic-messages`, `bedrock-converse`.  
OpenAI/Gemini skip the policy pass (implicit caching).

### Anthropic lowering (`protocols/anthropic-messages.ts`)

- Max **4** `cache_control` breakpoints per request (tools + system + messages).
  Exceeding → API **400**; they count and **silently drop** extras.
- `ephemeral` default (5m); optional `ttl: "1h"`.
- Applied on:
  - last tool via `tool.cache`
  - system / message content parts via `part.cache`
- Usage tracking: `cacheReadInputTokens` / cache write fields projected into
  session stats.

---

## Target for crabcode (parity with opencode auto)

Implement the same three breakpoints on the Anthropic request body:

1. **Last tool** in `body["tools"]` → `cache_control: { "type": "ephemeral" }`
2. **Last system** block (already done) — keep
3. **Latest user-role message’s last content block** after `anthropic_messages()`
   — covers normal user text *and* multi-`tool_result` user messages

Constraints:

- Never emit more than **4** breakpoints.
- Prefer order: tools → system → messages (matches stable-prefix-first).
- Do **not** put breakpoints on every tool/message (wastes cap + write cost).
- Optional later: `ttl: "1h"` behind config if provider supports it.

### Usage visibility (recommended same PR or follow-up)

Parse Anthropic stream usage events for:

- `cache_read_input_tokens`
- `cache_creation_input_tokens` (and/or ephemeral 5m/1h variants)

Surface in logs + session token accounting so we can verify hits.

---

## Implementation sketch

**Primary file:** `src/aisdk/providers/anthropic.rs`

1. After building `tool_params`, if non-empty, set `cache_control` on the
   **last** tool object.
2. Keep system last-block mark.
3. After `anthropic_messages(messages)`, find the **last** message with
   `role == "user"`, and mark its **last** content block (object form) with
   `cache_control`. If content is a plain string, wrap as
   `[{ type: "text", text, cache_control }]`.
4. Helper: `fn apply_cache_control(value: &mut Value)` + counter ≤ 4.
5. Unit tests:
   - system-only → 1 breakpoint  
   - system + tools → last tool + last system  
   - system + tools + multi-step history → + latest user  
   - never exceeds 4  

**Secondary (optional):**

- `src/llm/client.rs` / response usage path: map cache read/write into
  existing token stats.
- Config knobs later (`cache: auto | none`) — not required for v1.

**Do not** confuse with OpenAI `prompt_cache_key` — different mechanism.

---

## Provider caveats

| Provider | Notes |
|---|---|
| Anthropic Claude | Full support; min token thresholds apply for cache writes |
| Kimi coding (`api.kimi.com/coding`) | Anthropic-shaped API; confirm they honor `cache_control` |
| Vercel AI Gateway → kimi | OpenAI path — implicit / gateway caching, not this plan |
| Bedrock | Separate protocol; opencode has `bedrock-cache.ts` — out of scope |

If Kimi ignores `cache_control`, emitting it should still be harmless (or
strip via provider capability flag if they 400).

---

## Repro / verify

1. Anthropic or kimi-for-coding session with a large system prompt + tools.
2. Multi-step tool loop (3+ steps after one user message).
3. Compare step 2+ usage:
   - **Before:** high input tokens every step  
   - **After:** `cache_read_input_tokens` grows; uncached input stays small  
4. Unit tests for breakpoint placement (no live API required).

---

## Acceptance criteria

- [ ] Last tool + last system + latest user get `cache_control: ephemeral`
- [ ] ≤ 4 breakpoints always
- [ ] Unit tests cover placement + cap
- [ ] (Nice) cache read/write tokens logged and/or stored per step
- [ ] No regression on non-Anthropic providers

---

## Out of scope (for this plan)

- OpenAI / xAI `prompt_cache_key` improvements  
- Bedrock cache TTL mapping  
- Manual per-part cache hints in the public API  
- Changing system prompt content itself  
