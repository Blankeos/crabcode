# Tool System + Permissions Implementation Plan

## Goal

Bring crabcode close to OpenCode basic-tool and permission behavior while fitting the current Rust architecture.

## Scope

This plan covers:

1. Agent-specific tool access (Plan vs Build vs custom agents).
2. Permission-gated execution UX for blocked tool calls.
3. Nuanced permission checks for paths, gitignored files, and sensitive files.
4. Core tool parity improvements needed to support these workflows.

This plan does **not** attempt full feature parity with every OpenCode tool in one pass.

## Current State (crabcode)

- Tools are globally registered and effectively globally available.
- Agent mode exists in UI, but does not materially change tool access.
- Permission handling is mostly hard-coded guardrails inside individual tools.
- No generic "ask user and resume" permission workflow in the execution pipeline.
- `glob`/`list` do not use gitignore-aware file discovery.
- Sensitive reads (like `.env`) are not centrally permission-managed.

## Target State

- Tool availability is resolved from active agent policy (Plan/Build/custom).
- Tool execution passes through a centralized permission engine.
- Blocked actions become interactive permission requests (deny, allow once, allow always).
- Permission requests trigger existing sound/notification hooks.
- File discovery and path checks are gitignore-aware and external-directory-aware.
- Sensitive path patterns (especially `.env*`) are permission-gated for reads and writes.

## Parity Matrix (high-level)

1. **Tool filtering by agent**
   - Current: static registry.
   - Target: runtime filtering based on active agent + configured permissions.
2. **Permission engine**
   - Current: ad-hoc checks in tool implementations.
   - Target: shared rule evaluator with `allow | ask | deny` and pattern matching.
3. **Permission prompt lifecycle**
   - Current: missing.
   - Target: request queue + UI prompt + decision persistence.
4. **External directory access**
   - Current: inconsistent.
   - Target: centralized check requiring permission for outside-workdir access.
5. **Gitignore-aware operations**
   - Current: weak coverage.
   - Target: default ignore behavior for discovery tools and guarded writes.
6. **Sensitive file policy (`.env*`)**
   - Current: limited write-only hard block.
   - Target: read/write permission policy with ask/deny defaults.

## Proposed Architecture

### 1) Permission Domain Model

Add a `permission` module with:

- `PermissionDecision`: `Allow`, `Ask`, `Deny`.
- `PermissionRule`: pattern + decision + optional tool scope.
- `PermissionRequest`: tool name, action type, target path/command metadata, reason text.
- `PermissionResponse`: `Deny`, `AllowOnce`, `AllowAlways`.

Add pattern matching rules with:

- last-match-wins,
- wildcard support for tool and path patterns,
- separate defaults per action category (read/write/exec/network if needed later).

### 2) Execution Interceptor

Introduce a centralized preflight in the tool execution path (before tool handler runs):

1. Build execution context (active agent, tool name, arguments metadata).
2. Resolve tool availability from agent policy.
3. Run permission evaluation:
   - if `Allow`: execute immediately,
   - if `Deny`: return denied error,
   - if `Ask`: emit permission request and suspend execution.
4. Resume execution only after explicit UI response.

This should replace scattered one-off permission checks where feasible.

### 3) Session Permission State

Maintain session-scoped permission state:

- pending request queue (at most one active prompt in UI),
- once-grants keyed to request signature,
- always-grants persisted in config/runtime policy store,
- rejection tracking for repeat behavior.

### 4) Agent Tool Policy Layer

Define agent policy in config/runtime:

- `plan`: restricted tools (no mutating filesystem by default, no bash by default unless explicitly enabled).
- `build`: full engineering toolset with permission checks.
- `custom`: explicit allow/deny lists inherited from base defaults.

Effective tool list = `registered tools` intersect `agent allowed tools` intersect `permission-enabled tools`.

### 5) File Safety and Path Policy

Create shared path-policy helpers:

- `is_outside_workdir(path)`,
- `is_gitignored(path)` (via a gitignore-aware matcher),
- `is_sensitive_env_path(path)` for `.env*` and related secrets.

Use these in read/write/edit/glob/list/grep style tools.

## Implementation Phases

## Phase 0: Baseline and Safety

1. Add integration tests that capture current behavior for read/write/glob/list/bash.
2. Add snapshot tests for active tool list by mode (Plan/Build).
3. Add fixtures for gitignored files and external directory targets.

Deliverable: failing tests for target behavior, safety net for refactors.

## Phase 1: Permission Engine Foundation

1. Implement permission types, matcher, and evaluator with `allow|ask|deny`.
2. Add config parsing and in-memory policy defaults.
3. Add unit tests for wildcard matching, precedence, and default fallbacks.

Deliverable: reusable evaluator independent from tool implementations.

## Phase 2: Execution Pipeline Integration

1. Add preflight interceptor in tool execution pipeline.
2. Convert tool permission failures to centralized permission requests.
3. Implement suspend/resume flow for tool calls waiting on user decision.
4. Keep backward-compatible errors until UI flow is fully wired.

Deliverable: tools route through centralized permission logic.

## Phase 3: UI Permission Prompt + Sound

1. Add UI state for pending permission request and decision actions.
2. Present clear prompt with: tool, target, reason, risk hint.
3. Add actions: `Deny`, `Allow Once`, `Allow Always`.
4. Trigger permission sound and notification hooks on new prompt.
5. Ensure decision feeds back to session processor and resumes execution.

Deliverable: end-to-end permission request UX.

## Phase 4: Agent-Specific Tool Access

1. Define Plan/Build default tool policies.
2. Add custom agent policy schema support.
3. Apply policy when exposing tool schemas to the model.
4. Add tests ensuring unavailable tools are not callable per active agent.

Deliverable: active mode materially controls tool access.

## Phase 5: Path + Gitignore + Sensitive File Rules

1. Implement centralized external-directory checks for fs tools.
2. Make glob/list (and grep when added) gitignore-aware by default.
3. Add write/edit ask behavior for gitignored targets.
4. Add read/write ask-or-deny defaults for `.env*` and similar secrets.
5. Replace legacy hard-coded `.env` write block with policy-based handling.

Deliverable: nuanced, predictable path safety behavior.

## Phase 6: Basic Tool Parity Additions

1. Add `grep` tool (regex content search) with permission preflight.
2. Revisit `glob` and `list` behavior to align with documented semantics.
3. Ensure tool argument schemas and guidance match runtime behavior.

Deliverable: stronger basic-tool parity baseline.

## Validation Checklist

- Plan agent cannot access disallowed mutating tools.
- Build agent can access full configured set, still permission-gated.
- Read outside workspace triggers ask flow and respects user decision.
- Write outside workspace triggers ask flow and can be denied.
- Writes to gitignored paths trigger ask flow.
- Discovery tools do not leak ignored files by default.
- Reading `.env` requires explicit permission.
- Permission prompt plays sound and appears in UI with actionable choices.
- `Allow Once` applies only to matching request.
- `Allow Always` persists and suppresses repeated prompts.
- Deny returns clear error to model and user transcript.

## Test Strategy

1. **Unit tests**
   - permission matcher precedence and wildcard behavior,
   - path classification helpers (external/gitignored/sensitive).
2. **Integration tests**
   - tool call preflight and blocked/resume flow,
   - agent-mode tool exposure and invocation constraints.
3. **UI tests**
   - permission prompt render and action dispatch,
   - sound/notification trigger on permission request.
4. **Regression tests**
   - existing safe bash checks still enforced,
   - existing tool outputs remain stable where semantics unchanged.

## Rollout Notes

- Ship behind a feature flag for initial validation.
- Log permission decisions during beta to tune defaults.
- Document final config behavior in `_docs/config.mdx` once implementation lands.

## Recommended Build Order

1. Phase 1 and Phase 2 (permission core + interceptor).
2. Phase 4 (agent tool gating) so model behavior aligns early.
3. Phase 3 (UI prompt) to unlock interactive approvals.
4. Phase 5 (path/gitignore/sensitive policy hardening).
5. Phase 6 (extra tool parity work).
