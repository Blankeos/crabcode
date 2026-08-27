#!/usr/bin/env bash
# Fail if aisdk imports host product modules.
# Host → aisdk is fine; aisdk → host is not.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AISDK="$ROOT/src/aisdk"

# Product modules that must never be imported from aisdk.
# Keep this list focused on host/product crates; aisdk may use crate::{chunk,error,log,...}
# via the host re-export shim until extraction.
FORBIDDEN_PATTERN='crate::(tools|agent|config|session|ui|llm|prefs|persistence|tui|commands|bridge|remote)::'

hits="$(rg -n --glob '*.rs' "$FORBIDDEN_PATTERN" "$AISDK" || true)"

if [[ -n "$hits" ]]; then
  echo "aisdk boundary violation: host product imports found under src/aisdk/" >&2
  echo "$hits" >&2
  echo >&2
  echo "aisdk may only know provider capabilities (e.g. hosted_web_search)." >&2
  echo "Host policy (websearch.native, tool registry, sessions) belongs outside aisdk." >&2
  exit 1
fi

# Soft check: avoid product-policy identifiers leaking into providers.
# Ignore comment/doc lines so explanatory docs can mention the host flag name.
policy_hits="$(
  rg -n --glob '*.rs' 'prefer_provider_websearch|preferProvider' "$AISDK"     | rg -v '^[^:]+:[0-9]+:\s*(///|//!|//|\*)'     || true
)"
if [[ -n "$policy_hits" ]]; then
  echo "aisdk naming smell: host policy names found under src/aisdk/" >&2
  echo "$policy_hits" >&2
  echo "Use capability/tool factories + HostedSearchSelection instead of host policy knobs." >&2
  exit 1
fi

echo "aisdk boundary ok"
