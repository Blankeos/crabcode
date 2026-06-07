# OpenAI ChatGPT Plus/Pro OAuth Plan (Codex)

## Short answer

Yes, this is possible.

But for ChatGPT Plus/Pro specifically (using Codex via ChatGPT subscription), this is **not** the same as normal OpenAI API-key auth. It relies on ChatGPT OAuth tokens and ChatGPT backend endpoints, so we need a small transport/auth layer beyond the current API-key-only flow.

## What I verified

### Current crabcode behavior

- `auth.json` persistence exists and already supports `type: "api"` and `type: "oauth"` in `src/persistence/auth.rs`.
- `/connect` currently always routes provider selection to API key entry in `src/app.rs` + `src/ui/components/api_key_input.rs`.
- LLM calls are built from `stream_llm_with_cancellation()` in `src/llm/client.rs` and currently assume API-key style provider setup.

### How opencode does OpenAI/Codex auth

From `_dev_reference1/packages/opencode/src/plugin/codex.ts`:

- Three methods for `openai`:
  1. `ChatGPT Pro/Plus (browser)` (OAuth + PKCE + local callback)
  2. `ChatGPT Pro/Plus (headless)` (device auth + polling)
  3. `Manually enter API Key`
- OAuth issuer and token exchange endpoints:
  - `https://auth.openai.com/oauth/authorize`
  - `https://auth.openai.com/oauth/token`
- Codex request target is rewritten to:
  - `https://chatgpt.com/backend-api/codex/responses`
- It sets `Authorization: Bearer <oauth_access_token>` and, when present, `ChatGPT-Account-Id`.
- It stores extra OAuth data (not just refresh/access/expires), including optional `accountId`.

### aisdk-rs fork capability check

From `/Users/carlo/Desktop/Projects/aisdk-rs`:

- OpenAI provider currently has:
  - fixed request path (`/v1/responses`)
  - fixed header construction (Content-Type + Authorization)
  - no built-in request interceptor/custom fetch hook
  - no generic extra header injection on OpenAI provider settings
- OpenAI builder also enforces non-empty API key.

Conclusion: **aisdk-rs in current form is not enough for full ChatGPT Plus/Pro Codex transport behavior** without extending it (or bypassing it for this provider).

---

## Scope for this implementation

### In scope (now)

- OpenAI-only OAuth support in crabcode.
- `/connect` method selection for OpenAI:
  1. ChatGPT Plus/Pro (browser)
  2. ChatGPT Plus/Pro (headless)
  3. Manually enter API key (existing path)
- Use OAuth tokens for Codex completions.

### Out of scope (for now)

- OAuth for other providers.
- Full plugin framework like opencode.
- Multi-provider OAuth abstractions beyond what OpenAI needs.

---

## Proposed architecture

## 1) Auth data model updates (compat with opencode format)

### File

- `src/persistence/auth.rs`

### Changes

- Extend OAuth variant to include optional fields used by OpenAI OAuth:
  - `accountId` (serde rename from `account_id`)
  - optional `enterpriseUrl` (future-safe; can be ignored in logic)
- Keep existing `refresh`, `access`, `expires` unchanged.

### Why

- You said auth.json uses opencode-compatible shape.
- `ChatGPT-Account-Id` header should be set when available.

## 2) New OpenAI OAuth service module

### New module

- `src/auth/openai_oauth.rs` (or `src/llm/openai_oauth.rs` if you want to keep auth+transport together)

### Responsibilities

- Build browser OAuth authorize URL (PKCE + state).
- Run local callback server for browser flow (localhost callback).
- Implement headless/device flow:
  - request user code
  - poll token readiness
  - exchange code for tokens
- Parse JWT claims to extract optional `chatgpt_account_id`.
- Refresh access token when expired.
- Return normalized auth payload ready to persist into `auth.json`.

### Notes

- Reuse opencode-known constants/endpoints for compatibility.
- All network calls via `reqwest`.

## 3) Connect UX changes (OpenAI method picker)

### Existing behavior to keep

- Non-openai providers can continue using current API key flow.

### New behavior for openai

- After selecting `openai` in `/connect`, show a second step for method selection:
  1. ChatGPT Plus/Pro (browser)
  2. ChatGPT Plus/Pro (headless)
  3. Manually enter API key
- If method 3 selected: show existing API key input.
- If method 1/2 selected: show OAuth progress/status overlay and finalize into OAuth auth config.

### Files likely touched

- `src/app.rs`
- `src/views/connect_dialog.rs`
- maybe a new lightweight overlay component for OAuth status/code display

## 4) LLM transport integration for OAuth Codex

### Key requirement

When provider is `openai` and auth type is `oauth`, requests must go to ChatGPT Codex endpoint with OAuth bearer token (+ optional account header), not standard API-key flow.

### Recommended implementation path

#### A. Extend aisdk-rs fork (recommended)

Add to OpenAI provider settings in aisdk-rs:

- customizable response path (default `/v1/responses`)
- additional headers map

Then crabcode can configure:

- base URL: `https://chatgpt.com/backend-api/codex`
- response path: `responses`
- auth token: OAuth access token
- extra headers:
  - `ChatGPT-Account-Id` when present
  - optional `originator` / user-agent parity headers if required

This keeps crabcode on one streaming stack and avoids a separate custom SSE client for OpenAI OAuth.

#### B. Fallback path (if you avoid aisdk-rs patch)

Implement a dedicated reqwest SSE transport in crabcode just for OpenAI OAuth Codex and map events into existing `ChunkMessage` flow.

This works, but increases maintenance and duplicates provider logic already centralized in aisdk-rs.

## 5) Token refresh strategy

- On every OpenAI OAuth request, check expiry.
- If expired (or near expiry), refresh first and persist updated token.
- If refresh fails:
  - surface clear toast/actionable error
  - keep auth entry but mark as needing re-auth in UX messaging

## 6) Model availability strategy

For OpenAI + OAuth auth type:

- Prefer showing a codex-focused allowlist (as opencode does) to reduce unsupported model failures.
- Initial allowlist can include:
  - `gpt-5.3-codex`
  - `gpt-5.2-codex`
  - `gpt-5.1-codex`
  - `gpt-5.1-codex-mini`
  - `gpt-5.1-codex-max`
  - `codex-mini-latest`
- Keep API-key OpenAI auth path unchanged (full OpenAI model list).

---

## Implementation phases

## Phase 0 - Foundation and compatibility

1. Extend `AuthConfig::OAuth` with optional `accountId`/`enterpriseUrl` fields.
2. Add serde tests to confirm opencode-compatible roundtrip JSON.

## Phase 1 - OAuth engine

1. Build OpenAI OAuth service module for browser flow.
2. Add headless/device flow.
3. Add refresh-token support.
4. Add JWT claim parsing helper for account id extraction.

## Phase 2 - Connect UX

1. Add openai method-selection step in `/connect` flow.
2. Wire method actions:
   - browser OAuth
   - headless OAuth
   - manual API key (existing)
3. Add cancellation + timeout handling in UI state.

## Phase 3 - Inference transport

1. Implement recommended aisdk-rs extension (path override + extra headers), then bump/update usage.
2. In crabcode stream path, branch OpenAI auth mode:
   - API key: standard OpenAI API
   - OAuth: Codex endpoint + OAuth headers
3. Add base URL fallback for OpenAI when models.dev `api` is empty (`https://api.openai.com`).

## Phase 4 - Model filtering and polish

1. Apply codex-focused model filtering when auth type is OpenAI OAuth.
2. Improve known error mapping (e.g., `usage_not_included`) to user-friendly guidance.
3. Ensure `/models` and model dialog behavior remain coherent across API vs OAuth auth types.

## Phase 5 - Tests and verification

1. Unit tests:
   - OAuth URL + PKCE/state generation
   - JWT account id extraction
   - auth.json serde compatibility
   - token refresh behavior
2. Integration-style tests:
   - `/connect` openai -> method select -> persisted auth
   - OAuth auth path can stream with OpenAI provider branch
3. Manual smoke checks:
   - browser flow on macOS
   - headless flow on terminal-only path
   - fallback to manual API key

---

## Risks and mitigations

- Private/undocumented endpoints may change:
  - Mitigation: isolate constants + transport logic in one module, clear errors, easy hotfix points.
- OAuth token refresh failures:
  - Mitigation: proactive refresh + explicit reconnect flow + preserved auth state.
- Divergence from aisdk-rs upstream:
  - Mitigation: keep patch minimal and generic (path/header extensibility useful beyond this feature).

---

## Acceptance criteria

- `/connect` for `openai` offers exactly three methods (browser OAuth, headless OAuth, manual API key).
- OAuth success writes opencode-compatible `auth.json` entry with `type: "oauth"` and token fields.
- OpenAI OAuth path can request Codex completions without API key.
- Manual API key path for OpenAI still works as before.
- Non-openai provider behavior remains unchanged.

---

## Final feasibility verdict

This feature is feasible and a good fit for crabcode.

The only notable blocker is that current aisdk-rs OpenAI transport is too rigid for ChatGPT Plus/Pro Codex endpoint requirements. Once we add minimal path/header configurability (or implement a temporary custom transport), the rest is straightforward engineering work in auth flow + connect UX.

---

## Runtime loop issue (tool-call step stops early)

### Symptom (current user-reported issue)

- Codex OAuth path can execute one tool call, then frequently ends the stream before the follow-up model step.
- Repro pattern: assistant emits short preamble text + one tool call, tool runs, then turn ends without the next assistant/tool step.

### Brainstorm and likely root cause

- In `aisdk-rs` `stream_text()` orchestration, step termination currently marks `StopReason::Finish` as soon as any `Done(Text|Reasoning)` output appears.
- Codex can return mixed outputs in a single completed response (for example: an output text message plus one or more function calls).
- If text is seen first, the loop marks the step finished even when tool calls are also present in that same step payload.
- This explains intermittency: it depends on output composition/order from the backend for that turn.

### Fix plan

1. Update `aisdk-rs` step-finalization logic to decide finish based on the whole step, not the first non-tool output seen.
2. Continue looping when a step contains any tool call, even if text/reasoning is also present.
3. Only set `StopReason::Finish` when a step has terminal assistant content and no tool calls.
4. Apply the same mixed-output guard to non-streaming `generate_text()` for consistency.
5. Add regression tests for mixed output (`Text + ToolCall`) to prevent future regressions.

### Validation

- `cargo test` targeted for new mixed-output tests in `aisdk-rs`.
- `cargo check --features openai` in `aisdk-rs`.
- `cargo check` in `crabcode`.
- Manual smoke: ask for a task that typically needs `glob -> read -> summarize` and verify multi-step tool loop no longer stops after first call.
