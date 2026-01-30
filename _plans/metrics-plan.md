# Metrics Plan (TTFT + TPS + Full Latency)

Goal: store timing primitives (T0/T1/Tn) + output token count (N) so we can compute TTFT, TPS (decode-only), and full latency reliably and display them in message metadata.

This repo currently stores only `duration_ms` and `token_count` on messages, where `duration_ms` effectively measures time since first token (Tn - T1). That yields a decode-phase TPS already, but we do not persist TTFT or the underlying primitives.

## Definitions

Primitives:

- T0: timestamp when the API request is sent
- T1: timestamp when the first output token arrives
- Tn: timestamp when the last output token arrives
- N: total number of output tokens generated

Derived metrics:

- TTFT: T1 - T0
- Decode duration: Tn - T1
- Full latency (total time): Tn - T0
- TPS (decode-only): N / (Tn - T1)

## Data Model

Persist primitives (do not persist derived metrics):

- `t0_ms` (INTEGER, epoch milliseconds)
- `t1_ms` (INTEGER, epoch milliseconds)
- `tn_ms` (INTEGER, epoch milliseconds)
- `output_tokens` (INTEGER)

Notes:

- Persist as epoch milliseconds so values survive across process restarts.
- Derive TTFT/TPS/latency when rendering UI or exporting data.

## Schema Changes

Greenfield preference: update the baseline schema so a new DB includes these columns.

- Add columns to `messages` table (assistant rows will use them; others may be NULL/0):
  - `t0_ms INTEGER`
  - `t1_ms INTEGER`
  - `tn_ms INTEGER`
  - `output_tokens INTEGER`

Migration for existing installs (keeps codebase consistent even if we treat DB as new):

- Add a new migration version that `ALTER TABLE messages` to add the above columns.

Optional cleanup:

- The `responses` table appears unused by current reads/writes; consider removing it from the baseline schema and/or formalizing its purpose.

## Capture Points (Streaming Lifecycle)

Capture these during a single model generation:

- Record T0 when we start the provider request (right before creating/awaiting the stream).
- Record T1 when the first text/reasoning chunk is received (only once).
- Record Tn when the end-of-stream signal is received.
- Track N as output token count:
  - Prefer provider-reported usage if available at end-of-stream.
  - Otherwise fallback to the existing heuristic (chars/4) to keep metrics available.

Implementation notes:

- Use `Instant` for high-resolution runtime timing, but persist epoch millis for storage.
- Guard against edge cases:
  - If T1 missing, TTFT/TPS should be absent.
  - If (Tn - T1) <= 0, TPS should be absent or 0.

## Wiring Through the App

- Extend `crate::session::types::Message` to carry the primitives (or a `GenerationMetrics` struct):
  - `t0_ms: Option<u64>`
  - `t1_ms: Option<u64>`
  - `tn_ms: Option<u64>`
  - `output_tokens: Option<usize>`

- Extend persistence structs + SQL:
  - `src/persistence/history.rs` insert/select must include these fields.
  - `src/persistence/conversions.rs` must map session message <-> persistence message.

## UI / Metadata Display

Update chat metadata to show explicit, spec-aligned metrics (for completed assistant messages):

- TTFT: `(t1_ms - t0_ms)`
- TPS: `output_tokens / ((tn_ms - t1_ms) / 1000.0)`
- Total: `(tn_ms - t0_ms)`

Keep the existing `duration_ms` display only for backwards compatibility if needed; otherwise prefer displaying total + TTFT + TPS.

## Tests

- Unit tests for metric calculations (normal case + missing fields + decode duration zero).
- UI formatting test for metadata (ensures stable ordering/precision).
