# Agent Context

This file contains important information about the codebase that the AI agent should be aware of.

## Common Project Commands

Before adding/changing scripts, make sure to check `justfile` for existing recipes (this repo uses `just` and typically runs scripts via `bun`).

## File Locations

### Configuration Docs

- **Location**: `_docs/config.mdx`
- **Purpose**: Source-of-truth, human/AI-readable contract for `crabcode.json(c)`

### SQLite Database

- **Location**:
  - macOS: `~/Library/Application Support/crabcode/data.db`
  - Linux: `~/.local/share/crabcode/data.db`
- **Implementation**: `src/persistence/prefs.rs`
- **Contents**: Stores user preferences including:
  - Model preferences (recent models, favorites, active model)
  - Preference keys and values with timestamps

### Authentication Credentials

- **Location**:
  - macOS: `~/Library/Application Support/crabcode/auth.json`
  - Linux: `~/.local/share/crabcode/auth.json`
- **Implementation**: `src/persistence/auth.rs`
- **Format**: JSON with provider ID as keys
- **Contents**: API keys and OAuth tokens for LLM providers
- **Example format**:
  ```json
  {
    "provider-id": {
      "type": "api",
      "key": "api-key-here"
    }
  }
  ```

### Models.dev API Cache

- **Location**:
  - macOS: `~/Library/Caches/crabcode/models_dev_cache.json`
  - Linux: `~/.cache/crabcode/models_dev_cache.json`
  - Test mode: `/tmp/crabcode_test_cache/models_dev_cache.json`
- **TTL**: 24 hours (`CACHE_TTL_SECONDS = 86400`)
- **Source**: `https://models.dev/api.json`
- **Implementation**: `src/model/discovery.rs`

The cache stores provider and model information from models.dev and expires after 24 hours. The cached data includes:

- Provider information (id, name, API endpoints, documentation, env vars, npm packages)
- Model information per provider (id, name, family, capabilities, modalities, cost, limits)

### Writing official documentation

- Important: always refer to this when asked to write inside \_docs.
- Traverse this llms.txt as often as you can if you write docs: https://gittydocs.carlo.tl/llms.txt
- When writing titles + first text in the body, never use the same 'title' (in mdx data) and '# <title>` (in the body).
