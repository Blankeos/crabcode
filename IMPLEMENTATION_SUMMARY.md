# Implementation Plan Summary

## Decision Made

**Single Implementation Plan:** `AISDK_INTEGRATION_PLAN.md`

## Rationale

1. **LLM_INTEGRATION_PLAN.md is obsolete**
   - Based on using unfinished custom streaming code
   - User wants to use AISDK instead

2. **AISDK_INTEGRATION_PLAN.md is the correct plan**
   - Updated to use AISDK's recommended approach
   - Uses `OpenAI::<DynamicModel>` with custom base URLs
   - Simplified based on AISDK documentation

3. **New information incorporated**
    - Confirmed approach: OpenAI provider with custom base URL
    - Simplified LLMClient (no separate LLMProvider struct)
    - ProviderConfig just maps provider_id → base_url
    - Lazy initialization on every message submission

4. **Use Existing Discovery Infrastructure**
    - Leverage `src/model/discovery.rs` (already exists!)
    - Models cached at `~/Library/Caches/crabcode/models_dev_cache.json` (macOS)
    - 24-hour TTL with auto-refresh
    - ModelRegistry wraps Discovery for provider/model lookups
    - No need to fetch models.dev manually - it's already there!

## Models.dev Integration Details

## What Changed in AISDK_INTEGRATION_PLAN.md

### 1. Simplified Architecture
- Removed separate `LLMProvider` struct
- Added `ProviderConfig` (just maps IDs to URLs)
- `LLMClient` builds provider directly from config

### 2. Updated Code Examples
All code examples now follow AISDK documentation:

```rust
// OLD: Separate LLMProvider struct
let provider = LLMProvider::new(&id, &model, key);
let client = LLMClient::new(provider);

// NEW: Direct configuration
let provider_config = ProviderConfig::new(&provider_id);
let client = LLMClient::new(
    provider_config.base_url,
    api_key,
    model_name,
    provider_name,
);
```

### 3. Updated Based on AISDK Docs
```rust
// Correct pattern from AISDK documentation
let openai = OpenAI::<DynamicModel>::builder()
    .base_url(&self.base_url)      // For custom endpoints
    .api_key(&self.api_key)
    .model_name(&self.model_name)
    .build()?;
```

### 4. Key Requirements Confirmed
✅ Lazy initialization (rebuild on every message)
✅ No system prompts for now
✅ Clear failed messages, show toast only
✅ Send full conversation history
✅ Use `self.model` (don't hardcode glm-4.7)
✅ z.ai uses `/api/coding/paas/v4` endpoint

## Provider Mappings

| Provider ID | Display Name | Base URL | Notes |
|-------------|---------------|------------|-------|
| `nano-gpt` | Nano-GPT | `https://nano-gpt.com/api/v1` | OpenAI-compatible |
| `zai` | Z.AI | `https://api.z.ai/api/coding/paas/v4` | GLM Coding Plan |
| `zai-coding-plan` | Z.AI | `https://api.z.ai/api/coding/paas/v4` | Alternative ID |

## Next Steps

1. **Follow AISDK_INTEGRATION_PLAN.md phases:**
   - Phase 1: Setup & Foundation (add AISDK, create modules)
   - Phase 2: Core Integration (connect to app state)
   - Phase 3: Streaming Implementation (connect stream to UI)
   - Phase 4: UI Updates (streaming indicator, spacing fix)
   - Phase 5: Testing (nano-gpt & z.ai)

2. **Reference only AISDK_INTEGRATION_PLAN.md**
   - Ignore LLM_INTEGRATION_PLAN.md (obsolete)
   - All code examples are in AISDK_INTEGRATION_PLAN.md

3. **Test priority order:**
   - First: nano-gpt (simple OpenAI-compatible)
   - Second: z.ai GLM-4.7 (coding endpoint)

## Success Criteria Validation

All 4 criteria from requirements are covered:

✅ **Criteria 1:** Chat → Session → Chat page → First message
   - Handled in `process_input()` for Home focus

✅ **Criteria 2:** "Streaming..." label below input (left)
   - Added `is_streaming` state
   - Modified `render_chat()` to show indicator

✅ **Criteria 3:** Stream messages as they arrive
   - AISDK's `stream_text()` provides real-time chunks
   - Callback appends via `append_to_last_assistant()`

✅ **Criteria 4:** "Streaming..." disappears when done
   - `LanguageModelStreamChunkType::End` triggers cleanup
   - Message persisted to database

## Timeline Estimate

- **Phase 1 (Setup):** 1 hour (use existing Discovery)
- **Phase 2 (Core Integration):** 1-2 hours (simplified - no custom provider logic)
- **Phase 3 (Streaming):** 2 hours (AISDK handles streaming)
- **Phase 4 (UI Updates):** 1 hour (add indicator, fix spacing)
- **Phase 5 (Testing):** 2-3 hours (nano-gpt & z.ai)

**Total Estimated Time:** 7-9 hours (reduced due to existing Discovery)

## Files to Create

- `src/llm/mod.rs` - Module exports
- `src/llm/registry.rs` - ModelRegistry (wraps existing Discovery)
- `src/llm/client.rs` - AISDK wrapper
- (NOTE: No need for `src/llm/provider.rs` - using existing `Discovery`!)

## Files to Modify

- `Cargo.toml` - Add aisdk dependency
- `src/app.rs` - Add model_registry, is_streaming, implement LLM calls
- `src/views/chat.rs` - Add streaming indicator, fix spacing
- `src/mod.rs` - Add llm module
- `src/llm/registry.rs` - Import existing Discovery from `src/model/discovery.rs`

## Conclusion

**Single, unified plan ready for implementation.**

Start with Phase 1 when ready. All requirements are documented, all decisions are made, all code examples are correct based on AISDK documentation.

✅ **Existing Discovery infrastructure leveraged** (no reimplementation!)
✅ **Data-driven, scalable architecture** (uses models.dev cache)
✅ **Clear error handling** (no silent fallbacks)
✅ **Future-proof** (new providers via models.dev, not hardcoding)

Ready to implement! 🚀
