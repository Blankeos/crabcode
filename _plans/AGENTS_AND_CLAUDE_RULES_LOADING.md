# Plan: Read AGENTS.md / CLAUDE.md As Default Rules

Goal: crabcode should automatically include project rule files in the system prompt, matching OpenCode/Claude Code conventions.

## Desired Behavior (Compatibility)

### Local (project) rules
- Starting from the current working directory, traverse upward to the filesystem root.
- At each directory, look for rule files in this order:
  1. `AGENTS.md`
  2. `CLAUDE.md`
- The first match wins (exactly one local rules file is loaded).

### Global (user) rules
- Load a global rules file from `~/.config/crabcode/AGENTS.md`.
- If that does not exist, fall back to `~/.claude/CLAUDE.md`.
- The first match wins (exactly one global rules file is loaded).

### Precedence
- Local rules and global rules are independent categories.
- If both exist, include both in the system prompt (local first, then global), each with a clear source label.
- If neither exists, omit the custom instructions section entirely.

### Disabling Claude Code compatibility
- Mirror OpenCode-style env toggles for `.claude` fallbacks:
  - `CRABCODE_DISABLE_CLAUDE_CODE=1` disables all `~/.claude` usage.
  - `CRABCODE_DISABLE_CLAUDE_CODE_PROMPT=1` disables only `~/.claude/CLAUDE.md`.
- (Optional later) Also support `CRABCODE_DISABLE_CLAUDE_CODE_PROJECT=1` to disable local `CLAUDE.md` fallback while still allowing `AGENTS.md`.

## System Prompt Output Format

Insert rule content as a dedicated section so it is obvious and stable:

```text
Instructions from: /abs/path/to/AGENTS.md
<file contents>

---

Instructions from: /abs/path/to/global/AGENTS.md
<file contents>
```

Notes:
- Use absolute paths when possible.
- If canonicalization fails, print the best-effort path.
- Keep the raw markdown; do not reflow or rewrite.

## Implementation Sketch (Rust)

### 1) Add a small resolver module
- New file: `src/prompt/rules.rs` (or `src/rules.rs`).
- Responsibilities:
  - Determine local rule file by upward traversal from `working_directory`.
  - Determine global rule file by checking config directory, then `.claude` fallback (unless disabled).
  - Read file contents with size limits.
  - Return a struct describing sources.

Proposed types:
- `struct RuleFile { path: PathBuf, contents: String }`
- `struct ResolvedRules { local: Option<RuleFile>, global: Option<RuleFile> }`

### 2) Wire into the system prompt composer
- Update `src/prompt/mod.rs`:
  - Make `get_custom_instructions` async (or call into an async helper) so it can use `tokio::fs`.
  - Append rule sections after the tools context (or after env context) so they reliably influence behavior.

Likely touch points:
- `src/prompt/mod.rs`:
  - Replace `parts.push(self.get_custom_instructions());` with `parts.push(self.get_custom_instructions().await);`
  - Implement `get_custom_instructions` to call the resolver with `self.working_directory`.

### 3) Size limits and guardrails
- Enforce a max bytes limit per rule file (suggested: 64 KiB).
- If the file is larger:
  - Read only the first N bytes; append a short truncation note.
- If the file is unreadable:
  - Omit it (do not fail composing the system prompt).

### 4) Tests
- Unit tests in `src/prompt/rules.rs` (or a `tests/` integration test) using temp dirs:
  1. Local precedence: `AGENTS.md` overrides `CLAUDE.md` in same directory.
  2. Upward traversal: rule found in parent directory is used.
  3. First match wins: if child has `CLAUDE.md` and parent has `AGENTS.md`, child `CLAUDE.md` wins (because it is found first in traversal).
  4. Global precedence: `~/.config/crabcode/AGENTS.md` overrides `~/.claude/CLAUDE.md`.
  5. Disable flags: `.claude` ignored when env var set.
  6. Truncation behavior for large files.

Testability notes:
- Make the resolver accept injected “home/config dir” paths (or an override env var) to avoid writing into the real home directory during tests.

## Acceptance Criteria

- When a project has `AGENTS.md`, crabcode includes it in the system prompt automatically.
- When a project has only `CLAUDE.md`, crabcode includes it automatically.
- Local traversal and precedence match OpenCode behavior.
- Global fallback to `.claude` works and can be disabled.
- Missing/unreadable rules never crash crabcode.

## Follow-ups (Optional)

- Add `crabcode.json` support for `instructions: []` (like OpenCode) to include additional files/URLs.
- Expose a `/rules` or `/debug prompt` command to show which rule files were loaded.
