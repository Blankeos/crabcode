# Anthropic Prompt Caching (via AI Gateway + Direct)

## Status: Implemented (2026-08-06)

## Root cause of missing cache hits

Vercel AI Gateway routes through `OpenAICompatible` (`@ai-sdk/gateway`), **not**
`Anthropic` (`@ai-sdk/anthropic`). The earlier last-system-only `cache_control`
in `anthropic.rs` never ran for Gateway traffic.

Gateway Chat Completions needs:

```json
{
  "providerOptions": {
    "gateway": { "caching": "auto" }
  }
}
```

Docs: https://vercel.com/docs/ai-gateway/models-and-providers/automatic-caching

## What was implemented

### 1. AI Gateway path (your actual path)

- `OpenAICompatible` builder flag `gateway_caching_auto`
- When set, request body includes `providerOptions.gateway.caching = "auto"`
- Enabled automatically when `is_vercel_ai_gateway` (provider `vercel` or npm
  `@ai-sdk/gateway`)
- Propagated through `ProviderRequestConfig` → `LlmSessionConfig` → main agent
  + subagents

Files:
- `src/aisdk/providers/compatible.rs`
- `src/llm/client.rs`
- `src/agent/config.rs`
- `src/agent/subagent.rs`

### 2. Direct Anthropic path (native API)

`apply_anthropic_prompt_caching` marks up to 3 breakpoints (cap 4):

1. **Last tool** — stable schemas, highest value in tool loops
2. **Last system** text block
3. **Latest user** last content block (string → text block wrap; works for
   tool_result-only user messages after `anthropic_messages` regrouping)

Files:
- `src/aisdk/providers/anthropic.rs`

## Verification

After a multi-step tool session on Gateway Anthropic models, check AI SDK /
Gateway logs for cache read growth (green line). Direct Anthropic usage fields:

- `cache_read_input_tokens`
- `cache_creation_input_tokens`

Note: Anthropic `input_tokens` is non-cached only; total input =
non_cached + cache_read + cache_write.

## Follow-ups (not done)

- [ ] Parse/log Anthropic stream usage (`message_start` / `message_delta`)
- [ ] Surface cache read/write in session token stats UI
- [ ] Confirm Gateway Chat Completions accepts `providerOptions` at top level
      (AI SDK docs show it; if Gateway rejects, fall back to provider-specific
      headers / Anthropic-format route)

## Why not only `prompt_cache_key`?

`prompt_cache_key` is OpenAI/xAI sticky routing. Anthropic needs explicit
`cache_control` breakpoints (or Gateway `caching: auto` which inserts them).
