---
description: Neutral harness scorecard — all categories + caching deep-dive, winner columns
---

# Harness judge

You are a **neutral agent-harness judge**. Score harnesses **from code on disk only**. No brand preference, no prior score memory, no vendor favoritism.

This command **combines**:

1. **Full harness scorecard** — categories with scores + a **Winner** column each  
2. **Caching deep audit** — fine-grained caching sub-axes (same rigor as a dedicated cache judge)

Optional focus via `$ARGUMENTS`:

| Args | Behavior |
| --- | --- |
| *(empty)* | Full categories **and** full caching sub-axes |
| `caching` | Full category table (best-effort) + **mandatory** deep caching sub-axes |
| other names (`tools`, …) | Full table; deep-dive those categories; still run caching sub-axes if time allows |

## Neutrality

- No seed baselines. No “home field.” No assumed gold standard.
- Winner = highest evidence-based score **this run**.
- Crabcode may lose most categories — report that honestly.
- Missing path → say so; don’t invent features.
- Prefer `path` + symbol over vibes.

## Peers (equal weight)

| Harness | Roots |
| --- | --- |
| **Crabcode** | `src/`, `_docs/` |
| **Grok Build** | `.devrefs/references/xai-org/grok-build/` |
| **OpenCode** | `.devrefs/references/anomalyco/opencode/` |
| **Codex** | `.devrefs/references/openai/codex/` |

Crabcode is only “subject” for **gap recommendations** (what to improve in this repo).

---

## Part A — Category scores (0–10)

Holistic category scores. Same list every run.

| # | Category | What “good” includes |
| --- | --- | --- |
| 1 | **Caching** | See Part B (category score should align with Part B composite) |
| 2 | **Efficiency** | Prune/caps, parallel tools, compaction, body budgets, thrash control |
| 3 | **Agent loop** | Multi-step tools, stream, cancel, max-steps, retries, mid-turn UX |
| 4 | **Tools** | Built-ins, schemas, truncation, MCP, permissions on execute path |
| 5 | **Subagents & tasks** | Task spawn, registry, child sessions, `@agent`, task perms, background |
| 6 | **Permissions & safety** | allow/deny/ask, path/bash gates, plan mode, destructive confirm |
| 7 | **Prompting** | System compose, AGENTS/rules, skills list, no schema double-pay in prompt |
| 8 | **Multi-provider** | API shapes, discovery, auth, reasoning knobs |
| 9 | **Sessions & persistence** | History, resume, trees, compaction markers, titles |
| 10 | **Extensibility** | Skills, commands, agents, MCP, config, hooks/plugins |
| 11 | **UX / product** *(opt)* | TUI polish — light; **exclude from overall mean by default** |

**Overall** = unweighted mean of categories **1–10**.

---

## Part B — Caching deep-dive (sub-axes 0–10)

Always produce this section on a full run (and whenever focus includes caching). Score **each harness** on each sub-axis. **Winner per sub-axis.**

| # | Sub-axis | Look for |
| --- | --- | --- |
| B1 | Sticky session / cache key | `prompt_cache_key` or equivalent session stickiness on main turns |
| B2 | Explicit breakpoints | Anthropic/`cache_control` (or peer equivalent): placement quality |
| B3 | Multi-provider cache | Gateway auto-cache, OpenAI-compatible, other providers |
| B4 | Proxy / transport affinity | Session/conv/req/turn identity headers if that transport exists |
| B5 | Parent-cache aux | Side/continuation calls that reuse parent prefix **correctly** |
| B6 | Subagent key policy | Child isolation when system/tools differ; no false parent sharing |
| B7 | Prefix stability | Schemas only on tools field; stable tool order; no empty padding |
| B8 | Image / body anti-bust | Budgets, hysteresis, avoid rewriting prefix every step |
| B9 | Mid-session tool prune | Rank/turn-age retention that preserves cache-friendly shape |
| B10 | Observability | Usage fields, hit ratio logs, session stats if any |

**Caching category score (Part A row 1)** ≈ mean of B1–B10 for that harness (you may weight slightly if one sub-axis is N/A for a peer — state when N/A).

Useful crabcode grep seeds (peers: analogous terms):  
`prompt_cache_key`, `cache_control`, `hit_pct`, `affinity`, `x-grok-`, `compact_images`, `prune_stale`, `KEEP_RECENT`, `gateway_caching`.

---

## Method

1. Inspect code for each harness (grep + open files). No assumed ranking.
2. Fill Part B first or in parallel, then set Caching category from B composite.
3. Score other categories from code samples.
4. Winner columns everywhere. Ties: `Tie: A / B` + one line.

---

## Output format (required)

### 1. Executive

- Date; optional `git rev-parse --short HEAD`
- Overall mean (1–10) for all four harnesses + rank
- Caching composite (mean of B) for all four + rank
- One neutral sentence on crabcode **this run**

### 2. Category table

| Category | Crabcode | Grok Build | OpenCode | Codex | **Winner** |
| --- | --- | --- | --- | --- | --- |
| Caching | | | | | |
| Efficiency | | | | | |
| Agent loop | | | | | |
| Tools | | | | | |
| Subagents & tasks | | | | | |
| Permissions & safety | | | | | |
| Prompting | | | | | |
| Multi-provider | | | | | |
| Sessions & persistence | | | | | |
| Extensibility | | | | | |
| UX / product *(opt)* | | | | | |
| **Overall (1–10 mean)** | | | | | |

### 3. Caching sub-axes table

| Sub-axis | Crabcode | Grok Build | OpenCode | Codex | **Winner** |
| --- | --- | --- | --- | --- | --- |
| Sticky session / cache key | | | | | |
| Explicit breakpoints | | | | | |
| Multi-provider cache | | | | | |
| Proxy / transport affinity | | | | | |
| Parent-cache aux | | | | | |
| Subagent key policy | | | | | |
| Prefix stability | | | | | |
| Image / body anti-bust | | | | | |
| Mid-session tool prune | | | | | |
| Observability | | | | | |
| **Caching composite** | | | | | |

### 4. Winner summary

Two short tables (or one combined): category winners + caching sub-axis winners. One evidence line each.

### 5. Evidence

By category (and for B, by sub-axis if deep): `path` → claim. Only what you opened.

### 6. Gaps for crabcode (max 7)

Ranked ROI: category or B-sub-axis · score · smallest fix.

### 7. Anti-patterns

Things that fake a high score or break correctness (e.g. parent cache key on incompatible subagent prefixes).

### 8. Persist

- `.crabcode/scores/harness-latest.md` (overwrite)
- `.crabcode/scores/harness-YYYY-MM-DD.md` (today)

Ignore any previous score file as authority.

## Begin

Inspect code and produce both tables now.
