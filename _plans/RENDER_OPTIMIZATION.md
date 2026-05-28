# Render Optimization Plan

## Goal

Keep typing, scrolling, hover, and streaming responsive during long chat sessions.

## Done

- Cache the active-tool scan by chat render revision so repeated frames do not repeatedly parse tool JSON.
- Move `message_line_positions` mirroring into chat cache rebuilds instead of cloning positions on every render.

## Next

1. Add lightweight performance tracing behind `CRABCODE_PERF_TRACE=1`.
   - Track total frame render time.
   - Track chat render time.
   - Track chat cache rebuild time.
   - Track markdown render time.
   - Include message count, rendered line count, visible line range, cache hit/miss, and active overlay.

2. Replace linear line-to-message hit testing with indexed lookups.
   - Build message line ranges when chat lines are cached.
   - Use binary search for `message_index_at_content_line`.
   - Reuse the same ranges for hover, image/link lookup, message actions, timeline highlight, and selection action placement.

3. Add per-message render caching.
   - Key cached message lines by message revision, width, theme hash, hover state where needed, and streaming state.
   - Re-render only changed messages when a stream chunk arrives.
   - Preserve logical grouping for task/exploration tool rows.

4. Virtualize transcript rendering.
   - Maintain per-message rendered heights and prefix sums.
   - Resolve visible messages from scroll offset and viewport height.
   - Render only visible messages plus a small buffer.
   - Keep full-copy selection behavior using cached rendered lines or on-demand range rendering.

5. Coalesce streaming updates.
   - Drain text chunks into one append per event-loop tick.
   - Invalidate render once per tick instead of once per chunk.
   - Make `SimpleStreamingRenderer` append incrementally instead of reset-and-copying the full streaming message.

6. Decouple active tool animation from full transcript cache invalidation.
   - Store marker state separately from message lines, or render marker spans in a lightweight visible-line pass.
   - Avoid full cache rebuilds every animation phase.

7. Cache input wrapping.
   - Add an input text revision and width-keyed `visual_lines` cache.
   - Recompute wrapping only when input text, cursor-affecting layout, or width changes.

8. Reduce autocomplete cloning.
   - Avoid cloning the complete file autocomplete entry cache for each suggestion query.
   - Score against borrowed entries while holding the cache lock, or store the cache behind `Arc<[FileEntry]>`.

9. Add regression benchmarks.
   - Synthetic long transcript render benchmark.
   - Streaming append benchmark with a large prior transcript.
   - Mouse move and scroll hit-test benchmark.
   - Long prompt input typing benchmark.
