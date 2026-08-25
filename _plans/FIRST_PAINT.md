# First Paint Speed (vs Codex)

Target: Codex ~53ms first frame. Crabcode today: ~103–123ms (`PERF.md`).

## Ranked wins

| Rank | Win | Est. | Status |
|------|-----|------|--------|
| 1 | Skip blocking `supports_keyboard_enhancement()` (CSI probe + timeout). Always push enhancement flags like Codex. Opt out: `CRABCODE_DISABLE_KEYBOARD_ENHANCEMENT`. | 10–50ms | done |
| 2 | Defer `SessionManager` SQLite history until after first draw. Sync load only for `--session`. | 5–30ms | done |
| 3 | Split `App::new`: minimal shell for frame 0; hydrate config/prefs/themes/skills after. Codex `StartupDraft` pattern. | 20–40ms | done |
| 4 | Theme resolve before first paint (peek config + prefs + discover); keep skills/autocomplete deferred. | flash fix | done |
| 5 | Move prefs SQLite + model preference reads off the critical path when CLI model override is set. | 2–10ms | later |
| 6 | Optional: draw before full `App::new` (terminal init → empty frame → hydrate). | variable | later |

## Codex reference

- Probe skip: `.devrefs/references/openai/codex/codex-rs/tui/src/tui.rs` (`enable_keyboard_mode`)
- Policy: `tui/src/tui/keyboard_modes.rs` (`always_enable` unless disabled)
- Startup draft / deferred hydrate: TUI app entry around `StartupDraft`

## Our critical path today (`main` → first `terminal.draw`)

1. `App::new_with_model_override` (config, prefs DB, themes, skills, autocomplete, …) — history deferred
2. `enable_raw_mode`
3. alt screen + mouse + paste (+ keyboard flags, no CSI probe)
4. `Terminal::new`
5. `run_event_loop` → first `terminal.draw` → then `ensure_session_history`

## Measurement

Re-run `scripts/bench-perf.py` / `PERF.md` workflow after each win. Prefer `CRABCODE_STARTUP_DIAG=1` spans if we add timed checkpoints.
