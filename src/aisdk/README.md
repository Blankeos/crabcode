# aisdk (vendored)

In-tree AI SDK used by the host binary (`mod aisdk` in `src/main.rs`).

## Status

- **Not** a published crates.io package.
- Dogfooded by the host app while the API stabilizes.
- Treat as a **possible future extractable crate** (e.g. public `aisdk` / `aisdk-rs`) once mature — not before.

## Rules

1. **Single source of truth:** all SDK code lives under `src/aisdk/`.
2. Do not reintroduce a parallel `aisdk/` workspace package until we intentionally extract and publish.
3. Host app imports via `crate::aisdk::...` (binary module path), not an external crate dep.
4. **No hard coupling to the host product.** Code here must not depend on product-specific concepts (TUI, sessions, tools registry, prefs DB, agent loop, product config, host `crate::` app modules, etc.). Keep this layer a generic multi-provider AI SDK — same spirit as Vercel AI SDK for Rust: providers, messages, streams, tools, retries. Product-specific behavior belongs outside this tree (call sites in the app).
5. **Logging:** use `crate::log::log(...)` (host-injected via `aisdk::log::set_logger`). Never call host `emit_log!` from this tree.
6. **Debug SSE dumps:** feature-gated behind `aisdk-sse-debug` (optional path via `AISDK_SSE_DEBUG_LOG`).

## Extract later (when ready)

1. Move this tree into a real workspace crate with a normal `src/lib.rs` layout (this directory becomes the crate root).
2. Point the host at it as a path dependency.
3. Publish the crate first, then depend on the registry version from the host.
4. Until then, keep everything vendored here so publishing the host needs no sibling crate.

## Extract readiness

**Score: ~9/10** (**10/10** needs external users + API freeze).

Done for packaging/host hooks:

- Neutral logging (`log` module + host `set_logger`)
- No `crate::aisdk::...` inside the tree (`mod.rs` / re-exports use `super::`)
- Absolute `crate::{chunk,error,...}` paths are crate-root-shaped (host re-exports them today)
- Product-leaky debug path renamed/feature-gated
- Product-flavored comments/tests scrubbed

Keep app glue outside this tree (`src/tools/aisdk_bridge.rs`, `src/llm/*`).
