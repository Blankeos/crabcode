# aisdk (vendored)

In-tree AI SDK used by the crabcode binary (`mod aisdk` in `src/main.rs`).

## Status

- **Not** a published crates.io package.
- Dogfooded only by crabcode while the API stabilizes.
- Treat as a **possible future extractable crate** (e.g. public `aisdk` / `aisdk-rs`) once mature — not before.

## Rules

1. **Single source of truth:** all SDK code lives under `src/aisdk/`.
2. Do not reintroduce a parallel `aisdk/` workspace package until we intentionally extract and publish.
3. App code imports via `crate::aisdk::...` (binary module path), not an external crate dep.
4. **No hard coupling to crabcode.** Code here must not depend on crabcode-specific concepts (TUI, sessions, tools registry, prefs DB, agent loop, product config, `crate::` app modules, etc.). Keep this layer a generic multi-provider AI SDK — same spirit as Vercel AI SDK for Rust: providers, messages, streams, tools, retries. Crabcode-specific behavior belongs outside this tree (call sites in the app).

## Extract later (when ready)

1. Move this tree into a real workspace crate with a normal `src/lib.rs` layout.
2. Point crabcode at it as a path dependency.
3. Publish the crate first, then depend on the registry version from crabcode.
4. Until then, keep everything vendored here so `cargo publish -p crabcode` needs no sibling crate.

## Extract readiness

**Score today: ~7/10** (→ **9/10** after packaging/host-hook todos in `_plans/__TODOS.md`; **10/10** needs external users + API freeze).

Domain shape is good; remaining work is packaging hooks, not product coupling. Keep app glue outside this tree (`src/tools/aisdk_bridge.rs`, `src/llm/*`).
