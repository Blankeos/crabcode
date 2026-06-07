# Configuration Feature Plan

Goal: Add a layered configuration system for Crabcode that is (1) compatible with OpenCode configs, (2) supports both global + per-project config, and (3) can be extended incrementally. For the first implementation pass, only `theme`, `sounds`, and `model` are functional. `sounds.<event>.notify` extends `sounds` with native desktop notifications (default off per event); other supported keys are parsed/merged but treated as unimplemented.

## Non-Goals (Initial Scope)

- Implementing the behavior of OpenCode features we explicitly do not support (keybinds, theme selection via OpenCode config, custom tools, share, tui, server, plugin).
- Remote config (OpenCode `.well-known/opencode`) and OpenCode env overrides (`OPENCODE_CONFIG`, `OPENCODE_CONFIG_CONTENT`). These can be added later.

## Sources + Precedence

We load up to four JSON/JSONC files and deep-merge them with increasing priority:

1. OpenCode global (lowest priority)
2. Crabcode global
3. OpenCode local
4. Crabcode local (highest priority)

This is the inverse of how we describe merge application (base -> overrides). In code, we typically load in base-first order and apply overrides after.

### Global Files

Global config can live in either the app directory (preferred) or directly under the config home.

Notes:

- Prefer the XDG path resolution: use `$XDG_CONFIG_HOME` if set, else `~/.config`.
- Each layer (OpenCode global, Crabcode global) must resolve to at most one file. If multiple candidates exist for the same layer, Crabcode errors and tells the user to keep only one.

OpenCode global candidates (zero or one must exist):

- `$XDG_CONFIG_HOME/opencode/opencode.jsonc`
- `$XDG_CONFIG_HOME/opencode/opencode.json`
- `$XDG_CONFIG_HOME/opencode.jsonc`
- `$XDG_CONFIG_HOME/opencode.json`

Crabcode global candidates (zero or one must exist):

- `$XDG_CONFIG_HOME/crabcode/crabcode.jsonc`
- `$XDG_CONFIG_HOME/crabcode/crabcode.json`
- `$XDG_CONFIG_HOME/crabcode.jsonc`
- `$XDG_CONFIG_HOME/crabcode.json`

### Local (Per-Project) Files

We treat “local” as “nearest project root” (see discovery algorithm below).

As with global configs, each layer (OpenCode local, Crabcode local) must resolve to at most one file; multiple candidates for the same layer is an error.

OpenCode local candidates (zero or one must exist):

- `<project-root>/.opencode/opencode.jsonc`
- `<project-root>/.opencode/opencode.json`
- `<project-root>/opencode.jsonc`
- `<project-root>/opencode.json`

Crabcode local candidates (zero or one must exist):

- `<project-root>/.crabcode/crabcode.jsonc`
- `<project-root>/.crabcode/crabcode.json`
- `<project-root>/.opencode/crabcode.jsonc`
- `<project-root>/.opencode/crabcode.json`
- `<project-root>/crabcode.jsonc`
- `<project-root>/crabcode.json`

Rationale:

- This supports existing OpenCode users without forcing duplicated config.
- Supporting `.opencode/crabcode.json(c)` allows teams to keep config near existing OpenCode structure while adopting Crabcode-specific keys.

### Project Root Discovery

Algorithm:

- Start at current working directory.
- Walk upward until:
  - A `.git` directory is found (treat that directory as project root), or
  - The filesystem root is reached.
- If no `.git` is found, treat the current working directory as project root.

This matches OpenCode’s “traverse up to nearest Git directory” behavior, but scoped to our use.

## File Format + Parsing

We support JSON and JSONC:

- `.json` is strict JSON.
- `.jsonc` allows comments and trailing commas.

Implementation approach (Rust):

- Parse each config file into a `serde_json::Value` (not a strongly-typed struct).
- Use a JSONC-capable parser for `.jsonc` (recommended: `json5` crate; it handles comments + trailing commas).
- Keep track of the file path and source label for diagnostics.

## Deep Merge Semantics

We need predictable, “graceful” merges.

Recommended merge behavior:

- Object + object: recursively merge keys.
- Array + array: override entire array with higher-priority value.
- Primitive (string/number/bool) or type mismatch: higher-priority value replaces lower.
- `null`: treat as “unset” (removes the key from the merged result) rather than a literal null.

Rationale for `null` as unset: it provides an escape hatch to disable values from lower layers (useful when the global config is shared).

## Variable Substitution

Support OpenCode-style placeholders inside string values:

- `{env:VAR_NAME}` -> environment variable value, or empty string if unset.
- `{file:path}` -> file contents (trim trailing newlines).

Path rules for `{file:...}`:

- `~` expands to home directory.
- Relative paths resolve relative to the config file’s directory.
- Absolute paths are allowed.

Processing rules:

- Apply substitution after all config sources are merged (so placeholders in the winning value get resolved).
- Traverse the merged `serde_json::Value` recursively and only substitute within string leaves.
- Support multiple placeholders within the same string.
- If a `{file:...}` read fails, replace with empty string and record a warning diagnostic.

## Compatibility Strategy (OpenCode + Crabcode)

We load both OpenCode and Crabcode sources, but we do not implement all OpenCode keys.

### Keys We Intend to Parse/Merge (OpenCode-Compatible)

These keys should be accepted from OpenCode config files and merged (even if unimplemented at runtime initially):

- `agent`
- `instructions`
- `tools` (tool enable/disable map)
- `mcp`
- `model` (default model)
- `provider` (providers outside models.dev)
- `command`
- `permission`
- `compaction`
- `watcher`
- `default_agent`
- `formatter`
- `disabled_providers`
- `enabled_providers`

If we later expand the compatibility set, we do it by:

- Adding parsing/normalization for the new key into our internal config representation.
- Implementing the behavior in the relevant subsystem.

### Keys We Explicitly Ignore From OpenCode

We ignore these keys when they appear in OpenCode configs:

- `keybinds`
- `theme` (Crabcode does not read theme selection from OpenCode config)
- `custom tools` (in OpenCode schema: `tool` / `tools` are not the same as “custom tools”; we ignore the custom-tool feature)
- `share`
- `tui`
- `server`
- `plugin`

We should still allow these keys to exist (no parse error); we just exclude them from the merged config we act upon.

### Crabcode-Specific Additions

Crabcode config supports everything in the compatibility set above, plus:

- `sounds` (Crabcode-only)
- `sounds.<event>.notify` (Crabcode-only desktop notifications, default off per event)
- `theme` (Crabcode controls the theme selection, but the theme system is compatible with OpenCode)

If these appear in OpenCode config files, they are ignored.

## Crabcode Config Schema (Initial)

Minimal schema we actively apply in the first iteration:

```jsonc
{
  "$schema": "https://crabcode.ai/config.json", // future

  // Crabcode-only theme values
  "theme": "default",

  // OpenCode-compatible
  "model": "openai/gpt-5.2",

  // Crabcode-only (All are optional to use, but these are the defaults)
  "sounds": {
    "error": { "file": "/absolute/path.wav", "enabled": false, "notify": false },
    "complete": { "file": "/absolute/path.wav", "enabled": true, "notify": true },
    "permission": { "file": "/absolute/path.wav", "enabled": false, "notify": false },
    "question": { "file": "/absolute/path.wav", "enabled": false, "notify": false },
  },
}
```

Sounds requirements:

- Sound event keys (`error`, `complete`, `permission`, `question`) should accept either:
  - Object form: `{ "enabled": bool, "file": "/absolute/path.wav" }`
  - Boolean shorthand: `true`/`false` (e.g. `"complete": true`)
- `file` must be an absolute path (no `~`, no relative). If invalid, record a warning and treat sound as disabled.
- `enabled` default behavior:
  - If not specified: default to `false` except `complete` default to `true` (per requirement).
- `notify` is an optional boolean under each event object with default `false`.

### Desktop Notification Delivery (`sounds.<event>.notify`)

When `sounds.<event>.notify` is `true`, Crabcode should emit a native desktop notification for that event.

Cross-platform backend plan:

- macOS: use Notification Center via `osascript` (`display notification ...`).
- Linux: use `notify-send` (libnotify); if unavailable, log a warning and continue.
- Windows: use a PowerShell/WinRT toast invocation; if unavailable, log a warning and continue.

Behavioral rules:

- Fire exactly once per completed assistant response (not per chunk).
- Notification delivery must be best-effort and non-blocking (spawn background process/task).
- For completion notifications, include concise runtime stats when available (for example `1.0s | 30t/s`).
- `sounds.<event>.enabled` and `sounds.<event>.notify` are independent toggles:
  - `complete: { enabled: false, notify: true }` => silent audio + desktop notification.
  - `complete: { enabled: true, notify: false }` => sound only.
- If notification permission is denied by the OS, do not fail app startup or streaming.

## .opencode Directory Structure Compatibility

We support discovering config additions from `.opencode/` (and global `~/.config/opencode/`) similar to OpenCode:

- Agents:
  - `.opencode/agents/*.md`
  - `.opencode/agent/*.md` (back-compat)
- Skills:
  - `.opencode/skills/**`
  - `.opencode/skill/**` (back-compat)

Initial behavior:

- Discover these files/directories and record them in a “config inventory” for future use.
- Do not change runtime behavior yet (unimplemented), but surface them as diagnostics so users know they were found.

Later behavior (future phases):

- Parse agent markdown frontmatter or content per OpenCode docs and integrate into agent registry.
- Load skills from discovered skill folders.

## Theme Support (OpenCode-Compatible)

Crabcode supports OpenCode's theme JSON format and reads theme definitions from the same `themes/` folders OpenCode uses (https://opencode.ai/docs/themes/#custom-themes).

### Theme Selection Rules

- `theme` is read from Crabcode config files only (e.g. `~/.config/crabcode/crabcode.json(c)` and `.crabcode/crabcode.json(c)` and `.opencode/crabcode.json(c)`).
- If `theme` is present only in OpenCode config files, Crabcode ignores it.

Theme value format:

- `theme` is a theme ID (string), not a path.
- The ID is resolved by searching theme definitions in both OpenCode and Crabcode theme folders.

### Theme Discovery (Built-in + Custom)

Load themes with higher priority overriding lower when the same theme name exists in multiple locations.

Recommended combined hierarchy:

1. Built-in themes (embedded or shipped with the binary)
2. OpenCode user themes: `$XDG_CONFIG_HOME/opencode/themes/*.json`
3. Crabcode user themes: `$XDG_CONFIG_HOME/crabcode/themes/*.json`
4. OpenCode project themes: `<project-root>/.opencode/themes/*.json`
5. Crabcode project themes: `<project-root>/.crabcode/themes/*.json`
6. Current working directory themes: `./.opencode/themes/*.json` (if different from project root)

This preserves OpenCode's precedence while allowing Crabcode-native theme folders.

## Decisions Locked In (From Discussion)

- Theme selection: theme ID only; resolve from `themes/` folders in both OpenCode and Crabcode locations.
- Support global "flat" configs (e.g. `$XDG_CONFIG_HOME/opencode.json(c)`, `$XDG_CONFIG_HOME/crabcode.json(c)`) in addition to app directories.
- Support project-root configs (e.g. `<project-root>/opencode.json(c)`, `<project-root>/crabcode.json(c)`) in addition to dot-directories.
- Only one config file per layer (OpenCode global, Crabcode global, OpenCode local, Crabcode local); multiple candidates for the same layer is an error.
- `null` means unset during merge.
- No `CRABCODE_CONFIG` env override for now.

## Diagnostics and “Unimplemented” Reporting

We want it to “merge gracefully without issues” and also make it obvious what is currently unused.

Proposed diagnostic design:

- Collect warnings during load/merge/resolve:
  - Parse errors per file (non-fatal; skip file).
  - `{file:...}` read failures.
  - Invalid `sounds.*.file` (non-absolute).
  - Notification backend unavailable when any `sounds.<event>.notify=true` (e.g., missing `notify-send`).
  - Unknown keys (only if they look like they were intended, optional).

- Collect “unimplemented keys” present in the merged config:
  - If a supported-but-unimplemented top-level key exists (e.g. `permission`), record it once.
  - If an ignored key exists in OpenCode configs (e.g. `keybinds`), do not warn (silently ignore).

Where to surface:

- Log at startup (once), and optionally show in a UI “Config” screen later.

## Integration Points in Current Codebase

Current state observations:

- `src/config.rs` currently manages `api_keys.json` and is not a general config loader.
- Theme is currently loaded from `src/theme.json` with a fallback to `src/themes/ayu.json` (`src/app.rs`).
- Model selection is persisted in SQLite (`src/persistence/prefs.rs`) and in message history; config should only set the default.
- `src/sound.rs` already contains event-based sound resolution and OS-specific command dispatch (good pattern to mirror for desktop notifications).
- `src/app.rs` already emits `SoundEvent::Complete` at streaming end (and `SoundEvent::Error` on failures); these are integration points for `sounds.<event>.notify`.

Planned integration:

- Add a new module (recommended: `src/config/mod.rs` or rename `src/config.rs` -> `src/config/api_keys.rs` and create `src/config/mod.rs`).
- Add `ConfigLoader` that returns:
  - `MergedConfig` (typed subset we act upon: theme/model/sounds)
  - `RawMergedValue` (full merged JSON value, for future keys)
  - `Diagnostics` (warnings + unimplemented)
- Add a desktop notification module (e.g. `src/notify.rs`) with OS-specific backends and a no-op fallback.

## Phase 1 Implementation Checklist

Phase 1 should implement behavior for `theme`, `sounds`, `model` only.

- Load config sources (4-tier) + deep merge.
- Fail-fast duplicate checks per layer: error if more than one candidate exists for any single layer.
- Variable substitution.
- Apply `theme`:
  - Decide how to map a theme string to an actual theme file.
  - Recommended: treat it as an ID that maps to a built-in JSON file in `src/themes/*.json`.
  - If theme is invalid/missing, keep current fallback behavior.
- Apply `model`:
  - Use config `model` only as the default when there is no active model in prefs yet.
  - Do not overwrite persisted “active model” selection.
- Apply `sounds`:
  - Introduce an audio playback layer and trigger events from existing UI flows.
  - Add per-event `notify` parsing (`sounds.<event>.notify`, default `false`) and boolean shorthand support for sound event toggles.
  - Add native desktop notifications for completion events on macOS/Linux/Windows.
  - Keep notification dispatch best-effort/non-blocking with warning diagnostics on backend failures.
  - If we can’t add playback immediately, still wire config parsing + diagnostics so the shape is stable.

## Phase 2+ (Future)

- Implement additional OpenCode-compatible keys in priority order:
  - `permission` (ties into tool execution)
  - `tools` enable/disable
  - `instructions` loader
  - `agent` + `.opencode/agents/*.md`
  - `skills` + `.opencode/skills/**`
  - `command`, `watcher`, `formatter`, `mcp`, `provider`

## References

- OpenCode config docs: https://opencode.ai/docs/config/
- OpenCode config schema: https://opencode.ai/config.json
- OpenCode agents: https://opencode.ai/docs/agents/
- OpenCode skills: https://opencode.ai/docs/skills/
