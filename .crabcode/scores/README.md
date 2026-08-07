# Harness judge scores

One command:

```text
/judge-harness
/judge-harness caching
/judge-harness tools
```

It produces:

1. **Full category scorecard** (winner per category)  
2. **Caching deep-dive** (sub-axes + winner per sub-axis)

## Outputs

- `harness-latest.md` — last run (overwritten)
- `harness-YYYY-MM-DD.md` — dated snapshot

No seed baselines. Score from current code only; different models may disagree.
